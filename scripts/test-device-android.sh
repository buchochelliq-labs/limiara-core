#!/usr/bin/env bash
# #213: the device crates and the app shell on Android. Linux with the
# Android SDK and NDK; CI runs it in the `android` workflow.
#
#   test-device-android.sh build
#     Builds platform-objects, platform-sync and platform-device for
#     arm64-v8a and x86_64, builds the x86_64 test binaries, and packages the
#     Tauri shell as an x86_64 debug APK. No device needed.
#   test-device-android.sh device
#     With an emulator attached (adb): runs the object engine's conformance
#     suite on SQLite and the platform-device tests on it, installs and
#     launches the APK, and records its size and cold start in the job
#     summary against ADR 0024's footprint budget.
set -Eeuo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

phase="${1:-}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
work_root="$repository_root/target/android"
tests_list="$work_root/tests.txt"
shell_dir=apps/device-shell/src-tauri
api_level=24
remote=/data/local/tmp/limiara

log() { printf '== %s\n' "$*"; }
fail() { echo "$*" >&2; exit 1; }

ndk_environment() {
  : "${ANDROID_HOME:?ANDROID_HOME must point at the Android SDK}"
  export NDK_HOME="${NDK_HOME:-${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}}"
  [[ -n "$NDK_HOME" && -d "$NDK_HOME" ]] || fail "no Android NDK: set NDK_HOME"
  export ANDROID_NDK_HOME="$NDK_HOME" ANDROID_NDK_ROOT="$NDK_HOME"
  # OpenSSL's Android configuration (SQLCipher's vendored build) looks for
  # the NDK's clang on PATH.
  export PATH="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"
}

build() {
  ndk_environment
  mkdir -p "$work_root"
  crates=(-p platform-objects -p platform-sync -p platform-device)
  rustup target add aarch64-linux-android x86_64-linux-android
  command -v cargo-ndk >/dev/null || cargo install cargo-ndk --version "^3.5" --locked

  log "building the device crates for arm64-v8a and x86_64"
  cargo ndk --platform "$api_level" -t arm64-v8a -t x86_64 build --locked "${crates[@]}"

  log "building the test binaries for the x86_64 emulator"
  # Built in two invocations: `--test` filters the targets of every package
  # on the command line, so it would drop platform-device's own tests.
  test_binaries() {
    cargo ndk --platform "$api_level" -t x86_64 test --locked --no-run --message-format=json "$@" |
      jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .executable != null) | .executable'
  }
  conformance=$(test_binaries -p platform-objects --test store_sqlite)
  device=$(test_binaries -p platform-device)
  [[ -n "$conformance" ]] || fail "the conformance suite was not built"
  grep -q '/bridge-' <<<"$device" || fail "the platform-device bridge tests were not built"
  printf '%s\n%s\n' "$conformance" "$device" >"$tests_list"

  log "packaging the app shell as an x86_64 APK"
  pnpm --filter web build
  (
    cd "$shell_dir"
    [[ -d gen/android ]] || cargo tauri android init --ci
    cargo tauri android build --ci --debug --apk --target x86_64
  )
  apk=$(find "$shell_dir/gen/android" -name '*.apk' -path '*debug*' | head -n 1)
  [[ -n "$apk" ]] || fail "the APK was not built"
  echo "$apk" >"$work_root/apk.txt"
  log "built $apk"
}

device() {
  command -v adb >/dev/null || fail "adb is not on PATH"
  [[ -s "$tests_list" && -s "$work_root/apk.txt" ]] || fail "run the build phase first"
  adb wait-for-device
  adb shell "rm -rf $remote && mkdir -p $remote/tmp"

  log "running the conformance suite and the platform-device tests on the emulator"
  while IFS= read -r binary; do
    name=$(basename "$binary")
    log "emulator: $name"
    # adb reads stdin; without </dev/null it would swallow the rest of the list.
    adb push "$binary" "$remote/$name" >/dev/null </dev/null
    # `adb shell` returns the command's exit status (adb 1.0.39 and later);
    # the marker guards against an older adb that does not.
    output=$(adb shell "cd $remote && chmod 755 ./$name && TMPDIR=$remote/tmp ./$name; echo \"exit=\$?\"" </dev/null)
    printf '%s\n' "$output"
    grep -q '^exit=0' <<<"$(tr -d '\r' <<<"$output")" || fail "$name failed on the emulator"
  done <"$tests_list"

  apk=$(cat "$work_root/apk.txt")
  package=$(sed -n 's/.*"identifier": *"\([^"]*\)".*/\1/p' "$shell_dir/tauri.conf.json")
  [[ -n "$package" ]] || fail "no app identifier in tauri.conf.json"
  log "installing and launching $(basename "$apk")"
  adb install -r "$apk" >/dev/null
  activity=$(adb shell cmd package resolve-activity --brief "$package" | tr -d '\r' | tail -n 1)
  [[ "$activity" == */* ]] || fail "no launcher activity for $package"
  adb logcat -c
  # Cold start: `am start -W` waits for the first frame and reports it.
  launch=$(adb shell am start -W -n "$activity" | tr -d '\r')
  printf '%s\n' "$launch"
  total=$(sed -n 's/^TotalTime: *//p' <<<"$launch")
  sleep 10
  adb shell pidof "$package" >/dev/null || {
    # The app's own lines, Rust's output, and any crash or tombstone.
    adb logcat -d -v brief |
      grep -E "$package|RustStdoutStderr|AndroidRuntime|DEBUG|libc|Tauri|wry|ActivityManager|ActivityTaskManager" |
      tail -n 200 || true
    fail "the app exited within 10 seconds of launch"
  }

  apk_size=$(du -k "$apk" | awk '{print $1}')
  library=$(unzip -l "$apk" | awk '/lib\/x86_64\/.*\.so$/ {print $1, $4}' | sort -n | tail -n 1)
  {
    echo "### Android footprint (debug x86_64 build)"
    echo
    echo "| Measure | Value |"
    echo "| --- | --- |"
    echo "| APK | $((apk_size / 1024)) MiB |"
    echo "| Largest native library | $(awk '{printf "%d MiB (%s)", $1 / 1048576, $2}' <<<"$library") |"
    echo "| Cold launch (\`am start -W\`) | ${total:-unknown} ms |"
    echo
    echo "ADR 0024's budget is for release device builds; a debug build is an upper bound."
  } >>"$summary"
  log "Android build, emulator tests and app launch passed"
}

case "$phase" in
  build) build ;;
  device) device ;;
  *) fail "usage: $0 build|device" ;;
esac
