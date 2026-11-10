# 0035: Contacts on the object engine

## Status

Proposed (2026-11-10). Builds on ADR 0015 (metadata-driven objects; §6
leaves Contacts on its typed store "until it is migrated"), ADR 0033 §2
(plugin-declared engine objects), ADR 0012 (plugin extensions), ADR 0013
(storage backends), ADR 0014 (standard record fields), ADR 0016 (sharing),
ADR 0017 (field history and change sources) and ADR 0011 (UX). This is a
design only; nothing in it is implemented yet. Delivery slices are in §8 and
`docs/roadmap/README.md`.

## Context

Contacts (`platform.contacts`) is the last first-party capability with its
own typed storage. Everything built since ADR 0015 (sharing, the query
language, validation rules, automations, quick actions, related lists, the
Activity timeline, field history with change sources, global search, bulk
jobs, API clients, change data capture and device replicas) works on engine
objects only. People and organisations get none of it, and ADR 0033 had to
leave Activities' *Who* field pointing at workspace objects, because Person
is not an engine object.

### What Contacts is today

**Records.** Two record kinds in `p_contacts` (PostgreSQL) and in
`p_contacts_*` tables in each workspace file (SQLite):

| Kind | Public ID | Own fields | Other columns |
| --- | --- | --- | --- |
| Person (`people`) | `per…` | `full_name` (name, 1 to 240 characters), `email` (up to 320), `telephone` (not validated), `organisation_id` (foreign key to an organisation), `tags` (free-form `text[]`, no limits) | `custom_fields jsonb`, `extensions jsonb`, `version`, `archived_at`, `deleted_at`, `created_*`, `updated_*`, `touched_at`, `name_key`, `email_key` |
| Organisation (`organisations`) | `org…` | `name` (1 to 240), `email`, `telephone` | the same, without tags or a parent |

Public IDs use the platform format (`platform_id`, 18 characters) from
per-workspace counters in `p_contacts.public_id_counters`; the prefixes are
registered in the kernel's `id_prefix_registry`. Records have no owner.

**Supporting tables.** `custom_field_definitions` (per workspace and
target: `__c` API name, label, type, required, unique, indexed, options,
`validation_schema` limited to `min_length`/`max_length` and
`minimum`/`maximum`, `data_classification`, `purpose`, `retention_days`,
`encrypted`, and an active → disabled → archived lifecycle), `list_cursors`
(keyset cursors, cleaned by a job), and `activity`, the change timeline
built by the worker consumer `platform.contacts.activity-timeline-v1` from
the plugin's own events (event id, action, version, actor, time; never
values). Name and email search keys are computed in Rust (migration 0007).
All tables are tenant-first with forced RLS and a restrictive
`app.plugin_id = 'platform.contacts'` policy (migration 0005).

Custom field types are `text`, `textarea`, `email`, `phone`, `number`,
`boolean`, `date`, `datetime`, `select`, `multi_select` and `json`.

**Extensions (ADR 0012).** Other plugins add simple fields (`text`,
`number`, `boolean`, `date`, `choice`) to `platform.contacts.person` and
`platform.contacts.organisation`. Values live in `extensions` keyed by
plugin and then field key, each field with its own read and write
permission; the kernel extension service validates every write and
Contacts applies it in its transaction. The loyalty fixture is the
reference extender.

**Behaviour.** Create, update (whole-record `PUT` with `expected_version`),
archive, delete to the recycle bin and undelete; a purge job removes
records 15 days after deletion and unlinks people from purged
organisations as a system change. Lists filter by name (substring of the
name key), email (exact, case-insensitive), organisation, extension field
and status (`active`, `archived`, `all`, `deleted`), sorted by name,
creation or update.

