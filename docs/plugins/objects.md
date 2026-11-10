# Declaring objects

**Status:** experimental. Design: [ADR 0033 §2](../adr/0033-activities.md),
naming in [ADR 0015 §3](../adr/0015-metadata-driven-objects.md).

A plugin can ship engine objects: definitions the object engine stores and
serves like the objects a workspace administrator creates, with records,
sharing, permissions, validation rules, automations, quick actions, search,
reports and device sync. When a workspace enables the plugin, the kernel
creates or migrates the objects there. The fixture `fixtures/plugins/tracker`
declares a Milestone and a Ticket that looks one up, and the first-party
plugin `plugins/activities` declares Task and Event with a Log a call
related action ([Activities](../product/activities.md)).

## Declaring objects and fields

Declare each object under `[[objects]]` and its fields under
`[[objects.fields]]`:

```toml
[[objects]]
name = "milestone"
label = "Milestone"
plural_label = "Milestones"
name_label = "Milestone name"
key_prefix = "tkm"

[[objects]]
name = "ticket"
label = "Ticket"
plural_label = "Tickets"
key_prefix = "tkt"
track_history = true

[[objects.fields]]
name = "status"
label = "Status"
field_type = "picklist"
required = true
options = [
  { value = "open", label = "Open", default = true },
  { value = "done", label = "Done" },
]

[[objects.fields]]
name = "milestone"
label = "Milestone"
field_type = "lookup"
reference_object = "example.tracker:milestone"
```

API names are `<plugin id>:<name>`: the tracker's objects are
`example.tracker:milestone` and `example.tracker:ticket`, and their fields
`example.tracker:status` and so on. The plugin id must work as a namespace:
dotted tokens of lowercase letters, digits and underscores, each starting
with a letter (`example.tracker`, not `example.my-tracker`).

| Object setting | Rule |
| --- | --- |
| `name` | A lowercase letter, then lowercase letters, digits or single underscores, up to 63 characters; unique in the manifest. |
| `label`, `plural_label`, `name_label` | 1 to 80 printable characters. `name_label` labels the standard name field and defaults to "Name". |
| `description` | Up to 1000 characters. |
| `key_prefix` | Three characters: a lowercase letter, then lowercase letters or digits. Unique in the manifest and in the workspace; fixed once installed, because every record id starts with it. |
| `sharing_model` | `private`, `read` or `read_write` (the default). Only the starting value: the workspace owns it afterwards. |
| `searchable`, `in_navigation` | Default `true`. |
| `track_history` | Default `false`. |

| Field setting | Rule |
| --- | --- |
| `name`, `label`, `description` | As for objects; names unique per object. |
| `field_type` | `text`, `long_text`, `email`, `phone`, `url`, `number`, `currency`, `percent`, `boolean`, `date`, `date_time`, `picklist`, `multi_picklist`, `lookup` or `related_to`. Formula, master-detail and roll-up summary fields cannot be declared yet. |
| `required`, `indexed`, `track_history` | As for workspace fields. |
| `unique` | Text, email, phone, URL and number fields only. |
| `searchable` | Text-like fields only. |
| `max_length` | Text (1 to 255) and long text (1 to 131072) only. |
| `scale` | Number, currency and percent only; 0 to 8. |
| `options` | Required for picklists and only allowed there: up to 200, each with `value`, `label` and optional `default`. |
| `reference_object` | A lookup's target. |
| `reference_objects` | A related-to field's starting allowlist: 0 to 10 distinct targets. Workspace administrators choose the rest (see below). |
| `delete_behaviour` | A lookup's: `clear` (the default) or `restrict`. |

A target is an API name: one of the plugin's own objects, or an object of a
plugin the manifest declares a dependency on. A plugin cannot point at
`custom` objects, which exist only in some workspaces, and cannot declare
anything in the `custom` namespace. A manifest declares at most 20 objects
and 100 fields per object. The manifest is refused when it is read if any
rule is broken (`ManifestError::InvalidObject` or `InvalidObjectField`),
and unknown keys are refused rather than ignored.

## Related-to fields the workspace points

A plugin rarely knows which objects a workspace has. Its related-to field
may therefore start with an empty or partial allowlist, and the workspace's
administrators change it with the field API's `PATCH ... reference_objects`
(up to 10 objects, any active object, the list may be emptied). This is the
one property of a plugin's field administrators may change; anything else
in the same request is refused with `objects.plugin_owned`. The manifest's
list is only the starting value: an upgrade never changes an installed
field's allowlist, as with `sharing_model`.

