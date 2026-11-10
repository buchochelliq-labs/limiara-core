# Declaring objects

**Status:** experimental. Design: [ADR 0033 §2](../adr/0033-activities.md),
naming in [ADR 0015 §3](../adr/0015-metadata-driven-objects.md).

A plugin can ship engine objects: definitions the object engine stores and
serves like the objects a workspace administrator creates, with records,
sharing, permissions, validation rules, automations, quick actions, search,
reports and device sync. When a workspace enables the plugin, the kernel
creates or migrates the objects there. The fixture `fixtures/plugins/tracker`
declares a Milestone and a Ticket that looks one up.

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
| `reference_objects` | A related-to field's allowlist: 1 to 10 distinct targets. |
| `delete_behaviour` | A lookup's: `clear` (the default) or `restrict`. |

A target is an API name: one of the plugin's own objects, or an object of a
plugin the manifest declares a dependency on. A plugin cannot point at
`custom` objects, which exist only in some workspaces, and cannot declare
anything in the `custom` namespace. A manifest declares at most 20 objects
and 100 fields per object. The manifest is refused when it is read if any
rule is broken (`ManifestError::InvalidObject` or `InvalidObjectField`),
and unknown keys are refused rather than ignored.

## Enabling, upgrading and disabling

| Plugin change | What happens to its objects in the workspace |
| --- | --- |
| Enable | The kernel creates the declared objects and fields that are missing, and brings the labels, descriptions, options and flags of existing ones in line with the manifest. Then they come into service. |
| Enable again, or enable a new version | The same, idempotently. A field the manifest no longer declares stays, with its values. A picklist option it no longer declares stays, inactive. |
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
  quick actions and object permissions, and choose the object's
  `sharing_model` and `grant_hierarchy`.
- **Administrators may not** change or delete the plugin's own fields, or
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
