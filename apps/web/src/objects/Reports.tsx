import React, {useEffect, useId, useState, type RefObject} from 'react';
import {Alert, Badge, Button, EmptyState, Select, Skeleton, TextField} from '../components/ui';
import {ObjectsApiError, reportCharts, type ObjectsClient, type QueryResult, type Report, type ReportChart, type ReportInput} from './api';
import {formatCell, queryProblem, QueryResults} from './QueryConsole';
import './objects.css';

/** Saved reports (ADR 0027): record queries with a name and a chart. */
export const reportsRoute = '/reports';
export function reportRoute(id: string): string {
  return `${reportsRoute}/${encodeURIComponent(id)}`;
}

type Navigate = (route: string) => void;
type ReportsClient = Pick<ObjectsClient, 'listReports' | 'runReport' | 'createReport' | 'updateReport' | 'deleteReport' | 'query'>;

const example = 'SELECT custom:status, COUNT() FROM custom:project GROUP BY custom:status';
const numeric = new Set(['number', 'currency', 'percent', 'integer']);
/** Chart colours, from the object tokens so they follow the theme. */
const palette = ['var(--object-people)', 'var(--object-organisations)', 'var(--object-activity)', 'var(--object-tasks)', 'var(--object-app)'];
/** Charts plot at most this many rows; the table shows them all. */
const CHART_ROWS = 50;

function problem(reason: unknown, fallback: string): string {
  if (reason instanceof ObjectsApiError) {
    if (reason.reason === 'denied') return 'You do not have permission to do that with this report.';
    if (reason.reason === 'conflict') return 'Someone changed this report. Open it again to see their change.';
    return reason.issues[0]?.detail ?? fallback;
  }
  return fallback;
}

