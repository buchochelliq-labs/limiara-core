import {associatedData, DRAFT_FORMAT, decodeText, encodeText, open, seal, type Sealed} from './crypto';
import {expiresAt, serialiseKey, type DraftDocument, type DraftMeta, type DraftNotices, type DraftStore, type PutResult, type StoredDraft} from './types';

/**
 * The browser's draft store (ADR 0036 §3): one IndexedDB database per
 * origin, `limiara-drafts`, with
 *
 * - `drafts`: one entry per draft with plain metadata (ids, the draft key,
 *   times, size, revision, writer tab) and one encrypted payload;
 * - `browser`: this browser's random id and the member who last used it;
 * - `notices`: counts of drafts expired or dropped, with no content.
 *
 * Every save is one `readwrite` transaction that checks the entry's
 * revision and puts the whole entry, asking for strict durability, so after
 * a crash an entry is either the previous version or the new one.
 */
export const DRAFTS_DATABASE = 'limiara-drafts';
const DATABASE_VERSION = 1;
const DRAFTS = 'drafts';
const BROWSER = 'browser';
const NOTICES = 'notices';
const BY_SCOPE = 'member_workspace';
const BY_KEY = 'member_workspace_key';
const BY_MEMBER = 'member';
const THIS_BROWSER = 'this';

type Entry = DraftMeta & {payload: Sealed};
type BrowserEntry = {id: typeof THIS_BROWSER; browserId: string; member: string | null};
type NoticeEntry = {member: string; workspace: string; expired: number; dropped: number};

export type DraftLimits = {maxDrafts: number; maxBytes: number; expiryDays: number};

const strict: IDBTransactionOptions = {durability: 'strict'};

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new DOMException('aborted', 'AbortError'));
  });
}

function isQuota(error: unknown): boolean {
  return error instanceof DOMException && (error.name === 'QuotaExceededError' || error.name === 'NS_ERROR_DOM_QUOTA_REACHED');
}

/** The minimal shape every restored document must have; anything else is dropped. */
function readDocument(text: string): DraftDocument | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!value || typeof value !== 'object') return null;
  const document = value as Partial<DraftDocument>;
  if (document.format !== DRAFT_FORMAT || typeof document.object !== 'string' || !document.key || typeof document.key !== 'object') return null;
  if (!document.fields || typeof document.fields !== 'object' || Array.isArray(document.fields)) return null;
  if (typeof document.changedAt !== 'number' || typeof document.startedAt !== 'number') return null;
  return document as DraftDocument;
}

/** The drafts database of this origin, shared by every workspace and member in the browser. */
export class BrowserDraftDatabase {
  private database: Promise<IDBDatabase> | null = null;

  constructor(private readonly factory: IDBFactory) {}

  db(): Promise<IDBDatabase> {
    if (!this.database) {
      this.database = new Promise((resolve, reject) => {
        const opening = this.factory.open(DRAFTS_DATABASE, DATABASE_VERSION);
        opening.onupgradeneeded = () => {
          const db = opening.result;
          const drafts = db.createObjectStore(DRAFTS, {keyPath: 'id'});
          drafts.createIndex(BY_SCOPE, ['member', 'workspace']);
          drafts.createIndex(BY_KEY, ['member', 'workspace', 'key']);
          drafts.createIndex(BY_MEMBER, 'member');
          db.createObjectStore(BROWSER, {keyPath: 'id'});
          db.createObjectStore(NOTICES, {keyPath: ['member', 'workspace']});
        };
        opening.onsuccess = () => {
          const db = opening.result;
          // Another tab upgrading later closes this connection rather than blocking it.
          db.onversionchange = () => db.close();
          resolve(db);
        };
        opening.onerror = () => reject(opening.error);
        opening.onblocked = () => reject(new DOMException('blocked', 'InvalidStateError'));
      });
      this.database.catch(() => { this.database = null; });
    }
    return this.database;
  }

  /** This browser's random id, created on first use; it names the browser's draft key on the server. */
  async browserId(): Promise<string> {
    const db = await this.db();
    const tx = db.transaction(BROWSER, 'readwrite', strict);
    const store = tx.objectStore(BROWSER);
    const found = await request(store.get(THIS_BROWSER)) as BrowserEntry | undefined;
    let id = found?.browserId;
    if (!id) {
      id = crypto.randomUUID();
      store.put({id: THIS_BROWSER, browserId: id, member: null} satisfies BrowserEntry);
    }
    await done(tx);
    return id;
  }

