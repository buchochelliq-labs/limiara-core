# 0028: Related lists and lookup filters

## Status

Accepted (2026-10-13). Builds on ADR 0025 (master-detail relationships),
ADR 0019 (record query language) and ADR 0016 (sharing). Part of epic #292
(issue #295, relationships). Implemented in `platform_objects::related`,
the API's related-list and candidate routes, PostgreSQL migration 0031 and
SQLite tenant step 0016.

## Context

A lookup or master-detail field points from one record to another. Before
this decision, the record pointed at could not show what points at it, so
an account page did not list its contacts. A lookup also took any record of
its target object that the member could see. Salesforce offers both:
related lists on the record page, and lookup filters that limit the
records a lookup may take, such as only active accounts.

`record_references` already keeps every reference with an index from the
target, and the query language already filters on reference fields with
sharing applied.

## Decision

### 1. Related lists are queries run as the viewer

Every active lookup or master-detail field that points at an object gives
that object's records one related list, when the viewer can read the
pointing object. A related list is the query
`SELECT id, name FROM <child> WHERE <field> = '<record>' ORDER BY name`,
run as the viewer. So object permissions, sharing, master-detail
visibility and the recycle bin apply exactly as they do to any query. Two
viewers of the same account may see different contacts under it.

The record page shows the first 5 records of each list, for at most 20
lists, ordered by the pointing object's plural label and then the field's
label. Further records come a page of 50 at a time with the query's own
cursor. Nothing is stored for related lists; there is no layout to keep in
step with the fields.

### 2. A lookup filter is a condition in the query language

A lookup or master-detail field may keep a `lookup_filter`: one condition
on the target object, as written after `WHERE`, such as
`custom:active = true`. It takes up to 1,000 characters. When it is set,
the engine checks that it is a condition and nothing more: no `ORDER BY`,
`GROUP BY` or `LIMIT`. It also checks that
`SELECT id FROM <target> WHERE (<filter>)` runs for whoever sets it. Its
problems come back under `lookup_filter` with the query language's codes.
There is no second filter language to learn.

### 3. Only new values must meet the filter

When a member creates or changes a record and sets a filtered lookup to a
new value, the engine runs
`SELECT id FROM <target> WHERE id = '<value>' AND (<filter>)` as that
member, in the write's own transaction. A target that does not match
fails with `records.lookup_filter_mismatch`.

A value the change leaves as it was is kept, even if its target no longer
matches. This is the rule that already keeps a lookup whose target the
editor cannot see. Setting or changing a filter does not change
existing values.

As with target visibility, automation writes are not bound by the filter:
an automation sets what its author configured.

A filter can stop running, for example when a field it names is deleted.
It then refuses every new value with `records.lookup_filter_invalid` until
an administrator fixes or removes it, rather than letting any value
through.

### 4. The picker offers only records within the filter

`GET .../fields/{field}/candidates` lists the target records the caller
sees that meet the filter, by name, 50 to a page. With `search`, it lists
only those whose name contains the text. The record form's lookup picker
uses it, so members choose from records that will be accepted.

## Consequences

- Related lists cost one query per list on each record page, at most 20.
  Each query uses the `record_references` target index.
- A filter that names a later-deleted field fails closed, so an
  administrator must fix or remove it.
- Clearing a nullable setting with `null` now works for every engine
  update input (descriptions, defaults, report columns, conditions). The
  JSON reader had treated `null` as "left out", so those values could be
  set but never cleared.

## Follow-up

Recorded in `docs/roadmap/README.md`:
- filters that compare the target with the record being edited;
- optional filters that only warn;
- creating a related record from its list with the lookup filled in;
- choosing a related list's columns and order;
- related lists that cross a junction object.
