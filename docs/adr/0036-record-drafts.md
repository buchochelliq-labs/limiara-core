# 0036: Record drafts

## Status

Proposed (2026-11-17). Builds on ADR 0011 (UX: the record form and its
states), ADR 0015 (metadata-driven objects), ADR 0016 (sharing), ADR 0017
(field history), ADR 0021 (validation rules), ADR 0024 (the device
replica, §5 the local command bridge, "Encryption at rest" and "Continual
sync"), ADR 0029 (device sign-in, the offline lease and wipe), ADR 0032
(quick actions and global New), ADR 0034 §9 (the replica change feed) and
ADR 0035 G5 (field read and write permissions). This is a design only;
nothing in it is implemented yet. Delivery slices are under Consequences
and in `docs/roadmap/README.md`.

Decided by the product owner before design (2026-11-17):

- Drafts are kept on the device first: IndexedDB in the browser, the
  encrypted replica in the device app. Server-synced drafts are a later
  slice (§12).
- Drafts expire after 7 days by default; administrators may change this
  per workspace (§9). Sign-out, wipe and discarding also clear them.
- Conflicts are compared field by field, with the starting, draft and
  current values, and the member chooses per field (§6).

## Context

A member who is editing a record and loses power, has the browser crash or
closes the tab loses everything typed since the form opened. The record
form (`RecordForm` in `apps/web/src/objects/ObjectPages.tsx`) keeps its
input only in React state, which it already calls the form's `Draft`; it
is gone with the page.

### What exists today

**Forms that edit engine records.** All of them are `RecordForm`, usually
inside `RecordModal`:

| Where | What it does |
| --- | --- |
| Record page, Edit | Updates the record, sending only changed fields with the record's `version` (`records.version_conflict` on a stale version). |
| Object home New and `NewRecordPage` | Creates a record. |
| New on a related list (ADR 0032 §1) | Creates a child with the reference field preset. |
| Global New (`GlobalNew.tsx`, ADR 0032 §1) | Creates a record of a chosen object over the current page. |
| Update quick action (ADR 0032 §2) | Updates the action's fields, with its presets sent as `always`. |
| Create quick action (ADR 0032 §2) | Creates a child with the reference preset and the action's fields. |
| Save & New | After a save, opens a fresh create form in the same modal. |

There is no inline (per-field) editing on the record page today, although
a comment on `ObjectRecordPage` says "inline edit"; editing opens the
modal. The legacy Contacts pages (`apps/web/src/contacts`) and plugin
forms rendered by `apps/web/src/plugin-ui` do not edit engine records.
The Object Manager's metadata forms are administration, not records.

**Browser storage.** The authenticated shell keeps nothing in browser
storage today. The only use of `localStorage` in `apps/web` is the theme of
the external host (`ExternalHostApp.tsx`), which serves external forms and
portals. Drafts would be the first record data the authenticated origin
keeps on disk.

**Sign-out on the web.** Browser sessions are backend-managed: the OIDC
callback creates a session row and sets an HTTP-only cookie that lasts
`platform.sessions.ttl_seconds` (8 hours by default, renewed near the end),
and `POST /auth/logout` revokes the session and expires the cookie. The
shell's user menu shows a Sign out item, but it is disabled: the web shell
has no sign-out of its own yet. A session can therefore end in three ways:
an explicit sign-out (once the menu calls the route), expiry, or revocation
on the server.

**The device app.** The same web shell runs in the Tauri shell, whose
`/api/...` requests go to the local command bridge (ADR 0024 §5) over the
SQLCipher replica. The replica uses SQLite's write-ahead log. Sign-out and
`auth.device_wipe_required` delete the workspace file and destroy its key
(ADR 0029 §7); past the offline lease the bridge answers `409
sync.lease_expired`. The replica already keeps three-way conflict data for
edits not yet pushed (`replica_bases`, `FieldConflict` with base, device and
server values; ADR 0024, "Continual sync").

**Workspace settings.** There is no workspace settings permission. The
kernel's manage permissions are `objects.manage`, `members.manage`,
`devices.manage`, `ui.metadata.manage`, `reports.manage` and
`platform.plugins.manage`. Engine metadata such as validation rules and
quick actions is managed under `objects.manage`, versioned, audited,
published as an event and delivered to replicas by moving the workspace
metadata version.

## Decision

Record forms autosave their input as a **draft**: a local, private copy of
what the member has typed, kept per workspace and member until it is saved,
discarded, cleared or expires. A draft is never a record change. Restoring
one puts its values back into the same form; saving it is the ordinary
create or update.

### 1. What a draft is not

A draft is the member's unfinished input. It is not a record change, so:

- **No platform effect until Save.** Writing a draft creates no record
  version, field history (ADR 0017), outbox event, change-log entry (ADR
  0018), audit entry, automation run (ADR 0022), search index entry or
  sync push. Nothing on the server knows a draft exists (until §12).
- **Validation runs only on Save.** Validation rules (ADR 0021), required
  fields, uniqueness, lookup filters and before-save automations are not
  evaluated while drafting, and a draft may hold values that would fail.
  The form shows the usual errors when the member saves.
- **Saving is the normal path.** Save sends the restored form through
  `createRecord` or `updateRecord` exactly as an untouched form would, with
  the full server checks: permissions, sharing, field permissions,
  validation and automations. A draft grants nothing and skips nothing.
- **A draft holds no authority.** It names an object, possibly a record and
  a quick action, and holds input values. It carries no token, version
  claim or permission the server would trust.

### 2. Scope

Drafts are kept for every `RecordForm` that edits an engine record:

| Form | Draft key |
| --- | --- |
| Record edit | `edit`, object, record id |
| Update quick action | `action`, object, record id, action name |
| New record (object home, `NewRecordPage`, related-list New, global New) | `create`, object, a new draft id; with a reference preset, the parent's object, record id and field |
| Create quick action | `create`, object, a new draft id, the parent and the action name |
| Save & New | each new form after a save is a fresh `create` draft |

- **Edits have one draft per form.** Opening Edit on a record that has a
  draft offers it (§8); a record has at most one edit draft and one draft
  per update action.
- **New records have one draft per form opened.** Each New form starts a
  new draft, so several unfinished new records of one object can exist.
- **Inline edits.** None exist today. If the record page gains inline field
  editing (ADR 0011 §3), each pending inline edit is an `edit` draft of one
  field under the same rules.
- **Out of scope:** the legacy Contacts pages (removed by ADR 0035 slice 6),
  plugin-rendered forms over plugin APIs, the Object Manager and other
  metadata forms, search boxes and filters.

### 3. Where drafts live

The web shell writes drafts through one small interface, `DraftStore`
(`list`, `get`, `put`, `delete`, `clear`), with two implementations. Which
one it uses comes from the host's UI contributions, as the route context of
ADR 0034 §9 does, so `apps/web` has no device-specific code:

- **In a browser** (the server's contributions say `drafts.store =
  "browser"`): IndexedDB, encrypted (§4).
- **In the device app** (the bridge's contributions say `drafts.store =
  "host"`): the bridge's draft routes, kept in the replica (§10). The
  web view's own IndexedDB is never used for drafts there, because it lies
  outside the SQLCipher file, the wipe and the lease.

The contributions also carry the workspace's expiry (§9) and the limits
(§7). A host that sends no `drafts` block gets no drafts, and the forms
behave as today.

#### Browser layout

One IndexedDB database per origin, `limiara-drafts`, with:

- `drafts`: one entry per draft, keyed by draft id, with indexes on
  `[member, workspace]` and `[member, workspace, key]` (the draft key of
  §2). Each entry has plain metadata (draft id, format version, member id,
  workspace id, draft key, last change time, expiry time, size, revision,
  writer tab id) and one encrypted payload (§4).
- `browser`: one entry holding this browser's random **browser id** and the
  member id that last used it.
- `notices`: counts of drafts expired or dropped since the member last
  looked (§8), with no content.

#### The draft document

The payload is one JSON document, written whole on every save:

- the form kind and draft key, the object's API name and the workspace
  metadata version the form was built from;
- for an edit or update action: the record's id, its `version` when the
  form opened, and its name as shown then (the draft's title); for a new
  record: the object's label and the parent's name, if any;
- for each field the member changed (the name counts as a field): the
  **start value** (the record's value, or the form's initial value from a
  preset or picklist default) and the **draft value**, both as the form's
  input values; for a lookup, the chosen record's id and the name the
  picker showed;
- started and last-changed times.

Fields the member has not changed are not stored, so a draft is never a
copy of the whole record. A quick action's presets are not stored either;
they are read from the current action when the draft is restored (§5).

### 4. Encrypting web drafts

Browsers do not encrypt IndexedDB for the app. A draft holds record values
the member read and values they typed, so a plain IndexedDB entry is
readable from a copy of the browser profile, by another person who uses the
same computer account, and by forensic tools for as long as it exists (up to
90 days, §9). Four options were weighed:

| Option | Against a copy of the profile | Shared computers | Cost |
| --- | --- | --- | --- |
| Plain IndexedDB | Readable for the draft's whole life. | Safe only if every deletion succeeds. | None. |
| A non-extractable WebCrypto key stored in IndexedDB | The same as plain: "non-extractable" only stops script from exporting the key, and the browser keeps its material in the same profile. | The same as plain. | Small. |
| A key bound to the session | Unreadable once the session ends. | Sign-out shreds the drafts. | A session lasts 8 hours, so a member whose power fails in the evening loses their drafts by morning, which defeats the purpose. |
| **A per-browser key held by the server** | Unreadable without a live session of the same member. | Sign-out destroys the key, so drafts become unreadable even if deleting them failed. | One user-scoped table and one route. |

**Decision: web drafts are encrypted with a per-browser key held by the
server.**

- **The key.** `GET /api/v1/me/draft-key?browser={browser id}` returns a
  random 256-bit key for the signed-in user and that browser, creating it
  on first use. It needs a browser session: API client and device tokens
  are refused (`403 drafts.browser_only`). It answers with `Cache-Control:
  no-store`. The key is never logged and never returned to anyone else.
- **Where it is kept.** A user-scoped kernel table next to the session
  store, `draft_keys (user_id, browser_id, key, created_at,
  last_used_at)`. It is not tenant data: one key serves every workspace in
  that browser, and the client derives a separate key per workspace. A
  user has at most 20 browser keys; creating a 21st deletes the least
  recently used. Keys unused for longer than the longest allowed expiry
  (90 days, §9) are deleted by a job, as are all of a user's keys when the
  user is deactivated.
- **In the browser.** The shell fetches the key once per page load, derives
  a per-workspace AES-256-GCM key with the RFC 5869 key derivation over SHA-256 (info:
  `limiara.record-drafts.v1`, salt: the workspace id), imports it as
  non-extractable and keeps it in memory only. It is never written to
  IndexedDB, `localStorage` or anywhere else.
- **Each write** uses a fresh random 96-bit nonce. The associated data is
  the format version, member id, workspace id and draft id, so a payload
  copied into another member's or workspace's entry fails to decrypt.
- **What stays plain:** the metadata listed in §3 (ids, the draft key,
  times, size), so expiry and clearing work without the key. Record names,
  field values and lookup names are only in the payload.
- **No plain fallback.** If the key cannot be fetched (the server is
  unreachable when the page loads, or answers an error), drafting is off
  for that page and the form says "Drafts are unavailable"; it never writes
  an unencrypted draft. The web shell cannot load or save without the
  server anyway.

What this protects: a copy of the profile, or a later user of the same
computer account, after the member's session has ended. What it does not
protect: someone using the member's live session in the same browser, or
script running in the origin, which could read the form itself. The
existing content security policy and the rule against third-party script
in the authenticated origin (ADR 0005) remain the defence there.

### 5. Restoring a draft safely

Restoring re-reads everything from the server and trusts nothing in the
draft except the member's own input. In order:

1. **Member and workspace.** Drafts are listed only for the signed-in
   member and the current workspace (§11 covers a different member).
2. **The object.** The object must be in the member's current description
   (`GET /objects/{object}`), with create access for a `create` draft and
   edit access for the others. A missing object (deleted, or suspended with
   its plugin, which the member cannot tell apart) makes the draft
   **unavailable**: it shows as "Not available right now", without its
   title, and can only be discarded; it is kept until it expires, so a
   resumed plugin brings it back.
3. **The record.** An edit or update-action draft reads the record again.
   A record that is gone, in the recycle bin, unreadable, or no longer
   editable (`rights.edit`) makes the draft unavailable in the same way.
   Undeleting the record or regaining access brings it back before expiry.
4. **The action.** An action draft whose quick action no longer exists, or
   whose action the member can no longer run (ADR 0032 §3), restores into
   the full edit or create form instead, with the parent preset kept for a
   create action when the member may still read the parent. The current
   action's presets apply, never stored ones.
5. **Fields**, against the current definitions and the member's current
   access, including field read and write permissions once they exist (ADR
   0035 G5):
   - a field that no longer exists, or that the member can no longer read,
     is dropped without being named; the form says "Some changes were
     dropped because those fields no longer exist or are not available to
     you";
   - a field the member can read but not write, a field the engine now
     computes (formula, roll-up, a composed name, ADR 0035 G17), and a
     field whose type changed so that the value no longer converts, are
     dropped and named by their current label;
   - a lookup whose chosen record the member can no longer read is
     dropped and named;
   - an inactive picklist value is kept and fails on Save like any value.
6. **Start values** shown in the conflict review (§6) are shown only for
   fields that survive step 5.

The server checks everything again on Save. Restoring never sends
anything.

**Inert data.** Draft titles, values, start values and lookup names are
rendered as text through the same field components as the record page,
never as markup, links built from values, or script. A payload that fails
to decrypt or parse, or has an unknown format version, is deleted and
counted as dropped (§8).

### 6. Conflicts are compared field by field

A draft records the record's `version` when its form opened and the start
value of each changed field. On restoring an edit or update-action draft:

- **Same version:** the draft's values go back into the form. Nothing else
  is needed.
- **Newer version:** each field in the draft is compared with the record's
  current value, after the form's own conversion to API values
  (`toApiValue`), so time zones and number formats do not cause false
  conflicts:
  - current equals start: nobody else changed it; the draft value applies;
  - current equals the draft value: both made the same change; nothing to
    do;
  - otherwise: a **conflict**.
- **Conflicts open the review** (§8): for each conflicting field, three
  values: where the member started, their draft, and the current saved
  value, with who saved it and when (from the record's last-modified
  stamps). The member chooses the draft or the saved value per field.
  Nothing is applied until they confirm, and they may discard the draft
  instead. After they confirm, the form opens with the chosen values and the
  record's current `version`, so Save is an ordinary update against the
  current record and a further change in the meantime is the usual
  `records.version_conflict`.
- **Fields only the draft changed** apply as usual, with or without
  conflicts elsewhere.
- **Nothing left to restore.** When every field is resolved to the saved
  value or already equal to it, the draft is deleted and the member is told
  "Your changes are already saved". This also covers a crash between a
  successful Save and the draft's deletion.

This is the same three-way rule the replica uses for edits not yet pushed (ADR
0024, "Continual sync"), applied to a form instead of a queue. `create`
drafts have no record and therefore no conflicts.

### 7. Autosave, durability and limits

**Timing.**

- A change to any input schedules a write after **1 second** without
  further input, and at most **5 seconds** after the first unsaved change
  during continuous typing.
- The pending write is flushed at once on `visibilitychange` to `hidden`
  and on `pagehide`, and when the form closes. IndexedDB work started in
  `pagehide` may not finish before the page is torn down, which is why the
  debounce is short; a power cut can lose at most the last few seconds.
- No write happens until the member changes something: opening a form
  creates no draft.
- A successful Save deletes the draft; a failed Save (validation,
  uniqueness, version conflict, network) keeps it.

**Atomic writes.**

- **IndexedDB.** Each save is one `readwrite` transaction that reads the
  entry's revision, checks it, and puts the whole entry (metadata and
  payload together). Transactions are atomic, so after a crash or power cut
  an entry is either the previous version or the new one, never a mix. The
  transaction asks for `durability: "strict"` where the browser supports
  it, so the browser flushes before reporting success; where it does not,
  a power cut may lose the last completed write, but never corrupts one.
- **SQLite on the device.** Each save is one `INSERT … ON CONFLICT DO
  UPDATE` of the whole row in one transaction, on the replica's
  write-ahead log with `synchronous = FULL` (sqlx's default), so a draft
  the bridge reports as saved survives power loss.
- **Two tabs.** Each entry carries a revision and the id of the tab that
  wrote it. If another tab has written a newer revision of the same draft,
  the write is abandoned and the form says "This draft is open in another
  tab"; the member can take over, which reloads the other tab's version.

**Limits.**

| Limit | Value | When reached |
| --- | --- | --- |
| Drafts per member per workspace, per browser or device | 50 | New forms stop autosaving and say "Draft not saved: you have 50 drafts. Discard some from Drafts." Existing drafts keep saving. Nothing is evicted silently. |
| Size of one draft document (before encryption) | 1 MiB | That draft stops saving and says "Too large to keep as a draft". |
| Storage refused by the browser (`QuotaExceededError`) | browser's quota | The same message as size; the form keeps working. |

- **Long text** is kept whole. A long-text field may hold 131,072
  characters; 1 MiB holds a full one with its start value and the rest of
  a form.
- **Files and attachments are excluded.** The engine has no file field
  today. When files arrive (#288), a draft keeps no file content and no
  upload reference; the form says files must be added again after a
  restore.
- The shell does not request persistent storage (`navigator.storage.persist()`),
  because some browsers ask the member with a permission prompt. Drafts are
  best-effort storage that a browser under storage pressure may evict; the
  member guide says so.

### 8. Experience

All states follow ADR 0011's form patterns: labelled controls, keyboard
operation, visible focus, and loading, error and denied states.

- **Status.** The form footer shows the draft state as text: "Saving
  draft…", "Draft saved at 14:32", or the problem ("Draft not saved: …",
  "Drafts are unavailable"). It is a polite live region (`role="status"`),
  announced on the first save of a form and on changes of state (saved to
  failed, failed to saved), not on every save, so a screen reader is not
  interrupted while the member types.
- **Restore prompt.** Opening a form that has a draft shows, above the
  fields: "You have an unsaved draft from {time}." with **Restore** and
  **Discard**, Restore first in tab order. Until the member chooses, the
  form shows the saved record and does not autosave, so choosing is never
  forced and nothing is overwritten. The record page itself shows a quiet
  notice, "Unsaved draft from {time}", with Continue editing. For new
  records, opening New on an object with drafts lists them ("2 unsaved
  drafts: Continue / Start new").
- **Cancel.** Cancel or Escape on a form with a saved draft asks, in the
  footer: **Keep draft**, **Discard draft** or **Back to form**. Keeping is
  the default action, as closing an email draft keeps it.
- **Drafts list.** The user menu gets **Drafts ({count})**, opening a
  page that lists the workspace's drafts: title, object, kind (edit, new,
  action), last change, expiry, and state (available, conflicts, not
  available). Each has **Open** and **Discard**; **Discard all** asks for
  confirmation. The list checks availability in the background with one
  query per object (ADR 0019) for the listed record ids; a draft whose
  object or record is not available shows no title.
- **Leave-page guard.** A `beforeunload` prompt is registered only while a
  draft write is pending (the debounce timer runs or a write is in flight),
  and removed when it completes. Once a draft is saved, leaving the page is
  safe and is not interrupted. In-app navigation needs no guard: the form
  is a modal and Cancel already asks.
- **Conflict review.** A dialog titled "This record changed since you
  started", with one group per conflicting field: a fieldset whose legend
  is the field label, three labelled values ("You started with", "Your
  draft", "Saved now, by {name} at {time}") and a radio choice between
  "Use my draft" and "Keep saved value", with no default selected. The
  summary says how many non-conflicting changes will also apply. **Apply
  and continue editing** stays disabled, with its reason, until every
  field is chosen; **Discard draft** and **Cancel** are always available.
  It is a full screen on phones (ADR 0011 §3).
- **Expiry notice.** Drafts that expire within a day show "Expires
  tomorrow" in the list and in the restore prompt. When drafts expire or
  are dropped (§5, §11), the next visit to the Drafts list or the next
  restore prompt says "3 drafts expired" or "1 draft could not be
  restored", once, from the `notices` counts.
- **Dropped fields** (§5) are listed in the form's alert after restoring,
  by label where the member may read the field.

### 9. The expiry setting

**Default 7 days; administrators may set 1 to 90 days per workspace.**

- **Meaning.** A draft expires its setting's number of days after its
  last change. Clients compute expiry from the current setting, so
  shortening it expires existing drafts at the next sweep; lengthening it
  does not bring back drafts already deleted. Sweeps run when the shell
  starts, when the workspace changes, and hourly while it is open; on a
  device also after each sync.
- **Where it lives.** It is object engine metadata, because drafts are
  drafts of engine records: a workspace-wide row in a new engine table,
  `record_draft_settings (tenant_id, expiry_days, version, updated_at,
  updated_by)`, in both stores, with `tenant_id` as its key and forced RLS
  on PostgreSQL. No row means the default.
- **Route and permission.** `GET /api/v1/workspaces/{id}/objects/draft-settings`
  for any member; `PUT` with `expected_version` for holders of
  `objects.manage`, which already governs workspace-wide engine settings.
  No workspace settings permission exists today; if one is added, this
  setting moves under it. Values outside 1 to 90 are refused with
  `drafts.expiry_invalid`. The setting appears in Settings as **Record
  drafts**, shown to holders of `objects.manage`.
- **Versioned, audited, published.** A change writes an audit entry (old
  and new number of days, which are configuration, not personal data) and
  `platform.objects.draft-settings-updated.v1` in the same transaction, and
  moves the workspace metadata version.
- **Delivery.** The web shell reads the expiry from the `drafts` block of
  the UI contributions it already loads. Replicas take it with the
  definitions when the metadata version moves, and the bridge serves it in
  its own contributions. A device that has not synced uses the last value
  it took.

**Cleared regardless of expiry:** on sign-out (§11), on a device's wipe or
sign-out (§10), when the member discards, and after a successful Save.

### 10. The device app

On the device, drafts live in the replica, so they are encrypted by
SQLCipher with the workspace key and follow the replica's lease and wipe:

- **Storage.** A replica-only SQLite table, `replica_drafts`, added as a
  tenant step next to the other `replica_*` tables, with `tenant_id`,
  member id, draft id, draft key, last change, revision and the document of
  §3 (unencrypted inside the encrypted file; §4 is not needed). The engine
  never reads it for records.
- **Routes.** The bridge answers `GET|PUT|DELETE
  /api/v1/workspaces/{id}/drafts[/{draft}]` as the signed-in member, with
  the limits of §7. The server does not answer these routes until §12, and
  its contributions say `drafts.store = "browser"`.
- **Lease.** A locked replica answers `409 sync.lease_expired` for drafts
  as for everything else: drafts are unavailable, not deleted, and return
  after the next successful sync.
- **Wipe and sign-out** delete the workspace file and its key, so drafts go
  with them. An interrupted wipe leaves nothing readable, because the key is
  destroyed first.
- **Not record changes.** Draft writes enter neither the push queue nor the
  change log, nor the backup journal (ADR 0024, "Backup to the member's own
  cloud"); a backup snapshot empties `replica_drafts` in its copy before
  encrypting it.
- **The change feed ignores drafts.** Writing a draft emits no replica
  change feed event (ADR 0034 §9), because no record changed.
- **Not for assistants.** The assistant bridge (ADR 0034) has no operation
  that reads drafts, `describe` does not mention them, and the OS index
  never receives them. Drafts are the member's unfinished input, not
  workspace data.
- **Standalone workspaces** (ADR 0024 §2) keep drafts the same way.
- **Restoring** follows §5 and §6 against the replica: the replica's
  definitions and the member's access on the device, then the server's
  checks again when the change is pushed.

### 11. Sign-out, expired sessions and shared computers

- **Explicit sign-out.** The user menu's Sign out (enabled as part of
  slice 1) first deletes every draft of the signed-in member in this
  browser, for every workspace, then posts to `POST /auth/logout` with the
  browser id. The server revokes the session and deletes that browser's
  draft key in the same transaction. If the browser closes part-way, what
  remains cannot be decrypted.
- **Session expiry without sign-out.** Drafts and the key stay: the member
  may have closed the browser after a crash and signed in again the next
  day. When the same member signs in, drafts decrypt as before.
- **A different member signs in.** Before rendering anything, the shell
  compares the signed-in member with the member ids on stored drafts and
  deletes every draft of any other member, counting nothing for them and
  showing nothing about them. Even if that deletion failed, the other
  member's payloads cannot be decrypted: the new member's key is a
  different key, and the associated data names the original member. A
  member mismatch can therefore never reveal another member's draft. The
  other member's server-held key ages out after 90 days unused.
- **Removed from a workspace.** At start the shell deletes drafts for any
  workspace not in `/api/v1/workspaces`.
- **On the device**, sign-out and wipe are already per member and
  workspace (ADR 0029 §2), and delete the replica with its drafts.

### 12. Server-synced drafts (later)

Resuming a draft on another device needs drafts on the server. This ADR
does not design them; it sets what that design must meet:

- **Storage:** a tenant table (`record_drafts`) with `tenant_id`,
  tenant-first keys, forced RLS on PostgreSQL, and a restrictive owner
  policy on a transaction-local member setting beside `app.tenant_id`, so
  only the draft's member can read or write it.
- **Owner-only access:** no administrator, `records.view_all` or
  `records.modify_all` holder, API client or plugin can read drafts; they
  are excluded from the query language, search, reports, bulk jobs, change
  data capture, replicas and the change feed.
- **Contract:** the same draft routes the device bridge answers (§10), so
  the shell's host-backed store works against the server too.
- **Retention:** the workspace expiry of §9, enforced by a server purge
  job; drafts deleted when the member leaves the workspace; included in a
  member's personal data export and erasure.
- **Audit:** individual draft writes are not audited (they are not record
  changes); purges are audited per batch, and the expiry setting already
  is. Whether creating a server draft should leave an audit entry is for
  that review.
- **Also to settle:** encryption at rest of draft values, merging a
  server draft with a local one, conflicts between two devices editing the
  same draft, and limits per member.

It needs its own review before it is built.

## Alternatives considered

- **Server drafts first.** They resume anywhere, but they put unfinished,
  unvalidated input on the server, need owner-only tenancy rules and
  retention before anything else works, and do not work offline. The
  product owner chose local drafts first.
- **Plain IndexedDB.** Simplest, but drafts would be readable from the
  profile for up to 90 days, and shared computers would depend on every
  deletion succeeding. Rejected for the server-held key (§4).
- **A non-extractable key kept in IndexedDB.** Looks like encryption but
  protects nothing against a copy of the profile, because the key material
  is in the same profile. Rejected.
- **A session-bound key.** Strong on shared computers, but 8-hour sessions
  would destroy drafts overnight, which is when a power cut or crash is
  most often discovered. Rejected.
- **A key derived from something the member types.** It would be a
  password, which the platform does not have. Rejected.
- **`localStorage` or `sessionStorage`.** `localStorage` is synchronous,
  string-only, small and has no transactions, so a write can be torn;
  `sessionStorage` is lost when the tab closes, which is one of the cases
  drafts exist for. Rejected.
- **The web view's IndexedDB in the device app.** It sits outside the
  SQLCipher file, the lease and the wipe. Rejected for the replica (§10).
- **Drafts as unsaved record versions in the engine.** They would enter
  history, events, sync and audit, and need validation exemptions. A draft
  is input, not a record change (§1).
- **Restoring over a changed record without review.** Either the draft or
  the other person's change would be lost silently. Rejected for the
  field-by-field review (§6).
- **Evicting the oldest draft at the limit.** Silent loss of work. Rejected
  for refusing new drafts with a message (§7).

## Consequences

Delivery is sliced. Each slice ships alone.

1. **Web drafts, restore and discard, and the expiry setting.** The
   `DraftStore` interface with its IndexedDB implementation and the §4
   encryption, including `GET /api/v1/me/draft-key`, the `draft_keys`
   table and its cleanup job; autosave, status, restore prompt, Cancel
   choices, Drafts list, leave-page guard and expiry notices; the
   re-checks of §5; the `record_draft_settings` table in both stores with
   its route, audit, event, metadata version move and Settings page; the
   `drafts` block in the server's UI contributions; and the user menu's
   Sign out, wired to clear drafts and call `POST /auth/logout`. In this
   slice an edit draft whose record has a newer version restores only when
   it has no conflicting fields (non-conflicting changes apply); a draft
   with conflicts says "This record changed since you started; reviewing
   changes is coming soon" and can be kept or discarded.
2. **Conflict review.** The three-value review of §6 and §8, with
   component and accessibility tests for every resolution path, including
   "nothing left to restore".
3. **Device app drafts.** The `replica_drafts` tenant step, the bridge's
   draft routes and its `drafts` contributions, expiry sweeps after sync,
   the exclusions from the push queue, change feed and backup, and device
   tests for lease, wipe and sign-out.
4. **Server-synced drafts.** Designed and reviewed separately (§12).

Also:

- **A new kind of local data.** The authenticated origin starts keeping
  record values on disk. The threat model and the security documentation
  record what §4 protects and what it does not.
- **A small server footprint before §12.** The draft key table and route
  are user-scoped, hold no record data, and exist only to make browser
  drafts unreadable after sign-out.
- **Nothing changes for records.** No route, event, history entry, sync
  operation or audit entry changes for record saves; the save path is the
  one forms use today.
- **Tests.** Unit tests for the conflict comparison and §5's field rules;
  IndexedDB tests (with a fake implementation) for atomic writes, the
  two-tab check, member mismatch, decryption failure and expiry; browser
  tests for crash recovery (closing the page mid-edit), sign-out clearing
  and the leave-page guard; and accessibility checks of the status region,
  restore prompt, Drafts list and conflict review.

## Open questions

- **Global New.** Should global New's menu also offer new-record drafts
  ("Continue: New Opportunity, 2 drafts"), or should they appear only in
  the Drafts list and on the object's New?
- **Encrypting web drafts.** Is the server-held per-browser key (§4) worth
  its table and route, or should slice 1 start with plain IndexedDB and
  add encryption later? This ADR recommends encrypting from the start,
  because unencrypted drafts written before encryption exists would stay
  readable until they expire.
- **Default limits.** Are 50 drafts per member per workspace and 1 MiB per
  draft right, and are the expiry bounds of 1 to 90 days with a default of
  7 right?
- **Turning drafts off.** Should administrators be able to turn drafts off
  for a workspace, for example where policy forbids record data on
  unmanaged computers, beyond setting a short expiry?
- **Cancel.** Should Cancel default to keeping the draft (as proposed) or
  to discarding it?
- **Persistent storage.** Should the shell ask the browser for persistent
  storage, accepting a permission prompt in some browsers, so drafts are
  not evicted under storage pressure?
