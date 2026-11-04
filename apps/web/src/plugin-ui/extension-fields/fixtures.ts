import type {ExtensionField, ExtensionFieldSet, FieldZone} from './model';

/** Fields a loyalty feature adds to people, mirroring fixtures/plugins/loyalty. */
export const loyaltyPersonFields: ExtensionField[] = [
  {id: 'example.loyalty.member_since', pluginId: 'example.loyalty', pluginName: 'Loyalty', key: 'member_since', label: 'Member since', description: '', fieldType: 'date', options: [], maxLength: null, writable: true},
  {id: 'example.loyalty.points', pluginId: 'example.loyalty', pluginName: 'Loyalty', key: 'points', label: 'Points balance', description: '', fieldType: 'number', options: [], maxLength: null, writable: true},
  {id: 'example.loyalty.tier', pluginId: 'example.loyalty', pluginName: 'Loyalty', key: 'tier', label: 'Loyalty tier', description: 'Membership tier used for service priority', fieldType: 'choice', options: [{value: 'bronze', label: 'Bronze'}, {value: 'silver', label: 'Silver'}, {value: 'gold', label: 'Gold'}], maxLength: null, writable: true},
];

/** A second feature on the same record, read-only for the caller. */
export const billingPersonFields: ExtensionField[] = [
  {id: 'example.billing.account_code', pluginId: 'example.billing', pluginName: 'Billing', key: 'account_code', label: 'Account code', description: '', fieldType: 'text', options: [], maxLength: 12, writable: false},
  {id: 'example.billing.paperless', pluginId: 'example.billing', pluginName: 'Billing', key: 'paperless', label: 'Paperless invoices', description: '', fieldType: 'boolean', options: [], maxLength: null, writable: false},
];

/** The zones Contacts declares for people, as the API returns them. */
export const contactsPersonZones: FieldZone[] = [
  {id: 'highlights', label: 'Highlights', kind: 'display', page: 'platform.contacts.person-detail', node: 'person-highlights', default: false},
  {id: 'contact-information', label: 'Contact information', kind: 'display', page: 'platform.contacts.person-detail', node: 'person-identity-summary', default: true},
  {id: 'workspace-fields', label: 'Workspace fields', kind: 'display', page: 'platform.contacts.person-detail', node: 'person-workspace-fields', default: false},
  {id: 'record-metadata', label: 'Record metadata', kind: 'display', page: 'platform.contacts.person-detail', node: 'person-record-metadata', default: false},
  {id: 'form-contact', label: 'Form: contact details', kind: 'form', page: 'platform.contacts.person-form', node: 'contact-fields', default: false},
  {id: 'form-workspace', label: 'Form: workspace', kind: 'form', page: 'platform.contacts.person-form', node: 'workspace-fields', default: false},
  {id: 'form-additional', label: 'Form: additional details', kind: 'form', page: 'platform.contacts.person-form', node: 'additional-fields', default: true},
  {id: 'list-columns', label: 'People list columns', kind: 'list', page: 'platform.contacts.people-directory', node: 'people-collection', default: false},
];

/** Loyalty fields as an unarranged workspace receives them: in each default zone. */
export const loyaltyDefaultSet: ExtensionFieldSet = {
  fields: loyaltyPersonFields,
  zones: contactsPersonZones,
  placements: loyaltyPersonFields.flatMap(field => [
    {field_id: field.id, zone: 'contact-information', position: 1000},
    {field_id: field.id, zone: 'form-additional', position: 1000},
  ]),
};
