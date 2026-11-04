import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import type {CallerSafeResourceDescriptor, UiNode} from '../contracts';
import {MetadataPageRenderer, type ResolvedUiPage} from './MetadataPageRenderer';
import {collectionSelectionIndex, createDefaultHostRegistry} from './hostComponents';
import {collectionRendererDataSources, collectionRendererFixture, collectionRendererResources, rendererFixture, rendererMessages} from './fixtures';
import {HostComponentRegistry} from './registry';

function page(root: UiNode): ResolvedUiPage {
  return {page_id: 'platform.test.page', definition_version: 1, selected_variant: null, source_versions: {'platform.test': '1.0.0'}, publication_versions: [], resource: null, data_sources: [], actions: [
    {id: 'search', kind: 'navigate', target: 'example', parameters: {query: {source: 'literal', value: ''}}},
    {id: 'sort', kind: 'navigate', target: 'example', parameters: {value: {source: 'literal', value: ''}}},
    {id: 'previous', kind: 'navigate', target: 'example', parameters: {cursor: {source: 'literal', value: ''}}},
    {id: 'next', kind: 'navigate', target: 'example', parameters: {cursor: {source: 'literal', value: ''}}},
  ], root, content_hash: 'test-hash'};
}

describe('metadata host page renderer', () => {
  it('documents every registered component contract', () => {
    const definitions = createDefaultHostRegistry().definitions();
    expect(definitions.map(definition => definition.id)).toEqual([
      'platform.ui.accordion', 'platform.ui.action', 'platform.ui.action-bar', 'platform.ui.alert',
      'platform.ui.card', 'platform.ui.collection', 'platform.ui.collection-controls', 'platform.ui.cursor', 'platform.ui.denied', 'platform.ui.dialog',
      'platform.ui.empty', 'platform.ui.error', 'platform.ui.field.select', 'platform.ui.field.text', 'platform.ui.filter-chips', 'platform.ui.form',
      'platform.ui.fragment', 'platform.ui.grid', 'platform.ui.highlights', 'platform.ui.loading', 'platform.ui.page', 'platform.ui.path',
      'platform.ui.query-input', 'platform.ui.query-select', 'platform.ui.record-preview', 'platform.ui.record-summary',
      'platform.ui.relationship-list', 'platform.ui.section', 'platform.ui.slot', 'platform.ui.split', 'platform.ui.stack', 'platform.ui.state',
      'platform.ui.table', 'platform.ui.tabs', 'platform.ui.text', 'platform.ui.timeline', 'platform.ui.wizard',
    ]);
    for (const definition of definitions) {
      expect(definition.documentation.summary.length).toBeGreaterThan(10);
      expect(definition.documentation.keyboard.length).toBeGreaterThan(10);
      expect(definition.documentation.semantics.length).toBeGreaterThan(10);
      expect(definition.hosts.length).toBeGreaterThan(0);
    }
  });

  it('rejects malformed host registrations before pages can use them', () => {
    const registry = new HostComponentRegistry();
    expect(() => registry.register({
      id: 'platform.test.unsafe', hosts: ['internal-web'], props: {style: {kind: 'string'}},
      documentation: {summary: 'Unsafe registration fixture.', children: 'none', slots: [], keyboard: 'No keyboard behavior.', semantics: 'Test-only content.'},
      render: () => null,
    })).toThrow('forbidden visual property style');
  });

  it('renders one fixture at wide and compact widths without plugin styling', () => {
    const wide = renderToStaticMarkup(<MetadataPageRenderer page={rendererFixture} host="internal-web" messages={rendererMessages} formFactor="wide" />);
    const compact = renderToStaticMarkup(<MetadataPageRenderer page={rendererFixture} host="internal-web" messages={rendererMessages} formFactor="compact" />);
    expect(wide).toContain('metadata-renderer--wide');
    expect(compact).toContain('metadata-renderer--compact');
    expect(wide).toContain('People workspace');
    expect(compact).toContain('People workspace');
    expect(wide).toContain('<main class="meta-page" data-metadata-node="person-page" tabindex="-1">');
    expect(wide).not.toContain('style=');
    expect(compact).not.toContain('style=');
  });

  it('generates heading, label, description, error, and live-region relationships', () => {
    const markup = renderToStaticMarkup(<MetadataPageRenderer page={rendererFixture} host="internal-web" messages={rendererMessages} announcement="Person details loaded." />);
    expect(markup).toContain('<h1>People workspace</h1>');
    expect(markup).toContain('<h2 id="details-section-heading">Contact details</h2>');
    expect(markup).toContain('for="full-name"');
    expect(markup).toContain('id="full-name-description"');
    expect(markup).toContain('aria-describedby="email-error"');
    expect(markup).toContain('aria-invalid="true"');
    expect(markup).toContain('aria-live="polite"');
    expect(markup).toContain('Person details loaded.');
  });

  it('renders the collection development fixture at wide and compact form factors', () => {
    for (const formFactor of ['wide', 'compact'] as const) {
      const markup = renderToStaticMarkup(<MetadataPageRenderer dataSources={collectionRendererDataSources} formFactor={formFactor} host="internal-web" page={collectionRendererFixture} resources={collectionRendererResources} />);
      expect(markup).toContain('Collection and record workspace');
      expect(markup).toContain('role="search"');
      expect(markup).toContain('aria-current="true"');
      expect(markup).toContain('aria-label="Selected person"');
      expect(markup).toContain('<span>Next</span>');
      expect(markup).not.toContain('Timeline contributions');
      expect(markup).not.toContain('meta-component-fallback');
    }
  });

  it('preserves every caller-safe collection field in compact layouts', () => {
    const resources: CallerSafeResourceDescriptor[] = [{
      id: 'example.inventory.item', definition_version: 1, owner_plugin: 'example.inventory', label: 'Item', relationships: [], operations: [], fields: [
        {api_name: 'first', label: 'First field', field_type: 'text', source: 'standard', readable: true, writable: false},
        {api_name: 'second', label: 'Second field', field_type: 'text', source: 'standard', readable: true, writable: false},
        {api_name: 'third', label: 'Third field', field_type: 'text', source: 'standard', readable: true, writable: false},
      ],
    }];
    const fields = [{key: 'first', label: 'First'}, {key: 'second', label: 'Second'}, {key: 'third', label: 'Third'}];
    const fixture = page({id: 'compact-fields', component: 'platform.ui.stack', children: [
      {id: 'items', component: 'platform.ui.collection', props: {label: 'Items', resource: 'example.inventory.item', fields, items: [{id: 'item', title: 'Item', values: {first: 'Collection first', second: 'Collection second', third: 'Collection third', hidden: 'Collection hidden'}}], select_action: 'search', selection_parameter: 'query'}},
      {id: 'relationships', component: 'platform.ui.relationship-list', props: {label: 'Relationships', resource: 'example.inventory.item', fields, items: [{id: 'related', title: 'Related item', values: {first: 'Relationship first', second: 'Relationship second', third: 'Relationship third', hidden: 'Relationship hidden'}}]}},
    ]});

    const compact = renderToStaticMarkup(<MetadataPageRenderer formFactor="compact" host="internal-web" page={fixture} resources={resources} />);
    for (const value of ['Collection first', 'Collection second', 'Collection third', 'Relationship first', 'Relationship second', 'Relationship third']) {
      expect(compact).toContain(value);
    }
    expect(compact.match(/<dt>First field<\/dt>/g)).toHaveLength(2);
    expect(compact.match(/<dt>Second field<\/dt>/g)).toHaveLength(2);
    expect(compact.match(/<dt>Third field<\/dt>/g)).toHaveLength(2);
    expect(compact).not.toContain('Collection hidden');
    expect(compact).not.toContain('Relationship hidden');
  });

  it('renders compact tables as definition-list cards instead of a scrolling desktop table', () => {
    const fixture = page({id: 'table', component: 'platform.ui.table', props: {label: 'Results', columns: [{key: 'name', label: 'Name'}, {key: 'email', label: 'Email'}], rows: [{id: 'one', name: 'One', email: 'one@example.test'}]}});
    const wide = renderToStaticMarkup(<MetadataPageRenderer formFactor="wide" host="internal-web" page={fixture} />);
    const compact = renderToStaticMarkup(<MetadataPageRenderer formFactor="compact" host="internal-web" page={fixture} />);
    expect(wide).toContain('<table>');
    expect(compact).toContain('class="meta-table-cards"');
    expect(compact).toContain('<dt>Name</dt><dd>One</dd>');
    expect(compact).not.toContain('<table>');
  });

  it('offsets heading levels without nesting a second main in development previews', () => {
    const markup = renderToStaticMarkup(<main><h1>Showcase</h1><section><h2>Renderer example</h2><MetadataPageRenderer page={rendererFixture} host="internal-web" messages={rendererMessages} embedded /></section></main>);
    expect(markup.match(/<main/g)).toHaveLength(1);
    expect(markup).toContain('<h3 id="person-page-embedded-heading">People workspace</h3>');
    expect(markup).toContain('<h4 id="details-section-heading">Contact details</h4>');
    expect(markup).toContain('<h5 id="activity-card-heading">Recent activity</h5>');
  });

  it('contains unknown components and hides diagnostics from ordinary users', () => {
    const unknown = page({id: 'mystery', component: 'customer.arbitrary-widget'});
    const production = renderToStaticMarkup(<MetadataPageRenderer page={unknown} host="internal-web" />);
    const development = renderToStaticMarkup(<MetadataPageRenderer page={unknown} host="internal-web" development />);
    expect(production).toContain('This content is unavailable.');
    expect(production).not.toContain('customer.arbitrary-widget');
    expect(development).toContain('customer.arbitrary-widget');
    expect(development).toContain('platform.test.page');
  });

  it('contains renderer failures to the failed node', () => {
    const registry = new HostComponentRegistry().register({
      id: 'platform.test.broken',
      hosts: ['internal-web'],
      props: {},
      documentation: {summary: 'Deliberately broken test renderer.', children: 'none', slots: [], keyboard: 'No keyboard behavior.', semantics: 'Test-only status region.'},
      render: () => { throw new Error('fixture exploded'); },
    });
    const diagnostic = vi.fn();
    const markup = renderToStaticMarkup(<MetadataPageRenderer page={page({id: 'broken', component: 'platform.test.broken'})} host="internal-web" registry={registry} development onDiagnostic={diagnostic} />);
    expect(markup).toContain('This content is unavailable.');
    expect(markup).toContain('fixture exploded');
    expect(diagnostic).toHaveBeenCalledWith(expect.objectContaining({category: 'render-failure', nodeId: 'broken'}));
  });

  it('returns safe states for disabled and unsupported host components', () => {
    const disabledRegistry = new HostComponentRegistry().register({
      id: 'platform.test.disabled',
      enabled: false,
      hosts: ['internal-web'],
      props: {},
      documentation: {summary: 'Disabled test component contract.', children: 'none', slots: [], keyboard: 'No keyboard behavior.', semantics: 'Test-only status region.'},
      render: () => null,
    });
    const disabled = renderToStaticMarkup(<MetadataPageRenderer page={page({id: 'disabled', component: 'platform.test.disabled'})} host="internal-web" registry={disabledRegistry} />);
    const unsupported = renderToStaticMarkup(<MetadataPageRenderer page={page({id: 'table', component: 'platform.ui.table', props: {label: 'Results', columns: [], rows: []}})} host="public-form" />);
    expect(disabled).toContain('data-failure-category="disabled-component"');
    expect(unsupported).toContain('data-failure-category="unsupported-host"');
  });

  it('rejects raw colours, CSS, and undeclared props before rendering', () => {
    const unsafe = page({id: 'unsafe', component: 'platform.ui.text', props: {text: 'Hello', color: 'red', className: 'escape'}});
    const markup = renderToStaticMarkup(<MetadataPageRenderer page={unsafe} host="internal-web" development />);
    expect(markup).toContain('data-failure-category="invalid-props"');
    expect(markup).toContain('Raw visual styling is not permitted.');
    expect(markup).not.toContain('style="color');
  });

  it('rejects data-bound or unavailable action references before rendering controls', () => {
    const boundDiagnostic = vi.fn();
    const bound = page({id: 'query', component: 'platform.ui.query-input', props: {label: 'Search'}, bindings: {action: {source: 'data-source', data_source: 'config', path: 'action'}}});
    const boundMarkup = renderToStaticMarkup(<MetadataPageRenderer dataSources={{config: {action: 'search'}}} host="internal-web" onDiagnostic={boundDiagnostic} page={bound} />);
    expect(boundMarkup).toContain('data-failure-category="invalid-props"');
    expect(boundDiagnostic).toHaveBeenCalledWith(expect.objectContaining({failedBinding: 'action'}));

    const missing = page({id: 'query', component: 'platform.ui.query-input', props: {label: 'Search', action: 'missing-action'}});
    const missingMarkup = renderToStaticMarkup(<MetadataPageRenderer host="internal-web" page={missing} />);
    expect(missingMarkup).toContain('data-failure-category="invalid-props"');
    expect(missingMarkup).not.toContain('missing-action');
  });

  it('renders keyboard-operable tabs and native accordion semantics', () => {
    const fixture = page({
      id: 'tabs', component: 'platform.ui.tabs', props: {label: 'Record sections', tabs: ['Summary', 'History']}, children: [
        {id: 'summary', component: 'platform.ui.text', props: {text: 'Summary panel'}},
        {id: 'history', component: 'platform.ui.accordion', props: {title: 'History details'}, children: [{id: 'history-text', component: 'platform.ui.text', props: {text: 'History panel'}}]},
      ],
    });
    const markup = renderToStaticMarkup(<MetadataPageRenderer page={fixture} host="internal-web" />);
    expect(markup).toContain('role="tablist"');
    expect(markup).toContain('role="tab"');
    expect(markup).toContain('aria-selected="true"');
    expect(markup).toContain('<details');
    expect(markup).toContain('<summary>History details</summary>');
  });

  it('resolves caller-supplied data-source bindings into configured collection and record fields', () => {
    const resources: CallerSafeResourceDescriptor[] = [{id: 'platform.test.person', definition_version: 1, owner_plugin: 'platform.test', label: 'Person', fields: [{api_name: 'email', label: 'Email address', field_type: 'email', source: 'standard', readable: true, writable: false}], relationships: [], operations: []}];
    const fixture = page({
      id: 'workspace', component: 'platform.ui.split', children: [
        {id: 'results', component: 'platform.ui.collection', props: {label: 'People', resource: 'platform.test.person', fields: [{key: 'email', label: 'Email'}, {key: 'hidden', label: 'Forbidden field'}], presentation: 'auto'}, bindings: {items: {source: 'data-source', data_source: 'people', path: 'items'}, selected_id: {source: 'context', path: 'selected'}}},
        {id: 'preview', component: 'platform.ui.record-preview', props: {label: 'Person preview'}, children: [
          {id: 'summary', component: 'platform.ui.record-summary', props: {label: 'Person summary', resource: 'platform.test.person', fields: [{key: 'email', label: 'Email'}, {key: 'hidden', label: 'Forbidden field'}]}, bindings: {record: {source: 'data-source', data_source: 'people', path: 'selected'}}},
        ]},
      ],
    });
    const dataSources = {people: {items: [{id: 'maya', title: 'Maya Patel', description: 'Northstar', values: {email: 'maya@example.test', hidden: 'do not render'}}], selected: {id: 'maya', title: 'Maya Patel', values: {email: 'maya@example.test', hidden: 'do not render'}}}};
    const markup = renderToStaticMarkup(<MetadataPageRenderer contextValues={{selected: 'maya'}} dataSources={dataSources} host="internal-web" page={fixture} resources={resources} />);
    expect(markup).toContain('Maya Patel');
    expect(markup).toContain('maya@example.test');
    expect(markup).not.toContain('do not render');
    expect(markup).not.toContain('Forbidden field');
    expect(markup).toContain('aria-label="Person preview"');
  });

  it('links member references to user records and leaves former members and hostile ids inert', () => {
    const resources: CallerSafeResourceDescriptor[] = [{id: 'platform.test.person', definition_version: 1, owner_plugin: 'platform.test', label: 'Person', fields: [
      {api_name: 'created_by', label: 'Created by', field_type: 'user', source: 'system', readable: true, writable: false},
      {api_name: 'updated_by', label: 'Last modified by', field_type: 'user', source: 'system', readable: true, writable: false},
    ], relationships: [], operations: []}];
    const fixture = page({id: 'summary', component: 'platform.ui.record-summary', props: {label: 'Record metadata', resource: 'platform.test.person', fields: [{key: 'created_by', label: 'Created by'}, {key: 'updated_by', label: 'Last modified by'}]}, bindings: {record: {source: 'data-source', data_source: 'person', path: 'record'}}});
    const record = {id: 'maya', title: 'Maya Patel', values: {created_by: {id: 'u/../1"><script>', name: 'Grace <b>Hopper</b>'}, updated_by: {id: 'u2', name: null}}};
    const markup = renderToStaticMarkup(<MetadataPageRenderer dataSources={{person: {record}}} host="internal-web" page={fixture} resources={resources} />);
    expect(markup).toContain('<a href="/users/u%2F..%2F1%22%3E%3Cscript%3E">Grace &lt;b&gt;Hopper&lt;/b&gt;</a>');
    expect(markup).toContain('<dd>Former member</dd>');
    expect(markup).not.toContain('<script>');
  });

  it('contains malformed data-source shapes and undeclared binding targets without exposing their values', () => {
    const malformedDiagnostic = vi.fn();
    const malformed = page({id: 'results', component: 'platform.ui.collection', props: {label: 'People'}, bindings: {items: {source: 'data-source', data_source: 'people', path: 'items'}}});
    const malformedMarkup = renderToStaticMarkup(<MetadataPageRenderer dataSources={{people: {items: [{id: 'maya', title: 42}]}}} development host="internal-web" onDiagnostic={malformedDiagnostic} page={malformed} />);
    expect(malformedMarkup).toContain('data-failure-category="invalid-props"');
    expect(malformedDiagnostic).toHaveBeenCalledWith(expect.objectContaining({failedBinding: 'items'}));

    const unsafeDiagnostic = vi.fn();
    const unsafe = page({id: 'results', component: 'platform.ui.collection', props: {label: 'People', items: []}, bindings: {className: {source: 'data-source', data_source: 'people', path: 'secret'}}});
    const unsafeMarkup = renderToStaticMarkup(<MetadataPageRenderer dataSources={{people: {secret: 'sensitive-value'}}} development host="internal-web" onDiagnostic={unsafeDiagnostic} page={unsafe} />);
    expect(unsafeMarkup).not.toContain('sensitive-value');
    expect(unsafeDiagnostic).toHaveBeenCalledWith(expect.objectContaining({failedBinding: 'className'}));
  });

  it('renders collection controls, cursor, relationship, state, and empty-slot semantics', () => {
    const fixture = page({id: 'stack', component: 'platform.ui.stack', children: [
      {id: 'controls', component: 'platform.ui.collection-controls', props: {label: 'People controls'}, children: [
        {id: 'search', component: 'platform.ui.query-input', props: {label: 'Search people', action: 'search', submit_label: 'Search'}},
        {id: 'sort', component: 'platform.ui.query-select', props: {label: 'Sort people', action: 'sort', options: [{value: 'name', label: 'Name'}]}},
      ]},
      {id: 'relationships', component: 'platform.ui.relationship-list', props: {label: 'Related people', items: [{id: 'one', title: 'One person'}]}},
      {id: 'pages', component: 'platform.ui.cursor', props: {label: 'People pages', previous_action: 'previous', next_action: 'next', next_cursor: 'opaque-next'}},
      {id: 'archived', component: 'platform.ui.state', props: {variant: 'archived', title: 'Archived person', description: 'Read only'}},
      {id: 'denied', component: 'platform.ui.state', props: {variant: 'denied', title: 'Secret customer', description: 'Secret details'}},
      {id: 'slot', component: 'platform.ui.slot', props: {label: 'Timeline contributions', kind: 'timeline'}},
    ]});
    const markup = renderToStaticMarkup(<MetadataPageRenderer host="internal-web" page={fixture} />);
    expect(markup).toContain('role="search"');
    expect(markup).toContain('type="search"');
    expect(markup).toContain('aria-label="People pages"');
    expect(markup).toContain('Related people');
    expect(markup).toContain('Archived person');
    expect(markup).toContain('Content unavailable');
    expect(markup).not.toContain('Secret customer');
    expect(markup).not.toContain('Timeline contributions');
  });

  it('defines deterministic collection focus movement without changing selection', () => {
    expect(collectionSelectionIndex(1, 'ArrowDown', 3)).toBe(2);
    expect(collectionSelectionIndex(2, 'ArrowDown', 3)).toBe(0);
    expect(collectionSelectionIndex(0, 'ArrowUp', 3)).toBe(2);
    expect(collectionSelectionIndex(1, 'Home', 3)).toBe(0);
    expect(collectionSelectionIndex(1, 'End', 3)).toBe(2);
    expect(collectionSelectionIndex(1, 'Enter', 3)).toBe(1);
  });
});
