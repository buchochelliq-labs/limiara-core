#!/usr/bin/env bash
# Start a disposable NATS JetStream broker without registry pulls.
#
#   eval "$(bash scripts/local-nats.sh start)"   # exports PLATFORM_TEST_NATS_URL
#   bash scripts/local-nats.sh stop
#
# Uses NATS_SERVER_BIN or nats-server on PATH, else a pinned, checksum-verified
# release binary, else the pinned Compose image through Docker. A missing
# broker is a failure, never a skipped check.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
nats_root="${PLATFORM_LOCAL_NATS_ROOT:-$repository_root/target/local-nats}"
port="${PLATFORM_LOCAL_NATS_PORT:-54222}"
nats_version="v2.10.24"
nats_sha256="ee6500f364e3a741b496ae0296c04f2a9d53bbaabac457104ac74596b4a59d85"
nats_cache="$repository_root/target/nats-server-$nats_version"
container_name="limiara-local-nats-$port"

fetch_pinned_nats() {
  [[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]] || return 1
  command -v curl >/dev/null 2>&1 || return 1
  if [[ ! -x "$nats_cache/nats-server" ]]; then
    local archive="$nats_cache.tar.gz"
    mkdir -p "$nats_cache"
    curl --fail --silent --show-error --location --retry 3 --output "$archive" \
      "https://github.com/nats-io/nats-server/releases/download/$nats_version/nats-server-$nats_version-linux-amd64.tar.gz" || return 1
    echo "$nats_sha256  $archive" | sha256sum --check --status || { rm -f "$archive"; echo "NATS archive checksum mismatch; refusing the binary" >&2; return 1; }
    tar --extract --gzip --file "$archive" --directory "$nats_cache" --strip-components=1 \
      "nats-server-$nats_version-linux-amd64/nats-server"
    rm -f "$archive"
  fi
  printf '%s\n' "$nats_cache/nats-server"
}

case "${1:-}" in
  start)
    rm -rf "$nats_root"
    mkdir -p "$nats_root/data"
    nats_bin="${NATS_SERVER_BIN:-$(command -v nats-server || fetch_pinned_nats || true)}"
    if [[ -n "$nats_bin" ]]; then
      "$nats_bin" -js -sd "$nats_root/data" -a 127.0.0.1 -p "$port" >"$nats_root/nats.log" 2>&1 &
      echo $! >"$nats_root/pid"
    elif command -v docker >/dev/null 2>&1; then
      docker run --detach --rm --name "$container_name" -p "127.0.0.1:${port}:4222" \
        nats:2.10-alpine -js >/dev/null
    else
      echo "no NATS broker: set NATS_SERVER_BIN, install nats-server, or install Docker" >&2
      exit 1
    fi
    for _ in $(seq 1 50); do
      (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null && break
      sleep 0.2
    done
    (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null || { echo "NATS did not start" >&2; exit 1; }
    printf 'export PLATFORM_TEST_NATS_URL=nats://127.0.0.1:%s\n' "$port"
    ;;
  stop)
    if [[ -f "$nats_root/pid" ]]; then
      kill "$(cat "$nats_root/pid")" 2>/dev/null || true
    fi
    docker rm --force "$container_name" >/dev/null 2>&1 || true
    rm -rf "$nats_root"
    ;;
  *)
    echo "usage: $0 start|stop" >&2
    exit 2
    ;;
esac
