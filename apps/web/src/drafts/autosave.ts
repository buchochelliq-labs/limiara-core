import type {DraftDocument, DraftRefusal, DraftStore} from './types';

/** A write 1 second after the last change… */
export const DRAFT_IDLE_MS = 1_000;
/** …and at most 5 seconds after the first unsaved change while typing goes on (ADR 0036 §7). */
export const DRAFT_MAX_WAIT_MS = 5_000;

export type DraftState =
  | {kind: 'idle'}
  | {kind: 'saving'}
  | {kind: 'saved'; at: number}
  | {kind: 'failed'; reason: DraftRefusal};

type Timers = {set: (callback: () => void, ms: number) => unknown; clear: (handle: unknown) => void};
const realTimers: Timers = {set: (callback, ms) => setTimeout(callback, ms), clear: handle => clearTimeout(handle as ReturnType<typeof setTimeout>)};

/**
 * Autosave for one form's draft. A change schedules a write; writes run
 * one at a time, each the whole document; nothing is written until the
 * member changes something, and a form whose changes are all undone has
 * its draft deleted. The controller knows nothing of React, so it is
 * tested on its own.
 */
export class DraftAutosave {
  private idle: unknown = null;
  private deadline: unknown = null;
  private writing: Promise<void> | null = null;
  private dirty = false;
  private stopped = false;
  state: DraftState = {kind: 'idle'};

  constructor(
    private readonly store: DraftStore,
    readonly id: string,
    private revision: number,
    private readonly build: () => DraftDocument | null,
    private readonly onState: (state: DraftState) => void,
    private readonly onPending: (pending: boolean) => void = () => undefined,
    private readonly timers: Timers = realTimers,
  ) {}

  /** Whether a draft of this form is stored. */
  get saved(): boolean {
    return this.revision > 0;
  }

  /** Whether a write is waiting or running: the only time leaving the page loses input. */
  get pending(): boolean {
    return this.idle !== null || this.deadline !== null || this.writing !== null;
  }

  /** The revision this tab last wrote or read, for the two-tab check. */
  get currentRevision(): number {
    return this.revision;
  }

  /** Takes over a newer revision written elsewhere, such as another tab's. */
  adopt(revision: number) {
    this.revision = revision;
    this.setState({kind: 'saved', at: Date.now()});
  }

  /** A change: write after 1 s of quiet, and no later than 5 s after the first unsaved change. */
  changed() {
    if (this.stopped) return;
    this.dirty = true;
    if (this.idle !== null) this.timers.clear(this.idle);
    this.idle = this.timers.set(() => { this.idle = null; void this.flush(); }, DRAFT_IDLE_MS);
    if (this.deadline === null) this.deadline = this.timers.set(() => { this.deadline = null; void this.flush(); }, DRAFT_MAX_WAIT_MS);
    this.onPending(true);
  }

  /** Writes now: on `visibilitychange` to hidden, `pagehide`, and when the form closes. */
  async flush(): Promise<void> {
    this.clearTimers();
    if (this.writing) {
      await this.writing;
      if (this.dirty) return this.flush();
      return;
    }
    if (!this.dirty) { this.onPending(this.pending); return; }
    this.dirty = false;
    this.writing = this.write().finally(() => { this.writing = null; this.onPending(this.pending); });
    await this.writing;
    if (this.dirty && !this.stopped) await this.flush();
  }

  private async write() {
    const document = this.build();
    if (!document || Object.keys(document.fields).length === 0) {
      // Every change was undone: nothing is worth keeping.
      if (this.revision > 0) {
        await this.store.delete(this.id).catch(() => undefined);
        this.revision = 0;
      }
      this.setState({kind: 'idle'});
      return;
    }
    if (this.state.kind !== 'saved') this.setState({kind: 'saving'});
    let result;
    try {
      result = await this.store.put(this.id, document, this.revision);
    } catch {
      result = {ok: false as const, reason: 'failed' as const};
    }
    if (result.ok) {
      this.revision = result.meta.revision;
      this.setState({kind: 'saved', at: result.meta.changedAt});
    } else {
      this.setState({kind: 'failed', reason: result.reason});
    }
  }

  /** Discards the draft: Discard, or a successful Save. Pending writes are dropped first. */
  async discard(): Promise<void> {
    this.stopped = true;
    this.clearTimers();
    this.dirty = false;
    if (this.writing) await this.writing.catch(() => undefined);
    await this.store.delete(this.id);
    this.revision = 0;
    this.onPending(false);
  }

  /** Stops autosaving without writing, as when a save starts. */
  pause() {
    this.clearTimers();
    this.onPending(this.pending);
  }

  private clearTimers() {
    if (this.idle !== null) { this.timers.clear(this.idle); this.idle = null; }
    if (this.deadline !== null) { this.timers.clear(this.deadline); this.deadline = null; }
  }

  private setState(state: DraftState) {
    this.state = state;
    this.onState(state);
  }
}

/** What the form footer says (ADR 0036 §8). */
export function draftStatusText(state: DraftState | 'unavailable', formatTime: (at: number) => string = defaultTime): string {
  if (state === 'unavailable') return 'Drafts are unavailable';
  switch (state.kind) {
    case 'idle': return '';
    case 'saving': return 'Saving draft…';
    case 'saved': return `Draft saved at ${formatTime(state.at)}`;
    case 'failed': return refusalText(state.reason);
  }
}

export function refusalText(reason: DraftRefusal, maxDrafts = 50): string {
  switch (reason) {
    case 'limit': return `Draft not saved: you have ${maxDrafts} drafts. Discard some from Drafts.`;
    case 'too_large':
    case 'quota': return 'Draft not saved: Too large to keep as a draft';
    case 'other_tab': return 'This draft is open in another tab';
    case 'failed': return 'Draft not saved: it could not be written';
  }
}

/** The part of the status a screen reader hears: only a change of state, not each save (ADR 0036 §8). */
export function draftAnnouncement(state: DraftState | 'unavailable', maxDrafts = 50): string {
  if (state === 'unavailable') return 'Drafts are unavailable';
  if (state.kind === 'saved') return 'Draft saved';
  if (state.kind === 'failed') return state.reason === 'limit' ? refusalText('limit', maxDrafts) : refusalText(state.reason);
  return '';
}

function defaultTime(at: number) {
  return new Date(at).toLocaleTimeString('en-GB', {hour: '2-digit', minute: '2-digit'});
}
