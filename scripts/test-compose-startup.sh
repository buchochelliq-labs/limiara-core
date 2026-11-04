#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-compose-startup-${RANDOM:-0}-$$"
failure_project="${project}-failure"
export API_PORT=0 KEYCLOAK_PORT=0 NATS_PORT=0 POSTGRES_PORT=0 WEB_PORT=0
export LIMIARA_COMPOSE_PROJECT="$project"

cleanup_project() {
  local name=$1
  docker compose --project-name "$name" down --volumes --remove-orphans >/dev/null 2>&1 || true
}

cleanup() {
  status=$?
  trap - EXIT
  if (( status != 0 )); then
    docker compose --project-name "$project" ps --all || true
    docker compose --project-name "$project" logs --no-color || true
    for container_id in $(docker compose --project-name "$project" ps --all --quiet); do
      docker inspect --format '{{.Name}} {{json .State.Health}}' "$container_id" || true
    done
    docker compose --project-name "$failure_project" ps --all || true
    docker compose --project-name "$failure_project" logs --no-color || true
  fi
  cleanup_project "$project"
  cleanup_project "$failure_project"
  exit "$status"
}
trap cleanup EXIT

compose=(docker compose --project-name "$project")
failure_compose=(docker compose --project-name "$failure_project")

bash scripts/integration-stack.sh up

for service in postgres nats keycloak api worker plugin-runner web; do
  container_id=$("${compose[@]}" ps --quiet "$service")
  [[ -n "$container_id" ]]
  health=$(docker inspect --format '{{.State.Health.Status}}' "$container_id")
  [[ "$health" == "healthy" ]]
done

migration_status=$(docker inspect --format '{{.State.ExitCode}}' "$("${compose[@]}" ps --all --quiet migrate)")
[[ "$migration_status" == "0" ]]

ledger_count=$("${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM plugin_migration_ledger")

api_port=$("${compose[@]}" port api 8080 | awk -F: '{print $NF}')
api_base="http://127.0.0.1:${api_port}"
login_headers=$(curl --silent --dump-header - --output /dev/null "$api_base/auth/oidc/login")
printf '%s' "$login_headers" | grep -q '^HTTP/.* 307 '
printf '%s' "$login_headers" | grep -q 'code_challenge_method=S256'
printf '%s' "$login_headers" | grep -q 'state='
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/auth/oidc/callback?code=invalid&state=invalid")" == "401" ]]
member_token="compose-member-session-token"
member_hash=$(printf '%s' "$member_token" | sha256sum | awk '{print $1}')
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES ('018f0000-0000-7000-8000-000000000301', '$member_hash', '018f0000-0000-7000-8000-000000000101', now() + interval '1 hour')"

[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/api/v1/me")" == "401" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/me")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins")" == "200" ]]

# Plugin state is durable, tenant-scoped, idempotent, and bound to the digest
# verified from the package artifact rather than a manifest-only assertion.
plugin_route="$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins/platform.contacts"
for _ in 1 2; do
  [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$member_token" "$plugin_route/disable")" == "200" ]]
done
for _ in 1 2; do
  [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$member_token" "$plugin_route/enable")" == "200" ]]
done
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT state || ':' || active_version FROM tenant_plugin_installations WHERE tenant_id = '018f0000-0000-7000-8000-000000000001' AND plugin_id = 'platform.contacts'" |
  grep -qx 'enabled:0.1.0'
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT verified_digest FROM plugin_versions WHERE plugin_id = 'platform.contacts' AND version = '0.1.0'" |
  grep -qx 'sha256:8406662dc1ed4d4b13e054767c45ff8fed8740c9f032be0abf66c2672a178554'

persisted_person_id=$(bash scripts/test-contacts-api.sh \
  "$project" "$api_base" "$member_token" \
  '018f0000-0000-7000-8000-000000000001')

denied_token="compose-denied-session-token"
denied_hash=$(printf '%s' "$denied_token" | sha256sum | awk '{print $1}')
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="INSERT INTO users (id, display_name) VALUES ('018f0000-0000-7000-8000-000000000102', 'No Role'); INSERT INTO memberships (tenant_id, user_id) VALUES ('018f0000-0000-7000-8000-000000000001', '018f0000-0000-7000-8000-000000000102'); INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES ('018f0000-0000-7000-8000-000000000302', '$denied_hash', '018f0000-0000-7000-8000-000000000102', now() + interval '1 hour')"

