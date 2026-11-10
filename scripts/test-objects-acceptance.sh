#!/usr/bin/env bash
# ADR 0015 / #293: one acceptance journey over HTTP against the production API
# binary proving workspace administrators define objects and fields at
# runtime and members work with their records through the generic engine.
#
# The journey covers object and field definition, naming, key prefixes,
# picklists, uniqueness, lookups, related-to fields, plugin-declared objects,
# the Activities plugin and its Log a call action, the Activity timeline,
# record create/read/update with optimistic
# versions, filters and search, the recycle bin, object permissions per role,
# forced RLS on the engine tables, and evidence that names changes without
# carrying record values.
#
# PostgreSQL and NATS run locally (scripts/local-postgres.sh,
# scripts/local-nats.sh). The test touches the database directly only to seed
# sessions, a second role and to inspect evidence and RLS.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

work_root="$repository_root/target/objects-acceptance"
api_port="${PLATFORM_ACCEPTANCE_API_PORT:-18091}"
tenant="018f0000-0000-7000-8000-000000000001"
member="018f0000-0000-7000-8000-000000000101"
viewer="018f0000-0000-7000-8000-000000000102"
viewer_role="018f0000-0000-7000-8000-000000000202"
api_base="http://127.0.0.1:${api_port}"
workspace="$api_base/api/v1/workspaces/$tenant"
member_token="objects-acceptance-member"
viewer_token="objects-acceptance-viewer"
rep="018f0000-0000-7000-8000-000000000171"
manager="018f0000-0000-7000-8000-000000000172"
sales_role="018f0000-0000-7000-8000-000000000203"
rep_token="objects-acceptance-rep"
manager_token="objects-acceptance-manager"
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
  if (( status != 0 )) && [[ -f "$work_root/api.log" ]]; then
    printf '\n--- api.log (tail)\n' >&2
    tail -n 60 "$work_root/api.log" >&2
  fi
  $nats_started && bash scripts/local-nats.sh stop
  $postgres_started && bash scripts/local-postgres.sh stop
  exit "$status"
}
trap cleanup EXIT

rm -rf "$work_root"
mkdir -p "$work_root"
log "building api"
cargo build --locked -p api --bins
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
app_sql() {
  psql "$PLATFORM_TEST_DATABASE_URL" --no-psqlrc --set=ON_ERROR_STOP=1 \
    --tuples-only --no-align --quiet "$@"
}

hash_token() { printf '%s' "$1" | sha256sum | awk '{print $1}'; }

# The seeded administrator holds objects.manage (migration 0015). The viewer
# role starts with no object access at all.
sql --command="
  INSERT INTO roles (tenant_id, id, name) VALUES ('$tenant', '$viewer_role', 'Project viewer');
  INSERT INTO users (id, display_name) VALUES ('$viewer', 'Viewer');
  INSERT INTO memberships (tenant_id, user_id) VALUES ('$tenant', '$viewer');
  INSERT INTO user_roles (tenant_id, user_id, role_id) VALUES ('$tenant', '$viewer', '$viewer_role');
  INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES
    ('018f0000-0000-7000-8000-000000000611', '$(hash_token "$member_token")', '$member', now() + interval '2 hours'),
    ('018f0000-0000-7000-8000-000000000612', '$(hash_token "$viewer_token")', '$viewer', now() + interval '2 hours');
  INSERT INTO roles (tenant_id, id, name) VALUES ('$tenant', '$sales_role', 'Sales');
  INSERT INTO users (id, display_name) VALUES ('$rep', 'Riley Rep'), ('$manager', 'Morgan Manager');
  INSERT INTO memberships (tenant_id, user_id) VALUES ('$tenant', '$rep'), ('$tenant', '$manager');
  INSERT INTO user_roles (tenant_id, user_id, role_id) VALUES ('$tenant', '$rep', '$sales_role'), ('$tenant', '$manager', '$sales_role');
  INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES
    ('018f0000-0000-7000-8000-000000000613', '$(hash_token "$rep_token")', '$rep', now() + interval '2 hours'),
    ('018f0000-0000-7000-8000-000000000614', '$(hash_token "$manager_token")', '$manager', now() + interval '2 hours');"

cat >"$work_root/config.yml" <<EOF
platform:
  devices:
    redirect_uris:
      - com.example.app:/oauth
  plugins:
    manifest_paths:
      - plugins/contacts/plugin.toml
      - plugins/activities/plugin.toml
      - fixtures/plugins/tracker/plugin.toml
EOF

env PLATFORM_DEFAULT_CONFIG_FILE="$repository_root/config/default.yml" \
  PLATFORM_CONFIG_FILE="$work_root/config.yml" \
  PLATFORM__PLATFORM__NATS__URL="$PLATFORM_TEST_NATS_URL" \
  PLATFORM__PLATFORM__DATABASE__URL="$PLATFORM_TEST_DATABASE_URL" \
  PLATFORM__PLATFORM__SERVICES__API_BIND="127.0.0.1:$api_port" \
  RUST_LOG="${RUST_LOG:-info}" \
  target/debug/api >>"$work_root/api.log" 2>&1 &
api_pid=$!

deadline=$((SECONDS + 60))
until [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' "$api_base/health/ready")" == 200 ]]; do
  (( SECONDS < deadline )) || fail "api did not become ready"
  sleep 0.3
done

member_cookie="Cookie: platform_session=$member_token"
viewer_cookie="Cookie: platform_session=$viewer_token"
rep_cookie="Cookie: platform_session=$rep_token"
manager_cookie="Cookie: platform_session=$manager_token"
body="$work_root/body.json"

# call <cookie> <method> <url> [json]: prints the status; the body is in $body.
call() {
  local args=(--max-time 20 --silent --output "$body" --write-out '%{http_code}'
    --header "$1" --request "$2" "$3")
  [[ $# -ge 4 ]] && args+=(--header 'Content-Type: application/json' --data "$4")
  curl "${args[@]}"
}
expect() {
  local expected=$1 actual=$2 description=$3
  [[ "$actual" == "$expected" ]] || fail "$description: expected HTTP $expected, got $actual: $(cat "$body")"
}
field() { jq -r "$1" "$body"; }

log "administrators define workspace objects; names are namespaced, prefixes digit-first"
expect 200 "$(call "$member_cookie" GET "$workspace/objects")" "list objects"
[[ "$(field '.can_manage')" == true ]] || fail "member can manage"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"client","label":"Client","plural_label":"Clients","in_navigation":false}')" "create client"
[[ "$(field '.api_name')" == custom:client && "$(field '.origin')" == workspace ]] || fail "client naming: $(cat "$body")"
[[ "$(field '.key_prefix')" =~ ^[0-9] ]] || fail "workspace key prefix is digit-first"
[[ "$(field '[.fields[].api_name] | length')" == 0 ]] || fail "new object has no custom fields"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"project","label":"Project","plural_label":"Projects","name_label":"Project name","in_navigation":true}')" "create project"
project_version=$(field '.version')
[[ "$(field '.key_prefix')" != "$(jq -r .key_prefix <<<"$(curl --silent --header "$member_cookie" "$workspace/objects/custom:client")")" ]] || fail "prefixes are distinct"
expect 409 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"project","label":"Project","plural_label":"Projects"}')" "duplicate object"
expect 400 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"Project__c","label":"Project","plural_label":"Projects"}')" "invalid short name"
expect 403 "$(call "$viewer_cookie" POST "$workspace/objects" \
  '{"name":"secret","label":"Secret","plural_label":"Secrets"}')" "viewer cannot define objects"
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project" \
  "{\"expected_version\":$project_version,\"description\":\"Client engagements\"}")" "update object"
[[ "$(field '.description')" == "Client engagements" ]] || fail "object description"

log "fields: picklist, unique text, number, date and a restricting lookup"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"status","label":"Status","field_type":"picklist","options":[{"value":"open","label":"Open","default":true},{"value":"won","label":"Won"}]}')" "status field"
[[ "$(field '.api_name')" == custom:status ]] || fail "field api name"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"code","label":"Code","field_type":"text","unique":true,"max_length":20}')" "code field"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"budget","label":"Budget","field_type":"currency","scale":2,"indexed":true}')" "budget field"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"starts_on","label":"Starts on","field_type":"date"}')" "date field"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"client","label":"Client","field_type":"lookup","reference_object":"custom:client","delete_behaviour":"restrict"}')" "lookup field"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" \
  '{"name":"mood","label":"Mood","field_type":"boolean","options":[{"value":"x","label":"X"}]}')" "options on a boolean"
[[ "$(field '.errors[0].code')" == objects.options_not_allowed ]] || fail "options error code"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project")" "describe project"
[[ "$(field '[.fields[].api_name] | join(",")')" == "custom:status,custom:code,custom:budget,custom:starts_on,custom:client" ]] || fail "fields in order: $(cat "$body")"

log "records: create with defaults, validation, uniqueness and lookups"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:client/records" '{"name":"Analytical Engines"}')" "create client record"
client=$(field '.id')
client_version=$(field '.version')
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" \
  "{\"name\":\"Apollo\",\"custom:code\":\"AP-1\",\"custom:budget\":1250.5,\"custom:starts_on\":\"2026-11-02\",\"custom:client\":\"$client\"}")" "create project record"
apollo=$(field '.id')
apollo_version=$(field '.version')
[[ "$(field '."custom:status"')" == open ]] || fail "picklist default applied"
[[ "$(field ".related[\"$client\"]")" == "Analytical Engines" ]] || fail "lookup target named: $(cat "$body")"
[[ "$(field '.object')" == custom:project && "$(field '.is_deleted')" == false ]] || fail "standard fields"
[[ "$(field '.created_by.id')" == "$member" && -n "$(field '.created_by.name // empty')" ]] || fail "created_by names the member"
[[ "$(field '.owner.id')" == "$member" ]] || fail "creator owns the record"
[[ "$(field '.touched_at >= .updated_at')" == true ]] || fail "touched_at"
expect 409 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" \
  '{"name":"Apollo copy","custom:code":"AP-1"}')" "duplicate unique value"
[[ "$(field '.code')" == records.duplicate_value ]] || fail "duplicate code"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" \
  '{"name":"Bad","custom:status":"lost","custom:starts_on":"2 Nov"}')" "invalid values"
[[ "$(field '[.errors[].code] | sort | join(",")')" == "records.date_invalid,records.picklist_value" ]] || fail "validation codes: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"custom:code":"AP-2"}')" "name required"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"X","custom:nope":1}')" "unknown field"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" \
  '{"name":"Gemini","custom:code":"GE-1","custom:status":"won","custom:budget":90}')" "second project"
gemini=$(field '.id')

log "updates are partial and versioned"
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" \
  "{\"expected_version\":$apollo_version,\"custom:status\":\"won\",\"custom:starts_on\":null}")" "update project"
[[ "$(field '."custom:status"')" == won && "$(field '."custom:starts_on" // "cleared"')" == cleared ]] || fail "partial update"
[[ "$(field '."custom:code"')" == AP-1 ]] || fail "untouched values kept"
[[ "$(field '.version')" == $((apollo_version + 1)) ]] || fail "version bumped"
apollo_version=$(field '.version')
expect 409 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" \
  "{\"expected_version\":$((apollo_version - 1)),\"name\":\"Stale\"}")" "stale version"
expect 400 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" '{"name":"No version"}')" "version required"

log "lists: filters on indexed fields, search, sort and keyset cursors"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?filter%5Bcustom%3Astatus%5D=won&sort=name&direction=asc")" "filter won"
[[ "$(field '[.data[].name] | join(",")')" == "Apollo,Gemini" ]] || fail "won filter: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?filter%5Bcustom%3Acode%5D=GE-1")" "filter unique"
[[ "$(field '[.data[].id] | join(",")')" == "$gemini" ]] || fail "unique filter"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?search=apol")" "search"
[[ "$(field '[.data[].id] | join(",")')" == "$apollo" ]] || fail "search: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?sort=name&direction=desc&limit=1")" "page one"
[[ "$(field '.data[0].name')" == Gemini ]] || fail "desc sort"
cursor=$(field '.next_cursor')
[[ "$cursor" != null ]] || fail "next cursor"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?sort=name&direction=desc&limit=1&cursor=$cursor")" "page two"
[[ "$(field '.data[0].name')" == Apollo ]] || fail "keyset page two"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?sort=created_at&direction=desc&limit=1&cursor=$cursor")" "cursor from another sort"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?filter%5Bcustom%3Astarts_on%5D=x")" "filter value"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?colour=red")" "unknown query key"

