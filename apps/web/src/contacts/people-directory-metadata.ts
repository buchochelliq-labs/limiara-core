import {formatExtensionResourceValue, placeExtensionFields, type ExtensionFieldSet} from '../plugin-ui/extension-fields/model';
import peoplePageDocument from '../../../../plugins/contacts/ui/pages/people-directory.json';
import type {
  CallerSafeFieldDescriptor,
  CallerSafeResourceDescriptor,
  CollectionItem,
  PageDefinition,
  RecordSummary,
  ResolvedPage,
  UiNode,
} from '../plugin-ui/contracts';
import type {PeoplePage, PersonRecord} from './api';
import {contactsPersonPermissions, resolveContactsPersonResource} from './person-metadata';
import {userReference} from '../plugin-ui/standard-fields/model';

export type PeopleDirectoryState = 'ready' | 'loading' | 'empty' | 'denied' | 'error' | 'disabled' | 'archived';

export type PeopleDirectoryQuery = {
  query: string;
  label: string;
  sort: 'name' | 'recent';
  /** Opaque cursor of the page being shown; empty for the first page. */
  cursor: string;
  selected: string;
  /** How the directory presents records; the host falls back to rows on compact screens. */
  view?: 'table' | 'cards';
};

export type ResolvedPeopleDirectory = {
  page: ResolvedPage;
  resources: CallerSafeResourceDescriptor[];
  dataSources: {directory: Record<string, unknown>};
  announcement: string;
};

type ResolveOptions = {
  query: PeopleDirectoryQuery;
  selectedId?: string;
  state?: PeopleDirectoryState;
  /** The page of records loaded for `query`; absent while loading or unavailable. */
  page?: PeoplePage;
  /** The selected record when it is known, whether or not it is on the current page. */
  selected?: PersonRecord;
  /** True while the selected record is still being read. */
  selectedPending?: boolean;
  /** Cursor that returns to the previous page; `first-page` returns to the start. */
  previousCursor?: string;
  /** Whether the directory source can filter by relationship label. */
  labelFilter?: boolean;
  pluginEnabled?: boolean;
  permissions: ReadonlySet<string>;
  /** Every field api name the caller may see; unset means every readable resource field. */
  configuredFields?: ReadonlySet<string>;
  /** Tenant custom field api names configured for people; unset keeps the plugin's declared custom fields. */
  configuredCustomFields?: ReadonlySet<string>;
  /** Fields other plugins add to people and where this workspace places them. */
  extensions?: ExtensionFieldSet;
  notice?: string;
};

/** Cursor value that means "return to the first page"; the first page itself has an empty cursor. */
export const firstPageCursor = 'page-1';

const pageDefinition = peoplePageDocument as PageDefinition;

function cloneNode(node: UiNode): UiNode {
  return {
    ...node,
    props: node.props ? {...node.props} : undefined,
    bindings: node.bindings ? {...node.bindings} : undefined,
    conditions: node.conditions ? {...node.conditions} : undefined,
    children: node.children?.map(cloneNode),
  };
}

function replaceNode(node: UiNode, id: string, replacement: UiNode): UiNode {
  if (node.id === id) return replacement;
  return {...node, children: node.children?.map(child => replaceNode(child, id, replacement))};
}

function transformNodes(node: UiNode, transform: (candidate: UiNode) => UiNode | null): UiNode | null {
  const transformed = transform(node);
  if (!transformed) return null;
  const children = transformed.children?.map(child => transformNodes(child, transform)).filter((child): child is UiNode => child !== null);
  return {...transformed, ...(children ? {children} : {})};
}

function allowed(permission: string | undefined, permissions: ReadonlySet<string>) {
  return !permission || permissions.has(permission);
}

function filterCallerSafeNode(node: UiNode, permissions: ReadonlySet<string>, actionIds: ReadonlySet<string>, fieldNames: ReadonlySet<string>): UiNode | null {
  if (!allowed(node.permission, permissions)) return null;
  const action = node.component === 'platform.ui.action' ? node.props?.action : undefined;
  if (typeof action === 'string' && !actionIds.has(action)) return null;
  const props = {...node.props};
  if (Array.isArray(props.fields)) props.fields = props.fields.filter(field => typeof field === 'object' && field !== null && fieldNames.has(String((field as {key?: unknown}).key)));
  const children = node.children?.map(child => filterCallerSafeNode(child, permissions, actionIds, fieldNames)).filter((child): child is UiNode => child !== null);
  const {permission: _, ...safe} = node;
  return {...safe, props, ...(children ? {children} : {})};
}

