// @vitest-environment happy-dom
import {act} from 'react';
import {createRoot, type Root} from 'react-dom/client';
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {createFixtureObjectsClient, type ObjectDescription, type ObjectsClient} from './api';
import {ObjectHome} from './ObjectPages';
import {BOARD_PAGE, boardColumns, boardFieldOf, RecordBoard} from './RecordBoard';

(globalThis as {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  window.localStorage.clear();
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

const settle = () => act(async () => { for (let i = 0; i < 3; i += 1) await new Promise(resolve => setTimeout(resolve, 0)); });
const column = (label: string) => [...container.querySelectorAll('.record-board__column')].find(item => item.querySelector('h3')?.textContent?.startsWith(label));
const cards = (label: string) => [...(column(label)?.querySelectorAll('.record-board__open') ?? [])].map(item => item.textContent);
const button = (name: string) => [...container.querySelectorAll('button')].find(item => item.textContent?.trim() === name);

async function board(client: ObjectsClient, onOpen = vi.fn()) {
  const object = await client.describe('custom:project');
  const field = boardFieldOf(object)!;
  await act(async () => { root.render(<RecordBoard client={client} object={object} field={field} onOpen={onOpen} />); });
  await settle();
  return {object, onOpen};
}

async function moveTo(card: string, target: string) {
  const item = [...container.querySelectorAll('.record-board__card')].find(element => element.querySelector('.record-board__open')?.textContent === card);
  const select = item?.querySelector('select') as HTMLSelectElement;
  await act(async () => {
    select.value = target;
    select.dispatchEvent(new Event('change', {bubbles: true}));
  });
  await settle();
}

describe('the board view (ADR 0037 §3)', () => {
  it('groups by the path field, or the first readable picklist, and hides closed columns unless asked', async () => {
    const client = createFixtureObjectsClient();
    const object = await client.describe('custom:project');
    expect(boardFieldOf(object)?.apiName).toBe('custom:status');
    const status = object.fields.find(field => field.apiName === 'custom:status')!;
    expect(boardFieldOf({...object, pathField: null})?.apiName).toBe('custom:status');
    const none: ObjectDescription = {...object, pathField: null, fields: object.fields.filter(field => field.type !== 'picklist')};
    expect(boardFieldOf(none)).toBeUndefined();
    expect(boardColumns(status, false).map(option => option.value)).toEqual(['open']);
    expect(boardColumns(status, true).map(option => option.value)).toEqual(['open', 'won']);
  });

  it('loads each column through the equality filter and shows closed ones on request', async () => {
    const client = createFixtureObjectsClient();
    const list = vi.spyOn(client, 'listRecords');
    await board(client);
    expect(list).toHaveBeenCalledWith('custom:project', expect.objectContaining({filters: {'custom:status': 'open'}, limit: BOARD_PAGE}), expect.anything());
    expect(cards('Open')).toEqual(['Apollo']);
    expect(column('Won')).toBeUndefined();
    const toggle = container.querySelector('.record-board__toolbar input') as HTMLInputElement;
    await act(async () => { toggle.click(); });
    await settle();
    expect(cards('Won')).toEqual(['Gemini']);
  });

  it('moves a card with its version and announces it', async () => {
    const client = createFixtureObjectsClient();
    await board(client);
    const toggle = container.querySelector('.record-board__toolbar input') as HTMLInputElement;
    await act(async () => { toggle.click(); });
    await settle();
    const update = vi.spyOn(client, 'updateRecord');
    await moveTo('Apollo', 'won');
    expect(update).toHaveBeenCalledWith('custom:project', '0000000000000001', 1, {'custom:status': 'won'});
    expect(cards('Open')).toEqual([]);
    expect(cards('Won')).toEqual(['Apollo', 'Gemini']);
    expect(container.textContent).toContain('Apollo moved to Won.');
  });

  it('leaves a card where it was on a conflict and says why', async () => {
    const client = createFixtureObjectsClient();
    await board(client);
    await client.updateRecord('custom:project', '0000000000000001', 1, {'custom:code': 'AP-2'});
    await moveTo('Apollo', 'won');
    expect(cards('Open')).toEqual(['Apollo']);
    expect(container.textContent).toContain('Apollo changed since the board loaded, so it was not moved.');
  });

  it('pages a long column and opens a card\'s record', async () => {
    const client = createFixtureObjectsClient();
    for (let index = 0; index < BOARD_PAGE + 2; index += 1) await client.createRecord('custom:project', {name: `Extra ${String(index).padStart(2, '0')}`, 'custom:status': 'open'});
    const {onOpen} = await board(client);
    expect(cards('Open')).toHaveLength(BOARD_PAGE);
    expect(column('Open')?.querySelector('.record-board__count')?.textContent).toBe(`${BOARD_PAGE}+`);
    await act(async () => { button('Load more')?.click(); });
    await settle();
    expect(cards('Open')).toHaveLength(BOARD_PAGE + 3);
    expect(button('Load more')).toBeUndefined();
    await act(async () => { button('Apollo')?.click(); });
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({id: '0000000000000001'}));
  });

  it('is read-only without edit access', async () => {
    const client = createFixtureObjectsClient();
    const object = await client.describe('custom:project');
    await act(async () => { root.render(<RecordBoard client={client} object={{...object, access: {...object.access, edit: false}}} field={boardFieldOf(object)!} onOpen={vi.fn()} />); });
    await settle();
    expect(container.querySelector('.record-board__card select')).toBeNull();
    expect(container.querySelector('.record-board__card')?.getAttribute('draggable')).toBe('false');
  });
});

describe('the object home view switch', () => {
  it('switches to the board and remembers the choice per object', async () => {
    const client = createFixtureObjectsClient();
    await act(async () => { root.render(<ObjectHome client={client} apiName="custom:project" onNavigate={vi.fn()} />); });
    await settle();
    expect(container.querySelector('table')).not.toBeNull();
    await act(async () => { button('Board')?.click(); });
    await settle();
    expect(container.querySelector('.record-board')).not.toBeNull();
    expect(container.querySelector('table')).toBeNull();
    expect(window.localStorage.getItem('limiara.objects.view.custom:project')).toBe('board');
    // A later visit opens on the board.
    act(() => root.unmount());
    root = createRoot(container);
    await act(async () => { root.render(<ObjectHome client={client} apiName="custom:project" onNavigate={vi.fn()} />); });
    await settle();
    expect(container.querySelector('.record-board')).not.toBeNull();
  });
});
