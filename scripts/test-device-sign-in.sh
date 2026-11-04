#!/usr/bin/env bash
# ADR 0029 / #391: a device app's sign-in end to end against the local
# Keycloak realm. The API runs as built for production, Keycloak runs from
# docker-compose.yml, and Chromium plays the system browser while
# scripts/device-sign-in-browser.mjs plays the app on its loopback redirect.
#
# The realm's hostname is auth.localhost:8081 and its web client returns to
# http://localhost:8080, so both ports are fixed and auth.localhost must
# resolve to 127.0.0.1 for the API (CI adds it to /etc/hosts).
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

work_root="$repository_root/target/device-sign-in"
api_base="http://localhost:8080"
loopback_port="${PLATFORM_DEVICE_LOOPBACK_PORT:-18095}"
tenant="018f0000-0000-7000-8000-000000000001"
project="limiara-device-sign-in-${RANDOM:-0}-$$"
compose=(docker compose --project-name "$project")
api_pid=""
nats_started=false
postgres_started=false

log() { printf '== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
  status=$?
  trap - EXIT
  [[ -n "$api_pid" ]] && kill "$api_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  if (( status != 0 )); then
    [[ -f "$work_root/api.log" ]] && { printf '\n--- api.log (tail)\n' >&2; tail -n 60 "$work_root/api.log" >&2; }
    "${compose[@]}" logs --no-color --tail 60 keycloak >&2 || true
  fi
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  $nats_started && bash scripts/local-nats.sh stop
  $postgres_started && bash scripts/local-postgres.sh stop
  exit "$status"
}
trap cleanup EXIT

resolved=$(getent hosts auth.localhost | awk '{print $1}' | head -n 1)
[[ "$resolved" == 127.0.0.1 || "$resolved" == ::1 ]] ||
  fail "auth.localhost does not resolve to loopback; add '127.0.0.1 auth.localhost' to /etc/hosts"

rm -rf "$work_root"
mkdir -p "$work_root"

log "starting Keycloak"
KEYCLOAK_PORT=8081 "${compose[@]}" up --detach --wait keycloak

log "building api"
cargo build --locked -p api --bins
if [[ -z "${PLATFORM_TEST_NATS_URL:-}" ]]; then
  eval "$(bash scripts/local-nats.sh start)"
  nats_started=true
fi
eval "$(bash scripts/local-postgres.sh start)"
postgres_started=true

cat >"$work_root/config.yml" <<EOF
platform:
  devices:
    redirect_uris:
      - http://127.0.0.1:${loopback_port}/callback
EOF

env PLATFORM_DEFAULT_CONFIG_FILE="$repository_root/config/default.yml" \
  PLATFORM_CONFIG_FILE="$work_root/config.yml" \
  PLATFORM__PLATFORM__NATS__URL="$PLATFORM_TEST_NATS_URL" \
  PLATFORM__PLATFORM__DATABASE__URL="$PLATFORM_TEST_DATABASE_URL" \
  PLATFORM__PLATFORM__SERVICES__API_BIND="127.0.0.1:8080" \
  RUST_LOG="${RUST_LOG:-info}" \
  target/debug/api >>"$work_root/api.log" 2>&1 &
api_pid=$!

deadline=$((SECONDS + 60))
until [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/ready")" == 200 ]]; do
  (( SECONDS < deadline )) || fail "api did not become ready"
  sleep 0.3
done

API_BASE="$api_base" WORKSPACE_ID="$tenant" LOOPBACK_PORT="$loopback_port" \
  node scripts/device-sign-in-browser.mjs

# What the sign-ins left behind: one device for the member, none for the
# person outside the workspace, and no code of the member's left to spend.
sql() {
  psql "$PLATFORM_TEST_BOOTSTRAP_DATABASE_URL" --no-psqlrc --set=ON_ERROR_STOP=1 \
    --tuples-only --no-align --quiet "$@"
}
devices=$(sql --command="SELECT count(*) FROM devices WHERE tenant_id = '$tenant';")
[[ "$devices" == 1 ]] || fail "one device registered: $devices"
member_codes=$(sql --command="SELECT count(*) FROM device_authorizations WHERE user_id = '018f0000-0000-7000-8000-000000000101';")
[[ "$member_codes" == 0 ]] || fail "the member's codes are spent, the refused one too: $member_codes"
log "device sign-in end to end passed"