function stateNode(id: string, variant: 'loading' | 'empty' | 'error' | 'denied' | 'archived' | 'unavailable', title: string, description: string, children?: UiNode[]): UiNode {
  return {id, component: 'platform.ui.state', props: {variant, title, description}, ...(children ? {children} : {})};
}

function actionNode(id: string, action: string, label: string, variant: 'primary' | 'secondary' | 'quiet' = 'secondary'): UiNode {
  return {id, component: 'platform.ui.action', props: {action, label, variant}};
}

function safeResource(permissions: ReadonlySet<string>, configuredFields?: ReadonlySet<string>, configuredCustomFields?: ReadonlySet<string>, extensions?: ExtensionFieldSet) {
  const resource = resolveContactsPersonResource(permissions, {configuredCustomFields, extensions});
  if (!resource) return null;
  if (!configuredFields) return resource;
  return {...resource, fields: resource.fields.filter(field => field.source === 'extension' || configuredFields.has(field.api_name))};
}

function displayValue(field: CallerSafeFieldDescriptor | undefined, value: unknown): unknown {
  // Member references stay structured so the renderer can link to the user's record.
  if (field?.field_type === 'user') return userReference(value) ?? undefined;
  if (field?.options && (typeof value === 'string' || typeof value === 'number')) {
    return field.options.find(option => option.value === value)?.label ?? value;
  }
  if (Array.isArray(value)) return value.filter(item => typeof item === 'string').join(', ');
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? value : undefined;
}

/** Values the caller may see for a record, keyed by the resource field api names they are allowed to read. */
export function personRecordValues(person: PersonRecord, fields: readonly CallerSafeFieldDescriptor[]): Record<string, unknown> {
  const standard: Record<string, unknown> = {
    full_name: person.name,
    email: person.email,
    telephone: person.phone,
    job_title: person.role,
    organisation_id: person.organisation,
    tags: person.tags,
    // Standard record fields (ADR 0014).
    id: person.id,
    name: person.name,
    created_at: person.createdAt,
    created_by: person.createdBy,
    updated_at: person.updated,
    updated_by: person.updatedBy,
    touched_at: person.touchedAt,
    version: person.version,
  };
  const values: Record<string, unknown> = {};
  for (const field of fields) {
    if (field.source === 'extension') {
      const text = formatExtensionResourceValue(field, person.extensions?.[field.api_name]);
      if (text) values[field.api_name] = text;
      continue;
    }
    const raw = field.source === 'tenant-custom' ? person.customFields[field.api_name] : standard[field.api_name];
    const value = displayValue(field, raw);
    if (value !== undefined) values[field.api_name] = value;
  }
  return values;
}

function collectionItem(person: PersonRecord, fields: readonly CallerSafeFieldDescriptor[], fieldNames: ReadonlySet<string>): CollectionItem {
  return {
    id: person.id,
    title: person.name,
    description: fieldNames.has('job_title') && person.role ? person.role : undefined,
    archived: person.archived || undefined,
    values: personRecordValues(person, fields),
  };
}

function recordSummary(person: PersonRecord, fields: readonly CallerSafeFieldDescriptor[], fieldNames: ReadonlySet<string>): RecordSummary {
  return collectionItem(person, fields, fieldNames);
}

