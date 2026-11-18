import {canReadField, type ObjectDescription, type ObjectField} from './api';

/** What a form input holds for one field: text, a checkbox, or the chosen values of a multi-select. */
export type DraftValue = string | boolean | string[];

/**
 * Fields a form shows: stored fields the member may read, since formula
 * fields are computed (ADR 0021). Those the member may not change are shown
 * read-only and never sent (G5).
 */
export function editableFields(object: ObjectDescription) {
  return object.fields.filter(field => field.type !== 'formula' && field.type !== 'rollup' && canReadField(field));
}

/** An API value as what a form input holds; the reverse of `toApiValue`. */
export function toDraftValue(field: ObjectField, value: unknown): DraftValue {
  if (field.type === 'boolean') return value === true;
  if (field.type === 'multi_picklist') return Array.isArray(value) ? value.map(String) : [];
  if (field.type === 'date_time' && typeof value === 'string') return toLocalInput(value);
  return value === undefined || value === null ? '' : String(value);
}

/** `datetime-local` works in the member's time zone; the API stores UTC. */
function toLocalInput(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}

/** What a form input holds as the value the record API takes for its field. */
export function toApiValue(field: ObjectField, value: DraftValue): unknown {
  if (field.type === 'boolean') return value === true;
  if (field.type === 'multi_picklist') return Array.isArray(value) && value.length ? value : null;
  if (typeof value !== 'string' || value.trim() === '') return null;
  if (field.type === 'number' || field.type === 'currency' || field.type === 'percent') {
    const number = Number(value);
    return Number.isFinite(number) ? number : value;
  }
  if (field.type === 'date_time') {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toISOString();
  }
  return value.trim();
}

