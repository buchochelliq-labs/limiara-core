import {describe, expect, it} from 'vitest';
import {fieldTypes, formatValue, type ObjectField} from './api';
import {toApiValue, toDraftValue} from './formValues';

const tags: ObjectField = {apiName: 'custom:tags', label: 'Tags', description: null, type: 'text_list', origin: 'workspace', required: false, unique: false, indexed: true, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1};

describe('text list fields (ADR 0035 G7)', () => {
  it('is a type administrators can add', () => {
    expect(fieldTypes.some(type => type.value === 'text_list')).toBe(true);
  });

  it('is typed as one line of comma-separated values', () => {
    expect(toDraftValue(tags, ['VIP', 'Speaker'])).toBe('VIP, Speaker');
    expect(toDraftValue(tags, null)).toBe('');
    expect(toApiValue(tags, ' VIP , Speaker,, ')).toEqual(['VIP', 'Speaker']);
    expect(toApiValue(tags, ' , ')).toBeNull();
    expect(toApiValue(tags, '')).toBeNull();
  });

  it('shows its values joined', () => {
    expect(formatValue(tags, ['VIP', 'Speaker'])).toBe('VIP, Speaker');
  });
});
