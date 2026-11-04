#!/usr/bin/env bash
# #190: one isolated acceptance journey proving a tenant plugin lifecycle
# transition takes effect across every Contacts contribution surface — API,
# OData, UI delivery, the web people workflows, the event consumer, and the
# scheduled job — while preserving tenant data and processing ledgers.
#
# The production `api` and `worker` binaries run against a disposable
# PostgreSQL cluster (scripts/local-postgres.sh, production roles and
# migrations) and JetStream (scripts/local-nats.sh). Lifecycle changes go
# through the public API only. The test touches the database directly only to
# seed sessions, plant expired list cursors, advance the job clock, and read
# evidence.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

work_root="$repository_root/target/lifecycle-acceptance"
api_port="${PLATFORM_ACCEPTANCE_API_PORT:-58080}"
worker_port="${PLATFORM_ACCEPTANCE_WORKER_PORT:-58081}"
tenant="018f0000-0000-7000-8000-000000000001"
member="018f0000-0000-7000-8000-000000000101"
viewer="018f0000-0000-7000-8000-000000000102"
plugin="platform.contacts"
consumer="platform.contacts.activity-timeline-v1"
job="platform.contacts.list-cursor-cleanup-v1"
api_base="http://127.0.0.1:${api_port}"
workspace="$api_base/api/v1/workspaces/$tenant"
member_token="lifecycle-acceptance-member"
viewer_token="lifecycle-acceptance-viewer"
api_pid=""
worker_pid=""
nats_started=false
postgres_started=false

