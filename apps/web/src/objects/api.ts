// Workspace objects (ADR 0015): objects and fields administrators define at
// runtime and the records members keep in them. Served by the kernel object
// engine, never by a feature plugin. Every value from the API is data and is
// rendered as text.

export type FieldType =
  | 'text' | 'long_text' | 'email' | 'phone' | 'url' | 'number' | 'currency' | 'percent'
  | 'boolean' | 'date' | 'date_time' | 'picklist' | 'multi_picklist' | 'lookup' | 'formula'
  | 'master_detail' | 'rollup' | 'related_to';

export const fieldTypes: readonly {value: FieldType; label: string}[] = [
  {value: 'text', label: 'Text'},
  {value: 'long_text', label: 'Long text'},
  {value: 'email', label: 'Email'},
  {value: 'phone', label: 'Phone'},
  {value: 'url', label: 'URL'},
  {value: 'number', label: 'Number'},
  {value: 'currency', label: 'Currency'},
  {value: 'percent', label: 'Percent'},
  {value: 'boolean', label: 'Checkbox'},
  {value: 'date', label: 'Date'},
  {value: 'date_time', label: 'Date and time'},
  {value: 'picklist', label: 'Picklist'},
  {value: 'multi_picklist', label: 'Multi-select picklist'},
  {value: 'lookup', label: 'Lookup'},
  {value: 'master_detail', label: 'Master-detail'},
  {value: 'related_to', label: 'Related to'},
  {value: 'formula', label: 'Formula'},
  {value: 'rollup', label: 'Roll-up summary'},
];

/** How a roll-up summary field summarises its detail records (ADR 0026). */
export type RollupFunction = 'count' | 'sum' | 'min' | 'max';

export const rollupFunctions: readonly {value: RollupFunction; label: string}[] = [
  {value: 'count', label: 'Count'},
  {value: 'sum', label: 'Sum'},
  {value: 'min', label: 'Minimum'},
  {value: 'max', label: 'Maximum'},
];

/** The detail records a roll-up summary reads, by API names. */
export type FieldRollup = {function: RollupFunction; object: string; relationship: string; field: string | null};

/** Whether a field points to another record: a lookup or a master-detail field. */
export function isReference(type: FieldType): boolean {
  return type === 'lookup' || type === 'master_detail';
}

/** Whether a field's value is a record id: a lookup, master-detail or related-to field (ADR 0033). */
export function holdsReference(type: FieldType): boolean {
  return isReference(type) || type === 'related_to';
}

/** The objects a reference field may point at: its allowlist, or its one target. */
export function referenceTargets(field: ObjectField): string[] {
  if (field.type === 'related_to') return field.referenceObjects ?? [];
  return isReference(field.type) && field.referenceObject ? [field.referenceObject] : [];
}

/** The most objects a related-to field may allow (ADR 0033). */
export const referenceObjectsMax = 10;

export type PicklistOption = {value: string; label: string; active: boolean; default: boolean};

export type ObjectField = {
  /** For formula fields (ADR 0021): the formula and the type it gives. */
  formula?: string | null;
  formulaType?: FieldType | null;
  /** For roll-up summary fields (ADR 0026): what they summarise. */
  rollup?: FieldRollup | null;
  apiName: string;
  label: string;
  description: string | null;
  type: FieldType;
  origin: 'system' | 'plugin' | 'workspace';
  required: boolean;
  unique: boolean;
  indexed: boolean;
  options: PicklistOption[];
  referenceObject: string | null;
  /** For related-to fields (ADR 0033): the objects a value may point at, in order. */
  referenceObjects?: string[] | null;
  deleteBehaviour: 'clear' | 'restrict' | null;
  /** A query condition new lookup values must meet (ADR 0028). */
  lookupFilter?: string | null;
  trackHistory: boolean;
  version: number;
};

/** A record in a related list or a lookup picker (ADR 0028). */
export type RecordLink = {id: string; name: string};
/** The records of another object that point at a record through one field. */
export type RelatedList = {object: string; objectLabel: string; objectPluralLabel: string; field: string; fieldLabel: string; masterDetail: boolean; records: RecordLink[]; nextCursor: string | null};

export type ObjectAccess = {read: boolean; create: boolean; edit: boolean; delete: boolean; view_all: boolean; modify_all: boolean};

export const noAccess: ObjectAccess = {read: false, create: false, edit: false, delete: false, view_all: false, modify_all: false};

/** Who sees an object's records by default (ADR 0016). */
export type SharingModel = 'private' | 'read' | 'read_write';

export const sharingModels: readonly {value: SharingModel; label: string; description: string}[] = [
  {value: 'read_write', label: 'Public read/write', description: 'Everyone with access to the object can see and edit every record.'},
  {value: 'read', label: 'Public read only', description: 'Everyone can see every record; only owners, their managers and people it is shared with can edit.'},
  {value: 'private', label: 'Private', description: 'Only owners, their managers and people it is shared with can see a record.'},
];

/** What the caller may do with one record. */
export type RecordRights = {read: boolean; edit: boolean; delete: boolean; share: boolean; transfer: boolean};

export type RecordShare = {granteeKind: 'user' | 'role'; granteeId: string; granteeName: string | null; access: 'read' | 'edit'; createdAt: string};

export type RecordSharing = {owner: UserReference; version: number; access: RecordRights; shares: RecordShare[]};

export type Member = {id: string; name: string; managerId: string | null};

/** One tracked change (ADR 0017). `field` is `name`, `owner` or a field API name for changes. */
export type HistoryEntry = {
  id: string;
  kind: 'created' | 'changed' | 'deleted' | 'undeleted';
  field: string | null;
  oldValue: unknown;
  newValue: unknown;
  oldUser: UserReference;
  newUser: UserReference;
  changedAt: string;
  changedBy: UserReference;
};

export type HistoryPage = {entries: HistoryEntry[]; related: Record<string, string>; nextCursor: string | null};
export type Role = {id: string; name: string};

export type ObjectSummary = {
  apiName: string;
  label: string;
  pluralLabel: string;
  description: string | null;
  origin: 'system' | 'plugin' | 'workspace';
  keyPrefix: string;
  nameLabel: string;
  inNavigation: boolean;
  sharingModel: SharingModel;
  grantHierarchy: boolean;
  trackHistory: boolean;
  version: number;
  access: ObjectAccess;
};

export type ObjectDescription = ObjectSummary & {fields: ObjectField[]};

export type UserReference = {id: string; name?: string} | null;

export type ObjectRecord = {
  id: string;
  name: string;
  owner: UserReference;
  createdAt: string;
  createdBy: UserReference;
  updatedAt: string;
  updatedBy: UserReference;
  version: number;
  isDeleted: boolean;
  values: Record<string, unknown>;
  /** Names of lookup targets the caller can read, by record id. */
  related: Record<string, string>;
  /** The object of each named target, by record id. */
  relatedObjects?: Record<string, string>;
  /** What the caller may do with this record; only single-record reads carry it. */
  rights?: RecordRights;
};

export type RecordPage = {records: ObjectRecord[]; nextCursor: string | null};

export type RecordListQuery = {search?: string; status?: 'active' | 'deleted'; sort?: 'name' | 'created_at' | 'updated_at'; direction?: 'asc' | 'desc'; cursor?: string; limit?: number};

export type ValidationIssue = {field: string; code: string; detail: string};

export type ObjectsErrorReason = 'missing' | 'denied' | 'invalid' | 'conflict' | 'unavailable';

export class ObjectsApiError extends Error {
  constructor(readonly reason: ObjectsErrorReason, readonly code = '', readonly issues: ValidationIssue[] = []) {
    super(messages[reason]);
  }
}

const messages: Record<ObjectsErrorReason, string> = {
  missing: 'This item is not available.',
  denied: 'You do not have permission to do that.',
  invalid: 'Some values need attention.',
  conflict: 'This changed since you opened it, or conflicts with another record.',
  unavailable: 'The service is unavailable. Try again in a moment.',
};

