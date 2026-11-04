#!/usr/bin/env bash
set -Eeuo pipefail

project=${1:?compose project is required}
api_base=${2:?API base URL is required}
session_token=${3:?session token is required}
workspace_id=${4:?workspace ID is required}

compose=(docker compose --project-name "$project")
cookie="Cookie: platform_session=$session_token"
json_header="Content-Type: application/json"
actor_id="018f0000-0000-7000-8000-000000000101"
audit_failure_correlation="018f2900-0000-7000-8000-000000000091"
outbox_failure_correlation="018f2900-0000-7000-8000-000000000092"
response_body=$(mktemp)

run_sql() {
  local statement=$1
  shift
  printf '%s\n' "$statement" | "${compose[@]}" exec --no-TTY postgres \
    psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
    --quiet "$@" --file=-
}

drop_failure_injections() {
  run_sql "DROP TRIGGER IF EXISTS limiara_test_fail_contacts_audit ON public.audit_log; DROP FUNCTION IF EXISTS public.limiara_test_fail_contacts_audit(); DROP TRIGGER IF EXISTS limiara_test_fail_contacts_outbox ON public.outbox; DROP FUNCTION IF EXISTS public.limiara_test_fail_contacts_outbox();" \
    >/dev/null
  local remaining
  remaining=$(run_sql \
    "SELECT (SELECT count(*) FROM pg_trigger WHERE tgname IN ('limiara_test_fail_contacts_audit', 'limiara_test_fail_contacts_outbox')) + (SELECT count(*) FROM pg_proc JOIN pg_namespace ON pg_namespace.oid = pg_proc.pronamespace WHERE pg_namespace.nspname = 'public' AND pg_proc.proname IN ('limiara_test_fail_contacts_audit', 'limiara_test_fail_contacts_outbox'));" \
    --tuples-only --no-align)
  remaining=${remaining//$'\r'/}
  if [[ "$remaining" != '0' ]]; then
    printf 'test failure injections remain installed\n' >&2
    return 1
  fi
}

cleanup() {
  local status=$?
  trap - EXIT
  drop_failure_injections >/dev/null 2>&1 || true
  rm -f "$response_body" || true
  exit "$status"
}
trap cleanup EXIT

request() {
  local expected=$1
  shift
  local actual
  actual=$(curl --silent --show-error --output "$response_body" --write-out '%{http_code}' \
    --header "$cookie" --header "$json_header" "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected HTTP %s, received %s\n' "$expected" "$actual" >&2
    cat "$response_body" >&2
    return 1
  fi
}

request_without_session() {
  local expected=$1
  shift
  local actual
  actual=$(curl --silent --show-error --output "$response_body" --write-out '%{http_code}' \
    --header "$json_header" "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected HTTP %s without a session, received %s\n' "$expected" "$actual" >&2
    cat "$response_body" >&2
    return 1
  fi
}

mutation_request() {
  local expected=$1
  local correlation_id=$2
  shift 2
  request "$expected" --header "x-request-id: $correlation_id" "$@"
}

json() {
  node -e '
    const fs = require("node:fs");
    const document = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const value = Function("document", `return (${process.argv[2]})`)(document);
    if (!value) process.exit(1);
    process.stdout.write(typeof value === "string" ? value : JSON.stringify(value));
  ' "$response_body" "$1"
}

assert_sql_value() {
  local expected=$1
  local statement=$2
  shift 2
  local actual
  actual=$(run_sql "$statement" --tuples-only --no-align "$@")
  actual=${actual//$'\r'/}
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected database assertion %s, received %s\n' "$expected" "$actual" >&2
    return 1
  fi
}

assert_atomic_mutation() {
  local correlation_id=$1
  local action=$2
  local target=$3
  local event_type=$4
  local entity_id=$5
  local version=$6
  local applies_to=${7:-}
  local statement
  # Exact metadata and payload key sets make accidental personal-data or
  # private-schema expansion fail this acceptance gate.
  statement=$(cat <<'SQL'
SELECT
  (SELECT count(*) FROM audit_log
    WHERE correlation_id = :'correlation_id'::uuid)::text || '|' ||
  (SELECT count(*) FROM audit_log AS audit
    WHERE audit.tenant_id = :'workspace_id'::uuid
      AND audit.actor_id = :'actor_id'::uuid
      AND audit.correlation_id = :'correlation_id'::uuid
      AND audit.causation_id IS NULL
      AND audit.action = :'action'
      AND audit.target = :'target'
      AND jsonb_typeof(audit.metadata) = 'object'
      AND audit.metadata->>'entity_id' = :'entity_id'
      AND (audit.metadata->>'version')::bigint = :'version'::bigint
      AND EXISTS (
        SELECT 1 FROM outbox AS linked_outbox
        WHERE linked_outbox.envelope->>'correlation_id' = :'correlation_id'
          AND linked_outbox.id::text = audit.metadata->>'event_id'
          AND linked_outbox.envelope->>'id' = audit.metadata->>'event_id'
      )
      AND CASE
        WHEN :'applies_to' = '' THEN
          (SELECT count(*) FROM jsonb_object_keys(audit.metadata)) = 3
          AND audit.metadata ?& ARRAY['entity_id', 'event_id', 'version']
        ELSE
          (SELECT count(*) FROM jsonb_object_keys(audit.metadata)) = 4
          AND audit.metadata ?& ARRAY['entity_id', 'event_id', 'applies_to', 'version']
          AND audit.metadata->>'applies_to' = :'applies_to'
      END)::text || '|' ||
  (SELECT count(*) FROM outbox
    WHERE envelope->>'correlation_id' = :'correlation_id')::text || '|' ||
  (SELECT count(*) FROM outbox AS outbound
    WHERE outbound.tenant_id = :'workspace_id'::uuid
      AND outbound.event_type = :'event_type'
      AND outbound.id::text = outbound.envelope->>'id'
      AND outbound.envelope->>'event_type' = :'event_type'
      AND (outbound.envelope->>'schema_version')::integer = 1
      AND outbound.envelope->>'source_plugin' = 'platform.contacts'
      AND outbound.envelope->>'tenant_id' = :'workspace_id'
      AND outbound.envelope->>'actor_id' = :'actor_id'
      AND outbound.envelope->>'correlation_id' = :'correlation_id'
      AND outbound.envelope->'causation_id' = 'null'::jsonb
      AND jsonb_typeof(outbound.envelope->'payload') = 'object'
      AND outbound.envelope->'payload'->>'entity_id' = :'entity_id'
      AND (outbound.envelope->'payload'->>'version')::bigint = :'version'::bigint
      AND CASE
        WHEN :'applies_to' = '' THEN
          (SELECT count(*) FROM jsonb_object_keys(outbound.envelope->'payload')) = 2
          AND outbound.envelope->'payload' ?& ARRAY['entity_id', 'version']
        ELSE
          (SELECT count(*) FROM jsonb_object_keys(outbound.envelope->'payload')) = 3
          AND outbound.envelope->'payload' ?& ARRAY['entity_id', 'applies_to', 'version']
          AND outbound.envelope->'payload'->>'applies_to' = :'applies_to'
      END)::text;
SQL
  )
  assert_sql_value '1|1|1|1' "$statement" \
    --set=workspace_id="$workspace_id" \
    --set=actor_id="$actor_id" \
    --set=correlation_id="$correlation_id" \
    --set=action="$action" \
    --set=target="$target" \
    --set=event_type="$event_type" \
    --set=entity_id="$entity_id" \
    --set=version="$version" \
    --set=applies_to="$applies_to"
}

assert_no_audit_or_outbox() {
  local correlation_id=$1
  assert_sql_value '0|0' \
    "SELECT (SELECT count(*) FROM audit_log WHERE correlation_id = :'correlation_id'::uuid)::text || '|' || (SELECT count(*) FROM outbox WHERE envelope->>'correlation_id' = :'correlation_id')::text;" \
    --set=correlation_id="$correlation_id"
}

assert_generic_unavailable() {
  json 'document.type === "about:blank" && document.title === "Service Unavailable" && document.status === 503 && document.code === "contacts.unavailable" && document.detail === "contacts are temporarily unavailable" && Object.keys(document).length === 5' >/dev/null
}

install_audit_failure_injection() {
  local statement
  statement=$(cat <<'SQL'
CREATE FUNCTION public.limiara_test_fail_contacts_audit()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'test-only injected Contacts audit failure';
  RETURN NEW;
END;
$$;
CREATE TRIGGER limiara_test_fail_contacts_audit
BEFORE INSERT ON public.audit_log
FOR EACH ROW
WHEN (
  NEW.tenant_id = :'workspace_id'::uuid
  AND NEW.correlation_id = :'correlation_id'::uuid
)
EXECUTE FUNCTION public.limiara_test_fail_contacts_audit();
SQL
  )
  run_sql "$statement" \
    --set=workspace_id="$workspace_id" \
    --set=correlation_id="$audit_failure_correlation" \
    >/dev/null
}

install_outbox_failure_injection() {
  local statement
  statement=$(cat <<'SQL'
CREATE FUNCTION public.limiara_test_fail_contacts_outbox()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'test-only injected Contacts outbox failure';
  RETURN NEW;
END;
$$;
CREATE TRIGGER limiara_test_fail_contacts_outbox
BEFORE INSERT ON public.outbox
FOR EACH ROW
WHEN (
  NEW.tenant_id = :'workspace_id'::uuid
  AND NEW.event_type = 'platform.contacts.organisation-created.v1'
  AND NEW.envelope->>'correlation_id' = :'correlation_id'
)
EXECUTE FUNCTION public.limiara_test_fail_contacts_outbox();
SQL
  )
  run_sql "$statement" \
    --set=workspace_id="$workspace_id" \
    --set=correlation_id="$outbox_failure_correlation" \
    >/dev/null
}

contacts="$api_base/api/v1/workspaces/$workspace_id/contacts"

# Make the authorization matrix deterministic even when the local fixture role
# gains permissions in a later migration.
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="DELETE FROM role_permissions WHERE tenant_id = '$workspace_id' AND role_id = '018f0000-0000-7000-8000-000000000201' AND permission_id IN ('contacts.people.read', 'contacts.people.write', 'contacts.organisations.read', 'contacts.organisations.write', 'contacts.settings.manage')" \
  >/dev/null

request_without_session 401 "$contacts/people"
json 'document.code === "auth.unauthenticated"' >/dev/null

request 403 "$contacts/people"
json 'document.code === "auth.permission_denied"' >/dev/null
request 403 --request POST --data '{}' "$contacts/people"
json 'document.code === "auth.permission_denied"' >/dev/null
request 403 "$contacts/organisations"
json 'document.code === "auth.permission_denied"' >/dev/null
request 403 --request POST --data '{}' "$contacts/organisations"
json 'document.code === "auth.permission_denied"' >/dev/null
request 403 "$contacts/custom-fields/person"
json 'document.code === "auth.permission_denied"' >/dev/null

# Restore the exact permission set used by the permitted Contacts journey.
"${compose[@]}" exec --no-TTY postgres \
  psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1 \
  --command="INSERT INTO role_permissions (tenant_id, role_id, permission_id) SELECT '$workspace_id', '018f0000-0000-7000-8000-000000000201', id FROM permissions WHERE id IN ('contacts.people.read', 'contacts.people.write', 'contacts.organisations.read', 'contacts.organisations.write', 'contacts.settings.manage') ON CONFLICT DO NOTHING" \
  >/dev/null

definition_create_correlation="018f2900-0000-7000-8000-000000000001"
mutation_request 201 "$definition_create_correlation" --request POST --data '{
  "applies_to":"person",
  "api_name":"tier__c",
  "label":"Tier",
  "field_type":"select",
  "required":true,
  "unique_per_tenant":true,
  "indexed":true,
  "options":["gold","silver","bronze"],
  "validation_schema":{},
  "data_classification":"personal",
  "purpose":"Customer relationship management",
  "retention_days":365,
  "encrypted":false
}' "$contacts/custom-fields"
definition_id=$(json 'document.id')
assert_atomic_mutation \
  "$definition_create_correlation" \
  'contacts.custom-field-definition.created' \
  "custom-field-definition:$definition_id" \
  'platform.contacts.custom-field-definition-created.v1' \
  "$definition_id" 1 person

definition_update_correlation="018f2900-0000-7000-8000-000000000002"
mutation_request 200 "$definition_update_correlation" --request PUT --data '{
  "expected_version":1,
  "label":"Customer tier",
  "description":"Synthetic acceptance fixture",
  "required":true,
  "unique_per_tenant":true,
  "indexed":true,
  "options":["gold","silver","bronze","platinum"],
  "validation_schema":{},
  "data_classification":"personal",
  "purpose":"Customer relationship management",
  "retention_days":365,
  "encrypted":false
}' "$contacts/custom-fields/definitions/$definition_id"
json 'document.version === 2 && document.label === "Customer tier"' >/dev/null
assert_atomic_mutation \
  "$definition_update_correlation" \
  'contacts.custom-field-definition.updated' \
  "custom-field-definition:$definition_id" \
  'platform.contacts.custom-field-definition-updated.v1' \
  "$definition_id" 2 person

invalid_definition_correlation="018f2900-0000-7000-8000-000000000080"
mutation_request 400 "$invalid_definition_correlation" --request POST --data '{
  "applies_to":"person",
  "api_name":"bad__c",
  "label":"Bad",
  "field_type":"number",
  "options":["not-supported"],
  "validation_schema":{},
  "data_classification":"public"
}' "$contacts/custom-fields"
json 'document.code === "validation.failed"' >/dev/null
assert_no_audit_or_outbox "$invalid_definition_correlation"

organisation_create_correlation="018f2900-0000-7000-8000-000000000003"
mutation_request 201 "$organisation_create_correlation" --request POST --data '{
  "name":"Northstar Labs",
  "email":"hello@northstar.example",
  "telephone":"+44 20 7946 0000",
  "custom_fields":{}
}' "$contacts/organisations"
organisation_id=$(json 'document.id?.startsWith("org") ? document.id : false')
assert_atomic_mutation \
  "$organisation_create_correlation" \
  'contacts.organisation.created' \
  "organisation:$organisation_id" \
  'platform.contacts.organisation-created.v1' \
  "$organisation_id" 1