function previewNode(selected: PersonRecord | undefined, selectedId: string | undefined, pending: boolean, fields: readonly CallerSafeFieldDescriptor[], fieldNames: ReadonlySet<string>): UiNode {
  if (!selectedId) return {id: 'person-preview', component: 'platform.ui.record-preview', props: {label: 'Person preview', empty_title: 'Select a person', empty_description: 'Choose a person to keep key contact details visible while you scan the directory.'}};
  if (!selected) return {
    id: 'person-preview',
    component: 'platform.ui.record-preview',
    props: {label: 'Person preview'},
    children: [pending
      ? stateNode('pending-person-selection', 'loading', 'Loading person', 'The selected person is loading.')
      : stateNode('stale-person-selection', 'unavailable', 'Person unavailable', 'This person may have been archived, removed, or belong to another workspace.')],
  };
  const summaryFields = [
    {key: 'organisation_id', label: 'Organisation'},
    {key: 'email', label: 'Email', format: 'email'},
    {key: 'telephone', label: 'Phone', format: 'phone'},
    {key: 'relationship_type__c', label: 'Relationship'},
  ].filter(field => fieldNames.has(field.key));
  return {
    id: 'person-preview',
    component: 'platform.ui.record-preview',
    props: {label: `${selected.name} preview`},
    children: [
      {id: 'selected-person-summary', component: 'platform.ui.record-summary', props: {label: `${selected.name} summary`, resource: 'platform.contacts.person', record: recordSummary(selected, fields, fieldNames), fields: summaryFields}},
      {id: 'selected-person-actions', component: 'platform.ui.action-bar', props: {label: 'Selected person actions'}, children: [
        ...(selected.archived ? [] : [actionNode('edit-selected-person', 'edit-person', 'Edit person', 'primary')]),
        actionNode('open-selected-person', 'open-person', 'Open full record', 'quiet'),
      ]},
    ],
  };
}

function contentState(root: UiNode, state: PeopleDirectoryState, noResults: boolean, empty: boolean) {
  if (state === 'loading') return replaceNode(root, 'people-content', stateNode('people-loading', 'loading', 'Loading people', 'The directory is loading.'));
  if (state === 'error') return replaceNode(root, 'people-content', stateNode('people-error', 'error', 'People could not be loaded', 'Check your connection and try again. Your search has been preserved.'));
  if (state === 'empty' || empty) return replaceNode(root, 'people-content', stateNode('people-empty', 'empty', 'Add your first person', 'People you add to this workspace will appear here.', [actionNode('empty-add-person', 'add-person', 'Add person', 'primary')]));
  if (noResults) return replaceNode(root, 'people-content', stateNode('people-no-results', 'empty', 'No people match this search', 'Try a different name or clear the relationship filter.', [actionNode('no-results-clear', 'clear-people-query', 'Clear filters')]));
  return root;
}

function describeResults(page: PeoplePage | undefined): {heading: string; description: string} {
  const shown = page?.records.length ?? 0;
  if (page?.total !== undefined) return {heading: `${page.total} ${page.total === 1 ? 'person' : 'people'}`, description: `Showing ${shown} of ${page.total}`};
  return {heading: `${shown} ${shown === 1 ? 'person' : 'people'}${page?.nextCursor ? ' on this page' : ''}`, description: page?.nextCursor ? `Showing ${shown}, more available` : `Showing ${shown}`};
}

