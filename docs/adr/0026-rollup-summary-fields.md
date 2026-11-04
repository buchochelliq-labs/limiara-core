# 0026: Roll-up summary fields

## Status

Accepted (2026-10-13). Builds on ADR 0025 (master-detail relationships) and
ADR 0021 (stored formula values). Part of epic #292 (issue #295,
relationships). Implemented in `platform_objects` (`FieldType::Rollup`,
the record service), PostgreSQL migration 0029 and SQLite tenant step 0014.

## Context

A master often needs a figure about its details: an order's line count and
total, an account's first and latest purchase. Salesforce keeps these as
roll-up summary fields on the master. Computing them when a record is read
would cost one aggregate per record in every list, and could not be
filtered or sorted on without scanning every detail.

## Decision

### 1. A field type on the master

`rollup` is a field type on a master object. It names:

- the detail object and the master-detail field on it that points here;
- the function: `count`, `sum`, `min` or `max`;
- for anything but a count, the detail field it summarises.

Sums take number, currency and percent fields. Minimums and maximums also
take dates and date-times. A count gives a whole number; otherwise the
result has the summarised field's type and scale, kept in `formula_type`,
as for formulas. The summarised field is indexed when the roll-up is
created, because roll-ups read the index rows, and it cannot stop being
indexed while a roll-up reads it. An object has at most 25 roll-ups.

### 2. Stored on the master, kept current

The value is stored in the master record like any other value, so it is
read, indexed, filtered, sorted and synced like one. It is read-only:
writing it fails with `records.field_read_only`, for members and
automations alike.

Whenever a detail is created, changed, deleted or restored, the engine
recomputes the roll-ups of the details' masters in the same transaction:

1. lock the master;
2. read it again;
3. aggregate the master's details outside the recycle bin, from the index;
4. write the master if a value changed.

That write is a system update of the master. It bumps the version, stamps
the editor, and leaves a change event and fresh index rows, but no history
and no automations. A change to a detail that leaves every summary as it
was leaves the master alone.

A new master starts with a count and sum of zero, and an empty minimum and
maximum. A roll-up created on an object that already has records computes
each one. Creating a roll-up locks the master object, so a master
inserted meanwhile either is backfilled or, having read the object before
the field committed, fails with `records.definition_changed` to retry. A
roll-up's settings stay those of a computed field: it cannot be made
required, tracked or defaulted (`objects.rollup_settings_invalid`). Restoring a master recomputes its roll-ups, so they count the
details that come back with it.

### 3. Locks, masters first

Updating the master from a detail's save means every path that writes a
detail locks the detail's masters first, exclusively, and only then the
detail: creating or changing a detail, deleting one, and restoring one.
Deleting a master takes the master and then its details, the same order,
so saving a detail and deleting its master cannot deadlock. A junction
record is the exception: saving one locks both masters in field order,
while deleting one master locks the other last, through the cascade. If
PostgreSQL detects that deadlock it fails one of the two transactions,
which the API reports as `503 platform.unavailable`, to retry.
Concurrent changes to details of one master take turns, each counting the
others' committed details. A concurrent edit of the master itself fails with a version
conflict instead of overwriting the new summary.

### 4. What a roll-up reads stays

While a roll-up reads them, these cannot be deleted:

- the summarised field (`objects.field_in_use`);
- the master-detail field (`objects.field_in_use`);
- the detail object (`objects.object_in_use`).

## Consequences

- Detail saves do more work: one aggregate per roll-up on each master, and
  the master's write. Objects without roll-ups pay only the master lock of
  ADR 0025.
- Masters' versions move when their details change, as their roll-ups are
  part of them. Clients that hold a master open see a version conflict and
  read it again.

## Follow-up

Tracked in `docs/roadmap/README.md`:

- roll-up filter criteria (summarise only details that match a condition);
- roll-ups over lookups rather than master-detail fields;
- averages;
- recalculating roll-ups in the background for very large masters;
- a lock order for junction records that cannot deadlock with a cascade
  from either master.
