import React, {useEffect, useMemo, useRef, useState, type RefObject} from 'react';
import {Alert, Badge, Button, EmptyState, Modal, Select, Skeleton, TextField} from '../components/ui';
import {formatRecordTimestamp} from '../plugin-ui/standard-fields/model';
import {formatValue, holdsReference, isReference, ObjectsApiError, type ObjectDescription, type ObjectField, type ObjectRecord, type ObjectsClient, type QuickAction, type RecordLink, type RecordListStatus, type RecordPage, type RelatedList, type ValidationIssue} from './api';
import {SharingPanel} from './SharingPanel';
import {HistoryPanel} from './HistoryPanel';
import {ActivityTimeline} from './ActivityTimeline';
import {BlockedDraft, CancelChoice, CreateDraftChooser, DraftMessages, DraftStatus, formatDraftTime, RestorePrompt} from '../drafts/components';
import {changedFields} from '../drafts/model';
import {createDraftsFor, editDraftFor, useDraftList} from '../drafts/lists';
import {useDrafts} from '../drafts/runtime';
import {useFormDraft, type Restored} from '../drafts/useFormDraft';
import type {DraftDocument, DraftKey, DraftParent, StoredDraft} from '../drafts/types';
import {editableFields, toApiValue, toDraftValue, type DraftValue} from './formValues';
import './objects.css';

export {editableFields, toApiValue, toDraftValue, type DraftValue} from './formValues';

type Navigate = (route: string) => void;

export type Load<T> = {status: 'loading'} | {status: 'ready'; value: T} | {status: 'missing'} | {status: 'denied'} | {status: 'unavailable'};

export function failed(error: unknown): Load<never> {
  if (error instanceof ObjectsApiError && error.reason === 'missing') return {status: 'missing'};
  if (error instanceof ObjectsApiError && error.reason === 'denied') return {status: 'denied'};
  return {status: 'unavailable'};
}

export const objectRoute = (apiName: string) => `/o/${encodeURIComponent(apiName)}`;
export const recordRoute = (apiName: string, id: string) => `${objectRoute(apiName)}/${encodeURIComponent(id)}`;

/** Describes an object once per page; every engine page starts here. */
export function useObject(client: ObjectsClient, apiName: string): [Load<ObjectDescription>, () => void] {
  const [load, setLoad] = useState<Load<ObjectDescription>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.describe(apiName, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(failed(error)); });
    return () => controller.abort();
  }, [client, apiName, attempt]);
  return [load, () => setAttempt(value => value + 1)];
}

function PageHeader({breadcrumb, title, headingRef, badge, children}: {breadcrumb: string; title: string; headingRef?: RefObject<HTMLHeadingElement | null>; badge?: React.ReactNode; children?: React.ReactNode}) {
  return <header className="page-header">
    <div><p className="page-header__breadcrumb">{breadcrumb}</p><h1 ref={headingRef} tabIndex={-1}>{title}</h1>{badge}</div>
    {children && <div className="object-page__actions">{children}</div>}
  </header>;
}

function LoadState({load, noun, onRetry}: {load: Load<unknown>; noun: string; onRetry: () => void}) {
  if (load.status === 'loading') return <Skeleton label={`Loading ${noun}`} lines={4} />;
  if (load.status === 'missing') return <Alert variant="warning" title={`${noun[0]?.toUpperCase() ?? ''}${noun.slice(1)} not available`}>It may have been deleted, or you may not have access to it.</Alert>;
  if (load.status === 'denied') return <Alert variant="warning" title="Permission needed">You do not have permission to open this {noun}. Ask a workspace administrator to review your access.</Alert>;
  if (load.status === 'unavailable') return <div className="object-page__state"><Alert variant="danger" title="Something went wrong">The {noun} could not be loaded. Try again in a moment.</Alert><Button variant="secondary" onClick={onRetry}>Try again</Button></div>;
  return null;
}

