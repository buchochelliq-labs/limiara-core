# 0014: Standard record fields

## Status

Proposed. Issue #285 tracks the work.

## Context

Every record resource today chooses its own bookkeeping. Contacts people and
organisations carry a public identifier, `created_at`, `updated_at`, a
version and `archived_at`, but nothing records who made or last changed a
record, there is no common display name across resources, and a change that
no person made (for example the retention job purging a removed plugin's
extension values, ADR 0012) leaves no trace on the record at all. Each
resource document also redeclares `updated_at` and a version field by hand.

Administrators, integrations and plugins need to rely on the same handful of
fields on every record, whichever plugin owns it, in the way Salesforce gives
every object `Id`, `Name`, `CreatedDate`, `CreatedById`, `LastModifiedDate`,
`LastModifiedById` and `SystemModstamp`.

## Decision

### 1. The kernel defines the standard fields

Every record resource has these fields. The kernel defines them once
(`plugin_contract::ui::standard_fields`); resource documents do not declare
them, and a document that declares one of these names is rejected (the
`name` exception is in section 2).

| Field | Label | Type | Meaning |
| --- | --- | --- | --- |
| `id` | Record ID | text | Stable public identifier, such as `per_…`. Never reused. |
| `name` | Name | text | The record's display name, used in titles, lookups, search results and references. |
| `created_at` | Created | date-time | When the record was created. |
| `created_by` | Created by | user | The workspace member who created it. |
| `updated_at` | Last modified | date-time | The last change a person made, directly or through a plugin acting for them. |
| `updated_by` | Last modified by | user | The member who made that change. |
| `touched_at` | Last touched | date-time | Moves forward on every change to the stored record, including changes no person made: retention purges, migrations, automation. Never earlier than `updated_at`. |
| `version` | Version | number | Optimistic concurrency version; moves with `updated_at`. |
| `archived_at` | Archived | date-time | When the record was archived, or empty. |
| `is_deleted` | Deleted | boolean | Whether the record is in the recycle bin (Salesforce `IsDeleted`). Derived from `deleted_at`. |
| `deleted_at` | Deleted on | date-time | When the record was moved to the recycle bin, or empty. |

The standard fields are read-only to every caller. They carry the field
source `system`, so the web app, layouts and integrations can tell them apart
from the owning plugin's fields, tenant custom fields and extension fields,
while still rendering them as ordinary fields of the record.

`touched_at` is the platform's equivalent of Salesforce's `SystemModstamp`,
under a name that says what it does. Integrations that synchronise records
use it: "everything touched since my last run" includes changes that do not
count as an edit, which `updated_at` deliberately leaves out so people are
not shown system activity as their colleagues' edits.

Record ownership (Salesforce `OwnerId`) is not a standard field yet. Owning a
record implies record-level sharing, which needs its own decision; it is
listed as a follow-up.

### 2. The owning plugin names the name field

A record resource declares which of its own fields is its display name:

```json
{ "id": "platform.contacts.person", "name_field": "full_name", ... }
```

The named field must be a declared, required text field. When it is called
`name` (as on organisations), that declaration is the standard `name` field
and must be required text. Otherwise the kernel serves a read-only `name`
that mirrors it. A resource with a `read` or `list` operation is a record
resource and must declare `name_field`.

### 3. The kernel stamps who; the store stamps when

The acting member comes from the authenticated workspace session, never from
the request body. Stores receive the actor when the unit of work begins:

- a member's change sets `updated_at`, `updated_by`, `touched_at` and the
  next `version`; creation also sets `created_at` and `created_by`;
- a system change (retention purge, migration, background repair) sets only
  `touched_at`.

`created_by` and `updated_by` hold the kernel user identifier. Rows that
predate this decision have no creator, and show as unknown.

### 4. The API resolves people

Record responses carry `created_by` and `updated_by` as
`{ "id": "<user id>", "name": "<display name>" }`. The kernel resolves names
from workspace membership, so a plugin never reads the user directory. A
user who is no longer a member of the workspace is returned with no name and
shown as a former member; the platform never discloses names of users
outside the workspace.

Each reference links to the member's user record, as Salesforce links
`CreatedById` to the User record. The record is kernel-owned and served at
`GET /api/v1/workspaces/{workspace}/users/{user}` (name, email, roles and
when the user was created) to any member of the workspace, whatever features
are enabled; the web app shows it at `/users/{user}`. A user who is not a
member of the workspace, an unknown id and a malformed id all get the same
`404`, so the route cannot be used to discover users elsewhere. In page
metadata, the `user` display format renders a reference as that link, and a
former member as plain text.

### 5. Deleting is not archiving

As in Salesforce, a deleted record goes to a recycle bin (#290). It is hidden
from reads, lists, writes, links and extension writes, as Salesforce hides
`IsDeleted` records from `query`; reading it needs `include_deleted=true`
(Salesforce `queryAll`), and listing the bin needs `status=deleted`. Undelete
restores it unchanged. Both delete and undelete are a member's edit: they
move `version`, `updated_at`, `updated_by` and `touched_at`, and each emits a
versioned `*-deleted.v1` or `*-undeleted.v1` event with its audit entry.

After 15 days in the bin the host's worker removes the record permanently,
in batches per workspace, with one audit entry per run. Removing an
organisation unlinks its people, which is a system change: only their
`touched_at` moves. Archive stays a separate state for records that are kept
but no longer active; an archived record can also be deleted.

### 5. Every backend keeps the same guarantees

The Contacts store conformance suite (ADR 0013) checks the stamps on every
backend: creation sets both creator and modifier, a member's edit moves
`updated_at` and `touched_at` together, a system change moves only
`touched_at`, and archiving counts as a member's edit.

## Alternatives considered

- **Let each plugin declare its own audit fields.** That is today's state;
  it drifts, and integrations cannot rely on any field across resources.
- **Read `created_by` and `updated_by` from the audit log.** The audit log is
  evidence, not a query store; listing and sorting records by creator would
  need a join against it for every page.
- **Reuse `updated_at` for system changes.** Simpler, but a purge or
  migration would then look like a colleague edited thousands of records.

## Consequences

- Every record looks the same to administrators, layouts and integrations,
  whatever plugin owns it.
- Plugins that host records add three columns (`created_by`, `updated_by`,
  `touched_at`) and a tenant-first `touched_at` index, on every backend they
  support.
- Responses gain user references; the API resolves names with one lookup per
  page of records.

## Implementation status

Implemented with this ADR (#285):

- The standard field set, `name_field`, the reserved-name check and the
  `system` field source in the plugin contract; delivery adds the standard
  fields to every record resource.
- Contacts people and organisations on PostgreSQL and SQLite (Contacts
  migration 0008), with system purges moving only `touched_at`.
- API user references, and the web record metadata section showing who
  created and last modified each person. The web organisation pages still
  read fixture data and show only the version and last modified time until
  they are connected to the API.

Implemented with #289: the kernel user record route, the web user record
page, and the `user` display format linking Created by and Last modified by
to it.

Implemented with #290: `is_deleted` and `deleted_at`, the Contacts recycle
bin on PostgreSQL and SQLite, undelete, the 15-day purge job on PostgreSQL,
and Delete and Undelete on the person page.

Follow-ups:

- record ownership and sharing (`owner`), with its own ADR (#286);
- `touched_at` list filters for incremental sync across every record API
  (#287);
- a recycle bin page across features (UX slice 7), configurable retention,
  and the purge job on SQLite once the worker runs there (#282);
- a members directory, and "created by me" or "modified by me" list
  filters;
- last viewed and last referenced timestamps per member (needs the recently
  viewed service in UX slice 7).
