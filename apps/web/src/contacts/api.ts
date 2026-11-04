// Typed access to the Contacts plugin API for the people workflows.
//
// Components talk to a `ContactsClient`; the production client calls the
// workspace-scoped API same-origin, and tests use a fixture client with the
// same contract. Errors keep the API's stable problem codes so the UI can map
// validation, denial, conflict, and outage states from real responses.

import {
  extensionFieldFromApi,
  fieldPlacementFromApi,
  fieldZoneFromApi,
  type ApiExtensionField,
  type ExtensionField,
  type ExtensionFieldSet,
  type FieldPlacement,
  type FieldZone,
} from '../plugin-ui/extension-fields/model';
import {formatRecordTimestamp, userReference, type UserReference} from '../plugin-ui/standard-fields/model';

export type PersonRecord = {
  id: string;
  name: string;
  email: string;
  phone: string;
  organisationId: string;
  organisation: string;
  role: string;
  label: string;
  tags: string[];
  customFields: Record<string, unknown>;
  updatedAt: string;
  updated: string;
  updatedOrder: number;
  version: number;
  archived: boolean;
  /** In the recycle bin (the standard `is_deleted` field). */
  deleted?: boolean;
  /** Standard record fields (ADR 0014), formatted for display. Empty when not recorded. */
  createdAt?: string;
  /** Members link to their user record; null when not recorded. */
  createdBy?: UserReference | null;
  lastModifiedAt?: string;
  updatedBy?: UserReference | null;
  touchedAt?: string;
  /** Values other features add to this person, as the kernel projected them for the caller. */
  extensions?: Record<string, unknown>;
};

export type PeopleQuery = {
  search: string;
  label: string;
  sort: 'name' | 'recent';
  status: 'active' | 'archived' | 'all';
  cursor: string;
};

export type PeoplePage = {
  records: PersonRecord[];
  nextCursor: string;
  /** Known only when the source can count the whole result set. */
  total?: number;
};

export type PersonInput = {
  full_name: string;
  email: string | null;
  telephone: string | null;
  organisation_id: string | null;
  tags: string[];
  custom_fields: Record<string, unknown>;
  /** Changed extension values, keyed by field id; null clears a value. Saved in the same request. */
  extensions?: Record<string, unknown>;
};

/** An administrator's view of where extension fields are placed on one resource. */
export type FieldLayoutState = {
  revision: number;
  fields: ExtensionField[];
  zones: FieldZone[];
  placements: FieldPlacement[];
};

export type FieldLayoutInput = {arranged: string[]; placements: FieldPlacement[]};

export type OrganisationOption = {id: string; name: string};

/** One timeline entry for a record, as the activity consumer projected it. */
export type ActivityEntry = {
  id: string;
  action: 'created' | 'updated' | 'archived' | string;
  version: number;
  actorId: string;
  occurredAt: string;
};

export type PersonFieldDefinition = {
  apiName: string;
  label: string;
  fieldType: string;
  required: boolean;
  options: string[];
};

export type ValidationIssue = {field: string; code: string; detail: string};

/** The Contacts resource other features extend with person fields (ADR 0012). */
export const personResource = 'platform.contacts.person';

export class ContactsApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly detail: string;
  readonly errors: ValidationIssue[];

  constructor(status: number, code: string, detail: string, errors: ValidationIssue[] = []) {
    super(`${code} (${status}): ${detail}`);
    this.name = 'ContactsApiError';
    this.status = status;
    this.code = code;
    this.detail = detail;
    this.errors = errors;
  }
}

/** A synchronously known outcome: the value, or the error the asynchronous call would reject with. */
export type Seed<T> = {value: T} | {error: ContactsApiError};

/** Synchronous seeds a client may offer so server rendering and tests can paint without waiting. */
export type ContactsClientSeeds = {
  people?: (query: PeopleQuery) => Seed<PeoplePage> | undefined;
  person?: (id: string) => Seed<PersonRecord> | undefined;
  organisations?: () => Seed<OrganisationOption[]> | undefined;
  personFields?: () => Seed<PersonFieldDefinition[]> | undefined;
  personActivity?: (id: string) => Seed<ActivityEntry[]> | undefined;
  extensionFields?: (resource: string) => Seed<ExtensionFieldSet> | undefined;
};

