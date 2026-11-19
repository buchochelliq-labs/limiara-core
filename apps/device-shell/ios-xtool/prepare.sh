#!/usr/bin/env bash
# Prepares the xtool project in this directory on Linux or Windows (WSL):
#
#   1. builds the web shell (apps/web/dist), which the Rust crate embeds;
#   2. builds the Rust shell (crate device-shell) as a static library for
#      aarch64-apple-ios against the iPhoneOS SDK that `xtool setup`
#      extracted from Xcode.xip, into build/rust/libdevice_shell.a;
#   3. copies the shell's Swift plugin packages and Tauri's own iOS
#      package into build/packages, laid out as their Package.swift files
#      expect (each plugin finds Tauri at ../.tauri/tauri-api).
#
# Then `xtool dev` builds, signs and installs the app. See README.md.
#
# Tauri links its Swift packages only when the build runs on macOS, so on
# Linux the Rust library leaves those symbols to the final link, which
# SwiftPM does here with the copied packages.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
shell="$(cd "$here/../src-tauri" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
build="$here/build"
target="aarch64-apple-ios"
min_ios="14.0"

fail() {
    echo "prepare: $*" >&2
    exit 1
}

for tool in clang llvm-ar llvm-ranlib perl make rsync cargo rustup pnpm; do
    command -v "$tool" >/dev/null 2>&1 || fail "'$tool' is not installed (see README.md)"
done
rustup target list --installed | grep -qx "$target" \
    || fail "the Rust target $target is missing: rustup target add $target"

# The iPhoneOS SDK inside the Darwin SDK that `xtool setup` installed.
sdk_bundle="${XTOOL_DARWIN_SDK:-$HOME/.swiftpm/swift-sdks/darwin.artifactbundle}"
[ -d "$sdk_bundle" ] || fail "no Darwin SDK at $sdk_bundle: run 'xtool setup' first"
sdk="$(find -L "$sdk_bundle" -maxdepth 10 -type d -path '*iPhoneOS.platform/Developer/SDKs/iPhoneOS*.sdk' -prune -print \
    | sort | tail -n 1)"
[ -n "$sdk" ] || fail "no iPhoneOS SDK under $sdk_bundle"
echo "prepare: iPhoneOS SDK: $sdk"

echo "prepare: building the web shell"
(cd "$repo" && pnpm install --frozen-lockfile && pnpm --filter web build)

echo "prepare: building the Rust shell for $target"
export SDKROOT="$sdk"
export IPHONEOS_DEPLOYMENT_TARGET="$min_ios"
export CC_aarch64_apple_ios=clang
export CXX_aarch64_apple_ios=clang++
export AR_aarch64_apple_ios=llvm-ar
# The vendored OpenSSL indexes its archives with ranlib, which otherwise
# falls back to GNU ranlib. That cannot read Mach-O archives: its first
# pass leaves libcrypto.a malformed (OpenSSL's build ignores that), and
# its second, in `make install_dev`, fails the build.
export RANLIB_aarch64_apple_ios=llvm-ranlib
# The vendored OpenSSL reads the SDK from -isysroot (its ios64-cross
# configuration), so no xcrun is needed.
export CFLAGS_aarch64_apple_ios="--target=arm64-apple-ios$min_ios -isysroot $sdk"
export CXXFLAGS_aarch64_apple_ios="$CFLAGS_aarch64_apple_ios"
(
    cd "$shell"
    # Only the static library: the cdylib would need an iOS linker.
    cargo rustc --locked --release --lib --target "$target" \
        --target-dir "$build/cargo" --crate-type staticlib \
        -- --print native-static-libs
)
mkdir -p "$build/rust"
cp "$build/cargo/$target/release/libdevice_shell.a" "$build/rust/"

echo "prepare: copying the Swift packages"
rm -rf "$build/packages"
mkdir -p "$build/packages/.tauri"
for plugin in browser push route-probe; do
    rsync -a --exclude .build --exclude .tauri --exclude Packages \
        "$shell/$plugin/ios/" "$build/packages/device-$plugin/"
done
tauri_version="$(awk '$0 == "name = \"tauri\"" { getline; gsub(/"/, "", $3); print $3; exit }' "$shell/Cargo.lock")"
[ -n "$tauri_version" ] || fail "no tauri version in Cargo.lock"
tauri_api="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 4 -type d \
    -path "*/tauri-$tauri_version/mobile/ios-api" | head -n 1)"
[ -n "$tauri_api" ] || fail "Tauri $tauri_version's iOS package is not in the cargo registry"
rsync -a --exclude .build "$tauri_api/" "$build/packages/.tauri/tauri-api/"

cat <<EOF
prepare: done.
  Compare the frameworks rustc printed above (native-static-libs) with
  the linkedFramework list in Package.swift, then, with the iPhone
  connected and trusted:
    cd "$here" && xtool dev
EOF
