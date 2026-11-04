# 0020: Global search

## Status

Accepted (2026-10-12). Builds on ADR 0015 (object engine), ADR 0016
(sharing) and ADR 0019 (query language). Issue #305, part of epic #292.
Implemented in `platform_objects::search`, migration 0019, `GET /search` and
the shell search bar.

## Context

Salesforce global search finds records across every searchable object from
one box: names and fields marked searchable, results grouped by object, only
records the person can see, and instant results while typing. It runs on a
separate search index that is updated asynchronously, so a record saved a
moment ago may not be found yet.

Limiara's engine already has the pieces search needs: objects and fields
carry a `searchable` flag (ADR 0015), sharing is a predicate the store can
add to any SQL (ADR 0016, ADR 0019), and record writes maintain typed pivots
in the same transaction. The question was where the index lives and how it
stays current.

## Decision

### 1. An inverted index in the engine's own tables

`record_search_terms` holds one row per distinct term of each record:
`(tenant_id, term, record_id)` as the primary key, with the object and a
weight. It is tenant-first, carries `tenant_id`, and forces row-level
security on PostgreSQL like every engine table; on SQLite it lives in the
workspace's file.

- **What is indexed:** the record name (weight 3) and the values of fields
  marked searchable (weight 1). Only text, long text, email, phone and URL
  fields can be searchable, as before.
- **Terms:** values are lowercased (Unicode) and split on anything that is
  not a letter or digit; terms are cut to 40 characters, and a record keeps
  at most 500 distinct terms, name terms first. Phone numbers also index
  their digits run together, so `447946` finds `+44 7946 0958`.
- **Kept current in the write transaction.** Terms are written with the
  pivots whenever a record is created, changed, deleted or restored, so a
  saved record is findable immediately and the index can never disagree
  with the record. Changing a field's `searchable` flag reindexes that
  object's records in the same transaction. The index ignores the object's
  own `searchable` flag, which is applied at query time, so switching an
  object off and on needs no reindex.

The issue proposed PostgreSQL full-text search first. We chose our own
table instead because it behaves identically on PostgreSQL and SQLite
(ADR 0013), supports prefix matching on every term with a plain B-tree
range scan on a byte-ordered (`COLLATE "C"`) column, and needs no
language configuration: workspaces write in many languages, and the
`simple` text-search configuration we would have used does no stemming
anyway. The index is behind the store port, so an external engine can
replace it later (§4).

### 2. Matching and ranking

- The query is split into terms the same way, at most eight. Every term
  must match the start of an indexed term (`acm ind` finds "Acme
  Industries"). Queries need at least two characters.
- Each record scores, per query term, its best weight for that term plus
  one when the term matches exactly rather than as a prefix; scores add up
  across terms. Results sort by score, then name, then ID. Recently viewed
  records are not boosted yet: that needs the recently viewed service
  (#285) and is listed in the roadmap.

### 3. Permissions and sharing

- Only active objects that are searchable and that the caller can read are
  searched.
- Each object's record visibility (ADR 0016) is part of the SQL, the same
  predicate queries use, so a hidden record is never returned or counted.
  Records in the recycle bin are never returned.
- Results show the record name and the values of its searchable fields,
  which the caller can read because they can read the record. Field-level
  security (#316) will remove fields the caller cannot read.

### 4. Results and the API

`GET /api/v1/workspaces/:workspace/search?q=…&object=…&limit=…` returns
groups, one per object with matches, ordered by their best result:

```json
{
  "data": [
    {
      "object": "custom:account", "label": "Account", "plural_label": "Accounts",
      "records": [{ "id": "0a1000000000000001", "name": "Acme Industries", "fields": { "custom:email": "sales@acme.example" } }],
      "has_more": false
    }
  ]
}
```

`limit` is the number of records per object (default 5, at most 50, or 200
with `object`), and `object` restricts the search to one object for a full
results page.

### 5. In the shell

The search bar in the header becomes a combobox: after two characters it
shows the top results per object as you type, the arrow keys move through
them, Enter opens one, and "See all results" opens a results page with a
tab per object. `/` and Ctrl+K (⌘K) focus it.

## Alternatives considered

- **PostgreSQL `tsvector` with GIN.** Fine on PostgreSQL, but it needs a
  second implementation on SQLite (FTS5) with different tokenising and
  ranking, so results would differ by backend. Prefix matching needs
  `:*` queries, and tenant isolation relies on the GIN index plus a
  separate tenant filter.
- **Update the index from change events asynchronously**, as Salesforce
  does. It decouples writes from indexing but makes new records
  unfindable for a while and needs a worker per backend. Indexing a few
  hundred short terms per write is cheap enough to do in the transaction;
  an external engine can still follow the change log (ADR 0018) later.
- **Search with `LIKE '%term%'` on names.** No index can serve it, so it
  scans every record of every object.

## Consequences

- Searchable fields cost a few rows per record in `record_search_terms`.
- Ranking is simple and explainable; relevance features (recency boosts,
  synonyms, typo tolerance, highlighting on the server) remain open on #305
  together with an external engine and plugin objects with typed stores
  (Contacts) joining the index (#318).
