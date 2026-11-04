#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

project="${LIMIARA_COMPOSE_PROJECT:-limiara-integration}"
action="${1:-up}"
compose=(docker compose --project-name "$project")

case "$action" in
  up)
    docker compose config --quiet
    build=(--build)
    if [[ "${LIMIARA_SKIP_BUILD:-0}" == "1" ]]; then
      build=()
    fi
    "${compose[@]}" up --detach "${build[@]}" --wait --wait-timeout "${LIMIARA_WAIT_TIMEOUT:-240}"
    ;;
  down) "${compose[@]}" down --remove-orphans ;;
  reset) "${compose[@]}" down --volumes --remove-orphans ;;
  status) "${compose[@]}" ps --all ;;
  *) echo "usage: $0 {up|down|reset|status}" >&2; exit 2 ;;
esac