export type ContactsClient = {
  readonly workspaceId: string;
  listPeople(query: PeopleQuery, signal?: AbortSignal): Promise<PeoplePage>;
  getPerson(id: string, signal?: AbortSignal): Promise<PersonRecord>;
  createPerson(input: PersonInput): Promise<PersonRecord>;
  updatePerson(id: string, expectedVersion: number, input: PersonInput): Promise<PersonRecord>;
  archivePerson(id: string, expectedVersion: number): Promise<PersonRecord>;
  /** Absent when the backing API cannot restore an archived person. */
  restorePerson?(id: string, expectedVersion: number): Promise<PersonRecord>;
  /** Moves a person to the recycle bin (ADR 0014). */
  deletePerson?(id: string, expectedVersion: number): Promise<PersonRecord>;
  /** Restores a person from the recycle bin. */
  undeletePerson?(id: string, expectedVersion: number): Promise<PersonRecord>;
  listOrganisations(signal?: AbortSignal): Promise<OrganisationOption[]>;
  listPersonFields(signal?: AbortSignal): Promise<PersonFieldDefinition[]>;
  /** Most recent timeline entries for a person, newest first. */
  listPersonActivity(id: string, signal?: AbortSignal): Promise<ActivityEntry[]>;
  /** Fields other plugins add to a resource that the caller may read, with where they are placed; absent when the source has none. */
  loadExtensionFields?(resource: string, signal?: AbortSignal): Promise<ExtensionFieldSet>;
  /** Every extension field of a resource and its arrangement, for administrators. */
  loadFieldLayout?(resource: string, signal?: AbortSignal): Promise<FieldLayoutState>;
  /** Saves an arrangement when the stored revision still matches; returns the new revision and placements. */
  saveFieldLayout?(resource: string, expectedRevision: number, layout: FieldLayoutInput): Promise<{revision: number; placements: FieldPlacement[]}>;
  readonly seeds?: ContactsClientSeeds;
  /** What the backing source can do beyond the shared contract. */
  readonly capabilities?: {
    /** The source can filter people by the relationship label custom field. */
    labelFilter?: boolean;
  };
};

// --- API wire shapes -------------------------------------------------------

export type ApiPerson = {
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
  /** Standard record fields (ADR 0014). */
  name?: string;
  created_by?: UserReference | null;
  updated_by?: UserReference | null;
  touched_at?: string;
  is_deleted?: boolean;
  deleted_at?: string | null;
  extensions?: Record<string, unknown>;
};

type ApiOrganisation = {id: string; name: string};

type ApiPage<T> = {data: T[]; next_cursor: string | null};

type ApiDefinition = {
  api_name: string;
  label: string;
  field_type: string;
  required?: boolean;
  options?: string[] | null;
  disabled_at?: string | null;
  archived_at?: string | null;
};

type ApiProblem = {status?: number; code?: string; detail?: string; errors?: ValidationIssue[]};

type ApiActivityEntry = {event_id: string; action: string; entity_version: number; actor_id: string | null; occurred_at: string};

export function activityFromApi(entry: ApiActivityEntry): ActivityEntry {
  return {id: entry.event_id, action: entry.action, version: entry.entity_version, actorId: entry.actor_id ?? '', occurredAt: entry.occurred_at};
}

export const defaultPageSize = 25;

/** The relationship labels the fixture catalogue and directory filter understand. */
export const relationshipLabels = ['Client', 'Prospect', 'Partner'] as const;

export function describeUpdated(updatedAt: string, now = Date.now()): string {
  const timestamp = Date.parse(updatedAt);
  if (Number.isNaN(timestamp)) return '';
  const days = Math.floor((now - timestamp) / 86_400_000);
  if (days <= 0) return 'Today';
  if (days === 1) return 'Yesterday';
  if (days < 7) return `${days} days ago`;
  if (days < 14) return '1 week ago';
  if (days < 30) return `${Math.floor(days / 7)} weeks ago`;
  return new Date(timestamp).toLocaleDateString('en-GB', {day: 'numeric', month: 'short', year: 'numeric'});
}

