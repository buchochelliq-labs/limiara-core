# Extending other plugins

**Status:** experimental. Design: [ADR 0012](../adr/0012-plugin-extensions.md).

A plugin can add fields to the records of other plugins without touching their
code or tables. One plugin may extend several record resources, and resources
of several host plugins, at the same time. The fixture
`fixtures/plugins/loyalty` adds a loyalty tier, a join date and a points
balance to Contacts people, and an account code to Contacts organisations.

Extensions only add. They cannot change or remove a host's fields,
validation, required flags, layouts or permissions.

## Declaring fields

Declare each host as a dependency with the `extension` reason. Its version
requirement is the range of host versions your fields support. Then declare
each field under `[[extensions.fields]]`:

```toml
[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.fields.read"
description = "See loyalty fields on records"
[[permissions]]
id = "loyalty.fields.write"
description = "Change loyalty fields on records"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.fields.read"
write_permission = "loyalty.fields.write"
```

| Setting | Rule |
| --- | --- |
| `host` | Another plugin's id, declared as a dependency with the `extension` reason. A plugin cannot extend itself. |
| `target` | A resource id owned by the host, such as `platform.contacts.person`. |
| `key` | Lowercase letters, digits and underscores, starting with a letter, up to 40 characters, unique per target. |
| `field_type` | `text`, `number`, `boolean`, `date` or `choice`. |
| `options` | Required for `choice` and only allowed there; up to 50 unique values. |
| `max_length` | Text only; 1 to 2000 characters, default 500. |
| `read_permission`, `write_permission` | Permissions your plugin declares. Roles grant them like any other permission. |

A manifest may declare up to 100 extension fields. The public field id is
`<plugin id>.<key>`, for example `example.loyalty.tier`.

## How values are stored and checked

Simple values are stored with the host record, in a separate namespace for
your plugin. Hosts only store values the kernel extension service has
validated. For every write the kernel checks that:

- your plugin and the host are both active in the workspace;
- the active host version still satisfies your dependency requirement and
  still declares the target;
- the caller holds the field's write permission (and its read permission);
- each value matches the declared type, options and length.

Reads return only fields the caller may read from active extensions. Event and
audit payloads never carry extension values. Larger data, history or data
with its own lifecycle belongs in your plugin's own tables, shown on host
pages through your public API.

## Where fields appear

Extension fields appear as ordinary fields of the record, with no separate
section or label naming the plugin that adds them. A host declares the places
that can hold them as field zones:

```toml
[[ui.field_zones]]
id = "contact-information"
resource = "platform.contacts.person"
label = "Contact information"
kind = "display"          # display, form or list
page = "platform.contacts.person-detail"
node = "person-identity-summary"
default = true            # unarranged fields appear here
```

Fields nobody has arranged appear in the default zone of each kind, so a newly
enabled extension shows on the record page and in the edit form without setup.
An administrator with `ui.metadata.manage` uses **Arrange fields** on the record
page to put each field into any zone, before, between or after the host's own
fields, or nowhere. The arrangement is stored per workspace and resource,
validated by the kernel against active fields and declared zones, versioned,
and audited.

Host forms save extension values in the same request and transaction as the
record: `PUT` and `POST` bodies accept an `extensions` object keyed by field id.

## When things change

- **Host upgrade.** If a new host version no longer satisfies your
  requirement or drops the target, your fields are paused, not the host.
  The extension plan shows the reason.
- **Disable.** Your fields disappear from every API and screen; stored values
  stay and return when the plugin is enabled again.
- **Remove.** Only a disabled plugin can be removed. Values stay for 30 days,
  during which an administrator can export them or reinstall the plugin.
  After that the worker purges your namespace from every host and records
  the purge in the audit log.

## Endpoints

All routes are workspace-scoped under `/api/v1/workspaces/{workspace_id}`.

| Route | Purpose |
| --- | --- |
| `GET /extensions/fields?resource={id}` | Field descriptors the caller may read, with `writable` per field, the host's zones, and where each field is placed. |
| `GET`, `PUT /extensions/layout?resource={id}` | Every field, the host's zones and the stored arrangement; saving needs `expected_revision`. Requires `ui.metadata.manage`. |
| `GET /plugins/{plugin}/extension-plan` | What enabling a plugin would add, where values are stored, the permissions it introduces, and fields that would pause. Requires `platform.plugins.manage`. |
| `POST /plugins/{plugin}/remove` | Removes a disabled plugin and starts the retention window. Requires `platform.plugins.manage`. |
| `GET /plugins/{plugin}/extension-export` | Every value the plugin stored, up to 10,000 records per call. Requires `platform.plugins.manage` and every field's read permission; audited. |

Hosts expose their own write routes. For Contacts see
[Contacts API](../api/contacts.md#extension-values).

## Not yet supported

- Related lists, actions and new objects contributed through the same
  contract; today they use `[[ui.extensions]]` and your own API.
- Sorting host lists by extension fields, and extension fields in search.
- Dropping your plugin's own tables when it is removed.
- Administration screens for the extension plan, removal and export.
- Field zones on organisation pages, which still use fixture data in the web app.