organisation_update_correlation="018f2900-0000-7000-8000-000000000004"
mutation_request 200 "$organisation_update_correlation" --request PUT --data '{
  "expected_version":1,
  "name":"Northstar Research Labs",
  "email":"hello@northstar.example",
  "telephone":"+44 20 7946 0000",
  "custom_fields":{}
}' "$contacts/organisations/$organisation_id"
json 'document.version === 2 && document.name === "Northstar Research Labs"' >/dev/null
assert_atomic_mutation \
  "$organisation_update_correlation" \
  'contacts.organisation.updated' \
  "organisation:$organisation_id" \
  'platform.contacts.organisation-updated.v1' \
  "$organisation_id" 2

person_create_correlation="018f2900-0000-7000-8000-000000000005"
mutation_request 201 "$person_create_correlation" --request POST --data "{
  \"full_name\":\"Ada Lovelace\",
  \"email\":\"ada@northstar.example\",
  \"telephone\":\"+44 20 7946 0001\",
  \"organisation_id\":\"$organisation_id\",
  \"tags\":[\"founder\"],
  \"custom_fields\":{\"tier__c\":\"gold\"}
}" "$contacts/people"
person_id=$(json 'document.id?.startsWith("per") ? document.id : false')
json "document.version === 1 && document.organisation_id === '$organisation_id'" >/dev/null
assert_atomic_mutation \
  "$person_create_correlation" \
  'contacts.person.created' \
  "person:$person_id" \
  'platform.contacts.person-created.v1' \
  "$person_id" 1