log "lookups restrict deleting their target"
expect 409 "$(call "$member_cookie" DELETE "$workspace/objects/custom:client/records/$client?expected_version=$client_version")" "restricted delete"
[[ "$(field '.code')" == records.delete_restricted ]] || fail "restrict code"
expect 409 "$(call "$member_cookie" DELETE "$workspace/objects/custom:client?expected_version=1")" "object referenced by a lookup"

log "the recycle bin: delete, hide, include_deleted, undelete"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/records/$gemini?expected_version=1")" "delete gemini"
[[ "$(field '.is_deleted')" == true ]] || fail "deleted flag"
gemini_version=$(field '.version')
expect 404 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$gemini")" "deleted record hidden"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$gemini?include_deleted=true")" "include_deleted"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?status=deleted")" "recycle bin"
[[ "$(field '[.data[].id] | join(",")')" == "$gemini" ]] || fail "bin holds gemini"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Gemini two","custom:code":"GE-1"}')" "deleted records release unique values"
expect 409 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records/$gemini/undelete?expected_version=$gemini_version")" "undelete conflicts with the reused value"
[[ "$(field '.code')" == records.duplicate_value ]] || fail "undelete duplicate code"

log "object permissions: no access until granted per role"
expect 404 "$(call "$viewer_cookie" GET "$workspace/objects/custom:project")" "viewer without access"
expect 200 "$(call "$viewer_cookie" GET "$workspace/objects")" "viewer object list"
[[ "$(field '.data | length')" == 0 && "$(field '.can_manage')" == false ]] || fail "viewer sees no objects: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/ui/contributions")" "member navigation"
[[ "$(field '[.navigation[] | select(.id == "objects.custom:project") | .route] | join(",")')" == "/o/custom:project" ]] || fail "in-navigation objects are contributed: $(cat "$body")"
[[ "$(field '[.navigation[] | select(.id == "objects.custom:client")] | length')" == 0 ]] || fail "objects outside navigation are not contributed"
[[ "$(field '.permissions | index("objects.manage") != null')" == true ]] || fail "engine permissions are listed"
member_revision=$(field '.revision')
expect 200 "$(call "$viewer_cookie" GET "$workspace/ui/contributions")" "viewer navigation"
[[ "$(field '[.navigation[] | select(.id | startswith("objects."))] | length')" == 0 ]] || fail "unreadable objects are not contributed"
viewer_revision=$(field '.revision')
expect 403 "$(call "$viewer_cookie" PUT "$workspace/objects/custom:project/permissions/$viewer_role" '{"read":true}')" "viewer cannot grant"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:project/permissions/$viewer_role" '{"read":true}')" "grant read"
[[ "$(field '.access.read')" == true && "$(field '.access.edit')" == false ]] || fail "read grant"
expect 404 "$(call "$member_cookie" PUT "$workspace/objects/custom:project/permissions/018f0000-0000-7000-8000-00000000ffff" '{"read":true}')" "unknown role"
expect 200 "$(call "$viewer_cookie" GET "$workspace/ui/contributions")" "viewer navigation after grant"
[[ "$(field '[.navigation[] | select(.id == "objects.custom:project")] | length')" == 1 ]] || fail "granted object appears"
[[ "$(field '.revision')" != "$viewer_revision" ]] || fail "navigation revision changes with object access"
[[ -n "$member_revision" ]] || fail "member revision"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/permissions")" "permission matrix"
[[ "$(field "[.data[] | select(.role_id == \"$viewer_role\") | .access.read] | first")" == true ]] || fail "matrix shows the grant"
expect 200 "$(call "$viewer_cookie" GET "$workspace/objects/custom:project/records/$apollo")" "viewer reads"
[[ "$(field '."custom:client"')" == "$client" && "$(field '.related | length')" == 0 ]] || fail "names of unreadable lookup targets are withheld: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" \
  "{\"expected_version\":$apollo_version,\"name\":\"Viewer edit\"}")" "viewer cannot edit"
expect 403 "$(call "$viewer_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Viewer project"}')" "viewer cannot create"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:project/permissions/$viewer_role" '{"edit":true}')" "grant edit"
[[ "$(field '.access.read')" == true ]] || fail "edit implies read"
expect 200 "$(call "$viewer_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" \
  "{\"expected_version\":$apollo_version,\"name\":\"Apollo Prime\"}")" "viewer edits once granted"
[[ "$(field '.updated_by.id')" == "$viewer" ]] || fail "updated_by is the viewer"

log "fields can be retired; values disappear from records"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/fields/custom:budget?expected_version=1")" "delete budget field"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo")" "read after field delete"
[[ "$(field 'has("custom:budget")')" == false ]] || fail "retired field hidden"

log "field history (ADR 0017): tracked fields, owner changes, who and when"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project")" "describe before tracking"
project_version=$(field '.version')
status_version=$(field '.fields[] | select(.api_name == "custom:status") | .version')
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project" "{\"expected_version\":$project_version,\"track_history\":true}")" "track object history"
[[ "$(field '.track_history')" == true ]] || fail "object tracks history"
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/fields/custom:status" "{\"expected_version\":$status_version,\"track_history\":true}")" "track status"
[[ "$(field '.track_history')" == true ]] || fail "field tracks history"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo")" "read before tracked edit"
apollo_version=$(field '.version')
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" \
  "{\"expected_version\":$apollo_version,\"custom:status\":\"open\",\"custom:code\":\"AP-9\"}")" "tracked edit"
apollo_version=$(field '.version')
expect 200 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records/$apollo/owner" \
  "{\"owner_id\":\"$viewer\",\"expected_version\":$apollo_version}")" "tracked transfer"
expect 200 "$(call "$viewer_cookie" GET "$workspace/objects/custom:project/records/$apollo/history")" "history"
[[ "$(field '[.data[] | "\(.kind):\(.field // "")"] | join(",")')" == "changed:owner,changed:custom:status" ]] || fail "history entries: $(cat "$body")"
[[ "$(field '.data[1] | "\(.old_value)>\(.new_value)"')" == "won>open" ]] || fail "status values kept"
[[ "$(field '.data[0].new_user.name')" == Viewer && "$(field '.data[0].old_user.id')" == "$member" ]] || fail "owner change names members"
[[ "$(field '.data[0].changed_by.id')" == "$member" && -n "$(field '.data[0].changed_by.name // empty')" ]] || fail "who changed it"
grep -q AP-9 "$body" && fail "untracked fields are not kept"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo/history?limit=0")" "history limit"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo/history?cursor=nope")" "history cursor"
expect 404 "$(call "$rep_cookie" GET "$workspace/objects/custom:project/records/$apollo/history")" "no object access, no history"

log "change data capture (ADR 0018) and sync on touched_at (#287)"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/changes?limit=200")" "change log"
head=$(field '.head')
(( head > 0 )) || fail "earlier changes are in the log"
since=$(date -u +%Y-%m-%dT%H:%M:%S.%6NZ)
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Synced","custom:code":"SY-1"}')" "sync create"
synced=$(field '.id')
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$synced" "{\"expected_version\":1,\"custom:code\":\"SY-2\"}")" "sync update"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/records/$synced?expected_version=2")" "sync delete"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/changes?after=$head")" "new changes"
[[ "$(field '[.data[] | "\(.position):\(.kind):\(.fields | sort | join("+"))"] | join(",")')" == "$((head + 1)):created:custom:code+custom:status+name,$((head + 2)):updated:custom:code,$((head + 3)):deleted:" ]] || fail "change log entries: $(cat "$body")"
[[ "$(field '.data[0].record')" == "$synced" && "$(field '.data[0].object')" == custom:project ]] || fail "changes name the record"
grep -q "SY-" "$body" && fail "the change log carries no values"
[[ "$(field '.next_after')" == $((head + 3)) && "$(field '.head')" == $((head + 3)) ]] || fail "positions"
metadata_version=$(field '.metadata_version')
(( metadata_version > 0 )) || fail "definitions moved the metadata version: $(cat "$body")"
expect 200 "$(call "$rep_cookie" GET "$workspace/objects/changes?after=$head")" "another member's view"
[[ "$(field '.data | length')" == 0 && "$(field '.next_after')" == $((head + 3)) ]] || fail "invisible changes are withheld but the position advances"
expect 404 "$(call "$rep_cookie" GET "$workspace/objects/changes?object=custom:project")" "object filter needs access"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/changes?after=$((head + 99))")" "unknown position"
stream="$work_root/stream.txt"
curl --silent --no-buffer --max-time 3 --header "$member_cookie" --header "Last-Event-ID: $head" \
  "$workspace/objects/changes/stream" >"$stream" || true
grep -q "^id: $((head + 1))$" "$stream" && grep -q "^id: $((head + 3))$" "$stream" && grep -q "^event: change$" "$stream" \
  || fail "the stream replays from Last-Event-ID: $(head -c 600 "$stream")"
grep -q "^id: $head$" "$stream" && fail "replay starts after the given position"
grep -q "^event: metadata$" "$stream" && grep -q "\"metadata_version\":$metadata_version" "$stream" \
  || fail "the stream announces the metadata version first (ADR 0024): $(head -c 600 "$stream")"
[[ "$(grep -n -m1 '^event: ' "$stream")" == *"event: metadata"* ]] || fail "metadata comes before changes"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?status=any&sort=touched_at&touched_after=$since")" "touched since"
[[ "$(field '[.data[] | "\(.id):\(.is_deleted)"] | join(",")')" == "$synced:true" ]] || fail "sync returns tombstones: $(cat "$body")"
expect 400 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records?touched_after=yesterday")" "touched_after format"
sql --command="UPDATE change_counters SET expired_through = last_position WHERE tenant_id = '$tenant';"
expect 410 "$(call "$member_cookie" GET "$workspace/objects/changes?after=$head")" "expired position"
[[ "$(field '.code')" == changes.position_expired ]] || fail "expired code"
curl --silent --no-buffer --max-time 3 --header "$member_cookie" "$workspace/objects/changes/stream?after=$head" >"$stream" || true
grep -q "changes.position_expired" "$stream" || fail "an expired stream says so: $(head -c 300 "$stream")"

log "query language (ADR 0019): typed filters, relationships, aggregates, paging"
query() { printf '%s/query?q=%s%s' "$workspace" "$(jq -rn --arg q "$1" '$q | @uri')" "${2:-}"; }
expect 200 "$(call "$member_cookie" GET "$(query "SELECT name, custom:code, custom:client.name client FROM custom:project WHERE custom:code = 'ap-9' AND custom:client.name LIKE 'analytical%'")")" "query with a relationship"
[[ "$(field '.object')" == custom:project && "$(field '.data[0].id')" == "$apollo" ]] || fail "query row: $(cat "$body")"
[[ "$(field '.data[0].client')" == "Analytical Engines" && "$(field '.data[0]["custom:code"]')" == AP-9 ]] || fail "query values: $(cat "$body")"
[[ "$(field '[.columns[] | "\(.key):\(.type)"] | join(",")')" == "name:text,custom:code:text,client:text" ]] || fail "query columns: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$(query "SELECT COUNT() n FROM custom:project WHERE created_at = LAST_N_DAYS:1")")" "aggregate"
(( $(field '.data[0].n') >= 1 )) || fail "count: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:project ORDER BY created_at" "&page_size=1")")" "first page"
[[ "$(field '.data | length')" == 1 && -n "$(field '.next_cursor // empty')" ]] || fail "query paging: $(cat "$body")"
cursor=$(field '.next_cursor')
expect 200 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:project ORDER BY created_at" "&page_size=1&cursor=$cursor")")" "next page"
expect 400 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:project ORDER BY name" "&cursor=$cursor")")" "cursor of another query"
[[ "$(field '.errors[0].code')" == query.cursor_invalid ]] || fail "cursor code"
expect 400 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:project WHERE")")" "syntax"
[[ "$(field '.errors[0].code')" == query.syntax && "$(field '.errors[0].field')" == q ]] || fail "syntax code: $(cat "$body")"
expect 400 "$(call "$rep_cookie" GET "$(query "SELECT name FROM custom:project")")" "unreadable object"
[[ "$(field '.errors[0].code')" == query.object_unknown ]] || fail "unreadable object is unknown"
expect 400 "$(call "$member_cookie" GET "$workspace/query?q=x&bogus=1")" "unknown parameter"

