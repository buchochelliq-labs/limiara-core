import React, {useEffect, useRef, useState, type KeyboardEvent, type ReactNode} from 'react';
import {Alert, Button, EmptyState, Select, Skeleton, TextField} from '../../components/ui';
import type {CollectionItem, DisplayField, FilterChip, HostProfile, RecordSummary, UiNode} from '../contracts';
import {ObjectTile} from '../../shell/ObjectIcon';
import {MetadataFormShell} from './interactions';
import {
  HostComponentRegistry,
  booleanProp,
  isRecord,
  numberProp,
  stringProp,
  type ComponentDocumentation,
  type HostComponentDefinition,
  type HostRenderContext,
  type PropRule,
} from './registry';
import {formatUserReference, userRecordRoute, userRecordTarget, userReference, type UserReference} from '../standard-fields/model';

const allHosts: readonly HostProfile[] = ['internal-web', 'desktop', 'portal', 'public-form'];
const internalHosts: readonly HostProfile[] = ['internal-web', 'desktop', 'portal'];

type DefinitionOptions = {
  id: string;
  props?: Readonly<Record<string, PropRule>>;
  hosts?: readonly HostProfile[];
  children?: ComponentDocumentation['children'];
  slots?: readonly string[];
  keyboard?: string;
  semantics: string;
  summary: string;
  render: HostComponentDefinition['render'];
};

function define(options: DefinitionOptions): HostComponentDefinition {
  return {
    id: options.id,
    hosts: options.hosts ?? allHosts,
    props: options.props ?? {},
    documentation: {
      summary: options.summary,
      children: options.children ?? 'optional',
      slots: options.slots ?? [],
      keyboard: options.keyboard ?? 'Follows document reading order; no custom keyboard interaction.',
      semantics: options.semantics,
    },
    render: options.render,
  };
}

const labelProps = {
  title: {kind: 'string'} as const,
  label: {kind: 'string'} as const,
  description: {kind: 'string'} as const,
};

function text(context: HostRenderContext, node: UiNode, property: string, fallback: string) {
  return context.resolveText(node, property, fallback);
}

function hasText(node: UiNode, property: string) {
  return node.props?.[property] !== undefined || node.props?.[`${property}_key`] !== undefined;
}

