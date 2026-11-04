# Limiara Roadmap

This roadmap is the active delivery guide for Limiara maintainers and contributors. It is gate-based rather than date-based: work advances only when the stated behaviour is implemented and the exit criteria are demonstrably met.

## How to read this roadmap

- **Delivered** means the stated repository artifacts exist. It does not imply that every scaffold or contract is wired into a production-ready path.
- **Current** is the gate being completed now.
- **Next** is the next gated outcome, not work to start independently before the Current gate is stable.
- **Later** describes ordered product direction. Later milestones are intentionally less prescriptive until they are promoted.

Design documents, interfaces, SQL test plans, and proof-of-concept routes are identified as such. They do not count as shipped end-to-end behaviour.

Actionable work is tracked in the [roadmap implementation issue index](https://github.com/buchochelliq-labs/limiara-core/issues/90). Oversized roadmap bullets are split there into bounded issues with dependencies and acceptance criteria.

## Roadmap principles

- **Kernel remains domain-neutral.** CRM, field service, workflow, analytics, and AI functionality are plugins rather than privileged kernel modules.
- **Modular monolith first.** New domains begin inside the deployable `api` and `worker` boundary unless measurements justify extraction.
- **Contracts before marketplace scale.** Plugin manifests, events, UI contributions, migrations, entitlements, and service-plugin protocols remain explicitly versioned.
- **Community edition stays self-hostable.** Hosted control-plane features must not become a runtime dependency for the open/community data plane.
- **Security gates drive sequencing.** Tenant isolation, capability denial, auditability, and supply-chain controls are release prerequisites rather than polish.
- **Evidence determines status.** A capability is not complete until its normal path and important failure modes are executable and tested.

## Delivered — Foundation scaffold

**Outcome:** establish the repository structure, architectural boundaries, and contracts needed to build the first secure vertical slice.

### What exists

- A Rust/TypeScript monorepo with `api`, `worker`, `plugin-runner`, and React web application scaffolds.
- Domain-neutral crates for configuration, observability, database access, identifiers, permissions, audit, events, outbox records, entitlements, and plugin contracts.
- Core and Contacts SQL migrations, including tenant-scoped tables, forced RLS policies for Contacts data, indexes, and plugin migration metadata.
- Docker Compose provisions separate migration and application database roles,
  applies lexically ordered core and enabled-plugin migrations through a
  checksum ledger, and prevents the API and worker from starting when
  migrations fail.
- The complete local Compose stack uses bounded health checks, dependency-safe
  startup, supervised runtime processes, and named PostgreSQL, JetStream, and
  Keycloak volumes. Automated clean-start, failed-dependency, and restart tests
  exercise the operator path.
- A pinned local-only Keycloak fixture imports a public authorization-code and
  PKCE client plus member and non-member test identities, with automated
  readiness, discovery, authorization-entry, and credential checks.
- API, worker, and plugin runner consume one validated YAML/override/nested-env
  configuration contract and expose responsibility-aware liveness/readiness.
- Provider-neutral OIDC authorization-code + PKCE login, durable hashed-token
  sessions, workspace membership resolution, RBAC route boundaries, and stable
  non-disclosing denial responses are wired through the API.
- Plugin manifest parsing, compatibility checks, dependency ordering, lifecycle states, native-plugin interfaces, a Wasm capability-denial proof of concept, and an external service-plugin protobuf contract.
- PostgreSQL-backed Contacts repositories, custom-field definitions, CRUD,
  filtering, pagination, and concurrency behaviour, plus responsive
  metadata-driven UI. The people workflows run against the Contacts API;
  organisation workflows remain fixture-driven Gate 2 work.
- ADRs, threat modelling, plugin guidance, operations notes, and initial security, performance, integration, and usability test artifacts.
- Required supply-chain metadata validation for plugins declared as core out-of-box distributions.

### Current boundaries after Gate 1 and early Gate 2

- Contacts persistence, core API behaviour, route/service permission
  enforcement, and atomic domain/audit/outbox writes are delivered for issues
  #23-#29. At-least-once JetStream publication is delivered for issue #30, and
  the forced-RLS transactional inbox pattern is delivered for issue #31.
  Activation governs API/UI (#188) and worker consumers and jobs (#189).
  Contacts contributes the first real ones: an activity-timeline consumer
  (exposed at `GET .../activity`) and an expired-cursor cleanup job.
  `scripts/test-plugin-lifecycle-acceptance.sh` proves the enable → disable →
  re-enable journey across API, OData, UI navigation and deep links, the
  consumer, and the job on the production binaries (#190). Page and resource
  metadata are delivered from a durable per-workspace snapshot: compiled-in,
  manifest-verified plugin UI plus published customisations
  ([metadata delivery](../architecture/metadata-delivery.md)). The web image serves the shell same-origin with
  the API and supports deep links (`infra/web/default.conf.template`, #263).
  The acceptance harness runs the binaries without containers and supplies an
  equivalent proxy. A transaction-local plugin
  execution context, enforced by restrictive RLS, separates plugins inside the
  shared runtime role ([ADR 0010](../adr/0010-plugin-execution-context.md)).
- The worker claims bounded outbox batches, publishes stored envelopes to
  tenant-scoped versioned NATS subjects, retries broker failures, and exposes
  aggregate delivery metrics. The Wasm runner does not yet execute a Wasmtime
  component; external service plugins remain protocol-only.
- The React application has a responsive, metadata-driven shell. The people
  directory, person record, and person create/edit/archive workflows read and
  write the Contacts API (issue #34, see the
  [people workflow notes](../product/application-shell.md#contacts-people-workflows));
  organisation and custom-field administration workflows remain issues #33
  and #35. The platform UX direction ([ADR 0011](../adr/0011-platform-ux-direction.md))
  is being delivered in slices: shell vocabulary, rail and feature launcher
  first (delivered); then object home (table list view, sortable headers,
  filter chips and open actions delivered; bulk actions, hover cards and
  saved list views remain), record page (highlights, stage path, tabs and
  activity timeline delivered; inline edit remains) and record form host
  components;
  then the workspace layout builder with customisation overlays; then
  personal preferences. Follow-ups: the API searches people by full name (plus exact
  email) only, not by organisation or role; it cannot yet filter people by
  a custom field value; and listing custom-field definitions requires
  `contacts.settings.manage`, so callers holding only
  `contacts.people.custom-fields.read` see no custom fields in the web
  workflows until a read-only definitions listing exists.
- Reproducible lockfiles and Rust/web validation in CI are delivered. Contacts
  application-role RLS and tenant-isolation acceptance, integration,
  performance, and complete Playwright coverage remain issues #36-#39 and #103.

## Delivered — Gate 1: Reproducible platform baseline

**Outcome:** a fresh checkout boots into a durable, authenticated platform kernel that is safe to use for the Contacts implementation.

### Delivery work

- Commit Rust and pnpm lockfiles, pin frontend dependencies, and make formatting, linting, type checking, unit tests, and builds deterministic in CI. **Delivered.**
- Persist plugin discovery, validation, migration, enablement, disablement, and failure state; keep lifecycle changes auditable and safe across service restarts. **Delivered.**
- Reserve the platform plugin namespace and verify required provenance metadata plus the digest of the artifact actually loaded for core plugins. Signature transparency, automated SBOM policy, and marketplace-grade verification remain later work. **Delivered.**
- Add executable integration fixtures and operator documentation for first boot, migrations, identity setup, failure recovery, and health diagnosis. **Delivered.**

### Exit criteria

- From a fresh checkout, the documented Compose command builds the stack, applies migrations with the migration role, reaches healthy readiness, and supports login through the local OIDC fixture.
- Runtime queries use a non-owner application role that cannot bypass RLS; migration ownership and application access are verified automatically.
- Unauthenticated, non-member, and permission-denied requests fail consistently without leaking workspace or plugin state.
- Plugin lifecycle state and audit records survive API/worker restarts, and failed migrations or validation leave the previous usable state intact.
- A core plugin with a non-reserved identity, missing provenance, or mismatched loaded-artifact digest is rejected before enablement and produces an audit record.
- Rust and web validation run from committed lockfiles and pass in CI without documented skips.

## Current — Gate 2: Secure, durable Contacts vertical slice

**Outcome:** prove the platform contracts end to end with a usable Contacts plugin backed by PostgreSQL.

### Product and platform work

- Implement database-backed people, organisations, and tenant-defined custom fields through the Contacts plugin service rather than API-local storage. **Delivered for issues #23-#24.**
- Complete create, read, update, archive, list, filtering, deterministic cursor pagination, and optimistic-concurrency behaviour with stable problem responses. **Delivered for issues #25-#27.**
- Enforce Contacts permissions and membership-derived workspace scope at route and service boundaries, with RLS as the database backstop. **Delivered for issue #28.**
- Write contact changes, audit records, and versioned outbox events in one database transaction. **Delivered for issue #29.**
- Claim and publish the transactional outbox through NATS JetStream with acknowledged at-least-once delivery, bounded retries, and aggregate observability. **Delivered for issue #30.** Deduplicate native consumer effects through the tenant-scoped inbox marker in the same transaction, with typed ACK/retry/terminate outcomes. **Delivered for issue #31.** Subscription lifecycle wiring and full broker integration evidence remain issues #32 and #37.
- Make persistent plugin enablement drive API availability, navigation, subscriptions, and jobs while disablement retains data and records the lifecycle change. **Delivered for issues #188-#190** ([ADR 0009](../adr/0009-plugin-worker-contributions.md)); lifecycle audit rows carry the request's correlation id, and refused lifecycle requests are audited.
- Replace the demonstration web screen with plugin-contributed people and
  organisation workflows for list/search/pagination, create, view/edit,
  archive, validation, and permission-denied states. The interaction benchmark,
  visual foundations, responsive shell, and responsive metadata behaviour are
  delivered for issues #95-#97 and #102. **People workflows are API-backed for
  issue #34**: live search, sort, cursor paging, create, edit with version
  conflicts, and archive, with the browser lifecycle journey exercising
  create → reload → edit → archive against the production build. Organisation
  workflows, custom-field administration, and the complete acceptance gate
  remain issues #33, #35, #39, #98-#101, and #103.
- Convert the security, integration, performance, and usability plans into repeatable automated suites, including application-role RLS tests and Playwright accessibility coverage.

### Exit criteria

- Workspace A cannot read, mutate, enumerate, or infer Workspace B Contacts data, including with known internal or public identifiers and pagination probes.
- Contact operations persist across restarts, enforce version conflicts, and atomically produce the expected audit record and versioned outbox event.
- Duplicate delivery is idempotent, failed publication is retryable, and successful publication is observable without claiming exactly-once delivery.
- Disabling Contacts removes its contributed navigation, blocks its routes and background work, retains its data, and writes an audit record; re-enabling restores access safely.
- The web workflows are driven by enabled plugin contributions, pass their accessibility checks, and cover both successful and denied/error paths.
- Unit, integration, tenant-isolation, frontend, and end-to-end suites pass in CI from the reproducible baseline established in Gate 1.

## Next — Limiara in your pocket

**Outcome:** the same engine runs on the phone over an embedded SQLite database, as a standalone personal CRM or as a synced replica of a server workspace ([ADR 0024](../adr/0024-limiara-on-device.md)).

- The object engine and SQLite adapter built for iOS and Android, with the conformance suites run on simulators. For iOS this is wired up: the `ios` workflow (`scripts/test-device-ios.sh`, macOS runner) builds the device crates for iPhone and the simulator, runs the SQLite conformance suite and the `platform-device` tests inside an iOS Simulator, and packages, installs and launches the shell as a simulator app, recording its size and cold launch. Android follows (#387).
- A device build without the PostgreSQL driver, retention for the outbox, audit log and change log on the device, and batched writes for imports and the first sync ([ADR 0024](../adr/0024-limiara-on-device.md), "Measured footprint").
- A standalone app: the React shell packaged with Tauri 2 over a local command bridge, with encrypted storage. The bridge is built: the server's object routes now live in `platform-objects-http` and `platform-device` answers them in process over the replica, with the shell's `/me`, `/workspaces` and UI contributions ([ADR 0024](../adr/0024-limiara-on-device.md) §5). The replica is encrypted with SQLCipher under a per-workspace key from a keystore port, and a wipe destroys the key with the files (ADR 0024, "Encryption at rest"). The desktop shell is built too (`apps/device-shell`): Tauri 2 serving the web build and the bridge from one `limiara://` origin, with a demonstration workspace. Its sign-in is built too: the member enters the workspace address from Settings → Devices, signs in through the system browser on a loopback redirect, and the replica opens after the first sync, with its session and keys in the platform keychain and wipe on sign-out or refused refresh ([ADR 0029](../adr/0029-device-sign-in.md) §7). Still to come: the Android Keystore adapter, a Secret Service keychain on Linux that survives logging out, and the iOS and Android builds with their private-use redirect schemes.
- Continual sync pull over the live change stream, which already announces the workspace metadata version ([ADR 0024](../adr/0024-limiara-on-device.md), "Continual sync"), filtered by the member’s sharing, with the revocation set that tells a device which records to delete when access is lost (`POST /sync/revocations`, built), and content-free push notifications that wake a suspended device (the APNs and FCM adapters and the worker's dispatcher are built, [ADR 0030](../adr/0030-push-notifications.md)).
- Sync push of queued local changes as composite requests, with field-by-field conflict resolution, and the sync loop that drives push, pull, definitions and the revocation set over the workspace API (`platform-sync`, built). The foreground loop over `GET /objects/changes/stream` and the time-boxed background run a push notification starts are built too. Members read the active validation rules of objects they can read, so a device checks them offline.
- Device sign-in with OIDC and PKCE through the system browser, rotating device tokens, revocation, and wipe on refused contact are built ([ADR 0029](../adr/0029-device-sign-in.md)); the offline lease that locks the replica past the workspace's limit is built too; the device list in the web app's settings (Settings → Devices, with revoke) is built too; still to come is an end-to-end check of the browser leg against the Keycloak fixture.
- Phone capabilities as plugins (camera and attachments, location, calling, notifications), and publishing a standalone workspace to a server.
- Encrypted backup of standalone workspaces to the member's own cloud: weekly snapshots plus a journal of record, metadata and file changes, streamed as they happen, to the CloudKit private database on Apple devices and to OneDrive, Google Drive or Dropbox elsewhere. Files are uploaded once by content hash. It has a recovery key and replays the journal on restore ([ADR 0024](../adr/0024-limiara-on-device.md), "Backup to the member's own cloud"). The same journal can later sync one person's own devices without a server.
- Litestream backups for self-hosted SQLite servers to S3-compatible storage (R2, B2, SeaweedFS, Garage), with a restore drill in the operations guide ([ADR 0013](../adr/0013-swappable-storage-backends.md)).

## Later — CRM application

**Outcome:** assemble a coherent CRM from plugins after the secure Contacts slice is proven.

- Leads, opportunities, pipelines, stages, activities, notes, reminders, and cross-plugin timeline contributions.
- Import/export jobs with preview, validation, duplicate-detection hooks, progress, cancellation, and audit history.
- Reporting read models for pipeline, activity, conversion, and owner performance without raw cross-plugin table access.
- Signed webhooks with replay protection and delivery logs, plus hardened cursor/filter conventions and typed API clients.
- Governed, read-only OData projections for reporting and low-code integrations, including allowlisted filtering, selection, ordering, pagination, permission checks, and RLS. The existing OData endpoints are architectural prototypes, not a Current or Next gate commitment.

## Later — Trameia automation, integrations, and plugin trust

**Outcome:** add governed automation and a connector/plugin ecosystem without granting ambient authority.

- Automation beyond the record-triggered automations of [ADR 0022](../adr/0022-record-triggered-automation.md) (#298): compensation; approval gates; replay diagnostics; loops over related records; and actions that call connectors.
- Integration APIs beyond the composite requests, bulk jobs and API clients of [ADR 0023](../adr/0023-integration-apis.md): API client scopes narrower than the run-as member, per-client rate limits, an administrator page for API clients, naming the client in record evidence, composite requests that span other routes, bulk inserts or updates matched by external ID, CSV bulk input, and an administrator page for bulk jobs.
- Connector configuration using mediated secret references and capability-scoped outbound access.
- Full artifact signature and provenance verification, transparency identity policy, SBOM generation/scanning, capability-diff review, staged upgrades, rollback, and incident diagnostics.
- Marketplace discovery, compatibility, review, deprecation, and reserved-namespace governance.
- Stronger Wasm execution and external-service isolation, including resource limits, workload identity, mTLS, and network policy.
- Swappable storage backends, as proposed in [ADR 0013](../adr/0013-swappable-storage-backends.md) and tracked in issue #279: PostgreSQL stays the reference backend; SQLite serves single-node installations with one database file per workspace; DuckDB serves analytics. The Contacts store runs on both transactional backends today; kernel stores, the SQLite migration runner, DuckDB analytics and CI conformance runs follow.
- Change data capture beyond [ADR 0018](../adr/0018-change-data-capture.md) (#306, #287): the `touched_after` contract on plugin record APIs (Contacts), purged-record tombstones, and workspace-defined platform events published by automation.
- The query language beyond [ADR 0019](../adr/0019-record-query-language.md) (#304): child subqueries, `HAVING`, date functions in `GROUP BY`, traversal into users (`owner.name`), sorting and grouping through relationships, workspace time zones for relative dates, a `POST` form for long queries, and query cost limits on SQLite.
- Relationships beyond the master-detail fields of [ADR 0025](../adr/0025-master-detail-relationships.md) and the roll-up summary fields of [ADR 0026](../adr/0026-rollup-summary-fields.md) (#295): roll-up filter criteria, averages and roll-ups over lookups, background recalculation for very large masters, a junction lock order that cannot deadlock with a cascade, multi-level master-detail, moving details between masters as an opt-in field setting, and converting a lookup into a master-detail field on an object that already has records.
- Related lists and lookup filters beyond [ADR 0028](../adr/0028-related-lists-and-lookup-filters.md) (#295): filters that compare the target with the record being edited, optional filters that only warn, creating a related record from its list with the lookup filled in, choosing a related list's columns and order, and related lists through junction objects.
- Reports beyond the saved reports of [ADR 0027](../adr/0027-saved-reports.md) (#313, #314): a report builder that writes the query from chosen fields, groupings and filters, matrix reports, folders with sharing, scheduled email subscriptions, CSV and XLSX export, heavy aggregates on the analytics store (#284), and dashboards of report components with running user and refresh.
- Formulas beyond [ADR 0021](../adr/0021-formulas-and-validation-rules.md) (#298): filtering on formulas that follow lookups or use the time, which needs values kept current from related changes and a schedule.
- Global search beyond [ADR 0020](../adr/0020-global-search.md) (#305): boosting recently viewed records (with the recently viewed service, #285), synonyms and typo tolerance, server-side highlighting, the search bar on narrow screens, plugin objects with typed stores (Contacts) in the index, and a pluggable external search engine fed from the change log.
- Field history beyond [ADR 0017](../adr/0017-field-history.md) (#311): per-workspace retention limits, export and archival of history to object storage, and hiding entries for fields hidden by field-level security (#316).
- Standard record fields beyond the first set ([ADR 0014](../adr/0014-standard-record-fields.md), issue #285): record ownership and sharing beyond the first slice of [ADR 0016](../adr/0016-record-sharing.md) (#286: criteria- and owner-based sharing rules, public groups, queues as owners, and implicit sharing from child records), `touched_at` filters for incremental sync across every record API (#287), and per-member last viewed and last referenced timestamps with the recently viewed service.
- Object-store storage (issue #288): a kernel file port on local disk and S3-compatible stores (SeaweedFS and Garage self-hosted, R2 or B2 hosted; [ADR 0013](../adr/0013-swappable-storage-backends.md), "supported free backends"), a check that runs the PostgreSQL store suites against managed providers before they are listed, Parquet/Iceberg analytics and archives, and a spike of the Contacts store on an embedded engine over object storage, judged by the existing conformance suite.
- An admin and developer CLI (#366), rendered with rs-rich: manifest validation with errors shown at their source line, migration status, plugin capability checks, workspace health (forced RLS, record counts, evidence-table size), query results as tables, and progress for bulk and restore jobs. Every command has plain and `--json` output for scripts. It reaches workspaces only through the API with OAuth or through the migrator role, and the engine and phone build never depend on it.
- Plugins extending other plugins, as proposed in [ADR 0012](../adr/0012-plugin-extensions.md): manifest declarations for extension fields, related lists, actions and new objects, with one plugin able to extend several hosts (issue #271); namespaced extension columns and an extension-value port in Contacts; the kernel field service and merged resource metadata; extension plan review, disable, 30-day removal retention, export and purge; and host-upgrade compatibility checks that pause broken contributions.

## Later — Service operations

**Outcome:** extend the platform beyond CRM while preserving plugin ownership and tenant isolation.

- Jobs, visits, scheduling, dispatch, engineers, skills, territories, and availability.
- Assets, inspections, planned maintenance, service history, customer sites, inventory, purchasing, and job parts.
- Quote-to-job and contract/invoice integration points.
- An offline-capable field PWA with tenant-scoped sync cursors, selective projection, and conflict handling.
- Object-storage and performance profiles for attachments, photos, exports, asset histories, and mobile workloads.

## Later — Governed intelligence

**Outcome:** make AI a governed plugin capability rather than a kernel privilege.

- Model-provider, retrieval, and vector-store adapters controlled by tenant AI policy.
- AI assistants and agents with explicit tool grants, human approval gates, audit trails, and rate limits.
- Data minimisation/redaction, prompt/tool/version audit, evaluation harnesses, residency policy, and cost attribution.

## Cross-roadmap non-goals until explicitly promoted

- A microservice per plugin by default.
- Full marketplace installation or remote secret brokering before artifact and capability governance are mature.
- Arbitrary third-party JavaScript in the authenticated web origin.
- Claims of exactly-once event delivery.
- An untyped EAV model for business data. Metadata-driven objects over typed, validated generic storage were promoted by [ADR 0015](../adr/0015-metadata-driven-objects.md).
- Autonomous AI agents without explicit tool capabilities and human approval gates.
