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

## Authorisation

| Operations | Required access |
| --- | --- |
| Create, change and delete objects and fields; read and set object permissions | `objects.manage` |
| See an object and read its records | object `read` |
| Create records | object `create` |
| Change records | object `edit` |
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

`PATCH .../fields/{field}` takes a new `reference_objects`, which replaces
the allowlist and is checked the same way. Taking an object out clears
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

Lists accept `status` (`active`, `deleted`, or `any` for sync clients, where
deleted records are tombstones), `sort` (`name`, `created_at`, `updated_at`,
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
or, for `restrict` lookups, fails with `409 records.delete_restricted`. A
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

## Change data capture

`GET /objects/changes` returns the workspace's change log after `after`
(ADR 0018): `data` holds `position`, `object`, `record`, `kind` (`created`,
`updated`, `deleted`, `undeleted`), the changed `fields` by name,
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
object permission, validation rule or automation, and never with record
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
the record is deleted, missing or no longer shared with the caller (ADR
0024 §4). Records are read as the caller through the query language, so
sharing and object permissions apply as they do to a query. Objects with
nothing revoked are left out. A request names at most 1,000 record ids
(`400 sync.manifest_too_large`), so a device with more sends its replica in
several requests.

A synced device takes its definitions from the routes above: `GET
/objects`, then `GET /objects/{api_name}` and its validation rules for each
object, whenever a change page's `metadata_version` is newer than the one it
applied. It reads each changed record with `include_deleted=true`, and pushes
its queued changes as composite requests (ADR 0024). A related-to field
keeps, on the device, only the objects of its allowlist the device holds,
and is left out when it holds none of them; values to other objects are
kept as the server sent them and new ones are refused offline, so the
server decides them on push. Device clients ignore `related_objects` as
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
  `version`, `is_deleted` and `deleted_at`. A dot follows a lookup, up to
  three hops.
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
  `include_deleted=true` adds records in the recycle bin.

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
  its quick-action event in the same transaction.

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
newest first with `kind` (`created`, `changed`, `deleted`, `undeleted`),
`field`, `old_value`, `new_value`, `changed_at` and `changed_by`; owner
changes also carry `old_user` and `new_user`. `related` names lookup values
the caller can see, and `cursor` and `limit` (1 to 200) page through older
entries. History follows record visibility, includes records in the recycle
bin and hides retired fields. Set the flags with `PATCH /objects/{api_name}`
and `PATCH /objects/{api_name}/fields/{field}`.

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

## Events and audit

Every mutation writes an audit record and a `platform.objects.*.v1` event in
the same transaction: object, field and object-permission changes, record
created, updated, deleted and undeleted, and reports created, updated and
deleted (`report-created`, `report-updated`, `report-deleted`, naming the
report, chart and visibility but not the query), and quick actions created,
updated and deleted (`platform.objects.quick-action-created.v1`,
`-updated.v1`, `-deleted.v1`, naming the object, the action, its kind and
target, its fields and the fields it presets, never the preset values). Payloads name the object, record and
changed fields, never field values; lookup and related-to values, which
are record ids, are no exception.
