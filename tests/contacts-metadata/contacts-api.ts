import type {Page, Route} from '@playwright/test';
import {organisations} from '../../apps/web/src/contacts/organisations-data';
import {people} from '../../apps/web/src/contacts/people-data';

// The browser suites run the production people workflows against the same
// Contacts API contract the server exposes, answered here from the fixture
// catalogue so screenshots and journeys are deterministic and need no server.
type ApiPerson = {
  id: string;
  full_name: string;
  email: string | null;
  telephone: string | null;
  organisation_id: string | null;
  tags: string[];
  custom_fields: Record<string, unknown>;
  version: number;
  archived_at: string | null;
  created_at: string;
  updated_at: string;
};

const daysAgoByOrder: Record<number, number> = {8: 0, 7: 1, 6: 2, 5: 4, 4: 6, 3: 7, 2: 14, 1: 21};

function isoDaysAgo(days: number) {
  return new Date(Date.now() - days * 86_400_000).toISOString();
}

function organisationId(name: string) {
  return organisations.find(item => item.name === name)?.id ?? null;
}

function problem(status: number, code: string, detail: string, errors: Array<{field: string; code: string; detail: string}> = []) {
  return {status, contentType: 'application/problem+json', body: JSON.stringify({type: 'about:blank', title: detail, status, code, detail, errors})};
}

