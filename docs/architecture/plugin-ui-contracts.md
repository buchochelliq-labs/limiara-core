# Plugin UI contract authoring

## Tenant draft and publication lifecycle

Tenant customisation is stored as versioned patches; it never overwrites the
plugin-owned page definition. `ui-metadata::publication` owns the aggregate and
requires `ui.metadata.manage` for every draft, preview, validation,
publish, rollback, import, and export operation.

- Aggregate and draft revisions provide optimistic concurrency. A stale write
  fails without changing the draft or current publication.
- Preview resolves the caller's draft through the same catalogue and
  caller-safe filter as runtime delivery, but returns `private, no-store` and
  never reads or writes the published cache.
- Publish validates first, then creates an immutable publication and increments
  `publication_generation`. Invalid drafts leave the last known-good
  publication unchanged.
- Rollback creates a new publication from an older immutable version. It does
  not rewrite history.
- Import supports a non-mutating conflict plan before applying an atomic draft;
  bundles contain patches and source versions, never plugin-owned definitions.
- PostgreSQL stores one aggregate per tenant and page behind forced row-level
  security. Revision compare-and-swap, state, generation, and redacted audit
  rows commit in the same tenant transaction.

The persisted aggregate is intentionally independent of operational CRM data.
Audit metadata records IDs and versions only; patch values are not copied into
the platform audit log.

### Management and inspector API

Authorized administrators use the workspace-scoped customisation resource:

- `GET /api/v1/workspaces/{workspace}/ui/pages/{page}/customisation` returns
  the draft, immutable publication history, audit history, aggregate revision,
  and publication generation. The response ETag is `"ui-revision-{revision}"`.
- `POST` to the same resource accepts an `expected_revision` and one tagged
  operation: `begin-draft`, `update-draft`, `validate`, `publish`, `rollback`,
  `abandon`, `import`, or `export`. Draft updates, publish, and abandon also
  require the expected draft version.
- `GET .../customisation/preview` resolves either `view=draft` (default) or
  `view=published` for the caller's locale, host, mode, form factor, audience,
  resource subtype, permissions, and record context. Draft responses are
  always `private, no-store`.
- `GET .../customisation/compare?from={version}&to={version}` reports stable
  patch IDs added, removed, or changed between immutable publications.

Import requests support `dry_run: true`; the response returns exact conflicts
without saving state. A non-dry-run import with conflicts fails atomically.
Resolver diagnostics are retained only by this administrator-facing inspector;
ordinary runtime delivery continues to return non-enumerating errors.

Every operation requires an active membership bound to the workspace path and
`ui.metadata.manage`. Stale aggregate or draft revisions return HTTP
412, invalid definitions return 422, invalid lifecycle transitions return 409,
and missing permission or mismatched workspace membership fail closed as 404.

## External host compilation boundary

Declaring `portal` or `public-form` on a page is necessary but not sufficient
to publish it externally. Before publication, `ui-metadata::external_host`
compiles the resolved page against a versioned `ExternalHostPolicy`.

The policy explicitly pins:

- every permitted host component and owner/dependency plugin;
- resource fields and their allowed classification, including whether personal
  input is expected;
- data-source IDs to an exact resource and operation;
- action IDs to an exact kind and target;
- reusable fragment IDs, context paths, and portal permissions.

Compilation walks node props, bindings, conditions, parameters, nested nodes,
data sources, actions, and resource descriptors. Public forms reject permission
conditions entirely. Portal permissions must be portal-specific and explicitly
listed. Internal/sensitive fields, relationships, internal-host operations,
retargeted actions/data sources, undeclared fragments, and unapproved plugin
dependencies fail compilation rather than being silently published.

External publications are addressed by 32–96 character base64url-style opaque
slugs. The registry indexes only a SHA-256 digest of the slug and returns the
same unavailable result for unknown, wrong-host, stale-generation, revoked, or
disabled-dependency lookups. Revocation and plugin disablement take effect on
the next lookup. Internal pages and fragments are never automatically added to
an external policy or publication.

This compiler is shared by self-hosted and future managed deployments. Hosting
changes may alter the public origin, rate-limit adapter, and abuse provider, but
must not weaken the compiled policy or opaque lookup semantics.

### Portal identity and anonymous submissions

Portal access is authorized against the opaque publication before resolution.
The grant requires an active non-zero-version session whose tenant and audience
exactly match the publication. The resulting permission set is the intersection
of identity claims and the portal permissions approved at compilation; internal
permissions are never forwarded into portal resolution.

Anonymous form submissions use `PublicSubmissionGateway`. Callers provide only
the opaque slug, not a workspace, plugin, resource, action, or operation. The
publication selects the tenant and the registered policy pins the compiled
action and allowed fields. Implementations must populate `body_bytes` from the
transport and `client_fingerprint` from a deployment-keyed one-way digest; raw
IP addresses or device identifiers must not enter submission state or audit.

The gateway enforces, in order:

- a 1 MiB absolute request ceiling plus stricter per-form body, field-count, and
  per-field limits;
- opaque idempotency keys and correlation IDs with bounded syntax;
- exact HTTPS origin allowlists and constant-time double-submit CSRF matching;
- required/known field validation and safe failure-retention policy;
- publication-scoped idempotency, payload-conflict detection, and rate windows;
- a pluggable abuse decision (`allow`, `deny`, or `challenge`);
- tenant-selected execution of the single compiled action.

Successful replays return the original receipt without executing twice. Unknown,
revoked, stale-generation, disabled-plugin, bad-origin, and bad-CSRF requests use
non-enumerating unavailable responses. Failures may return only fields explicitly
marked `retain_on_failure`; audit contains correlation, tenant/action when safely
resolved, outcome, and field names, never field values or client fingerprints.

The API host exposes the restricted documents at
`GET /api/v1/external/forms/{slug}` and
`GET /api/v1/external/portals/{slug}`, with anonymous actions mediated by
`POST /api/v1/external/forms/{slug}/submissions`. Responses are `no-store` and
add `nosniff` and `no-referrer` protections. Malformed slugs, missing or invalid
portal identity, cross-tenant access, revocation, and unknown publications share
the same non-enumerating response. Portal identity must be installed by trusted
session middleware as a tenant-, audience-, and version-bound principal; public
request fields are never accepted as identity or routing context.

External publication snapshots are durable PostgreSQL records. Only the
SHA-256 digest of the capability slug is stored; the original slug is never
persisted. Tenant management writes run under forced row-level security, while
the public route can perform one exact digest lookup through a fixed-search-path
`SECURITY DEFINER` function. That function has no list or prefix-search form and
is revoked from `PUBLIC`; production provisioning must grant only its `EXECUTE`
permission to the API runtime role. Each request reloads the snapshot, so
revocation, generation changes, and dependency-plugin disablement are effective
without an API restart. The compiled page and its public submission policy are
stored together, preventing a restart from silently changing the validation
boundary.

The portal route accepts a principal only after the trusted session boundary has
verified it. Request headers, cookies, query parameters, and submitted fields do
not directly create that principal. Provider-neutral login, durable session
verification, and membership-derived permissions are delivered by #15, #16,
and #17 respectively; until those dependencies install a verified session, the
portal route fails with the same non-enumerating unavailable response.

Submission transport applies a 1 MiB body limit before buffering. It derives the
client fingerprint from the peer address and the deployment-only
`platform.external_hosts.fingerprint_key`; the key must be at least 32 bytes and
is normally supplied as `PLATFORM__EXTERNAL_HOSTS__FINGERPRINT_KEY`. If either
the trusted peer address or key is unavailable, submission fails closed. The
current domain adapter keeps rate and idempotency state in memory for local and
contract testing. A multi-instance deployment must replace that adapter with an
atomic shared store while preserving the same tenant/publication scoping,
expiry, payload-conflict, and audit semantics.

The web host reserves `/forms/{opaque-slug}` for anonymous forms and
`/portal/{opaque-slug}` for authenticated portal pages. Malformed paths stay in
the standalone external shell and never fall through to staff navigation. The
shell fetches only the caller-safe external document, renders it through the
shared trusted component registry, and submits only the filtered field payload;
tenant, plugin, resource, operation, and action routing remain server-selected.
It provides English and French host chrome, light/dark/high-contrast/device
colour schemes, responsive compact rendering, a skip link, non-enumerating
loading/error states, multi-step progress with focus transfer, safe retry-state
replacement, and an accessible confirmation reference. Browser conformance
tests cover desktop/mobile rendering, 320 px reflow, forced colours, reduced
motion, keyboard step progression, submission, confirmation, theme/language
switching, and equivalent malformed/unknown-link responses. Visual baselines
are stored alongside the external-host Playwright test.

This document describes UI contract v1 from issue #113. Resolution and rendering
are deliberately separate concerns owned by #114–#117.

## File layout

Keep metadata in the owning plugin artifact and reference it from `plugin.toml`:

```toml
[[ui.resources]]
id = "platform.contacts.person"
path = "ui/resources/person.json"

[[ui.pages]]
id = "platform.contacts.person-form"
path = "ui/pages/person-form.json"

[[ui.pages]]
id = "platform.contacts.people-directory"
path = "ui/pages/people-directory.json"

[[ui.fragments]]
id = "platform.contacts.person-summary"
path = "ui/fragments/person-summary.json"
```

Paths are relative forward-slash JSON paths inside the plugin artifact. Absolute
paths, traversal, backslashes, duplicate IDs, and non-JSON files are rejected.
Existing navigation-only manifests remain valid.

Each JSON document has `contract_version`, stable `id`, positive
`definition_version`, and `owner_plugin`. The authoritative Rust types are in
`plugin-contract::ui`; the web type mirror is in
`apps/web/src/plugin-ui/contracts.ts`; the machine-readable schema is
`docs/architecture/ui-metadata.schema.json`.

