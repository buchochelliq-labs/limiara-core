# 0012: Plugins extending other plugins

## Status

Proposed. Accept after the extension declaration schema, the kernel field
service, and the host-upgrade compatibility check have an executable spike.

## Context

Customer operations workspaces rarely run one feature in isolation. A loyalty
plugin needs a tier and a points balance on every person; a billing plugin
needs an invoice list on an organisation record; a partner may ship a brand-new
object, such as Memberships, that belongs next to People. Today a plugin can
contribute UI extensions to another plugin's pages
(`ExtensionContribution` in `crates/plugin-contract/src/ui.rs`, exercised by
`fixtures/plugins/person-context`), and Contacts lets tenants define custom
fields (`p_contacts.custom_field_definitions` with values in
`p_contacts.people.custom_fields`). Nothing yet lets one plugin add *data* or
*behaviour* to another plugin's objects.

The constraints are fixed by earlier decisions:

- The kernel stays domain-neutral and the platform rejects a universal
  object/EAV engine (ADR 0005, since superseded by ADR 0015). Domain data stays typed, indexed and owned by
  its plugin.
- Plugins never read another plugin's private tables. Cross-plugin interaction
  goes through public plugin APIs, versioned events, governed read models or
  kernel metadata services.
- Every tenant table carries `tenant_id`, tenant-first indexes and forced RLS
  (ADR 0003), and writes go through tenancy, permission, audit, lifecycle,
  capability and event policy.
- User permissions, plugin runtime capabilities and commercial entitlements
  stay separate (ADR 0006).

The product direction was set in the UX design review for extensions: values
are stored "both, by field kind"; new objects live in the extending feature and
an administrator chooses whether to pin them into another feature; disabling
hides an extension and removing it keeps its data for 30 days; and any plugin
may extend any object, with safeguards.

## Decision

### 1. Extensions only add

An extending plugin may add fields, record-page sections and cards, actions,
related lists, list columns and filters, and new objects. It may not remove,
rename, retype or relax anything the host declares: no changes to another
plugin's fields, validation rules, required flags, layouts, slots, permissions
or lifecycle hooks. The kernel rejects a manifest that tries.

Any installed plugin may target any record resource another plugin declares,
and one plugin may extend several hosts at once, for example adding a tier to
Contacts people and a renewal date to a billing plugin's accounts. Hosts do not
opt in per resource: providing extension storage is part of being a record
host. Kernel-internal tables such as sessions, audit and the outbox are not
plugin resources and cannot be extended.

### 2. Declarations live in the manifest

The extending plugin declares each field in `plugin.toml` under
`[[extensions.fields]]`, naming the `host` plugin, the `target` resource, a
short `key`, the field type, and its own read and write permissions. Related
lists, actions and objects follow the same pattern in later contract versions,
alongside the existing `[[ui.extensions]]`. A dependency on each host with
`reasons = ["extension"]` is required, and its version requirement is the
host contract range the extension supports. Field identifiers are namespaced
by the extending plugin (for example `example.loyalty.tier`) so they can never
collide with host fields, tenant custom fields or another extension.

### 3. Storage depends on the field kind

- **Simple values stay with the record.** Text, choice, number, date, yes/no
  and reference fields with a single value are persisted by the host in a
  namespaced extension column on its own record table (for Contacts,
  `p_contacts.people.extensions jsonb`, keyed by plugin namespace, with a GIN
  index next to the existing `custom_fields` index). This keeps them under the
  host's RLS policy, in the same row version, and filterable, sortable,
  searchable and exportable through the host's list and search APIs without a
  join across plugins.
- **Everything else stays with the extension.** Values with many rows, their
  own history, their own lifecycle or their own permissions (points ledgers,
  invoices, memberships) live in the extending plugin's own tables, which
  follow every tenancy rule, and are shown on host pages through the extending
  plugin's public API as related lists or cards.

The host owns the column; the extending plugin owns the meaning of its
namespace. Neither writes the other's storage directly.

### 4. The kernel field service mediates every write

Writes to simple extension values go through a kernel field service, not the
host's tables and not a direct host API call from the extending plugin:

1. The kernel resolves the declaration and checks that both the extending
   plugin and the host are active for the tenant and still compatible.
2. It checks the caller's user permission for the extension field (each
   extension declares its own read and write permissions, which roles grant
   like any other), the extending plugin's runtime capability when the write
   comes from plugin code, and the tenant's entitlement when the feature is
   commercial. These three checks stay separate.
3. It validates the value against the declared type and constraints.
4. It calls the host's extension-value port with the namespaced patch and the
   expected record version. The host applies it in its own transaction, with
   `app.tenant_id` set transaction-locally, bumps the record version, writes
   the audit entry and emits the record-changed event through the outbox like
   any other edit.

Host user interfaces render extension fields from the kernel's merged
resource metadata, so a user editing a record sees host fields, tenant custom
fields and extension fields in one form and one save. Extension field labels
and values are plugin data and render inertly.

