# Database migrations

The one-shot `migrate` Compose service applies lexically ordered core SQL files
from `crates/platform-db/migrations` before applying migrations for enabled
plugins. Enabled plugins are selected with `PLATFORM_ENABLED_PLUGINS` as a
comma-separated list of directory names under `plugins` (the local default is
`contacts`). Enabled plugins and the files within each plugin are applied in
lexical order. The API and worker wait for this service to complete successfully.

Run migrations independently with:

```bash
docker compose run --rm migrate
```

The database-owning `platform_migrator` role performs DDL. Request-serving
runtime services use the non-owner `platform_app` role. Background processing
uses the non-owner `platform_worker` role, which has no direct access to tenant
tables and can execute only bounded plugin recovery, outbox claim, settlement,
and backlog routines.
Neither runtime role owns tables or can bypass RLS.

Compose runs the idempotent `provision-db-roles` one-shot service before every
`migrate` run. This is required for upgrades as well as fresh databases:
PostgreSQL image entrypoint scripts run only when a volume is first created, so
new runtime roles must not depend on entrypoint initialization being replayed.
The one-shot service connects with the bootstrap identity, creates or rotates
the bounded roles, updates default privileges and explicit core capabilities,
and exits before the migration owner applies SQL. It never blanket-grants over
existing plugin objects, so deliberate plugin table and function ACLs survive
credential rotation.

## Tenant-RLS assurance rollout

Core migration `0009_tenant_rls_assurance.sql` requires `platform_worker` to
exist. Always invoke migrations through Compose, or run the role-provisioning
step first when operating the migration runner elsewhere. The migration forces
RLS on the remaining tenant tables, makes audit and outbox identity
`(tenant_id, id)`, and installs the bounded background-worker capabilities.
Contacts migration `0003_tenant_scoped_cursor_identity.sql` applies the same
tenant-scoped identity rule to cursor hashes.

As a one-time upgrade repair, 0009 revokes function `EXECUTE` from `PUBLIC`,
`platform_app`, and `platform_worker` across every existing non-system schema,
then regrants only the declared core capabilities. If a plugin intentionally
exports a database function to a runtime role, add a new, lexically later
plugin migration that grants that exact function to the exact role. An older,
already-ledgered plugin migration will not rerun after 0009, and a manual grant
is not a durable substitute. Do not grant plugin functions to `PUBLIC`.

Each successful file is recorded in `plugin_migration_ledger` with its plugin
ID, filename, and SHA-256 checksum. Re-running an unchanged migration is a
no-op. Never edit an applied file: checksum drift stops startup and reports the
affected file. Restore that file and add a new, lexically later migration.
Failed migrations are rolled back together with their ledger entry, so fix the
reported SQL file and rerun the migration service.

The integration gate exercises fresh installation, repeat execution, checksum
drift, transactional failure recovery, runtime-role reprovisioning, and the
application-role tenant-isolation harness:

```bash
bash scripts/test-postgres-migrations.sh
```
