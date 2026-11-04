#!/usr/bin/env bash
set -Eeuo pipefail

: "${POSTGRES_USER:?POSTGRES_USER is required}"
: "${POSTGRES_DB:?POSTGRES_DB is required}"
: "${PLATFORM_MIGRATOR_PASSWORD:?PLATFORM_MIGRATOR_PASSWORD is required}"
: "${PLATFORM_APP_PASSWORD:?PLATFORM_APP_PASSWORD is required}"
: "${PLATFORM_WORKER_PASSWORD:?PLATFORM_WORKER_PASSWORD is required}"

psql \
  --username "$POSTGRES_USER" \
  --dbname "$POSTGRES_DB" \
  --set=ON_ERROR_STOP=1 \
  --set=migrator_password="$PLATFORM_MIGRATOR_PASSWORD" \
  --set=app_password="$PLATFORM_APP_PASSWORD" \
  --set=worker_password="$PLATFORM_WORKER_PASSWORD" \
  --file="$(dirname "${BASH_SOURCE[0]}")/010_roles.sql.tmpl"
