import {useEffect, useState} from 'react';
import {Alert, Button, Skeleton} from '../components/ui';
import {formatRecordTimestamp} from '../plugin-ui/standard-fields/model';
import {formatValue, type ChangeSource, type HistoryEntry, type ObjectDescription, type ObjectRecord, type ObjectsClient, type UserReference} from './api';
import {failed, type Load} from './ObjectPages';

type Props = {
  client: ObjectsClient;
  object: ObjectDescription;
  record: ObjectRecord;
  onNavigate: (route: string) => void;
};

const lifecycle: Record<Exclude<HistoryEntry['kind'], 'changed'>, string> = {
  created: 'Created.',
  deleted: 'Moved to the recycle bin.',
  undeleted: 'Restored from the recycle bin.',
};

const assistants: Record<string, string> = {siri: 'Siri', gemini: 'Gemini'};

/**
 * What a change came through (#233), as "via …" text, or null when it was
 * not recorded. Details are ids and API names from the server, shown only
 * where they name something a member recognises (an automation or quick
 * action); device and client ids are not shown.
 */
export function describeSource(source: ChangeSource): string | null {
  if (!source) return null;
  switch (source.kind) {
    case 'web': return 'via web app';
    case 'api_client': return 'via API client';
    case 'device': return 'via device';
    case 'automation': return source.detail ? `via automation ${source.detail}` : 'via automation';
    case 'quick_action': return source.detail ? `via quick action ${source.detail}` : 'via quick action';
    case 'assistant': return `via ${(source.detail && assistants[source.detail]) || 'assistant'}`;
    case 'import': return 'via import';
    case 'system': return 'via system';
  }
}

/**
 * The record's tracked changes, newest first (ADR 0017). Values are data
 * from the API and are rendered as text; lookups show names only for
 * records the viewer can see.
 */
export function HistoryPanel({client, object, record, onNavigate}: Props) {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [related, setRelated] = useState<Record<string, string>>({});
  const [cursor, setCursor] = useState<string | null>(null);
  const [load, setLoad] = useState<Load<true>>({status: 'loading'});
  const [more, setMore] = useState(false);

  useEffect(() => {
    if (!object.trackHistory) return;
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.history(object.apiName, record.id, undefined, controller.signal)
      .then(page => {
        if (controller.signal.aborted) return;
        setEntries(page.entries);
        setRelated(page.related);
        setCursor(page.nextCursor);
        setLoad({status: 'ready', value: true});
      })
      .catch(reason => { if (!controller.signal.aborted) setLoad(failed(reason)); });
    return () => controller.abort();
    // record.version reloads history after every save on this page.
  }, [client, object.apiName, object.trackHistory, record.id, record.version]);

  if (!object.trackHistory) return null;

  const loadMore = async () => {
    if (!cursor) return;
    setMore(true);
    try {
      const page = await client.history(object.apiName, record.id, cursor);
      setEntries(current => [...current, ...page.entries]);
      setRelated(current => ({...current, ...page.related}));
      setCursor(page.nextCursor);
    } catch {
      setLoad({status: 'unavailable'});
    } finally {
      setMore(false);
    }
  };

  const fieldLabel = (field: string) => field === 'name' ? object.nameLabel : field === 'owner' ? 'Owner' : object.fields.find(item => item.apiName === field)?.label ?? field;
  const user = (reference: UserReference) => reference
    ? <a href={`/users/${encodeURIComponent(reference.id)}`} onClick={event => { event.preventDefault(); onNavigate(`/users/${encodeURIComponent(reference.id)}`); }}>{reference.name ?? 'Former member'}</a>
    : <span className="object-value--empty">Not recorded</span>;
  // The member who made the change, and what it came through.
  const changedBy = (entry: HistoryEntry) => {
    const via = describeSource(entry.source);
    return <td>{user(entry.changedBy)}{via && <span className="object-history__source"> {via}</span>}</td>;
  };
  const value = (entry: HistoryEntry, side: 'old' | 'new') => {
    if (entry.field === 'owner') return user(side === 'old' ? entry.oldUser : entry.newUser);
    const raw = side === 'old' ? entry.oldValue : entry.newValue;
    const field = object.fields.find(item => item.apiName === entry.field);
    const text = field ? formatValue(field, raw, related) : raw === null || raw === undefined ? '' : String(raw);
    return text ? text : <span className="object-value--empty">Blank</span>;
  };

  return <section className="object-section" aria-labelledby="history-heading">
    <h2 id="history-heading">History</h2>
    {load.status === 'loading' && <Skeleton label="Loading history" lines={3} />}
    {load.status !== 'loading' && load.status !== 'ready' && <Alert variant="danger" title="History could not be loaded" />}
    {load.status === 'ready' && entries.length === 0 && <p className="object-value--empty">No tracked changes yet.</p>}
    {load.status === 'ready' && entries.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">History of {record.name}</caption>
      <thead><tr><th scope="col">Date</th><th scope="col">Field</th><th scope="col">User</th><th scope="col">Original value</th><th scope="col">New value</th></tr></thead>
      <tbody>{entries.map(entry => <tr key={entry.id}>
        <td>{formatRecordTimestamp(entry.changedAt)}</td>
        {entry.kind === 'changed'
          ? <><th scope="row">{fieldLabel(entry.field ?? '')}</th>{changedBy(entry)}<td>{value(entry, 'old')}</td><td>{value(entry, 'new')}</td></>
          : <><th scope="row">{lifecycle[entry.kind]}</th>{changedBy(entry)}<td /><td /></>}
      </tr>)}</tbody>
    </table></div>}
    {load.status === 'ready' && cursor && <div><Button variant="secondary" loading={more} onClick={() => void loadMore()}>Show older changes</Button></div>}
  </section>;
}
