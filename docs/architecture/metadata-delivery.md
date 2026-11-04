# Permission-filtered metadata delivery

Issue #116 adds the `ui-metadata-delivery` crate and two authenticated runtime
endpoints:

- `GET /api/v1/workspaces/{workspace}/ui/pages/{page}`
- `GET /api/v1/workspaces/{workspace}/ui/resources/{resource}`

The delivery service is injected with a `UiSnapshotSource` and requires a
membership-derived `UiCaller`. The API's source is `UiSnapshotProvider`
(`apps/api/src/ui_snapshots.rs`), built from durable state on every request:

- **Linked UI, not loose files.** A native plugin's artifact digest covers its
  linked code, so its UI documents are compiled in (`NativeUiPlugin`). At
  startup `plugin_runtime_native::verify_ui_plugin` requires every declared
  document to be linked exactly once, nothing undeclared, the declared ids and
  owner, and a valid bundle. The bundle must also install on a catalogue with
  the host component registry. A failure stops the API from starting.
- **Per-workspace catalogue.** Only plugins active in the workspace, per the
  activation projection, contribute. The linked version and declared document
  set must also match the active, digest-verified manifest exactly. A skewed
  plugin's pages stay unavailable rather than being served from mismatched
  metadata. Catalogues are cached per distinct set of active packages.
- **Published customisations.** The workspace's published page patches and
  publication generations come from `ui_metadata_page_state`.
- **Priming.** Delivery is synchronous, so each page, resource, preview, and
  publication request first builds the snapshot asynchronously. A store
  failure answers 503 (`temporarily-unavailable`), never stale or partial
  metadata.

`HOST_COMPONENTS` mirrors the web host registry
(`apps/web/src/plugin-ui/renderer/hostComponents.tsx`), and a unit test fails
when they drift. It does not fall back to the demo user or an in-memory plugin
set.

## Security filtering

Resolution happens first against the workspace's enabled catalogue and published
tenant patches. Before serialization, the service:

1. removes resources without an allowed operation for the current host;
2. removes fields, relationships, operations, data sources, and actions whose
   permissions are absent;
3. removes nodes bound to forbidden fields, data sources, or actions;
4. evaluates and removes permission predicates, applying their resulting
   visible, enabled, read-only, or required state without returning permission
   names; and
5. strips all remaining permission annotations.

Caller-safe resource DTOs deliberately have no permission property. Missing,
disabled, non-member, and denied pages all return the same 404 code and detail.
Incompatible, conflicted, and temporarily unavailable outcomes use stable,
redacted categories without workspace, plugin, page, field, or action names.

## Cache and conditional requests

The private cache key includes:

- workspace and target;
- a SHA-256 fingerprint of user, membership version, roles, and permissions;
- locale, host, mode, form factor, audience, subtype, and record context; and
- catalogue and publication generations.

Consequently, users and permission sets cannot reuse one another's entries.
ETags include the filtered response, the security fingerprint, and both
generations. A 304 is returned only when `If-None-Match` equals the current
caller's current authorized ETag. Responses use
`private, max-age=0, must-revalidate`.

Generation changes naturally miss stale entries. Lifecycle and publication
adapters should also call `invalidate_workspace` after a committed state change
to bound memory and remove superseded entries immediately.

## Operations and observability

Readiness exposes only a boolean provider state and cache entry count. Delivery
observations contain kind, outcome category, cache result, definition version,
and elapsed time; their type has no tenant, caller, permission, record, field, or
page value. HTTP request tracing may correlate the observation through the
existing request ID without logging metadata values.

The TypeScript mirror in `plugin-ui/contracts.ts` checks the Rust response shape.
Integration tests cover permission separation, ETag isolation, 304 behavior,
locale, form factor, role, host, membership denial, lifecycle disablement,
publication/catalogue generations, resource filtering, readiness, and stable
failure categories.
