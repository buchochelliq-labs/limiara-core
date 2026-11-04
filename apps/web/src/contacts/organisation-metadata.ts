import organisationDetailDocument from '../../../../plugins/contacts/ui/pages/organisation-detail.json';
import organisationDirectoryDocument from '../../../../plugins/contacts/ui/pages/organisation-directory.json';
import organisationResourceDocument from '../../../../plugins/contacts/ui/resources/organisation.json';
import {withStandardFields} from '../plugin-ui/standard-fields/model';
import type {CallerSafeResourceDescriptor, CollectionItem, PageDefinition, RecordSummary, ResolvedPage, ResourceDescriptor, UiNode} from '../plugin-ui/contracts';
import {organisations, type Organisation} from './organisations-data';
import {people, type Person} from './people-data';
import {contactsPersonPermissions, resolveContactsPersonResource} from './person-metadata';

export const contactsOrganisationPermissions = {
  read: 'contacts.organisations.read',
  write: 'contacts.organisations.write',
  customFields: 'contacts.organisations.custom-fields.read',
} as const;

export type OrganisationDirectoryState = 'ready' | 'loading' | 'empty' | 'denied' | 'error' | 'disabled' | 'archived';
export type OrganisationDetailState = 'ready' | 'loading' | 'missing' | 'denied' | 'error' | 'archived' | 'version-changed' | 'disabled' | 'workspace-changed';
export type OrganisationDirectoryQuery = {query: string; status: 'All' | Organisation['accountStatus']; sort: 'name' | 'recent'; cursor: '' | 'page-2'; selected: string};

const resourceDefinition = organisationResourceDocument as ResourceDescriptor;
const directoryDefinition = organisationDirectoryDocument as PageDefinition;
const detailDefinition = organisationDetailDocument as unknown as PageDefinition;
const pageSize = 6;

function allowed(permission: string | undefined, permissions: ReadonlySet<string>) {
  return !permission || permissions.has(permission);
}

function cloneNode(node: UiNode): UiNode {
  return {...node, props: node.props ? {...node.props} : undefined, bindings: node.bindings ? {...node.bindings} : undefined, conditions: node.conditions ? {...node.conditions} : undefined, children: node.children?.map(cloneNode)};
}

function replaceNode(node: UiNode, id: string, replacement: UiNode): UiNode {
  if (node.id === id) return replacement;
  return {...node, children: node.children?.map(child => replaceNode(child, id, replacement))};
}

function removeNode(node: UiNode, id: string): UiNode {
  return {...node, children: node.children?.filter(child => child.id !== id).map(child => removeNode(child, id))};
}

function stateNode(id: string, variant: 'loading' | 'empty' | 'error' | 'denied' | 'archived' | 'unavailable', title: string, description: string, children?: UiNode[]): UiNode {
  return {id, component: 'platform.ui.state', props: {variant, title, description}, ...(children ? {children} : {})};
}

function actionNode(id: string, action: string, label: string): UiNode {
  return {id, component: 'platform.ui.action', props: {action, label, variant: 'secondary'}};
}

function filterNode(node: UiNode, permissions: ReadonlySet<string>, actionIds: ReadonlySet<string>, fieldIds: ReadonlySet<string>): UiNode | null {
  if (!allowed(node.permission, permissions)) return null;
  const action = node.component === 'platform.ui.action' ? node.props?.action : undefined;
  if (typeof action === 'string' && !actionIds.has(action)) return null;
  const props = {...node.props};
  if (Array.isArray(props.fields)) props.fields = props.fields.filter(field => typeof field === 'object' && field !== null && fieldIds.has(String((field as {key?: unknown}).key)));
  const children = node.children?.map(child => filterNode(child, permissions, actionIds, fieldIds)).filter((child): child is UiNode => child !== null);
  const {permission: _, ...safe} = node;
  return {...safe, props, ...(children ? {children} : {})};
}

function actionsFor(definition: PageDefinition, permissions: ReadonlySet<string>) {
  return (definition.actions ?? []).filter(action => allowed(action.permission, permissions)).map(({permission: _, ...action}) => action);
}

