#!/usr/bin/env bash
# #385: the app's sign-in lifecycle end to end. The headless probe
# (crates/platform-device/examples/sign_in_probe.rs) runs the same
# controller as the device shell against the API binary and the local
# Keycloak realm, with a loopback redirect as a desktop app has.
# scripts/device-app-sign-in-driver.mjs signs the person in with Chromium
# and revokes the device from the web.
#
# The realm's hostname is auth.localhost:8081 and its web client returns to
# http://localhost:8080, so both ports are fixed and auth.localhost must
# resolve to 127.0.0.1 for the API (CI adds it to /etc/hosts).
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

work_root="$repository_root/target/device-app-sign-in"
api_base="http://localhost:8080"
loopback_port="${PLATFORM_DEVICE_APP_LOOPBACK_PORT:-18096}"
tenant="018f0000-0000-7000-8000-000000000001"
member="018f0000-0000-7000-8000-000000000101"
member_session="device-app-sign-in-member"
project="limiara-device-app-${RANDOM:-0}-$$"
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
mkdir -p "$work_root/data"

log "starting Keycloak"
KEYCLOAK_PORT=8081 "${compose[@]}" up --detach --wait keycloak

log "building the api and the app probe"
cargo build --locked -p api --bins
cargo build --locked -p platform-device --example sign_in_probe
if [[ -z "${PLATFORM_TEST_NATS_URL:-}" ]]; then
  eval "$(bash scripts/local-nats.sh start)"
  nats_started=true
fi
eval "$(bash scripts/local-postgres.sh start)"
postgres_started=true

sql() {
  psql "$PLATFORM_TEST_BOOTSTRAP_DATABASE_URL" --no-psqlrc --set=ON_ERROR_STOP=1 \
    --tuples-only --no-align --quiet "$@"
}
# The member's browser session on the web, to revoke the device with.
session_hash=$(printf '%s' "$member_session" | sha256sum | awk '{print $1}')
sql --command="INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES
  ('018f0000-0000-7000-8000-000000000621', '$session_hash', '$member', now() + interval '2 hours');"

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

PROBE="$repository_root/target/debug/examples/sign_in_probe" \
  LIMIARA_ADDRESS="$api_base/workspaces/$tenant" \
  REDIRECT_URI="http://127.0.0.1:${loopback_port}/callback" \
  DATA_DIR="$work_root/data" \
  API_BASE="$api_base" WORKSPACE_ID="$tenant" MEMBER_SESSION="$member_session" \
  DATABASE_URL="$PLATFORM_TEST_BOOTSTRAP_DATABASE_URL" \
  node scripts/device-app-sign-in-driver.mjs

# The server's side: the first device was revoked, the second signed out.
reasons=$(sql --command="SELECT string_agg(revoked_reason, ',' ORDER BY created_at) FROM devices WHERE tenant_id = '$tenant' AND name = 'Probe laptop';")
[[ "$reasons" == "revoked,signed_out" ]] || fail "device reasons: $reasons"
log "device app sign-in end to end passed"
