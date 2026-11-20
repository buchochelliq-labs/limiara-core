// @vitest-environment happy-dom
import {act} from 'react';
import {createRoot, type Root} from 'react-dom/client';
import {afterEach, beforeEach, describe, expect, it, vi} from 'vitest';
import {createFixtureObjectsClient, type ObjectDescription, type ObjectField, type ObjectRecord, type ObjectsClient} from './api';
import {PathSettings} from './ObjectManager';
import {ObjectRecordPage} from './ObjectPages';
import {pathFieldOf, pathSteps, RecordPath} from './RecordPath';

(globalThis as {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;

const access = {read: true, create: true, edit: true, delete: true, view_all: false, modify_all: false};
const stage: ObjectField = {
  apiName: 'custom:stage', label: 'Stage', description: null, type: 'picklist', origin: 'workspace', required: false, unique: false, indexed: true,
  options: [
    {value: 'prospecting', label: 'Prospecting', active: true, default: true, outcome: 'open', weight: 10},
    {value: 'proposal', label: 'Proposal', active: true, default: false, outcome: 'open', weight: 60},
    {value: 'retired', label: 'Retired', active: false, default: false},
    {value: 'won', label: 'Won', active: true, default: false, outcome: 'positive', weight: 100},
    {value: 'lost', label: 'Lost', active: true, default: false, outcome: 'negative', weight: 0},
  ],
  referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1,
};
const deal: ObjectDescription = {
  apiName: 'custom:deal', label: 'Deal', pluralLabel: 'Deals', description: null, origin: 'workspace', keyPrefix: '001', nameLabel: 'Deal name', inNavigation: true,
  sharingModel: 'read_write', grantHierarchy: true, trackHistory: false, pathField: 'custom:stage', version: 3, access, fields: [stage],
};
const record = (value: string | null): ObjectRecord => ({
  id: '0010000000000001', name: 'Acme renewal', owner: null, createdAt: '2026-11-20T10:00:00Z', createdBy: null, updatedAt: '2026-11-20T10:00:00Z', updatedBy: null,
  version: 2, isDeleted: false, archivedAt: null, values: value === null ? {} : {'custom:stage': value}, related: {}, relatedObjects: {},
} as ObjectRecord);

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const steps = () => [...container.querySelectorAll('.record-path__steps li')].map(item => ({
  label: item.querySelector('.record-path__step')?.childNodes[0]?.textContent,
  current: item.getAttribute('aria-current') === 'step',
  complete: item.getAttribute('data-complete') === 'true',
}));
const button = (name: string) => [...container.querySelectorAll('button')].find(item => item.textContent?.startsWith(name));

describe('the record path (ADR 0037 §2)', () => {
  it('lists open steps in order, then closing values, and leaves out inactive ones the record does not hold', () => {
    expect(pathSteps(stage, 'proposal')).toEqual({open: stage.options.slice(0, 2), closed: stage.options.slice(3)});
    // A record keeps showing a retired value it still holds.
    expect(pathSteps(stage, 'retired').open.map(option => option.value)).toEqual(['prospecting', 'proposal', 'retired']);
  });

  it('only uses a readable single-value picklist', () => {
    expect(pathFieldOf(deal)?.apiName).toBe('custom:stage');
    expect(pathFieldOf({...deal, pathField: null})).toBeUndefined();
    expect(pathFieldOf({...deal, fields: [{...stage, access: {read: false, edit: false}}]})).toBeUndefined();
    expect(pathFieldOf({...deal, fields: [{...stage, type: 'multi_picklist'}]})).toBeUndefined();
  });

  it('marks the current and completed steps and offers the next one', () => {
    const onMove = vi.fn();
    act(() => root.render(<RecordPath object={deal} field={stage} record={record('proposal')} canEdit busy={false} onMove={onMove} onClose={vi.fn()} />));
    expect(container.querySelector('nav')?.getAttribute('aria-label')).toBe('Deal stage path');
    expect(steps()).toEqual([
      {label: 'Prospecting', current: false, complete: true},
      {label: 'Proposal', current: true, complete: false},
      {label: 'Won', current: false, complete: false},
      {label: 'Lost', current: false, complete: false},
    ]);
    expect(container.textContent).toContain('Stage: Proposal (60%)');
    // The last open step has no next one: closing goes through the form.
    expect(button('Mark as')).toBeUndefined();
    act(() => button('Prospecting')?.click());
    expect(onMove).toHaveBeenCalledWith('prospecting');
  });

  it('opens the form to close, and shows a closed record as closed', () => {
    const onClose = vi.fn();
    act(() => root.render(<RecordPath object={deal} field={stage} record={record('prospecting')} canEdit busy={false} onMove={vi.fn()} onClose={onClose} />));
    expect(button('Mark as Proposal')).toBeDefined();
    act(() => button('Lost')?.click());
    expect(onClose).toHaveBeenCalledWith(expect.objectContaining({value: 'lost'}));
    act(() => root.render(<RecordPath object={deal} field={stage} record={record('won')} canEdit busy={false} onMove={vi.fn()} onClose={onClose} />));
    expect(container.textContent).toContain('Closed as Won.');
    expect(steps().filter(step => step.complete)).toEqual([]);
  });

  it('is read-only without edit access, and starts a record with no value', () => {
    act(() => root.render(<RecordPath object={deal} field={stage} record={record('proposal')} canEdit={false} busy={false} onMove={vi.fn()} onClose={vi.fn()} />));
    expect(container.querySelectorAll('button')).toHaveLength(0);
    const onMove = vi.fn();
    act(() => root.render(<RecordPath object={deal} field={stage} record={record(null)} canEdit busy={false} onMove={onMove} onClose={vi.fn()} />));
    expect(container.textContent).toContain('No stage yet.');
    act(() => button('Start at Prospecting')?.click());
    expect(onMove).toHaveBeenCalledWith('prospecting');
  });
});

describe('the path on the generic record page', () => {
  const apollo = '0000000000000001'; // Open
  const gemini = '0000000000000002'; // Won, a closed value
  async function page(client: ObjectsClient, id: string) {
    await act(async () => { root.render(<ObjectRecordPage client={client} apiName="custom:project" recordId={id} onNavigate={vi.fn()} />); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  }
  const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

  it('moves the record to an open value with its version, and says so', async () => {
    const client = createFixtureObjectsClient();
    await page(client, gemini);
    expect(container.textContent).toContain('Closed as Won.');
    const update = vi.spyOn(client, 'updateRecord');
    await act(async () => { button('Open')?.click(); });
    await settle();
    expect(update).toHaveBeenCalledWith('custom:project', gemini, 1, {'custom:status': 'open'});
    expect(container.textContent).toContain('Status is now Open.');
    expect(container.textContent).toContain('Status: Open (20%)');
  });

  it('opens the form with a closing value preset instead of saving at once', async () => {
    const client = createFixtureObjectsClient();
    await page(client, apollo);
    const update = vi.spyOn(client, 'updateRecord');
    await act(async () => { button('Won')?.click(); });
    await settle();
    expect(update).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain('Close as Won');
  });

  it('says what was saved when the member picks another value in the closing form', async () => {
    const client = createFixtureObjectsClient();
    await page(client, apollo);
    await act(async () => { button('Won')?.click(); });
    await settle();
    const status = [...document.querySelectorAll('select')].find(select => [...select.options].some(option => option.value === 'won')) as HTMLSelectElement;
    expect(status.value).toBe('won');
    await act(async () => {
      status.value = 'open';
      status.dispatchEvent(new Event('change', {bubbles: true}));
    });
    const save = [...document.querySelectorAll('button')].find(item => item.textContent?.trim() === 'Save') as HTMLButtonElement;
    await act(async () => { save.click(); });
    await settle();
    expect(container.textContent).toContain('Status is now Open.');
    expect(container.textContent).not.toContain('was closed as Won');
  });

  it('keeps the record and explains a conflict', async () => {
    const client = createFixtureObjectsClient();
    await page(client, gemini);
    // Someone else saves first, so the page's version is stale.
    await client.updateRecord('custom:project', gemini, 1, {'custom:code': 'GE-2'});
    await act(async () => { button('Open')?.click(); });
    await settle();
    expect(container.textContent).toContain('This record changed since you opened it.');
    expect(container.textContent).toContain('Closed as Won.');
  });
});

describe('the Object Manager path setting', () => {
  it('saves a picklist as the path, and None clears it', async () => {
    const updateObject = vi.fn(async () => deal);
    const onSaved = vi.fn();
    act(() => root.render(<PathSettings client={{updateObject}} object={{...deal, pathField: null}} onSaved={onSaved} />));
    const select = container.querySelector('select') as HTMLSelectElement;
    expect([...select.options].map(option => option.text)).toEqual(['None', 'Stage']);
    act(() => {
      select.value = 'custom:stage';
      select.dispatchEvent(new Event('change', {bubbles: true}));
    });
    await act(async () => { button('Save path')?.click(); });
    expect(updateObject).toHaveBeenCalledWith('custom:deal', 3, {path_field: 'custom:stage'});
    expect(onSaved).toHaveBeenCalledWith('Records of deals now show Stage as their path.');
  });

  it('shows a plugin object\'s choice without changing it', () => {
    act(() => root.render(<PathSettings client={{updateObject: vi.fn()}} object={{...deal, origin: 'plugin'}} onSaved={vi.fn()} />));
    expect((container.querySelector('select') as HTMLSelectElement).disabled).toBe(true);
    expect(button('Save path')).toBeUndefined();
  });
});
