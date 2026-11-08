# 0029: Device sign-in and revocation

## Status

Accepted (2026-10-20). Builds on ADR 0024 (Limiara on the device, §4 and
slice 5), ADR 0023 §3 (API clients) and ADR 0003 (tenancy under forced
RLS).
Implemented in the API's `devices` module and token endpoint,
`platform_db::devices`, and PostgreSQL migration 0033.

## Context

A phone or desktop app holds a replica of one workspace for one member
(ADR 0024). It needs to:
- sign the member in, with no password and no browser cookie, since the
  app is not the browser;
- call the object engine routes for a long time between sign-ins;
- learn that it must delete the replica when an administrator revokes it
  or the member leaves.

The web already signs people in with the OIDC authorization code flow and
PKCE, run by the server with a backend-managed session. API clients
(ADR 0023 §3) already call the engine routes with bearer tokens stored only
as hashes.

## Decision

### 1. Sign-in goes through the system browser and the server's OIDC flow

The app follows OAuth for native apps (RFC 8252):
1. It creates its own PKCE verifier and opens the system browser at
   `GET /auth/device/login` with its `redirect_uri`, the S256
   `code_challenge` and a `state`.
2. The server checks the redirect URI against
   `platform.devices.redirect_uris`, matched exactly, then runs the same
   authorization code flow with PKCE against the identity provider as the
   web. The device's redirect URI, challenge and state are kept with the
   OIDC transaction. The identity provider sees one client, the server.
3. On the callback, the server links the identity as it does for the web.
   Instead of a session, it stores a one-time code for the person, valid
   for two minutes, and redirects to the app's redirect URI with `code`
   and `state`.
4. The app sends `POST /oauth/token` with `grant_type=authorization_code`,
   the code, its verifier, the redirect URI, the workspace, a device name
   and its platform. The server checks the verifier against the stored
   challenge and the person's membership of the workspace, then registers
   the device and returns its tokens. The code is used up by any attempt,
   right or wrong.

Redirect URIs are private-use schemes in reverse-domain form, such as
`com.example.app:/oauth`, or plain HTTP to a loopback address. Web schemes
to other hosts are refused at startup, since any site could receive the
code. An empty list turns device sign-in off.

There is no password, the app never sees the identity provider's tokens,
and the browser session at the identity provider is the only single
sign-on state.

### 2. A device belongs to one member in one workspace

A device row carries `tenant_id` and is under forced RLS, like everything
tenant-scoped. An app that syncs two workspaces signs in to each and holds
two devices. A member has at most 10 active devices per workspace.

### 3. Tokens are short access tokens and rotating refresh tokens

- An access token lasts an hour and reaches the same routes as an API
  client token, plus the calling device's own routes
  (`/devices/current…`). It acts with exactly its member's current access.
- A refresh token lasts 30 days unused, and each use replaces it.
  Presenting a used refresh token again means it was copied, so the device
  is revoked and told to wipe (RFC 9700 §4.14.2).
- Codes and tokens are random and stored only as SHA-256 hashes. Token
  issue for refresh runs in a bounded `SECURITY DEFINER` routine, because
  the token endpoint knows no workspace yet, as it does for API clients.
- The token response also returns `offline_limit_seconds`
  (`platform.devices.offline_limit_seconds`, 14 days by default): after
  that long without reaching the server, the app locks its replica
  (ADR 0024 §4). The app passes the value to the replica's engine, which
  enforces it and unlocks at the next sync that reaches the server.

### 4. Revocation tells the device to wipe

A member revokes their own devices; `devices.manage` revokes anyone's. A
device can sign itself out with `DELETE /devices/current`. Revoking
deletes the device's push token and keeps its unexpired access and refresh
tokens, which now authorise nothing. When a revoked device, or one whose
member left the workspace, calls with an access token, the server answers
`401 auth.device_wipe_required` instead of a plain `401`. A refresh with
such a device answers `invalid_grant` with `"wipe": true`, so a device
that refreshes before its next call learns it too. The app then
deletes the workspace's replica: signing in again does not keep it.

A device that never calls again is covered by the offline limit.

### 5. Push notifications are registered, not yet sent

