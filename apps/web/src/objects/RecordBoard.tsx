import {useEffect, useMemo, useState} from 'react';
import {Alert, Button, Skeleton} from '../components/ui';
import {formatValue, ObjectsApiError, optionOutcome, readableFields, type ObjectDescription, type ObjectField, type ObjectRecord, type ObjectsClient, type PicklistOption, type RecordListStatus} from './api';

/** Cards a column loads at a time (ADR 0037 §3). */
export const BOARD_PAGE = 25;

type Column = {records: ObjectRecord[]; next: string | null; status: 'loading' | 'ready' | 'failed'; more: boolean};

/** The picklist a board groups by: the object's path field, else its first single-value picklist (ADR 0037 §3). */
export function boardFieldOf(object: ObjectDescription): ObjectField | undefined {
  const readable = (field: ObjectField) => field.type === 'picklist' && field.access?.read !== false;
  return object.fields.find(field => field.apiName === object.pathField && readable(field)) ?? object.fields.find(readable);
}

/** The board's columns: active open values in order, then closed ones when asked for. */
export function boardColumns(field: ObjectField, showClosed: boolean): PicklistOption[] {
  return field.options.filter(option => option.active && (showClosed || optionOutcome(option) === 'open'));
}

type RecordBoardProps = {
  client: ObjectsClient;
  object: ObjectDescription;
  field: ObjectField;
  search?: string;
  status?: Exclude<RecordListStatus, 'deleted'>;
  /** Opens a record. */
  onOpen: (record: ObjectRecord) => void;
};

/**
 * The board view (ADR 0037 §3): one column per value of a picklist, each
 * loading its records 25 at a time through the list's equality filter. A
 * card moves by dragging it or through its "Move to" menu, which keyboard
 * and touch users get too; either saves an ordinary edit with the record's
 * version, so a conflict leaves the card where it was. On a phone the
 * columns are a strip that swipes one column at a time.
 */
