import {IDBFactory, IDBObjectStore} from 'fake-indexeddb';
import {renderToStaticMarkup} from 'react-dom/server';
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {createFixtureObjectsClient, type ObjectField, type ObjectsClient} from '../objects/api';
import {DraftAutosave, DRAFT_IDLE_MS, DRAFT_MAX_WAIT_MS, draftAnnouncement, draftStatusText, type DraftState} from './autosave';
import {BrowserDraftDatabase} from './browserStore';
import {CancelChoice, CreateDraftChooser, DraftStatus, noticeLines, RestorePrompt} from './components';
import {associatedData, decodeKey, deriveWorkspaceKey, DRAFT_KEY_INFO, importBrowserKey, open, seal} from './crypto';
import {DraftsPage, draftRoute} from './DraftsPage';
import {DraftSettingsPage} from './DraftSettingsPage';
import {createDraftsFor, parseKey} from './lists';
import {changedFields, convertInput, draftTitle, droppedMessage, fieldAccess, planRestore} from './model';
import {createDraftRuntime, DraftsContext, loadBrowserKey, prepareBrowserDrafts, resetDraftRuntimeForTests, signOut, type DraftRuntime} from './runtime';
import {serialiseKey, type DraftDocument, type DraftKey, type DraftStore} from './types';

const member = '018f0000-0000-7000-8000-000000000101';
const other = '018f0000-0000-7000-8000-000000000102';
const workspace = '018f0000-0000-7000-8000-000000000001';
const otherWorkspace = '018f0000-0000-7000-8000-000000000002';
const limits = {maxDrafts: 50, maxBytes: 1_048_576, expiryDays: 7};

function rawKey(fill = 7): Uint8Array {
  return new Uint8Array(32).fill(fill);
}

async function workspaceKey(fill = 7, id = workspace) {
  return deriveWorkspaceKey(await importBrowserKey(rawKey(fill)), id);
}

function documentFor(key: DraftKey, fields: DraftDocument['fields'] = {name: {start: '', value: 'Vega', type: 'text'}}, extra: Partial<DraftDocument> = {}): DraftDocument {
  return {format: 1, key, object: key.object, objectLabel: 'Project', fields, startedAt: Date.now(), changedAt: Date.now(), ...extra};
}

const createKey = (draft: string = crypto.randomUUID()): DraftKey => ({kind: 'create', object: 'custom:project', draft});

let factory: IDBFactory;
let database: BrowserDraftDatabase;

