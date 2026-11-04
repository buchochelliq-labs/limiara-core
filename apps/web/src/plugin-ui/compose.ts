import type {ExtensionContribution, HostProfile, UiNode} from './contracts';

export type ContributionCompositionOptions = {
  host?: HostProfile;
  permissions?: ReadonlySet<string>;
  enabledPlugins?: ReadonlySet<string>;
};

function cloneNode(node: UiNode): UiNode {
  return {
    ...node,
    props: node.props ? {...node.props} : undefined,
    bindings: node.bindings ? {...node.bindings} : undefined,
    conditions: node.conditions ? {...node.conditions} : undefined,
    children: node.children?.map(cloneNode),
  };
}

function visit(node: UiNode, callback: (candidate: UiNode) => void) {
  callback(node);
  node.children?.forEach(child => visit(child, callback));
}

function nodeIds(node: UiNode) {
  const ids = new Set<string>();
  visit(node, candidate => {
    if (ids.has(candidate.id)) throw new Error(`Duplicate metadata node: ${candidate.id}`);
    ids.add(candidate.id);
  });
  return ids;
}

function findNode(root: UiNode, id: string): UiNode | undefined {
  if (root.id === id) return root;
  for (const child of root.children ?? []) {
    const match = findNode(child, id);
    if (match) return match;
  }
  return undefined;
}

export function applicableInsideContributions(pageId: string, definitionVersion: number, contributions: readonly ExtensionContribution[], options: ContributionCompositionOptions = {}) {
  return contributions
    .filter(contribution => contribution.target_page === pageId && contribution.target_version === definitionVersion && contribution.operation === 'inside' && contribution.node)
    .filter(contribution => !options.enabledPlugins || options.enabledPlugins.has(contribution.owner_plugin))
    .filter(contribution => !options.host || !contribution.node?.hosts?.length || contribution.node.hosts.includes(options.host))
    .filter(contribution => !contribution.node?.permission || options.permissions?.has(contribution.node.permission))
    .slice()
    .sort((left, right) => (left.priority ?? 0) - (right.priority ?? 0) || left.owner_plugin.localeCompare(right.owner_plugin) || left.id.localeCompare(right.id));
}

export function composeInsideContributions(root: UiNode, pageId: string, definitionVersion: number, contributions: readonly ExtensionContribution[], options: ContributionCompositionOptions = {}) {
  const composed = cloneNode(root);
  const ids = nodeIds(composed);
  const applicable = applicableInsideContributions(pageId, definitionVersion, contributions, options);

  for (const contribution of applicable) {
    const target = findNode(composed, contribution.target_node);
    if (!target) throw new Error(`Missing contribution target: ${contribution.target_node}`);
    const added = cloneNode(contribution.node!);
    for (const id of nodeIds(added)) {
      if (ids.has(id)) throw new Error(`Duplicate contributed metadata node: ${id}`);
      ids.add(id);
    }
    target.children = [...(target.children ?? []), added];
  }
  return composed;
}