## Minimal resource and page

```json
{
  "contract_version": 1,
  "id": "platform.contacts.person",
  "definition_version": 1,
  "owner_plugin": "platform.contacts",
  "label": "Person",
  "fields": [
    {
      "api_name": "first_name",
      "label": "First name",
      "field_type": "text",
      "source": "standard",
      "readable": true,
      "writable": true
    }
  ],
  "operations": [
    {"id": "read", "kind": "read", "permission": "contacts.person.read"}
  ]
}
```

```json
{
  "contract_version": 1,
  "id": "platform.contacts.person-form",
  "definition_version": 1,
  "owner_plugin": "platform.contacts",
  "kind": "record-form",
  "hosts": ["internal-web"],
  "resource": "platform.contacts.person",
  "root": {
    "id": "person-form",
    "component": "platform.ui.form",
    "children": [
      {
        "id": "first-name-field",
        "component": "platform.ui.field",
        "props": {"field": "first_name"}
      }
    ]
  }
}
```

Resources describe domain-facing fields and registered operations, not tables,
queries, driver errors, or database transactions. A UI `required` condition never
overrides the plugin API's authoritative validation or authorization.

## Multi-plugin composition

A dependent customer plugin contributes a new node without editing the Contacts
page:

```toml
[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[ui.extensions]]
id = "customer.activity.person-card"
path = "ui/extensions/person-card.json"
```

```json
{
  "contract_version": 1,
  "id": "customer.activity.person-card",
  "owner_plugin": "customer.activity",
  "target_page": "platform.contacts.person-form",
  "target_version": 1,
  "target_node": "person-sidebar",
  "operation": "inside",
  "node": {
    "id": "activity-card",
    "component": "platform.ui.card",
    "props": {"title": "Activity"}
  }
}
```

Contributions target stable node IDs and exact definition versions. Missing pages,
versions, nodes, required payloads, host escalation, and duplicate node IDs fail
validation. Resolver ordering and conflict behavior are implemented in #114.

### Contacts person-detail extension points

`platform.contacts.person-detail` version 1 exposes four stable, typed targets:

| Node ID | Kind | Intended content |
| --- | --- | --- |
| `person-actions-slot` | `action` | Authorised actions declared by the page |
| `person-card-slot` | `card` | Compact record context owned by another plugin |
| `person-related-list-slot` | `related-list` | Permission-safe related-record collections |
| `person-timeline-slot` | `timeline` | Activity or history presentations |

Empty slots render no landmark or visual chrome. Contributions receive only the
caller-safe resolved page and data-source values; disabled, denied, missing,
failed, or cross-workspace responses contain no person resource or record data.
Extension nodes cannot restore actions or fields removed by permission filtering.
The web composition boundary filters the contributing plugin's enabled state,
the requested host, and the contribution root permission before inserting a
node. Active contribution IDs participate in the resolved content hash, so
disablement cannot reuse a cached page that still contains old extension nodes.
Applicable nodes are ordered by priority, owner plugin ID, then contribution ID;
missing targets and duplicate IDs fail with bounded diagnostics.

The Contacts organisation resource declares its `people` relationship to
`platform.contacts.person`. The organisation detail page resolves that relationship
as a separate caller-safe data source: removing person-read permission removes the
relationship section, its navigation action, its resource descriptor, and all
related values together. Organisation directory query state is limited to `q`,
`status`, `sort`, opaque `cursor`, and `selected`; the resolver normalises custom
field filters when the field is not configured for the caller.

## Standard and custom fields

Standard typed fields and tenant fields share `FieldDescriptor`. Tenant fields use
`source: "tenant-custom"` and retain the explicit `__c` suffix. This is only a
presentation-normalization contract: the owning plugin still validates definitions
and values and decides how they are stored and indexed.

## Standard record fields

Every record resource (one with a `read` or `list` operation) has the
kernel's standard fields: `id`, `name`, `created_at`, `created_by`,
`updated_at`, `updated_by`, `touched_at`, `version` and `archived_at`
([ADR 0014](../adr/0014-standard-record-fields.md)). Their definition is
published in [`standard-record-fields.json`](standard-record-fields.json).
Resource documents do not declare them; delivery adds them to every record
resource with `source: "system"`, and pages may bind them like any other
field. A record resource names its display-name field with `name_field`,
which must be a required standard text field; a resource whose name field is
itself called `name` declares it, and any other declaration of a standard
field name is rejected.

## Conditions and bindings

Bindings may reference a field, runtime context, literal value, or registered data
source. The v1 expression AST permits `and`, `or`, `not`, equality/inequality,
membership, existence, and permission predicates. It cannot contain JavaScript,
SQL, URLs, or another executable language.

## Golden fixture

`crates/plugin-contract/fixtures/ui/bundle.json` is parsed, serialized, and
semantically validated by the Rust test suite. It covers typed and `__c` fields,
relationships, operations, data binding, conditions, a variant, reusable fragment,
component definition, accessibility contract, and a customer extension.
