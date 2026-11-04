#!/usr/bin/env bash
# ADR 0012 / #271: one isolated acceptance journey proving a plugin can add
# fields to several records of another plugin through the kernel extension
# service, end to end over HTTP against the production API binary.
#
# The fixture plugin example.loyalty (fixtures/plugins/loyalty) extends
# Contacts people and organisations. The journey covers the extension plan,
# field descriptors, writes, projection, list filters, permission and
# validation failures, version conflicts, and disable hiding values that
# re-enable restores.
#
# PostgreSQL and NATS run locally (scripts/local-postgres.sh,
# scripts/local-nats.sh). The test touches the database directly only to seed
# sessions and role permissions.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

work_root="$repository_root/target/extensions-acceptance"
api_port="${PLATFORM_ACCEPTANCE_API_PORT:-18090}"
tenant="018f0000-0000-7000-8000-000000000001"
member="018f0000-0000-7000-8000-000000000101"
viewer="018f0000-0000-7000-8000-000000000102"
# A real user with no membership in this workspace.
outsider="018f0000-0000-7000-8000-000000000103"
role="018f0000-0000-7000-8000-000000000201"
api_base="http://127.0.0.1:${api_port}"
workspace="$api_base/api/v1/workspaces/$tenant"
member_token="extensions-acceptance-member"
viewer_token="extensions-acceptance-viewer"
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

hash_token() { printf '%s' "$1" | sha256sum | awk '{print $1}'; }

# The seeded administrator also gets the loyalty permissions; the viewer may
# read people and loyalty fields but not change them.
sql --command="
  INSERT INTO permissions (id, description) VALUES
    ('loyalty.fields.read', 'See loyalty fields on records'),
    ('loyalty.fields.write', 'Change loyalty fields on records'),
    ('contacts.organisations.read', 'Read organisations'),
    ('contacts.organisations.write', 'Change organisations')
  ON CONFLICT (id) DO NOTHING;
  INSERT INTO role_permissions (tenant_id, role_id, permission_id) VALUES
    ('$tenant', '$role', 'loyalty.fields.read'),
    ('$tenant', '$role', 'loyalty.fields.write'),
    ('$tenant', '$role', 'contacts.organisations.read'),
    ('$tenant', '$role', 'contacts.organisations.write')
  ON CONFLICT DO NOTHING;
  INSERT INTO roles (tenant_id, id, name)
  VALUES ('$tenant', '018f0000-0000-7000-8000-000000000202', 'Loyalty viewer');
  INSERT INTO role_permissions (tenant_id, role_id, permission_id) VALUES
    ('$tenant', '018f0000-0000-7000-8000-000000000202', 'contacts.people.read'),
    ('$tenant', '018f0000-0000-7000-8000-000000000202', 'loyalty.fields.read');
  INSERT INTO users (id, display_name) VALUES ('$viewer', 'Viewer');
  INSERT INTO users (id, display_name) VALUES ('$outsider', 'Outsider');
  INSERT INTO memberships (tenant_id, user_id) VALUES ('$tenant', '$viewer');
  INSERT INTO user_roles (tenant_id, user_id, role_id)
  VALUES ('$tenant', '$viewer', '018f0000-0000-7000-8000-000000000202');
  INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES
    ('018f0000-0000-7000-8000-000000000601', '$(hash_token "$member_token")', '$member', now() + interval '2 hours'),
    ('018f0000-0000-7000-8000-000000000602', '$(hash_token "$viewer_token")', '$viewer', now() + interval '2 hours');"

cat >"$work_root/config.yml" <<EOF
platform:
  plugins:
    manifest_paths:
      - plugins/contacts/plugin.toml
      - fixtures/plugins/loyalty/plugin.toml
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

