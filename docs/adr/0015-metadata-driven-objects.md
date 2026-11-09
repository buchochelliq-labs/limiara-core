# 0015: Metadata-driven objects over generic record storage

## Status

Accepted (2026-10-11). Supersedes the rejection of a universal object engine
recorded in ADR 0005 and ADR 0012 and listed as a non-goal in the roadmap.
Epic #292; implementation issues #293 (engine), #294 (naming), #295
(relationships), #296 (picklists).

## Context

Salesforce's defining capability is that an administrator creates objects
and fields at runtime, without a deployment, and every platform service
(security, sharing, validation, automation, query, search, reports, APIs,
change events, the UI) works on them immediately. It achieves this with a
*universal data dictionary*: metadata tables describe objects and fields,
records of every object and tenant live in a few shared generic tables, and
typed pivot tables provide indexes and uniqueness. No tenant ever causes DDL.

Limiara chose the opposite: each plugin owns typed tables, and tenants can
only add custom fields that the owning plugin chose to support (Contacts).
That keeps standard data well-typed, but it means:

- administrators cannot create objects;
- every platform service (sharing, automation, query, search, reports, CDC)
  would have to be re-implemented per plugin store;
- every plugin needs an adapter per storage backend (ADR 0013).

The product owner has asked for Salesforce parity on all of these. The
remaining open question was whether to keep typed storage as the only model.

## Decision

### 1. A kernel object engine with metadata tables

The kernel gains a domain-neutral object engine (`platform-objects`):

- `object_definitions`: API name, labels, origin, owning plugin, public-ID
  key prefix, sharing model, flags (searchable, history tracked,
  navigation), lifecycle state and version.
- `field_definitions`: API name, label, type, required, unique, indexed,
  searchable, default, length/precision/scale, picklist values, reference
  target and delete behaviour, position, lifecycle state and version.
- `object_permissions`: create, read, edit, delete, view all and modify all
  per object per role (Salesforce object permissions on profiles and
  permission sets).

The engine is platform machinery; it holds no business concepts. Objects are
defined by workspace administrators and, since ADR 0033 §2, declared by
plugins in their manifests (#318).

### 2. Generic record storage with typed pivots

- `records`: one row per record of every engine object, with the standard
  fields of ADR 0014 as real columns (`public_id`, `name`, `owner_id`,
  `record_type_id`, `created_*`, `updated_*`, `touched_at`, `version`,
  `deleted_at`) and a `data` document holding custom field values keyed by
  field API name.
- Values are validated and normalised in Rust against field metadata before
  any write, so the document is never free-form. Types: text, long text,
  email, phone, URL, number, currency, percent, boolean, date, date-time,
  picklist, multi-select picklist and lookup (formula, auto-number and
  master-detail follow in #298 and #295).
- `record_index`: typed pivot rows (text, number, time) for fields marked
  indexed, giving tenant-first B-tree filtering and sorting without per-tenant
  DDL (Salesforce's `MT_Indexes`).
- `record_unique`: one row per value of a unique field, with a primary key
  that enforces uniqueness transactionally. Values compare without case. A
  record in the recycle bin releases its unique values, as in Salesforce, so
  undeleting fails with `records.duplicate_value` if another record took
  one meanwhile.
- `record_references`: one row per lookup value, for related lists, delete
  behaviour and query traversal (Salesforce's relationship pivots).

All tables carry `tenant_id`, tenant-first keys and forced row-level security
on PostgreSQL. On SQLite they live in the workspace's own file (ADR 0013).

### 3. Distinguishing custom objects and fields

Salesforce appends `__c` and prefixes managed items with `ns__`. Limiara uses
one rule instead:

- **API names are `namespace:name`.** A colon separates the owner from the
  name; a dot is reserved for traversing relationships in the query language,
  so `SELECT name, custom:status, owner.name FROM custom:project` is
  unambiguous.
- **Namespaces** are `custom` for anything a workspace administrator defines,
  and a plugin's id for anything a plugin defines. Kernel standard fields
  (ADR 0014) have no namespace. `custom` can never be a plugin id.
- **Origin is metadata.** Every object and field carries `origin`
  (`system`, `plugin`, `workspace`), so tools never parse names.
- **Key prefixes say who owns the object.** Plugin objects register
  letter-first prefixes (`per`, `org`); workspace objects get digit-first
  prefixes (`0a1`), allocated per workspace, so a public ID shows at a glance
  that it belongs to a custom object and the two ranges can never collide.

Existing `__c` custom fields in Contacts and dotted extension field ids
(ADR 0012) keep working and move to the new form under #294.

### 4. Permissions

- System permissions: `objects.manage` (define objects, fields and object
  permissions; Salesforce "Customize Application"), `records.view_all` and
  `records.modify_all` (Salesforce "View All Data" and "Modify All Data").
- Object permissions are rows, not permission strings, because objects are
  created at runtime.
- Record visibility within an object comes from sharing (#286, ADR 0016).
- Setting a lookup needs read access to the target object; an unreadable
  target is reported exactly like a missing one. A lookup that a change
  leaves untouched is not re-checked, so an editor who cannot read its
  target can still change the record's other fields.
- Field-level security follows in #316.

### 5. Events and audit

Every record and metadata change writes its audit entry and a versioned
outbox event in the same transaction: `platform.objects.record-created.v1`,
`-updated.v1`, `-deleted.v1`, `-undeleted.v1` (payload: object API name,
record public ID, version, changed field names) and
`platform.objects.object-*.v1` / `field-*.v1` for metadata. The engine
writes in its own execution context, `platform.objects`, so the existing
restrictive outbox policy (ADR 0010) applies to it like any plugin. These
events are the basis for change data capture (#306).

### 6. What happens to typed plugin storage

Typed plugin stores remain allowed for plugins with unusual access patterns,
but the default for new business capabilities is to declare objects as
metadata in the engine (#318). Contacts keeps its typed store until it is
migrated, and gains sharing and query through the same services once it is.

### 7. Storage backends (amends ADR 0013)

The generic record store is now the main storage port. It replaces most
per-plugin adapters with one adapter per backend, which makes PostgreSQL and
SQLite parity far cheaper. DuckDB reads the same tables for analytics and
reports. Object storage holds files and big objects (#329, #330).

## Alternatives considered

- **Per-tenant DDL** (create a real table per custom object). Fastest
  queries, but schema changes per tenant at runtime, migrations for every
  workspace, and catalogue bloat; Salesforce rejected it for the same reasons.
- **JSON document only, no pivots.** Simple, but range filters, sorting and
  uniqueness either scan or need per-field indexes (DDL again).
- **Untyped string columns** like Salesforce's flex columns. Typed JSON with
  validation in Rust gives the same flexibility with real types.
- **Keep `__c`.** Familiar, but it collides with Salesforce-trained
  expectations that `__c` means Salesforce, gives no place for the owner, and
  the dotted extension ids already conflict with relationship traversal.

## Consequences

- Administrators can create objects and fields at runtime, and every later
  service (sharing, validation, automation, query, search, reports, CDC,
  bulk) is built once on the engine.
- Filtering on non-indexed fields scans one object's records in one
  workspace; administrators mark fields indexed for large objects.
- Two storage models coexist until Contacts and other plugins move onto the
  engine.
- `scripts/test-objects-acceptance.sh` proves the engine over HTTP against
  the production API binary: definitions, fields, records, validation,
  uniqueness, lookups, lists, the recycle bin, object permissions, forced RLS
  and evidence without record values.
