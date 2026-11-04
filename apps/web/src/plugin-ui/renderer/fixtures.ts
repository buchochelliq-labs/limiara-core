import type {CallerSafeResourceDescriptor} from '../contracts';
import type {ResolvedUiPage} from './MetadataPageRenderer';

export const rendererMessages = {
  'people.page.title': 'People workspace',
  'people.page.description': 'A single metadata fixture rendered by the host at every supported width.',
  'people.details.title': 'Contact details',
  'people.details.description': 'Labels, descriptions, errors, and actions remain host-owned.',
  'people.name.label': 'Full name',
  'people.name.description': 'Use the name this person prefers.',
  'people.email.label': 'Email address',
  'people.email.error': 'Enter a valid email address.',
  'people.actions.label': 'Record actions',
  'people.save': 'Save person',
  'people.cancel': 'Cancel',
} as const;

export const rendererFixture: ResolvedUiPage = {
  page_id: 'platform.contacts.person.fixture',
  definition_version: 1,
  selected_variant: null,
  source_versions: {'platform.contacts': '0.1.0'},
  publication_versions: [],
  resource: 'platform.contacts.person',
  data_sources: [],
  actions: [],
  content_hash: 'fixture-content-hash',
  root: {
    id: 'person-page',
    component: 'platform.ui.page',
    props: {title_key: 'people.page.title', description_key: 'people.page.description'},
    children: [{
      id: 'page-stack',
      component: 'platform.ui.stack',
      props: {gap: 'large'},
      children: [{
        id: 'details-section',
        component: 'platform.ui.section',
        props: {title_key: 'people.details.title', description_key: 'people.details.description'},
        children: [{
          id: 'field-grid',
          component: 'platform.ui.grid',
          props: {columns: 2},
          children: [{
            id: 'full-name',
            component: 'platform.ui.field.text',
            props: {name: 'full_name', label_key: 'people.name.label', description_key: 'people.name.description', value: 'Hannah Carter', required: true},
          }, {
            id: 'email',
            component: 'platform.ui.field.text',
            props: {name: 'email', label_key: 'people.email.label', error_key: 'people.email.error', value: 'invalid-address', type: 'email'},
          }],
        }],
      }, {
        id: 'summary-split',
        component: 'platform.ui.split',
        children: [{
          id: 'activity-card',
          component: 'platform.ui.card',
          props: {title: 'Recent activity', description: 'A responsive card contributed through a stable slot.'},
          children: [{id: 'activity-text', component: 'platform.ui.text', props: {text: 'No activity has been recorded yet.', tone: 'muted'}}],
        }, {
          id: 'related-slot',
          component: 'platform.ui.slot',
          props: {label: 'Related information'},
          children: [{id: 'notice', component: 'platform.ui.alert', props: {title: 'Fixture mode', variant: 'info'}, children: [{id: 'notice-text', component: 'platform.ui.text', props: {text: 'This example does not submit data.'}}]}],
        }],
      }, {
        id: 'record-actions',
        component: 'platform.ui.action-bar',
        props: {label_key: 'people.actions.label'},
        children: [{id: 'cancel-action', component: 'platform.ui.action', props: {label_key: 'people.cancel', variant: 'quiet'}}, {id: 'save-action', component: 'platform.ui.action', props: {label_key: 'people.save', variant: 'primary'}}],
      }],
    }],
  },
};

export const collectionRendererDataSources = {
  people: {
    items: [
      {id: 'maya', title: 'Maya Patel', description: 'Northstar Services', values: {email: 'maya@northstar.example', relationship: 'Client'}},
      {id: 'daniel', title: 'Daniel Okafor', description: 'Field House Trust', values: {email: 'daniel@fieldhouse.example', relationship: 'Partner'}},
      {id: 'ella', title: 'Ella Brooks', description: 'Morrow Engineering', archived: true, values: {email: 'ella@morrow.example', relationship: 'Client'}},
    ],
    selected: {id: 'maya', title: 'Maya Patel', description: 'Northstar Services', values: {email: 'maya@northstar.example', relationship: 'Client'}},
    next_cursor: 'opaque-next-page',
  },
} as const;

