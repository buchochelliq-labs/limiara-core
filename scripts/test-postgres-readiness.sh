#!/usr/bin/env bash
# Cold-start regression for #174: across fresh volumes, the PostgreSQL
# healthcheck must never pass before the image's temporary init server has
# been replaced by the final server, and must pass once it has.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

cycles=${POSTGRES_READINESS_CYCLES:-3}
project="limiara-pg-readiness-${RANDOM:-0}-$$"
export POSTGRES_PORT=0
compose=(docker compose --project-name "$project")

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    "${compose[@]}" logs --no-color postgres || true
  fi
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

for cycle in $(seq 1 "$cycles"); do
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  "${compose[@]}" up --detach --no-deps postgres >/dev/null
  container_id=$("${compose[@]}" ps --quiet postgres)

  socket_ready_during_init=false
  healthy=false
  deadline=$((SECONDS + 120))
  while (( SECONDS < deadline )); do
    # PID 1 is the entrypoint script until it execs the final server.
    final_server=false
    if docker exec "$container_id" sh -c 'test "$(cat /proc/1/comm)" = postgres' 2>/dev/null; then
      final_server=true
    fi
    if ! $final_server && docker exec "$container_id" pg_isready --quiet --username platform_bootstrap --dbname platform 2>/dev/null; then
      socket_ready_during_init=true
    fi
    if docker exec "$container_id" bash /opt/platform/healthcheck.sh >/dev/null 2>&1; then
      # The healthcheck can only pass once the final server is up. If PID 1
      # was not yet postgres at the start of this iteration, recheck: the
      # entrypoint may have exec'd between the two probes.
      if ! $final_server && ! docker exec "$container_id" sh -c 'test "$(cat /proc/1/comm)" = postgres'; then
        printf 'cycle %s: healthcheck passed against the temporary init server\n' "$cycle" >&2
        exit 1
      fi
      healthy=true
      break
    fi
  done
  if ! $healthy; then
    printf 'cycle %s: PostgreSQL never became healthy\n' "$cycle" >&2
    exit 1
  fi

  # Dependants can connect as each runtime role over the network.
  for role in migrator app worker; do
    password_var="PLATFORM_${role^^}_PASSWORD"
    docker exec --env "PGPASSWORD=${!password_var:-platform_${role}}" "$container_id" \
      psql --host=127.0.0.1 --username="platform_${role}" --dbname=platform --no-psqlrc --tuples-only --command='SELECT 1' >/dev/null
  done

  printf 'cycle %s: healthy after init (socket probe ready during init: %s)\n' "$cycle" "$socket_ready_during_init"
done

# Warm restart on the existing volume skips init and still becomes healthy.
"${compose[@]}" restart postgres >/dev/null
container_id=$("${compose[@]}" ps --quiet postgres)
deadline=$((SECONDS + 60))
until docker exec "$container_id" bash /opt/platform/healthcheck.sh >/dev/null 2>&1; do
  (( SECONDS < deadline )) || { printf 'warm restart: PostgreSQL never became healthy\n' >&2; exit 1; }
  sleep 0.5
done
printf 'warm restart: healthy\n'