/** Object home: the record list for one engine object. */
export function ObjectHome({client, apiName, onNavigate, headingRef}: {client: ObjectsClient; apiName: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [objectLoad, retryObject] = useObject(client, apiName);
  const [search, setSearch] = useState('');
  const [submitted, setSubmitted] = useState('');
  const [bin, setBin] = useState(false);
  // Which live records the list shows; the recycle bin is its own view.
  const [shown, setShown] = useState<Exclude<RecordListStatus, 'deleted'>>('active');
  const [cursor, setCursor] = useState<string | undefined>();
  const [page, setPage] = useState<Load<RecordPage>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [notice, setNotice] = useState('');
  const [creating, setCreating] = useState(false);
  const object = objectLoad.status === 'ready' ? objectLoad.value : undefined;

  useEffect(() => {
    if (!object) return;
    const controller = new AbortController();
    setPage({status: 'loading'});
    client.listRecords(object.apiName, {search: submitted || undefined, status: bin ? 'deleted' : shown === 'active' ? undefined : shown, sort: 'name', direction: 'asc', cursor, limit: 50}, controller.signal)
      .then(value => { if (!controller.signal.aborted) setPage({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setPage(failed(error)); });
    return () => controller.abort();
  }, [client, object, submitted, bin, shown, cursor, attempt]);

  useEffect(() => { if (object) document.title = `${object.pluralLabel} - Limiara`; }, [object]);

  const columns = useMemo(() => object?.fields.slice(0, 4) ?? [], [object]);
  const restore = async (record: ObjectRecord) => {
    if (!object) return;
    try {
      await client.undeleteRecord(object.apiName, record.id, record.version);
      setNotice(`${record.name} was restored.`);
      setAttempt(value => value + 1);
    } catch (error) {
      setNotice(error instanceof ObjectsApiError && error.code === 'records.duplicate_value'
        ? `${record.name} could not be restored because another record now uses one of its unique values.`
        : `${record.name} could not be restored. ${error instanceof Error ? error.message : ''}`);
    }
  };

  if (!object) return <div className="content-canvas object-page"><PageHeader breadcrumb="Objects" title="Records" headingRef={headingRef} /><LoadState load={objectLoad} noun="object" onRetry={retryObject} /></div>;

  return <div className="content-canvas object-page">
    <PageHeader breadcrumb={`Objects / ${object.label}`} title={bin ? `${object.pluralLabel} recycle bin` : object.pluralLabel} headingRef={headingRef}>
      {object.access.create && !bin && <Button aria-haspopup="dialog" onClick={() => setCreating(true)}>New {object.label.toLowerCase()}</Button>}
    </PageHeader>
    {creating && <RecordModal client={client} object={object} onNavigate={onNavigate} onClose={() => setCreating(false)} onSaved={(saved, next) => {
      if (next === 'close') onNavigate(recordRoute(object.apiName, saved.id));
      else setAttempt(value => value + 1);
    }} />}
    <div className="sr-only" aria-live="polite">{notice}</div>
    {notice && <Alert variant="info" title={notice} />}
    <form className="object-toolbar" role="search" onSubmit={event => { event.preventDefault(); setCursor(undefined); setSubmitted(search.trim()); }}>
      <TextField label={`Search ${object.pluralLabel.toLowerCase()}`} hideLabel placeholder={`Search ${object.pluralLabel.toLowerCase()}`} value={search} maxLength={100} onChange={event => setSearch(event.target.value)} />
      <Button type="submit" variant="secondary">Search</Button>
      {!bin && <Select label="Show" value={shown} onChange={event => { setCursor(undefined); setShown(event.target.value as typeof shown); }}>
        <option value="active">Active records</option>
        <option value="archived">Archived records</option>
        <option value="all">All records</option>
      </Select>}
      {object.access.delete && <Button type="button" variant="quiet" aria-pressed={bin} onClick={() => { setCursor(undefined); setBin(value => !value); }}>{bin ? 'Back to records' : 'Recycle bin'}</Button>}
    </form>
    {page.status !== 'ready' && <LoadState load={page} noun="record list" onRetry={() => setAttempt(value => value + 1)} />}
    {page.status === 'ready' && page.value.records.length === 0 && <EmptyState title={bin ? 'The recycle bin is empty' : submitted ? 'No matching records' : shown === 'archived' ? `No archived ${object.pluralLabel.toLowerCase()}` : `No ${object.pluralLabel.toLowerCase()} yet`} description={bin ? 'Deleted records appear here until they are restored or purged.' : submitted ? 'Try a different search.' : shown === 'archived' ? 'Archived records appear here. They stay readable but leave lists and search.' : object.access.create ? `Create the first ${object.label.toLowerCase()} to get started.` : 'Records you can see will appear here.'} />}
    {page.status === 'ready' && page.value.records.length > 0 && <div className="object-table-wrap">
      <table className="object-table">
        <caption className="sr-only">{object.pluralLabel}</caption>
        <thead><tr><th scope="col">{object.nameLabel}</th>{columns.map(field => <th scope="col" key={field.apiName}>{field.label}</th>)}<th scope="col">{bin ? 'Deleted' : 'Last modified'}</th>{bin && <th scope="col"><span className="sr-only">Actions</span></th>}</tr></thead>
        <tbody>{page.value.records.map(record => <tr key={record.id}>
          <th scope="row">{bin ? record.name : <a href={recordRoute(object.apiName, record.id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(object.apiName, record.id)); }}>{record.name}</a>}{record.archivedAt && <> <Badge variant="warning">Archived</Badge></>}</th>
          {columns.map(field => <td key={field.apiName}>{formatValue(field, record.values[field.apiName], record.related)}</td>)}
          <td>{formatRecordTimestamp(record.updatedAt)}</td>
          {bin && <td><Button size="small" variant="secondary" onClick={() => void restore(record)}>Restore</Button></td>}
        </tr>)}</tbody>
      </table>
    </div>}
    {page.status === 'ready' && (cursor || page.value.nextCursor) && <div className="object-pager">
      {cursor && <Button variant="secondary" onClick={() => setCursor(undefined)}>First page</Button>}
      {page.value.nextCursor && <Button variant="secondary" onClick={() => setCursor(page.value.nextCursor ?? undefined)}>Next page</Button>}
    </div>}
  </div>;
}

function UserLink({user, onNavigate}: {user: ObjectRecord['owner']; onNavigate: Navigate}) {
  if (!user) return <>Not recorded</>;
  const route = `/users/${encodeURIComponent(user.id)}`;
  return <a href={route} onClick={event => { event.preventDefault(); onNavigate(route); }}>{user.name ?? 'Workspace member'}</a>;
}

function FieldValue({field, value, related, relatedObjects = {}, onNavigate}: {field: ObjectField; value: unknown; related: Record<string, string>; relatedObjects?: Record<string, string>; onNavigate: Navigate}) {
  const text = formatValue(field, value, related);
  if (!text) return <span className="object-value--empty">Not set</span>;
  // A related-to value names its object only when the caller can read the target.
  const target = typeof value === 'string' ? (field.type === 'related_to' ? relatedObjects[value] : isReference(field.type) ? field.referenceObject : null) : null;
  if (target && typeof value === 'string') {
    const route = recordRoute(target, value);
    return <a href={route} onClick={event => { event.preventDefault(); onNavigate(route); }}>{text}</a>;
  }
  if (field.type === 'email') return <a href={`mailto:${text}`}>{text}</a>;
  if (field.type === 'url' && /^https?:\/\//i.test(text)) return <a href={text} rel="noreferrer noopener" target="_blank">{text}</a>;
  return <>{text}</>;
}

/** One record of an engine object, with inline edit and delete. */
export function ObjectRecordPage({client, apiName, recordId, onNavigate, headingRef, draftId}: {client: ObjectsClient; apiName: string; recordId: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>; draftId?: string}) {
  const [objectLoad, retryObject] = useObject(client, apiName);
  const [load, setLoad] = useState<Load<ObjectRecord>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState('');
  const [archiving, setArchiving] = useState(false);
  const [notice, setNotice] = useState('');
  const [running, setRunning] = useState<QuickAction | null>(null);
  const [relatedRefresh, setRelatedRefresh] = useState(0);
  const object = objectLoad.status === 'ready' ? objectLoad.value : undefined;
  const actions = useQuickActions(client, apiName);
  // A draft of this record (ADR 0036 §8): a quiet notice, or opened straight from Drafts.
  const [draftsSeen, setDraftsSeen] = useState(0);
  const drafts = useDraftList(draftsSeen);
  const [resume, setResume] = useState<StoredDraft | undefined>();
  const opening = useRef(draftId);

  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.getRecord(apiName, recordId, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(reason => { if (!controller.signal.aborted) setLoad(failed(reason)); });
    return () => controller.abort();
  }, [client, apiName, recordId, attempt]);

  const record = load.status === 'ready' ? load.value : undefined;
  useEffect(() => { if (record) document.title = `${record.name} - Limiara`; }, [record]);
  // Open from Drafts: an edit draft opens Edit, an action draft its action, or Edit when the action is gone (ADR 0036 §5).
  useEffect(() => {
    const wanted = opening.current;
    if (!wanted || !record || !drafts.loaded) return;
    const found = drafts.drafts.find(item => item.meta.id === wanted);
    opening.current = undefined;
    if (!found) return;
    const key = found.document.key;
    const action = key.kind === 'action' ? actions.list.find(item => item.name === key.action && item.kind === 'update') : undefined;
    setResume(found);
    if (action) setRunning(action);
    else setEditing(true);
  }, [record, drafts, actions.list]);

  if (!object || !record) {
    const pending = !object ? objectLoad : load;
    return <div className="content-canvas object-page"><PageHeader breadcrumb={object ? `Objects / ${object.label}` : 'Objects'} title={object?.label ?? 'Record'} headingRef={headingRef} /><LoadState load={pending} noun={!object ? 'object' : 'record'} onRetry={!object ? retryObject : () => setAttempt(value => value + 1)} /></div>;
  }

  const remove = async () => {
    try {
      await client.deleteRecord(object.apiName, record.id, record.version);
      onNavigate(objectRoute(object.apiName));
    } catch (reason) {
      setConfirmDelete(false);
      setError(reason instanceof ObjectsApiError && reason.code === 'records.delete_restricted'
        ? 'This record cannot be deleted while other records point to it.'
        : reason instanceof Error ? reason.message : 'The record could not be deleted.');
    }
  };

  // Archiving is an edit (ADR 0035 G1): it moves the version, so the page takes the saved record.
  const toggleArchive = async () => {
    const archive = record.archivedAt === null;
    setError('');
    setNotice('');
    setArchiving(true);
    try {
      const saved = archive
        ? await client.archiveRecord(object.apiName, record.id, record.version)
        : await client.unarchiveRecord(object.apiName, record.id, record.version);
      setLoad({status: 'ready', value: {...saved, rights: saved.rights ?? record.rights}});
      setNotice(archive ? `${record.name} was archived.` : `${record.name} was restored from the archive.`);
    } catch (reason) {
      setError(archiveProblem(reason, archive));
    } finally {
      setArchiving(false);
    }
  };

  // Record-level rights (ADR 0016) refine the object's permissions.
  const canEdit = record.rights ? record.rights.edit : object.access.edit;
  const canDelete = record.rights ? record.rights.delete : object.access.delete;
  // An action grants nothing (ADR 0032 §3): it shows only where the member could do the same through the full form.
  const available = actions.list.filter(action => action.kind === 'update' ? canEdit : actionTarget(action, actions.targets)?.access.create === true);
  const runningTarget = running?.kind === 'create' ? actionTarget(running, actions.targets) : undefined;
  const editDraft = editDraftFor(drafts.drafts, object.apiName, record.id);
  const closeForm = () => { setResume(undefined); setDraftsSeen(value => value + 1); };

  return <div className="content-canvas object-page">
    <PageHeader breadcrumb={`Objects / ${object.label}`} title={record.name} headingRef={headingRef} badge={record.archivedAt && <Badge variant="warning">Archived</Badge>}>
      {available.map(action => <Button key={action.name} variant="secondary" aria-haspopup="dialog" onClick={() => { setError(''); setRunning(action); }}>{action.label}</Button>)}
      {canEdit && <Button variant="secondary" aria-haspopup="dialog" onClick={() => { setError(''); setEditing(true); }}>Edit</Button>}
      {canEdit && <Button variant="secondary" disabled={archiving} aria-busy={archiving || undefined} onClick={() => void toggleArchive()}>{record.archivedAt ? 'Unarchive' : 'Archive'}</Button>}
      {canDelete && <Button variant="danger" onClick={() => setConfirmDelete(true)}>Delete</Button>}
    </PageHeader>
    <div className="sr-only" aria-live="polite">{notice}</div>
    {error && <Alert variant="danger" title={error} />}
    <ArchivedNotice record={record} />
    {editDraft && !editing && canEdit && <p className="draft-notice">Unsaved draft from {formatDraftTime(editDraft.meta.changedAt)} <Button size="small" variant="secondary" aria-haspopup="dialog" onClick={() => { setError(''); setEditing(true); }}>Continue editing</Button></p>}
    {confirmDelete && <div className="object-confirm" role="alertdialog" aria-labelledby="object-confirm-title">
      <p id="object-confirm-title"><strong>Delete {record.name}?</strong> It moves to the recycle bin, where it can be restored.</p>
      <div className="object-page__actions"><Button variant="danger" onClick={() => void remove()} autoFocus>Delete</Button><Button variant="secondary" onClick={() => setConfirmDelete(false)}>Cancel</Button></div>
    </div>}
    {editing && <RecordModal client={client} object={object} record={record} resume={resume} onNavigate={onNavigate} onClose={() => { setEditing(false); closeForm(); }} onSaved={(saved, next) => {
      // The first save is this record's edit; after Save & New, saves create others.
      if (saved.id === record.id) setLoad({status: 'ready', value: {...saved, rights: saved.rights ?? record.rights}});
      else if (next === 'close') onNavigate(recordRoute(object.apiName, saved.id));
      if (next === 'close') { setEditing(false); closeForm(); }
    }} />}
    {running?.kind === 'update' && <RecordModal client={client} object={object} record={record} title={running.label} only={running.fields} always={running.presets} offerNew={false} draftContext={{action: {name: running.name, label: running.label}}} resume={resume} onNavigate={onNavigate} onClose={() => { setRunning(null); closeForm(); }} onSaved={saved => {
      setLoad({status: 'ready', value: {...saved, rights: saved.rights ?? record.rights}});
      setRunning(null);
      closeForm();
    }} />}
    {running?.kind === 'create' && runningTarget && running.targetField && <RecordModal client={client} object={runningTarget} title={running.label} presets={{[running.targetField]: {value: record.id, label: record.name}}} only={running.fields} always={running.presets} draftContext={{parent: {object: object.apiName, id: record.id, field: running.targetField, name: record.name}, action: {name: running.name, label: running.label}}} onNavigate={onNavigate}
      // As with New on a related list, the member stays on this record and its lists show what they added.
      onClose={() => setRunning(null)} onSaved={(_, next) => { setRelatedRefresh(value => value + 1); if (next === 'close') setRunning(null); }} />}
    <section aria-label="Details" className="object-section">
      <h2>Details</h2>
      <dl className="object-fields">
        <div><dt>{object.nameLabel}</dt><dd>{record.name}</dd></div>
        {object.fields.map(field => <div key={field.apiName}><dt>{field.label}</dt><dd><FieldValue field={field} value={record.values[field.apiName]} related={record.related} relatedObjects={record.relatedObjects} onNavigate={onNavigate} /></dd></div>)}
      </dl>
    </section>
    <RelatedLists client={client} apiName={object.apiName} recordId={record.id} recordName={record.name} refresh={relatedRefresh} onNavigate={onNavigate} />
    {/* Its actions reuse the quick-action modal above; a save refreshes it with the related lists. */}
    <ActivityTimeline client={client} apiName={object.apiName} recordId={record.id} recordName={record.name} actions={available} refresh={relatedRefresh} onRun={action => { setError(''); setRunning(action); }} onNavigate={onNavigate} />
    <section aria-label="System information" className="object-section">
      <h2>System information</h2>
      <dl className="object-fields">
        <div><dt>Owner</dt><dd><UserLink user={record.owner} onNavigate={onNavigate} /></dd></div>
        <div><dt>Created by</dt><dd><UserLink user={record.createdBy} onNavigate={onNavigate} />, {formatRecordTimestamp(record.createdAt)}</dd></div>
        <div><dt>Last modified by</dt><dd><UserLink user={record.updatedBy} onNavigate={onNavigate} />, {formatRecordTimestamp(record.updatedAt)}</dd></div>
        <div><dt>Record ID</dt><dd><code>{record.id}</code></dd></div>
      </dl>
    </section>
    {/* After a transfer the caller may no longer see the record; reload to find out. */}
    <SharingPanel client={client} object={object} record={record} onTransferred={() => setAttempt(value => value + 1)} />
    <HistoryPanel client={client} object={object} record={record} onNavigate={onNavigate} />
  </div>;
}

/** What the member reads when archiving or unarchiving fails. */
export function archiveProblem(reason: unknown, archive: boolean): string {
  const code = reason instanceof ObjectsApiError ? reason.code : '';
  if (code === 'records.already_archived' || code === 'records.not_archived' || code === 'records.version_conflict') return 'This record changed since it was opened. Reload it and try again.';
  if (code === 'records.deleted') return 'This record is in the recycle bin. Restore it first.';
  if (reason instanceof ObjectsApiError && reason.reason === 'denied') return 'You do not have permission to change this record.';
  return archive ? 'The record could not be archived. Try again in a moment.' : 'The record could not be restored from the archive. Try again in a moment.';
}

/** The archived state of a record (ADR 0035 G1): readable by id, left out of lists. */
export function ArchivedNotice({record}: {record: ObjectRecord}) {
  if (!record.archivedAt) return null;
  return <Alert variant="info" title="This record is archived">Archived {formatRecordTimestamp(record.archivedAt)}. It stays readable here but is left out of lists, search, lookup pickers and related lists.</Alert>;
}

/** The child object a create action makes records of, when the member's description of it names the action's reference field. */
function actionTarget(action: QuickAction, targets: Record<string, ObjectDescription>): ObjectDescription | undefined {
  const target = action.targetObject ? targets[action.targetObject] : undefined;
  return target?.fields.some(field => field.apiName === action.targetField) ? target : undefined;
}

/**
 * An object's quick actions in position order (ADR 0032), with the child
 * objects of its create actions described to learn whether the member may
 * create them. A failure leaves the page without actions.
 */
function useQuickActions(client: ObjectsClient, apiName: string): {list: QuickAction[]; targets: Record<string, ObjectDescription>} {
  const [list, setList] = useState<QuickAction[]>([]);
  const [targets, setTargets] = useState<Record<string, ObjectDescription>>({});
  useEffect(() => {
    const controller = new AbortController();
    setList([]);
    setTargets({});
    client.quickActions(apiName, controller.signal)
      .then(actions => {
        if (controller.signal.aborted) return;
        setList([...actions].sort((left, right) => left.position - right.position || left.name.localeCompare(right.name)));
        for (const target of new Set(actions.flatMap(action => action.kind === 'create' && action.targetObject ? [action.targetObject] : []))) {
          client.describe(target, controller.signal)
            .then(description => { if (!controller.signal.aborted) setTargets(known => ({...known, [target]: description})); })
            // Without a description the action is not offered.
            .catch(() => undefined);
        }
      })
      .catch(() => undefined);
    return () => controller.abort();
  }, [client, apiName]);
  return {list, targets};
}

/** The records of other objects that point at this one (ADR 0028), each list showing only what the member may see. */
export function RelatedLists({client, apiName, recordId, recordName, refresh = 0, onNavigate}: {client: ObjectsClient; apiName: string; recordId: string; recordName?: string; refresh?: number; onNavigate: Navigate}) {
  const [load, setLoad] = useState<Load<RelatedList[]>>({status: 'loading'});
  const [more, setMore] = useState<Record<string, 'loading' | 'error'>>({});
  const [ownAttempt, setAttempt] = useState(0);
  // A save elsewhere on the page, such as a quick action, refreshes the lists too.
  const attempt = ownAttempt + refresh;
  // The child objects, described to learn whether the member may create them (ADR 0032 §1).
  const [children, setChildren] = useState<Record<string, ObjectDescription>>({});
  const [creating, setCreating] = useState<RelatedList | null>(null);
  const [refreshFailed, setRefreshFailed] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    if (attempt === 0) setLoad({status: 'loading'});
    client.relatedLists(apiName, recordId, controller.signal)
      .then(value => {
        if (controller.signal.aborted) return;
        setLoad({status: 'ready', value});
        setRefreshFailed(false);
        for (const object of new Set(value.map(list => list.object))) {
          client.describe(object, controller.signal)
            .then(description => { if (!controller.signal.aborted) setChildren(known => ({...known, [object]: description})); })
            // Without a description the list simply offers no New.
            .catch(() => undefined);
        }
      })
      .catch(reason => {
        if (controller.signal.aborted) return;
        // A refresh after a save keeps what is shown, and any open form with it.
        if (attempt > 0) setRefreshFailed(true);
        else setLoad(failed(reason));
      });
    return () => controller.abort();
  }, [client, apiName, recordId, attempt]);
  if (load.status === 'loading') return <section aria-label="Related" className="object-section"><h2>Related</h2><Skeleton lines={3} /></section>;
  if (load.status !== 'ready') return <section aria-label="Related" className="object-section"><h2>Related</h2><p className="object-form__meta">Related records could not be loaded.</p></section>;
  if (load.value.length === 0) return null;
  const key = (list: RelatedList) => `${list.object}/${list.field}`;
  // Two lists from one object are told apart by the field they use.
  const repeated = new Set(load.value.map(list => list.object).filter((object, index, all) => all.indexOf(object) !== index));
  const showMore = async (list: RelatedList) => {
    if (!list.nextCursor) return;
    setMore(state => ({...state, [key(list)]: 'loading'}));
    try {
      const next = await client.relatedRecords(apiName, recordId, list.object, list.field, list.nextCursor);
      setLoad(state => state.status === 'ready' ? {status: 'ready', value: state.value.map(item => key(item) === key(list) ? {...item, records: [...item.records, ...next.records], nextCursor: next.nextCursor} : item)} : state);
      setMore(state => { const {[key(list)]: _, ...rest} = state; return rest; });
    } catch {
      setMore(state => ({...state, [key(list)]: 'error'}));
    }
  };
  return <section aria-label="Related" className="object-section">
    <h2>Related</h2>
    {load.value.map(list => {
      const headingId = `related-${list.object}-${list.field}`.replace(/[^a-zA-Z0-9_-]/g, '-');
      return <section key={key(list)} className="object-related" aria-labelledby={headingId}>
        <div className="object-related__header">
          <h3 id={headingId}>{list.objectPluralLabel}{repeated.has(list.object) ? ` (${list.fieldLabel})` : ''}</h3>
          {children[list.object]?.access.create && <Button size="small" variant="secondary" aria-haspopup="dialog" aria-label={`New ${children[list.object]?.label.toLowerCase()}${repeated.has(list.object) ? ` (${list.fieldLabel})` : ''}`} onClick={() => setCreating(list)}>New</Button>}
        </div>
        {list.records.length === 0
          ? <p className="object-form__meta">No {list.objectPluralLabel.toLowerCase()} you can see.</p>
          : <ul className="object-related__list">{list.records.map(item => <li key={item.id}><a href={recordRoute(list.object, item.id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(list.object, item.id)); }}>{item.name}</a></li>)}</ul>}
        {list.nextCursor && <Button variant="secondary" loading={more[key(list)] === 'loading'} onClick={() => void showMore(list)}>Show more</Button>}
        {more[key(list)] === 'error' && <Alert variant="danger" title={`More ${list.objectPluralLabel.toLowerCase()} could not be loaded. Try again.`} />}
      </section>;
    })}
    {refreshFailed && <Alert variant="warning" title="The related lists could not be refreshed. Reload the page to see the latest records." />}
    {creating && children[creating.object] && <RecordModal client={client} object={children[creating.object] as ObjectDescription} presets={{[creating.field]: {value: recordId, label: recordName ?? recordId}}} draftContext={{parent: {object: apiName, id: recordId, field: creating.field, name: recordName ?? recordId}}} onNavigate={onNavigate}
      // The member stays on this record; the list shows what they added.
      onClose={() => setCreating(null)} onSaved={(_, next) => { setAttempt(value => value + 1); if (next === 'close') setCreating(null); }} />}
  </section>;
}