export type NewObject = {name: string; label: string; plural_label: string; description?: string; name_label?: string; in_navigation?: boolean; sharing_model?: SharingModel; grant_hierarchy?: boolean; track_history?: boolean};
export type ObjectChanges = {sharing_model?: SharingModel; grant_hierarchy?: boolean; in_navigation?: boolean; track_history?: boolean};
export type FieldChanges = {track_history?: boolean; label?: string; required?: boolean; lookup_filter?: string | null; reference_objects?: string[]};
export type NewField = {name: string; label: string; field_type: FieldType; required?: boolean; unique?: boolean; indexed?: boolean; track_history?: boolean; options?: {value: string; label: string; default?: boolean}[]; reference_object?: string; reference_objects?: string[]; delete_behaviour?: 'clear' | 'restrict'; lookup_filter?: string; formula?: string; formula_type?: FieldType; scale?: number; rollup?: {function: RollupFunction; object: string; relationship: string; field?: string}};
export type RolePermission = {roleId: string; roleName: string; access: ObjectAccess};

/** A validation rule (ADR 0021): the save is blocked when `condition` is true. */
export type ValidationRule = {name: string; description: string | null; condition: string; message: string; errorField: string | null; active: boolean; version: number};
export type NewValidationRule = {name: string; description?: string | null; condition: string; message: string; error_field?: string | null; active?: boolean};
export type ValidationRuleChanges = {description?: string | null; condition?: string; message?: string; error_field?: string | null; active?: boolean};
/** A quick action (ADR 0032): a record-page shortcut that updates chosen fields or creates a related record. */
export type QuickActionKind = 'update' | 'create';
/** `fields` and `presets` use field API names, `name` meaning the record name. Create actions name the child object and its reference field. */
export type QuickAction = {name: string; label: string; kind: QuickActionKind; position: number; targetObject: string | null; targetField: string | null; fields: string[]; presets: Record<string, unknown>; version: number};
export type NewQuickAction = {name: string; label: string; kind: QuickActionKind; position?: number; target_object?: string; target_field?: string; fields?: string[]; presets?: Record<string, unknown>};
/** Kind and target are fixed once an action exists. */
export type QuickActionChanges = {label?: string; position?: number; fields?: string[]; presets?: Record<string, unknown>};
/** Quick actions per object (ADR 0032 §2). */
export const quickActionsMax = 10;
/** Record-triggered automations (ADR 0022). */
export type AutomationTrigger = 'create' | 'update' | 'create_or_update' | 'delete' | 'undelete';
export type AutomationTiming = 'before_save' | 'after_save' | 'async' | 'scheduled';
/** When a scheduled automation runs: minutes after (or before) a date field, or the change. */
export type Schedule = {offset_minutes: number; from?: string | null};
/** Sets `field` to the value of `formula`, evaluated on the saved record. */
export type Assignment = {field: string; formula: string};
/** `when`, when set, is a formula the action runs under (ADR 0022 §7). */
export type AutomationAction =
  | {type: 'update_record'; when?: string; values: Assignment[]}
  | {type: 'update_related'; when?: string; lookup: string; values: Assignment[]}
  | {type: 'create_record'; when?: string; object: string; values: Assignment[]}
  | {type: 'publish_event'; when?: string; event: string; values: Assignment[]};
