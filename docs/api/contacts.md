# Contacts API

**Status:** experimental authenticated workspace API

The Contacts plugin owns durable people, organisations, and tenant-defined
custom fields. Every route is scoped beneath
`/api/v1/workspaces/{workspace_id}/contacts`, requires membership plus the
relevant Contacts permission, and returns `404` when the workspace or record
must be concealed.

## Authorisation

The workspace boundary derives the caller's tenant and permissions from an
active membership. The same membership-derived context is mandatory inside the
Contacts service, so a native caller cannot invoke a service method with only a
tenant identifier and bypass route policy. Permission checks run before input
validation, identifier parsing, or storage access.

| Operations | Required permission |
| --- | --- |
| List and read people, including the experimental OData projection | `contacts.people.read` |
| Create, update, and archive people | `contacts.people.write` |
| List and read organisations | `contacts.organisations.read` |
| Create, update, and archive organisations | `contacts.organisations.write` |
| List, read, create, update, disable, and archive custom-field definitions | `contacts.settings.manage` |

A request without a valid session returns `401 auth.unauthenticated`. A valid
session without membership receives the same `404 workspace.not_found` response
as an unknown workspace. An active member who lacks the operation permission
receives `403 auth.permission_denied`; the response does not disclose the
missing permission or whether a record exists. Forced PostgreSQL row-level
security remains a backstop if an application query omits its tenant predicate.

## People and organisations

People support `GET` and `POST` on `/people`, plus `GET`, `PUT`, and `DELETE`
on `/people/{id}` and `POST` on `/people/{id}/archive` and
`/people/{id}/undelete`. Organisations expose the same operations on
`/organisations`. IDs use caller-safe `per...` and `org...` public identifiers;
database UUIDs and tenant IDs are never serialized.

Updates are full replacements and require `expected_version` in the JSON body.
Archive, delete and undelete take `?expected_version={version}`. A stale
version returns `409` with code `contacts.version_conflict`.

Archiving and deleting are different:

- **Archive** (`POST .../archive`) keeps a record that is no longer active.
  Archived records remain available by ID and through `status=archived` or
  `status=all`, while lists default to active records. Archiving an
  organisation retains the historical link from its people, but new links to
  that organisation are rejected.
- **Delete** (`DELETE .../{id}`) moves a record to the recycle bin, as
  Salesforce does ([ADR 0014](../adr/0014-standard-record-fields.md)). A
  deleted record returns `404` from reads, updates, archive, activity and
  extension writes, and is left out of every list except `status=deleted`.
  `GET .../{id}?include_deleted=true` reads it, and `POST .../undelete`
  restores it. The worker removes records permanently 15 days after they
  were deleted; people linked to a purged organisation lose the link. A
  deleted record keeps its unique custom-field values, so undeleting it can
  never collide. Before #290, `DELETE` archived; archive now has its own
  route.

### Standard record fields

Every person and organisation response carries the kernel's standard record
fields ([ADR 0014](../adr/0014-standard-record-fields.md)):

| Field | Meaning |
| --- | --- |
| `id` | Public identifier (`per...`, `org...`). |
| `name` | Display name: a person's `full_name`, an organisation's `name`. |
| `created_at`, `created_by` | When and by whom the record was created. |
| `updated_at`, `updated_by` | The last change a person made, and who made it. |
| `touched_at` | The last change of any kind, including system changes such as extension retention purges. Never earlier than `updated_at`. |
| `version` | Optimistic concurrency version. |
| `archived_at` | Archive time, or `null`. |
| `is_deleted`, `deleted_at` | Whether, and since when, the record is in the recycle bin. |

`created_by` and `updated_by` are `{ "id": "<user id>", "name": "<display
name>" }`. The name comes from the workspace's membership; a user who is no
longer a member keeps their `id` with `"name": null`. Records created before
these fields existed return `null` for both. Each reference links to the member's user
record at `GET /api/v1/workspaces/{workspace}/users/{id}`, which any member
of the workspace may read; users outside the workspace get `404`. The acting member always comes
from the session; request bodies cannot set any standard field.

List routes accept these shared query parameters:

- `search` performs a case-insensitive name substring match.
- `email` performs a normalized exact email match.
- `organisation_id` filters people by a public organisation ID.
- `status` is `active`, `archived`, `all` (active and archived), or
  `deleted` (the recycle bin).
- `sort` is `name`, `created_at`, or `updated_at`; `direction` is `asc` or
  `desc`.
