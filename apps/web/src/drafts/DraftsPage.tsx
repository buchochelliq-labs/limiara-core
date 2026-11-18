import {useEffect, useState, type RefObject} from 'react';
import {Alert, Button, EmptyState, Skeleton} from '../components/ui';
import type {ObjectsClient} from '../objects/api';
import {formatDraftTime, noticeLines} from './components';
import {draftTitle, kindLabel, planRestore} from './model';
import {useDrafts} from './runtime';
import {expiresAt, expiresSoon, type DraftNotices, type StoredDraft} from './types';
import './drafts.css';

export const draftsRoute = '/drafts';

export type DraftAvailability = 'available' | 'conflicts' | 'unavailable' | 'checking';

type Row = {draft: StoredDraft; state: DraftAvailability; objectLabel: string};

/** Ids as the record API spells them; anything else never reaches a query. */
const safeId = (id: string) => /^[A-Za-z0-9]+$/.test(id);

/**
 * Checks the listed drafts in the background (ADR 0036 §8): one describe
 * and one query per object for the listed record ids, then a full restore
 * check only for records that changed since the draft started.
 */
export async function checkAvailability(client: ObjectsClient, drafts: readonly StoredDraft[]): Promise<Map<string, {state: DraftAvailability; objectLabel?: string}>> {
  const result = new Map<string, {state: DraftAvailability; objectLabel?: string}>();
  const byObject = new Map<string, StoredDraft[]>();
  for (const draft of drafts) byObject.set(draft.document.object, [...(byObject.get(draft.document.object) ?? []), draft]);
  for (const [object, group] of byObject) {
    let label: string | undefined;
    let canCreate = false;
    let canEdit = false;
    try {
      const description = await client.describe(object);
      label = description.label;
      canCreate = description.access.create;
      canEdit = description.access.edit;
    } catch {
      for (const draft of group) result.set(draft.meta.id, {state: 'unavailable'});
      continue;
    }
    const ids = [...new Set(group.flatMap(draft => draft.document.key.kind === 'create' ? [] : [draft.document.key.record]))].filter(safeId);
    const versions = new Map<string, number>();
    let queried = ids.length === 0;
    if (ids.length) {
      try {
        const rows = await client.query(`SELECT id, version FROM ${object} WHERE id IN (${ids.map(id => `'${id}'`).join(', ')})`, {includeArchived: true});
        for (const row of rows.rows) if (typeof row.id === 'string' && typeof row.version === 'number') versions.set(row.id, row.version);
        queried = true;
      } catch {
        queried = false;
      }
    }
    for (const draft of group) {
      const key = draft.document.key;
      if (key.kind === 'create') { result.set(draft.meta.id, {state: canCreate ? 'available' : 'unavailable', objectLabel: label}); continue; }
      if (!canEdit || (queried && !versions.has(key.record))) { result.set(draft.meta.id, {state: 'unavailable', objectLabel: label}); continue; }
      const version = versions.get(key.record);
      if (queried && version === draft.document.record?.version) { result.set(draft.meta.id, {state: 'available', objectLabel: label}); continue; }
      try {
        const plan = await planRestore(client, draft.document);
        result.set(draft.meta.id, {state: plan.status === 'conflicts' ? 'conflicts' : plan.status === 'unavailable' ? 'unavailable' : 'available', objectLabel: label});
      } catch {
        result.set(draft.meta.id, {state: 'available', objectLabel: label});
      }
    }
  }
  return result;
}

const stateLabels: Record<DraftAvailability, string> = {available: 'Available', conflicts: 'Record changed: conflicts', unavailable: 'Not available right now', checking: 'Checking…'};

/** Where Open takes a draft: its record's form, or a new-record form that continues it. */
export function draftRoute(draft: StoredDraft): string {
  const key = draft.document.key;
  const object = `/o/${encodeURIComponent(draft.document.object)}`;
  const id = encodeURIComponent(draft.meta.id);
  return key.kind === 'create' ? `${object}/new?draft=${id}` : `${object}/${encodeURIComponent(key.record)}?draft=${id}`;
}