export type Automation = {name: string; description: string | null; trigger: AutomationTrigger; timing: AutomationTiming; condition: string | null; actions: AutomationAction[]; schedule: Schedule | null; active: boolean; version: number};
export type NewAutomation = {name: string; description?: string; trigger: AutomationTrigger; timing: AutomationTiming; condition?: string; actions: AutomationAction[]; schedule?: Schedule; active?: boolean};
/** Empty text clears the description or condition. */
export type AutomationChanges = {description?: string; trigger?: AutomationTrigger; condition?: string; actions?: AutomationAction[]; schedule?: Schedule; active?: boolean};
/** A queued background or scheduled run (ADR 0022 §6). */
export type AutomationJob = {id: string; record: string; scheduled: boolean; dueAt: string; status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled'; attempts: number; lastError: string | null};
export const automationTriggers: {value: AutomationTrigger; label: string}[] = [
  {value: 'create', label: 'A record is created'},
  {value: 'update', label: 'A record is updated'},
  {value: 'create_or_update', label: 'A record is created or updated'},
  {value: 'delete', label: 'A record is deleted'},
  {value: 'undelete', label: 'A record is restored'},
];
/** One time an automation acted (ADR 0022 §5). */
export type AutomationRun = {id: string; record: string; event: 'create' | 'update' | 'delete' | 'undelete'; outcome: 'succeeded' | 'failed'; detail: string | null; saves: number; ranAt: string; ranBy: UserReference | null};
export type AutomationRunPage = {runs: AutomationRun[]; nextCursor: string | null};
export type SearchHit = {id: string; name: string; fields: Record<string, unknown>};
export type SearchGroup = {object: string; label: string; pluralLabel: string; records: SearchHit[]; hasMore: boolean};
export type SearchOptions = {object?: string; limit?: number};
export type QueryColumn = {key: string; type: string};
/** Rows are keyed by each selected item; record rows always carry `id`. */
export type QueryResult = {object: string; columns: QueryColumn[]; rows: Record<string, unknown>[]; nextCursor: string | null};
export type QueryOptions = {cursor?: string; pageSize?: number; includeDeleted?: boolean};

/** How a saved report shows its rows (ADR 0027). */
export type ReportChart = 'table' | 'bar' | 'line' | 'pie' | 'metric';
export const reportCharts: readonly {value: ReportChart; label: string}[] = [
  {value: 'table', label: 'Table'},
  {value: 'bar', label: 'Bar chart'},
  {value: 'line', label: 'Line chart'},
  {value: 'pie', label: 'Pie chart'},
  {value: 'metric', label: 'Single number'},
];
/** A saved report: a record query with a name and a way to show it. */
export type Report = {
  id: string;
  name: string;
  description: string | null;
  query: string;
  chart: ReportChart;
  labelColumn: string | null;
  valueColumn: string | null;
  visibility: 'private' | 'workspace';
  owner: UserReference;
  version: number;
  updatedAt: string;
  /** Whether the member may change or delete it: its owner, or a manager. */
  canEdit: boolean;
};
export type ReportInput = {name: string; description?: string | null; query: string; chart: ReportChart; label_column?: string | null; value_column?: string | null; visibility: 'private' | 'workspace'};

export type ObjectsClient = {
  /** Saved reports the member sees (ADR 0027). */
  listReports(signal?: AbortSignal): Promise<{reports: Report[]; canManage: boolean}>;
  /** A report and a page of its rows, run as the member. */
  runReport(id: string, options?: {cursor?: string}, signal?: AbortSignal): Promise<{report: Report; result: QueryResult}>;
  createReport(input: ReportInput): Promise<Report>;
  updateReport(id: string, version: number, changes: Partial<ReportInput>): Promise<Report>;
  deleteReport(id: string, version: number): Promise<void>;
  listObjects(signal?: AbortSignal): Promise<{objects: ObjectSummary[]; canManage: boolean}>;
  describe(apiName: string, signal?: AbortSignal): Promise<ObjectDescription>;
  createObject(input: NewObject): Promise<ObjectDescription>;
  updateObject(apiName: string, version: number, changes: ObjectChanges): Promise<ObjectDescription>;
  createField(apiName: string, input: NewField): Promise<ObjectField>;
  updateField(apiName: string, field: string, version: number, changes: FieldChanges): Promise<ObjectField>;
  deleteField(apiName: string, field: string, version: number): Promise<void>;
  permissions(apiName: string, signal?: AbortSignal): Promise<RolePermission[]>;
  setPermission(apiName: string, roleId: string, access: ObjectAccess): Promise<ObjectAccess>;
  listRecords(apiName: string, query: RecordListQuery, signal?: AbortSignal): Promise<RecordPage>;
  getRecord(apiName: string, id: string, signal?: AbortSignal): Promise<ObjectRecord>;
  createRecord(apiName: string, values: Record<string, unknown>): Promise<ObjectRecord>;
  updateRecord(apiName: string, id: string, version: number, values: Record<string, unknown>): Promise<ObjectRecord>;
  deleteRecord(apiName: string, id: string, version: number): Promise<ObjectRecord>;
  undeleteRecord(apiName: string, id: string, version: number): Promise<ObjectRecord>;
  getSharing(apiName: string, id: string, signal?: AbortSignal): Promise<RecordSharing>;
  history(apiName: string, id: string, cursor?: string, signal?: AbortSignal): Promise<HistoryPage>;
  /** The first records of each related list of a record (ADR 0028). */
  relatedLists(apiName: string, id: string, signal?: AbortSignal): Promise<RelatedList[]>;
  /** One more page of one related list. */
  relatedRecords(apiName: string, id: string, child: string, field: string, cursor: string, signal?: AbortSignal): Promise<RelatedList>;
  /** The records a lookup may point at, within its filter; a related-to field names the `target` object to list. */
  lookupCandidates(apiName: string, field: string, search: string, signal?: AbortSignal, target?: string): Promise<{records: RecordLink[]; nextCursor: string | null}>;
  share(apiName: string, id: string, kind: 'user' | 'role', granteeId: string, access: 'read' | 'edit'): Promise<void>;
  unshare(apiName: string, id: string, kind: 'user' | 'role', granteeId: string): Promise<void>;
  changeOwner(apiName: string, id: string, ownerId: string, version: number): Promise<ObjectRecord>;
  validationRules(apiName: string, signal?: AbortSignal): Promise<ValidationRule[]>;
  createValidationRule(apiName: string, input: NewValidationRule): Promise<ValidationRule>;
  updateValidationRule(apiName: string, name: string, version: number, changes: ValidationRuleChanges): Promise<ValidationRule>;
  deleteValidationRule(apiName: string, name: string, version: number): Promise<void>;
  /** An object's quick actions, by position then name (ADR 0032). */
  quickActions(apiName: string, signal?: AbortSignal): Promise<QuickAction[]>;
  createQuickAction(apiName: string, input: NewQuickAction): Promise<QuickAction>;
  updateQuickAction(apiName: string, name: string, version: number, changes: QuickActionChanges): Promise<QuickAction>;
  deleteQuickAction(apiName: string, name: string, version: number): Promise<void>;
  automations(apiName: string, signal?: AbortSignal): Promise<Automation[]>;
  createAutomation(apiName: string, input: NewAutomation): Promise<Automation>;
  updateAutomation(apiName: string, name: string, version: number, changes: AutomationChanges): Promise<Automation>;
  deleteAutomation(apiName: string, name: string, version: number): Promise<void>;
  automationRuns(apiName: string, name: string, cursor?: string, signal?: AbortSignal): Promise<AutomationRunPage>;
  automationJobs(apiName: string, name: string, signal?: AbortSignal): Promise<AutomationJob[]>;
  changeAutomationJob(apiName: string, name: string, id: string, change: 'retry' | 'cancel'): Promise<AutomationJob>;
  /** Global search across searchable objects (ADR 0020). */
  search(q: string, options?: SearchOptions, signal?: AbortSignal): Promise<SearchGroup[]>;
  /** Runs a record query (ADR 0019) as the signed-in member. */
  query(q: string, options?: QueryOptions, signal?: AbortSignal): Promise<QueryResult>;
  members(signal?: AbortSignal): Promise<{members: Member[]; canManage: boolean}>;
  roles(signal?: AbortSignal): Promise<Role[]>;
  setManager(userId: string, managerId: string | null): Promise<void>;
};

type ApiField = {api_name: string; label: string; description: string | null; field_type: FieldType; origin: ObjectField['origin']; required: boolean; unique: boolean; indexed: boolean; track_history?: boolean; options?: PicklistOption[]; reference_object: string | null; reference_objects?: string[] | null; delete_behaviour: ObjectField['deleteBehaviour']; lookup_filter?: string | null; formula?: string | null; formula_type?: FieldType | null; rollup?: FieldRollup | null; version: number};
type ApiHistory = {id: string; kind: HistoryEntry['kind']; field: string | null; old_value: unknown; new_value: unknown; old_user?: UserReference; new_user?: UserReference; changed_at: string; changed_by: UserReference};
type ApiObject = {api_name: string; label: string; plural_label: string; description: string | null; origin: ObjectSummary['origin']; key_prefix: string; name_label: string; in_navigation: boolean; sharing_model?: SharingModel; grant_hierarchy?: boolean; track_history?: boolean; version: number; access?: ObjectAccess; fields?: ApiField[]};
type ApiShare = {grantee_kind: 'user' | 'role'; grantee_id: string; grantee_name: string | null; access: 'read' | 'edit'; created_at: string};
type ApiRecord = Record<string, unknown> & {id: string; name: string; owner: UserReference; created_at: string; created_by: UserReference; updated_at: string; updated_by: UserReference; version: number; is_deleted: boolean; related?: Record<string, string>; access?: RecordRights};

const standardKeys = new Set(['id', 'object', 'name', 'owner', 'created_at', 'created_by', 'updated_at', 'updated_by', 'touched_at', 'version', 'archived_at', 'is_deleted', 'deleted_at', 'related', 'related_objects', 'access']);

type ApiRule = {name: string; description: string | null; condition: string; message: string; error_field: string | null; active: boolean; version: number};
export function ruleFromApi(rule: ApiRule): ValidationRule {
  return {name: rule.name, description: rule.description ?? null, condition: rule.condition, message: rule.message, errorField: rule.error_field ?? null, active: rule.active === true, version: rule.version};
}
type ApiQuickAction = {name: string; label: string; kind: QuickActionKind; position: number; target_object?: string | null; target_field?: string | null; fields?: unknown; presets?: unknown; version: number};
export function quickActionFromApi(action: ApiQuickAction): QuickAction {
  const presets = typeof action.presets === 'object' && action.presets !== null && !Array.isArray(action.presets) ? {...action.presets as Record<string, unknown>} : {};
  const fields = Array.isArray(action.fields) ? action.fields.filter((field): field is string => typeof field === 'string') : [];
  return {name: action.name, label: action.label, kind: action.kind === 'create' ? 'create' : 'update', position: typeof action.position === 'number' ? action.position : 0, targetObject: action.target_object ?? null, targetField: action.target_field ?? null, fields, presets, version: action.version};
}
type ApiRun = {id: string; record: string; event: AutomationRun['event']; outcome: AutomationRun['outcome']; detail: string | null; saves: number; ran_at: string; ran_by: UserReference | null};
export function runFromApi(run: ApiRun): AutomationRun {
  return {id: run.id, record: run.record, event: run.event, outcome: run.outcome, detail: run.detail ?? null, saves: run.saves, ranAt: run.ran_at, ranBy: run.ran_by ?? null};
}
type ApiAutomation = {name: string; description: string | null; trigger: AutomationTrigger; timing: AutomationTiming; condition: string | null; actions: AutomationAction[]; schedule?: Schedule | null; active: boolean; version: number};
type ApiJob = {id: string; record: string; scheduled: boolean; due_at: string; status: AutomationJob['status']; attempts: number; last_error: string | null};
export function jobFromApi(job: ApiJob): AutomationJob {
  return {id: job.id, record: job.record, scheduled: job.scheduled, dueAt: job.due_at, status: job.status, attempts: job.attempts, lastError: job.last_error ?? null};
}
export function automationFromApi(automation: ApiAutomation): Automation {
  return {name: automation.name, description: automation.description ?? null, trigger: automation.trigger, timing: automation.timing, condition: automation.condition ?? null, actions: Array.isArray(automation.actions) ? automation.actions : [], schedule: automation.schedule ?? null, active: automation.active === true, version: automation.version};
}
export function fieldFromApi(field: ApiField): ObjectField {
  return {apiName: field.api_name, label: field.label, description: field.description ?? null, type: field.field_type, origin: field.origin, required: field.required, unique: field.unique, indexed: field.indexed, options: Array.isArray(field.options) ? field.options : [], referenceObject: field.reference_object ?? null, referenceObjects: Array.isArray(field.reference_objects) ? field.reference_objects.filter(item => typeof item === 'string') : null, deleteBehaviour: field.delete_behaviour ?? null, lookupFilter: field.lookup_filter ?? null, trackHistory: field.track_history === true, formula: field.formula ?? null, formulaType: field.formula_type ?? null, rollup: field.rollup ?? null, version: field.version};
}

/** A JSON object of strings, dropping anything else. */
function stringMap(value: unknown): Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
}

