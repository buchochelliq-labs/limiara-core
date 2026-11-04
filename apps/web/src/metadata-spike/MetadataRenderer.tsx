import type {ReactNode} from 'react';

export type MetadataNode = {
  id: string;
  component: string;
  props?: Record<string, unknown>;
  children?: MetadataNode[];
};

export type ResolvedPage = {
  id: string;
  definitionVersion: number;
  root: MetadataNode;
};

type ComponentRenderer = (node: MetadataNode, children: ReactNode) => ReactNode;

function text(node: MetadataNode, property: string, fallback = '') {
  const value = node.props?.[property];
  return typeof value === 'string' ? value : fallback;
}

const components: Record<string, ComponentRenderer> = {
  page: (node, children) => <main data-metadata-node={node.id}>{children}</main>,
  form: (node, children) => <form data-metadata-node={node.id}>{children}</form>,
  section: (node, children) => <section data-metadata-node={node.id} aria-labelledby={`${node.id}-heading`}><h2 id={`${node.id}-heading`}>{text(node, 'title', 'Section')}</h2>{children}</section>,
  field: node => {
    const field = text(node, 'field', node.id);
    const label = text(node, 'label', field);
    return <div data-metadata-node={node.id}><label htmlFor={node.id}>{label}</label><input id={node.id} name={field} /></div>;
  },
  'action-bar': (node, children) => <footer data-metadata-node={node.id}>{children}<button type="submit">Save</button><button type="button">Cancel</button></footer>,
  action: node => <button data-metadata-node={node.id} type={text(node, 'action') === 'submit' ? 'submit' : 'button'}>{text(node, 'label', 'Continue')}</button>,
  'record-summary': node => <header data-metadata-node={node.id}><h1>{text(node, 'title', 'Person record')}</h1></header>,
  slot: (node, children) => <aside data-metadata-node={node.id} aria-label={text(node, 'label', 'Related information')}>{children}</aside>,
  card: (node, children) => <article data-metadata-node={node.id}><h2>{text(node, 'title', 'Card')}</h2>{children}</article>,
  search: node => <div data-metadata-node={node.id}><label htmlFor={node.id}>Search people</label><input id={node.id} type="search" name={text(node, 'queryParam', 'q')} /></div>,
  collection: node => <section data-metadata-node={node.id} aria-label={text(node, 'label', 'People')}><p>Collection results</p></section>,
};

function renderNode(node: MetadataNode): ReactNode {
  const renderer = components[node.component];
  if (!renderer) return <section key={node.id} data-metadata-node={node.id} role="alert">Unsupported component: {node.component}</section>;
  const children = node.children?.map(child => <MetadataNodeView key={child.id} node={child} />) ?? null;
  return renderer(node, children);
}

function MetadataNodeView({node}: {node: MetadataNode}) {
  return renderNode(node);
}

export function MetadataRenderer({page}: {page: ResolvedPage}) {
  return <div data-page-id={page.id} data-definition-version={page.definitionVersion}><MetadataNodeView node={page.root} /></div>;
}