# Required, typed and unique custom values are enforced before persistence.
invalid_person_type_correlation="018f2900-0000-7000-8000-000000000081"
mutation_request 400 "$invalid_person_type_correlation" --request POST --data '{
  "full_name":"Invalid Contact",
  "custom_fields":{"tier__c":42}
}' "$contacts/people"
assert_no_audit_or_outbox "$invalid_person_type_correlation"
invalid_person_unique_correlation="018f2900-0000-7000-8000-000000000082"
mutation_request 400 "$invalid_person_unique_correlation" --request POST --data '{
  "full_name":"Duplicate Tier",
  "custom_fields":{"tier__c":"gold"}
}' "$contacts/people"
assert_no_audit_or_outbox "$invalid_person_unique_correlation"

grace_create_correlation="018f2900-0000-7000-8000-000000000006"
mutation_request 201 "$grace_create_correlation" --request POST --data '{
  "full_name":"Grace Hopper",
  "email":"grace@example.test",
  "custom_fields":{"tier__c":"silver"}
}' "$contacts/people"
grace_id=$(json 'document.id?.startsWith("per") ? document.id : false')
assert_atomic_mutation \
  "$grace_create_correlation" \
  'contacts.person.created' \
  "person:$grace_id" \
  'platform.contacts.person-created.v1' \
  "$grace_id" 1

