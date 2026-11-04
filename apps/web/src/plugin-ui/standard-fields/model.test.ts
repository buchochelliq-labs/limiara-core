import {describe, expect, it} from 'vitest';
import {formatRecordTimestamp, formatUserReference, standardRecordFields, userReference, withStandardFields} from './model';

describe('standard record fields', () => {
  it('lists the kernel fields in display order, all read-only system fields', () => {
    expect(standardRecordFields.map(field => field.api_name)).toEqual(['id', 'name', 'created_at', 'created_by', 'updated_at', 'updated_by', 'touched_at', 'version', 'archived_at', 'is_deleted', 'deleted_at']);
    expect(standardRecordFields.every(field => field.source === 'system' && field.writable === false)).toBe(true);
    expect(standardRecordFields.find(field => field.api_name === 'touched_at')?.label).toBe('Last touched');
  });

  it('adds only the standard fields a resource does not declare', () => {
    const declared = [{api_name: 'name', label: 'Name', field_type: 'text' as const, source: 'standard' as const, readable: true, writable: true}];
    const fields = withStandardFields(declared);
    expect(fields.filter(field => field.api_name === 'name')).toEqual(declared);
    expect(fields).toHaveLength(standardRecordFields.length);
  });

  it('reads member references and names former members', () => {
    expect(formatUserReference(userReference({id: 'u1', name: 'Grace Hopper'}))).toBe('Grace Hopper');
    expect(formatUserReference(userReference({id: 'u2', name: null}))).toBe('Former member');
    expect(formatUserReference(userReference(null))).toBe('');
    expect(userReference({name: 'no id'})).toBeNull();
  });

  it('formats timestamps and ignores missing ones', () => {
    expect(formatRecordTimestamp('2026-09-01T10:05:00Z', 'UTC')).toBe('1 Sept 2026, 10:05');
    expect(formatRecordTimestamp(null)).toBe('');
    expect(formatRecordTimestamp('not a date')).toBe('');
  });
});