/** Tracks Save & New: each new record gets a fresh form, and the last one saved is announced. */
function useSaveAndNew() {
  const [form, setForm] = useState(0);
  const [created, setCreated] = useState<ObjectRecord | null>(null);
  return {form, created, next: (saved: ObjectRecord) => { setCreated(saved); setForm(value => value + 1); }, reset: () => setCreated(null)};
}

function Created({object, record, onNavigate}: {object: ObjectDescription; record: ObjectRecord; onNavigate: Navigate}) {
  return <Alert variant="success" title={`${record.name} was saved.`}>
    Enter the next {object.label.toLowerCase()}, or <a href={recordRoute(object.apiName, record.id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(object.apiName, record.id)); }}>open {record.name}</a>.
  </Alert>;
}

/** Where a form's draft belongs (ADR 0036 §2): the parent a new record starts under, and the quick action it runs. */
export type DraftContext = {parent?: DraftParent & {name: string}; action?: {name: string; label: string}};

/** The draft key of one form: edits per record (and action), each new-record form its own draft. */
export function formDraftKey(object: ObjectDescription, record: ObjectRecord | undefined, context: DraftContext, newDraft: string): DraftKey {
  if (record) return context.action ? {kind: 'action', object: object.apiName, record: record.id, action: context.action.name} : {kind: 'edit', object: object.apiName, record: record.id};
  const parent = context.parent ? {object: context.parent.object, id: context.parent.id, field: context.parent.field} : undefined;
  return {kind: 'create', object: object.apiName, draft: newDraft, parent, action: context.action?.name};
}

