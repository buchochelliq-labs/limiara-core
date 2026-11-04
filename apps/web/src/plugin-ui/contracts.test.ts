import {describe, expect, it} from 'vitest';
import {UI_CONTRACT_VERSION, type CallerSafePageResponse, type UiBundle} from './contracts';

const goldenBundle = {
  resources: [{
    contract_version: UI_CONTRACT_VERSION,
    id: 'platform.contacts.person',
    definition_version: 1,
    owner_plugin: 'platform.contacts',
    label: 'Person',
    fields: [{
      api_name: 'service_tier__c',
      label: 'Service tier',
      field_type: 'choice',
      source: 'tenant-custom',
      readable: true,
      writable: true,
      options: [{value: 'priority', label: 'Priority'}],
    }],
    operations: [{id: 'read', kind: 'read', permission: 'contacts.person.read'}],
  }],
  pages: [{
    contract_version: UI_CONTRACT_VERSION,
    id: 'platform.contacts.person-form',
    definition_version: 1,
    owner_plugin: 'platform.contacts',
    kind: 'record-form',
    hosts: ['internal-web'],
    resource: 'platform.contacts.person',
    root: {
      id: 'person-form',
      component: 'platform.ui.form',
      children: [{
        id: 'custom-field',
        component: 'platform.ui.field',
        props: {field: 'service_tier__c'},
        conditions: {visible: {op: 'has-permission', permission: 'contacts.person.read-custom'}},
      }],
    },
  }],
  extensions: [{
    contract_version: UI_CONTRACT_VERSION,
    id: 'customer.activity.card',
    owner_plugin: 'customer.activity',
    target_page: 'platform.contacts.person-form',
    target_version: 1,
    target_node: 'person-form',
    operation: 'inside',
    node: {id: 'activity-card', component: 'platform.ui.card', props: {title: 'Activity'}},
  }],
} satisfies UiBundle;

const callerSafeResponse = {
  page: {
    page_id: 'platform.contacts.person-form',
    definition_version: 1,
    selected_variant: null,
    source_versions: {'platform.contacts': '1.0.0'},
    publication_versions: [4],
    resource: 'platform.contacts.person',
    data_sources: [],
    actions: [],
    root: {id: 'person-form', component: 'platform.ui.form'},
    content_hash: 'caller-safe-hash',
  },
  resources: [{
    id: 'platform.contacts.person', definition_version: 1, owner_plugin: 'platform.contacts', label: 'Person',
    fields: [{api_name: 'first_name', label: 'First name', field_type: 'text', source: 'standard', readable: true, writable: true, required: false, options: []}],
    relationships: [],
    operations: [{id: 'read', kind: 'read', hosts: ['internal-web']}],
  }],
} satisfies CallerSafePageResponse;

describe('plugin UI contract mirror', () => {
  it('keeps the golden resource, page, condition, and extension shapes type-safe', () => {
    expect(goldenBundle.resources[0].contract_version).toBe(1);
    expect(goldenBundle.pages[0].root.children[0].conditions.visible.op).toBe('has-permission');
    expect(goldenBundle.extensions[0].operation).toBe('inside');
  });

  it('keeps caller-safe delivery free of permission properties', () => {
    expect(callerSafeResponse.resources[0].operations[0]).not.toHaveProperty('permission');
    expect(callerSafeResponse.page.content_hash).toBe('caller-safe-hash');
  });
});