log "global search (ADR 0020): names and searchable fields, grouped, shared"
search() { printf '%s/search?q=%s%s' "$workspace" "$(jq -rn --arg q "$1" '$q | @uri')" "${2:-}"; }
expect 200 "$(call "$member_cookie" GET "$(search "apollo pri")")" "search"
[[ "$(field '[.data[] | select(.object == "custom:project") | .records[] | .id] | index("'"$apollo"'")')" != null ]] || fail "search finds Apollo Prime: $(cat "$body")"
[[ "$(field '.data[0].label')" != null && "$(field '.data[0].has_more')" =~ ^(true|false)$ ]] || fail "search groups: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$(search "apollo" "&object=custom:project&limit=200")")" "search one object"
[[ "$(field '[.data[].object] | unique | join(",")')" == custom:project ]] || fail "object filter: $(cat "$body")"
expect 200 "$(call "$rep_cookie" GET "$(search "apollo")")" "search without object access"
[[ "$(field '[.data[] | select(.object == "custom:project")] | length')" == 0 ]] || fail "unreadable objects are not searched"
expect 400 "$(call "$member_cookie" GET "$(search "a")")" "search too short"
[[ "$(field '.errors[0].code')" == search.too_short ]] || fail "too short code"

log "validation rules (ADR 0021): formulas checked on save, enforced on records"
rules="$workspace/objects/custom:project/validation-rules"
expect 400 "$(call "$member_cookie" POST "$rules" '{"name":"broken","condition":"BEGINS(custom:code,","message":"m"}')" "rule syntax"
[[ "$(field '.errors[0].code')" == rules.condition_invalid && "$(field '.errors[0].detail')" == *"position"* ]] || fail "rule syntax error: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$rules" '{"name":"no_x_codes","condition":"BEGINS(UPPER(custom:code), \"X-\")","message":"Codes cannot start with X-","error_field":"custom:code"}')" "create rule"
[[ "$(field '.version')" == 1 && "$(field '.active')" == true ]] || fail "rule: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$rules")" "list rules"
[[ "$(field '[.data[].name] | join(",")')" == no_x_codes ]] || fail "rules listed: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$rules")" "members who read the object read its active rules"
[[ "$(field '[.data[].name] | join(",")')" == no_x_codes ]] || fail "rules listed to the viewer: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" POST "$rules" '{"name":"viewer_rule","condition":"TRUE","message":"m"}')" "changing rules needs objects.manage"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Rejected","custom:code":"x-1"}')" "rule blocks the save"
[[ "$(field '.errors[0].code')" == records.validation_rule && "$(field '.errors[0].field')" == custom:code && "$(field '.errors[0].detail')" == "Codes cannot start with X-" ]] || fail "rule error: $(cat "$body")"
expect 200 "$(call "$member_cookie" DELETE "$rules/no_x_codes?expected_version=1")" "delete rule"
audited=$(sql --command="SELECT count(*) FROM audit_log WHERE action LIKE 'objects.validation_rule.%';")
(( audited == 2 )) || fail "rule changes audited: $audited"

log "quick actions (ADR 0032): checked on save, read by readers, changed by administrators"
actions="$workspace/objects/custom:project/quick-actions"
expect 400 "$(call "$member_cookie" POST "$actions" '{"name":"tag","label":"Tag","kind":"update","fields":["custom:nope"]}')" "quick action field check"
[[ "$(field '.errors[0].code')" == quick_actions.field_unknown && "$(field '.errors[0].field')" == fields.custom:nope ]] || fail "quick action error: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$actions" '{"name":"tag","label":"Tag","kind":"update","fields":["custom:code"],"presets":{"name":"Tagged"}}')" "create quick action"
[[ "$(field '.kind')" == update && "$(field '.version')" == 1 && "$(field '.presets.name')" == Tagged && "$(field '.target_object')" == null ]] || fail "quick action: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$actions")" "members who read the object read its quick actions"
[[ "$(field '[.data[].name] | join(",")')" == tag ]] || fail "quick actions listed: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" POST "$actions" '{"name":"mine","label":"Mine","kind":"update","fields":["name"]}')" "changing quick actions needs objects.manage"
expect 200 "$(call "$member_cookie" PATCH "$actions/tag" '{"expected_version":1,"label":"Tag it"}')" "change quick action"
[[ "$(field '.version')" == 2 && "$(field '.label')" == "Tag it" ]] || fail "changed quick action: $(cat "$body")"
expect 409 "$(call "$member_cookie" DELETE "$actions/tag?expected_version=1")" "stale quick action delete"
expect 200 "$(call "$member_cookie" DELETE "$actions/tag?expected_version=2")" "delete quick action"
audited=$(sql --command="SELECT count(*) FROM audit_log WHERE action LIKE 'objects.quick_action.%';")
(( audited == 3 )) || fail "quick action changes audited: $audited"

log "formula fields (ADR 0021): computed on read, read-only"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" '{"name":"bad_formula","label":"Bad","field_type":"formula","formula":"LOWER(","formula_type":"text"}')" "formula syntax"
[[ "$(field '.errors[0].code')" == objects.formula_invalid ]] || fail "formula error: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" '{"name":"code_lower","label":"Code lower","field_type":"formula","formula":"LOWER(custom:code)","formula_type":"text"}')" "formula field"
[[ "$(field '.field_type')" == formula && "$(field '.formula_type')" == text ]] || fail "formula field: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo")" "read a formula value"
[[ "$(field '.["custom:code_lower"]')" == ap-9 ]] || fail "formula value: $(cat "$body")"
apollo_version=$(field '.version')
expect 400 "$(call "$member_cookie" PATCH "$workspace/objects/custom:project/records/$apollo" "{\"expected_version\":$apollo_version,\"custom:code_lower\":\"x\"}")" "formula is read-only"
[[ "$(field '.errors[0].code')" == records.field_read_only ]] || fail "read-only code"
expect 200 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:project WHERE custom:code_lower = 'ap-9' ORDER BY custom:code_lower")")" "filter on a stored formula"
[[ "$(field '[.data[].id] | join(",")')" == "$apollo" ]] || fail "formula filter: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/fields" '{"name":"code_check","label":"Code check","field_type":"formula","formula":"IF(REGEX(custom:code_lower, \"[a-z]{2}-[0-9]+\"), LPAD(custom:code_lower, 6, \"*\"), \"bad\")","formula_type":"text"}')" "formula on a formula"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:project/records/$apollo")" "read the formula on a formula"
[[ "$(field '.["custom:code_check"]')" == "**ap-9" ]] || fail "formula on a formula: $(cat "$body")"
expect 409 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/fields/custom:code_lower?expected_version=1")" "a formula another formula uses"
[[ "$(field '.code')" == objects.field_in_use ]] || fail "in-use code: $(cat "$body")"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/fields/custom:code_check?expected_version=1")" "delete the using formula"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/fields/custom:code_lower?expected_version=1")" "delete formula field"

log "automations (ADR 0022): checked on save, run in the save transaction"
automations="$workspace/objects/custom:project/automations"
expect 400 "$(call "$member_cookie" POST "$automations" '{"name":"bad","trigger":"create","timing":"before_save","actions":[{"type":"update_record","values":[{"field":"custom:code","formula":"1 +"}]}]}')" "automation syntax"
[[ "$(field '.errors[0].code')" == automations.formula_invalid && "$(field '.errors[0].field')" == "actions[0].values[0].formula" ]] || fail "automation error: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$automations" '{"name":"default_code","trigger":"create","timing":"before_save","condition":"ISBLANK(custom:code)","actions":[{"type":"update_record","values":[{"field":"custom:code","formula":"\"AUTO-\" & RIGHT(id, 6)"}]}]}')" "before-save automation"
[[ "$(field '.timing')" == before_save && "$(field '.actions[0].type')" == update_record && "$(field '.version')" == 1 ]] || fail "automation: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$automations" '{"name":"client_for_each","trigger":"create","timing":"after_save","actions":[{"type":"create_record","object":"custom:client","values":[{"field":"name","formula":"\"Client of \" & name"}]}]}')" "after-save automation"
expect 200 "$(call "$member_cookie" GET "$automations")" "list automations"
[[ "$(field '[.data[].name] | join(",")')" == client_for_each,default_code ]] || fail "automations listed: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" GET "$automations")" "automations need objects.manage"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Automated"}')" "save runs automations"
automated=$(field '.id')
[[ "$(field '.["custom:code"]')" == "AUTO-${automated: -6}" ]] || fail "before-save value: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$(query "SELECT name FROM custom:client WHERE name LIKE 'client of%'")")" "the created record"
[[ "$(field '[.data[].name] | join(",")')" == "Client of Automated" ]] || fail "after-save record: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$automations/client_for_each/runs?limit=5")" "automation runs"
[[ "$(field '.data[0].outcome')" == succeeded && "$(field '.data[0].record')" == "$automated" && "$(field '.data[0].saves')" == 1 ]] || fail "runs: $(cat "$body")"
marked=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.record-created.v1' AND envelope::text LIKE '%automation_id%';")
(( marked >= 1 )) || fail "automation-made changes name the automation in their events"
expect 200 "$(call "$member_cookie" DELETE "$automations/client_for_each?expected_version=1")" "delete automation"
expect 200 "$(call "$member_cookie" DELETE "$automations/default_code?expected_version=1")" "delete automation"
audited=$(sql --command="SELECT count(*) FROM audit_log WHERE action LIKE 'objects.automation.%';")
(( audited == 4 )) || fail "automation changes audited: $audited"
expect 201 "$(call "$member_cookie" POST "$automations" '{"name":"later_code","trigger":"create","timing":"async","actions":[{"type":"update_record","values":[{"field":"custom:code","formula":"\"BG-\" & RIGHT(id, 4)"}]}]}')" "background automation"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Background"}')" "save queues a job"
expect 200 "$(call "$member_cookie" GET "$automations/later_code/jobs")" "queued jobs"
[[ "$(field '.data | length')" == 1 && "$(field '.data[0].status')" == pending ]] || fail "queued job: $(cat "$body")"
job=$(field '.data[0].id')
discovered=$(sql --command="SELECT count(*) FROM platform_due_automation_jobs(500) WHERE id = '$job';")
[[ "$discovered" == 1 ]] || fail "the worker's discovery routine finds the due job"
expect 200 "$(call "$member_cookie" POST "$automations/later_code/jobs/$job/cancel")" "cancel a job"
[[ "$(field '.status')" == cancelled ]] || fail "cancelled: $(cat "$body")"
expect 409 "$(call "$member_cookie" POST "$automations/later_code/jobs/$job/retry")" "only failed jobs are retried"
[[ "$(field '.code')" == automations.job_state ]] || fail "job state code: $(cat "$body")"
expect 200 "$(call "$member_cookie" DELETE "$automations/later_code?expected_version=1")" "delete background automation"
expect 201 "$(call "$member_cookie" POST "$automations" '{"name":"announce","trigger":"create","timing":"after_save","actions":[{"type":"publish_event","event":"project_started","when":"BEGINS(name, \"Announced\")","values":[{"field":"title","formula":"UPPER(name)"}]}]}')" "event automation"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Quiet"}')" "condition false: no event"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/records" '{"name":"Announced launch"}')" "publish an event"
published=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.workspace-event.v1' AND envelope::text LIKE '%project_started%' AND envelope::text LIKE '%ANNOUNCED LAUNCH%';")
[[ "$published" == 1 ]] || fail "one workspace event with its fields: $published"
quiet=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.workspace-event.v1' AND envelope::text LIKE '%QUIET%';")
[[ "$quiet" == 0 ]] || fail "the action's own condition held it back"
audited_event=$(sql --command="SELECT count(*) FROM audit_log WHERE action = 'objects.workspace_event.published' AND metadata::text NOT LIKE '%ANNOUNCED%';")
[[ "$audited_event" == 1 ]] || fail "the audit entry names the event without its values"
expect 200 "$(call "$member_cookie" DELETE "$automations/announce?expected_version=1")" "delete event automation"

