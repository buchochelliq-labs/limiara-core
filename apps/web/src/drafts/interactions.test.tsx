// @vitest-environment happy-dom
import {IDBFactory} from 'fake-indexeddb';
import {act} from 'react';
import {createRoot, type Root} from 'react-dom/client';
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {createFixtureObjectsClient, ObjectsApiError, type ObjectsClient} from '../objects/api';
import {RecordForm} from '../objects/ObjectPages';
import {DraftAutosave} from './autosave';
import {BrowserDraftDatabase} from './browserStore';
import {BlockedDraft, CancelChoice, CreateDraftChooser, RestorePrompt} from './components';
import {deriveWorkspaceKey, importBrowserKey} from './crypto';
import {DraftsContext, type DraftRuntime} from './runtime';
import type {DraftDocument, DraftKey, DraftStore, StoredDraft} from './types';
import {discardDraft, takeOverDraft} from './useFormDraft';

(globalThis as {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;

const member = '018f0000-0000-7000-8000-000000000101';
const workspace = '018f0000-0000-7000-8000-000000000001';
const apollo = '0000000000000001';
const editKey: DraftKey = {kind: 'edit', object: 'custom:project', record: apollo};
const contribution = {store: 'browser' as const, expiryDays: 7, maxDrafts: 50, maxBytes: 1_048_576};

let container: HTMLDivElement;
let root: Root;
let store: DraftStore;

async function newStore(writer = 'tab-a'): Promise<DraftStore> {
  const key = await deriveWorkspaceKey(await importBrowserKey(new Uint8Array(32).fill(7)), workspace);
  return database.store(member, workspace, key, contribution, writer);
}
let database: BrowserDraftDatabase;

function editDraft(code: string, version = 1): DraftDocument {
  return {format: 1, key: editKey, object: 'custom:project', objectLabel: 'Project', record: {id: apollo, version, name: 'Apollo'}, fields: {'custom:code': {start: 'AP-1', value: code, type: 'text'}}, startedAt: Date.now(), changedAt: Date.now()};
}

beforeEach(async () => {
  database = new BrowserDraftDatabase(new IDBFactory());
  store = await newStore();
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

async function until(check: () => boolean, what: string) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (check()) return;
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); });
  }
  throw new Error(`timed out waiting for ${what}`);
}

const text = () => container.textContent ?? '';
function button(name: string): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll('button')).find(item => item.textContent?.trim() === name);
  if (!found) throw new Error(`no button ${name}: ${text()}`);
  return found;
}
function input(label: string): HTMLInputElement {
  const found = Array.from(container.querySelectorAll('label')).find(item => item.textContent?.trim() === label);
  const control = found?.htmlFor ? document.getElementById(found.htmlFor) : null;
  if (!(control instanceof HTMLInputElement)) throw new Error(`no input ${label}`);
  return control;
}
async function click(element: HTMLElement) {
  await act(async () => { element.click(); });
}

/** Renders an edit form for Apollo with drafts on, counting every save. */
async function renderEdit(client: ObjectsClient, key = 'form') {
  const saves = {update: vi.spyOn(client, 'updateRecord'), create: vi.spyOn(client, 'createRecord')};
  const onSaved = vi.fn();
  const object = await client.describe('custom:project');
  const record = await client.getRecord('custom:project', apollo);
  const runtime: DraftRuntime = {status: 'ready', store, contribution, member, workspace};
  await act(async () => {
    root.render(<DraftsContext.Provider value={runtime}><RecordForm key={key} client={client} object={object} record={record} draft={{key: editKey}} onCancel={() => undefined} onSaved={onSaved} /></DraftsContext.Provider>);
  });
  return {saves, onSaved};
}