**Permissions.** `contacts.people.read|write`,
`contacts.organisations.read|write`, `contacts.settings.manage` (custom
field definitions), and `contacts.people.custom-fields.read` /
`contacts.organisations.custom-fields.read`. Write covers create, update,
archive and delete of any record. The two custom-field permissions are
checked only by the web app (`apps/web/src/contacts`); the API returns
custom fields to every reader.

**Events.** Fourteen `platform.contacts.*.v1` types: person and
organisation `-created`, `-updated`, `-archived`, `-deleted`,
`-undeleted`, and custom-field definition `-created`, `-updated`,
`-disabled`, `-archived`. Payloads carry only the public ID (or the
definition id and target) and the version. Audit entries name the action
and never values.

**API.** `/api/v1/workspaces/{id}/contacts/people` and
`.../organisations` (list, create, get, `PUT`, `DELETE`, `/archive`,
`/undelete`, `PATCH /extensions`, `GET /activity`),
`.../contacts/custom-fields` (create, list per target, get, update,
`/disable`, `/archive`), and the prototype
`.../odata/contacts/people` with its `$metadata`.

**UI.** Hand-written React pages in `apps/web/src/contacts` (people and
organisation directories, person and organisation detail, person form)
driven by the plugin's page and resource metadata (`plugins/contacts/ui`),
with ADR 0012 field zones on the person pages and workspace
customisation overlays (ADR 0011 §4). The shell imports these pages
directly.

**Devices.** Contacts does not reach phones. Device replicas (ADR 0024)
carry engine objects only, so no Contacts data is synced today.

**Tests.** The store conformance suite runs on both adapters
(`plugins/contacts/backend/tests`); `scripts/test-contacts-api.sh`,
`scripts/test-plugin-extensions-acceptance.sh`,
`scripts/test-plugin-lifecycle-acceptance.sh`,
`apps/worker/tests/contacts_worker_postgres.rs`,
`tests/security/tenant_isolation.sql` and `tests/contacts-metadata` cover
the API, extensions, lifecycle, worker and isolation.

**Found while investigating.** PostgreSQL migration 0004 limits
`p_contacts.activity.action` to `created`, `updated` and `archived`, but
the worker also writes `deleted` and `undeleted` (the SQLite table has no
such check). Those inserts fail as retryable on PostgreSQL. This is a
separate defect to fix on the current store; the migration in §2 must not
depend on those rows existing.

## Decision

Contacts moves onto the object engine. People and organisations become
plugin-declared engine objects; existing data is copied per workspace by a
verified, restartable, reversible migration; the public REST API and events
stay as a thin compatibility layer for a deprecation period; the UI becomes
the generic engine pages. Afterwards the plugin owns metadata and a little
translation code, and no tables.

### 1. Target model

Contacts declares two objects through `[[objects]]` in its manifest. Their
key prefixes stay `per` and `org`, which ADR 0015 §3 already names as the
plugin prefixes, so every existing public ID stays valid.

| Object | Label / plural | `name_label` | Prefix | Sharing | History |
| --- | --- | --- | --- | --- | --- |
| `platform.contacts:person` | Person / People | Full name | `per` | `read_write` | tracked |
| `platform.contacts:organisation` | Organisation / Organisations | Name | `org` | `read_write` | tracked |

`read_write` is the starting value because it matches today: every member
with object access sees and edits every record. Workspaces may tighten it
afterwards (ADR 0033 §2).

Fields (API names `platform.contacts:<name>`):

| Object | Field | Type | Flags |
| --- | --- | --- | --- |
| Person | standard `name` | (full name) | searchable, as every name |
| Person | `email` | email | indexed, searchable, `max_length` 320 (see gap G8) |
| Person | `telephone` | phone | searchable |
| Person | `organisation` | lookup to `platform.contacts:organisation` | `delete_behaviour = "clear"`, history tracked |
| Person | `tags` | text list (gap G7) | indexed |
| Organisation | standard `name` | (name) | searchable |
| Organisation | `email` | email | indexed, searchable |
| Organisation | `telephone` | phone | searchable |