function PageFrame({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const Heading = context.embedded && context.embeddedHeadingOffset === 2 ? 'h3' : 'h1';
  const icon = stringProp(node, 'icon');
  const header = <header className={`meta-page__header${icon ? ' meta-page__header--iconed' : ''}`}>{icon && <ObjectTile item={{icon}} size="medium" />}<div><Heading id={context.embedded ? `${node.id}-embedded-heading` : undefined}>{text(context, node, 'title', 'Page')}</Heading>{hasText(node, 'description') && <p>{text(context, node, 'description', '')}</p>}</div></header>;
  return context.embedded
    ? <section aria-labelledby={`${node.id}-embedded-heading`} className="meta-page" data-metadata-node={node.id} tabIndex={-1}>{header}{children}</section>
    : <main className="meta-page" data-metadata-node={node.id} tabIndex={-1}>{header}{children}</main>;
}

function SectionFrame({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const Heading = context.embedded && context.embeddedHeadingOffset === 2 ? 'h4' : 'h2';
  return <section aria-labelledby={`${node.id}-heading`} className="meta-section" data-metadata-node={node.id}><header><Heading id={`${node.id}-heading`}>{text(context, node, 'title', 'Section')}</Heading>{hasText(node, 'description') && <p>{text(context, node, 'description', '')}</p>}</header>{children}</section>;
}

function CardFrame({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const Heading = context.embedded && context.embeddedHeadingOffset === 2 ? 'h5' : 'h3';
  return <article aria-labelledby={`${node.id}-heading`} className="meta-card" data-metadata-node={node.id}><Heading id={`${node.id}-heading`}>{text(context, node, 'title', 'Card')}</Heading>{hasText(node, 'description') && <p>{text(context, node, 'description', '')}</p>}{children}</article>;
}

function Wizard({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const steps = React.Children.toArray(children);
  const active = Math.min(Math.max(context.interaction?.step ?? 0, 0), Math.max(steps.length - 1, 0));
  const panelRef = useRef<HTMLDivElement>(null);
  const previous = useRef(active);
  useEffect(() => {
    if (previous.current !== active) panelRef.current?.focus({preventScroll: true});
    previous.current = active;
  }, [active]);
  const progress = (context.messages['platform.ui.wizard.progress'] ?? (context.locale.startsWith('fr') ? '\u00c9tape {current} sur {total}' : 'Step {current} of {total}'))
    .replace('{current}', String(active + 1))
    .replace('{total}', String(steps.length));
  return <section aria-label={text(context, node, 'label', 'Form steps')} className="meta-wizard" data-metadata-node={node.id}>
    <p aria-atomic="true" aria-live="polite" id={`${node.id}-progress`}>{progress}</p>
    <div aria-labelledby={`${node.id}-progress`} className="meta-wizard__step" ref={panelRef} role="group" tabIndex={-1}>{steps[active]}</div>
  </section>;
}

function Tabs({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const panels = React.Children.toArray(children);
  const labels = Array.isArray(node.props?.tabs) ? node.props.tabs.filter((item): item is string => typeof item === 'string') : [];
  const [active, setActive] = useState(0);
  const selectRelative = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? panels.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + panels.length) % panels.length;
    setActive(next);
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  };
  return <section className="meta-tabs" aria-label={text(context, node, 'label', 'Sections')} data-metadata-node={node.id}>
    <div className="meta-tabs__list" role="tablist">
      {panels.map((_, index) => <button aria-controls={`${node.id}-panel-${index}`} aria-selected={active === index} id={`${node.id}-tab-${index}`} key={index} onClick={() => setActive(index)} onKeyDown={event => selectRelative(event, index)} role="tab" tabIndex={active === index ? 0 : -1} type="button">{labels[index] ? context.messages[labels[index]] ?? labels[index] : `Section ${index + 1}`}</button>)}
    </div>
    {panels.map((panel, index) => <div aria-labelledby={`${node.id}-tab-${index}`} hidden={active !== index} id={`${node.id}-panel-${index}`} key={index} role="tabpanel" tabIndex={0}>{panel}</div>)}
  </section>;
}

function tableValue(row: Record<string, unknown>, key: string) {
  const value = row[key];
  return typeof value === 'string' || typeof value === 'number' ? String(value) : '';
}

function displayValue(value: unknown, format: DisplayField['format'], locale: string) {
  if (value === undefined || value === null || value === '') return 'Not provided';
  if (format === 'user') return formatUserReference(userReference(value)) || 'Not provided';
  if (format === 'boolean' && typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (format === 'number' && typeof value === 'number') return new Intl.NumberFormat(locale).format(value);
  if ((format === 'date' || format === 'date-time') && (typeof value === 'string' || typeof value === 'number')) {
    const parsed = new Date(value);
    if (!Number.isNaN(parsed.valueOf())) return new Intl.DateTimeFormat(locale, format === 'date-time' ? {dateStyle: 'medium', timeStyle: 'short'} : {dateStyle: 'medium'}).format(parsed);
  }
  if (Array.isArray(value)) return value.filter(item => typeof item === 'string' || typeof item === 'number').join(', ') || 'Not provided';
  return typeof value === 'string' || typeof value === 'number' ? String(value) : 'Not provided';
}

function defaultDisplayFormat(fieldType: string): DisplayField['format'] {
  return ['email', 'phone', 'number', 'date', 'date-time', 'boolean', 'money', 'user'].includes(fieldType) ? fieldType as DisplayField['format'] : 'text';
}

function displayFields(node: UiNode, context: HostRenderContext) {
  const resource = context.resources.get(stringProp(node, 'resource'));
  if (!resource) return [];
  const requested = Array.isArray(node.props?.fields) ? node.props.fields.filter(isRecord) as DisplayField[] : [];
  return requested.flatMap(field => {
    const descriptor = resource.fields.find(candidate => candidate.api_name === field.key && candidate.readable !== false);
    return descriptor ? [{key: field.key, label: descriptor.label, format: field.format ?? defaultDisplayFormat(descriptor.field_type), sort: field.sort}] : [];
  });
}

function collectionItems(node: UiNode) {
  return Array.isArray(node.props?.items) ? node.props.items.filter(isRecord) as CollectionItem[] : [];
}

function fieldValue(context: HostRenderContext, field: DisplayField, item: CollectionItem | RecordSummary, links: boolean) {
  const value = item.values?.[field.key];
  const formatted = displayValue(value, field.format, context.locale);
  if (links && field.format === 'email' && typeof value === 'string') return <a href={`mailto:${value}`}>{formatted}</a>;
  if (links && field.format === 'phone' && typeof value === 'string') return <a href={`tel:${value.replace(/[^+\d]/g, '')}`}>{formatted}</a>;
  const user = field.format === 'user' ? userReference(value) : null;
  // A member links to their user record; a former member stays plain text.
  if (links && user?.name) return <UserLink context={context} user={user}>{formatted}</UserLink>;
  return formatted;
}

function UserLink({context, user, children}: {context: HostRenderContext; user: UserReference; children: React.ReactNode}) {
  const href = userRecordRoute(user.id);
  return <a href={href} onClick={event => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || !context.interaction) return;
    event.preventDefault();
    context.interaction.open(userRecordTarget, {id: user.id});
  }}>{children}</a>;
}

function ItemValues({context, fields, item, links = true}: {context: HostRenderContext; fields: DisplayField[]; item: CollectionItem | RecordSummary; links?: boolean}) {
  if (!fields.length) return null;
  return <dl className="meta-display-fields">{fields.map(field => <div key={field.key}><dt>{context.messages[field.label] ?? field.label}</dt><dd>{fieldValue(context, field, item, links)}</dd></div>)}</dl>;
}

export function collectionSelectionIndex(current: number, key: string, count: number) {
  if (count <= 0) return -1;
  if (key === 'Home') return 0;
  if (key === 'End') return count - 1;
  if (key === 'ArrowDown' || key === 'ArrowRight') return (current + 1) % count;
  if (key === 'ArrowUp' || key === 'ArrowLeft') return (current - 1 + count) % count;
  return current;
}

function Collection({node, context}: {node: UiNode; context: HostRenderContext}) {
  const items = collectionItems(node);
  const fields = displayFields(node, context);
  const action = stringProp(node, 'select_action');
  const parameter = stringProp(node, 'selection_parameter', 'id');
  const selected = stringProp(node, 'selected_id');
  const requested = stringProp(node, 'presentation', 'auto');
  // A table needs width; compact hosts fall back to stacked rows.
  const presentation = requested === 'table' && context.formFactor === 'compact' ? 'auto' : requested;
  const openAction = stringProp(node, 'open_action');
  const openParameter = stringProp(node, 'open_parameter', 'id');
  const openLabel = text(context, node, 'open_label', 'Open');
  const sortAction = stringProp(node, 'sort_action');
  const sortParameter = stringProp(node, 'sort_parameter', 'sort');
  const currentSort = stringProp(node, 'sort');
  const sortDirection = stringProp(node, 'sort_direction') === 'descending' ? 'descending' : 'ascending';
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const next = collectionSelectionIndex(index, event.key, items.length);
    event.currentTarget.closest('[data-metadata-node]')?.querySelectorAll<HTMLButtonElement>('.meta-collection__select')[next]?.focus();
  };
  const identity = (item: CollectionItem) => <span className="meta-collection__identity"><strong>{item.title}</strong>{item.description && <span>{item.description}</span>}{item.archived && <span className="meta-collection__status">Archived</span>}</span>;
  const open = (item: CollectionItem) => openAction ? <Button className="meta-collection__open" onClick={() => void context.interaction?.invoke(openAction, {[openParameter]: item.id})} size="small" type="button" variant="quiet">{openLabel}<span className="sr-only">: {item.title}</span></Button> : null;
  const label = text(context, node, 'label', 'Items');
  if (items.length === 0) {
    return <section aria-label={label} className={`meta-collection meta-collection--${presentation}`} data-metadata-node={node.id}><EmptyState title={text(context, node, 'empty_title', 'Nothing here yet')} description={text(context, node, 'empty_description', 'There are no items to show.')} /></section>;
  }
  if (presentation === 'table') {
    const header = (key: string, columnLabel: string, sort?: string) => {
      const active = Boolean(sort) && sort === currentSort;
      const content = context.messages[columnLabel] ?? columnLabel;
      return <th aria-sort={active ? sortDirection : undefined} key={key} scope="col">{sortAction && sort
        ? <button aria-pressed={active} className="meta-collection__sort" onClick={() => void context.interaction?.invoke(sortAction, {[sortParameter]: sort})} type="button">{content}</button>
        : content}</th>;
    };
    return <section aria-label={label} className="meta-collection meta-collection--table" data-metadata-node={node.id}>
      <div className="meta-collection__scroll"><table>
        <caption className="sr-only">{label}</caption>
        <thead><tr>{header('identity', text(context, node, 'identity_label', 'Name'), stringProp(node, 'identity_sort'))}{fields.map(field => header(field.key, field.label, field.sort))}{openAction && <th scope="col"><span className="sr-only">{openLabel}</span></th>}</tr></thead>
        <tbody>{items.map((item, index) => <tr aria-selected={selected === item.id ? true : undefined} data-archived={item.archived || undefined} key={item.id}>
          <th scope="row">{action
            ? <button aria-current={selected === item.id ? 'true' : undefined} className="meta-collection__select" onClick={() => void context.interaction?.invoke(action, {[parameter]: item.id})} onKeyDown={event => onKeyDown(event, index)} type="button">{identity(item)}</button>
            : identity(item)}</th>
          {fields.map(field => <td key={field.key}>{fieldValue(context, field, item, true)}</td>)}
          {openAction && <td className="meta-collection__open-cell">{open(item)}</td>}
        </tr>)}</tbody>
      </table></div>
    </section>;
  }
  return <section aria-label={label} className={`meta-collection meta-collection--${presentation}`} data-metadata-node={node.id}>
    <ul>{items.map((item, index) => <li data-archived={item.archived || undefined} key={item.id}>
      {action
        ? <button aria-current={selected === item.id ? 'true' : undefined} className="meta-collection__select" onClick={() => void context.interaction?.invoke(action, {[parameter]: item.id})} onKeyDown={event => onKeyDown(event, index)} type="button">{identity(item)}<ItemValues context={context} fields={fields} item={item} links={false} /></button>
        : <article aria-label={item.title}>{identity(item)}<ItemValues context={context} fields={fields} item={item} /></article>}
      {open(item)}
    </li>)}</ul>
  </section>;
}

function Highlights({node, children, context}: {node: UiNode; children: ReactNode; context: HostRenderContext}) {
  const record = node.props?.record as RecordSummary;
  const fields = displayFields(node, context);
  const icon = stringProp(node, 'icon');
  const actions = React.Children.toArray(children);
  return <section aria-label={text(context, node, 'label', 'Record highlights')} className="meta-highlights" data-archived={record.archived || undefined} data-metadata-node={node.id}>
    <div className="meta-highlights__header">
      {icon && <ObjectTile item={{icon}} size="large" />}
      <div className="meta-highlights__identity"><strong>{record.title}</strong>{record.description && <span>{record.description}</span>}{record.archived && <span className="meta-record-summary__status">Archived</span>}</div>
      {actions.length > 0 && <div className="meta-highlights__actions">{actions}</div>}
    </div>
    {fields.length > 0 && <ItemValues context={context} fields={fields} item={record} />}
  </section>;
}

function Path({node, context}: {node: UiNode; context: HostRenderContext}) {
  const stages = Array.isArray(node.props?.stages) ? node.props.stages.filter((item): item is string => typeof item === 'string') : [];
  const current = stringProp(node, 'current');
  const currentIndex = stages.indexOf(current);
  return <nav aria-label={text(context, node, 'label', 'Stage')} className="meta-path" data-metadata-node={node.id}>
    <ol>{stages.map((stage, index) => <li aria-current={index === currentIndex ? 'step' : undefined} data-complete={currentIndex > index || undefined} key={stage}>{context.messages[stage] ?? stage}</li>)}</ol>
    {currentIndex === -1 && <p className="meta-path__note">{text(context, node, 'empty_description', 'No stage recorded.')}</p>}
  </nav>;
}

function Timeline({node, context}: {node: UiNode; context: HostRenderContext}) {
  const items = collectionItems(node);
  const label = text(context, node, 'label', 'Activity');
  if (items.length === 0) return <section aria-label={label} className="meta-timeline" data-metadata-node={node.id}><EmptyState title={text(context, node, 'empty_title', 'No activity yet')} description={text(context, node, 'empty_description', 'Changes to this record will appear here.')} /></section>;
  return <section aria-label={label} className="meta-timeline" data-metadata-node={node.id}>
    <ol>{items.map(item => {
      const when = item.values?.occurred_at;
      const stamp = typeof when === 'string' ? when : undefined;
      const actor = item.values?.actor;
      return <li key={item.id}><div className="meta-timeline__marker" aria-hidden="true" /><div className="meta-timeline__body"><strong>{item.title}</strong>{item.description && <span>{item.description}</span>}<span className="meta-timeline__meta">{typeof actor === 'string' && actor ? `${actor} · ` : ''}{stamp ? <time dateTime={stamp}>{displayValue(stamp, 'date-time', context.locale)}</time> : null}</span></div></li>;
    })}</ol>
  </section>;
}

function FilterChips({node, context}: {node: UiNode; context: HostRenderContext}) {
  const chips = (Array.isArray(node.props?.chips) ? node.props.chips.filter(isRecord) : []) as FilterChip[];
  const clearAction = stringProp(node, 'clear_action');
  if (chips.length === 0) return null;
  return <div aria-label={text(context, node, 'label', 'Active filters')} className="meta-filter-chips" data-metadata-node={node.id} role="group">
    {chips.map(chip => <span className="meta-filter-chip" key={chip.id}>{context.messages[chip.label] ?? chip.label}<button aria-label={`Remove ${context.messages[chip.label] ?? chip.label}`} onClick={() => void context.interaction?.invoke(chip.action, chip.parameter ? {[chip.parameter]: chip.value ?? ''} : {})} type="button">×</button></span>)}
    {clearAction && <Button onClick={() => void context.interaction?.invoke(clearAction)} size="small" type="button" variant="quiet">{text(context, node, 'clear_label', 'Clear all')}</Button>}
  </div>;
}

function QueryInput({node, context}: {node: UiNode; context: HostRenderContext}) {
  const boundValue = stringProp(node, 'value');
  const [value, setValue] = useState(boundValue);
  useEffect(() => setValue(boundValue), [boundValue]);
  const run = () => void context.interaction?.invoke(stringProp(node, 'action'), {[stringProp(node, 'parameter', 'query')]: value});
  return <div className="meta-query-control" data-metadata-node={node.id}><TextField id={node.id} label={text(context, node, 'label', 'Search')} onChange={event => setValue(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); run(); } }} placeholder={text(context, node, 'placeholder', '')} type="search" value={value} /><Button onClick={run} size="small" type="button" variant="secondary">{text(context, node, 'submit_label', 'Apply')}</Button></div>;
}

