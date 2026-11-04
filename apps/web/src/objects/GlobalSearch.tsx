import React, {useEffect, useId, useMemo, useRef, useState, type RefObject} from 'react';
import {MagnifyingGlass} from '@phosphor-icons/react';
import {Alert, Button, EmptyState, Skeleton} from '../components/ui';
import {ObjectsApiError, type ObjectsClient, type SearchGroup} from './api';
import {recordRoute} from './ObjectPages';
import {formatCell} from './QueryConsole';
import './objects.css';

export const searchRoute = '/search';
export const searchResultsRoute = (q: string, object?: string) => `${searchRoute}?${new URLSearchParams(object ? {q, object} : {q})}`;
/** Searches start after this many letters or digits, as the server requires. */
export const searchMinimum = 2;
const typingDelay = 200;

/** Whether a query has enough letters or digits to search. */
export function searchable(q: string): boolean {
  return q.replace(/[^\p{L}\p{N}]+/gu, '').length >= searchMinimum;
}

type Option = {id: string; label: string; route: string; group?: string};

/** The options a suggestion list shows: records per group, then "See all". */
export function searchOptions(groups: SearchGroup[], q: string, prefix: string): Option[] {
  const records = groups.flatMap(group => group.records.map(record => ({id: `${prefix}-${group.object}-${record.id}`, label: record.name, route: recordRoute(group.object, record.id), group: group.label})));
  return [...records, {id: `${prefix}-all`, label: `See all results for “${q.trim()}”`, route: searchResultsRoute(q.trim())}];
}

function subtitle(fields: Record<string, unknown>): string {
  return Object.values(fields).map(value => formatCell(value)).join(' · ');
}

type Suggestions = {status: 'idle'} | {status: 'loading'} | {status: 'ready'; groups: SearchGroup[]} | {status: 'failed'};

/**
 * The shell search bar (ADR 0020): a combobox with instant results grouped
 * by object. Arrow keys move through results, Enter opens one or the full
 * results page, Escape closes the list. `/` and Ctrl+K or ⌘K focus it.
 */
export function GlobalSearch({client, onNavigate}: {client: Pick<ObjectsClient, 'search'>; onNavigate: (route: string) => void}) {
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [suggestions, setSuggestions] = useState<Suggestions>({status: 'idle'});
  const input = useRef<HTMLInputElement>(null);
  const closing = useRef<number | undefined>(undefined);
  const id = useId();
  const listId = `${id}-results`;

  useEffect(() => {
    const focus = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = target?.closest('input, textarea, select, [contenteditable="true"]');
      if ((event.key === 'k' && (event.metaKey || event.ctrlKey)) || (event.key === '/' && !typing)) {
        event.preventDefault();
        input.current?.focus();
        input.current?.select();
      }
    };
    document.addEventListener('keydown', focus);
    return () => { document.removeEventListener('keydown', focus); window.clearTimeout(closing.current); };
  }, []);

  useEffect(() => {
    if (!searchable(q)) { setSuggestions({status: 'idle'}); return; }
    const controller = new AbortController();
    setSuggestions({status: 'loading'});
    const timer = window.setTimeout(() => {
      client.search(q, {limit: 3}, controller.signal)
        .then(groups => { if (!controller.signal.aborted) { setSuggestions({status: 'ready', groups}); setActive(-1); } })
        .catch(() => { if (!controller.signal.aborted) setSuggestions({status: 'failed'}); });
    }, typingDelay);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [client, q]);

  const options = useMemo(() => suggestions.status === 'ready' ? searchOptions(suggestions.groups, q, id) : [], [suggestions, q, id]);
  const expanded = open && searchable(q);
  const go = (route: string) => { setOpen(false); setActive(-1); input.current?.blur(); onNavigate(route); };

  const keyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (!options.length) return;
      event.preventDefault();
      setOpen(true);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setActive(current => (current + step + options.length) % options.length);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const chosen = options[active];
      if (chosen) go(chosen.route);
      else if (searchable(q)) go(searchResultsRoute(q.trim()));
    } else if (event.key === 'Escape') {
      if (open) { setOpen(false); setActive(-1); } else setQ('');
    }
  };

  let index = -1;
  const groups = suggestions.status === 'ready' ? suggestions.groups : [];
  return <div className="global-search-box">
    <label className="global-search">
      <MagnifyingGlass aria-hidden="true" />
      <span className="sr-only">Search Limiara</span>
      <input
        ref={input}
        type="search"
        placeholder="Search Limiara"
        role="combobox"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={expanded && options[active] ? options[active].id : undefined}
        autoComplete="off"
        value={q}
        onChange={event => { setQ(event.target.value); setOpen(true); setActive(-1); }}
        onFocus={() => { window.clearTimeout(closing.current); setOpen(true); }}
        onBlur={() => { closing.current = window.setTimeout(() => setOpen(false), 150); }}
        onKeyDown={keyDown}
      />
      <kbd aria-hidden="true">⌘ K</kbd>
    </label>
    <div className="global-search__panel" hidden={!expanded}>
      <div className="sr-only" aria-live="polite">{expanded && suggestions.status === 'ready' ? `${options.length - 1} results` : ''}</div>
      {suggestions.status === 'loading' && <p className="global-search__note">Searching…</p>}
      {suggestions.status === 'failed' && <p className="global-search__note">Search is unavailable. Try again in a moment.</p>}
      {suggestions.status === 'ready' && groups.length === 0 && <p className="global-search__note">No records match “{q.trim()}”.</p>}
      <ul id={listId} role="listbox" aria-label="Search results" className="global-search__list">
        {groups.map(group => <li key={group.object} role="presentation">
          <ul role="group" aria-label={group.pluralLabel} className="global-search__group">
            <li role="presentation" className="global-search__heading" aria-hidden="true">{group.pluralLabel}</li>
            {group.records.map(record => {
              index += 1;
              const option = options[index];
              if (!option) return null;
              return <li key={record.id} id={option.id} role="option" aria-selected={active === index} className="global-search__option" onMouseDown={event => event.preventDefault()} onClick={() => go(option.route)}>
                <span className="global-search__name">{record.name}</span>
                {Object.keys(record.fields).length > 0 && <span className="global-search__detail">{subtitle(record.fields)}</span>}
              </li>;
            })}
          </ul>
        </li>)}
        {suggestions.status === 'ready' && (() => {
          const all = options[options.length - 1];
          return all ? <li id={all.id} role="option" aria-selected={active === options.length - 1} className="global-search__option global-search__all" onMouseDown={event => event.preventDefault()} onClick={() => go(all.route)}>{all.label}</li> : null;
        })()}
      </ul>
    </div>
  </div>;
}