function text(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

/**
 * Map an API person onto the record the metadata resolvers render. `organisations`
 * is `null` when the caller may not read organisations: the link is kept but no
 * name or identifier is shown.
 */
export function personFromApi(person: ApiPerson, organisations: ReadonlyMap<string, string> | null = new Map(), now = Date.now()): PersonRecord {
  const customFields = person.custom_fields ?? {};
  const label = text(customFields.relationship_type__c);
  return {
    id: person.id,
    name: person.full_name,
    email: person.email ?? '',
    phone: person.telephone ?? '',
    organisationId: person.organisation_id ?? '',
    organisation: person.organisation_id && organisations ? organisations.get(person.organisation_id) ?? person.organisation_id : '',
    role: text(customFields.job_title),
    label,
    tags: person.tags ?? [],
    customFields,
    updatedAt: person.updated_at,
    updated: describeUpdated(person.updated_at, now),
    updatedOrder: Date.parse(person.updated_at) || 0,
    version: person.version,
    archived: person.archived_at !== null,
    deleted: person.is_deleted === true,
    createdAt: formatRecordTimestamp(person.created_at),
    createdBy: userReference(person.created_by),
    lastModifiedAt: formatRecordTimestamp(person.updated_at),
    updatedBy: userReference(person.updated_by),
    touchedAt: formatRecordTimestamp(person.touched_at),
    extensions: person.extensions ?? {},
  };
}

/** The API matches `search` against the full name only; an address-shaped term is sent as an exact email match. */
export function peopleQueryParams(query: PeopleQuery): URLSearchParams {
  const params = new URLSearchParams();
  const term = query.search.trim().slice(0, 200);
  if (term.includes('@')) params.set('email', term.toLocaleLowerCase());
  else if (term) params.set('search', term);
  params.set('status', query.status);
  if (query.sort === 'recent') {
    params.set('sort', 'updated_at');
    params.set('direction', 'desc');
  } else {
    params.set('sort', 'name');
    params.set('direction', 'asc');
  }
  params.set('limit', String(defaultPageSize));
  if (query.cursor) params.set('cursor', query.cursor);
  return params;
}

async function parseProblem(response: Response): Promise<ContactsApiError> {
  const problem = await response.json().catch(() => ({})) as ApiProblem;
  return new ContactsApiError(
    response.status,
    problem.code ?? (response.status === 401 ? 'auth.unauthenticated' : 'contacts.unavailable'),
    problem.detail ?? response.statusText,
    Array.isArray(problem.errors) ? problem.errors : [],
  );
}

/** Upper bound on organisations read into the name cache; beyond it names fall back to identifiers. */
export const organisationCatalogueLimit = 5000;

export function createContactsApiClient(workspaceId: string, fetcher: typeof fetch = (...args) => fetch(...args)): ContactsClient {
  const workspaceBase = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}`;
  const base = `${workspaceBase}/contacts`;
  let organisationCache: Promise<OrganisationOption[]> | null = null;

  async function request<T>(path: string, init: RequestInit = {}, signal?: AbortSignal, root = base): Promise<T> {
    let response: Response;
    try {
      response = await fetcher(`${root}${path}`, {
        ...init,
        headers: {accept: 'application/json', ...(init.body ? {'content-type': 'application/json'} : {}), ...init.headers},
        credentials: 'same-origin',
        signal,
      });
    } catch (error) {
      if (signal?.aborted) throw error;
      throw new ContactsApiError(0, 'network.unreachable', 'The Contacts service could not be reached.');
    }
    if (!response.ok) throw await parseProblem(response);
    return response.json() as Promise<T>;
  }

  // The organisation catalogue is shared by every caller, so it is read
  // without any one caller's abort signal and follows the API's cursors to
  // the end rather than treating one page as the whole set.
  const readOrganisations = async () => {
    const all: OrganisationOption[] = [];
    let cursor: string | null = null;
    do {
      const params = new URLSearchParams({status: 'active', sort: 'name', limit: '200'});
      if (cursor) params.set('cursor', cursor);
      const page: ApiPage<ApiOrganisation> = await request<ApiPage<ApiOrganisation>>(`/organisations?${params}`);
      all.push(...page.data.map(item => ({id: item.id, name: item.name})));
      cursor = page.next_cursor;
    } while (cursor && all.length < organisationCatalogueLimit);
    return all;
  };
  const organisations = () => {
    organisationCache ??= readOrganisations().catch(error => {
      organisationCache = null;
      throw error;
    });
    return organisationCache;
  };

  const organisationNames = async (): Promise<ReadonlyMap<string, string> | null> => {
    try {
      return new Map((await organisations()).map(item => [item.id, item.name]));
    } catch (error) {
      // People still render when organisations cannot be read; a caller without
      // permission sees no organisation name or identifier.
      if (error instanceof ContactsApiError && error.status === 403) return null;
      if (error instanceof ContactsApiError && error.status !== 401) return new Map<string, string>();
      throw error;
    }
  };

  return {
    workspaceId,
    async listPeople(query, signal) {
      const [page, names] = await Promise.all([
        request<ApiPage<ApiPerson>>(`/people?${peopleQueryParams(query)}`, {}, signal),
        organisationNames(),
      ]);
      return {records: page.data.map(person => personFromApi(person, names)), nextCursor: page.next_cursor ?? ''};
    },
    async getPerson(id, signal) {
      const [person, names] = await Promise.all([
        request<ApiPerson>(`/people/${encodeURIComponent(id)}`, {}, signal),
        organisationNames(),
      ]);
      return personFromApi(person, names);
    },
    async createPerson(input) {
      const person = await request<ApiPerson>('/people', {method: 'POST', body: JSON.stringify(input)});
      return personFromApi(person, await organisationNames());
    },
    async updatePerson(id, expectedVersion, input) {
      const person = await request<ApiPerson>(`/people/${encodeURIComponent(id)}`, {method: 'PUT', body: JSON.stringify({expected_version: expectedVersion, ...input})});
      return personFromApi(person, await organisationNames());
    },
    async archivePerson(id, expectedVersion) {
      const person = await request<ApiPerson>(`/people/${encodeURIComponent(id)}/archive?${new URLSearchParams({expected_version: String(expectedVersion)})}`, {method: 'POST'});
      return personFromApi(person, await organisationNames());
    },
    async deletePerson(id, expectedVersion) {
      const person = await request<ApiPerson>(`/people/${encodeURIComponent(id)}?${new URLSearchParams({expected_version: String(expectedVersion)})}`, {method: 'DELETE'});
      return personFromApi(person, await organisationNames());
    },
    async undeletePerson(id, expectedVersion) {
      const person = await request<ApiPerson>(`/people/${encodeURIComponent(id)}/undelete?${new URLSearchParams({expected_version: String(expectedVersion)})}`, {method: 'POST'});
      return personFromApi(person, await organisationNames());
    },
    listOrganisations: () => organisations(),
    async listPersonActivity(id, signal) {
      const page = await request<{data: ApiActivityEntry[]}>(`/people/${encodeURIComponent(id)}/activity`, {}, signal);
      return page.data.map(activityFromApi).sort((left, right) => right.occurredAt.localeCompare(left.occurredAt));
    },
    async loadExtensionFields(resource, signal) {
      const page = await request<{data: ApiExtensionField[]; zones?: unknown[]; placements?: unknown[]}>(`/extensions/fields?${new URLSearchParams({resource})}`, {}, signal, workspaceBase);
      return {
        fields: page.data.map(extensionFieldFromApi).filter((field): field is ExtensionField => field !== null),
        zones: (page.zones ?? []).map(fieldZoneFromApi).filter((zone): zone is FieldZone => zone !== null),
        placements: (page.placements ?? []).map(fieldPlacementFromApi).filter((placement): placement is FieldPlacement => placement !== null),
      };
    },
    async loadFieldLayout(resource, signal) {
      const state = await request<{revision: number; fields: ApiExtensionField[]; zones: unknown[]; placements: unknown[]}>(`/extensions/layout?${new URLSearchParams({resource})}`, {}, signal, workspaceBase);
      return {
        revision: state.revision,
        fields: state.fields.map(extensionFieldFromApi).filter((field): field is ExtensionField => field !== null),
        zones: state.zones.map(fieldZoneFromApi).filter((zone): zone is FieldZone => zone !== null),
        placements: state.placements.map(fieldPlacementFromApi).filter((placement): placement is FieldPlacement => placement !== null),
      };
    },
    async saveFieldLayout(resource, expectedRevision, layout) {
      const saved = await request<{revision: number; placements: unknown[]}>(`/extensions/layout?${new URLSearchParams({resource})}`, {method: 'PUT', body: JSON.stringify({expected_revision: expectedRevision, ...layout})}, undefined, workspaceBase);
      return {revision: saved.revision, placements: saved.placements.map(fieldPlacementFromApi).filter((placement): placement is FieldPlacement => placement !== null)};
    },
    async listPersonFields(signal) {
      const definitions = await request<{data: ApiDefinition[]} | ApiDefinition[]>('/custom-fields/person', {}, signal);
      const list = Array.isArray(definitions) ? definitions : definitions.data;
      return list
        .filter(definition => !definition.disabled_at && !definition.archived_at)
        .map(definition => ({
          apiName: definition.api_name,
          label: definition.label,
          fieldType: definition.field_type,
          required: definition.required === true,
          options: definition.options ?? [],
        }));
    },
  };
}