linus_create_correlation="018f2900-0000-7000-8000-000000000007"
mutation_request 201 "$linus_create_correlation" --request POST --data '{
  "full_name":"Linus Torvalds",
  "email":"linus@example.test",
  "custom_fields":{"tier__c":"bronze"}
}' "$contacts/people"
linus_id=$(json 'document.id?.startsWith("per") ? document.id : false')
assert_atomic_mutation \
  "$linus_create_correlation" \
  'contacts.person.created' \
  "person:$linus_id" \
  'platform.contacts.person-created.v1' \
  "$linus_id" 1

# Optimistic concurrency is explicit and list traversal is deterministic.
update="{\"expected_version\":1,\"full_name\":\"Ada Byron\",\"email\":\"ada@northstar.example\",\"organisation_id\":\"$organisation_id\",\"tags\":[\"founder\"],\"custom_fields\":{\"tier__c\":\"gold\"}}"
person_update_correlation="018f2900-0000-7000-8000-000000000008"
mutation_request 200 "$person_update_correlation" --request PUT --data "$update" "$contacts/people/$person_id"
json 'document.version === 2 && document.full_name === "Ada Byron"' >/dev/null
assert_atomic_mutation \
  "$person_update_correlation" \
  'contacts.person.updated' \
  "person:$person_id" \
  'platform.contacts.person-updated.v1' \
  "$person_id" 2