export function RecordBoard({client, object, field, search, status = 'active', onOpen}: RecordBoardProps) {
  const [showClosed, setShowClosed] = useState(false);
  const options = useMemo(() => boardColumns(field, showClosed), [field, showClosed]);
  const [columns, setColumns] = useState<Record<string, Column>>({});
  const [attempt, setAttempt] = useState(0);
  const [moving, setMoving] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const [error, setError] = useState('');
  const [dragging, setDragging] = useState<{record: ObjectRecord; from: string} | null>(null);
  // Card fields: the first readable ones other than the grouping field.
  const cardFields = useMemo(() => readableFields(object.fields).filter(item => item.apiName !== field.apiName).slice(0, 3), [object, field]);
  const editable = object.access.edit && field.access?.edit !== false;

  const load = (value: string, cursor: string | undefined, signal?: AbortSignal) =>
    client.listRecords(object.apiName, {filters: {[field.apiName]: value}, search: search || undefined, status: status === 'active' ? undefined : status, sort: 'name', direction: 'asc', cursor, limit: BOARD_PAGE}, signal);

  useEffect(() => {
    const controller = new AbortController();
    setColumns(Object.fromEntries(options.map(option => [option.value, {records: [], next: null, status: 'loading', more: false}])));
    for (const option of options) {
      load(option.value, undefined, controller.signal)
        .then(page => { if (!controller.signal.aborted) setColumns(current => ({...current, [option.value]: {records: page.records, next: page.nextCursor, status: 'ready', more: false}})); })
        .catch(() => { if (!controller.signal.aborted) setColumns(current => ({...current, [option.value]: {records: [], next: null, status: 'failed', more: false}})); });
    }
    return () => controller.abort();
    // `load` reads only the props listed here.
  }, [client, object.apiName, field.apiName, options, search, status, attempt]);

  const loadMore = async (value: string) => {
    const column = columns[value];
    if (!column?.next) return;
    setColumns(current => ({...current, [value]: {...column, more: true}}));
    try {
      const page = await load(value, column.next);
      setColumns(current => ({...current, [value]: {records: [...(current[value]?.records ?? []), ...page.records], next: page.nextCursor, status: 'ready', more: false}}));
    } catch {
      setColumns(current => ({...current, [value]: {...column, more: false}}));
      setError(`More ${object.pluralLabel.toLowerCase()} could not be loaded. Try again.`);
    }
  };

  const move = async (record: ObjectRecord, from: string, to: string) => {
    if (from === to || moving) return;
    const label = (value: string) => field.options.find(option => option.value === value)?.label ?? value;
    setError('');
    setMoving(record.id);
    try {
      const saved = await client.updateRecord(object.apiName, record.id, record.version, {[field.apiName]: to});
      setColumns(current => {
        const next = {...current};
        const source = next[from];
        if (source) next[from] = {...source, records: source.records.filter(item => item.id !== record.id)};
        const target = next[to];
        // A hidden closed column simply does not show the card until it is shown.
        if (target) next[to] = {...target, records: [saved, ...target.records.filter(item => item.id !== saved.id)]};
        return next;
      });
      setAnnouncement(`${record.name} moved to ${label(to)}.`);
    } catch (reason) {
      setError(moveProblem(reason, record.name, field.label));
    } finally {
      setMoving(null);
    }
  };

  const countText = (column: Column | undefined) => !column || column.status !== 'ready' ? '' : `${column.records.length}${column.next ? '+' : ''}`;

  return <section className="record-board" aria-label={`${object.pluralLabel} by ${field.label.toLowerCase()}`}>
    <div className="record-board__toolbar">
      <label className="object-checkbox"><input type="checkbox" checked={showClosed} onChange={event => setShowClosed(event.target.checked)} />Show closed {field.label.toLowerCase()} columns</label>
    </div>
    <div className="sr-only" aria-live="polite">{announcement}</div>
    {error && <Alert variant="danger" title={error} />}
    <div className="record-board__columns">
      {options.map(option => {
        const column = columns[option.value];
        const headingId = `board-${object.keyPrefix}-${option.value}`.replace(/[^A-Za-z0-9_-]/g, '-');
        return <section key={option.value} className="record-board__column" data-outcome={optionOutcome(option)} aria-labelledby={headingId}
          onDragOver={event => { if (dragging && editable) event.preventDefault(); }}
          onDrop={event => { event.preventDefault(); if (dragging) void move(dragging.record, dragging.from, option.value); setDragging(null); }}>
          <h3 id={headingId} className="record-board__heading">{option.label}<span className="record-board__count" aria-label={column?.status === 'ready' ? `${countText(column)} ${object.pluralLabel.toLowerCase()}` : undefined}>{countText(column)}</span></h3>
          {column?.status === 'loading' && <Skeleton label={`Loading ${option.label}`} lines={2} />}
          {column?.status === 'failed' && <div className="record-board__failed"><p>This column could not be loaded.</p><Button size="small" variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
          {column?.status === 'ready' && column.records.length === 0 && <p className="record-board__empty">None</p>}
          {column?.status === 'ready' && <ol className="record-board__cards">
            {column.records.map(record => <li key={record.id} className="record-board__card" draggable={editable && moving === null}
              aria-busy={moving === record.id || undefined}
              onDragStart={event => { event.dataTransfer.effectAllowed = 'move'; setDragging({record, from: option.value}); }}
              onDragEnd={() => setDragging(null)}>
              <button type="button" className="record-board__open" onClick={() => onOpen(record)}>{record.name}</button>
              {cardFields.length > 0 && <dl className="record-board__fields">
                {cardFields.map(item => {
                  const text = formatValue(item, record.values[item.apiName], record.related);
                  return text ? <div key={item.apiName}><dt>{item.label}</dt><dd>{text}</dd></div> : null;
                })}
              </dl>}
              {editable && <label className="record-board__move">
                <span className="sr-only">Move {record.name} to</span>
                <select value="" disabled={moving !== null} onChange={event => { const to = event.target.value; if (to) void move(record, option.value, to); }}>
                  <option value="">Move to…</option>
                  {field.options.filter(item => item.active && item.value !== option.value).map(item => <option key={item.value} value={item.value}>{item.label}</option>)}
                </select>
              </label>}
            </li>)}
          </ol>}
          {column?.next && <Button size="small" variant="quiet" disabled={column.more} aria-busy={column.more || undefined} onClick={() => void loadMore(option.value)}>Load more</Button>}
        </section>;
      })}
    </div>
  </section>;
}

function moveProblem(reason: unknown, name: string, fieldLabel: string): string {
  if (reason instanceof ObjectsApiError) {
    if (reason.reason === 'conflict') return `${name} changed since the board loaded, so it was not moved. Reload the board and try again.`;
    if (reason.reason === 'invalid') return reason.issues.map(issue => issue.detail).filter(Boolean).join(' ') || `${name} needs other values before its ${fieldLabel.toLowerCase()} can change. Open it to edit them.`;
    if (reason.reason === 'denied') return `You do not have permission to move ${name}.`;
  }
  return `${name} could not be moved. ${reason instanceof Error ? reason.message : ''}`.trim();
}
