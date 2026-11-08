# 0030: Content-free push notifications

## Status

Accepted (2026-10-20). Builds on ADR 0024 ("Continual sync") and ADR 0029
(device sign-in, which registers push tokens). Implemented in the
`platform-notify` crate and the worker's `device_notifications` dispatcher.

## Context

Phones suspend apps, so the server cannot hold a connection open to a
sleeping device (ADR 0024). A suspended app learns that its workspace
changed only through the platform's push service: Apple Push Notification
service (APNs) on iOS and macOS, Firebase Cloud Messaging (FCM) on Android.
Both services carry the message through the provider's own servers.

## Decision

### 1. One message, with no customer data

The only notification is "workspace changed". It carries the workspace's
id and nothing else: no record names, values, counts, object names or
member names. The app wakes, pulls the change stream and fetches records
through the record API, which applies sharing. Apple and Google therefore
learn which devices use which workspace and when it changes, and never its
content.

- **APNs:** a background push (`apns-push-type: background`, priority 5,
  `content-available`), collapsed per workspace so a newer notification
  replaces an undelivered one, and expiring after an hour.
- **FCM:** a high-priority data message with the same collapse key and
  time to live, and no notification text.

### 2. A kernel port with one adapter per provider

`platform-notify` defines `PushNotifier`, with an adapter for each
provider:
- `ApnsNotifier` sends over Apple's HTTP/2 API. Its ES256 provider token is
  signed with the deployment's `.p8` key and reused for 50 minutes, inside
  Apple's 20 to 60 minute window. A token Apple reports expired is signed
  again once.
- `FcmNotifier` sends over the FCM HTTP v1 API. Its access token comes from
  the deployment's service account: an RS256 assertion exchanged at the
  account's token endpoint (RFC 7523), reused until two minutes before it
  expires and renewed once if refused. RSA signing uses `ring`, which is
  constant-time, rather than the `rsa` crate (RUSTSEC-2023-0071).

Each result is one of:
- delivered;
- `TokenInvalid`: the device is gone, so the caller forgets its token;
- `Unavailable`: try again later;
- `Rejected`: the deployment's credentials or configuration need fixing;
- `NotConfigured`: no credentials for this provider.

Credentials belong to the deployment, not to plugins. Plugins never see
them, and no plugin can send a notification (ADR 0024 §6).

### 3. The worker dispatches, at most once every few minutes per device

The dispatcher runs in the worker:
- The bounded `platform_claim_device_notifications` routine, the only
  cross-tenant step, claims devices that:
  - have a push token;
  - are not revoked, and whose member still belongs to the workspace;
  - are in a workspace whose change log position or metadata version moved
    past what the device was last told;
  - were not claimed within `platform.devices.push.quiet_seconds` (180 by
    default).

  Claiming stamps the device, so workers running side by side skip each
  other's devices, and an unavailable provider is retried once per quiet
  window rather than on every pass.
- It sends one notification for each and, in the device's workspace under
  forced RLS, records the position it told the device about. It forgets a
  token the provider reports invalid, unless the app has registered a new
  one since.
- Registering a push token counts as being told about the workspace as it
  is, since the app has just synced.

A burst of changes therefore costs one notification per device, and the
app's own catch-up schedule (`BGAppRefreshTask`, WorkManager) covers any
notification the provider drops. A device's own pushes also move the
change log, so it may be woken once to pull what it sent; the pull is
small, and telling the device's edits apart from the same member's other
devices would need the log to record devices.

Provider keys live in files the worker reads at startup
(`platform.devices.push.apns.key_file`, `fcm.service_account_file`). A key
that cannot be read stops the worker, since retrying cannot fix it.

### 4. The app's side (2026-11-08)

`platform_device::app::Controller` keeps the platform's push token:
- `register_push(provider, token)` takes each token the platform hands
  the app (APNs on iOS, FCM on Android) and sends it with
  `PUT /devices/current/push`, using the device's own access token.
  Every time a workspace opens, after sign-in or at launch, it is sent
  again, so a device that signs in again, which is a new device to the
  server, keeps being woken. A failure leaves the app working; the next
  launch or token tries again.
- `wake(budget)` is what the app runs when a notification or the OS
  background scheduler wakes it: `SyncClient::wake` for the open workspace
  within the time the platform allows. It ends as `synced`,
  `out_of_time` (what was stored stays stored), `wiped` (the server asked
  for it), or `no_workspace`.

A wake can start while the foreground loop is still alive. On iOS a
suspended app resumes both. So `SyncClient` serialises its steps: a sync,
a push of queued changes, storing a streamed change, and the contact
renewal each hold one lock. The two runs therefore never push the same
queued change twice, and the wake's deadline cuts off only the wake's own
requests.

### 5. The phone's side (2026-11-08)

The shell's `device-push` plugin carries the platform half.

On iOS:
- **Callbacks:** when the plugin loads, it adds the three APNs callbacks to the app delegate's class with the Objective-C runtime: registration, a registration failure, and `didReceiveRemoteNotification:fetchCompletionHandler:`. Any callback the delegate already has still runs. It then registers for remote notifications. The app declares the `remote-notification` background mode (`Info.ios.plist`).
- **Events:** each callback becomes an event the shell reads on a thread of its own:
  - a device token goes to `register_push(apns, …)`;
  - a push runs `wake` with a 25-second budget, inside the roughly 30 seconds iOS allows;
  - the outcome completes the push for iOS: `new_data` after a sync, `no_data` with no workspace or after a wipe, `failed` when out of time or on an error. iOS schedules later wakes by those answers.
- **What a real token needs:** a build signed by an Apple team with the `aps-environment` entitlement, and the deployment's APNs key (§2). Without them, iOS refuses registration, and the app syncs when it is opened.
- **CI:** the iOS simulator job delivers a simulated push with `xcrun simctl push`. It checks in the system log that the push reached the plugin and completed through the controller's wake.

The plugin crate denies `unsafe` rather than forbidding it, as `device-browser` does (ADR 0029 §8). Tauri's `ios_plugin_binding!` declares the Swift entry point through FFI, and only that binding is allowed.

Android's half comes with a deployment's Firebase project: the FCM SDK, its `google-services.json` supplied at build time, and a messaging service that hands the token and wakes to the same controller calls.

## Alternatives considered

- **Notifications that name the changed records**, so the app can fetch
  only those. That puts customer data on Apple's and Google's servers. A
  pull after waking costs one request and keeps sharing on the server.
- **Visible notifications with text.** They need content, and a member's
  phone would show workspace data on the lock screen.
- **A third-party push service.** It adds another processor of device
  identifiers and is not needed: both providers have direct APIs.
- **The `a2` and `fcm` crates.** Each brings its own HTTP stack and token
  handling. The adapters here are small, use the platform's `reqwest`, and
  are tested against stand-in servers.

## Consequences

- A deployment that wants background sync needs an Apple developer key and
  a Firebase service account, configured outside any plugin.
- `reqwest` is built with HTTP/2, which APNs requires.
- Delivery is best-effort. A notification is a hint to sync, never the
  only path.
