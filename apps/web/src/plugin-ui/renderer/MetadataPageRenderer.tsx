import React, {useEffect, useRef, type ReactNode} from 'react';
import type {CallerSafeResourceDescriptor, HostProfile, ResolvedPage, UiNode} from '../contracts';
import {defaultHostRegistry} from './hostComponents';
import {MetadataInteractionBoundary, type MetadataFormMode, type MetadataInteractionAdapter, type MetadataInteractionRuntime, type MetadataValues} from './interactions';
import type {HostComponentRegistry, HostRenderContext} from './registry';
import './renderer.css';

export type ResolvedUiPage = ResolvedPage;

export type RenderDiagnostic = {
  category: 'unknown-component' | 'disabled-component' | 'unsupported-host' | 'invalid-props' | 'render-failure';
  pageId: string;
  nodeId: string;
  componentId: string;
  host: HostProfile;
  formFactor: 'wide' | 'compact';
  sourceDefinition?: string;
  failedBinding?: string;
  message: string;
};

type RendererProps = {
  page: ResolvedUiPage;
  host: HostProfile;
  locale?: string;
  messages?: Readonly<Record<string, string>>;
  registry?: HostComponentRegistry;
  development?: boolean;
  formFactor?: 'wide' | 'compact';
  announcement?: string;
  focusOnRender?: boolean;
  embedded?: boolean;
  embeddedHeadingOffset?: 0 | 2;
  onDiagnostic?: (diagnostic: RenderDiagnostic) => void;
  resources?: CallerSafeResourceDescriptor[];
  mode?: MetadataFormMode;
  initialValues?: MetadataValues;
  contextValues?: MetadataValues;
  dataSources?: Record<string, unknown>;
  permissions?: ReadonlySet<string>;
  interactionAdapter?: MetadataInteractionAdapter;
  recordId?: string;
  etag?: string;
};

type BoundaryProps = {
  fallback: ReactNode;
  onFailure: (error: Error) => void;
  children: ReactNode;
};

class NodeErrorBoundary extends React.Component<BoundaryProps, {failed: boolean}> {
  state = {failed: false};

  static getDerivedStateFromError() {
    return {failed: true};
  }