describe('draft controls never submit the record form (review P1)', () => {
  it('restores into the form without saving the record', async () => {
    const client = createFixtureObjectsClient();
    await store.put('draft-1', editDraft('AP-9'), 0);
    const {saves, onSaved} = await renderEdit(client);
    await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
    expect(button('Restore').type).toBe('button');
    await click(button('Restore'));
    await until(() => input('Code').value === 'AP-9', 'the restored value');
    expect(saves.update).not.toHaveBeenCalled();
    expect(saves.create).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
  });

  it('discards from the prompt without saving the record, and deletes the draft', async () => {
    const client = createFixtureObjectsClient();
    await store.put('draft-1', editDraft('AP-9'), 0);
    const {saves, onSaved} = await renderEdit(client);
    await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
    await click(button('Discard'));
    await until(() => !text().includes('You have an unsaved draft'), 'the prompt to close');
    expect(saves.update).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
    expect(await store.get('draft-1')).toBeNull();
    expect(input('Code').value).toBe('AP-1');
  });

  it('gives every draft control type="button", so none submits a form around it', () => {
    const submit = vi.fn((event: Event) => event.preventDefault());
    const form = document.createElement('form');
    form.addEventListener('submit', submit);
    container.append(form);
    const formRoot = createRoot(form);
    const draft: StoredDraft = {meta: {id: 'd', format: 1, member, workspace, key: '', kind: 'edit', object: 'custom:project', changedAt: Date.now(), expiresAt: 0, size: 1, revision: 1, writer: 't'}, document: editDraft('AP-9')};
    act(() => {
      formRoot.render(<>
        <RestorePrompt draft={draft} notices={{expired: 0, dropped: 0}} expiryDays={7} onRestore={() => undefined} onDiscard={() => undefined} />
        <CancelChoice onKeep={() => undefined} onDiscard={() => undefined} onBack={() => undefined} />
        <BlockedDraft onKeep={() => undefined} onDiscard={() => undefined} />
        <CreateDraftChooser drafts={[draft]} expiryDays={7} onContinue={() => undefined} onStartNew={() => undefined} />
      </>);
    });
    const buttons = Array.from(form.querySelectorAll('button'));
    expect(buttons.length).toBe(9);
    for (const item of buttons) {
      expect(item.getAttribute('type'), item.textContent ?? '').toBe('button');
      act(() => { item.click(); });
    }
    expect(submit).not.toHaveBeenCalled();
    act(() => formRoot.unmount());
  });
});

async function type(element: HTMLInputElement, value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(element, value);
    element.dispatchEvent(new Event('input', {bubbles: true}));
  });
}

describe('input typed while the form looks for its draft (CI timing)', () => {
  it('is autosaved once the lookup finishes, however slow it is', async () => {
    await store.put('other', {...editDraft('AP-7'), key: {kind: 'edit', object: 'custom:project', record: '0000000000000002'}, record: {id: '0000000000000002', version: 1, name: 'Gemini'}}, 0);
    let release = () => undefined as void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const real = store;
    const slow = {list: async () => { await held; return real.list(); }, put: (...args: Parameters<DraftStore['put']>) => real.put(...args), delete: (id: string) => real.delete(id), get: (id: string) => real.get(id), takeNotices: () => real.takeNotices()} as unknown as DraftStore;
    store = slow;
    const client = createFixtureObjectsClient();
    const {saves} = await renderEdit(client);
    // The member types before the form knows whether a draft exists.
    await type(input('Code'), 'AP-3');
    release();
    await until(() => /Draft saved at/.test(text()), 'the draft to be saved');
    const drafts = await real.list();
    expect(drafts.find(draft => draft.meta.key.includes(apollo))?.document.fields['custom:code']?.value).toBe('AP-3');
    expect(saves.update).not.toHaveBeenCalled();
  });

  it('keeps input typed during the restore prompt unwritten until the member chooses, then drafts it on Discard', async () => {
    await store.put('draft-1', editDraft('AP-9'), 0);
    const {saves} = await renderEdit(createFixtureObjectsClient());
    await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
    await type(input('Code'), 'AP-4');
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 1200)); });
    expect((await store.get('draft-1'))?.document.fields['custom:code']?.value).toBe('AP-9');
    await click(button('Discard'));
    await until(() => /Draft saved at/.test(text()), 'the new draft');
    const drafts = await store.list();
    expect(drafts.map(draft => draft.document.fields['custom:code']?.value)).toEqual(['AP-4']);
    expect(saves.update).not.toHaveBeenCalled();
  });
});

