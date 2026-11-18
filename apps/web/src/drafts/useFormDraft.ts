import {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import type {ObjectsClient} from '../objects/api';
import {DraftAutosave, type DraftState} from './autosave';
import {planRestore, type RestorePlan} from './model';
import {useDrafts} from './runtime';
import {serialiseKey, type DraftDocument, type DraftKey, type DraftNotices, type DraftStore, type StoredDraft} from './types';

/** Where the form is with its draft. */
export type FormDraftPhase =
  /** No drafts on this host, or none for this kind of form. */
  | {kind: 'off'}
  /** The host offers drafts but the key could not be had: "Drafts are unavailable". */
  | {kind: 'unavailable'}
  /** Looking for a draft of this form. */
  | {kind: 'checking'}
  /** A draft exists; until the member chooses, nothing is restored or overwritten. */
  | {kind: 'prompt'; draft: StoredDraft; notices: DraftNotices}
  | {kind: 'restoring'}
  /** The draft cannot be restored now; it is kept, and may only be discarded or left. It keeps its identity so Discard deletes it. */
  | {kind: 'blocked'; reason: 'unavailable' | 'conflicts' | 'failed'; draft: StoredDraft}
  /** Autosaving. */
  | {kind: 'active'};

export type Restored = Extract<RestorePlan, {status: 'ready'}>;

/**
 * One record form's draft (ADR 0036): finds an existing draft, offers it,
 * restores it through the re-checks of §5, and autosaves the member's
 * changes with flushes on `visibilitychange`, `pagehide` and close. A
 * `beforeunload` guard stands only while a write is pending.
 */
export function useFormDraft({client, draftKey, resume, build, apply, only}: {
  client: ObjectsClient;
  /** Which form this is; `null` for a form that keeps no drafts. */
  draftKey: DraftKey | null;
  /** A draft the member already chose to continue, from Drafts or a New chooser. */
  resume?: StoredDraft;
  build: () => DraftDocument | null;
  /** Puts restored values back into the form. */
  apply: (restored: Restored) => void;
  only?: readonly string[];
}) {
  const runtime = useDrafts();
  const [phase, setPhase] = useState<FormDraftPhase>(() => !runtime || !draftKey ? {kind: 'off'} : runtime.status === 'unavailable' ? {kind: 'unavailable'} : {kind: 'checking'});
  const [state, setState] = useState<DraftState>({kind: 'idle'});
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<{tone: 'info' | 'warning'; text: string} | null>(null);
  const [dropped, setDropped] = useState<string[]>([]);
  const controller = useRef<DraftAutosave | null>(null);
  const buildRef = useRef(build);
  buildRef.current = build;
  const applyRef = useRef(apply);
  applyRef.current = apply;
  const store = runtime?.status === 'ready' ? runtime.store : null;
  const keyText = useMemo(() => draftKey ? serialiseKey(draftKey) : '', [draftKey]);
  const newId = useCallback(() => draftKey?.kind === 'create' ? draftKey.draft : crypto.randomUUID(), [draftKey]);

  const startAutosave = useCallback((id: string, revision: number) => {
    if (!store) return;
    controller.current = new DraftAutosave(store, id, revision, () => buildRef.current(), setState, setPending);
    setPhase({kind: 'active'});
  }, [store]);

  const restore = useCallback(async (draft: StoredDraft) => {
    if (!store) return;
    setPhase({kind: 'restoring'});
    setMessage(null);
    let plan: RestorePlan;
    try {
      plan = await planRestore(client, draft.document, {only});
    } catch {
      setPhase({kind: 'blocked', reason: 'failed', draft});
      setMessage({tone: 'warning', text: 'The draft could not be checked against the record. Try again in a moment.'});
      return;
    }
    if (plan.status === 'unavailable') {
      setPhase({kind: 'blocked', reason: 'unavailable', draft});
      setMessage({tone: 'warning', text: 'Not available right now. This draft is kept until it expires, and can only be discarded.'});
      return;
    }
    if (plan.status === 'conflicts') {
      setPhase({kind: 'blocked', reason: 'conflicts', draft});
      setMessage({tone: 'warning', text: 'This record changed since you started; reviewing changes is coming soon. Keep the draft for later or discard it.'});
      return;
    }
    if (plan.status === 'already_saved') {
      await store.delete(draft.meta.id).catch(() => undefined);
      setMessage({tone: 'info', text: 'Your changes are already saved.'});
      startAutosave(newId(), 0);
      return;
    }
    applyRef.current(plan);
    const lost = plan.dropped.unnamed > 0 ? ['Some changes were dropped because those fields no longer exist or are not available to you.'] : [];
    setDropped([...lost, ...plan.dropped.named.map(label => `${label} was not restored.`)]);
    startAutosave(draft.meta.id, draft.meta.revision);
    // Restored onto the current record: the next write records its version.
    controller.current?.changed();
  }, [client, only, store, startAutosave, newId]);

  // Find this form's draft once, when the form opens.
  useEffect(() => {
    if (runtime?.status === 'unavailable' && draftKey) { setPhase({kind: 'unavailable'}); return; }
    if (!store || !draftKey) return;
    let cancelled = false;
    (async () => {
      if (resume) { await restore(resume); return; }
      if (draftKey.kind === 'create') { startAutosave(draftKey.draft, 0); return; }
      try {
        const drafts = await store.list();
        if (cancelled) return;
        const found = drafts.find(draft => draft.meta.key === keyText);
        if (!found) { startAutosave(newId(), 0); return; }
        const notices = await store.takeNotices();
        if (!cancelled) setPhase({kind: 'prompt', draft: found, notices});
      } catch {
        if (!cancelled) setPhase({kind: 'unavailable'});
      }
    })();
    return () => { cancelled = true; };
    // The form's identity is fixed for its life; it looks once per store.
  }, [store, runtime?.status]);

  // Flush on hide, page hide and close; guard leaving only while a write is pending.
  useEffect(() => {
    if (phase.kind !== 'active') return;
    const flush = () => { void controller.current?.flush(); };
    const onVisibility = () => { if (document.visibilityState === 'hidden') flush(); };
    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('pagehide', flush);
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('pagehide', flush);
    };
  }, [phase.kind]);
  useEffect(() => () => { void controller.current?.flush(); }, []);
  useEffect(() => {
    if (!pending) return;
    const guard = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  }, [pending]);

  return {
    phase,
    state,
    pending,
    message,
    dropped,
    /** The draft is stored, so Cancel asks whether to keep it. */
    get saved() { return controller.current?.saved ?? false; },
    changed: () => { if (phase.kind === 'active') controller.current?.changed(); },
    flush: async () => { await controller.current?.flush(); },
    restore: async () => { if (phase.kind === 'prompt') await restore(phase.draft); },
    /** Discards the offered, blocked or current draft and starts afresh. */
    discard: async () => {
      if (!store) return;
      const outcome = await discardDraft(store, phase, controller.current);
      if (!outcome.ok) {
        setMessage({tone: 'warning', text: 'The draft could not be discarded. Try again in a moment.'});
        return;
      }
      controller.current = null;
      setMessage(null);
      setDropped([]);
      startAutosave(newId(), 0);
    },
    /** A successful save: the draft goes. */
    clear: async () => { await controller.current?.discard().catch(() => undefined); controller.current = null; },
    /** Leaves a blocked draft as it is and lets the form be used without drafting. */
    keepBlocked: () => { setPhase({kind: 'off'}); },
    /** Another tab wrote a newer revision: load it here and carry on from it, only if it restores. */
    takeOver: async () => {
      const current = controller.current;
      if (!store || !current) return;
      const outcome = await takeOverDraft(store, current, client, only, plan => applyRef.current(plan));
      setMessage(takeOverMessages[outcome]);
    },
    maxDrafts: runtime?.contribution.maxDrafts ?? 50,
    expiryDays: runtime?.contribution.expiryDays ?? 7,
  };
}

