# 0016: Record ownership and sharing

## Status

Accepted (2026-10-11). Builds on ADR 0015. Issue #286, part of epic #292.
Implemented in migration 0016, `platform-objects::sharing`, the `/sharing`,
`/owner`, `/users` and `/roles` routes, and the record page's Sharing panel;
both record stores pass the sharing conformance suite.

## Context

ADR 0015 gave every engine record an owner and every object a sharing model,
but nothing enforced them: anyone who could read an object could read and
change every record in it. Salesforce decides record visibility with five
layers: the owner, organisation-wide defaults, the role hierarchy, sharing
rules and manual shares. It materialises the result into per-object share
tables and recalculates them in background jobs whenever roles, groups,
rules or owners change. Those recalculations are a well-known operational
weakness: they lock rows, run for hours on large orgs, and leave access
briefly wrong while they run.

Limiara already has two properties Salesforce lacks: every query runs inside
one transaction under forced row-level security, and the PostgreSQL and
SQLite stores share one conformance suite. Sharing should use both rather
than copy the recalculation design.

## Decision

### 1. Layers, in Salesforce order

Access to a record is the union of these grants, always capped by the
caller's object permissions (ADR 0015 section 4):

1. **Object permissions decide the ceiling.** Without object `read` a record
   does not exist for the caller; without `edit` it cannot be changed, and
   without `delete` it cannot be deleted, whatever sharing says.
2. **View all and modify all bypass sharing**, as object permissions or the
   `records.view_all` and `records.modify_all` system permissions.
3. **The owner** reads, edits, deletes, shares and transfers their records.
4. **Organisation-wide default** per object: `private` (owner and grants
   only), `read` (everyone with object read sees every record, edits need a
   grant) or `read_write` (everyone with object access may read and edit).
5. **The reporting hierarchy**: a member sees and edits what anyone below
   them in the reporting line owns, when the object grants access using
   hierarchies (the default, as in Salesforce).
6. **Manual shares** grant `read` or `edit` on one record to a member or to
   every holder of a role.

Deleting needs ownership, the hierarchy above the owner, or modify all; an
`edit` share never grants delete. Managing a record's shares needs
ownership, the hierarchy above the owner, or modify all; grantees cannot
share the record further. Changing the owner needs edit access to the record plus one of:
being the owner, modify all, or the `records.transfer` system permission
(Salesforce "Transfer Record"). A manager therefore needs `records.transfer`
to move a report's records.

### 2. Query-time visibility, no recalculation

Visibility is evaluated by the store in the same SQL statement that reads
records: `owner_id` is one of the caller and their reports, or a share row
names the caller or one of their roles. The caller's reports are resolved
once per request from `reporting_lines` with a recursive query bounded at
64 levels and guarded against cycles, and passed to the engine with the
caller's roles, so the PostgreSQL and SQLite record stores evaluate the same
predicate.

Nothing is materialised, so there is nothing to recalculate: changing a
manager, a role or an owner changes access in the next statement. Lists stay
index-driven through `records (tenant_id, object_id, owner_id)` and
`record_shares (tenant_id, grantee_kind, grantee_id, record_id)`. If a
workspace ever outgrows query-time evaluation, a materialised closure table
can be added behind the same store method without changing callers.

### 3. Tables

- `record_shares`: tenant, record, grantee kind (`user` or `role`), grantee
  id, access (`read` or `edit`), reason (`manual`), created at and by. Forced
  RLS like every engine table; a share disappears with its record.
- `reporting_lines`: who each member reports to, both members of the same
  workspace, never the member themself. It is a separate table because
  memberships stay read-only to the application role (migration 0011);
  changing a line needs the `members.manage` permission and is audited.
- `object_definitions.grant_hierarchy`: whether the hierarchy grants access
  to the object's records.

### 4. Ownership transfer

Changing the owner is its own operation, audited and emitted as
`platform.objects.record-owner-changed.v1`. The new owner must be a member
of the workspace. As in Salesforce, a transfer removes the record's manual
shares, because they were granted by the previous owner.

### 5. Disclosure

A record the caller cannot see is reported exactly like a missing one (404),
in reads, updates, lookups and the `related` names of ADR 0015. A lookup to
an invisible record keeps its id and shows no name. Events and audit name
the grantee and the access level, never record values.

### 6. Deferred

Criteria- and owner-based sharing rules, public groups, queues as owners,
"controlled by parent" for master-detail (#295), and implicit sharing from
child records follow in later slices; they add grant sources to the same
query and do not change this model.

## Alternatives considered

- **Salesforce-style materialised share tables with recalculation.** Fast
  reads at any scale, but every role, rule or owner change becomes a
  background job and access is briefly wrong while it runs.
- **Using role membership as the hierarchy.** Limiara roles bundle
  permissions, like Salesforce profiles and permission sets; reusing them
  for reporting lines would conflate what someone may do with whose records
  they see.
- **Enforcing sharing in PostgreSQL RLS policies.** RLS remains the tenant
  boundary. Putting per-user sharing in policies would need user context in
  every session, would not exist on SQLite, and would make the conformance
  suite unable to prove the two stores agree.

## Consequences

- Private objects are now private: lists, reads, edits, deletes, lookups
  and related names all apply the same visibility.
- Changing a reporting line or an owner takes effect immediately.
- The query language, search and reports (#304, #305, #313) must reuse the
  store's visibility predicate rather than reimplement it.
