#!/usr/bin/env bash
# Builds limiara-sideload inside WSL and starts its guided setup.
#
# Run it from a checkout (apps/device-shell/ios-xtool/cli/bootstrap.sh),
# or anywhere with LIMIARA_REPO pointing at where the repository should
# be cloned. It installs only what building the tool needs (git, curl,
# a C compiler, rustup) and leaves everything else to the tool itself.
#
#   bootstrap.sh [--branch NAME] [--yes] [setup arguments...]
#
# Nothing is piped from the network into a shell: rustup-init is
# downloaded to a temporary file and checked against the SHA-256 pinned
# in src/versions.rs and the one rustup publishes before it runs.
set -euo pipefail

repo_url="https://github.com/buchochelliq-labs/limiara-core"
branch="claude/xtool-sideload-spike"
assume_yes=0
forward=()
while [ $# -gt 0 ]; do
    case "$1" in
        --branch) branch="${2:?--branch needs a name}"; shift 2 ;;
        --yes|-y) assume_yes=1; forward+=("$1"); shift ;;
        *) forward+=("$1"); shift ;;
    esac
done

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
fail() { printf 'bootstrap: %s\n' "$*" >&2; exit 1; }
confirm() {
    [ "$assume_yes" = 1 ] && return 0
    local answer
    read -r -p "$1 [Y/n] " answer
    case "$answer" in ""|y|Y|yes|Yes) return 0 ;; *) return 1 ;; esac
}

case "$branch" in
    -*|*..*|*[!A-Za-z0-9._/-]*) fail "not a branch name: $branch" ;;
esac
[ "$(id -u)" != 0 ] || fail "run this as your own Linux user, not root"

# 1. What building the tool needs, from Ubuntu's archive.
missing=()
for pkg in git curl build-essential ca-certificates; do
    dpkg-query -W -f '${Status}' "$pkg" 2>/dev/null | grep -qx 'install ok installed' || missing+=("$pkg")
done
if [ "${#missing[@]}" -gt 0 ]; then
    say "These packages are needed to build the setup tool: ${missing[*]}"
    echo "  sudo apt-get update"
    echo "  sudo apt-get install -y ${missing[*]}"
    echo "sudo asks for your Linux password; it is typed into sudo, not into this script."
    confirm "Run them now?" || fail "stopped"
    sudo apt-get update
    sudo apt-get install -y "${missing[@]}"
fi

# 2. The repository: this checkout, or a clone.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$here/src/versions.rs" ] && [ -e "$here/../../../../.git" ]; then
    repo="$(cd "$here/../../../.." && pwd)"
else
    repo="${LIMIARA_REPO:-$HOME/limiara-core}"
    if [ ! -e "$repo/.git" ]; then
        say "Cloning $repo_url ($branch) into $repo"
        git clone --branch "$branch" -- "$repo_url" "$repo"
    fi
fi
cli="$repo/apps/device-shell/ios-xtool/cli"
[ -f "$cli/Cargo.toml" ] || fail "$repo has no apps/device-shell/ios-xtool/cli; check out $branch there"

# 3. rustup, verified, when cargo is missing.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
    versions="$cli/src/versions.rs"
    rustup_version="$(sed -n 's/^pub const RUSTUP_VERSION: &str = "\(.*\)";$/\1/p' "$versions")"
    case "$(uname -m)" in
        x86_64) arch=x86_64; pin_name=RUSTUP_SHA256_X86_64 ;;
        aarch64) arch=aarch64; pin_name=RUSTUP_SHA256_AARCH64 ;;
        *) fail "unsupported architecture $(uname -m)" ;;
    esac
    pinned="$(grep -A1 "^pub const $pin_name" "$versions" | grep -oE '[0-9a-f]{64}' | head -n 1)"
    if [ -z "$rustup_version" ] || [ -z "$pinned" ]; then
        fail "no rustup pin in $versions"
    fi
    url="https://static.rust-lang.org/rustup/archive/$rustup_version/$arch-unknown-linux-gnu/rustup-init"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    say "Downloading rustup $rustup_version"
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$tmp/rustup-init" "$url"
    published="$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$url.sha256" | grep -oE '^[0-9a-f]{64}')"
    actual="$(sha256sum "$tmp/rustup-init" | cut -d ' ' -f 1)"
    if [ "$actual" != "$pinned" ] || [ "$actual" != "$published" ]; then
        fail "rustup-init's SHA-256 does not match the pinned and published values"
    fi
    chmod 700 "$tmp/rustup-init"
    "$tmp/rustup-init" -y --profile minimal --default-toolchain none
fi

# 4. Build the tool (the repository's rust-toolchain.toml picks Rust) and
#    start the setup.
say "Building limiara-sideload"
(cd "$cli" && cargo build --release --locked)
say "Starting the guided setup"
exec "${CARGO_TARGET_DIR:-$cli/target}/release/limiara-sideload" setup ${forward[@]+"${forward[@]}"}