  componentDidCatch(error: Error) {
    this.props.onFailure(error);
  }

  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

function fallbackContent(diagnostic: RenderDiagnostic, development: boolean) {
  return <section className="meta-component-fallback" data-failure-category={diagnostic.category} data-metadata-node={diagnostic.nodeId} role={diagnostic.category === 'render-failure' ? 'alert' : 'status'}>
    <strong>This content is unavailable.</strong>
    <span>The rest of the page is still available.</span>
    {development && <details><summary>Component diagnostic</summary><dl><dt>Page</dt><dd>{diagnostic.pageId}</dd><dt>Node</dt><dd>{diagnostic.nodeId}</dd><dt>Component</dt><dd>{diagnostic.componentId}</dd>{diagnostic.sourceDefinition && <><dt>Source</dt><dd>{diagnostic.sourceDefinition}</dd></>}<dt>Reason</dt><dd>{diagnostic.message}</dd></dl></details>}
  </section>;
}

type NodeViewProps = {
  node: UiNode;
  page: ResolvedUiPage;
  context: HostRenderContext;
  registry: HostComponentRegistry;
  development: boolean;
  onDiagnostic?: (diagnostic: RenderDiagnostic) => void;
};

function emitFailure(props: NodeViewProps, category: RenderDiagnostic['category'], message: string, failedBinding?: string) {
  const diagnostic: RenderDiagnostic = {
    category,
    pageId: props.page.page_id,
    nodeId: props.node.id,
    componentId: props.node.component,
    host: props.context.host,
    formFactor: props.context.formFactor,
    sourceDefinition: Object.keys(props.page.source_versions).join(', ') || undefined,
    failedBinding,
    message,
  };
  props.onDiagnostic?.(diagnostic);
  return fallbackContent(diagnostic, props.development);
}

function MetadataNodeView(props: NodeViewProps): ReactNode {
  const {node, registry, context} = props;
  const nodeState = context.interaction?.state(node);
  if (nodeState && !nodeState.visible) return null;
  const definition = registry.get(node.component);
  if (!definition) return emitFailure(props, 'unknown-component', 'The component is not registered.');
  if (definition.enabled === false) return emitFailure(props, 'disabled-component', 'The component is disabled.');
  if (!definition.hosts.includes(context.host) || (node.hosts?.length && !node.hosts.includes(context.host))) {
    return emitFailure(props, 'unsupported-host', `The component is unavailable on host ${context.host}.`);
  }
  const boundProps = Object.fromEntries(Object.entries(node.bindings ?? {})
    .map(([property, binding]) => [property, context.interaction?.resolve(binding)])
    .filter(([, value]) => value !== undefined));
  const boundNode = {
    ...node,
    props: {
      ...node.props,
      ...boundProps,
    },
  };
  const validationErrors = registry.validate(boundNode, definition);
  if (validationErrors.length) {
    const failedProperty = validationErrors[0]?.property;
    return emitFailure(props, 'invalid-props', validationErrors.map(error => `${error.property}: ${error.message}`).join(' '), failedProperty && node.bindings?.[failedProperty] ? failedProperty : undefined);
  }
  const actionReferences = Object.entries(boundNode.props ?? {}).filter(([property, value]) => typeof value === 'string' && (property === 'action' || property.endsWith('_action')));
  const missingAction = actionReferences.find(([, actionId]) => !props.page.actions.some(action => action.id === actionId));
  if (missingAction) return emitFailure(props, 'invalid-props', `${missingAction[0]}: The declared action is unavailable.`);
  const children = node.children?.map(child => <MetadataNodeView {...props} key={child.id} node={child} />) ?? null;
  const effectiveNode = nodeState ? {
    ...boundNode,
    props: {
      ...boundNode.props,
      ...(definition.props.disabled ? {disabled: !nodeState.enabled || node.props?.disabled === true} : {}),
      ...(definition.props.read_only ? {read_only: nodeState.readOnly || node.props?.read_only === true} : {}),
      ...(definition.props.required ? {required: nodeState.required || node.props?.required === true} : {}),
    },
  } : boundNode;
  let rendered: ReactNode;
  try {
    rendered = definition.render(effectiveNode, children, context);
  } catch (reason) {
    return emitFailure(props, 'render-failure', reason instanceof Error ? reason.message : 'The component renderer failed.');
  }
  const boundaryDiagnostic: RenderDiagnostic = {
    category: 'render-failure',
    pageId: props.page.page_id,
    nodeId: node.id,
    componentId: node.component,
    host: context.host,
    formFactor: context.formFactor,
    sourceDefinition: Object.keys(props.page.source_versions).join(', ') || undefined,
    message: 'The component renderer failed.',
  };
  return <NodeErrorBoundary fallback={fallbackContent(boundaryDiagnostic, props.development)} onFailure={error => props.onDiagnostic?.({...boundaryDiagnostic, message: error.message})}>{rendered}</NodeErrorBoundary>;
}

export function MetadataPageRenderer({
  page,
  host,
  locale = 'en-GB',
  messages = {},
  registry = defaultHostRegistry,
  development = false,
  formFactor = 'wide',
  announcement,
  focusOnRender = false,
  embedded = false,
  embeddedHeadingOffset = 2,
  onDiagnostic,
  resources = [],
  mode = 'view',
  initialValues = {},
  contextValues = {},
  dataSources = {},
  permissions = new Set<string>(),
  interactionAdapter,
  recordId,
  etag,
}: RendererProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!focusOnRender) return;
    containerRef.current?.querySelector<HTMLElement>('h1, h2, h3, [role="heading"], main')?.focus();
  }, [focusOnRender, page.page_id, page.content_hash]);
  const resource = resources.find(candidate => candidate.id === page.resource);
  return <MetadataInteractionBoundary adapter={interactionAdapter} contextValues={contextValues} dataSources={dataSources} etag={etag} initialValues={initialValues} mode={mode} page={page} permissions={permissions} recordId={recordId} resource={resource}>
    {(interaction: MetadataInteractionRuntime) => {
      const context: HostRenderContext = {
        host,
        formFactor,
        embedded,
        embeddedHeadingOffset,
        locale,
        messages,
        resources: new Map(resources.map(candidate => [candidate.id, candidate])),
        interaction,
        resolveText: (node, property, fallback) => {
          const key = node.props?.[`${property}_key`];
          const value = node.props?.[property];
          if (typeof key === 'string') return messages[key] ?? (typeof value === 'string' ? value : fallback);
          return typeof value === 'string' ? value : fallback;
        },
      };
      return <div className={`metadata-renderer metadata-renderer--${formFactor}`} data-content-hash={page.content_hash} data-definition-version={page.definition_version} data-host={host} data-locale={locale} data-page-id={page.page_id} ref={containerRef}>
        {announcement && <p aria-atomic="true" aria-live="polite" className="sr-only" role="status">{announcement}</p>}
        <MetadataNodeView context={context} development={development} node={page.root} onDiagnostic={onDiagnostic} page={page} registry={registry} />
      </div>;
    }}
  </MetadataInteractionBoundary>;
}
