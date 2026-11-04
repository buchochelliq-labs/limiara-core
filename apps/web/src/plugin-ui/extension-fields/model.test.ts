import {describe, expect, it} from 'vitest';
import {billingPersonFields, contactsPersonZones, loyaltyPersonFields} from './fixtures';
import {
  extensionFieldFromApi,
  extensionFieldNodeId,
  extensionResourceField,
  fieldPlacementFromApi,
  fieldZoneFromApi,
  formatExtensionResourceValue,
  formatExtensionValue,
  placeExtensionFields,
  type ExtensionFieldSet,
} from './model';

const fields = [...loyaltyPersonFields, ...billingPersonFields];
const byId = (id: string) => {
  const field = fields.find(item => item.id === id);
  if (!field) throw new Error(id);
  return field;
};

type Node = {id: string; component: string; props?: Record<string, unknown>; children?: Node[]};

const page: Node = {id: 'page', component: 'platform.ui.page', children: [
  {id: 'person-highlights', component: 'platform.ui.highlights', props: {fields: [{key: 'organisation_id', label: 'Organisation'}, {key: 'email', label: 'Email'}]}},
  {id: 'person-identity-summary', component: 'platform.ui.record-summary', props: {fields: [{key: 'full_name', label: 'Full name'}, {key: 'email', label: 'Email'}, {key: 'telephone', label: 'Phone'}]}},
]};
const form: Node = {id: 'form', component: 'platform.ui.form', children: [
  {id: 'contact-fields', component: 'platform.ui.grid', children: [{id: 'person-email', component: 'platform.ui.field.text', props: {field: 'email'}}]},
  {id: 'additional-fields', component: 'platform.ui.grid', children: []},
]};

describe('extension field model', () => {
  it('maps API descriptors, zones and placements and drops what the client does not understand', () => {
    expect(extensionFieldFromApi({id: 'example.loyalty.tier', plugin_id: 'example.loyalty', plugin_name: 'Loyalty', target: 'platform.contacts.person', key: 'tier', label: 'Tier', description: null, field_type: 'choice', options: [{value: 'gold', label: 'Gold'}], max_length: null, writable: true}))
      .toMatchObject({id: 'example.loyalty.tier', fieldType: 'choice', writable: true});
    expect(extensionFieldFromApi({id: 'x.y.z', plugin_id: 'x.y', plugin_name: 'X', target: 't', key: 'z', label: 'Z', field_type: 'geo-point', writable: true})).toBeNull();
    expect(fieldZoneFromApi({id: 'highlights', label: 'Highlights', kind: 'display', page: 'p', node: 'n', default: false})).toMatchObject({kind: 'display'});
    expect(fieldZoneFromApi({id: 'x', label: 'X', kind: 'sidebar', page: 'p', node: 'n'})).toBeNull();
    expect(fieldPlacementFromApi({field_id: 'a.b.c', zone: 'z', position: 2})).toEqual({field_id: 'a.b.c', zone: 'z', position: 2});
    expect(fieldPlacementFromApi({field_id: 'a.b.c', zone: 'z'})).toBeNull();
  });

  it('formats values for reading and leaves unset values empty', () => {
    expect(formatExtensionValue(byId('example.loyalty.tier'), 'gold')).toBe('Gold');
    expect(formatExtensionValue(byId('example.loyalty.points'), 1200)).toBe('1,200');
    expect(formatExtensionValue(byId('example.loyalty.member_since'), '2019-05-01')).toBe('1 May 2019');
    expect(formatExtensionValue(byId('example.billing.paperless'), false)).toBe('No');
    expect(formatExtensionValue(byId('example.billing.account_code'), undefined)).toBe('');
  });

  it('describes extension fields as ordinary resource fields', () => {
    expect(extensionResourceField(byId('example.loyalty.tier'))).toMatchObject({api_name: 'example.loyalty.tier', field_type: 'choice', source: 'extension', writable: true, options: [{value: '', label: 'Not set'}, {value: 'bronze', label: 'Bronze'}, {value: 'silver', label: 'Silver'}, {value: 'gold', label: 'Gold'}]});
    expect(extensionResourceField(byId('example.billing.paperless')).options?.map(option => option.label)).toEqual(['Not set', 'Yes', 'No']);
    const tier = extensionResourceField(byId('example.loyalty.tier'));
    expect(formatExtensionResourceValue(tier, 'silver')).toBe('Silver');
    expect(formatExtensionResourceValue(extensionResourceField(byId('example.billing.paperless')), true)).toBe('Yes');
  });

  it('places fields at their positions among the host fields of each zone', () => {
    const set: ExtensionFieldSet = {fields, zones: contactsPersonZones, placements: [
      {field_id: 'example.loyalty.tier', zone: 'highlights', position: 1},
      {field_id: 'example.loyalty.points', zone: 'contact-information', position: 0},
      {field_id: 'example.loyalty.tier', zone: 'contact-information', position: 1000},
      {field_id: 'example.missing.field', zone: 'highlights', position: 0},
    ]};
    const placed = placeExtensionFields(page, 'platform.contacts.person-detail', set);
    const keys = (id: string) => (placed.children?.find(node => node.id === id)?.props?.fields as Array<{key: string}>).map(field => field.key);
    expect(keys('person-highlights')).toEqual(['organisation_id', 'example.loyalty.tier', 'email']);
    expect(keys('person-identity-summary')).toEqual(['example.loyalty.points', 'full_name', 'email', 'telephone', 'example.loyalty.tier']);
    // The original page is untouched and other pages are not affected.
    expect((page.children?.[0]?.props?.fields as unknown[]).length).toBe(2);
    expect(placeExtensionFields(page, 'platform.contacts.person-form', set)).toEqual(page);
  });

  it('adds form controls typed by field kind', () => {
    const set: ExtensionFieldSet = {fields, zones: contactsPersonZones, placements: [
      {field_id: 'example.loyalty.tier', zone: 'form-contact', position: 0},
      {field_id: 'example.loyalty.points', zone: 'form-additional', position: 5},
    ]};
    const placed = placeExtensionFields(form, 'platform.contacts.person-form', set);
    const contact = placed.children?.[0]?.children ?? [];
    expect(contact.map(node => node.id)).toEqual([extensionFieldNodeId('example.loyalty.tier'), 'person-email']);
    expect(contact[0]).toMatchObject({component: 'platform.ui.field.select', props: {field: 'example.loyalty.tier'}, bindings: {value: {source: 'field', field: 'example.loyalty.tier'}}});
    expect(placed.children?.[1]?.children?.[0]).toMatchObject({component: 'platform.ui.field.text', props: {field: 'example.loyalty.points'}});
    expect(extensionFieldNodeId('example.loyalty.tier')).toBe('extension-field-example-loyalty-tier');
  });
});