export function resolveContactsPeopleDirectory({query, selectedId, state = 'ready', page, selected, selectedPending = false, previousCursor, labelFilter = true, pluginEnabled = true, permissions, configuredFields, configuredCustomFields, extensions, notice = ''}: ResolveOptions): ResolvedPeopleDirectory {
  selectedId ??= query.selected || undefined;
  const permitted = permissions.has(contactsPersonPermissions.read);
  const accessAllowed = pluginEnabled && state !== 'disabled' && permitted && state !== 'denied';
  const canExposeRecords = accessAllowed && (state === 'ready' || state === 'archived');
  const resource = accessAllowed ? safeResource(permissions, configuredFields, configuredCustomFields, extensions) : null;
  let root = cloneNode(extensions ? placeExtensionFields(pageDefinition.root, pageDefinition.id, extensions) : pageDefinition.root);

  if (!pluginEnabled || state === 'disabled') {
    root = {...root, children: [stateNode('contacts-disabled', 'unavailable', 'Contacts is disabled', 'Enable the Contacts plugin to use the people directory.')]};
  } else if (!permitted || state === 'denied') {
    root = {...root, children: [stateNode('people-denied', 'denied', 'People are unavailable', 'You do not have permission to view people in this workspace.')]};
  }

  const readableFields = resource?.fields.filter(field => field.readable !== false) ?? [];
  const fieldNames = new Set(readableFields.map(field => field.api_name));
  const relationshipConfigured = labelFilter && fieldNames.has('relationship_type__c');
  const actions = (pageDefinition.actions ?? [])
    .filter(action => allowed(action.permission, permissions))
    .filter(action => action.id !== 'filter-people' || relationshipConfigured)
    .map(({permission: _, ...action}) => action);
  const actionIds = new Set(actions.map(action => action.id));
  const effectiveLabel = relationshipConfigured ? query.label : 'All';
  const needle = query.query.trim();
  const visiblePage = canExposeRecords ? page : undefined;
  const records = visiblePage?.records ?? [];
  const exposedSelected = canExposeRecords ? selected : undefined;
  const items = records.map(person => collectionItem(person, readableFields, fieldNames));
  const hasFilters = Boolean(needle || effectiveLabel !== 'All');
  const noResults = canExposeRecords && visiblePage !== undefined && records.length === 0 && !query.cursor && hasFilters;
  const empty = state === 'ready' && visiblePage !== undefined && records.length === 0 && !query.cursor && !hasFilters;
  const archived = Boolean(exposedSelected?.archived) || (state === 'archived' && Boolean(exposedSelected));
  const previewSelected = exposedSelected && archived ? {...exposedSelected, archived: true} : exposedSelected;

  if (resource && permitted && pluginEnabled && state !== 'disabled' && state !== 'denied') {
    root = transformNodes(root, node => {
      if (!relationshipConfigured && node.id === 'people-relationship-filter') return null;
      const fields = Array.isArray(node.props?.fields)
        ? node.props.fields.filter(field => typeof field === 'object' && field !== null && fieldNames.has(String((field as {key?: unknown}).key)))
        : undefined;
      return fields ? {...node, props: {...node.props, fields}} : node;
    }) ?? root;
    root = replaceNode(root, 'person-preview', previewNode(previewSelected, selectedId, selectedPending && canExposeRecords, readableFields, fieldNames));
    root = contentState(root, state, noResults, empty);
  }
  root = filterCallerSafeNode(root, permissions, actionIds, fieldNames) ?? stateNode('people-denied', 'denied', 'People are unavailable', 'You do not have permission to view people.');

  const selectedVariant = !pluginEnabled || state === 'disabled' ? 'plugin-disabled'
    : !permitted || state === 'denied' ? 'denied'
    : state === 'loading' ? 'loading'
    : state === 'error' ? 'error'
    : state === 'empty' || empty ? 'empty'
    : noResults ? 'no-results'
    : selectedId && !previewSelected ? (selectedPending ? 'selection-loading' : 'stale-selection')
    : archived ? 'archived-selection'
    : previewSelected ? 'selected'
    : 'ready';
  const results = describeResults(visiblePage);
  const chips = [
    ...(needle ? [{id: 'query', label: `Search: ${needle}`, action: 'search-people', parameter: 'query', value: ''}] : []),
    ...(effectiveLabel !== 'All' ? [{id: 'label', label: `Relationship: ${effectiveLabel}`, action: 'filter-people', parameter: 'label', value: 'All'}] : []),
  ];
  const dataSources = {
    directory: {
      query: query.query,
      label: effectiveLabel,
      sort: query.sort,
      // The API reads newest first for `recent` and A to Z for `name`.
      sort_direction: query.sort === 'recent' ? 'descending' : 'ascending',
      cursor: query.cursor,
      presentation: query.view ?? 'table',
      chips,
      has_filters: hasFilters,
      notice: notice || undefined,
      items: items.map(item => item.id === selectedId && archived ? {...item, archived: true} : item),
      selected: previewSelected ? recordSummary(previewSelected, readableFields, fieldNames) : null,
      selected_id: previewSelected?.id ?? '',
      previous_cursor: query.cursor ? previousCursor || firstPageCursor : '',
      next_cursor: visiblePage?.nextCursor ?? '',
      result_heading: results.heading,
      result_description: results.description,
    },
  };
  return {
    page: {
      page_id: pageDefinition.id,
      definition_version: pageDefinition.definition_version,
      selected_variant: selectedVariant,
      source_versions: {'platform.contacts': '0.1.0'},
      publication_versions: [],
      resource: resource ? pageDefinition.resource ?? null : null,
      data_sources: resource ? pageDefinition.data_sources ?? [] : [],
      actions: resource ? actions : [],
      root,
      content_hash: `platform.contacts.people-directory@1:${selectedVariant}:${relationshipConfigured ? 'custom' : 'standard'}:${actions.map(action => action.id).join(',')}:${(extensions?.placements ?? []).map(placement => `${placement.field_id}@${placement.zone}#${placement.position}`).join(',')}`,
    },
    resources: resource ? [resource] : [],
    dataSources,
    announcement: selectedVariant === 'ready' || selectedVariant === 'selected' ? `${results.heading} found. ${results.description}.` : '',
  };
}
