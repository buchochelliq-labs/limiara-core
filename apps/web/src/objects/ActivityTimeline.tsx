import {useEffect, useState} from 'react';
import {Alert, Badge, Button, Skeleton} from '../components/ui';
import {formatRecordTimestamp} from '../plugin-ui/standard-fields/model';
import {formatValue, type ObjectsClient, type QuickAction, type Timeline, type TimelineItem} from './api';
import {failed, recordRoute, type Load} from './ObjectPages';

type Navigate = (route: string) => void;

const today = () => {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
};

/** Whether an open item's date has passed: a date before today, or a date-time before now. */
export function isOverdue(item: TimelineItem, now: Date = new Date()): boolean {
  if (!item.open || !item.date) return false;
  if (item.dateType === 'date') return item.date < today();
  const at = Date.parse(item.date);
  return !Number.isNaN(at) && at < now.getTime();
}

/** The date an item shows, as text: its own date, or when it was logged. */
export function itemDate(item: TimelineItem): string {
  if (item.date) return formatValue({apiName: item.dateField, label: '', description: null, type: item.dateType, origin: 'plugin', required: false, unique: false, indexed: false, options: [], referenceObject: null, deleteBehaviour: null, trackHistory: false, version: 1}, item.date);
  if (!item.open && item.createdAt) return `Logged ${formatRecordTimestamp(item.createdAt)}`;
  return 'No date';
}

type ViewProps = {
  timeline: Timeline;
  past: TimelineItem[];
  nextCursor: string | null;
  more: 'idle' | 'loading' | 'error';
  actions: QuickAction[];
  recordName: string;
  onMore: () => void;
  onRun: (action: QuickAction) => void;
  onNavigate: Navigate;
};

function Item({item, onNavigate}: {item: TimelineItem; onNavigate: Navigate}) {
  const route = recordRoute(item.object, item.id);
  return <li className="object-timeline__item">
    <Badge>{item.objectLabel}</Badge>
    <a href={route} onClick={event => { event.preventDefault(); onNavigate(route); }}>{item.name}</a>
    <span className="object-form__meta">{itemDate(item)}</span>
    {isOverdue(item) && <Badge variant="warning">Overdue</Badge>}
    {item.details.length > 0 && <span className="object-form__meta">{item.details.map(detail => `${detail.label}: ${detail.value}`).join(' · ')}</span>}
  </li>;
}

/** The loaded timeline: its actions, the open group, then the past group with Show more. */
export function TimelineView({timeline, past, nextCursor, more, actions, recordName, onMore, onRun, onNavigate}: ViewProps) {
  const empty = timeline.open.length === 0 && past.length === 0;
  return <>
    {actions.length > 0 && <div className="object-page__actions" role="group" aria-label={`Add activity to ${recordName}`}>
      {actions.map(action => <Button key={action.name} size="small" variant="secondary" aria-haspopup="dialog" onClick={() => onRun(action)}>{action.label}</Button>)}
    </div>}
    {empty && <p className="object-value--empty">No activity yet.</p>}
    {timeline.open.length > 0 && <section className="object-related" aria-labelledby="activity-open-heading">
      <h3 id="activity-open-heading">Open and upcoming</h3>
      <ul className="object-related__list">{timeline.open.map(item => <Item key={`${item.object}/${item.id}`} item={item} onNavigate={onNavigate} />)}</ul>
      {timeline.openMore && <p className="object-form__meta">Only the first {timeline.open.length} open items are shown.</p>}
    </section>}
    {past.length > 0 && <section className="object-related" aria-labelledby="activity-past-heading">
      <h3 id="activity-past-heading">Past</h3>
      <ul className="object-related__list">{past.map(item => <Item key={`${item.object}/${item.id}`} item={item} onNavigate={onNavigate} />)}</ul>
      {nextCursor && <Button variant="secondary" loading={more === 'loading'} onClick={onMore}>Show more</Button>}
      {more === 'error' && <Alert variant="danger" title="More activity could not be loaded. Try again." />}
    </section>}
  </>;
}

/**
 * The record's Activity timeline (ADR 0033 §4): what plugin objects that
 * declare a timeline record about this record, read as the member. It
 * offers the record's quick actions that create timeline records, and
 * renders nothing when no timeline source can point at the record.
 */
export function ActivityTimeline({client, apiName, recordId, recordName, actions, refresh = 0, onRun, onNavigate}: {client: Pick<ObjectsClient, 'timeline'>; apiName: string; recordId: string; recordName: string; actions: QuickAction[]; refresh?: number; onRun: (action: QuickAction) => void; onNavigate: Navigate}) {
  const [load, setLoad] = useState<Load<Timeline>>({status: 'loading'});
  const [past, setPast] = useState<TimelineItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [more, setMore] = useState<'idle' | 'loading' | 'error'>('idle');
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    // A refresh after a save keeps what is shown until the new timeline arrives.
    if (refresh === 0) setLoad({status: 'loading'});
    client.timeline(apiName, recordId, undefined, controller.signal)
      .then(value => {
        if (controller.signal.aborted) return;
        setLoad({status: 'ready', value});
        setPast(value.past);
        setCursor(value.nextCursor);
        setMore('idle');
      })
      .catch(reason => { if (!controller.signal.aborted) setLoad(failed(reason)); });
    return () => controller.abort();
  }, [client, apiName, recordId, refresh, attempt]);

  const showMore = async () => {
    if (!cursor) return;
    setMore('loading');
    try {
      const page = await client.timeline(apiName, recordId, cursor);
      setPast(current => [...current, ...page.past]);
      setCursor(page.nextCursor);
      setMore('idle');
    } catch {
      setMore('error');
    }
  };
  return <TimelineSection load={load} past={past} nextCursor={cursor} more={more} actions={actions} recordName={recordName} onRetry={() => setAttempt(value => value + 1)} onMore={() => void showMore()} onRun={onRun} onNavigate={onNavigate} />;
}

/** The section in each state; nothing at all once the timeline turns out to have no sources. */
export function TimelineSection({load, past, nextCursor, more, actions, recordName, onRetry, onMore, onRun, onNavigate}: Omit<ViewProps, 'timeline' | 'actions'> & {load: Load<Timeline>; actions: QuickAction[]; onRetry: () => void}) {
  if (load.status === 'ready' && load.value.sources === 0) return null;
  // Offered only when the timeline names them; the page already left out those the member may not run.
  const offered = load.status === 'ready' ? actions.filter(action => load.value.actions.includes(action.name)) : [];
  return <section className="object-section" aria-labelledby="activity-heading">
    <h2 id="activity-heading">Activity</h2>
    {load.status === 'loading' && <Skeleton label="Loading activity" lines={3} />}
    {(load.status === 'unavailable' || load.status === 'denied' || load.status === 'missing') && <div className="object-page__state">
      <Alert variant="danger" title="Activity could not be loaded">Try again in a moment.</Alert>
      <Button variant="secondary" onClick={onRetry}>Try again</Button>
    </div>}
    {load.status === 'ready' && <TimelineView timeline={load.value} past={past} nextCursor={nextCursor} more={more} actions={offered} recordName={recordName} onMore={onMore} onRun={onRun} onNavigate={onNavigate} />}
  </section>;
}