/** New on an object with unsaved new-record drafts first asks which to continue (ADR 0036 §8). */
function useCreateChooser(object: ObjectDescription, creating: boolean, context: DraftContext, resume?: StoredDraft) {
  const runtime = useDrafts();
  const list = useDraftList();
  const [choice, setChoice] = useState<'pending' | 'new' | StoredDraft>(resume ?? 'pending');
  const parent = context.parent ? {object: context.parent.object, id: context.parent.id, field: context.parent.field} : undefined;
  const drafts = creating && runtime?.status === 'ready' ? createDraftsFor(list.drafts, object.apiName, parent, context.action?.name) : [];
  // Offered above a fresh form, so the form is never replaced while the list loads or while the member types.
  const offering = choice === 'pending' && creating && runtime?.status === 'ready' && list.loaded && drafts.length > 0;
  return {
    offering,
    drafts,
    resume: typeof choice === 'object' ? choice : undefined,
    expiryDays: runtime?.contribution.expiryDays ?? 7,
    choose: (next: 'new' | StoredDraft) => setChoice(next),
  };
}

/**
 * Create or edit in a modal over the current page. Save opens the record
 * (or, when editing, shows it updated); Save & New keeps the modal open on
 * a blank form for the next record.
 */
export function RecordModal({client, object, record, presets, title, only, always, offerNew, onClose, onSaved, onNavigate, draftContext = {}, resume}: {client: ObjectsClient; object: ObjectDescription; record?: ObjectRecord; presets?: Presets; title?: string; only?: readonly string[]; always?: Record<string, unknown>; offerNew?: boolean; onClose: () => void; onSaved: (record: ObjectRecord, next: AfterSave) => void; onNavigate: Navigate; draftContext?: DraftContext; resume?: StoredDraft}) {
  const sequence = useSaveAndNew();
  // After Save & New on an edit, the modal goes on to create a record.
  const [editing, setEditing] = useState(record);
  // A save in flight keeps the modal open, so its result is never lost.
  const [saving, setSaving] = useState(false);
  // Cancel and Escape go through the form, which may ask about its draft.
  const cancel = useRef<(() => void) | null>(null);
  const chooser = useCreateChooser(object, !editing && sequence.form === 0, draftContext, resume);
  const newDraft = useMemo(() => crypto.randomUUID(), [sequence.form]);
  const noun = object.label.toLowerCase();
  // An edit that became a create after Save & New starts its drafts afresh, without the action.
  const context = editing || sequence.form === 0 ? draftContext : {parent: draftContext.parent};
  return <Modal title={title ?? (editing ? `Edit ${editing.name}` : `New ${noun}`)} onClose={() => (cancel.current ?? onClose)()} busy={saving}>
    {sequence.created && <Created object={object} record={sequence.created} onNavigate={onNavigate} />}
    {chooser.offering && <CreateDraftChooser drafts={chooser.drafts} expiryDays={chooser.expiryDays} onContinue={chooser.choose} onStartNew={() => chooser.choose('new')} />}
    {/* Each form after Save & New takes focus, since the one that had it is gone. */}
    {<RecordForm key={`${sequence.form}-${chooser.resume?.meta.id ?? ''}`} focusFirst={sequence.form > 0 || (chooser.resume !== undefined && !resume)} onSavingChange={setSaving} client={client} object={object} record={editing} presets={editing ? undefined : presets} only={only} always={always} offerNew={offerNew} cancelRef={cancel} draft={{key: formDraftKey(object, editing, context, chooser.resume ? (chooser.resume.document.key.kind === 'create' ? chooser.resume.document.key.draft : newDraft) : newDraft), resume: sequence.form === 0 ? chooser.resume : undefined, parentName: draftContext.parent?.name, action: context.action}} onCancel={onClose} onSaved={(saved, next) => {
        onSaved(saved, next);
        if (next === 'new') { setEditing(undefined); sequence.next(saved); }
      }} />}
  </Modal>;
}

