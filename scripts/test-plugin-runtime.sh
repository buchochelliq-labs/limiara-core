#!/usr/bin/env bash
# Prove plugin consumers and jobs follow tenant plugin lifecycle against real
# PostgreSQL roles, forced RLS, the production migrations, and JetStream.
#
# PostgreSQL runs as a disposable local cluster (scripts/local-postgres.sh);
# set PG_BIN_DIR if the server binaries are not under /usr/lib/postgresql/*/bin.
# NATS comes from PLATFORM_TEST_NATS_URL, else scripts/local-nats.sh (a local
# or pinned, checksum-verified binary; no registry pull limits). A missing
# broker is a failure, never a skipped check.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

nats_started=false

cleanup() {
  status=$?
  trap - EXIT
  $nats_started && bash scripts/local-nats.sh stop
  bash scripts/local-postgres.sh stop
  exit "$status"
}
trap cleanup EXIT

if [[ -z "${PLATFORM_TEST_NATS_URL:-}" ]]; then
  nats_started=true
  eval "$(bash scripts/local-nats.sh start)"
fi

eval "$(bash scripts/local-postgres.sh start)"
cargo test -p worker --locked --test plugin_runtime_postgres --test contacts_worker_postgres --test extension_retention_postgres --test automation_jobs_postgres --test bulk_jobs_postgres --test device_notifications_postgres -- --ignored --test-threads=1 --nocapture
cargo test -p platform-db --locked --test identity_postgres -- --ignored --test-threads=1 --nocapture
cargo test -p contacts-plugin --locked --test extensions_postgres -- --ignored --test-threads=1 --nocapture
cargo test -p platform-objects --locked --test store_postgres -- --ignored --test-threads=1 --nocapture

printf 'plugin runtime acceptance passed for enable, disable, drain, re-enable, restart, lease, broker, outage, extension-host, extension-retention, automation-job and object-engine behaviour\n'