/**
 * Deletes the draft the form is showing: the one offered or blocked (whose
 * controller, if any, belongs to another id) and the one being autosaved.
 */
export async function discardDraft(store: DraftStore, phase: FormDraftPhase, current: DraftAutosave | null): Promise<{ok: boolean}> {
  try {
    if (phase.kind === 'prompt' || phase.kind === 'blocked') await store.delete(phase.draft.meta.id);
    if (current) await current.discard();
    return {ok: true};
  } catch {
    return {ok: false};
  }
}

export type TakeOverOutcome = 'taken' | 'gone' | 'already_saved' | 'unavailable' | 'conflicts' | 'failed';

const takeOverMessages: Record<TakeOverOutcome, {tone: 'info' | 'warning'; text: string} | null> = {
  taken: null,
  gone: {tone: 'info', text: 'The other tab saved or discarded this draft. Your changes here are kept as a new draft.'},
  already_saved: {tone: 'info', text: 'The other tab’s changes are already saved. Your changes here are kept as a new draft.'},
  unavailable: {tone: 'warning', text: 'The newer draft is not available right now. Nothing was changed here.'},
  conflicts: {tone: 'warning', text: 'The record changed since the other tab’s draft started; reviewing changes is coming soon. Nothing was changed here.'},
  failed: {tone: 'warning', text: 'The newer draft could not be loaded. Nothing was changed here; try again in a moment.'},
};

/**
 * Takes over a draft another tab wrote (ADR 0036 §7). The newer revision is
 * adopted only once its values were restored into this form; otherwise the
 * controller keeps its revision, so this tab can never overwrite the newer
 * draft with its older values.
 */
export async function takeOverDraft(store: DraftStore, current: DraftAutosave, client: ObjectsClient, only: readonly string[] | undefined, apply: (plan: Restored) => void): Promise<TakeOverOutcome> {
  let latest: StoredDraft | null;
  try {
    latest = await store.get(current.id);
  } catch {
    return 'failed';
  }
  if (!latest) {
    // Saved or discarded elsewhere: this tab's values become a new draft.
    current.reset(0);
    current.changed();
    return 'gone';
  }
  let plan: RestorePlan;
  try {
    plan = await planRestore(client, latest.document, {only});
  } catch {
    return 'failed';
  }
  switch (plan.status) {
    case 'ready':
      apply(plan);
      current.adopt(latest.meta.revision);
      return 'taken';
    case 'already_saved':
      try {
        await store.delete(latest.meta.id);
      } catch {
        return 'failed';
      }
      current.reset(0);
      current.changed();
      return 'already_saved';
    case 'unavailable':
      return 'unavailable';
    case 'conflicts':
      return 'conflicts';
  }
}

