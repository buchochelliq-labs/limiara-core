import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {AppShell} from '../shell/AppShell';
import {shellFixture} from '../shell/data';
import {createFixtureObjectsClient, createObjectsApiClient, formatValue, ObjectsApiError, quickActionFromApi, recordFromApi, type ObjectDescription, type QuickAction} from './api';
import {AllowlistForm, allowlistChoices, LookupFilterForm, suggestName, ValidationRules} from './ObjectManager';
import {RecordForm, RecordModal, RelatedLists, valuesToSend} from './ObjectPages';
import {creatable, GlobalNew} from './GlobalNew';
import {SharingPanel} from './SharingPanel';
import {actionFields, actionTargets, QuickActionForm, QuickActions, targetLabel} from './QuickActions';
import {MembersPage} from './MembersPage';
import {HistoryPanel} from './HistoryPanel';
import {formatCell, QueryConsole, queryProblem, QueryResults} from './QueryConsole';
import {AutomationForm, automationDoes, AutomationJobs, AutomationRuns, Automations, automationWhen} from './Automations';
import {GlobalSearch, searchable, searchOptions, searchResultsRoute, SearchResultsPage} from './GlobalSearch';
import {barExtents, ReportChartView, reportRoute, ReportsPage} from './Reports';

const workspace = '018f0000-0000-7000-8000-000000000001';
const noop = () => undefined;

function respond(status: number, body: unknown = {}) {
  return vi.fn(async () => new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
}

const project: ObjectDescription = {
  apiName: 'custom:project', label: 'Project', pluralLabel: 'Projects', description: null, origin: 'workspace', keyPrefix: '000', nameLabel: 'Project name', inNavigation: true, sharingModel: 'private', grantHierarchy: true, trackHistory: true, version: 1,
  access: {read: true, create: true, edit: true, delete: true, view_all: false, modify_all: false},
  fields: [
    {apiName: 'custom:status', label: 'Status', description: null, type: 'picklist', origin: 'workspace', required: true, unique: false, indexed: true, options: [{value: 'open', label: 'Open', active: true, default: true}, {value: 'lost', label: 'Lost', active: false, default: false}], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1},
    {apiName: 'custom:code', label: 'Code', description: null, type: 'text', origin: 'workspace', required: false, unique: true, indexed: true, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1},
    {apiName: 'custom:notes', label: 'Notes <i>', description: null, type: 'long_text', origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1},
  ],
};

describe('workspace objects client', () => {
  it('addresses objects by API name and keeps record values apart from standard fields', async () => {
    const fetcher = respond(200, {id: '000abc', object: 'custom:project', name: 'Apollo', owner: {id: 'u1', name: 'Grace'}, created_at: '2026-01-01T00:00:00Z', created_by: {id: 'u1'}, updated_at: '2026-01-02T00:00:00Z', updated_by: {id: 'u1'}, touched_at: '2026-01-02T00:00:00Z', version: 3, archived_at: null, is_deleted: false, deleted_at: null, 'custom:code': 'AP-1'});
    const record = await createObjectsApiClient(workspace, fetcher).getRecord('custom:project', '000abc');
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/records/000abc`);
    expect(record.values).toEqual({'custom:code': 'AP-1'});
    expect(record.version).toBe(3);
    expect(record.owner).toEqual({id: 'u1', name: 'Grace'});
  });

  it('sends the version with partial updates and maps problems to reasons', async () => {
    const fetcher = respond(200, recordFromApiShape());
    await createObjectsApiClient(workspace, fetcher).updateRecord('custom:project', '000abc', 4, {'custom:code': null});
    const [, init] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(init?.method).toBe('PATCH');
    expect(JSON.parse(String(init?.body))).toEqual({'custom:code': null, expected_version: 4});
    await expect(createObjectsApiClient(workspace, respond(404)).describe('custom:x')).rejects.toMatchObject({reason: 'missing'});
    await expect(createObjectsApiClient(workspace, respond(403)).describe('custom:x')).rejects.toMatchObject({reason: 'denied'});
    await expect(createObjectsApiClient(workspace, respond(409, {code: 'records.duplicate_value'})).createRecord('custom:x', {name: 'A'})).rejects.toMatchObject({reason: 'conflict', code: 'records.duplicate_value'});
    await expect(createObjectsApiClient(workspace, respond(400, {code: 'validation.failed', errors: [{field: 'name', code: 'records.name_required', detail: 'enter a name'}]})).createRecord('custom:x', {})).rejects.toMatchObject({reason: 'invalid', issues: [{field: 'name'}]});
    await expect(createObjectsApiClient(workspace, respond(503)).listObjects()).rejects.toBeInstanceOf(ObjectsApiError);
  });

  it('lists records with only the query values that are set', async () => {
    const fetcher = respond(200, {data: [], next_cursor: null});
    await createObjectsApiClient(workspace, fetcher).listRecords('custom:project', {search: 'apo', status: 'deleted', cursor: undefined, limit: 50});
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/records?search=apo&status=deleted&limit=50`);
  });

  it('runs create, update, delete and undelete against the fixture engine', async () => {
    const client = createFixtureObjectsClient();
    const created = await client.createRecord('custom:project', {name: 'Vega', 'custom:code': 'VE-1'});
    expect(created.values['custom:status']).toBe('open');
    const updated = await client.updateRecord('custom:project', created.id, created.version, {'custom:code': null});
    expect(updated.values['custom:code']).toBeUndefined();
    await expect(client.updateRecord('custom:project', created.id, created.version, {name: 'Stale'})).rejects.toMatchObject({code: 'records.version_conflict'});
    const deleted = await client.deleteRecord('custom:project', created.id, updated.version);
    await expect(client.getRecord('custom:project', created.id)).rejects.toMatchObject({reason: 'missing'});
    expect((await client.listRecords('custom:project', {status: 'deleted'})).records.map(record => record.name)).toEqual(['Vega']);
    await client.undeleteRecord('custom:project', created.id, deleted.version);
    expect((await client.getRecord('custom:project', created.id)).name).toBe('Vega');
  });

  it('formats values by field type as plain text', () => {
    const [status, code] = project.fields;
    if (!status || !code) throw new Error('fixture');
    expect(formatValue(status, 'open')).toBe('Open');
    expect(formatValue(status, 'unknown')).toBe('unknown');
    expect(formatValue({...code, type: 'boolean'}, true)).toBe('Yes');
    expect(formatValue({...code, type: 'percent'}, 12)).toBe('12%');
    expect(formatValue(code, null)).toBe('');
    const lookup = {...code, type: 'lookup' as const, referenceObject: 'custom:client'};
    expect(formatValue(lookup, '000x', {'000x': 'Analytical Engines'})).toBe('Analytical Engines');
    expect(formatValue(lookup, '000y', {})).toBe('000y');
    const master = {...code, type: 'master_detail' as const, referenceObject: 'custom:client'};
    expect(formatValue(master, '000x', {'000x': 'Analytical Engines'})).toBe('Analytical Engines');
  });

  it('keeps a detail with its master when editing (ADR 0025)', () => {
    const [, code] = project.fields;
    if (!code) throw new Error('fixture');
    const master = {...code, apiName: 'custom:client', label: 'Client', type: 'master_detail' as const, required: true, unique: false, referenceObject: 'custom:client'};
    const detail = {...project, fields: [master]};
    const record = recordFromApi({id: '000a', name: 'Line', owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 1, is_deleted: false, 'custom:client': '000x', related: {'000x': 'Analytical Engines'}});
    const editing = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={detail} record={record} onCancel={noop} onSaved={noop} />);
    expect(editing).toContain('A record stays with its master.');
    expect(editing).toMatch(/value="Analytical Engines"[^>]*readOnly|readOnly[^>]*value="Analytical Engines"/i);
    const creating = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={detail} onCancel={noop} onSaved={noop} />);
    expect(creating).not.toContain('A record stays with its master.');
  });

  it('suggests API names an administrator can accept', () => {
    expect(suggestName('Project stage')).toBe('project_stage');
    expect(suggestName('2nd Café — Notes!')).toBe('nd_cafe_notes');
    expect(suggestName('   ')).toBe('');
  });
});