log "composite requests (ADR 0023): references, independent and all-or-none"
expect 200 "$(call "$member_cookie" POST "$workspace/composite" '{"operations":[{"method":"create","reference":"client","object":"custom:client","values":{"name":"Composite Client"}},{"method":"create","reference":"project","object":"custom:project","values":{"name":"Composite Project","custom:client":"@{client.id}"}},{"method":"create","reference":"bad","object":"custom:project","values":{"custom:code":"x"}}]}')" "independent operations"
[[ "$(field '[.results[].status] | map(tostring) | join(",")')" == "201,201,400" ]] || fail "composite statuses: $(cat "$body")"
[[ "$(field '.results[1].body["custom:client"]')" == "$(field '.results[0].body.id')" ]] || fail "references resolve: $(cat "$body")"
[[ "$(field '.results[2].body.errors[0].code')" == records.name_required ]] || fail "failure body: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$workspace/composite" '{"all_or_none":true,"operations":[{"method":"create","reference":"kept","object":"custom:client","values":{"name":"Never Kept"}},{"method":"create","reference":"bad","object":"custom:project","values":{"custom:code":"y"}}]}')" "all or none"
[[ "$(field '[.results[].body.code] | join(",")')" == "composite.rolled_back,validation.failed" ]] || fail "rolled back: $(cat "$body")"
kept=$(sql --command="SELECT count(*) FROM records WHERE name = 'Never Kept';")
[[ "$kept" == 0 ]] || fail "nothing of an all-or-none failure is kept"
expect 400 "$(call "$member_cookie" POST "$workspace/composite" '{"operations":[]}')" "empty composite"
expect 200 "$(call "$member_cookie" POST "$workspace/composite" '{"all_or_none":true,"operations":[{"method":"create","reference":"temp","object":"custom:client","values":{"name":"Restored Client"}},{"method":"delete","reference":"out","object":"custom:client","id":"@{temp.id}","expected_version":1},{"method":"undelete","reference":"back","object":"custom:client","id":"@{temp.id}","expected_version":2}]}')" "a composite undelete"
[[ "$(field '[.results[].status] | map(tostring) | join(",")')" == "201,200,200" && "$(field '.results[2].body.deleted_at')" == null ]] || fail "undelete in a composite request: $(cat "$body")"
restored=$(field '.results[2].body.id')
expect 200 "$(call "$member_cookie" POST "$workspace/composite" "{\"operations\":[{\"method\":\"change_owner\",\"reference\":\"moved\",\"object\":\"custom:client\",\"id\":\"$restored\",\"expected_version\":3,\"owner_id\":\"$viewer\"}]}")" "a composite owner change"
[[ "$(field '.results[0].status')" == 200 && "$(field '.results[0].body.owner.id')" == "$viewer" ]] || fail "change_owner in a composite request: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$workspace/composite" "{\"operations\":[{\"method\":\"change_owner\",\"reference\":\"away\",\"object\":\"custom:client\",\"id\":\"$restored\",\"expected_version\":4,\"owner_id\":\"018f0000-0000-7000-8000-00000000ffff\"}]}")" "a composite transfer to a non-member"
[[ "$(field '.errors[0].field')" == "operations[0].owner_id" && "$(field '.errors[0].code')" == records.owner_unknown ]] || fail "non-member owner: $(cat "$body")"

log "bulk jobs (ADR 0023 §2): submit, discover, read, abort"
expect 202 "$(call "$member_cookie" POST "$workspace/bulk" '{"object":"custom:client","operation":"insert","rows":[{"name":"Bulk One"},{"name":"Bulk Two"}]}')" "submit a bulk job"
[[ "$(field '.status')" == queued && "$(field '.total')" == 2 ]] || fail "bulk job: $(cat "$body")"
bulk=$(field '.id')
discovered_bulk=$(sql --command="SELECT count(*) FROM platform_due_bulk_jobs(100) WHERE id = '$bulk';")
[[ "$discovered_bulk" == 1 ]] || fail "the worker's discovery routine finds the queued bulk job"
expect 200 "$(call "$member_cookie" GET "$workspace/bulk/$bulk/results?status=pending")" "bulk results"
[[ "$(field '[.data[].index] | map(tostring) | join(",")')" == "0,1" ]] || fail "pending rows: $(cat "$body")"
expect 404 "$(call "$viewer_cookie" GET "$workspace/bulk/$bulk")" "other members do not see the job"
expect 200 "$(call "$member_cookie" POST "$workspace/bulk/$bulk/abort")" "abort a bulk job"
[[ "$(field '.status')" == aborted ]] || fail "aborted: $(cat "$body")"
expect 409 "$(call "$member_cookie" POST "$workspace/bulk/$bulk/abort")" "abort only once"
[[ "$(field '.code')" == bulk.job_state ]] || fail "bulk state code: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$workspace/bulk" '{"object":"custom:client","operation":"update","rows":[{"values":{}}]}')" "malformed bulk rows"
[[ "$(field '.errors[0].field')" == "rows[0]" && "$(field '.errors[0].code')" == bulk.row_invalid ]] || fail "row problem: $(cat "$body")"
bulk_audited=$(sql --command="SELECT count(*) FROM audit_log WHERE action IN ('objects.bulk_job.submitted', 'objects.bulk_job.aborted') AND target = 'bulk_job:$bulk';")
[[ "$bulk_audited" == 2 ]] || fail "bulk jobs audited: $bulk_audited"

log "API clients (ADR 0023 §3): client credentials, bearer tokens, route limits, revocation"
expect 403 "$(call "$viewer_cookie" POST "$workspace/api-clients" '{"name":"Not allowed"}')" "API clients need api_clients.manage"
expect 201 "$(call "$member_cookie" POST "$workspace/api-clients" '{"name":"Nightly sync"}')" "create an API client"
api_client=$(field '.id')
client_id=$(field '.client_id')
client_secret=$(field '.client_secret')
[[ -n "$client_secret" && "$client_secret" != null && "$(field '.run_as.id')" == "$member" ]] || fail "client: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/api-clients")" "list API clients"
[[ "$(field '.data[0].client_secret')" == null ]] || fail "secrets are shown only once"
token_call() {
  curl --max-time 20 --silent --output "$body" --write-out '%{http_code}' --request POST "$api_base/oauth/token" "$@"
}
expect 401 "$(token_call --data-urlencode grant_type=client_credentials --data-urlencode "client_id=$client_id" --data-urlencode client_secret=wrong)" "wrong secret"
[[ "$(field '.error')" == invalid_client ]] || fail "oauth error: $(cat "$body")"
expect 400 "$(token_call --data-urlencode grant_type=password --data-urlencode "client_id=$client_id" --data-urlencode "client_secret=$client_secret")" "only client credentials"
[[ "$(field '.error')" == unsupported_grant_type ]] || fail "grant error: $(cat "$body")"
expect 200 "$(token_call --user "$client_id:$client_secret" --data-urlencode grant_type=client_credentials)" "token with HTTP Basic"
[[ "$(field '.token_type')" == Bearer && "$(field '.expires_in')" == 3600 && "$(field '.workspace_id')" == "$tenant" ]] || fail "token: $(cat "$body")"
bearer="Authorization: Bearer $(field '.access_token')"
expect 200 "$(call "$bearer" GET "$workspace/objects")" "the client reads objects"
expect 201 "$(call "$bearer" POST "$workspace/objects/custom:client/records" '{"name":"Synced Client"}')" "the client creates a record as its member"
[[ "$(field '.created_by.id')" == "$member" ]] || fail "client changes are the member's: $(cat "$body")"
expect 200 "$(call "$bearer" POST "$workspace/sync/revocations" '{"records":{"custom:client":["nope"]}}')" "clients reach device sync"
expect 403 "$(call "$bearer" GET "$workspace/api-clients")" "clients cannot manage clients"
[[ "$(field '.code')" == auth.client_route_denied ]] || fail "route limit: $(cat "$body")"
expect 403 "$(call "$bearer" GET "$workspace/users/$member")" "clients reach only engine routes"
# The member directory is a device's (ADR 0024, "Offline directory"), not an API client's.
expect 403 "$(call "$bearer" GET "$workspace/users")" "clients cannot read the member directory"
expect 403 "$(call "$bearer" GET "$workspace/roles")" "clients cannot read the role directory"
expect 401 "$(call "Authorization: Bearer not-a-token" GET "$workspace/objects")" "unknown token"
expect 401 "$(curl --max-time 20 --silent --output "$body" --write-out '%{http_code}' --header "$member_cookie" --header 'Authorization: Bearer ' "$workspace/objects")" "a malformed bearer header does not fall back to the session"
expect 200 "$(call "$member_cookie" POST "$workspace/api-clients/$api_client/revoke")" "revoke the client"
[[ "$(field '.revoked_at')" != null ]] || fail "revoked: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$workspace/api-clients/$api_client/revoke")" "revoking again changes nothing"
expect 401 "$(call "$bearer" GET "$workspace/objects")" "revoking ends its tokens"
expect 401 "$(token_call --user "$client_id:$client_secret" --data-urlencode grant_type=client_credentials)" "revoked clients get no tokens"
client_audited=$(sql --command="SELECT count(*) FROM audit_log WHERE target = 'api_client:$api_client' AND action IN ('api_clients.client.created', 'api_clients.token.issued', 'api_clients.client.revoked');")
[[ "$client_audited" == 3 ]] || fail "API clients audited: $client_audited"
stored_secret=$(sql --command="SELECT count(*) FROM api_clients WHERE secret_hash = '$client_secret' OR client_id = '$client_secret';")
[[ "$stored_secret" == 0 ]] || fail "only the secret's hash is stored"

log "devices (ADR 0029): sign-in codes, rotating tokens, revocation and wipe"
token_form() {
  curl --max-time 20 --silent --output "$body" --write-out '%{http_code}' --request POST "$api_base/oauth/token" "$@"
}
login_status() { curl --max-time 20 --silent --output "$body" --write-out '%{http_code}' "$api_base/auth/device/login?$1"; }
pkce_challenge() { python3 -c 'import base64, hashlib, sys; print(base64.urlsafe_b64encode(hashlib.sha256(sys.argv[1].encode()).digest()).rstrip(b"=").decode())' "$1"; }
app_redirect="com.example.app:/oauth"
challenge=$(pkce_challenge "$(printf 'v%.0s' {1..43})")
expect 400 "$(login_status "redirect_uri=https%3A%2F%2Fevil.example%2Fcb&code_challenge=$challenge&code_challenge_method=S256&state=s")" "unregistered redirect"
[[ "$(field '.code')" == auth.device_redirect_invalid ]] || fail "redirect code: $(cat "$body")"
expect 400 "$(login_status "redirect_uri=com.example.app%3A%2Foauth&code_challenge=short&code_challenge_method=S256&state=s")" "malformed challenge"
[[ "$(field '.code')" == auth.device_challenge_invalid ]] || fail "challenge code"
expect 400 "$(login_status "redirect_uri=com.example.app%3A%2Foauth&code_challenge=$challenge&code_challenge_method=plain&state=s")" "plain PKCE is refused"
# The identity provider's leg is the web's; the test stores the one-time
# code the callback would, for the viewer, and redeems it as the app would.
grant_code() {
  local code=$1 verifier=$2
  sql --command="INSERT INTO device_authorizations (code_hash, user_id, code_challenge, redirect_uri, expires_at)
    VALUES ('$(hash_token "$code")', '$viewer', '$(pkce_challenge "$verifier")', '$app_redirect', now() + interval '2 minutes');"
}
redeem() {
  token_form --data-urlencode grant_type=authorization_code --data-urlencode "code=$1" --data-urlencode "code_verifier=$2" \
    --data-urlencode "redirect_uri=$app_redirect" --data-urlencode "workspace_id=$tenant" \
    --data-urlencode "device_name=$3" --data-urlencode platform=ios
}
verifier="device-verifier-0123456789-0123456789-0123456789"
grant_code code-one "$verifier"
expect 400 "$(redeem code-one "wrong-verifier-0123456789-0123456789-01234567" "Viewer's phone")" "a different verifier"
[[ "$(field '.error')" == invalid_grant ]] || fail "verifier grant error: $(cat "$body")"
expect 400 "$(redeem code-one "$verifier" "Viewer's phone")" "a code is used up by any attempt"
grant_code code-two "$verifier"
expect 200 "$(redeem code-two "$verifier" "Viewer's phone")" "redeem the code"
[[ "$(field '.token_type')" == Bearer && "$(field '.workspace_id')" == "$tenant" && "$(field '.offline_limit_seconds')" == 1209600 ]] || fail "device tokens: $(cat "$body")"
phone=$(field '.device_id')
phone_bearer="Authorization: Bearer $(field '.access_token')"
phone_refresh=$(field '.refresh_token')
expect 200 "$(call "$phone_bearer" GET "$workspace/objects")" "the device reads objects as its member"
expect 200 "$(call "$phone_bearer" POST "$workspace/sync/revocations" '{"records":{}}')" "the device reaches device sync"
expect 200 "$(call "$phone_bearer" GET "$workspace/devices/current")" "the device reads its member and workspace"
[[ "$(field '.member.id')" == "$viewer" && "$(field '.member.display_name')" == Viewer && "$(field '.workspace.id')" == "$tenant" && "$(field '.device.id')" == "$phone" ]] || fail "current device: $(cat "$body")"
[[ "$(field '.member.role_ids | index("'"$viewer_role"'") != null')" == true && "$(field '.member.roles | index("Project viewer") != null')" == true ]] || fail "current device roles: $(cat "$body")"
expect 404 "$(call "$viewer_cookie" GET "$workspace/devices/current")" "a browser session has no current device"
expect 200 "$(call "$phone_bearer" GET "$workspace/users")" "the device reads the member directory"
[[ "$(field '.data | length')" -ge 1 ]] || fail "member directory: $(cat "$body")"
expect 200 "$(call "$phone_bearer" GET "$workspace/roles")" "the device reads the role directory"
expect 403 "$(call "$phone_bearer" GET "$workspace/users/$member")" "the device reads names only, not member records"
expect 403 "$(call "$phone_bearer" GET "$workspace/devices")" "devices are managed by people"
expect 200 "$(call "$phone_bearer" PUT "$workspace/devices/current/push" '{"provider":"apns","token":"apns-token"}')" "the device registers for push"
[[ "$(field '.push')" == apns && "$(field '.current')" == true ]] || fail "push registered: $(cat "$body")"
expect 400 "$(call "$phone_bearer" PUT "$workspace/devices/current/push" '{"provider":"carrier-pigeon","token":"x"}')" "unknown push provider"
expect 404 "$(call "$viewer_cookie" PUT "$workspace/devices/current/push" '{"provider":"apns","token":"x"}')" "a browser session is no device"
expect 200 "$(call "$viewer_cookie" GET "$workspace/devices")" "members list their devices"
[[ "$(field '[.data[] | "\(.name):\(.platform):\(.push)"] | join(",")')" == "Viewer's phone:ios:apns" ]] || fail "device list: $(cat "$body")"
[[ "$(field '.data[0] | has("push_token")')" == false ]] || fail "push tokens are never shown"
expect 200 "$(call "$member_cookie" GET "$workspace/devices")" "devices.manage lists everyone's"
[[ "$(field '.data | length')" == 1 ]] || fail "administrators see the viewer's device"
[[ "$(field '.data[0].member.name | type')" == string ]] || fail "the device list names its member: $(cat "$body")"
stored=$(sql --command="SELECT count(*) FROM device_tokens WHERE token_hash = '$phone_refresh';")
[[ "$stored" == 0 ]] || fail "only token hashes are stored"

