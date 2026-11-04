// A `ContactsClient` over the in-repo fixture catalogue. It answers
// synchronously through `seeds`, so server-rendered tests and the browser
// suites paint complete pages without a network, and it can be forced into
// each state the workflows must handle.
import {organisations} from './organisations-data';
import {organisationIdFor, people, peopleFixtureWorkspaceId, type Person} from './people-data';
import {
  ContactsApiError,
  type ContactsClient,
  type OrganisationOption,
  type PeoplePage,
  type PeopleQuery,
  type PersonFieldDefinition,
  type PersonInput,
  type ActivityEntry,
  type PersonRecord,
  type Seed,
} from './api';
import type {ExtensionFieldSet, FieldPlacement} from '../plugin-ui/extension-fields/model';

export type FixtureListState = 'ready' | 'loading' | 'empty' | 'denied' | 'error';
export type FixtureRecordState = 'ready' | 'missing' | 'denied' | 'error' | 'archived' | 'loading';
export type FixtureWriteState = 'ready' | 'denied' | 'error' | 'duplicate' | 'conflict';

export type FixtureClientOptions = {
  workspaceId?: string;
  list?: FixtureListState;
  record?: FixtureRecordState;
  write?: FixtureWriteState;
  archive?: 'ready' | 'error';
  restore?: 'ready' | 'error';
  /** Custom fields the fixture tenant has configured; defaults to the fixture catalogue's two. */
  fields?: PersonFieldDefinition[];
  /** Fields other plugins add to people, with zones and placements (ADR 0012); omitted means the source has no extensions. */
  extensionFields?: ExtensionFieldSet | 'error';
  /** Extension values per person id. */
  extensionValues?: Record<string, Record<string, unknown>>;
};

export const fixturePageSize = 6;

export const fixturePersonFields: PersonFieldDefinition[] = [
  {apiName: 'relationship_type__c', label: 'Relationship', fieldType: 'choice', required: false, options: ['Client', 'Prospect', 'Partner']},
  {apiName: 'service_tier__c', label: 'Service tier', fieldType: 'choice', required: false, options: ['standard', 'priority']},
];

/** The member the fixture client acts as. */
export const fixtureCurrentUser = {id: 'usr-hannah', name: 'Hannah Okafor'};

export function fixtureRecord(person: Person, archived = false): PersonRecord {
  return {
    id: person.id,
    name: person.name,
    email: person.email,
    phone: person.phone,
    organisationId: organisationIdFor(person.organisation),
    organisation: person.organisation,
    role: person.role,
    label: person.label,
    tags: person.tags,
    customFields: {relationship_type__c: person.label, service_tier__c: person.tags.includes('priority') ? 'priority' : 'standard', job_title: person.role},
    updatedAt: '',
    updated: person.updated,
    updatedOrder: person.updatedOrder,
    version: person.version,
    archived,
    createdAt: '2 Mar 2026, 09:15',
    createdBy: {id: 'usr-hannah', name: 'Hannah Okafor'},
    updatedBy: person.version > 1 ? {id: 'usr-morgan', name: 'Morgan Lee'} : {id: 'usr-hannah', name: 'Hannah Okafor'},
    touchedAt: person.updated,
  };
}

function denied(): ContactsApiError {
  return new ContactsApiError(403, 'auth.permission_denied', 'permission denied');
}

function unreachable(): ContactsApiError {
  return new ContactsApiError(0, 'network.unreachable', 'The Contacts service could not be reached.');
}

function pending<T>(): Promise<T> {
  return new Promise<T>(() => undefined);
}

export function fixturePeoplePage(query: PeopleQuery, source: readonly Person[] = people): PeoplePage {
  const needle = query.search.trim().toLocaleLowerCase();
  const filtered = source
    .filter(person => {
      // Mirrors the API: the term matches the name, or an address-shaped term matches the email exactly.
      const matchesText = !needle || (needle.includes('@') ? person.email.toLocaleLowerCase() === needle : person.name.toLocaleLowerCase().includes(needle));
      return matchesText && (query.label === 'All' || !query.label || person.label === query.label);
    })
    .sort((left, right) => query.sort === 'recent' ? right.updatedOrder - left.updatedOrder : left.name.localeCompare(right.name));
  const start = query.cursor === 'page-2' ? fixturePageSize : 0;
  const visible = filtered.slice(start, start + fixturePageSize);
  return {
    records: visible.map(person => fixtureRecord(person)),
    nextCursor: start + fixturePageSize < filtered.length ? 'page-2' : '',
    total: filtered.length,
  };
}

