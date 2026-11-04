# Device shell

The Limiara app for desktop and, next, iOS and Android (ADR 0024 §5): the
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

## Status

Until device sign-in lands (#385), the app opens a demonstration workspace:
an encrypted replica with one object and three records, keyed in memory, so
it starts afresh at each launch. The Keychain and Keystore adapters come with
the iOS and Android builds (#386, #387).

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
