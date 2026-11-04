# Metadata catalogue and page resolution

Issue #114 promotes the executable #112 spike into the production
`ui-metadata` crate. The crate consumes the versioned contracts from
`plugin-contract::ui`; it does not contain CRM resource names, persistence
types, HTTP behavior, or React rendering.

## Catalogue lifecycle

`MetadataCatalogue` accepts validated `PluginUiPackage` values. Every document
must be owned by the package's plugin ID, and the combined catalogue must pass
the shared UI contract validator. Node components must resolve either to an
explicitly registered host component or to a component definition in the
combined bundle. A package remains catalogued when disabled,
but only enabled packages can own pages or contribute to a resolved snapshot.
An enabled package with a missing or disabled dependency fails resolution with
a structured diagnostic.

Package replacement and persistence are intentionally delegated to the plugin
lifecycle work in #18 and #19. Callers should construct a candidate catalogue,
validate it, and swap it atomically only after plugin lifecycle checks pass.

## Deterministic layer order

The resolver starts from the owning page and selects the highest-priority
matching variant using only the bounded condition AST. It then applies layers
in this order:

1. enabled plugin contributions in dependency/topological order;
2. published tenant patches compatible with the page definition version;
3. non-structural user preferences from a closed property enum.

Within a layer, priority and stable contribution ID break ordering ties. Input,
filesystem, and database iteration order do not affect the result. Two writes
to the same node property in the same layer are an error; a later layer may
intentionally override an earlier layer.

All six stable-ID operations are supported: `inside`, `before`, `after`,
`move`, `configure`, and restricted `replace`. Replacement must retain the
target node ID, and moving a node into its own subtree is rejected.

## Publication and upgrade safety

Tenant patches declare an inclusive/exclusive compatible definition-version
range. Resolution works on clones and returns an immutable value, so a missing
anchor, incompatible version, ambiguous write, or dependency failure cannot
mutate the catalogue or the last published result. The caller can therefore
retry with the prior publication as a rollback input.

`ResolvedPage` records the selected variant, all source/dependency plugin
versions, tenant publication versions, and a SHA-256 content hash over its
canonical deterministic representation. Later API caching in #116 can use
these values without inventing a second composition model.
