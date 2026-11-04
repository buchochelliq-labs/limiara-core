import React, {useEffect, useMemo, useRef, useState} from 'react';
import type {MetadataInteractionAdapter} from '../plugin-ui/renderer/interactions';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import {responsiveQueries, useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import {contactsOrganisationPermissions, resolveContactsOrganisationDirectory, type OrganisationDirectoryQuery, type OrganisationDirectoryState} from './organisation-metadata';
import {contactsPersonPermissions} from './person-metadata';
import './people-directory.css';

const permissions = new Set<string>([...Object.values(contactsOrganisationPermissions), ...Object.values(contactsPersonPermissions)]);
const statuses = new Set<OrganisationDirectoryQuery['status']>(['All', 'Active', 'Prospect', 'Partner']);
const states = new Set<OrganisationDirectoryState>(['ready', 'loading', 'empty', 'denied', 'error', 'disabled', 'archived']);

export function readOrganisationDirectoryLocation(search = typeof window === 'undefined' ? '' : window.location.search): {query: OrganisationDirectoryQuery; state: OrganisationDirectoryState} {
  const params = new URLSearchParams(search);
  const status = params.get('status') as OrganisationDirectoryQuery['status'] | null;
  const state = params.get('organisation-state') as OrganisationDirectoryState | null;
  return {query: {query: (params.get('q') ?? '').slice(0, 200), status: status && statuses.has(status) ? status : 'All', sort: params.get('sort') === 'recent' ? 'recent' : 'name', cursor: params.get('cursor') === 'page-2' ? 'page-2' : '', selected: (params.get('selected') ?? '').slice(0, 200)}, state: state && states.has(state) ? state : 'ready'};
}

export function buildOrganisationDirectoryUrl(query: OrganisationDirectoryQuery, state: OrganisationDirectoryState = 'ready') {
  const params = new URLSearchParams();
  if (query.query) params.set('q', query.query);
  if (query.status !== 'All') params.set('status', query.status);
  if (query.sort !== 'name') params.set('sort', query.sort);
  if (query.cursor) params.set('cursor', query.cursor);
  if (query.selected) params.set('selected', query.selected);
  if (state !== 'ready') params.set('organisation-state', state);
  return `/contacts/organisations${params.size ? `?${params}` : ''}`;
}

type Props = {search?: string; onNavigate: (route: string) => void; callerPermissions?: ReadonlySet<string>; pluginEnabled?: boolean; configuredFields?: ReadonlySet<string>; formFactor?: 'wide' | 'compact'};

export function OrganisationDirectory({search, onNavigate, callerPermissions = permissions, pluginEnabled = true, configuredFields, formFactor}: Props) {
  const initial = useMemo(() => readOrganisationDirectoryLocation(search), [search]);
  const [query, setQuery] = useState(initial.query);
  const [state, setState] = useState(initial.state);
  const responsiveFormFactor = useResponsiveFormFactor(responsiveQueries.compactCollection);
  const effectiveFormFactor = formFactor ?? responsiveFormFactor;
  const effectiveCompact = effectiveFormFactor === 'compact';
  const previousSelected = useRef(query.selected);
  const customAvailable = callerPermissions.has(contactsOrganisationPermissions.customFields) && (!configuredFields || configuredFields.has('account_status__c'));
  const effectiveQuery = useMemo(() => customAvailable ? query : {...query, status: 'All' as const}, [customAvailable, query]);

  useEffect(() => {
    if (search === undefined) return;
    const next = readOrganisationDirectoryLocation(search);
    setQuery(next.query);
    setState(next.state);
  }, [search]);

  useEffect(() => { document.title = 'Organisations - Limiara'; }, []);
  useEffect(() => {
    if (!effectiveCompact || !query.selected || previousSelected.current === query.selected) { previousSelected.current = query.selected; return; }
    previousSelected.current = query.selected;
    requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-metadata-node="organisation-preview"]')?.focus());
  }, [effectiveCompact, query.selected]);

  const resolved = useMemo(() => resolveContactsOrganisationDirectory({query: effectiveQuery, state, pluginEnabled, permissions: callerPermissions, configuredFields}), [callerPermissions, configuredFields, effectiveQuery, pluginEnabled, state]);
  const commit = (next: OrganisationDirectoryQuery) => {
    const safe = customAvailable ? next : {...next, status: 'All' as const};
    setQuery(safe);
    onNavigate(buildOrganisationDirectoryUrl(safe, state));
  };
  const adapter = useMemo<MetadataInteractionAdapter>(() => ({
    execute: async () => ({status: 'success'}),
    navigate: (target, parameters) => {
      if (target === 'contacts.people.list') { onNavigate('/contacts'); return; }
      if (target === 'contacts.organisations.list') { commit(query); return; }
      if (target === 'contacts.organisations.create') { onNavigate('/contacts/organisations/new'); return; }
      if (target === 'contacts.organisations.search') { commit({...query, query: String(parameters.query ?? '').slice(0, 200), cursor: ''}); return; }
      if (target === 'contacts.organisations.filter') { const status = String(parameters.status ?? 'All') as OrganisationDirectoryQuery['status']; commit({...query, status: statuses.has(status) ? status : 'All', cursor: ''}); return; }
      if (target === 'contacts.organisations.sort') { commit({...query, sort: parameters.sort === 'recent' ? 'recent' : 'name', cursor: ''}); return; }
      if (target === 'contacts.organisations.clear') { commit({query: '', status: 'All', sort: query.sort, cursor: '', selected: query.selected}); return; }
      if (target === 'contacts.organisations.cursor') { commit({...query, cursor: parameters.cursor === 'page-2' ? 'page-2' : ''}); return; }
      const id = String(parameters.id ?? '');
      if (target === 'contacts.organisations.select' && id) { commit({...query, selected: id}); return; }
      if (target === 'contacts.organisations.edit' && id) { onNavigate(`/contacts/organisations/${encodeURIComponent(id)}/edit`); return; }
      if (target === 'contacts.organisations.open' && id) onNavigate(`/contacts/organisations/${encodeURIComponent(id)}`);
    },
  }), [customAvailable, onNavigate, query, state]);

  return <div className="content-canvas people-directory person-metadata-directory organisation-metadata-directory">
    <MetadataPageRenderer announcement={resolved.announcement} dataSources={resolved.dataSources} embedded embeddedHeadingOffset={0} formFactor={effectiveFormFactor} host="internal-web" interactionAdapter={adapter} page={resolved.page} permissions={callerPermissions} resources={resolved.resources} />
  </div>;
}
