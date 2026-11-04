#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-postgres-migrations-${RANDOM:-0}-$$"
upgrade_project="${project}-upgrade"
fixture_root="$(mktemp -d)"
fixture_mount="$fixture_root"
docker_path_env=()
if command -v cygpath >/dev/null 2>&1; then
  fixture_mount="$(cygpath --windows "$fixture_root")"
  docker_path_env=(env MSYS_NO_PATHCONV=1)
fi
export POSTGRES_PORT=0

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    docker compose --project-name "$project" ps --all || true
    docker compose --project-name "$project" logs --no-color postgres migrate || true
    docker compose --project-name "$upgrade_project" ps --all || true
    docker compose --project-name "$upgrade_project" logs --no-color postgres provision-db-roles migrate || true
  fi
  docker compose --project-name "$project" down --volumes --remove-orphans >/dev/null 2>&1 || true
  docker compose --project-name "$upgrade_project" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$fixture_root"
  exit "$status"
}
trap cleanup EXIT

compose=(docker compose --project-name "$project")
psql_bootstrap=("${compose[@]}" exec --no-TTY postgres psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 --tuples-only --no-align)
upgrade_compose=(docker compose --project-name "$upgrade_project")
upgrade_psql_bootstrap=("${upgrade_compose[@]}" exec --no-TTY postgres psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 --tuples-only --no-align)

docker compose config --quiet
"${compose[@]}" up --detach --wait postgres

# Fresh database: core migrations must complete before an enabled plugin can
# reference core objects, and every file gets one immutable ledger entry.
"${compose[@]}" run --rm migrate
ledger_count=$("${psql_bootstrap[@]}" --command="SELECT count(*) FROM plugin_migration_ledger WHERE tenant_id = '00000000-0000-0000-0000-000000000000'")
# One ledger row per core and enabled-plugin (Contacts) migration file, derived
# from the checkout so adding a migration does not require editing this gate.
expected_ledger_count=$(( $(find crates/platform-db/migrations -maxdepth 1 -name '*.sql' | wc -l) + $(find plugins/contacts/migrations -maxdepth 1 -name '*.sql' | wc -l) ))
[[ "$ledger_count" == "$expected_ledger_count" ]]
"${psql_bootstrap[@]}" --command="SELECT count(*) FROM plugin_migration_ledger WHERE tenant_id = '00000000-0000-0000-0000-000000000000' AND plugin_id = 'platform.core' AND migration_id = '0009_tenant_rls_assurance.sql'" | grep -qx 1
"${psql_bootstrap[@]}" --command="SELECT to_regclass('public.tenants') IS NOT NULL AND to_regclass('p_contacts.people') IS NOT NULL" | grep -qx t

# Password rotation and the guarded application-role suite share the same
# runner as the dedicated tenant-RLS command.
bash scripts/run-tenant-rls-harness.sh "$project"

# A second run skips every recorded checksum and does not add ledger rows.
rerun_output=$("${compose[@]}" run --rm migrate 2>&1)
grep -q "migration already applied: platform.core/0001_core.sql" <<<"$rerun_output"
rerun_count=$("${psql_bootstrap[@]}" --command="SELECT count(*) FROM plugin_migration_ledger WHERE tenant_id = '00000000-0000-0000-0000-000000000000'")
[[ "$rerun_count" == "$ledger_count" ]]