/** Create page for an engine object, for links straight to a new record; `draftId` continues a draft from Drafts. */
export function NewRecordPage({client, apiName, onNavigate, headingRef, draftId}: {client: ObjectsClient; apiName: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>; draftId?: string}) {
  const [objectLoad, retryObject] = useObject(client, apiName);
  const sequence = useSaveAndNew();
  const object = objectLoad.status === 'ready' ? objectLoad.value : undefined;
  const opened = useOpenedDraft(client, draftId);
  useEffect(() => { if (object) document.title = `New ${object.label.toLowerCase()} - Limiara`; }, [object]);
  if (!object || opened.status === 'loading') return <div className="content-canvas object-page"><PageHeader breadcrumb="Objects" title="New record" headingRef={headingRef} /><LoadState load={object ? {status: 'loading'} : objectLoad} noun="object" onRetry={retryObject} /></div>;
  return <div className="content-canvas object-page">
    <PageHeader breadcrumb={`Objects / ${object.label}`} title={`New ${object.label.toLowerCase()}`} headingRef={headingRef} />
    {sequence.created && <Created object={object} record={sequence.created} onNavigate={onNavigate} />}
    {opened.status === 'gone' && <Alert variant="info" title="That draft is no longer here. It may have been saved, discarded or expired." />}
    {object.access.create
      ? <NewRecordForm key={sequence.form} client={client} object={object} first={sequence.form === 0} resume={sequence.form === 0 && opened.status === 'ready' ? opened.draft : undefined} presets={sequence.form === 0 && opened.status === 'ready' ? opened.presets : undefined} context={sequence.form === 0 && opened.status === 'ready' ? opened.context : {}} onCancel={() => onNavigate(objectRoute(object.apiName))} onSaved={(saved, next) => next === 'new' ? sequence.next(saved) : onNavigate(recordRoute(object.apiName, saved.id))} />
      : <Alert variant="warning" title="Permission needed">You do not have permission to create {object.pluralLabel.toLowerCase()}.</Alert>}
  </div>;
}