describe('a blocked draft keeps its identity (review P2)', () => {
  it('Discard after a conflict deletes the stored draft', async () => {
    const client = createFixtureObjectsClient();
    await store.put('draft-1', editDraft('AP-9'), 0);
    // Someone else changed the same field since the draft started.
    await client.updateRecord('custom:project', apollo, 1, {'custom:code': 'AP-5'});
    await renderEdit(client);
    await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
    await click(button('Restore'));
    await until(() => text().includes('reviewing changes is coming soon'), 'the conflict notice');
    await click(button('Discard draft'));
    await until(() => !text().includes('reviewing changes is coming soon'), 'the notice to clear');
    expect(await store.get('draft-1')).toBeNull();
    expect(await store.list()).toEqual([]);
  });

  it('Discard after an unavailable or failed check deletes the stored draft', async () => {
    for (const failure of [new ObjectsApiError('missing'), new Error('offline')]) {
      await store.clear();
      await store.put('draft-1', editDraft('AP-9'), 0);
      const client = createFixtureObjectsClient();
      const restored = {...client, getRecord: vi.fn(client.getRecord)};
      await renderEdit(restored, failure.message);
      await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
      restored.getRecord.mockRejectedValue(failure);
      await click(button('Restore'));
      await until(() => text().includes('Discard draft'), 'the blocked draft');
      await click(button('Discard draft'));
      await until(() => !text().includes('Discard draft'), 'the blocked draft to go');
      expect(await store.get('draft-1')).toBeNull();
    }
  });

  it('reports a failed delete instead of starting afresh', async () => {
    const draft = (await store.put('draft-1', editDraft('AP-9'), 0)).ok ? await store.get('draft-1') : null;
    if (!draft) throw new Error('seeded');
    const failing = {...store, delete: vi.fn(async () => { throw new Error('locked'); })} as unknown as DraftStore;
    expect(await discardDraft(failing, {kind: 'blocked', reason: 'conflicts', draft}, null)).toEqual({ok: false});
    expect(await discardDraft(store, {kind: 'blocked', reason: 'conflicts', draft}, null)).toEqual({ok: true});
    expect(await store.get('draft-1')).toBeNull();
  });
});

describe('taking over another tab’s draft (review P3)', () => {
  async function twoTabs(newer: DraftDocument) {
    const mine = new DraftAutosave(store, 'draft-1', 0, () => editDraft('AP-mine'), () => undefined);
    mine.changed();
    await mine.flush();
    const other = await newStore('tab-b');
    expect((await other.put('draft-1', newer, 1)).ok).toBe(true);
    // This tab's next write meets the newer revision.
    mine.changed();
    await mine.flush();
    expect(mine.state).toEqual({kind: 'failed', reason: 'other_tab'});
    return mine;
  }

  it('adopts the newer revision only once its values are applied', async () => {
    const mine = await twoTabs(editDraft('AP-other'));
    const apply = vi.fn();
    expect(await takeOverDraft(store, mine, createFixtureObjectsClient(), undefined, apply)).toBe('taken');
    expect(apply).toHaveBeenCalledWith(expect.objectContaining({status: 'ready', values: {'custom:code': 'AP-other'}}));
    expect(mine.currentRevision).toBe(2);
  });

  it('keeps its revision when the newer draft cannot be restored, so it never overwrites it', async () => {
    const client = createFixtureObjectsClient();
    const cases: [string, ObjectsClient, DraftDocument][] = [
      ['failed', {...client, describe: async () => { throw new Error('offline'); }}, editDraft('AP-other')],
      ['unavailable', {...client, describe: async () => { throw new ObjectsApiError('missing'); }}, editDraft('AP-other')],
      ['conflicts', {...client, getRecord: async (api, id) => ({...(await client.getRecord(api, id)), version: 2, values: {'custom:code': 'AP-5'}})}, editDraft('AP-other')],
    ];
    for (const [expected, source, newer] of cases) {
      await store.clear();
      const mine = await twoTabs(newer);
      const apply = vi.fn();
      expect(await takeOverDraft(store, mine, source, undefined, apply)).toBe(expected);
      expect(apply).not.toHaveBeenCalled();
      expect(mine.currentRevision).toBe(1);
      expect(mine.state).toEqual({kind: 'failed', reason: 'other_tab'});
      // A further change still cannot overwrite the newer draft.
      mine.changed();
      await mine.flush();
      expect((await store.get('draft-1'))?.document.fields['custom:code']?.value).toBe('AP-other');
    }
  });

  it('starts a new draft from this tab’s values when the other tab’s draft is gone', async () => {
    const mine = await twoTabs(editDraft('AP-other'));
    await store.delete('draft-1');
    expect(await takeOverDraft(store, mine, createFixtureObjectsClient(), undefined, vi.fn())).toBe('gone');
    await mine.flush();
    expect((await store.get('draft-1'))?.document.fields['custom:code']?.value).toBe('AP-mine');
  });
});

