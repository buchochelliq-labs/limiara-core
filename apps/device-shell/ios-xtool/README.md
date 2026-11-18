# iOS shell with xtool (experiment)

This folder builds the iOS app shell (`apps/device-shell`) with
[xtool](https://github.com/xtool-org/xtool) instead of Xcode, so it can be
built on Linux or on Windows through WSL and installed on a connected
iPhone. It is an experiment. The supported iOS builds are still the ones
that run on macOS: the `ios` workflow (simulator) and the `testflight`
workflow. This folder does not replace them, and nothing in CI runs it.

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

## Set up WSL (once)

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

## Build and install

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