# Disablement conceals every delivered Contacts surface without deleting the
# tenant-owned row. Re-enable must restore the same caller-authorized record.
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$member_token" "$plugin_route/disable")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people/$persisted_person_id")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$denied_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/odata/\$metadata")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/odata/contacts/people")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/pages/platform.contacts.people-directory")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/pages/platform.contacts.people-directory/customisation")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$member_token" --header 'content-type: application/json' --data '{' "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/pages/platform.contacts.people-directory/customisation")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/resources/platform.contacts.person")" == "404" ]]
disabled_navigation=$(curl --fail --silent --header "Cookie: platform_session=$member_token" \
  "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/contributions")
# Kernel engine permissions, object navigation and reports (ADR 0015,
# ADR 0016, ADR 0027) belong to the workspace, not the plugin, and stay.
kernel_contributions='const kernelNavigation=item=>item.id.startsWith("objects.")||item.id==="reports"; const kernelPermissions=["objects.manage","records.view_all","records.modify_all","records.transfer","reports.manage","members.manage"];'
node -e "$kernel_contributions"' const body=JSON.parse(process.argv[1]); if (!body.navigation.every(kernelNavigation) || !body.permissions.every(p => kernelPermissions.includes(p)) || !/^[0-9a-f]{64}(\.[0-9a-f]{16})?$/.test(body.revision)) process.exit(1)' "$disabled_navigation"
curl --silent --dump-header - --output /dev/null --header "Cookie: platform_session=$member_token" \
  "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/contributions" |
  tr -d '\r' |
  grep --ignore-case --quiet '^cache-control: private, no-store$'
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM p_contacts.people WHERE tenant_id = '018f0000-0000-7000-8000-000000000001' AND public_id = '$persisted_person_id'" |
  grep -qx 1
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$member_token" "$plugin_route/enable")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people/$persisted_person_id")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/odata/\$metadata")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/odata/contacts/people")" == "200" ]]
enabled_navigation=$(curl --fail --silent --header "Cookie: platform_session=$member_token" \
  "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/contributions")
node -e "$kernel_contributions"' const disabled=JSON.parse(process.argv[1]); const enabled=JSON.parse(process.argv[2]); const ids=enabled.navigation.filter(item => !kernelNavigation(item)).map(item => item.id).join(","); if (ids !== "contacts,contacts-organisations" || enabled.navigation.length <= disabled.navigation.length || enabled.navigation.some(item => "permission" in item) || !enabled.permissions.includes("contacts.people.read") || !enabled.permissions.includes("contacts.organisations.read") || enabled.revision === disabled.revision) process.exit(1)' "$disabled_navigation" "$enabled_navigation"

[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST --header "Cookie: platform_session=$denied_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins/platform.contacts/enable")" == "403" ]]

nonmember_token="compose-nonmember-session-token"
nonmember_hash=$(printf '%s' "$nonmember_token" | sha256sum | awk '{print $1}')
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="INSERT INTO users (id, display_name) VALUES ('018f0000-0000-7000-8000-000000000103', 'Nonmember'); INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES ('018f0000-0000-7000-8000-000000000303', '$nonmember_hash', '018f0000-0000-7000-8000-000000000103', now() + interval '1 hour')"
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$nonmember_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins")" == "404" ]]

# Liveness remains local while readiness follows a recoverable dependency.
"${compose[@]}" stop nats
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/live")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/ready")" == "503" ]]

# A worker that starts while NATS is unavailable must still expose its local
# operational surface and reconnect without an external process restart.
"${compose[@]}" restart worker
worker_live=""
for _ in $(seq 1 30); do
  if worker_live=$("${compose[@]}" exec --no-TTY worker \
    curl --silent --output /dev/null --write-out '%{http_code}' \
    http://127.0.0.1:8080/health/live); then
    [[ "$worker_live" == "200" ]] && break
  fi
  sleep 1
done
[[ "$worker_live" == "200" ]]
worker_container_id=$("${compose[@]}" ps --quiet worker)
worker_restart_count=$(docker inspect --format '{{.RestartCount}}' "$worker_container_id")
worker_pid=$(docker inspect --format '{{.State.Pid}}' "$worker_container_id")
[[ "$("${compose[@]}" exec --no-TTY worker \
  curl --silent --output /dev/null --write-out '%{http_code}' \
  http://127.0.0.1:8080/health/ready)" == "503" ]]
"${compose[@]}" exec --no-TTY worker \
  curl --fail --silent http://127.0.0.1:8080/metrics |
  grep --quiet '^limiara_outbox_backlog '

"${compose[@]}" rm --force nats
"${compose[@]}" up --detach --wait --wait-timeout 60 nats
for _ in $(seq 1 30); do
  [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/ready")" == "200" ]] && break
  sleep 2
