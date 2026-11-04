#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="${1:?compose project name is required}"
compose=(docker compose --project-name "$project")
psql_bootstrap=("${compose[@]}" exec --no-TTY postgres psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 --tuples-only --no-align)

assert_refusal() {
  label=$1
  status=$2
  expected=$3
  output=$4
  if (( status == 0 )); then
    printf 'tenant RLS harness unexpectedly ran as %s\n' "$label" >&2
    exit 1
  fi
  if [[ "$output" != *"$expected"* ]]; then
    printf 'tenant RLS harness rejected %s for the wrong reason:\n%s\n' "$label" "$output" >&2
    exit 1
  fi
  printf 'tenant RLS harness rejected the %s session\n' "$label"
}

# Reprovision after migrations to prove password rotation preserves core
# ledger revokes and explicit capabilities without restoring blanket access.
env MSYS_NO_PATHCONV=1 "${compose[@]}" exec --no-TTY postgres \
  bash /docker-entrypoint-initdb.d/010_roles.sh

set +e
superuser_output=$("${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  < tests/security/tenant_isolation.sql 2>&1)
superuser_status=$?
set -e
assert_refusal superuser "$superuser_status" "RLS harness refuses superuser sessions" "$superuser_output"

set +e
owner_output=$("${compose[@]}" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_MIGRATOR_PASSWORD"; psql --host=127.0.0.1 --username=platform_migrator --dbname=platform --set=ON_ERROR_STOP=1' \
  < tests/security/tenant_isolation.sql 2>&1)
owner_status=$?
set -e
assert_refusal table-owner "$owner_status" "RLS harness refuses table-owner sessions" "$owner_output"

"${psql_bootstrap[@]}" --command="CREATE ROLE platform_rls_bypass_probe LOGIN PASSWORD 'platform_rls_bypass_probe' NOSUPERUSER BYPASSRLS; GRANT CONNECT ON DATABASE platform TO platform_rls_bypass_probe" >/dev/null
set +e
bypass_output=$("${compose[@]}" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD=platform_rls_bypass_probe; psql --host=127.0.0.1 --username=platform_rls_bypass_probe --dbname=platform --set=ON_ERROR_STOP=1' \
  < tests/security/tenant_isolation.sql 2>&1)
bypass_status=$?
set -e
"${psql_bootstrap[@]}" --command="REVOKE CONNECT ON DATABASE platform FROM platform_rls_bypass_probe; DROP ROLE platform_rls_bypass_probe" >/dev/null
assert_refusal BYPASSRLS "$bypass_status" "RLS harness refuses BYPASSRLS roles" "$bypass_output"

"${compose[@]}" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_APP_PASSWORD"; psql --host=127.0.0.1 --username=platform_app --dbname=platform --set=ON_ERROR_STOP=1' \
  < tests/security/tenant_isolation.sql

printf 'tenant RLS harness passed as platform_app and rejected privileged roles\n'
