import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import type {ResolvedPage, UiNode} from '../contracts';
import {MetadataPageRenderer} from './MetadataPageRenderer';
import {createDefaultHostRegistry} from './hostComponents';

const textNode = (id = 'content'): UiNode => ({id, component: 'platform.ui.text', props: {text: 'Representative content'}});

function fixtureNode(component: string): UiNode {
  const fixtures: Record<string, UiNode> = {
    'platform.ui.page': {id: 'subject', component, props: {title: 'Page'}, children: [textNode()]},
    'platform.ui.stack': {id: 'subject', component, props: {gap: 'medium'}, children: [textNode()]},
    'platform.ui.grid': {id: 'subject', component, props: {columns: 2}, children: [textNode()]},
    'platform.ui.split': {id: 'subject', component, children: [textNode('primary'), textNode('secondary')]},
    'platform.ui.section': {id: 'subject', component, props: {title: 'Section'}, children: [textNode()]},
    'platform.ui.card': {id: 'subject', component, props: {title: 'Card'}, children: [textNode()]},
    'platform.ui.tabs': {id: 'subject', component, props: {label: 'Tabs', tabs: ['First']}, children: [textNode()]},
    'platform.ui.accordion': {id: 'subject', component, props: {title: 'Details'}, children: [textNode()]},
    'platform.ui.text': textNode('subject'),
    'platform.ui.alert': {id: 'subject', component, props: {title: 'Notice', variant: 'info'}, children: [textNode()]},
    'platform.ui.form': {id: 'subject', component, props: {label: 'Form'}, children: [textNode()]},
    'platform.ui.wizard': {id: 'subject', component, props: {label: 'Steps'}, children: [textNode()]},
    'platform.ui.field.text': {id: 'subject', component, props: {name: 'name', label: 'Name'}},
    'platform.ui.field.select': {id: 'subject', component, props: {name: 'choice', label: 'Choice', options: [{value: 'one', label: 'One'}]}},
    'platform.ui.collection': {id: 'subject', component, props: {label: 'Items', items: [{id: 'one', title: 'One'}], presentation: 'table', select_action: 'sort', open_action: 'sort', sort_action: 'sort', identity_sort: 'name'}},
    'platform.ui.highlights': {id: 'subject', component, props: {label: 'Highlights', record: {id: 'one', title: 'One', description: 'First'}, fields: [], icon: 'tasks'}, children: [{id: 'highlight-actions', component: 'platform.ui.action-bar', props: {label: 'Actions'}, children: [{id: 'highlight-action', component: 'platform.ui.action', props: {label: 'Run'}}]}]},
    'platform.ui.path': {id: 'subject', component, props: {label: 'Stage', stages: ['First', 'Second'], current: 'Second'}},
    'platform.ui.timeline': {id: 'subject', component, props: {label: 'Activity', items: [{id: 'one', title: 'Created', values: {occurred_at: '2026-08-12T09:00:00Z', actor: 'Someone'}}]}},
    'platform.ui.filter-chips': {id: 'subject', component, props: {label: 'Active filters', chips: [{id: 'query', label: 'Search: One', action: 'search', parameter: 'query', value: ''}], clear_action: 'search'}},
    'platform.ui.collection-controls': {id: 'subject', component, props: {label: 'Controls'}, children: [{id: 'query', component: 'platform.ui.query-input', props: {label: 'Search', action: 'search'}}]},
    'platform.ui.query-input': {id: 'subject', component, props: {label: 'Search', action: 'search'}},
    'platform.ui.query-select': {id: 'subject', component, props: {label: 'Sort', action: 'sort', options: [{value: 'name', label: 'Name'}]}},
    'platform.ui.record-preview': {id: 'subject', component, props: {label: 'Preview'}, children: [textNode()]},
    'platform.ui.record-summary': {id: 'subject', component, props: {label: 'Summary', record: {id: 'one', title: 'One'}, fields: []}},
    'platform.ui.relationship-list': {id: 'subject', component, props: {label: 'Relationships', items: [{id: 'one', title: 'One'}]}},
    'platform.ui.cursor': {id: 'subject', component, props: {label: 'Pages', previous_action: 'previous', next_action: 'next', next_cursor: 'next'}},
    'platform.ui.state': {id: 'subject', component, props: {variant: 'archived', title: 'Archived'}},
    'platform.ui.table': {id: 'subject', component, props: {label: 'Rows', columns: [{key: 'name', label: 'Name'}], rows: [{id: 'one', name: 'One'}]}},
    'platform.ui.action-bar': {id: 'subject', component, props: {label: 'Actions'}, children: [{id: 'action', component: 'platform.ui.action', props: {label: 'Run'}}]},
    'platform.ui.action': {id: 'subject', component, props: {label: 'Run'}},
    'platform.ui.dialog': {id: 'subject', component, props: {title: 'Dialog', open: true}, children: [textNode()]},
    'platform.ui.slot': {id: 'subject', component, props: {label: 'Extension slot'}, children: [textNode()]},
    'platform.ui.fragment': {id: 'subject', component, props: {fragment_id: 'example.fragment'}, children: [textNode()]},
    'platform.ui.loading': {id: 'subject', component, props: {label: 'Loading'}},
    'platform.ui.empty': {id: 'subject', component, props: {title: 'Empty', description: 'Nothing here'}},
    'platform.ui.denied': {id: 'subject', component, props: {title: 'Unavailable'}},
    'platform.ui.error': {id: 'subject', component, props: {title: 'Error'}},
  };
  const fixture = fixtures[component];
  if (!fixture) throw new Error(`Missing conformance fixture for ${component}`);
  return fixture;
}