export function resolveContactsOrganisationResource(permissions: ReadonlySet<string>, configuredFields?: ReadonlySet<string>): CallerSafeResourceDescriptor | null {
  if (!permissions.has(contactsOrganisationPermissions.read)) return null;
  const declared = (resourceDefinition.fields ?? [])
    .filter(field => allowed(field.permission, permissions))
    .map(({permission: _, ...field}) => field);
  const fields = withStandardFields(declared).filter(field => !configuredFields || configuredFields.has(field.api_name));
  return {
    id: resourceDefinition.id,
    definition_version: resourceDefinition.definition_version,
    owner_plugin: resourceDefinition.owner_plugin,
    label: resourceDefinition.label,
    name_field: resourceDefinition.name_field,
    fields,
    relationships: (resourceDefinition.relationships ?? []).filter(item => allowed(item.permission, permissions)).map(({permission: _, ...item}) => item),
    operations: (resourceDefinition.operations ?? []).filter(item => permissions.has(item.permission)).map(({permission: _, ...item}) => item),
  };
}

function organisationValues(organisation: Organisation, fieldIds: ReadonlySet<string>) {
  const candidates: Record<string, unknown> = {
    name: organisation.name,
    sector: organisation.sector,
    website: organisation.website,
    telephone: organisation.phone,
    address: organisation.address,
    updated_at: organisation.updated,
    version: organisation.version,
    account_status__c: organisation.accountStatus,
    service_region__c: organisation.serviceRegion,
  };
  return Object.fromEntries(Object.entries(candidates).filter(([key]) => fieldIds.has(key)));
}

function organisationRecord(organisation: Organisation, fieldIds: ReadonlySet<string>, archived = false): RecordSummary {
  return {id: organisation.id, title: organisation.name, description: fieldIds.has('sector') ? organisation.sector : undefined, archived: archived || undefined, values: organisationValues(organisation, fieldIds)};
}

function preview(selected: Organisation | undefined, selectedId: string | undefined, fieldIds: ReadonlySet<string>, archived: boolean): UiNode {
  if (!selectedId) return {id: 'organisation-preview', component: 'platform.ui.record-preview', props: {label: 'Organisation preview', empty_title: 'Select an organisation', empty_description: 'Choose an organisation to keep its account context visible while you scan the directory.'}};
  if (!selected) return {id: 'organisation-preview', component: 'platform.ui.record-preview', props: {label: 'Organisation preview'}, children: [stateNode('stale-organisation-selection', 'unavailable', 'Organisation unavailable', 'This organisation may have been archived, removed, or belong to another workspace.')]};
  return {id: 'organisation-preview', component: 'platform.ui.record-preview', props: {label: `${selected.name} preview`}, children: [
    {id: 'selected-organisation-summary', component: 'platform.ui.record-summary', props: {label: `${selected.name} summary`, resource: resourceDefinition.id, record: organisationRecord(selected, fieldIds, archived), fields: [{key: 'sector', label: 'Sector'}, {key: 'telephone', label: 'Phone', format: 'phone'}, {key: 'address', label: 'Address'}, {key: 'account_status__c', label: 'Account status'}].filter(field => fieldIds.has(field.key))}},
    {id: 'selected-organisation-actions', component: 'platform.ui.action-bar', props: {label: 'Selected organisation actions'}, children: [actionNode('edit-selected-organisation', 'edit-organisation', 'Edit organisation'), actionNode('open-selected-organisation', 'open-organisation', 'Open full record')]},
  ]};
}

