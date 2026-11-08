# 0024: Limiara on the phone

## Status

Accepted as direction (2026-10-12). Builds on ADR 0001 (modular
monolith), ADR 0006 (open core), ADR 0007 (control and data plane),
ADR 0013 (swappable storage), ADR 0015 (object engine), ADR 0016
(sharing), ADR 0018 (change data capture), ADR 0022 (automation) and
ADR 0023 (integration APIs). The work is tracked as an epic with one
issue per slice in §8.

## Context

Limiara aims to be an open-core CRM that people can carry: "Salesforce in
your pocket". Salesforce's mobile app is a thin client. It caches some
records, but the platform (objects, formulas, validation, automation,
sharing) lives on the server, and offline work is limited and brittle.

Limiara can do better because of choices it has already made:
- The object engine is a Rust library (`platform-objects`), not a
  server. Metadata, records, formulas, validation rules, automations,
  sharing, history and the change log all run through `ObjectService`
  over a store trait.
- That trait has a SQLite adapter that passes the same conformance suites
  as PostgreSQL, with one database file per workspace (ADR 0013).
- Every change has a version and lands in an ordered change log with
  positions (ADR 0018). Composite and bulk APIs apply changes with
  expected versions (ADR 0023), and API clients and OIDC give machines and
  people a way in without passwords.

So the engine can run on the phone itself, over an embedded SQLite file,
rather than a client imitating it.

## Decision

### 1. The engine runs on the device

The mobile app embeds the same `platform-objects` engine as the server,
compiled for iOS and Android, over the SQLite adapter. Formulas,
validation rules, before-save automations, sharing checks, history and the
change log behave on the phone exactly as on the server, because they are
the same code and pass the same conformance suites. No business rule is
re-implemented in the app.

Domain-neutrality holds: the device carries the kernel engine and
first-party plugins that declare the `sqlite` backend (ADR 0013 §5). It
does not carry plugins that need PostgreSQL.

### 2. Two modes: standalone and synced

- **Standalone.** One person, one or more local workspaces, no server.
  This is a complete personal CRM:
  - objects, fields and records, defined on the phone;
  - formulas, validation, automations, search and history.
  It is part of the community edition and needs no account, network or
  Limiara-hosted service (ADR 0006, ADR 0007).
- **Synced.** The device holds a replica of a server workspace for one
  member. The server stays the authority:
  - for metadata, sharing, after-save automations and every other member's
    changes;
  - the device works offline and syncs when it can.
  Any Limiara server can be the other end, self-hosted or hosted.

A standalone workspace can later be *published* to a server, becoming a
synced one, because both share the same metadata and record model.

### 3. Sync is built from the change log and versions