/** A plausible timeline for a fixture person: creation, then one update per version. */
export function fixtureActivity(person: Person): ActivityEntry[] {
  const base = Date.parse('2026-08-12T09:00:00Z');
  const entries: ActivityEntry[] = [{id: `${person.id}-v1`, action: 'created', version: 1, actorId: 'usr-hannah', occurredAt: new Date(base).toISOString()}];
  for (let version = 2; version <= person.version; version += 1) {
    entries.push({id: `${person.id}-v${version}`, action: 'updated', version, actorId: version % 2 ? 'usr-hannah' : 'usr-morgan', occurredAt: new Date(base + version * 5 * 86_400_000).toISOString()});
  }
  return entries.reverse();
}

function mergeExtensions(stored: Record<string, unknown> | undefined, changes: Record<string, unknown> | undefined): Record<string, unknown> {
  const merged = {...stored};
  for (const [key, value] of Object.entries(changes ?? {})) {
    if (value === null) delete merged[key];
    else merged[key] = value;
  }
  return merged;
}

export function createFixtureContactsClient(options: FixtureClientOptions = {}): ContactsClient {
  const {workspaceId = peopleFixtureWorkspaceId, list = 'ready', record = 'ready', write = 'ready', archive = 'ready', restore = 'ready', fields = fixturePersonFields} = options;
  const organisationOptions: OrganisationOption[] = organisations.map(item => ({id: item.id, name: item.name}));
  const archivedIds = new Set<string>();
  let layoutRevision = 0;
  let savedPlacements: FieldPlacement[] | null = null;
  const currentExtensions = (): ExtensionFieldSet | 'error' | undefined => {
    const configured = options.extensionFields;
    if (configured === undefined || configured === 'error' || savedPlacements === null) return configured;
    return {...configured, placements: savedPlacements};
  };
  const overrides = new Map<string, PersonRecord>();

  const find = (id: string): PersonRecord | undefined => {
    if (record === 'missing') return undefined;
    const override = overrides.get(id);
    if (override) return override;
    const person = people.find(item => item.id === id);
    if (!person) return undefined;
    const base = fixtureRecord(person, record === 'archived' || archivedIds.has(id));
    return options.extensionFields === undefined ? base : {...base, extensions: options.extensionValues?.[id] ?? {}};
  };

  const notFound = () => new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
  const peek = {
    people: (query: PeopleQuery): Seed<PeoplePage> | undefined => {
      if (list === 'loading') return undefined;
      if (list === 'denied') return {error: denied()};
      if (list === 'error') return {error: unreachable()};
      if (list === 'empty') return {value: {records: [], nextCursor: '', total: 0}};
      return {value: fixturePeoplePage(query)};
    },
    person: (id: string): Seed<PersonRecord> | undefined => {
      if (record === 'loading') return undefined;
      if (record === 'denied') return {error: denied()};
      if (record === 'error') return {error: unreachable()};
      const found = find(id);
      return found ? {value: found} : {error: notFound()};
    },
    organisations: (): Seed<OrganisationOption[]> => ({value: organisationOptions}),
    personFields: (): Seed<PersonFieldDefinition[]> => ({value: fields}),
    personActivity: (id: string): Seed<ActivityEntry[]> | undefined => {
      if (record === 'loading') return undefined;
      const person = people.find(item => item.id === id);
      return person ? {value: fixtureActivity(person)} : {error: notFound()};
    },
    extensionFields: (): Seed<ExtensionFieldSet> | undefined => {
      const configured = currentExtensions();
      if (configured === undefined) return undefined;
      return configured === 'error' ? {error: unreachable()} : {value: configured};
    },
  };

  // Mirrors the kernel extension service closely enough for UI states: the
  // placements an administrator saves become what pages receive.
  const extensionMethods: Pick<ContactsClient, 'loadExtensionFields' | 'loadFieldLayout' | 'saveFieldLayout'> = options.extensionFields === undefined ? {} : {
    async loadExtensionFields() {
      const configured = currentExtensions();
      if (configured === 'error' || configured === undefined) throw unreachable();
      return configured;
    },
    async loadFieldLayout() {
      const configured = currentExtensions();
      if (configured === 'error' || configured === undefined) throw unreachable();
      return {revision: layoutRevision, ...configured};
    },
    async saveFieldLayout(_resource, expectedRevision, layout) {
      if (write === 'denied') throw denied();
      if (write === 'error') throw unreachable();
      if (expectedRevision !== layoutRevision) throw new ContactsApiError(409, 'extension.layout_conflict', 'the layout changed since it was read');
      layoutRevision += 1;
      savedPlacements = layout.placements;
      return {revision: layoutRevision, placements: layout.placements};
    },
  };

  const applyWrite = (id: string | undefined, input: PersonInput, expectedVersion?: number): PersonRecord => {
    const current = id ? find(id) : undefined;
    if (write === 'denied') throw denied();
    if (write === 'error') throw unreachable();
    if (write === 'duplicate') throw new ContactsApiError(400, 'validation.failed', 'one or more values are invalid', [{field: 'email', code: 'person.duplicate', detail: 'A person with this email already exists.'}]);
    if (write === 'conflict' && current && expectedVersion === current.version) {
      overrides.set(current.id, {...current, email: 'maya.patel@northstar.example', version: current.version + 1});
      throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
    }
    if (id && !current) throw new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
    if (current && expectedVersion !== undefined && expectedVersion !== current.version) throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
    const organisation = organisationOptions.find(item => item.id === input.organisation_id);
    const next: PersonRecord = {
      id: current?.id ?? `per${Math.random().toString(36).slice(2, 17).padEnd(15, '0')}`,
      name: input.full_name,
      email: input.email ?? '',
      phone: input.telephone ?? '',
      organisationId: organisation?.id ?? '',
      organisation: organisation?.name ?? '',
      role: typeof input.custom_fields.job_title === 'string' ? input.custom_fields.job_title : current?.role ?? '',
      label: typeof input.custom_fields.relationship_type__c === 'string' ? input.custom_fields.relationship_type__c : current?.label ?? '',
      tags: input.tags,
      customFields: input.custom_fields,
      updatedAt: new Date().toISOString(),
      updated: 'Today',
      updatedOrder: Number.MAX_SAFE_INTEGER,
      version: (current?.version ?? 0) + 1,
      archived: current?.archived ?? false,
      createdAt: current?.createdAt ?? 'Today',
      createdBy: current?.createdBy ?? fixtureCurrentUser,
      updatedBy: fixtureCurrentUser,
      touchedAt: 'Today',
      ...(current?.extensions || input.extensions ? {extensions: mergeExtensions(current?.extensions, input.extensions)} : {}),
    };
    overrides.set(next.id, next);
    return next;
  };

  return {
    ...extensionMethods,
    workspaceId,
    seeds: peek,
    capabilities: {labelFilter: true},
    async listPeople(query) {
      if (list === 'loading') return pending();
      if (list === 'denied') throw denied();
      if (list === 'error') throw unreachable();
      return fixturePeoplePage(query);
    },
    async getPerson(id) {
      if (record === 'loading') return pending();
      if (record === 'denied') throw denied();
      if (record === 'error') throw unreachable();
      const found = find(id);
      if (!found) throw notFound();
      return found;
    },
    async createPerson(input) {
      return applyWrite(undefined, input);
    },
    async updatePerson(id, expectedVersion, input) {
      return applyWrite(id, input, expectedVersion);
    },
    async archivePerson(id, expectedVersion) {
      if (archive === 'error') throw unreachable();
      const current = find(id);
      if (!current) throw new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
      if (current.version !== expectedVersion) throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
      archivedIds.add(id);
      const next = {...current, archived: true, version: current.version + 1};
      overrides.set(id, next);
      return next;
    },
    async restorePerson(id, expectedVersion) {
      if (restore === 'error') throw unreachable();
      const current = find(id);
      if (!current) throw new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
      if (current.version !== expectedVersion) throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
      archivedIds.delete(id);
      const next = {...current, archived: false, version: current.version + 1};
      overrides.set(id, next);
      return next;
    },
    async deletePerson(id, expectedVersion) {
      const current = find(id);
      if (!current || current.deleted) throw new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
      if (current.version !== expectedVersion) throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
      const next = {...current, deleted: true, version: current.version + 1, updatedBy: fixtureCurrentUser};
      overrides.set(id, next);
      return next;
    },
    async undeletePerson(id, expectedVersion) {
      const current = find(id);
      if (!current?.deleted) throw new ContactsApiError(404, 'contacts.record_not_found', 'the contact record was not found');
      if (current.version !== expectedVersion) throw new ContactsApiError(409, 'contacts.version_conflict', 'the record changed since it was read');
      const next = {...current, deleted: false, version: current.version + 1, updatedBy: fixtureCurrentUser};
      overrides.set(id, next);
      return next;
    },
    async listOrganisations() {
      return organisationOptions;
    },
    async listPersonFields() {
      return fields;
    },
    async listPersonActivity(id) {
      if (record === 'loading') return pending();
      const person = people.find(item => item.id === id);
      if (!person) throw notFound();
      return fixtureActivity(person);
    },
  };
}