export function createContactsApiStub() {
  const records = new Map<string, ApiPerson>(people.map(person => [person.id, {
    id: person.id,
    full_name: person.name,
    email: person.email,
    telephone: person.phone,
    organisation_id: organisationId(person.organisation),
    tags: person.tags,
    custom_fields: {relationship_type__c: person.label, service_tier__c: person.tags.includes('priority') ? 'priority' : 'standard', job_title: person.role},
    version: person.version,
    archived_at: null,
    created_at: isoDaysAgo(60),
    updated_at: isoDaysAgo(daysAgoByOrder[person.updatedOrder] ?? 30),
  }]));
  let sequence = 0;

  const definitions = [
    {id: 'def-relationship', applies_to: 'person', api_name: 'relationship_type__c', label: 'Relationship', field_type: 'choice', required: false, options: ['Client', 'Prospect', 'Partner'], disabled_at: null, archived_at: null},
    {id: 'def-tier', applies_to: 'person', api_name: 'service_tier__c', label: 'Service tier', field_type: 'choice', required: false, options: ['standard', 'priority'], disabled_at: null, archived_at: null},
  ];

  function list(url: URL) {
    const params = url.searchParams;
    const search = (params.get('search') ?? '').toLocaleLowerCase();
    const status = params.get('status') ?? 'active';
    const limit = Math.min(Math.max(Number(params.get('limit') ?? 50), 1), 200);
    const sort = params.get('sort') ?? 'name';
    const direction = params.get('direction') ?? 'asc';
    const filtered = [...records.values()]
      .filter(person => status === 'all' || (status === 'archived') === (person.archived_at !== null))
      .filter(person => !search || [person.full_name, person.email ?? '', organisations.find(item => item.id === person.organisation_id)?.name ?? ''].some(value => value.toLocaleLowerCase().includes(search)))
      .sort((left, right) => {
        const compare = sort === 'updated_at' ? left.updated_at.localeCompare(right.updated_at) : sort === 'created_at' ? left.created_at.localeCompare(right.created_at) : left.full_name.localeCompare(right.full_name);
        return direction === 'desc' ? -compare : compare;
      });
    const cursor = params.get('cursor');
    const start = cursor ? Number(cursor.replace('offset-', '')) || 0 : 0;
    const data = filtered.slice(start, start + limit);
    return {data, next_cursor: start + limit < filtered.length ? `offset-${start + limit}` : null};
  }

  function validate(body: Record<string, unknown>, existingId?: string) {
    const errors: Array<{field: string; code: string; detail: string}> = [];
    const fullName = typeof body.full_name === 'string' ? body.full_name.trim() : '';
    if (!fullName) errors.push({field: 'full_name', code: 'full_name.required', detail: 'Enter the person’s full name.'});
    const email = typeof body.email === 'string' ? body.email.trim() : '';
    if (email && [...records.values()].some(person => person.id !== existingId && person.email === email)) errors.push({field: 'email', code: 'person.duplicate', detail: 'A person with this email already exists.'});
    for (const key of Object.keys((body.custom_fields as Record<string, unknown> | undefined) ?? {})) {
      if (!definitions.some(definition => definition.api_name === key)) errors.push({field: key, code: 'custom_field.undefined', detail: `custom field ${key} is not defined for this workspace`});
    }
    return errors;
  }

  async function handle(route: Route): Promise<boolean> {
    const request = route.request();
    const url = new URL(request.url());
    const match = url.pathname.match(/^\/api\/v1\/workspaces\/[^/]+\/contacts\/(.*)$/);
    if (!match) return false;
    const [, rest] = match;
    const method = request.method();

    if (rest === 'organisations' && method === 'GET') {
      await route.fulfill({json: {data: organisations.map(item => ({id: item.id, name: item.name})), next_cursor: null}});
      return true;
    }
    if (rest === 'custom-fields/person' && method === 'GET') {
      await route.fulfill({json: definitions});
      return true;
    }
    if (rest === 'people' && method === 'GET') {
      await route.fulfill({json: list(url)});
      return true;
    }
    if (rest === 'people' && method === 'POST') {
      const body = request.postDataJSON() as Record<string, unknown>;
      const errors = validate(body);
      if (errors.length) { await route.fulfill(problem(400, 'validation.failed', 'one or more values are invalid', errors)); return true; }
      sequence += 1;
      const now = new Date().toISOString();
      const created: ApiPerson = {
        id: `per000000000${String(sequence).padStart(4, '0')}`,
        full_name: String(body.full_name).trim(),
        email: typeof body.email === 'string' && body.email ? body.email : null,
        telephone: typeof body.telephone === 'string' && body.telephone ? body.telephone : null,
        organisation_id: typeof body.organisation_id === 'string' && body.organisation_id ? body.organisation_id : null,
        tags: Array.isArray(body.tags) ? body.tags.map(String) : [],
        custom_fields: (body.custom_fields as Record<string, unknown>) ?? {},
        version: 1,
        archived_at: null,
        created_at: now,
        updated_at: now,
      };
      records.set(created.id, created);
      await route.fulfill({status: 201, json: created});
      return true;
    }
    const activity = rest.match(/^people\/([^/?]+)\/activity$/);
    if (activity && method === 'GET') {
      const current = records.get(decodeURIComponent(activity[1]));
      if (!current) { await route.fulfill(problem(404, 'contacts.record_not_found', 'the contact record was not found')); return true; }
      const data = [{event_id: `${current.id}-created`, action: 'created', entity_version: 1, actor_id: 'usr-hannah', occurred_at: current.created_at}];
      if (current.version > 1) data.push({event_id: `${current.id}-updated`, action: 'updated', entity_version: current.version, actor_id: 'usr-morgan', occurred_at: current.updated_at});
      if (current.archived_at) data.push({event_id: `${current.id}-archived`, action: 'archived', entity_version: current.version, actor_id: 'usr-hannah', occurred_at: current.archived_at});
      await route.fulfill({json: {data, next_cursor: null}});
      return true;
    }
    const person = rest.match(/^people\/([^/?]+)$/);
    if (person) {
      const current = records.get(decodeURIComponent(person[1]));
      if (!current) { await route.fulfill(problem(404, 'contacts.record_not_found', 'the contact record was not found')); return true; }
      if (method === 'GET') { await route.fulfill({json: current}); return true; }
      if (method === 'PUT') {
        const body = request.postDataJSON() as Record<string, unknown>;
        if (Number(body.expected_version) !== current.version) { await route.fulfill(problem(409, 'contacts.version_conflict', 'the record changed since it was read')); return true; }
        const errors = validate(body, current.id);
        if (errors.length) { await route.fulfill(problem(400, 'validation.failed', 'one or more values are invalid', errors)); return true; }
        const updated: ApiPerson = {
          ...current,
          full_name: String(body.full_name).trim(),
          email: typeof body.email === 'string' && body.email ? body.email : null,
          telephone: typeof body.telephone === 'string' && body.telephone ? body.telephone : null,
          organisation_id: typeof body.organisation_id === 'string' && body.organisation_id ? body.organisation_id : null,
          tags: Array.isArray(body.tags) ? body.tags.map(String) : [],
          custom_fields: (body.custom_fields as Record<string, unknown>) ?? {},
          version: current.version + 1,
          updated_at: new Date().toISOString(),
        };
        records.set(updated.id, updated);
        await route.fulfill({json: updated});
        return true;
      }
      if (method === 'DELETE') {
        if (Number(url.searchParams.get('expected_version')) !== current.version) { await route.fulfill(problem(409, 'contacts.version_conflict', 'the record changed since it was read')); return true; }
        const archived: ApiPerson = {...current, version: current.version + 1, archived_at: new Date().toISOString(), updated_at: new Date().toISOString()};
        records.set(archived.id, archived);
        await route.fulfill({json: archived});
        return true;
      }
    }
    await route.fulfill(problem(404, 'not_found', 'no such contacts route'));
    return true;
  }

  return {handle, records};
}

export async function stubContactsApi(page: Page) {
  const stub = createContactsApiStub();
  await page.route('**/api/v1/workspaces/*/contacts/**', async route => {
    if (!(await stub.handle(route))) await route.fallback();
  });
  return stub;
}
