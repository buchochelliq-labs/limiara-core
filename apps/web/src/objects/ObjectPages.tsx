import React, {useEffect, useMemo, useRef, useState, type RefObject} from 'react';
import {Alert, Badge, Button, EmptyState, Modal, Select, Skeleton, TextField} from '../components/ui';
import {formatRecordTimestamp} from '../plugin-ui/standard-fields/model';
import {formatValue, isReference, ObjectsApiError, type ObjectDescription, type ObjectField, type ObjectRecord, type ObjectsClient, type RecordLink, type RecordPage, type RelatedList, type ValidationIssue} from './api';
import {SharingPanel} from './SharingPanel';
import {HistoryPanel} from './HistoryPanel';
import './objects.css';

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

function PageHeader({breadcrumb, title, headingRef, children}: {breadcrumb: string; title: string; headingRef?: RefObject<HTMLHeadingElement | null>; children?: React.ReactNode}) {
  return <header className="page-header">
    <div><p className="page-header__breadcrumb">{breadcrumb}</p><h1 ref={headingRef} tabIndex={-1}>{title}</h1></div>
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
    client.listRecords(object.apiName, {search: submitted || undefined, status: bin ? 'deleted' : undefined, sort: 'name', direction: 'asc', cursor, limit: 50}, controller.signal)
      .then(value => { if (!controller.signal.aborted) setPage({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setPage(failed(error)); });
    return () => controller.abort();
  }, [client, object, submitted, bin, cursor, attempt]);

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
      {object.access.delete && <Button type="button" variant="quiet" aria-pressed={bin} onClick={() => { setCursor(undefined); setBin(value => !value); }}>{bin ? 'Back to records' : 'Recycle bin'}</Button>}
    </form>
    {page.status !== 'ready' && <LoadState load={page} noun="record list" onRetry={() => setAttempt(value => value + 1)} />}
    {page.status === 'ready' && page.value.records.length === 0 && <EmptyState title={bin ? 'The recycle bin is empty' : submitted ? 'No matching records' : `No ${object.pluralLabel.toLowerCase()} yet`} description={bin ? 'Deleted records appear here until they are restored or purged.' : submitted ? 'Try a different search.' : object.access.create ? `Create the first ${object.label.toLowerCase()} to get started.` : 'Records you can see will appear here.'} />}
    {page.status === 'ready' && page.value.records.length > 0 && <div className="object-table-wrap">
      <table className="object-table">
        <caption className="sr-only">{object.pluralLabel}</caption>
        <thead><tr><th scope="col">{object.nameLabel}</th>{columns.map(field => <th scope="col" key={field.apiName}>{field.label}</th>)}<th scope="col">{bin ? 'Deleted' : 'Last modified'}</th>{bin && <th scope="col"><span className="sr-only">Actions</span></th>}</tr></thead>
        <tbody>{page.value.records.map(record => <tr key={record.id}>
          <th scope="row">{bin ? record.name : <a href={recordRoute(object.apiName, record.id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(object.apiName, record.id)); }}>{record.name}</a>}</th>
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

function FieldValue({field, value, related, onNavigate}: {field: ObjectField; value: unknown; related: Record<string, string>; onNavigate: Navigate}) {
  const text = formatValue(field, value, related);
  if (!text) return <span className="object-value--empty">Not set</span>;
  if (isReference(field.type) && field.referenceObject && typeof value === 'string') {
    const route = recordRoute(field.referenceObject, value);
    return <a href={route} onClick={event => { event.preventDefault(); onNavigate(route); }}>{text}</a>;
  }
  if (field.type === 'email') return <a href={`mailto:${text}`}>{text}</a>;
  if (field.type === 'url' && /^https?:\/\//i.test(text)) return <a href={text} rel="noreferrer noopener" target="_blank">{text}</a>;
  return <>{text}</>;
}

/** One record of an engine object, with inline edit and delete. */
export function ObjectRecordPage({client, apiName, recordId, onNavigate, headingRef}: {client: ObjectsClient; apiName: string; recordId: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [objectLoad, retryObject] = useObject(client, apiName);
  const [load, setLoad] = useState<Load<ObjectRecord>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState('');
  const object = objectLoad.status === 'ready' ? objectLoad.value : undefined;

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

  // Record-level rights (ADR 0016) refine the object's permissions.
  const canEdit = record.rights ? record.rights.edit : object.access.edit;
  const canDelete = record.rights ? record.rights.delete : object.access.delete;

  return <div className="content-canvas object-page">
    <PageHeader breadcrumb={`Objects / ${object.label}`} title={record.name} headingRef={headingRef}>
      {canEdit && <Button variant="secondary" aria-haspopup="dialog" onClick={() => { setError(''); setEditing(true); }}>Edit</Button>}
      {canDelete && <Button variant="danger" onClick={() => setConfirmDelete(true)}>Delete</Button>}
    </PageHeader>
    {error && <Alert variant="danger" title={error} />}
    {confirmDelete && <div className="object-confirm" role="alertdialog" aria-labelledby="object-confirm-title">
      <p id="object-confirm-title"><strong>Delete {record.name}?</strong> It moves to the recycle bin, where it can be restored.</p>
      <div className="object-page__actions"><Button variant="danger" onClick={() => void remove()} autoFocus>Delete</Button><Button variant="secondary" onClick={() => setConfirmDelete(false)}>Cancel</Button></div>
    </div>}
    {editing && <RecordModal client={client} object={object} record={record} onNavigate={onNavigate} onClose={() => setEditing(false)} onSaved={(saved, next) => {
      // The first save is this record's edit; after Save & New, saves create others.
      if (saved.id === record.id) setLoad({status: 'ready', value: {...saved, rights: saved.rights ?? record.rights}});
      else if (next === 'close') onNavigate(recordRoute(object.apiName, saved.id));
      if (next === 'close') setEditing(false);
    }} />}
    <section aria-label="Details" className="object-section">
      <h2>Details</h2>
      <dl className="object-fields">
        <div><dt>{object.nameLabel}</dt><dd>{record.name}</dd></div>
        {object.fields.map(field => <div key={field.apiName}><dt>{field.label}</dt><dd><FieldValue field={field} value={record.values[field.apiName]} related={record.related} onNavigate={onNavigate} /></dd></div>)}
      </dl>
    </section>
    <RelatedLists client={client} apiName={object.apiName} recordId={record.id} onNavigate={onNavigate} />
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

/** The records of other objects that point at this one (ADR 0028), each list showing only what the member may see. */
export function RelatedLists({client, apiName, recordId, onNavigate}: {client: ObjectsClient; apiName: string; recordId: string; onNavigate: Navigate}) {
  const [load, setLoad] = useState<Load<RelatedList[]>>({status: 'loading'});
  const [more, setMore] = useState<Record<string, 'loading' | 'error'>>({});
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.relatedLists(apiName, recordId, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(reason => { if (!controller.signal.aborted) setLoad(failed(reason)); });
    return () => controller.abort();
  }, [client, apiName, recordId]);
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
        <h3 id={headingId}>{list.objectPluralLabel}{repeated.has(list.object) ? ` (${list.fieldLabel})` : ''}</h3>
        {list.records.length === 0
          ? <p className="object-form__meta">No {list.objectPluralLabel.toLowerCase()} you can see.</p>
          : <ul className="object-related__list">{list.records.map(item => <li key={item.id}><a href={recordRoute(list.object, item.id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(list.object, item.id)); }}>{item.name}</a></li>)}</ul>}
        {list.nextCursor && <Button variant="secondary" loading={more[key(list)] === 'loading'} onClick={() => void showMore(list)}>Show more</Button>}
        {more[key(list)] === 'error' && <Alert variant="danger" title={`More ${list.objectPluralLabel.toLowerCase()} could not be loaded. Try again.`} />}
      </section>;
    })}
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

/**
 * Create or edit in a modal over the current page. Save opens the record
 * (or, when editing, shows it updated); Save & New keeps the modal open on
 * a blank form for the next record.
 */
export function RecordModal({client, object, record, onClose, onSaved, onNavigate}: {client: ObjectsClient; object: ObjectDescription; record?: ObjectRecord; onClose: () => void; onSaved: (record: ObjectRecord, next: AfterSave) => void; onNavigate: Navigate}) {
  const sequence = useSaveAndNew();
  // After Save & New on an edit, the modal goes on to create a record.
  const [editing, setEditing] = useState(record);
  const noun = object.label.toLowerCase();
  return <Modal title={editing ? `Edit ${editing.name}` : `New ${noun}`} onClose={onClose}>
    {sequence.created && <Created object={object} record={sequence.created} onNavigate={onNavigate} />}
    <RecordForm key={sequence.form} client={client} object={object} record={editing} onCancel={onClose} onSaved={(saved, next) => {
      onSaved(saved, next);
      if (next === 'new') { setEditing(undefined); sequence.next(saved); }
    }} />
  </Modal>;
}

/** Create page for an engine object, for links straight to a new record. */
export function NewRecordPage({client, apiName, onNavigate, headingRef}: {client: ObjectsClient; apiName: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [objectLoad, retryObject] = useObject(client, apiName);
  const sequence = useSaveAndNew();
  const object = objectLoad.status === 'ready' ? objectLoad.value : undefined;
  useEffect(() => { if (object) document.title = `New ${object.label.toLowerCase()} - Limiara`; }, [object]);
  if (!object) return <div className="content-canvas object-page"><PageHeader breadcrumb="Objects" title="New record" headingRef={headingRef} /><LoadState load={objectLoad} noun="object" onRetry={retryObject} /></div>;
  return <div className="content-canvas object-page">
    <PageHeader breadcrumb={`Objects / ${object.label}`} title={`New ${object.label.toLowerCase()}`} headingRef={headingRef} />
    {sequence.created && <Created object={object} record={sequence.created} onNavigate={onNavigate} />}
    {object.access.create
      ? <RecordForm key={sequence.form} client={client} object={object} onCancel={() => onNavigate(objectRoute(object.apiName))} onSaved={(saved, next) => next === 'new' ? sequence.next(saved) : onNavigate(recordRoute(object.apiName, saved.id))} />
      : <Alert variant="warning" title="Permission needed">You do not have permission to create {object.pluralLabel.toLowerCase()}.</Alert>}
  </div>;
}

type Draft = Record<string, string | boolean | string[]>;

/** Fields a person can type into; formula fields are computed (ADR 0021). */
function editableFields(object: ObjectDescription) {
  return object.fields.filter(field => field.type !== 'formula' && field.type !== 'rollup');
}

function initialDraft(object: ObjectDescription, record?: ObjectRecord): Draft {
  const draft: Draft = {name: record?.name ?? ''};
  for (const field of editableFields(object)) {
    const value = record ? record.values[field.apiName] : field.options.find(option => option.default)?.value;
    if (field.type === 'boolean') draft[field.apiName] = value === true;
    else if (field.type === 'multi_picklist') draft[field.apiName] = Array.isArray(value) ? value.map(String) : [];
    else if (field.type === 'date_time' && typeof value === 'string') draft[field.apiName] = toLocalInput(value);
    else draft[field.apiName] = value === undefined || value === null ? '' : String(value);
  }
  return draft;
}

/** `datetime-local` works in the member's time zone; the API stores UTC. */
function toLocalInput(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}

function toApiValue(field: ObjectField, value: Draft[string]): unknown {
  if (field.type === 'boolean') return value === true;
  if (field.type === 'multi_picklist') return Array.isArray(value) && value.length ? value : null;
  if (typeof value !== 'string' || value.trim() === '') return null;
  if (field.type === 'number' || field.type === 'currency' || field.type === 'percent') {
    const number = Number(value);
    return Number.isFinite(number) ? number : value;
  }
  if (field.type === 'date_time') {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toISOString();
  }
  return value.trim();
}

const inputTypes: Partial<Record<ObjectField['type'], string>> = {email: 'email', phone: 'tel', url: 'url', number: 'number', currency: 'number', percent: 'number', date: 'date', date_time: 'datetime-local'};

/** A lookup picker over the records the field may point at: those the member sees that meet its filter (ADR 0028). */
function LookupInput({client, objectApiName, field, value, currentName, error, onChange}: {client: ObjectsClient; objectApiName: string; field: ObjectField; value: string; currentName?: string; error?: string; onChange: (value: string) => void}) {
  const [options, setOptions] = useState<Load<RecordLink[]>>({status: 'loading'});
  const [search, setSearch] = useState('');
  const [more, setMore] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    // Waits for typing to pause before asking again.
    const timer = setTimeout(() => {
      client.lookupCandidates(objectApiName, field.apiName, search, controller.signal)
        .then(page => { if (!controller.signal.aborted) { setOptions({status: 'ready', value: page.records}); setMore(page.nextCursor !== null); } })
        .catch(reason => { if (!controller.signal.aborted) setOptions(failed(reason)); });
    }, search ? 250 : 0);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [client, objectApiName, field.apiName, search]);
  if (options.status !== 'ready' && options.status !== 'loading') {
    return <TextField label={field.label} required={field.required} value={value} error={error} description="Enter the record ID; the related records could not be listed." onChange={event => onChange(event.target.value)} />;
  }
  const records = options.status === 'ready' ? options.value : [];
  const known = records.some(record => record.id === value);
  const notes = [field.lookupFilter ? 'Only records that meet this field’s filter are listed.' : '', more ? 'Search to find records not listed.' : ''].filter(Boolean).join(' ');
  return <div className="object-lookup">
    <TextField label={`Search ${field.label.toLowerCase()}`} type="search" value={search} onChange={event => setSearch(event.target.value)} />
    <Select label={field.label} required={field.required} value={value} error={error} description={options.status === 'loading' ? 'Loading records…' : notes || undefined} onChange={event => onChange(event.target.value)}>
      <option value="">None</option>
      {!known && value && <option value={value}>{currentName ?? `Current record (${value})`}</option>}
      {records.map(record => <option key={record.id} value={record.id}>{record.name}</option>)}
    </Select>
  </div>;
}

/** What to do once a record is saved: close the form, or start a new record. */
export type AfterSave = 'close' | 'new';

/** The create and edit form, built from field metadata, with Save, Save & New and Cancel. */
export function RecordForm({client, object, record, onCancel, onSaved}: {client: ObjectsClient; object: ObjectDescription; record?: ObjectRecord; onCancel: () => void; onSaved: (record: ObjectRecord, next: AfterSave) => void}) {
  const [draft, setDraft] = useState<Draft>(() => initialDraft(object, record));
  const [saving, setSaving] = useState(false);
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [error, setError] = useState('');
  // Both buttons submit the form; the one pressed says what comes next.
  const next = useRef<AfterSave>('close');
  const initial = useMemo(() => initialDraft(object, record), [object, record]);
  const issueFor = (field: string) => issues.find(issue => issue.field === field)?.detail;
  const set = (key: string, value: Draft[string]) => setDraft(current => ({...current, [key]: value}));

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setIssues([]);
    setError('');
    const values: Record<string, unknown> = {};
    if (!record || draft.name !== initial.name) values.name = typeof draft.name === 'string' ? draft.name.trim() : '';
    for (const field of editableFields(object)) {
      const value = draft[field.apiName];
      // Edits send only what changed, so untouched values are never re-checked.
      if (record && JSON.stringify(value) === JSON.stringify(initial[field.apiName])) continue;
      const apiValue = toApiValue(field, value ?? '');
      if (!record && apiValue === null) continue;
      values[field.apiName] = apiValue;
    }
    try {
      const saved = record ? await client.updateRecord(object.apiName, record.id, record.version, values) : await client.createRecord(object.apiName, values);
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

  return <form className="object-form" onSubmit={event => void submit(event)} noValidate aria-busy={saving || undefined}>
    {error && <Alert variant="danger" title={error}>{issues.length > 0 && <ul>{issues.map(issue => <li key={`${issue.field}-${issue.code}-${issue.detail}`}>{issue.field === 'record' ? issue.detail : <>{object.fields.find(field => field.apiName === issue.field)?.label ?? (issue.field === 'name' ? object.nameLabel : issue.field)}: {issue.detail}</>}</li>)}</ul>}</Alert>}
    <TextField label={object.nameLabel} required value={String(draft.name ?? '')} error={issueFor('name')} maxLength={255} onChange={event => set('name', event.target.value)} />
    {editableFields(object).map(field => {
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
      if (field.type === 'master_detail' && record) return <TextField key={field.apiName} label={field.label} value={record.related[String(value ?? '')] ?? String(value ?? '')} readOnly description="A record stays with its master." />;
      if (isReference(field.type)) return <LookupInput key={field.apiName} client={client} objectApiName={object.apiName} field={field} value={String(value ?? '')} currentName={record?.related[String(value ?? '')]} error={fieldError} onChange={next => set(field.apiName, next)} />;
      if (field.type === 'long_text') return <div key={field.apiName} className="ui-field">
        <label className="ui-field__label" htmlFor={`field-${field.apiName}`}>{field.label}</label>
        <textarea id={`field-${field.apiName}`} className="ui-field__control object-textarea" required={field.required} value={String(value ?? '')} aria-invalid={fieldError ? true : undefined} onChange={event => set(field.apiName, event.target.value)} />
        {fieldError && <span className="ui-field__error">{fieldError}</span>}
      </div>;
      return <TextField key={field.apiName} label={field.label} type={inputTypes[field.type] ?? 'text'} step={field.type === 'currency' ? '0.01' : field.type === 'number' || field.type === 'percent' ? 'any' : undefined} required={field.required} value={String(value ?? '')} error={fieldError} description={field.unique ? 'Must be unique' : undefined} onChange={event => set(field.apiName, event.target.value)} />;
    })}
    <div className="object-page__actions">
      <Button type="submit" loading={saving} onClick={() => { next.current = 'close'; }}>Save</Button>
      {object.access.create && <Button type="submit" variant="secondary" disabled={saving} onClick={() => { next.current = 'new'; }}>Save &amp; New</Button>}
      <Button type="button" variant="quiet" onClick={onCancel} disabled={saving}>Cancel</Button>
    </div>
    {record && <p className="object-form__meta"><Badge variant="neutral">Version {record.version}</Badge></p>}
  </form>;
}
