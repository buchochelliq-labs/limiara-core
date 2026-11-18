import {useEffect, useState} from 'react';
import {useDrafts} from './runtime';
import {serialiseKey, type DraftKey, type DraftParent, type StoredDraft} from './types';

/** Reads a plain draft key back; `null` for anything malformed. */
export function parseKey(text: string): DraftKey | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!Array.isArray(value)) return null;
  const [kind, object, second, third, fourth, fifth, sixth] = value as unknown[];
  if (typeof object !== 'string' || typeof second !== 'string') return null;
  if (kind === 'edit') return {kind, object, record: second};
  if (kind === 'action' && typeof third === 'string') return {kind, object, record: second, action: third};
  if (kind === 'create') {
    const parent: DraftParent | undefined = typeof third === 'string' && typeof fourth === 'string' && typeof fifth === 'string' ? {object: third, id: fourth, field: fifth} : undefined;
    return {kind, object, draft: second, parent, action: typeof sixth === 'string' ? sixth : undefined};
  }
  return null;
}

/** The new-record drafts a New form for this object (and parent and action) may continue. */
export function createDraftsFor(drafts: readonly StoredDraft[], object: string, parent?: DraftParent, action?: string): StoredDraft[] {
  return drafts.filter(draft => {
    const key = parseKey(draft.meta.key);
    if (!key || key.kind !== 'create' || key.object !== object) return false;
    if ((key.action ?? null) !== (action ?? null)) return false;
    if (!parent) return !key.parent;
    return key.parent?.object === parent.object && key.parent.id === parent.id && key.parent.field === parent.field;
  });
}

/** The edit draft of a record, if any, for the record page's quiet notice. */
export function editDraftFor(drafts: readonly StoredDraft[], object: string, record: string): StoredDraft | undefined {
  const key = serialiseKey({kind: 'edit', object, record});
  return drafts.find(draft => draft.meta.key === key);
}

/** The member's drafts in this workspace, read when the component mounts and on each `refresh`; empty without drafts. */
export function useDraftList(refresh = 0): {drafts: StoredDraft[]; loaded: boolean} {
  const runtime = useDrafts();
  // A list belongs to the runtime and refresh it was read for, so a new runtime is never "loaded" with an old list.
  const [state, setState] = useState<{drafts: StoredDraft[]; source: unknown; refresh: number}>({drafts: [], source: null, refresh: -1});
  useEffect(() => {
    if (runtime?.status !== 'ready') return;
    let cancelled = false;
    runtime.store.list()
      .then(drafts => { if (!cancelled) setState({drafts, source: runtime, refresh}); })
      .catch(() => { if (!cancelled) setState({drafts: [], source: runtime, refresh}); });
    return () => { cancelled = true; };
  }, [runtime, refresh]);
  if (runtime?.status !== 'ready') return {drafts: [], loaded: true};
  const current = state.source === runtime;
  return {drafts: current ? state.drafts : [], loaded: current && state.refresh === refresh};
}