function number(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

/** The pages under /reports: the list, a report, and the builder. */
export function ReportsPage({client, route, onNavigate, headingRef}: {client: ReportsClient; route: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const rest = route.slice(reportsRoute.length).split('/').filter(Boolean).map(decodeURIComponent);
  if (rest.length === 0) return <ReportsHome client={client} onNavigate={onNavigate} headingRef={headingRef} />;
  if (rest[0] === 'new') return <ReportEditor client={client} onNavigate={onNavigate} headingRef={headingRef} />;
  const [id, action] = rest;
  if (!id) return null;
  if (action === 'edit') return <ReportEditor key={id} client={client} reportId={id} onNavigate={onNavigate} headingRef={headingRef} />;
  return <ReportView key={id} client={client} reportId={id} onNavigate={onNavigate} headingRef={headingRef} />;
}

function ReportsHome({client, onNavigate, headingRef}: {client: ReportsClient; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, setLoad] = useState<{status: 'loading'} | {status: 'ready'; reports: Report[]} | {status: 'error'; message: string}>({status: 'loading'});
  useEffect(() => {
    const controller = new AbortController();
    client.listReports(controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', reports: value.reports}); })
      .catch(reason => { if (!controller.signal.aborted) setLoad({status: 'error', message: problem(reason, 'Reports could not be loaded.')}); });
    return () => controller.abort();
  }, [client]);
  return <div className="content-canvas">
    <header className="page-header">
      <div><h1 ref={headingRef} tabIndex={-1}>Reports</h1><p>Saved questions about your records. Each report shows only the records you can see.</p></div>
      <Button onClick={() => onNavigate(`${reportsRoute}/new`)}>New report</Button>
    </header>
    {load.status === 'loading' && <Skeleton lines={4} />}
    {load.status === 'error' && <Alert variant="danger" title={load.message} />}
    {load.status === 'ready' && load.reports.length === 0 && <EmptyState title="No reports yet" description="Ask a question with the query language, then save it as a table or a chart." />}
    {load.status === 'ready' && load.reports.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Reports</caption>
      <thead><tr><th scope="col">Name</th><th scope="col">Shows</th><th scope="col">Shared with</th><th scope="col">Owner</th></tr></thead>
      <tbody>{load.reports.map(report => <tr key={report.id}>
        <th scope="row"><a href={reportRoute(report.id)} onClick={event => { event.preventDefault(); onNavigate(reportRoute(report.id)); }}>{report.name}</a>{report.description && <div className="object-form__meta">{report.description}</div>}</th>
        <td>{reportCharts.find(chart => chart.value === report.chart)?.label ?? report.chart}</td>
        <td>{report.visibility === 'workspace' ? <Badge>Workspace</Badge> : 'Only its owner'}</td>
        <td>{report.owner?.name ?? 'Workspace member'}</td>
      </tr>)}</tbody>
    </table></div>}
  </div>;
}

function ReportView({client, reportId, onNavigate, headingRef}: {client: ReportsClient; reportId: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, setLoad] = useState<{status: 'loading'} | {status: 'ready'; report: Report; result: QueryResult} | {status: 'error'; message: string; missing: boolean}>({status: 'loading'});
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState('');
  const [more, setMore] = useState<'idle' | 'loading' | 'error'>('idle');
  useEffect(() => {
    const controller = new AbortController();
    client.runReport(reportId, {}, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', ...value}); })
      .catch(reason => { if (!controller.signal.aborted) setLoad({status: 'error', message: reason instanceof ObjectsApiError && reason.reason === 'missing' ? 'This report does not exist or is not shared with you.' : queryProblem(reason), missing: reason instanceof ObjectsApiError && reason.reason === 'missing'}); });
    return () => controller.abort();
  }, [client, reportId]);
  if (load.status === 'loading') return <div className="content-canvas"><Skeleton lines={6} /></div>;
  if (load.status === 'error') return <div className="content-canvas">
    <header className="page-header"><div><p className="page-header__breadcrumb">Reports</p><h1 ref={headingRef} tabIndex={-1}>Report unavailable</h1></div></header>
    <Alert variant={load.missing ? 'warning' : 'danger'} title={load.message} />
    <Button variant="secondary" onClick={() => onNavigate(reportsRoute)}>All reports</Button>
  </div>;
  const {report, result} = load;
  const loadMore = async () => {
    if (!result.nextCursor) return;
    setMore('loading');
    try {
      const next = await client.runReport(report.id, {cursor: result.nextCursor});
      setLoad({status: 'ready', report: next.report, result: {...next.result, rows: [...result.rows, ...next.result.rows]}});
      setMore('idle');
    } catch {
      setMore('error');
    }
  };
  const remove = async () => {
    setError('');
    try {
      await client.deleteReport(report.id, report.version);
      onNavigate(reportsRoute);
    } catch (reason) {
      setError(problem(reason, 'The report could not be deleted.'));
      setConfirming(false);
    }
  };
  return <div className="content-canvas">
    <header className="page-header">
      <div><p className="page-header__breadcrumb"><a href={reportsRoute} onClick={event => { event.preventDefault(); onNavigate(reportsRoute); }}>Reports</a></p><h1 ref={headingRef} tabIndex={-1}>{report.name}</h1>{report.description && <p>{report.description}</p>}</div>
      {report.canEdit && <div className="object-page__actions">
        <Button variant="secondary" onClick={() => onNavigate(`${reportRoute(report.id)}/edit`)}>Edit</Button>
        {confirming
          ? <><Button variant="danger" onClick={() => void remove()}>Delete report</Button><Button variant="secondary" onClick={() => setConfirming(false)}>Keep it</Button></>
          : <Button variant="secondary" onClick={() => setConfirming(true)}>Delete</Button>}
      </div>}
    </header>
    {error && <Alert variant="danger" title={error} />}
    <ReportChartView report={report} result={result} />
    <section className="object-section" aria-labelledby={`report-rows-${report.id}`}>
      <h2 id={`report-rows-${report.id}`}>Rows</h2>
      <QueryResults result={result} onNavigate={onNavigate} />
      {result.nextCursor && <div className="object-page__actions">
        <p className="object-form__meta">The first {result.rows.length} rows are shown.</p>
        <Button variant="secondary" loading={more === 'loading'} onClick={() => void loadMore()}>Show more rows</Button>
      </div>}
      {more === 'error' && <Alert variant="danger" title="More rows could not be loaded. Try again." />}
    </section>
    <details className="object-section"><summary>Query</summary><pre className="query-console__text"><code>{report.query}</code></pre></details>
  </div>;
}

/** A chart of a report's rows, with the rows as its accessible alternative below. */
export function ReportChartView({report, result}: {report: Pick<Report, 'chart' | 'labelColumn' | 'valueColumn' | 'name'>; result: QueryResult}) {
  const titleId = useId();
  if (report.chart === 'table' || !report.valueColumn || result.rows.length === 0) return null;
  const value = report.valueColumn;
  if (report.chart === 'metric') {
    return <figure className="report-metric" aria-labelledby={titleId}>
      <figcaption id={titleId} className="report-metric__label">{value}</figcaption>
      <p className="report-metric__value">{formatCell(result.rows[0]?.[value])}</p>
    </figure>;
  }
  const label = report.labelColumn ?? '';
  const points = result.rows.slice(0, CHART_ROWS).map(row => ({label: formatCell(row[label]), value: number(row[value])}));
  const summary = `${report.name}: ${points.map(point => `${point.label} ${point.value}`).join(', ')}`;
  return <figure className="report-chart" aria-labelledby={titleId}>
    <figcaption id={titleId} className="sr-only">{summary}</figcaption>
    {report.chart === 'bar' && <BarChart points={points} />}
    {report.chart === 'line' && <LineChart points={points} />}
    {report.chart === 'pie' && <PieChart points={points} />}
  </figure>;
}

type Point = {label: string; value: number};

/** Bars grow from a zero baseline, right for positive values and left for negative ones. */
export function barExtents(points: Point[], span = 360): {zero: number; scale: number} {
  const high = Math.max(0, ...points.map(point => point.value));
  const low = Math.min(0, ...points.map(point => point.value));
  const scale = span / ((high - low) || 1);
  return {zero: low < 0 ? -low * scale : 0, scale};
}

function BarChart({points}: {points: Point[]}) {
  const row = 28;
  const plot = 180;
  const {zero, scale} = barExtents(points);
  return <svg className="report-chart__svg" viewBox={`0 0 640 ${points.length * row + 8}`} role="presentation" aria-hidden="true">
    {points.some(point => point.value < 0) && <line x1={plot + zero} y1="0" x2={plot + zero} y2={points.length * row + 8} className="report-chart__axis" />}
    {points.map((point, index) => {
      const width = Math.abs(point.value) * scale;
      const x = point.value < 0 ? plot + zero - width : plot + zero;
      const y = index * row + 4;
      return <g key={index}>
        <text x="0" y={y + 17} className="report-chart__label">{point.label.slice(0, 24)}</text>
        <rect x={x} y={y} width={width} height={row - 8} rx="3" fill={palette[point.value < 0 ? 3 : 0]} />
        <text x={point.value < 0 ? x - 6 : x + width + 6} y={y + 15} textAnchor={point.value < 0 ? 'end' : 'start'} className="report-chart__label">{point.value}</text>
      </g>;
    })}
  </svg>;
}

function LineChart({points}: {points: Point[]}) {
  const max = Math.max(...points.map(point => point.value), 0) || 1;
  const step = points.length > 1 ? 600 / (points.length - 1) : 0;
  const path = points.map((point, index) => `${20 + index * step},${220 - (point.value / max) * 200}`).join(' ');
  return <svg className="report-chart__svg" viewBox="0 0 640 260" role="presentation" aria-hidden="true">
    <line x1="20" y1="220" x2="620" y2="220" className="report-chart__axis" />
    <polyline points={path} fill="none" stroke={palette[2]} strokeWidth="3" />
    {points.map((point, index) => <g key={index}>
      <circle cx={20 + index * step} cy={220 - (point.value / max) * 200} r="4" fill={palette[2]} />
      {(points.length <= 12 || index % Math.ceil(points.length / 12) === 0) && <text x={20 + index * step} y="244" textAnchor="middle" className="report-chart__label">{point.label.slice(0, 12)}</text>}
    </g>)}
  </svg>;
}

function PieChart({points}: {points: Point[]}) {
  const total = points.reduce((sum, point) => sum + Math.max(0, point.value), 0);
  if (total === 0) return <p className="object-form__meta">Nothing to chart: every value is zero.</p>;
  let angle = -Math.PI / 2;
  const slices = points.map((point, index) => {
    const share = Math.max(0, point.value) / total;
    const start = angle;
    angle += share * Math.PI * 2;
    const large = share > 0.5 ? 1 : 0;
    const [x1, y1, x2, y2] = [130 + 110 * Math.cos(start), 130 + 110 * Math.sin(start), 130 + 110 * Math.cos(angle), 130 + 110 * Math.sin(angle)];
    const d = share >= 1 ? 'M130,20 A110,110 0 1,1 129.99,20 Z' : `M130,130 L${x1},${y1} A110,110 0 ${large},1 ${x2},${y2} Z`;
    return {d, color: palette[index % palette.length] ?? palette[0], point, share};
  });
  return <div className="report-pie">
    <svg className="report-chart__svg report-chart__svg--pie" viewBox="0 0 260 260" role="presentation" aria-hidden="true">
      {slices.map((slice, index) => <path key={index} d={slice.d} fill={slice.color} stroke="var(--surface-raised)" strokeWidth="2" />)}
    </svg>
    <ul className="report-legend">{slices.map((slice, index) => <li key={index}><span className="report-legend__swatch" style={{background: slice.color}} aria-hidden="true" />{slice.point.label} <span className="object-form__meta">{slice.point.value} ({Math.round(slice.share * 100)}%)</span></li>)}</ul>
  </div>;
}

function ReportEditor({client, reportId, onNavigate, headingRef}: {client: ReportsClient; reportId?: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [existing, setExisting] = useState<Report | null>(null);
  const [loading, setLoading] = useState(Boolean(reportId));
  const [canManage, setCanManage] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [query, setQuery] = useState(example);
  const [chart, setChart] = useState<ReportChart>('table');
  const [labelColumn, setLabelColumn] = useState('');
  const [valueColumn, setValueColumn] = useState('');
  const [shared, setShared] = useState(false);
  const [preview, setPreview] = useState<QueryResult | null>(null);
  const [previewError, setPreviewError] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const queryId = useId();
  useEffect(() => {
    const controller = new AbortController();
    client.listReports(controller.signal).then(value => {
      if (controller.signal.aborted) return;
      setCanManage(value.canManage);
      const found = reportId ? value.reports.find(report => report.id === reportId) : undefined;
      if (found) {
        setExisting(found);
        setName(found.name);
        setDescription(found.description ?? '');
        setQuery(found.query);
        setChart(found.chart);
        setLabelColumn(found.labelColumn ?? '');
        setValueColumn(found.valueColumn ?? '');
        setShared(found.visibility === 'workspace');
      } else if (reportId) {
        setError('This report does not exist or is not shared with you.');
      }
      setLoading(false);
    }).catch(reason => { if (!controller.signal.aborted) { setError(problem(reason, 'The report could not be loaded.')); setLoading(false); } });
    return () => controller.abort();
  }, [client, reportId]);
  const runPreview = async () => {
    setPreviewError('');
    try {
      setPreview(await client.query(query, {pageSize: CHART_ROWS}));
    } catch (reason) {
      setPreview(null);
      setPreviewError(queryProblem(reason));
    }
  };
  const columns = preview?.columns ?? [];
  const values = columns.filter(column => numeric.has(column.type));
  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError('');
    const input: ReportInput = {
      name: name.trim(),
      description: description.trim() || null,
      query: query.trim(),
      chart,
      label_column: chart === 'table' || chart === 'metric' ? null : labelColumn || null,
      value_column: chart === 'table' ? null : valueColumn || null,
      visibility: shared ? 'workspace' : 'private',
    };
    try {
      const saved = existing ? await client.updateReport(existing.id, existing.version, input) : await client.createReport(input);
      onNavigate(reportRoute(saved.id));
    } catch (reason) {
      setError(problem(reason, 'The report could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  if (loading) return <div className="content-canvas"><Skeleton lines={6} /></div>;
  if (existing && !existing.canEdit) return <div className="content-canvas">
    <header className="page-header"><div><p className="page-header__breadcrumb">Reports</p><h1 ref={headingRef} tabIndex={-1}>Edit {existing.name}</h1></div></header>
    <Alert variant="warning" title="Only the report's owner or someone who manages reports can change it." />
    <Button variant="secondary" onClick={() => onNavigate(reportRoute(existing.id))}>Open the report</Button>
  </div>;
  return <div className="content-canvas">
    <header className="page-header"><div>
      <p className="page-header__breadcrumb"><a href={reportsRoute} onClick={event => { event.preventDefault(); onNavigate(reportsRoute); }}>Reports</a></p>
      <h1 ref={headingRef} tabIndex={-1}>{existing ? `Edit ${existing.name}` : 'New report'}</h1>
    </div></header>
    <form className="object-form" onSubmit={event => void save(event)} aria-label={existing ? 'Edit report' : 'New report'}>
      {error && <Alert variant="danger" title={error} />}
      <TextField label="Name" required maxLength={80} value={name} onChange={event => setName(event.target.value)} />
      <TextField label="Description" maxLength={1000} value={description} onChange={event => setDescription(event.target.value)} />
      <div className="ui-field">
        <label className="ui-field__label" htmlFor={queryId}>Query</label>
        <span className="ui-field__description" id={`${queryId}-help`}>A record query, such as <code>{example}</code>. It runs as whoever opens the report, so each person sees only their records.</span>
        <textarea id={queryId} className="ui-field__control object-textarea query-console__text" aria-describedby={`${queryId}-help`} spellCheck={false} required value={query} onChange={event => { setQuery(event.target.value); setPreview(null); }} />
      </div>
      <div className="object-page__actions"><Button type="button" variant="secondary" onClick={() => void runPreview()}>Preview</Button></div>
      {previewError && <Alert variant="danger" title={previewError} />}
      <Select label="Show as" value={chart} onChange={event => setChart(event.target.value as ReportChart)}>
        {reportCharts.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
      </Select>
      {chart !== 'table' && !preview && <p className="object-form__meta">Preview the query to choose the columns to chart.</p>}
      {chart !== 'table' && chart !== 'metric' && preview && <Select label="Label each value with" required value={labelColumn} onChange={event => setLabelColumn(event.target.value)}>
        <option value="">Choose a column</option>
        {columns.map(column => <option key={column.key} value={column.key}>{column.key}</option>)}
      </Select>}
      {chart !== 'table' && preview && <Select label="Value" required value={valueColumn} onChange={event => setValueColumn(event.target.value)}>
        <option value="">Choose a number column</option>
        {values.map(column => <option key={column.key} value={column.key}>{column.key}</option>)}
      </Select>}
      {(canManage || existing?.visibility === 'workspace') && <label className="object-checkbox"><input type="checkbox" checked={shared} onChange={event => setShared(event.target.checked)} />Share with the workspace</label>}
      <div className="object-page__actions">
        <Button type="submit" loading={saving} disabled={!name.trim() || !query.trim()}>Save report</Button>
        <Button type="button" variant="secondary" onClick={() => onNavigate(existing ? reportRoute(existing.id) : reportsRoute)} disabled={saving}>Cancel</Button>
      </div>
    </form>
    {preview && <section className="object-section" aria-label="Preview">
      <ReportChartView report={{name: name || 'Preview', chart, labelColumn: labelColumn || null, valueColumn: valueColumn || null}} result={preview} />
      <QueryResults result={preview} onNavigate={onNavigate} />
    </section>}
  </div>;
}
