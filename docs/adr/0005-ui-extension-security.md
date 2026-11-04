# 0005: Metadata-driven UI composition and extension security

## Status

Proposed by issue #112. Accept after the executable spike and security review pass.

## Context

Limiara's current plugin UI contract contributes navigation while feature pages are
hard-coded React. That cannot support tenant layouts, metadata-rendered custom
fields, customer plugin cards, portals, or public forms without every plugin
owning a competing frontend framework.

The platform already rejects a universal object/EAV engine. Standard domain data
must remain typed, indexed, and owned by its plugin. A metadata UI therefore has
to describe presentation and interaction without becoming the source of truth for
persistence, validation, permission, or business behavior.

Competitor research in `docs/architecture/metadata-ui-competitor-research.md`
shows a consistent split between resource metadata, composed pages, controlled
extension locations, and an isolated escape hatch for custom code.

## Decision

### One presentation contract, several hosts

Use one versioned composition model for internal web, a future desktop WebView,
authenticated portals, and public forms. A host profile restricts the components,
data sources, actions, and fields that a definition may reference. Publishing an
internal definition never publishes it to another host automatically.

The principal contracts are:

- resource descriptors for fields, relationships, operations, permissions,
  classifications, validation/error mapping, and concurrency;
- page definitions containing stable nodes, component properties, bindings,
  actions, variants, and explicit slots;
- reusable fragments composed from the same nodes;
- extension contributions targeting stable page/node/slot IDs;
- component definitions declaring property schemas, child slots, supported hosts,
  capabilities, events, and accessibility behavior.

Standard typed fields and tenant `__c` fields normalize into the same resource
descriptor for rendering. Their owning plugin remains responsible for storage,
authoritative validation, authorization, audit, and transactional behavior.

### Bounded grammar with a safe escape hatch

The trusted host renders maintained layout, content, field, collection, action,
feedback, and navigation primitives. Customer plugins compose those primitives by
default. Metadata contains neither arbitrary JavaScript nor arbitrary URLs, SQL,
CSS, or HTML.

Cases the palette cannot express run in a separately originated sandboxed iframe
or equivalent isolated WebView. The sandbox receives short-lived, audience- and
capability-scoped tokens and communicates through a versioned, validated message
protocol. It receives no host DOM, cookies, application state, database handles,
ambient credentials, or undeclared network access. Failure is contained to the
component. Arbitrary customer React bundles never execute in the authenticated
host origin.

### Layered composition

Resolve definitions in this order:

1. platform shell and primitive registry;
2. page-owning OOB plugin;
3. enabled plugin contributions in dependency order and explicit priority;
4. the tenant's published patch layer;
5. non-structural user preferences.

Contributions use stable IDs and `inside`, `before`, `after`, `move`, `configure`,
or restricted `replace` operations. CSS selectors and XPath are not contract
anchors. Multiple writes to the same property without an explicit compatible
relationship are a validation conflict, not last-writer-wins behavior.

Tenant customisation is a patch over an immutable delivered definition. A
compatible upgrade re-resolves the patch. A missing or incompatible anchor keeps
the last known-good publication active and reports an actionable conflict.

### Conditions and authority

Visibility, enabled/read-only/required state, defaults, and variant selection use
a small typed JSON expression AST with boolean logic, comparison, membership,
existence, field/context references, and permission predicates. No dynamic code is
evaluated. The server validates the expression and is authoritative for any
security-relevant outcome; the client evaluates only the approved interaction
subset.

The server removes forbidden resources, fields, relationships, actions, and data
sources before returning resolved metadata. Client-side hiding is never a security
boundary. Server domain validation remains authoritative even when a form marks a
field required, hidden, or read-only.

### Lifecycle and delivery

Plugin definitions are versioned files referenced by `plugin.toml` and ingested
during plugin validation. Tenant definitions have draft, validated, published,
superseded, and rolled-back states with optimistic concurrency and audit history.
Import/export operates on metadata bundles and supports validation/dry run.

The runtime API returns an immutable permission-filtered `ResolvedPage` plus its
definition/dependency versions, content hash, and ETag. Cache keys include every
security and rendering input: workspace, permission/audience fingerprint, plugin
state, publication, locale, host, mode, and form factor.

## Consequences

- Plugins can own coherent product experiences without owning the application
  shell or bypassing the design and accessibility system.
- Tenant changes can survive compatible plugin upgrades and fail safely on
  incompatible ones.
- Public forms and portals reuse contracts but require separate security profiles.
- The renderer and resolver become platform infrastructure with compatibility,
  performance, security, and accessibility release gates.
- The visual editor must edit the same patch format; it is not allowed to create a
  second private representation.

## Alternatives considered

- **Hard-coded React per plugin:** simple initially, but cannot support safe tenant
  composition, consistent accessibility, or predictable upgrades.
- **Arbitrary plugin React in the host:** flexible, but grants excessive authority
  and makes isolation, themes, accessibility, and compatibility ungovernable.
- **Declarative components only:** safest, but lacks an escape hatch for genuinely
  bespoke integrations.
- **Universal object/EAV engine:** conflates presentation with persistence and
  weakens typed plugin ownership, indexes, and domain invariants. *Superseded
  for persistence by [ADR 0015](0015-metadata-driven-objects.md), which adopts
  metadata-driven objects over typed, validated generic storage; this ADR's
  point that presentation metadata must not become the source of truth for
  persistence still holds.*
- **Clone full pages for tenant changes:** easy to explain, but prevents customers
  inheriting safe OOB improvements and makes upgrade conflicts opaque.

## Revisit triggers

Revisit the grammar or sandbox only when real plugins cannot express a scenario,
not speculatively. Revisit the public-host boundary after its threat model, and
revisit component trust if a signed marketplace plus browser isolation provides
evidence for a safer additional execution mode.
