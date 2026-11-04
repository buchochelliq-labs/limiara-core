import React, {useEffect, useId, useRef, useState, type RefObject} from 'react';
import {Alert, Button, EmptyState} from '../components/ui';
import {ObjectsApiError, type ObjectsClient, type QueryResult} from './api';
import {recordRoute} from './ObjectPages';
import './objects.css';

export const queryRoute = '/settings/query';

const example = 'SELECT name, owner, created_at FROM custom:project WHERE created_at = LAST_N_DAYS:30 ORDER BY name';

/** A query result value as inert text. */
export function formatCell(value: unknown): string {
  if (value === null || value === undefined) return '—';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (Array.isArray(value)) return value.map(item => formatCell(item)).join(', ');
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** What went wrong with a query, in words a person can act on. */
export function queryProblem(error: unknown): string {
  if (error instanceof ObjectsApiError) {
    const issue = error.issues[0];
    if (issue) return issue.detail;
    if (error.code === 'query.timeout') return 'The query took too long. Filter on more selective indexed fields.';
    if (error.reason === 'denied') return 'You do not have permission to run queries here.';
  }
  return 'The query could not be run. Try again in a moment.';
}

export function QueryResults({result, onNavigate}: {result: QueryResult; onNavigate: (route: string) => void}) {
  const records = result.rows.some(row => typeof row.id === 'string');
  if (result.rows.length === 0) return <EmptyState title="No rows" description="Nothing you can see matches this query." />;
  return <div className="object-table-wrap"><table className="object-table">
    <caption className="sr-only">Query results from {result.object}</caption>
    <thead><tr>
      {records && <th scope="col">ID</th>}
      {result.columns.map(column => <th key={column.key} scope="col"><code>{column.key}</code></th>)}
    </tr></thead>
    <tbody>{result.rows.map((row, index) => {
      const id = typeof row.id === 'string' ? row.id : null;
      return <tr key={id ?? index}>
        {records && <th scope="row">{id
          ? <a href={recordRoute(result.object, id)} onClick={event => { event.preventDefault(); onNavigate(recordRoute(result.object, id)); }}><code>{id}</code></a>
          : '—'}</th>}
        {result.columns.map(column => {
          const value = row[column.key];
          return <td key={column.key} className={value === null || value === undefined ? 'object-value--empty' : undefined}>{formatCell(value)}</td>;
        })}
      </tr>;
    })}</tbody>
  </table></div>;
}

/**
 * Settings → Query: run the record query language (ADR 0019) as yourself.
 * Sharing applies, so people see exactly what they could see in lists.
 */
export function QueryConsole({client, onNavigate, headingRef}: {client: Pick<ObjectsClient, 'query'>; onNavigate: (route: string) => void; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [text, setText] = useState(example);
  const [includeDeleted, setIncludeDeleted] = useState(false);
  const [result, setResult] = useState<QueryResult | null>(null);
  const [ran, setRan] = useState<{q: string; includeDeleted: boolean} | null>(null);
  const [problem, setProblem] = useState('');
  const [busy, setBusy] = useState(false);
  const controller = useRef<AbortController | null>(null);
  const id = useId();
  useEffect(() => { document.title = 'Query - Limiara'; }, []);
  useEffect(() => () => controller.current?.abort(), []);

  const run = async (cursor?: string) => {
    const q = cursor && ran ? ran.q : text.trim();
    const deleted = cursor && ran ? ran.includeDeleted : includeDeleted;
    if (!q) return;
    controller.current?.abort();
    const current = new AbortController();
    controller.current = current;
    setBusy(true);
    setProblem('');
    try {
      const page = await client.query(q, {cursor, includeDeleted: deleted}, current.signal);
      if (current.signal.aborted) return;
      setRan({q, includeDeleted: deleted});
      setResult(previous => cursor && previous ? {...page, rows: [...previous.rows, ...page.rows]} : page);
    } catch (error) {
      if (current.signal.aborted) return;
      setProblem(queryProblem(error));
      if (!cursor) setResult(null);
    } finally {
      if (!current.signal.aborted) setBusy(false);
    }
  };

  return <div className="content-canvas object-page">
    <header className="page-header"><div>
      <p className="page-header__breadcrumb"><a href="/settings" onClick={event => { event.preventDefault(); onNavigate('/settings'); }}>Settings</a></p>
      <h1 ref={headingRef} tabIndex={-1}>Query</h1>
      <p>Query records of any object you can read. Record access applies, so you see what you would see in lists.</p>
    </div></header>
    <form className="object-form query-console" onSubmit={event => { event.preventDefault(); void run(); }}>
      <div className="ui-field">
        <label className="ui-field__label" htmlFor={id}>Query</label>
        <span className="ui-field__description" id={`${id}-description`}>Filter, sort and group on standard fields and indexed fields. Press Ctrl+Enter to run.</span>
        <textarea
          id={id}
          className={`ui-field__control object-textarea query-console__text${problem ? ' ui-field__control--error' : ''}`}
          value={text}
          spellCheck={false}
          aria-invalid={problem ? true : undefined}
          aria-describedby={`${id}-description${problem ? ` ${id}-error` : ''}`}
          onChange={event => setText(event.target.value)}
          onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void run(); } }}
        />
        {problem && <span className="ui-field__error" id={`${id}-error`} role="alert">{problem}</span>}
      </div>
      <label className="object-checkbox"><input type="checkbox" checked={includeDeleted} onChange={event => setIncludeDeleted(event.target.checked)} /> Include records in the recycle bin</label>
      <div className="object-page__actions"><Button type="submit" loading={busy} disabled={busy || !text.trim()}>Run query</Button></div>
    </form>
    <div className="sr-only" aria-live="polite">{result && !busy ? `${result.rows.length} rows${result.nextCursor ? ', more available' : ''}` : ''}</div>
    {result && <>
      <QueryResults result={result} onNavigate={onNavigate} />
      {result.nextCursor && <div className="object-pager"><Button variant="secondary" loading={busy} disabled={busy} onClick={() => void run(result.nextCursor ?? undefined)}>Load more</Button></div>}
    </>}
    {!result && !problem && <Alert variant="info" title="Example">
      <code>SELECT custom:stage, COUNT() n, SUM(custom:amount) FROM custom:deal GROUP BY custom:stage</code>
    </Alert>}
  </div>;
}