  /**
   * Before the shell renders anything for a signed-in member (ADR 0036 §11):
   * deletes every draft of any other member, and this member's drafts for
   * workspaces they no longer belong to. Nothing about the other member's
   * drafts is counted or shown. Returns how many drafts were deleted.
   */
  async prepareFor(member: string, workspaces: readonly string[]): Promise<number> {
    const db = await this.db();
    const keep = new Set(workspaces);
    const tx = db.transaction([DRAFTS, NOTICES, BROWSER], 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    const notices = tx.objectStore(NOTICES);
    let deleted = 0;
    const entries = await request(drafts.getAll()) as Entry[];
    for (const entry of entries) {
      if (entry.member !== member || !keep.has(entry.workspace)) {
        drafts.delete(entry.id);
        deleted += 1;
      }
    }
    const counts = await request(notices.getAll()) as NoticeEntry[];
    for (const count of counts) {
      if (count.member !== member || !keep.has(count.workspace)) notices.delete([count.member, count.workspace]);
    }
    const browser = tx.objectStore(BROWSER);
    const found = await request(browser.get(THIS_BROWSER)) as BrowserEntry | undefined;
    if (found) browser.put({...found, member});
    await done(tx);
    return deleted;
  }

  /** Sign-out (ADR 0036 §11): deletes every draft and notice of this member, in every workspace. */
  async clearMember(member: string): Promise<void> {
    const db = await this.db();
    const tx = db.transaction([DRAFTS, NOTICES], 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    const ids = await request(drafts.index(BY_MEMBER).getAllKeys(member));
    for (const id of ids) drafts.delete(id);
    const notices = tx.objectStore(NOTICES);
    const counts = await request(notices.getAll()) as NoticeEntry[];
    for (const count of counts) if (count.member === member) notices.delete([count.member, count.workspace]);
    await done(tx);
  }

  /** The store of one member and workspace, encrypting with that workspace's key. */
  store(member: string, workspace: string, key: CryptoKey, limits: DraftLimits, writer: string): DraftStore {
    return new BrowserDraftStore(this, member, workspace, key, limits, writer);
  }
}

class BrowserDraftStore implements DraftStore {
  constructor(
    private readonly database: BrowserDraftDatabase,
    private readonly member: string,
    private readonly workspace: string,
    private readonly key: CryptoKey,
    private readonly limits: DraftLimits,
    private readonly writer: string,
  ) {}

  private aad(id: string) {
    return associatedData(this.member, this.workspace, id);
  }

  private async decrypt(entry: Entry): Promise<StoredDraft | null> {
    if (entry.format !== DRAFT_FORMAT || entry.member !== this.member || entry.workspace !== this.workspace) return null;
    try {
      const plain = await open(this.key, this.aad(entry.id), entry.payload);
      const document = readDocument(decodeText(plain));
      if (!document) return null;
      const {payload: _payload, ...meta} = entry;
      return {meta, document};
    } catch {
      return null;
    }
  }

  private async scopeEntries(): Promise<Entry[]> {
    const db = await this.database.db();
    const tx = db.transaction(DRAFTS, 'readonly');
    return await request(tx.objectStore(DRAFTS).index(BY_SCOPE).getAll([this.member, this.workspace])) as Entry[];
  }

  /** Deletes drafts that cannot be read and counts them as dropped (ADR 0036 §5). */
  private async drop(ids: string[]): Promise<void> {
    if (!ids.length) return;
    const db = await this.database.db();
    const tx = db.transaction([DRAFTS, NOTICES], 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    for (const id of ids) drafts.delete(id);
    this.addNotice(tx, {dropped: ids.length});
    await done(tx);
  }

  private addNotice(tx: IDBTransaction, add: Partial<DraftNotices>) {
    const notices = tx.objectStore(NOTICES);
    const read = notices.get([this.member, this.workspace]);
    read.onsuccess = () => {
      const current = (read.result as NoticeEntry | undefined) ?? {member: this.member, workspace: this.workspace, expired: 0, dropped: 0};
      notices.put({...current, expired: current.expired + (add.expired ?? 0), dropped: current.dropped + (add.dropped ?? 0)});
    };
  }

  async list(): Promise<StoredDraft[]> {
    const entries = await this.scopeEntries();
    const readable: StoredDraft[] = [];
    const unreadable: string[] = [];
    for (const entry of entries) {
      const draft = await this.decrypt(entry);
      if (draft) readable.push(draft);
      else unreadable.push(entry.id);
    }
    await this.drop(unreadable);
    return readable.sort((left, right) => right.meta.changedAt - left.meta.changedAt);
  }

  async get(id: string): Promise<StoredDraft | null> {
    const db = await this.database.db();
    const tx = db.transaction(DRAFTS, 'readonly');
    const entry = await request(tx.objectStore(DRAFTS).get(id)) as Entry | undefined;
    if (!entry || entry.member !== this.member || entry.workspace !== this.workspace) return null;
    const draft = await this.decrypt(entry);
    if (!draft) await this.drop([id]);
    return draft;
  }

  async put(id: string, document: DraftDocument, expectedRevision: number): Promise<PutResult> {
    const plain = encodeText(JSON.stringify(document));
    if (plain.byteLength > this.limits.maxBytes) return {ok: false, reason: 'too_large'};
    let payload: Sealed;
    try {
      payload = await seal(this.key, this.aad(id), plain);
    } catch {
      return {ok: false, reason: 'failed'};
    }
    const db = await this.database.db();
    let tx: IDBTransaction;
    try {
      tx = db.transaction(DRAFTS, 'readwrite', strict);
    } catch (error) {
      return {ok: false, reason: isQuota(error) ? 'quota' : 'failed'};
    }
    const drafts = tx.objectStore(DRAFTS);
    const meta: DraftMeta = {
      id,
      format: DRAFT_FORMAT,
      member: this.member,
      workspace: this.workspace,
      key: serialiseKey(document.key),
      kind: document.key.kind,
      object: document.object,
      changedAt: document.changedAt,
      expiresAt: expiresAt(document.changedAt, this.limits.expiryDays),
      size: plain.byteLength,
      revision: expectedRevision + 1,
      writer: this.writer,
    };
    let refusal: PutResult | null = null;
    const write = () => {
      try {
        drafts.put({...meta, payload} satisfies Entry);
      } catch (error) {
        refusal = {ok: false, reason: isQuota(error) ? 'quota' : 'failed'};
        tx.abort();
      }
    };
    // Read, check and write in one transaction: no other tab's write can land in between.
    const read = drafts.get(id);
    read.onsuccess = () => {
      const existing = read.result as Entry | undefined;
      if (existing && (existing.member !== this.member || existing.workspace !== this.workspace)) {
        refusal = {ok: false, reason: 'failed'};
        tx.abort();
        return;
      }
      if (existing && existing.revision !== expectedRevision) {
        refusal = {ok: false, reason: 'other_tab'};
        tx.abort();
        return;
      }
      if (existing) { write(); return; }
      // A new draft counts against the limit; existing drafts keep saving (ADR 0036 §7).
      const counting = drafts.index(BY_SCOPE).count([this.member, this.workspace]);
      counting.onsuccess = () => {
        if (counting.result >= this.limits.maxDrafts) {
          refusal = {ok: false, reason: 'limit'};
          tx.abort();
          return;
        }
        write();
      };
    };
    try {
      await done(tx);
      return {ok: true, meta};
    } catch (error) {
      if (refusal) return refusal;
      return {ok: false, reason: isQuota(error) ? 'quota' : 'failed'};
    }
  }

  async delete(id: string): Promise<void> {
    const db = await this.database.db();
    const tx = db.transaction(DRAFTS, 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    const found = await request(drafts.get(id)) as Entry | undefined;
    if (found && found.member === this.member && found.workspace === this.workspace) drafts.delete(id);
    await done(tx);
  }

  async clear(): Promise<void> {
    const db = await this.database.db();
    const tx = db.transaction(DRAFTS, 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    const ids = await request(drafts.index(BY_SCOPE).getAllKeys([this.member, this.workspace]));
    for (const id of ids) drafts.delete(id);
    await done(tx);
  }

  async sweep(expiryDays: number, now = Date.now()): Promise<number> {
    const db = await this.database.db();
    const tx = db.transaction([DRAFTS, NOTICES], 'readwrite', strict);
    const drafts = tx.objectStore(DRAFTS);
    const entries = await request(drafts.index(BY_SCOPE).getAll([this.member, this.workspace])) as Entry[];
    // Expiry follows the current setting, so shortening it expires existing drafts now.
    const expired = entries.filter(entry => expiresAt(entry.changedAt, expiryDays) <= now);
    for (const entry of expired) drafts.delete(entry.id);
    if (expired.length) this.addNotice(tx, {expired: expired.length});
    await done(tx);
    return expired.length;
  }

  async takeNotices(): Promise<DraftNotices> {
    const db = await this.database.db();
    const tx = db.transaction(NOTICES, 'readwrite', strict);
    const notices = tx.objectStore(NOTICES);
    const found = await request(notices.get([this.member, this.workspace])) as NoticeEntry | undefined;
    if (found) notices.delete([this.member, this.workspace]);
    await done(tx);
    return {expired: found?.expired ?? 0, dropped: found?.dropped ?? 0};
  }
}
