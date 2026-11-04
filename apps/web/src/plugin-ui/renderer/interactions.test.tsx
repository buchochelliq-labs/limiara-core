import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import type {CallerSafeResourceDescriptor, ConditionExpression, ResolvedPage} from '../contracts';
import {MetadataPageRenderer} from './MetadataPageRenderer';
import {buildActionParameters, buildSubmissionPayload, evaluateCondition} from './interactions';

const resource: CallerSafeResourceDescriptor = {
  id: 'platform.test.record', definition_version: 1, owner_plugin: 'platform.test', label: 'Test record',
  fields: [
    {api_name: 'name', label: 'Name', field_type: 'text', source: 'standard', readable: true, writable: true, required: true},
    {api_name: 'secret', label: 'Secret', field_type: 'text', source: 'standard', readable: true, writable: true, submit_when_hidden: false},
    {api_name: 'retained', label: 'Retained', field_type: 'text', source: 'standard', readable: true, writable: true, submit_when_hidden: true},
  ],
  relationships: [],
  operations: [{id: 'save', kind: 'update', concurrency: 'etag', hosts: ['internal-web']}],
};

const hiddenUnlessAdvanced: ConditionExpression = {op: 'equal', left: {source: 'field', field: 'mode'}, right: {source: 'literal', value: 'advanced'}};
const formPage: ResolvedPage = {
  page_id: 'platform.test.form', definition_version: 1, selected_variant: null,
  source_versions: {'platform.test': '1.0.0'}, publication_versions: [], resource: resource.id,
  data_sources: [],
  actions: [
    {id: 'save', kind: 'submit', target: 'platform.test.record.save'},
    {id: 'next', kind: 'navigate', target: 'form.next'},
  ],
  root: {id: 'form', component: 'platform.ui.form', props: {label: 'Test form'}, children: [
    {id: 'steps', component: 'platform.ui.wizard', props: {label: 'Test steps'}, children: [
      {id: 'first-step', component: 'platform.ui.section', props: {title: 'Basics'}, children: [
        {id: 'name', component: 'platform.ui.field.text', props: {field: 'name'}, bindings: {value: {source: 'field', field: 'name'}}},
        {id: 'secret', component: 'platform.ui.field.text', props: {field: 'secret'}, bindings: {value: {source: 'field', field: 'secret'}}, conditions: {visible: hiddenUnlessAdvanced}},
        {id: 'retained', component: 'platform.ui.field.text', props: {field: 'retained'}, bindings: {value: {source: 'field', field: 'retained'}}, conditions: {visible: hiddenUnlessAdvanced}},
      ]},
      {id: 'second-step', component: 'platform.ui.section', props: {title: 'Review'}},
    ]},
    {id: 'actions', component: 'platform.ui.action-bar', children: [
      {id: 'save-button', component: 'platform.ui.action', props: {action: 'save', label: 'Save', submit: true}},
    ]},
  ]},
  content_hash: 'interaction-fixture',
};

describe('metadata interactions', () => {
  it('matches the server condition truth table for the approved AST subset', () => {
    const context = {fields: {status: 'active'}, dataSources: {account: {tier: 'priority'}}, permissions: new Set(['records.edit'])};
    const cases: Array<[ConditionExpression, boolean]> = [
      [{op: 'equal', left: {source: 'field', field: 'status'}, right: {source: 'literal', value: 'active'}}, true],
      [{op: 'in', value: {source: 'data-source', data_source: 'account', path: 'tier'}, set: [{source: 'literal', value: 'priority'}]}, true],
      [{op: 'and', args: [{op: 'has-permission', permission: 'records.edit'}, {op: 'not', arg: {op: 'exists', value: {source: 'field', field: 'missing'}}}]}, true],
    ];
    for (const [condition, expected] of cases) expect(evaluateCondition(condition, context)).toBe(expected);
  });

  it('uses one definition for create, edit, view, and wizard modes', () => {
    for (const mode of ['create', 'edit', 'view', 'wizard'] as const) {
      const markup = renderToStaticMarkup(<MetadataPageRenderer page={formPage} resources={[resource]} host="internal-web" mode={mode} initialValues={{name: 'A record'}} />);
      expect(markup).toContain(`meta-form--${mode}`);
      expect(markup).toContain('Step 1 of 2');
      expect(markup).toContain('name="name"');
      expect(markup).not.toContain('name="secret"');
      if (mode === 'view') expect(markup).toContain('readOnly=""');
    }
  });

  it('omits hidden fields unless the resource contract explicitly retains them', () => {
    expect(buildSubmissionPayload(formPage, resource, {name: 'A', secret: 'drop', retained: 'keep', mode: 'basic'}, {fields: {mode: 'basic'}})).toEqual({name: 'A', retained: 'keep'});
  });

  it('allows controls to override only literal placeholder parameters declared by the action contract', () => {
    const action = {id: 'search', kind: 'navigate' as const, target: 'records', parameters: {q: {source: 'literal' as const, value: ''}, workspace: {source: 'context' as const, path: 'workspace'}}};
    expect(buildActionParameters(action, {fields: {}, context: {workspace: 'trusted-workspace'}}, {q: 'maya', workspace: 'attacker-workspace', undeclared: 'drop'})).toEqual({q: 'maya', workspace: 'trusted-workspace'});
  });

  it('renders registered actions without invoking adapters during render', () => {
    const execute = vi.fn();
    const markup = renderToStaticMarkup(<MetadataPageRenderer page={formPage} resources={[resource]} host="internal-web" mode="edit" interactionAdapter={{execute}} />);
    expect(markup).toContain('<span>Save</span>');
    expect(execute).not.toHaveBeenCalled();
  });
});