log "contacts is enabled; the extension plan for loyalty is reviewable before enabling"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/platform.contacts/enable")" "enable contacts"
expect 200 "$(call "$member_cookie" GET "$workspace/plugins/example.loyalty/extension-plan")" "plan"
[[ "$(field '.data.hosts | join(",")')" == "platform.contacts" ]] || fail "plan hosts"
[[ "$(field '[.data.fields[].target] | unique | join(",")')" == "platform.contacts.organisation,platform.contacts.person" ]] || fail "plan targets"
[[ "$(field '[.data.fields[] | select(.paused != null)] | length')" == 0 ]] || fail "plan pauses"
[[ "$(field '[.data.fields[].storage] | unique | join(",")')" == "host-record" ]] || fail "plan storage"
expect 403 "$(call "$viewer_cookie" GET "$workspace/plugins/example.loyalty/extension-plan")" "viewer plan"

log "before loyalty is enabled there are no extension fields"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "fields before"
[[ "$(field '.data | length')" == 0 ]] || fail "fields before enable"

log "enabling loyalty adds fields to people and organisations"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/enable")" "enable loyalty"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "person fields"
[[ "$(field '[.data[].id] | join(",")')" == "example.loyalty.member_since,example.loyalty.points,example.loyalty.tier" ]] || fail "person field ids: $(cat "$body")"
[[ "$(field '[.data[].writable] | all')" == true ]] || fail "member can write"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.organisation")" "organisation fields"
[[ "$(field '[.data[].id] | join(",")')" == "example.loyalty.account_code" ]] || fail "organisation field ids"
expect 200 "$(call "$viewer_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "viewer fields"
[[ "$(field '[.data[].writable] | any')" == false ]] || fail "viewer cannot write"
expect 404 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.unknown.thing")" "unknown resource"

log "writing extension values through the kernel field service"
expect 201 "$(call "$member_cookie" POST "$workspace/contacts/people" '{"full_name":"Grace Hopper"}')" "create person"
person=$(field '.id')
version=$(field '.version')
[[ "$(field '.extensions | length')" == 0 ]] || fail "new person has no extension values"
# Standard record fields (ADR 0014): name, who created and last changed the
# record, resolved to member names, and the touched stamp.
[[ "$(field '.name')" == "Grace Hopper" ]] || fail "standard name: $(cat "$body")"
[[ "$(field '.created_by.id')" == "$member" ]] || fail "created_by is the acting member"
[[ "$(field '.updated_by.id')" == "$member" ]] || fail "updated_by is the acting member"
[[ -n "$(field '.created_by.name // empty')" ]] || fail "created_by resolves to a member name"
[[ "$(field '.touched_at >= .updated_at')" == true ]] || fail "touched_at is never earlier than updated_at"
creator_name=$(field '.created_by.name')
# created_by links to the member's kernel user record; users outside the
# workspace and malformed ids are indistinguishable from unknown ones.
expect 200 "$(call "$viewer_cookie" GET "$workspace/users/$member")" "member opens a colleague's user record"
[[ "$(field '.name')" == "$creator_name" ]] || fail "user record name matches the reference"
[[ "$(field '.id')" == "$member" ]] || fail "user record id"
expect 404 "$(call "$member_cookie" GET "$workspace/users/018f0000-0000-7000-8000-00000000ffff")" "unknown user"
[[ "$(field '.code')" == resource.unavailable ]] || fail "unknown user code"
expect 404 "$(call "$member_cookie" GET "$workspace/users/not-a-uuid")" "malformed user id"
expect 404 "$(call "$member_cookie" GET "$workspace/users/$outsider")" "user outside the workspace"
grep -q Outsider "$body" && fail "an outsider's name must never be disclosed"
expect 200 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$version,\"values\":{\"example.loyalty.tier\":\"gold\",\"example.loyalty.member_since\":\"2019-05-01\",\"example.loyalty.points\":1200}}")" "write person extensions"
[[ "$(field '.extensions["example.loyalty.tier"]')" == gold ]] || fail "tier written"
[[ "$(field '.version')" == $((version + 1)) ]] || fail "version bumped"
version=$(field '.version')
expect 200 "$(call "$viewer_cookie" GET "$workspace/contacts/people/$person")" "viewer reads person"
[[ "$(field '.extensions["example.loyalty.points"]')" == 1200 ]] || fail "viewer sees values"
[[ "$(field '.updated_by.id')" == "$member" ]] || fail "extension write stamped the member"

