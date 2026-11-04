# 0019: Record query language

## Status

Accepted (2026-10-11). Builds on ADR 0015 (object engine), ADR 0016
(sharing) and ADR 0018 (change data capture). Issue #304, part of epic #292.
Implemented in `platform_objects::query` and `GET /query`.

## Context

Salesforce's SOQL lets integrations, reports and code read records of any
object with one language: typed filters, relationship traversal, aggregates
and paging, always with the caller's sharing applied. The engine's record
lists (ADR 0015) offer equality filters on indexed fields for one object at a
time. Search (#305), reports (#307), formulas and automation (#298) all need
something richer, and they must not each grow their own filter syntax or
their own way of applying sharing.

The query must run identically on PostgreSQL and SQLite (ADR 0013), never
expose another workspace's data, never let a caller learn about records they
cannot see, and never let one query monopolise the database.

## Decision

### 1. The language

```
SELECT name, custom:stage, custom:amount, custom:account.name
FROM custom:deal
WHERE custom:stage IN ('won', 'negotiation')
  AND custom:amount >= 1000
  AND (custom:close_date = THIS_MONTH OR custom:close_date = NEXT_N_DAYS:30)
  AND NOT custom:account.name LIKE 'test%'
ORDER BY custom:amount DESC, name
LIMIT 500
```

- **Names** are API names (ADR 0015 §3): `namespace:name` for custom and
  plugin fields and objects, bare names for the standard fields of ADR 0014
  (`id`, `name`, `owner`, `created_at`, `created_by`, `updated_at`,
  `updated_by`, `touched_at`, `version`, `is_deleted`, `deleted_at`). A dot
  follows a lookup to its target: `custom:account.name`,
  `custom:account.custom:region.name`, up to three hops.
- **Keywords** are case-insensitive; names are matched exactly.
- **Literals** are typed: `'text'` (with `\'`, `\\`, `\n`, `\t`, `\%`, `\_`
  escapes), numbers, `true`, `false`, `null`, dates (`2026-10-11`), date-times
  (`2026-10-11T09:30:00Z`) and relative dates: `TODAY`, `YESTERDAY`,
  `TOMORROW`, `THIS_WEEK`, `LAST_WEEK`, `NEXT_WEEK`, `THIS_MONTH`,
  `LAST_MONTH`, `NEXT_MONTH`, `THIS_YEAR`, `LAST_N_DAYS:n` (the last n days
  including today) and `NEXT_N_DAYS:n` (the next n days after today). A
  literal must suit the field's type, so `custom:amount = 'ten'` is an error,
  not an empty result.
- **Conditions**: `=`, `!=` (also `<>`), `<`, `<=`, `>`, `>=`, `IN (…)`,
  `NOT IN (…)`, `LIKE` (`%` and `_` wildcards), `NOT LIKE`, `= null`,
  `!= null`, and `INCLUDES (…)` / `EXCLUDES (…)` for multi-select picklists,
  combined with `AND`, `OR`, `NOT` and parentheses.
- **Dates are ranges.** A date or relative date compared with a date or
  date-time field means the whole period: `= TODAY` is "on or after the start
  of today and before the start of tomorrow", `> TODAY` is "from tomorrow".
  Periods are computed in UTC until workspaces have a time zone.
- **Text comparisons ignore case**, as names and unique values already do
  (ADR 0015): text, email, phone and URL values are compared through their
  lowercase key. Picklist values compare exactly.
- **No value is not a value.** A positive condition is false for a record
  without a value, so its negation (`!=`, `NOT IN`, `NOT LIKE`, `EXCLUDES`,
  `NOT …`) includes such records, as in SOQL. Conditions are therefore
  two-valued and `NOT` behaves classically.
- **Aggregates**: `COUNT()`, `COUNT(field)`, `SUM`, `AVG`, `MIN` and `MAX`,
  optionally aliased (`SUM(custom:amount) total`), with `GROUP BY` on up to
  three fields. Every selected field that is not aggregated must be grouped.
  `ORDER BY` may name a grouped field, an aggregate or an alias.

### 2. What can be filtered, sorted and grouped

Only what an index answers:

- standard fields, which are columns with tenant-first indexes;
- custom fields that are indexed (ADR 0015 §2: `indexed`, `unique`, and
  picklists always), through the typed `record_index` pivot;
- lookups, through `record_references`.

Any field can be selected; a field that is not indexed in `WHERE`,
`ORDER BY` or `GROUP BY` is `query.field_not_indexed`, the same rule as
record lists. This is the main cost control: every condition is an indexed
probe, never a scan of record documents, and it behaves the same on every
backend because pivots are typed columns with the same byte ordering.
Long text and multi-select picklists cannot be sorted or grouped; lookups
cannot be sorted or aggregated but can be grouped (by the target's ID).

### 3. Sharing and the recycle bin, applied to every query

- The object in `FROM` must be readable (object permission), otherwise it
  is `query.object_unknown`, exactly like a missing object.
- Record visibility (ADR 0016) is the same predicate record lists use, added
  to the compiled SQL, so a query never returns, counts, sums or groups a
  record the caller cannot see.
- **Every relationship hop applies the target object's permission and
  sharing too.** A condition on `custom:account.name` is true only when the
  account is visible and not in the recycle bin, so filtering cannot reveal
  a hidden account's name; a selected `custom:account.name` is `null` when
  the account is not visible. A lookup whose target object the caller cannot
  read is `query.relationship_unknown`.
- Records in the recycle bin are excluded unless the request sets
  `include_deleted=true`, which adds them (they can then be told apart with
  `is_deleted`). Targets reached through a lookup are never in the recycle
  bin.
- Field-level security (#316) will plug into the same resolver: an
  unreadable field will be reported exactly like an unknown one.

### 4. Compilation

The query is parsed into a syntax tree, resolved against the object and
field metadata into a backend-neutral plan (field ids, typed values,
resolved date ranges and visibility per hop), and only then rendered to SQL
by a renderer shared by both backends with one dialect switch for
placeholders. Every value is a bound parameter; no name or literal from the
query text ever reaches SQL text. Conditions on pivots compile to `EXISTS`
probes on the tenant-first pivot indexes, relationship hops to nested
`EXISTS` through `record_references`, sort and group keys to `LEFT JOIN`s on
single-valued pivots. The query runs in the workspace transaction, so
PostgreSQL row-level security and the per-workspace SQLite file still bound
it to one workspace.

### 5. Paging and limits

- Record queries page with keyset cursors over the `ORDER BY` keys plus the
  record ID (default order: `name`, then ID); records without a value sort
  last in both directions on every backend. The cursor is bound to the query
  text and the `include_deleted` flag, and carries how many rows `LIMIT` still
  allows. Pages hold up to 200 rows (`page_size`).
- Aggregate queries return at most 2,000 groups in one response; more
  without an explicit `LIMIT` is `query.too_many_groups`.
- Limits, all `query.too_complex` when exceeded: 10,000 characters of query
  text, 100 selected items, 50 conditions, 200 values per `IN` list, three
  relationship hops, 10 distinct relationship paths, three `ORDER BY` keys,
  three `GROUP BY` fields and a `LIMIT` of at most 50,000.
- On PostgreSQL each query runs with a 10-second statement timeout; a query
  that exceeds it fails with `query.timeout` (503) and has changed nothing.

### 6. The endpoint

`GET /api/v1/workspaces/:workspace/query?q=…&cursor=…&page_size=…&include_deleted=…`
returns:

```json
{
  "object": "custom:deal",
  "columns": [{ "key": "name", "type": "text" }, { "key": "custom:account.name", "type": "text" }],
  "data": [{ "id": "0a1000000000000001", "name": "Big deal", "custom:account.name": "Acme" }],
  "next_cursor": "…"
}
```

Rows are keyed by each selected item as written (or its alias). Record rows
always carry `id`. User fields (`owner`, `created_by`, `updated_by`) are user
IDs and lookups are the target's public ID; select `custom:account.name` for
the target's name. Errors are problem responses with `field: "q"` and a
`detail` that names the position in the query text.

## Alternatives considered

- **Expose SQL or a JSON filter tree.** SQL would bypass metadata, sharing
  and the pivots; a JSON tree is fine for machines but is a second language
  people would still have to learn, and SOQL-shaped text is what Salesforce
  administrators and developers already know. The plan is effectively that
  tree, and a JSON form of it can be added later without a new semantics.
- **Filter any field through the record document** (`data->>'x'` and
  `json_extract`). It works on small objects but turns every non-indexed
  filter into a scan whose cost grows with the object, and JSON comparisons
  differ between backends. Administrators can index the fields they filter
  on, as Salesforce asks them to with custom indexes.
- **Apply sharing after the query in Rust.** Simple for record rows, but
  aggregates and `LIMIT` would be wrong and parent conditions would leak
  through counts. Sharing has to be part of the SQL.
- **Leave hidden parents out of conditions but not of selected values.**
  Salesforce returns parent fields only through sharing too; any other
  choice leaks.

## Consequences

- Search, reports, list views and automation can be built on the plan
  instead of their own filters.
- Administrators must index fields before they can filter on them; the
  Object Manager already offers the switch.
- Child subqueries (`(SELECT … FROM children)`), `HAVING`, date functions in
  `GROUP BY` (`CALENDAR_MONTH`), traversal into users (`owner.name`),
  workspace time zones and a `POST` form for very long queries remain open
  on #304 and are listed in the roadmap.
