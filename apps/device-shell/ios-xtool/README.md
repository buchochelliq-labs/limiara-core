# iOS shell with xtool (experiment)

This folder builds the iOS app shell (`apps/device-shell`) with
[xtool](https://github.com/xtool-org/xtool) instead of Xcode, so it can be
built on Linux or on Windows through WSL and installed on a connected
iPhone. It is an experiment. The supported iOS builds are still the ones
that run on macOS: the `ios` workflow (simulator) and the `testflight`
workflow. This folder does not replace them. CI does not build the app
here; it only checks the setup tool in `cli/` (the `ios-sideload-cli`
job).

## What it builds

- **The Rust shell** (`src-tauri`, crate `device-shell`) as a static
  library for `aarch64-apple-ios`, with the web shell embedded, exactly
  as the Xcode build embeds it.
- **The Swift halves** of the shell's native plugins (`device-browser`,
  `device-push`, `device-route-probe`) and Tauri's own iOS package. Tauri
  links these only when it builds on macOS. Here SwiftPM builds and links
  them.
- **A Swift entry point** that calls the Rust `start_app`, as Tauri's
  generated `main.mm` does.

xtool packages the app, signs it with your Apple ID and installs it. The
bundle identifier is `app.limiara.shell.xtool`, so it sits next to a
TestFlight build of `app.limiara.shell` and never replaces it.

## What it does not do

- **No push notifications.** Sideloaded builds get no `aps-environment`
  entitlement unless the Apple ID belongs to a paid developer team and
  the entitlement is added to `xtool.yml` (`entitlementsPath`).
- **No add-on plugins.** Plugins that need Xcode's App Intents metadata
  step are not built here.
- **No App Store or TestFlight upload.** Use the `testflight` workflow.
- **A free Apple ID's builds expire after 7 days** and must be installed
  again. A paid developer account's last a year.

## The way in: `limiara-sideload`

A guided setup does everything below for you, asks what it needs to know,
and repeats xtool's own questions and warnings before xtool asks them.

- **On Windows:** download this folder's `setup-windows.ps1` and run, in
  PowerShell,

  ```powershell
  powershell -ExecutionPolicy Bypass -File .\setup-windows.ps1
  ```

  It asks for administrator rights, installs WSL with Ubuntu 24.04 (if
  Windows wants a restart it says so: restart and run it again), installs
  usbipd-win with winget, clones this repository inside Ubuntu, builds
  the tool and starts it. `-ExecutionPolicy Bypass` applies to that one
  PowerShell process only.
- **Already in WSL's Ubuntu:** from a clone, run

  ```sh
  apps/device-shell/ios-xtool/cli/bootstrap.sh
  ```

  It installs git, curl and a C compiler (apt), rustup if missing
  (checked against its SHA-256), builds `limiara-sideload` and runs
  `limiara-sideload setup`.

`limiara-sideload setup` can be stopped and run again: each step checks
whether it is already done and skips it. Its steps, in order: WSL and
Ubuntu; apt packages; Swift 6.4 (swiftly); xtool; Rust and the iOS
target; Node.js 22 and pnpm; the repository and branch; Xcode.xip and the
iOS SDK; the Apple login; the iPhone (usbipd, usbmuxd); build and install.

Other commands: `doctor` (a table of every check, including the login
token's permissions, the SDK, the login and the phone; `--fix` tightens
permissions), `login` (log in again, or switch account or mode), `sdk`,
`phone`, `run` (`prepare.sh` then `xtool dev`; run it again every 7 days
with a free Apple ID). `--yes` answers yes/no confirmations with their
default; it never answers for a password, a code or a key.

## Security

What the tool asks for, and where things end up:

- **Apple ID password and two-factor code: never seen by this tool.** In
  password mode it asks only for the Apple ID, shows xtool's warning, and
  runs `xtool auth login --mode password -u <Apple ID>` on your terminal:
  you type the password (hidden), the code and the team into xtool
  itself. xtool's `-p` option is never used. No intuiTUIve screen is
  active while xtool or sudo has the terminal.
- **Linux password:** typed into `sudo`, which runs only `apt-get` and
  `usbmuxd`. Each sudo command is shown and confirmed first.
- **API key (`AuthKey_<KEYID>.p8`):** the tool asks for the Key ID and
  Issuer ID (validated) and where the file is (it looks in the Windows
  Downloads folders). It checks the file looks like a PEM private key
  without printing any of it, then copies it to
  `~/.config/limiara-sideload/keys/AuthKey_<KEYID>.p8` (folder `0700`,
  file `0600`). It refuses to keep a key under `/mnt` or inside any git
  working tree, and offers to delete the original: a key on the Windows
  drive (NTFS under `/mnt`) is readable by Windows apps, sync and backup
  tools. Apple lets you download a key once, so keep your backup in a
  password manager. xtool is then given the Key ID, the Issuer ID and the
  key's **path** on its standard input; the key's contents never go
  through command lines, environment variables, logs or the state file.
- **xtool's login token:** xtool keeps it in
  `~/.config/xtool/data/XTLAuthToken`; in API-key mode it contains the
  private key. xtool writes it with default permissions, so the tool runs
  xtool with `umask 077` and then sets `~/.config/xtool` and its `data`
  folder to `0700` and the token to `0600`. `doctor` reports anything
  looser.
- **The state file** `~/.config/limiara-sideload/state.toml` (`0600`)
  holds the repository path, the branch, the Apple ID, the login mode,
  the key file's location and the finished steps. No secrets.
