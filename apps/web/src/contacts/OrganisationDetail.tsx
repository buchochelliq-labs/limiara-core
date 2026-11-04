import React, {useMemo, useState} from 'react';
import type {MetadataActionRequest, MetadataActionResult, MetadataInteractionAdapter} from '../plugin-ui/renderer/interactions';
import {useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import {contactsOrganisationPermissions, resolveContactsOrganisationDetail, type OrganisationDetailState} from './organisation-metadata';
import {organisations} from './organisations-data';
import {people, peopleFixtureWorkspaceId} from './people-data';
import {contactsPersonPermissions} from './person-metadata';
import './person-detail.css';

type ActionFixtureState = 'ready' | 'archive-error' | 'restore-error' | 'link-error' | 'unlink-error';
const permissions = new Set<string>([...Object.values(contactsOrganisationPermissions), ...Object.values(contactsPersonPermissions)]);

function readState(): OrganisationDetailState | Exclude<ActionFixtureState, 'ready'> {
  if (typeof window === 'undefined') return 'ready';
  const value = new URLSearchParams(window.location.search).get('organisation-detail-state');
  return value === 'loading' || value === 'missing' || value === 'denied' || value === 'error' || value === 'archived' || value === 'version-changed' || value === 'disabled' || value === 'workspace-changed' || value === 'archive-error' || value === 'restore-error' || value === 'link-error' || value === 'unlink-error' ? value : 'ready';
}

export function executeOrganisationFixture(request: MetadataActionRequest, state: ActionFixtureState, confirmed: boolean, archived: boolean, relatedIds: readonly string[]) {
  const id = request.action.id;
  if (id === 'archive-organisation' && !confirmed) return {result: {status: 'success'} as MetadataActionResult, archived, relatedIds, notice: 'Archive cancelled.'};
  const failure = id === 'archive-organisation' ? 'archive-error' : id === 'restore-organisation' ? 'restore-error' : id === 'link-person' ? 'link-error' : 'unlink-error';
  if (state === failure) return {result: {status: 'failure', category: 'network', message: 'The organisation update could not be completed. Try again.'} as MetadataActionResult, archived, relatedIds, notice: 'The organisation update could not be completed. Try again.'};
  if (id === 'archive-organisation') return {result: {status: 'success', etag: String(Number(request.etag ?? 0) + 1)} as MetadataActionResult, archived: true, relatedIds, notice: 'Organisation archived.'};
  if (id === 'restore-organisation') return {result: {status: 'success', etag: String(Number(request.etag ?? 0) + 1)} as MetadataActionResult, archived: false, relatedIds, notice: 'Organisation restored.'};
  if (id === 'link-person') {
    const candidate = people.find(person => !relatedIds.includes(person.id));
    return candidate ? {result: {status: 'success'} as MetadataActionResult, archived, relatedIds: [...relatedIds, candidate.id], notice: `${candidate.name} linked.`} : {result: {status: 'failure', category: 'unavailable', message: 'Everyone is already linked.'} as MetadataActionResult, archived, relatedIds, notice: 'Everyone is already linked.'};
  }
  const [removed, ...remaining] = relatedIds;
  return removed ? {result: {status: 'success'} as MetadataActionResult, archived, relatedIds: remaining, notice: `${people.find(person => person.id === removed)?.name ?? 'Person'} unlinked.`} : {result: {status: 'failure', category: 'unavailable', message: 'There is no person to unlink.'} as MetadataActionResult, archived, relatedIds, notice: 'There is no person to unlink.'};
}

type Props = {organisationId: string; workspaceId: string; onNavigate: (route: string) => void; callerPermissions?: ReadonlySet<string>; configuredFields?: ReadonlySet<string>; pluginEnabled?: boolean; fixtureState?: OrganisationDetailState | Exclude<ActionFixtureState, 'ready'>; formFactor?: 'wide' | 'compact'};

export function OrganisationDetail({organisationId, workspaceId, onNavigate, callerPermissions = permissions, configuredFields, pluginEnabled = true, fixtureState, formFactor}: Props) {
  const responsiveFormFactor = useResponsiveFormFactor();
  const effectiveFormFactor = formFactor ?? responsiveFormFactor;
  const organisation = useMemo(() => organisations.find(item => item.id === organisationId), [organisationId]);
  const requested = fixtureState ?? readState();
  const initialState: OrganisationDetailState = !pluginEnabled ? 'disabled' : workspaceId !== peopleFixtureWorkspaceId ? 'workspace-changed' : !organisation ? 'missing' : requested.endsWith('-error') ? 'ready' : requested as OrganisationDetailState;
  const initialRelated = useMemo(() => people.filter(person => person.organisation === organisation?.name).map(person => person.id), [organisation?.name]);
  const [relatedIds, setRelatedIds] = useState(initialRelated);
  const [archived, setArchived] = useState(initialState === 'archived');
  const [notice, setNotice] = useState('');
  const state: OrganisationDetailState = initialState === 'ready' && archived ? 'archived' : initialState === 'archived' && !archived ? 'ready' : initialState;
  const relatedPeople = useMemo(() => people.filter(person => relatedIds.includes(person.id)), [relatedIds]);
  const resolved = useMemo(() => resolveContactsOrganisationDetail({organisation, state, permissions: callerPermissions, configuredFields, relatedPeople, notice}), [callerPermissions, configuredFields, notice, organisation, relatedPeople, state]);
  const adapter = useMemo<MetadataInteractionAdapter>(() => ({
    navigate: (target, parameters) => {
      if (target === 'contacts.organisations.list') onNavigate('/contacts/organisations');
      if (target === 'contacts.organisations.edit') onNavigate(`/contacts/organisations/${String(parameters.id)}/edit`);
      if (target === 'contacts.people.open') onNavigate(`/contacts/people/${String(parameters.id)}`);
    },
    execute: async request => {
      const confirmed = request.action.id !== 'archive-organisation' || typeof window === 'undefined' || window.confirm(`Archive ${organisation?.name ?? 'this organisation'}?`);
      const actionState = requested.endsWith('-error') ? requested as ActionFixtureState : 'ready';
      const result = executeOrganisationFixture(request, actionState, confirmed, archived, relatedIds);
      setNotice(result.notice);
      if (result.result.status === 'success') { setArchived(result.archived); setRelatedIds([...result.relatedIds]); }
      return result.result;
    },
  }), [archived, onNavigate, organisation?.name, relatedIds, requested]);
  return <div className="content-canvas person-detail-page organisation-detail-page"><MetadataPageRenderer contextValues={{record_id: organisationId, archived, version_changed: state === 'version-changed', has_people: relatedIds.length > 0}} dataSources={resolved.dataSources} embedded embeddedHeadingOffset={0} etag={organisation ? String(organisation.version) : undefined} focusOnRender formFactor={effectiveFormFactor} host="internal-web" interactionAdapter={adapter} mode="view" page={resolved.page} permissions={callerPermissions} recordId={organisationId} resources={resolved.resources} /></div>;
}
