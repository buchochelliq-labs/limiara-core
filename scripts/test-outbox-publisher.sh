#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="limiara-outbox-publisher-${RANDOM:-0}-$$"
export POSTGRES_PORT=0
export NATS_PORT=0

compose=(docker compose --project-name "$project")
initial_event_type="platform.acceptance.concurrent.v1"
failure_event_type="platform.acceptance.retry.v1"
failure_event_id="018f3000-0000-7000-8000-000000000100"
quarantine_event_type="platform.acceptance.quarantine.v1"
quarantine_event_id="018f3000-0000-7000-8000-000000000150"
quarantine_envelope_id="018f3000-0000-7000-8000-000000000151"
dedupe_event_id="018f3000-0000-7000-8000-000000000001"
claim_probe_event_type="platform.acceptance.claim-probe.v1"
tenant_id="018f3000-0000-7000-8000-000000000200"
stream_name="platform-events"

run_sql() {
  local statement=$1
  printf '%s\n' "$statement" | "${compose[@]}" exec --no-TTY postgres \
    psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
    --quiet --tuples-only --no-align --file=-
}

sql_value() {
  local value
  value=$(run_sql "$1")
  value=${value//$'\r'/}
  printf '%s' "$value" | tr -d '[:space:]'
}

diagnostics() {
  "${compose[@]}" ps --all || true
  "${compose[@]}" logs --no-color --tail=200 postgres migrate nats worker || true
  run_sql \
    "SELECT tenant_id, id, event_type, published_at IS NOT NULL AS published, attempt_count, failure_count, claim_token IS NOT NULL AS claimed, next_attempt_at, last_error_code FROM outbox ORDER BY created_at, tenant_id, id;" \
    || true
  "${compose[@]}" exec --no-TTY nats \
    wget -qO- 'http://127.0.0.1:8222/jsz?streams=true&config=true' || true
}

cleanup() {
  local status=$?
  trap - EXIT
  nats_id=$("${compose[@]}" ps --quiet nats 2>/dev/null || true)
  if [[ -n "$nats_id" ]]; then
    docker unpause "$nats_id" >/dev/null 2>&1 || true
  fi
  if (( status != 0 )); then
    diagnostics >&2
  fi
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

wait_for_final_postgres() {
  local deadline=$((SECONDS + 120))
  while (( SECONDS < deadline )); do
    # The official image briefly starts an initialization server. Requiring
    # the final postgres process at PID 1 prevents migrations from connecting
    # to that temporary server and being interrupted when initialization ends.
    if "${compose[@]}" exec --no-TTY postgres sh -ec \
      'test "$(cat /proc/1/comm)" = postgres && pg_isready --username "$POSTGRES_USER" --dbname "$POSTGRES_DB"' \
      >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  printf 'PostgreSQL did not reach its final PID 1 server\n' >&2
  return 1
}

wait_for_healthy_containers() {
  local service=$1
  local expected=$2
  local deadline=$((SECONDS + 180))
  local -a container_ids=()
  local container_id
  local all_healthy
  while (( SECONDS < deadline )); do
    mapfile -t container_ids < <("${compose[@]}" ps --quiet "$service")
    if (( ${#container_ids[@]} == expected )); then
      all_healthy=true
      for container_id in "${container_ids[@]}"; do
        if [[ "$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id")" != healthy ]]; then
          all_healthy=false
          break
        fi
      done
      if [[ "$all_healthy" == true ]]; then
        return 0
      fi
    fi
    sleep 1
  done
  printf 'expected %s healthy %s container(s)\n' "$expected" "$service" >&2
  return 1
}

wait_for_sql_value() {
  local expected=$1
  local statement=$2
  local timeout_seconds=${3:-60}
  local deadline=$((SECONDS + timeout_seconds))
  local actual=""
  while (( SECONDS < deadline )); do
    actual=$(sql_value "$statement" 2>/dev/null || true)
    if [[ "$actual" == "$expected" ]]; then
      return 0
    fi
    sleep 0.25
  done
  printf 'expected database assertion %s, received %s\n' "$expected" "$actual" >&2
  return 1
}

wait_for_jetstream_messages() {
  local expected=$1
  local deadline=$((SECONDS + 30))
  local state=""
  while (( SECONDS < deadline )); do
    state=$("${compose[@]}" exec --no-TTY nats \
      wget -qO- 'http://127.0.0.1:8222/jsz?streams=true&config=true' 2>/dev/null || true)
    if grep -Eq "\"messages\"[[:space:]]*:[[:space:]]*${expected}([,}[:space:]]|$)" <<<"$state"; then
      return 0
    fi
    sleep 0.25
  done
  printf 'expected JetStream to retain %s message(s)\n' "$expected" >&2
  return 1
}

docker compose config --quiet
"${compose[@]}" up --detach postgres
wait_for_final_postgres
"${compose[@]}" up --detach nats
wait_for_healthy_containers nats 1
"${compose[@]}" run --rm migrate

# Twelve valid, bounded envelopes provide enough work for two worker processes
# while retaining deterministic identities and no customer data.
seed_initial=$(cat <<SQL
WITH fixtures AS (
  SELECT
    sequence,
    ('018f3000-0000-7000-8000-' || lpad(sequence::text, 12, '0'))::uuid AS id,
    ('018f3000-0000-7000-9000-' || lpad(sequence::text, 12, '0'))::uuid AS correlation_id
  FROM generate_series(1, 12) AS sequence
)
INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at)
SELECT
  id,
  '$tenant_id'::uuid,
  '$initial_event_type',
  jsonb_build_object(
    'id', id,
    'event_type', '$initial_event_type',
    'schema_version', 1,
    'source_plugin', 'platform.acceptance',
    'tenant_id', '$tenant_id'::uuid,
    'actor_id', NULL,
    'correlation_id', correlation_id,
    'causation_id', NULL,
    'occurred_at', '2026-07-20T12:00:00Z',
    'payload', jsonb_build_object('sequence', sequence)
  ),
  now()
FROM fixtures;
SQL
)
run_sql "$seed_initial" >/dev/null

# SQL is deliberately used to model a corrupted durable row that cannot be
# produced through the typed event API. Its envelope identity does not match
# the outbox identity, so it must be quarantined rather than retried forever or
# published to JetStream.
seed_quarantine=$(cat <<SQL
INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at)
VALUES (
  '$quarantine_event_id'::uuid,
  '$tenant_id'::uuid,
  '$quarantine_event_type',
  jsonb_build_object(
    'id', '$quarantine_envelope_id'::uuid,
    'event_type', '$quarantine_event_type',
    'schema_version', 1,
    'source_plugin', 'platform.acceptance',
    'tenant_id', '$tenant_id'::uuid,
    'actor_id', NULL,
    'correlation_id', '018f3000-0000-7000-9000-000000000150'::uuid,
    'causation_id', NULL,
    'occurred_at', '2026-07-20T12:00:30Z',
    'payload', jsonb_build_object('sequence', 150)
  ),
  now()
);
SQL
)
run_sql "$seed_quarantine" >/dev/null

if [[ "${LIMIARA_SKIP_BUILD:-0}" == 1 ]]; then
  docker image inspect limiara-worker:local >/dev/null
else
  "${compose[@]}" build worker
fi
"${compose[@]}" up --detach --no-deps --scale worker=2 worker
wait_for_healthy_containers worker 2

wait_for_sql_value '12|0|1|1' \
  "SELECT concat_ws('|', count(*) FILTER (WHERE published_at IS NOT NULL), count(*) FILTER (WHERE claim_token IS NOT NULL OR claim_expires_at IS NOT NULL), min(attempt_count), max(attempt_count)) FROM outbox WHERE event_type = '$initial_event_type';"
wait_for_sql_value 't|t|t|t|t|t' \
  "SELECT concat_ws('|', published_at IS NULL, quarantined_at IS NOT NULL, attempt_count = 1, failure_count = 1, claim_token IS NULL AND claim_expires_at IS NULL, last_error_code = 'envelope_mismatch') FROM outbox WHERE tenant_id = '$tenant_id'::uuid AND id = '$quarantine_event_id'::uuid;"
sleep 2
wait_for_sql_value 't' \
  "SELECT attempt_count = 1 AND published_at IS NULL AND quarantined_at IS NOT NULL AND claim_token IS NULL AND claim_expires_at IS NULL FROM outbox WHERE tenant_id = '$tenant_id'::uuid AND id = '$quarantine_event_id'::uuid;"

# Recreate the durable state left by a process that received the broker ack but
# stopped before its database finalisation. The second acknowledgement must
# finalise the row, while stable Nats-Msg-Id suppresses a second stored message
# within the configured duplicate window.
run_sql \
  "UPDATE outbox SET published_at = NULL, next_attempt_at = now() WHERE tenant_id = '$tenant_id'::uuid AND id = '$dedupe_event_id'::uuid AND published_at IS NOT NULL AND attempt_count = 1;" \
  >/dev/null
wait_for_sql_value 't|t|t' \
  "SELECT concat_ws('|', published_at IS NOT NULL, attempt_count = 2, claim_token IS NULL AND claim_expires_at IS NULL) FROM outbox WHERE tenant_id = '$tenant_id'::uuid AND id = '$dedupe_event_id'::uuid;"
wait_for_jetstream_messages 12

# Freeze the already-connected broker long enough to expose two simultaneous
# claim tokens. Each worker processes at most one claim batch at a time, so two
# bounded live tokens prove that SKIP LOCKED distributed work across the two
# processes. A publish timeout may release part of either batch before the
# assertion observes it; stable event IDs and the final stream count still
# prove retries do not create stored duplicates.
nats_id=$("${compose[@]}" ps --quiet nats)
[[ -n "$nats_id" ]]
docker pause "$nats_id" >/dev/null
seed_claim_probe=$(cat <<SQL
WITH fixtures AS (
  SELECT
    sequence,
    ('018f3000-0000-7000-a000-' || lpad(sequence::text, 12, '0'))::uuid AS id,
    ('018f3000-0000-7000-b000-' || lpad(sequence::text, 12, '0'))::uuid AS correlation_id
  FROM generate_series(1, 64) AS sequence
)
INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at)
SELECT
  id,
  '$tenant_id'::uuid,
  '$claim_probe_event_type',
  jsonb_build_object(
    'id', id,
    'event_type', '$claim_probe_event_type',
    'schema_version', 1,
    'source_plugin', 'platform.acceptance',
    'tenant_id', '$tenant_id'::uuid,
    'actor_id', NULL,
    'correlation_id', correlation_id,
    'causation_id', NULL,
    'occurred_at', '2026-07-20T12:00:45Z',
    'payload', jsonb_build_object('sequence', sequence)
  ),
  now()
FROM fixtures;
SQL
)
run_sql "$seed_claim_probe" >/dev/null
wait_for_sql_value '64|2|t|t' \
  "WITH claims AS (SELECT claim_token, count(*) AS claimed FROM outbox WHERE event_type = '$claim_probe_event_type' AND claim_token IS NOT NULL GROUP BY claim_token) SELECT concat_ws('|', (SELECT count(*) FROM outbox WHERE event_type = '$claim_probe_event_type'), count(*), min(claimed) BETWEEN 1 AND 32, max(claimed) BETWEEN 1 AND 32) FROM claims;" \
  4
printf 'claim proof: rows=64 live_tokens=2 rows_per_token=1..32\n'
docker unpause "$nats_id" >/dev/null
wait_for_sql_value '64|0|t|t' \
  "SELECT concat_ws('|', count(*) FILTER (WHERE published_at IS NOT NULL), count(*) FILTER (WHERE claim_token IS NOT NULL OR claim_expires_at IS NOT NULL), min(attempt_count) >= 1, max(attempt_count) <= 3) FROM outbox WHERE event_type = '$claim_probe_event_type';" \
  60
wait_for_jetstream_messages 76

# A connected worker must preserve an event when the broker disappears. Once
# the worker records the real failure, postpone the next attempt atomically so
# the assertion cannot race the short first retry interval.
"${compose[@]}" stop --timeout 10 nats
seed_failure=$(cat <<SQL
INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at)
VALUES (
  '$failure_event_id'::uuid,
  '$tenant_id'::uuid,
  '$failure_event_type',
  jsonb_build_object(
    'id', '$failure_event_id'::uuid,
    'event_type', '$failure_event_type',
    'schema_version', 1,
    'source_plugin', 'platform.acceptance',
    'tenant_id', '$tenant_id'::uuid,
    'actor_id', NULL,
    'correlation_id', '018f3000-0000-7000-9000-000000000100'::uuid,
    'causation_id', NULL,
    'occurred_at', '2026-07-20T12:01:00Z',
    'payload', jsonb_build_object('sequence', 100)
  ),
  now()
);
SQL
)
run_sql "$seed_failure" >/dev/null

postpone_failed_retry="
WITH postponed AS (
  UPDATE outbox
  SET next_attempt_at = now() + INTERVAL '5 minutes'
  WHERE tenant_id = '$tenant_id'::uuid
    AND id = '$failure_event_id'::uuid
    AND published_at IS NULL
    AND failure_count >= 1
    AND claim_token IS NULL
    AND claim_expires_at IS NULL
  RETURNING 1
)
SELECT count(*) FROM postponed;
"
wait_for_sql_value '1' "$postpone_failed_retry" 60
wait_for_sql_value 't|t|t|t|t' \
  "SELECT concat_ws('|', published_at IS NULL, attempt_count >= 1, failure_count >= 1, claim_token IS NULL AND claim_expires_at IS NULL, next_attempt_at > last_failure_at) FROM outbox WHERE tenant_id = '$tenant_id'::uuid AND id = '$failure_event_id'::uuid;"

"${compose[@]}" start nats
wait_for_healthy_containers nats 1
wait_for_healthy_containers worker 2
run_sql "UPDATE outbox SET next_attempt_at = now() WHERE tenant_id = '$tenant_id'::uuid AND id = '$failure_event_id'::uuid AND published_at IS NULL;" >/dev/null
wait_for_sql_value 't|t|t|t' \
  "SELECT concat_ws('|', published_at IS NOT NULL, attempt_count >= 2, failure_count >= 1, claim_token IS NULL AND claim_expires_at IS NULL) FROM outbox WHERE tenant_id = '$tenant_id'::uuid AND id = '$failure_event_id'::uuid;"

# Every worker exposes the same label-free database aggregates. Process-local
# publication counters may differ, so assert names and privacy on each body and
# final backlog/attempt values on one representative scrape.
mapfile -t worker_ids < <("${compose[@]}" ps --quiet worker)
[[ ${#worker_ids[@]} -eq 2 ]]
required_metrics=(
  limiara_outbox_backlog
  limiara_outbox_claimable
  limiara_outbox_claimed
  limiara_outbox_quarantined
  limiara_outbox_publish_attempts_total
  limiara_outbox_publish_failures_total
  limiara_outbox_published_total
  limiara_outbox_oldest_pending_age_seconds
  limiara_outbox_publish_latency_seconds_sum
  limiara_outbox_publish_latency_seconds_count
)
total_attempts=0
total_failures=0
total_published=0
for worker_id in "${worker_ids[@]}"; do
  body=$(docker exec "$worker_id" curl --fail --silent http://127.0.0.1:8080/metrics)
  for metric in "${required_metrics[@]}"; do
    grep -Eq "^${metric} " <<<"$body"
  done
  if grep -Eqi 'tenant|workspace|event_type|subject|\{' <<<"$body"; then
    printf 'worker metrics expose a tenant/event dimension or label\n' >&2
    exit 1
  fi
  grep -qx 'limiara_outbox_backlog 0' <<<"$body"
  grep -qx 'limiara_outbox_claimable 0' <<<"$body"
  grep -qx 'limiara_outbox_claimed 0' <<<"$body"
  grep -qx 'limiara_outbox_quarantined 1' <<<"$body"
  attempts=$(awk '$1 == "limiara_outbox_publish_attempts_total" { print $2 }' <<<"$body")
  failures=$(awk '$1 == "limiara_outbox_publish_failures_total" { print $2 }' <<<"$body")
  published=$(awk '$1 == "limiara_outbox_published_total" { print $2 }' <<<"$body")
  [[ "$attempts" =~ ^[0-9]+$ && "$failures" =~ ^[0-9]+$ && "$published" =~ ^[0-9]+$ ]]
  total_attempts=$((total_attempts + attempts))
  total_failures=$((total_failures + failures))
  total_published=$((total_published + published))
done
(( total_attempts >= 80 ))
(( total_failures >= 2 ))
(( total_published == 78 ))
printf 'metrics proof: attempts=%s failures=%s published=%s\n' \
  "$total_attempts" "$total_failures" "$total_published"

jetstream_state=$("${compose[@]}" exec --no-TTY nats \
  wget -qO- 'http://127.0.0.1:8222/jsz?streams=true&config=true')
grep -Eq "\"name\"[[:space:]]*:[[:space:]]*\"${stream_name}\"" <<<"$jetstream_state"
if ! grep -Fq 'platform.events.v1.tenant.*.>' <<<"$jetstream_state" \
  && ! grep -Fq 'platform.events.v1.tenant.*.\u003e' <<<"$jetstream_state"; then
  printf 'JetStream does not own the Limiara v1 subject filter\n' >&2
  exit 1
fi
grep -Eq '"max_bytes"[[:space:]]*:[[:space:]]*1073741824([,}[:space:]]|$)' <<<"$jetstream_state"
grep -Eq '"messages"[[:space:]]*:[[:space:]]*77([,}[:space:]]|$)' <<<"$jetstream_state"
printf 'stream proof: max_bytes=1073741824 messages=77\n'

printf 'outbox publisher acceptance passed for two workers, broker retry, metrics, and JetStream state\n'
