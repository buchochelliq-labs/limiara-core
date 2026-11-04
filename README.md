# Limiara / neutral platform scaffold

Limiara is a forward-looking, open-core, plugin-native customer-operations platform. It starts with CRM capabilities, but the architecture is intentionally broader: the long-term lifecycle is **lead → opportunity → quote → job → visit → asset history → invoice → ongoing service**.

The internal composability architecture is provisionally called **Trameia**. The product name is not embedded into crate names, API paths, schemas, or infrastructure identifiers so the platform remains rebrandable.

## Current repository state

The foundation scaffold is delivered. The active milestone is the **reproducible platform baseline** described in the [roadmap](docs/roadmap/README.md); the secure Contacts vertical slice has not yet shipped end to end.

The repository currently contains:

- Rust services for `api`, `worker`, and `plugin-runner`, all using the same validated nested configuration contract and dependency-aware health endpoints.
- Domain-neutral platform interfaces and initial implementations for configuration, database access, tenancy, permissions, audit, events, outbox records, entitlements, plugin contracts, plugin registry, native runtime, Wasm capability denial, and service-plugin protocol bindings.
- A React/Vite demonstration shell for workspace selection, plugin-aware navigation, and basic person creation rather than the complete Contacts workflow.
- A first-party Contacts manifest, SQL schema, domain types, validation helpers, and an in-memory API demonstration; persistence and lifecycle wiring remain roadmap work.
- A hello Wasm interface and capability-denial proof of concept; Wasmtime component execution is not yet implemented.
- Docker Compose and container scaffolds, architecture and operations documentation, a threat model, and initial test plans or SQL artifacts.

## Architecture at a glance

Limiara uses a **modular monolith first** approach. The target initial deployable services are:

1. `api` — Rust HTTP application, platform kernel, and trusted native plugin route registration.
2. `worker` — transactional outbox publishing, consumers, scheduled work, and background jobs; the current binary is a startup scaffold.
3. `plugin-runner` — sandboxed Wasm execution boundary; the current implementation proves default-deny behaviour without executing a component.
4. `web` — React/TypeScript frontend shell.
5. PostgreSQL and NATS JetStream as required local infrastructure.

The kernel is deliberately domain-neutral. Business capabilities such as contacts, deals, jobs, visits, assets, quotes, invoices, workflow automation, reporting, and AI assistants are plugins.

## Plugin execution modes

The platform defines one versioned plugin contract with three execution modes:

These are architectural contracts at the current maturity level: native registration is an interface, Wasm is a capability-denial proof of concept, and external services are protocol-only.

- **Trusted native plugins:** Rust crates linked into `api`/`worker`, using the same manifest, permissions, migrations, lifecycle, events, and UI contribution contracts as other plugins.
- **Sandboxed Wasm plugins:** Wasmtime/WASI Component Model plugins with default-deny capabilities, resource limits, mediated host interfaces, and no ambient filesystem, environment, database, secrets, or network access.
- **External service plugins:** future polyglot OCI/container workloads using the protobuf protocol under `proto/plugin/v1`; the contract is defined now, but a full supervisor is deferred.

## Local development

The target local-development workflow is:

```bash
cp config/local.yml.example config/local.yml
docker compose up --build
```

Add `--wait` to block until the dependency-ordered stack is healthy. See the
[local startup runbook](docs/operations/local-startup.md) for service state,
failure diagnostics, restarts, and explicit durable-state reset.

Compose runs core and enabled-plugin database migrations before starting the API
or worker. Use `PLATFORM_ENABLED_PLUGINS=contacts,another-plugin` to select the
local plugin migration set. See the [migration runbook](docs/operations/migrations.md)
for reruns and failure recovery.

Compose also imports a disposable `local` realm into a pinned Keycloak service
at `http://localhost:8081`. The fixture includes a PKCE-enabled public client
and member/non-member test identities; see the
[local identity runbook](docs/operations/local-identity.md) for credentials,
inspection, and reset commands.

