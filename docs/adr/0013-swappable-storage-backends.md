# 0013: Swappable storage backends

## Status

Accepted (2026-10-11). Issue #279 tracks the work. The backend roles in
section 2 are confirmed, including DuckDB for analytics and reporting only,
never as the transactional store of a running service. The first slice
(backend selection and the Contacts store on PostgreSQL and SQLite) is
implemented; the kernel stores follow.

## Context

Limiara runs only on PostgreSQL. Operators want smaller installations
(single node, desktop, evaluation, edge) without a database server, and
analysts want columnar analytics. The requested backends are PostgreSQL,
SQLite and DuckDB, with room for others.

The current design leans on PostgreSQL in ways that matter for security, not
just syntax:

- Tenant isolation is enforced by forced row-level security keyed on
  `app.tenant_id`, and plugin isolation by restrictive policies keyed on
  `app.plugin_id` (ADR 0003, migration 0012).
- Privilege separation uses three database roles, and every cross-tenant
  operation goes through a bounded `SECURITY DEFINER` routine.
- Queues lease work with `FOR UPDATE SKIP LOCKED`; lifecycle drains use
  `FOR SHARE`; six places serialise with advisory locks.
- The API and the worker are separate processes sharing one database.

About 3,000 lines of SQL and 4,000 lines of Rust data access are written for
PostgreSQL today, across sessions, RBAC, the plugin registry, audit, outbox,
inbox, jobs, UI publications, extension layouts and the Contacts plugin.

## Decision

### 1. Storage is a set of ports, not a database

Every component that persists data does so through a store trait it owns:
identity and sessions, workspace access, plugin registry and lifecycle, audit,
outbox, inbox, jobs, UI publications, extension layouts, and each plugin's
own repository. Domain and service code depends only on these traits. Each
supported backend provides an adapter for every trait. No code outside an
adapter names a database driver type.

A unit of work spans one tenant: the store hands out a tenant transaction in
which the domain change, its audit entry and its outbox event commit
atomically, on every backend.

### 2. Backends and what each is for

| Backend | Role | Tenant isolation | Concurrency model |
| --- | --- | --- | --- |
| PostgreSQL | Reference backend for every deployment, required for multi-node and multi-tenant service. | Forced RLS plus tenant predicates, plugin-context policies, separate roles. | Many processes and nodes; row leases. |
| SQLite | Single-node, self-hosted, desktop and evaluation installations. | One database file per workspace plus tenant predicates; a workspace store can open only its own file. | One node; API and worker share files in WAL mode; one writer at a time. |
| DuckDB | Analytics and reporting over events and read models; never the transactional store of a running service. | One analytics file per workspace. | One writing process; readers open snapshots. |

DuckDB is not offered as the primary store for the service because a DuckDB
file accepts one read-write process at a time, and Limiara runs the API and
the worker as separate processes. A future all-in-one single-process mode may
revisit this; it would still use the same ports.

Other relational databases (for example MySQL or SQL Server) can be added by
writing adapters for every port and passing the conformance suite. Each must
document its tenant isolation strategy, since none of them offer PostgreSQL's
RLS semantics.

### 3. Backends declare capabilities; the platform refuses unsafe combinations

Each backend declares its capabilities: row-level security, plugin-context
isolation, multi-process writers, multi-node, row leasing, and JSON
containment indexes. At startup the platform compares them with the
deployment profile and refuses, with a clear error, combinations it cannot
make safe. For example SQLite is refused for a multi-node profile, and DuckDB
is refused as the primary store.

### 4. Tenancy without row-level security

Where a backend lacks RLS, tenant isolation is structural: one database file
(or schema, for server databases) per workspace, opened only through the
tenant store for that workspace. Tables still carry `tenant_id`, queries
still filter on it, and the conformance suite proves one workspace can never
read or write another's rows. This keeps the CLAUDE.md rule that every tenant
table has `tenant_id` while replacing "force RLS" with an equivalent guarantee
on backends that cannot express it.