export function objectFromApi(object: ApiObject): ObjectDescription {
  return {apiName: object.api_name, label: object.label, pluralLabel: object.plural_label, description: object.description ?? null, origin: object.origin, keyPrefix: object.key_prefix, nameLabel: object.name_label, inNavigation: object.in_navigation, sharingModel: object.sharing_model ?? 'read_write', grantHierarchy: object.grant_hierarchy !== false, trackHistory: object.track_history === true, version: object.version, access: object.access ?? noAccess, fields: (object.fields ?? []).map(fieldFromApi)};
}

type ApiReport = {id: string; name: string; description: string | null; query: string; chart: ReportChart; label_column: string | null; value_column: string | null; visibility: 'private' | 'workspace'; owner?: UserReference; version: number; updated_at: string; can_edit?: boolean};

type ApiRelated = {object: string; object_label: string; object_plural_label: string; field: string; field_label: string; master_detail: boolean; data: unknown[]; next_cursor: string | null};

function linksFromApi(rows: unknown[]): RecordLink[] {
  return rows.flatMap(row => {
    if (typeof row !== 'object' || row === null) return [];
    const {id, name} = row as Record<string, unknown>;
    return typeof id === 'string' ? [{id, name: typeof name === 'string' ? name : id}] : [];
  });
}

function relatedFromApi(list: ApiRelated): RelatedList {
  return {object: list.object, objectLabel: list.object_label, objectPluralLabel: list.object_plural_label, field: list.field, fieldLabel: list.field_label, masterDetail: list.master_detail === true, records: linksFromApi(list.data ?? []), nextCursor: list.next_cursor ?? null};
}

function reportFromApi(report: ApiReport): Report {
  return {id: report.id, name: report.name, description: report.description ?? null, query: report.query, chart: report.chart, labelColumn: report.label_column ?? null, valueColumn: report.value_column ?? null, visibility: report.visibility, owner: report.owner ?? null, version: report.version, updatedAt: report.updated_at, canEdit: report.can_edit === true};
}

export function recordFromApi(record: ApiRecord): ObjectRecord {
  const values: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(record)) if (!standardKeys.has(key)) values[key] = value;
  return {id: record.id, name: record.name, owner: record.owner ?? null, createdAt: record.created_at, createdBy: record.created_by ?? null, updatedAt: record.updated_at, updatedBy: record.updated_by ?? null, version: record.version, isDeleted: record.is_deleted === true, values, related: record.related && typeof record.related === 'object' ? Object.fromEntries(Object.entries(record.related).filter(([, name]) => typeof name === 'string')) : {}, relatedObjects: stringMap(record.related_objects), ...(isRights(record.access) ? {rights: record.access} : {})};
}

function isRights(value: unknown): value is RecordRights {
  return typeof value === 'object' && value !== null && ['read', 'edit', 'delete', 'share', 'transfer'].every(key => typeof (value as Record<string, unknown>)[key] === 'boolean');
}

async function failure(response: Response): Promise<ObjectsApiError> {
  let body: {code?: string; errors?: ValidationIssue[]} = {};
  try { body = await response.json() as typeof body; } catch { /* no problem body */ }
  const code = typeof body.code === 'string' ? body.code : '';
  const issues = Array.isArray(body.errors) ? body.errors : [];
  if (response.status === 404) return new ObjectsApiError('missing', code);
  if (response.status === 403) return new ObjectsApiError('denied', code);
  if (response.status === 400) return new ObjectsApiError('invalid', code, issues);
  if (response.status === 409) return new ObjectsApiError('conflict', code);
  return new ObjectsApiError('unavailable', code);
}