While the allowlist is empty, the field holds no new values: writes answer
`records.reference_object_not_allowed`, the candidates route refuses every
object, and device replicas keep the field with nothing allowed. Changing
the allowlist runs the usual cascades: values pointing at an object taken
out are kept, and create quick actions that fill the field from that object
are deleted (ADR 0032 §4).

## Related actions

An object may declare create quick actions ([ADR 0032](../adr/0032-quick-actions.md))
for the objects its related-to fields allow:

```toml
[[objects.related_actions]]
field = "related_to"   # one of this object's related-to fields
name = "log_a_call"
label = "Log a call"
fields = ["name", "platform.activities:description"]
presets = { "platform.activities:type" = "call", "platform.activities:status" = "completed" }
```

Whenever an object enters the field's allowlist, it gains a create quick
action of that name that makes a record of the declaring object, with the
field set to the current record, the form asking for `fields` and the
`presets` applied on save.
An object that is out of service when it enters the allowlist, because its
own plugin is disabled, gains the action when that plugin is enabled again.

| Setting | Rule |
| --- | --- |
| `field` | The short name of one of the object's related-to fields. |
| `name` | As for field names; unique per declaring object. It becomes the quick action's name. |
| `label` | 1 to 80 printable characters. |
| `fields` | 1 to 20 distinct fields the form asks for: `name`, or API names of the object's declared fields. Not the related-to field itself. |
| `presets` | Up to 20 constants by field API name, checked against the field's type: an option of a picklist, a list of options of a multi-select picklist, a number, `true` or `false`, a date as `YYYY-MM-DD`, a date and time with a zone, or text within the field's length. Lookups and related-to fields cannot be preset. |

An object declares at most 10 related actions. Unknown keys are refused, and
a broken rule refuses the manifest when it is read
(`ManifestError::InvalidObjectField`, naming `related_actions/<name>`).

What the engine does with them:

- **On entry only.** The action is created when an object enters the
  allowlist: when the plugin's objects come into service (at install and
  on every resume, for objects in the starting list), when an
  administrator adds the object, and, for a declaration a new plugin
  version adds, for every object already in the list. The engine remembers
  per declaration which objects it was offered to (`related_actions` in
  both stores), and forgets an object when it leaves the allowlist. An
  action an administrator deleted therefore stays deleted until its object
  leaves the allowlist and enters it again.
- **An ordinary workspace action.** Once created, administrators may edit,
  reorder or delete it like their own. Upgrades do not touch it; a changed
  declaration applies to objects entering the allowlist afterwards.
- **Never in the way.** It is created only if the object has no quick
  action of that name and fewer than 10 actions, and only if it passes the
  same checks as an administrator's action. Otherwise it is skipped and
  the skip is audited as `objects.quick_action.skipped`, with the reason
  (`quick_actions.duplicate_name`, `quick_actions.limit_reached` or the
  check's code); the allowlist change or resume still succeeds. A skipped
  object counts as offered.
- **Hidden with the plugin.** While the plugin is disabled, an object's
  create actions that make its records are left out of the quick-action
  list, and come back unchanged when it is enabled.
- **It grants nothing.** Running it is the record create API with the
  member's own permissions, sharing, validation rules and automations
  (ADR 0032 §3); members without `create` on the declaring object cannot
  use it.

## Activity timeline

An object may declare how its records appear on the Activity timeline
(ADR 0033 §4) of the records they point at, at most once:

```toml
[objects.timeline]
through = ["related_to", "who"]                # lookup or related-to fields that point at the viewed record
date = "due_date"                              # the date or date-time field ordering the timeline
open = { field = "status", equals = "open" }   # optional: open while this picklist has this value
```

`open = "upcoming"` instead makes a record open while its `date` is today or
later (UTC).

| Setting | Rule |
| --- | --- |
| `through` | 1 to 5 distinct short names of the object's lookup or related-to fields, in order of preference: an item names the first one that points at the viewed record. |
| `date` | The short name of an indexed `date` or `date_time` field: the timeline filters and sorts on it. It is required, so `"upcoming"` always has a date. |
| `open` | Optional. `{ field, equals }` names an indexed picklist and one of its options; `"upcoming"` makes a record open while its date is today or later. Without `open`, every record is past. |

Unknown keys are refused, and a broken rule refuses the manifest when it is
read (`ManifestError::InvalidObjectField`, naming `timeline/through`,
`timeline/date` or `timeline/open`).

What the engine does with it:

- **Kept per workspace.** Installing or upgrading the plugin stores the
  declaration with its fields' ids (`timeline_sources` in both stores, per
  workspace, with forced RLS in PostgreSQL); a new version that drops it
  drops it.
