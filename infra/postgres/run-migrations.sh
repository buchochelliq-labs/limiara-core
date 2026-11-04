#!/usr/bin/env sh
set -eu

export LC_ALL=C

: "${PGHOST:?PGHOST is required}"
: "${PGDATABASE:?PGDATABASE is required}"
: "${PGUSER:?PGUSER is required}"
: "${PGPASSWORD:?PGPASSWORD is required}"

workspace="${PLATFORM_MIGRATIONS_ROOT:-/workspace}"
enabled_plugins="${PLATFORM_ENABLED_PLUGINS-contacts}"
ledger_tenant="00000000-0000-0000-0000-000000000000"

fail() {
  printf 'migration error: %s\n' "$*" >&2
  exit 1
}

sql() {
  psql --no-psqlrc --set=ON_ERROR_STOP=1 --tuples-only --no-align "$@"
}

apply_migration() {
  plugin_id=$1
  migration_file=$2
  migration_id=$(basename "$migration_file")

  case "$plugin_id" in
    *[!a-z0-9.-]*|'') fail "invalid plugin id '$plugin_id'" ;;
  esac
  case "$migration_id" in
    *[!A-Za-z0-9._-]*|'') fail "invalid migration filename '$migration_id'" ;;
  esac

  checksum=$(sha256sum "$migration_file" | awk '{print $1}')
  ledger_exists=$(sql --command="SELECT to_regclass('public.plugin_migration_ledger') IS NOT NULL")
  recorded_checksum=""
  if [ "$ledger_exists" = "t" ]; then
    recorded_checksum=$(sql --command="SELECT checksum FROM public.plugin_migration_ledger WHERE tenant_id = '$ledger_tenant' AND plugin_id = '$plugin_id' AND migration_id = '$migration_id'")
  fi

  if [ -n "$recorded_checksum" ]; then
    if [ "$recorded_checksum" = "$checksum" ]; then
      printf 'migration already applied: %s/%s\n' "$plugin_id" "$migration_id"
      return
    fi
    fail "checksum drift for $plugin_id/$migration_id (recorded $recorded_checksum, found $checksum); restore the applied migration and add a new file"
  fi

  printf 'applying migration: %s/%s\n' "$plugin_id" "$migration_id"
  if ! sql --single-transaction \
    --file="$migration_file" \
    --command="INSERT INTO public.plugin_migration_ledger (tenant_id, plugin_id, migration_id, checksum) VALUES ('$ledger_tenant', '$plugin_id', '$migration_id', '$checksum')" \
    >/dev/null; then
    fail "failed to apply $plugin_id/$migration_id; the transaction was rolled back"
  fi
}

core_directory="$workspace/crates/platform-db/migrations"
[ -d "$core_directory" ] || fail "core migration directory not found: $core_directory"

found_core=false
for migration_file in "$core_directory"/*.sql; do
  [ -f "$migration_file" ] || continue
  found_core=true
  apply_migration platform.core "$migration_file"
done
[ "$found_core" = true ] || fail "no core migrations found in $core_directory"

plugin_names=$(printf '%s' "$enabled_plugins" | tr ', ' '\n\n' | sed '/^$/d' | sort)
for plugin_name in $plugin_names; do
  case "$plugin_name" in
    *[!A-Za-z0-9._-]*|'') fail "invalid enabled plugin directory '$plugin_name'" ;;
  esac
  plugin_directory="$workspace/plugins/$plugin_name"
  manifest="$plugin_directory/plugin.toml"
  migration_directory="$plugin_directory/migrations"
  [ -f "$manifest" ] || fail "enabled plugin '$plugin_name' has no plugin.toml"
  [ -d "$migration_directory" ] || fail "enabled plugin '$plugin_name' has no migrations directory"

  plugin_id=$(awk -F '"' '/^id[[:space:]]*=[[:space:]]*"/ { print $2; exit }' "$manifest")
  [ -n "$plugin_id" ] || fail "enabled plugin '$plugin_name' has no manifest id"

  found_plugin_migration=false
  for migration_file in "$migration_directory"/*.sql; do
    [ -f "$migration_file" ] || continue
    found_plugin_migration=true
    apply_migration "$plugin_id" "$migration_file"
  done
  [ "$found_plugin_migration" = true ] || fail "enabled plugin '$plugin_name' has no SQL migrations"
done

printf 'database migrations are current\n'
