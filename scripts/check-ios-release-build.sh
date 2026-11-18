#!/usr/bin/env bash
# Checks that a release build of the app shell carries no debug-only code
# and is signed for App Store distribution. The `testflight` workflow runs it
# before uploading anything to App Store Connect.
#
#   check-ios-release-build.sh web <apps/web/dist>
#     The web build embedded in the app: Vite drops the `/__ui` showcase,
#     the `?shell-state=` and `?form-state=` probes and the Contacts fixture
#     client from production builds (`import.meta.env.DEV` and `MODE`). This
#     fails if any of them is still in the bundle. It runs anywhere.
#
#   check-ios-release-build.sh ipa <app.ipa> <bundle-version> <short-version>
#     The exported IPA (macOS only): the bundle identifier and versions, the
#     export compliance key, distribution entitlements (production APNs, no
#     debugger attachment) and no trace of the debug simulator's push hook,
#     which `#if DEBUG && targetEnvironment(simulator)` keeps out of device
#     builds.
set -Eeuo pipefail

bundle_identifier="app.limiara.shell"
failures=0
fail() { printf 'release check failed: %s\n' "$*" >&2; failures=$((failures + 1)); }
pass() { printf 'release check: %s\n' "$*"; }

# Text that exists only in debug-only code. Each marker comes from one
# source file, named so a failure points at what leaked.
web_markers=(
  "Accessible, quiet, and made for work|apps/web/src/showcase/UiShowcase.tsx (the /__ui showcase)"
  "usr-hannah|apps/web/src/contacts/fixture-client.ts (the Contacts fixture client)"
  "form-state|apps/web/src/contacts/client.ts (the ?form-state= probe)"
)
native_markers=(
  "simulate-push|push/ios/Sources/PushPlugin.swift (the simulated push watcher)"
  "delivering a simulated push|push/ios/Sources/PushPlugin.swift (the simulated push watcher)"
  "keystore-probe-plaintext|src/keychain.rs (the Android Keystore probe)"
)

check_web() {
  local dist=$1
  [[ -f "$dist/index.html" ]] || { fail "$dist/index.html does not exist; build the web shell first"; return; }
  local entry marker source before=$failures
  for entry in "${web_markers[@]}"; do
    marker=${entry%%|*}
    source=${entry#*|}
    if grep -rqF --include='*.js' --include='*.html' -e "$marker" "$dist"; then
      fail "the web build contains debug-only code from $source"
    fi
  done
  if ((failures == before)); then pass "the web build has no debug-only showcase, probes or fixture client"; fi
}

plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null || true; }

check_ipa() {
  local ipa=$1 expected_build=$2 expected_short=$3 before=$failures
  [[ "$(uname -s)" == Darwin ]] || { fail "checking an IPA needs macOS"; return; }
  [[ -f "$ipa" ]] || { fail "$ipa does not exist"; return; }
  local work
  work=$(mktemp -d)
  # shellcheck disable=SC2064 # expand now: the directory is local
  trap "rm -rf '$work'" RETURN
  unzip -q "$ipa" -d "$work"
  local app
  app=$(find "$work/Payload" -maxdepth 1 -type d -name '*.app' | head -n 1)
  [[ -n "$app" ]] || { fail "the IPA has no Payload/*.app"; return; }
  local info="$app/Info.plist"

  local value
  value=$(plist_value "$info" CFBundleIdentifier)
  [[ "$value" == "$bundle_identifier" ]] || fail "CFBundleIdentifier is '$value', expected $bundle_identifier"
  value=$(plist_value "$info" CFBundleVersion)
  [[ "$value" == "$expected_build" ]] || fail "CFBundleVersion is '$value', expected $expected_build"
  value=$(plist_value "$info" CFBundleShortVersionString)
  [[ "$value" == "$expected_short" ]] || fail "CFBundleShortVersionString is '$value', expected $expected_short"
  value=$(plist_value "$info" ITSAppUsesNonExemptEncryption)
  [[ "$value" == true ]] || fail "ITSAppUsesNonExemptEncryption is '$value', expected true (docs/operations/testflight.md)"
  plist_value "$info" UIBackgroundModes | grep -q remote-notification ||
    fail "UIBackgroundModes does not declare remote-notification"

  local entitlements="$work/entitlements.plist"
  codesign -d --entitlements - --xml "$app" >"$entitlements" 2>/dev/null ||
    { fail "could not read the app's code signature"; return; }
  value=$(plist_value "$entitlements" aps-environment)
  [[ "$value" == production ]] || fail "aps-environment is '$value', expected production"
  value=$(plist_value "$entitlements" get-task-allow)
  [[ "$value" != true ]] || fail "get-task-allow is true: the app is signed for development, not distribution"
  [[ -f "$app/embedded.mobileprovision" ]] || fail "the app has no embedded provisioning profile"

  local executable
  executable="$app/$(plist_value "$info" CFBundleExecutable)"
  [[ -f "$executable" ]] || { fail "the app's executable is missing"; return; }
  local entry marker source
  for entry in "${native_markers[@]}"; do
    marker=${entry%%|*}
    source=${entry#*|}
    if strings -a "$executable" | grep -qF -e "$marker"; then
      fail "the executable contains debug-only code from apps/device-shell/src-tauri/$source"
    fi
  done
  # The web build is embedded in the executable too.
  for entry in "${web_markers[@]}"; do
    marker=${entry%%|*}
    source=${entry#*|}
    if strings -a "$executable" | grep -qF -e "$marker"; then
      fail "the executable embeds debug-only web code from $source"
    fi
  done
  if ((failures == before)); then
    pass "$(basename "$ipa"): $bundle_identifier $expected_short ($expected_build), distribution-signed, no debug-only code"
  fi
}

case "${1:-}" in
  web)
    [[ $# -eq 2 ]] || { echo "usage: $0 web <dist>" >&2; exit 2; }
    check_web "$2"
    ;;
  ipa)
    [[ $# -eq 4 ]] || { echo "usage: $0 ipa <app.ipa> <bundle-version> <short-version>" >&2; exit 2; }
    check_ipa "$2" "$3" "$4"
    ;;
  *)
    echo "usage: $0 web <dist> | ipa <app.ipa> <bundle-version> <short-version>" >&2
    exit 2
    ;;
esac

if ((failures > 0)); then
  exit 1
fi