/** The member's drafts in this workspace, from the user menu (ADR 0036 §8). */
export function DraftsPage({client, onNavigate, headingRef}: {client: ObjectsClient; onNavigate: (route: string) => void; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const runtime = useDrafts();
  const [rows, setRows] = useState<Row[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [notices, setNotices] = useState<DraftNotices>({expired: 0, dropped: 0});
  const [confirmAll, setConfirmAll] = useState(false);
  const [announcement, setAnnouncement] = useState('');
  const [refresh, setRefresh] = useState(0);
  useEffect(() => { document.title = 'Drafts - Limiara'; }, []);
  useEffect(() => {
    if (runtime?.status !== 'ready') return;
    let cancelled = false;
    (async () => {
      try {
        await runtime.store.sweep(runtime.contribution.expiryDays);
        const drafts = await runtime.store.list();
        const seen = await runtime.store.takeNotices();
        if (cancelled) return;
        if (refresh === 0) setNotices(seen);
        setRows(drafts.map(draft => ({draft, state: 'checking', objectLabel: draft.document.objectLabel})));
        const states = await checkAvailability(client, drafts);
        if (cancelled) return;
        setRows(drafts.map(draft => {
          const found = states.get(draft.meta.id);
          return {draft, state: found?.state ?? 'available', objectLabel: found?.objectLabel ?? draft.document.objectLabel};
        }));
      } catch {
        if (!cancelled) setFailed(true);
      }
    })();
    return () => { cancelled = true; };
  }, [client, runtime, refresh]);

  const header = <header className="page-header"><div><p className="page-header__breadcrumb">Account</p><h1 ref={headingRef} tabIndex={-1}>Drafts</h1><p>Unsaved changes to records, kept in this browser until you save or discard them.</p></div></header>;
  if (!runtime) return <div className="content-canvas">{header}<Alert variant="info" title="Drafts are not kept here">This workspace does not keep drafts on this device.</Alert></div>;
  if (runtime.status === 'unavailable') return <div className="content-canvas">{header}<Alert variant="warning" title="Drafts are unavailable">Drafts could not be unlocked for this page. Reload the page to try again.</Alert></div>;
  if (failed) return <div className="content-canvas">{header}<Alert variant="danger" title="Your drafts could not be read. Try again in a moment." /><Button variant="secondary" onClick={() => { setFailed(false); setRefresh(value => value + 1); }}>Try again</Button></div>;
  if (!rows) return <div className="content-canvas">{header}<Skeleton label="Loading drafts" lines={3} /></div>;

  const discard = async (draft: StoredDraft) => {
    await runtime.store.delete(draft.meta.id).catch(() => undefined);
    setAnnouncement('Draft discarded.');
    setRows(current => current?.filter(row => row.draft.meta.id !== draft.meta.id) ?? null);
  };
  const discardAll = async () => {
    await runtime.store.clear().catch(() => undefined);
    setConfirmAll(false);
    setAnnouncement('All drafts discarded.');
    setRows([]);
  };
  const expiryDays = runtime.contribution.expiryDays;
  return <div className="content-canvas">
    {header}
    <div className="sr-only" aria-live="polite">{announcement}</div>
    {noticeLines(notices).map(line => <Alert key={line} variant="info" title={line} />)}
    {rows.length === 0
      ? <EmptyState title="No drafts" description="When you edit or create a record, what you type is kept here until you save it." />
      : <>
        <div className="drafts-table-wrap">
          <table className="drafts-table">
            <caption className="sr-only">Your drafts</caption>
            <thead><tr><th scope="col">Draft</th><th scope="col">Object</th><th scope="col">Kind</th><th scope="col">Last change</th><th scope="col">Expires</th><th scope="col">State</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>{rows.map(({draft, state, objectLabel}) => {
              // A draft whose object or record is not available shows no title.
              const title = state === 'unavailable' ? 'Not available right now' : draftTitle(draft.document);
              return <tr key={draft.meta.id}>
                <th scope="row">{title}</th>
                <td>{objectLabel}</td>
                <td>{draft.document.action ? `Action: ${draft.document.action.label}` : kindLabel(draft.document.key)}</td>
                <td>{formatDraftTime(draft.meta.changedAt)}</td>
                <td>{expiresSoon(draft.meta.changedAt, expiryDays) ? 'Expires tomorrow' : formatDraftTime(expiresAt(draft.meta.changedAt, expiryDays))}</td>
                <td>{stateLabels[state]}</td>
                <td><div className="draft-actions">
                  {state !== 'unavailable' && <Button size="small" variant="secondary" aria-label={`Open ${title}`} onClick={() => onNavigate(draftRoute(draft))}>Open</Button>}
                  <Button size="small" variant="quiet" aria-label={`Discard ${title}`} onClick={() => void discard(draft)}>Discard</Button>
                </div></td>
              </tr>;
            })}</tbody>
          </table>
        </div>
        {confirmAll
          ? <div className="object-confirm" role="alertdialog" aria-labelledby="drafts-discard-all"><p id="drafts-discard-all"><strong>Discard all {rows.length} drafts?</strong> This cannot be undone.</p><div className="draft-actions"><Button variant="danger" autoFocus onClick={() => void discardAll()}>Discard all</Button><Button variant="secondary" onClick={() => setConfirmAll(false)}>Cancel</Button></div></div>
          : <Button variant="secondary" onClick={() => setConfirmAll(true)}>Discard all</Button>}
      </>}
  </div>;
}
