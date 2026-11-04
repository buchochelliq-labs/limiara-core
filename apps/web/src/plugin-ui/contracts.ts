export const UI_CONTRACT_VERSION = 1 as const;

export type HostProfile = 'internal-web' | 'desktop' | 'portal' | 'public-form';
export type FieldType = 'text' | 'long-text' | 'email' | 'phone' | 'number' | 'boolean' | 'date' | 'date-time' | 'choice' | 'multi-choice' | 'reference' | 'money' | 'json' | 'user';
/**
 * `extension`: a field another plugin adds to this resource (ADR 0012).
 * `system`: a standard field the kernel defines on every record (ADR 0014).
 */
export type FieldSource = 'standard' | 'tenant-custom' | 'extension' | 'system';
export type DataClassification = 'public' | 'internal' | 'personal' | 'sensitive';

export type FieldDescriptor = {
  api_name: string;
  label: string;
  description?: string;
  field_type: FieldType;
  source: FieldSource;
  readable?: boolean;
  writable?: boolean;
  required?: boolean;
  permission?: string;
  classification?: DataClassification;
  options?: Array<{value: string; label: string}>;
  validation_schema?: unknown;
  default_value?: unknown;
  submit_when_hidden?: boolean;
};

export type ResourceDescriptor = {
  contract_version: typeof UI_CONTRACT_VERSION;
  id: string;
  definition_version: number;
  owner_plugin: string;
  label: string;
  /** The declared field that is the record's display name (ADR 0014). */
  name_field?: string;
  fields?: FieldDescriptor[];
  relationships?: Array<{
    id: string;
    target_resource: string;
    cardinality: 'one' | 'many';
    permission?: string;
  }>;
  operations?: Array<{
    id: string;
    kind: 'read' | 'list' | 'create' | 'update' | 'archive' | 'command';
    permission: string;
    concurrency?: 'none' | 'etag';
    hosts?: HostProfile[];
  }>;
};

export type Binding =
  | {source: 'field'; field: string}
  | {source: 'context'; path: string}
  | {source: 'literal'; value: unknown}
  | {source: 'data-source'; data_source: string; path: string};

export type DisplayField = {
  key: string;
  label: string;
  format?: 'text' | 'email' | 'phone' | 'number' | 'date' | 'date-time' | 'boolean' | 'money' | 'user';
  /** The sort value a collection sends when this column's header is activated. */
  sort?: string;
};

export type FilterChip = {
  id: string;
  label: string;
  action: string;
  parameter?: string;
  value?: string;
};

export type CollectionItem = {
  id: string;
  title: string;
  description?: string;
  archived?: boolean;
  values?: Record<string, unknown>;
};

export type RecordSummary = {
  id: string;
  title: string;
  description?: string;
  archived?: boolean;
  values?: Record<string, unknown>;
};

export type ConditionExpression =
  | {op: 'and' | 'or'; args: ConditionExpression[]}
  | {op: 'not'; arg: ConditionExpression}
  | {op: 'equal' | 'not-equal'; left: Binding; right: Binding}
  | {op: 'exists'; value: Binding}
  | {op: 'in'; value: Binding; set: Binding[]}
  | {op: 'has-permission'; permission: string};

export type UiNode = {
  id: string;
  component: string;
  props?: Record<string, unknown>;
  bindings?: Record<string, Binding>;
  conditions?: Partial<Record<'visible' | 'enabled' | 'read-only' | 'required', ConditionExpression>>;
  children?: UiNode[];
  permission?: string;
  hosts?: HostProfile[];
};

export type PageDefinition = {
  contract_version: typeof UI_CONTRACT_VERSION;
  id: string;
  definition_version: number;
  owner_plugin: string;
  kind: 'record' | 'record-form' | 'collection' | 'kanban' | 'calendar' | 'timeline' | 'dashboard' | 'wizard' | 'settings' | 'portal' | 'public-form';
  hosts: HostProfile[];
  resource?: string;
  data_sources?: Array<{
    id: string;
    resource: string;
    operation: string;
    parameters?: Record<string, Binding>;
    load?: 'eager' | 'explicit';
  }>;
  actions?: Array<{
    id: string;
    kind: 'navigate' | 'submit' | 'overlay' | 'resource-operation';
    target: string;
    parameters?: Record<string, Binding>;
    permission?: string;
  }>;
  variants?: Array<{
    id: string;
    priority?: number;
    condition?: ConditionExpression;
    root: UiNode;
  }>;
  root: UiNode;
};

export type FragmentDefinition = {
  contract_version: typeof UI_CONTRACT_VERSION;
  id: string;
  definition_version: number;
  owner_plugin: string;
  hosts: HostProfile[];
  root: UiNode;
};

export type ComponentDefinition = {
  contract_version: typeof UI_CONTRACT_VERSION;
  id: string;
  definition_version: number;
  owner_plugin: string;
  props_schema: unknown;
  slots?: Array<{id: string; accepts?: string[]; multiple?: boolean}>;
  hosts: HostProfile[];
  capabilities?: string[];
  accessibility: {
    semantics: string;
    keyboard?: string[];
    required_label_prop?: string;
  };
};

export type ExtensionContribution = {
  contract_version: typeof UI_CONTRACT_VERSION;
  id: string;
  owner_plugin: string;
  target_page: string;
  target_version: number;
  target_node: string;
  operation: 'inside' | 'before' | 'after' | 'move' | 'configure' | 'replace';
  priority?: number;
  node?: UiNode;
  props?: Record<string, unknown>;
};

export type UiBundle = {
  resources?: ResourceDescriptor[];
  pages?: PageDefinition[];
  fragments?: FragmentDefinition[];
  components?: ComponentDefinition[];
  extensions?: ExtensionContribution[];
};

export type ResolvedPage = {
  page_id: string;
  definition_version: number;
  selected_variant: string | null;
  source_versions: Record<string, string>;
  publication_versions: number[];
  resource: string | null;
  data_sources: NonNullable<PageDefinition['data_sources']>;
  actions: NonNullable<PageDefinition['actions']>;
  root: UiNode;
  content_hash: string;
};

export type CallerSafeFieldDescriptor = Omit<FieldDescriptor, 'permission'>;
export type CallerSafeRelationshipDescriptor = Omit<NonNullable<ResourceDescriptor['relationships']>[number], 'permission'>;
export type CallerSafeOperationDescriptor = Omit<NonNullable<ResourceDescriptor['operations']>[number], 'permission'>;

export type CallerSafeResourceDescriptor = {
  id: string;
  definition_version: number;
  owner_plugin: string;
  label: string;
  name_field?: string;
  fields: CallerSafeFieldDescriptor[];
  relationships: CallerSafeRelationshipDescriptor[];
  operations: CallerSafeOperationDescriptor[];
};

export type FormMode = 'create' | 'edit' | 'view' | 'quick-create' | 'wizard' | 'embedded' | 'related-record';

export type CallerSafePageResponse = {
  page: ResolvedPage;
  resources: CallerSafeResourceDescriptor[];
};

export type CallerSafeResourceResponse = {
  resource: CallerSafeResourceDescriptor;
};
