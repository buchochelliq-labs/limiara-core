import personPageDocument from '../../../../plugins/contacts/ui/pages/person-form.json';
import personResourceDocument from '../../../../plugins/contacts/ui/resources/person.json';
import type {
  CallerSafeFieldDescriptor,
  CallerSafeOperationDescriptor,
  CallerSafeRelationshipDescriptor,
  CallerSafeResourceDescriptor,
  PageDefinition,
  ResolvedPage,
  ResourceDescriptor,
  UiNode,
} from '../plugin-ui/contracts';
import {extensionResourceField, placeExtensionFields, type ExtensionFieldSet} from '../plugin-ui/extension-fields/model';
import {withStandardFields} from '../plugin-ui/standard-fields/model';
import type {OrganisationOption} from './api';

export const contactsPersonPermissions = {
  read: 'contacts.people.read',
  write: 'contacts.people.write',
  customFields: 'contacts.people.custom-fields.read',
} as const;

/** Reading organisations is what lets a caller choose or see a person's organisation. */
export const organisationsReadPermission = 'contacts.organisations.read';

const resourceDefinition = personResourceDocument as ResourceDescriptor;
const pageDefinition = personPageDocument as PageDefinition;

function withoutPermission<T extends {permission?: string}>(value: T): Omit<T, 'permission'> {
  const {permission: _, ...safe} = value;
  return safe;
}

function allowed(permission: string | undefined, permissions: ReadonlySet<string>) {
  return !permission || permissions.has(permission);
}

function filterNode(node: UiNode, permissions: ReadonlySet<string>, actionIds: ReadonlySet<string>, fieldNames: ReadonlySet<string>): UiNode | null {
  if (!allowed(node.permission, permissions)) return null;
  const action = node.component === 'platform.ui.action' ? node.props?.action : undefined;
  if (typeof action === 'string' && !actionIds.has(action)) return null;
  const field = node.component.startsWith('platform.ui.field.') ? node.props?.field : undefined;
  if (typeof field === 'string' && !fieldNames.has(field)) return null;
  const children = node.children?.map(child => filterNode(child, permissions, actionIds, fieldNames)).filter((child): child is UiNode => child !== null);
  // A section or grid whose fields were all removed, or that no placed field
  // filled, has nothing left to say.
  if ((node.component === 'platform.ui.section' || node.component === 'platform.ui.grid') && node.children && children?.length === 0) return null;
  const {permission: _, ...safe} = node;
  return {...safe, ...(children ? {children} : {})};
}

export type PersonResourceOptions = {
  /** Organisations the caller may link; the field offers no choices until they are known. */
  organisations?: readonly OrganisationOption[];
  /** Permissions deciding whether the organisation field is exposed at all. */
  permissions?: ReadonlySet<string>;
  /** Tenant custom field api names configured for people. Unknown means none are configured yet. */
  configuredCustomFields?: ReadonlySet<string>;
  /** Fields other plugins add to people and where this workspace places them. */
  extensions?: ExtensionFieldSet;
};

function callerSafeFields(permissions: ReadonlySet<string>, options: PersonResourceOptions): CallerSafeFieldDescriptor[] {
  return (resourceDefinition.fields ?? [])
    .filter(field => allowed(field.permission, permissions))
    .filter(field => field.source !== 'tenant-custom' || options.configuredCustomFields === undefined || options.configuredCustomFields.has(field.api_name))
    .filter(field => field.api_name !== 'organisation_id' || !options.permissions || options.permissions.has(organisationsReadPermission))
    .map(field => {
      const safe = withoutPermission(field);
      if (field.api_name !== 'organisation_id') return safe;
      return {
        ...safe,
        options: [
          {value: '', label: 'No organisation'},
          ...(options.organisations ?? []).map(option => ({value: option.id, label: option.name})),
        ],
      };
    });
}

function callerSafeOperations(permissions: ReadonlySet<string>): CallerSafeOperationDescriptor[] {
  return (resourceDefinition.operations ?? [])
    .filter(operation => permissions.has(operation.permission))
    .map(withoutPermission);
}

function callerSafeRelationships(permissions: ReadonlySet<string>): CallerSafeRelationshipDescriptor[] {
  return (resourceDefinition.relationships ?? [])
    .filter(relationship => allowed(relationship.permission, permissions))
    .map(withoutPermission);
}

export function resolveContactsPersonResource(permissions: ReadonlySet<string>, options: PersonResourceOptions = {}): CallerSafeResourceDescriptor | null {
  if (!permissions.has(contactsPersonPermissions.read)) return null;
  return {
    id: resourceDefinition.id,
    definition_version: resourceDefinition.definition_version,
    owner_plugin: resourceDefinition.owner_plugin,
    label: resourceDefinition.label,
    name_field: resourceDefinition.name_field,
    fields: [...withStandardFields(callerSafeFields(permissions, options)), ...(options.extensions?.fields ?? []).map(extensionResourceField)],
    relationships: callerSafeRelationships(permissions),
    operations: callerSafeOperations(permissions),
  };
}

export function resolveContactsPersonForm(mode: 'create' | 'edit', permissions: ReadonlySet<string>, options: PersonResourceOptions = {}): {page: ResolvedPage; resource: CallerSafeResourceDescriptor} | null {
  if (!permissions.has(contactsPersonPermissions.read) || !permissions.has(contactsPersonPermissions.write)) return null;
  const resource = resolveContactsPersonResource(permissions, options);
  if (!resource) return null;
  const fieldNames = new Set(resource.fields.map(field => field.api_name));
  const actions = (pageDefinition.actions ?? [])
    .filter(action => allowed(action.permission, permissions))
    .filter(action => action.kind !== 'resource-operation' || action.id === (mode === 'create' ? 'create-person' : 'update-person'))
    .map(withoutPermission);
  const actionIds = new Set(actions.map(action => action.id));
  const placed = options.extensions ? placeExtensionFields(pageDefinition.root, pageDefinition.id, options.extensions) : pageDefinition.root;
  const root = filterNode(placed, permissions, actionIds, fieldNames);
  if (!root) return null;
  return {
    resource,
    page: {
      page_id: pageDefinition.id,
      definition_version: pageDefinition.definition_version,
      selected_variant: `${mode}-mode`,
      source_versions: {'platform.contacts': '0.1.0'},
      publication_versions: [],
      resource: pageDefinition.resource ?? null,
      data_sources: pageDefinition.data_sources ?? [],
      actions,
      root,
      content_hash: `platform.contacts.person-form@1:${mode}:${permissions.has(contactsPersonPermissions.customFields) ? 'custom' : 'standard'}:${[...fieldNames].join(',')}`,
    },
  };
}
