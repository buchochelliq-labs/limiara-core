# 0018: Change data capture and incremental sync

## Status

Accepted (2026-10-11). Builds on ADR 0014, ADR 0015 and ADR 0016. Issues
#306 and #287, part of epic #292. Implemented in migration 0018, the
engine's change log, `GET /objects/changes`, `GET /objects/changes/stream`
and the `touched_after` list filter.

## Context

Salesforce Change Data Capture publishes an event for every record change,
with the changed field values, to a channel that subscribers replay from a
replay id for three days. Field-level security is applied per subscriber by
filtering the payloads. Integrations that miss the window fall back to
querying on `SystemModstamp`.

Limiara writes an audit row and an outbox event in the same transaction as
every engine change, and evaluates record visibility at query time (ADR
0016). The outbox is a delivery mechanism for internal consumers, not a
replayable public log, and its events deliberately carry no values.

## Decision

1. **One change log per workspace, written with the change.** Every record
   created, updated (including owner transfers and lookups cleared by a
   deleted target), deleted or restored appends to `record_changes` in the
   same transaction, with the record, object, kind, the changed field names,
   who, when and the correlation id. Sharing changes alter visibility, not
   data, and are not in the log.
2. **Gap-free positions in commit order.** A per-workspace counter row
   (`change_counters`) issues the next position and stays locked until
   commit, so positions are dense and follow commit order. A reader that
   resumes after position N can never miss a change that commits later with
   a smaller position, which a sequence would allow. Writers in one
   workspace serialise only for the moment between their first change and
   commit. On SQLite one writer at a time gives the same order.
3. **Names, not values.** Entries name the changed fields; subscribers read
   current values through the record API. Record visibility (ADR 0016) and
   field-level security (#316) therefore apply to every subscriber without
   a second filtering path, and the log holds no personal data. Changes to
   records a caller cannot currently see are withheld, but the returned
   position still advances past them, so clients never stall.
4. **Feed and stream with replay.** `GET /objects/changes` pages the log
   after a position. `GET /objects/changes/stream` sends server-sent events
   whose ids are positions and resumes from `Last-Event-ID`; it polls once a
   second when idle and ends after 15 minutes so the client reconnects
   through authentication again.
5. **72-hour retention, enforced by the database.** The application role
   may insert and read the log, and may delete only rows older than 72
   hours (a row-level security policy); it cannot update or truncate. The
   engine expires the log as it grows and records how far it has expired; a
   position before that point is `410 changes.position_expired`, and the
   client resynchronises with a sync list.
6. **Sync lists on `touched_at`.** Record lists accept `touched_after`,
   `sort=touched_at` and `status=any`, so a client can fetch everything
   touched since a moment, with deleted records as tombstones, using keyset
   cursors (#287).

## Alternatives considered

- **Values in change events, filtered per subscriber.** Matches Salesforce,
  but every subscriber needs its own field-level filter and the log becomes
  a personal-data store with its own retention obligations.
- **Exposing the outbox.** It is not ordered per workspace, is shaped for
  internal delivery and has no replay contract.
- **A sequence for positions.** Cheaper, but positions would not follow
  commit order and readers could skip changes.

## Consequences

- Integrations get an ordered, replayable feed, and a resynchronisation path
  for when they fall behind.
- Each change costs one more row and a brief per-workspace serialisation
  of commits that change records.
- Plugin record APIs (Contacts) still need the same `touched_after`
  contract, and workspace-defined platform events published by automation
  remain open in #306.