log "push notifications (ADR 0030 §3): the worker claims devices whose workspace moved"
due() { sql --command="SELECT count(*) FROM platform_claim_device_notifications(500, 180) WHERE device = '$phone';"; }
[[ "$(due)" == 0 ]] || fail "a device that just registered for push has nothing to be told"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:client/records" '{"name":"Pushed Client"}')" "a change in the workspace"
[[ "$(due)" == 1 ]] || fail "the device is due once its workspace moves"
[[ "$(due)" == 0 ]] || fail "a claimed device waits out the quiet window"

log "refresh tokens rotate, and a reused one revokes the device"
expect 200 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode "refresh_token=$phone_refresh")" "refresh"
rotated_bearer="Authorization: Bearer $(field '.access_token')"
[[ "$(field '.refresh_token')" != "$phone_refresh" ]] || fail "the refresh token rotates"
expect 200 "$(call "$rotated_bearer" GET "$workspace/objects")" "the new access token works"
expect 400 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode "refresh_token=$phone_refresh")" "a refresh token used twice"
[[ "$(field '.error')" == invalid_grant && "$(field '.wipe')" == true ]] || fail "reuse wipes: $(cat "$body")"
expect 401 "$(call "$rotated_bearer" GET "$workspace/objects")" "a revoked device is refused"
[[ "$(field '.code')" == auth.device_wipe_required ]] || fail "wipe instruction: $(cat "$body")"
expect 400 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode refresh_token=unknown)" "unknown refresh token"
[[ "$(field '.wipe')" == null ]] || fail "an unknown token is no wipe"

log "members and administrators revoke devices; a device signs itself out"
grant_code code-three "$verifier"
expect 200 "$(redeem code-three "$verifier" "Viewer's tablet")" "a second device"
tablet=$(field '.device_id')
tablet_bearer="Authorization: Bearer $(field '.access_token')"
tablet_refresh=$(field '.refresh_token')
expect 404 "$(call "$rep_cookie" POST "$workspace/devices/$tablet/revoke")" "members cannot revoke others' devices"
expect 200 "$(call "$member_cookie" POST "$workspace/devices/$tablet/revoke")" "devices.manage revokes"
[[ "$(field '.revoked_reason')" == revoked && "$(field '.revoked_by.id')" == "$member" ]] || fail "revoked: $(cat "$body")"
expect 401 "$(call "$tablet_bearer" GET "$workspace/objects")" "the revoked device is told to wipe"
[[ "$(field '.code')" == auth.device_wipe_required ]] || fail "wipe after revoke"
expect 400 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode "refresh_token=$tablet_refresh")" "revoking ends refresh"
[[ "$(field '.error')" == invalid_grant && "$(field '.wipe')" == true ]] || fail "a revoked device refreshing is told to wipe: $(cat "$body")"
grant_code code-four "$verifier"
expect 200 "$(redeem code-four "$verifier" "Viewer's laptop")" "a third device"
laptop_bearer="Authorization: Bearer $(field '.access_token')"
laptop_refresh=$(field '.refresh_token')
expect 200 "$(call "$laptop_bearer" DELETE "$workspace/devices/current")" "sign out"
[[ "$(field '.revoked_reason')" == signed_out ]] || fail "signed out: $(cat "$body")"
expect 401 "$(call "$laptop_bearer" GET "$workspace/objects")" "a signed-out device wipes too"
expect 400 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode "refresh_token=$laptop_refresh")" "a signed-out device refreshing"
[[ "$(field '.wipe')" == true ]] || fail "refresh after sign-out wipes: $(cat "$body")"
audited=$(sql --command="SELECT string_agg(action || ':' || coalesce(metadata->>'reason', ''), ',' ORDER BY occurred_at, id) FROM audit_log WHERE action LIKE 'devices.%';")
[[ "$audited" == "devices.device.registered:,devices.device.revoked:refresh_reused,devices.device.registered:,devices.device.revoked:revoked,devices.device.registered:,devices.device.revoked:signed_out" ]] || fail "device evidence: $audited"

log "forced RLS: the application role sees no engine rows without tenant context"
for table in object_definitions field_definitions object_permissions records record_index record_unique record_references record_shares reporting_lines record_history record_changes change_counters record_search_terms validation_rules quick_actions field_reference_objects automations automation_runs automation_jobs bulk_jobs bulk_job_rows api_clients api_client_tokens formula_index_marks devices device_tokens timeline_sources; do
  rows=$(app_sql --command="SELECT count(*) FROM $table;")
  [[ "$rows" == 0 ]] || fail "$table leaks $rows rows without tenant context"
  forced=$(sql --command="SELECT relforcerowsecurity FROM pg_class WHERE relname = '$table';")
  [[ "$forced" == t ]] || fail "$table forces RLS"
done

log "evidence names changes and carries no record values"
created=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.record-created.v1';")
(( created >= 4 )) || fail "record-created events: $created"
audited=$(sql --command="SELECT count(*) FROM audit_log WHERE action LIKE 'objects.%';")
(( audited >= 1 )) || fail "engine mutations audited"
leaks=$(sql --command="
  SELECT (SELECT count(*) FROM outbox WHERE envelope::text LIKE '%AP-1%' OR envelope::text LIKE '%Analytical Engines%')
       + (SELECT count(*) FROM audit_log WHERE metadata::text LIKE '%AP-1%' OR metadata::text LIKE '%Analytical Engines%');")
[[ "$leaks" == 0 ]] || fail "record values leaked into events or audit"

log "sharing (ADR 0016): private records, the reporting line, shares and transfer"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"deal","label":"Deal","plural_label":"Deals","sharing_model":"private"}')" "private object"
[[ "$(field '.sharing_model')" == private && "$(field '.grant_hierarchy')" == true ]] || fail "sharing settings: $(cat "$body")"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:deal/permissions/$sales_role" '{"create":true,"delete":true}')" "sales access"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:deal/permissions/$viewer_role" '{"read":true}')" "viewer reads deals"
expect 403 "$(call "$rep_cookie" PUT "$workspace/users/$rep/manager" "{\"manager_id\":\"$manager\"}")" "members cannot set reporting lines"
expect 200 "$(call "$member_cookie" PUT "$workspace/users/$rep/manager" "{\"manager_id\":\"$manager\"}")" "rep reports to manager"
expect 409 "$(call "$member_cookie" PUT "$workspace/users/$manager/manager" "{\"manager_id\":\"$rep\"}")" "no reporting cycles"
[[ "$(field '.code')" == members.reporting_cycle ]] || fail "cycle code"
expect 404 "$(call "$member_cookie" PUT "$workspace/users/$rep/manager" '{"manager_id":"018f0000-0000-7000-8000-00000000ffff"}')" "managers are members"
expect 200 "$(call "$viewer_cookie" GET "$workspace/users/$rep")" "user record shows the manager"
[[ "$(field '.manager.id')" == "$manager" && "$(field '.manager.name')" == "Morgan Manager" ]] || fail "manager reference: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$workspace/users")" "member directory"
[[ "$(field "[.data[] | select(.id == \"$rep\") | .manager_id] | first")" == "$manager" ]] || fail "directory shows reporting lines"

expect 201 "$(call "$rep_cookie" POST "$workspace/objects/custom:deal/records" '{"name":"Rep deal"}')" "rep creates a deal"
deal=$(field '.id')
deal_version=$(field '.version')
expect 200 "$(call "$rep_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "owner reads"
[[ "$(field '.access | [.read, .edit, .delete, .share, .transfer] | all')" == true ]] || fail "owner rights: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records")" "manager list"
[[ "$(field '[.data[].id] | join(",")')" == "$deal" ]] || fail "managers see their reports' records"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "manager reads"
[[ "$(field '.access.edit')" == true && "$(field '.access.transfer')" == false ]] || fail "manager rights: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records")" "viewer list"
[[ "$(field '.data | length')" == 0 ]] || fail "private records are hidden from peers"
expect 404 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "invisible record is missing"

expect 404 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records/$deal/sharing")" "sharing of an invisible record is missing too"
expect 400 "$(call "$rep_cookie" PUT "$workspace/objects/custom:deal/records/$deal/sharing/user/018f0000-0000-7000-8000-00000000ffff" '{"access":"read"}')" "unknown grantee"
[[ "$(field '.errors[0].code')" == records.grantee_unknown ]] || fail "grantee code"
expect 200 "$(call "$rep_cookie" PUT "$workspace/objects/custom:deal/records/$deal/sharing/user/$viewer" '{"access":"read"}')" "share with viewer"
expect 200 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "shared record is visible"
[[ "$(field '.access.edit')" == false ]] || fail "a read share does not edit"
expect 200 "$(call "$viewer_cookie" POST "$workspace/sync/revocations" "{\"records\":{\"custom:deal\":[\"$deal\",\"nope\"]}}")" "revocation set (ADR 0024)"
[[ "$(field '.revoked | tojson')" == '{"custom:deal":["nope"]}' ]] || fail "a shared record stays on the device: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records/$deal/sharing")" "grantees cannot see or change shares"
expect 200 "$(call "$rep_cookie" GET "$workspace/objects/custom:deal/records/$deal/sharing")" "sharing list"
[[ "$(field '[.shares[] | "\(.grantee_kind):\(.grantee_name):\(.access)"] | join(",")')" == "user:Viewer:read" ]] || fail "shares named: $(cat "$body")"
[[ "$(field '.owner.name')" == "Riley Rep" ]] || fail "owner named"
expect 200 "$(call "$rep_cookie" PUT "$workspace/objects/custom:deal/records/$deal/sharing/role/$sales_role" '{"access":"edit"}')" "role share"