- **Downloads** use HTTPS only and are checked before anything runs:
  Node.js against the pinned SHA-256 and its `SHASUMS256.txt`;
  rustup-init against the pinned SHA-256 and its published `.sha256`;
  swiftly against its signature by a pinned swift.org key (swiftly then
  verifies the toolchain); the xtool AppImage against the SHA-256 GitHub
  records for the pinned release's asset (xtool publishes no checksum
  file), and against a pinned SHA-256 once one is recorded. pnpm comes
  through corepack, which verifies it. Every pin is in
  `cli/src/versions.rs`.
- Error messages carry a stable code (`LS0xx_…`) and a plain sentence;
  they never quote a key, a password or a command's full arguments.

### Updating the pins

All pins are in `cli/src/versions.rs`; change them there only.

- **Node.js:** take the version from <https://nodejs.org/dist/index.json>
  and the `linux-x64.tar.xz` and `linux-arm64.tar.xz` lines of that
  release's `SHASUMS256.txt`.
- **rustup:** the version from
  <https://static.rust-lang.org/rustup/release-stable.toml>, and each
  `rustup-init.sha256` under `rustup/archive/<version>/<target>/`.
- **Swift:** `SWIFT_VERSION` follows xtool's Linux guide; the signing-key
  fingerprints come from <https://www.swift.org/keys/all-keys.asc>.
- **xtool:** set `XTOOL_VERSION` to a release tag, then record each
  AppImage's SHA-256 in `XTOOL_SHA256_X86_64` / `XTOOL_SHA256_AARCH64`
  (`sha256sum xtool-x86_64.AppImage`, compared with the asset's `digest`
  in `https://api.github.com/repos/xtool-org/xtool/releases/tags/<tag>`).
  Also update `XTOOL_SDK_VERSION` from that tag's
  `Sources/XToolSupport/SDKBuilder.swift` (`currentSDKVersion`) and
  `XCODE_VERSION` from its Linux guide.
- **Rust and pnpm** follow `rust-toolchain.toml` and `package.json`; the
  tests fail when they disagree.

## Manual setup (reference)

What the tool automates, by hand.

### Set up WSL (once)

On Windows, install WSL with Ubuntu, then
[usbipd](https://learn.microsoft.com/en-us/windows/wsl/connect-usb) so the
iPhone reaches WSL over USB. Inside Ubuntu:

1. Install the Swift toolchain that xtool's
   [Linux installation guide](https://github.com/xtool-org/xtool/blob/main/Documentation/xtool.docc/Installation-Linux.md)
   names, and `usbmuxd`.
2. Download the Xcode version that guide names from
   <https://developer.apple.com/download/all/?q=Xcode>, in a browser,
   signed in with your Apple ID.
3. Install xtool and run `xtool setup`, giving it the path to `Xcode.xip`.
   It extracts the iOS SDK into `~/.swiftpm/swift-sdks/darwin.artifactbundle`
   and asks you to log in. With a paid developer account, choose the API
   key: a Team Key with the App Manager role. Keep the key file in WSL,
   never in this repository.
4. Install the build tools for the Rust half:

   ```sh
   sudo apt-get install clang llvm perl make rsync
   rustup target add aarch64-apple-ios
   ```

   You also need Node.js 22 and pnpm 9 for the web shell, as for any
   build of this repository.

### Build and install

With the iPhone connected, unlocked and trusting the computer:

```sh
cd apps/device-shell/ios-xtool
./prepare.sh
xtool dev
```

`prepare.sh` builds the web shell, then the Rust library, then copies the
Swift packages into `build/`. It stops with a message naming what is
missing: a tool, the Rust target or the iOS SDK. If your Darwin SDK is
somewhere else, set `XTOOL_DARWIN_SDK` to its `darwin.artifactbundle`.
`xtool dev` builds the Swift side, links everything, signs it and
installs it. On the phone, trust the developer profile under
**Settings → General → VPN & Device Management** the first time.

## Not yet confirmed

None of this has run yet. The cloud session that wrote it has no Swift
toolchain, no iOS SDK and no iPhone. These are the likely first failures
and what to try:

- **Missing symbols at the final link** (`_OBJC_CLASS_$_…`, framework
  functions). The Rust library needs a system framework that
  `Package.swift` does not list. `prepare.sh` prints rustc's
  `native-static-libs` line; add any framework it names to
  `linkedFramework`.
- **Missing `_init_plugin_…` or Tauri Swift symbols.** A Swift package was
  not linked. Check that `build/packages` holds all three plugins and
  `.tauri/tauri-api`.
- **`unsafeFlags` refused.** SwiftPM allows unsafe flags only in root and
  local packages. If xtool's builder still refuses them, wrap
  `libdevice_shell.a` in a hand-made `.xcframework` and use a
  `binaryTarget` instead of `-L`.
- **A C dependency fails to compile** (OpenSSL, SQLCipher, ring). Check that
  `clang` can find the SDK: `prepare.sh` passes it as `SDKROOT` and as
  `-isysroot`.
- **The app starts and closes at once.** Read the device log with
  `idevicesyslog` while it starts. The Rust shell logs why it could not
  start.

Report what happens and it can be fixed from there.

## Follow-up

- The xtool AppImage's SHA-256 is not pinned in `cli/src/versions.rs` yet
  (`XTOOL_SHA256_*` are `None`): the session that wrote the tool could not
  reach GitHub's release downloads. Until it is, the tool relies on the
  digest GitHub records for the release asset, and refuses to install
  xtool when that cannot be read.
- `setup-windows.ps1`, the WSL-only steps (usbipd, explorer.exe), Apple's
  login and the phone have not been run yet; the tool's own logic is
  covered by `cargo test` in `cli/`.
