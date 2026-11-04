// Fields that other plugins add to a host record (ADR 0012).
//
// The kernel extension service decides which fields exist for the caller and
// whether each is writable; this module only shapes them for display and
// turns an edit draft into the values the service validates again. Labels,
// descriptions and values come from other plugins, so they are always
// rendered as plain text.

export type ExtensionFieldType = 'text' | 'number' | 'boolean' | 'date' | 'choice';

export type ExtensionFieldOption = {value: string; label: string};

export type ExtensionField = {
  /** `<plugin id>.<key>`, the id the API reads and writes. */
  id: string;
  pluginId: string;
  pluginName: string;
  key: string;
  label: string;
  description: string;
  fieldType: ExtensionFieldType;
  options: ExtensionFieldOption[];
  maxLength: number | null;
  writable: boolean;
};

/** The wire shape of `GET .../extensions/fields`. */
export type ApiExtensionField = {
  id: string;
  plugin_id: string;
  plugin_name: string;
  target: string;
  key: string;
  label: string;
  description?: string | null;
  field_type: string;
  options?: ExtensionFieldOption[] | null;
  max_length?: number | null;
  writable: boolean;
};

const fieldTypes: readonly ExtensionFieldType[] = ['text', 'number', 'boolean', 'date', 'choice'];

function isFieldType(value: string): value is ExtensionFieldType {
  return (fieldTypes as readonly string[]).includes(value);
}

/** Maps one API descriptor, dropping types this client does not understand. */
export function extensionFieldFromApi(field: ApiExtensionField): ExtensionField | null {
  if (!isFieldType(field.field_type)) return null;
  return {
    id: field.id,
    pluginId: field.plugin_id,
    pluginName: field.plugin_name,
    key: field.key,
    label: field.label,
    description: field.description ?? '',
    fieldType: field.field_type,
    options: Array.isArray(field.options) ? field.options : [],
    maxLength: typeof field.max_length === 'number' ? field.max_length : null,
    writable: field.writable === true,
  };
}

/** Display text for a stored value; empty when the value is not set. */
export function formatExtensionValue(field: ExtensionField, value: unknown, locale = 'en-GB'): string {
  if (value === null || value === undefined || value === '') return '';
  switch (field.fieldType) {
    case 'boolean':
      return value === true ? 'Yes' : value === false ? 'No' : '';
    case 'choice':
      return field.options.find(option => option.value === value)?.label ?? String(value);
    case 'number':
      return typeof value === 'number' ? new Intl.NumberFormat(locale).format(value) : String(value);
    case 'date': {
      if (typeof value !== 'string') return '';
      const parsed = new Date(`${value}T00:00:00Z`);
      return Number.isNaN(parsed.valueOf()) ? value : new Intl.DateTimeFormat(locale, {dateStyle: 'medium', timeZone: 'UTC'}).format(parsed);
    }
    case 'text':
      return typeof value === 'string' ? value : String(value);
  }
}

// --- Placement on host pages ---------------------------------------------

export type FieldZoneKind = 'display' | 'form' | 'list';

/** A place on a host page that can hold extension fields, as the host declares it. */
export type FieldZone = {id: string; label: string; kind: FieldZoneKind; page: string; node: string; default: boolean};

/** One field shown in one zone; `position` counts every field in the zone, the host's included. */
export type FieldPlacement = {field_id: string; zone: string; position: number};

/** What a page needs to show extension fields as part of the record. */
export type ExtensionFieldSet = {fields: ExtensionField[]; zones: FieldZone[]; placements: FieldPlacement[]};

export const emptyExtensionFieldSet: ExtensionFieldSet = {fields: [], zones: [], placements: []};

const zoneKinds: readonly FieldZoneKind[] = ['display', 'form', 'list'];

export function fieldZoneFromApi(value: unknown): FieldZone | null {
  if (typeof value !== 'object' || value === null) return null;
  const zone = value as Record<string, unknown>;
  const kind = zone.kind;
  if (typeof zone.id !== 'string' || typeof zone.label !== 'string' || typeof zone.page !== 'string' || typeof zone.node !== 'string' || typeof kind !== 'string' || !(zoneKinds as readonly string[]).includes(kind)) return null;
  return {id: zone.id, label: zone.label, kind: kind as FieldZoneKind, page: zone.page, node: zone.node, default: zone.default === true};
}