describe('record sharing (ADR 0016)', () => {
  it('reads rights and shares from the API and sends shares and transfers', async () => {
    const record = await createObjectsApiClient(workspace, respond(200, {id: '000a', name: 'Deal', owner: {id: 'u1'}, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 2, is_deleted: false, access: {read: true, edit: true, delete: false, share: false, transfer: false}})).getRecord('custom:deal', '000a');
    expect(record.rights).toEqual({read: true, edit: true, delete: false, share: false, transfer: false});
    expect(record.values).toEqual({});
    const sharing = await createObjectsApiClient(workspace, respond(200, {owner: {id: 'u1', name: 'Riley'}, version: 2, access: {read: true, edit: true, delete: true, share: true, transfer: true}, shares: [{grantee_kind: 'role', grantee_id: 'r1', grantee_name: 'Sales', access: 'edit', created_at: '2026-10-11T00:00:00Z'}]})).getSharing('custom:deal', '000a');
    expect(sharing.shares).toEqual([{granteeKind: 'role', granteeId: 'r1', granteeName: 'Sales', access: 'edit', createdAt: '2026-10-11T00:00:00Z'}]);
    const put = respond(200, {});
    await createObjectsApiClient(workspace, put).share('custom:deal', '000a', 'user', 'u2', 'read');
    expect(vi.mocked(put).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Adeal/records/000a/sharing/user/u2`);
    expect(JSON.parse(String(vi.mocked(put).mock.calls[0]?.[1]?.body))).toEqual({access: 'read'});
    const owner = respond(200, {id: '000a', name: 'Deal', owner: {id: 'u2'}, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 3, is_deleted: false});
    await createObjectsApiClient(workspace, owner).changeOwner('custom:deal', '000a', 'u2', 2);
    expect(JSON.parse(String(vi.mocked(owner).mock.calls[0]?.[1]?.body))).toEqual({owner_id: 'u2', expected_version: 2});
    const removed = vi.fn(async () => new Response(null, {status: 204})) as unknown as typeof fetch;
    await createObjectsApiClient(workspace, removed).unshare('custom:deal', '000a', 'role', 'r1');
    expect(vi.mocked(removed).mock.calls[0]?.[1]?.method).toBe('DELETE');
    await expect(createObjectsApiClient(workspace, respond(409, {code: 'members.reporting_cycle'})).setManager('u1', 'u2')).rejects.toMatchObject({code: 'members.reporting_cycle'});
  });

  it('transfers in the fixture engine and drops manual shares', async () => {
    const client = createFixtureObjectsClient();
    await client.share('custom:project', '0000000000000001', 'user', 'usr-morgan', 'edit');
    expect((await client.getSharing('custom:project', '0000000000000001')).shares).toHaveLength(1);
    const moved = await client.changeOwner('custom:project', '0000000000000001', 'usr-morgan', 1);
    expect(moved.owner?.name).toBe('Morgan Lee');
    expect((await client.getSharing('custom:project', '0000000000000001')).shares).toHaveLength(0);
  });

  it('offers sharing only to those who may share or transfer', () => {
    const client = createFixtureObjectsClient();
    const base = {id: '000a', name: 'Deal', owner: {id: 'u1', name: 'Riley <b>Rep</b>'}, createdAt: '', createdBy: null, updatedAt: '', updatedBy: null, version: 1, isDeleted: false, values: {}, related: {}};
    expect(renderToStaticMarkup(<SharingPanel client={client} object={project} record={{...base, rights: {read: true, edit: true, delete: false, share: false, transfer: false}}} onTransferred={noop} />)).toBe('');
    const owner = renderToStaticMarkup(<SharingPanel client={client} object={project} record={{...base, rights: {read: true, edit: true, delete: true, share: true, transfer: true}}} onTransferred={noop} />);
    expect(owner).toContain('Change owner');
    expect(owner).toContain('Share with');
    expect(owner).toContain('Riley &lt;b&gt;Rep&lt;/b&gt;');
    expect(owner).toContain('This object is private');
    const manager = renderToStaticMarkup(<SharingPanel client={client} object={project} record={{...base, rights: {read: true, edit: true, delete: true, share: true, transfer: false}}} onTransferred={noop} />);
    expect(manager).not.toContain('Change owner');
    expect(renderToStaticMarkup(<MembersPage client={client} onNavigate={noop} />)).toContain('Loading members');
  });
});

describe('field history (ADR 0017)', () => {
  it('reads entries, owner references and cursors from the history route', async () => {
    const fetcher = respond(200, {data: [{id: 'h1', kind: 'changed', field: 'owner', old_value: 'u1', new_value: 'u2', old_user: {id: 'u1', name: 'Riley'}, new_user: {id: 'u2', name: 'Morgan'}, changed_at: '2026-10-11T00:00:00Z', changed_by: {id: 'u1', name: 'Riley'}}], related: {'000x': 'Acme'}, next_cursor: 'abc'});
    const page = await createObjectsApiClient(workspace, fetcher).history('custom:deal', '000a', 'prev');
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Adeal/records/000a/history?cursor=prev`);
    expect(page.entries[0]).toMatchObject({kind: 'changed', field: 'owner', newUser: {id: 'u2', name: 'Morgan'}});
    expect(page.related).toEqual({'000x': 'Acme'});
    expect(page.nextCursor).toBe('abc');
    const toggle = respond(200, {api_name: 'custom:code', label: 'Code', description: null, field_type: 'text', origin: 'workspace', required: false, unique: false, indexed: false, track_history: true, reference_object: null, delete_behaviour: null, version: 2});
    const updated = await createObjectsApiClient(workspace, toggle).updateField('custom:project', 'custom:code', 1, {track_history: true});
    expect(updated.trackHistory).toBe(true);
    expect(JSON.parse(String(vi.mocked(toggle).mock.calls[0]?.[1]?.body))).toEqual({track_history: true, expected_version: 1});
  });

  it('shows history only for objects that track it', () => {
    const client = createFixtureObjectsClient();
    const record = {id: '000a', name: 'Deal', owner: null, createdAt: '', createdBy: null, updatedAt: '', updatedBy: null, version: 1, isDeleted: false, values: {}, related: {}};
    expect(renderToStaticMarkup(<HistoryPanel client={client} object={{...project, trackHistory: false}} record={record} onNavigate={noop} />)).toBe('');
    expect(renderToStaticMarkup(<HistoryPanel client={client} object={project} record={record} onNavigate={noop} />)).toContain('Loading history');
  });
});