log "validation, permission and version failures"
expect 400 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$version,\"values\":{\"example.loyalty.tier\":\"platinum\"}}")" "invalid option"
[[ "$(field '.errors[0].code')" == extension.value_option ]] || fail "option error code"
expect 400 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$version,\"values\":{\"example.loyalty.account_code\":\"A1\"}}")" "field from another resource"
[[ "$(field '.errors[0].code')" == extension.field_unknown ]] || fail "unknown field code"
expect 403 "$(call "$viewer_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$version,\"values\":{\"example.loyalty.tier\":\"silver\"}}")" "viewer cannot write"
expect 409 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$((version - 1)),\"values\":{\"example.loyalty.tier\":\"silver\"}}")" "stale version"

log "list filters on extension fields"
expect 201 "$(call "$member_cookie" POST "$workspace/contacts/people" '{"full_name":"Alan Turing"}')" "create second person"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people?extension_field=example.loyalty.tier&extension_value=gold&limit=200")" "filter gold"
[[ "$(field '[.data[].id] | join(",")')" == "$person" ]] || fail "gold filter: $(cat "$body")"
expect 400 "$(call "$member_cookie" GET "$workspace/contacts/people?extension_field=example.loyalty.tier")" "incomplete filter"

log "the recycle bin: delete, hide, include_deleted, undelete (ADR 0014)"
expect 201 "$(call "$member_cookie" POST "$workspace/contacts/people" '{"full_name":"Ada Lovelace"}')" "create person to delete"
binned=$(field '.id')
[[ "$(field '.is_deleted')" == false ]] || fail "new person is not deleted"
expect 200 "$(call "$member_cookie" DELETE "$workspace/contacts/people/$binned?expected_version=$(field '.version')")" "delete person"
[[ "$(field '.is_deleted')" == true ]] || fail "deleted flag"
binned_version=$(field '.version')
expect 404 "$(call "$member_cookie" GET "$workspace/contacts/people/$binned")" "deleted person is hidden"
expect 200 "$(call "$viewer_cookie" GET "$workspace/contacts/people/$binned?include_deleted=true")" "include_deleted read"
[[ "$(field '.is_deleted')" == true ]] || fail "include_deleted shows the flag"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people?status=deleted&limit=200")" "recycle bin list"
[[ "$(field '[.data[].id] | join(",")')" == "$binned" ]] || fail "recycle bin holds only the deleted person"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people?status=all&limit=200")" "all list"
[[ "$(field "[.data[] | select(.id == \"$binned\")] | length")" == 0 ]] || fail "status=all excludes deleted"
expect 404 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$binned/extensions" \
  "{\"expected_version\":$binned_version,\"values\":{\"example.loyalty.tier\":\"gold\"}}")" "no extension writes in the bin"
expect 403 "$(call "$viewer_cookie" POST "$workspace/contacts/people/$binned/undelete?expected_version=$binned_version")" "viewer cannot undelete"
expect 200 "$(call "$member_cookie" POST "$workspace/contacts/people/$binned/undelete?expected_version=$binned_version")" "undelete person"
[[ "$(field '.is_deleted')" == false ]] || fail "undeleted flag"
expect 200 "$(call "$member_cookie" POST "$workspace/contacts/people/$binned/archive?expected_version=$(field '.version')")" "archive is separate from delete"
[[ "$(field '.archived_at != null and .is_deleted == false')" == true ]] || fail "archived, not deleted"

log "organisations host loyalty values too"
expect 201 "$(call "$member_cookie" POST "$workspace/contacts/organisations" '{"name":"Analytical Engines"}')" "create organisation"
organisation=$(field '.id')
expect 200 "$(call "$member_cookie" PATCH "$workspace/contacts/organisations/$organisation/extensions" \
  "{\"expected_version\":$(field '.version'),\"values\":{\"example.loyalty.account_code\":\"AE-1\"}}")" "write organisation extensions"
[[ "$(field '.extensions["example.loyalty.account_code"]')" == AE-1 ]] || fail "account code written"

log "fields sit in the host's default zones until an administrator arranges them"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "zones and placements"
[[ "$(field '[.zones[].id] | join(",")')" == "highlights,contact-information,workspace-fields,record-metadata,form-contact,form-workspace,form-additional,list-columns" ]] || fail "zones: $(cat "$body")"
[[ "$(field '[.placements[] | select(.field_id == "example.loyalty.tier") | .zone] | join(",")')" == "contact-information,form-additional" ]] || fail "default placements"

log "one form save writes host fields and extension values together"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "read before form save"
version=$(field '.version')
expect 200 "$(call "$member_cookie" PUT "$workspace/contacts/people/$person" \
  "{\"expected_version\":$version,\"full_name\":\"Grace B. Hopper\",\"tags\":[],\"custom_fields\":{},\"extensions\":{\"example.loyalty.points\":1500}}")" "form save with extensions"
[[ "$(field '.full_name')" == "Grace B. Hopper" && "$(field '.extensions["example.loyalty.points"]')" == 1500 ]] || fail "both saved"
[[ "$(field '.extensions["example.loyalty.tier"]')" == gold ]] || fail "untouched extension values kept"
[[ "$(field '.version')" == $((version + 1)) ]] || fail "one version for one save"
expect 400 "$(call "$member_cookie" PUT "$workspace/contacts/people/$person" \
  "{\"expected_version\":$((version + 1)),\"full_name\":\"Grace B. Hopper\",\"tags\":[],\"custom_fields\":{},\"extensions\":{\"example.loyalty.tier\":\"platinum\"}}")" "invalid extension value in form save"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "nothing saved from the invalid form"
[[ "$(field '.version')" == $((version + 1)) ]] || fail "invalid form saved nothing"

log "administrators arrange fields anywhere among the host's fields"
expect 403 "$(call "$viewer_cookie" GET "$workspace/extensions/layout?resource=platform.contacts.person")" "viewer cannot arrange"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/layout?resource=platform.contacts.person")" "layout"
[[ "$(field '.revision')" == 0 && "$(field '.fields | length')" == 3 ]] || fail "layout state: $(cat "$body")"
layout='{"expected_revision":0,"arranged":["example.loyalty.tier","example.loyalty.points","example.loyalty.member_since"],"placements":[{"field_id":"example.loyalty.tier","zone":"highlights","position":1},{"field_id":"example.loyalty.tier","zone":"list-columns","position":2},{"field_id":"example.loyalty.points","zone":"form-contact","position":0}]}'
expect 200 "$(call "$member_cookie" PUT "$workspace/extensions/layout?resource=platform.contacts.person" "$layout")" "save layout"
[[ "$(field '.revision')" == 1 ]] || fail "layout revision"
expect 409 "$(call "$member_cookie" PUT "$workspace/extensions/layout?resource=platform.contacts.person" "$layout")" "stale layout"
expect 400 "$(call "$member_cookie" PUT "$workspace/extensions/layout?resource=platform.contacts.person" \
  '{"expected_revision":1,"placements":[{"field_id":"example.loyalty.tier","zone":"sidebar","position":0}]}')" "unknown zone"
[[ "$(field '.errors[0].code')" == layout.zone_unknown ]] || fail "zone error code"
expect 200 "$(call "$viewer_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "placements for readers"
[[ "$(field '[.placements[] | "\(.field_id)@\(.zone)#\(.position)"] | sort | join(",")')" == "example.loyalty.points@form-contact#0,example.loyalty.tier@highlights#1,example.loyalty.tier@list-columns#2" ]] || fail "arranged placements: $(cat "$body")"
saved=$(sql --command="SELECT count(*) FROM audit_log WHERE action = 'extension.layout.saved' AND target = 'resource:platform.contacts.person';")
(( saved == 1 )) || fail "layout save audited once"

log "disabling loyalty hides values and fields; re-enabling restores them"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/disable")" "disable loyalty"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "read while disabled"
[[ "$(field '.extensions | length')" == 0 ]] || fail "values hidden while disabled"
expect 200 "$(call "$member_cookie" GET "$workspace/extensions/fields?resource=platform.contacts.person")" "fields while disabled"
[[ "$(field '.data | length')" == 0 ]] || fail "fields hidden while disabled"
expect 400 "$(call "$member_cookie" PATCH "$workspace/contacts/people/$person/extensions" \
  "{\"expected_version\":$version,\"values\":{\"example.loyalty.tier\":\"silver\"}}")" "write while disabled"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/enable")" "re-enable loyalty"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "read after re-enable"
[[ "$(field '.extensions["example.loyalty.tier"]')" == gold ]] || fail "values restored after re-enable"

log "removal requires disabling, keeps values for export, and reinstalling restores them"
expect 409 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/remove")" "remove while enabled"
[[ "$(field '.code')" == plugin.remove_requires_disabled ]] || fail "remove error code"
expect 403 "$(call "$viewer_cookie" GET "$workspace/plugins/example.loyalty/extension-export")" "viewer export"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/disable")" "disable before removal"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/remove")" "remove loyalty"
[[ "$(field '.state')" == removed ]] || fail "removed state"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "read while removed"
[[ "$(field '.extensions | length')" == 0 ]] || fail "values hidden while removed"
expect 200 "$(call "$member_cookie" GET "$workspace/plugins/example.loyalty/extension-export")" "export while removed"
[[ "$(field "[.data[] | select(.record_id == \"$person\" and .resource == \"platform.contacts.person\") | .values.tier] | first")" == gold ]] || fail "export carries the person's tier: $(cat "$body")"
[[ "$(field "[.data[] | select(.record_id == \"$organisation\") | .values.account_code] | first")" == AE-1 ]] || fail "export carries the organisation code"
[[ "$(field '.truncated')" == false ]] || fail "export complete"
exports=$(sql --command="SELECT count(*) FROM audit_log WHERE action = 'plugin.extensions.exported' AND target = 'plugin:example.loyalty';")
(( exports >= 1 )) || fail "export audited"
removed_at=$(sql --command="SELECT removed_at IS NOT NULL FROM tenant_plugin_installations WHERE tenant_id = '$tenant' AND plugin_id = 'example.loyalty';")
[[ "$removed_at" == t ]] || fail "retention window started"
expect 200 "$(call "$member_cookie" POST "$workspace/plugins/example.loyalty/enable")" "reinstall within the window"
expect 200 "$(call "$member_cookie" GET "$workspace/contacts/people/$person")" "read after reinstall"
[[ "$(field '.extensions["example.loyalty.tier"]')" == gold ]] || fail "values restored after reinstall"

log "events and audit carry no extension values"
leaks=$(sql --command="
  SELECT (SELECT count(*) FROM outbox WHERE envelope::text LIKE '%gold%' OR envelope::text LIKE '%AE-1%')
       + (SELECT count(*) FROM audit_log WHERE metadata::text LIKE '%gold%' OR metadata::text LIKE '%AE-1%');")
updates=$(sql --command="SELECT count(*) FROM outbox WHERE event_type = 'platform.contacts.person-updated.v1';")
(( updates >= 1 )) || fail "extension writes emit person-updated events"
[[ "$leaks" == 0 ]] || fail "extension values leaked into events"

printf 'plugin extension acceptance passed for plan, fields, writes, projection, filters, failures, disable, removal, export, and reinstall\n'