# Editing an applied migration must fail with a repair instruction.
mkdir -p "$fixture_root/drift/crates/platform-db/migrations" "$fixture_root/drift/plugins"
cp crates/platform-db/migrations/*.sql "$fixture_root/drift/crates/platform-db/migrations/"
printf '\n-- forbidden historical edit\n' >> "$fixture_root/drift/crates/platform-db/migrations/0001_core.sql"
set +e
drift_output=$("${docker_path_env[@]}" "${compose[@]}" run --rm --no-deps \
  --volume "$fixture_mount/drift:/fixture:ro" \
  --env PLATFORM_MIGRATIONS_ROOT=/fixture \
  --env PLATFORM_ENABLED_PLUGINS= migrate 2>&1)
drift_status=$?
set -e
[[ $drift_status -ne 0 ]]
grep -q "checksum drift for platform.core/0001_core.sql" <<<"$drift_output"
grep -q "add a new file" <<<"$drift_output"

# A broken plugin migration is atomic: neither its DDL nor ledger row survives.
mkdir -p "$fixture_root/failure/crates/platform-db" "$fixture_root/failure/plugins/failure/migrations"
cp -R crates/platform-db/migrations "$fixture_root/failure/crates/platform-db/"
printf 'id = "test.failure"\n' > "$fixture_root/failure/plugins/failure/plugin.toml"
printf 'CREATE TABLE failed_migration_probe (id integer);\nSELECT definitely_not_valid;\n' > "$fixture_root/failure/plugins/failure/migrations/0001_failure.sql"
set +e
failure_output=$("${docker_path_env[@]}" "${compose[@]}" run --rm --no-deps \
  --volume "$fixture_mount/failure:/fixture:ro" \
  --env PLATFORM_MIGRATIONS_ROOT=/fixture \
  --env PLATFORM_ENABLED_PLUGINS=failure migrate 2>&1)
failure_status=$?
set -e
[[ $failure_status -ne 0 ]]
grep -q "failed to apply test.failure/0001_failure.sql" <<<"$failure_output"
"${psql_bootstrap[@]}" --command="SELECT to_regclass('public.failed_migration_probe') IS NULL" | grep -qx t
"${psql_bootstrap[@]}" --command="SELECT count(*) FROM plugin_migration_ledger WHERE plugin_id = 'test.failure'" | grep -qx 0

# Existing PostgreSQL volumes do not replay image entrypoint scripts. Recreate
# the pre-0009 state without platform_worker, then prove the Compose one-shot
# provisions that role before the current migration is attempted.
mkdir -p "$fixture_root/upgrade/crates/platform-db/migrations" \
  "$fixture_root/upgrade/plugins/contacts/migrations"
cp crates/platform-db/migrations/000[1-8]_*.sql \
  "$fixture_root/upgrade/crates/platform-db/migrations/"
cp plugins/contacts/plugin.toml "$fixture_root/upgrade/plugins/contacts/"
cp plugins/contacts/migrations/000[1-2]_*.sql \
  "$fixture_root/upgrade/plugins/contacts/migrations/"

"${upgrade_compose[@]}" up --detach --wait postgres
"${docker_path_env[@]}" "${upgrade_compose[@]}" run --rm \
  --volume "$fixture_mount/upgrade:/fixture:ro" \
  --env PLATFORM_MIGRATIONS_ROOT=/fixture migrate
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM plugin_migration_ledger WHERE tenant_id = '00000000-0000-0000-0000-000000000000'" |
  grep -qx 10
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM plugin_migration_ledger WHERE plugin_id = 'platform.core' AND migration_id = '0009_tenant_rls_assurance.sql'" |
  grep -qx 0
"${upgrade_psql_bootstrap[@]}" \
  --command="SET ROLE platform_migrator; CREATE SCHEMA legacy_plugin; CREATE FUNCTION legacy_plugin.leaked_runtime_function() RETURNS integer LANGUAGE sql AS 'SELECT 1'; REVOKE ALL ON FUNCTION legacy_plugin.leaked_runtime_function() FROM PUBLIC; GRANT USAGE ON SCHEMA legacy_plugin TO platform_app; GRANT EXECUTE ON FUNCTION legacy_plugin.leaked_runtime_function() TO platform_app; RESET ROLE" \
  >/dev/null
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT has_function_privilege('platform_app', 'legacy_plugin.leaked_runtime_function()', 'EXECUTE')" |
  grep -qx t
"${upgrade_psql_bootstrap[@]}" \
  --command="REVOKE CONNECT ON DATABASE platform FROM platform_worker; DROP OWNED BY platform_worker; DROP ROLE platform_worker" \
  >/dev/null
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname = 'platform_worker'" |
  grep -qx 0

"${upgrade_compose[@]}" rm --force --stop provision-db-roles >/dev/null
upgrade_output=$("${upgrade_compose[@]}" run --rm migrate 2>&1)
grep -q "applying migration: platform.core/0009_tenant_rls_assurance.sql" <<<"$upgrade_output"
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM plugin_migration_ledger WHERE plugin_id = 'platform.core' AND migration_id = '0009_tenant_rls_assurance.sql'" |
  grep -qx 1
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM plugin_migration_ledger WHERE plugin_id = 'platform.contacts' AND migration_id = '0003_tenant_scoped_cursor_identity.sql'" |
  grep -qx 1
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT has_function_privilege('platform_app', 'legacy_plugin.leaked_runtime_function()', 'EXECUTE')" |
  grep -qx f
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname = 'platform_worker' AND rolcanlogin AND NOT rolsuper AND NOT rolbypassrls" |
  grep -qx 1
"${upgrade_psql_bootstrap[@]}" \
  --command="SELECT has_function_privilege('platform_worker', 'platform_claim_outbox_batch(bigint,uuid,bigint)', 'EXECUTE') AND NOT has_function_privilege('platform_app', 'platform_claim_outbox_batch(bigint,uuid,bigint)', 'EXECUTE') AND has_function_privilege('platform_worker', 'platform_recover_plugin_transitions()', 'EXECUTE') AND NOT has_function_privilege('platform_app', 'platform_recover_plugin_transitions()', 'EXECUTE')" |
  grep -qx t