function NewRecordForm({client, object, first, resume, presets, context, onCancel, onSaved}: {client: ObjectsClient; object: ObjectDescription; first: boolean; resume?: StoredDraft; presets?: Presets; context: DraftContext; onCancel: () => void; onSaved: (record: ObjectRecord, next: AfterSave) => void}) {
  const chooser = useCreateChooser(object, first && !resume, context, resume);
  const newDraft = useMemo(() => crypto.randomUUID(), []);
  const chosen = chooser.resume;
  const draftKey = formDraftKey(object, undefined, context, chosen?.document.key.kind === 'create' ? chosen.document.key.draft : newDraft);
  return <>
    {chooser.offering && <CreateDraftChooser drafts={chooser.drafts} expiryDays={chooser.expiryDays} onContinue={chooser.choose} onStartNew={() => chooser.choose('new')} />}
    <RecordForm key={chosen?.meta.id ?? 'new'} focusFirst={!first || chosen !== undefined} client={client} object={object} presets={presets} draft={{key: draftKey, resume: chosen, parentName: context.parent?.name}} onCancel={onCancel} onSaved={onSaved} />
  </>;
}

/**
 * A draft opened from Drafts (ADR 0036 §8): read by id, with its parent
 * preset only while the member may still read the parent (§5).
 */
function useOpenedDraft(client: ObjectsClient, draftId?: string): {status: 'none' | 'loading' | 'gone'} | {status: 'ready'; draft: StoredDraft; presets?: Presets; context: DraftContext} {
  const runtime = useDrafts();
  const [state, setState] = useState<ReturnType<typeof useOpenedDraft>>(() => draftId ? {status: 'loading'} : {status: 'none'});
  useEffect(() => {
    if (!draftId) { setState({status: 'none'}); return; }
    if (runtime?.status !== 'ready') { setState({status: runtime ? 'gone' : 'none'}); return; }
    let cancelled = false;
    (async () => {
      const draft = await runtime.store.get(draftId).catch(() => null);
      if (cancelled) return;
      if (!draft) { setState({status: 'gone'}); return; }
      const key = draft.document.key;
      if (key.kind !== 'create' || !key.parent) { setState({status: 'ready', draft, context: key.kind === 'create' && key.action ? {action: {name: key.action, label: draft.document.action?.label ?? key.action}} : {}}); return; }
      const parent = key.parent;
      const action = key.action ? {name: key.action, label: draft.document.action?.label ?? key.action} : undefined;
      try {
        const record = await client.getRecord(parent.object, parent.id);
        if (!cancelled) setState({status: 'ready', draft, presets: {[parent.field]: {value: record.id, label: record.name}}, context: {parent: {...parent, name: record.name}, action}});
      } catch {
        // The parent is no longer readable: the form opens without it.
        if (!cancelled) setState({status: 'ready', draft, context: {parent: {...parent, name: ''}, action}});
      }
    })();
    return () => { cancelled = true; };
  }, [client, draftId, runtime]);
  return state;
}

type Draft = Record<string, DraftValue>;

/** Starts the inputs of preset fields at their preset, so the form shows what a save sends. */
function withValues(object: ObjectDescription, draft: Draft, values: Record<string, unknown> = {}): Draft {
  const next = {...draft};
  for (const [key, value] of Object.entries(values)) {
    if (key === 'name') next.name = typeof value === 'string' ? value : '';
    const field = object.fields.find(item => item.apiName === key);
    if (field) next[key] = toDraftValue(field, value);
  }
  return next;
}

/** Values a new record starts with, such as the parent of a related record (ADR 0032). */
export type Presets = Record<string, {value: string; label: string}>;

function initialDraft(object: ObjectDescription, record?: ObjectRecord, presets: Presets = {}): Draft {
  const draft: Draft = {name: record?.name ?? ''};
  for (const field of editableFields(object)) {
    const preset = record ? undefined : presets[field.apiName];
    const value = preset ? preset.value : record ? record.values[field.apiName] : field.options.find(option => option.default)?.value;
    draft[field.apiName] = toDraftValue(field, value);
  }
  return draft;
}

export const inputTypes: Partial<Record<ObjectField['type'], string>> = {email: 'email', phone: 'tel', url: 'url', number: 'number', currency: 'number', percent: 'number', date: 'date', date_time: 'datetime-local'};

/** A lookup picker over the records the field may point at: those the member sees that meet its filter (ADR 0028). */
function LookupInput({client, objectApiName, field, target, value, currentName, error, onChange}: {client: ObjectsClient; objectApiName: string; field: ObjectField; target?: string; value: string; currentName?: string; error?: string; onChange: (value: string, name?: string) => void}) {
  const [options, setOptions] = useState<Load<RecordLink[]>>({status: 'loading'});
  const [search, setSearch] = useState('');
  const [more, setMore] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    // Waits for typing to pause before asking again.
    const timer = setTimeout(() => {
      client.lookupCandidates(objectApiName, field.apiName, search, controller.signal, target)
        .then(page => { if (!controller.signal.aborted) { setOptions({status: 'ready', value: page.records}); setMore(page.nextCursor !== null); } })
        .catch(reason => { if (!controller.signal.aborted) setOptions(failed(reason)); });
    }, search ? 250 : 0);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [client, objectApiName, field.apiName, search, target]);
  if (options.status !== 'ready' && options.status !== 'loading') {
    return <TextField label={field.label} required={field.required} value={value} error={error} description="Enter the record ID; the related records could not be listed." onChange={event => onChange(event.target.value)} />;
  }
  const records = options.status === 'ready' ? options.value : [];
  const known = records.some(record => record.id === value);
  const notes = [field.lookupFilter ? 'Only records that meet this field’s filter are listed.' : '', more ? 'Search to find records not listed.' : ''].filter(Boolean).join(' ');
  return <div className="object-lookup">
    <TextField label={`Search ${field.label.toLowerCase()}`} type="search" value={search} onChange={event => setSearch(event.target.value)} />
    <Select label={field.label} required={field.required} value={value} error={error} description={options.status === 'loading' ? 'Loading records…' : notes || undefined} onChange={event => onChange(event.target.value, records.find(record => record.id === event.target.value)?.name ?? (event.target.value === value ? currentName : undefined))}>
      <option value="">None</option>
      {!known && value && <option value={value}>{currentName ?? `Current record (${value})`}</option>}
      {records.map(record => <option key={record.id} value={record.id}>{record.name}</option>)}
    </Select>
  </div>;
}

/**
 * A related-to picker (ADR 0033): choose one of the objects the field allows,
 * then one of its records the member sees. Changing the object clears the
 * value, since a record belongs to one object.
 */
