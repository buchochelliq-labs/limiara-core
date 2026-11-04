import {afterEach, describe, expect, it, vi} from 'vitest';
import {ContactsApiError, createContactsApiClient, describeUpdated, peopleQueryParams, personFromApi, type ApiPerson} from './api';

const workspace = '018f0000-0000-7000-8000-000000000001';
const base = `/api/v1/workspaces/${workspace}/contacts`;

const apiPerson: ApiPerson = {
  id: 'per0000000000001',
  full_name: 'Maya Patel',
  email: 'maya@northstar.example',
  telephone: '+44 20 7946 0271',
  organisation_id: 'org0000000000001',
  tags: ['operations', 'priority'],
  custom_fields: {relationship_type__c: 'Client'},
  version: 7,
  archived_at: null,
  created_at: '2026-09-01T10:00:00Z',
  updated_at: '2026-09-29T12:00:00Z',
};

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}});
}

type Call = {url: string; init: RequestInit};

function fetchStub(handler: (call: Call) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const call = {url: String(input), init};
    calls.push(call);
    return handler(call);
  });
  return {calls, fetcher: fetcher as unknown as typeof fetch};
}

function organisationsOr(call: Call, otherwise: Response | Promise<Response>) {
  return call.url.startsWith(`${base}/organisations?`) ? json({data: [{id: 'org0000000000001', name: 'Northstar Services'}], next_cursor: null}) : otherwise;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('contacts API client', () => {
  it('maps API people onto records with organisation names and relative updates', () => {
    const now = Date.parse('2026-10-02T09:00:00Z');
    const record = personFromApi(apiPerson, new Map([['org0000000000001', 'Northstar Services']]), now);
    expect(record).toMatchObject({id: 'per0000000000001', name: 'Maya Patel', organisation: 'Northstar Services', organisationId: 'org0000000000001', label: 'Client', version: 7, archived: false, updated: '2 days ago'});
    expect(personFromApi({...apiPerson, organisation_id: 'org-unknown', archived_at: '2026-10-01T00:00:00Z', email: null}, new Map(), now)).toMatchObject({organisation: 'org-unknown', archived: true, email: ''});
    expect(describeUpdated('2026-10-02T08:00:00Z', now)).toBe('Today');
    expect(describeUpdated('2026-10-01T08:00:00Z', now)).toBe('Yesterday');
    expect(describeUpdated('2026-09-24T08:00:00Z', now)).toBe('1 week ago');
    expect(describeUpdated('not a date', now)).toBe('');
  });

  it('translates directory queries into the API list contract', () => {
    expect(peopleQueryParams({search: 'Maya', label: 'Client', sort: 'recent', status: 'active', cursor: 'abc'}).toString()).toBe('search=Maya&status=active&sort=updated_at&direction=desc&limit=25&cursor=abc');
    expect(peopleQueryParams({search: '', label: 'All', sort: 'name', status: 'all', cursor: ''}).toString()).toBe('status=all&sort=name&direction=asc&limit=25');
    // The API's search matches the name only; an address-shaped term becomes an exact email match.
    expect(peopleQueryParams({search: ' Maya@Northstar.example ', label: 'All', sort: 'name', status: 'active', cursor: ''}).toString()).toBe('email=maya%40northstar.example&status=active&sort=name&direction=asc&limit=25');
  });

  it('reads every organisation page once and shares the catalogue across callers', async () => {
    const {calls, fetcher} = fetchStub(call => {
      if (call.url.includes('/organisations')) {
        const cursor = new URL(call.url, 'http://x').searchParams.get('cursor');
        return cursor === 'page-2'
          ? json({data: [{id: 'org0000000000002', name: 'Second Page Org'}], next_cursor: null})
          : json({data: [{id: 'org0000000000001', name: 'Northstar Services'}], next_cursor: 'page-2'});
      }
      return json({...apiPerson, organisation_id: 'org0000000000002'});
    });
    const client = createContactsApiClient(workspace, fetcher);
    const aborted = new AbortController();
    const first = client.getPerson('per0000000000001', aborted.signal);
    aborted.abort();
    await first.catch(() => undefined);
    const record = await client.getPerson('per0000000000001');
    expect(record.organisation).toBe('Second Page Org');
    expect(calls.filter(call => call.url.includes('/organisations')).map(call => call.init.signal)).toEqual([undefined, undefined]);
    expect(calls.filter(call => call.url.includes('/organisations'))).toHaveLength(2);
  });

  it('lists people same-origin and resolves organisation names once', async () => {
    const {calls, fetcher} = fetchStub(call => organisationsOr(call, json({data: [apiPerson], next_cursor: 'next-token'})));
    const client = createContactsApiClient(workspace, fetcher);
    const page = await client.listPeople({search: 'Maya', label: 'All', sort: 'name', status: 'active', cursor: ''});
    expect(page.nextCursor).toBe('next-token');
    expect(page.records[0]).toMatchObject({name: 'Maya Patel', organisation: 'Northstar Services'});
    await client.getPerson('per0000000000001');
    const urls = calls.map(call => call.url);
    expect(urls.filter(url => url.includes('/organisations'))).toHaveLength(1);
    expect(urls).toContain(`${base}/people?search=Maya&status=active&sort=name&direction=asc&limit=25`);
    expect(urls).toContain(`${base}/people/per0000000000001`);
    expect(calls.every(call => call.init.credentials === 'same-origin')).toBe(true);
  });

  it('sends create, update, and archive requests with the expected version', async () => {
    const {calls, fetcher} = fetchStub(call => organisationsOr(call, json(apiPerson, call.init.method === 'POST' ? 201 : 200)));
    const client = createContactsApiClient(workspace, fetcher);
    const input = {full_name: 'Maya Patel', email: null, telephone: null, organisation_id: null, tags: [], custom_fields: {}};
    await client.createPerson(input);
    await client.updatePerson('per0000000000001', 7, input);
    await client.archivePerson('per0000000000001', 8);
    await client.deletePerson?.('per0000000000001', 9);
    await client.undeletePerson?.('per0000000000001', 10);
    const writes = calls.filter(call => call.init.method);
    // Archive and delete are different: DELETE moves the record to the
    // recycle bin (ADR 0014).
    expect(writes.map(call => [call.init.method, call.url])).toEqual([
      ['POST', `${base}/people`],
      ['PUT', `${base}/people/per0000000000001`],
      ['POST', `${base}/people/per0000000000001/archive?expected_version=8`],
      ['DELETE', `${base}/people/per0000000000001?expected_version=9`],
      ['POST', `${base}/people/per0000000000001/undelete?expected_version=10`],
    ]);
    expect(JSON.parse(String(writes[1].init.body))).toMatchObject({expected_version: 7, full_name: 'Maya Patel'});
    expect(client.restorePerson).toBeUndefined();
    expect(client.capabilities?.labelFilter).toBeFalsy();
  });

  it('keeps the API problem code and field errors, and names network failures', async () => {
    const problem = {type: 'about:blank', title: 'Bad Request', status: 400, code: 'validation.failed', detail: 'one or more values are invalid', errors: [{field: 'email', code: 'email.invalid', detail: 'Enter a valid email address.'}]};
    const failing = createContactsApiClient(workspace, fetchStub(call => organisationsOr(call, json(problem, 400))).fetcher);
    const error = await failing.createPerson({full_name: 'x', email: 'bad', telephone: null, organisation_id: null, tags: [], custom_fields: {}}).catch((reason: unknown) => reason);
    expect(error).toBeInstanceOf(ContactsApiError);
    expect(error).toMatchObject({status: 400, code: 'validation.failed', errors: [{field: 'email', code: 'email.invalid'}]});

    const offline = createContactsApiClient(workspace, fetchStub(() => { throw new TypeError('Failed to fetch'); }).fetcher);
    await expect(offline.getPerson('per0000000000001')).rejects.toMatchObject({status: 0, code: 'network.unreachable'});

    const unauthenticated = createContactsApiClient(workspace, fetchStub(() => new Response('', {status: 401})).fetcher);
    await expect(unauthenticated.listOrganisations()).rejects.toMatchObject({status: 401, code: 'auth.unauthenticated'});
  });

  it('still lists people when organisation names cannot be read', async () => {
    const forbidden = fetchStub(call => call.url.includes('/organisations') ? json({status: 403, code: 'auth.permission_denied'}, 403) : json({data: [apiPerson], next_cursor: null}));
    const page = await createContactsApiClient(workspace, forbidden.fetcher).listPeople({search: '', label: 'All', sort: 'name', status: 'active', cursor: ''});
    expect(page.records[0]).toMatchObject({organisation: '', organisationId: 'org0000000000001'});

    const unavailable = fetchStub(call => call.url.includes('/organisations') ? json({status: 503, code: 'contacts.unavailable'}, 503) : json({data: [apiPerson], next_cursor: null}));
    const degraded = await createContactsApiClient(workspace, unavailable.fetcher).listPeople({search: '', label: 'All', sort: 'name', status: 'active', cursor: ''});
    expect(degraded.records[0].organisation).toBe('org0000000000001');
  });

  it('keeps only active custom field definitions', async () => {
    const definitions = [
      {api_name: 'relationship_type__c', label: 'Relationship', field_type: 'choice', required: false, options: ['Client'], disabled_at: null, archived_at: null},
      {api_name: 'legacy__c', label: 'Legacy', field_type: 'text', disabled_at: '2026-01-01T00:00:00Z', archived_at: null},
      {api_name: 'gone__c', label: 'Gone', field_type: 'text', disabled_at: null, archived_at: '2026-01-01T00:00:00Z'},
    ];
    const client = createContactsApiClient(workspace, fetchStub(() => json(definitions)).fetcher);
    expect(await client.listPersonFields()).toEqual([{apiName: 'relationship_type__c', label: 'Relationship', fieldType: 'choice', required: false, options: ['Client']}]);
  });
});

describe('extension fields through the kernel service', () => {
  it('loads descriptors, zones and placements, and saves layouts', async () => {
    const {calls, fetcher} = fetchStub(call => {
      if (call.url.startsWith(`/api/v1/workspaces/${workspace}/extensions/fields?`)) {
        return json({resource: 'platform.contacts.person', data: [
          {id: 'example.loyalty.tier', plugin_id: 'example.loyalty', plugin_name: 'Loyalty', target: 'platform.contacts.person', key: 'tier', label: 'Tier', description: null, field_type: 'choice', options: [{value: 'gold', label: 'Gold'}], max_length: null, writable: true},
          {id: 'example.future.shape', plugin_id: 'example.future', plugin_name: 'Future', target: 'platform.contacts.person', key: 'shape', label: 'Shape', field_type: 'polygon', writable: false},
        ], zones: [{id: 'highlights', label: 'Highlights', kind: 'display', page: 'p', node: 'n', default: false}], placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 1}]});
      }
      if (call.url.startsWith(`/api/v1/workspaces/${workspace}/extensions/layout?`)) {
        return call.init.method === 'PUT'
          ? json({revision: 4, placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 0}]})
          : json({revision: 3, fields: [], zones: [], placements: []});
      }
      return organisationsOr(call, json({...apiPerson, version: 8, extensions: {'example.loyalty.tier': 'gold'}}));
    });
    const client = createContactsApiClient(workspace, fetcher);
    const set = await client.loadExtensionFields?.('platform.contacts.person');
    expect(set?.fields.map(field => field.id)).toEqual(['example.loyalty.tier']);
    expect(set?.zones.map(zone => zone.id)).toEqual(['highlights']);
    expect(set?.placements).toEqual([{field_id: 'example.loyalty.tier', zone: 'highlights', position: 1}]);
    expect(calls[0]?.url).toBe(`/api/v1/workspaces/${workspace}/extensions/fields?resource=platform.contacts.person`);
    expect((await client.loadFieldLayout?.('platform.contacts.person'))?.revision).toBe(3);
    const saved = await client.saveFieldLayout?.('platform.contacts.person', 3, {arranged: ['example.loyalty.tier'], placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 0}]});
    expect(saved?.revision).toBe(4);
    const put = calls.find(call => call.init.method === 'PUT');
    expect(JSON.parse(String(put?.init.body))).toEqual({expected_version: undefined, expected_revision: 3, arranged: ['example.loyalty.tier'], placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 0}]});
  });

  it('sends changed extension values with a person update and reads them back', async () => {
    const {calls, fetcher} = fetchStub(call => organisationsOr(call, json({...apiPerson, version: 8, extensions: {'example.loyalty.tier': 'gold'}})));
    const client = createContactsApiClient(workspace, fetcher);
    const record = await client.updatePerson('per0000000000001', 7, {full_name: 'Maya Patel', email: null, telephone: null, organisation_id: null, tags: [], custom_fields: {}, extensions: {'example.loyalty.tier': 'gold'}});
    const update = calls.find(call => call.init.method === 'PUT');
    expect(JSON.parse(String(update?.init.body))).toMatchObject({expected_version: 7, extensions: {'example.loyalty.tier': 'gold'}});
    expect(record.extensions).toEqual({'example.loyalty.tier': 'gold'});
  });

  it('defaults missing extension values to an empty map', () => {
    expect(personFromApi(apiPerson).extensions).toEqual({});
  });
});

describe('standard record fields', () => {
  it('maps who created and last changed a person, naming former members', () => {
    const record = personFromApi({
      ...apiPerson,
      name: 'Maya Patel',
      created_by: {id: 'usr-1', name: 'Hannah Okafor'},
      updated_by: {id: 'usr-2', name: null},
      touched_at: '2026-09-30T08:00:00Z',
    });
    expect(record.createdBy).toEqual({id: 'usr-1', name: 'Hannah Okafor'});
    expect(record.updatedBy).toEqual({id: 'usr-2', name: null});
    expect(record.createdAt).toContain('2026');
    expect(record.touchedAt).toContain('2026');
    expect(record.deleted).toBe(false);
    expect(personFromApi({...apiPerson, is_deleted: true, deleted_at: '2026-10-01T00:00:00Z'}).deleted).toBe(true);
  });

  it('leaves stamps empty for records written before they were recorded', () => {
    const record = personFromApi({...apiPerson, created_by: null});
    expect(record.createdBy).toBeNull();
    expect(record.updatedBy).toBeNull();
    expect(record.touchedAt).toBe('');
  });
});