The configuration contract is nested YAML-first: required defaults from
`config/default.yml`, then an optional file named by `PLATFORM_CONFIG_FILE`,
then `PLATFORM__...` environment keys. Later sources win. Compose mounts the
`config` directory read-only and uses the same contract in API, worker, and
plugin runner. See the [configuration runbook](docs/operations/configuration.md).

Compose provisions a database-owning `platform_migrator` role, a non-owner
`platform_app` request role, and a non-owner `platform_worker` background role
through a bootstrap-only PostgreSQL identity. The checked-in password defaults
are local-development fixtures only; override
`PLATFORM_BOOTSTRAP_PASSWORD`, `PLATFORM_MIGRATOR_PASSWORD`, and
`PLATFORM_APP_PASSWORD`, and `PLATFORM_WORKER_PASSWORD` outside an isolated
developer machine. Runtime roles cannot create or alter schema objects and
cannot bypass row-level security; the worker has no direct tenant-table access.
An idempotent `provision-db-roles` one-shot service runs before migrations so
existing PostgreSQL volumes receive newly introduced runtime roles safely. It
preserves explicit plugin object ACLs when rotating credentials.

Verify first-run and repeated role provisioning with:

```bash
bash scripts/test-postgres-roles.sh
```

On Windows PowerShell, run `./scripts/test-postgres-roles.ps1` instead.

The local stack builds and starts in dependency order. `/health/live` reports
only process health; `/health/ready` checks each service's required database,
migration, NATS, and plugin-registry responsibilities.

Seeded workspaces for tests:

- Workspace A: `018f0000-0000-7000-8000-000000000001`
- Workspace B: `018f0000-0000-7000-8000-000000000002`

OIDC remains provider-neutral in the product. Keycloak is used only as a seeded
development and integration-test fixture.

## Validation

The required validation suite is:

```bash
cargo build --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm lint
pnpm build
pnpm docs:check
```

CI runs the same commands on every push and pull request using Rust 1.95.0,
Node.js 22, pnpm 9.12.0, and the committed Cargo and pnpm lockfiles. Required
checks fail when a command is unavailable or unsuccessful; do not replace a
required check with a warning or silently skip it.

## Documentation map

- Product overview: [docs/product/overview.md](docs/product/overview.md)
- Quick start: [docs/guide/getting-started.md](docs/guide/getting-started.md)
- Documentation contribution policy: [docs/contributing/documentation.md](docs/contributing/documentation.md)
- Documentation inventory and ownership: [docs/inventory.json](docs/inventory.json) and [docs/owners.json](docs/owners.json)
- Roadmap: [docs/roadmap/README.md](docs/roadmap/README.md)
- Roadmap issue index: <https://github.com/buchochelliq-labs/limiara-core/issues/90>
- Architecture decisions: [docs/adr/](docs/adr/)
- Threat model: [docs/threat-model/phase-1.md](docs/threat-model/phase-1.md)
- Open-core boundary: [docs/architecture/open-core.md](docs/architecture/open-core.md)
- Plugin manifest schema: `docs/architecture/plugin-manifest.schema.json`
- Native plugin guide: [docs/plugins/native-plugin.md](docs/plugins/native-plugin.md)
- Wasm plugin guide: [docs/plugins/wasm.md](docs/plugins/wasm.md)
- Service-plugin protocol: [docs/architecture/service-plugins.md](docs/architecture/service-plugins.md)
- Migration runbook: [docs/operations/migrations.md](docs/operations/migrations.md)
- Backup/restore runbook: [docs/operations/backup-restore.md](docs/operations/backup-restore.md)
- Support and safe issue reporting: [docs/support/index.md](docs/support/index.md)
- Release/version policy: [docs/releases/index.md](docs/releases/index.md)

## Licence

Limiara is licensed under the [Apache License 2.0](LICENSE).