- `limit` is between 1 and 200; `cursor` continues the exact same query.

Responses use `{ "data": [...], "next_cursor": "..." }`. Cursors are random,
opaque, expire after one hour, and are bound to the workspace, resource, and
normalized query. Reusing one for another tenant or query returns
`contacts.cursor_invalid` without exposing a database position or identifier.

## Activity timeline

`GET /people/{id}/activity` and `GET /organisations/{id}/activity` return the
record's 50 most recent lifecycle entries, newest first, as
`{ "data": [{ "event_id", "action", "entity_version", "actor_id", "occurred_at" }] }`.
`action` is `created`, `updated`, or `archived`. Each route needs the matching
read permission, and an unknown or foreign ID returns the same `404` as
reading the record.

The timeline is a read model that the Contacts worker consumer
`platform.contacts.activity-timeline-v1` builds from the plugin's own events,
so it is eventually consistent: an entry appears after the outbox publishes
the event. It never stores personal attributes. While Contacts is disabled
for a workspace, events are not added to the timeline, and they are not
replayed after re-enable. Cursor pagination beyond the latest 50 entries is
planned with the Customer 360 work (#218).

Expired list cursors are removed per workspace by the hourly
`platform.contacts.list-cursor-cleanup-v1` job.

## Custom fields

Administrators create definitions with `POST /custom-fields`, list them with
`GET /custom-fields/{person|organisation}`, and read or update a definition at
`/custom-fields/definitions/{id}`. The `/disable` and `/archive` actions accept
`{ "expected_version": n }`.

Supported field types are text, textarea, email, phone, number, boolean, date,
datetime, select, multi-select, and JSON. Definitions can declare required and
tenant-unique values, select options, bounded validation rules, data
classification, processing purpose, retention, and an encryption requirement.
Sensitive fields must declare encryption. Disabled or archived definitions and
their stored values remain readable, but clients cannot change those values;
omitting them from a later update preserves the stored value.

## Extension values

Other plugins may add fields to people and organisations
([Extending other plugins](../plugins/extensions.md)). Every person and
organisation response carries an `extensions` object holding only the values
the caller may read from active extensions, keyed by field id such as
`example.loyalty.tier`.

`POST /people` and `PUT /people/{id}` also accept an `extensions` object, so
a form saves host fields and extension values in one transaction and one
record version; an invalid extension value rejects the whole request.

`PATCH /people/{id}/extensions` and `PATCH /organisations/{id}/extensions`
take `{ "expected_version": n, "values": { "<field id>": value } }`; `null`
clears a value. The route needs the record's read permission; the kernel
extension service checks each field's own write permission and validates
every value. Writes bump the record version and emit the record's `updated`
event without the values.

| Response | Meaning |
| --- | --- |
| `400 validation.failed` | `errors[].field` is the field id; codes include `extension.field_unknown`, `extension.value_type`, `extension.value_option`, `extension.value_too_long`, `extension.value_date` and `extension.values_too_large`. |
| `403 auth.permission_denied` | The caller can read but not write one of the fields. |
| `409 contacts.version_conflict` | The record changed, or is archived. |

Lists accept `extension_field` and `extension_value` together to keep records
whose field equals the value. Only fields the caller may read can be used, and
the filter is part of the cursor identity.

## Atomic mutation evidence

Every successful Contacts create, update, disable, or archive operation commits
the domain change, one audit record, and one versioned outbox event in the same
PostgreSQL transaction. A version conflict or failure while appending either
record rolls back the complete operation, so it leaves no audit or outbox row.
Callers never receive the underlying database error.

A valid UUID in `x-request-id` becomes the audit and event `correlation_id`.
The API generates an internal UUID when that header is absent or invalid. The
authenticated membership supplies the tenant and `actor_id`; direct HTTP
mutations have a null `causation_id`. Audit metadata includes the resulting
entity ID and version plus the `event_id` that links it to the outbox envelope.

Contacts event payloads are deliberately bounded. Person and organisation
events contain only `entity_id` and `version`. Custom-field definition events
add `applies_to`. Names, email addresses, telephone numbers, tags, custom-field
values, labels, options, validation rules, purposes, and other private record
state are excluded. See the [events reference](./events.md) for the experimental
event names and envelope contract; durable broker publication remains planned.

Validation failures use `400 validation.failed` with an `errors` array whose
entries contain stable `field`, `code`, and `detail` properties. Storage faults
return a generic `503 contacts.unavailable` response and do not disclose SQL or
tenant details.
