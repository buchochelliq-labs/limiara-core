# Device shell

The Limiara app for desktop, iOS and Android (ADR 0024 §5): the
same React shell as the browser (`apps/web`), packaged with Tauri 2 and
served from the `limiara://` scheme.

One scheme handler (`src-tauri/src/lib.rs`) answers the webview:

- `/api/...` goes to the local command bridge (`crates/platform-device`),
  which runs the server's own object routes over the encrypted replica on the
  device;
- everything else is the embedded web build, with `index.html` for the
  shell's client-side routes.

The shell and its API share one origin, so the web shell's relative
`/api/v1` requests need no changes, and nothing listens on a port. Every
response carries a strict content security policy: no third-party script
can run in the origin.

## Sign-in

The app opens signed out. The member enters the workspace address shown on
the web in Settings → Devices, and the app signs them in through their
default browser (ADR 0029 §7): the browser returns to
`http://127.0.0.1:47185/callback`, which the server must list in
`platform.devices.redirect_uris` (`LIMIARA_REDIRECT_URI` overrides it for
development). The first sync brings the workspace down, then the web shell
opens over it, with a bar for the sync state, the offline lease and
sign-out. Sign-out deletes the device's copy and its key.

The session and the database keys live in the platform keychain
(`src-tauri/src/keychain.rs`): the Keychain on macOS, the Credential Manager
on Windows, and the kernel keyring on Linux, which forgets them at logout.
The sign-in screen also opens a demonstration workspace with no server.

`bash scripts/test-device-app-sign-in.sh` runs the same sign-in controller
headless against the API and the Keycloak fixture.

## Build and run

It is its own Cargo workspace because it links the platform's GUI libraries,
which the server workspace must not need. On Debian or Ubuntu:

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libgtk-3-dev librsvg2-dev
pnpm --filter web build          # the shell embeds apps/web/dist
cd apps/device-shell/src-tauri
cargo run
```

`bash scripts/test-device-shell.sh` runs formatting, lints and the scheme
tests, then launches the app under Xvfb and checks that it stays up. CI runs
it in the `device-shell` job.

## iOS

The app is built from the library (`src/app.rs`, `tauri::mobile_entry_point`)
so Tauri can package it for iOS. On a Mac with Xcode and
`cargo install tauri-cli --version "^2.9" --locked`:

```sh
pnpm --filter web build
cd apps/device-shell/src-tauri
cargo tauri ios init     # generates gen/apple, which is not committed
cargo tauri ios dev      # or: cargo tauri ios build --debug --target aarch64-sim
```

`bash scripts/test-device-ios.sh` builds the device crates for iPhone and the
simulator, runs the object engine's conformance suite and the
`platform-device` tests inside an iOS Simulator, then packages, installs and
launches the app there. The `ios` workflow runs it on a macOS runner when
the device code changes.

## Android

With the Android SDK, the NDK (`NDK_HOME`), Java 17 and the Tauri CLI:

```sh
pnpm --filter web build
cd apps/device-shell/src-tauri
cargo tauri android init     # generates gen/android, which is not committed
cargo tauri android dev      # or: cargo tauri android build --debug --apk --target x86_64
```

On Android the keychain is the Android Keystore, through the
`device-keystore` plugin (`keystore/`): its Kotlin half keeps an AES-256-GCM
key in the Keystore and writes only ciphertext to the app's private
preferences. Debug builds store and read back a probe secret at launch, and
the emulator check confirms it reached the preferences only as ciphertext.

`bash scripts/test-device-android.sh build` builds the device crates for
arm64-v8a and x86_64, the x86_64 test binaries and a debug APK;
`bash scripts/test-device-android.sh device`, with an emulator attached, runs
the object engine's conformance suite and the `platform-device` tests on it,
then installs and launches the app and records its size and cold start. The
`android` workflow runs both on an Ubuntu runner when the device code
changes.

## Phone sign-in

Phones sign in through the `device-browser` plugin (`browser/`):
`ASWebAuthenticationSession` on iOS and a Custom Tab on Android, redirecting
to `app.limiara.shell:/callback`, which the server must list in
`platform.devices.redirect_uris` (ADR 0029 §8). `src/phone.rs` maps the
plugin onto the controller's browser and redirect ports; its tests run on
the desktop. The emulator check confirms Android routes the redirect to the
plugin's redirect activity and that a stray redirect leaves the app running.