export function createObjectsApiClient(workspaceId: string, fetcher: typeof fetch = (...args) => fetch(...args)): ObjectsClient {
  const workspaceBase = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}`;
  const base = `${workspaceBase}/objects`;
  const object = (apiName: string) => `${base}/${encodeURIComponent(apiName)}`;
  async function request<T>(url: string, init: RequestInit & {json?: unknown} = {}): Promise<T> {
    const {json, ...rest} = init;
    let response: Response;
    try {
      response = await fetcher(url, {...rest, credentials: 'same-origin', headers: {accept: 'application/json', ...(json === undefined ? {} : {'content-type': 'application/json'})}, body: json === undefined ? undefined : JSON.stringify(json)});
    } catch (error) {
      if (rest.signal?.aborted) throw error;
      throw new ObjectsApiError('unavailable');
    }
    if (!response.ok) throw await failure(response);
    return await response.json() as T;
  }
  return {
    async listObjects(signal) {
      const body = await request<{data: ApiObject[]; can_manage: boolean}>(base, {signal});
      return {objects: body.data.map(objectFromApi), canManage: body.can_manage === true};
    },
    async describe(apiName, signal) {
      return objectFromApi(await request<ApiObject>(object(apiName), {signal}));
    },
    async createObject(input) {
      return objectFromApi(await request<ApiObject>(base, {method: 'POST', json: input}));
    },
    async updateObject(apiName, version, changes) {
      return objectFromApi(await request<ApiObject>(object(apiName), {method: 'PATCH', json: {...changes, expected_version: version}}));
    },
    async updateField(apiName, field, version, changes) {
      return fieldFromApi(await request<ApiField>(`${object(apiName)}/fields/${encodeURIComponent(field)}`, {method: 'PATCH', json: {...changes, expected_version: version}}));
    },
    async createField(apiName, input) {
      return fieldFromApi(await request<ApiField>(`${object(apiName)}/fields`, {method: 'POST', json: input}));
    },
    async deleteField(apiName, field, version) {
      await request<unknown>(`${object(apiName)}/fields/${encodeURIComponent(field)}?expected_version=${version}`, {method: 'DELETE'});
    },
    async permissions(apiName, signal) {
      const body = await request<{data: {role_id: string; role_name: string; access: ObjectAccess}[]}>(`${object(apiName)}/permissions`, {signal});
      return body.data.map(row => ({roleId: row.role_id, roleName: row.role_name, access: row.access}));
    },
    async setPermission(apiName, roleId, access) {
      const body = await request<{access: ObjectAccess}>(`${object(apiName)}/permissions/${encodeURIComponent(roleId)}`, {method: 'PUT', json: access});
      return body.access;
    },
    async listRecords(apiName, query, signal) {
      const params = new URLSearchParams();
      for (const [key, value] of Object.entries(query)) if (value !== undefined && value !== '') params.set(key, String(value));
      const body = await request<{data: ApiRecord[]; next_cursor: string | null}>(`${object(apiName)}/records${params.size ? `?${params}` : ''}`, {signal});
      return {records: body.data.map(recordFromApi), nextCursor: body.next_cursor ?? null};
    },
    async getRecord(apiName, id, signal) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records/${encodeURIComponent(id)}`, {signal}));
    },
    async createRecord(apiName, values) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records`, {method: 'POST', json: values}));
    },
    async updateRecord(apiName, id, version, values) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records/${encodeURIComponent(id)}`, {method: 'PATCH', json: {...values, expected_version: version}}));
    },
    async deleteRecord(apiName, id, version) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records/${encodeURIComponent(id)}?expected_version=${version}`, {method: 'DELETE'}));
    },
    async undeleteRecord(apiName, id, version) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records/${encodeURIComponent(id)}/undelete?expected_version=${version}`, {method: 'POST'}));
    },
    async getSharing(apiName, id, signal) {
      const body = await request<{owner: UserReference; version: number; access: RecordRights; shares: ApiShare[]}>(`${object(apiName)}/records/${encodeURIComponent(id)}/sharing`, {signal});
      return {owner: body.owner ?? null, version: body.version, access: body.access, shares: body.shares.map(share => ({granteeKind: share.grantee_kind, granteeId: share.grantee_id, granteeName: share.grantee_name ?? null, access: share.access, createdAt: share.created_at}))};
    },
    async relatedLists(apiName, id, signal) {
      const body = await request<{data: ApiRelated[]}>(`${object(apiName)}/records/${encodeURIComponent(id)}/related`, {signal});
      return body.data.map(relatedFromApi);
    },
    async relatedRecords(apiName, id, child, field, cursor, signal) {
      return relatedFromApi(await request<ApiRelated>(`${object(apiName)}/records/${encodeURIComponent(id)}/related/${encodeURIComponent(child)}/${encodeURIComponent(field)}?cursor=${encodeURIComponent(cursor)}`, {signal}));
    },
    async lookupCandidates(apiName, field, search, signal, target) {
      const params = new URLSearchParams();
      if (target) params.set('object', target);
      if (search.trim()) params.set('search', search.trim());
      const query = params.toString() ? `?${params.toString()}` : '';
      const body = await request<{data: unknown[]; next_cursor: string | null}>(`${object(apiName)}/fields/${encodeURIComponent(field)}/candidates${query}`, {signal});
      return {records: linksFromApi(body.data ?? []), nextCursor: body.next_cursor ?? null};
    },
    async history(apiName, id, cursor, signal) {
      const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : '';
      const body = await request<{data: ApiHistory[]; related?: Record<string, string>; next_cursor: string | null}>(`${object(apiName)}/records/${encodeURIComponent(id)}/history${query}`, {signal});
      return {
        entries: body.data.map(entry => ({id: entry.id, kind: entry.kind, field: entry.field ?? null, oldValue: entry.old_value ?? null, newValue: entry.new_value ?? null, oldUser: entry.old_user ?? null, newUser: entry.new_user ?? null, changedAt: entry.changed_at, changedBy: entry.changed_by ?? null})),
        related: body.related ?? {},
        nextCursor: body.next_cursor ?? null,
      };
    },
    async share(apiName, id, kind, granteeId, access) {
      await request<unknown>(`${object(apiName)}/records/${encodeURIComponent(id)}/sharing/${kind}/${encodeURIComponent(granteeId)}`, {method: 'PUT', json: {access}});
    },
    async unshare(apiName, id, kind, granteeId) {
      let response: Response;
      try {
        response = await fetcher(`${object(apiName)}/records/${encodeURIComponent(id)}/sharing/${kind}/${encodeURIComponent(granteeId)}`, {method: 'DELETE', credentials: 'same-origin', headers: {accept: 'application/json'}});
      } catch {
        throw new ObjectsApiError('unavailable');
      }
      if (!response.ok) throw await failure(response);
    },
    async changeOwner(apiName, id, ownerId, version) {
      return recordFromApi(await request<ApiRecord>(`${object(apiName)}/records/${encodeURIComponent(id)}/owner`, {method: 'POST', json: {owner_id: ownerId, expected_version: version}}));
    },
    async validationRules(apiName, signal) {
      const body = await request<{data: ApiRule[]}>(`${object(apiName)}/validation-rules`, {signal});
      return body.data.map(ruleFromApi);
    },
    async createValidationRule(apiName, input) {
      return ruleFromApi(await request<ApiRule>(`${object(apiName)}/validation-rules`, {method: 'POST', json: input}));
    },
    async updateValidationRule(apiName, name, version, changes) {
      return ruleFromApi(await request<ApiRule>(`${object(apiName)}/validation-rules/${encodeURIComponent(name)}`, {method: 'PATCH', json: {...changes, expected_version: version}}));
    },
    async deleteValidationRule(apiName, name, version) {
      await request<unknown>(`${object(apiName)}/validation-rules/${encodeURIComponent(name)}?expected_version=${version}`, {method: 'DELETE'});
    },
    async quickActions(apiName, signal) {
      const body = await request<{data: ApiQuickAction[]}>(`${object(apiName)}/quick-actions`, {signal});
      return body.data.map(quickActionFromApi);
    },
    async createQuickAction(apiName, input) {
      return quickActionFromApi(await request<ApiQuickAction>(`${object(apiName)}/quick-actions`, {method: 'POST', json: input}));
    },
    async updateQuickAction(apiName, name, version, changes) {
      return quickActionFromApi(await request<ApiQuickAction>(`${object(apiName)}/quick-actions/${encodeURIComponent(name)}`, {method: 'PATCH', json: {...changes, expected_version: version}}));
    },
    async deleteQuickAction(apiName, name, version) {
      await request<unknown>(`${object(apiName)}/quick-actions/${encodeURIComponent(name)}?expected_version=${version}`, {method: 'DELETE'});
    },
    async automations(apiName, signal) {
      const body = await request<{data: ApiAutomation[]}>(`${object(apiName)}/automations`, {signal});
      return body.data.map(automationFromApi);
    },
    async createAutomation(apiName, input) {
      return automationFromApi(await request<ApiAutomation>(`${object(apiName)}/automations`, {method: 'POST', json: input}));
    },
    async updateAutomation(apiName, name, version, changes) {
      return automationFromApi(await request<ApiAutomation>(`${object(apiName)}/automations/${encodeURIComponent(name)}`, {method: 'PATCH', json: {...changes, expected_version: version}}));
    },
    async deleteAutomation(apiName, name, version) {
      await request<unknown>(`${object(apiName)}/automations/${encodeURIComponent(name)}?expected_version=${version}`, {method: 'DELETE'});
    },
    async automationRuns(apiName, name, cursor, signal) {
      const params = new URLSearchParams({limit: '20'});
      if (cursor) params.set('cursor', cursor);
      const body = await request<{data: ApiRun[]; next_cursor: string | null}>(`${object(apiName)}/automations/${encodeURIComponent(name)}/runs?${params}`, {signal});
      return {runs: body.data.map(runFromApi), nextCursor: body.next_cursor ?? null};
    },
    async automationJobs(apiName, name, signal) {
      const body = await request<{data: ApiJob[]}>(`${object(apiName)}/automations/${encodeURIComponent(name)}/jobs?limit=20`, {signal});
      return body.data.map(jobFromApi);
    },
    async changeAutomationJob(apiName, name, id, change) {
      return jobFromApi(await request<ApiJob>(`${object(apiName)}/automations/${encodeURIComponent(name)}/jobs/${encodeURIComponent(id)}/${change}`, {method: 'POST'}));
    },
    async search(q, options = {}, signal) {
      const params = new URLSearchParams({q});
      if (options.object) params.set('object', options.object);
      if (options.limit !== undefined) params.set('limit', String(options.limit));
      const body = await request<{data: {object: string; label: string; plural_label: string; records: SearchHit[]; has_more: boolean}[]}>(`${workspaceBase}/search?${params}`, {signal});
      return body.data.map(group => ({object: group.object, label: group.label, pluralLabel: group.plural_label, records: group.records.map(hit => ({id: hit.id, name: hit.name, fields: hit.fields ?? {}})), hasMore: group.has_more === true}));
    },
    async listReports(signal) {
      const body = await request<{data: ApiReport[]; can_manage: boolean}>(`${workspaceBase}/reports`, {signal});
      return {reports: body.data.map(reportFromApi), canManage: body.can_manage === true};
    },
    async runReport(id, options = {}, signal) {
      const params = new URLSearchParams();
      if (options.cursor) params.set('cursor', options.cursor);
      const search = params.size ? `?${params}` : '';
      const body = await request<ApiReport & {result: {object: string; columns: QueryColumn[]; data: Record<string, unknown>[]; next_cursor: string | null}}>(`${workspaceBase}/reports/${encodeURIComponent(id)}${search}`, {signal});
      return {report: reportFromApi(body), result: {object: body.result.object, columns: body.result.columns, rows: body.result.data, nextCursor: body.result.next_cursor ?? null}};
    },
    async createReport(input) {
      return reportFromApi(await request<ApiReport>(`${workspaceBase}/reports`, {method: 'POST', json: input}));
    },
    async updateReport(id, version, changes) {
      return reportFromApi(await request<ApiReport>(`${workspaceBase}/reports/${encodeURIComponent(id)}`, {method: 'PATCH', json: {expected_version: version, ...changes}}));
    },
    async deleteReport(id, version) {
      await request<unknown>(`${workspaceBase}/reports/${encodeURIComponent(id)}?expected_version=${version}`, {method: 'DELETE'});
    },
    async query(q, options = {}, signal) {
      const params = new URLSearchParams({q});
      if (options.cursor) params.set('cursor', options.cursor);
      if (options.pageSize !== undefined) params.set('page_size', String(options.pageSize));
      if (options.includeDeleted) params.set('include_deleted', 'true');
      const body = await request<{object: string; columns: QueryColumn[]; data: Record<string, unknown>[]; next_cursor: string | null}>(`${workspaceBase}/query?${params}`, {signal});
      return {object: body.object, columns: body.columns, rows: body.data, nextCursor: body.next_cursor ?? null};
    },
    async members(signal) {
      const body = await request<{data: {id: string; name: string; manager_id: string | null}[]; can_manage: boolean}>(`${workspaceBase}/users`, {signal});
      return {members: body.data.map(member => ({id: member.id, name: member.name, managerId: member.manager_id ?? null})), canManage: body.can_manage === true};
    },
    async roles(signal) {
      const body = await request<{data: Role[]}>(`${workspaceBase}/roles`, {signal});
      return body.data;
    },
    async setManager(userId, managerId) {
      await request<unknown>(`${workspaceBase}/users/${encodeURIComponent(userId)}/manager`, {method: 'PUT', json: {manager_id: managerId}});
    },
  };
}