beforeEach(() => {
  factory = new IDBFactory();
  database = new BrowserDraftDatabase(factory);
  resetDraftRuntimeForTests();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function storeFor(id = member, space = workspace, writer = 'tab-a', keyFill = 7, overrides: Partial<typeof limits> = {}): Promise<DraftStore> {
  return database.store(id, space, await workspaceKey(keyFill, space), {...limits, ...overrides}, writer);
}

describe('draft encryption (ADR 0036 §4)', () => {
  it('round-trips a payload under the member, workspace and draft it names', async () => {
    const key = await workspaceKey();
    const aad = associatedData(member, workspace, 'draft-1');
    const sealed = await seal(key, aad, new TextEncoder().encode('secret value'));
    expect(sealed.nonce.byteLength).toBe(12);
    expect(new TextDecoder().decode(await open(key, aad, sealed))).toBe('secret value');
    // A fresh nonce on every write.
    const again = await seal(key, aad, new TextEncoder().encode('secret value'));
    expect(Array.from(again.nonce)).not.toEqual(Array.from(sealed.nonce));
  });

  it('refuses a payload moved to another member, workspace or draft, or opened with another key', async () => {
    const key = await workspaceKey();
    const sealed = await seal(key, associatedData(member, workspace, 'draft-1'), new TextEncoder().encode('secret value'));
    for (const aad of [associatedData(other, workspace, 'draft-1'), associatedData(member, otherWorkspace, 'draft-1'), associatedData(member, workspace, 'draft-2'), associatedData(member, workspace, 'draft-1', 2)]) {
      await expect(open(key, aad, sealed)).rejects.toThrow('cannot be read');
    }
    await expect(open(await workspaceKey(7, otherWorkspace), associatedData(member, workspace, 'draft-1'), sealed)).rejects.toThrow();
    await expect(open(await workspaceKey(9), associatedData(member, workspace, 'draft-1'), sealed)).rejects.toThrow();
    const tampered = new Uint8Array(sealed.data);
    tampered[0] = (tampered[0] ?? 0) ^ 1;
    await expect(open(key, associatedData(member, workspace, 'draft-1'), {nonce: sealed.nonce, data: tampered.buffer})).rejects.toThrow();
  });

  it('derives non-extractable keys with HKDF-SHA-256 and wipes the raw key', async () => {
    const raw = rawKey(3);
    const browserKey = await importBrowserKey(raw);
    expect(Array.from(raw).every(byte => byte === 0)).toBe(true);
    expect(browserKey.extractable).toBe(false);
    const derived = await deriveWorkspaceKey(browserKey, workspace);
    expect(derived.extractable).toBe(false);
    expect(derived.algorithm).toMatchObject({name: 'AES-GCM', length: 256});
    await expect(crypto.subtle.exportKey('raw', derived)).rejects.toThrow();
    // The scheme as specified: HKDF(SHA-256, salt = workspace id, info = limiara.record-drafts.v1).
    const reference = await crypto.subtle.deriveKey(
      {name: 'HKDF', hash: 'SHA-256', salt: new TextEncoder().encode(workspace), info: new TextEncoder().encode(DRAFT_KEY_INFO)},
      await crypto.subtle.importKey('raw', rawKey(3), 'HKDF', false, ['deriveKey']),
      {name: 'AES-GCM', length: 256}, true, ['encrypt', 'decrypt'],
    );
    const aad = associatedData(member, workspace, 'x');
    const sealed = await seal(reference, aad, new TextEncoder().encode('same key'));
    expect(new TextDecoder().decode(await open(derived, aad, sealed))).toBe('same key');
    expect(DRAFT_KEY_INFO).toBe('limiara.record-drafts.v1');
  });

  it('decodes only 32-byte base64url keys', () => {
    const encoded = btoa(String.fromCharCode(...rawKey(255))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    expect(decodeKey(encoded).byteLength).toBe(32);
    expect(() => decodeKey('c2hvcnQ')).toThrow('wrong length');
    expect(() => decodeKey('***')).toThrow();
  });
});

describe('the browser draft store (ADR 0036 §3, §7)', () => {
  it('writes the whole entry in one strict transaction and keeps values only in the encrypted payload', async () => {
    const store = await storeFor();
    const opened = await database.db();
    const transaction = vi.spyOn(opened, 'transaction');
    const key = createKey();
    const result = await store.put('d1', documentFor(key, {name: {start: '', value: 'Secret name', type: 'text'}}), 0);
    expect(result).toMatchObject({ok: true, meta: {revision: 1, member, workspace, key: serialiseKey(key), kind: 'create', object: 'custom:project', writer: 'tab-a'}});
    expect(transaction.mock.calls.some(([, mode, options]) => mode === 'readwrite' && options?.durability === 'strict')).toBe(true);
    const raw = await new Promise<Record<string, unknown>>((resolve, reject) => {
      const request = opened.transaction('drafts').objectStore('drafts').get('d1');
      request.onsuccess = () => resolve(request.result as Record<string, unknown>);
      request.onerror = () => reject(request.error);
    });
    expect(JSON.stringify({...raw, payload: undefined})).not.toContain('Secret name');
    expect(new TextDecoder().decode(new Uint8Array((raw.payload as {data: ArrayBuffer}).data))).not.toContain('Secret name');
    expect((await store.get('d1'))?.document.fields.name?.value).toBe('Secret name');
  });

  it('stops one tab overwriting another tab’s newer revision', async () => {
    const first = await storeFor(member, workspace, 'tab-a');
    const second = await storeFor(member, workspace, 'tab-b');
    const key = createKey();
    expect((await first.put('d1', documentFor(key), 0)).ok).toBe(true);
    expect(await second.put('d1', documentFor(key), 0)).toEqual({ok: false, reason: 'other_tab'});
    expect((await second.put('d1', documentFor(key), 1)).ok).toBe(true);
    expect(await first.put('d1', documentFor(key), 1)).toEqual({ok: false, reason: 'other_tab'});
    const latest = await first.get('d1');
    expect(latest?.meta).toMatchObject({revision: 2, writer: 'tab-b'});
  });

  it('refuses a new draft at the limit without evicting any, while existing drafts keep saving', async () => {
    const store = await storeFor(member, workspace, 'tab-a', 7, {maxDrafts: 3});
    for (const id of ['a', 'b', 'c']) expect((await store.put(id, documentFor(createKey()), 0)).ok).toBe(true);
    expect(await store.put('d', documentFor(createKey()), 0)).toEqual({ok: false, reason: 'limit'});
    expect((await store.put('a', documentFor(createKey()), 1)).ok).toBe(true);
    expect((await store.list()).map(draft => draft.meta.id).sort()).toEqual(['a', 'b', 'c']);
    // Another workspace has its own count.
    expect((await (await storeFor(member, otherWorkspace, 'tab-a', 7, {maxDrafts: 3})).put('e', documentFor(createKey()), 0)).ok).toBe(true);
  });

  it('refuses a document over the size limit and reports a refused quota', async () => {
    const store = await storeFor(member, workspace, 'tab-a', 7, {maxBytes: 200});
    expect(await store.put('big', documentFor(createKey(), {name: {start: '', value: 'x'.repeat(300), type: 'text'}}), 0)).toEqual({ok: false, reason: 'too_large'});
    const roomy = await storeFor();
    vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(() => { throw new DOMException('full', 'QuotaExceededError'); });
    expect(await roomy.put('full', documentFor(createKey()), 0)).toEqual({ok: false, reason: 'quota'});
  });

  it('deletes and counts drafts that fail to decrypt or have an unknown format', async () => {
    const store = await storeFor();
    await store.put('good', documentFor(createKey()), 0);
    await store.put('moved', documentFor(createKey()), 0);
    // A payload copied into another draft id fails its associated data.
    const opened = await database.db();
    await new Promise<void>((resolve, reject) => {
      const tx = opened.transaction('drafts', 'readwrite');
      const drafts = tx.objectStore('drafts');
      const read = drafts.get('good');
      read.onsuccess = () => drafts.put({...read.result, id: 'copy'});
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    // A key the member no longer has: drafts written with another key cannot be read.
    await (await storeFor(member, workspace, 'tab-a', 9)).put('stale-key', documentFor(createKey()), 0);
    const listed = await store.list();
    expect(listed.map(draft => draft.meta.id).sort()).toEqual(['good', 'moved']);
    expect(await store.takeNotices()).toEqual({expired: 0, dropped: 2});
    expect(await store.takeNotices()).toEqual({expired: 0, dropped: 0});
  });

  it('expires drafts from the current setting and counts them once', async () => {
    const store = await storeFor();
    const old = Date.now() - 3 * 86_400_000;
    await store.put('old', documentFor(createKey(), undefined, {changedAt: old}), 0);
    await store.put('new', documentFor(createKey()), 0);
    expect(await store.sweep(7)).toBe(0);
    // Shortening the expiry expires existing drafts at the next sweep.
    expect(await store.sweep(2)).toBe(1);
    expect((await store.list()).map(draft => draft.meta.id)).toEqual(['new']);
    expect(noticeLines(await store.takeNotices())).toEqual(['1 draft expired']);
  });

  it('clears another member’s drafts and drafts of workspaces left, before anything renders', async () => {
    const mine = await storeFor(member);
    const theirs = await storeFor(other);
    const left = await storeFor(member, otherWorkspace);
    await mine.put('mine', documentFor(createKey()), 0);
    await theirs.put('theirs', documentFor(createKey()), 0);
    await left.put('left', documentFor(createKey()), 0);
    await prepareBrowserDrafts(member, [workspace], database);
    expect((await mine.list()).map(draft => draft.meta.id)).toEqual(['mine']);
    expect(await theirs.list()).toEqual([]);
    expect(await left.list()).toEqual([]);
    // Nothing is counted for the other member.
    expect(await theirs.takeNotices()).toEqual({expired: 0, dropped: 0});
    // Even undeleted, their payloads are unreadable to this member: another key, and the associated data names them.
    const sealed = await seal(await workspaceKey(5), associatedData(other, workspace, 'x'), new TextEncoder().encode('theirs'));
    await expect(open(await workspaceKey(7), associatedData(member, workspace, 'x'), sealed)).rejects.toThrow();
  });

  it('signs out by deleting every draft of the member first, then posting with the browser id', async () => {
    const order: string[] = [];
    const here = await storeFor(member);
    const elsewhere = await storeFor(member, otherWorkspace);
    const theirs = await storeFor(other);
    await here.put('a', documentFor(createKey()), 0);
    await elsewhere.put('b', documentFor(createKey()), 0);
    await theirs.put('c', documentFor(createKey()), 0);
    const browserId = await database.browserId();
    await signOut(member, {drafts: database, submit: id => { order.push(`submit:${id}`); }});
    expect(order).toEqual([`submit:${browserId}`]);
    expect(await here.list()).toEqual([]);
    expect(await elsewhere.list()).toEqual([]);
    expect((await theirs.list()).length).toBe(1);
  });

  it('keeps one random browser id', async () => {
    const first = await database.browserId();
    expect(first).toMatch(/^[0-9a-f-]{36}$/);
    expect(await new BrowserDraftDatabase(factory).browserId()).toBe(first);
  });
});

describe('the draft key and runtime', () => {
  it('fetches the key once per page load, and turns drafting off without one', async () => {
    const key = btoa(String.fromCharCode(...rawKey(4))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const fetcher = vi.fn(async () => new Response(JSON.stringify({key}), {status: 200})) as unknown as typeof fetch;
    const contribution = {store: 'browser' as const, ...limits};
    const ready = await createDraftRuntime({contribution, member, workspace, drafts: database, fetcher});
    const again = await createDraftRuntime({contribution, member, workspace: otherWorkspace, drafts: database, fetcher});
    expect(ready?.status).toBe('ready');
    expect(again?.status).toBe('ready');
    expect(vi.mocked(fetcher)).toHaveBeenCalledTimes(1);
    expect(String(vi.mocked(fetcher).mock.calls[0]?.[0])).toBe(`/api/v1/me/draft-key?browser=${await database.browserId()}`);
    resetDraftRuntimeForTests();
    const refused = vi.fn(async () => new Response('{}', {status: 503})) as unknown as typeof fetch;
    expect(await createDraftRuntime({contribution, member, workspace, drafts: database, fetcher: refused})).toEqual({status: 'unavailable', contribution});
    // No drafts block, or only a host store: no drafts in this slice.
    expect(await createDraftRuntime({contribution: null, member, workspace, drafts: database, fetcher})).toBeNull();
    expect(await createDraftRuntime({contribution: {...contribution, store: 'host'}, member, workspace, drafts: database, fetcher})).toBeNull();
    resetDraftRuntimeForTests();
    await expect(loadBrowserKey(database, vi.fn(async () => new Response(JSON.stringify({key: 'short'}), {status: 200})) as unknown as typeof fetch)).rejects.toThrow();
  });
});

describe('autosave (ADR 0036 §7)', () => {
  function fakeStore() {
    const writes: DraftDocument[] = [];
    const deleted: string[] = [];
    let revision = 0;
    const store = {
      put: vi.fn(async (_id: string, document: DraftDocument, expected: number) => {
        if (expected !== revision) return {ok: false as const, reason: 'other_tab' as const};
        revision += 1;
        writes.push(document);
        return {ok: true as const, meta: {id: _id, format: 1 as const, member, workspace, key: '', kind: 'create' as const, object: 'custom:project', changedAt: document.changedAt, expiresAt: 0, size: 1, revision, writer: 't'}};
      }),
      delete: vi.fn(async (id: string) => { deleted.push(id); revision = 0; }),
    } as unknown as DraftStore;
    return {store, writes, deleted};
  }

  it('writes one second after the last change, and no later than five seconds while typing goes on', async () => {
    vi.useFakeTimers();
    const {store, writes} = fakeStore();
    let value = 'a';
    const states: DraftState[] = [];
    const pending: boolean[] = [];
    const autosave = new DraftAutosave(store, 'd', 0, () => documentFor(createKey('d'), {name: {start: '', value, type: 'text'}}), state => states.push(state), next => pending.push(next));
    expect(autosave.saved).toBe(false);
    autosave.changed();
    expect(autosave.pending).toBe(true);
    await vi.advanceTimersByTimeAsync(DRAFT_IDLE_MS - 1);
    expect(writes).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(writes).toHaveLength(1);
    expect(autosave.saved).toBe(true);
    // Continuous typing: a change every 500 ms still writes within five seconds.
    for (let elapsed = 0; elapsed < DRAFT_MAX_WAIT_MS; elapsed += 500) {
      value += 'b';
      autosave.changed();
      await vi.advanceTimersByTimeAsync(500);
    }
    expect(writes.length).toBeGreaterThanOrEqual(2);
    expect(states.at(-1)).toMatchObject({kind: 'saved'});
    await vi.advanceTimersByTimeAsync(DRAFT_IDLE_MS);
    expect(autosave.pending).toBe(false);
    expect(pending.at(-1)).toBe(false);
  });

  it('flushes at once, writes nothing before a change, and deletes the draft once every change is undone', async () => {
    const {store, writes, deleted} = fakeStore();
    let changed = true;
    const autosave = new DraftAutosave(store, 'd', 0, () => changed ? documentFor(createKey('d')) : documentFor(createKey('d'), {}), () => undefined);
    await autosave.flush();
    expect(writes).toHaveLength(0);
    autosave.changed();
    await autosave.flush();
    expect(writes).toHaveLength(1);
    changed = false;
    autosave.changed();
    await autosave.flush();
    expect(deleted).toEqual(['d']);
    expect(autosave.saved).toBe(false);
  });

  it('reports another tab’s newer write and takes it over', async () => {
    const {store} = fakeStore();
    const states: DraftState[] = [];
    const first = new DraftAutosave(store, 'd', 0, () => documentFor(createKey('d')), state => states.push(state));
    first.changed();
    await first.flush();
    const second = new DraftAutosave(store, 'd', 0, () => documentFor(createKey('d')), state => states.push(state));
    second.changed();
    await second.flush();
    expect(states.at(-1)).toEqual({kind: 'failed', reason: 'other_tab'});
    second.adopt(1);
    second.changed();
    await second.flush();
    expect(states.at(-1)).toMatchObject({kind: 'saved'});
  });

  it('words each state, announcing changes of state rather than each save', () => {
    expect(draftStatusText({kind: 'saving'})).toBe('Saving draft…');
    expect(draftStatusText({kind: 'saved', at: 0}, () => '14:32')).toBe('Draft saved at 14:32');
    expect(draftStatusText({kind: 'failed', reason: 'too_large'})).toBe('Draft not saved: Too large to keep as a draft');
    expect(draftStatusText({kind: 'failed', reason: 'quota'})).toBe('Draft not saved: Too large to keep as a draft');
    expect(draftStatusText('unavailable')).toBe('Drafts are unavailable');
    expect(draftAnnouncement({kind: 'saved', at: 1})).toBe(draftAnnouncement({kind: 'saved', at: 2}));
    expect(draftAnnouncement({kind: 'failed', reason: 'limit'}, 50)).toBe('Draft not saved: you have 50 drafts. Discard some from Drafts.');
  });
});

describe('restoring a draft (ADR 0036 §5, §6)', () => {
  const editKey = (record = '0000000000000001'): DraftKey => ({kind: 'edit', object: 'custom:project', record});

  it('puts values back into a form whose record has not changed', async () => {
    const client = createFixtureObjectsClient();
    const plan = await planRestore(client, documentFor(editKey(), {'custom:code': {start: 'AP-1', value: 'AP-9', type: 'text'}}, {record: {id: '0000000000000001', version: 1, name: 'Apollo'}}));
    expect(plan).toMatchObject({status: 'ready', values: {'custom:code': 'AP-9'}, dropped: {named: [], unnamed: 0}, record: {version: 1}});
  });

  it('applies changes nobody else made, refuses conflicting ones in this slice, and notices what is already saved', async () => {
    const client = createFixtureObjectsClient();
    const record = await client.getRecord('custom:project', '0000000000000001');
    await client.updateRecord('custom:project', record.id, record.version, {'custom:status': 'won'});
    const drafted = {record: {id: record.id, version: 1, name: 'Apollo'}};
    // Someone else changed another field: the draft applies onto the current version.
    const clean = await planRestore(client, documentFor(editKey(), {'custom:code': {start: 'AP-1', value: 'AP-9', type: 'text'}}, drafted));
    expect(clean).toMatchObject({status: 'ready', values: {'custom:code': 'AP-9'}, record: {version: 2}});
    // Both changed the same field differently: a conflict, kept or discarded only.
    expect(await planRestore(client, documentFor(editKey(), {'custom:status': {start: 'open', value: 'open-ish', type: 'picklist'}, 'custom:code': {start: 'AP-1', value: 'AP-9', type: 'text'}}, drafted))).toEqual({status: 'conflicts', fields: 1});
    // The same change on both sides: nothing left to restore.
    expect(await planRestore(client, documentFor(editKey(), {'custom:status': {start: 'open', value: 'won', type: 'picklist'}}, drafted))).toEqual({status: 'already_saved'});
  });

  it('makes a draft unavailable when the object or record is gone or no longer editable', async () => {
    const client = createFixtureObjectsClient();
    expect(await planRestore(client, documentFor({kind: 'create', object: 'custom:gone', draft: 'x'}))).toEqual({status: 'unavailable'});
    const record = await client.getRecord('custom:project', '0000000000000002');
    await client.deleteRecord('custom:project', record.id, record.version);
    expect(await planRestore(client, documentFor(editKey('0000000000000002'), {}, {record: {id: record.id, version: 1, name: 'Gemini'}}))).toEqual({status: 'unavailable'});
    const readOnly: ObjectsClient = {...client, describe: async api => ({...(await client.describe(api)), access: {read: true, create: false, edit: false, delete: false, view_all: false, modify_all: false}})};
    expect(await planRestore(readOnly, documentFor(createKey()))).toEqual({status: 'unavailable'});
    const notMine: ObjectsClient = {...client, getRecord: async (api, id) => ({...(await client.getRecord(api, id)), rights: {read: true, edit: false, delete: false, share: false, transfer: false}})};
    expect(await planRestore(notMine, documentFor(editKey(), {}, {record: {id: '0000000000000001', version: 1, name: 'Apollo'}}))).toEqual({status: 'unavailable'});
  });

  it('drops fields that are gone or unreadable without naming them, and names the rest it drops', async () => {
    const client = createFixtureObjectsClient();
    await client.createField('custom:project', {name: 'score', label: 'Score', field_type: 'formula', formula: '1', formula_type: 'number'});
    await client.createField('custom:project', {name: 'owner_ref', label: 'Partner', field_type: 'lookup', reference_object: 'custom:project'});
    const secretive: ObjectsClient = {...client, describe: async api => {
      const description = await client.describe(api);
      return {...description, fields: description.fields.map(field => field.apiName === 'custom:starts_on' ? {...field, access: {read: false, write: false}} as typeof field : field.apiName === 'custom:budget' ? {...field, access: {read: true, write: false}} as typeof field : field)};
    }};
    const plan = await planRestore(secretive, documentFor(createKey(), {
      name: {start: '', value: 'Vega', type: 'text'},
      'custom:removed': {start: '', value: 'x', type: 'text'},
      'custom:starts_on': {start: '', value: '2026-12-01', type: 'date'},
      'custom:budget': {start: '', value: '10', type: 'currency'},
      'custom:score': {start: '', value: '5', type: 'number'},
      'custom:code': {start: '', value: 'true', type: 'boolean'},
      'custom:owner_ref': {start: '', value: '000missing00000', type: 'lookup', label: 'Lost'},
      'custom:status': {start: 'open', value: 'gone-value', type: 'picklist'},
    }));
    expect(plan.status).toBe('ready');
    if (plan.status !== 'ready') return;
    expect(plan.values).toEqual({name: 'Vega', 'custom:status': 'gone-value'});
    expect(plan.dropped.unnamed).toBe(2);
    expect(plan.dropped.named.sort()).toEqual(['Budget', 'Code', 'Partner', 'Score']);
    expect(droppedMessage(plan.dropped)[0]).toBe('Some changes were dropped because those fields no longer exist or are not available to you.');
  });

  it('keeps a lookup the member can still read, with the name the record has now', async () => {
    const client = createFixtureObjectsClient();
    await client.createField('custom:project', {name: 'partner', label: 'Partner', field_type: 'lookup', reference_object: 'custom:project'});
    const plan = await planRestore(client, documentFor(createKey(), {'custom:partner': {start: '', value: '0000000000000002', type: 'lookup', label: '<b>Old name</b>'}}));
    expect(plan).toMatchObject({status: 'ready', values: {'custom:partner': '0000000000000002'}, lookupNames: {'custom:partner': 'Gemini'}});
  });

  it('restores an action draft into the full form when its action is gone', async () => {
    const client = createFixtureObjectsClient();
    const plan = await planRestore(client, documentFor({kind: 'action', object: 'custom:project', record: '0000000000000001', action: 'removed'}, {'custom:code': {start: 'AP-1', value: 'AP-2', type: 'text'}}, {record: {id: '0000000000000001', version: 1, name: 'Apollo'}}));
    expect(plan).toMatchObject({status: 'ready', fullForm: true, values: {'custom:code': 'AP-2'}});
  });

  it('converts values only between compatible field types', () => {
    const text: ObjectField = {apiName: 'f', label: 'F', description: null, type: 'long_text', origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1};
    expect(convertInput('text', text, 'hello')).toBe('hello');
    expect(convertInput('boolean', text, true)).toBeUndefined();
    expect(convertInput('multi_picklist', {...text, type: 'picklist'}, ['a'])).toBeUndefined();
    expect(fieldAccess({...text, type: 'formula'})).toEqual({read: true, write: false});
  });

  it('keeps only changed fields with their start values, and titles drafts as inert text', () => {
    expect(changedFields({name: 'A', 'custom:code': 'X'}, {name: 'A', 'custom:code': 'Y'}, {'custom:code': 'text'})).toEqual({'custom:code': {start: 'X', value: 'Y', type: 'text'}});
    expect(draftTitle(documentFor(createKey(), {name: {start: '', value: '<img src=x onerror=alert(1)>', type: 'text'}}))).toBe('<img src=x onerror=alert(1)>');
    expect(draftTitle(documentFor(createKey(), {}))).toBe('New project');
  });
});

describe('draft keys and lists', () => {
  it('reads keys back and finds the drafts a New form may continue', () => {
    const parent = {object: 'custom:account', id: 'acc1', field: 'custom:account'};
    const keys: DraftKey[] = [createKey('a'), {kind: 'create', object: 'custom:project', draft: 'b', parent}, {kind: 'create', object: 'custom:project', draft: 'c', parent, action: 'log'}, {kind: 'edit', object: 'custom:project', record: 'r1'}];
    for (const key of keys) expect(parseKey(serialiseKey(key))).toEqual(key);
    const drafts = keys.map((key, index) => ({meta: {id: String(index), key: serialiseKey(key)} as never, document: documentFor(key)}));
    expect(createDraftsFor(drafts, 'custom:project').map(draft => draft.meta.id)).toEqual(['0']);
    expect(createDraftsFor(drafts, 'custom:project', parent).map(draft => draft.meta.id)).toEqual(['1']);
    expect(createDraftsFor(drafts, 'custom:project', parent, 'log').map(draft => draft.meta.id)).toEqual(['2']);
    expect(parseKey('not json')).toBeNull();
    expect(draftRoute(drafts[3] as never)).toBe('/o/custom%3Aproject/r1?draft=3');
    expect(draftRoute(drafts[0] as never)).toBe('/o/custom%3Aproject/new?draft=0');
  });
});

describe('draft experience (ADR 0036 §8)', () => {
  const stored = {meta: {id: 'd1', format: 1 as const, member, workspace, key: '', kind: 'edit' as const, object: 'custom:project', changedAt: Date.now() - 6.5 * 86_400_000, expiresAt: 0, size: 1, revision: 1, writer: 't'}, document: documentFor({kind: 'edit', object: 'custom:project', record: 'r'}, {name: {start: 'A', value: '<script>x</script>', type: 'text'}}, {record: {id: 'r', version: 1, name: '<script>x</script>'}})};

  it('keeps the status in a polite live region that names only the state', () => {
    const html = renderToStaticMarkup(<DraftStatus state={{kind: 'saved', at: Date.now()}} />);
    expect(html).toContain('role="status"');
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain('>Draft saved</span>');
    expect(html).toContain('Draft saved at');
    expect(renderToStaticMarkup(<DraftStatus state={{kind: 'idle'}} unavailable />)).toContain('Drafts are unavailable');
  });

  it('offers Restore before Discard, with the expiry notice and the notices once', () => {
    const html = renderToStaticMarkup(<RestorePrompt draft={stored} notices={{expired: 3, dropped: 1}} expiryDays={7} onRestore={() => undefined} onDiscard={() => undefined} />);
    expect(html).toContain('You have an unsaved draft from');
    expect(html).toContain('Expires tomorrow.');
    expect(html).toContain('3 drafts expired');
    expect(html).toContain('1 draft could not be restored');
    expect(html.indexOf('Restore')).toBeLessThan(html.indexOf('Discard'));
  });

  it('asks on Cancel with Keep draft first', () => {
    const html = renderToStaticMarkup(<CancelChoice onKeep={() => undefined} onDiscard={() => undefined} onBack={() => undefined} />);
    expect(html.indexOf('Keep draft')).toBeLessThan(html.indexOf('Discard draft'));
    expect(html).toContain('Back to form');
  });

  it('renders draft values as text, never markup', () => {
    const html = renderToStaticMarkup(<CreateDraftChooser drafts={[stored]} expiryDays={7} onContinue={() => undefined} onStartNew={() => undefined} />);
    expect(html).not.toContain('<script>');
    expect(html).toContain('&lt;script&gt;');
    expect(html).toContain('1 unsaved draft');
    expect(html).toContain('Start new');
  });

  it('shows the Drafts page states: no drafts here, unavailable, and loading', () => {
    const client = createFixtureObjectsClient();
    expect(renderToStaticMarkup(<DraftsContext.Provider value={null}><DraftsPage client={client} onNavigate={() => undefined} /></DraftsContext.Provider>)).toContain('Drafts are not kept here');
    const unavailable: DraftRuntime = {status: 'unavailable', contribution: {store: 'browser', ...limits}};
    expect(renderToStaticMarkup(<DraftsContext.Provider value={unavailable}><DraftsPage client={client} onNavigate={() => undefined} /></DraftsContext.Provider>)).toContain('Drafts are unavailable');
  });

  it('shows the draft settings only to holders of objects.manage', () => {
    expect(renderToStaticMarkup(<DraftSettingsPage workspaceId={workspace} canManage={false} />)).toContain('Permission needed');
    expect(renderToStaticMarkup(<DraftSettingsPage workspaceId={workspace} canManage client={{get: async () => { throw new Error('x'); }, update: async () => { throw new Error('x'); }}} />)).toContain('Loading draft settings');
  });
});