- **Domain-neutral.** The kernel serves every source the same way
  (`GET .../records/{id}/timeline`, see the [objects API](../api/objects.md)):
  it does not know what a task or an event is.
- **As the viewer.** Records are read through the query language with the
  viewer's permissions and sharing, like related lists. A suspended object
  (its plugin disabled) and an object the viewer cannot read contribute
  nothing.
- **Which records.** A source is followed through each `through` field that
  can point at the viewed record's object: a lookup to it, a related-to
  field whose allowlist names it, or one still holding a value to one of
  its records after the object left the allowlist.

## Enabling, upgrading and disabling

| Plugin change | What happens to its objects in the workspace |
| --- | --- |
| Enable | The kernel creates the declared objects and fields that are missing, and brings the labels, descriptions, options and flags of existing ones in line with the manifest. Then they come into service. |
| Enable again, or enable a new version | The same, idempotently. A field the manifest no longer declares stays, with its values. A picklist option it no longer declares stays, inactive. A related-to field keeps the workspace's allowlist. New related actions are offered to the objects already allowed. Timeline declarations are replaced, or dropped with the manifest's. |
| Disable | The objects are suspended: describe, lists, records, queries, search, related lists, global New, navigation and device replicas treat them as missing. Nothing is deleted. |
| Enable after disabling | They come back unchanged, records included. |
| Remove | They stay suspended and keep their records for export. Reinstalling brings them back. No purge happens yet. |

Enabling is refused, and the plugin stays as it was, when the workspace
cannot take the objects:

| Code | Reason |
| --- | --- |
| `409 plugin.objects_field_type_changed` | A declared field's type differs from the installed field's. |
| `409 plugin.objects_field_incompatible` | A declared field changes its uniqueness, decimal places, lookup target or delete behaviour. |
| `409 plugin.objects_key_prefix_changed` | A declared object's key prefix differs from the installed one. |
| `409 plugin.objects_key_prefix_taken` | Another object already uses the key prefix in the workspace. |
| `409 plugin.objects_name_taken` | A declared API name belongs to a definition the plugin does not own. |
| `409 plugin.objects_invalid` | The object engine refuses a declaration. |
| `503 plugin.objects_unavailable` | The plugin's state changed but its objects did not follow yet; repeat the request. |

Every change is made through the object engine by a kernel installer, not
with a member's permissions, and leaves the usual audit entry and
`platform.objects.object-*.v1` or `field-*.v1` event. Their payloads carry
`"plugin": "<plugin id>"`, and the actor is the member who changed the
plugin's state. Suspending and resuming are audited as
`objects.object.suspended` and `objects.object.resumed`.

## What workspaces may change

Plugin objects are described with `"origin": "plugin"` and
`"owner_plugin": "<plugin id>"`, and their fields with `"origin": "plugin"`.

- **Administrators may** add `custom` fields, validation rules, automations,
  quick actions and object permissions, choose the object's
  `sharing_model` and `grant_hierarchy`, and choose the allowlist of the
  plugin's related-to fields.
- **Administrators may not** otherwise change or delete the plugin's own fields, or
  change the plugin object's labels, description or flags, or delete it:
  the API answers `409 objects.plugin_owned`. Relabelling is the plugin's,
  through a new version.
- **Only installation** creates objects and fields in a plugin's namespace;
  the object API always creates `custom` names.

Role permissions on a plugin object start as for a new workspace object: no
role has any access until an administrator grants it, and only members with
`records.view_all` or `records.modify_all` see its records before then.

Formulas, validation rules and queries name plugin fields by their API name,
dots and all: `SELECT name, example.tracker:milestone.name FROM
example.tracker:ticket`. A dotted namespace directly followed by `:` is read
as one name, so traversal into a plugin field goes through a namespaced
lookup, as in `custom:ticket.example.tracker:summary`.