Plugin isolation on such backends rests on code ownership: only linked native
plugins, reviewed with the platform, run in-process; each owns tables under
its own prefix, and the conformance suite checks that a plugin store touches
only its prefix. Wasm and external plugins never receive database access on
any backend.

### 5. Plugins declare the backends they support

Native plugins that persist data ship a store adapter per backend they
support and declare them in the manifest:

```toml
[storage]
backends = ["postgres", "sqlite"]
```

The registry refuses to enable a plugin on a backend it does not declare. A
manifest without `[storage]` is treated as PostgreSQL-only.

### 6. One conformance suite decides "supported"

Each port has a behavioural conformance suite run against every adapter:
tenant isolation, optimistic versions, atomic audit and outbox, idempotent
inbox, lease expiry, and the plugin lifecycle. A backend is supported only
when it passes all suites for every port. The SQLite suites run in ordinary
`cargo test` with no external services; the PostgreSQL suites run through the
existing scripts.

### 7. Configuration

```yaml
platform:
  database:
    backend: postgres      # postgres or sqlite
    url: postgres://...    # postgres
    sqlite_directory: /var/lib/limiara   # sqlite: one file per workspace
  analytics:
    backend: none          # none or duckdb
    duckdb_directory: /var/lib/limiara/analytics
```

## Alternatives considered

- **A portable SQL layer or ORM over every backend.** It hides dialects but
  not semantics: RLS, leases and isolation would still differ, and the
  lowest common denominator would weaken PostgreSQL deployments. Rejected in
  favour of explicit ports with per-backend adapters.
- **Application-only tenant filters on one shared SQLite file.** Simpler, but
  one missing predicate leaks data across workspaces with no backstop.
  Rejected; a file per workspace makes the boundary physical.
- **DuckDB as a primary store.** Its single read-write process conflicts with
  the API and worker process model and it is optimised for analytics, not
  short concurrent transactions. Rejected for now; used for analytics.

## Consequences

- PostgreSQL remains the reference and the only backend for multi-node or
  hosted service; its security model is unchanged.
- Every new persistence feature needs an adapter per supported backend and
  conformance coverage, which raises the cost of kernel changes.
- SQLite installations scale to one node and moderate write volume; their
  plugin isolation is weaker than PostgreSQL's and is documented as such.
- Analytics on DuckDB is eventually consistent with the operational store.

## Amendment: the generic record store (ADR 0015)

ADR 0015 adopts Salesforce's universal data dictionary: metadata tables over
generic record storage with typed pivots. For storage this means:

- The **generic record store** becomes the main port. One adapter per
  backend serves every metadata-defined object, instead of one adapter per
  plugin per backend. Typed plugin stores remain for plugins that need them.
