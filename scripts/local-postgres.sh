#!/usr/bin/env bash
# Start a disposable PostgreSQL cluster without Docker, provision the platform
# roles, and apply core plus enabled-plugin migrations with the production
# provisioning and migration scripts.
#
#   eval "$(bash scripts/local-postgres.sh start)"   # exports PLATFORM_TEST_* URLs
#   bash scripts/local-postgres.sh halt|resume       # outage without data loss
#   bash scripts/local-postgres.sh stop
#
# Intended for environments where the Compose stack is unavailable. The
# cluster lives under target/local-postgres and is safe to delete.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data_root="${PLATFORM_LOCAL_POSTGRES_ROOT:-$repository_root/target/local-postgres}"
port="${PLATFORM_LOCAL_POSTGRES_PORT:-15432}"
bin_dir="${PG_BIN_DIR:-$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -n 1)}"
[[ -x "$bin_dir/initdb" ]] || { echo "PostgreSQL server binaries not found; set PG_BIN_DIR" >&2; exit 1; }

as_postgres() {
  if [[ "$(id -u)" == 0 ]]; then
    runuser -u postgres -- "$@"
  else
    "$@"
  fi
}

case "${1:-}" in
  start)
    rm -rf "$data_root"
    mkdir -p "$data_root"
    [[ "$(id -u)" == 0 ]] && chown postgres "$data_root"
    as_postgres "$bin_dir/initdb" --pgdata="$data_root/data" --username=platform_bootstrap \
      --auth=trust --encoding=UTF8 >/dev/null
    as_postgres "$bin_dir/pg_ctl" --pgdata="$data_root/data" --log="$data_root/postgres.log" \
      --options="-p $port -k $data_root -c listen_addresses=127.0.0.1" --wait start >/dev/null
    export PGHOST=127.0.0.1 PGPORT="$port"
    psql --username=platform_bootstrap --dbname=postgres --quiet --command="CREATE DATABASE platform" >/dev/null
    POSTGRES_USER=platform_bootstrap POSTGRES_DB=platform \
      PLATFORM_MIGRATOR_PASSWORD=platform_migrator PLATFORM_APP_PASSWORD=platform_app \
      PLATFORM_WORKER_PASSWORD=platform_worker \
      bash "$repository_root/infra/postgres/init/010_roles.sh" >/dev/null
    PGDATABASE=platform PGUSER=platform_migrator PGPASSWORD=platform_migrator \
      PLATFORM_MIGRATIONS_ROOT="$repository_root" \
      sh "$repository_root/infra/postgres/run-migrations.sh" >&2
    cat <<EOF
export PLATFORM_TEST_DATABASE_URL=postgres://platform_app:platform_app@127.0.0.1:$port/platform
export PLATFORM_TEST_WORKER_DATABASE_URL=postgres://platform_worker:platform_worker@127.0.0.1:$port/platform
export PLATFORM_TEST_MIGRATOR_DATABASE_URL=postgres://platform_migrator:platform_migrator@127.0.0.1:$port/platform
export PLATFORM_TEST_BOOTSTRAP_DATABASE_URL=postgres://platform_bootstrap@127.0.0.1:$port/platform
EOF
    ;;
  halt)
    # Stop the server but keep the cluster, to exercise database outages.
    as_postgres "$bin_dir/pg_ctl" --pgdata="$data_root/data" --mode=fast --wait stop >/dev/null
    ;;
  resume)
    as_postgres "$bin_dir/pg_ctl" --pgdata="$data_root/data" --log="$data_root/postgres.log" \
      --options="-p $port -k $data_root -c listen_addresses=127.0.0.1" --wait start >/dev/null
    ;;
  stop)
    if [[ -d "$data_root/data" ]]; then
      as_postgres "$bin_dir/pg_ctl" --pgdata="$data_root/data" --mode=fast stop >/dev/null || true
    fi
    rm -rf "$data_root"
    ;;
  *)
    echo "usage: $0 start|halt|resume|stop" >&2
    exit 2
    ;;
esac