const fullAccess: ObjectAccess = {read: true, create: true, edit: true, delete: true, view_all: true, modify_all: true};
const fixtureUser = {id: 'usr-hannah', name: 'Hannah Okafor'};

function field(apiName: string, label: string, type: FieldType, extra: Partial<ObjectField> = {}): ObjectField {
  return {apiName, label, description: null, type, origin: 'workspace', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1, ...extra};
}

/** An in-memory engine with one Projects object, for tests and offline previews. */
export function createFixtureObjectsClient(): ObjectsClient {
  const reports: Report[] = [];
  const objects: ObjectDescription[] = [{
    apiName: 'custom:project', label: 'Project', pluralLabel: 'Projects', description: 'Client engagements', origin: 'workspace', keyPrefix: '000', nameLabel: 'Project name', inNavigation: true, sharingModel: 'read_write', grantHierarchy: true, trackHistory: true, version: 1, access: fullAccess,
    fields: [
      field('custom:status', 'Status', 'picklist', {indexed: true, trackHistory: true, options: [{value: 'open', label: 'Open', active: true, default: true}, {value: 'won', label: 'Won', active: true, default: false}]}),
      field('custom:code', 'Code', 'text', {unique: true, indexed: true}),
      field('custom:budget', 'Budget', 'currency'),
      field('custom:starts_on', 'Starts on', 'date'),
    ],
  }];
  const records: Record<string, ObjectRecord[]> = {'custom:project': [
    {id: '0000000000000001', name: 'Apollo', owner: fixtureUser, createdAt: '2026-09-01T09:00:00Z', createdBy: fixtureUser, updatedAt: '2026-09-02T09:00:00Z', updatedBy: fixtureUser, version: 1, isDeleted: false, values: {'custom:status': 'open', 'custom:code': 'AP-1', 'custom:budget': 1250.5, 'custom:starts_on': '2026-11-02'}, related: {}},
    {id: '0000000000000002', name: 'Gemini', owner: fixtureUser, createdAt: '2026-09-03T09:00:00Z', createdBy: fixtureUser, updatedAt: '2026-09-03T09:00:00Z', updatedBy: fixtureUser, version: 1, isDeleted: false, values: {'custom:status': 'won', 'custom:code': 'GE-1'}, related: {}},
  ]};
  let sequence = 3;
  const members: Member[] = [{id: 'usr-hannah', name: 'Hannah Okafor', managerId: null}, {id: 'usr-morgan', name: 'Morgan Lee', managerId: 'usr-hannah'}];
  const shares: Record<string, RecordShare[]> = {};
  const rules: Record<string, ValidationRule[]> = {};
  const automations: Record<string, Automation[]> = {};
  const quickActions: Record<string, QuickAction[]> = {};
  const byPosition = (left: QuickAction, right: QuickAction) => left.position - right.position || left.name.localeCompare(right.name);
  const fullRights: RecordRights = {read: true, edit: true, delete: true, share: true, transfer: true};
  const find = (apiName: string) => {
    const object = objects.find(item => item.apiName === apiName);
    if (!object) throw new ObjectsApiError('missing');
    return object;
  };
  const record = (apiName: string, id: string) => {
    const found = (records[apiName] ?? []).find(item => item.id === id);
    if (!found) throw new ObjectsApiError('missing');
    return found;
  };
  const stamp = (target: ObjectRecord, changes: Partial<ObjectRecord>) => Object.assign(target, changes, {version: target.version + 1, updatedAt: new Date().toISOString(), updatedBy: fixtureUser});
  return {
    async listObjects() { return {objects: objects.map(({fields: _, ...summary}) => summary), canManage: true}; },
    async describe(apiName) { return structuredClone(find(apiName)); },
    async createObject(input) {
      const apiName = `custom:${input.name}`;
      if (objects.some(item => item.apiName === apiName)) throw new ObjectsApiError('conflict', 'objects.duplicate_api_name');
      const created: ObjectDescription = {apiName, label: input.label, pluralLabel: input.plural_label, description: input.description ?? null, origin: 'workspace', keyPrefix: String(objects.length).padStart(3, '0'), nameLabel: input.name_label ?? `${input.label} name`, inNavigation: input.in_navigation ?? true, sharingModel: input.sharing_model ?? 'read_write', grantHierarchy: input.grant_hierarchy ?? true, trackHistory: input.track_history ?? false, version: 1, access: fullAccess, fields: []};
      objects.push(created);
      records[apiName] = [];
      return structuredClone(created);
    },
    async updateObject(apiName, version, changes) {
      const object = find(apiName);
      if (object.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(object, {version: object.version + 1}, changes.track_history === undefined ? {} : {trackHistory: changes.track_history}, changes.sharing_model ? {sharingModel: changes.sharing_model} : {}, changes.grant_hierarchy === undefined ? {} : {grantHierarchy: changes.grant_hierarchy}, changes.in_navigation === undefined ? {} : {inNavigation: changes.in_navigation});
      return structuredClone(object);
    },
    async createField(apiName, input) {
      const object = find(apiName);
      const created = field(`custom:${input.name}`, input.label, input.field_type, {required: (input.required ?? false) || input.field_type === 'master_detail', unique: input.unique ?? false, indexed: input.indexed ?? false, options: (input.options ?? []).map(option => ({value: option.value, label: option.label, active: true, default: option.default ?? false})), referenceObject: input.reference_object ?? null, referenceObjects: input.field_type === 'related_to' ? input.reference_objects ?? [] : null, deleteBehaviour: input.field_type === 'lookup' || input.field_type === 'related_to' ? input.delete_behaviour ?? 'clear' : null, lookupFilter: input.lookup_filter ?? null, rollup: input.rollup ? {...input.rollup, field: input.rollup.field ?? null} : null, formulaType: input.field_type === 'rollup' ? 'number' : input.formula_type ?? null});
      object.fields.push(created);
      return structuredClone(created);
    },
    async deleteField(apiName, fieldName) {
      const object = find(apiName);
      object.fields = object.fields.filter(item => item.apiName !== fieldName);
    },
    async permissions() { return [{roleId: 'role-admin', roleName: 'Administrator', access: fullAccess}, {roleId: 'role-viewer', roleName: 'Viewer', access: noAccess}]; },
    async setPermission(_apiName, _roleId, access) { return access; },
    async listRecords(apiName, query) {
      find(apiName);
      const search = query.search?.toLowerCase() ?? '';
      const deleted = query.status === 'deleted';
      const matching = (records[apiName] ?? []).filter(item => item.isDeleted === deleted && item.name.toLowerCase().includes(search));
      return {records: structuredClone(matching), nextCursor: null};
    },
    async getRecord(apiName, id) {
      const found = record(apiName, id);
      if (found.isDeleted) throw new ObjectsApiError('missing');
      return {...structuredClone(found), rights: fullRights};
    },
    async createRecord(apiName, values) {
      const object = find(apiName);
      const {name, ...rest} = values;
      if (typeof name !== 'string' || !name.trim()) throw new ObjectsApiError('invalid', 'validation.failed', [{field: 'name', code: 'records.name_required', detail: 'enter a name'}]);
      const defaults = Object.fromEntries(object.fields.flatMap(item => item.options.filter(option => option.default).map(option => [item.apiName, option.value])));
      const created: ObjectRecord = {id: `${object.keyPrefix}${String(sequence++).padStart(13, '0')}`, name: name.trim(), owner: fixtureUser, createdAt: new Date().toISOString(), createdBy: fixtureUser, updatedAt: new Date().toISOString(), updatedBy: fixtureUser, version: 1, isDeleted: false, values: {...defaults, ...Object.fromEntries(Object.entries(rest).filter(([, value]) => value !== null))}, related: {}};
      (records[apiName] ??= []).push(created);
      return structuredClone(created);
    },
    async updateRecord(apiName, id, version, values) {
      const found = record(apiName, id);
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      const {name, ...rest} = values;
      const next = {...found.values};
      for (const [key, value] of Object.entries(rest)) {
        if (value === null) delete next[key];
        else next[key] = value;
      }
      stamp(found, {name: typeof name === 'string' ? name : found.name, values: next});
      return structuredClone(found);
    },
    async deleteRecord(apiName, id, version) {
      const found = record(apiName, id);
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      stamp(found, {isDeleted: true});
      return structuredClone(found);
    },
    async undeleteRecord(apiName, id, version) {
      const found = record(apiName, id);
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      stamp(found, {isDeleted: false});
      return structuredClone(found);
    },
    async updateField(apiName, fieldName, version, changes) {
      const target = find(apiName).fields.find(item => item.apiName === fieldName);
      if (!target) throw new ObjectsApiError('missing');
      if (target.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(target, {version: target.version + 1}, changes.track_history === undefined ? {} : {trackHistory: changes.track_history}, changes.label ? {label: changes.label} : {}, changes.required === undefined ? {} : {required: changes.required}, changes.lookup_filter === undefined ? {} : {lookupFilter: changes.lookup_filter}, changes.reference_objects === undefined ? {} : {referenceObjects: changes.reference_objects});
      return structuredClone(target);
    },
    async relatedLists(apiName, id) {
      record(apiName, id);
      return objects.flatMap(child => child.fields.filter(item => referenceTargets(item).includes(apiName)).map(item => {
        const pointing = (records[child.apiName] ?? []).filter(row => !row.isDeleted && row.values[item.apiName] === id).sort((a, b) => a.name.localeCompare(b.name));
        return {object: child.apiName, objectLabel: child.label, objectPluralLabel: child.pluralLabel, field: item.apiName, fieldLabel: item.label, masterDetail: item.type === 'master_detail', records: pointing.map(row => ({id: row.id, name: row.name})), nextCursor: null};
      }));
    },
    async relatedRecords(apiName, id, child, field) {
      const list = (await this.relatedLists(apiName, id)).find(item => item.object === child && item.field === field);
      if (!list) throw new ObjectsApiError('missing');
      return list;
    },
    async lookupCandidates(apiName, fieldName, search, _signal, object) {
      const found = find(apiName).fields.find(item => item.apiName === fieldName);
      const targets = found ? referenceTargets(found) : [];
      const target = found?.type === 'related_to' ? object : targets[0];
      if (!found || !target) throw new ObjectsApiError('missing');
      if (!targets.includes(target)) throw new ObjectsApiError('invalid', 'records.reference_object_not_allowed');
      const term = search.trim().toLowerCase();
      const matching = (records[target] ?? []).filter(row => !row.isDeleted && row.name.toLowerCase().includes(term)).sort((a, b) => a.name.localeCompare(b.name));
      return {records: matching.map(row => ({id: row.id, name: row.name})), nextCursor: null};
    },
    async history(apiName, id) {
      const found = record(apiName, id);
      return {entries: [
        {id: 'h2', kind: 'changed', field: 'custom:status', oldValue: 'open', newValue: 'won', oldUser: null, newUser: null, changedAt: found.updatedAt, changedBy: found.updatedBy},
        {id: 'h1', kind: 'created', field: null, oldValue: null, newValue: null, oldUser: null, newUser: null, changedAt: found.createdAt, changedBy: found.createdBy},
      ], related: {}, nextCursor: null};
    },
    async getSharing(apiName, id) {
      const found = record(apiName, id);
      return {owner: found.owner, version: found.version, access: fullRights, shares: structuredClone(shares[id] ?? [])};
    },
    async share(apiName, id, kind, granteeId, access) {
      record(apiName, id);
      const name = kind === 'user' ? members.find(member => member.id === granteeId)?.name ?? null : granteeId === 'role-viewer' ? 'Viewer' : 'Administrator';
      shares[id] = [...(shares[id] ?? []).filter(share => share.granteeId !== granteeId), {granteeKind: kind, granteeId, granteeName: name, access, createdAt: new Date().toISOString()}];
    },
    async unshare(apiName, id, _kind, granteeId) {
      record(apiName, id);
      shares[id] = (shares[id] ?? []).filter(share => share.granteeId !== granteeId);
    },
    async changeOwner(apiName, id, ownerId, version) {
      const found = record(apiName, id);
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      const owner = members.find(member => member.id === ownerId);
      if (!owner) throw new ObjectsApiError('invalid', 'validation.failed', [{field: 'owner_id', code: 'records.owner_unknown', detail: 'choose a member of this workspace'}]);
      stamp(found, {owner: {id: owner.id, name: owner.name}});
      delete shares[id];
      return structuredClone(found);
    },
    async validationRules(apiName) { find(apiName); return structuredClone(rules[apiName] ?? []); },
    async createValidationRule(apiName, input) {
      find(apiName);
      const list = rules[apiName] ??= [];
      if (list.some(rule => rule.name === input.name)) throw new ObjectsApiError('conflict', 'rules.duplicate_name');
      const created: ValidationRule = {name: input.name, description: input.description ?? null, condition: input.condition, message: input.message, errorField: input.error_field ?? null, active: input.active ?? true, version: 1};
      list.push(created);
      return structuredClone(created);
    },
    async updateValidationRule(apiName, name, version, changes) {
      const rule = (rules[apiName] ?? []).find(item => item.name === name);
      if (!rule) throw new ObjectsApiError('missing');
      if (rule.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(rule, {
        ...(changes.description !== undefined ? {description: changes.description} : {}),
        ...(changes.condition !== undefined ? {condition: changes.condition} : {}),
        ...(changes.message !== undefined ? {message: changes.message} : {}),
        ...(changes.error_field !== undefined ? {errorField: changes.error_field} : {}),
        ...(changes.active !== undefined ? {active: changes.active} : {}),
        version: rule.version + 1,
      });
      return structuredClone(rule);
    },
    async deleteValidationRule(apiName, name) { rules[apiName] = (rules[apiName] ?? []).filter(rule => rule.name !== name); },
    async quickActions(apiName) { find(apiName); return structuredClone(quickActions[apiName] ?? []); },
    async createQuickAction(apiName, input) {
      find(apiName);
      const list = quickActions[apiName] ??= [];
      if (list.length >= quickActionsMax) throw new ObjectsApiError('invalid', 'validation.failed', [{field: 'name', code: 'quick_actions.limit_reached', detail: `an object can have at most ${quickActionsMax} quick actions`}]);
      if (list.some(item => item.name === input.name)) throw new ObjectsApiError('conflict', 'quick_actions.duplicate_name');
      const issues: ValidationIssue[] = [];
      if (!input.fields?.length) issues.push({field: 'fields', code: 'quick_actions.fields_invalid', detail: 'choose 1 to 20 fields'});
      if (input.kind === 'create' && (!input.target_object || !input.target_field)) issues.push({field: 'target_object', code: 'quick_actions.target_required', detail: 'choose the object a create action makes records of'});
      if (input.kind === 'update' && (input.target_object || input.target_field)) issues.push({field: 'target_object', code: 'quick_actions.target_not_allowed', detail: 'only create actions have a target'});
      if (issues.length) throw new ObjectsApiError('invalid', 'validation.failed', issues);
      const position = input.position ?? list.reduce((last, item) => Math.max(last, item.position + 1), 0);
      const created: QuickAction = {name: input.name, label: input.label, kind: input.kind, position, targetObject: input.target_object ?? null, targetField: input.target_field ?? null, fields: [...(input.fields ?? [])], presets: structuredClone(input.presets ?? {}), version: 1};
      list.push(created);
      list.sort(byPosition);
      return structuredClone(created);
    },
    async updateQuickAction(apiName, name, version, changes) {
      const list = quickActions[apiName] ?? [];
      const found = list.find(item => item.name === name);
      if (!found) throw new ObjectsApiError('missing');
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(found, {
        ...(changes.label !== undefined ? {label: changes.label} : {}),
        ...(changes.position !== undefined ? {position: changes.position} : {}),
        ...(changes.fields !== undefined ? {fields: [...changes.fields]} : {}),
        ...(changes.presets !== undefined ? {presets: structuredClone(changes.presets)} : {}),
        version: found.version + 1,
      });
      list.sort(byPosition);
      return structuredClone(found);
    },
    async deleteQuickAction(apiName, name, version) {
      const found = (quickActions[apiName] ?? []).find(item => item.name === name);
      if (!found) throw new ObjectsApiError('missing');
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      quickActions[apiName] = (quickActions[apiName] ?? []).filter(item => item.name !== name);
    },
    async automations(apiName) { find(apiName); return structuredClone(automations[apiName] ?? []); },
    async createAutomation(apiName, input) {
      find(apiName);
      const list = automations[apiName] ??= [];
      if (list.some(item => item.name === input.name)) throw new ObjectsApiError('conflict', 'automations.duplicate_name');
      const created: Automation = {name: input.name, description: input.description?.trim() || null, trigger: input.trigger, timing: input.timing, condition: input.condition?.trim() || null, actions: structuredClone(input.actions), schedule: input.schedule ?? null, active: input.active ?? true, version: 1};
      list.push(created);
      list.sort((left, right) => left.name.localeCompare(right.name));
      return structuredClone(created);
    },
    async updateAutomation(apiName, name, version, changes) {
      const found = (automations[apiName] ?? []).find(item => item.name === name);
      if (!found) throw new ObjectsApiError('missing');
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(found, {
        ...(changes.description !== undefined ? {description: changes.description.trim() || null} : {}),
        ...(changes.trigger !== undefined ? {trigger: changes.trigger} : {}),
        ...(changes.condition !== undefined ? {condition: changes.condition.trim() || null} : {}),
        ...(changes.actions !== undefined ? {actions: structuredClone(changes.actions)} : {}),
        ...(changes.schedule !== undefined ? {schedule: changes.schedule} : {}),
        ...(changes.active !== undefined ? {active: changes.active} : {}),
        version: found.version + 1,
      });
      return structuredClone(found);
    },
    async deleteAutomation(apiName, name) { automations[apiName] = (automations[apiName] ?? []).filter(item => item.name !== name); },
    async automationRuns(apiName, name) {
      if (!(automations[apiName] ?? []).some(item => item.name === name)) throw new ObjectsApiError('missing');
      return {runs: [], nextCursor: null};
    },
    async automationJobs(apiName, name) {
      if (!(automations[apiName] ?? []).some(item => item.name === name)) throw new ObjectsApiError('missing');
      return [];
    },
    async changeAutomationJob() { throw new ObjectsApiError('missing'); },
    async search(q, options = {}) {
      const terms = q.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
      if (terms.join('').length < 2) throw new ObjectsApiError('invalid', 'search.too_short');
      const limit = options.limit ?? 5;
      return objects.filter(object => !options.object || object.apiName === options.object).flatMap(object => {
        const matches = (records[object.apiName] ?? []).filter(item => !item.isDeleted && terms.every(term => item.name.toLowerCase().split(/[^\p{L}\p{N}]+/u).some(word => word.startsWith(term))));
        return matches.length ? [{object: object.apiName, label: object.label, pluralLabel: object.pluralLabel, records: matches.slice(0, limit).map(item => ({id: item.id, name: item.name, fields: {}})), hasMore: matches.length > limit}] : [];
      });
    },
    async query() { throw new ObjectsApiError('unavailable'); },
    async listReports() { return {reports: structuredClone(reports), canManage: true}; },
    async runReport(id) {
      const report = reports.find(item => item.id === id);
      if (!report) throw new ObjectsApiError('missing');
      return {report: structuredClone(report), result: {object: 'custom:project', columns: [], rows: [], nextCursor: null}};
    },
    async createReport(input) {
      const created: Report = {id: `report-${reports.length + 1}`, name: input.name.trim(), description: input.description?.trim() || null, query: input.query.trim(), chart: input.chart, labelColumn: input.label_column ?? null, valueColumn: input.value_column ?? null, visibility: input.visibility, owner: fixtureUser, version: 1, updatedAt: new Date().toISOString(), canEdit: true};
      reports.push(created);
      return structuredClone(created);
    },
    async updateReport(id, version, changes) {
      const found = reports.find(item => item.id === id);
      if (!found) throw new ObjectsApiError('missing');
      if (found.version !== version) throw new ObjectsApiError('conflict', 'records.version_conflict');
      Object.assign(found, {
        ...(changes.name !== undefined ? {name: changes.name.trim()} : {}),
        ...(changes.query !== undefined ? {query: changes.query.trim()} : {}),
        ...(changes.chart !== undefined ? {chart: changes.chart} : {}),
        ...(changes.label_column !== undefined ? {labelColumn: changes.label_column} : {}),
        ...(changes.value_column !== undefined ? {valueColumn: changes.value_column} : {}),
        ...(changes.visibility !== undefined ? {visibility: changes.visibility} : {}),
        version: found.version + 1,
      });
      return structuredClone(found);
    },
    async deleteReport(id) { reports.splice(0, reports.length, ...reports.filter(item => item.id !== id)); },
    async members() { return {members: structuredClone(members), canManage: true}; },
    async roles() { return [{id: 'role-admin', name: 'Administrator'}, {id: 'role-viewer', name: 'Viewer'}]; },
    async setManager(userId, managerId) {
      const member = members.find(item => item.id === userId);
      if (!member) throw new ObjectsApiError('missing');
      member.managerId = managerId;
    },
  };
}

export function resolveObjectsClient(workspaceId: string): ObjectsClient {
  return import.meta.env.MODE === 'test' ? createFixtureObjectsClient() : createObjectsApiClient(workspaceId);
}

/** A field value as display text; lookups show the related record's name when the caller can read it. */
export function formatValue(field: ObjectField, value: unknown, related: Record<string, string> = {}): string {
  if (value === undefined || value === null || value === '') return '';
  if (field.type === 'formula') return formatValue({...field, type: field.formulaType ?? 'text'}, value, related);
  if (field.type === 'rollup') return formatValue({...field, type: field.formulaType ?? 'number'}, value, related);
  if (holdsReference(field.type) && typeof value === 'string') return related[value] ?? value;
  if (field.type === 'boolean') return value === true ? 'Yes' : 'No';
  if (field.type === 'picklist') return field.options.find(option => option.value === value)?.label ?? String(value);
  if (field.type === 'multi_picklist' && Array.isArray(value)) return value.map(item => field.options.find(option => option.value === item)?.label ?? String(item)).join(', ');
  if (field.type === 'currency' && typeof value === 'number') return value.toLocaleString(undefined, {minimumFractionDigits: 2, maximumFractionDigits: 2});
  if (field.type === 'percent' && typeof value === 'number') return `${value}%`;
  if (field.type === 'date' && typeof value === 'string') {
    const date = new Date(`${value}T00:00:00`);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString(undefined, {day: 'numeric', month: 'short', year: 'numeric'});
  }
  if (field.type === 'date_time' && typeof value === 'string') {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'});
  }
  return String(value);
}
