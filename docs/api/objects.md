# Objects API

**Status:** experimental authenticated workspace API

The kernel object engine (ADR 0015) lets workspace administrators define
objects and fields at runtime and lets members work with their records. Every
route sits beneath `/api/v1/workspaces/{workspace_id}/objects`, requires an
active membership, and returns `404` when the workspace, object or record must
be concealed.

## Naming

API names are `namespace:name`. Everything a workspace administrator defines
lives in the `custom` namespace (`custom:project`, `custom:status`); plugin
objects and fields use the plugin id (`example.loyalty:tier`). Requests take
the short `name` and the engine qualifies it. Names are a lowercase letter
followed by lowercase letters, digits or single underscores, at most 63
characters. Dots are reserved for relationship traversal in the query
language.

Every object has a three-character key prefix that starts every record id.
Workspace objects always get a digit-first prefix (`000`, `001`, ...); plugin
prefixes start with a letter, so the two never collide. Each definition also
carries an explicit `origin` (`system`, `plugin` or `workspace`).

In the query language and in formulas a dotted plugin namespace directly
followed by `:` is one name, so `example.tracker:milestone.name` traverses
the plugin lookup `example.tracker:milestone`.

## Authorisation

| Operations | Required access |
| --- | --- |
| Create, change and delete objects and fields; read and set object permissions; change the record draft expiry | `objects.manage` |
| Read the record draft settings | any member |
| See an object and read its records | object `read` |
| Create records | object `create` |
| Change, archive and unarchive records | object `edit`, and edit access to the record |
| Delete and undelete records | object `delete` |
| Read or change every record whatever sharing says | object `view_all` / `modify_all`, or the system permissions `records.view_all` / `records.modify_all` |

Object permissions are granted per role. A stronger grant implies the weaker
ones: `modify_all` implies `view_all` and `delete`, `delete` implies `edit`,
and every grant implies `read`. An object the caller cannot read is reported as `404`, exactly
like a missing one; an operation the caller may not perform on an object they
can read returns `403 auth.permission_denied`.

## Objects and fields

| Method and path | Purpose |
| --- | --- |
| `GET /objects` | Objects the caller can read, with their effective `access`, plus `can_manage` |
| `POST /objects` | Create an object: `name`, `label`, `plural_label`, optional `description`, `name_label`, `sharing_model` (`private`, `read`, `read_write`), `grant_hierarchy`, `searchable`, `in_navigation` |
| `GET /objects/{api_name}` | Describe an object with its active fields in order |
| `PATCH /objects/{api_name}` | Change labels and settings; send `expected_version` |
| `DELETE /objects/{api_name}?expected_version=` | Delete an object; refused with `409 objects.object_referenced` while lookups, or related-to fields that allow nothing else, point to it, and `409 objects.object_in_use` while an automation creates its records |
| `POST /objects/{api_name}/fields` | Create a field |
| `PATCH /objects/{api_name}/fields/{field}` | Change a field; send `expected_version` |
| `DELETE /objects/{api_name}/fields/{field}?expected_version=` | Retire a field; its values disappear from records |
| `GET /objects/{api_name}/permissions` | Every workspace role with its access to the object |
| `PUT /objects/{api_name}/permissions/{role_id}` | Set a role's access: `read`, `create`, `edit`, `delete`, `view_all`, `modify_all` |
| `GET /field-permissions` | The permissions a field may name, with their descriptions, for the Object Manager's pickers; needs `objects.manage` |

Field types are `text`, `long_text`, `email`, `phone`, `url`, `number`,
`currency`, `percent`, `boolean`, `date`, `date_time`, `picklist`,
`multi_picklist`, `lookup`, `formula`, `master_detail`, `rollup` and
`related_to`. A field may be `required`, `unique` (text,
email, phone, URL and number types; case-insensitive), `indexed` (for filters
and sorting) and `searchable`. Picklists take `options` with `value`, `label`,
`active` and `default`. Lookups take `reference_object` and a
`delete_behaviour` of `clear` (the default) or `restrict`. A field's type,
uniqueness and lookup target cannot change once it exists.

A `master_detail` field (ADR 0025) is a lookup to the record's master. It
takes `reference_object`, is always required, and has no delete behaviour,
default or uniqueness. Only an object without records can get one, an object
has at most two (two make a junction object), and masters cannot themselves
be details: creating one otherwise fails with `objects.master_detail_invalid`,
`409 objects.master_detail_limit_reached` or
`409 objects.master_detail_has_records`. Turning `required` off fails with
`objects.master_detail_settings_invalid`.

A `rollup` field (ADR 0026) summarises the master's detail records. It takes
`rollup` with `function` (`count`, `sum`, `min` or `max`), `object` (the
detail object), `relationship` (its master-detail field pointing here) and,
for anything but a count, `field` (the detail field to summarise: a number,
currency or percent field, or for a minimum or maximum also a date or
date-time). Creating one fails with:
- `objects.rollup_object_unknown`;
- `objects.rollup_relationship_invalid`;
- `objects.rollup_field_required`, `objects.rollup_field_not_allowed` or
  `objects.rollup_field_invalid`;
- `409 objects.rollup_limit_reached` beyond 25 roll-ups on an object.

The result type is in `formula_type`. The summarised field is indexed if it
was not, and turning its index off fails with `objects.rollup_needs_index`.
Deleting the summarised field, the master-detail field or the detail object
fails with `409 objects.field_in_use` or `objects.object_in_use`.

### Field permissions

A field may name a `read_permission` and a `write_permission` (G5 of
ADR 0035): permission ids from the workspace's catalog, such as
`contacts.people.custom-fields.read`. Administrators with `objects.manage`
set them on workspace fields with `POST .../fields` and
`PATCH .../fields/{field}` (`null` removes one); a plugin declares them on
its own fields in its manifest, and administrators cannot change those
(`409 objects.plugin_owned`). A malformed id fails with
`objects.field_permission_invalid`, and one the catalog does not hold with
`objects.field_permission_unknown`. Formula and roll-up summary fields take
a read permission only. The settings are metadata: changing them moves the
workspace metadata version, and device replicas take them with the rest of
the definition.

A member without a field's read permission never sees the field:

- `GET /objects/{api_name}` leaves it out; administrators with
  `objects.manage` see every field, each with the caller's `access`
  (`{"read": …, "edit": …}`);
- records, lists, composite results and device sync never carry its value;
- the query language, list filters and reports treat it as a field the
  object does not have (`query.field_unknown`, `records.field_unknown`):
  it cannot be selected, filtered, sorted or grouped on, also across a
  relationship, so a shared report that names one fails for that member;
- search never matches a field with a read permission, for anyone, and
  results show only fields the caller reads;
