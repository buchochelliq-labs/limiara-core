#!/usr/bin/env bash
# Builds and checks the desktop app shell (apps/device-shell, #383):
# formatting, lints and the scheme tests, then launches the app under a
# virtual display and checks it stays up. Needs WebKitGTK 4.1, libsoup 3,
# GTK 3 and Xvfb (see apps/device-shell/README.md).
set -euo pipefail
cd "$(dirname "$0")/.."

pnpm --filter web build
cd apps/device-shell/src-tauri
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked

log="$(mktemp)"
export WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1
XDG_DATA_HOME="$(mktemp -d)" xvfb-run -a -s "-screen 0 1280x860x24" \
  timeout --preserve-status 15 ./target/debug/device-shell >"$log" 2>&1 || status=$?
# `timeout` stops the app with SIGTERM (143): the app was still running.
if [ "${status:-0}" -ne 143 ]; then
  echo "the shell exited early (status ${status:-0}):" >&2
  cat "$log" >&2
  exit 1
fi
if grep -qiE "panicked|could not start" "$log"; then
  cat "$log" >&2
  exit 1
fi
echo "device shell passed: scheme tests, and the app ran for 15 seconds"