describe('workspace object pages', () => {
  it('opens engine objects, the Object Manager and settings as kernel routes', () => {
    const render = (route: string) => renderToStaticMarkup(<AppShell model={shellFixture('ready')} route={route} onNavigate={noop} onWorkspaceChange={noop} onRetry={noop} />);
    for (const route of ['/o/custom:project', '/o/custom:project/0000000000000001', '/o/custom:project/new']) {
      const markup = render(route);
      expect(markup).not.toContain('Route unavailable');
      expect(markup).toContain('Loading object');
    }
    expect(render('/settings')).toContain('Object Manager');
    expect(render('/settings')).toContain('Members');
    expect(render('/settings/members')).toContain('Loading members');
    expect(render('/settings/objects')).toContain('Loading objects');
    expect(render('/settings/objects/custom:project')).toContain('Loading object');
  });

  it('builds the form from field metadata with labels, required markers and inert labels', () => {
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={project} onCancel={noop} onSaved={noop} />);
    expect(markup).toContain('Project name');
    expect(markup).toMatch(/<select[^>]*required/);
    expect(markup).toContain('<option value="open" selected="">Open</option>');
    expect(markup).not.toContain('>Lost<');
    expect(markup).toContain('Must be unique');
    expect(markup).toContain('Notes &lt;i&gt;');
    expect(markup).toContain('<textarea');
  });

  it('offers Save, Save & New and Cancel, with Save & New only for those who may create', () => {
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={project} onCancel={noop} onSaved={noop} />);
    expect(markup.match(/type="submit"/g)).toHaveLength(2);
    expect(markup).toContain('<span>Save</span>');
    expect(markup).toContain('<span>Save &amp; New</span>');
    expect(markup).toContain('<span>Cancel</span>');
    const readOnly = {...project, access: {...project.access, create: false}};
    expect(renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={readOnly} onCancel={noop} onSaved={noop} />)).not.toContain('Save &amp; New');
  });

  it('fills a related record\'s parent in and shows it read-only (ADR 0032 §1)', () => {
    const task: ObjectDescription = {...project, apiName: 'custom:task', label: 'Task', pluralLabel: 'Tasks', nameLabel: 'Task name', fields: [
      {apiName: 'custom:project', label: 'Project', description: null, type: 'lookup', origin: 'workspace', required: true, unique: false, indexed: true, options: [], referenceObject: 'custom:project', deleteBehaviour: 'restrict', trackHistory: false, version: 1},
    ]};
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={task} presets={{'custom:project': {value: '000abc', label: 'Apollo <b>'}}} onCancel={noop} onSaved={noop} />);
    expect(markup).toMatch(/<input[^>]*readOnly=""[^>]*value="Apollo &lt;b&gt;"/);
    expect(markup).toContain('Set to Apollo &lt;b&gt;.');
    expect(markup).not.toContain('Search project');
  });

  it('offers New for engine objects in navigation the member may create', () => {
    const summary = ({fields: _, ...rest}: ObjectDescription) => rest;
    const objects = [
      summary({...project, apiName: 'custom:zeta', label: 'Zeta'}),
      summary(project),
      summary({...project, apiName: 'custom:hidden', label: 'Hidden', inNavigation: false}),
      summary({...project, apiName: 'custom:locked', label: 'Locked', access: {...project.access, create: false}}),
      summary({...project, apiName: 'contacts:person', label: 'Person', origin: 'plugin'}),
    ];
    expect(creatable(objects).map(object => object.label)).toEqual(['Project', 'Zeta']);
    const markup = renderToStaticMarkup(<GlobalNew client={createFixtureObjectsClient()} onNavigate={noop} />);
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('aria-controls="global-new-panel"');
    expect(markup).toContain('New</span>');
  });

  it('creates and edits in a labelled modal dialog', () => {
    const create = renderToStaticMarkup(<RecordModal client={createFixtureObjectsClient()} object={project} onClose={noop} onSaved={noop} onNavigate={noop} />);
    expect(create).toMatch(/role="dialog" aria-modal="true" aria-labelledby="([^"]+)"/);
    const titleId = /aria-labelledby="([^"]+)"/.exec(create)?.[1];
    expect(create).toContain(`<h2 id="${titleId}">New project</h2>`);
    expect(create).toContain('aria-label="Close"');
    const record = recordFromApi({id: '000abc', name: 'Apollo <b>', owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 2, is_deleted: false});
    const edit = renderToStaticMarkup(<RecordModal client={createFixtureObjectsClient()} object={project} record={record} onClose={noop} onSaved={noop} onNavigate={noop} />);
    expect(edit).toContain('Edit Apollo &lt;b&gt;');
    expect(edit).toContain('value="Apollo &lt;b&gt;"');
  });
});


describe('quick action values (ADR 0032)', () => {
  const [status, code] = project.fields;
  const shown = [status, code].filter((field): field is NonNullable<typeof field> => field !== undefined);

  it('sends a shown preset field as submitted, even unchanged, so the member wins', () => {
    const initial = {name: 'Apollo', 'custom:status': 'open', 'custom:code': 'AP-1'};
    // The record is open; the action presets won; the member chose open again.
    const sent = valuesToSend({record: true, draft: {...initial}, initial, shown, showsName: false, always: {'custom:status': 'won'}});
    expect(sent).toEqual({'custom:status': 'open'});
  });

  it('sends a cleared preset field on create instead of the preset', () => {
    const draft = {name: 'New', 'custom:status': '', 'custom:code': ''};
    const sent = valuesToSend({record: false, draft, initial: draft, shown, showsName: true, always: {'custom:status': 'won', 'custom:code': 'X-1'}});
    expect(sent).toEqual({name: 'New', 'custom:status': null, 'custom:code': null});
  });

  it('still sends hidden presets and leaves untouched fields out of edits', () => {
    const initial = {name: 'Apollo', 'custom:status': 'open', 'custom:code': 'AP-1'};
    const sent = valuesToSend({record: true, draft: {...initial, 'custom:code': 'AP-2'}, initial, shown: shown.filter(field => field.apiName === 'custom:code'), showsName: false, always: {'custom:status': 'won'}});
    expect(sent).toEqual({'custom:status': 'won', 'custom:code': 'AP-2'});
  });
});

function recordFromApiShape() {
  const record = recordFromApi({id: '000abc', name: 'Apollo', owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 5, is_deleted: false});
  return {id: record.id, name: record.name, owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: record.version, is_deleted: false};
}

describe('query console', () => {
  it('sends the query, cursor and recycle bin flag and maps the response', async () => {
    const fetcher = respond(200, {object: 'custom:project', columns: [{key: 'name', type: 'text'}], data: [{id: '000abc', name: 'Apollo'}], next_cursor: 'next'});
    const result = await createObjectsApiClient(workspace, fetcher).query("SELECT name FROM custom:project WHERE name = 'a&b'", {cursor: 'c1', includeDeleted: true});
    const url = new URL(String(vi.mocked(fetcher).mock.calls[0]?.[0]), 'http://localhost');
    expect(url.pathname).toBe(`/api/v1/workspaces/${workspace}/query`);
    expect(url.searchParams.get('q')).toBe("SELECT name FROM custom:project WHERE name = 'a&b'");
    expect(url.searchParams.get('cursor')).toBe('c1');
    expect(url.searchParams.get('include_deleted')).toBe('true');
    expect(result).toEqual({object: 'custom:project', columns: [{key: 'name', type: 'text'}], rows: [{id: '000abc', name: 'Apollo'}], nextCursor: 'next'});
  });

  it('explains problems with the position the server names', async () => {
    const fetcher = respond(400, {code: 'validation.failed', errors: [{field: 'q', code: 'query.syntax', detail: 'expected a name, found the end of the query (at position 32)'}]});
    const error = await createObjectsApiClient(workspace, fetcher).query('SELECT name FROM custom:x WHERE').catch((caught: unknown) => caught);
    expect(queryProblem(error)).toContain('position 32');
    expect(queryProblem(new ObjectsApiError('unavailable', 'query.timeout'))).toContain('took too long');
  });

  it('renders rows as inert text with links to records', () => {
    const markup = renderToStaticMarkup(<QueryResults onNavigate={noop} result={{object: 'custom:project', columns: [{key: 'name', type: 'text'}, {key: 'custom:client.name', type: 'text'}, {key: 'custom:tags', type: 'multi_picklist'}], rows: [{id: '000abc', name: '<b>Apollo</b>', 'custom:client.name': null, 'custom:tags': ['a', 'b']}], nextCursor: null}} />);
    expect(markup).toContain('href="/o/custom%3Aproject/000abc"');
    expect(markup).toContain('&lt;b&gt;Apollo&lt;/b&gt;');
    expect(markup).toContain('a, b');
    expect(markup).toContain('object-value--empty');
    const aggregate = renderToStaticMarkup(<QueryResults onNavigate={noop} result={{object: 'custom:project', columns: [{key: 'n', type: 'number'}], rows: [{n: 3}], nextCursor: null}} />);
    expect(aggregate).not.toContain('>ID<');
    expect(formatCell(true)).toBe('Yes');
    expect(formatCell(undefined)).toBe('—');
  });

  it('labels the query editor and is reachable from settings', () => {
    const markup = renderToStaticMarkup(<QueryConsole client={createFixtureObjectsClient()} onNavigate={noop} />);
    expect(markup).toMatch(/<label class="ui-field__label" for="[^"]+">Query<\/label>/);
    expect(markup).toContain('Include records in the recycle bin');
    expect(markup).toContain('Run query');
  });
});