- history entries for it are left out, and change log entries do not name
  it (an entry whose changes were all to hidden fields stays, naming none,
  because the record's version moved);
- related lists through it and timeline sources that need it are left out;
- quick actions that preset it, or a create action whose reference field
  it is, are left out of `GET .../quick-actions` (and so of device
  replicas) and of the timeline's actions, as are those that preset a
  field the member reads but may not change: a member is offered only
  actions they can complete. Administrators with `objects.manage` list
  every action to maintain it; the save stays refused for anyone;
- a formula field that reads it, on the record, through a lookup or through
  another formula field, and a roll-up summary of it, are hidden too.
  Formulas and roll-ups are computed with full access and hidden whenever
  they depend on a field the member cannot read, so they never leak one.

Writing needs reading: a write that names a field the caller cannot read
fails with `records.field_unknown`, before its value is looked at. A
member who reads a field but lacks its write permission gets
`403 records.field_not_writable`, with an `errors` entry for each such
field, when a create gives it a value or an update changes it; an update
that sends its current value, and a create that leaves it to its default,
succeed.

The check is one engine function (`FieldSecurity::field`), applied to
every request as the member, on the server and in a device's replica.
System context bypasses it, as it bypasses sharing: the kernel, a plugin's
installation, the migrator (G4, when it lands), automations, validation rules, lookup filters
and roll-up summaries kept current read and write every field. Member permissions
never bypass it: `records.view_all` and `records.modify_all` widen record
access, not field access. Role-based field-level security (#316) is a
follow-up that extends the same function.

### Plugin objects

Plugins declare objects and fields in their manifest (`[[objects]]`, see
[Declaring objects](../plugins/objects.md)); enabling the plugin in a
workspace creates or migrates them, and only installation creates names in
a plugin's namespace: `POST /objects` and `POST .../fields` always create
`custom` names. Describing a plugin object returns `"origin": "plugin"` and
`"owner_plugin": "<plugin id>"`, and each of its fields its own `origin`:
`plugin` for the plugin's, `workspace` for the `custom` fields
administrators add.

- `PATCH .../fields/{field}` and `DELETE .../fields/{field}` on a plugin's
  field, and `DELETE /objects/{api_name}` on a plugin object, answer
  `409 objects.plugin_owned`, with one exception: a `PATCH` of a plugin's
  related-to field that sends only `expected_version` and
  `reference_objects` changes its allowlist (see Related-to fields).
- `PATCH /objects/{api_name}` on a plugin object may change only
  `sharing_model` and `grant_hierarchy`; any other setting answers
  `409 objects.plugin_owned`.
- Custom fields, validation rules, automations, quick actions and object
  permissions on plugin objects work as on any object. Role permissions
  start empty, as for a new workspace object.

While the plugin is disabled or removed in the workspace, its objects are
suspended: every route answers `404` for them as for a missing object,
`GET /objects`, search, queries, related lists, navigation and device sync
leave them out, and nothing about them changes. Lookup and related-to
values on other objects that point at their records are kept and saved
again unchanged, but are not named in `related`, and new values to them
are refused like values to a missing record. Enabling the plugin again
brings them back as they were.

### Related-to fields

A `related_to` field (ADR 0033 §1) points to a record of any object in an
allowlist set on the field, such as an activity about an account, a deal or
a case. It takes `reference_objects`, 1 to 10 distinct API names of active
objects, kept in the order given; `reference_object` stays `null`. Its
delete behaviour is always `clear`; it has no default, is never unique and
takes no lookup filter yet.

```json
{"name": "regarding", "label": "Regarding", "field_type": "related_to",
 "reference_objects": ["custom:account", "custom:deal"]}
```

Describing the object returns the field with
`"reference_objects": ["custom:account", "custom:deal"]`,
`"reference_object": null` and `"delete_behaviour": "clear"`; other field
types return `"reference_objects": null`. Creating one fails with:
- `objects.reference_objects_invalid` when the list is missing, empty, longer
  than 10 or names an object twice, or when another type sends one;
- `objects.reference_object_unknown` when it names an unknown or deleted
  object;
- `objects.reference_object_invalid` when it also sends `reference_object`;
- `objects.delete_behaviour_not_allowed` for `restrict`;
- `objects.lookup_filter_not_supported` for a `lookup_filter`, also on
  `PATCH`;
- `objects.unique_not_allowed` and `objects.default_not_allowed`.

A plugin's related-to field (`"origin": "plugin"`) may have an empty
allowlist: plugins may ship one empty or partial, and its list takes 0 to
10 objects instead of 1 to 10. While it is empty, every value is refused
with `records.reference_object_not_allowed`, and so is every `object` on
the candidates route. Upgrading the plugin never changes it.

`PATCH .../fields/{field}` takes a new `reference_objects`, which replaces
the allowlist and is checked the same way; on a plugin's field it may be
the request's only change. An object entering the allowlist of a plugin's
field gains the create quick actions the plugin declares for it
([related actions](../plugins/objects.md#related-actions)); one that cannot
be added, because the object already has an action of that name or 10
actions, is skipped and audited as `objects.quick_action.skipped`, and the
`PATCH` still succeeds. Taking an object out clears
nothing: values that point to its records stay and are still named in
`related`, but new values to it are refused, its records lose the field's
related list, and create quick actions on it that fill the field are
deleted. Deleting an allowed object takes it out of every allowlist, as a
field change with its own event, and clears the values that point to its
records outside the recycle bin, each as a record update. An object that
is the only one a related-to field of another object allows cannot be
deleted (`409 objects.object_referenced`), as for a lookup's target.

Formulas, validation rules and automation formulas cannot use a related-to
field (`objects.formula_invalid`, `rules.condition_invalid`), and it cannot
be a roll-up's relationship (`objects.rollup_relationship_invalid`).

## Records

| Method and path | Purpose |
| --- | --- |
| `GET /objects/{api_name}/records` | List records |
| `POST /objects/{api_name}/records` | Create a record from `name` and field values |
| `GET /objects/{api_name}/records/{id}` | Read a record; `include_deleted=true` reads it from the recycle bin |
| `PATCH /objects/{api_name}/records/{id}` | Change only the values sent; `null` clears; send `expected_version` |
| `DELETE /objects/{api_name}/records/{id}?expected_version=` | Move a record to the recycle bin |
| `POST /objects/{api_name}/records/{id}/undelete?expected_version=` | Restore a record from the recycle bin |
| `POST /objects/{api_name}/records/{id}/archive?expected_version=` | Archive a record |
| `POST /objects/{api_name}/records/{id}/unarchive?expected_version=` | Bring an archived record back |

A record carries the standard fields of ADR 0014 (`id`, `object`, `name`,
`owner`, `created_at`, `created_by`, `updated_at`, `updated_by`,
`touched_at`, `version`, `archived_at`, `is_deleted`, `deleted_at`) followed by
its values keyed by field API name, then `related`, which maps the id of
each lookup, master-detail or related-to target the caller can read to that
record's name, and `related_objects`, which maps the same ids to the
target's object API name, so a client can link to a related-to target.
Targets the caller cannot read are left out of both, so a lookup shows only
its id.

```json
{"id": "0a2000000000001AB", "object": "custom:task", "name": "Call Acme",
 "custom:regarding": "0a0000000000003CD",
 "related": {"0a0000000000003CD": "Acme"},
 "related_objects": {"0a0000000000003CD": "custom:account"}}
```
`owner`, `created_by` and `updated_by` are references to the member's user
record with their display name.

A create may send `id` to keep an id a device assigned offline (ADR 0031):
it must carry the object's key prefix and come from the device range, or
the create is `400 records.id_invalid`. Creating again with the same id
returns the record the caller already created; anyone else's id is
`409 records.id_taken`. Composite creates and bulk inserts take `id` the
same way, so offline records that refer to each other push with their
references intact.

Lists accept `status`: `active` (the default: live records that are not
archived), `archived` (live, archived records), `all` (every live record),
`deleted` (the recycle bin, archived or not), or `any` for sync clients, where
deleted records are tombstones and archived ones carry `archived_at`; `sort` (`name`, `created_at`, `updated_at`,
`touched_at`), `touched_after` (an RFC 3339 time; only records touched
strictly after it), `direction` (`asc`, `desc`), `limit` (1 to 200), `cursor`,
`search` (text the record name contains, at most 100 characters), `owner=me` and up to ten
`filter[{field_api_name}]={value}` equality filters on indexed fields. Pages
use keyset cursors that are only valid for the sort and direction that issued
them.

Records of an object with a master-detail field follow their master, through
the first master-detail field created: members see a detail when they can
see its master, and create, edit and delete details when they can edit the
master. The object's own default access, and manual shares, do not apply to
details, and the object is only accessible to roles that can read every
master object. A new detail needs a master the member can edit
(`records.master_read_only`), and a detail cannot move to another master
(`records.master_locked`). Deleting a master moves its details to the
recycle bin with it, and restoring the master restores them; a detail whose
master is in the recycle bin cannot be restored on its own
(`409 records.master_deleted`). A record created while a master-detail field is
being added to its object fails with `409 records.definition_changed`;
retrying it applies the new field.

Roll-up values are stored on the master and are read-only
(`records.field_read_only`). Saving, deleting or restoring a detail updates
its masters' roll-ups in the same request, as a new version of each master
whose summary changed.

Lookup values are record ids. Setting a lookup needs read access to the target
object; an unreadable target is reported like a missing one
(`records.reference_not_found`). Deleting a target clears referencing lookups
or, for `restrict` lookups, fails with `409 records.delete_restricted`;
undeleting it puts the cleared values back where they were not changed since
(see [Recycle bin](#recycle-bin)). A
record in the recycle bin releases its unique values; undeleting it fails with
`409 records.duplicate_value` if another record took one meanwhile.

Related-to values are record ids too, checked like lookups: the target must
exist and be visible to the writer (`records.reference_not_found`), and be a
record of an object the field allows now
(`records.reference_object_not_allowed`, only for a target the writer can
see). A value a change leaves as it was is kept, even if its object was
taken out of the allowlist. Deleting the target always clears the value.
The query language filters a related-to field by `=`, `!=`, `IN` and
`NOT IN` on record ids, and `= null` or `!= null`; following it, as in
`custom:regarding.name`, fails with `query.relationship_unsupported`.

### Related lists and lookup filters

Related lists and lookup filters are described in ADR 0028.

| Method and path | Purpose |
| --- | --- |
| `GET /objects/{api_name}/records/{id}/related` | The first 5 records of each related list |
| `GET /objects/{api_name}/records/{id}/related/{object}/{field}?cursor=` | The next 50 records of one related list |
| `GET /objects/{api_name}/fields/{field}/candidates` | The records a lookup may take, with optional `search` and `cursor`; a related-to field needs `object`, one its allowlist names |
| `GET /objects/{api_name}/records/{id}/timeline?cursor=` | The record's Activity timeline (ADR 0033 §4): open records and a page of past ones |

A record has one related list for each lookup or master-detail field that
points at its object, and each related-to field whose allowlist names it,
from an object the caller can read, up to 20 lists. Each list has `object`,
`object_label`, `object_plural_label`, `field`, `field_label`,
`master_detail`, `field_type` (`lookup`, `master_detail` or `related_to`),
`data` (`id` and `name` of records, by name)
and `next_cursor`. The lists are read as the caller, like a query, so each
member sees only the related records they may see. An unknown relationship
is `404`.

A lookup or master-detail field may have a `lookup_filter`. This is one
condition on the target object, as written after `WHERE` in a query, such
as `custom:active = true`, at most 1,000 characters. Setting it checks that
it runs on the target for the caller, with room under the query's
condition limit for the two conditions the engine adds (the record and the
picker's search). Its problems are reported under `lookup_filter`. Ordering, grouping or limits, or a filter on another field
type, fail with `objects.lookup_filter_invalid`. Send `"lookup_filter": null`
on `PATCH .../fields/{field}` to remove it.

When a member sets a filtered lookup to a new value, the target must meet
the filter (`records.lookup_filter_mismatch`). Values a change leaves as they
were are kept. Automation writes are not bound by the filter. A filter that
no longer runs, for example after a field it names was deleted, refuses new
values with `records.lookup_filter_invalid` until it is fixed. Candidates are
the target records the caller sees that meet the filter, by name, 50 to a
page. `search` matches text the name contains, at most 80 characters. A
related-to field's candidates are those of the object named by `object`;
leaving it out, or naming an object the field does not allow, fails with
`records.reference_object_not_allowed` on `object`.

### Activity timeline

`GET /objects/{api_name}/records/{id}/timeline` lists the records of plugin
objects that declare a timeline (`[objects.timeline]`, see
[Declaring objects](../plugins/objects.md)) and point at the record through
one of their declared fields, merged across sources. It runs as the caller,
like related lists: a record the caller cannot read is `404`, and only
records the caller may see are listed. Sources whose plugin is disabled, or
whose object the caller cannot read, contribute nothing.

```json
{"sources": 2,
 "open": [{"object": "platform.activities:task", "object_label": "Task",
   "id": "tsk00000000001AB", "name": "Send the proposal",
   "date": "2026-11-12", "date_field": "platform.activities:due_date",
   "date_type": "date", "created_at": "2026-11-10T09:12:00.000000Z",
   "open": true, "through": "platform.activities:related_to",
   "details": [{"field": "platform.activities:status", "label": "Status", "value": "Open"},
               {"field": "platform.activities:priority", "label": "Priority", "value": "Normal"},
               {"field": "platform.activities:type", "label": "Type", "value": "To do"}]}],
 "open_more": false,
 "past": [{"object": "platform.activities:event", "object_label": "Event",
   "id": "evt00000000002CD", "name": "Kick-off",
   "date": "2026-11-09T10:00:00.000000Z", "date_field": "platform.activities:starts_at",
   "date_type": "date_time", "created_at": "2026-11-01T08:00:00.000000Z",
   "open": false, "through": "platform.activities:related_to", "details": []}],
 "next_cursor": null,
 "actions": ["log_a_call"]}
```

- `sources` counts the sources that can point at the record and that the
  caller can read. With none the response is empty, not an error, and the
  web record page shows no Activity section.
- `open` holds open records (a declared picklist value, or a date today or
  later for `"upcoming"`), soonest date first, then those without a date,
  oldest first: at most 50, with `open_more` when there are more. It comes
  only with the first page.
- `past` holds every other record, newest first by its date, or by
  `created_at` when it has none; a date counts from midnight UTC. Pages
  hold 20; pass `next_cursor` back as `cursor`. A cursor another timeline
  issued, or an altered one, is `400 timeline.cursor_invalid` on `cursor`.
- Each item carries its object's API name and label, the record's `id` and
  `name`, its `date` with `date_field` and `date_type` (`date` or
  `date_time`), `created_at`, `open`, the `through` field that points at
  the record, and `details`: up to three of the object's picklist values,
  as their labels, in field order. Everything else is read from the record.
- `actions` names the record's create quick actions whose target object is
  a timeline source the caller can read, in quick-action order: the web
  page offers these in the Activity section. Whether the member may run
  one is still decided by the target object's `create` access.

### Archiving

Archiving (ADR 0035 G1) takes a live record out of everyday use without
deleting it, as Salesforce's archived records are kept: it stays readable by
id, keeps its values, lookups, owner and shares, and can still be changed,
deleted and restored. Archive and unarchive are edits: they need the
object's `edit` permission and edit access to the record through ownership,
the role hierarchy, a manual edit share or `modify_all` (ADR 0016), exactly
as an update does, and they take `expected_version`. Each one moves the
record's `version`, `updated_at`, `updated_by` and `touched_at`, sets or
clears `archived_at`, and answers with the record. Neither runs validation
rules or automations, since no value changes.

Archiving is not idempotent. The refusals are, in this order after access:

| Status and code | When |
| --- | --- |
| `404 resource.unavailable` | The record does not exist or the caller cannot see it |
| `403 auth.permission_denied` | The caller can see it but has no edit access |
| `409 records.deleted` | The record is in the recycle bin; restore it first |
| `409 records.already_archived` | Archiving a record that is archived |
| `409 records.not_archived` | Unarchiving a record that is not archived |
| `409 records.version_conflict` | `expected_version` is not the current version |

The record's state is checked before its version, so a client that missed
someone else's archive learns that rather than seeing a version conflict.

Where an archived record shows:

| Surface | Archived records |
| --- | --- |
| `GET .../records/{id}` and record history | shown, with `archived_at` |
| `GET .../records` | left out unless `status=archived` or `status=all` |
| Record queries and saved reports | left out unless the query sends `include_archived=true` (or `include_deleted=true`) |
| Global search | left out |
| Lookup and related-to pickers (`.../candidates`) | left out |
| Related lists and the Activity timeline | left out |
| Existing lookups to an archived record | kept, with its name in `related`; paths through the lookup still read it |
| Setting a lookup to an archived record by id | allowed, subject to the field's lookup filter |
| Sync (`status=any`, revocations) | kept, with `archived_at`, so devices hold it |

A record in the recycle bin keeps its `archived_at`: `status=deleted` lists
it, and undeleting it brings it back archived. On a synced device
archiving and unarchiving are made on the server: the command bridge
answers `403 sync.server_only`, and the replica takes the server's
`archived_at` with each record it pulls.

### Recycle bin

A deleted record stays in the recycle bin for 15 days (ADR 0014 §5, ADR
0035 G2), archived or not, and can be undeleted until then. The window is
an engine constant with no workspace setting. After it, the worker's purge
removes the record for good, workspace by workspace, as the platform rather
than as a member: the record, its index and search terms, its shares, its
history, its change-log entries and the cleared lookups kept for it (below).
It runs hourly, in bounded batches of 100 records per transaction and about
30 seconds per workspace per pass, so a large bin empties over several
passes; a pass with nothing due changes nothing. A record undeleted before
the purge reaches it is never purged: each record is claimed with a write
that checks it is still in the bin.

What the purge leaves and adds:

- Audit entries and published events of the record's life stay, as all
  evidence does. Each purge transaction writes one audit entry for the
  workspace, `records.recycle_bin.purged`, with no actor, the `system`
  source, the records it removed by object, the cut-off and
  `retention_days`, and one `platform.objects.record-purged.v1` event per
  record (see [Events](events.md)).
- A record elsewhere still pointing at a purged record (one undeleted while
  its target stayed in the bin) loses the value, as an update with the
  `system` source, no `updated_by`, a history entry and an `updated` change.
  Details of a purged master are purged with it, since they could never be
  restored.
- `GET /objects/changes` pages carry `purges`, the number of purge
  transactions that removed records in the workspace. A purged record leaves
  no change behind, so a synced device that sees `purges` move asks for its
  revocation set, which now includes the records in its recycle bin (see
  [Device sync](#device-sync)).

Deleting a record clears the lookups and related-to values that point to it,
each as an update of the referencing record, as before. Undeleting it puts
each cleared value back (ADR 0035 G3, as Salesforce does) where the
referencing record is out of the recycle bin and has not written that field
since: setting the field to anything, clearing it again included, counts as
a change. Each restore is an update of its own with the next version, a
history entry with the `system` source, an `updated` change naming the
field and a `record-updated` event; validation rules, lookup filters, field
permissions and automations do not apply, as they do not to the clear. A
cleared value is skipped when the referencing record changed the field
(`changed`), is in the recycle bin (`deleted`), or the field or its object
is no longer in service (`field_removed`); skipped values are named, with
their reason, in a `records.lookups.restore_skipped` audit entry. Details
undeleted with their master get their cleared values back the same way:
the master and every detail coming back with it are restored first, and the
cleared values after, so links between them (a detail to another detail or
to its master, the master to a detail) survive.
What was kept for a record is forgotten once it is undeleted, whether each
value came back or not, or when it is purged.

`POST .../records/{id}/undelete` answers with the record and
`lookups_restored`: `{"restored": 2, "skipped": 1, "skipped_by_reason":
{"changed": 1}}`. These are counts only, since the member may not be able
to read the records involved. A synced device's own undelete answers
without it; the server restores the values when the undelete is pushed and
the device takes them with the next pull.

## Change data capture

`GET /objects/changes` returns the workspace's change log after `after`
(ADR 0018): `data` holds `position`, `object`, `record`, `kind` (`created`,
`updated`, `deleted`, `undeleted`), the changed `fields` by name (an
archive or unarchive is an `updated` entry naming `archived_at`, as an
owner change names `owner`),
`changed_at`, `changed_by` and `correlation_id`; `next_after` is where to
resume and `head` the newest position. Positions are gap-free and in commit
order. Only changes to records the caller can currently see are returned,
but `next_after` still moves past the others. `object` limits the page to
one object, and `limit` takes 1 to 200. Entries never carry values; read
them through the record API.

`GET /objects/changes/stream` sends the same entries as server-sent events
(`event: change`, `id:` the position), resuming after `after` or the
`Last-Event-ID` header. Streams end after 15 minutes; reconnect with
`Last-Event-ID`. Changes are kept for 72 hours: a position older than that
returns `410 changes.position_expired` (or an `expired` event on a stream),
and the client resynchronises with `status=any&sort=touched_at&touched_after=`
lists.

Each page also carries `metadata_version`, the workspace's definitions
version (ADR 0024). It increases with every change to an object, field,
object permission, validation rule, automation or quick action, and never with record
changes, which leave no entry for definitions. It is read after the page's
entries, so it is never older than the definitions they were made under. A
stream sends `event: metadata` with `{"metadata_version": n}` before the
first entries and again whenever the version moves. These events have no
`id:`, so `Last-Event-ID` stays a log position. A synced client fetches
definitions again when the version moves, before it applies the entries
that follow.

## Device sync

The change log omits records a member can no longer read, so it cannot
tell a device to drop them. `POST /sync/revocations` takes the records a
device holds, `{"records": {"custom:deal": ["a0B…", …]}}`, and returns
those the caller can no longer read: `{"revoked": {"custom:deal": […]}}`.
A record is revoked when its object is gone or no longer readable, or when
the record is missing (never made, or purged from the recycle bin) or no
longer shared with the caller (ADR 0024 §4). A record in the server's
recycle bin is not revoked: the device takes its deletion from the change
log, and keeps it in its own bin until the purge removes it. A device names
the records in its recycle bin too, and asks again whenever a change page's
`purges` moves (ADR 0035 G2). Records are read as the caller through the query language, so
sharing and object permissions apply as they do to a query. Objects with
nothing revoked are left out. A request names at most 1,000 record ids
(`400 sync.manifest_too_large`), so a device with more sends its replica in
several requests.

A synced device takes its definitions from the routes above: `GET
/objects/assistant-policy`, `GET /objects`, then `GET /objects/{api_name}`,
its validation rules and its quick actions for each object, whenever a change page's `metadata_version` is newer than the one it
applied. It reads each changed record with `include_deleted=true`, and pushes
its queued changes as composite requests (ADR 0024). A related-to field
keeps, on the device, only the objects of its allowlist the device holds.
A field the device does not have yet is left out when it holds none of
them; one it already has keeps an empty allowlist instead, so nothing can
be chosen for it offline until an allowed object returns. Values to other
objects are kept as the server sent them and new ones are refused
offline, so the server decides them on push. Device clients ignore `related_objects` as
they do `related`.

## Query

`GET /query?q=…` runs the record query language (ADR 0019) as the caller:

```text
SELECT name, custom:stage, custom:amount, custom:account.name
FROM custom:deal
WHERE custom:stage IN ('won', 'negotiation') AND custom:close_date = THIS_MONTH
ORDER BY custom:amount DESC
LIMIT 500
```

- Names are API names; standard fields are `id`, `name`, `owner`,
  `created_at`, `created_by`, `updated_at`, `updated_by`, `touched_at`,
  `version`, `is_deleted`, `deleted_at` and `archived_at`. A dot follows a
  lookup, up to three hops.
- Conditions: `=`, `!=`, `<`, `<=`, `>`, `>=`, `IN`, `NOT IN`, `LIKE`,
  `NOT LIKE`, `= null`, `!= null`, and `INCLUDES` / `EXCLUDES` for
  multi-select picklists, with `AND`, `OR`, `NOT` and parentheses. Text
  compares without case. A negated condition includes records without a
  value.
- Literals are typed: `'text'`, numbers, `true`, `false`, `null`, dates
  (`2026-10-11`), date-times (`2026-10-11T09:30:00Z`) and `TODAY`,
  `YESTERDAY`, `TOMORROW`, `THIS_WEEK`, `LAST_WEEK`, `NEXT_WEEK`
  (weeks start on Monday), `THIS_MONTH`, `LAST_MONTH`, `NEXT_MONTH`,
  `THIS_YEAR`, `LAST_N_DAYS:n` and `NEXT_N_DAYS:n`, in UTC. A date means
  the whole day or period.
- `WHERE`, `ORDER BY` and `GROUP BY` take standard fields, lookups and
  indexed fields only (`query.field_not_indexed` otherwise); any field can
  be selected.
- Aggregates: `COUNT()`, `COUNT(field)`, `SUM`, `AVG`, `MIN`, `MAX`, with an
  optional alias and `GROUP BY` on up to three fields. Grouped text values
  come back as their lowercase key.
- Sharing applies to the object and to every relationship hop, so a hidden
  parent never matches a condition and its selected fields are `null`.
  Archived records are left out, as lists leave them out:
  `include_archived=true` adds them, and `include_deleted=true` adds records
  in the recycle bin, archived or not. A cursor works only with the flags it
  was issued with.

The response holds `object`, `columns` (`key` and `type`), `data` (rows keyed
by each selected item or alias; record rows always carry `id`) and
`next_cursor`. Record queries page with `cursor` and `page_size` (1 to 200,
default 200); a cursor only works with the same query. Problems are
`400 validation.failed` with `field: "q"`, a stable code
(`query.syntax`, `query.object_unknown`, `query.field_unknown`,
`query.field_not_indexed`, `query.formula_not_filterable`,
`query.relationship_unknown`,
`query.operator_invalid`, `query.value_invalid`, `query.too_complex`,
`query.too_many_groups`, `query.cursor_invalid`) and a detail naming the
position; a query that runs past its time limit returns
`503 query.timeout`. Limits are listed in ADR 0019.

## Validation rules

Per-object rules (ADR 0021) live at `/objects/{object}/validation-rules`.
`GET` lists them: every rule for administrators with `objects.manage`, and
the active rules for anyone else who can read the object, so a synced device
checks them offline. Administrators create one with `POST`, and change or
remove one with `PATCH` / `DELETE …/validation-rules/{name}` and
`expected_version`.

```json
{"name": "won_needs_close", "condition": "ISPICKVAL(custom:stage, 'won') && ISBLANK(custom:close_date)",
 "message": "Won deals need a close date", "error_field": "custom:close_date", "active": true}
```

- `condition` is a formula that is **true when the record is wrong**. It is
  type-checked when saved; problems are `400` with `rules.condition_invalid`
  and the position. Other codes: `rules.name_invalid`,
  `rules.message_invalid`, `rules.field_unknown`, `rules.limit_reached`
  (100 per object) and `409 rules.duplicate_name`.
- The language has numbers, text, booleans, dates and date-times; fields by
  API name, following lookups with a dot (up to three); operators
  `+ - * / & = != < <= > >= && || !`; and `IF`, `CASE`, `AND`, `OR`, `NOT`,
  `ISBLANK`, `BLANKVALUE`, `ISPICKVAL`, `INCLUDES`, `LEN`, `LOWER`, `UPPER`,
  `TRIM`, `LEFT`, `RIGHT`, `MID`, `CONTAINS`, `BEGINS`, `SUBSTITUTE`,
  `TEXT`, `VALUE`, `ABS`, `FLOOR`, `CEILING`, `ROUND`, `MOD`, `MIN`, `MAX`,
  `TODAY`, `NOW`, `DATE`, `DATEVALUE`, `YEAR`, `MONTH`, `DAY`, `ADDMONTHS`,
  `REGEX`, `ISNUMBER`, `FIND`, `LPAD`, `RPAD`, `SQRT`, `EXP`, `LN`, `LOG`,
  `TRUNC`, `WEEKDAY`, `DATETIMEVALUE`, `ISNEW`, `ISCHANGED` and
  `PRIORVALUE`. Formula fields can be used like other fields; their
  formulas are inlined, with cycles refused.
- Active rules run on every create, update and owner change. A save that
  breaks rules fails with `400 validation.failed`; each broken rule is an
  error with `code: records.validation_rule`, its message, and its
  `error_field` or `record`. A rule that cannot be evaluated (division by
  zero, an invalid date) reports `records.validation_rule_error` and also
  blocks the save.
- Rules read lookup targets whatever the saving member can see.
- Deleting a field an active rule needs fails with
  `409 objects.field_in_use`.

## Quick actions

Per-object shortcuts (ADR 0032) live at `/objects/{object}/quick-actions`.
`GET` lists them by `position`, then `name`, to anyone who can read the
object; to anyone else the object is unknown. Administrators with
`objects.manage` create one with `POST`, and change or remove one with
`PATCH` / `DELETE …/quick-actions/{name}` and `expected_version` (in the
body for `PATCH`, as a query parameter for `DELETE`). An action grants
nothing: running it uses the normal record create and update APIs.

```json
{"name": "add_contact", "label": "Add contact", "kind": "create", "position": 1,
 "target_object": "custom:contact", "target_field": "custom:account",
 "fields": ["name", "custom:email"], "presets": {"custom:role": "buyer"}}
```

- `kind` is `update` (a short form of this object's fields) or `create` (a
  form for a child record). A create action names its `target_object` and
  `target_field`, an active lookup or master-detail field on that object
  pointing to this one, or a related-to field whose allowlist names this
  object, which is set to the current record; an update
  action has neither. `kind`, `target_object` and `target_field` cannot
  change; `PATCH` takes `label`, `position`, `fields` and `presets`.
- `name` is `[a-z][a-z0-9_]{0,62}` and unique per object; `label` is 1 to
  80 characters; `position` is 0 or more and defaults to after the
  existing actions.
- `fields` lists 1 to 20 distinct field API names, the record name as
  `name`, of this object for `update` and of the target object for
  `create`. `presets` maps up to 20 such fields to constant values, checked
  and normalised as record values are; a preset need not be in `fields`.
  Formula and roll-up summary fields, and a create action's `target_field`,
  cannot be chosen or preset.
- Problems are `400` with `quick_actions.name_invalid`,
  `quick_actions.label_invalid`, `quick_actions.position_invalid`,
  `quick_actions.target_required`, `quick_actions.target_not_allowed`,
  `quick_actions.target_object_unknown`,
  `quick_actions.target_field_invalid`, `quick_actions.fields_invalid`,
  `quick_actions.presets_invalid`, `quick_actions.field_duplicate`,
  `quick_actions.field_unknown`, `quick_actions.field_not_allowed` (on
  `fields.{field}` or `presets.{field}`), a record value code such as
  `records.value_type` on `presets.{field}`, `quick_actions.limit_reached`
  (10 per object) and `409 quick_actions.duplicate_name`.
- The web record page shows the actions in this order next to Edit and
  Delete: an update action only on records the member may edit, a create
  action only when the member may create the target object. An update
  action's form shows only its `fields` and saves their changes with all
  of its presets through the record update API; a create action's form
  shows its `fields` with the `target_field` set to the current record,
  read-only, and sends its presets, shown or not, on create. A preset field
  the form also shows starts at the preset, and the member may change it.
  Administrators manage the actions in the Object Manager.
- Deleting a field removes it from the `fields` and `presets` of the
  actions that use it; an action left with no fields is deleted. Deleting
  a create action's `target_field` or target object deletes the action,
  as does taking this object out of a related-to `target_field`'s
  allowlist, and deleting an object deletes its own actions. Each change publishes
  its quick-action event in the same transaction and moves the workspace
  `metadata_version`, including the related actions a plugin adds.
- A synced device takes each object's actions from this `GET` with its
  other definitions (ADR 0032 §5, #236) and keeps only those it can run
  offline: their `fields` and `presets` are fields the device holds on
  this object and, for a create action, its `target_object` is held and in
  service and its `target_field` still points here. Any other action, such
  as one that names a lookup to an object the member cannot read, or makes
  records of a plugin object suspended with its plugin, stays with the
  server and comes back with what it needs; the sync report lists it as
  deferred. A device answers this `GET` from its replica, with the same
  shape, and refuses action changes with `403 sync.server_only`. Running an
  action on the device is the ordinary local create or update as the
  member, which the server checks again when it is pushed.

## Assistant policy

What a device's assistant bridge (ADR 0034, #234) may reach is object
metadata. Every setting is off until an administrator turns it on, and the
policy only narrows: assistants act as the member, after the member's
permissions and sharing, and the policy grants nothing.

- `GET /objects/assistant-policy` returns the workspace switch with the
  policy of every object the caller can read (every object for holders of
  `objects.manage`):

  ```json
  {"enabled": true, "version": 2, "updated_at": "…", "updated_by": {"id": "…", "name": "…"},
   "objects": [{"object": "custom:deal", "exposed": true, "writable": true, "indexable": true,
                "excluded_fields": ["custom:amount"], "offered_actions": ["close"],
                "version": 3, "updated_at": "…", "updated_by": {"id": "…", "name": "…"}}]}
  ```

  A workspace or object that never had a policy reads as everything off
  with `version: 0` and `updated_at: null`.
- `PATCH /objects/assistant-policy` with `{"expected_version": 2,
  "enabled": false}` turns assistant access on or off for the workspace.
- `GET /objects/{object}/assistant-policy` returns one object's policy to
  anyone who can read the object; to anyone else the object is unknown.
  `PATCH` with `expected_version` changes any of `exposed`, `writable`,
  `indexable`, `excluded_fields` and `offered_actions`; what it leaves out
  stays as it is.
- `exposed`: assistants may find and read the object's records, as the
  member. `writable`: they may create and update them, each change confirmed
  by the member; it requires `exposed`. `indexable`: devices may put the
  records into the phone's search index; it requires `exposed`.
  `excluded_fields` lists the fields, the record name as `name`, whose values
  never enter that index; they apply to the index only, never to assistant
  reads, which field-level security decides. `offered_actions` lists the
  quick actions offered to assistants, at most 10: each requires `exposed`,
  and the object it writes (this object for an update action, the target
  object for a create action) must be `writable`.
- Changes need `objects.manage`, otherwise `403 auth.permission_denied`. A
  plugin's installation never sets the policy, so a plugin object follows the
  workspace's like its sharing model and starts off; a suspended plugin
  object is unknown here too. Problems are `400` with
  `assistant_policy.requires_exposed` (on `writable`, `indexable` or
  `offered_actions.{name}`), `assistant_policy.field_unknown` and
  `assistant_policy.field_duplicate` (on `excluded_fields.{field}`),
  `assistant_policy.action_unknown`, `assistant_policy.action_duplicate` and
  `assistant_policy.action_not_writable` (on `offered_actions.{name}`), and
  `assistant_policy.actions_invalid`; a stale `expected_version` is `409
  records.version_conflict`.
- Fields and actions are kept by id: a deleted field or action leaves the
  policy, and one made again under the same name is not excluded or offered.
  Turning `writable` off takes the create actions of other objects that make
  its records out of their objects' `offered_actions`, each as its own change.
- Each change is audited (`objects.assistant_settings.updated`,
  `objects.assistant_policy.updated`) and publishes
  `platform.objects.assistant-policy-updated.v1` with `scope` (`workspace` or
  `object`) and the settings by API names in the same transaction, and moves
  the workspace `metadata_version`.
- A synced device reads `GET /objects/assistant-policy` with its other
  definitions and keeps each object's policy against its own fields and
  quick actions; names it does not hold are left out, and a server that does
  not answer leaves assistant access off on the device. A device answers both
  `GET` routes from its replica and refuses `PATCH` with `403
  sync.server_only`.

### Assistant writes on push

A create or update in a composite request may carry an assistant marker
(ADR 0034 §6, ADR 0017 "Change source" rule 3 as amended), which only a
synced device's push sends for a change its assistant bridge made:

```json
{"method": "update", "reference": "r0", "object": "custom:deal", "id": "a0B…",
 "expected_version": 4, "values": {"custom:stage": "won"},
 "assistant": {"assistant": "siri", "quick_action": {"object": "custom:deal", "name": "close"}}}
```

- `assistant` is `siri` or `gemini`; any other name makes the request
  invalid. `quick_action`, when the assistant ran one, names the action by
  its object and API name.
- The marker only adds checks. The operation first passes every check it
  would pass unmarked; then, in the same transaction, the server checks its
  current policy: assistant access on, the written object exposed and
  writable, and a named quick action offered on an exposed object and
  writing this object this way (an update action updates its own object, a
  create action creates its target object). Otherwise the operation fails
  with `403 assistant.not_allowed`, and the device keeps the change like any
  write the server refuses.
- Only a request a device's token authenticated may carry it; anything else,
  a device's own command bridge included, fails the operation with `400
  assistant.marker_not_accepted`.
- The change's source is `{"kind": "assistant", "detail": "siri"}` in
  history and audit, and the audit entry keeps the verified device id as
  `device_id` beside it. Events carry only the kind.

## Record drafts

Record forms keep the member's unfinished input as a draft (ADR 0036). A
draft lives on the member's device, never on the server: in the browser,
IndexedDB encrypted with a per-browser key the server holds. A draft is not
a record change: writing one creates no record version, history entry,
event, change-log entry, audit entry or sync push, and saving one is the
ordinary create or update with every check. The server keeps only the
workspace's expiry and each browser's key.

### Draft settings

`GET /objects/draft-settings` returns the workspace's draft expiry to any
member; `PUT /objects/draft-settings` with `{"expiry_days": 14,
"expected_version": 0}` changes it and needs `objects.manage`.

```json
{
  "expiry_days": 14,
  "default_expiry_days": 7,
  "min_expiry_days": 1,
  "max_expiry_days": 90,
  "version": 1,
  "updated_at": "2026-11-18T09:30:00Z",
  "updated_by": {"id": "…", "name": "Hannah Okafor"}
}
```

- A draft expires `expiry_days` after its last change. Without a change the
  workspace has the default of 7 days, at `version` 0 with `updated_at` and
  `updated_by` null. Browsers compute expiry from the current setting, so
  shortening it removes older drafts at their next sweep.
- `expected_version` is the version read (0 for the default); a stale one
  returns `409 records.version_conflict`. Values outside 1 to 90 return
  `400 validation.failed` with `drafts.expiry_invalid` on `expiry_days`;
  unknown fields are refused.
- A change writes an audit record (`objects.draft_settings.updated`, target
  `draft_settings`, with the old and new number of days) and
  `platform.objects.draft-settings-updated.v1` in the same transaction, and
  moves the workspace `metadata_version`. It writes no change-log entry.
- The setting is engine metadata in both stores (`record_draft_settings`,
  keyed by `tenant_id`, forced row-level security on PostgreSQL). A synced
  device answers the `GET` from its replica and refuses the `PUT` with
  `403 sync.server_only`.
- The web shell offers the setting as Settings → Record drafts to holders of
  `objects.manage`.

### Where drafts live

The workspace's UI contributions (`GET /ui/contributions`) carry a `drafts`
block naming where the shell keeps drafts, with the expiry and the limits:

```json
"drafts": {"store": "browser", "expiry_days": 14, "max_drafts": 50, "max_bytes": 1048576}
```

The server always says `browser`. A host that sends no block (or `null`)
keeps no drafts, and forms behave as before; the device bridge sends none
until device drafts exist (ADR 0036 slice 3). The block is part of the
contribution `revision`, so a changed expiry reaches open shells on their
next revalidation. Limits: 50 drafts per member per workspace in one browser
(new forms then stop autosaving; nothing is evicted), and 1 MiB per draft
document before encryption.

### The browser draft key

`GET /api/v1/me/draft-key?browser={browser id}` returns the signed-in
user's key for that browser, creating it on first use:

```json
{"browser": "6f1c…", "key": "base64url, 32 bytes"}
```

- Browser sessions only: a request with an `Authorization` header (an API
  client's or a device's token) is refused with `403 drafts.browser_only`
  before the session is looked at; no session is `401 auth.unauthenticated`.
  `browser` must be a UUID (`400 drafts.browser_invalid`).
- The response is `Cache-Control: no-store`. Keys are random, never logged
  and never audited, and never returned to another user.
- Keys are user-scoped (`draft_keys`, next to sessions): one per user and
  browser, at most 20 per user (creating a 21st deletes the least recently
  used), deleted by the worker after 90 days unused, and deleted with the
  user.
- The shell derives a per-workspace AES-256-GCM key with HKDF-SHA-256
  (salt: the workspace id; info: `limiara.record-drafts.v1`), keeps it in
  memory only as a non-extractable key, and encrypts each draft with a fresh
  96-bit nonce and associated data naming the format version, member,
  workspace and draft id. Without a key the form says "Drafts are
  unavailable" and writes nothing.

### Sign-out

`POST /auth/logout?browser={browser id}` revokes the session and, in the
same transaction, deletes that browser's draft key, so drafts left in the
browser can no longer be decrypted. The web shell's Sign out deletes the
member's drafts in the browser first, then posts. Without `browser` (or with
one that is not a UUID) sign-out revokes the session only.

## Formula fields

A field with `"field_type": "formula"` takes a `formula` and a
`formula_type` (`text`, `number`, `currency`, `percent`, `boolean`, `date`
or `date_time`; `scale` sets decimal places for numbers):

```json
{"name": "total", "label": "Total", "field_type": "formula",
 "formula": "custom:amount * (1 + custom:tax_rate / 100)", "formula_type": "currency"}
```

Its value is computed whenever a record is read and appears with the other
field values; lookups follow the reader's sharing, so a hidden target reads
as blank. Problems with the formula are `400 objects.formula_invalid` with
the position; writing a value is `records.field_read_only`. Formula fields
cannot be required, unique, indexed, searchable, tracked or defaulted.
Queries can select every formula field. A formula that reads only its own
record (no lookups, no `TODAY` or `NOW`) has a stored value, and queries
can also filter, sort and aggregate on it. Using any other formula in those
clauses is `400 query.formula_not_filterable`. `PATCH` with `formula` changes
the formula; a change that would break a formula, rule or automation using
this field is `409 objects.field_in_use`.

## Composite requests

`POST /composite` runs up to 25 record operations in one call (ADR 0023):

```json
{"all_or_none": true, "operations": [
  {"method": "create", "reference": "acme", "object": "custom:company", "values": {"name": "Acme"}},
  {"method": "create", "reference": "ada", "object": "custom:person",
   "values": {"name": "Ada", "custom:employer": "@{acme.id}"}},
  {"method": "update", "reference": "renamed", "object": "custom:person", "id": "@{ada.id}",
   "expected_version": 1, "values": {"name": "Ada L."}}]}
```

- `method` is `create`, `update`, `delete`, `undelete`, which restores a
  record from the recycle bin, or `change_owner`, which takes `owner_id`
  and transfers the record as `POST .../owner` does: an owner who is not a
  member of the workspace rejects the request with `400
  records.owner_unknown` on `operations[i].owner_id`. All but `create` take
  `id` and `expected_version`. `reference` is unique and names the
  operation.
- A whole string value of `"@{reference.id}"`, including `id`, means the
  public ID of an earlier operation's record. Using a failed or later
  operation is `composite.reference_unknown`.
- Without `all_or_none`, each operation runs on its own and a failure does
  not stop the rest. With it, they share one transaction, and the first
  failure rolls everything back: earlier operations report `409
  composite.rolled_back` and later ones `424 composite.not_run`.
- The response is `200` with `results`, one per operation in order. Each
  has `reference`, `status` and `body`: the record, or the problem the
  single-record route would return.
- Problems with the request itself are `composite.operations_invalid`
  (1 to 25 operations) and `composite.reference_invalid`.
- A create or update a device pushes for its assistant bridge carries an
  `assistant` marker, checked against the assistant policy
  ([Assistant writes on push](#assistant-writes-on-push)).

## API clients

Integrations call the routes on this page with an OAuth 2.0
client-credentials token instead of a session (ADR 0023 §3).

1. An administrator with `api_clients.manage` creates a client:
   `POST /api-clients` with `{"name": "Nightly sync"}`. The client acts as
   the administrator. To act as another member, send `"run_as"` with that
   member's id; this also needs `members.manage`. The `201` response
   includes `client_id` and `client_secret`. The secret is shown only this
   once.
2. The integration exchanges the credentials for a token:

   ```sh
   curl -u "$CLIENT_ID:$CLIENT_SECRET" -d grant_type=client_credentials \
     https://api.example.com/oauth/token
   ```

   The response is `{"access_token", "token_type": "Bearer",
   "expires_in": 3600, "workspace_id"}`. Failures use OAuth's `error` codes:
   - `401 invalid_client` for unknown or revoked clients, wrong secrets,
     and clients whose member left;
   - `400 unsupported_grant_type` or `400 invalid_request` for other
     problems.
3. It sends `Authorization: Bearer <token>` to the workspace's object
   routes and to `query`, `search`, `composite`, `bulk` and `sync`. It has exactly
   its member's access. Other routes return
   `403 auth.client_route_denied`, and an unknown or expired token returns
   `401`.

`GET /api-clients` lists clients without secrets, and
`POST /api-clients/{id}/revoke` revokes one along with its tokens. A
workspace has at most 20 active clients (`409 api_clients.limit_reached`).

## Devices

Phone and desktop apps sign a member in through the system browser and
call the routes on this page with their own tokens (ADR 0029). The
deployment lists each app's redirect URI in
`platform.devices.redirect_uris`: a private-use scheme such as
`com.example.app:/oauth`, or HTTP to a loopback address.

1. The app creates a PKCE verifier and opens the system browser at
   `GET /auth/device/login?redirect_uri=…&code_challenge=…&code_challenge_method=S256&state=…`.
   An unregistered redirect URI is `400 auth.device_redirect_invalid`, and a
   missing or plain challenge is `400 auth.device_challenge_invalid`. The
   person signs in with the identity provider, as on the web, and the
   browser returns to the redirect URI with `code` and `state`.
2. Within two minutes, the app redeems the code:

   ```sh
   curl -d grant_type=authorization_code -d code="$CODE" -d code_verifier="$VERIFIER" \
     -d redirect_uri=com.example.app:/oauth -d workspace_id="$WORKSPACE" \
     -d device_name="Ana's phone" -d platform=ios \
     https://api.example.com/oauth/token
   ```

   The response is `{"access_token", "token_type": "Bearer",
   "expires_in": 3600, "refresh_token", "workspace_id", "device_id",
   "offline_limit_seconds"}`. A code works once, for the verifier that
   started the sign-in; anything else is `400 invalid_grant`. A member has
   at most 10 active devices per workspace.
3. The access token reaches the same routes as an API client's, with the
   member's access, plus the device's own routes below.
4. Before it expires, the app sends `grant_type=refresh_token` with
   `refresh_token`. Each use returns a new refresh token, and the old one
   stops working. Presenting a used refresh token again revokes the
   device.

When a device is revoked, or its member leaves the workspace, its access
token gets `401 auth.device_wipe_required` and its refresh token
`400 invalid_grant` with `"wipe": true`. The app then deletes the
workspace's data. After `offline_limit_seconds` without reaching the
server, the app locks its data until it signs in again.

- `GET /devices` lists the caller's devices, or everyone's with
  `devices.manage`: `name`, `platform` (`ios`, `android`, `desktop`),
  `member`, `push` (the provider, never the token), `last_seen_at`,
  `revoked_at`, `revoked_by`, `revoked_reason` (`revoked`, `signed_out`,
  `refresh_reused`) and `current`.
- `POST /devices/{id}/revoke` revokes one of the caller's devices, or
  anyone's with `devices.manage`. Others' devices are `404`.
- With the device's own token only: `GET /devices/current` returns the
  device, its `member` (`id`, `display_name`, `email`, `permissions`,
  `roles` by name and `role_ids`) and the `workspace` (`id`, `name`), which
  the app builds its replica's engine from; `DELETE /devices/current` signs
  the device out, `PUT /devices/current/push` with `{"provider": "apns" |
  "fcm", "token": "…"}` sets where its push notifications go, and
  `DELETE /devices/current/push` stops them.

A device with a push token gets a background notification when its
workspace's change log or metadata version moves, at most once every
`platform.devices.push.quiet_seconds` (ADR 0030). It carries only
`workspace_id`: the app wakes and pulls `GET /objects/changes` as usual. A token the
provider reports invalid is forgotten, and the app registers again.

## Bulk jobs

`POST /bulk` queues up to 5,000 rows to insert, update or delete in the
background (ADR 0023 §2), and returns `202` with the job:

```json
{"object": "custom:person", "operation": "update", "rows": [
  {"id": "0a1000000000000001", "values": {"name": "Ada L."}},
  {"id": "0a1000000000000002", "expected_version": 3, "values": {"name": "Grace H."}}]}
```

- `operation` is `insert`, `update` or `delete`. Insert rows are the
  record's values. Update rows take `id`, `values` and an optional
  `expected_version`; delete rows take `id` and an optional
  `expected_version`. Without one, a row uses the record's current version.
- The caller needs create, edit or delete access to the object. Malformed
  rows reject the job with `bulk.row_invalid` against `rows[i]`, and an
  empty or oversized job is `bulk.rows_invalid`.
- The worker applies each row as its own change, with the caller's access
  as it is when the row runs, and the same validation, automations and
  evidence as the single-record routes.
- `GET /bulk/{id}` returns `status` (`queued`, `running`, `completed` or
  `aborted`), `total`, `succeeded` and `failed`. Only the submitter and
  administrators with `objects.manage` see a job.
- `GET /bulk/{id}/results?status=&after=&limit=` lists rows in order, up to
  1,000 at a time, each with `index`, `status` (`pending`, `succeeded` or
  `failed`), `record` (the public ID) and `errors` (the code the
  single-record route would return, and validation issues). Pass
  `next_after` as `after` for the next page.
- `POST /bulk/{id}/abort` stops a queued or running job; applied rows stay
  applied. A finished job is `409 bulk.job_state`.

## Automations

Administrators with `objects.manage` manage record-triggered automations
(ADR 0022) at `/objects/{object}/automations`: `GET` lists them in the order
they run, `POST` creates one, and `PATCH` / `DELETE …/automations/{name}`
change or remove one with `expected_version`.

```json
{"name": "follow_up", "trigger": "create", "timing": "after_save",
 "condition": "custom:amount >= 1000",
 "actions": [{"type": "create_record", "object": "custom:task", "values": [
   {"field": "name", "formula": "'Follow up ' & name"},
   {"field": "custom:deal", "formula": "id"},
   {"field": "custom:due", "formula": "TODAY() + 7"}]}]}
```

- `trigger` is `create`, `update` (including owner changes),
  `create_or_update`, `delete` or `undelete`. `timing` is `before_save` or
  `after_save` and cannot change. `delete` and `undelete` run after saving
  only (`automations.trigger_invalid`).
- A `before_save` automation has one `update_record` action, which sets
  fields on the record being saved before validation rules run. An
  `after_save` automation has 1 to 10 actions: `update_related` with a
  `lookup` on the record, or `create_record` with an `object`.
- Each value is a formula evaluated on the saved record, in the language of
  validation rules, including `ISNEW`, `ISCHANGED` and `PRIORVALUE`. It is
  type-checked against the field it sets. `condition` is an optional
  boolean formula, and empty text clears it.
- Problems name their place, such as `actions[0].values[1].formula`, with
  codes `automations.formula_invalid`, `automations.condition_invalid`,
  `automations.field_invalid`, `automations.lookup_invalid`,
  `automations.object_unknown`, `automations.actions_invalid`,
  `automations.value_required` and `automations.limit_reached`, plus
  `409 automations.duplicate_name`.
- Automations run in system context in the save transaction. When one
  fails, the member's save fails with `400 validation.failed` and an error
  with `field: "record"` and `code: records.automation_failed` that names
  the automation and the reason. Automations stop at 5 levels deep and
  100 saves per change.
- Deleting a field an active automation needs fails with
  `409 objects.field_in_use`.
- Records that after-save actions save carry `automation_id` in their
  events and audit entries.
- `timing: "async"` runs the actions in the background shortly after the
  change. `timing: "scheduled"` runs them at a time set by
  `"schedule": {"offset_minutes": 60, "from": "custom:due"}`, where `from`
  is a date or date-time field or is left out for the time of the change.
  Both can also use `update_record` on the record they ran for. A job is
  queued with the change and run by the worker as the member who made it.
  Failed attempts are retried with backoff, up to 5. Problems include
  `automations.schedule_invalid`.
- Every action can have a `when` formula: it runs only when that holds,
  so an automation can branch. `{"type": "publish_event", "event":
  "order_placed", "values": [{"field": "total", "formula": "custom:total"}]}`
  publishes a `platform.objects.workspace-event.v1` event through the
  outbox, carrying the event name, object, record, automation and fields.
  It is not allowed before saving. Problems include
  `automations.event_invalid`.
- `GET …/automations/{name}/jobs?limit=` lists the automation's jobs,
  newest first: `status` (`pending`, `running`, `succeeded`, `failed` or
  `cancelled`), `due_at`, `attempts` and `last_error`.
  `POST …/jobs/{id}/retry` retries a failed job, and `POST …/jobs/{id}/cancel`
  cancels a pending one. Any other state is `409 automations.job_state`.
- `GET …/automations/{name}/runs?limit=&cursor=` lists the automation's
  runs from the last 30 days, newest first, with `next_cursor`. Each run has
  the record, `event`, `outcome` (`succeeded` or `failed`, with `detail`),
  `saves`, `ran_at` and `ran_by`. A failed run's change was rolled back.

## Search

`GET /search?q=…` searches every searchable object the caller can read
(ADR 0020). Each query term matches the start of a word in a record's name
or in a field marked searchable; every term must match. Results are grouped
by object, best group first:

```json
{"data": [{"object": "custom:account", "label": "Account", "plural_label": "Accounts",
  "records": [{"id": "0a1000000000000001", "name": "Acme Industries", "fields": {"custom:email": "sales@acme.example"}}],
  "has_more": false}]}
```

- Names count more than fields, exact words more than prefixes; ties sort
  by name.
- Sharing applies per object, and records in the recycle bin are never
  returned. `fields` holds up to three searchable field values.
- `limit` sets the records per object (default 5, at most 50). `object`
  searches one object only, with up to 200 records, for a results page.
- Queries need at least two letters or digits (`400 search.too_short`) and
  at most 200 characters (`search.too_long`); other codes are
  `search.limit_invalid` and `search.object_unknown`.
- The index is updated in the same transaction as each record change, so a
  saved record is found at once.

## History

Objects with `track_history` keep each record's lifecycle, name and owner
changes; fields with `track_history` also keep their old and new values
(ADR 0017). `GET /objects/{api_name}/records/{id}/history` returns entries
newest first with `kind` (`created`, `changed`, `deleted`, `undeleted`,
`archived`, `unarchived`),
`field`, `old_value`, `new_value`, `changed_at` and `changed_by`; owner
changes also carry `old_user` and `new_user`. `related` names lookup values
the caller can see, and `cursor` and `limit` (1 to 200) page through older
entries. History follows record visibility, includes records in the recycle
bin and hides retired fields. Set the flags with `PATCH /objects/{api_name}`
and `PATCH /objects/{api_name}/fields/{field}`.

Each entry also has `source`: what the change came through (#233, ADR 0017
"Change source"), as `{"kind": "...", "detail": "..."}`, or `null` for
entries written before sources were recorded. `kind` is one of `web` (a
browser session), `api_client` (an API client's token; `detail` is the
client's id), `device` (a synced device's token or its replica; `detail` is
the device id when known), `automation` (`detail` is its API name),
`quick_action` (its API name), `assistant` (a device assistant such as
`siri`), `import` (a bulk job; `detail` is the job id) or `system`.
`detail` is omitted when there is none. The server sets the source from
what authenticated the request; no header or body field sets it, and it
never changes what the caller may do. Audit entries carry the same
`source` in their metadata; outbox events carry only `source` as the kind,
without the detail.

## Sharing and ownership

Record access follows ADR 0016. Object permissions set the ceiling; within
it a caller sees a record when they hold view all or modify all, own it,
manage its owner through the reporting line (when the object's
`grant_hierarchy` is on), the object's `sharing_model` is `read` or
`read_write`, or the record is shared with them or one of their roles. A
record the caller cannot see is `404` everywhere: reads, edits, lists,
lookups and `related` names.

A single-record read carries `access`: `read`, `edit`, `delete`, `share`
and `transfer` for the caller, so clients offer only the actions that will
succeed.

| Method and path | Purpose |
| --- | --- |
| `GET /objects/{api_name}/records/{id}/sharing` | Owner, the caller's rights and the manual shares with grantee names; needs the right to share |
| `PUT /objects/{api_name}/records/{id}/sharing/{user\|role}/{grantee_id}` | Grant or change `{"access": "read" \| "edit"}` |
| `DELETE /objects/{api_name}/records/{id}/sharing/{user\|role}/{grantee_id}` | Revoke a share |
| `POST /objects/{api_name}/records/{id}/owner` | Transfer with `{"owner_id", "expected_version"}`; removes manual shares |
| `GET /users` | Workspace members with `manager_id`, plus `can_manage` |
| `PUT /users/{id}/manager` | Set or clear `{"manager_id"}`; needs `members.manage`; cycles return `409 members.reporting_cycle` |
| `GET /roles` | Workspace role names, for role shares |

Owners, their managers and modify-all holders share; an `edit` share never
grants delete or the right to share further. Transferring needs edit access plus ownership,
modify all or `records.transfer`. Unknown grantees return `400
records.grantee_unknown`, unknown new owners `400 records.owner_unknown`,
and sharing with the owner `400 records.share_owner`. Shares and transfers
emit `platform.objects.record-sharing-changed.v1` and
`platform.objects.record-owner-changed.v1`.

## Reports

Saved reports (ADR 0027) are record queries with a name and a way to show
their rows. A report runs as whoever opens it, so a shared report shows
each member only the records they can see.

| Method and path | Purpose |
| --- | --- |
| `GET /reports` | The reports the caller sees, and `can_manage` |
| `POST /reports` | Save a report |
| `GET /reports/{id}` | The report and its rows, run as the caller (`cursor`, `page_size`) |
| `PATCH /reports/{id}` | Change only the values sent; send `expected_version` |
| `DELETE /reports/{id}?expected_version=` | Delete a report |

A report has a `name` (1 to 80 characters), an optional `description`, a
`query` in the query language, a `chart` and a `visibility`:
- `chart` is `table` (the default), `bar`, `line`, `pie` or `metric`;
- charts take a `value_column`, a number, currency or percent column of the
  query; bar, line and pie charts also take a `label_column`;
- `visibility` is `private` (its owner, the default) or `workspace` (every
  member).

Saving checks that the query runs for the caller, and reports its problems
under `query` with the query language's codes. A chart column that is not
in the query fails with `reports.value_column_invalid` or
`reports.label_column_invalid`. Sharing a report with the workspace, and
changing or deleting someone else's report, needs `reports.manage`; a
private report of someone else is missing. A workspace keeps at most 1,000
reports (`409 reports.limit_reached`); concurrent saves are serialised, so
they cannot exceed it.

Each report carries `can_edit`: whether the caller may change or delete
it (its owner, or a member with `reports.manage`). The run result is under
`result`, in the shape of `GET /query`; pass its `next_cursor` back as
`cursor` for the next page of rows.

## Navigation

Objects marked `in_navigation` that the caller can read are added to the
workspace's UI contributions (`GET /ui/contributions`) as
`objects.{api_name}` entries routed to `/o/{api_name}`, after feature
navigation. The contribution `revision` changes when object navigation or
the caller's engine permissions change, so the shell refreshes without a
reload. The engine permissions the caller holds (`objects.manage`,
`records.view_all`, `records.modify_all`, `reports.manage`) are listed with
the feature permissions. A `reports` entry routed to `/reports`
follows the objects.

## Errors

Errors use the problem format with a stable `code`. Validation failures
return `400 validation.failed` with an `errors` list of `{field, code,
detail}`. Version mismatches return `409 records.version_conflict`.
Changing a field the caller reads but may not change returns
`403 records.field_not_writable`, with the fields in `errors`.
A pushed assistant write the workspace's assistant policy does not allow
returns `403 assistant.not_allowed`, and an assistant marker on anything but
a device's push `400 assistant.marker_not_accepted` (see
[Assistant policy](#assistant-policy)).
Changing or deleting what a plugin owns returns `409 objects.plugin_owned`;
the codes for refused plugin installations are listed in
[Declaring objects](../plugins/objects.md#enabling-upgrading-and-disabling).

## Events and audit

Every mutation writes an audit record and a `platform.objects.*.v1` event in
the same transaction: object, field and object-permission changes, record
created, updated, deleted, undeleted, archived and unarchived
(`record-archived`, `record-unarchived`, with the record payload), records
purged from the recycle bin (`record-purged`, one per record, with one
`records.recycle_bin.purged` audit entry per purge transaction rather than
one per record), and reports created, updated and
deleted (`report-created`, `report-updated`, `report-deleted`, naming the
report, chart and visibility but not the query), the record draft expiry
(`platform.objects.draft-settings-updated.v1`, naming the old and new number
of days), and quick actions created,
updated and deleted (`platform.objects.quick-action-created.v1`,
`-updated.v1`, `-deleted.v1`, naming the object, the action, its kind and
target, its fields and the fields it presets, never the preset values), and
assistant policy changes (`platform.objects.assistant-policy-updated.v1`). Payloads name the object, record and
changed fields, never field values; lookup and related-to values, which
are record ids, are no exception. Changes a plugin's installation makes
also carry `"plugin": "<plugin id>"`, with the member who changed the
plugin's state as actor; suspending and resuming a plugin's objects are
`object-updated` events with `"suspended"`, audited as
`objects.object.suspended` and `objects.object.resumed`. A related action the
engine skips leaves only an audit record, `objects.quick_action.skipped`,
naming the object, the action, the plugin and the reason; nothing changed,
so no event is published.