version_conflict_correlation="018f2900-0000-7000-8000-000000000083"
mutation_request 409 "$version_conflict_correlation" --request PUT --data "$update" "$contacts/people/$person_id"
json 'document.code === "contacts.version_conflict"' >/dev/null
assert_no_audit_or_outbox "$version_conflict_correlation"
request 200 "$contacts/people/$person_id"
json 'document.version === 2 && document.full_name === "Ada Byron"' >/dev/null

# A failure while appending either audit evidence or its outbox event must
# unwind the domain write. Test-only triggers are limited to one workspace and
# one deterministic correlation ID, then removed immediately and again on exit.
drop_failure_injections
install_audit_failure_injection
mutation_request 503 "$audit_failure_correlation" --request POST --data '{
  "full_name":"Atomicity Audit Failure",
  "email":"atomic-audit-failure@example.test",
  "custom_fields":{"tier__c":"platinum"}
}' "$contacts/people"
assert_generic_unavailable
drop_failure_injections
assert_no_audit_or_outbox "$audit_failure_correlation"
assert_sql_value '0' \
  "SELECT count(*) FROM p_contacts.people WHERE tenant_id = :'workspace_id'::uuid AND email = :'email';" \
  --set=workspace_id="$workspace_id" \
  --set=email='atomic-audit-failure@example.test'

