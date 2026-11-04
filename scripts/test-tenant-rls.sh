#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-tenant-rls-${RANDOM:-0}-$$"
export POSTGRES_PORT=0

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    docker compose --project-name "$project" ps --all || true
    docker compose --project-name "$project" logs --no-color postgres migrate || true
  fi
  docker compose --project-name "$project" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

docker compose config --quiet
docker compose --project-name "$project" up --detach --wait postgres
docker compose --project-name "$project" run --rm migrate
bash scripts/run-tenant-rls-harness.sh "$project"