export function fieldPlacementFromApi(value: unknown): FieldPlacement | null {
  if (typeof value !== 'object' || value === null) return null;
  const placement = value as Record<string, unknown>;
  if (typeof placement.field_id !== 'string' || typeof placement.zone !== 'string' || typeof placement.position !== 'number') return null;
  return {field_id: placement.field_id, zone: placement.zone, position: placement.position};
}

const resourceFieldTypes = {text: 'text', number: 'number', boolean: 'boolean', date: 'date', choice: 'choice'} as const;

/**
 * The extension field as an ordinary resource field, so host pages, forms and
 * lists treat it like any field of the record.
 */
export function extensionResourceField(field: ExtensionField) {
  const options = field.fieldType === 'choice'
    ? [{value: '', label: 'Not set'}, ...field.options]
    : field.fieldType === 'boolean'
      ? [{value: '', label: 'Not set'}, {value: 'true', label: 'Yes'}, {value: 'false', label: 'No'}]
      : undefined;
  return {
    api_name: field.id,
    label: field.label,
    ...(field.description ? {description: field.description} : {}),
    field_type: resourceFieldTypes[field.fieldType],
    source: 'extension' as const,
    readable: true,
    writable: field.writable,
    required: false,
    ...(options ? {options} : {}),
  };
}

type NodeLike = {id: string; component: string; props?: Record<string, unknown>; children?: NodeLike[]};

function insertAt<T>(items: T[], placements: readonly {position: number; item: T}[]): T[] {
  const result = [...items];
  for (const {position, item} of [...placements].sort((left, right) => left.position - right.position)) {
    result.splice(Math.min(position, result.length), 0, item);
  }
  return result;
}

/** A stable, selector-safe node id for an extension field's form control. */
export function extensionFieldNodeId(fieldId: string): string {
  return `extension-field-${fieldId.replace(/[^a-z0-9]+/gi, '-')}`;
}

/**
 * Puts placed extension fields into the zones of one page, at their positions
 * among the host's own fields. Display and list zones gain field entries;
 * form zones gain field controls. Fields the caller cannot read are absent
 * from `set.fields` and therefore never placed.
 */
export function placeExtensionFields<T extends NodeLike>(root: T, pageId: string, set: ExtensionFieldSet): T {
  const fields = new Map(set.fields.map(field => [field.id, field]));
  const zonesByNode = new Map(set.zones.filter(zone => zone.page === pageId).map(zone => [zone.node, zone]));
  if (zonesByNode.size === 0 || fields.size === 0) return root;
  const visit = (node: NodeLike): NodeLike => {
    const children = node.children?.map(visit);
    const zone = zonesByNode.get(node.id);
    const next: NodeLike = {...node, ...(children ? {children} : {})};
    if (!zone) return next;
    const placed = set.placements
      .filter(placement => placement.zone === zone.id)
      .map(placement => ({placement, field: fields.get(placement.field_id)}))
      .filter((entry): entry is {placement: FieldPlacement; field: ExtensionField} => entry.field !== undefined);
    if (placed.length === 0) return next;
    if (zone.kind === 'form') {
      const controls = placed.map(({placement, field}) => ({position: placement.position, item: {
        id: extensionFieldNodeId(field.id),
        component: field.fieldType === 'choice' || field.fieldType === 'boolean' ? 'platform.ui.field.select' : 'platform.ui.field.text',
        props: {field: field.id},
        bindings: {value: {source: 'field', field: field.id}},
      } as NodeLike}));
      return {...next, children: insertAt(next.children ?? [], controls)};
    }
    const entries = Array.isArray(next.props?.fields) ? next.props.fields as unknown[] : [];
    const added = placed.map(({placement, field}) => ({position: placement.position, item: {key: field.id, label: field.label} as unknown}));
    return {...next, props: {...next.props, fields: insertAt(entries, added)}};
  };
  return visit(root) as T;
}

/** Display text for a value of an extension field described as a resource field. */
export function formatExtensionResourceValue(field: {api_name: string; label: string; field_type: string; options?: ReadonlyArray<{value: string; label: string}>}, value: unknown): string {
  const fieldType = (fieldTypes as readonly string[]).includes(field.field_type) ? field.field_type as ExtensionFieldType : 'text';
  return formatExtensionValue({
    id: field.api_name, pluginId: '', pluginName: '', key: '', label: field.label, description: '',
    fieldType, options: (field.options ?? []).filter(option => option.value !== ''), maxLength: null, writable: false,
  }, value);
}