- **Pull.**
  - The device reads the server's change log for the workspace from its
    last position (ADR 0018) and fetches the records and metadata that
    changed.
  - If the position has expired (beyond the change log's retention), it
    resynchronises with `touched_after` (#287), as integrations do.
- **Metadata has its own feed.** The record change log (ADR 0018) carries
  record changes only; object, field, validation-rule and automation
  changes leave events but no log entries. The server therefore keeps a
  workspace metadata version that every metadata change increments. Each
  pull returns it, and when it moved the device fetches the current
  definitions before applying record changes, so it never validates
  against a stale schema. Offline edits made under the old metadata are
  revalidated by the server on push, as any push is.
- **Push.**
  - Local changes are queued in order with the version each was based on.
  - They are sent as composite requests (ADR 0023 §1), so related changes
    land together and keep their references.
  - The server runs its full save path: validation, sharing, automations,
    evidence. The result, including any automation changes, comes back
    through the next pull.
- **Conflicts.**
  - A version conflict never overwrites silently. The member sees both
    versions, field by field, and chooses.
  - Fields only one side changed merge automatically.
  - Deletes win over edits only with confirmation.
- **Rules on both sides.** Validation rules and before-save automations
  run on the device for immediate feedback and again on the server, which
  decides. A change the server rejects returns to the member with the
  server's message, like a conflict.
- **Server-only work.** After-save automations, background and scheduled
  jobs, workspace events and integrations run on the server in synced
  mode, so they run once. In standalone mode they run on the device, and
  background jobs run while the app is active.

### 4. Only what the member may see reaches the device

- Sync is per member and filtered by the server's sharing (ADR 0016).
  The device receives only records the member can read, and only the
  fields they may read once field-level security exists (#316).
- Losing access needs its own channel, because the change log cannot
  carry it: ADR 0018 leaves sharing changes out of the log, the log omits
  records a member can no longer read, and a removed member cannot pull at
  all. So:
  - each pull also returns a per-device revocation set: the ids of
    replicated records the member can no longer read, computed by the
    server against what the device holds (its replica manifest), and the
    device deletes them;
  - a removed member's or revoked device's next contact, refused by the
    server, returns a wipe instruction rather than a plain `401`, and the
    device deletes the workspace replica;
  - a device that cannot reach the server holds a lease: past a
    workspace-set offline limit it locks the replica until the next
    successful pull, so data does not outlive revocation indefinitely on
    a device that never reconnects.
- Local data is encrypted at rest: SQLite with a key held in the OS
  keychain or keystore.
- An administrator can remotely revoke a device. The device is wiped on
  its next contact, and its tokens are revoked immediately.
- Sign-in uses OIDC authorization code with PKCE through the system
  browser (RFC 8252), as the web does. There are no passwords. The device
  holds a refresh token scoped to the object engine routes, like an API
  client (ADR 0023 §3), not a browser session.

### 5. One interface for web and phone

The React app shell is packaged for iOS and Android with Tauri 2, whose
Rust core links the engine directly:
- the shell talks to a local command bridge that implements the same
  routes and problem codes as the HTTP API;
- the same pages, Object Manager, record pages and search work on the
  phone;
- navigation still comes from the UI contribution endpoint, never
  hard-coded.

The phone layouts are the responsive layouts the UX work already defines
(narrow screens, touch targets, the search bar on small screens), not a
second design. No third-party JavaScript runs in the app's origin.

Built (2026-11-02): the bridge serves the server's own handlers, not a copy
of them.
- The object routes live in `platform-objects-http`, generic over an
  `ObjectsHost`. A host supplies the engine for a verified `Caller`, and
  the member directory that names users, roles and share grantees. The
  workspace API is one host, over PostgreSQL; it inserts the `Caller`
  from its workspace boundary. Its routes, JSON and problem codes did not
  change, and its acceptance suite proves it.
- `platform-device` is the other host, over the replica. `Bridge::handle`
  takes an `http::Request` and answers it in process through the same
  router, so the native shell can hand it requests from a custom URI
  scheme with no port and no network.
- The bridge adds the three routes the shell needs first: `/api/v1/me`,
  `/api/v1/workspaces` (the one synced workspace) and the workspace's
  `ui/contributions`, which carries the engine's navigation and
  permissions. Plugin pages are server-delivered and are absent offline.
- Every request runs as the signed-in member. Another workspace answers
  `404 workspace.not_found`, as the server does for a workspace the caller
  is not a member of. A locked replica answers `409 sync.lease_expired`.
  The change stream answers `404`, because the device follows the server's
  stream with the sync client.
- Only the writes the sync loop takes to the server run on the device:
  record changes (create, update, delete, undelete, owner change), which
  the push queue sends as composite operations. Definitions, validation
  rules, automations, object permissions, sharing, saved reports and bulk
  jobs are made on the server; on the device they would never reach it.
  The bridge answers `403 sync.server_only` for them and leaves the manage
  permissions out of its contributions, so the shell hides those actions.
- Member and role names come from the offline directory (below). Before
  the first sync, only the member's own name is known.
- Still to come: the Tauri project with its URI-scheme handler,
  keychain-backed encrypted storage, the device's sign-in screens, and
  the iOS and Android builds.

Built (2026-11-03): the desktop shell (`apps/device-shell`).
- Tauri 2 serves the web build and the bridge from one `limiara://`
  origin: `/api/...` goes to `Bridge::handle`, and every other path is the
  embedded web build, with `index.html` for the shell's own routes. The
  web shell's relative `/api/v1` requests reach the bridge unchanged, so
  `apps/web` has no device-specific code.
- Every response carries a content security policy that allows only the
  shell's own origin (`default-src 'self'`, no framing, no plugins). No
  script from elsewhere runs in the origin.
- The shell is its own Cargo workspace, because it links the platform's
  GUI libraries (WebKitGTK on Linux), which the server workspace must not
  need.
- Until device sign-in (#385), it opens a demonstration workspace: an
  encrypted replica with sample records, keyed in memory and recreated
  at each launch.
- Verified on Linux under Xvfb: navigation from the bridge's
  contributions, an object list, a record page, and an edit saved through
  a `PATCH` with a request body. The iOS and Android builds (#386, #387)
  reuse the same handler; on those platforms, as on Windows, the start URL
  is `http://limiara.localhost/`.

### 6. Local-only capabilities are plugins

Phone features are first-party plugins with explicit runtime capabilities,
never ambient device access:
- camera and attachments;
- location on a record;
- calling and messaging from phone fields;
- contacts import;
- push notifications.

Each asks the OS permission only when used.

### 7. Edition boundary

The app, standalone mode, sync with any Limiara server and export of a
local workspace are all in this repository. Entitlements, where a hosted
offering uses them, are checked by `EntitlementService` and are never
authorization decisions (ADR 0006).

### 8. Slices

1. **Engine on device:** build `platform-objects` and the SQLite adapter
   for iOS and Android. Run the conformance suites on device simulators in
   CI.
2. **Standalone app:** the Tauri shell with the local command bridge.
   Objects, records, search, formulas, validation and automations on the
   phone. Encrypted storage.
3. **Sync, pull:** per-member replica from the change log, with sharing
   filtering and access-loss deletion.
4. **Sync, push:** a queue of local changes sent as composite requests,
   with conflict resolution.
5. **Device identity:** OIDC with PKCE, device tokens, revocation and wipe.
6. **Phone capabilities:** camera and attachments (with the object-store
   file port, #288), location, calling, notifications.
7. **Publish:** turn a standalone workspace into a synced one.

## Backup to the member's own cloud (2026-10-19)

A standalone workspace exists only on the phone until it is published
(§2), so losing the phone must not lose the workspace. The device keeps a
continuous, encrypted copy in the member's own cloud storage. This is not
sync and has no authority: it holds a copy of one person's data and is
read only to restore it.

**What is not used.**
- The live SQLite file is never kept in a syncing folder such as iCloud
  Drive or OneDrive. Those services copy whole files in their own order,
  so a database and its write-ahead log can be uploaded out of step and
  merged into a "conflicted copy". Either corrupts the workspace.
- Litestream does not run on the phone. It streams SQLite's write-ahead
  log from a separate process to S3-compatible storage, Azure, Google
  Cloud Storage, SFTP or a folder. iOS gives an app no background process,
  iCloud has no S3 or server API, and streaming into an iCloud Drive
  folder on a Mac has the same out-of-order and offloading risks as the
  live file. Litestream is the backup for self-hosted SQLite servers
  instead (ADR 0013, "supported free backends").

**What is used.** The same approach as Litestream, a snapshot plus every
change after it, applied to workspace changes rather than database pages:
1. **Snapshot.** Weekly, and after a large import or a restore, the device
   takes a consistent copy with SQLite's backup API and encrypts it. It
   uploads the copy and keeps the latest two.
2. **Change journal.** Every local write also enters a backup journal in
   the same transaction. The journal is separate from the push queue,
   because the push queue sends only record operations through the
   composite API (ADR 0023) and metadata changes stay out of the record
   change log. The journal holds three kinds of entry:
   - record changes;
   - metadata changes: objects, fields, permissions, validation rules and
     automations, as the definition each change produced;
   - file changes from the file port (ADR 0013): a file added, by content
     hash, or a file removed.
   The device sends each entry, encrypted, to the member's cloud as soon as
   the app is online. When the app is suspended, it catches up the next
   time the OS wakes it.
3. **Files.** Each file is uploaded once as its own encrypted object, named
   by its content hash. A snapshot lists the hashes it needs, so a file is
   deleted from the cloud only when no kept snapshot or later journal
   entry refers to it.
4. **Restore.** A new device downloads the newest snapshot, decrypts it,
   fetches the files it lists, then replays the journal after it in order:
   metadata, records and files, as they happened. A metadata change
   therefore lands before the records that depend on it. Each entry
   carries the version it produced, so an entry that is already applied is
   skipped.

**Where it goes.**
- On Apple devices, the CloudKit private database. Changes are separate
  records, not files, so there is no file merge, and storage counts
  against the member's own iCloud allowance.
- Elsewhere, an app folder in OneDrive (Microsoft Graph), Google Drive or
  Dropbox, holding journal bundles, files and snapshots.
- Each destination is an adapter behind one backup port, the same way files
  sit behind the file port (#288).

**Security.**
- Everything is encrypted on the device before upload. The provider sees
  ciphertext and object sizes, never records.
- The key lives in the keychain or keystore. iCloud Keychain can carry it
  to the member's other Apple devices. Otherwise the member keeps a
  recovery key, shown once when backup is turned on. Without it, a restore
  on a non-Apple device is impossible, which is the point.
- Provider sign-in is OAuth with PKCE. Tokens stay in the keychain and are
  limited to the app's own container or folder, never the whole drive.
- Backup is a first-party plugin with an explicit capability (§6). It is
  off until the member turns it on.

**Synced workspaces.** The server is the record of a synced workspace, so
the device does not back it up. It backs up only standalone workspaces and
any changes still waiting to be pushed.

**Edition.** Backup to the member's own account is part of the app (§7).

**Also possible later.** The same journal, read by the member's
other devices, gives one person standalone sync between their own phone,
tablet and Mac, with no Limiara server. It is one-person only: a shared
workspace still needs a server to enforce sharing and validation.

## Measured footprint (2026-10-13)

The first measurements come from a host build (x86-64 Linux), with no
phone build yet:
- **Engine:** `platform-objects` and the SQLite store, built for size
  (`opt-level = "z"`, LTO, `panic = "abort"`, stripped), come to 3.6 MB
  (1.6 MB gzipped). That build still links the PostgreSQL driver.
- **Web shell:** the bundle is 660 KB, about 150 KB of gzipped JavaScript.
  Tauri uses the system web view, so the whole app should install in the
  low tens of megabytes. That is an estimate until slice 1 measures it.
- **Records:** 10,000 contacts with five fields each fill about 50 MB of
  SQLite, about 5 KB per record:
  - the records table itself is about 6 MB;
  - search terms and indexes are about 20 MB;
  - the evidence tables are about 16 MB: outbox 8 MB, audit log 4.5 MB,
    change log 3.3 MB.
- **Speed:** saving one record, in its own transaction, takes about 3 ms.
  Finding one by an indexed email among 10,000 takes about 14 ms.

This changes the plan in three ways:

1. **Device builds leave out the PostgreSQL driver.** The database driver
   in `platform-objects` becomes a feature, so a device build compiles
   SQLite only.
2. **Evidence tables have retention on the device.**
   - In standalone mode nothing publishes the outbox, so it would grow
     without bound. Device workspaces prune outbox entries and keep audit
     and change-log entries for a set period.
   - In synced mode the server is the record of evidence, so the device
     keeps only what sync has not yet acknowledged.
   - With retention, a record should cost about 2.5 KB before attachments.
3. **Imports and the first sync write in batches.** Writing many records
   in one transaction keeps a 10,000-record import well under the 30
   seconds that one-record transactions would take on server hardware.
   Phone storage is slower still.

Slice 1 measures binary size, memory and timings on real ARM devices
before these numbers go into release notes.

## Continual sync (2026-10-19)

Sync runs continually while it can, not on a timer or a button. The live
change stream (ADR 0018) carries it, and nothing waits for a full pull.

- **Foreground, pull.** While the app is open and online, the device holds
  `GET /objects/changes/stream` open from its last position.
  - Each `change` event names a record. The device fetches that record
    through the record API, which applies sharing, and writes it to the
    replica.
  - A `metadata` event announces the workspace metadata version: before
    the first changes, and whenever definitions move. When it differs from
    the replica's version, the device fetches definitions before it applies
    further changes.
  - Streams end after 15 minutes. The device reconnects with
    `Last-Event-ID`, which also re-checks its session and membership.
  - After an `expired` event, it resynchronises with `touched_after`.
- **Foreground, push.** A local change is queued in the same transaction
  as the write. The queue is flushed as soon as the device is online:
  - straight away after a write, after a short pause so a burst of edits
    goes as one composite request;
  - again after each reconnect.
  The server's result, including any automation changes, comes back through
  the stream, not a second request.
- **Background.** Phones suspend apps, so the server never holds a
  connection to a sleeping device.
  - A change in a workspace sends a content-free push notification, at
    most one every few minutes per device: "workspace changed", with no
    record names or values. It goes through Apple's or Google's service,
    which therefore sees no customer data.
  - The device wakes and runs one short pull and push within the time the
    OS allows.
  - The OS background schedulers (`BGAppRefreshTask`, WorkManager) also
    run a periodic catch-up.
- **Offline.** Edits keep queueing. On reconnect, the push queue flushes
  and then the stream resumes from the stored position. The offline lease
  (§4) still applies.

Built (2026-10-19):
- The stream with resume and expiry (ADR 0018).
- The workspace metadata version: a column on `change_counters`
  (PostgreSQL migration 0032, SQLite step 0017). The engine increments it
  in the same transaction as each object, field, permission, rule or
  automation change. Change pages return it, and the stream announces it.
- Device sign-in and revocation (ADR 0029): OIDC through the system
  browser with the app's own PKCE, one device per member and workspace,
  rotating refresh tokens, and `auth.device_wipe_required` for a revoked
  device or a member who left. Devices register their push token.
- The revocation set (§4): `POST /sync/revocations` takes the records a
  device holds, up to 1,000 ids a request, and returns those the member can
  no longer read. The server reads them as the member through the query
  language, so sharing and object permissions decide, and it keeps no
  per-device manifest. A device sends its replica after each reconnect and
  after a `metadata` event, since permission changes move the version.
- The push notification port and its APNs and FCM adapters (ADR 0030):
  one content-free "workspace changed" message.
- The worker's notification dispatcher (ADR 0030 §3): devices whose
  workspace moved are told at most once per quiet window.
- Record ids assigned by devices (ADR 0031): a replica's engine takes ids
  from a device range the server keeps on create, so offline records push
  with their references intact.
- Server records in a replica (`ObjectService::with_replica`,
  `apply_remote`, SQLite only, as devices are). A record the server sends is
  stored with its version, owner, stamps, values (roll-ups included) and
  recycle bin state, without validation, automation or a change-log entry.
  The replica keeps the server version each record last took:
  - a pull that is not newer changes nothing;
  - a pull over an edit the member has not pushed yet is a conflict, and the edit
    stays;
  - the server's answer to a push replaces the record unless the member
    edited it again meanwhile.
  A lookup whose target is not on the device yet is indexed when the
  target arrives. After-save automations and background and scheduled work
  do not run in a replica, since the server runs them on push. The device's
  change log therefore holds only the member's own edits, which is the
  push queue.
- The push queue and the replica's stored positions
  (`ObjectService::push_queue`, `take_push_answers`, `record_pull`,
  `sync_positions`):
  - the queue walks the device's change log from the last pushed
    position. Each record whose version moved past the server version it
    last took goes once, in the order of its first change, up to 25 in a
    composite request:
    - a record made on the device as `create` with its id (ADR 0031);
    - any other record as `update` of the fields its changes name, or as
      `delete` or `undelete` (added to composite requests for this) when
      only one side has it in the recycle bin. One push reads up to 1,000
      log entries; when they run on past that, an update sends every
      field, which `expected_version` keeps safe.
    - an owner change as `change_owner` (added to composite requests for
      this). Made with other edits, it is held back like a lookup and goes
      next time.
    - Only changes after the log position at which a record last took the
      server's copy count, so a held-back change pushes alone.
  - A lookup to a device-made record that reaches the server later is held
    back and edited in again after the answer, so it pushes next time.
    Records made offline that point at each other therefore all arrive.
  - Each answer is stored with `apply_remote`, keeping an edit the member
    made meanwhile. An answer with the version the replica already took
    (the server found nothing to change) replaces the local record too. A failed operation, such as a version conflict, keeps
    its edit queued and is reported for the member to settle.
  - The pushed position moves past entries whose records have nothing
    left to send. A device-made record discarded before it went is never
    sent.
  - The pulled position and metadata version are stored after a page is
    applied, and never move back.
- Conflict resolution (`ObjectService::merge_remote`, `settle`), for a
  push answered with a version conflict or a pull over edits not yet pushed:
  - the replica remembers the server's value of each field the member
    changes, the first time after it took the server's copy, so a merge is
    three-way. Only those fields of records with edits not yet pushed are kept,
    and taking a server copy forgets them;
  - fields only one side changed merge on their own, and a field both
    changed to the same value needs nothing;
  - a field both changed differently is a `FieldConflict` with the base,
    the device's and the server's values, for the member to choose. `name`
    and `owner` count as fields;
  - a delete against an edit, on either side, stands only once the member
    confirms;
  - `settle` takes the server's copy, then makes the changes the member
    keeps again as ordinary edits, so validation and history apply and
    they push next time against the server's version. A conflict left
    without a choice is `sync.choice_missing`.

- The sync loop (`platform-sync`, `SyncClient::sync_once`), which the
  phone's Rust core links. It reaches the server through a transport: the
  HTTP one calls the workspace API with the device's access token, and the
  app keeps and rotates the refresh token. One sync:
  1. takes the server's definitions on the first sync and whenever a change
     page's metadata version is newer than the one stored, before applying
     that page (`ObjectService::apply_remote_definitions`):
     - objects, fields and validation rules are matched by API name and
       keep the server's key prefixes, so device-assigned ids match;
     - objects and fields the server no longer describes are retired;
     - nothing is logged, and the server's version is stored with the
       pull;
     - the member's access to each object is granted to the replica
       caller's roles, with read access covering every record the replica
       holds, since the server sent only what the member may read;
     - a lookup or roll-up that names an object the member cannot read is
       left out. The object's active rules come with it, since every
       member who can read an object may read them (ADR 0021, "Reading
       rules"), so the device checks the member's edits offline;
     - a key prefix or field type that differs from the replica's is
       `sync.definitions_diverged`: the device starts its replica again;
     - an object the member can read only now has its records read in
       full, since a permission change leaves no log entry and the log
       already passed them while they were hidden.
  2. asks for the revocation set after start, after each reconnect and
     after definitions moved, 1,000 ids a request, and forgets the revoked
     records with their index rows, history and the edits not pushed yet, without a
     log entry (`ObjectService::forget_remote`). Only records the server
     sent are asked about, including those of objects the replica retired,
     so losing access to a whole object erases its records.
  3. pushes the queue. After `records.version_conflict` it fetches the
     server's record and merges it: on its own when no field needs a
     choice, so the merged edits push again in the same sync; otherwise
     the record waits for the member's choice (`SyncClient::settle`). A
     request that stores and merges nothing ends the push, so a change the
     server keeps refusing waits for the member rather than looping.
  4. pulls `GET /objects/changes` from the stored position, reading each
     record through the record API. The first sync, and a pull whose
     position expired, read every record again with `status=any` lists,
     under the current definitions, and then follow the log from the head
     read before.
- The foreground and background runs of the loop:
  - `SyncClient::follow` runs while the app is open and online. It syncs
    once, then holds `GET /objects/changes/stream` open from the stored
    position:
    - it stores each change as it arrives and moves the pulled position
      with it;
    - a `metadata` event with a newer version brings the definitions first,
      with the records of objects the member can read only now, and the
      revocation set;
    - the app calls `local_change` after each write, and the loop pushes
      after a short pause, so a burst of edits goes as one request;
    - when the server ends the stream, every 15 minutes, another sync runs.
      That sync re-checks access with the revocation set, and the stream
      resumes. An expired position reads every record again.
    - A step that started (a sync, a push, storing a change) finishes before
      the app's stop signal is honoured, so no report is lost.
  - `SyncClient::wake` is the short run a push notification or the OS
    background scheduler starts. It is one sync with a time budget:
    - once the budget runs out, it stops between steps, or cuts off the
      request in flight;
    - a step that writes to the replica always finishes, since a SQLite
      transaction cut off part-way would hold the database;
    - what it stored stays stored, and the next sync carries on from there.
    - it never interleaves with the foreground loop: each step that talks
      to the server or writes the replica holds one lock, so a wake that
      starts while `follow` is still alive waits for the step in flight
      and the loop waits for the wake (ADR 0030 §4).
- The offline lease (§4). The app passes the token response's
  `offline_limit_seconds` to `ObjectService::set_offline_limit` at each
  sign-in and refresh, and every sync records contact as soon as the server
  first answers (`record_contact`):
  - past the limit without contact, the replica refuses every member read
    and write with `sync.lease_expired`. The check sits where the engine
    works out what the caller may do, so it covers lists, reads, queries,
    search, saves and conflict resolution;
  - syncing does not go through that check, so the next sync that reaches
    the server unlocks the replica, in the foreground or a background wake;
  - the replica writes down the latest device time it has seen, at most
    once a minute and in a transaction of its own, and measures the lease
    against it. Turning the clock back therefore cannot reopen a lease that
    has run out, and a clock more than five minutes behind that time locks
    too. A clock turned back while the app was not running, before the
    replica saw the later time, is caught by trusted elapsed time when the
    app supplies the OS's monotonic clock (below), within the same boot;
  - while the foreground loop holds the stream open, it confirms contact
    with a request of its own every five minutes (`with_renew_every`), so
    a connected device's lease never runs out;
  - without a limit (a standalone workspace) or before the first contact,
    nothing locks. `ObjectService::lease` tells the app when the replica
    locks, so it can warn the member first.

## Offline directory (2026-11-03)

Built: a synced device holds the workspace's member and role names, so
records name the people and roles they mention while offline.

- Each sync reads `GET /users` and `GET /roles`, which every member may
  read in the browser. A device's access token may read these two lists,
  and nothing else outside the object engine and its own device routes.
  An API client's token is still refused them (`403
  auth.client_route_denied`). It replaces the replica's `replica_members` and `replica_roles`
  tables whole, because members and roles change without the metadata
  version moving. Only names are kept: no email addresses, permissions or
  reporting lines.
- `ObjectService::apply_directory` and `directory` work on a replica only.
  PostgreSQL answers `replica.unsupported`.
- The bridge serves `users` and `roles` from the directory, in the server's
  JSON shapes, so the owner and share pickers work offline. It names
  `owner`, `created_by` and the other user fields in record responses, and
  fills role names in the sharing panel. A member someone added since the
  last sync appears after the next one.
- A locked replica (an expired lease) answers `409 sync.lease_expired` for
  the directory too.
- Sharing stays a server change (`403 sync.server_only`), as before. The
  directory only names existing shares.
## Trusted elapsed time (2026-11-03)

Built: the offline lease can also count time on the device's monotonic
clock, which the member cannot set.

- `TrustedClock` is an optional port. `now()` returns a `TrustedInstant`:
  an identifier of the current boot, and the time since that boot,
  including sleep. The app supplies it with
  `ObjectService::with_trusted_clock`:
  - iOS: `mach_continuous_time` with the kernel's boot time;
  - Android: `SystemClock.elapsedRealtime` with the boot count.

  The adapters come with the mobile builds (#386, #387). Without a
  clock, the lease behaves exactly as before.
- Each contact stores the clock's reading next to the wall-clock time.
  This needs two `replica_sync` columns, migration
  `0024_replica_trusted_contact`.
- Within the boot of the last contact, the lease runs out by whichever
  clock says more time has passed. Its expiry moves forward to what
  trusted time leaves, so the app's warning stays accurate. A wall clock
  turned back while the app was closed therefore cannot stretch the
  lease.
- After a restart, the two readings cannot be compared, and the
  wall-clock rule above applies alone. A device can still be restarted
  and have its clock turned back before the app runs again. That case
  is the remaining limitation, and the server-side revocation covers it
  the next time the device makes contact.

## Encryption at rest (2026-11-02)

Built: every workspace file on a device is encrypted with SQLCipher.

- `platform-storage` has an `encryption` feature that builds SQLite as
  SQLCipher 4, with OpenSSL compiled in, so no system library is needed
  on iOS or Android. `platform-device` turns it on. A server built alone
  stays on plain SQLite. In a build of the whole workspace the feature is
  shared, but SQLCipher without a key reads and writes plain files, so
  the server behaves the same.
- Each workspace has its own random 256-bit key, which sqlx sends as the
  first pragma on every connection. It is a raw key, so no password-based
  derivation runs. SQLCipher's defaults apply: AES-256 for each page,
  with a message authentication code on each page.
- The key lives behind the `DatabaseKeys` port: get or create a key, and
  destroy it. The storage layer never stores, logs or derives the key;
  `DatabaseKey` is zeroed when dropped and never prints. sqlx's statement
  and slow-statement logs are off on encrypted connections, because sqlx
  runs the key pragma through its executor and would log its text. The app
  shell's keychain implements it (`apps/device-shell/src-tauri/src/keychain.rs`):
  the Keychain on macOS and iOS, the Credential Manager on Windows, the
  kernel keyring on Linux, and on Android the Keystore, through the shell's
  `device-keystore` plugin. On Android an AES-256-GCM key generated inside
  the Android Keystore encrypts each secret, with the entry's name as
  associated data, and only the nonce and ciphertext are written to the
  app's private preferences; a copy of those preferences without the
  device's keystore decrypts to nothing. `MemoryKeys` serves tests and the
  demonstration workspace.
- A wrong or missing key fails as `SqliteStoreError::Key`, which the
  engine reports as `409 storage.key_unavailable`, never as an empty
  workspace.
- `SqliteTenantDatabases::wipe` closes the workspace's pool, deletes the
  file and its write-ahead log, and destroys the key, holding the lock
  that opens pools until the key is gone, so a request made during the
  wipe opens a fresh workspace under a new key. The app calls it on
  sign-out and on `auth.device_wipe_required`.
- **Threat model.** This protects a lost or stolen device and a copy of
  the app's files, such as an unencrypted backup. It does not protect
  against code running inside the unlocked app, which holds the key while
  the replica is open. It does not replace the OS's own file protection;
  the shells keep that on too.

## Alternatives considered

- **A thin client over the server API**, as Salesforce does. It is simpler,
  but it gives no real offline work and forces business rules to be
  re-implemented or skipped on the phone. The engine is already a library,
  so it doesn't need to be a thin client.
- **A different embedded store or a conflict-free replicated database.**
  Such databases merge automatically, but the platform's rules (validation, uniqueness, sharing,
  automation) need an authority and explicit versions. Last-writer-wins or
  automatic merges would bypass them. Versions with explicit conflict
  resolution keep the server's rules intact.
- **React Native or fully native UIs.** These mean a second interface to
  build and keep in step with the web. Tauri reuses the web shell and links
  the Rust engine with no bridge to another language.
- **A PWA only.** It is easy to ship, but browser storage can be evicted,
  it is weak for encryption keys, and it can't link the Rust engine without
  WebAssembly limits. A PWA can still serve the web on phones.

## Consequences

- The object engine must stay portable: no PostgreSQL-only logic in
  `platform-objects`, a bounded binary size, and every new feature tested
  on the SQLite adapter. The conformance suites already enforce most of
  this.
- Sync makes versions, the change log and composite requests a public
  contract that old app versions depend on, so they must evolve compatibly.
- Field-level security (#316) becomes a prerequisite for synced data that
  includes restricted fields.
- Devices hold customer data. Encryption, revocation and wipe are part of
  the first synced release, not follow-ups.