function page(root: UiNode): ResolvedPage {
  return {page_id: 'example.conformance.page', definition_version: 1, selected_variant: null, source_versions: {'example.conformance': '0.1.0'}, publication_versions: [], resource: null, data_sources: [], actions: [
    {id: 'search', kind: 'navigate', target: 'example', parameters: {query: {source: 'literal', value: ''}}},
    {id: 'sort', kind: 'navigate', target: 'example', parameters: {value: {source: 'literal', value: ''}}},
    {id: 'previous', kind: 'navigate', target: 'example', parameters: {cursor: {source: 'literal', value: ''}}},
    {id: 'next', kind: 'navigate', target: 'example', parameters: {cursor: {source: 'literal', value: ''}}},
  ], root, content_hash: 'conformance'};
}

describe('metadata host conformance gate', () => {
  it('renders every maintained component on each supported host and form factor', () => {
    const registry = createDefaultHostRegistry();
    for (const definition of registry.definitions()) {
      for (const host of definition.hosts) {
        for (const formFactor of ['wide', 'compact'] as const) {
          const diagnostic = vi.fn();
          const markup = renderToStaticMarkup(<MetadataPageRenderer formFactor={formFactor} host={host} onDiagnostic={diagnostic} page={page(fixtureNode(definition.id))} registry={registry} />);
          expect(markup, `${definition.id} on ${host}/${formFactor}`).not.toContain('meta-component-fallback');
          expect(diagnostic, `${definition.id} on ${host}/${formFactor}`).not.toHaveBeenCalled();
          expect(markup).toContain(`metadata-renderer--${formFactor}`);
        }
      }
    }
  });

  it('emits actionable host and form-factor context for invalid definitions', () => {
    const diagnostic = vi.fn();
    renderToStaticMarkup(<MetadataPageRenderer development formFactor="compact" host="portal" onDiagnostic={diagnostic} page={page({id: 'broken', component: 'example.unknown'})} />);
    expect(diagnostic).toHaveBeenCalledWith(expect.objectContaining({pageId: 'example.conformance.page', nodeId: 'broken', componentId: 'example.unknown', host: 'portal', formFactor: 'compact'}));
  });
});
