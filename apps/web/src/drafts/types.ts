/**
 * Record drafts (ADR 0036): a member's unfinished input to a record form,
 * kept on this device until they save it, discard it, sign out or it
 * expires. A draft is never a record change and holds no authority: saving
 * one is the ordinary create or update, with every server check.
 */

/** What a form input holds: text, a checkbox, or a multi-select's values. Same as the record form's. */
export type DraftInput = string | boolean | string[];

/** The `drafts` block of the host's UI contributions (ADR 0036 §3). A host that sends none keeps no drafts. */
export type DraftsContribution = {
  /** `browser`: IndexedDB, encrypted with a server-held key. `host`: the device bridge's routes (a later slice). */
  store: 'browser' | 'host';
  expiryDays: number;
  maxDrafts: number;
  maxBytes: number;
};

/** Reads the contribution block, refusing anything malformed so a bad host means no drafts rather than wrong ones. */
export function draftsContributionFromApi(value: unknown): DraftsContribution | null {
  if (!value || typeof value !== 'object') return null;
  const block = value as Record<string, unknown>;
  const store = block.store;
  const expiryDays = block.expiry_days;
  const maxDrafts = block.max_drafts;
  const maxBytes = block.max_bytes;
  if (store !== 'browser' && store !== 'host') return null;
  if (!Number.isInteger(expiryDays) || (expiryDays as number) < 1 || (expiryDays as number) > 90) return null;
  if (!Number.isInteger(maxDrafts) || (maxDrafts as number) < 1) return null;
  if (!Number.isInteger(maxBytes) || (maxBytes as number) < 1) return null;
  return {store, expiryDays: expiryDays as number, maxDrafts: maxDrafts as number, maxBytes: maxBytes as number};
}

export type DraftKind = 'edit' | 'action' | 'create';

/** The parent a new record starts under, such as New on a related list. Ids only; the parent's name is in the encrypted document. */
export type DraftParent = {object: string; id: string; field: string};

/**
 * Which form a draft belongs to (ADR 0036 §2). Edits have one draft per
 * record (and per update action); each new-record form has its own.
 */
export type DraftKey =
  | {kind: 'edit'; object: string; record: string}
  | {kind: 'action'; object: string; record: string; action: string}
  | {kind: 'create'; object: string; draft: string; parent?: DraftParent; action?: string};

/** One changed field: where the member started and what they typed. */
export type DraftField = {
  start: DraftInput;
  value: DraftInput;
  /** The field's type when drafted, so a changed type is noticed on restore. */
  type: string;
  /** For a lookup: the name the picker showed for the chosen record. */
  label?: string;
  /** For a related-to field: the chosen record's object. */
  object?: string;
};

/** The encrypted payload: everything that names or holds record data. */
export type DraftDocument = {
  format: 1;
  key: DraftKey;
  object: string;
  /** The object's label when drafted, for the Drafts list of a new record. */
  objectLabel: string;
  /** An edit's or action's record when the form opened. */
  record?: {id: string; version: number; name: string};
  /** A new record's parent name, as the form showed it. */
  parentName?: string;
  action?: {name: string; label: string};
  /** Only the fields the member changed, keyed by API name; the record name is `name`. */
  fields: Record<string, DraftField>;
  startedAt: number;
  changedAt: number;
};

/** What stays plain beside the payload, so expiry and clearing work without the key. */
export type DraftMeta = {
  id: string;
  format: 1;
  member: string;
  workspace: string;
  key: string;
  kind: DraftKind;
  object: string;
  changedAt: number;
  expiresAt: number;
  size: number;
  revision: number;
  writer: string;
};

export type StoredDraft = {meta: DraftMeta; document: DraftDocument};

/** Why a draft could not be written; each has its own message (ADR 0036 §7). */
export type DraftRefusal = 'limit' | 'too_large' | 'quota' | 'other_tab' | 'failed';

export type PutResult = {ok: true; meta: DraftMeta} | {ok: false; reason: DraftRefusal};

/** Counts of drafts that expired or could not be restored since the member last looked. */
export type DraftNotices = {expired: number; dropped: number};

/**
 * The one interface the shell writes drafts through (ADR 0036 §3). A store
 * is bound to one member and one workspace; the browser implementation
 * encrypts, the device implementation (a later slice) calls the bridge.
 */
export interface DraftStore {
  /** Every readable draft of this member and workspace, newest first. Unreadable ones are deleted and counted as dropped. */
  list(): Promise<StoredDraft[]>;
  get(id: string): Promise<StoredDraft | null>;
  /**
   * Writes the whole draft in one atomic step, if it is still at
   * `expectedRevision` (0 for a new draft), so one tab never overwrites
   * another's newer write.
   */
  put(id: string, document: DraftDocument, expectedRevision: number): Promise<PutResult>;
  delete(id: string): Promise<void>;
  /** Deletes every draft of this member and workspace. */
  clear(): Promise<void>;
  /** Deletes drafts older than the expiry, counting them for the next notice. */
  sweep(expiryDays: number): Promise<number>;
  /** Reads and resets the notice counts. */
  takeNotices(): Promise<DraftNotices>;
}

const dayMs = 86_400_000;

export const expiresAt = (changedAt: number, expiryDays: number) => changedAt + expiryDays * dayMs;

/** Whether a draft expires within a day, for "Expires tomorrow". */
export const expiresSoon = (changedAt: number, expiryDays: number, now = Date.now()) => expiresAt(changedAt, expiryDays) - now <= dayMs;

/** The plain index key of a draft key; record ids and API names only. */
export function serialiseKey(key: DraftKey): string {
  switch (key.kind) {
    case 'edit': return JSON.stringify(['edit', key.object, key.record]);
    case 'action': return JSON.stringify(['action', key.object, key.record, key.action]);
    case 'create': return JSON.stringify(['create', key.object, key.draft, key.parent?.object ?? null, key.parent?.id ?? null, key.parent?.field ?? null, key.action ?? null]);
  }
}