function RelatedToInput({client, objectApiName, field, value, currentName, currentObject, error, onChange}: {client: ObjectsClient; objectApiName: string; field: ObjectField; value: string; currentName?: string; currentObject?: string; error?: string; onChange: (value: string, name?: string, object?: string) => void}) {
  const allowed = field.referenceObjects ?? [];
  // A value to an object taken out of the allowlist is kept and shown as is.
  const [target, setTarget] = useState(() => currentObject ?? allowed[0] ?? '');
  const [labels, setLabels] = useState<Record<string, string>>({});
  useEffect(() => {
    const controller = new AbortController();
    // Labels are a nicety: API names stand in when they cannot be loaded.
    client.listObjects(controller.signal)
      .then(value => { if (!controller.signal.aborted) setLabels(Object.fromEntries(value.objects.map(object => [object.apiName, object.label]))); })
      .catch(() => undefined);
    return () => controller.abort();
  }, [client]);
  const choices = target && !allowed.includes(target) ? [target, ...allowed] : allowed;
  const objectLabel = (apiName: string) => labels[apiName] ?? apiName;
  return <fieldset className="object-related-to">
    <legend>{field.label}</legend>
    <Select label={`${field.label}: object`} value={target} onChange={event => { setTarget(event.target.value); if (value) onChange(''); }}>
      {choices.map(apiName => <option key={apiName} value={apiName}>{objectLabel(apiName)}</option>)}
    </Select>
    {target && <LookupInput key={target} client={client} objectApiName={objectApiName} field={{...field, label: `${field.label}: ${objectLabel(target)}`}} target={target} value={value} currentName={target === currentObject ? currentName : undefined} error={error} onChange={(next, name) => onChange(next, name, target)} />}
  </fieldset>;
}

/**
 * The values a save sends. Edits send only what changed, so untouched values
 * are never re-checked, and a new record leaves empty fields out. An action's
 * presets (ADR 0032) go with every save, but a shown preset field always sends
 * what the member submitted, even unchanged or empty, so their input wins.
 */
export function valuesToSend({record, draft, initial, shown, showsName, always = {}}: {record: boolean; draft: Draft; initial: Draft; shown: readonly ObjectField[]; showsName: boolean; always?: Record<string, unknown>}): Record<string, unknown> {
  const overrides = (apiName: string) => Object.prototype.hasOwnProperty.call(always, apiName);
  const values: Record<string, unknown> = {};
  if (showsName && (!record || draft.name !== initial.name || overrides('name'))) values.name = typeof draft.name === 'string' ? draft.name.trim() : '';
  for (const field of shown) {
    const value = draft[field.apiName];
    if (record && !overrides(field.apiName) && JSON.stringify(value) === JSON.stringify(initial[field.apiName])) continue;
    const apiValue = toApiValue(field, value ?? '');
    if (!record && apiValue === null && !overrides(field.apiName)) continue;
    values[field.apiName] = apiValue;
  }
  return {...always, ...values};
}

/** What to do once a record is saved: close the form, or start a new record. */
export type AfterSave = 'close' | 'new';

/** What a form drafts into (ADR 0036): its key, a draft chosen to continue, and what the draft names. */
export type FormDraft = {key: DraftKey; resume?: StoredDraft; parentName?: string; action?: {name: string; label: string}};