log "transfer moves the record and removes its shares"
expect 400 "$(call "$rep_cookie" POST "$workspace/objects/custom:deal/records/$deal/owner" "{\"owner_id\":\"018f0000-0000-7000-8000-00000000ffff\",\"expected_version\":$deal_version}")" "transfer to a non-member"
[[ "$(field '.errors[0].code')" == records.owner_unknown ]] || fail "owner code"
expect 403 "$(call "$manager_cookie" POST "$workspace/objects/custom:deal/records/$deal/owner" "{\"owner_id\":\"$manager\",\"expected_version\":$deal_version}")" "managers need records.transfer"
expect 200 "$(call "$rep_cookie" POST "$workspace/objects/custom:deal/records/$deal/owner" "{\"owner_id\":\"$manager\",\"expected_version\":$deal_version}")" "rep transfers to manager"
[[ "$(field '.owner.id')" == "$manager" && "$(field '.version')" == $((deal_version + 1)) ]] || fail "transferred: $(cat "$body")"
expect 404 "$(call "$viewer_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "a transfer removes manual shares"
expect 200 "$(call "$viewer_cookie" POST "$workspace/sync/revocations" "{\"records\":{\"custom:deal\":[\"$deal\"]}}")" "revocation after transfer"
[[ "$(field '.revoked["custom:deal"][0]')" == "$deal" ]] || fail "a record no longer shared is revoked: $(cat "$body")"
expect 400 "$(call "$viewer_cookie" POST "$workspace/sync/revocations" '{"held":{}}')" "unknown revocation body"
expect 404 "$(call "$rep_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "the previous owner loses a private record"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal/sharing")" "new owner manages sharing"
[[ "$(field '.shares | length')" == 0 ]] || fail "no shares after transfer"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:deal/records")" "modify all sees every deal"
[[ "$(field '.data | length')" == 1 ]] || fail "admin bypasses sharing"
for event in record-owner-changed record-sharing-changed; do
  emitted=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.$event.v1';")
  (( emitted >= 1 )) || fail "$event emitted"
done
lines=$(sql --command="SELECT count(*) FROM audit_log WHERE action = 'members.reporting_line.changed';")
(( lines == 1 )) || fail "reporting line change audited once, got $lines"

log "master-detail (ADR 0025): details follow their master's sharing and recycle bin"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"deal_line","label":"Deal line","plural_label":"Deal lines"}')" "detail object"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:deal_line/fields" \
  '{"name":"deal","label":"Deal","field_type":"master_detail","reference_object":"custom:deal"}')" "master-detail field"
[[ "$(field '.required')" == true && "$(field '.delete_behaviour')" == null ]] || fail "master-detail shape: $(cat "$body")"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:deal_line/permissions/$sales_role" '{"create":true,"delete":true}')" "sales access to lines"
expect 201 "$(call "$manager_cookie" POST "$workspace/objects/custom:deal_line/records" "{\"name\":\"Seats\",\"custom:deal\":\"$deal\"}")" "the deal's owner adds a line"
line=$(field '.id')
expect 400 "$(call "$rep_cookie" POST "$workspace/objects/custom:deal_line/records" "{\"name\":\"Sneaky\",\"custom:deal\":\"$deal\"}")" "a member who cannot see the deal adds no line"
[[ "$(field '.errors[0].code')" == records.reference_not_found ]] || fail "hidden master code: $(cat "$body")"
expect 404 "$(call "$rep_cookie" GET "$workspace/objects/custom:deal_line/records/$line")" "the line follows the deal's sharing"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal_line/records/$line")" "the deal's owner reads the line"
[[ "$(field '.access.edit')" == true && "$(field '.access.share')" == false ]] || fail "detail rights: $(cat "$body")"
expect 409 "$(call "$member_cookie" POST "$workspace/objects/custom:deal_line/fields" \
  '{"name":"client","label":"Client","field_type":"master_detail","reference_object":"custom:client"}')" "a master-detail field needs an object without records"
[[ "$(field '.code')" == objects.master_detail_has_records ]] || fail "has records code: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "deal before delete"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:deal/records/$deal?expected_version=$(field '.version')")" "delete the master"
deleted_deal_version=$(field '.version')
expect 404 "$(call "$member_cookie" GET "$workspace/objects/custom:deal_line/records/$line")" "the line went to the recycle bin"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:deal_line/records/$line?include_deleted=true")" "the line is in the recycle bin"
expect 409 "$(call "$member_cookie" POST "$workspace/objects/custom:deal_line/records/$line/undelete?expected_version=$(field '.version')")" "no detail without its master"
[[ "$(field '.code')" == records.master_deleted ]] || fail "master deleted code: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$workspace/objects/custom:deal/records/$deal/undelete?expected_version=$deleted_deal_version")" "restore the master"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal_line/records/$line")" "the line came back with its master"

log "roll-up summaries (ADR 0026): the deal counts its lines"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:deal/fields" \
  '{"name":"line_count","label":"Lines","field_type":"rollup","rollup":{"function":"count","object":"custom:deal_line","relationship":"custom:deal"}}')" "roll-up field"
[[ "$(field '.formula_type')" == number && "$(field '.rollup.function')" == count ]] || fail "roll-up shape: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "deal with its count"
[[ "$(field '."custom:line_count"')" == 1 ]] || fail "existing lines counted: $(cat "$body")"
expect 201 "$(call "$manager_cookie" POST "$workspace/objects/custom:deal_line/records" "{\"name\":\"Support\",\"custom:deal\":\"$deal\"}")" "another line"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal")" "deal after the new line"
[[ "$(field '."custom:line_count"')" == 2 ]] || fail "new line counted: $(cat "$body")"
expect 400 "$(call "$manager_cookie" PATCH "$workspace/objects/custom:deal/records/$deal" "{\"expected_version\":$(field '.version'),\"custom:line_count\":9}")" "roll-ups are read-only"
[[ "$(field '.errors[0].code')" == records.field_read_only ]] || fail "read-only code: $(cat "$body")"

log "saved reports (ADR 0027): shared questions, answered as each viewer"
expect 201 "$(call "$member_cookie" POST "$workspace/reports" \
  '{"name":"Deals","query":"SELECT name FROM custom:deal ORDER BY name","visibility":"workspace"}')" "workspace report"
report=$(field '.id')
expect 400 "$(call "$member_cookie" POST "$workspace/reports" '{"name":"Broken","query":"SELECT nope FROM custom:deal"}')" "a report's query must run"
[[ "$(field '.errors[0].field')" == query ]] || fail "query problems are addressed to the query: $(cat "$body")"
expect 403 "$(call "$viewer_cookie" POST "$workspace/reports" '{"name":"Mine","query":"SELECT name FROM custom:deal","visibility":"workspace"}')" "sharing reports needs reports.manage"
expect 201 "$(call "$rep_cookie" POST "$workspace/reports" '{"name":"Rep only","query":"SELECT name FROM custom:deal"}')" "private report"
private=$(field '.id')
expect 200 "$(call "$viewer_cookie" GET "$workspace/reports")" "viewer's reports"
[[ "$(field '[.data[].name] | join(",")')" == Deals ]] || fail "private reports stay private: $(cat "$body")"
[[ "$(field '.data[0].can_edit')" == false ]] || fail "viewers cannot edit someone else's report: $(cat "$body")"
expect 404 "$(call "$viewer_cookie" GET "$workspace/reports/$private")" "someone else's private report is missing"
expect 200 "$(call "$manager_cookie" GET "$workspace/reports/$report")" "the deal's owner runs the report"
[[ "$(field '.result.data | length')" == 1 ]] || fail "owner sees the deal: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$workspace/reports/$report")" "a viewer runs the same report"
[[ "$(field '.result.data | length')" == 0 ]] || fail "the report runs with the viewer's sharing: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/reports/$report")" "the author runs the report"
[[ "$(field '.can_edit')" == true ]] || fail "the author may edit the report: $(cat "$body")"
expect 200 "$(call "$viewer_cookie" GET "$workspace/ui/contributions")" "navigation"
[[ "$(field '[.navigation[] | select(.id == "reports") | .route] | join(",")')" == /reports ]] || fail "reports in navigation: $(cat "$body")"

log "related lists and lookup filters (ADR 0028): read as the viewer"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal/related")" "the deal's related lists"
[[ "$(field '[.data[] | select(.object == "custom:deal_line" and .field == "custom:deal") | .data[].name] | join(",")')" == Seats,Support ]] || fail "the deal lists its lines: $(cat "$body")"
expect 404 "$(call "$rep_cookie" GET "$workspace/objects/custom:deal/records/$deal/related")" "no related lists for a hidden record"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:deal_line")" "detail object"
deal_field_version=$(field '[.fields[] | select(.api_name == "custom:deal") | .version] | first')
expect 400 "$(call "$member_cookie" PATCH "$workspace/objects/custom:deal_line/fields/custom:deal" "{\"expected_version\":$deal_field_version,\"lookup_filter\":\"nope = 1\"}")" "a lookup filter must run"
[[ "$(field '.errors[0].field')" == lookup_filter ]] || fail "filter problems are addressed to the filter: $(cat "$body")"
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:deal_line/fields/custom:deal" "{\"expected_version\":$deal_field_version,\"lookup_filter\":\"name = 'no such deal'\"}")" "filter the master field"
[[ "$(field '.lookup_filter')" == "name = 'no such deal'" ]] || fail "filter kept: $(cat "$body")"
deal_field_version=$(field '.version')
expect 400 "$(call "$manager_cookie" POST "$workspace/objects/custom:deal_line/records" "{\"name\":\"Training\",\"custom:deal\":\"$deal\"}")" "new values must meet the filter"
[[ "$(field '.errors[0].code')" == records.lookup_filter_mismatch ]] || fail "mismatch code: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal_line/fields/custom:deal/candidates")" "candidates within the filter"
[[ "$(field '.data | length')" == 0 ]] || fail "no deal meets the filter: $(cat "$body")"
expect 200 "$(call "$member_cookie" PATCH "$workspace/objects/custom:deal_line/fields/custom:deal" "{\"expected_version\":$deal_field_version,\"lookup_filter\":null}")" "remove the filter"
[[ "$(field '.lookup_filter')" == null ]] || fail "filter removed: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal_line/fields/custom:deal/candidates")" "candidates without a filter"
[[ "$(field "any(.data[]; .id == \"$deal\")")" == true ]] || fail "the deal is offered: $(cat "$body")"

log "related-to fields (ADR 0033 §1): a reference to a record of any allowed object"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"activity","label":"Activity","plural_label":"Activities","sharing_model":"private"}')" "activity object"
expect 200 "$(call "$member_cookie" PUT "$workspace/objects/custom:activity/permissions/$sales_role" '{"create":true,"edit":true,"delete":true}')" "sales access to activities"
activity_fields="$workspace/objects/custom:activity/fields"
expect 400 "$(call "$member_cookie" POST "$activity_fields" '{"name":"regarding","label":"Regarding","field_type":"related_to"}')" "a related-to field needs its objects"
[[ "$(field '.errors[0].code')" == objects.reference_objects_invalid && "$(field '.errors[0].field')" == reference_objects ]] || fail "allowlist required: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$activity_fields" '{"name":"regarding","label":"Regarding","field_type":"related_to","reference_objects":["custom:deal","custom:deal"]}')" "objects are distinct"
[[ "$(field '.errors[0].code')" == objects.reference_objects_invalid ]] || fail "duplicate objects: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$activity_fields" '{"name":"regarding","label":"Regarding","field_type":"related_to","reference_objects":["custom:deal","custom:nope"]}')" "objects exist"
[[ "$(field '.errors[0].code')" == objects.reference_object_unknown ]] || fail "unknown object: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$activity_fields" '{"name":"regarding","label":"Regarding","field_type":"related_to","reference_objects":["custom:deal"],"lookup_filter":"name = '"'x'"'"}')" "no lookup filter yet"
[[ "$(field '.errors[0].code')" == objects.lookup_filter_not_supported ]] || fail "lookup filter refused: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$activity_fields" '{"name":"regarding","label":"Regarding","field_type":"related_to","reference_objects":["custom:deal","custom:client"]}')" "related-to field"
[[ "$(field '.field_type')" == related_to && "$(field '.reference_objects | join(",")')" == custom:deal,custom:client && "$(field '.reference_object')" == null && "$(field '.delete_behaviour')" == clear ]] || fail "related-to shape: $(cat "$body")"
regarding_version=$(field '.version')
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:activity")" "describe the activity"
[[ "$(field '[.fields[] | select(.api_name == "custom:regarding") | .reference_objects | join(",")] | first')" == custom:deal,custom:client ]] || fail "described allowlist: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$activity_fields" '{"name":"about","label":"About","field_type":"formula","formula":"custom:regarding","formula_type":"text"}')" "formulas cannot use it"
[[ "$(field '.errors[0].code')" == objects.formula_invalid ]] || fail "formula refused: $(cat "$body")"

