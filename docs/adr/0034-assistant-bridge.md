# 0034: The assistant bridge

## Status

Proposed (2026-11-10). Builds on ADR 0024 (Limiara on the device: §3
sync, §4 access on the device, §5 the local command bridge, §6 local-only
capabilities, and "Encryption at rest"), ADR 0029 (device sign-in, the
offline lease and wipe), ADR 0030 (the `device-push` plugin pattern),
ADR 0032 (quick actions), ADR 0033 §4 (the Activity timeline) and
ADR 0017 ("Change source", the `assistant` kind). Decides issue #232, with
the assistant policy of #234 and the replica change feed and route context
of #235. Quick actions on device replicas (#236) are a prerequisite of the
bridge's quick-action operations. Nothing is implemented yet; the slices
are listed under Consequences.

## Context

Both phone platforms now let system assistants act inside apps:

- **iOS (Siri).** The iOS 27.3 developer preview (November 2026) extends
  App Intents. An app describes its records as `AppEntity` types, and
  `IndexedEntity` puts them in the semantic index, so Siri can find them by
  meaning. App Schemas map an app's intents onto shapes the assistant
  already understands, such as `.system.searchInApp`. On-screen awareness
  reads what the app is showing through `NSUserActivity` and its
  `appEntityIdentifier`. Entities that conform to `OwnershipProvidingEntity`
  get the system's confirmations, and app skills chain several actions.
  This runs only on devices with Apple Intelligence, and App Store
  submission for it opens in the spring.
- **Android 16 and later (AppFunctions).** An app declares functions with
  the Jetpack `@AppFunctionDeclaration` annotation. Their KDoc describes
  them to the agent, and agents holding `EXECUTE_APP_FUNCTIONS` discover
  and call them on the device, much like MCP tools. Gemini's access is an
  experimental preview with an early access programme.

The Limiara app is a Tauri shell over the on-device engine (ADR 0024 §5).
Native features are Tauri plugins with a Swift half and a Kotlin half, as
push notifications are (`apps/device-shell/src-tauri/push`, ADR 0030 §5).
Phone features are plugins with explicit runtime capabilities, never
ambient device access (ADR 0024 §6).

The two platforms differ in their details, but they ask the same of an app:
describe what it holds, find records, read them, say what is on screen, and
create or change records when the member asks. Workspaces define their own
objects at run time (ADR 0015), so none of this can be written into the
app as fixed types such as "Contact" or "Deal".

Three things constrain the design:

- **The engine already decides everything.** Sharing, permissions,
  validation rules, before-save automations, history and audit run on the
  device through `ObjectService`, behind the local command bridge
  (`platform_device::Bridge`, over `DeviceHost`). Swift and Kotlin must
  not re-implement any of it.
- **An assistant is a new way out of the app.** Data spoken aloud, shown
  in an assistant's answer or copied into the OS's index leaves the
  boundary the replica's encryption, lease and wipe protect (ADR 0024 §4,
  "Encryption at rest"). Administrators must decide what may leave.
- **Record content is untrusted.** An assistant may be driven by a
  language model, and a record's text can contain instructions aimed at
  it. The bridge cannot stop an assistant from reading text, but it can
  make sure no write happens without the member confirming the exact
  change.

## Decision

### 1. One domain-neutral bridge in the device crate

`platform-device` gains an **assistant bridge**: one typed Rust interface
that native assistant plugins call. It sits beside the local command
bridge and is built from the same `DeviceHost`:

- **It goes through the command bridge.** Each operation becomes one or
  more requests to `Bridge::handle`, through the same router, handlers
  and problem codes the web shell uses. The engine therefore applies every
  check it applies to the web shell: the member's permissions and sharing,
  field-level security once it exists (#316), validation rules, lookup
  filters, uniqueness, before-save automations, history and audit. The
  assistant bridge adds checks (the policy in §4, the confirmations in §5);
  it never skips one.
- **It runs as the signed-in member.** The caller is the member the device
  signed in as (ADR 0029 §7), with the source described in §6. There is no
  other identity: the assistant has no account, token or role of its own.
- **Native code holds no business logic.** A Swift or Kotlin plugin maps
  the platform's calls (an App Intent's `perform`, an AppFunction) onto
  bridge operations and the bridge's answers back onto the platform's
  types. It does not read the replica, evaluate policy, check permissions,
  format values or choose error messages.
