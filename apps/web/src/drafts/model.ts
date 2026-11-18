import {canEditField, canReadField, holdsReference, isReference, ObjectsApiError, type ObjectDescription, type ObjectField, type ObjectRecord, type ObjectsClient, type QuickAction} from '../objects/api';
import {toApiValue, toDraftValue} from '../objects/formValues';
import type {DraftDocument, DraftField, DraftInput, DraftKey} from './types';

/** The record name is drafted like a field, under this key. */
export const NAME = 'name';

/**
 * Field-level access (ADR 0035 G5) for restoring a draft, from the
 * member's per-field `access` that describe carries: a field they may not
 * read is dropped without being named, and one they may read but not
 * change is dropped and named. Computed fields are never written.
 */
export function fieldAccess(field: ObjectField): {read: boolean; write: boolean} {
  const computed = field.type === 'formula' || field.type === 'rollup';
  return {read: canReadField(field), write: canEditField(field) && !computed};
}

/** The fields a form input differs from where the form started, each with its start value (ADR 0036 §3). */
export function changedFields(start: Record<string, DraftInput>, current: Record<string, DraftInput>, types: Record<string, string>, names: Record<string, string> = {}, objects: Record<string, string> = {}): Record<string, DraftField> {
  const changed: Record<string, DraftField> = {};
  for (const [key, value] of Object.entries(current)) {
    const begin = start[key] ?? (Array.isArray(value) ? [] : typeof value === 'boolean' ? false : '');
    if (JSON.stringify(begin) === JSON.stringify(value)) continue;
    const field: DraftField = {start: begin, value, type: types[key] ?? 'text'};
    if (names[key]) field.label = names[key];
    if (objects[key]) field.object = objects[key];
    changed[key] = field;
  }
  return changed;
}

/** The form a draft key belongs to, as the Drafts list names it. */
export function kindLabel(key: DraftKey): string {
  if (key.kind === 'edit') return 'Edit';
  if (key.kind === 'action') return 'Action';
  return 'New';
}

/** A draft's title: the record's name when it was drafted, or for a new record what the member typed. */
export function draftTitle(document: DraftDocument): string {
  if (document.record) return document.record.name;
  const typed = document.fields[NAME]?.value;
  if (typeof typed === 'string' && typed.trim()) return typed.trim();
  return `New ${document.objectLabel.toLowerCase()}`;
}

/** Whether a value of one field type still means the same for another (ADR 0036 §5); `undefined` when it does not convert. */
export function convertInput(fromType: string, field: ObjectField, value: DraftInput): DraftInput | undefined {
  if (fromType === field.type) return value;
  const text = new Set(['text', 'long_text', 'email', 'phone', 'url', 'picklist']);
  if (typeof value === 'string' && text.has(fromType) && text.has(field.type)) {
    if (field.type === 'picklist') return field.options.some(option => option.value === value) || value === '' ? value : undefined;
    return value;
  }
  const numbers = new Set(['number', 'currency', 'percent']);
  if (typeof value === 'string' && numbers.has(fromType) && numbers.has(field.type)) return value;
  if (typeof value === 'string' && numbers.has(fromType) && text.has(field.type) && field.type !== 'picklist') return value;
  return undefined;
}

/** The value a save would send for one drafted key, for comparing with the saved record (ADR 0036 §6). */
function apiValue(field: ObjectField | undefined, value: DraftInput): string {
  if (!field) return JSON.stringify(typeof value === 'string' ? value.trim() : value);
  return JSON.stringify(toApiValue(field, value));
}

export type Dropped = {named: string[]; unnamed: number};

export type RestorePlan =
  /** The object, record or action is not available to the member right now; the draft can only be discarded. */
  | {status: 'unavailable'}
  /** Everything in the draft is already in the saved record; the draft was deleted. */
  | {status: 'already_saved'}
  /** Someone changed fields the draft changed too; in this slice it can only be kept or discarded. */
  | {status: 'conflicts'; fields: number}
  | {
    status: 'ready';
    /** Values to put back into the form, by field key. */
    values: Record<string, DraftInput>;
    /** Lookup names the picker showed, for chosen records the member can still read. */
    lookupNames: Record<string, string>;
    relatedObjects: Record<string, string>;
    /** The record as it is now, which the restored form edits (with its current version). */
    record?: ObjectRecord;
    dropped: Dropped;
    /** An action draft whose action is gone or no longer runs: restore into the full form. */
    fullForm: boolean;
  };

function unavailable(error: unknown): boolean {
  return error instanceof ObjectsApiError && (error.reason === 'missing' || error.reason === 'denied');
}

/**
 * Restores a draft safely (ADR 0036 §5): re-reads the object, the record
 * and the action, and checks every field against the current definitions
 * and the member's current access. Nothing in the draft is trusted except
 * the member's own input; nothing is sent. Errors other than "not
 * available" propagate, so a network failure never deletes a draft.
 */
