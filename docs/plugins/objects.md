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
| `name_from` | Optional. `{ fields = ["first_name", "last_name"], separator = " " }`: two or three distinct short names of the object's own `text` fields, and a separator of at most five printable characters (a space by default). The engine then composes every record's name from them and refuses writes to the name (ADR 0035 G17). An upgrade that changes it renames the object's records. |
| `dormant` | Optional, default `false`. A dormant object is installed out of service and stays out of service when the plugin is enabled, until the plugin wakes it (see below; ADR 0035 G11). |
| `path_field` | Optional. The short name of one of the object's single-value picklists: the record page shows it as a path and the board groups by it (ADR 0037 §2). Upgrades keep it in line with the manifest. |

| Field setting | Rule |
| --- | --- |
| `name`, `label`, `description` | As for objects; names unique per object. |
| `field_type` | `text`, `long_text`, `email`, `phone`, `url`, `number`, `currency`, `percent`, `boolean`, `date`, `date_time`, `picklist`, `multi_picklist`, `lookup` or `related_to`. Formula, master-detail and roll-up summary fields cannot be declared yet. |
| `required`, `indexed`, `track_history` | As for workspace fields. |
| `unique` | Text, email, phone, URL and number fields only. |
| `searchable` | Text-like fields only. |
| `max_length` | Text (1 to 255), long text (1 to 131072) and email (1 to 320; 255 when left out) only. |
| `scale` | Number, currency and percent only; 0 to 8. |
| `options` | Required for picklists and only allowed there: up to 200, each with `value`, `label` and optional `default`, `outcome` and `weight`. `outcome` is `open` (the default), `positive` or `negative`, and only a single-value picklist's options may close a record (ADR 0037 §1). `weight` is a percentage from 0 to 100. |
| `reference_object` | A lookup's target. |
| `reference_objects` | A related-to field's starting allowlist: 0 to 10 distinct targets. Workspace administrators choose the rest (see below). |
| `delete_behaviour` | A lookup's: `clear` (the default) or `restrict`. |
| `read_permission`, `write_permission` | Optional. A permission the plugin declares in `[[permissions]]` that a member needs to read the field, and on top of that to change it ([Field permissions](../api/objects.md#field-permissions)). Upgrades apply the manifest's; workspace administrators cannot change them. |

A target is an API name: one of the plugin's own objects, or an object of a
plugin the manifest declares a dependency on (or, in a related-to
allowlist, an optional dependency: see below). A plugin cannot point at
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

### Objects of optional dependencies

A plugin that works on its own but fits with another may declare it as an
optional dependency (ADR 0035 G18):

```toml
[optional_dependencies."platform.sales"]
version = ">=0.1.0"
reasons = ["composition"]
description = "Leads and opportunities in Related to and Who"
```

It takes the same keys as `[dependencies]`, but the plugin installs and
enables without it, and it grants nothing. It may not name the plugin
itself or one of its dependencies. Its only effect is that a related-to
field's `reference_objects` may name that plugin's objects; a lookup may
not, since it has one target and would not work without it.

Such an entry is not part of the field at install. It waits until the
named object is active in the workspace, at install if the other plugin is
already enabled, or when that plugin is enabled later, and then joins the
allowlist through an ordinary field change, so the field's related actions
are offered to it as to any object entering the allowlist. Each entry
joins once per field: an administrator who removes it keeps it removed
through re-enabling and upgrades of either plugin. An allowlist that is
already full leaves the entry waiting until there is room: it joins as soon
as an administrator's change to that allowlist makes some.

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

## Starting automations

An object may declare automations the engine adds when it creates the
object (ADR 0037 §6). Each one sets values on the record being saved,
before it is saved, from formulas over the record:

```toml
[[objects.automations]]
name = "probability_from_stage"
description = "Sets the probability to the stage's weight when the stage changes"
trigger = "create_or_update"           # create, update or create_or_update
condition = "IF(ISNEW(), ISBLANK(platform.sales:probability), ISCHANGED(platform.sales:stage) && !ISCHANGED(platform.sales:probability))"
set = [{ field = "probability", formula = "PICKLISTWEIGHT(platform.sales:stage)" }]
```

| Setting | Rule |
| --- | --- |
| `name` | Unique per object: a lowercase letter, then lowercase letters, digits or single underscores. |
| `trigger` | `create`, `update` or `create_or_update`. |
| `condition` | Optional. A boolean formula of at most 2,000 characters; without one, the automation runs on every save its trigger covers. |
| `set` | 1 to 20 values. Each `field` is the short name of one of the object's own fields, set once; each `formula` has at most 2,000 characters. |

Formulas name fields by API name (`<plugin id>:<field>`), as in any
automation (ADR 0022). An object may declare up to 10. Unknown keys are
refused, and a broken rule refuses the manifest when it is read, naming
`automations/<name>`.

What the engine does with them:

- **Added once.** When an install creates the object, the engine adds each
  declaration as a before-save automation of the workspace, through the
  same checks as an administrator's: a formula the engine refuses fails
  the whole install, naming `<object>/automations/<name>/<part>`.
- **Then the workspace's.** Administrators may change, turn off or delete
  it like any other automation. Upgrades never add a starting automation
  to an object that already exists, never change one and never delete one,
  so a deleted automation stays deleted. Declaring a new one in a later
  version therefore reaches only workspaces that install the object then.
- **Only the record being saved.** It runs before save and sets fields of
  that record only, inside the same save, so validation rules still check
  the result and field history records it. It cannot update other records,
  create records or publish events.

## Reports

A plugin may declare saved reports (ADR 0037 §5, ADR 0027) at the top level
of its manifest:

```toml
[[reports]]
name = "open_pipeline_by_stage"        # unique per plugin; never shown
label = "Open pipeline by stage"       # the report's name in the workspace
description = "The amount and number of open opportunities in each stage"
query = "SELECT platform.sales:stage, SUM(platform.sales:amount) amount FROM platform.sales:opportunity GROUP BY platform.sales:stage"
chart = "bar"                          # table, bar, line, pie or metric
label_column = "platform.sales:stage"  # bar, line and pie
value_column = "amount"                # every chart but a table
```

| Setting | Rule |
| --- | --- |
| `name` | A lowercase letter, then lowercase letters, digits or single underscores, up to 63; unique per plugin. |
| `label` | 1 to 80 characters. |
| `description` | Optional, at most 1,000 characters. |
| `query` | A record query (ADR 0019) of at most 10,000 characters, reading the plugin's own objects or its dependencies'. |
| `chart` | `table` (the default) names no columns; `metric` names `value_column`; `bar`, `line` and `pie` name both. |

A plugin declares at most 20. Unknown keys are refused, and a broken rule
refuses the manifest when it is read (`ManifestError::InvalidReport`).

What the engine does with them:

- **Added on enable.** Once the plugin's objects are in service, each
  report is added as a report every member sees, owned by the member who
  enabled the plugin and naming the plugin as `owner_plugin`. Its query is
  checked against the workspace's objects as an administrator's report is:
  it must run, and its chart's columns must exist.
- **Skipped, not fatal.** A report whose query reads another plugin's
  objects, directly or through a lookup, or that the engine refuses, is skipped and audited as
  `reports.plugin_report.skipped` with the reason. Enabling the plugin
  goes ahead.
- **The workspace's to change.** Members who may edit it can change or
  delete a plugin's report. An upgrade updates only reports nobody has
  changed since the plugin last did, and never deletes one; a report the
  workspace deleted is never added again.
- **As the viewer.** A report runs as whoever opens it, with their
  permissions and sharing, like any report.
- **Hidden while disabled.** Disabling the plugin hides its reports, and
  enabling it brings them back unchanged.

## Enabling, upgrading and disabling

| Plugin change | What happens to its objects in the workspace |
| --- | --- |
| Enable | The kernel creates the declared objects and fields that are missing, and brings the labels, descriptions, options and flags of existing ones in line with the manifest. Then they come into service. |
| Enable again, or enable a new version | The same, idempotently. A field the manifest no longer declares stays, with its values. A picklist option it no longer declares stays, inactive. A related-to field keeps the workspace's allowlist. New related actions are offered to the objects already allowed. Timeline declarations are replaced, or dropped with the manifest's. |
| Disable | The objects are suspended: describe, lists, records, queries, search, related lists, global New, navigation and device replicas treat them as missing. Nothing is deleted. |
| Enable after disabling | They come back unchanged, records included. |
| Remove | They stay suspended and keep their records for export. Reinstalling brings them back. No purge happens yet. |

A **dormant** object (`dormant = true`, ADR 0035 G11) is created like any
other but does not come into service when the plugin is enabled. The
kernel brings it into service only when the plugin wakes it, as its own
step (for Contacts, the cutover of a migrated workspace), through the
engine's `wake_plugin_objects`, which only the plugin's installer may call.
Waking is for good: from then on the object follows its plugin like any
other, and an upgrade that still declares it dormant leaves it awake. Only
a dormant object can be woken; waking an awake one does nothing, and an
object the plugin does not own is refused.

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
`objects.object.suspended` and `objects.object.resumed`, and waking a
dormant object as `objects.object.woken`.

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
Its [assistant policy](../api/objects.md#assistant-policy) (ADR 0034 §4)
is the workspace's too: a manifest cannot declare one, so a plugin object
starts with assistant access off, and only an administrator turns it on.

Formulas, validation rules and queries name plugin fields by their API name,
dots and all: `SELECT name, example.tracker:milestone.name FROM
example.tracker:ticket`. A dotted namespace directly followed by `:` is read
as one name, so traversal into a plugin field goes through a namespaced
lookup, as in `custom:ticket.example.tracker:summary`.
