#!/usr/bin/env bash
# #386: the device crates and the app shell on iOS. macOS with Xcode only;
# CI runs it in the `ios` workflow.
#
# 1. Builds platform-objects, platform-sync and platform-device for iPhone
#    (aarch64-apple-ios) and for the Apple-silicon simulator.
# 2. Runs the object engine's conformance suite on SQLite and the
#    platform-device tests inside a booted iOS Simulator: the test binaries
#    are built for the simulator and started there with `simctl spawn`.
# 3. Packages the Tauri shell as a simulator app (`cargo tauri ios build`),
#    installs and launches it, and records its size and cold start in the
#    job summary against ADR 0024's footprint budget.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

[[ "$(uname -s)" == Darwin ]] || { echo "iOS builds need macOS with Xcode" >&2; exit 1; }

summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
work_root="$repository_root/target/ios"
mkdir -p "$work_root/tmp"
simulator=""

log() { printf '== %s\n' "$*"; }
cleanup() {
  status=$?
  [[ -n "$simulator" ]] && { xcrun simctl shutdown "$simulator" >/dev/null 2>&1 || true; xcrun simctl delete "$simulator" >/dev/null 2>&1 || true; }
  exit "$status"
}
trap cleanup EXIT

crates=(-p platform-objects -p platform-sync -p platform-device)
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

log "building the device crates for iPhone and the simulator"
cargo build --locked --target aarch64-apple-ios "${crates[@]}"
cargo build --locked --target aarch64-apple-ios-sim "${crates[@]}"

log "booting a simulator"
# The newest available iOS runtime, and an iPhone that runtime supports: the
# newest device type overall can be one the runtime cannot run.
runtime_json=$(xcrun simctl list runtimes --json |
  jq '[.runtimes[] | select(.isAvailable and .platform == "iOS")] | sort_by(.version | split(".") | map(tonumber)) | last')
runtime=$(jq -r '.identifier' <<<"$runtime_json")
device_type=$(jq -r '[.supportedDeviceTypes[]? | select(.productFamily == "iPhone")] | last | .identifier' <<<"$runtime_json")
[[ -n "$runtime" && "$runtime" != null && -n "$device_type" && "$device_type" != null ]] ||
  { echo "no iOS simulator runtime is installed" >&2; exit 1; }
simulator=$(xcrun simctl create "limiara-ci-$$" "$device_type" "$runtime")
xcrun simctl boot "$simulator"
xcrun simctl bootstatus "$simulator" -b >/dev/null

log "running the conformance suite and the platform-device tests on the simulator"
# Built in two invocations: `--test` filters the targets of every package
# on the command line, so it would drop platform-device's own tests.
test_binaries() {
  cargo test --locked --no-run --target aarch64-apple-ios-sim --message-format=json "$@" |
    jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .executable != null) | .executable'
}
conformance=$(test_binaries -p platform-objects --test store_sqlite)
device=$(test_binaries -p platform-device)
[[ -n "$conformance" ]] || { echo "the conformance suite was not built" >&2; exit 1; }
grep -q '/bridge-' <<<"$device" || { echo "the platform-device bridge tests were not built" >&2; exit 1; }
tests=$(printf '%s\n%s\n' "$conformance" "$device")
while IFS= read -r binary; do
  log "simulator: $(basename "$binary")"
  # The simulator shares the Mac's file system, so a host directory serves
  # as the tests' temporary directory.
  SIMCTL_CHILD_TMPDIR="$work_root/tmp" xcrun simctl spawn "$simulator" "$binary"
done <<<"$tests"

log "packaging the app shell for the simulator"
pnpm --filter web build
(
  cd apps/device-shell/src-tauri
  [[ -d gen/apple ]] || cargo tauri ios init --ci
  cargo tauri ios build --ci --debug --target aarch64-sim
)
app=$(find apps/device-shell/src-tauri/gen/apple/build -type d -name '*.app' -path '*sim*' | head -n 1)
[[ -n "$app" ]] || app=$(find apps/device-shell/src-tauri/gen/apple -type d -name '*.app' | head -n 1)
[[ -n "$app" ]] || { echo "the simulator app was not built" >&2; exit 1; }
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")

log "installing and launching $(basename "$app")"
xcrun simctl install "$simulator" "$app"
# Cold start: `simctl launch` returns once the app's process has started.
started=$(python3 -c 'import time; print(time.time())')
xcrun simctl launch "$simulator" "$bundle_id" >/dev/null
launched=$(python3 -c 'import time; print(time.time())')
sleep 10
xcrun simctl spawn "$simulator" launchctl list | grep -q "$bundle_id" ||
  { echo "the app exited within 10 seconds of launch" >&2; exit 1; }

app_size=$(du -sk "$app" | awk '{print $1}')
binary_size=$(du -k "$app/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Info.plist")" | awk '{print $1}')
{
  echo "### iOS footprint (debug simulator build)"
  echo
  echo "| Measure | Value |"
  echo "| --- | --- |"
  echo "| App bundle | $((app_size / 1024)) MiB |"
  echo "| Executable | $((binary_size / 1024)) MiB |"
  echo "| Cold launch (\`simctl launch\`) | $(python3 -c "print(f'{$launched - $started:.1f} s')") |"
  echo
  echo "ADR 0024's budget is for release device builds; a debug simulator build is an upper bound."
} >>"$summary"
log "iOS build, simulator tests and app launch passed"
