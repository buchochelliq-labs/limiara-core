# Metadata-driven UI model

ADR 0005 defines the decision. This document records the spike's proposed public
shape and the hand-off boundary to issues #113–#124.

## Contract families

- `ResourceDescriptor`: plugin-owned fields, relationships, operations,
  permissions, classification, validation/error mapping, and concurrency.
- `PageDefinition`: routeable page, stable node tree, host profiles, modes,
  variants, data sources, actions, and reusable fragments.
- `ComponentDefinition`: property schema, supported children/slots, hosts,
  capabilities, events, and accessibility behavior.
- `ExtensionContribution`: version-constrained patch targeting a stable page,
  node, or slot ID.
- `ResolvedPage`: immutable caller-safe result containing only authorized
  resources and operations plus source versions and a content hash.

The original executable proof remains in `crates/ui-metadata-spike` for the #112
decision record. Production documents and semantic validation live in
`plugin-contract::ui`; the production catalogue and deterministic layer resolver
live in `crates/ui-metadata`. Authoring guidance and the contract golden bundle
are described in `plugin-ui-contracts.md`, while `metadata-resolution.md`
documents runtime composition and upgrade behavior.

## Example bundle

`docs/architecture/examples/metadata-ui/prototype-bundle.json` contains:

1. a Contacts person resource with typed and tenant `__c` fields;
2. a create/edit form with sections, validation metadata, and conditions;
3. a record page with a contributed customer card;
4. a collection page with search, sorting, and table/card presentation;
5. a public contact form whose components and operation are explicitly public.

The original web proof in `apps/web/src/metadata-spike` renders the same node
grammar with host-owned components. Production Contacts now consumes the same
runtime: the person form is owned by `platform.contacts.person-form`, the person
record workspace by `platform.contacts.person-detail`, the People directory by
`platform.contacts.people-directory`, and organisation discovery/detail by
`platform.contacts.organisation-directory` and
`platform.contacts.organisation-detail`. The React
entrypoints retain only query, permission, and action orchestration; page layout,
states, collections, and record previews come from plugin metadata.

## Resolution algorithm

1. Validate contract version, stable IDs, component properties, references, host
   compatibility, and target definition version.
2. Order enabled contributions by plugin dependency order, explicit priority, and
   stable plugin ID.
3. Apply stable-ID patches. Reject ambiguous property writes, duplicate IDs,
   missing targets, and incompatible versions.
4. Apply the published tenant patch using the same rules.
5. Remove unauthorized resources, nodes, bindings, and actions server-side.
6. Validate the complete tree against its target host profile.
7. Return an immutable result with source versions/hash suitable for an ETag.

## Lifecycle

Plugin definitions are immutable artifacts. Tenant edits move through draft,
validated, published, superseded, and rolled-back states. Published pages remain
available while a new draft or incompatible upgrade is being resolved. Preview
uses an isolated draft cache key and never changes the ordinary runtime response.

## Security boundary

Declarative metadata can call only registered resource operations and actions.
The server remains authoritative for permissions and validation. Public hosts use
an explicit allowlist and opaque publication IDs. Custom code runs only through
the isolated component protocol described by ADR 0005 and issue #122.

## Spike exit and implementation order

The #112 spike and #113 contracts now feed the production #114 resolver, #115
host renderer, #116 permission-filtered delivery boundary, and #117 safe form
interaction runtime. Contacts proves forms in #118 and collection/record
primitives plus the People directory in #130–#131. Person detail, organisation
views, and migration assurance continue in #132–#134. Tenant lifecycle,
external hosts, the visual editor, and assurance continue in #120–#124.
