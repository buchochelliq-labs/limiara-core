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
| `device.assistant` | The assistant bridge: the `assistant.v1` operations and the index stream, for a system assistant. |

Declare them with `platform_device::capabilities::PluginDeclaration`, and
ask the controller for the grant when the shell builds the plugin:

```rust
let declaration = PluginDeclaration::new("device-route-probe", &["device.route_context"])?;
let grant = controller.grant(&declaration);
let routes = grant.route_context()?;      // declared: a subscription
let refused = grant.replica_changes();    // not declared: device.capability_not_granted
```

An unknown capability name refuses the whole declaration
(`device.capability_unknown`). None of them needs an OS permission, and
the web view reaches none of them: no route of the local command bridge
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

## The assistant bridge

A plugin that connects a system assistant (Siri's App Intents on iOS,
AppFunctions on Android) declares `device.assistant` and gets an
`AssistantBridge` from `grant.assistant()`
([ADR 0034](../adr/0034-assistant-bridge.md)). No route of the local
command bridge and no Tauri command serves it, so nothing in the web view
can reach it. The bridge holds all of the logic; the plugin's Swift or
Kotlin half maps the platform's calls onto bridge requests and the answers
back onto the platform's types, and never reads the replica, checks
permissions, formats values or writes its own messages.

### Requests and answers

Every request is an `AssistantRequest` under the `assistant.v1` contract,
as Rust types or as JSON through `call_json`:

```json
{"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
 "operation": {"name": "get", "object": "custom:account", "record": "a01-0000000001"}}
```

- `assistant` is `siri` or `gemini`. The shell builds the bridge for its
  platform's assistant, and a request naming another is refused.
- `device_unlocked` is what the platform says. Require an unlocked device
  with the platform's own setting as well; the bridge refuses a locked one
  (`assistant.device_locked`), so a mistake fails closed.

Every call runs as the signed-in member through the command bridge, so
permissions, sharing, field-level security, validation rules and lookup
filters apply as they do in the app. Before each one the bridge checks the
workspace's assistant policy in the replica's definitions, which only
narrows what the member could reach. Answers are inert data: values with
display text the bridge formatted, never markup.

| Operation | What it does |
| --- | --- |
| `describe` | The exposed objects with the fields the member reads, the offered quick actions and whether records have a timeline, with the metadata version. Ask again when it moves. |
| `search` | Search over the exposed objects, or one (`object`), at most 20 results. |
| `get` | One record's readable values, each with a display value. |
| `list` | Records of one object, with at most 5 typed `conditions` and one `sort`, 50 a page with a `cursor`. |
| `timeline` | A record's Activity timeline, paged with a `cursor`. |
| `actions` | The quick actions offered on an object or a record. |
| `create`, `update`, `run_action` | Prepare a change and answer with a confirmation. |
| `commit` | Makes a prepared change, with the confirmation's token only. |
| `on_screen` | The record the app shows, by id, when its object is exposed. |

A `list` condition is `{"test": ..., "field": ..., "value": ...}` with the
test `equals`, `not_equals`, `contains`, `before`, `after`, `between` (with
`from` and `to`) or `is_empty`. The bridge compiles conditions into the
record query language; never send query text. Delete, owner changes,
sharing, bulk changes and metadata changes are not offered.

### Writes: prepare, confirm, commit

`create`, `update` and `run_action` run the save path as a dry run and
answer with a `Confirmation`: the object's label, the record's name for an
update or a quick action, each field that would change with its label and
display value, and a `token`. Show it to the member with the platform's own
confirmation, every time, then send `commit` with the token. There is no
way to skip the confirmation, and nothing writes without a token.

A token is single use, lives two minutes and is bound to that exact change,
the record's version and the metadata version. `commit` refuses
`assistant.confirmation_invalid` (used, expired or unknown) and
`assistant.confirmation_stale` (the record or the definitions moved): ask
the member again. Writes record the assistant as the source, and the
device's push marks them so the server checks them against its current
policy; a write it no longer allows comes back as `assistant.not_allowed`,
like any write the server refuses.

### Errors

An error answer has a `code`, a `message` to speak or show as it is, and
for `assistant.invalid` the failing `fields` with their labels and
messages. Branch on the code; never compose a message.

| Code | When |
| --- | --- |
| `assistant.signed_out` | No workspace on the device, or a sign-out or wipe in progress. |
| `assistant.locked` | Past the offline lease. |
| `assistant.unavailable` | The key or storage cannot be opened. |
| `assistant.device_locked` | The device is locked. |
| `assistant.disabled` | Assistant access is off for the workspace or on this device. |
| `assistant.not_found` | No such record, or one the member cannot read: the same answer. |
| `assistant.object_not_exposed` | The policy does not expose the object, or the member cannot read it. |
| `assistant.not_writable` | The policy does not let assistants write the object. |
| `assistant.action_not_offered` | The policy does not offer the quick action. |
| `assistant.not_permitted` | The member may not make this change. |
| `assistant.invalid` | The request or a value is not valid. |
| `assistant.confirmation_invalid`, `assistant.confirmation_stale` | See above. |
| `assistant.not_allowed` | The server's policy refused a pushed write. |
| `assistant.failed` | Anything else. |

### The member's settings on the device

`DeviceAssistantSettings` holds the member's choice on the device, kept
with the stored session: `assistant_access` (on by default; off answers
`assistant.disabled`) and `indexing` (off until the member turns it on).
The shell changes them with `Controller::set_assistant_settings`.

### The index stream

A plugin that keeps the OS's semantic or search index takes
`bridge.index_stream()` and applies its updates, and nothing else, to the
index:

| Update | Apply it by |
| --- | --- |
| `upsert (entry)` | Adding the entry, or replacing the one for the same record. |
| `remove (workspace, object, record)` | Removing one entry. |
| `remove_object (workspace, object)` | Removing the object's entries. |
| `remove_workspace (workspace)` | Removing the workspace's entries. |
| `rebuild (workspace)` | Removing the workspace's entries; the `upsert` updates that follow are what it holds. |
| `refresh (workspace, expires_at)` | Moving every entry's expiry of the workspace. |

Entries are already filtered: indexing needs the workspace switch, the
object's indexable setting and the member's `indexing` setting, and holds
only the record's id, its object's label, its name unless the policy
excludes it, and the readable fields the policy does not exclude. Group
entries by `workspace` (a domain identifier on iOS, a namespace on
Android) and set each entry's expiry to `expires_at`, the offline lease
length from when it was written; `refresh` moves it after each sync. A
lease that locks keeps the index. Sign-out and wipe remove the workspace's
entries before its replica goes.

At each launch, after the shell opened what it holds, call
`bridge.sweep(&indexed)` with the workspaces the index holds entries for.
Remove every workspace it lists, and every entry that expired before
`remove_expired_before`.

On-screen awareness uses `on_screen` and ids only. Describe the activity
with the record's name only when the object is indexable, and never mark
it as eligible for public indexing.

## The route probe

`apps/device-shell/src-tauri/route-probe` is the reference: a debug-build
test plugin that declares `device.route_context` and records each route as
ids. Its Swift and Kotlin halves write a trail in the app's temporary
directory. The desktop tests drive a route through the `limiara://` scheme
to it, and the `ios` and `android` workflows ask a debug build for a
simulated route by creating `simulate-route` in that directory, then read
the trail from the simulator's container and the emulator.
