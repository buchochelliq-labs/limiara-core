import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {createFixtureObjectsClient, fieldTypes, formatValue, type ObjectDescription, type ObjectField} from './api';
import {RecordForm, valuesToSend} from './ObjectPages';
import {textListValues, toApiValue, toDraftValue} from './formValues';

const tags: ObjectField = {apiName: 'custom:tags', label: 'Tags', description: null, type: 'text_list', origin: 'workspace', required: false, unique: false, indexed: true, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1};

describe('text list fields (ADR 0035 G7)', () => {
  it('is a type administrators can add', () => {
    expect(fieldTypes.some(type => type.value === 'text_list')).toBe(true);
  });

  it('is edited as one chip per value', () => {
    expect(toDraftValue(tags, ['VIP', 'Speaker, keynote'])).toEqual(['VIP', 'Speaker, keynote']);
    expect(toDraftValue(tags, null)).toEqual([]);
    // A chip may hold a comma; it stays one value.
    expect(toApiValue(tags, ['VIP', ' Speaker, keynote '])).toEqual(['VIP', 'Speaker, keynote']);
    expect(toApiValue(tags, [])).toBeNull();
    expect(toApiValue(tags, ['  '])).toBeNull();
  });

  it('still reads a draft saved as one comma-separated line', () => {
    expect(textListValues(' VIP , Speaker,, ')).toEqual(['VIP', 'Speaker']);
    expect(toApiValue(tags, ' VIP , Speaker,, ')).toEqual(['VIP', 'Speaker']);
    expect(toApiValue(tags, ' , ')).toBeNull();
    expect(toApiValue(tags, '')).toBeNull();
  });

  it('shows its values joined', () => {
    expect(formatValue(tags, ['VIP', 'Speaker'])).toBe('VIP, Speaker');
  });

  it('shows the record values as chips in its form, and sends them as a list', () => {
    const access = {read: true, create: true, edit: true, delete: true, view_all: false, modify_all: false};
    const person = {apiName: 'custom:person', label: 'Person', pluralLabel: 'People', description: null, origin: 'workspace', keyPrefix: '002', nameLabel: 'Name', inNavigation: true, sharingModel: 'read_write', grantHierarchy: true, trackHistory: false, version: 1, access, fields: [tags]} as ObjectDescription;
    const record = {id: '0020000000000001', name: 'Ada', owner: null, createdAt: '2026-11-20T10:00:00Z', createdBy: null, updatedAt: '2026-11-20T10:00:00Z', updatedBy: null, version: 1, isDeleted: false, archivedAt: null, values: {'custom:tags': ['VIP', 'Speaker, keynote']}, related: {}};
    const markup = renderToStaticMarkup(<RecordForm client={createFixtureObjectsClient()} object={person} record={record} onCancel={() => undefined} onSaved={() => undefined} />);
    expect(markup).toContain('aria-label="Remove VIP"');
    expect(markup).toContain('aria-label="Remove Speaker, keynote"');
    expect(markup).not.toContain('value="VIP, Speaker, keynote"');
    const initial = {name: 'Ada', 'custom:tags': ['VIP', 'Speaker, keynote']};
    const draft = {...initial, 'custom:tags': ['VIP', 'Speaker, keynote', 'Partner']};
    expect(valuesToSend({record: true, draft, initial, shown: [tags], showsName: true})).toEqual({'custom:tags': ['VIP', 'Speaker, keynote', 'Partner']});
  });
});