expect 201 "$(call "$manager_cookie" POST "$workspace/objects/custom:activity/records" "{\"name\":\"Call about the deal\",\"custom:regarding\":\"$deal\"}")" "the deal's owner logs an activity"
activity=$(field '.id')
[[ "$(field '."custom:regarding"')" == "$deal" && "$(field ".related[\"$deal\"]")" == "Rep deal" && "$(field ".related_objects[\"$deal\"]")" == custom:deal ]] || fail "related-to target named with its object: $(cat "$body")"
expect 400 "$(call "$rep_cookie" POST "$workspace/objects/custom:activity/records" "{\"name\":\"Sneaky\",\"custom:regarding\":\"$deal\"}")" "a target the writer cannot see"
[[ "$(field '.errors[0].code')" == records.reference_not_found ]] || fail "hidden target code: $(cat "$body")"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:activity/records" "{\"name\":\"Project call\",\"custom:regarding\":\"$apollo\"}")" "an object the field does not allow"
[[ "$(field '.errors[0].code')" == records.reference_object_not_allowed ]] || fail "disallowed object code: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$(query "SELECT name FROM custom:activity WHERE custom:regarding IN ('$deal', '$client')")")" "filter a related-to field"
[[ "$(field '[.data[].id] | join(",")')" == "$activity" ]] || fail "related-to filter: $(cat "$body")"
expect 400 "$(call "$manager_cookie" GET "$(query "SELECT name FROM custom:activity WHERE custom:regarding.name = 'x'")")" "no traversal through a related-to field"
[[ "$(field '.errors[0].code')" == query.relationship_unsupported ]] || fail "traversal code: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:deal/records/$deal/related")" "the deal's related lists"
[[ "$(field '[.data[] | select(.object == "custom:activity" and .field == "custom:regarding" and .field_type == "related_to") | .data[].name] | join(",")')" == "Call about the deal" ]] || fail "the deal lists its activities: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:activity/fields/custom:regarding/candidates?object=custom:deal")" "candidates of one allowed object"
[[ "$(field "any(.data[]; .id == \"$deal\")")" == true ]] || fail "the deal is offered: $(cat "$body")"
expect 400 "$(call "$manager_cookie" GET "$workspace/objects/custom:activity/fields/custom:regarding/candidates")" "candidates need an allowed object"
[[ "$(field '.errors[0].code')" == records.reference_object_not_allowed ]] || fail "candidates code: $(cat "$body")"

expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:deal/quick-actions" '{"name":"log_call","label":"Log a call","kind":"create","target_object":"custom:activity","target_field":"custom:regarding","fields":["name"]}')" "a quick action fills a related-to field"
expect 400 "$(call "$member_cookie" POST "$workspace/objects/custom:project/quick-actions" '{"name":"log_call","label":"Log a call","kind":"create","target_object":"custom:activity","target_field":"custom:regarding","fields":["name"]}')" "only from an allowed object"
[[ "$(field '.errors[0].code')" == quick_actions.target_field_invalid ]] || fail "quick action target code: $(cat "$body")"
expect 200 "$(call "$member_cookie" PATCH "$activity_fields/custom:regarding" "{\"expected_version\":$regarding_version,\"reference_objects\":[\"custom:client\"]}")" "take deals out of the allowlist"
[[ "$(field '.reference_objects | join(",")')" == custom:client ]] || fail "narrowed allowlist: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:deal/quick-actions")" "the deal's quick actions"
[[ "$(field '.data | length')" == 0 ]] || fail "the action of a removed object went with it: $(cat "$body")"
expect 200 "$(call "$manager_cookie" GET "$workspace/objects/custom:activity/records/$activity")" "a value to a removed object stays"
[[ "$(field '."custom:regarding"')" == "$deal" ]] || fail "value kept: $(cat "$body")"
expect 400 "$(call "$manager_cookie" POST "$workspace/objects/custom:activity/records" "{\"name\":\"Another call\",\"custom:regarding\":\"$deal\"}")" "no new values to a removed object"
[[ "$(field '.errors[0].code')" == records.reference_object_not_allowed ]] || fail "removed object code: $(cat "$body")"

expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:client/records" '{"name":"Short-lived client"}')" "a client to delete"
short_lived=$(field '.id')
short_lived_version=$(field '.version')
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:activity/records" "{\"name\":\"Client call\",\"custom:regarding\":\"$short_lived\"}")" "an activity about the client"
client_activity=$(field '.id')
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:client/records/$short_lived?expected_version=$short_lived_version")" "delete the target"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:activity/records/$client_activity")" "the activity after its target went"
[[ "$(field '."custom:regarding"')" == null && "$(field '.version')" == 2 ]] || fail "deleting the target clears the value: $(cat "$body")"
leaks=$(sql --command="SELECT count(*) FROM outbox WHERE (envelope::text LIKE '%$activity%' OR envelope::text LIKE '%$client_activity%') AND (envelope::text LIKE '%$deal%' OR envelope::text LIKE '%$short_lived%');")
[[ "$leaks" == 0 ]] || fail "related-to values leaked into events"
allowlists=$(sql --command="SELECT count(*) FROM field_reference_objects WHERE tenant_id = '$tenant';")
(( allowlists == 1 )) || fail "the allowlist is stored per field: $allowlists"
[[ "$(app_sql --command="SELECT count(*) FROM field_reference_objects;")" == 0 ]] || fail "field_reference_objects leaks rows without tenant context"

log "plugins declare engine objects (ADR 0033 §2): enabling installs them, disabling hides them"
tracker="$workspace/plugins/example.tracker"
ticket_api="$workspace/objects/example.tracker:ticket"
expect 404 "$(call "$member_cookie" GET "$ticket_api")" "no tracker objects before the plugin is enabled"
expect 200 "$(call "$member_cookie" POST "$tracker/enable")" "enable the tracker"
expect 200 "$(call "$member_cookie" GET "$ticket_api")" "describe a plugin object"
[[ "$(field '.origin')" == plugin && "$(field '.owner_plugin')" == example.tracker && "$(field '.key_prefix')" == tkt ]] || fail "plugin object metadata: $(cat "$body")"
[[ "$(field '[.fields[] | select(.origin == "plugin") | .api_name] | join(",")')" == "example.tracker:status,example.tracker:summary,example.tracker:milestone" ]] || fail "plugin fields: $(cat "$body")"
summary_version=$(field '.fields[] | select(.api_name == "example.tracker:summary") | .version')
ticket_object_version=$(field '.version')
expect 409 "$(call "$member_cookie" PATCH "$ticket_api/fields/example.tracker:summary" "{\"expected_version\":$summary_version,\"label\":\"Mine\"}")" "administrators cannot change a plugin's field"
[[ "$(field '.code')" == objects.plugin_owned ]] || fail "plugin field code: $(cat "$body")"
expect 409 "$(call "$member_cookie" DELETE "$ticket_api/fields/example.tracker:summary?expected_version=$summary_version")" "administrators cannot delete a plugin's field"
[[ "$(field '.code')" == objects.plugin_owned ]] || fail "plugin field delete code: $(cat "$body")"
expect 409 "$(call "$member_cookie" DELETE "$ticket_api?expected_version=$ticket_object_version")" "administrators cannot delete a plugin's object"
[[ "$(field '.code')" == objects.plugin_owned ]] || fail "plugin object delete code: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$ticket_api/fields" '{"name":"priority","label":"Priority","field_type":"number"}')" "administrators add custom fields to plugin objects"
[[ "$(field '.api_name')" == custom:priority && "$(field '.origin')" == workspace ]] || fail "custom field on a plugin object: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/example.tracker:milestone/records" '{"name":"Launch"}')" "a milestone"
launch=$(field '.id')
[[ "$launch" == tkm* ]] || fail "plugin key prefix in public ids: $launch"
expect 201 "$(call "$member_cookie" POST "$ticket_api/records" "{\"name\":\"Ship it\",\"example.tracker:summary\":\"Ship the release\",\"example.tracker:milestone\":\"$launch\",\"custom:priority\":2}")" "a ticket"
ship=$(field '.id')
ship_version=$(field '.version')
[[ "$(field '."example.tracker:status"')" == open && "$(field ".related[\"$launch\"]")" == Launch ]] || fail "ticket values: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$tracker/enable")" "enabling again is idempotent"
expect 200 "$(call "$member_cookie" GET "$ticket_api")" "describe after re-enabling"
[[ "$(field '.version')" == "$ticket_object_version" && "$(field '.fields | length')" == 4 ]] || fail "re-enabling changed the object: $(cat "$body")"
by_plugin=$(sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND action = 'objects.object.created' AND metadata->>'plugin' = 'example.tracker' AND actor_id = '$member';")
[[ "$by_plugin" == 2 ]] || fail "installation evidence names the plugin and the member: $by_plugin"
expect 200 "$(call "$member_cookie" POST "$tracker/disable")" "disable the tracker"
expect 404 "$(call "$member_cookie" GET "$ticket_api")" "a disabled plugin's objects are hidden"
expect 404 "$(call "$member_cookie" GET "$ticket_api/records/$ship")" "and so are their records"
expect 200 "$(call "$member_cookie" GET "$workspace/objects")" "list objects while disabled"
[[ "$(field '[.data[].api_name | select(startswith("example.tracker"))] | length')" == 0 ]] || fail "listed while disabled: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$tracker/enable")" "enable the tracker again"
expect 200 "$(call "$member_cookie" GET "$ticket_api/records/$ship")" "records come back unchanged"
[[ "$(field '.version')" == "$ship_version" && "$(field '."custom:priority" == 2')" == true ]] || fail "record after re-enabling: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$tracker/disable")" "disable before removing"
expect 200 "$(call "$member_cookie" POST "$tracker/remove")" "remove the tracker"
kept=$(sql --command="SELECT count(*) FROM records r JOIN object_definitions o ON o.tenant_id = r.tenant_id AND o.id = r.object_id WHERE r.tenant_id = '$tenant' AND o.owner_plugin = 'example.tracker' AND o.suspended;")
[[ "$kept" == 2 ]] || fail "removal keeps the plugin's records for export: $kept"
expect 404 "$(call "$member_cookie" GET "$ticket_api")" "a removed plugin's objects are hidden"

log "the Activities plugin (ADR 0033 §3): administrators choose Related to's objects, which gain Log a call"
activities="$workspace/plugins/platform.activities"
task_api="$workspace/objects/platform.activities:task"
expect 404 "$(call "$member_cookie" GET "$task_api")" "no tasks before Activities is enabled"
expect 200 "$(call "$member_cookie" POST "$activities/enable")" "enable Activities"
expect 200 "$(call "$member_cookie" GET "$task_api")" "describe tasks"
[[ "$(field '.owner_plugin')" == platform.activities && "$(field '.key_prefix')" == tsk && "$(field '.name_label')" == Subject ]] || fail "task metadata: $(cat "$body")"
[[ "$(field '[.fields[] | select(.field_type == "related_to") | .reference_objects | length] | join(",")')" == "0,0" ]] || fail "related-to allowlists start empty: $(cat "$body")"
related_to_version=$(field '.fields[] | select(.api_name == "platform.activities:related_to") | .version')
expect 200 "$(call "$member_cookie" GET "$workspace/objects/platform.activities:event")" "describe events"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"account","label":"Account","plural_label":"Accounts","in_navigation":false}')" "an account object"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:account/records" '{"name":"Initech"}')" "an account"
initech=$(field '.id')
expect 400 "$(call "$member_cookie" POST "$task_api/records" "{\"name\":\"Call Initech\",\"platform.activities:related_to\":\"$initech\"}")" "an empty allowlist refuses values"
[[ "$(field '.errors[0].code')" == records.reference_object_not_allowed ]] || fail "empty allowlist code: $(cat "$body")"
expect 409 "$(call "$member_cookie" PATCH "$task_api/fields/platform.activities:related_to" "{\"expected_version\":$related_to_version,\"label\":\"About\",\"reference_objects\":[\"custom:account\"]}")" "only the allowlist of a plugin field changes"
[[ "$(field '.code')" == objects.plugin_owned ]] || fail "plugin field code: $(cat "$body")"
expect 200 "$(call "$member_cookie" PATCH "$task_api/fields/platform.activities:related_to" "{\"expected_version\":$related_to_version,\"reference_objects\":[\"custom:account\"]}")" "administrators choose Related to's objects"
[[ "$(field '.reference_objects | join(",")')" == custom:account && "$(field '.origin')" == plugin ]] || fail "allowlist: $(cat "$body")"
related_to_version=$(field '.version')
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:account/quick-actions")" "the account's quick actions"
[[ "$(field '[.data[] | select(.name == "log_a_call" and .kind == "create" and .target_object == "platform.activities:task" and .target_field == "platform.activities:related_to")] | length')" == 1 ]] || fail "the account gained Log a call: $(cat "$body")"
log_a_call=$(jq -c '.data[] | select(.name == "log_a_call")' "$body")
[[ "$(jq -r '.label' <<<"$log_a_call")" == "Log a call" && "$(jq -r '.fields | join(",")' <<<"$log_a_call")" == "name,platform.activities:description" ]] || fail "Log a call form: $log_a_call"
# The record page runs it through the normal create API: the form's values,
# the presets and the related-to field set to the current record.
call_body=$(jq -c --arg record "$initech" '.presets + {"name":"Called Initech","platform.activities:description":"Renewal next quarter","platform.activities:related_to":$record}' <<<"$log_a_call")
expect 201 "$(call "$member_cookie" POST "$task_api/records" "$call_body")" "log a call"
logged_call=$(field '.id')
[[ "$logged_call" == tsk* && "$(field '."platform.activities:status"')" == completed && "$(field '."platform.activities:type"')" == call && "$(field '."platform.activities:related_to"')" == "$initech" && "$(field ".related_objects[\"$initech\"]")" == custom:account ]] || fail "a completed call about the account: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:account/records/$initech/related")" "the account's related lists"
[[ "$(field '[.data[] | select(.object == "platform.activities:task") | .data[].name] | join(",")')" == "Called Initech" ]] || fail "the account lists its tasks: $(cat "$body")"
created_by_member=$(sql --command="SELECT count(*) FROM audit_log WHERE tenant_id = '$tenant' AND action = 'objects.quick_action.created' AND target = 'quick_action:custom:account/log_a_call' AND actor_id = '$member';")
[[ "$created_by_member" == 1 ]] || fail "Log a call was created as the administrator's change: $created_by_member"
expect 200 "$(call "$member_cookie" PATCH "$task_api/fields/platform.activities:related_to" "{\"expected_version\":$related_to_version,\"reference_objects\":[]}")" "take the account out again"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:account/quick-actions")" "the account's quick actions after"
[[ "$(field '.data | length')" == 0 ]] || fail "Log a call left with the account: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$task_api/records/$logged_call")" "the call keeps its value"
[[ "$(field '."platform.activities:related_to"')" == "$initech" ]] || fail "value kept: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$activities/disable")" "disable Activities"
expect 404 "$(call "$member_cookie" GET "$task_api")" "a disabled plugin's tasks are hidden"
expect 404 "$(call "$member_cookie" GET "$task_api/records/$logged_call")" "and so is the call"
expect 200 "$(call "$member_cookie" POST "$activities/enable")" "enable Activities again"
expect 200 "$(call "$member_cookie" GET "$task_api/records/$logged_call")" "the call comes back"
[[ "$(app_sql --command="SELECT count(*) FROM related_actions;")" == 0 ]] || fail "related_actions leaks rows without tenant context"
[[ "$(sql --command="SELECT count(*) FROM related_actions WHERE tenant_id = '$tenant';")" == 1 ]] || fail "Log a call is kept per workspace"

