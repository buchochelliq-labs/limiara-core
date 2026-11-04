import personDetailDocument from '../../../../plugins/contacts/ui/pages/person-detail.json';
import type {CollectionItem, ExtensionContribution, PageDefinition, ResolvedPage, UiNode} from '../plugin-ui/contracts';
import {applicableInsideContributions, composeInsideContributions} from '../plugin-ui/compose';
import type {ActivityEntry, PersonRecord} from './api';
import {personRecordValues} from './people-directory-metadata';
import {contactsPersonPermissions, resolveContactsPersonResource} from './person-metadata';
import {placeExtensionFields, type ExtensionFieldSet} from '../plugin-ui/extension-fields/model';

export type PersonDetailState = 'ready' | 'loading' | 'missing' | 'denied' | 'error' | 'archived' | 'version-changed' | 'disabled' | 'workspace-changed';

const definition = personDetailDocument as PageDefinition;

function allowed(permission: string | undefined, permissions: ReadonlySet<string>) {
  return !permission || permissions.has(permission);
}

function stateRoot(state: Exclude<PersonDetailState, 'ready' | 'archived' | 'version-changed'>): UiNode {
  const states = {
    loading: ['loading', 'Loading person', 'Contact details are loading.'],
    missing: ['unavailable', 'Person not found', 'This person may have been archived, removed, or belong to another workspace.'],
    denied: ['denied', 'Person unavailable', 'You do not have permission to view this person.'],
    error: ['error', 'Person could not be loaded', 'Check your connection and try again.'],
    disabled: ['unavailable', 'Person unavailable', 'The Contacts plugin is disabled for this workspace.'],
    'workspace-changed': ['unavailable', 'Person unavailable', 'This person belongs to a different workspace. Return to people to continue.'],
  } as const;
  const [variant, title, description] = states[state];
  return {id: `person-${state}`, component: 'platform.ui.state', props: {variant, title, description}};
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

export function resolveContactsPersonDetail(options: {
  person?: PersonRecord;
  state: PersonDetailState;
  permissions: ReadonlySet<string>;
  contributions?: readonly ExtensionContribution[];
  enabledContributionPlugins?: ReadonlySet<string>;
  /** Tenant custom field api names configured for people; unset keeps the plugin's declared custom fields. */
  configuredCustomFields?: ReadonlySet<string>;
  /** Whether the backing source can restore an archived person. */
  canRestore?: boolean;
  /** Timeline entries for the person, newest first; absent while loading. */
  activity?: readonly ActivityEntry[];
  notice?: string;
  /** Fields other plugins add to people and where this workspace places them. */
  extensions?: ExtensionFieldSet;
}) {
  const {person, state, permissions, contributions = [], enabledContributionPlugins, configuredCustomFields, canRestore = true, activity = [], notice, extensions} = options;
  const safeState = !permissions.has(contactsPersonPermissions.read) ? 'denied' : state;
  const mayExposeRecord = Boolean(person) && (safeState === 'ready' || safeState === 'archived' || safeState === 'version-changed');
  const resource = mayExposeRecord ? resolveContactsPersonResource(permissions, {configuredCustomFields, extensions}) : null;
  const actions = (definition.actions ?? [])
    .filter(action => allowed(action.permission, permissions))
    .filter(action => canRestore || action.id !== 'restore-person')
    .map(({permission: _, ...action}) => action);
  const actionIds = new Set(actions.map(action => action.id));
  const fields = resource?.fields ?? [];
  const fieldIds = new Set(fields.map(field => field.api_name));
  const compositionOptions = {host: 'internal-web' as const, permissions, enabledPlugins: enabledContributionPlugins};
  const activeContributions = applicableInsideContributions(definition.id, definition.definition_version, contributions, compositionOptions);
  const placedRoot = extensions ? placeExtensionFields(definition.root, definition.id, extensions) : definition.root;
  let root = mayExposeRecord ? composeInsideContributions(placedRoot, definition.id, definition.definition_version, activeContributions, compositionOptions) : stateRoot(safeState as Exclude<PersonDetailState, 'ready' | 'archived' | 'version-changed'>);
  root = filterNode(root, permissions, actionIds, fieldIds) ?? stateRoot('denied');
  const archived = safeState === 'archived' || Boolean(person?.archived);
  const record = mayExposeRecord && person ? {
    id: person.id,
    title: person.name,
    description: [person.role, person.organisation].filter(Boolean).join(' at ') || undefined,
    archived,
    values: personRecordValues(person, fields),
  } : undefined;
  const organisation: CollectionItem[] = mayExposeRecord && person?.organisationId ? [{id: person.organisationId, title: person.organisation, description: 'Organisation'}] : [];
  const stage = mayExposeRecord && fieldIds.has('relationship_type__c') && typeof person?.customFields.relationship_type__c === 'string' ? person.customFields.relationship_type__c : '';
  const activityItems: CollectionItem[] = mayExposeRecord ? activity.map(entry => ({
    id: entry.id,
    title: activityTitle(entry.action),
    description: `Version ${entry.version}`,
    values: {occurred_at: entry.occurredAt, actor: entry.actorId ? 'A team member' : ''},
  })) : [];
  const page: ResolvedPage = {
    page_id: definition.id,
    definition_version: definition.definition_version,
    selected_variant: safeState,
    source_versions: {'platform.contacts': '0.1.0'},
    publication_versions: [],
    resource: mayExposeRecord ? definition.resource ?? null : null,
    data_sources: mayExposeRecord ? definition.data_sources ?? [] : [],
    actions: mayExposeRecord ? actions : [],
    root,
    content_hash: `platform.contacts.person-detail@1:${safeState}:${permissions.has(contactsPersonPermissions.customFields) ? 'custom' : 'standard'}:${[...fieldIds].join(',')}:${activeContributions.map(item => item.id).join(',')}:${(extensions?.placements ?? []).map(placement => `${placement.field_id}@${placement.zone}#${placement.position}`).join(',')}`,
  };
  return {page, resources: resource ? [resource] : [], dataSources: mayExposeRecord ? {person: {record, organisation, stage, activity: activityItems, ...(notice ? {notice} : {})}} : {}};
}

function activityTitle(action: string) {
  if (action === 'created') return 'Person created';
  if (action === 'updated') return 'Person updated';
  if (action === 'archived') return 'Person archived';
  if (action === 'deleted') return 'Person moved to the recycle bin';
  if (action === 'undeleted') return 'Person restored from the recycle bin';
  return action.replace(/[-_]/g, ' ').replace(/^./, first => first.toUpperCase());
}