The extension-value store is deliberately limited to declared, typed, simple
fields on resources that opted in. It is not a general-purpose value store,
and it gives no plugin a way to define unbounded schemas at runtime.

### 5. New objects belong to the extending feature

An object a plugin adds, such as Memberships, is a full resource of that
plugin, with its own tables, permissions and pages, and appears in that
plugin's feature. An administrator may pin it into another feature's object
bar; the pin is workspace configuration, not a change to the host plugin.
Relationships to host records use references by identifier and the host's
public API, never foreign keys into the host's schema.

### 6. Lifecycle

- **Install and enable.** Before enabling, the administrator reviews an
  *extension plan*: which objects are extended, which fields, sections and
  actions are added, which permissions are introduced, and where data will be
  stored. Enabling applies the plan.
- **Disable.** Disabling hides every contribution immediately. Stored values
  stay in place, are not returned by host APIs, and reappear on re-enable.
- **Remove.** Removal hides contributions and starts a 30-day retention
  window, during which an administrator can export the extension's data or
  reinstall. After the window the kernel asks the host to purge the namespace
  from its extension column and the extending plugin's own tables are dropped
  by its uninstall migration. The purge is audited.
- **Host upgrade.** On every host upgrade the kernel re-checks each extension
  against the new host contract version. A contribution whose target slot,
  resource or field type no longer matches is paused, shown to administrators
  with the reason, and resumed when a compatible extension version is
  installed. Paused contributions never block the host upgrade.

### 7. Ordering and conflicts

Extensions that target the same slot are ordered by declared priority and
then by plugin identifier, as `ExtensionContribution` already does for UI
patches. Field namespaces cannot collide. Two extensions may add similar
fields; administrators decide which to show.

## Alternatives considered

- **A kernel-owned extension value table** keyed by tenant, resource, record
  and namespace. It keeps hosts unaware of extensions, but it turns the
  kernel into a universal value store that ADR 0005 rejects, splits one
  record across two stores with separate versions and audit trails, and needs
  a cross-store query helper for every host list and search. Rejected.
- **Extending plugins write host tables directly** with migrations that add
  columns. Fast to query, but it lets one plugin bypass the host's ownership,
  validation and RLS policy, and couples upgrades of unrelated plugins.
  Rejected.
- **Only related data in the extending plugin's tables.** Safe, but simple
  fields could not be filtered, sorted or shown in highlights without joins
  across plugins, which defeats the product goal. Rejected in favour of the
  split by field kind.
- **Allow extensions to override host behaviour** (hide fields, change
  validation). Rejected: it makes host behaviour unpredictable and breaks the
  guarantee that a host's rules hold whatever is installed.

## Consequences

- Each record host must add an extension column, its index and an
  extension-value port, and must merge extension metadata into its resource
  descriptions. Contacts is the first host.
- The kernel gains an extension registry, the field service, the extension
  plan, retention and purge jobs, and the host-upgrade compatibility check.
  These are domain-neutral: they reason about declarations and namespaces,
  not about loyalty or billing.
- Extension values share the host record's version, so an extension write
  and a host write to the same record conflict and are retried like any
  concurrent edit.
- Host list and search APIs must accept namespaced field keys for filtering
  and sorting, bounded to declared simple fields.
- Documentation for plugin developers must cover extension declarations,
  permissions and the lifecycle above.

## Implementation status

Issue #271 tracks the work. Implemented and covered by unit, PostgreSQL and
HTTP acceptance tests (`scripts/test-plugin-runtime.sh`,
`scripts/test-plugin-extensions-acceptance.sh`):

- `[[extensions.fields]]` manifest declarations, including one plugin
  extending several hosts (#272).
- The `platform-extensions` kernel crate: catalog, pause reasons, write
  validation into a patch only the kernel can build, read projection, list
  filters, descriptors and the extension plan (#273).
- Contacts as the first host: `extensions` columns, versioned and audited
  writes, list filters (#274).
- API routes for descriptors, the extension plan, value writes, export and
  removal, with projected values on every Contacts record response (#275).
- The person record page panel for reading and editing extension fields
  (#276).
- Removal with a 30-day retention window, export, reinstall, and a worker
  purge through a host port with audit evidence (#277).

Descriptors are served by `GET .../extensions/fields` rather than merged into
the UI resource metadata documents; merging them is a follow-up. Dropping an
extending plugin's own tables on removal waits for a plugin uninstall
migration mechanism.

## Follow-up work

Tracked in issue #271 and `docs/roadmap/`.

- Manifest schema for `[[extensions.*]]` and `[[objects]]`, with validation
  that rejects non-additive changes and missing `extension` dependencies.
- Contacts migration for `people.extensions` and `organisations.extensions`
  with GIN indexes, and the extension-value port.
- Kernel field service, extension registry and merged resource metadata.
- Extension plan review, disable, removal retention, export and purge.
- Host-upgrade compatibility check and paused-contribution reporting.
- Web: render extension fields, sections and related lists from metadata; the
  settings screens for extension plans and paused contributions.
