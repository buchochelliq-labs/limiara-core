import {describe, expect, it} from 'vitest';
import type {ExtensionContribution, UiNode} from './contracts';
import {applicableInsideContributions, composeInsideContributions} from './compose';

const root: UiNode = {id: 'root', component: 'platform.ui.page', children: [{id: 'slot', component: 'platform.ui.slot'}]};
const contribution = (id: string, owner: string, priority: number, node: UiNode = {id: `${id}-node`, component: 'platform.ui.card'}): ExtensionContribution => ({contract_version: 1, id, owner_plugin: owner, target_page: 'page', target_version: 1, target_node: 'slot', operation: 'inside', priority, node});

describe('web contribution composition boundary', () => {
  it('orders applicable contributions deterministically', () => {
    const source = [contribution('z', 'plugin.b', 100), contribution('b', 'plugin.a', 100), contribution('a', 'plugin.a', 100), contribution('first', 'plugin.z', 10)];
    expect(applicableInsideContributions('page', 1, source).map(item => item.id)).toEqual(['first', 'a', 'b', 'z']);
    const composed = composeInsideContributions(root, 'page', 1, source);
    expect(composed.children?.[0]?.children?.map(item => item.id)).toEqual(['first-node', 'a-node', 'b-node', 'z-node']);
  });

  it('removes contributions when plugin, host, or permission makes them unavailable', () => {
    const source = [
      contribution('enabled', 'plugin.enabled', 1),
      contribution('wrong-host', 'plugin.enabled', 2, {id: 'wrong-host-node', component: 'platform.ui.card', hosts: ['portal']}),
      contribution('denied', 'plugin.enabled', 3, {id: 'denied-node', component: 'platform.ui.card', permission: 'secret.read'}),
      contribution('disabled', 'plugin.disabled', 4),
    ];
    const options = {host: 'internal-web' as const, permissions: new Set<string>(), enabledPlugins: new Set(['plugin.enabled'])};
    expect(applicableInsideContributions('page', 1, source, options).map(item => item.id)).toEqual(['enabled']);
    expect(composeInsideContributions(root, 'page', 1, source, options).children?.[0]?.children?.map(item => item.id)).toEqual(['enabled-node']);
  });

  it('fails unavailable targets and duplicate IDs with bounded diagnostics', () => {
    expect(() => composeInsideContributions(root, 'page', 1, [{...contribution('missing', 'plugin.a', 1), target_node: 'removed'}])).toThrow('Missing contribution target: removed');
    expect(() => composeInsideContributions(root, 'page', 1, [contribution('duplicate', 'plugin.a', 1, {id: 'root', component: 'platform.ui.card'})])).toThrow('Duplicate contributed metadata node: root');
  });
});