export async function planRestore(client: ObjectsClient, document: DraftDocument, options: {only?: readonly string[]} = {}): Promise<RestorePlan> {
  const key = document.key;
  let object: ObjectDescription;
  try {
    object = await client.describe(document.object);
  } catch (error) {
    if (unavailable(error)) return {status: 'unavailable'};
    throw error;
  }
  const needs = key.kind === 'create' ? object.access.create : object.access.edit;
  if (!needs) return {status: 'unavailable'};

  let record: ObjectRecord | undefined;
  if (key.kind !== 'create') {
    try {
      record = await client.getRecord(document.object, key.record);
    } catch (error) {
      if (unavailable(error)) return {status: 'unavailable'};
      throw error;
    }
    const editable = record.rights ? record.rights.edit : object.access.edit;
    if (record.isDeleted || !editable) return {status: 'unavailable'};
  }

  let fullForm = false;
  let actionFields: readonly string[] | undefined = options.only;
  const actionName = key.kind === 'action' ? key.action : key.kind === 'create' ? key.action : undefined;
  if (actionName) {
    const parentObject = key.kind === 'create' ? key.parent?.object : document.object;
    let action: QuickAction | undefined;
    try {
      action = parentObject ? (await client.quickActions(parentObject)).find(item => item.name === actionName) : undefined;
    } catch (error) {
      if (!unavailable(error)) throw error;
    }
    const runs = action && (action.kind === 'update' ? key.kind === 'action' : key.kind === 'create' && action.targetObject === document.object);
    if (!action || !runs) fullForm = true;
    else actionFields = actionFields ?? action.fields;
  }

  const dropped: Dropped = {named: [], unnamed: 0};
  const values: Record<string, DraftInput> = {};
  const lookupNames: Record<string, string> = {};
  const relatedObjects: Record<string, string> = {};
  const kept: Record<string, DraftField> = {};
  for (const [api, drafted] of Object.entries(document.fields)) {
    if (api === NAME) {
      if (fullForm || !actionFields || actionFields.includes(NAME)) kept[api] = drafted;
      else dropped.named.push(object.nameLabel);
      continue;
    }
    const field = object.fields.find(item => item.apiName === api);
    // Gone, or no longer readable: dropped without being named.
    if (!field || !fieldAccess(field).read) { dropped.unnamed += 1; continue; }
    if (!fieldAccess(field).write) { dropped.named.push(field.label); continue; }
    if (!fullForm && actionFields && !actionFields.includes(api)) { dropped.named.push(field.label); continue; }
    const value = convertInput(drafted.type, field, drafted.value);
    if (value === undefined) { dropped.named.push(field.label); continue; }
    // A chosen record the member can no longer read is dropped and named.
    if (holdsReference(field.type) && typeof value === 'string' && value) {
      const target = field.type === 'related_to' ? drafted.object : field.referenceObject;
      if (!target || (field.type === 'related_to' && !(field.referenceObjects ?? []).includes(target))) { dropped.named.push(field.label); continue; }
      try {
        const chosen = await client.getRecord(target, value);
        lookupNames[api] = chosen.name;
        if (field.type === 'related_to') relatedObjects[api] = target;
      } catch (error) {
        if (unavailable(error)) { dropped.named.push(field.label); continue; }
        throw error;
      }
    } else if (isReference(field.type) && drafted.label) {
      lookupNames[api] = drafted.label;
    }
    kept[api] = {...drafted, value};
  }

  const drafted = Object.keys(document.fields).length;
  if (record && document.record && record.version !== document.record.version) {
    // A newer version: compare each surviving field with the saved value (ADR 0036 §6).
    let conflicts = 0;
    for (const [api, field] of Object.entries(kept)) {
      const definition = object.fields.find(item => item.apiName === api);
      const current: DraftInput = api === NAME ? record.name : definition ? toDraftValue(definition, record.values[api]) : '';
      const saved = apiValue(definition, current);
      if (saved === apiValue(definition, field.value)) continue;
      if (saved === apiValue(definition, field.start)) values[api] = field.value;
      else conflicts += 1;
    }
    if (conflicts > 0) return {status: 'conflicts', fields: conflicts};
  } else {
    for (const [api, field] of Object.entries(kept)) values[api] = field.value;
  }
  if (drafted > 0 && Object.keys(values).length === 0 && dropped.named.length === 0 && dropped.unnamed === 0) {
    return {status: 'already_saved'};
  }
  for (const api of Object.keys(lookupNames)) if (!(api in values)) delete lookupNames[api];
  return {status: 'ready', values, lookupNames, relatedObjects, record, dropped, fullForm};
}

/** The member-facing summary of fields a restore left out (ADR 0036 §5). */
export function droppedMessage(dropped: Dropped): string[] {
  const lines: string[] = [];
  if (dropped.unnamed > 0) lines.push('Some changes were dropped because those fields no longer exist or are not available to you.');
  if (dropped.named.length > 0) lines.push(`Not restored: ${dropped.named.join(', ')}.`);
  return lines;
}
