import React, {useEffect, useMemo, useRef, useState} from 'react';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import type {MetadataInteractionAdapter} from '../plugin-ui/renderer/interactions';
import {responsiveQueries, useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import {ContactsApiError, personResource, relationshipLabels, type ContactsClient, type PeoplePage, type PeopleQuery, type PersonRecord} from './api';
import {contactsPersonPermissions} from './person-metadata';
import {
  firstPageCursor,
  resolveContactsPeopleDirectory,
  type PeopleDirectoryQuery,
  type PeopleDirectoryState,
} from './people-directory-metadata';
import {useExtensionFields} from './useExtensionFields';
import {usePersonFieldNames} from './usePersonFields';
import './people-directory.css';

const allPermissions = new Set<string>(Object.values(contactsPersonPermissions));
const labels = new Set<string>(['All', ...relationshipLabels]);
const states = new Set<PeopleDirectoryState>(['ready', 'loading', 'empty', 'denied', 'error', 'disabled', 'archived']);
const cursorLimit = 512;

export function readPeopleDirectoryLocation(search = typeof window === 'undefined' ? '' : window.location.search): {query: PeopleDirectoryQuery; state: PeopleDirectoryState} {
  const params = new URLSearchParams(search);
  const label = params.get('label');
  const sort = params.get('sort');
  const cursor = params.get('cursor') ?? '';
  const view = params.get('view');
  const state = params.get('people-state') as PeopleDirectoryState | null;
  return {
    query: {
      query: (params.get('q') ?? '').slice(0, 200),
      label: label && labels.has(label) ? label : 'All',
      sort: sort === 'recent' ? 'recent' : 'name',
      cursor: cursor.length <= cursorLimit ? cursor : '',
      selected: (params.get('selected') ?? '').slice(0, 200),
      ...(view === 'cards' ? {view: 'cards' as const} : {}),
    },
    state: state && states.has(state) ? state : 'ready',
  };
}

export function buildPeopleDirectoryUrl(pathname: string, query: PeopleDirectoryQuery, state: PeopleDirectoryState = 'ready') {
  const params = new URLSearchParams();
  if (query.query) params.set('q', query.query);
  if (query.label !== 'All') params.set('label', query.label);
  if (query.sort !== 'name') params.set('sort', query.sort);
  if (query.cursor) params.set('cursor', query.cursor);
  if (query.selected) params.set('selected', query.selected);
  if (query.view === 'cards') params.set('view', 'cards');
  if (state !== 'ready') params.set('people-state', state);
  const suffix = params.toString();
  return `${pathname}${suffix ? `?${suffix}` : ''}`;
}

type PeopleLoad =
  | {status: 'loading'}
  | {status: 'ready'; page: PeoplePage}
  | {status: 'denied'}
  | {status: 'error'};

type SelectedLoad = {id: string; status: 'pending' | 'ready' | 'missing'; record?: PersonRecord};

function peopleQuery(query: PeopleDirectoryQuery, labelFilter: boolean): PeopleQuery {
  return {search: query.query, label: labelFilter ? query.label : 'All', sort: query.sort, status: 'active', cursor: query.cursor};
}

function loadFailure(error: unknown): PeopleLoad {
  return error instanceof ContactsApiError && (error.status === 403 || error.status === 401) ? {status: 'denied'} : {status: 'error'};
}

function seededPeople(client: ContactsClient, query: PeopleQuery): PeopleLoad {
  const seeded = client.seeds?.people?.(query);
  if (!seeded) return {status: 'loading'};
  return 'value' in seeded ? {status: 'ready', page: seeded.value} : loadFailure(seeded.error);
}

function seededSelection(client: ContactsClient, id: string): SelectedLoad | undefined {
  const seeded = client.seeds?.person?.(id);
  if (!seeded) return undefined;
  return 'value' in seeded ? {id, status: 'ready', record: seeded.value} : {id, status: 'missing'};
}

type PeopleDirectoryProps = {
  route: string;
  search?: string;
  onNavigate: (route: string) => void;
  client: ContactsClient;
  permissions?: ReadonlySet<string>;
  pluginEnabled?: boolean;
  /** Development-only state override; production reads the state from the client. */
  forcedState?: PeopleDirectoryState;
};

export function PeopleDirectory({onNavigate, client, permissions = allPermissions, pluginEnabled = true, forcedState, search}: PeopleDirectoryProps) {
  const initial = useMemo(() => readPeopleDirectoryLocation(search), [search]);
  const [query, setQuery] = useState(initial.query);
  const [notice, setNotice] = useState('');
  const formFactor = useResponsiveFormFactor(responsiveQueries.compactCollection);
  const compact = formFactor === 'compact';
  const selectedId = query.selected || undefined;
  const previousSelected = useRef(selectedId);
  const cursorHistory = useRef<string[]>([]);
  const configuredCustomFields = usePersonFieldNames(client, permissions);
  const [extensionLoad] = useExtensionFields(client, personResource, pluginEnabled && permissions.has(contactsPersonPermissions.read));

  useEffect(() => {
    if (search === undefined) return;
    setQuery(readPeopleDirectoryLocation(search).query);
  }, [search]);

  const relationshipAvailable = permissions.has(contactsPersonPermissions.customFields) && configuredCustomFields.has('relationship_type__c') && client.capabilities?.labelFilter === true;
  const effectiveQuery = useMemo(() => relationshipAvailable ? query : {...query, label: 'All'}, [query, relationshipAvailable]);
  const apiQuery = useMemo(() => peopleQuery(effectiveQuery, relationshipAvailable), [effectiveQuery, relationshipAvailable]);
  const apiQueryKey = JSON.stringify(apiQuery);

  const [load, setLoad] = useState<PeopleLoad>(() => seededPeople(client, apiQuery));
  const [selectedLoad, setSelectedLoad] = useState<SelectedLoad | undefined>(() => selectedId ? seededSelection(client, selectedId) : undefined);

  useEffect(() => {
    if (!pluginEnabled || !permissions.has(contactsPersonPermissions.read)) return;
    const controller = new AbortController();
    setLoad(seededPeople(client, apiQuery));
    client.listPeople(apiQuery, controller.signal)
      .then(page => { if (!controller.signal.aborted) setLoad({status: 'ready', page}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(loadFailure(error)); });
    return () => controller.abort();
    // apiQueryKey captures every field of apiQuery.
  }, [apiQueryKey, client, permissions, pluginEnabled]);

  const page = load.status === 'ready' ? load.page : undefined;
  const selectedOnPage = selectedId ? page?.records.find(record => record.id === selectedId) : undefined;

  useEffect(() => {
    if (!selectedId || selectedOnPage || load.status !== 'ready') return;
    if (selectedLoad?.id === selectedId && selectedLoad.status !== 'pending') return;
    const controller = new AbortController();
    setSelectedLoad(seededSelection(client, selectedId) ?? {id: selectedId, status: 'pending'});
    client.getPerson(selectedId, controller.signal)
      .then(record => { if (!controller.signal.aborted) setSelectedLoad({id: selectedId, status: 'ready', record}); })
      .catch(() => { if (!controller.signal.aborted) setSelectedLoad({id: selectedId, status: 'missing'}); });
    return () => controller.abort();
    // selectedLoad is read to avoid refetching a settled selection.
  }, [client, load.status, selectedId, selectedOnPage]);

  useEffect(() => {
    document.title = 'People - Limiara';
  }, []);

  useEffect(() => {
    const restore = () => {
      cursorHistory.current = [];
      setQuery(readPeopleDirectoryLocation().query);
      setNotice('');
    };
    window.addEventListener('popstate', restore);
    return () => window.removeEventListener('popstate', restore);
  }, []);

  useEffect(() => {
    if (!compact || !selectedId || previousSelected.current === selectedId) {
      previousSelected.current = selectedId;
      return;
    }
    previousSelected.current = selectedId;
    requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-metadata-node="person-preview"]')?.focus());
  }, [compact, selectedId]);

  useEffect(() => {
    if (relationshipAvailable || query.label === 'All') return;
    const next = {...query, label: 'All', cursor: ''};
    setQuery(next);
    window.history.replaceState({}, '', buildPeopleDirectoryUrl(window.location.pathname, next));
  }, [query, relationshipAvailable]);

  const state: PeopleDirectoryState = forcedState ?? (!pluginEnabled ? 'disabled' : load.status === 'ready' ? 'ready' : load.status);
  const settledSelection = selectedLoad && selectedLoad.id === selectedId ? selectedLoad : undefined;
  const selected = selectedOnPage ?? settledSelection?.record;
  const selectedPending = Boolean(selectedId) && !selectedOnPage && (load.status === 'loading' || !settledSelection || settledSelection.status === 'pending');
  const previousCursor = cursorHistory.current.length ? cursorHistory.current[cursorHistory.current.length - 1] || firstPageCursor : firstPageCursor;
  const resolved = useMemo(() => resolveContactsPeopleDirectory({query: effectiveQuery, selectedId, state, page, selected, selectedPending, previousCursor, labelFilter: client.capabilities?.labelFilter === true, pluginEnabled, permissions, configuredCustomFields, extensions: extensionLoad.set, notice}), [client, configuredCustomFields, effectiveQuery, extensionLoad.set, notice, page, permissions, pluginEnabled, previousCursor, selected, selectedId, selectedPending, state]);

  const commit = (pathname: string, next: PeopleDirectoryQuery, resetHistory = true) => {
    const safe = relationshipAvailable ? next : {...next, label: 'All'};
    if (resetHistory) cursorHistory.current = [];
    setQuery(safe);
    setNotice('');
    onNavigate(buildPeopleDirectoryUrl(pathname, safe, forcedState));
  };
  const adapter = useMemo<MetadataInteractionAdapter>(() => ({
    execute: async () => ({status: 'success'}),
    navigate: (target, parameters) => {
      if (target === 'contacts.people.create') { onNavigate('/contacts/people/new'); return; }
      if (target === 'contacts.people.list') { commit('/contacts', query); return; }
      if (target === 'contacts.organisations.list') { onNavigate('/contacts/organisations'); return; }
      if (target === 'contacts.people.search') { commit(window.location.pathname, {...query, query: String(parameters.query ?? '').slice(0, 200), cursor: ''}); return; }
      if (target === 'contacts.people.filter') {
        const label = String(parameters.label ?? 'All');
        commit(window.location.pathname, {...query, label: labels.has(label) ? label : 'All', cursor: ''});
        return;
      }
      if (target === 'contacts.people.sort') { commit(window.location.pathname, {...query, sort: parameters.sort === 'recent' ? 'recent' : 'name', cursor: ''}); return; }
      if (target === 'contacts.people.view') {
        const {view: _, ...rest} = query;
        commit(window.location.pathname, parameters.view === 'cards' ? {...rest, view: 'cards'} : rest, false);
        return;
      }
      if (target === 'contacts.people.clear') { commit(window.location.pathname, {query: '', label: 'All', sort: query.sort, cursor: '', selected: query.selected, ...(query.view ? {view: query.view} : {})}); return; }
      if (target === 'contacts.people.cursor') {
        const cursor = String(parameters.cursor ?? '').slice(0, cursorLimit);
        const previous = cursorHistory.current.length ? cursorHistory.current[cursorHistory.current.length - 1] : undefined;
        const goingBack = cursor === firstPageCursor || (previous !== undefined && cursor === (previous || firstPageCursor));
        if (goingBack) cursorHistory.current = cursorHistory.current.slice(0, -1);
        else cursorHistory.current = [...cursorHistory.current, query.cursor];
        commit(window.location.pathname, {...query, cursor: cursor === firstPageCursor ? '' : cursor}, false);
        return;
      }
      if (target === 'contacts.people.select') {
        const id = String(parameters.id ?? '');
        if (id) commit('/contacts', {...query, selected: id}, false);
        return;
      }
      if (target === 'contacts.people.edit') {
        const id = String(parameters.id ?? '');
        if (id) onNavigate(`/contacts/people/${encodeURIComponent(id)}/edit`);
        return;
      }
      if (target === 'contacts.people.open') {
        const id = String(parameters.id ?? '');
        if (id) onNavigate(`/contacts/people/${encodeURIComponent(id)}`);
      }
    },
    // commit reads the latest query and relationship availability.
  }), [onNavigate, query, relationshipAvailable, forcedState]);
  const announcement = notice || resolved.announcement;

  return <div className="content-canvas people-directory person-metadata-directory">
    <MetadataPageRenderer
      announcement={announcement}
      dataSources={resolved.dataSources}
      embedded
      embeddedHeadingOffset={0}
      formFactor={formFactor}
      host="internal-web"
      interactionAdapter={adapter}
      page={resolved.page}
      permissions={permissions}
      resources={resolved.resources}
    />
  </div>;
}