log "the Activity timeline (ADR 0033 §4): tasks and events on the records they point at, as the viewer"
event_api="$workspace/objects/platform.activities:event"
timeline="$workspace/objects/custom:account/records/$initech/timeline"
# The call keeps its value although the account left Related to's allowlist.
expect 200 "$(call "$member_cookie" GET "$timeline")" "the account's timeline"
[[ "$(field '.sources')" == 1 && "$(field '[.past[].name] | join(",")')" == "Called Initech" && "$(field '.past[0].through')" == platform.activities:related_to && "$(field '.open | length')" == 0 && "$(field '.actions | length')" == 0 ]] || fail "a kept value still shows: $(cat "$body")"
[[ "$(field '.past[0].object_label')" == Task && "$(field '.past[0].date')" == null && "$(field '[.past[0].details[] | .label + "=" + .value] | join(",")')" == "Status=Completed,Priority=Normal,Type=Call" ]] || fail "timeline item: $(cat "$body")"
# Back in both allowlists: Log a call returns, and Event joins the timeline.
expect 200 "$(call "$member_cookie" GET "$task_api")" "describe tasks"
related_to_version=$(field '.fields[] | select(.api_name == "platform.activities:related_to") | .version')
expect 200 "$(call "$member_cookie" PATCH "$task_api/fields/platform.activities:related_to" "{\"expected_version\":$related_to_version,\"reference_objects\":[\"custom:account\"]}")" "the account enters Related to again"
expect 200 "$(call "$member_cookie" GET "$event_api")" "describe events"
event_related_version=$(field '.fields[] | select(.api_name == "platform.activities:related_to") | .version')
expect 200 "$(call "$member_cookie" PATCH "$event_api/fields/platform.activities:related_to" "{\"expected_version\":$event_related_version,\"reference_objects\":[\"custom:account\"]}")" "events may be about accounts"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"contact","label":"Contact","plural_label":"Contacts","in_navigation":false}')" "a contact object"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:contact/records" '{"name":"Peter Gibbons"}')" "a contact"
peter=$(field '.id')
expect 200 "$(call "$member_cookie" GET "$task_api")" "describe tasks again"
who_version=$(field '.fields[] | select(.api_name == "platform.activities:who") | .version')
expect 200 "$(call "$member_cookie" PATCH "$task_api/fields/platform.activities:who" "{\"expected_version\":$who_version,\"reference_objects\":[\"custom:contact\"]}")" "Who points at contacts"
tomorrow=$(date -u -d tomorrow +%F)
yesterday=$(date -u -d yesterday +%F)
expect 201 "$(call "$member_cookie" POST "$task_api/records" "{\"name\":\"Send the TPS report\",\"platform.activities:due_date\":\"$tomorrow\",\"platform.activities:related_to\":\"$initech\",\"platform.activities:who\":\"$peter\"}")" "an open task"
expect 201 "$(call "$member_cookie" POST "$event_api/records" "{\"name\":\"Quarterly review\",\"platform.activities:starts_at\":\"${tomorrow}T15:00:00Z\",\"platform.activities:related_to\":\"$initech\"}")" "an upcoming event"
expect 201 "$(call "$member_cookie" POST "$event_api/records" "{\"name\":\"Kick-off lunch\",\"platform.activities:starts_at\":\"${yesterday}T12:00:00Z\",\"platform.activities:related_to\":\"$initech\"}")" "a past event"
expect 200 "$(call "$member_cookie" GET "$timeline")" "the timeline with both sources"
[[ "$(field '.sources')" == 2 && "$(field '.actions | join(",")')" == log_a_call ]] || fail "sources and timeline actions: $(cat "$body")"
[[ "$(field '[.open[].name] | join(",")')" == "Send the TPS report,Quarterly review" && "$(field '[.open[].open] | all')" == true ]] || fail "open, soonest first: $(cat "$body")"
[[ "$(field '[.past[].name] | join(",")')" == "Called Initech,Kick-off lunch" && "$(field '.next_cursor')" == null ]] || fail "past, newest first: $(cat "$body")"
[[ "$(field '.open[1].object')" == platform.activities:event && "$(field '.open[1].date_type')" == date_time ]] || fail "event item: $(cat "$body")"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:contact/records/$peter/timeline")" "the contact's timeline"
[[ "$(field '[.open[].name] | join(",")')" == "Send the TPS report" && "$(field '.open[0].through')" == platform.activities:who ]] || fail "reached through Who: $(cat "$body")"
# Run as the viewer: tasks and events are private, so another member sees none.
for object in custom:account platform.activities:task platform.activities:event; do
  expect 200 "$(call "$member_cookie" PUT "$workspace/objects/$object/permissions/$viewer_role" '{"read":true}')" "the viewer reads $object"
done
expect 200 "$(call "$viewer_cookie" GET "$timeline")" "the viewer's timeline"
[[ "$(field '.sources')" == 2 && "$(field '.open | length')" == 0 && "$(field '.past | length')" == 0 ]] || fail "another member's private activities are hidden: $(cat "$body")"
expect 404 "$(call "$viewer_cookie" GET "$task_api/records/$logged_call/timeline")" "a record the viewer cannot read"
expect 400 "$(call "$member_cookie" GET "$timeline?cursor=nonsense")" "a cursor the timeline did not issue"
[[ "$(field '.errors[0].code')" == timeline.cursor_invalid ]] || fail "cursor code: $(cat "$body")"
expect 201 "$(call "$member_cookie" POST "$workspace/objects" \
  '{"name":"region","label":"Region","plural_label":"Regions","in_navigation":false}')" "an object no timeline reaches"
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:region/records" '{"name":"North"}')" "a region"
expect 200 "$(call "$member_cookie" GET "$workspace/objects/custom:region/records/$(field '.id')/timeline")" "a timeline with no sources"
[[ "$(field '.sources')" == 0 && "$(field '.open + .past | length')" == 0 ]] || fail "no sources, no error: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$activities/disable")" "disable Activities"
expect 200 "$(call "$member_cookie" GET "$timeline")" "the timeline while Activities is disabled"
[[ "$(field '.sources')" == 0 ]] || fail "a disabled plugin contributes nothing: $(cat "$body")"
expect 200 "$(call "$member_cookie" POST "$activities/enable")" "enable Activities again"
expect 200 "$(call "$member_cookie" GET "$timeline")" "the timeline is back"
[[ "$(field '.sources')" == 2 && "$(field '.past | length')" == 2 ]] || fail "the timeline returns unchanged: $(cat "$body")"
[[ "$(app_sql --command="SELECT count(*) FROM timeline_sources;")" == 0 ]] || fail "timeline_sources leaks rows without tenant context"
[[ "$(sql --command="SELECT count(*) FROM timeline_sources WHERE tenant_id = '$tenant';")" == 2 ]] || fail "Task and Event are kept as timeline sources"

log "device sync (ADR 0024): the sync loop against the real API with a device token"
# Quick actions travel with the definitions (ADR 0032 §5, #236): one made
# here and the Activities plugin's Log a call reach the device.
expect 201 "$(call "$member_cookie" POST "$workspace/objects/custom:project/quick-actions" '{"name":"phone_tag","label":"Tag","kind":"update","fields":["name"]}')" "a quick action for the device"
grant_code code-sync "$verifier"
expect 200 "$(redeem code-sync "$verifier" "Viewer's synced phone")" "a device for the sync loop"
PLATFORM_SYNC_API="$api_base" PLATFORM_SYNC_WORKSPACE="$tenant" PLATFORM_SYNC_TOKEN="$(field '.access_token')" \
  PLATFORM_SYNC_OBJECT=custom:project PLATFORM_SYNC_QUICK_ACTIONS="custom:project.phone_tag custom:account.log_a_call" \
  cargo test --locked -p platform-sync --test api_acceptance -- --ignored --nocapture \
  || fail "the device did not sync with the API"
expect 200 "$(call "$member_cookie" DELETE "$workspace/objects/custom:project/quick-actions/phone_tag?expected_version=1")" "the device's quick action is removed again"

log "the browser: object pages and the Object Manager against the real API"
# A device the administrator revokes from Settings → Devices (#390).
grant_code code-watch "$verifier"
expect 200 "$(redeem code-watch "$verifier" "Viewer's watch")" "a device to revoke from the browser"
watch_refresh=$(field '.refresh_token')
pnpm --filter web build >"$work_root/web-build.log" 2>&1 || { cat "$work_root/web-build.log" >&2; fail "web build failed"; }
API_BASE="$api_base" SESSION_TOKEN="$member_token" SCREENSHOT_DIR="$work_root/screenshots" \
  node tests/objects-acceptance/browser.mjs || fail "the object pages did not complete the journey"
expect 400 "$(token_form --data-urlencode grant_type=refresh_token --data-urlencode "refresh_token=$watch_refresh")" "the device revoked in the browser"
[[ "$(field '.wipe')" == true ]] || fail "the device revoked from the page is told to wipe: $(cat "$body")"

printf 'object engine acceptance passed for definitions, fields, records, validation, uniqueness, lookups, lists, recycle bin, permissions, sharing, transfer, reporting lines, master-detail, roll-up summaries, saved reports, related lists, lookup filters, related-to fields, plugin-declared objects, the Activities plugin, the Activity timeline, field history, change data capture, sync, queries, search, validation rules, quick actions, formula fields, automations, composite requests, bulk jobs, API clients, devices, device sync, RLS, evidence and the browser pages\n'