/** The create and edit form, built from field metadata, with Save, Save & New and Cancel. */
export function RecordForm({client, object, record, presets = {}, only, always, offerNew = true, onCancel, onSaved, onSavingChange, focusFirst = false, draft: drafting, cancelRef}: {client: ObjectsClient; object: ObjectDescription; record?: ObjectRecord; presets?: Presets; only?: readonly string[]; always?: Record<string, unknown>; offerNew?: boolean; onCancel: () => void; onSaved: (record: ObjectRecord, next: AfterSave) => void; onSavingChange?: (saving: boolean) => void; focusFirst?: boolean; draft?: FormDraft; cancelRef?: React.MutableRefObject<(() => void) | null>}) {
  // The record the form edits: the one it opened with, or the current one a restored draft re-read (ADR 0036 §6).
  const [base, setBase] = useState(record);
  const [draft, setDraft] = useState<Draft>(() => withValues(object, initialDraft(object, record, presets), always));
  // Where the form started, with presets and defaults: a draft keeps only what differs from it.
  const start = useRef(draft);
  const [lookupNames, setLookupNames] = useState<Record<string, string>>({});
  const [chosenObjects, setChosenObjects] = useState<Record<string, string>>({});
  const [saving, setSavingState] = useState(false);
  const setSaving = (value: boolean) => { setSavingState(value); onSavingChange?.(value); };
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [error, setError] = useState('');
  const [cancelling, setCancelling] = useState(false);
  // Both buttons submit the form; the one pressed says what comes next.
  const next = useRef<AfterSave>('close');
  const initial = useMemo(() => initialDraft(object, base), [object, base]);
  const issueFor = (field: string) => issues.find(issue => issue.field === field)?.detail;
  // A quick action (ADR 0032) asks only for its fields; a parent preset stays visible, read-only.
  const showsName = !only || only.includes('name');
  const shown = editableFields(object).filter(field => !only || only.includes(field.apiName) || (!record && presets[field.apiName] !== undefined && holdsReference(field.type)));

  const latest = useRef({draft, base, lookupNames, chosenObjects});
  latest.current = {draft, base, lookupNames, chosenObjects};
  const startedAt = useRef(Date.now());
  const types = useMemo(() => ({name: 'text', ...Object.fromEntries(object.fields.map(field => [field.apiName, field.type]))}), [object]);
  const build = (): DraftDocument | null => {
    if (!drafting) return null;
    const current = latest.current;
    return {
      format: 1,
      key: drafting.key,
      object: object.apiName,
      objectLabel: object.label,
      record: current.base ? {id: current.base.id, version: current.base.version, name: current.base.name} : undefined,
      parentName: drafting.parentName,
      action: drafting.action,
      fields: changedFields(start.current, current.draft, types, current.lookupNames, current.chosenObjects),
      startedAt: startedAt.current,
      changedAt: Date.now(),
    };
  };
  const apply = (restored: Restored) => {
    if (restored.record) {
      setBase(restored.record);
      start.current = withValues(object, initialDraft(object, restored.record), always);
      setDraft({...start.current, ...restored.values});
    } else {
      setDraft(current => ({...current, ...restored.values}));
    }
    setLookupNames(restored.lookupNames);
    setChosenObjects(restored.relatedObjects);
  };
  const autosave = useFormDraft({client, draftKey: drafting?.key ?? null, resume: drafting?.resume, build, apply, only});
  const set = (key: string, value: Draft[string]) => { setDraft(current => ({...current, [key]: value})); autosave.changed(); };
  const setReference = (key: string, value: string, name?: string, chosenObject?: string) => {
    setLookupNames(current => { const {[key]: _, ...rest} = current; return name ? {...rest, [key]: name} : rest; });
    if (chosenObject) setChosenObjects(current => ({...current, [key]: chosenObject}));
    set(key, value);
  };

  // Cancel or Escape with a saved draft asks whether to keep it (ADR 0036 §8).
  const requestCancel = () => {
    if (saving) return;
    void (async () => {
      if (autosave.phase.kind === 'active') {
        await autosave.flush();
        if (autosave.saved) { setCancelling(true); return; }
      }
      onCancel();
    })();
  };
  if (cancelRef) cancelRef.current = requestCancel;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setIssues([]);
    setError('');
    setCancelling(false);
    // The draft is written first, so a failed save keeps everything typed.
    await autosave.flush();
    try {
      const sent = valuesToSend({record: !!base, draft, initial, shown, showsName, always});
      const saved = base ? await client.updateRecord(object.apiName, base.id, base.version, sent) : await client.createRecord(object.apiName, sent);
      // Saving is the normal create or update; the draft goes once it succeeds.
      await autosave.clear();
      onSaved(saved, next.current);
    } catch (reason) {
      if (reason instanceof ObjectsApiError && reason.reason === 'invalid' && reason.issues.length) {
        setIssues(reason.issues);
        setError('Some values need attention.');
      } else if (reason instanceof ObjectsApiError && reason.code === 'records.duplicate_value') {
        setError('Another record already uses one of these unique values.');
      } else if (reason instanceof ObjectsApiError && reason.code === 'records.version_conflict') {
        setError('Someone changed this record since you opened it. Reload it to see their changes.');
      } else {
        setError(reason instanceof Error ? reason.message : 'The record could not be saved.');
      }
    } finally {
      setSaving(false);
    }
  };

  const phase = autosave.phase;
  return <form className="object-form" onSubmit={event => void submit(event)} noValidate aria-busy={saving || undefined}>
    {phase.kind === 'prompt' && <RestorePrompt draft={phase.draft} notices={phase.notices} expiryDays={autosave.expiryDays} onRestore={() => void autosave.restore()} onDiscard={() => void autosave.discard()} />}
    {phase.kind === 'restoring' && <Skeleton label="Restoring your draft" lines={2} />}
    <DraftMessages message={autosave.message} dropped={autosave.dropped} />
    {phase.kind === 'blocked' && <BlockedDraft onKeep={autosave.keepBlocked} onDiscard={() => void autosave.discard()} />}
    {error && <Alert variant="danger" title={error}>{issues.length > 0 && <ul>{issues.map(issue => <li key={`${issue.field}-${issue.code}-${issue.detail}`}>{issue.field === 'record' ? issue.detail : <>{object.fields.find(field => field.apiName === issue.field)?.label ?? (issue.field === 'name' ? object.nameLabel : issue.field)}: {issue.detail}</>}</li>)}</ul>}</Alert>}
    {showsName && <TextField label={object.nameLabel} required autoFocus={focusFirst} value={String(draft.name ?? '')} error={issueFor('name')} maxLength={255} onChange={event => set('name', event.target.value)} />}
    {shown.map(field => {
      const value = draft[field.apiName];
      const fieldError = issueFor(field.apiName);
      if (field.type === 'boolean') return <label key={field.apiName} className="object-checkbox"><input type="checkbox" checked={value === true} onChange={event => set(field.apiName, event.target.checked)} />{field.label}{fieldError && <span className="ui-field__error">{fieldError}</span>}</label>;
      if (field.type === 'picklist') return <Select key={field.apiName} label={field.label} required={field.required} value={String(value ?? '')} error={fieldError} onChange={event => set(field.apiName, event.target.value)}>
        <option value="">None</option>
        {field.options.filter(option => option.active || option.value === value).map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
      </Select>;
      if (field.type === 'multi_picklist') return <fieldset key={field.apiName} className="object-multi"><legend>{field.label}</legend>
        {field.options.filter(option => option.active).map(option => <label key={option.value} className="object-checkbox"><input type="checkbox" checked={Array.isArray(value) && value.includes(option.value)} onChange={event => set(field.apiName, event.target.checked ? [...(Array.isArray(value) ? value : []), option.value] : (Array.isArray(value) ? value : []).filter(item => item !== option.value))} />{option.label}</label>)}
        {fieldError && <span className="ui-field__error">{fieldError}</span>}
      </fieldset>;
      const preset = record ? undefined : presets[field.apiName];
      if (preset && holdsReference(field.type)) return <TextField key={field.apiName} label={field.label} value={preset.label} readOnly description={`Set to ${preset.label}.`} />;
      if (field.type === 'master_detail' && base) return <TextField key={field.apiName} label={field.label} value={base.related[String(value ?? '')] ?? String(value ?? '')} readOnly description="A record stays with its master." />;
      const currentName = lookupNames[field.apiName] ?? base?.related[String(value ?? '')];
      if (field.type === 'related_to') return <RelatedToInput key={`${field.apiName}-${chosenObjects[field.apiName] ?? ''}`} client={client} objectApiName={object.apiName} field={field} value={String(value ?? '')} currentName={currentName} currentObject={chosenObjects[field.apiName] ?? base?.relatedObjects?.[String(value ?? '')]} error={fieldError} onChange={(next, name, chosen) => setReference(field.apiName, next, name, chosen)} />;
      if (isReference(field.type)) return <LookupInput key={field.apiName} client={client} objectApiName={object.apiName} field={field} value={String(value ?? '')} currentName={currentName} error={fieldError} onChange={(next, name) => setReference(field.apiName, next, name)} />;
      if (field.type === 'long_text') return <div key={field.apiName} className="ui-field">
        <label className="ui-field__label" htmlFor={`field-${field.apiName}`}>{field.label}</label>
        <textarea id={`field-${field.apiName}`} className="ui-field__control object-textarea" required={field.required} value={String(value ?? '')} aria-invalid={fieldError ? true : undefined} onChange={event => set(field.apiName, event.target.value)} />
        {fieldError && <span className="ui-field__error">{fieldError}</span>}
      </div>;
      return <TextField key={field.apiName} label={field.label} type={inputTypes[field.type] ?? 'text'} step={field.type === 'currency' ? '0.01' : field.type === 'number' || field.type === 'percent' ? 'any' : undefined} required={field.required} value={String(value ?? '')} error={fieldError} description={field.unique ? 'Must be unique' : undefined} onChange={event => set(field.apiName, event.target.value)} />;
    })}
    {cancelling
      ? <CancelChoice onKeep={() => { setCancelling(false); onCancel(); }} onDiscard={() => void autosave.discard().then(() => { setCancelling(false); onCancel(); })} onBack={() => setCancelling(false)} />
      : <div className="object-page__actions">
        <Button type="submit" loading={saving} onClick={() => { next.current = 'close'; }}>Save</Button>
        {offerNew && object.access.create && <Button type="submit" variant="secondary" disabled={saving} onClick={() => { next.current = 'new'; }}>Save &amp; New</Button>}
        <Button type="button" variant="quiet" onClick={requestCancel} disabled={saving}>Cancel</Button>
      </div>}
    {(phase.kind === 'active' || phase.kind === 'unavailable') && <DraftStatus state={autosave.state} unavailable={phase.kind === 'unavailable'} maxDrafts={autosave.maxDrafts} />}
    {autosave.state.kind === 'failed' && autosave.state.reason === 'other_tab' && <Button type="button" size="small" variant="secondary" onClick={() => void autosave.takeOver()}>Take over here</Button>}
    {base && <p className="object-form__meta"><Badge variant="neutral">Version {base.version}</Badge></p>}
  </form>;
}
