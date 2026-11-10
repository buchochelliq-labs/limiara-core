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

## Change source

Added for #233 (PostgreSQL migration 0040, SQLite tenant step
`0030_change_sources`). Every history entry and audit entry records what the
change came through, as a closed kind with an optional detail:
`{"kind": "device", "detail": "<device id>"}`. The kinds are `web`,
`api_client` (client id), `device` (device id), `automation` (API name),
`quick_action` (API name), `assistant` (`siri`, `gemini`), `import` (bulk job
id) and `system`. This is a section here rather than a new ADR because it
adds provenance to existing evidence without moving a platform boundary.

1. **Provenance only.** The source never changes authorization; it travels
   in `MutationContext` next to the correlation id and is written by the
   engine, so no plugin or caller can skip it.
2. **Set by the server, from what it verified.** The workspace API sets
   `web` for a browser session and `api_client` or `device` with the id
   behind the bearer token that authenticated the request. Automations name
   themselves for the saves they make (an after-save action; a before-save
   automation's values are part of the member's own save and carry its
   source), bulk job rows name their job as `import`, and plugin lifecycle
   changes are `system`. A device's replica records `device` for its own
   changes, and its command bridge names the signed-in device. No request
   header or body field names a source, so a client cannot claim to be an
   automation, an assistant or another client.
3. **Quick actions and assistants are reserved.** The web runs quick
   actions through the ordinary create and update routes, so those changes
   record `web`; recording `quick_action` needs a claim the server checks
   against the object's actions and is a follow-up. `assistant` will come
   only from a device's local assistant bridge (#232), never over HTTP. A
   device's push records `device` with the verified device id; a source
   the device kept locally (such as a quick action) is not sent, so nothing
   the device says widens what the server records.
4. **Details are ids and names, never free text.** A detail is at most 64
   plain ASCII characters. Audit metadata keeps the whole source; outbox
   events carry only its kind, so a device or client id is not routed with
   events.
5. **No backfill.** Entries written before migration 0040 have no source and
   read as `null`, shown as nothing in the History panel; the panel shows
   "via web app", "via device", "via automation …" and so on, as text.

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