A device registers where its notifications go with
`PUT /devices/current/push` (`apns` or `fcm` and the provider's token).
The token is never shown back. Sending the content-free "workspace changed"
notifications is a later slice (ADR 0024, "Continual sync").

### 6. Evidence

Registering and revoking a device are audited in its workspace with the
reason (`revoked`, `signed_out`, `refresh_reused`), never with tokens.

### 7. The app's side (2026-11-03)

The sign-in lifecycle is the same on every platform, in
`platform_device::app::Controller`, over `platform_sync::auth`:
1. The member enters the workspace address the web shows in Settings →
   Devices (`https://server/workspaces/{id}`), since the code is redeemed
   for one workspace and `/workspaces` belongs to browser sessions.
2. The controller opens the system browser at `GET /auth/device/login` and
   waits for the redirect: on desktop, a loopback listener on
   `http://127.0.0.1:47185/callback`, which desktop builds register; on
   phones, the app's private-use scheme. It redeems the code with its
   verifier.
3. `GET /devices/current`, with the device's own token, returns the device,
   the member (name, permissions, role names and ids) and the workspace.
   The replica's engine is built for that member, the offline limit from
   the token response is passed to it, and the first sync runs before the
   workspace opens. Signing in always starts from an empty replica.
4. The refresh token and the member's profile are kept in the platform
   keychain, never in the app's directory, with each workspace file's
   database key. At launch the replica opens from them before any
   network. One refresh runs at a time, since a refresh token sent twice
   revokes the device; every rotated token is stored at once.
5. Sign-out calls `DELETE /devices/current` and deletes the replica, its
   key and the stored session even when the server cannot be reached. A
   refused refresh, or `auth.device_wipe_required` from the sync, deletes
   them too.

On Linux the desktop keychain is the kernel keyring, which forgets the
session when the member logs out; the next launch signs in again.

### 8. Phone sign-in (2026-11-08)

Phones redirect to the app's private-use scheme, its application id
(RFC 8252 §7.1): `app.limiara.shell:/callback`, which deployments that
serve the phone apps add to `platform.devices.redirect_uris` next to the
desktop loopback URI. The shell's `device-browser` plugin opens the
system browser, never an embedded web view:
- iOS uses `ASWebAuthenticationSession`, which shares the system
  browser's sign-in at the identity provider and hands back the redirect
  to the scheme it was started with.
- Android opens a Custom Tab, or the default browser where none offers
  Custom Tabs. The redirect opens the plugin's redirect activity,
  registered for the application id's scheme, which hands it to the
  waiting sign-in and returns to the app. Returning to the app with no
  redirect cancels the sign-in; a redirect no sign-in waits for is
  dropped.

Another app could register the same scheme on Android, which is why the
code is bound to the sign-in: the redirect is accepted only with the
sign-in's `state`, and the code is redeemed only with its PKCE verifier,
which never leaves the app.

The shell maps the plugin onto the controller's `Browser` and
`Redirects` ports (`apps/device-shell/src-tauri/src/phone.rs`), so the
controller is the same on every platform.

The plugin crate denies `unsafe` rather than forbidding it. Tauri's iOS
binding, `ios_plugin_binding!`, declares the Swift plugin's entry point
through FFI as an `unsafe fn`; that one generated binding is allowed, in
its own module, and nothing else in the crate may use `unsafe`.

## Alternatives considered

- **The app talks to the identity provider directly**, as a public client
  with its own PKCE, and the server validates the identity provider's
  tokens. It ties every deployment's identity provider to the app's
  redirect URIs and puts identity provider tokens on the device. Running
  the flow on the server keeps one identity provider client and the same
  linking code as the web.
- **A browser session cookie inside the app.** Embedded web views are
  discouraged by RFC 8252, share no sign-in with the system browser, and
  would put a session cookie where the app's own code can read it.
- **One device token for every workspace of a person.** It needs an
  authority outside any workspace, which RLS cannot protect, and makes
  per-workspace revocation by an administrator harder.
- **Deleting tokens on revocation.** The device would only see a plain
  `401` or `invalid_grant`, which it must treat as "sign in again", so the
  replica would outlive the revocation.

## Consequences

- Device sign-in needs a registered redirect URI per app build, set by the
  deployment: the desktop loopback URI and, for the phone apps,
  `app.limiara.shell:/callback`.
- Old app versions depend on the token endpoint and the wipe signals, so
  they must evolve compatibly.
- Building it surfaced a defect in the shared callback: the identity lock
  key joined issuer and subject with a NUL byte, which PostgreSQL text
  rejects, so no real sign-in completed. It is fixed separately (#369),
  with a real-PostgreSQL test.

## Follow-up

- Sending push notifications through APNs and FCM, with no customer data
  (ADR 0024).
- A device list and revoke action in the web app's settings. (Built:
  Settings → Devices.)
- An end-to-end check of the browser leg against the Keycloak fixture.
  (Built: `scripts/test-device-sign-in.sh` signs fixture people in through
  Chromium on a loopback redirect, redeems the code, syncs, rotates the
  refresh token, and checks a reused code, a wrong verifier, a person
  outside the workspace and an unregistered redirect URI.)
- Loopback redirect URIs with any port (RFC 8252 §7.3), if a desktop app
  needs them.