install_outbox_failure_injection
mutation_request 503 "$outbox_failure_correlation" --request POST --data '{
  "name":"Atomicity Outbox Failure",
  "email":"atomic-outbox-failure@example.test",
  "custom_fields":{}
}' "$contacts/organisations"
assert_generic_unavailable
drop_failure_injections
assert_no_audit_or_outbox "$outbox_failure_correlation"
assert_sql_value '0' \
  "SELECT count(*) FROM p_contacts.organisations WHERE tenant_id = :'workspace_id'::uuid AND email = :'email';" \
  --set=workspace_id="$workspace_id" \
  --set=email='atomic-outbox-failure@example.test'

request 200 "$contacts/people?limit=2&sort=name&direction=asc"
cursor=$(json 'document.next_cursor?.length === 43 ? document.next_cursor : false')
first_ids=$(json 'JSON.stringify(document.data.map(record => record.id))')
request 200 "$contacts/people?limit=2&sort=name&direction=asc&cursor=$cursor"
second_ids=$(json 'JSON.stringify(document.data.map(record => record.id))')
[[ "$first_ids" != "$second_ids" ]]
request 400 "$contacts/people?limit=2&sort=created_at&direction=asc&cursor=$cursor"
json 'document.code === "contacts.cursor_invalid"' >/dev/null

# Disabled definitions retain readable values but reject changes.
definition_disable_correlation="018f2900-0000-7000-8000-000000000009"
mutation_request 200 "$definition_disable_correlation" --request POST --data '{"expected_version":2}' "$contacts/custom-fields/definitions/$definition_id/disable"
json 'document.version === 3 && document.disabled_at != null' >/dev/null
assert_atomic_mutation \
  "$definition_disable_correlation" \
  'contacts.custom-field-definition.disabled' \
  "custom-field-definition:$definition_id" \
  'platform.contacts.custom-field-definition-disabled.v1' \
  "$definition_id" 3 person

disabled_value_change_correlation="018f2900-0000-7000-8000-000000000084"
mutation_request 400 "$disabled_value_change_correlation" --request PUT --data "{
  \"expected_version\":2,
  \"full_name\":\"Ada Byron\",
  \"email\":\"ada@northstar.example\",
  \"organisation_id\":\"$organisation_id\",
  \"tags\":[\"founder\"],
  \"custom_fields\":{\"tier__c\":\"silver\"}
}" "$contacts/people/$person_id"
assert_no_audit_or_outbox "$disabled_value_change_correlation"

person_second_update_correlation="018f2900-0000-7000-8000-000000000010"
mutation_request 200 "$person_second_update_correlation" --request PUT --data "{
  \"expected_version\":2,
  \"full_name\":\"Ada Byron\",
  \"email\":\"ada@northstar.example\",
  \"organisation_id\":\"$organisation_id\",
  \"tags\":[\"founder\",\"vip\"],
  \"custom_fields\":{}
}" "$contacts/people/$person_id"
json 'document.version === 3 && document.custom_fields.tier__c === "gold"' >/dev/null
assert_atomic_mutation \
  "$person_second_update_correlation" \
  'contacts.person.updated' \
  "person:$person_id" \
  'platform.contacts.person-updated.v1' \
  "$person_id" 3