- **Order of work**: the record store lands on PostgreSQL and SQLite first
  (with the object engine), then the remaining kernel stores move to SQLite
  (#282), so a single-node install runs the whole platform on one file per
  workspace.
- **Analytics**: DuckDB reads the record, index and history tables (or
  Parquet exports of them) for reports and dashboards (#284, #313).
- **Object storage**: files, attachments and big objects (#329, #330, #288),
  never the transactional record store of a running service, matching
  Salesforce's split between its relational core and its file and big-object
  stores.

## Amendment: supported free backends (2026-10-13)

Limiara must run end to end without paying for storage: on a phone, on one
self-hosted machine and on hosted free tiers. These are the backends the
project supports or plans to support, and the conditions on each. A
backend is "supported" only once it passes the conformance suite for its
port (§6). Today that means SQLite and PostgreSQL for records. Everything
for files, and every hosted service, is planned and recorded here as a
candidate until its adapter exists and passes.

**On the device** (ADR 0024), with no service at all:
- SQLite (public domain) for the record store, as today.
- SQLCipher Community Edition (BSD-style licence) for encryption at rest.
  Its key lives in the iOS Keychain or Android Keystore.
- Planned: the app's sandboxed directory for files, through the file port
  (#288), which does not exist yet.

**Self-hosted**, free software on the operator's own machine:
- PostgreSQL for the record store, as today.
- Planned for the file port (#288), which does not exist yet, in order of
  preference:
  - local disk, the first adapter;
  - SeaweedFS (Apache 2.0), S3-compatible;
  - Garage (AGPLv3), S3-compatible. Running it unmodified, as a separate
    service, places no AGPL duty on Limiara.
- MinIO Community Edition is not supported. Upstream put it into
  maintenance mode in 2025, and its repository has since been archived.

**Hosted free tiers**, for evaluation and small teams:
- **Files:** any S3-compatible store through the same adapter. Cloudflare
  R2 (10 GB, no egress fees) and Backblaze B2 (10 GB) are candidates.
- **Record store:** managed PostgreSQL, with Neon and Supabase as
  candidates. A provider qualifies only if:
  - the migrations can create Limiara's own roles (`platform_migrator`,
    `platform_app`, `platform_worker`) and force row-level security;
  - the PostgreSQL store suites pass against it.
- **Auth:** a provider's bundled authentication is never used. Sign-in
  stays OIDC with backend sessions (no password system).
- **Limits:** free allowances change without notice. The operations guide
  states each limit with the date it was checked, and Limiara never relies
  on a provider-specific feature to stay within one.

Measured sizes, which tell operators what fits where (2026-10-13, 10,000
contacts with five fields each, created one at a time, no edits yet):
- **PostgreSQL:** 38 MB, about 3.8 KB per record.
  - Records, their indexes and search terms take about 19 MB.
  - The outbox, audit log and change log take about 20 MB.
  - At that rate a 0.5 GB managed database holds about 130,000 such
    records, with all evidence kept. Edits add history and evidence on top,
    so plan below that figure.
- **SQLite:** about 50 MB, roughly 5 KB per record (ADR 0024, "Measured
  footprint").

Backups of a self-hosted SQLite server use Litestream (Apache 2.0). It
streams each workspace file's write-ahead log continuously to an
S3-compatible store: the self-hosted ones above, or R2 or B2 within their
free tiers. A restore rebuilds the file from the stream. Each workspace is
its own file (§4), so each is replicated and restored on its own. Phones
do not use Litestream. They back up to the member's own cloud through the
engine's change stream instead (ADR 0024, "Backup to the member's own
cloud").

Follow-up, in #288 and #283:
- an S3-compatible adapter for the file port, with its conformance suite
  run against SeaweedFS and Garage in CI;
- a check that runs the PostgreSQL store suites against a managed provider
  before it is listed as supported;
- an operations-guide page listing the free backends and their checked
  limits;
- an operations-guide page for Litestream: configuration per workspace
  file, a restore drill, and a check that a restored file passes the
  conformance suite.

## Implementation status

Implemented in the first slice:

- Backend selection, capability declarations and startup validation
  (`platform-storage`, `platform.database.backend`) (#280). Services refuse
  to start on SQLite until every kernel store has a SQLite adapter, and say
  which store is missing.
- Plugin manifest `[storage] backends` and registry enforcement (#280).
- SQLite workspace databases: one file per workspace, a checksummed
  migration ledger, kernel audit and outbox tables, `BEGIN IMMEDIATE` write
  transactions (#281).
- The Contacts store port, with the existing PostgreSQL adapter and a new
  SQLite adapter, both passing one conformance suite through the real
  service (#281).

The conformance suite found a PostgreSQL defect along the way: search and
name ordering used `lower()`, which follows the database locale, so a
cluster initialised with the C locale did not match "ÉMILE" to "Émile" and
could order names differently from the list cursors. Contacts now stores
search keys the service computes, on both backends (Contacts migration 0007).

Follow-up work, each in its own issue under #279:

- kernel stores on SQLite: identity and sessions, workspace access, registry
  and lifecycle, audit and outbox publishing, inbox, jobs, UI publications
  and layouts, and the Contacts worker contributions and extension host
  port (#282);
- the SQLite migration runner, operations guide and CI runs (#283);
- DuckDB analytics (#284);
- evaluating object stores (on-device and cloud) for files, analytics and
  archives, and possibly as a transactional backend over an embedded engine
  such as SlateDB (#288).
