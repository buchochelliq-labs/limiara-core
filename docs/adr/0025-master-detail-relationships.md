# 0025: Master-detail relationships

## Status

Accepted (2026-10-12). Builds on ADR 0015 (object engine) and ADR 0016
(ownership and sharing). Part of epic #292 (issue #295, relationships).
Implemented in `platform_objects` (`FieldType::MasterDetail`, `sharing`,
`access` and the record service), PostgreSQL migration 0028 and SQLite
tenant step 0013. It changes the sharing boundary of ADR 0016: a detail
record's access comes from another record.

## Context

Lookups (ADR 0015) link records loosely: each record keeps its own owner
and sharing, and deleting the target clears or blocks the link. Salesforce
also has master-detail relationships, where the child (the detail) belongs
to its parent (the master): an order's lines, an invoice's items, the
junction record between a contact and a campaign. The detail has no life of
its own. Who can see it, and whether it exists at all, follows the master.

## Decision

### 1. A field type

`master_detail` is a field type next to `lookup`. It points to one object
(`reference_object`), is always required, and has no delete behaviour,
default or uniqueness. Everything that follows lookups (queries, formulas,
validation rules, automations, `related` names) follows master-detail
fields the same way: both are reference fields.

An object may have at most two master-detail fields; two make a junction
object. Masters cannot themselves be details, and an object cannot be its
own master, so there is one level of master-detail. An object can get a
master-detail field only while it has no records, because every existing
record would need a master. A master-detail field cannot become optional.

### 2. Sharing follows the master

The first master-detail field created is the primary one. A detail
record's rights come from the caller's rights on its primary master:

- reading needs read on the master;
- creating, editing and deleting need edit on the master;
- details have no manual shares and no owner of their own to transfer.

The object's own default access and hierarchy setting do not apply.
View all and modify all on the detail object still bypass the master, as
they do everywhere. Object permissions still cap what sharing allows, and a
detail object is only accessible at all to roles that can read every one
of its master objects.

Lists, queries, relationship hops and search evaluate the same rule at
query time: the visibility predicate gains an optional master field, and
when it is set the owner and share test applies to the master reached
through `record_references` instead of to the row. Nothing is materialised,
so transferring a master or sharing it changes what its details show at
once, exactly as for the master itself.

### 3. Details stay with their master

A new detail needs a master the member can edit
(`records.master_read_only`), and a detail cannot move to another master
(`records.master_locked`), whoever writes it, automations included.
Moving details to another master is a later, opt-in setting (see Follow-up).

### 4. The recycle bin cascades

Deleting a master moves its details to the recycle bin in the same
transaction, after the usual lookup handling (restrict blocks, clear
clears) for each detail. Each detail leaves its own deletion event and
history; record-triggered automations do not run for cascaded deletions.
`records.deleted_with` keeps the master whose deletion took the detail, so
restoring that master restores those details, and only those: a detail
deleted on its own before stays in the bin. A junction record whose other
master is still in the bin stays there, and comes back when that master
does. A detail whose master is in the bin cannot be restored on its own
(`records.master_deleted`).

### 5. Concurrent changes

On PostgreSQL, where transactions interleave:
- adding a master-detail field locks the object's definition row, and a
  record insert takes a shared lock on it, then re-counts the
  master-detail fields; an insert that read the object before the field
  committed fails with `records.definition_changed` instead of saving a
  record without a master;
- a new detail takes a shared lock on its master and reads it again, and
  deleting a master locks it before looking for details, so a delete
  either sees the new detail and cascades to it, or the detail sees the
  master as deleted.

SQLite workspace transactions begin `IMMEDIATE` and are serialised
already.

## Consequences

- Sharing has one more input. Every place that decides visibility goes
  through `ObjectService::visibility_of` and `rights_on`, and both stores'
  list queries and the query writer render the master rule, so the
  conformance suite runs the same scenario on PostgreSQL and SQLite.
- `ObjectAccess` knows which objects are details, from one small query per
  request.
- One level of master-detail keeps visibility to a single join and cascades
  to one step.

## Follow-up

Tracked in `docs/roadmap/README.md`:

- roll-up summary fields on masters (#295);
- related lists and lookup filters (#295);
- multi-level master-detail, moving details between masters as an opt-in field setting, and
  converting a lookup into a master-detail field on an object with records
  once every record has a master.