- **It is not reachable from the web view.** No Tauri command exposes it,
  and the shell's scheme handler does not route to it. Only native code the
  shell links, holding the capability (§8), can call it. The web shell
  keeps its existing origin and content security policy, and no script in
  the origin can make a change that records the `assistant` source.
- **It is versioned.** Requests and answers are serialisable types under
  one contract version (`assistant.v1`), so the Swift and Kotlin halves can
  be built and tested against the same shapes, and later versions add
  operations without changing old ones.

### 2. Discovery comes from metadata

Nothing in the bridge or the native plugins names a business object.
`describe` returns, from the replica's definitions and the member's access:

- **Objects** the member can read and the policy exposes (§4): API name,
  labels, the name field, whether the member may create or edit records,
  and whether the policy lets the assistant write them.
- **Fields** the member can read: API name, label, type, whether it is
  required or computed, and picklist values with their labels. Fields the
  engine computes (formulas, roll-ups) are listed as read-only.
- **Quick actions** (ADR 0032) the policy offers, with their kind, label,
  target object and form fields, once device replicas carry them (#236).
- **Timelines:** whether the object's records have an Activity timeline
  (ADR 0033 §4), that is, whether any followed timeline source can point at
  them.

Every entry carries the replica's metadata version. Native plugins declare
a fixed, generic set of intents or functions that mirror the operations in
§3 (find records, open a record, create a record of an object, run an
action), and fill in the workspace's objects at run time: as dynamic
options and entity queries on iOS, and as parameters on Android. When the
metadata version moves, the plugin asks again. Labels are the workspace's
own, in the language the workspace defines them in.

### 3. A small operation set

| Operation | What it does |
| --- | --- |
| `describe` | Objects, fields, quick actions and timelines, as in §2. |
| `search` | Global search (ADR 0020) over the replica, across exposed objects or one, at most 20 results. |
| `get` | One record: its readable field values, each with a display value. |
| `list` | Records of one object, with up to 5 conditions on readable fields and one sort, at most 50 a page. |
| `timeline` | A record's Activity timeline (ADR 0033 §4), paged as the route pages it. |
| `actions` | The quick actions offered on an object or a record. |
| `run_action` | Runs a quick action, in two steps (§5). |
| `create` | Creates a record, in two steps (§5). |
| `update` | Changes chosen fields of a record, in two steps (§5). |

The rules for all of them:

- **Structured, not textual.** `list` takes typed conditions (equals, not
  equals, contains, before, after, between, is empty) that the bridge
  compiles into the query language (ADR 0019). An assistant never sends
  query text, so every field it names is checked against what is readable
  and exposed, and every query is bounded.
- **Display values are formatted by the bridge.** Dates, numbers,
  currencies, picklist labels and lookup names are formatted once, in Rust,
  so iOS and Android say the same thing. Raw values travel with them, typed.
- **Results are inert data.** Record content is returned as values,
  never as markup or instructions, and the bridge adds nothing to it. A
  native plugin hands it to the platform as data fields.
- **Not offered.** Delete, undelete, owner change, sharing, bulk changes,
  metadata changes and anything else the command bridge answers with
  `403 sync.server_only` (ADR 0024 §5). These are deliberate acts that
  belong in the app.
- **Reads leave no audit entry,** as reads through the web shell do not.
  Writes are audited by the engine as usual.

### 4. Administrators decide what assistants may reach (#234)

A workspace's **assistant policy** is object metadata. Holders of
`objects.manage` change it in the Object Manager. Like validation rules
and quick actions (ADR 0032 §5), it is versioned, audited and published
as `platform.objects.assistant-policy-updated.v1` in the same transaction.
It is kept in both stores with `tenant_id` under forced row-level
security. It moves the workspace metadata version, so it reaches replicas
with the object definitions (ADR 0024 §3 and §4).

- **Workspace switch:** assistant access on or off. It is off until an
  administrator turns it on. While it is off, the bridge answers every
  operation with `assistant.disabled`, and devices index nothing.
- **Per object,** each off until turned on:
  - **Exposed:** assistants may find and read its records, as the member.
  - **Writable:** assistants may create and update its records. It
    requires exposed.
  - **Indexable:** the device may put its records into the OS's semantic
    or search index (§7), with a list of **excluded fields** whose values
    never enter the index. It requires exposed. The name field may be
    excluded too, in which case entries carry only the object's label.
- **Per quick action:** offered to assistants or not, off until turned on.
  An action can be offered only when its object is exposed and the object
  it writes (the record's own object, or the child a create action makes)
  is writable.

The rules that hold whatever the settings:

- **It only narrows.** The policy is applied after the member's
  permissions and sharing, and only ever removes. A member who cannot read
  an object, a record or a field cannot reach it through an assistant,
  whatever the policy says. Nothing in it is a permission, and it grants
  nothing.
- **Plugin objects follow the workspace.** The policy of a
  plugin-declared object (ADR 0033 §2) is the workspace's, like its
  sharing model: a plugin cannot ship it turned on. A suspended plugin
  object is missing to every engine read, so it is missing here too.
- **The device enforces it, and the server re-checks writes.** The bridge
  checks the policy in the replica's definitions before every operation.
  A write made through the bridge is marked as an assistant's when it is
  pushed (§6), and the server checks it against the server's current
  policy, not the one the device had. A write the policy no longer allows
  is rejected with `assistant.not_allowed` and returns to the member like
  any write the server rejects (ADR 0024 §3).
- **Taking something out takes it off devices.** Turning the workspace
  switch off, turning an object's exposed or indexable setting off, adding
  an excluded field, or the object leaving the replica, makes devices
  delete or rewrite their OS index entries on the next sync, through the
  index stream (§7).

The member keeps a narrower choice on each device: the app's settings let
them turn assistant access and indexing off on that device. Indexing is off
on a device until the member turns it on there (§7).

### 5. Writes: two steps, confirmed by the member

Every write the bridge makes (`create`, `update`, `run_action`) has two
steps:

1. **Prepare.** The bridge checks the policy and runs the engine's checks
   without saving: permissions, sharing, required fields, validation rules
   and lookup filters, as a dry run. It answers with a **confirmation**: the
   object's label, the record's name (for an update), each field that would
   change with its label and new display value, and a single-use token.
   The token is bound to the exact change, the record's version and the
   metadata version, and it expires after two minutes.
2. **Commit.** The native plugin commits with the token, after the member
   has confirmed. The bridge refuses a token that is used, expired or for
   another change (`assistant.confirmation_invalid`), and one whose record
   or metadata has moved since it was prepared
   (`assistant.confirmation_stale`), so a member never confirms one change
   and gets another. The commit runs the full save path.

**The native side must always confirm before a write.** Every create,
update and quick action needs the member's explicit confirmation of the
prepared summary, through the platform's own confirmation (an App Intent's
`requestConfirmation`, the agent's confirmation on Android), every time.
There is no "don't ask again", no confirmation carried over from an
earlier step of a skill or a chained call, and no write the bridge treats
as too small to confirm. The bridge cannot see the member's screen, so
it enforces what it can, that no write commits without a prepared,
unchanged token, and the review of each native plugin checks the rest:
that every commit follows a confirmation shown to the member. This is the
defence against instructions hidden in record content: an assistant may
read anything the member may, but it cannot change anything the member
has not seen and accepted.

Reads need no confirmation from the bridge. The platform may still ask
the member to unlock the device, and native plugins must require it (§8).

Writes are local writes. They land in the replica and the push queue in
the same transaction, as any write through the command bridge does, and
sync sends them when it can (ADR 0024, "Continual sync"). They work
offline. The server's full save path runs again on push, and the result
comes back through the stream.

### 6. Provenance: `MutationSource::assistant`

Every write through the assistant bridge carries
`MutationSource::assistant(...)` (`platform_objects::source`), with the
assistant's name: `siri` or `gemini`, from the closed `Assistant` list.

- **Only this bridge sets it.** The bridge builds its own `DeviceHost`
  whose caller has the assistant source; the web view's host always
  records `device` (ADR 0017, "Change source", rule 2). No HTTP request,
  header or body names it.
- **The native plugin names the assistant, the bridge checks the name.**
  The plugin says which platform called it; the bridge accepts only names
  in the closed list, and only from the plugin built for that platform.
- **On push, it is a narrowing claim.** ADR 0017's rule 3 keeps
  device-side sources off the push, so nothing a device says widens what
  the server records. This ADR amends that rule for one case: a pushed
  operation may say it came from the assistant bridge, with the
  assistant's name and, for a quick action, the action's API name. The
  claim can only add checks (the server's policy re-check in §4), never
  remove one, so a device that leaves it out gains nothing it did not
  already have as the member. The server records the source as
  `assistant` with the assistant's name, and keeps the verified device id
  in the audit metadata beside it, so History shows "via Siri" and the
  audit trail still names the device. The server cannot verify which
  assistant a device names, so History shows it as the device reported it.
- **It is provenance.** It never changes what the member may do.

### 7. The OS index is outside the replica's protection

The OS's semantic and search indexes (the App Intents semantic index and
Core Spotlight on iOS, the system's search index on Android) live in the
operating system's storage, not in the replica's SQLCipher file. An entry
copied there is:

- **not encrypted with the workspace key,** only by the OS's own file
  protection, so "Encryption at rest" no longer covers it;
- **not removed by the replica's wipe,** which deletes the workspace file
  and destroys its key (ADR 0024, "Encryption at rest");
- **not hidden by the offline lease,** which locks the replica but not
  the OS (ADR 0029 §3);
- **readable by OS features** such as system search, suggestions and the
  assistant's own models, under the OS's rules rather than the
  workspace's sharing.

Therefore:

- **Indexing is opt-in three times:** the workspace switch, the object's
  indexable setting, and the member's choice on that device. Turning any of
  them off removes the entries it covered.
- **Only what is needed is indexed:** the record's id, its object's
  label, its name field and the fields the object's policy does not
  exclude, and nothing the member cannot read.
- **Entries are grouped by workspace.** Each entry's group (a domain
  identifier on iOS, a namespace on Android) is the workspace id, so one
  call removes a workspace's entries without reading them.
- **Sign-out and wipe clear the workspace's entries,** before the replica
  is deleted where possible. A lease that locks clears them too, and the
  next successful sync rebuilds them. At each launch, the shell tells the
  index plugins which workspaces it holds, and they delete the entries of
  any other workspace, so entries do not outlive an interrupted wipe or a
  crash.
- **The bridge drives it.** Native plugins do not decide what to index.
  The bridge turns the replica change feed (§9) into an **index stream**
  already filtered by the policy and the member's access: add or replace
  an entry (with the fields to index), remove an entry, remove an object's
  entries, remove the workspace's entries, and rebuild. A plugin applies
  these to the OS index and nothing else.

On-screen awareness follows the same rule. The current-route context (§9)
lets a native plugin tell the OS which record is on screen, as an id. The
plugin may describe the activity with the record's name only when the
object is indexable, and never marks it as eligible for public indexing.

### 8. When the replica cannot answer

The bridge answers from the replica only while the member could open it
in the app:

| State | Code | What the member hears |
| --- | --- | --- |
| Signed out, or never signed in | `assistant.signed_out` | Sign in to Limiara on this device first. |
| Wiped by the server (ADR 0029 §4) | `assistant.signed_out` | The same: the replica is gone. |
| Past the offline lease (`sync.lease_expired`) | `assistant.locked` | Open Limiara while online so it can check your access. |
| Key or storage unavailable (`storage.key_unavailable`) | `assistant.unavailable` | Limiara can't open this workspace right now. |
| The device is locked | `assistant.device_locked` | Unlock your device first. |
| Assistant access off, for the workspace or on this device | `assistant.disabled` | Assistant access is off for this workspace. |

- **Native plugins require an unlocked device** for every operation, with
  the platform's own setting (such as an App Intent's authentication
  policy). They also tell the bridge whether the device is unlocked, and
  the bridge refuses when it is not, so a mistake in one plugin fails
  closed.
- **A wipe or sign-out in progress wins.** A call that arrives while the
  replica is being deleted answers `assistant.signed_out`; it never sees a
  half-deleted replica or a fresh, empty one.
- **Queued writes stay queued** while the replica is locked, and are
  pushed after the next successful sync, as any queued write is.

### 9. Replica change feed and current-route context (#235)

Two shell services serve the assistant bridge and any other native plugin
that needs them, such as OS search or Handoff.

**The replica change feed.** A subscription in `platform-device` yields an
event after each sync apply and each local write commits. Events carry ids
only; a plugin that needs content reads it back through the engine as the
member (here, through the assistant bridge):

- `upserted (object, record)`: created, changed or undeleted.
- `removed (object, record, reason)`, with the reason:
  - `deleted`, locally or on the server, including a delete that won a
    conflict;
  - `access_lost`, from the revocation set (ADR 0024 §4).
- `object_removed (object, reason)`, for every record of an object at once:
  the object left the replica (deleted, or no longer readable by the
  member) or its plugin was suspended (ADR 0033 §2).
- `definitions_changed (metadata_version)`: fields, labels or the policy
  moved, so a plugin re-reads what it derives from them.
- `workspace_locked` and `workspace_unlocked`: the lease locked the replica,
  or the next sync reopened it.
- `workspace_cleared (reason)`: sign-out or wipe. It is the last event for
  that workspace.
- `rebuild`: the subscriber fell behind (the feed's buffer is bounded) or
  has just started, so it rebuilds from the records it can list.

Delivery is at least once while the app runs, in commit order. The feed
is not persisted; `rebuild` at launch and after a gap, with the launch-time
list of workspaces (§7), covers what happened while the app was not
running.

**The current-route context.** When the web shell navigates, it sends the
native side an inert message: `{route, object?, record_id?}`. `route` is
the name of the shell's route pattern (such as the record page), never the
address itself, so search terms and filters in the address stay in the web
view. `object` and `record_id` are ids, never record content.

- It is a same-origin request to the bridge,
  `POST /api/v1/workspaces/{id}/ui/route`, which the web shell sends only
  when the host's UI contributions say it accepts one. The server's
  contributions do not, so `apps/web` stays free of device-specific code.
  No script is added to the origin, and the content security policy is
  unchanged.
- The bridge checks the message's shape, that the object is in the
  replica and that the id is well formed, and passes it to subscribers.
  The assistant bridge passes it on only for exposed objects the member
  can read.
- On iOS, a native plugin uses it for `NSUserActivity` and its
  `appEntityIdentifier`, so "this record" resolves; on Android, for the
  agent's view of the current screen.

**Both are runtime capabilities** (ADR 0024 §6), declared by each native
plugin and granted by the shell when it builds the plugin, never ambient:
`device.replica_changes` for the feed, `device.route_context` for the route
context, and `device.assistant` for the assistant bridge itself, which
consumes the other two. The shell hands a plugin a handle only for what it
declared. Neither needs an OS permission; the assistant integrations that
do (none on iOS today, the agent's `EXECUTE_APP_FUNCTIONS` on Android,
held by the agent rather than the app) are the OS's business.

### 10. Errors are stable and safe to speak

Every bridge answer is a result or an error with a stable code and a
message:

- **Codes** are in the `assistant.*` namespace: those in §8, and
  `assistant.not_found`, `assistant.object_not_exposed`,
  `assistant.not_writable`, `assistant.action_not_offered`,
  `assistant.not_permitted`, `assistant.invalid`,
  `assistant.confirmation_invalid`, `assistant.confirmation_stale`,
  `assistant.not_allowed` (from the server, §4) and `assistant.failed`.
  The bridge maps the command bridge's problem codes onto them in one
  table, in Rust.
- **No existence leaks.** A record the member cannot read, and one that
  does not exist, both answer `assistant.not_found`, as the record routes
  answer `404`.
- **Messages are fixed sentences,** written for speech, filled only with
  metadata: object and field labels, and a validation rule's own message,
  which the member would see in the form. They never contain record
  values, ids, codes or engine detail. Native plugins speak or show the
  message and may branch on the code; they never compose their own.
- **`assistant.invalid` names the fields.** It lists the failing fields
  by label, with each field's message, so the assistant can ask for the
  missing value.

## Alternatives considered

- **Native code calls the server's HTTP API.** Swift and Kotlin would hold
  device tokens, fail offline, bypass the replica's lease and wipe, and
  each map metadata, values and errors on its own. The device already runs
  the same engine; going round it gains nothing.
- **Native code calls the local command bridge's routes directly.** It
  keeps the engine's checks, but the routes are the web shell's: wide,
  shaped for pages, recorded as `device`, with no policy and no
  confirmation step. Each platform would rebuild the same mapping. The
  assistant bridge is a narrow, typed layer over those routes instead.
- **Exposing the web API to the OS,** for example by generating intents or
  functions from the API description. It offers every route, including
  ones an assistant must not reach, and fixes types at build time while
  workspaces define objects at run time.
- **Per-platform logic.** Discovery, policy, formatting, error messages
  and index filtering written once in Swift and once in Kotlin would drift,
  need two security reviews and could not be tested without phones. In
  Rust they are written once and tested in CI (slice 4).
- **Generated entity types per object.** Neither platform lets an app add
  entity types at run time, and every new object would need a new build.
  Generic intents and functions filled from metadata follow workspaces as
  they change.
- **Indexing on by default.** It would copy records out of the encrypted
  replica on every device the moment a workspace turned assistant access
  on. Opt-in keeps that a deliberate choice by the administrator and the
  member.
- **Confirming only some writes.** Skipping confirmation for small changes
  or after a first approval is convenient, but the bridge cannot tell a
  member's request from an instruction hidden in a record. One rule for
  every write is simpler to review and harder to abuse.
- **Native plugins reading the replica file.** It would bypass sharing,
  the lease and the policy, and need the workspace key outside the engine.

## Consequences

Delivery is sliced:

1. **The policy metadata (#234).** The workspace switch and the per-object
   and per-quick-action settings, in both stores with forced RLS, the
   object routes and the Object Manager, versioned, audited and published,
   moving the metadata version and carried in replica definitions. The
   server's re-check of pushed assistant writes, with the push marker of
   §6, lands here, so it exists before any device sends one.
2. **The change feed and route context (#235).** The feed in
   `platform-device`, with device tests for every removal case in §9; the
   route message from the web shell through the bridge; the
   `device.replica_changes` and `device.route_context` capabilities; and a
   test plugin that receives route changes on desktop, iOS and Android.
3. **The bridge operations, with conformance tests (#232).** The
   operations, the policy checks, the two-step writes, the source and the
   push marker, the error table and the index stream, tested over the
   SQLite replica like the engine's conformance suites. Quick-action
   operations need #236.
4. **A reference test harness.** A headless driver that runs scripted
   assistant sessions against the bridge with no phone: describe, search,
   read, prepare and commit writes, every refusal state in §8, and the
   index stream through sign-out and wipe. CI runs it, and it is the
   conformance check that native assistant plugins are built against.

Also:

- **Native assistant plugins are separate plugins** built against this
  bridge, one per platform. The Swift and Kotlin integrations themselves,
  and any on-device model features, are outside this ADR.
- **Nothing changes for the web.** The web shell sends route messages
  only to a host that asks for them, and the server records no
  `assistant` source except from a device's push.
- **ADR 0017's rule 3 is amended** for the push marker of §6, which can
  only add checks.
- **A new copy of data exists** wherever indexing is turned on. Its
  removal depends on the index stream and the launch-time sweep, so both
  are tested for every removal case.
- **Assistant access depends on the platforms.** Siri's support runs only
  on Apple Intelligence devices and opens to the App Store in the spring;
  Gemini's is an experimental preview. The bridge does not depend on
  either: the harness exercises it whatever the platforms ship.