export function resolveContactsOrganisationDirectory(options: {query: OrganisationDirectoryQuery; state?: OrganisationDirectoryState; pluginEnabled?: boolean; permissions: ReadonlySet<string>; configuredFields?: ReadonlySet<string>}) {
  const {query, state = 'ready', pluginEnabled = true, permissions, configuredFields} = options;
  const permitted = permissions.has(contactsOrganisationPermissions.read);
  const canExpose = pluginEnabled && permitted && state !== 'disabled' && state !== 'denied' && (state === 'ready' || state === 'archived');
  const resource = pluginEnabled && permitted && state !== 'disabled' && state !== 'denied' ? resolveContactsOrganisationResource(permissions, configuredFields) : null;
  const fieldIds = new Set(resource?.fields.filter(field => field.readable !== false).map(field => field.api_name) ?? []);
  const actions = actionsFor(directoryDefinition, permissions).filter(action => action.id !== 'filter-organisations' || fieldIds.has('account_status__c'));
  const actionIds = new Set(actions.map(action => action.id));
  const status = fieldIds.has('account_status__c') ? query.status : 'All';
  const needle = query.query.trim().toLocaleLowerCase();
  const filtered = (canExpose ? organisations : []).filter(item => (!needle || [item.name, item.sector, item.address].some(value => value.toLocaleLowerCase().includes(needle))) && (status === 'All' || item.accountStatus === status)).sort((left, right) => query.sort === 'recent' ? right.updatedOrder - left.updatedOrder : left.name.localeCompare(right.name));
  const start = query.cursor === 'page-2' ? pageSize : 0;
  const visible = filtered.slice(start, start + pageSize);
  const selected = canExpose ? organisations.find(item => item.id === query.selected) : undefined;
  const archived = state === 'archived' && Boolean(selected);
  const hasFilters = Boolean(needle || status !== 'All');
  const staleCursor = query.cursor === 'page-2' && filtered.length <= pageSize;
  let root = cloneNode(directoryDefinition.root);
  if (!pluginEnabled || state === 'disabled') root = {...root, children: [stateNode('organisations-disabled', 'unavailable', 'Contacts is disabled', 'Enable the Contacts plugin to use the organisation directory.')]};
  else if (!permitted || state === 'denied') root = {...root, children: [stateNode('organisations-denied', 'denied', 'Organisations are unavailable', 'You do not have permission to view organisations in this workspace.')]};
  else if (state === 'loading') root = replaceNode(root, 'organisations-content', stateNode('organisations-loading', 'loading', 'Loading organisations', 'The directory is loading.'));
  else if (state === 'error') root = replaceNode(root, 'organisations-content', stateNode('organisations-error', 'error', 'Organisations could not be loaded', 'Check your connection and try again. Your search has been preserved.'));
  else if (state === 'empty') root = replaceNode(root, 'organisations-content', stateNode('organisations-empty', 'empty', 'Add your first organisation', 'Organisations you add to this workspace will appear here.', [actionNode('empty-add-organisation', 'add-organisation', 'Add organisation')]));
  else if (staleCursor) root = replaceNode(root, 'organisations-content', stateNode('organisations-stale-cursor', 'unavailable', 'This result page is no longer available', 'The organisation list changed. Return to the first page.', [actionNode('stale-cursor-reset', 'clear-organisations-query', 'Return to first page')]));
  else if (filtered.length === 0 && hasFilters) root = replaceNode(root, 'organisations-content', stateNode('organisations-no-results', 'empty', 'No organisations match this search', 'Try a different name or clear the status filter.', [actionNode('no-results-clear', 'clear-organisations-query', 'Clear filters')]));
  else root = replaceNode(root, 'organisation-preview', preview(selected, query.selected || undefined, fieldIds, archived));
  if (!fieldIds.has('account_status__c')) root = removeNode(root, 'organisations-status-filter');
  root = filterNode(root, permissions, actionIds, fieldIds) ?? stateNode('organisations-denied', 'denied', 'Organisations are unavailable', 'You do not have permission to view organisations.');
  const variant = !pluginEnabled || state === 'disabled' ? 'plugin-disabled' : !permitted || state === 'denied' ? 'denied' : state === 'loading' || state === 'error' || state === 'empty' ? state : staleCursor ? 'stale-cursor' : filtered.length === 0 && hasFilters ? 'no-results' : query.selected && !selected ? 'stale-selection' : archived ? 'archived-selection' : selected ? 'selected' : 'ready';
  const page: ResolvedPage = {page_id: directoryDefinition.id, definition_version: 1, selected_variant: variant, source_versions: {'platform.contacts': '0.1.0'}, publication_versions: [], resource: resource ? resourceDefinition.id : null, data_sources: resource ? directoryDefinition.data_sources ?? [] : [], actions: resource ? actions : [], root, content_hash: `platform.contacts.organisation-directory@1:${variant}:${fieldIds.has('account_status__c') ? 'custom' : 'standard'}:${actions.map(action => action.id).join(',')}`};
  return {page, resources: resource ? [resource] : [], dataSources: {directory: {query: query.query, status, sort: query.sort, cursor: query.cursor, has_filters: hasFilters, items: visible.map(item => organisationRecord(item, fieldIds, archived && item.id === selected?.id) as CollectionItem), selected: selected ? organisationRecord(selected, fieldIds, archived) : null, selected_id: selected?.id ?? '', previous_cursor: query.cursor === 'page-2' && !staleCursor ? 'page-1' : '', next_cursor: start + pageSize < filtered.length ? 'page-2' : '', result_heading: `${filtered.length} ${filtered.length === 1 ? 'organisation' : 'organisations'}`, result_description: `Showing ${visible.length} of ${filtered.length}`}}, announcement: variant === 'ready' || variant === 'selected' ? `${filtered.length} organisations found. Showing ${visible.length}.` : ''};
}