describe('global search', () => {
  it('sends the query and maps groups', async () => {
    const fetcher = respond(200, {data: [{object: 'custom:project', label: 'Project', plural_label: 'Projects', records: [{id: '000abc', name: 'Apollo', fields: {'custom:code': 'AP-1'}}], has_more: true}]});
    const groups = await createObjectsApiClient(workspace, fetcher).search('apo & co', {object: 'custom:project', limit: 200});
    const url = new URL(String(vi.mocked(fetcher).mock.calls[0]?.[0]), 'http://localhost');
    expect(url.pathname).toBe(`/api/v1/workspaces/${workspace}/search`);
    expect(url.searchParams.get('q')).toBe('apo & co');
    expect(url.searchParams.get('object')).toBe('custom:project');
    expect(url.searchParams.get('limit')).toBe('200');
    expect(groups).toEqual([{object: 'custom:project', label: 'Project', pluralLabel: 'Projects', records: [{id: '000abc', name: 'Apollo', fields: {'custom:code': 'AP-1'}}], hasMore: true}]);
  });

  it('needs two letters or digits and lists records then "see all"', () => {
    expect(searchable('a')).toBe(false);
    expect(searchable('  !! ')).toBe(false);
    expect(searchable('ab')).toBe(true);
    const options = searchOptions([{object: 'custom:project', label: 'Project', pluralLabel: 'Projects', records: [{id: '000abc', name: 'Apollo', fields: {}}], hasMore: false}], ' apo ', 'id');
    expect(options.map(option => option.route)).toEqual(['/o/custom%3Aproject/000abc', searchResultsRoute('apo')]);
    expect(options[1]?.label).toBe('See all results for “apo”');
    expect(searchResultsRoute('a b', 'custom:x')).toBe('/search?q=a+b&object=custom%3Ax');
  });

  it('is an accessible combobox in the shell header', () => {
    const markup = renderToStaticMarkup(<GlobalSearch client={createFixtureObjectsClient()} onNavigate={noop} />);
    expect(markup).toContain('role="combobox"');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toMatch(/aria-controls="[^"]+-results"/);
    expect(markup).toContain('role="listbox"');
    expect(markup).toContain('Search Limiara');
  });

  it('searches fixture names by prefix and renders the results page states', async () => {
    const client = createFixtureObjectsClient();
    const groups = await client.search('apo');
    expect(groups.map(group => group.records.map(record => record.name))).toEqual([['Apollo']]);
    await expect(client.search('a')).rejects.toBeInstanceOf(ObjectsApiError);
    const page = renderToStaticMarkup(<SearchResultsPage client={client} search="?q=apollo" onNavigate={noop} />);
    expect(page).toContain('Results for “apollo”');
    expect(page).toContain('Searching');
  });
});

describe('validation rules', () => {
  it('addresses rules by name with versions', async () => {
    const fetcher = respond(200, {name: 'amount_positive', description: null, condition: 'custom:amount <= 0', message: 'Amount must be positive', error_field: 'custom:amount', active: false, version: 2});
    const client = createObjectsApiClient(workspace, fetcher);
    const rule = await client.updateValidationRule('custom:project', 'amount_positive', 1, {active: false});
    const [url, init] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(url).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/validation-rules/amount_positive`);
    expect(init?.method).toBe('PATCH');
    expect(JSON.parse(String(init?.body))).toEqual({active: false, expected_version: 1});
    expect(rule).toEqual({name: 'amount_positive', description: null, condition: 'custom:amount <= 0', message: 'Amount must be positive', errorField: 'custom:amount', active: false, version: 2});
  });

  it('keeps fixture rules per object and refuses duplicate names', async () => {
    const client = createFixtureObjectsClient();
    await client.createValidationRule('custom:project', {name: 'code_set', condition: 'ISBLANK(custom:code)', message: 'Add a code'});
    await expect(client.createValidationRule('custom:project', {name: 'code_set', condition: 'TRUE', message: 'x'})).rejects.toBeInstanceOf(ObjectsApiError);
    const updated = await client.updateValidationRule('custom:project', 'code_set', 1, {active: false});
    expect(updated).toMatchObject({active: false, version: 2});
    await client.deleteValidationRule('custom:project', 'code_set', 2);
    expect(await client.validationRules('custom:project')).toEqual([]);
  });

  it('renders the section with a labelled loading state', () => {
    const markup = renderToStaticMarkup(<ValidationRules client={createFixtureObjectsClient()} object={project} onNotice={noop} />);
    expect(markup).toContain('Validation rules');
    expect(markup).toContain('New rule');
    expect(markup).toContain('Loading validation rules');
  });
});

describe('formula fields', () => {
  const total = {apiName: 'custom:total', label: 'Total', description: null, type: 'formula' as const, origin: 'workspace' as const, required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1, formula: 'custom:amount * 1.2', formulaType: 'currency' as const};

  it('maps the formula and formats values by the result type', async () => {
    const fetcher = respond(200, {api_name: 'custom:total', label: 'Total', description: null, field_type: 'formula', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, delete_behaviour: null, formula: 'custom:amount * 1.2', formula_type: 'currency', version: 1});
    const field = await createObjectsApiClient(workspace, fetcher).createField('custom:project', {name: 'total', label: 'Total', field_type: 'formula', formula: 'custom:amount * 1.2', formula_type: 'currency'});
    expect(field).toMatchObject({type: 'formula', formula: 'custom:amount * 1.2', formulaType: 'currency'});
    expect(JSON.parse(String(vi.mocked(fetcher).mock.calls[0]?.[1]?.body))).toMatchObject({field_type: 'formula', formula: 'custom:amount * 1.2', formula_type: 'currency'});
    expect(formatValue(total, 1234.5)).toBe((1234.5).toLocaleString(undefined, {minimumFractionDigits: 2, maximumFractionDigits: 2}));
    expect(formatValue({...total, formulaType: 'boolean'}, true)).toBe('Yes');
  });

  it('reads and sends roll-up summary fields and formats them by result type (ADR 0026)', async () => {
    const rollup = {function: 'sum' as const, object: 'custom:line', relationship: 'custom:project', field: 'custom:amount'};
    const fetcher = respond(200, {api_name: 'custom:total', label: 'Total', description: null, field_type: 'rollup', origin: 'workspace', required: false, unique: false, indexed: true, reference_object: null, delete_behaviour: null, formula: null, formula_type: 'currency', rollup, version: 1});
    const field = await createObjectsApiClient(workspace, fetcher).createField('custom:project', {name: 'total', label: 'Total', field_type: 'rollup', rollup});
    expect(field).toMatchObject({type: 'rollup', formulaType: 'currency', rollup});
    expect(JSON.parse(String(vi.mocked(fetcher).mock.calls[0]?.[1]?.body))).toMatchObject({field_type: 'rollup', rollup});
    expect(formatValue(field, 1250.5)).toBe(formatValue({...field, type: 'currency'}, 1250.5));
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={{...project, fields: [...project.fields, field]}} onCancel={noop} onSaved={noop} />);
    expect(markup).not.toContain('>Total<');
  });

  it('leaves formula fields out of the record form', () => {
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={{...project, fields: [...project.fields, total]}} onCancel={noop} onSaved={noop} />);
    expect(markup).toContain('Code');
    expect(markup).not.toContain('Total');
  });
});

describe('automations (ADR 0022)', () => {
  const before = {name: 'set_code', description: null, trigger: 'create_or_update' as const, timing: 'before_save' as const, condition: 'ISBLANK(custom:code)', actions: [{type: 'update_record' as const, values: [{field: 'custom:code', formula: "'P-' & id"}]}], schedule: null, active: true, version: 1};

  it('addresses automations by name with versions and keeps actions as sent', async () => {
    const fetcher = respond(200, {...before, active: false, version: 2, created_at: '2026-10-12T00:00:00Z'});
    const client = createObjectsApiClient(workspace, fetcher);
    const automation = await client.updateAutomation('custom:project', 'set_code', 1, {active: false, condition: ''});
    const [url, init] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(url).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/automations/set_code`);
    expect(init?.method).toBe('PATCH');
    expect(JSON.parse(String(init?.body))).toEqual({active: false, condition: '', expected_version: 1});
    expect(automation).toEqual({...before, active: false, version: 2});
  });

  it('keeps fixture automations per object in name order and refuses duplicate names', async () => {
    const client = createFixtureObjectsClient();
    const input = {name: before.name, trigger: before.trigger, timing: before.timing, condition: before.condition, actions: before.actions};
    await client.createAutomation('custom:project', {...input, name: 'z_last', condition: ' '});
    await client.createAutomation('custom:project', input);
    await expect(client.createAutomation('custom:project', input)).rejects.toMatchObject({code: 'automations.duplicate_name'});
    expect((await client.automations('custom:project')).map(item => [item.name, item.condition])).toEqual([['set_code', 'ISBLANK(custom:code)'], ['z_last', null]]);
    expect(await client.updateAutomation('custom:project', 'set_code', 1, {active: false})).toMatchObject({active: false, version: 2});
    await client.deleteAutomation('custom:project', 'set_code', 2);
    expect((await client.automations('custom:project')).map(item => item.name)).toEqual(['z_last']);
  });

  it('describes when an automation runs and what it does', () => {
    const objects = [{apiName: 'custom:task', label: 'Task', pluralLabel: 'Tasks', description: null, origin: 'workspace' as const, keyPrefix: '001', nameLabel: 'Subject', inNavigation: true, sharingModel: 'read_write' as const, grantHierarchy: true, trackHistory: false, version: 1, access: project.access}];
    expect(automationWhen(before)).toBe('A record is created or updated, before saving');
    expect(automationWhen({trigger: 'update', timing: 'after_save'})).toBe('A record is updated, after saving');
    expect(automationDoes({actions: [
      ...before.actions,
      {type: 'create_record', object: 'custom:task', values: [{field: 'name', formula: "'x'"}]},
      {type: 'update_related', lookup: 'custom:status', values: [{field: 'name', formula: "'x'"}]},
    ]}, project, objects)).toEqual(['Sets Code', 'Creates a Task record', 'Updates the Status record']);
  });

  it('renders the section with a labelled loading state', () => {
    const markup = renderToStaticMarkup(<Automations client={createFixtureObjectsClient()} object={project} onNotice={noop} />);
    expect(markup).toContain('Automations');
    expect(markup).toContain('New automation');
    expect(markup).toContain('Loading automations');
  });

  it('builds a labelled form for a new before-save automation', () => {
    const markup = renderToStaticMarkup(<AutomationForm client={createFixtureObjectsClient()} object={project} objects={[]} automation={null} onCancel={noop} onSaved={noop} />);
    for (const text of ['Automation name', 'Runs when', 'Before the record is saved', 'After the record is saved', 'Only when', 'Set fields on the record', 'Field 1', 'Formula for field 1', 'Set another field', 'Add automation']) {
      expect(markup).toContain(text);
    }
    expect(markup).toContain('Project name');
    expect(markup).toContain('>Code<');
  });

  it('shows the timing of an existing automation without letting it change', () => {
    const markup = renderToStaticMarkup(<AutomationForm client={createFixtureObjectsClient()} object={project} objects={[]} automation={{...before, timing: 'after_save', actions: [{type: 'update_related', lookup: '', values: [{field: '', formula: ''}]}]}} onCancel={noop} onSaved={noop} />);
    expect(markup).toContain('after saving');
    expect(markup).not.toContain('Before the record is saved');
    expect(markup).toContain('Update a related record');
    expect(markup).toContain('Choose the record first');
  });
});