describe('a value the form opens with already changed (ADR 0037 §2, review P2)', () => {
  it('is kept in the draft, so resuming it restores the value', async () => {
    const client = createFixtureObjectsClient();
    const object = await client.describe('custom:project');
    const record = await client.getRecord('custom:project', apollo);
    const runtime: DraftRuntime = {status: 'ready', store, contribution, member, workspace};
    // "Close as Won" on the path opens the edit form with the closing value already chosen.
    await act(async () => {
      root.render(<DraftsContext.Provider value={runtime}><RecordForm client={client} object={object} record={record} changes={{'custom:status': 'won'}} draft={{key: editKey}} onCancel={() => undefined} onSaved={() => undefined} /></DraftsContext.Provider>);
    });
    await type(input('Code'), 'AP-9');
    await until(() => /Draft saved at/.test(text()), 'the draft to be saved');
    const stored = (await store.list()).find(draft => draft.meta.key.includes(apollo));
    expect(stored?.document.fields['custom:status']?.value).toBe('won');
    expect(stored?.document.fields['custom:code']?.value).toBe('AP-9');
  });
});


describe('text typed into a text list but not yet added (ADR 0035 G7, review P2)', () => {
  it('is autosaved with the draft, and restores into the input', async () => {
    const client = createFixtureObjectsClient();
    const described = await client.describe('custom:project');
    const tags = {apiName: 'custom:tags', label: 'Tags', description: null, type: 'text_list' as const, origin: 'workspace' as const, required: false, unique: false, indexed: true, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1};
    const object = {...described, fields: [...described.fields, tags]};
    vi.spyOn(client, 'describe').mockResolvedValue(object);
    const record = await client.getRecord('custom:project', apollo);
    const runtime: DraftRuntime = {status: 'ready', store, contribution, member, workspace};
    const render = async (key: string) => act(async () => {
      root.render(<DraftsContext.Provider value={runtime}><RecordForm key={key} client={client} object={object} record={record} draft={{key: editKey}} onCancel={() => undefined} onSaved={() => undefined} /></DraftsContext.Provider>);
    });
    await render('first');
    await until(() => !text().includes('Checking for a draft'), 'the draft check');
    // The member types a value and leaves without pressing Enter.
    await type(input('Tags'), 'Partner');
    await until(() => /Draft saved at/.test(text()), 'the draft to be saved');
    const saved = (await store.list()).find(draft => draft.meta.key.includes(apollo));
    expect(saved?.document.fields['custom:tags']?.value).toEqual(['Partner']);

    // Reopening the form offers the draft, and restoring puts the text back.
    await act(async () => root.unmount());
    root = createRoot(container);
    await render('second');
    await until(() => text().includes('You have an unsaved draft'), 'the restore prompt');
    await click(button('Restore'));
    await until(() => input('Tags').value === 'Partner', 'the restored text');
  });
});