type Results = {status: 'loading'} | {status: 'ready'; groups: SearchGroup[]} | {status: 'invalid'} | {status: 'failed'};

/** The full results page: every object, or one object with up to 200 records. */
export function SearchResultsPage({client, search, onNavigate, headingRef}: {client: Pick<ObjectsClient, 'search'>; search: string; onNavigate: (route: string) => void; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const params = new URLSearchParams(search);
  const q = params.get('q') ?? '';
  const object = params.get('object') ?? undefined;
  const [results, setResults] = useState<Results>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [tabs, setTabs] = useState<{object: string; label: string}[]>([]);
  useEffect(() => { document.title = q ? `Search: ${q} - Limiara` : 'Search - Limiara'; }, [q]);
  useEffect(() => {
    if (!searchable(q)) { setResults({status: 'invalid'}); return; }
    const controller = new AbortController();
    setResults({status: 'loading'});
    client.search(q, object ? {object, limit: 200} : {limit: 10}, controller.signal)
      .then(groups => {
        if (controller.signal.aborted) return;
        setResults({status: 'ready', groups});
        if (!object) setTabs(groups.map(group => ({object: group.object, label: group.pluralLabel})));
      })
      .catch(error => { if (!controller.signal.aborted) setResults(error instanceof ObjectsApiError && error.reason === 'invalid' ? {status: 'invalid'} : {status: 'failed'}); });
    return () => controller.abort();
  }, [client, q, object, attempt]);

  const link = (route: string, label: React.ReactNode, current = false) => <a href={route} aria-current={current ? 'page' : undefined} onClick={event => { event.preventDefault(); onNavigate(route); }}>{label}</a>;
  return <div className="content-canvas object-page">
    <header className="page-header"><div>
      <p className="page-header__breadcrumb">Search</p>
      <h1 ref={headingRef} tabIndex={-1}>{q ? <>Results for “{q}”</> : 'Search'}</h1>
      <p>Records you can see, from every searchable object.</p>
    </div></header>
    {tabs.length > 1 && <nav aria-label="Filter results by object" className="search-tabs">
      {link(searchResultsRoute(q), 'All', !object)}
      {tabs.map(tab => <React.Fragment key={tab.object}>{link(searchResultsRoute(q, tab.object), tab.label, object === tab.object)}</React.Fragment>)}
    </nav>}
    {results.status === 'loading' && <Skeleton label="Searching" lines={4} />}
    {results.status === 'invalid' && <Alert variant="info" title="Search with at least two letters or digits">Type a name, an email address or another value of a searchable field.</Alert>}
    {results.status === 'failed' && <div className="object-page__state"><Alert variant="danger" title="Search is unavailable">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {results.status === 'ready' && results.groups.length === 0 && <EmptyState title="No results" description={`No records you can see match “${q}”.`} />}
    {results.status === 'ready' && results.groups.map(group => <section key={group.object} className="object-section" aria-labelledby={`search-${group.object}`}>
      <div className="object-section__header">
        <h2 id={`search-${group.object}`}>{group.pluralLabel}</h2>
        {group.hasMore && !object && link(searchResultsRoute(q, group.object), `See all ${group.pluralLabel.toLowerCase()}`)}
      </div>
      <ul className="search-results">
        {group.records.map(record => <li key={record.id}>
          {link(recordRoute(group.object, record.id), record.name)}
          {Object.keys(record.fields).length > 0 && <span className="global-search__detail">{subtitle(record.fields)}</span>}
        </li>)}
      </ul>
      {group.hasMore && object && <p className="global-search__note">Showing the first {group.records.length}. Add words to narrow the search.</p>}
    </section>)}
  </div>;
}
