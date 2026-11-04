# UI metadata conformance validation

Run the metadata UI release gate from the repository root:

```text
node scripts/validate-ui-metadata.mjs
```

The command validates the representative plugin manifest and every referenced
resource/page document, runs the Rust contract, resolver, delivery, and validator
suites, then runs the TypeScript contract, renderer, form/action, and host
conformance suites. CI runs the same command with the pinned Rust, Node, and pnpm
versions.

The representative plugin lives in `fixtures/plugins/ui-conformance`. It is
deliberately non-production and covers a standard field, tenant custom field,
default, safe condition, eager data source, permission-bound operation, form,
and optimistic concurrency declaration.

## Validating authored plugins

The validator accepts one or more manifests so extension plugins can be checked
alongside the plugins that own their target pages:

```text
cargo run --locked -p ui-metadata-validator -- path/to/plugin.toml path/to/dependency/plugin.toml
```

Files are resolved relative to their declaring manifest. The validator checks
manifest rules, reads only declared JSON files, verifies reference IDs match the
loaded documents, combines the supplied bundles, and runs semantic contract
validation. Errors name the manifest or document path, reference kind and ID,
and the violated contract rule; they do not include record or tenant values.

## Current gate boundary

This incremental #124 gate covers contracts, deterministic resolution,
permission-filtered delivery, safe form interactions, and every maintained host
component across its supported hosts at wide and compact form factors. Public
publication, sandbox escape, automated browser accessibility/contrast, visual
diffs, upgrade matrices, and performance budgets join this command as #121,
#122, #103, #106, and the remaining #124 work land.