definition_archive_correlation="018f2900-0000-7000-8000-000000000011"
mutation_request 200 "$definition_archive_correlation" --request POST --data '{"expected_version":3}' "$contacts/custom-fields/definitions/$definition_id/archive"
json 'document.version === 4 && document.archived_at != null' >/dev/null
assert_atomic_mutation \
  "$definition_archive_correlation" \
  'contacts.custom-field-definition.archived' \
  "custom-field-definition:$definition_id" \
  'platform.contacts.custom-field-definition-archived.v1' \
  "$definition_id" 4 person

person_archive_correlation="018f2900-0000-7000-8000-000000000012"
mutation_request 200 "$person_archive_correlation" --request POST "$contacts/people/$linus_id/archive?expected_version=1"
json 'document.version === 2 && document.archived_at != null' >/dev/null
assert_atomic_mutation \
  "$person_archive_correlation" \
  'contacts.person.archived' \
  "person:$linus_id" \
  'platform.contacts.person-archived.v1' \
  "$linus_id" 2

# Archiving an organisation removes it from active lists without breaking the
# historical relationship held by a person.
organisation_archive_correlation="018f2900-0000-7000-8000-000000000013"
mutation_request 200 "$organisation_archive_correlation" --request POST "$contacts/organisations/$organisation_id/archive?expected_version=2"
json 'document.version === 3 && document.archived_at != null' >/dev/null
assert_atomic_mutation \
  "$organisation_archive_correlation" \
  'contacts.organisation.archived' \
  "organisation:$organisation_id" \
  'platform.contacts.organisation-archived.v1' \
  "$organisation_id" 3
request 200 "$contacts/people/$person_id"
json "document.organisation_id === '$organisation_id'" >/dev/null
request 200 "$contacts/organisations"
json "!document.data.some(record => record.id === '$organisation_id')" >/dev/null

# Deleting moves a record to the recycle bin (ADR 0014): hidden from reads
# and lists, visible with include_deleted and status=deleted, and restorable.
person_delete_correlation="018f2900-0000-7000-8000-000000000014"
mutation_request 200 "$person_delete_correlation" --request DELETE "$contacts/people/$linus_id?expected_version=2"
json 'document.version === 3 && document.is_deleted === true && document.deleted_at != null' >/dev/null
assert_atomic_mutation \
  "$person_delete_correlation" \
  'contacts.person.deleted' \
  "person:$linus_id" \
  'platform.contacts.person-deleted.v1' \
  "$linus_id" 3
request 404 "$contacts/people/$linus_id"
request 200 "$contacts/people/$linus_id?include_deleted=true"
json 'document.is_deleted === true' >/dev/null
request 200 "$contacts/people?status=all"
json "!document.data.some(record => record.id === '$linus_id')" >/dev/null
request 200 "$contacts/people?status=deleted"
json "document.data.length === 1 && document.data[0].id === '$linus_id'" >/dev/null
person_undelete_correlation="018f2900-0000-7000-8000-000000000015"
mutation_request 200 "$person_undelete_correlation" --request POST "$contacts/people/$linus_id/undelete?expected_version=3"
json 'document.version === 4 && document.is_deleted === false && document.deleted_at === null' >/dev/null
assert_atomic_mutation \
  "$person_undelete_correlation" \
  'contacts.person.undeleted' \
  "person:$linus_id" \
  'platform.contacts.person-undeleted.v1' \
  "$linus_id" 4
request 200 "$contacts/people/$linus_id"

# A known identifier in a workspace the caller does not belong to remains
# concealed at the membership boundary.
request 404 "$api_base/api/v1/workspaces/018f0000-0000-7000-8000-000000000002/contacts/people/$person_id"
json 'document.code === "workspace.not_found"' >/dev/null

printf '%s\n' "$person_id"