function personItem(person: Person, fields: ReadonlySet<string>): CollectionItem {
  const values = {job_title: person.role, email: person.email, telephone: person.phone};
  return {id: person.id, title: person.name, description: fields.has('job_title') ? person.role : undefined, values: Object.fromEntries(Object.entries(values).filter(([key]) => fields.has(key)))};
}

function detailStateRoot(state: Exclude<OrganisationDetailState, 'ready' | 'archived' | 'version-changed'>): UiNode {
  const values = {
    loading: ['loading', 'Loading organisation', 'Organisation details are loading.'],
    missing: ['unavailable', 'Organisation not found', 'This organisation may have been archived, removed, or belong to another workspace.'],
    denied: ['denied', 'Organisation unavailable', 'You do not have permission to view this organisation.'],
    error: ['error', 'Organisation could not be loaded', 'Check your connection and try again.'],
    disabled: ['unavailable', 'Organisation unavailable', 'The Contacts plugin is disabled for this workspace.'],
    'workspace-changed': ['unavailable', 'Organisation unavailable', 'This organisation belongs to a different workspace. Return to organisations to continue.'],
  } as const;
  const [variant, title, description] = values[state];
  return stateNode(`organisation-${state}`, variant, title, description);
}

export function resolveContactsOrganisationDetail(options: {organisation?: Organisation; state: OrganisationDetailState; permissions: ReadonlySet<string>; configuredFields?: ReadonlySet<string>; relatedPeople?: readonly Person[]; notice?: string}) {
  const {organisation, permissions, configuredFields, relatedPeople = [], notice} = options;
  const state = !permissions.has(contactsOrganisationPermissions.read) ? 'denied' : options.state;
  const expose = Boolean(organisation) && (state === 'ready' || state === 'archived' || state === 'version-changed');
  const organisationResource = expose ? resolveContactsOrganisationResource(permissions, configuredFields) : null;
  const personResource = expose && permissions.has(contactsPersonPermissions.read) ? resolveContactsPersonResource(permissions) : null;
  const actions = actionsFor(detailDefinition, permissions);
  const actionIds = new Set(actions.map(action => action.id));
  const fieldIds = new Set(organisationResource?.fields.map(field => field.api_name) ?? []);
  const personFields = new Set(personResource?.fields.map(field => field.api_name) ?? []);
  const visibleFieldIds = new Set([...fieldIds, ...personFields]);
  let root = expose ? cloneNode(detailDefinition.root) : detailStateRoot(state as Exclude<OrganisationDetailState, 'ready' | 'archived' | 'version-changed'>);
  root = filterNode(root, permissions, actionIds, visibleFieldIds) ?? detailStateRoot('denied');
  const record = expose ? organisationRecord(organisation!, fieldIds, state === 'archived') : undefined;
  const peopleItems = expose && personResource ? relatedPeople.map(item => personItem(item, personFields)) : [];
  const page: ResolvedPage = {page_id: detailDefinition.id, definition_version: 1, selected_variant: state, source_versions: {'platform.contacts': '0.1.0'}, publication_versions: [], resource: organisationResource ? resourceDefinition.id : null, data_sources: expose ? (detailDefinition.data_sources ?? []).filter(source => source.id !== 'related_people' || Boolean(personResource)) : [], actions: expose ? actions : [], root, content_hash: `platform.contacts.organisation-detail@1:${state}:${fieldIds.has('account_status__c') ? 'custom' : 'standard'}:${peopleItems.length}:${actions.map(action => action.id).join(',')}`};
  return {page, resources: [organisationResource, personResource].filter((item): item is CallerSafeResourceDescriptor => item !== null), dataSources: expose ? {organisation: {record, ...(notice ? {notice} : {})}, related_people: {items: peopleItems}} : {}};
}
