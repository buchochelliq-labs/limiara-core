# Device plugins

Native plugins in the Limiara app (`apps/device-shell`) are Tauri plugins
with a Rust half and, on phones, a Swift half for iOS and a Kotlin half
for Android, as `device-push` is ([ADR 0030](../adr/0030-push-notifications.md) §5).
They get nothing ambient: each one declares the local-only capabilities it
uses, and the shell hands it a handle for exactly those
([ADR 0024](../adr/0024-limiara-on-device.md) §6). These capabilities are
not user permissions and grant nothing in the workspace. A plugin that
needs record content reads it through the engine as the signed-in member,
so sharing, permissions and the offline lease still decide.

## Local capabilities

| Capability | What it reaches |
| --- | --- |
| `device.replica_changes` | The replica change feed: what changed in the device's copy of the workspace, by id. |
| `device.route_context` | The current-route context: which object or record the web shell shows, by id. |

Declare them with `platform_device::capabilities::PluginDeclaration`, and
ask the controller for the grant when the shell builds the plugin:

```rust
let declaration = PluginDeclaration::new("device-route-probe", &["device.route_context"])?;
let grant = controller.grant(&declaration);
let routes = grant.route_context()?;      // declared: a subscription
let refused = grant.replica_changes();    // not declared: device.capability_not_granted
```

An unknown capability name refuses the whole declaration
(`device.capability_unknown`). Neither capability needs an OS permission,
and the web view reaches neither: no route of the local command bridge
serves the feed, and the route context only takes messages from the web
shell, never hands them back.

## The replica change feed

A `ChangeSubscription` yields `FeedEvent`s: a sequence number, the
workspace, and one change. Changes carry ids, never field values
([ADR 0034](../adr/0034-assistant-bridge.md) §9):

| Change | Meaning |
| --- | --- |
| `upserted (object, record)` | Created, changed or undeleted, on the device or on the server. |
| `removed (object, record, reason)` | `deleted`, on either side and including a delete that won a conflict, or `access_lost`, when sharing or permissions changed on the server. |
| `object_removed (object, reason)` | Every record of the object at once: `left_replica` (deleted, or no longer readable) or `plugin_suspended` for an object a plugin declared. |
| `definitions_changed (metadata_version)` | Fields, labels or other definitions moved; re-read what you derive from them. |
| `workspace_locked`, `workspace_unlocked` | The offline lease locked the replica, or a sync reopened it. |
| `workspace_cleared (reason)` | `signed_out` or `wiped`: the last event for the workspace, published before its files are deleted. |
| `rebuild (reason)` | `subscribed`, `opened` (a workspace opened) or `fell_behind`: rebuild from the records you can list. |

Delivery is at least once, in the order changes were committed, while the
app runs. The feed keeps the latest 1,024 events; a subscriber that falls
further behind gets `rebuild` and carries on from there. It is not
persisted, so a plugin rebuilds at launch. Treat events as prompts to read
again, not as counts: an `upserted` record you can no longer read is gone.

## The current-route context

A `RouteSubscription` holds the latest `CurrentRoute`: the workspace, a
route pattern (`home`, `object`, `new_record`, `record`, `search`,
`settings` or `other`), and the object and record id where the pattern has
them, or nothing when no workspace page is shown. It holds one value, so a
slow plugin sees where the shell is now rather than every step. The route
never steps back: the web shell sends one message at a time, and the bridge
keeps the latest one it received over an earlier one it is still checking.
When the shell shows no workspace page while the workspace stays open, such
as when every feature is disabled, the route is `other`.

The web shell sends it only to a host whose UI contributions carry
`route_context: true`, which the server's never do. It sends the pattern's
name, never the address, so search terms and filters stay in the page. The
bridge treats the message as untrusted data: it accepts only that shape,
an object the member reads in the replica and a record id of that object,
and passes it on as data.

## The route probe

`apps/device-shell/src-tauri/route-probe` is the reference: a debug-build
test plugin that declares `device.route_context` and records each route as
ids. Its Swift and Kotlin halves write a trail in the app's temporary
directory. The desktop tests drive a route through the `limiara://` scheme
to it, and the `ios` and `android` workflows ask a debug build for a
simulated route by creating `simulate-route` in that directory, then read
the trail from the simulator's container and the emulator.
