// Pure editing operations behind the "Arrange fields" editor. Each zone is an
// ordered list mixing the host's own fields, which stay in their relative
// order, with placed extension fields an administrator can move anywhere
// among them.

import type {ExtensionField, FieldPlacement, FieldZone} from './model';

export type HostField = {key: string; label: string};

export type ArrangementItem =
  | {kind: 'host'; key: string; label: string}
  | {kind: 'field'; id: string};

export type Arrangement = Record<string, ArrangementItem[]>;

/** The current arrangement of every zone, from the host's fields and the effective placements. */
export function arrangementFrom(zones: readonly FieldZone[], hostFields: Readonly<Record<string, readonly HostField[]>>, placements: readonly FieldPlacement[]): Arrangement {
  const arrangement: Arrangement = {};
  for (const zone of zones) {
    const items: ArrangementItem[] = (hostFields[zone.id] ?? []).map(field => ({kind: 'host', key: field.key, label: field.label}));
    const placed = placements
      .filter(placement => placement.zone === zone.id)
      .sort((left, right) => left.position - right.position);
    for (const placement of placed) {
      if (items.some(item => item.kind === 'field' && item.id === placement.field_id)) continue;
      items.splice(Math.min(placement.position, items.length), 0, {kind: 'field', id: placement.field_id});
    }
    arrangement[zone.id] = items;
  }
  return arrangement;
}

/** Moves a placed field one step up or down within its zone, past host fields too. */
export function moveField(arrangement: Arrangement, zone: string, fieldId: string, direction: -1 | 1): Arrangement {
  const items = [...(arrangement[zone] ?? [])];
  const index = items.findIndex(item => item.kind === 'field' && item.id === fieldId);
  const target = index + direction;
  if (index < 0 || target < 0 || target >= items.length) return arrangement;
  const [item] = items.splice(index, 1);
  if (!item) return arrangement;
  items.splice(target, 0, item);
  return {...arrangement, [zone]: items};
}

export function removeField(arrangement: Arrangement, zone: string, fieldId: string): Arrangement {
  return {...arrangement, [zone]: (arrangement[zone] ?? []).filter(item => !(item.kind === 'field' && item.id === fieldId))};
}

/** Adds a field at the end of a zone; a field appears at most once per zone. */
export function addField(arrangement: Arrangement, zone: string, fieldId: string): Arrangement {
  const items = arrangement[zone] ?? [];
  if (items.some(item => item.kind === 'field' && item.id === fieldId)) return arrangement;
  return {...arrangement, [zone]: [...items, {kind: 'field', id: fieldId}]};
}

/** Fields that can still be added to a zone. */
export function addableFields(arrangement: Arrangement, zone: string, fields: readonly ExtensionField[]): ExtensionField[] {
  const present = new Set((arrangement[zone] ?? []).flatMap(item => item.kind === 'field' ? [item.id] : []));
  return fields.filter(field => !present.has(field.id));
}

/**
 * The layout to save. Every field is marked as arranged, so a field taken out
 * of every zone stays hidden instead of returning to the default zone.
 */
export function layoutFromArrangement(arrangement: Arrangement, fields: readonly ExtensionField[]): {arranged: string[]; placements: FieldPlacement[]} {
  const placements: FieldPlacement[] = [];
  for (const [zone, items] of Object.entries(arrangement)) {
    items.forEach((item, position) => {
      if (item.kind === 'field') placements.push({field_id: item.id, zone, position});
    });
  }
  return {arranged: fields.map(field => field.id), placements};
}