The person → organisation relation is an ordinary lookup, so the
organisation page gets a People related list (ADR 0028) and the query
language traverses it (`platform.contacts:organisation.name`). The name
stays one field: Contacts has always stored a full name, and splitting it
into first and last names is a product change, not a migration
requirement (open question 1).

#### Engine gaps

The engine cannot yet express everything Contacts does. Each gap is either
engine work that ships first (slice 1) or an accepted behaviour change.

| # | Gap | Plan |
| --- | --- | --- |
| G1 | **Archiving.** `records.archived_at` exists, but the engine has no archive operation, list filter or event. | Engine first: archive and unarchive operations (an edit, needing edit access), `status=archived` and `status=all` on lists, `archived_at` in the query language, `platform.objects.record-archived.v1` and `-unarchived.v1`, and a lifecycle entry in history. Unarchive is new for Contacts. |
| G2 | **Recycle-bin purge.** Engine records in the bin stay forever; Contacts purges after 15 days (ADR 0014 §5). | Engine first: a per-workspace purge job with the same 15-day window and batch audit, removing pivots, history and change-log rows with the record. |
| G3 | **Restoring cleared lookups.** Deleting an engine record clears lookups to it at once, as members' edits, and undelete does not put them back. Contacts keeps the people → organisation link until the purge. | Engine first: undeleting a record restores the lookup values its deletion cleared, where the referencing record has not changed that field since (Salesforce behaviour). Accepted change: the clear itself, which moves each person's version and emits `person-updated` when their organisation is deleted. |
| G4 | **Importing with preserved identity.** The engine keeps an assigned id only from the device range (ADR 0031), stamps the caller and now, starts version 1, and runs validation rules and automations. | Engine first: a kernel-only *migrator* caller (like `ObjectCaller::plugin_installer`, never reachable from a request) that writes a record with a given server-range public id and internal id, stamps, version, archive and delete state and owner; writes the change log but no per-record outbox events, runs no automations or validation rules, and leaves one audit entry per batch; raises the object's `record_sequence` to at least the source counter; and appends backdated history entries with a `system` source. |
| G5 | **Field-level permissions.** Extension fields have their own read and write permissions, enforced by the kernel; the engine has none (#316). | Engine first: a field may name a `read_permission` and `write_permission` (permission ids); without them a field is hidden from reads, queries, search results, history, replicas and the change feed, and refused on writes. Role-based field-level security (#316) can build on the same check later. |
| G6 | **Fields one plugin adds to another plugin's object.** ADR 0033 §2 lets a plugin declare fields only on its own objects. | Engine first: the installer accepts ADR 0012 `[[extensions.fields]]` whose host resource maps to an engine object (§2), creating fields in the extending plugin's namespace (`example.loyalty:tier`, origin `plugin`, owner the extender), suspended with the extender, kept on removal for the ADR 0012 retention window, then retired. Extension types map to `text`, `number`, `boolean`, `date` and `picklist`. |
| G7 | **Free-form tags.** A multi-select picklist needs a fixed option list (at most 200) that only the plugin may change on a plugin field. | Engine first: a domain-neutral *text list* field type: up to 50 distinct values of up to 80 characters, compared without case, indexed per value for `INCLUDES` filters. The dry run reports tags beyond those limits. |
| G8 | **Long email addresses.** Engine email values are limited to 255 characters; Contacts allows 320. | Engine first: let email fields declare `max_length` up to 320. |
| G9 | **Field governance metadata.** Custom fields carry `data_classification`, `purpose`, `retention_days` and `encrypted`; Contacts requires a purpose for non-public fields and `encrypted` for sensitive ones. Engine fields carry none of this. | Engine first: the same four properties, with the same rules, on every engine field (domain-neutral; every object benefits). They stay declarations, as today: nothing enforces retention or encryption yet. |
| G10 | **Customising one object.** `contacts.settings.manage` lets a role manage Contacts' custom fields only; the engine needs `objects.manage`, which covers every object. | Engine first: an object permission flag `customise` (add, change and retire `custom` fields, validation rules, quick actions and automations on that object), granted per role like the other flags. `objects.manage` keeps implying it. |
| G11 | **Keeping plugin objects dormant.** Installing a plugin's objects resumes them when the plugin is enabled. Contacts' objects must exist but stay out of service until a workspace is migrated. | Engine first: an object a manifest marks `dormant = true` is installed suspended, and only its plugin's explicit resume (the cutover, §3) brings it into service. |
| G12 | **Substring name search.** Contacts' `search` matches anywhere in the name; engine global search matches term prefixes. | No gap: the compatibility list maps `search` to `name LIKE '%…%'` in the query language, which scans one object in one workspace (ADR 0015 consequences). Global search in the shell uses prefixes, as for every object. |
| G13 | **JSON custom fields.** The engine has no JSON type. | Accepted change: a `json` custom field becomes a `custom:` long-text field holding the canonical JSON text. The compatibility API parses it back, so v1 clients see no change; the engine API and pages show text. |
| G14 | **Disabled custom fields.** The engine's field lifecycle is active or deleted (retired, values kept). | Accepted change: disabled and archived definitions become retired fields. Their values stay in the record document, hidden as today's archived values are; a disabled field can no longer be read back. |
| G15 | **Formula fields in manifests.** Not declarable (roadmap). | Not needed: Contacts declares none. |
| G16 | **Workspace page layouts.** Contacts pages carry field zones and published customisation overlays; generic engine pages have no layouts yet. | Handled by the UI slice (§3): a workspace with published overlays or arranged field zones keeps the Contacts pages until the engine has record layouts. |

Unique custom fields compare without case in the engine (ADR 0015 §2);
Contacts compares exact JSON values. The dry run reports values that
collide under the engine rule and the unique fields whose type the engine
cannot make unique (only text, email, phone, URL and number can be); both
block the migration until an administrator resolves them through the
current API.

### 2. Data migration

A kernel migration tool, `contacts-to-engine`, runs per workspace on both
stores. It is kernel code because it reads the Contacts tables and writes
engine tables through the G4 migrator caller; it runs in the Contacts
execution context for reads and the engine context for writes, with
`app.tenant_id` set transaction-locally, so forced RLS and the restrictive
plugin policies apply as for any other code. It is exposed as an
administrator operation (`POST .../contacts/migration` with `dry_run`,
`start`, `status`, `rollback`), needing `platform.plugins.manage`, and in
the admin CLI.

#### What is copied

| Source | Target |
| --- | --- |
| `people.id`, `public_id`; `organisations.id`, `public_id` | the engine record's internal and public id, unchanged |
| `full_name`, `name` | the standard `name` |
| `email`, `telephone`, `organisation_id`, `tags` | the plugin fields of §1; the lookup by internal id |
| `version`, `created_at`, `created_by`, `updated_at`, `updated_by`, `touched_at` | the same standard columns, unchanged |
| `archived_at`, `deleted_at` | the same; recycle-bin records stay in the bin with their deletion time, so the G2 purge keeps the 15-day window |
| (no owner) | `owner_id` = `created_by` when that user is still a workspace member, otherwise no owner (open question 2) |
| `public_id_counters` | `record_sequence` of each object, raised to at least the counter |
| `custom_field_definitions` | `custom:` fields on the plugin objects: `foo__c` becomes `custom:foo` (ADR 0015 §3, #294), keeping the definition id as the field id, label, description, required, unique, indexed, options and the G9 governance properties; `validation_schema` limits become `max_length` where the type has one, and workspace validation rules otherwise (`LEN(custom:foo) >= 3`, `custom:size <= 10`) |
| `custom_fields` values | the record document under the new API names |
| `extensions` values | the record document under the extender's field API names (`example.loyalty:tier`), the fields created first by the G6 installer |
| `activity` | backdated history entries (G4): `created`, `deleted` and `undeleted` as lifecycle entries, `updated` as a change with no field values ("details not recorded"), `archived` as a change of `archived_at`; actor and time kept, source `system` |

Custom field types map `text` → text (long text when any value or the
declared `max_length` exceeds 255), `textarea` → long text, `email`,
`phone`, `number` (scale from the data, at most 8, rounding reported),
`boolean` and `date` → the same, `datetime` → date-time, `select` →
picklist, `multi_select` → multi-select picklist, `json` → long text (G13).

**Extension fields become the extending plugin's engine fields** (G6),
rather than workspace `custom:` fields or a side store. ADR 0012 §3
already keeps simple extension values with the host record, under the
host's tenancy, version and audit; on the engine that is simply a field in
the extender's namespace on the host object, with its own permissions
(G5) and the extender's lifecycle. Extending plugins change nothing:
Contacts' manifest maps its resource ids `platform.contacts.person` and
`platform.contacts.organisation` to the two objects, and the kernel
extension service resolves declarations and writes through the engine.
Values that ADR 0012 keeps in the extender's own tables are untouched.

Audit entries and outbox events already written stay where they are; the
migration does not rewrite evidence.

#### How it runs

1. **Dry run.** Reads everything, writes nothing, and reports per object:
   record counts by state, the field mapping, every value the engine would
   refuse (phone format on `telephone`, which Contacts never validated;
   lengths; tags beyond G7's limits; case-insensitive unique collisions;
   unique fields of non-unique types), roles whose grants change shape
   (§4), published page customisations (G16), and an estimated duration.
   Any refusal blocks the start until fixed through the current API.
2. **Copy while live.** Batches of 1,000 records in id order, organisations
   before people, each batch one engine transaction that inserts or
   replaces by id and records a checkpoint in a Contacts-owned migration
   state row (`p_contacts.engine_migration`, one per workspace: phase,
   checkpoint, cutover time, rollback deadline, checksums). The copy is
   idempotent and restartable: rerunning skips records whose checksum
   already matches and continues after the checkpoint.
3. **Catch up.** Repeated passes copy records whose `touched_at` moved
   since the previous pass and remove engine records whose source was
   purged, until a pass finds fewer than 100 changes. Custom-field
   definition changes do not touch records, so every pass also compares
   each definition (type, label, required, unique, indexed, options,
   validation, governance properties, lifecycle state) with its engine
   field and re-applies any that differ, before copying records.
4. **Freeze and cut over.** Contacts writes for the workspace, including
   custom-field definition and settings writes, answer `503
   contacts.migrating` with `Retry-After`; reads continue from the old
   store. A final catch-up pass and the verification run, then the phase
   becomes `engine` and the dormant objects are resumed (G11), in one
   transaction on SQLite and as ordered, idempotent steps on PostgreSQL
   like the ADR 0033 §2 install. Only Contacts writes in that workspace
   pause, normally for seconds; the dry run's estimate decides whether a
   workspace needs a scheduled window.
5. **Verify.** Counts per object and state must match, and a per-record
   checksum over a canonical form (ids, name, every field value, stamps,
   version, states, owner rule) computed by the same Rust code on both
   sides must agree, as must an aggregate digest over the sorted ids. The
   same applies to definitions: a per-definition checksum over the
   canonical metadata listed in step 3 must match its engine field.
   Verification failing leaves the phase unchanged and writes unfrozen.

#### Reversal

For 30 days after cutover (the ADR 0012 retention window) an administrator
may roll a workspace back. The tool freezes writes, copies engine records
touched since cutover back to the old tables (new records keep their ids;
the old counters are raised past them), marks records deleted or purged
on the engine as deleted, re-creates definitions for `custom:` fields
added since cutover when their type maps back, suspends the objects again
and sets the phase back to `legacy`. A rollback that would lose data (a
field type with no Contacts equivalent, or values in fields added by
extenders after cutover) is refused with a report unless the administrator
accepts the listed losses. History, shares and owners created on the
engine are kept with the suspended objects, so migrating again later picks
them up. The old store is never written after cutover except by a rollback,
and the old activity consumer keeps projecting the compatibility events
(§3), so a rolled-back workspace keeps a complete timeline. After the
window the workspace's old rows are deleted (slice 7).

### 3. Compatibility

**The REST API and events stay as v1 over the engine, deprecated, rather
than being versioned.** A v2 would only repeat the engine's own record API,
which is the API new clients should use; keeping v1 working unchanged lets
existing integrations move on their own schedule.

- **REST.** After cutover the Contacts routes serve the engine: the
  plugin's handlers call the object service as the member (so object
  permissions, sharing, field permissions, validation rules and automations
  apply), translate field names (`full_name`, `custom_fields.foo__c`,
  `organisation_id`), turn a whole-record `PUT` into a full update, map
  `status` and sorts onto engine lists and the query language, serve
  `/activity` from history lifecycle entries, and map engine error codes to
  the existing Contacts codes. List cursors issued before cutover answer
  `400 contacts.cursor_invalid`, and clients start again. The custom-field
  routes manage `custom:` fields on the plugin objects under the G10
  `customise` flag. Responses carry `Deprecation` and `Sunset` headers
  once the compatibility layer ships.
- **Events.** Engine events are the source of truth for migrated
  workspaces. A Contacts worker consumer translates
  `platform.objects.record-*.v1`, `-archived.v1` and field events for its
  two objects into the same fourteen `platform.contacts.*.v1` types, with
  the same payloads (public ID and version), the engine event as causation
  and the same correlation and actor. It writes through the outbox in its
  own transaction, so delivery stays at least once and ordered per record.
- **Deprecation timeline.** Removed no earlier than **six months after the compatibility
  layer ships and not before the platform's 0.3 contract**: the v1 Contacts
  routes, the prototype OData endpoint, and the `platform.contacts.*.v1`
  events, which consumers replace with `platform.objects.*` events
  filtered by object. The removal is a Contacts major version. Until then
  the plugin stays in the `0.1` line so extenders' `>=0.1.0,<0.2.0`
  dependency ranges keep working.

**The UI becomes the generic engine pages.** Contacts keeps no custom pages
over engine data: the object home, record page and form of ADR 0011 §3
(`apps/web/src/objects`) already serve plugin objects, and keeping custom
pages would mean maintaining a second set of record screens forever. For a
migrated workspace the navigation entries come from the objects
(`in_navigation`), People and Organisations open the generic pages, and the
person page gets sharing, history, related lists, quick actions and the
Activity timeline. A workspace with published customisation overlays or
arranged field zones on Contacts pages (G16) keeps the Contacts pages,
working through the compatibility API, until engine record layouts exist;
the dry run lists such workspaces. Once no workspace needs them, the
Contacts pages and the shell's direct imports of them are removed, which
also puts the shell back on contribution-only navigation.

### 4. Permissions

At cutover each role's Contacts permissions become object permissions on
the two objects, so nobody loses or gains record access:

| Contacts permission | Object permission on the matching object |
| --- | --- |
| `contacts.people.read` | Person: read |
| `contacts.people.write` | Person: read, create, edit, delete, view all, modify all |
| `contacts.organisations.read` | Organisation: read |
| `contacts.organisations.write` | Organisation: read, create, edit, delete, view all, modify all |
| `contacts.settings.manage` | both: `customise` (G10) |
| `contacts.people.custom-fields.read` | read permission of Person's `custom:` fields (G5) |
| `contacts.organisations.custom-fields.read` | read permission of Organisation's `custom:` fields (G5) |

- Writers need *modify all* because ADR 0016 §1 lets only owners, the
  hierarchy above them and modify-all holders delete, while Contacts lets
  every writer delete any record. With the `read_write` model this
  reproduces today exactly. It also lets writers manage shares and transfer
  owners, which do not exist today; the dry run names these roles. A
  workspace that later tightens the sharing model must review these grants,
  which the Object Manager will point out.
- A role with write but not read gains read, because engine edit implies
  read; the dry run lists such roles.
- The custom-field read permissions become server-enforced. Today only the
  web app hides custom fields, so API callers without them lose API access
  to custom field values; the dry run lists such roles (open question 3).
- Extension fields keep their own permissions through G5.
- After cutover the `contacts.*` permissions no longer decide anything in
  that workspace; role screens mark them as replaced. They leave the
  manifest when the compatibility layer is removed.
- The `contacts.basic` entitlement still decides whether the workspace may
  enable the plugin (ADR 0006); it never becomes a permission.

### 5. Plugin boundary

After a workspace is migrated, the Contacts plugin owns:

- its manifest: the two objects and their fields, the resource-to-object
  mapping for extenders, navigation, permissions during deprecation, and
  the dormant flag;
- the compatibility REST handlers and the event-translating consumer, both
  calling only the engine and the outbox, as any plugin may;
- no tables.

Applied migrations are history, so `plugins/contacts/migrations` keeps
0001 to 0009 and gains a final migration (and a SQLite tenant step) that
drops the `p_contacts` tables once no workspace is in `legacy` phase or inside its
rollback window. The `per` and `org` rows in `id_prefix_registry` stay,
because the prefixes remain Contacts'. Retiring the old tables, the `ContactsStore` port, both
adapters, the conformance suite and the purge and cursor jobs is slice 7,
a later release. When the compatibility layer is removed too, Contacts
becomes a manifest-only plugin like Activities, with
`capabilities.database = false`.

### 6. Devices

Contacts reaches no device today. Once a workspace is migrated, people and
organisations sync to phones as ordinary engine replicas (ADR 0024): pulled
from the change log under sharing and field permissions, pushed as
composite requests, created offline with device-range ids (ADR 0031) under
`per` and `org`, and dropped from the device by the revocation set when
access is lost. The migrator's change-log entries (G4) let replicas and
change-feed clients pick up migrated records without per-record events. A
device's contacts-import capability (ADR 0024 §6) targets
`platform.contacts:person`.

### 7. Activities follow-up

Once Person is an engine object, administrators may add it to *Who* and add
Person and Organisation to *Related to*, which also gives them Log a call
and the Activity timeline. Activities' manifest does not add them itself:
a manifest may only name objects of plugins it depends on (ADR 0033 §2),
Activities must not require Contacts, and an upgrade never overwrites an
installed allowlist, so a manifest default would reach only new
installations. Instead, the cutover's completion report offers the
administrator to add Person to *Who* and both objects to *Related to* in
one step. An optional dependency that lets a manifest name another
plugin's objects as starting allowlist values when that plugin is present
is a follow-up (open question 4).

### 8. Delivery slices

Each slice ships alone and can be reverted without touching data written
by earlier slices.

1. **Engine gaps** G1 to G11, each its own change with both stores, the
   API and conformance tests. They serve every object and change nothing
   for Contacts.
2. **Dual definitions.** Contacts' manifest declares the two dormant
   objects and the resource mapping; installing creates them suspended and
   empty in every workspace. Reverting the manifest leaves suspended, empty
   definitions.
3. **Migration tool** with dry run, copy, catch-up, verification and
   rollback, on both stores, and acceptance runs on fixture workspaces. The
   cutover step is present but refused until slice 5.
4. **Compatibility layer.** The v1 REST handlers over the engine and the
   event translator, exercised by running `scripts/test-contacts-api.sh`
   and the extension and lifecycle acceptance scripts against a migrated
   workspace. Not reachable in production until slice 5.
5. **Per-workspace cutover.** The phase flag decides which store serves a
   workspace; cutover and rollback become available to administrators.
   Off by default; rolled out workspace by workspace.
6. **UI.** Generic engine pages for migrated workspaces without page
   customisations, the Activities offer of §7, and the removal of the
   Contacts pages once no workspace needs them (G16).
7. **Retire the typed store** after every workspace has passed its rollback
   window: delete old rows per workspace, then a release drops the tables,
   the store port, the adapters and the old jobs. Removing the v1 routes and
   events follows the §3 timeline.

### 9. Alternatives considered

- **Keep typed storage forever.** No migration risk, but every engine
  service stays unavailable to the most used objects or has to be rebuilt
  for one store on two backends, Activities cannot point at people, and
  two storage models live on indefinitely. Rejected; ADR 0015 §6 already
  made the engine the default.
- **Big-bang migration** of every workspace in one release. Simpler code,
  but one failure blocks every workspace, a rollback is all or nothing, and
  large workspaces force downtime on everyone. Rejected for per-workspace
  phases.
- **A sync bridge** keeping both stores current in both directions. It
  avoids a cutover, but it needs conflict resolution between two version
  counters, duplicates every event, makes "which store is right" a
  permanent question, and keeps the typed store alive. Rejected; the
  one-way copy with a bounded reverse copy gives reversibility without it.
- **Workspace `custom:` fields for extension values.** Simpler for the
  engine, but it hands another plugin's fields to the workspace, loses
  their permissions and their removal lifecycle, and breaks ADR 0012.
  Rejected for G6.
- **Versioning the Contacts API as v2.** It would duplicate the engine's
  record API. Rejected for a deprecated v1 over the engine.

### 10. Risks and consequences

- **Engine work comes first.** Eleven gaps must ship before any data
  moves; most are small, but G4 (the migrator), G5 (field permissions) and
  G6 (cross-plugin fields) touch security-relevant paths and need careful
  review and tests, including forced RLS and evidence checks.
- **The migrator is a powerful caller.** It writes ids, stamps and states
  no member can. It must stay kernel-only, workspace-scoped, audited per
  batch, and unreachable from requests and plugins.
- **Behaviour changes** that members and integrations will notice: an
  owner on every record, deleting an organisation clears its people's link
  as an edit (restored on undelete, G3), stricter phone validation,
  case-insensitive uniqueness, JSON custom fields as text in the engine
  API, disabled custom fields no longer readable, server-enforced
  custom-field read permissions, and invalidated list cursors at cutover.
- **Performance.** Contacts lists used typed columns; engine lists use the
  record table and pivots. `name`, `email` and `tags` are indexed, and a
  substring name search scans one object in one workspace. The migration
  acceptance runs include the 10,000-record measurement of ADR 0024 and
  the plans in `tests/performance` on engine tables.
- **Two paths during deprecation.** Until slice 7 both stores exist and
  the compatibility layer must be tested on both phases; the acceptance
  scripts run against a legacy and a migrated workspace.
- **Gains.** Once migrated, people and organisations get sharing, the
  query language, validation rules, automations, quick actions, related
  lists, the Activity timeline, history with sources, global search,
  reports, bulk jobs and API clients, change data capture and device sync,
  and the platform has one storage model.

## Open questions for the product owner

1. Should Person gain first and last name fields (with the name as a
   formula) as part of this work, or later?
2. Is `created_by` the right owner for migrated records, or should owners
   start empty, or be one chosen member per workspace?
3. Should the custom-field read permissions become server-enforced (the
   default above), or be dropped, so custom fields are visible to every
   reader as the API already allows?
4. Should a manifest be able to name an optional plugin's objects in a
   starting allowlist, so new installations of Activities include Person in
   *Who* when Contacts is present?
5. Are a 30-day rollback window and a deprecation of at least six months
   (and not before the 0.3 contract) acceptable?