log() { printf '== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
  status=$?
  trap - EXIT
  [[ -n "$worker_pid" ]] && kill -CONT "$worker_pid" 2>/dev/null || true
  [[ -n "$api_pid" ]] && kill "$api_pid" 2>/dev/null || true
  [[ -n "$worker_pid" ]] && kill "$worker_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  if (( status != 0 )); then
    for file in "$work_root"/*.log; do
      [[ -f "$file" ]] || continue
      printf '\n--- %s (tail)\n' "$file" >&2
      tail -n 60 "$file" >&2
    done
  fi
  $nats_started && bash scripts/local-nats.sh stop
  $postgres_started && bash scripts/local-postgres.sh stop
  exit "$status"
}
trap cleanup EXIT

# --- fixtures ---------------------------------------------------------------

rm -rf "$work_root"
mkdir -p "$work_root"
log "building api, worker, and the web shell"
cargo build --locked -p api -p worker --bins
pnpm --filter web build >"$work_root/web-build.log" 2>&1 || { cat "$work_root/web-build.log" >&2; fail "web build failed"; }
eval "$(bash scripts/local-nats.sh start)"
nats_started=true
eval "$(bash scripts/local-postgres.sh start)"
postgres_started=true

sql() {
  psql "$PLATFORM_TEST_BOOTSTRAP_DATABASE_URL" --no-psqlrc --set=ON_ERROR_STOP=1 \
    --tuples-only --no-align --quiet "$@"
}

hash_token() { printf '%s' "$1" | sha256sum | awk '{print $1}'; }

# The seeded member holds every Contacts and plugin-management permission; the
# viewer is a member of the same workspace with no role.
sql --command="
  INSERT INTO sessions (id, token_hash, user_id, expires_at)
  VALUES ('018f0000-0000-7000-8000-000000000501', '$(hash_token "$member_token")', '$member', now() + interval '2 hours');
  INSERT INTO users (id, display_name) VALUES ('$viewer', 'Viewer');
  INSERT INTO memberships (tenant_id, user_id) VALUES ('$tenant', '$viewer');
  INSERT INTO sessions (id, token_hash, user_id, expires_at)
  VALUES ('018f0000-0000-7000-8000-000000000502', '$(hash_token "$viewer_token")', '$viewer', now() + interval '2 hours');"

service_env=(
  PLATFORM_DEFAULT_CONFIG_FILE="$repository_root/config/default.yml"
  PLATFORM__PLATFORM__NATS__URL="$PLATFORM_TEST_NATS_URL"
  PLATFORM__PLATFORM__WORKER__TENANT_DATABASE_URL="$PLATFORM_TEST_DATABASE_URL"
  PLATFORM__PLATFORM__WORKER__RECONCILE_INTERVAL_MS=300
  PLATFORM__PLATFORM__WORKER__JOB_POLL_INTERVAL_MS=200
  PLATFORM__PLATFORM__WORKER__CONSUMER_RETRY_DELAY_MS=200
  PLATFORM__PLATFORM__NATS__OUTBOX__POLL_INTERVAL_MS=100
  PLATFORM__PLATFORM__SERVICES__API_BIND="127.0.0.1:$api_port"
  PLATFORM__PLATFORM__SERVICES__WORKER_BIND="127.0.0.1:$worker_port"
  RUST_LOG="${RUST_LOG:-info}"
)

start_api() {
  env "${service_env[@]}" PLATFORM__PLATFORM__DATABASE__URL="$PLATFORM_TEST_DATABASE_URL" \
    target/debug/api >>"$work_root/api.log" 2>&1 &
  api_pid=$!
}

start_worker() {
  env "${service_env[@]}" PLATFORM__PLATFORM__DATABASE__URL="$PLATFORM_TEST_WORKER_DATABASE_URL" \
    target/debug/worker >>"$work_root/worker.log" 2>&1 &
  worker_pid=$!
}

stop_worker() {
  kill "$worker_pid"
  wait "$worker_pid" 2>/dev/null || true
  worker_pid=""
}

# wait_until <seconds> <description> <command...>
wait_until() {
  local seconds=$1 description=$2
  shift 2
  local deadline=$((SECONDS + seconds))
  until "$@" >/dev/null 2>&1; do
    (( SECONDS < deadline )) || fail "timed out waiting for $description"
    sleep 0.2
  done
}

status_of() { curl --max-time 20 --silent --output /dev/null --write-out '%{http_code}' "$@"; }
http_is() { [[ "$(status_of "${@:2}")" == "$1" ]]; }
api_ready() { http_is 200 "$api_base/health/ready"; }
worker_ready() { http_is 200 "http://127.0.0.1:$worker_port/health/ready"; }
metric() {
  curl --max-time 20 --fail --silent "http://127.0.0.1:$worker_port/metrics" |
    awk -v name="$1" '$1 == name {print $2}'
}

member_cookie="Cookie: platform_session=$member_token"
viewer_cookie="Cookie: platform_session=$viewer_token"

# expect <status> <curl args...>: assert a member request's status.
expect() {
  local expected=$1
  shift
  local actual
  actual=$(status_of --header "$member_cookie" "$@")
  [[ "$actual" == "$expected" ]] || fail "expected $expected, got $actual: $*"
}

# lifecycle <enable|disable> <expected status> <correlation id> [cookie]
lifecycle() {
  local action=$1 expected=$2 correlation=$3 cookie=${4:-$member_cookie}
  local actual
  actual=$(status_of --request POST --header "$cookie" --header "x-request-id: $correlation" \
    "$workspace/plugins/$plugin/$action")
  [[ "$actual" == "$expected" ]] || fail "$action expected $expected, got $actual"
}

create_person() {
  local name=$1 correlation=$2
  curl --max-time 20 --fail --silent --request POST --header "$member_cookie" \
    --header 'Content-Type: application/json' --header "x-request-id: $correlation" \
    --data "{\"full_name\":\"$name\",\"email\":\"$(tr 'A-Z ' 'a-z.' <<<"$name")@example.test\"}" \
    "$workspace/contacts/people" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const id=JSON.parse(s).id;if(!/^per/.test(id))process.exit(1);process.stdout.write(id)})'
}

activity_count() {
  curl --max-time 20 --fail --silent --header "$member_cookie" "$workspace/contacts/people/$1/activity" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(String(JSON.parse(s).data.length)))'
}

activity_is() { [[ "$(activity_count "$1")" == "$2" ]]; }

# The browser layer: the production web build, same-origin with the API.
shell_is() {
  API_BASE="$api_base" SESSION_TOKEN="$member_token" node tests/lifecycle-acceptance/shell.mjs "$1" ||
    fail "the web shell does not reflect the $1 state"
}

plant_expired_cursor() {
  local seed=$1
  sql --command="
    INSERT INTO p_contacts.list_cursors (token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at)
    VALUES (encode(sha256('cursor-$seed'::bytea), 'hex'), '$tenant', 'people',
            encode(sha256('query-$seed'::bytea), 'hex'), 'x', gen_random_uuid(), now() - interval '1 hour')"
}

cursor_exists() {
  [[ "$(sql --command="SELECT count(*) FROM p_contacts.list_cursors WHERE token_hash = encode(sha256('cursor-$1'::bytea), 'hex')")" == "$2" ]]
}

job_runs() {
  sql --command="SELECT run_count FROM plugin_job_schedules WHERE tenant_id = '$tenant' AND plugin_id = '$plugin' AND job_id = '$job'"
}

job_runs_are() { [[ "$(job_runs)" == "$1" ]]; }

# Advance the schedule's clock instead of waiting out its hourly interval.
make_job_due() {
  sql --command="UPDATE plugin_job_schedules SET next_run_at = now() WHERE tenant_id = '$tenant' AND plugin_id = '$plugin' AND job_id = '$job'"
}

installation_state() {
  sql --command="SELECT state FROM tenant_plugin_installations WHERE tenant_id = '$tenant' AND plugin_id = '$plugin'"
}

directory="$workspace/ui/pages/platform.contacts.people-directory"

customisation_field() {
  curl --max-time 20 --fail --silent --header "$member_cookie" "$directory/customisation" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const b=JSON.parse(s).state;process.stdout.write(String(process.argv[1]==="revision"?b.revision:b.draft?.version))})' "$1"
}

customise() {
  local body=$1 expected=${2:-200}
  local actual
  actual=$(status_of --request POST --header "$member_cookie" --header 'Content-Type: application/json' \
    --data "$body" "$directory/customisation")
  [[ "$actual" == "$expected" ]] || fail "customisation $body: expected $expected, got $actual"
}

delivered_title() {
  curl --max-time 20 --fail --silent --header "$member_cookie" "$directory" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).page.root.props.title))'
}

# A semantic digest of tenant data and processing ledgers: Contacts records,
# timeline, inbox markers, and the job's checkpoint and run history.
ledger_digest() {
  sql --command="
    SELECT md5(string_agg(row, E'\n' ORDER BY row)) FROM (
      SELECT 'person|' || public_id || '|' || version || '|' || full_name AS row FROM p_contacts.people WHERE tenant_id = '$tenant'
      UNION ALL SELECT 'activity|' || event_id || '|' || entity_public_id || '|' || action || '|' || entity_version FROM p_contacts.activity WHERE tenant_id = '$tenant'
      UNION ALL SELECT 'inbox|' || consumer_id || '|' || event_id FROM inbox WHERE tenant_id = '$tenant'
      UNION ALL SELECT 'job|' || job_id || '|' || run_count || '|' || checkpoint::text || '|' || coalesce(last_succeeded_at::text, '') FROM plugin_job_schedules WHERE tenant_id = '$tenant'
    ) AS ledger"
}

audit_rows() {
  sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND correlation_id = '$1' AND target = 'plugin:$plugin' AND action = '$2' $3"
}

expect_audit() {
  local correlation=$1 action=$2 filter=${3:-}
  [[ "$(audit_rows "$correlation" "$action" "$filter")" == 1 ]] ||
    fail "missing audit $action for correlation $correlation $filter"
}

# Every Contacts surface a disabled workspace must conceal.
surfaces_concealed() {
  local person=$1
  expect 404 "$workspace/contacts/people"
  expect 404 "$workspace/contacts/people/$person"
  expect 404 "$workspace/contacts/people/$person/activity"
  expect 404 "$workspace/contacts/organisations"
  expect 404 "$workspace/odata/\$metadata"
  expect 404 "$workspace/odata/contacts/people"
  expect 404 "$workspace/ui/pages/platform.contacts.people-directory"
  expect 404 "$workspace/ui/resources/platform.contacts.person"
  # Kernel engine permissions, object navigation and reports (ADR 0015,
  # ADR 0016, ADR 0027) belong to the workspace, not the plugin, and stay
  # advertised.
  # Non-enumerating: an existing record and a never-issued identifier are
  # indistinguishable while the plugin is disabled.
  local existing missing
  existing=$(curl --max-time 20 --silent --header "$member_cookie" "$workspace/contacts/people/$person")
  missing=$(curl --max-time 20 --silent --header "$member_cookie" "$workspace/contacts/people/per000000000000000")
  [[ "$existing" == "$missing" ]] || fail "disabled responses differ for existing and missing records"
  curl --max-time 20 --fail --silent --header "$member_cookie" "$workspace/ui/contributions" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const b=JSON.parse(s);const kernel=["objects.manage","records.view_all","records.modify_all","records.transfer","reports.manage","members.manage"];if(b.navigation.some(item=>!item.id.startsWith("objects.")&&item.id!=="reports")||b.permissions.some(p=>!kernel.includes(p)))process.exit(1)})' ||
    fail "disabled workspace still advertises the plugin's navigation or permissions"
}

surfaces_available() {
  local person=$1
  expect 200 "$workspace/contacts/people"
  expect 200 "$workspace/contacts/people/$person"
  expect 200 "$workspace/contacts/people/$person/activity"
  expect 200 "$workspace/odata/\$metadata"
  expect 200 "$workspace/odata/contacts/people"
  # Page and resource metadata are delivered from the durable snapshot: the
  # linked Contacts UI, filtered for the caller, with no permission names.
  local page etag
  page=$(curl --max-time 20 --fail --silent --header "$member_cookie" \
    "$workspace/ui/pages/platform.contacts.people-directory") ||
    fail "the enabled people directory page is not delivered"
  node -e 'const b=JSON.parse(process.argv[1]);if(b.page?.page_id!=="platform.contacts.people-directory"||JSON.stringify(b).includes("contacts.people.read"))process.exit(1)' "$page" ||
    fail "the delivered page is not the caller-safe people directory"
  etag=$(curl --max-time 20 --silent --dump-header - --output /dev/null --header "$member_cookie" \
    "$workspace/ui/pages/platform.contacts.people-directory" | tr -d '\r' | awk 'tolower($1)=="etag:"{print $2}')
  [[ -n "$etag" ]] || fail "page delivery returned no ETag"
  expect 304 --header "If-None-Match: $etag" "$workspace/ui/pages/platform.contacts.people-directory"
  expect 200 "$workspace/ui/resources/platform.contacts.person"
  # Customisation resolves against the same durable snapshot.
  expect 200 "$workspace/ui/pages/platform.contacts.people-directory/customisation/preview?view=published"
  curl --max-time 20 --fail --silent --header "$member_cookie" "$workspace/ui/contributions" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const b=JSON.parse(s);if(!b.navigation.some(n=>n.id==="contacts")||!b.permissions.includes("contacts.people.read"))process.exit(1)})' ||
    fail "enabled workspace does not advertise Contacts navigation"
}

# --- journey ----------------------------------------------------------------

log "starting api and worker"
start_api
start_worker
wait_until 120 "api readiness" api_ready
wait_until 120 "worker readiness" worker_ready

log "enable: accepted, then repeated idempotently"
lifecycle enable 200 018f1900-0000-7000-8000-000000000001
lifecycle enable 200 018f1900-0000-7000-8000-000000000002
[[ "$(installation_state)" == enabled ]] || fail "plugin not enabled"
expect_audit 018f1900-0000-7000-8000-000000000001 plugin.transition.attempted
expect_audit 018f1900-0000-7000-8000-000000000002 plugin.transition.succeeded "AND metadata->>'idempotent' = 'true'"

log "enabled: API, OData, and UI navigation are available"
ada=$(create_person "Ada Lovelace" 018f1900-0000-7000-8000-000000000011)
surfaces_available "$ada"
shell_is enabled

log "enabled: a published customisation reaches page delivery"
[[ "$(delivered_title)" == People ]] || fail "unexpected default people directory title"
customise "{\"expected_revision\":$(customisation_field revision),\"operation\":\"begin-draft\"}"
customise "{\"expected_revision\":$(customisation_field revision),\"operation\":\"update-draft\",\"expected_draft_version\":$(customisation_field draft),\"patches\":[{\"id\":\"rename-title\",\"publication_version\":1,\"target_page\":\"platform.contacts.people-directory\",\"compatible_versions\":{\"min_inclusive\":1,\"max_exclusive\":2},\"target_node\":\"people-page\",\"operation\":\"configure\",\"props\":{\"title\":\"Clients\"}}]}"
customise "{\"expected_revision\":$(customisation_field revision),\"operation\":\"publish\",\"expected_draft_version\":$(customisation_field draft)}"
[[ "$(delivered_title)" == Clients ]] || fail "the published customisation was not delivered"

log "enabled: the consumer records the timeline entry exactly once"
wait_until 60 "Ada's timeline entry" activity_is "$ada" 1

log "enabled: the job runs and removes an expired list cursor"
wait_until 60 "the first scheduled run" job_runs_are 1
plant_expired_cursor before-disable
make_job_due
wait_until 60 "the cleanup job run" job_runs_are 2
cursor_exists before-disable 0 || fail "the enabled job did not remove the expired cursor"

log "rejected: a member without plugin management cannot disable"
lifecycle disable 403 018f1900-0000-7000-8000-000000000021 "$viewer_cookie"
expect_audit 018f1900-0000-7000-8000-000000000021 plugin.transition.rejected \
  "AND actor_id = '$viewer' AND metadata = '{\"control\": \"auth.permission_denied\", \"desired_state\": \"disabled\"}'::jsonb"
[[ "$(installation_state)" == enabled ]] || fail "a rejected request changed the lifecycle state"

log "in flight: an event committed before disable settles without effects"
# Read the worker's counter before pausing it: a stopped process cannot answer.
skipped_before=$(metric limiara_plugin_deliveries_skipped_inactive_total)
kill -STOP "$worker_pid"
grace=$(create_person "Grace Hopper" 018f1900-0000-7000-8000-000000000012)
lifecycle disable 200 018f1900-0000-7000-8000-000000000003
expect_audit 018f1900-0000-7000-8000-000000000003 plugin.transition.succeeded "AND metadata->>'state' = 'disabled'"
kill -CONT "$worker_pid"
grace_event=$(sql --command="SELECT id FROM outbox WHERE tenant_id = '$tenant' AND envelope->>'correlation_id' = '018f1900-0000-7000-8000-000000000012'")
[[ -n "$grace_event" ]] || fail "Grace's event was not committed to the outbox"
wait_until 60 "Grace's event to publish" bash -c "[[ \"\$(psql '$PLATFORM_TEST_BOOTSTRAP_DATABASE_URL' -tAc \"SELECT published_at IS NOT NULL FROM outbox WHERE id = '$grace_event'\")\" == t ]]"
skipped_after_ok() { (( $(metric limiara_plugin_deliveries_skipped_inactive_total) > ${skipped_before:-0} )); }
wait_until 60 "the inactive delivery to be skipped" skipped_after_ok
[[ "$(sql --command="SELECT count(*) FROM inbox WHERE tenant_id = '$tenant' AND event_id = '$grace_event'")" == 0 ]] ||
  fail "a skipped delivery left an inbox marker"
[[ "$(sql --command="SELECT count(*) FROM p_contacts.activity WHERE tenant_id = '$tenant' AND event_id = '$grace_event'")" == 0 ]] ||
  fail "a delivery for a disabled plugin produced an effect"

log "disabled: every surface is concealed and non-enumerating"
surfaces_concealed "$ada"
shell_is disabled
expect 404 "$directory/customisation"
expect 404 "$directory/customisation/preview?view=published"
disabled_digest=$(ledger_digest)

log "disabled: due job work does not run"
plant_expired_cursor while-disabled
make_job_due
runs_while_disabled=$(job_runs)
sleep 3
job_runs_are "$runs_while_disabled" || fail "the job ran while the plugin was disabled"
cursor_exists while-disabled 1 || fail "the disabled job removed a cursor"

log "outage: a lifecycle-store outage fails closed and recovers"
bash scripts/local-postgres.sh halt
expect 503 "$workspace/contacts/people/$ada"
expect 503 "$workspace/ui/contributions"
http_is 503 "$api_base/health/ready" || fail "api reports ready during the outage"
wait_until 30 "worker to report not ready" http_is 503 "http://127.0.0.1:$worker_port/health/ready"
bash scripts/local-postgres.sh resume
wait_until 60 "api recovery" api_ready
wait_until 60 "worker recovery" worker_ready
[[ "$(installation_state)" == disabled ]] || fail "the outage changed the committed lifecycle state"
surfaces_concealed "$ada"

log "restart: a worker restart while disabled keeps the plugin inert"
stop_worker
start_worker
wait_until 120 "worker readiness after restart" worker_ready
make_job_due
sleep 3
job_runs_are "$runs_while_disabled" || fail "the job ran after a restart while disabled"
[[ "$(ledger_digest)" == "$disabled_digest" ]] ||
  fail "tenant data or processing ledgers changed while the plugin was disabled"
cursor_exists while-disabled 1 || fail "a cursor was removed while disabled"

log "re-enable: authorized behaviour resumes once"
lifecycle enable 200 018f1900-0000-7000-8000-000000000004
expect_audit 018f1900-0000-7000-8000-000000000004 plugin.transition.succeeded "AND metadata->>'idempotent' = 'false'"
surfaces_available "$ada"
shell_is enabled
[[ "$(delivered_title)" == Clients ]] || fail "re-enable lost the published customisation"
expect 200 "$workspace/contacts/people/$grace"
activity_is "$ada" 1 || fail "re-enable duplicated Ada's timeline entry"
activity_is "$grace" 0 || fail "an event skipped while disabled was applied after re-enable"
linus=$(create_person "Linus Torvalds" 018f1900-0000-7000-8000-000000000013)
wait_until 60 "Linus's timeline entry" activity_is "$linus" 1

# The schedule became due while disabled; re-enable runs it exactly once
# (no catch-up burst for the missed time) and leaves the next run an interval out.
wait_until 60 "the overdue job run after re-enable" job_runs_are "$((runs_while_disabled + 1))"
cursor_exists while-disabled 0 || fail "the re-enabled job did not remove the cursor"
sleep 3
job_runs_are "$((runs_while_disabled + 1))" || fail "the job ran more than once after re-enable"
[[ "$(sql --command="SELECT next_run_at > now() + interval '50 minutes' FROM plugin_job_schedules WHERE tenant_id = '$tenant' AND plugin_id = '$plugin' AND job_id = '$job'")" == t ]] ||
  fail "the re-enabled job did not schedule its next run an interval out"

log "restart recovery: an interrupted transition converges to the committed state"
sql --command="UPDATE tenant_plugin_installations SET state = 'disabling', previous_state = 'enabled', transition_id = '018f1900-0000-7000-8000-000000000099', transition_started_at = now() WHERE tenant_id = '$tenant' AND plugin_id = '$plugin'"
stop_worker
start_worker
wait_until 120 "worker readiness after recovery" worker_ready
[[ "$(installation_state)" == enabled ]] || fail "recovery did not restore the enabled state"
[[ "$(sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND action = 'plugin.transition.recovered' AND metadata->>'transition_id' = '018f1900-0000-7000-8000-000000000099' AND metadata->>'recovered_state' = 'enabled'")" == 1 ]] ||
  fail "missing audit for the recovered transition"
surfaces_available "$ada"

log "duplicates: restarts and redelivery add no effects"
activity_is "$ada" 1 && activity_is "$linus" 1 || fail "a restart duplicated timeline entries"
# The browser journeys add their own person events; every consumed event has
# exactly one marker and one timeline entry, and the script's two have theirs.
[[ "$(sql --command="SELECT count(*) FROM inbox i JOIN p_contacts.activity a ON a.tenant_id = i.tenant_id AND a.event_id = i.event_id WHERE i.tenant_id = '$tenant' AND i.consumer_id = '$consumer' AND a.entity_public_id IN ('$ada', '$linus')")" == 2 ]] ||
  fail "unexpected inbox markers for the timeline consumer"
[[ "$(sql --command="SELECT count(*) FROM inbox WHERE tenant_id = '$tenant' AND consumer_id = '$consumer'")" == "$(sql --command="SELECT count(*) FROM p_contacts.activity WHERE tenant_id = '$tenant'")" ]] ||
  fail "inbox markers and timeline entries diverge"

log "audit: lifecycle evidence is complete, correlated, and redacted"
for correlation in 018f1900-0000-7000-8000-000000000001 018f1900-0000-7000-8000-000000000002 018f1900-0000-7000-8000-000000000003 018f1900-0000-7000-8000-000000000004; do
  expect_audit "$correlation" plugin.transition.attempted
  expect_audit "$correlation" plugin.transition.succeeded
done
[[ "$(sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND target = 'plugin:$plugin' AND action LIKE 'plugin.transition.%' AND action <> 'plugin.transition.recovered' AND correlation_id IS NULL")" == 0 ]] ||
  fail "a lifecycle audit row is missing its correlation id"
[[ "$(sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND target LIKE 'plugin:%' AND (metadata::text ILIKE '%lovelace%' OR metadata::text ILIKE '%hopper%' OR metadata::text ILIKE '%example.test%' OR metadata::text ILIKE '%$member_token%')")" == 0 ]] ||
  fail "lifecycle audit metadata contains personal data or credentials"

printf 'plugin lifecycle acceptance passed: API, OData, UI delivery, consumer, and job follow enable, disable, and re-enable with intact ledgers, correlated audit, outage and restart recovery\n'