done
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/ready")" == "200" ]]
worker_ready=""
for _ in $(seq 1 30); do
  worker_ready=$("${compose[@]}" exec --no-TTY worker \
    curl --silent --output /dev/null --write-out '%{http_code}' \
    http://127.0.0.1:8080/health/ready)
  [[ "$worker_ready" == "200" ]] && break
  sleep 2
done
[[ "$worker_ready" == "200" ]]
[[ "$("${compose[@]}" ps --quiet worker)" == "$worker_container_id" ]]
[[ "$(docker inspect --format '{{.RestartCount}}' "$worker_container_id")" == "$worker_restart_count" ]]
[[ "$(docker inspect --format '{{.State.Pid}}' "$worker_container_id")" == "$worker_pid" ]]

# Simulate a process interruption between intent and completion. Startup must
# converge conservatively without re-running candidate registration work.
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="UPDATE tenant_plugin_installations SET state = 'disabling', previous_state = 'enabled', transition_id = '018f0000-0000-7000-8000-000000000401', transition_started_at = now() WHERE tenant_id = '018f0000-0000-7000-8000-000000000001' AND plugin_id = 'platform.contacts'"
"${compose[@]}" restart api worker plugin-runner web
"${compose[@]}" up --detach --wait --wait-timeout 90 api worker plugin-runner web
api_port=$("${compose[@]}" port api 8080 | awk -F: '{print $NF}')
api_base="http://127.0.0.1:${api_port}"

# The same opaque session survives an API restart until expiry or revocation.
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/me")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people")" == "200" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people/$persisted_person_id")" == "200" ]]
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM audit_log WHERE tenant_id = '018f0000-0000-7000-8000-000000000001' AND actor_id = '018f0000-0000-7000-8000-000000000101' AND action = 'plugin.transition.attempted'" |
  grep -qx 6
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM audit_log WHERE tenant_id = '018f0000-0000-7000-8000-000000000001' AND action = 'plugin.transition.recovered' AND metadata->>'recovered_state' = 'enabled'" |
  grep -qx 1

# The web origin serves the shell for deep links and proxies the API
# same-origin, so the browser's relative /api/v1 calls reach the API — also
# after the API restart above.
web_port=$("${compose[@]}" port web 8080 | awk -F: '{print $NF}')
web_base="http://127.0.0.1:${web_port}"
for route in / /contacts "/contacts/people/$persisted_person_id"; do
  [[ "$(curl --silent --output /dev/null --write-out '%{http_code} %{content_type}' "$web_base$route")" == "200 text/html" ]]
done
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$web_base/assets/missing.js")" == "404" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$web_base/api/v1/me")" == "401" ]]
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$web_base/api/v1/me")" == "200" ]]
curl --fail --silent --header "Cookie: platform_session=$member_token" \
  "$web_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/ui/contributions" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{if(!JSON.parse(s).navigation.some(n=>n.id==="contacts"))process.exit(1)})'
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$web_base/auth/oidc/callback?code=invalid&state=invalid")" == "401" ]]
curl --silent --dump-header - --output /dev/null --header "x-request-id: 018f0000-0000-7000-8000-0000000004aa" \
  --header "Cookie: platform_session=$member_token" "$web_base/api/v1/me" |
  tr -d '\r' | grep --ignore-case --quiet '^x-request-id: 018f0000-0000-7000-8000-0000000004aa$'

"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM audit_log WHERE metadata::text ~* '(password|token|secret)'" |
  grep -qx 0
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="UPDATE sessions SET revoked_at = now() WHERE token_hash = '$member_hash'"
[[ "$(curl --silent --output /dev/null --write-out '%{http_code}' --header "Cookie: platform_session=$member_token" "$api_base/api/v1/me")" == "401" ]]

restarted_ledger_count=$("${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --tuples-only --no-align \
  --command="SELECT count(*) FROM plugin_migration_ledger")
[[ "$restarted_ledger_count" == "$ledger_count" ]]

# A required one-shot dependency must fail visibly and prevent the API from
# entering a false-ready state.
set +e
PLATFORM_ENABLED_PLUGINS=missing "${failure_compose[@]}" up \
  --detach --wait --wait-timeout 120 api >/dev/null 2>&1
failure_status=$?
set -e
[[ $failure_status -ne 0 ]]

failed_migration_id=$("${failure_compose[@]}" ps --all --quiet migrate)
[[ -n "$failed_migration_id" ]]
[[ "$(docker inspect --format '{{.State.ExitCode}}' "$failed_migration_id")" != "0" ]]

failed_api_id=$("${failure_compose[@]}" ps --all --quiet api)
if [[ -n "$failed_api_id" ]]; then
  [[ "$(docker inspect --format '{{.State.Running}}' "$failed_api_id")" == "false" ]]
fi
