#!/usr/bin/env bash
# Reports healthy only once first-run initialization has finished and the
# final server accepts TCP connections with the platform roles in place.
#
# The official image runs its init scripts against a temporary server that
# listens on the Unix socket only, so a socket probe can pass mid-init and
# let dependants connect to a server that is about to stop (#174). Probing
# over TCP cannot reach that temporary server.
set -euo pipefail

: "${POSTGRES_USER:?POSTGRES_USER is required}"
: "${POSTGRES_DB:?POSTGRES_DB is required}"

roles=$(PGPASSWORD="${POSTGRES_PASSWORD:-}" PGCONNECT_TIMEOUT=2 psql \
  --host=127.0.0.1 \
  --username="$POSTGRES_USER" \
  --dbname="$POSTGRES_DB" \
  --no-psqlrc --tuples-only --no-align \
  --command="SELECT count(*) FROM pg_roles WHERE rolname IN ('platform_migrator', 'platform_app', 'platform_worker')")
[[ "$roles" == 3 ]]
