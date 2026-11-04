# 0027: Saved reports

## Status

Accepted (2026-10-13). Builds on ADR 0019 (record query language) and ADR
0016 (sharing). Part of epic #292 (issue #313, reports). Implemented in
`platform_objects::reports`, the API's report routes, PostgreSQL migration
0030 and SQLite tenant step 0015. It adds the `reports.manage` system
permission.

## Context

Salesforce reports answer questions about records: how many open deals
each owner has, the pipeline by stage, revenue by month. Limiara already
has a query language that filters, groups and aggregates with sharing
applied (ADR 0019). What it lacks is a way to keep a question, name it,
show it as a chart and hand it to colleagues.

## Decision

### 1. A report is a saved query

A report keeps:
- a name and an optional description;
- a query in the record query language;
- how to show its rows: a table, a bar, line or pie chart, or a single
  number, with the columns the chart plots;
- who sees it: its owner, or the whole workspace.

There is no second query model. Report types, groupings and aggregates are
what the query language already expresses (`GROUP BY`, `COUNT()`, `SUM()`
and the rest), so a report can do exactly what a query can, with the same
limits.

### 2. Run as the viewer

A report is run as whoever opens it, through `ObjectService::query`, with
that member's object permissions and record sharing. Sharing a report
shares the question, never the answers: two members opening the same
report see the rows each may see. A report stores no results, and its
events carry no query text, because a query can name the values it
filters on.

### 3. Checked when saved

Saving a report runs its query once, as the person saving it, so a report
that would fail to run is refused with the query language's own codes,
addressed to `query`. The chart columns must be columns of the query, and
the value column a number, currency or percent column. A query that stops
working later, after a field is deleted, fails when it is opened, with the
same codes.

### 4. Who changes what

Any member can save private reports. Sharing a report with the workspace,
and changing or deleting another member's report, needs `reports.manage`,
which also shows private reports. Changes are versioned like every other
definition.

### 5. Shown in the shell, charts drawn in place

The shell lists reports under `/reports` from the UI contribution endpoint.
Charts are inline SVG drawn from the rows, using the theme's tokens, and the
rows are always shown beneath as a table, which is the chart's accessible
alternative. No charting script is loaded.

## Consequences

- Reports cost what their query costs, every time they are opened, within
  the query language's limits. Large aggregates move to the analytics
  store with #284.
- A report's audience is the workspace or one member. Folders and finer
  sharing come later.

## Follow-up

Tracked in `docs/roadmap/README.md`:
- a report builder that writes the query from chosen fields, groupings and
  filters;
- matrix reports (grouped rows and columns);
- folders with sharing;
- scheduled email subscriptions;
- CSV and XLSX export;
- dashboards of report components (#314).
