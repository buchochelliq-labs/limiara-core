import React, {useEffect, useMemo, useState} from 'react';
import type {ExtensionContribution} from '../plugin-ui/contracts';
import {useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import type {MetadataActionResult, MetadataInteractionAdapter} from '../plugin-ui/renderer/interactions';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import {Button} from '../components/ui';
import {FieldArrangement, type ArrangementSaveResult} from '../plugin-ui/extension-fields/FieldArrangement';
import type {FieldPlacement} from '../plugin-ui/extension-fields/model';
import {ContactsApiError, personResource, type ActivityEntry, type ContactsClient, type FieldLayoutState, type PersonRecord} from './api';
import {contactsZoneHostFields} from './field-zones';
import {useExtensionFields} from './useExtensionFields';
import {contactsPersonPermissions} from './person-metadata';
import {resolveContactsPersonDetail, type PersonDetailState} from './person-detail-metadata';
import {usePersonFieldNames} from './usePersonFields';
import './person-detail.css';
import {userRecordRoute, userRecordTarget} from '../plugin-ui/standard-fields/model';

const defaultPermissions = new Set<string>([...Object.values(contactsPersonPermissions), 'contacts.organisations.read']);

type RecordLoad =
  | {status: 'loading'}
  | {status: 'ready'; record: PersonRecord}
  | {status: 'missing'}
  | {status: 'denied'}
  | {status: 'error'};

function loadFailure(error: unknown): RecordLoad {
  const status = error instanceof ContactsApiError ? error.status : 0;
  return status === 404 ? {status: 'missing'} : status === 403 || status === 401 ? {status: 'denied'} : {status: 'error'};
}

/** Maps a failed archive or restore to the renderer's result contract and the notice the page shows. */
export function describeRecordOperationError(error: unknown, operation: 'archived' | 'restored' | 'deleted' | 'undeleted'): {result: MetadataActionResult; versionChanged: boolean; missing: boolean} {
  const status = error instanceof ContactsApiError ? error.status : 0;
  if (status === 409) return {result: {status: 'failure', category: 'unavailable', message: 'This person changed since it was loaded. Review the latest version before trying again.'}, versionChanged: true, missing: false};
  if (status === 404) return {result: {status: 'failure', category: 'unavailable', message: 'This person may have been archived or removed.'}, versionChanged: false, missing: true};
  if (status === 403 || status === 401) return {result: {status: 'failure', category: 'denied', message: 'You do not have permission to change this person.'}, versionChanged: false, missing: false};
  return {result: {status: 'failure', category: 'network', message: `The person could not be ${operation}. Try again.`}, versionChanged: false, missing: false};
}

type Props = {
  personId: string;
  onNavigate: (route: string) => void;
  workspaceId: string;
  client: ContactsClient;
  permissions?: ReadonlySet<string>;
  pluginEnabled?: boolean;
  /** Development-only state override; production derives the state from the client. */
  forcedState?: PersonDetailState;
  contributions?: readonly ExtensionContribution[];
  enabledContributionPlugins?: ReadonlySet<string>;
  formFactor?: 'wide' | 'compact';
};

/** Delays in milliseconds between timeline re-reads after an archive or restore. */
export const activityRefreshDelays = [1000, 2000, 4000, 8000];

/** A pending timeline re-read: which record version to wait for, and how many re-reads have run. */
export type ActivityRefresh = {version: number; attempt: number};

/** Keeps waiting until the timeline holds an entry for the written version or every delay is spent. */
export function nextActivityRefresh(current: ActivityRefresh | null, entries: readonly ActivityEntry[]): ActivityRefresh | null {
  if (!current) return null;
  if (entries.some(entry => entry.version >= current.version)) return null;
  return current.attempt < activityRefreshDelays.length ? current : null;
}

/** Permission to arrange where fields appear, shared with tenant layout customisation. */
export const arrangeFieldsPermission = 'ui.metadata.manage';

/** Saves an administrator's arrangement and describes the outcome for the editor. */
export async function savePersonFieldLayout(client: ContactsClient, revision: number, layout: {arranged: string[]; placements: FieldPlacement[]}): Promise<ArrangementSaveResult> {
  if (typeof client.saveFieldLayout !== 'function') return {status: 'failed', message: 'Fields cannot be arranged here.'};
  try {
    await client.saveFieldLayout(personResource, revision, layout);
    return {status: 'saved'};
  } catch (error) {
    const status = error instanceof ContactsApiError ? error.status : 0;
    if (status === 409) return {status: 'failed', message: 'Someone else changed the arrangement. Close and reopen it to see the latest version.'};
    if (status === 403 || status === 401) return {status: 'failed', message: 'You do not have permission to arrange fields.'};
    if (status === 400) return {status: 'failed', message: 'Some fields or places are no longer available. Close and reopen the arrangement.'};
    return {status: 'failed', message: 'The arrangement could not be saved. Try again.'};
  }
}

type LifecycleOperation = {
  past: 'archived' | 'restored' | 'deleted' | 'undeleted';
  notice: string;
  unavailable: string;
  confirm?: (name: string) => string;
  cancelled?: string;
  run: (client: ContactsClient) => ((id: string, version: number) => Promise<PersonRecord>) | undefined;
};

/** The record lifecycle actions on the person page, by metadata action id. */
const lifecycleOperations: Record<string, LifecycleOperation> = {
  'archive-person': {past: 'archived', notice: 'Person archived.', unavailable: 'Archiving is not available.', confirm: name => `Archive ${name}?`, cancelled: 'Archive cancelled.', run: client => client.archivePerson.bind(client)},
  'restore-person': {past: 'restored', notice: 'Person restored.', unavailable: 'Restoring an archived person is not available.', run: client => client.restorePerson?.bind(client)},
  'delete-person': {past: 'deleted', notice: 'Person moved to the recycle bin. Undelete it within 15 days to keep it.', unavailable: 'Deleting is not available.', confirm: name => `Delete ${name}? It stays in the recycle bin for 15 days.`, cancelled: 'Delete cancelled.', run: client => client.deletePerson?.bind(client)},
  'undelete-person': {past: 'undeleted', notice: 'Person restored from the recycle bin.', unavailable: 'Undeleting is not available.', run: client => client.undeletePerson?.bind(client)},
};

export function PersonDetail({personId, onNavigate, workspaceId, client, permissions = defaultPermissions, pluginEnabled = true, forcedState, contributions = [], enabledContributionPlugins, formFactor}: Props) {
  const responsiveFormFactor = useResponsiveFormFactor();
  const effectiveFormFactor = formFactor ?? responsiveFormFactor;
  const workspaceMatches = workspaceId === client.workspaceId;
  const [load, setLoad] = useState<RecordLoad>(() => {
    const seeded = workspaceMatches ? client.seeds?.person?.(personId) : undefined;
    if (!seeded) return {status: 'loading'};
    return 'value' in seeded ? {status: 'ready', record: seeded.value} : loadFailure(seeded.error);
  });
  const [versionChanged, setVersionChanged] = useState(false);
  const [notice, setNotice] = useState('');
  const configuredCustomFields = usePersonFieldNames(client, permissions);
  const readable = pluginEnabled && workspaceMatches && permissions.has(contactsPersonPermissions.read);
  const [extensionLoad, reloadExtensions] = useExtensionFields(client, personResource, readable);
  const canArrange = permissions.has(arrangeFieldsPermission) && typeof client.loadFieldLayout === 'function' && typeof client.saveFieldLayout === 'function';
  const [arranging, setArranging] = useState<{status: 'loading' | 'ready' | 'error'; state?: FieldLayoutState} | null>(null);
  const [activity, setActivity] = useState<ActivityEntry[]>(() => {
    const seeded = workspaceMatches ? client.seeds?.personActivity?.(personId) : undefined;
    return seeded && 'value' in seeded ? seeded.value : [];
  });
  const [activityKey, setActivityKey] = useState(0);
  // After a write the timeline consumer projects the change asynchronously.
  // Re-read with backoff until an entry for the written version appears, at
  // most once per delay; after the last delay the timeline stays as it is.
  const [activityRefresh, setActivityRefresh] = useState<ActivityRefresh | null>(null);

  useEffect(() => {
    if (!pluginEnabled || !workspaceMatches || !permissions.has(contactsPersonPermissions.read)) return;
    const controller = new AbortController();
    client.listPersonActivity(personId, controller.signal)
      .then(entries => {
        if (controller.signal.aborted) return;
        setActivity(entries);
        setActivityRefresh(current => nextActivityRefresh(current, entries));
      })
      .catch(() => undefined);
    return () => controller.abort();
  }, [activityKey, client, permissions, personId, pluginEnabled, workspaceMatches]);

  useEffect(() => {
    if (!activityRefresh) return;
    const delay = activityRefreshDelays[activityRefresh.attempt];
    if (delay === undefined) {
      setActivityRefresh(null);
      return;
    }
    const timer = globalThis.setTimeout(() => {
      setActivityRefresh(current => current ? {...current, attempt: current.attempt + 1} : null);
      setActivityKey(value => value + 1);
    }, delay);
    return () => globalThis.clearTimeout(timer);
  }, [activityRefresh]);

  useEffect(() => {
    if (!pluginEnabled || !workspaceMatches || !permissions.has(contactsPersonPermissions.read)) return;
    const controller = new AbortController();
    client.getPerson(personId, controller.signal)
      .then(record => { if (!controller.signal.aborted) setLoad({status: 'ready', record}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(loadFailure(error)); });
    return () => controller.abort();
  }, [client, permissions, personId, pluginEnabled, workspaceMatches]);

  const person = load.status === 'ready' ? load.record : undefined;
  const state: PersonDetailState = forcedState ?? (
    !pluginEnabled ? 'disabled'
      : !workspaceMatches ? 'workspace-changed'
        : load.status !== 'ready' ? load.status
          : versionChanged ? 'version-changed'
            : person?.archived ? 'archived'
              : 'ready');
  const archived = state === 'archived' || (state === 'version-changed' && Boolean(person?.archived));
  const canRestore = typeof client.restorePerson === 'function';
  const metadata = useMemo(() => resolveContactsPersonDetail({person, state, permissions, contributions, enabledContributionPlugins, configuredCustomFields, canRestore, activity, notice, extensions: extensionLoad.set}), [activity, canRestore, configuredCustomFields, contributions, enabledContributionPlugins, extensionLoad.set, notice, permissions, person, state]);
  const adapter = useMemo<MetadataInteractionAdapter>(() => ({
    navigate: (target, parameters) => {
      if (target === 'contacts.people.list') onNavigate('/contacts');
      if (target === 'contacts.people.edit') onNavigate(`/contacts/people/${encodeURIComponent(String(parameters.id))}/edit`);
      if (target === userRecordTarget) onNavigate(userRecordRoute(String(parameters.id)));
    },
    execute: async request => {
      const operation = lifecycleOperations[request.action.id];
      if (!operation) return {status: 'failure', category: 'unavailable', message: 'This action is not available.'};
      if (!person) return {status: 'failure', category: 'unavailable', message: 'This person is not loaded.'};
      if (operation.confirm && typeof window !== 'undefined' && !window.confirm(operation.confirm(person.name))) {
        setNotice(operation.cancelled ?? '');
        return {status: 'success'};
      }
      const version = Number(request.etag ?? person.version);
      try {
        const run = operation.run(client);
        const next = run ? await run(person.id, version) : undefined;
        if (!next) return {status: 'failure', category: 'unavailable', message: operation.unavailable};
        setLoad({status: 'ready', record: next});
        setVersionChanged(false);
        setNotice(operation.notice);
        setActivityRefresh({version: next.version, attempt: 0});
        return {status: 'success', etag: String(next.version)};
      } catch (error) {
        const described = describeRecordOperationError(error, operation.past);
        if (described.missing) setLoad({status: 'missing'});
        if (described.versionChanged) {
          setVersionChanged(true);
          client.getPerson(person.id).then(latest => setLoad({status: 'ready', record: latest})).catch(() => undefined);
        }
        setNotice(described.result.status === 'failure' ? described.result.message : '');
        return described.result;
      }
    },
  }), [client, onNavigate, person]);

  const openArrangement = () => {
    if (!client.loadFieldLayout) return;
    setArranging({status: 'loading'});
    client.loadFieldLayout(personResource)
      .then(layout => setArranging({status: 'ready', state: layout}))
      .catch(() => setArranging({status: 'error'}));
  };
  const showArrange = canArrange && Boolean(person) && (state === 'ready' || state === 'archived' || state === 'version-changed');

  return <div className="content-canvas person-detail-page">
    {showArrange && !arranging && <div className="person-detail-arrange"><Button variant="quiet" size="small" onClick={openArrangement}>Arrange fields</Button></div>}
    {showArrange && arranging && <FieldArrangement
      recordNoun="person"
      status={arranging.status}
      zones={arranging.state?.zones ?? []}
      fields={arranging.state?.fields ?? []}
      placements={arranging.state?.placements ?? []}
      hostFields={contactsZoneHostFields(arranging.state?.zones ?? [])}
      onSave={async layout => {
        const result = await savePersonFieldLayout(client, arranging.state?.revision ?? 0, layout);
        if (result.status === 'saved') reloadExtensions();
        return result;
      }}
      onClose={() => setArranging(null)}
    />}
    <MetadataPageRenderer
      contextValues={{record_id: personId, archived, deleted: Boolean(person?.deleted), version_changed: state === 'version-changed'}}
      dataSources={metadata.dataSources}
      embedded
      embeddedHeadingOffset={0}
      etag={person ? String(person.version) : undefined}
      focusOnRender
      formFactor={effectiveFormFactor}
      host="internal-web"
      interactionAdapter={adapter}
      mode="view"
      page={metadata.page}
      permissions={permissions}
      recordId={personId}
      resources={metadata.resources}
    />
  </div>;
}
