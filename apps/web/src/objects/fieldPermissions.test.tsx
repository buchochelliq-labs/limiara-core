import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {canCompleteAction, canEditField, canReadField, canSetField, createFixtureObjectsClient, createObjectsApiClient, fieldFromApi, readableFields, type ObjectDescription, type ObjectField} from './api';
import {FieldPermissionsForm} from './ObjectManager';
import {editableFields, RecordForm, valuesToSend} from './ObjectPages';

const workspace = '018f0000-0000-7000-8000-000000000001';
const noop = () => undefined;

function respond(status: number, body: unknown = {}) {
  return vi.fn(async () => new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
}

function field(apiName: string, label: string, extra: Partial<ObjectField> = {}): ObjectField {
  return {apiName, label, description: null, type: 'text', origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1, ...extra};
}

const budget = field('custom:budget', 'Budget', {type: 'currency', readPermission: 'projects.budget.read', writePermission: 'projects.budget.write', access: {read: true, edit: false}});
const salary = field('custom:salary', 'Salary', {type: 'number', readPermission: 'hr.salary.read', access: {read: false, edit: false}});
const code = field('custom:code', 'Code');

const project: ObjectDescription = {
  apiName: 'custom:project', label: 'Project', pluralLabel: 'Projects', description: null, origin: 'workspace', keyPrefix: '000', nameLabel: 'Project name', inNavigation: true, sharingModel: 'read_write', grantHierarchy: true, trackHistory: false, version: 1,
  access: {read: true, create: true, edit: true, delete: true, view_all: false, modify_all: false},
  fields: [code, budget, salary],
};

describe('field-level permissions (G5)', () => {
  it('reads permissions and the member access from the API, defaulting to full access', () => {
    const mapped = fieldFromApi({api_name: 'custom:budget', label: 'Budget', description: null, field_type: 'currency', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, delete_behaviour: null, read_permission: 'projects.budget.read', write_permission: 'projects.budget.write', access: {read: true, edit: false}, version: 2});
    expect(mapped.readPermission).toBe('projects.budget.read');
    expect(mapped.writePermission).toBe('projects.budget.write');
    expect(mapped.access).toEqual({read: true, edit: false});
    const open = fieldFromApi({api_name: 'custom:code', label: 'Code', description: null, field_type: 'text', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, delete_behaviour: null, version: 1});
    expect(open.access).toEqual({read: true, edit: true});
    expect(open.readPermission).toBeNull();
    // Edit never outlives read.
    expect(fieldFromApi({...{api_name: 'x', label: 'X', description: null, field_type: 'text', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, delete_behaviour: null, version: 1}, access: {read: false, edit: true}}).access).toEqual({read: false, edit: false});
  });

  it('decides what the member sees and changes', () => {
    expect(canReadField(code) && canEditField(code)).toBe(true);
    expect(canReadField(budget)).toBe(true);
    expect(canEditField(budget)).toBe(false);
    expect(canReadField(salary)).toBe(false);
    expect(readableFields(project.fields).map(item => item.apiName)).toEqual(['custom:code', 'custom:budget']);
    expect(editableFields(project).map(item => item.apiName)).toEqual(['custom:code', 'custom:budget']);
  });

  it('leaves unreadable fields out of forms and shows unwritable ones read-only', () => {
    const record = {id: '0000000000000001', name: 'Apollo', owner: null, createdAt: '2026-09-01T09:00:00Z', createdBy: null, updatedAt: '2026-09-01T09:00:00Z', updatedBy: null, version: 1, isDeleted: false, archivedAt: null, values: {'custom:code': 'AP-1', 'custom:budget': 1250}, related: {}};
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={project} record={record} onCancel={noop} onSaved={noop} />);
    expect(markup).not.toContain('Salary');
    expect(markup).toContain('Budget');
    expect(markup).toMatch(/<input[^>]*readOnly=""[^>]*value="1,250.00"/);
    expect(markup).toContain('You can see this field but not change it.');
    expect(markup).toContain('Code');
  });

  it('loads the catalog, lists pickers with labels, and saves both permissions', async () => {
    const fetcher = respond(200, {data: [{id: 'projects.budget.read', description: 'Read project budgets'}, {id: 7}]});
    const client = createObjectsApiClient(workspace, fetcher);
    expect(await client.fieldPermissions()).toEqual([{id: 'projects.budget.read', description: 'Read project budgets'}]);
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/field-permissions`);
    const update = respond(200, {api_name: 'custom:budget', label: 'Budget', description: null, field_type: 'currency', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, delete_behaviour: null, read_permission: null, version: 3});
    await createObjectsApiClient(workspace, update).updateField('custom:project', 'custom:budget', 2, {read_permission: null, write_permission: 'projects.budget.write'});
    expect(JSON.parse(String(vi.mocked(update).mock.calls[0]?.[1]?.body))).toEqual({read_permission: null, write_permission: 'projects.budget.write', expected_version: 2});
  });

  it('refusals of unwritable fields carry the fields they name', async () => {
    const fetcher = respond(403, {code: 'records.field_not_writable', errors: [{field: 'custom:budget', code: 'records.field_not_writable', detail: 'you may read this field but not change it'}]});
    await expect(createObjectsApiClient(workspace, fetcher).updateRecord('custom:project', '0001', 1, {'custom:budget': 1})).rejects.toMatchObject({reason: 'denied', code: 'records.field_not_writable', issues: [{field: 'custom:budget'}]});
  });

  it('shows a loading state, then labelled pickers that keep a permission the catalog lost', () => {
    const client = createFixtureObjectsClient();
    const loading = renderToStaticMarkup(<FieldPermissionsForm client={client} object={project} field={budget} onDone={noop} />);
    expect(loading).toContain('Loading permissions');
    expect(loading).toContain('aria-busy="true"');
    const ready = renderToStaticMarkup(<FieldPermissionsForm client={client} object={project} field={budget} onDone={noop} initialChoices={[{id: 'projects.budget.read', description: 'Read project budgets'}]} />);
    expect(ready).toMatch(/<label[^>]*>Read permission<\/label>/);
    expect(ready).toMatch(/<label[^>]*>Write permission<\/label>/);
    expect(ready).toContain('Everyone who can read the record');
    expect(ready).toContain('<option value="projects.budget.read" selected="">projects.budget.read (Read project budgets)</option>');
    expect(ready).toContain('<option value="projects.budget.write" selected="">projects.budget.write</option>');
  });

  it('gives computed fields a read permission only, and leaves a feature\'s fields to it', () => {
    const formula = field('custom:margin', 'Margin', {type: 'formula', formula: 'custom:budget / 2', formulaType: 'number'});
    const computed = renderToStaticMarkup(<FieldPermissionsForm client={createFixtureObjectsClient()} object={project} field={formula} onDone={noop} initialChoices={[]} />);
    expect(computed).toContain('Read permission');
    expect(computed).not.toContain('Write permission');
    const owned = renderToStaticMarkup(<FieldPermissionsForm client={createFixtureObjectsClient()} object={project} field={{...budget, origin: 'plugin'}} onDone={noop} initialChoices={[]} />);
    expect(owned).toContain('Set by its feature');
    expect(owned).toMatch(/<select[^>]*disabled=""/);
    expect(owned).not.toContain('Save access');
  });

  it('offers a quick action only when the member may set every value it presets (Codex P2 on #246)', () => {
    expect(canSetField(project, 'name')).toBe(true);
    expect(canSetField(project, 'custom:code')).toBe(true);
    expect(canSetField(project, 'custom:budget')).toBe(false);
    expect(canSetField(project, 'custom:salary')).toBe(false);
    // A member's description leaves out fields they cannot read.
    expect(canSetField({fields: [code]}, 'custom:salary')).toBe(false);
    expect(canCompleteAction({presets: {}, targetField: null}, project)).toBe(true);
    expect(canCompleteAction({presets: {'custom:code': 'X', name: 'Call'}, targetField: null}, project)).toBe(true);
    expect(canCompleteAction({presets: {'custom:budget': 1}, targetField: null}, project)).toBe(false);
    expect(canCompleteAction({presets: {'custom:salary': 1}, targetField: null}, project)).toBe(false);
    // A create action's reference to the record must be settable too.
    const line: ObjectDescription = {...project, apiName: 'custom:line', fields: [field('custom:project', 'Project', {type: 'lookup', referenceObject: 'custom:project', access: {read: true, edit: false}}), code]};
    expect(canCompleteAction({presets: {}, targetField: 'custom:project'}, line)).toBe(false);
    expect(canCompleteAction({presets: {}, targetField: 'custom:project'}, {...line, fields: [code, field('custom:project', 'Project', {type: 'lookup'})]})).toBe(true);
  });

  it('never sends a value the member may not change, presets included', () => {
    const shown = [code, budget];
    const draft = {name: 'Apollo', 'custom:code': 'AP-2', 'custom:budget': '9'};
    const initial = {name: 'Apollo', 'custom:code': 'AP-1', 'custom:budget': '1'};
    const writable = (apiName: string) => canSetField(project, apiName);
    expect(valuesToSend({record: true, draft, initial, shown, showsName: true, always: {'custom:budget': 5, 'custom:salary': 1, 'custom:code': 'P'}, writable})).toEqual({'custom:code': 'AP-2'});
    expect(valuesToSend({record: false, draft, initial, shown, showsName: true, always: {'custom:salary': 1}, writable})).toEqual({name: 'Apollo', 'custom:code': 'AP-2'});
    // Without a check, every preset still goes, as before.
    expect(valuesToSend({record: true, draft: initial, initial, shown: [code], showsName: false, always: {'custom:budget': 5}})).toEqual({'custom:budget': 5});
  });
});
