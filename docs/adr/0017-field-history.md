# 0017: Field history tracking

## Status

Accepted (2026-10-11). Builds on ADR 0015 and ADR 0016. Issue #311, part of
epic #292. Implemented in migration 0017, the engine's `track` and
`record_history`, `GET .../records/{id}/history` and the record page's
History section.

## Context

Salesforce field history tracking keeps the old and new value of up to 20
fields per object, for 18 to 24 months, with longer retention sold
separately as Field Audit Trail. History is a separate store that is written
after the change and queried on its own.

Limiara already writes an audit row and an outbox event in the same
transaction as every engine change. Those carry names, never values, so
they can be routed and retained without holding personal data (ADR 0015
section 5). Field history needs the values, so it cannot reuse them as they
are, but it can follow the same discipline.

## Decision

1. **Opt in per object, then per field.** An object's `track_history` keeps
   its records' lifecycle (created, moved to the recycle bin, restored),
   name and owner changes. Each field's `track_history` adds its old and new
   values. Field flags have no effect while the object does not track
   history. There is no limit on the number of tracked fields.
2. **Written in the same transaction as the change.** The engine writes
   `record_history` rows alongside the record, its pivots, its audit row and
   its event, so history can never disagree with the record. Lookup values
   cleared because their target was deleted are recorded on the referencing
   record.
3. **Values live only in history.** Events and audit still carry no values.
   `record_history` has forced row-level security and is append-only for the
   application role: `UPDATE`, `DELETE` and `TRUNCATE` are revoked on
   PostgreSQL. On SQLite the store only inserts.
4. **Read through the engine.** History is visible to callers who can see
   the record (ADR 0016), including records in the recycle bin. Entries of
   retired fields are hidden. Lookup values are named only when the caller
   can see the target. Field-level security (#316) will hide entries for
   fields the caller cannot read.
5. **Retention follows the record.** History rows are removed with their
   record when it is purged; nothing else deletes them. Per-workspace
   retention limits, export and archival to object storage are follow-ups
   recorded in the roadmap.

## Alternatives considered

- **Deriving history from audit and events.** Would need values in audit
  and events, which would make every consumer and log a personal-data
  store.
- **Salesforce's 20-field limit.** It exists to bound storage in a shared
  multi-tenant table; Limiara's history is tenant-first indexed and opt-in,
  so the limit buys nothing.

## Consequences

- Administrators can track any number of fields, and history is exact
  because it commits with the change.
- History storage grows with tracked changes; a retention policy is needed
  before large workspaces track busy fields.