export const collectionRendererResources: CallerSafeResourceDescriptor[] = [{
  id: 'platform.example.person',
  definition_version: 1,
  owner_plugin: 'platform.example',
  label: 'Person',
  fields: [
    {api_name: 'email', label: 'Email', field_type: 'email', source: 'standard', readable: true, writable: false},
    {api_name: 'relationship', label: 'Relationship', field_type: 'text', source: 'standard', readable: true, writable: false},
  ],
  relationships: [],
  operations: [{id: 'list', kind: 'list', hosts: ['internal-web', 'desktop']}],
}];

export const collectionRendererFixture: ResolvedUiPage = {
  page_id: 'platform.example.collection.fixture',
  definition_version: 1,
  selected_variant: null,
  source_versions: {'platform.example': '0.1.0'},
  publication_versions: [],
  resource: null,
  data_sources: [{id: 'people', resource: 'platform.example.person', operation: 'list'}],
  actions: [
    {id: 'search', kind: 'navigate', target: 'example.people', parameters: {q: {source: 'literal', value: ''}}},
    {id: 'sort', kind: 'navigate', target: 'example.people', parameters: {sort: {source: 'literal', value: 'name'}}},
    {id: 'select', kind: 'navigate', target: 'example.people', parameters: {person: {source: 'literal', value: ''}}},
    {id: 'previous', kind: 'navigate', target: 'example.people', parameters: {cursor: {source: 'literal', value: ''}}},
    {id: 'next', kind: 'navigate', target: 'example.people', parameters: {cursor: {source: 'literal', value: ''}}},
  ],
  content_hash: 'collection-fixture-content-hash',
  root: {
    id: 'collection-page',
    component: 'platform.ui.page',
    props: {title: 'Collection and record workspace', description: 'Generic data-source bindings, controls, selection, preview, cursors, and slots.'},
    children: [{
      id: 'collection-stack', component: 'platform.ui.stack', props: {gap: 'large'}, children: [{
        id: 'people-controls', component: 'platform.ui.collection-controls', props: {label: 'People controls'}, children: [
          {id: 'people-search', component: 'platform.ui.query-input', props: {label: 'Search people', action: 'search', parameter: 'q', placeholder: 'Name, email or organisation', submit_label: 'Search'}},
          {id: 'people-sort', component: 'platform.ui.query-select', props: {label: 'Sort people', action: 'sort', parameter: 'sort', value: 'name', options: [{value: 'name', label: 'Name'}, {value: 'organisation', label: 'Organisation'}]}},
        ],
      }, {
        id: 'people-workspace', component: 'platform.ui.split', children: [
          {id: 'people-results', component: 'platform.ui.collection', props: {label: 'People', resource: 'platform.example.person', fields: [{key: 'email', label: 'Email', format: 'email'}, {key: 'relationship', label: 'Relationship'}], presentation: 'auto', selected_id: 'maya', select_action: 'select', selection_parameter: 'person'}, bindings: {items: {source: 'data-source', data_source: 'people', path: 'items'}}},
          {id: 'person-preview', component: 'platform.ui.record-preview', props: {label: 'Selected person'}, children: [
            {id: 'person-summary', component: 'platform.ui.record-summary', props: {label: 'Person summary', resource: 'platform.example.person', fields: [{key: 'email', label: 'Email', format: 'email'}, {key: 'relationship', label: 'Relationship'}]}, bindings: {record: {source: 'data-source', data_source: 'people', path: 'selected'}}},
            {id: 'timeline-slot', component: 'platform.ui.slot', props: {label: 'Timeline contributions', kind: 'timeline'}},
          ]},
        ],
      }, {
        id: 'people-cursor', component: 'platform.ui.cursor', props: {label: 'People pages', previous_action: 'previous', next_action: 'next', parameter: 'cursor'}, bindings: {next_cursor: {source: 'data-source', data_source: 'people', path: 'next_cursor'}},
      }],
    }],
  },
};