function QuerySelect({node, context}: {node: UiNode; context: HostRenderContext}) {
  const options = node.props?.options as Array<{value: string; label: string}>;
  const action = stringProp(node, 'action');
  const parameter = stringProp(node, 'parameter', 'value');
  return <Select data-metadata-node={node.id} id={node.id} label={text(context, node, 'label', 'Filter')} onChange={event => void context.interaction?.invoke(action, {[parameter]: event.target.value})} value={stringProp(node, 'value')}>{options.map(option => <option key={option.value} value={option.value}>{context.messages[option.label] ?? option.label}</option>)}</Select>;
}

function RecordSummaryView({node, context}: {node: UiNode; context: HostRenderContext}) {
  const record = node.props?.record as RecordSummary;
  const showIdentity = node.props?.show_identity !== false;
  return <section aria-label={text(context, node, 'label', 'Record summary')} className="meta-record-summary" data-archived={record.archived || undefined} data-metadata-node={node.id}>{showIdentity && <div className="meta-record-summary__identity"><strong>{record.title}</strong>{record.description && <span>{record.description}</span>}{record.archived && <span className="meta-record-summary__status">Archived</span>}</div>}<ItemValues context={context} fields={displayFields(node, context)} item={record} /></section>;
}

export function createDefaultHostRegistry() {
  const registry = new HostComponentRegistry();
  const definitions: HostComponentDefinition[] = [
    define({id: 'platform.ui.page', summary: 'Top-level page landmark and heading container.', semantics: 'A main landmark with a level-one heading, or a labelled section in embedded development previews; an optional object tile is decorative.', children: 'required', props: {...labelProps, title: {kind: 'string', required: true}, icon: {kind: 'string'}}, render: (node, children, context) => <PageFrame node={node} context={context}>{children}</PageFrame>}),
    define({id: 'platform.ui.stack', summary: 'Vertical responsive content stack.', semantics: 'Neutral layout grouping; preserves child semantics.', children: 'required', props: {gap: {kind: 'string', values: ['small', 'medium', 'large']}}, render: (node, children) => <div className={`meta-stack meta-stack--${stringProp(node, 'gap', 'medium')}`} data-metadata-node={node.id}>{children}</div>}),
    define({id: 'platform.ui.grid', summary: 'Responsive equal-width grid.', semantics: 'Neutral layout grouping that collapses to one column.', children: 'required', props: {columns: {kind: 'number'}}, render: (node, children) => <div className={`meta-grid meta-grid--${Math.min(4, Math.max(1, numberProp(node, 'columns', 2)))}`} data-metadata-node={node.id}>{children}</div>}),
    define({id: 'platform.ui.split', summary: 'Responsive primary/sidebar split.', semantics: 'Two-region neutral layout; source order remains reading order.', children: 'required', slots: ['primary', 'secondary'], render: (node, children) => <div className="meta-split" data-metadata-node={node.id}>{children}</div>}),
    define({id: 'platform.ui.section', summary: 'Labelled page section.', semantics: 'A section landmark with a level-two heading.', children: 'optional', props: {...labelProps, title: {kind: 'string', required: true}}, render: (node, children, context) => <SectionFrame node={node} context={context}>{children}</SectionFrame>}),
    define({id: 'platform.ui.card', summary: 'Raised content group with optional heading.', semantics: 'An article with a level-three heading.', children: 'optional', props: labelProps, render: (node, children, context) => <CardFrame node={node} context={context}>{children}</CardFrame>}),
    define({id: 'platform.ui.tabs', summary: 'Keyboard-operable tabbed content.', semantics: 'Tablist, tab, and tabpanel relationships.', children: 'required', props: {label: {kind: 'string', required: true}, tabs: {kind: 'string-array', required: true}}, keyboard: 'Arrow Left/Right changes tab; Home/End selects first/last; Tab enters the active panel.', render: (node, children, context) => <Tabs node={node} context={context}>{children}</Tabs>}),
    define({id: 'platform.ui.accordion', summary: 'Progressively disclosed content section.', semantics: 'Native details and summary disclosure semantics.', children: 'optional', props: {title: {kind: 'string', required: true}, open: {kind: 'boolean'}}, keyboard: 'Enter or Space toggles the focused summary using native disclosure behavior.', render: (node, children, context) => <details className="meta-accordion" data-metadata-node={node.id} open={booleanProp(node, 'open')}><summary>{text(context, node, 'title', 'Details')}</summary><div>{children}</div></details>}),
    define({id: 'platform.ui.text', summary: 'Trusted plain text content.', semantics: 'A paragraph; metadata text is escaped by React.', children: 'none', props: {text: {kind: 'string', required: true}, tone: {kind: 'string', values: ['default', 'muted', 'strong']}}, render: (node, _, context) => <p className={`meta-text meta-text--${stringProp(node, 'tone', 'default')}`} data-metadata-node={node.id}>{text(context, node, 'text', '')}</p>}),
    define({id: 'platform.ui.alert', summary: 'Inline status or error message.', semantics: 'Status for ordinary feedback and alert for danger.', children: 'optional', props: {title: {kind: 'string', required: true}, variant: {kind: 'string', values: ['info', 'success', 'warning', 'danger']}}, render: (node, children, context) => <Alert title={text(context, node, 'title', 'Notice')} variant={stringProp(node, 'variant', 'info') as 'info' | 'success' | 'warning' | 'danger'} data-metadata-node={node.id}>{children}</Alert>}),
    define({id: 'platform.ui.form', summary: 'Metadata-bound form boundary for all supported record modes.', semantics: 'A labelled native form with status, validation, and conflict recovery regions.', children: 'required', props: {label: {kind: 'string', required: true}}, keyboard: 'Tab follows document order; Enter submits where native form behavior allows it.', render: (node, children, context) => context.interaction ? <MetadataFormShell label={text(context, node, 'label', 'Record form')} runtime={context.interaction}>{children}</MetadataFormShell> : children}),
    define({id: 'platform.ui.wizard', summary: 'Step-by-step form container driven by metadata actions.', semantics: 'A labelled region exposing the current step and progress.', children: 'required', props: {label: {kind: 'string', required: true}}, keyboard: 'Tab follows controls in the active step; after a metadata action changes steps, focus moves to the new labelled step group.', render: (node, children, context) => <Wizard node={node} context={context}>{children}</Wizard>}),
    define({id: 'platform.ui.field.text', summary: 'Labelled metadata-bound text field.', semantics: 'A labelled input with associated description and server or client error.', children: 'none', props: {field: {kind: 'string'}, name: {kind: 'string'}, label: {kind: 'string'}, description: {kind: 'string'}, error: {kind: 'string'}, value: {kind: 'string'}, type: {kind: 'string', values: ['text', 'email', 'tel', 'url', 'search']}, required: {kind: 'boolean'}, disabled: {kind: 'boolean'}, read_only: {kind: 'boolean'}}, render: (node, _, context) => {
      const descriptor = context.interaction?.field(node);
      const name = descriptor?.api_name ?? stringProp(node, 'name', node.id);
      const bound = context.interaction?.value(node);
      const value = typeof bound === 'string' || typeof bound === 'number' ? String(bound) : stringProp(node, 'value');
      const inferredType = descriptor?.field_type === 'email' ? 'email' : descriptor?.field_type === 'phone' ? 'tel' : descriptor?.field_type === 'number' ? 'number' : descriptor?.field_type === 'date' ? 'date' : stringProp(node, 'type', 'text');
      return <TextField id={node.id} name={name} label={descriptor?.label ?? text(context, node, 'label', name)} description={descriptor?.description ?? (hasText(node, 'description') ? text(context, node, 'description', '') : undefined)} error={context.interaction?.fieldErrors[name] ?? (hasText(node, 'error') ? text(context, node, 'error', '') : undefined)} value={value} onChange={event => context.interaction?.update(node, event.target.value)} type={inferredType} required={descriptor?.required || booleanProp(node, 'required')} disabled={booleanProp(node, 'disabled')} readOnly={(descriptor ? !descriptor.writable : false) || booleanProp(node, 'read_only')} data-field-api={name} data-metadata-node={node.id} />;
    }}),
    define({id: 'platform.ui.field.select', summary: 'Labelled metadata-bound single-choice field.', semantics: 'A labelled native select with description and server or client error.', children: 'none', props: {field: {kind: 'string'}, name: {kind: 'string'}, label: {kind: 'string'}, description: {kind: 'string'}, error: {kind: 'string'}, value: {kind: 'string'}, options: {kind: 'options'}, required: {kind: 'boolean'}, disabled: {kind: 'boolean'}}, render: (node, _, context) => {
      const descriptor = context.interaction?.field(node);
      const name = descriptor?.api_name ?? stringProp(node, 'name', node.id);
      const options = descriptor?.options?.length ? descriptor.options : (node.props?.options as Array<{value: string; label: string}> | undefined) ?? [];
      return <Select id={node.id} name={name} label={descriptor?.label ?? text(context, node, 'label', name)} description={descriptor?.description ?? (hasText(node, 'description') ? text(context, node, 'description', '') : undefined)} error={context.interaction?.fieldErrors[name] ?? (hasText(node, 'error') ? text(context, node, 'error', '') : undefined)} value={String(context.interaction?.value(node) ?? stringProp(node, 'value'))} onChange={event => context.interaction?.update(node, event.target.value)} required={descriptor?.required || booleanProp(node, 'required')} disabled={(descriptor ? !descriptor.writable : false) || booleanProp(node, 'disabled')} data-field-api={name} data-metadata-node={node.id}>{options.map(option => <option key={option.value} value={option.value}>{context.messages[option.label] ?? option.label}</option>)}</Select>;
    }}),
    define({id: 'platform.ui.collection-controls', summary: 'Labelled query controls for a collection.', semantics: 'A search landmark grouping query, filter, and sort controls.', children: 'required', props: {label: {kind: 'string', required: true}}, render: (node, children, context) => <section aria-label={text(context, node, 'label', 'Collection controls')} className="meta-collection-controls" data-metadata-node={node.id} role="search">{children}</section>}),
    define({id: 'platform.ui.query-input', summary: 'Metadata-bound text query control.', semantics: 'A labelled search input with an explicit apply action.', children: 'none', props: {label: {kind: 'string', required: true}, placeholder: {kind: 'string'}, value: {kind: 'string'}, action: {kind: 'string', required: true}, parameter: {kind: 'string'}, submit_label: {kind: 'string'}}, keyboard: 'Enter applies the current query; Tab reaches the explicit apply button.', render: (node, _, context) => <QueryInput node={node} context={context} />}),
    define({id: 'platform.ui.query-select', summary: 'Metadata-bound filter or sort control.', semantics: 'A labelled native select that invokes a declared action when changed.', children: 'none', props: {label: {kind: 'string', required: true}, value: {kind: 'string'}, options: {kind: 'options', required: true}, action: {kind: 'string', required: true}, parameter: {kind: 'string'}}, keyboard: 'Uses native select keyboard behavior and applies the selected value on change.', render: (node, _, context) => <QuerySelect node={node} context={context} />}),
    define({id: 'platform.ui.collection', summary: 'Selectable responsive row, card, or table collection.', semantics: 'A labelled section containing a semantic list, or a captioned table with scoped headers whose first column names each row; selectable items use native buttons and current-item state, sortable headers are native toggle buttons, and an optional open action is a named button per item.', children: 'none', props: {label: {kind: 'string', required: true}, resource: {kind: 'string'}, items: {kind: 'collection-items', required: true}, fields: {kind: 'columns'}, empty_title: {kind: 'string'}, empty_description: {kind: 'string'}, selected_id: {kind: 'string'}, select_action: {kind: 'string'}, selection_parameter: {kind: 'string'}, presentation: {kind: 'string', values: ['auto', 'rows', 'cards', 'table']}, identity_label: {kind: 'string'}, identity_sort: {kind: 'string'}, sort_action: {kind: 'string'}, sort_parameter: {kind: 'string'}, sort: {kind: 'string'}, sort_direction: {kind: 'string', values: ['ascending', 'descending']}, open_action: {kind: 'string'}, open_parameter: {kind: 'string'}, open_label: {kind: 'string'}}, keyboard: 'Tab enters item actions; Arrow keys move focus between selectable items; Home and End move to the first and last item; sortable headers and open actions are ordinary buttons in reading order.', render: (node, _, context) => <Collection node={node} context={context} />}),
    define({id: 'platform.ui.highlights', summary: 'Record highlights panel: identity, key fields, and primary actions.', semantics: 'A labelled section with the record identity, an optional decorative object tile, a definition list of configured key fields, and the child action groups in reading order.', children: 'optional', props: {label: {kind: 'string', required: true}, resource: {kind: 'string'}, record: {kind: 'record', required: true}, fields: {kind: 'columns'}, icon: {kind: 'string'}}, keyboard: 'Tab moves through the child actions in reading order; the panel itself is not focusable.', render: (node, children, context) => <Highlights node={node} context={context}>{children}</Highlights>}),
    define({id: 'platform.ui.path', summary: 'Ordered stages with the current one marked.', semantics: 'A labelled navigation landmark containing an ordered list; the current stage carries aria-current="step" and earlier stages are marked complete.', children: 'none', props: {label: {kind: 'string', required: true}, stages: {kind: 'string-array', required: true}, current: {kind: 'string'}, empty_description: {kind: 'string'}}, keyboard: 'Read-only; no focusable controls.', render: (node, _, context) => <Path node={node} context={context} />}),
    define({id: 'platform.ui.timeline', summary: 'Chronological record activity.', semantics: 'A labelled section containing an ordered list of entries with a title, optional description, actor, and a machine-readable time; a bounded empty state when there is no activity.', children: 'none', props: {label: {kind: 'string', required: true}, items: {kind: 'collection-items', required: true}, empty_title: {kind: 'string'}, empty_description: {kind: 'string'}}, keyboard: 'Read-only; no focusable controls.', render: (node, _, context) => <Timeline node={node} context={context} />}),
    define({id: 'platform.ui.filter-chips', summary: 'Active filters as removable chips.', semantics: 'A labelled group of chips, each with a named native remove button, plus an optional clear-all button; renders nothing when no filter is active.', children: 'none', props: {label: {kind: 'string'}, chips: {kind: 'chips', required: true}, clear_action: {kind: 'string'}, clear_label: {kind: 'string'}}, keyboard: 'Tab moves between remove buttons in reading order; Enter or Space removes the focused filter.', render: (node, _, context) => <FilterChips node={node} context={context} />}),
    define({id: 'platform.ui.record-preview', summary: 'Selected-record preview region.', semantics: 'A focusable labelled complementary region containing the selected record, or a bounded empty state.', children: 'optional', props: {label: {kind: 'string', required: true}, empty_title: {kind: 'string'}, empty_description: {kind: 'string'}}, render: (node, children, context) => {
      const content = React.Children.toArray(children);
      return <aside aria-label={text(context, node, 'label', 'Record preview')} className="meta-record-preview" data-empty={content.length ? 'false' : 'true'} data-metadata-node={node.id} tabIndex={-1}>{content.length ? content : <EmptyState title={text(context, node, 'empty_title', 'Select a record')} description={text(context, node, 'empty_description', 'Choose an item to see its details.')} />}</aside>;
    }}),
    define({id: 'platform.ui.record-summary', summary: 'Permission-safe record identity and field summary.', semantics: 'A labelled section with optional identity and a definition list of configured fields.', children: 'none', props: {label: {kind: 'string', required: true}, resource: {kind: 'string'}, record: {kind: 'record', required: true}, fields: {kind: 'columns'}, show_identity: {kind: 'boolean'}}, render: (node, _, context) => <RecordSummaryView node={node} context={context} />}),
    define({id: 'platform.ui.relationship-list', summary: 'Related-record collection with an explicit relationship label.', semantics: 'A labelled section containing a semantic list of related records.', children: 'none', props: {label: {kind: 'string', required: true}, resource: {kind: 'string'}, items: {kind: 'collection-items', required: true}, fields: {kind: 'columns'}, select_action: {kind: 'string'}, selection_parameter: {kind: 'string'}, empty_title: {kind: 'string'}, empty_description: {kind: 'string'}, presentation: {kind: 'string', values: ['auto', 'rows', 'cards']}}, render: (node, _, context) => <Collection node={node} context={context} />}),
    define({id: 'platform.ui.cursor', summary: 'Opaque previous and next cursor navigation.', semantics: 'A labelled navigation region with native previous and next buttons.', children: 'none', props: {label: {kind: 'string', required: true}, previous_action: {kind: 'string', required: true}, next_action: {kind: 'string', required: true}, previous_cursor: {kind: 'string'}, next_cursor: {kind: 'string'}, parameter: {kind: 'string'}, previous_label: {kind: 'string'}, next_label: {kind: 'string'}}, render: (node, _, context) => {
      const parameter = stringProp(node, 'parameter', 'cursor');
      const previous = stringProp(node, 'previous_cursor');
      const next = stringProp(node, 'next_cursor');
      return <nav aria-label={text(context, node, 'label', 'Results pages')} className="meta-cursor" data-metadata-node={node.id}><Button disabled={!previous} onClick={() => void context.interaction?.invoke(stringProp(node, 'previous_action'), {[parameter]: previous})} size="small" type="button" variant="secondary">{text(context, node, 'previous_label', 'Previous')}</Button><Button disabled={!next} onClick={() => void context.interaction?.invoke(stringProp(node, 'next_action'), {[parameter]: next})} size="small" type="button" variant="secondary">{text(context, node, 'next_label', 'Next')}</Button></nav>;
    }}),
    define({id: 'platform.ui.state', summary: 'Bounded collection or record state panel.', semantics: 'Uses status, alert, empty-state, or loading semantics appropriate to the declared state.', children: 'optional', props: {variant: {kind: 'string', required: true, values: ['loading', 'empty', 'error', 'denied', 'archived', 'unavailable']}, title: {kind: 'string'}, description: {kind: 'string'}, lines: {kind: 'number'}}, render: (node, children, context) => {
      const variant = stringProp(node, 'variant');
      if (variant === 'loading') return <div className="meta-state" data-metadata-node={node.id}><Skeleton label={text(context, node, 'title', 'Loading content')} lines={numberProp(node, 'lines', 3)} /></div>;
      if (variant === 'empty') return <div data-metadata-node={node.id}><EmptyState title={text(context, node, 'title', 'Nothing here yet')} description={text(context, node, 'description', 'There is no content to show.')} action={children} /></div>;
      if (variant === 'denied') return <div data-metadata-node={node.id}><Alert variant="warning" title="Content unavailable">You do not have access to this content.</Alert></div>;
      if (variant === 'error' || variant === 'unavailable') return <div data-metadata-node={node.id}><Alert variant="danger" title={text(context, node, 'title', 'Something went wrong')}>{text(context, node, 'description', 'This content could not be displayed.')}{children}</Alert></div>;
      return <section aria-label={text(context, node, 'title', 'Archived record')} className="meta-state" data-metadata-node={node.id} role="status"><strong>{text(context, node, 'title', 'Archived record')}</strong><p>{text(context, node, 'description', 'This record is archived.')}</p>{children}</section>;
    }}),
    define({id: 'platform.ui.table', summary: 'Responsive table and compact record-card data view.', semantics: 'A captioned table with scoped column headers when wide; a labelled list of definition-list cards when compact.', children: 'none', hosts: internalHosts, props: {label: {kind: 'string', required: true}, columns: {kind: 'columns', required: true}, rows: {kind: 'rows', required: true}}, render: (node, _, context) => {
      const columns = node.props?.columns as Array<{key: string; label: string}>;
      const rows = (node.props?.rows as unknown[]).filter(isRecord);
      const label = text(context, node, 'label', 'Results');
      if (context.formFactor === 'compact') return <section aria-label={label} className="meta-table-cards" data-metadata-node={node.id}><ul>{rows.map((row, index) => <li key={String(row.id ?? index)}><dl>{columns.map(column => <div key={column.key}><dt>{context.messages[column.label] ?? column.label}</dt><dd>{tableValue(row, column.key)}</dd></div>)}</dl></li>)}</ul></section>;
      return <div className="meta-table-wrap" data-metadata-node={node.id}><table><caption>{label}</caption><thead><tr>{columns.map(column => <th key={column.key} scope="col">{context.messages[column.label] ?? column.label}</th>)}</tr></thead><tbody>{rows.map((row, index) => <tr key={String(row.id ?? index)}>{columns.map(column => <td key={column.key}>{tableValue(row, column.key)}</td>)}</tr>)}</tbody></table></div>;
    }}),
    define({id: 'platform.ui.action-bar', summary: 'Grouped page actions.', semantics: 'A labelled toolbar-like action region.', children: 'required', props: {label: {kind: 'string'}}, render: (node, children, context) => <div aria-label={text(context, node, 'label', 'Page actions')} className="meta-action-bar" data-metadata-node={node.id} role="group">{children}</div>}),
    define({id: 'platform.ui.action', summary: 'Host-styled metadata action trigger.', semantics: 'A native button bound only to a registered action definition.', children: 'none', props: {action: {kind: 'string'}, label: {kind: 'string', required: true}, variant: {kind: 'string', values: ['primary', 'secondary', 'quiet', 'danger']}, disabled: {kind: 'boolean'}, submit: {kind: 'boolean'}}, render: (node, _, context) => {
      const submit = booleanProp(node, 'submit');
      return <Button variant={stringProp(node, 'variant', 'secondary') as 'primary' | 'secondary' | 'quiet' | 'danger'} disabled={booleanProp(node, 'disabled') || context.interaction?.submitting} loading={submit && context.interaction?.submitting} onClick={submit ? undefined : () => void context.interaction?.invoke(stringProp(node, 'action', node.id))} data-metadata-node={node.id} type={submit ? 'submit' : 'button'}>{text(context, node, 'label', 'Action')}</Button>;
    }}),
    define({id: 'platform.ui.dialog', summary: 'Modal or non-modal overlay content.', semantics: 'Native dialog labelled by its heading.', children: 'required', hosts: internalHosts, props: {title: {kind: 'string', required: true}, open: {kind: 'boolean'}}, keyboard: 'Tab follows controls within the dialog; Escape behavior is provided by the #117 action layer.', render: (node, children, context) => <dialog aria-labelledby={`${node.id}-title`} className="meta-dialog" data-metadata-node={node.id} open={booleanProp(node, 'open')}><h2 id={`${node.id}-title`}>{text(context, node, 'title', 'Dialog')}</h2>{children}</dialog>}),
    define({id: 'platform.ui.slot', summary: 'Stable contribution target.', semantics: 'A labelled contribution region when populated; empty slots add no blank landmark or chrome. Action slots remain neutral groups inside action bars.', children: 'optional', props: {label: {kind: 'string'}, kind: {kind: 'string', values: ['card', 'action', 'related-list', 'timeline']}}, render: (node, children, context) => {
      if (!React.Children.toArray(children).length) return null;
      const label = hasText(node, 'label') ? text(context, node, 'label', 'Related content') : undefined;
      return stringProp(node, 'kind') === 'action'
        ? <div aria-label={label} className="meta-slot meta-slot--action" data-metadata-node={node.id} role="group">{children}</div>
        : <aside aria-label={label} className={`meta-slot meta-slot--${stringProp(node, 'kind', 'card')}`} data-metadata-node={node.id}>{children}</aside>;
    }}),
    define({id: 'platform.ui.fragment', summary: 'Reusable fragment boundary.', semantics: 'Neutral grouping with source fragment identity.', children: 'required', props: {fragment_id: {kind: 'string', required: true}}, render: (node, children) => <div className="meta-fragment" data-fragment-id={stringProp(node, 'fragment_id')} data-metadata-node={node.id}>{children}</div>}),
    define({id: 'platform.ui.loading', summary: 'Named loading state.', semantics: 'Polite status with non-animated reduced-motion behavior.', children: 'none', props: {label: {kind: 'string', required: true}, lines: {kind: 'number'}}, render: (node, _, context) => <div className="meta-state" data-metadata-node={node.id}><Skeleton label={text(context, node, 'label', 'Loading content')} lines={numberProp(node, 'lines', 3)} /></div>}),
    define({id: 'platform.ui.empty', summary: 'Explanatory empty state.', semantics: 'Heading and description grouped as ordinary content.', children: 'optional', props: {title: {kind: 'string', required: true}, description: {kind: 'string', required: true}}, render: (node, children, context) => <div data-metadata-node={node.id}><EmptyState title={text(context, node, 'title', 'Nothing here yet')} description={text(context, node, 'description', 'There is no content to show.')} action={children} /></div>}),
    define({id: 'platform.ui.denied', summary: 'Non-disclosing permission-denied state.', semantics: 'Status message without inaccessible resource details.', children: 'none', props: {title: {kind: 'string'}, description: {kind: 'string'}}, render: (node, _, context) => <div data-metadata-node={node.id}><Alert variant="warning" title={text(context, node, 'title', 'Content unavailable')}>{text(context, node, 'description', 'You do not have access to this content.')}</Alert></div>}),
    define({id: 'platform.ui.error', summary: 'Contained component or page error.', semantics: 'Assertive error alert with a recovery-oriented message.', children: 'optional', props: {title: {kind: 'string'}, description: {kind: 'string'}}, render: (node, children, context) => <div data-metadata-node={node.id}><Alert variant="danger" title={text(context, node, 'title', 'Something went wrong')}>{text(context, node, 'description', 'This content could not be displayed.')}{children}</Alert></div>}),
  ];
  for (const definition of definitions) registry.register(definition);
  return registry;
}

export const defaultHostRegistry = createDefaultHostRegistry();
