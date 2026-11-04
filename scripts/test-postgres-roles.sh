#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-postgres-roles-${RANDOM:-0}-$$"
export POSTGRES_PORT=0

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    docker compose --project-name "$project" ps --all || true
    docker compose --project-name "$project" logs --no-color postgres || true
  fi
  docker compose --project-name "$project" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

docker compose config --quiet
docker compose --project-name "$project" up --detach --wait postgres
docker compose --project-name "$project" run --rm migrate

docker compose --project-name "$project" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  < tests/integration/postgres_role_reprovision_fixture.sql

# The image entrypoint applied the bootstrap once. Run it again against the
# initialized database to prove repeatability, password rotation safety, and
# preservation of an intentional plugin capability.
docker compose --project-name "$project" exec --no-TTY postgres \
  bash /docker-entrypoint-initdb.d/010_roles.sh

docker compose --project-name "$project" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_MIGRATOR_PASSWORD"; psql --host=127.0.0.1 --username=platform_migrator --dbname=platform --command="SELECT 1" >/dev/null'
docker compose --project-name "$project" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_APP_PASSWORD"; psql --host=127.0.0.1 --username=platform_app --dbname=platform --command="SELECT 1" >/dev/null'
docker compose --project-name "$project" exec --no-TTY postgres \
  bash -ceu 'export PGPASSWORD="$PLATFORM_WORKER_PASSWORD"; psql --host=127.0.0.1 --username=platform_worker --dbname=platform --command="SELECT 1" >/dev/null'

docker compose --project-name "$project" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  < tests/integration/postgres_roles.sql