describe('automation runs (ADR 0022 §5)', () => {
  it('pages runs newest first and maps them', async () => {
    const fetcher = respond(200, {data: [{id: 'run-1', record: '0a1000000000000001', event: 'delete', outcome: 'failed', detail: 'automation x: Too big', saves: 0, ran_at: '2026-10-12T10:00:00Z', ran_by: {id: 'usr-hannah', name: 'Hannah Okafor'}}], next_cursor: 'abc'});
    const page = await createObjectsApiClient(workspace, fetcher).automationRuns('custom:project', 'set_code', 'prev');
    const [url] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(url).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/automations/set_code/runs?limit=20&cursor=prev`);
    expect(page).toEqual({runs: [{id: 'run-1', record: '0a1000000000000001', event: 'delete', outcome: 'failed', detail: 'automation x: Too big', saves: 0, ranAt: '2026-10-12T10:00:00Z', ranBy: {id: 'usr-hannah', name: 'Hannah Okafor'}}], nextCursor: 'abc'});
  });

  it('labels lifecycle triggers and shows a labelled loading state', () => {
    expect(automationWhen({trigger: 'delete', timing: 'after_save'})).toBe('A record is deleted, after saving');
    expect(automationWhen({trigger: 'undelete', timing: 'after_save'})).toBe('A record is restored, after saving');
    const markup = renderToStaticMarkup(<AutomationRuns client={createFixtureObjectsClient()} object={project} name="set_code" />);
    expect(markup).toContain('Runs of');
    expect(markup).toContain('kept for 30 days');
    expect(markup).toContain('Loading runs of set_code');
  });
});

describe('background and scheduled automations (ADR 0022 §6)', () => {
  it('describes the new timings and maps jobs', async () => {
    expect(automationWhen({trigger: 'create', timing: 'async'})).toBe('A record is created, in the background');
    expect(automationWhen({trigger: 'update', timing: 'scheduled'})).toBe('A record is updated, on a schedule');
    const fetcher = respond(200, {id: 'job-1', record: '0a1000000000000001', event: 'create', scheduled: true, due_at: '2026-10-14T01:00:00Z', status: 'pending', attempts: 0, last_error: null, created_at: '2026-10-12T00:00:00Z', updated_at: '2026-10-12T00:00:00Z'});
    const job = await createObjectsApiClient(workspace, fetcher).changeAutomationJob('custom:project', 'escalate', 'job-1', 'retry');
    const [url, init] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(url).toBe(`/api/v1/workspaces/${workspace}/objects/custom%3Aproject/automations/escalate/jobs/job-1/retry`);
    expect(init?.method).toBe('POST');
    expect(job).toEqual({id: 'job-1', record: '0a1000000000000001', scheduled: true, dueAt: '2026-10-14T01:00:00Z', status: 'pending', attempts: 0, lastError: null});
  });

  it('asks when a scheduled automation runs', () => {
    const scheduled = {name: 'escalate', description: null, trigger: 'create_or_update' as const, timing: 'scheduled' as const, condition: null, actions: [{type: 'update_record' as const, values: [{field: 'custom:code', formula: "'late'"}]}], schedule: {offset_minutes: 90, from: 'custom:starts_on'}, active: true, version: 1};
    const withDate = {...project, fields: [...project.fields, {apiName: 'custom:starts_on', label: 'Starts on', description: null, type: 'date' as const, origin: 'workspace' as const, required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1}]};
    const markup = renderToStaticMarkup(<AutomationForm client={createFixtureObjectsClient()} object={withDate} objects={[]} automation={scheduled} onCancel={noop} onSaved={noop} />);
    for (const text of ['on a schedule', 'When it runs', 'Hours after', 'value="1.5"', 'Counting from', 'Starts on', 'Update this record']) {
      expect(markup).toContain(text);
    }
    const jobs = renderToStaticMarkup(<AutomationJobs client={createFixtureObjectsClient()} object={project} name="escalate" onNotice={noop} />);
    expect(jobs).toContain('Queued jobs of');
    expect(jobs).toContain('Loading jobs of escalate');
  });
});

describe('action conditions and events (ADR 0022 §7)', () => {
  it('describes events and offers the new inputs', () => {
    expect(automationDoes({actions: [{type: 'publish_event', event: 'order_placed', values: [{field: 'total', formula: '1'}]}]}, project, [])).toEqual(['Publishes the order_placed event']);
    expect(automationDoes({actions: [{type: 'publish_event', event: 'order_placed', when: 'custom:total > 1', values: [{field: 'total', formula: '1'}]}]}, project, [])).toEqual(['Publishes the order_placed event (when it applies)']);
    const markup = renderToStaticMarkup(<AutomationForm client={createFixtureObjectsClient()} object={project} objects={[]} automation={{name: 'route', description: null, trigger: 'create', timing: 'after_save', condition: null, actions: [{type: 'publish_event', event: 'order_placed', when: 'custom:budget > 10', values: [{field: 'total', formula: 'custom:budget'}]}], schedule: null, active: true, version: 1}} onCancel={noop} onSaved={noop} />);
    for (const text of ['Publish an event', 'Event name', 'value="order_placed"', 'Only when (action 1)', 'value="custom:budget &gt; 10"', 'value="total"']) {
      expect(markup).toContain(text);
    }
  });
});

describe('saved reports (ADR 0027)', () => {
  it('maps reports from the API and sends the version with changes', async () => {
    const api = {id: 'r1', name: 'By status', description: null, query: 'SELECT custom:status, COUNT() FROM custom:project GROUP BY custom:status', chart: 'bar', label_column: 'custom:status', value_column: 'count()', visibility: 'workspace', owner: {id: 'u1', name: 'Riley'}, version: 2, updated_at: '2026-10-13T00:00:00Z'};
    const fetcher = respond(200, {...api, result: {object: 'custom:project', columns: [{key: 'custom:status', type: 'picklist'}, {key: 'count()', type: 'number'}], data: [{'custom:status': 'open', 'count()': 3}], next_cursor: null}});
    const {report, result} = await createObjectsApiClient(workspace, fetcher).runReport('r1');
    expect(report).toMatchObject({name: 'By status', chart: 'bar', labelColumn: 'custom:status', valueColumn: 'count()', visibility: 'workspace', version: 2});
    expect(result.rows).toEqual([{'custom:status': 'open', 'count()': 3}]);
    expect(report.canEdit).toBe(false);
    const page = respond(200, {...api, can_edit: true, result: {object: 'custom:project', columns: [], data: [], next_cursor: null}});
    const next = await createObjectsApiClient(workspace, page).runReport('r1', {cursor: 'c2'});
    expect(String(vi.mocked(page).mock.calls[0]?.[0])).toContain('/reports/r1?cursor=c2');
    expect(next.report.canEdit).toBe(true);
    const update = respond(200, api);
    await createObjectsApiClient(workspace, update).updateReport('r1', 2, {name: 'Statuses'});
    expect(JSON.parse(String(vi.mocked(update).mock.calls[0]?.[1]?.body))).toEqual({expected_version: 2, name: 'Statuses'});
  });

  it('draws charts from the rows with the rows as their description', () => {
    const result = {object: 'custom:project', columns: [{key: 'custom:status', type: 'picklist'}, {key: 'count()', type: 'number'}], rows: [{'custom:status': 'open', 'count()': 3}, {'custom:status': 'won', 'count()': 1}], nextCursor: null};
    const bar = renderToStaticMarkup(<ReportChartView report={{name: 'By status', chart: 'bar', labelColumn: 'custom:status', valueColumn: 'count()'}} result={result} />);
    expect(bar).toContain('<svg');
    expect(bar).toContain('By status: open 3, won 1');
    const pie = renderToStaticMarkup(<ReportChartView report={{name: 'Share', chart: 'pie', labelColumn: 'custom:status', valueColumn: 'count()'}} result={result} />);
    expect(pie).toContain('75%');
    const metric = renderToStaticMarkup(<ReportChartView report={{name: 'Open', chart: 'metric', labelColumn: null, valueColumn: 'count()'}} result={result} />);
    expect(metric).toContain('>3<');
    expect(renderToStaticMarkup(<ReportChartView report={{name: 'Rows', chart: 'table', labelColumn: null, valueColumn: null}} result={result} />)).toBe('');
  });

  it('draws negative bars left of a zero baseline', () => {
    expect(barExtents([{label: 'a', value: 30}, {label: 'b', value: -10}], 400)).toEqual({zero: 100, scale: 10});
    expect(barExtents([{label: 'a', value: 5}], 400)).toEqual({zero: 0, scale: 80});
    const result = {object: 'custom:project', columns: [{key: 'custom:region', type: 'text'}, {key: 'total', type: 'currency'}], rows: [{'custom:region': 'North', total: 30}, {'custom:region': 'South', total: -10}], nextCursor: null};
    const bar = renderToStaticMarkup(<ReportChartView report={{name: 'Margin', chart: 'bar', labelColumn: 'custom:region', valueColumn: 'total'}} result={result} />);
    expect(bar).toContain('width="90"');
    expect(bar).toContain('report-chart__axis');
  });

  it('routes the report pages under /reports', () => {
    const client = createFixtureObjectsClient();
    const form = renderToStaticMarkup(<ReportsPage client={client} route="/reports/new" onNavigate={noop} />);
    expect(form).toContain('aria-label="New report"');
    expect(form).toContain('>Query</label>');
    expect(renderToStaticMarkup(<ReportsPage client={client} route="/reports/r1" onNavigate={noop} />)).toContain('ui-skeleton');
    expect(reportRoute('a b')).toBe('/reports/a%20b');
  });
});

describe('related lists and lookup filters (ADR 0028)', () => {
  it('maps related lists, pages them and asks for lookup candidates', async () => {
    const fetcher = respond(200, {data: [{object: 'custom:contact', object_label: 'Contact', object_plural_label: 'Contacts', field: 'custom:account', field_label: 'Account', master_detail: false, data: [{id: 'c1', name: 'Ana'}, {id: 'c2'}], next_cursor: 'n1'}]});
    const [list] = await createObjectsApiClient(workspace, fetcher).relatedLists('custom:account', 'a1');
    expect(list).toEqual({object: 'custom:contact', objectLabel: 'Contact', objectPluralLabel: 'Contacts', field: 'custom:account', fieldLabel: 'Account', masterDetail: false, records: [{id: 'c1', name: 'Ana'}, {id: 'c2', name: 'c2'}], nextCursor: 'n1'});
    const page = respond(200, {object: 'custom:contact', object_label: 'Contact', object_plural_label: 'Contacts', field: 'custom:account', field_label: 'Account', master_detail: false, data: [], next_cursor: null});
    await createObjectsApiClient(workspace, page).relatedRecords('custom:account', 'a1', 'custom:contact', 'custom:account', 'n1');
    expect(String(vi.mocked(page).mock.calls[0]?.[0])).toContain('/records/a1/related/custom%3Acontact/custom%3Aaccount?cursor=n1');
    const candidates = respond(200, {data: [{id: 'a1', name: 'Open Co'}], next_cursor: null});
    const found = await createObjectsApiClient(workspace, candidates).lookupCandidates('custom:contact', 'custom:account', ' open ');
    expect(String(vi.mocked(candidates).mock.calls[0]?.[0])).toContain('/fields/custom%3Aaccount/candidates?search=open');
    expect(found.records).toEqual([{id: 'a1', name: 'Open Co'}]);
  });

  it('sends null to remove a filter and shows the current one', async () => {
    const update = respond(200, {api_name: 'custom:account', label: 'Account', description: null, field_type: 'lookup', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: 'custom:account', delete_behaviour: 'clear', lookup_filter: null, version: 3});
    const field = await createObjectsApiClient(workspace, update).updateField('custom:contact', 'custom:account', 2, {lookup_filter: null});
    expect(JSON.parse(String(vi.mocked(update).mock.calls[0]?.[1]?.body))).toEqual({lookup_filter: null, expected_version: 2});
    expect(field.lookupFilter).toBeNull();
    const filtered = {...field, lookupFilter: 'custom:active = true'};
    const markup = renderToStaticMarkup(<LookupFilterForm client={createFixtureObjectsClient()} object={{...project, fields: [filtered]}} field={filtered} onDone={noop} />);
    expect(markup).toContain('Lookup filter for Account');
    expect(markup).toContain('custom:active = true');
    expect(markup).toContain('Remove filter');
  });

  it('lists the records pointing at a record in the fixture', async () => {
    const client = createFixtureObjectsClient();
    await client.createObject({name: 'task', label: 'Task', plural_label: 'Tasks'});
    await client.createField('custom:task', {name: 'project', label: 'Project', field_type: 'lookup', reference_object: 'custom:project', lookup_filter: 'custom:status = \'open\''});
    const parent = await client.createRecord('custom:project', {name: 'Zephyr Works'});
    await client.createRecord('custom:task', {name: 'Kick-off', 'custom:project': parent.id});
    const lists = await client.relatedLists('custom:project', parent.id);
    expect(lists.map(list => [list.objectPluralLabel, list.records.map(item => item.name)])).toEqual([['Tasks', ['Kick-off']]]);
    expect((await client.lookupCandidates('custom:task', 'custom:project', 'zeph')).records.map(item => item.name)).toEqual(['Zephyr Works']);
    expect((await client.describe('custom:task')).fields[0]?.lookupFilter).toBe("custom:status = 'open'");
    expect(renderToStaticMarkup(<RelatedLists client={client} apiName="custom:project" recordId={parent.id} onNavigate={noop} />)).toContain('ui-skeleton');
  });
});

describe('related-to fields (ADR 0033)', () => {
  const regarding = {apiName: 'custom:regarding', label: 'Regarding', description: null, type: 'related_to' as const, origin: 'workspace' as const, required: false, unique: false, indexed: false, options: [], referenceObject: null, referenceObjects: ['custom:account', 'custom:deal'], deleteBehaviour: 'clear' as const, trackHistory: false, version: 1};

  it('maps the allowlist and the objects of named targets', async () => {
    const fetcher = respond(200, {api_name: 'custom:regarding', label: 'Regarding', description: null, field_type: 'related_to', origin: 'workspace', required: false, unique: false, indexed: false, reference_object: null, reference_objects: ['custom:account', 'custom:deal'], delete_behaviour: 'clear', version: 2});
    const field = await createObjectsApiClient(workspace, fetcher).updateField('custom:task', 'custom:regarding', 1, {reference_objects: ['custom:account', 'custom:deal']});
    expect(JSON.parse(String(vi.mocked(fetcher).mock.calls[0]?.[1]?.body))).toEqual({reference_objects: ['custom:account', 'custom:deal'], expected_version: 1});
    expect(field.referenceObjects).toEqual(['custom:account', 'custom:deal']);
    const record = recordFromApi({id: 't1', name: 'Call', owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 1, is_deleted: false, 'custom:regarding': 'a1', related: {a1: 'Acme'}, related_objects: {a1: 'custom:account', bad: 3}});
    expect(record.values).toEqual({'custom:regarding': 'a1'});
    expect(record.relatedObjects).toEqual({a1: 'custom:account'});
    expect(formatValue(regarding, 'a1', record.related)).toBe('Acme');
  });

  it('asks for the candidates of the chosen object', async () => {
    const candidates = respond(200, {data: [{id: 'a1', name: 'Acme'}], next_cursor: null});
    await createObjectsApiClient(workspace, candidates).lookupCandidates('custom:task', 'custom:regarding', ' ac ', undefined, 'custom:account');
    expect(String(vi.mocked(candidates).mock.calls[0]?.[0])).toContain('/fields/custom%3Aregarding/candidates?object=custom%3Aaccount&search=ac');
  });

  it('points at records of any allowed object in the fixture', async () => {
    const client = createFixtureObjectsClient();
    await client.createObject({name: 'account', label: 'Account', plural_label: 'Accounts'});
    await client.createObject({name: 'task', label: 'Task', plural_label: 'Tasks'});
    await client.createField('custom:task', {name: 'regarding', label: 'Regarding', field_type: 'related_to', reference_objects: ['custom:account', 'custom:project']});
    const account = await client.createRecord('custom:account', {name: 'Acme'});
    await client.createRecord('custom:task', {name: 'Call Acme', 'custom:regarding': account.id});
    expect((await client.lookupCandidates('custom:task', 'custom:regarding', 'ac', undefined, 'custom:account')).records.map(item => item.name)).toEqual(['Acme']);
    await expect(client.lookupCandidates('custom:task', 'custom:regarding', '', undefined, 'custom:task')).rejects.toMatchObject({code: 'records.reference_object_not_allowed'});
    expect((await client.relatedLists('custom:account', account.id)).map(list => [list.fieldLabel, list.records.map(item => item.name)])).toEqual([['Regarding', ['Call Acme']]]);
    const task = await client.describe('custom:task');
    expect(task.fields[0]?.referenceObjects).toEqual(['custom:account', 'custom:project']);
    expect(actionTargets([task], 'custom:account').map(targetLabel)).toEqual(['Task (Regarding)']);
    expect(actionTargets([task], 'custom:deal')).toEqual([]);
  });

  it('edits the objects a related-to field allows', async () => {
    const task: ObjectDescription = {...project, apiName: 'custom:task', label: 'Task', fields: [regarding]};
    const markup = renderToStaticMarkup(<AllowlistForm client={createFixtureObjectsClient()} object={task} field={regarding} onDone={noop} />);
    expect(markup).toContain('Objects Regarding can point to');
    expect(markup).toContain('Loading objects…');
    expect(markup).toContain('Save objects');
    // An allowed object the list does not name stays offered after it is unchecked.
    const listed = [{...project, fields: undefined, apiName: 'custom:project', label: 'Project'}];
    expect(allowlistChoices(listed, ['custom:gone', 'custom:project'], ['custom:project']).map(item => item.apiName)).toEqual(['custom:project', 'custom:gone']);
    expect(allowlistChoices([], ['custom:gone'], []).map(item => item.label)).toEqual(['custom:gone']);
    const client = createFixtureObjectsClient();
    await client.createObject({name: 'task', label: 'Task', plural_label: 'Tasks'});
    const created = await client.createField('custom:task', {name: 'regarding', label: 'Regarding', field_type: 'related_to', reference_objects: ['custom:project']});
    const changed = await client.updateField('custom:task', created.apiName, created.version, {reference_objects: ['custom:project', 'custom:task']});
    expect(changed.referenceObjects).toEqual(['custom:project', 'custom:task']);
    expect(changed.version).toBe(created.version + 1);
  });

  it('offers the allowed objects, then their records, in the record form', () => {
    const task: ObjectDescription = {...project, apiName: 'custom:task', label: 'Task', fields: [regarding]};
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={task} onCancel={noop} onSaved={noop} />);
    expect(markup).toContain('<legend>Regarding</legend>');
    expect(markup).toContain('Regarding: object');
    expect(markup).toMatch(/<option value="custom:account"[^>]*>custom:account<\/option><option value="custom:deal"/);
    expect(markup).toContain('Search regarding: custom:account');
    // A preset (related-list New or a create action) shows read-only.
    const preset = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={task} presets={{'custom:regarding': {value: 'a1', label: 'Acme'}}} onCancel={noop} onSaved={noop} />);
    expect(preset).toContain('Set to Acme.');
    expect(preset).not.toContain('Regarding: object');
  });
});

describe('quick actions (ADR 0032)', () => {
  const account: ObjectDescription = {...project, apiName: 'custom:account', label: 'Account', pluralLabel: 'Accounts', nameLabel: 'Account name', fields: []};
  const contact: ObjectDescription = {...project, apiName: 'custom:contact', label: 'Contact', pluralLabel: 'Contacts', nameLabel: 'Full name', fields: [
    {apiName: 'custom:account', label: 'Account', description: null, type: 'lookup', origin: 'workspace', required: false, unique: false, indexed: true, options: [], referenceObject: 'custom:account', deleteBehaviour: 'clear', trackHistory: false, version: 1},
    {apiName: 'custom:email', label: 'Email', description: null, type: 'email', origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1},
    {apiName: 'custom:greeting', label: 'Greeting', description: null, type: 'formula', origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1, formula: '"Hi " & name', formulaType: 'text'},
  ]};
  const wire = {name: 'add_contact', label: 'Add contact', kind: 'create', position: 1, target_object: 'custom:contact', target_field: 'custom:account', fields: ['name', 'custom:email'], presets: {'custom:role': 'buyer'}, version: 1, created_at: '2026-11-09T00:00:00Z', created_by: {}, updated_at: '2026-11-09T00:00:00Z', updated_by: {}};

  it('maps actions from the API', () => {
    expect(quickActionFromApi(wire as Parameters<typeof quickActionFromApi>[0])).toEqual({name: 'add_contact', label: 'Add contact', kind: 'create', position: 1, targetObject: 'custom:contact', targetField: 'custom:account', fields: ['name', 'custom:email'], presets: {'custom:role': 'buyer'}, version: 1});
    expect(quickActionFromApi({name: 'close', label: 'Close', kind: 'update', position: 0, target_object: null, target_field: null, fields: ['custom:code'], presets: {}, version: 2})).toMatchObject({kind: 'update', targetObject: null, targetField: null, presets: {}});
  });

  it('lists, creates, changes and deletes actions by name with versions', async () => {
    const base = `/api/v1/workspaces/${workspace}/objects/custom%3Aaccount/quick-actions`;
    const listing = respond(200, {data: [wire]});
    expect((await createObjectsApiClient(workspace, listing).quickActions('custom:account')).map(action => action.name)).toEqual(['add_contact']);
    expect(vi.mocked(listing).mock.calls[0]?.[0]).toBe(base);

    const create = respond(201, wire);
    await createObjectsApiClient(workspace, create).createQuickAction('custom:account', {name: 'add_contact', label: 'Add contact', kind: 'create', target_object: 'custom:contact', target_field: 'custom:account', fields: ['name'], presets: {'custom:role': 'buyer'}});
    const [createUrl, createInit] = vi.mocked(create).mock.calls[0] ?? [];
    expect(createUrl).toBe(base);
    expect(createInit?.method).toBe('POST');
    expect(JSON.parse(String(createInit?.body))).toEqual({name: 'add_contact', label: 'Add contact', kind: 'create', target_object: 'custom:contact', target_field: 'custom:account', fields: ['name'], presets: {'custom:role': 'buyer'}});

    const update = respond(200, {...wire, version: 2});
    await createObjectsApiClient(workspace, update).updateQuickAction('custom:account', 'add_contact', 1, {label: 'New contact', position: 3});
    const [updateUrl, updateInit] = vi.mocked(update).mock.calls[0] ?? [];
    expect(updateUrl).toBe(`${base}/add_contact`);
    expect(updateInit?.method).toBe('PATCH');
    expect(JSON.parse(String(updateInit?.body))).toEqual({label: 'New contact', position: 3, expected_version: 1});

    const remove = respond(200, wire);
    await createObjectsApiClient(workspace, remove).deleteQuickAction('custom:account', 'add_contact', 2);
    const [deleteUrl, deleteInit] = vi.mocked(remove).mock.calls[0] ?? [];
    expect(deleteUrl).toBe(`${base}/add_contact?expected_version=2`);
    expect(deleteInit?.method).toBe('DELETE');

    await expect(createObjectsApiClient(workspace, respond(409, {code: 'quick_actions.duplicate_name'})).createQuickAction('custom:account', {name: 'x', label: 'X', kind: 'update', fields: ['name']})).rejects.toMatchObject({reason: 'conflict', code: 'quick_actions.duplicate_name'});
    await expect(createObjectsApiClient(workspace, respond(400, {code: 'validation.failed', errors: [{field: 'presets.custom:budget', code: 'records.value_type', detail: 'enter a number'}]})).createQuickAction('custom:account', {name: 'x', label: 'X', kind: 'update', fields: ['name']})).rejects.toMatchObject({reason: 'invalid', issues: [{field: 'presets.custom:budget'}]});
  });

  it('keeps fixture actions in position order, refuses duplicates and stops at ten', async () => {
    const client = createFixtureObjectsClient();
    await client.createQuickAction('custom:project', {name: 'second', label: 'Second', kind: 'update', position: 5, fields: ['custom:code']});
    await client.createQuickAction('custom:project', {name: 'first', label: 'First', kind: 'update', position: 1, fields: ['name'], presets: {'custom:status': 'won'}});
    expect((await client.quickActions('custom:project')).map(action => action.name)).toEqual(['first', 'second']);
    await expect(client.createQuickAction('custom:project', {name: 'first', label: 'Again', kind: 'update', fields: ['name']})).rejects.toMatchObject({reason: 'conflict', code: 'quick_actions.duplicate_name'});
    for (let index = 0; index < 8; index++) await client.createQuickAction('custom:project', {name: `extra_${index}`, label: `Extra ${index}`, kind: 'update', fields: ['name']});
    expect(await client.quickActions('custom:project')).toHaveLength(10);
    await expect(client.createQuickAction('custom:project', {name: 'eleventh', label: 'Eleventh', kind: 'update', fields: ['name']})).rejects.toMatchObject({reason: 'invalid', issues: [{code: 'quick_actions.limit_reached'}]});
    const changed = await client.updateQuickAction('custom:project', 'second', 1, {position: 0});
    expect(changed).toMatchObject({position: 0, version: 2});
    expect((await client.quickActions('custom:project'))[0]?.name).toBe('second');
    await expect(client.deleteQuickAction('custom:project', 'second', 1)).rejects.toMatchObject({code: 'records.version_conflict'});
    await client.deleteQuickAction('custom:project', 'second', 2);
    expect(await client.quickActions('custom:project')).toHaveLength(9);
  });

  it('offers the lookups of other objects as targets and never their computed or reference fields', () => {
    const targets = actionTargets([account, contact, project], 'custom:account');
    expect(targets.map(targetLabel)).toEqual(['Contact (Account)']);
    expect(actionFields(contact, 'custom:account').map(field => field.apiName)).toEqual(['custom:email']);
  });

  it('renders the Object Manager section and its form with labelled states', () => {
    const section = renderToStaticMarkup(<QuickActions client={createFixtureObjectsClient()} object={project} onNotice={noop} />);
    expect(section).toContain('<h2 id="quick-actions-heading">Quick actions</h2>');
    expect(section).toContain('Loading quick actions');
    const ready = {status: 'ready' as const, targets: actionTargets([contact], 'custom:account')};
    const blank = renderToStaticMarkup(<QuickActionForm client={createFixtureObjectsClient()} object={project} action={null} targets={ready} onCancel={noop} onSaved={noop} />);
    for (const label of ['Label', 'Action name', 'Kind', 'Position']) expect(blank).toContain(`>${label}`);
    expect(blank).toContain('<legend>Fields on the form</legend>');
    expect(blank).toContain('Project name');
    expect(blank).toContain('<legend>Presets</legend>');
    expect(blank).toContain('Add preset');
    expect(blank).toContain('Add quick action');
    const create: QuickAction = {name: 'add_contact', label: 'Add <b>contact</b>', kind: 'create', position: 0, targetObject: 'custom:contact', targetField: 'custom:account', fields: ['name', 'custom:email'], presets: {'custom:email': 'a@example.com'}, version: 1};
    const editing = renderToStaticMarkup(<QuickActionForm client={createFixtureObjectsClient()} object={account} action={create} targets={ready} onCancel={noop} onSaved={noop} />);
    expect(editing).toContain('Creates Contact (Account) records');
    expect(editing).toContain('The kind and target cannot change.');
    expect(editing).not.toContain('Action name');
    expect(editing).toContain('Full name');
    expect(editing).not.toContain('Greeting');
    expect(editing).toContain('Preset field 1');
    expect(editing).toContain('value="a@example.com"');
    expect(editing).toContain('Add &lt;b&gt;contact&lt;/b&gt;');
    expect(editing).not.toContain('<b>contact</b>');
  });

  it('shows only an action\'s fields in the record form, titled with its label as inert text', () => {
    const record = recordFromApi({id: '000abc', name: 'Apollo', owner: null, created_at: '', created_by: null, updated_at: '', updated_by: null, version: 2, is_deleted: false, 'custom:status': 'open', 'custom:code': 'AP-1'});
    const form = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={project} record={record} only={['custom:code']} always={{'custom:status': 'won'}} offerNew={false} onCancel={noop} onSaved={noop} />);
    expect(form).toContain('>Code');
    expect(form).toContain('value="AP-1"');
    expect(form).not.toContain('Project name');
    expect(form).not.toContain('>Status');
    expect(form).not.toContain('Notes');
    expect(form).not.toContain('Save &amp; New');
    const modal = renderToStaticMarkup(<RecordModal client={createFixtureObjectsClient()} object={project} record={record} title="Mark <i>won</i>" only={['custom:code']} offerNew={false} onClose={noop} onSaved={noop} onNavigate={noop} />);
    expect(modal).toContain('Mark &lt;i&gt;won&lt;/i&gt;</h2>');
    const task: ObjectDescription = {...project, apiName: 'custom:task', label: 'Task', fields: [
      {apiName: 'custom:project', label: 'Project', description: null, type: 'lookup', origin: 'workspace', required: true, unique: false, indexed: true, options: [], referenceObject: 'custom:project', deleteBehaviour: 'restrict', trackHistory: false, version: 1},
      ...project.fields,
    ]};
    const created = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={task} presets={{'custom:project': {value: '000abc', label: 'Apollo'}}} only={['name', 'custom:code']} always={{'custom:code': 'T-1', 'custom:status': 'open'}} onCancel={noop} onSaved={noop} />);
    expect(created).toContain('Project name');
    expect(created).toMatch(/<input[^>]*readOnly=""[^>]*value="Apollo"/);
    expect(created).toContain('value="T-1"');
    expect(created).not.toContain('>Status');
    expect(created).toContain('Save &amp; New');
  });
});
