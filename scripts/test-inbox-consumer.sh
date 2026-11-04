#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-inbox-consumer-${RANDOM:-0}-$$"
export POSTGRES_PORT=0
compose=(docker compose --project-name "$project")

diagnostics() {
  "${compose[@]}" ps --all || true
  "${compose[@]}" logs --no-color --tail=200 postgres migrate || true
  "${compose[@]}" exec --no-TTY postgres psql \
    --username platform_bootstrap --dbname platform --tuples-only --no-align \
    --command="SELECT tenant_id, consumer_id, event_id FROM inbox ORDER BY tenant_id, consumer_id, event_id" \
    || true
}

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    diagnostics >&2
  fi
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

docker compose config --quiet
"${compose[@]}" up --detach --wait postgres
"${compose[@]}" run --rm migrate

# This probe table exists only in the disposable acceptance database. It gives
# the Rust handler a real forced-RLS side effect without adding test state to a
# production migration.
fixture_sql=$(cat <<'SQL'
CREATE TABLE inbox_acceptance_effects (
  tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  consumer_id text NOT NULL,
  event_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, consumer_id, event_id)
);
ALTER TABLE inbox_acceptance_effects ENABLE ROW LEVEL SECURITY;
ALTER TABLE inbox_acceptance_effects FORCE ROW LEVEL SECURITY;
CREATE POLICY inbox_acceptance_effects_tenant_isolation
ON inbox_acceptance_effects
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
GRANT SELECT, INSERT ON inbox_acceptance_effects TO platform_app;
SQL
)
printf '%s\n' "$fixture_sql" | "${compose[@]}" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_MIGRATOR_PASSWORD"; psql --host=127.0.0.1 --username=platform_migrator --dbname=platform --set=ON_ERROR_STOP=1 --file=-'

postgres_port=$("${compose[@]}" port postgres 5432 | awk -F: 'NR == 1 { print $NF }')
[[ "$postgres_port" =~ ^[0-9]+$ ]]
export PLATFORM_TEST_DATABASE_URL="postgres://platform_app:platform_app@127.0.0.1:${postgres_port}/platform"

cargo test -p platform-inbox --locked --test postgres -- --ignored --nocapture

printf 'inbox acceptance passed for duplicate, rollback, tenant, and poison behavior\n'
