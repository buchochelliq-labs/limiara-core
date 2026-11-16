# 0032: Quick actions and global New

## Status

Accepted (2026-11-09). Builds on ADR 0011 (platform UX direction: the
record form and the `global-new` slot), ADR 0015 (metadata-driven objects),
ADR 0021 (validation rules, whose metadata model this follows) and ADR 0028
(related lists). Part of issue #179. The quick-action metadata is
implemented in `platform_objects::quick_actions`, the API's
`/objects/{object}/quick-actions` routes, PostgreSQL migration 0035 and
SQLite tenant step 0025. Sending actions to device replicas (§5) is
delivered by #236: quick-action changes move the workspace metadata
version, a device takes each object's actions with its definitions, keeps
those it can run offline and defers the rest (see §5).

## Context

Salesforce puts a record's everyday work one click from its page: buttons
that create a related record with the parent already filled in, or update
a few chosen fields without opening the full edit form. A global New menu
creates a record of any object from anywhere in the app. Limiara's record
form already opens as a modal with Save, Save & New and Cancel, but a
record page offers only Edit and Delete, and creating a record means going
to its object first.

## Decision

### 1. Two things need no metadata

- **New on a related list.** Each related list a member may create records
  in offers New. It opens the record form for the child object with the
  lookup or master-detail field set to the current record.
- **Global New.** The shell offers a New menu listing the objects in
  navigation that the member may create. Choosing one opens the record form
  over the current page. Plugin resources join it through ADR 0011's
  `global-new` slot when plugins contribute to it.

Both are host behaviour built from metadata the engine already serves, the
same for every workspace.

### 2. Quick actions are object metadata

An administrator adds **quick actions** to an object. Each has a name, a
label, a position and one of two kinds:

- **Update fields:** a short form of chosen fields of this record, with
  optional preset values applied when it is saved. An example is "Close"
  setting Stage to Closed and asking only for the close date.
- **Create a related record:** a form for a child object that points at
  this object through a lookup or master-detail field. It has chosen fields
  and optional preset values, and the reference field is set to the current
  record. An example is "Add contact" on an account.

An object has at most 10 quick actions. Its record page shows them in
order, next to Edit and Delete.

### 3. An action grants nothing

A quick action is a shortcut to the normal create and update APIs. It runs
with the member's own permissions and sharing, and under the object's
validation rules, uniqueness, lookup filters and automations, exactly as
the full form does. A member sees an update action only on records they may
edit, and a create action only when they may create the child object. The
API, not the button's visibility, enforces this.

### 4. Presets are constants

A preset is a literal value checked against the field's type when the
action is saved. A preset field the form does not show is still sent, so
an action can set Stage without asking. Presets are not formulas. A value
that later fails, such as an inactive picklist value, fails on save like
any other value, with the record's usual error.

Fields the engine computes (formulas and roll-up summaries) cannot be
chosen or preset. Deleting a field removes it from the actions that use
it. Deleting the child object or its reference field deletes the create
actions that depend on it.

### 5. Stored and published like validation rules

Quick actions are created, changed and deleted by administrators with
`objects.manage`. They are versioned, audited, and published as
`platform.objects.quick-action-created.v1` / `-updated.v1` / `-deleted.v1`
in the same transaction. Like other engine tables, they carry `tenant_id`
with forced row-level security. Both storage backends keep them, and they
are part of the object definitions a device replica receives (ADR 0024), so
the phone app can offer the same actions.

Delivered as follows (#236). Creating, changing or deleting an action,
including the create actions a plugin's related actions add (ADR 0033 §3),
moves the workspace metadata version, so devices take definitions again. A
device reads each object's actions from `GET /objects/{object}/quick-actions`
next to its validation rules. Its replica keeps an action only when the
server's own check of the action passes against the replica's definitions:
its fields and presets are on the replica's object and, for a create
action, its child object is held and in service there and its reference
field still points at the object. Any other action is deferred, as rules
that cannot compile on the replica are: it stays with the server and is
listed in the sync report, and it returns when what it needs does. An
action whose child object is suspended with its plugin (ADR 0033 §2) is
therefore removed from the device, and an object the device no longer
holds takes its actions with it. Running an action on the device is the
local record create or update as the member; the server checks it again on
push (ADR 0024). Taking the same definitions twice changes nothing.

### 6. Logging a call waits for activities

"Log a call" needs an activity object (tasks, calls and events on a
timeline) that the engine does not have yet. It is deferred to the
activities decision and recorded in the roadmap. ADR 0033 §3 delivers
it as a related action of the Activities plugin: an ordinary create action
the engine adds to each object entering Task's *related to* allowlist.

## Alternatives considered

- **Actions in the workspace layout store (ADR 0011 §4).** Layouts are
  overlays on plugin pages and do not reach devices. Actions are behaviour
  every client should share, so they live with the object's definition,
  as validation rules do.
- **Formula presets.** These are useful, for example "today", but they mean
  evaluating a formula against a record that does not exist yet. Constants
  cover the common cases first.

## Consequences

- A record page reads its object's quick actions with the description it
  already loads, so this adds no request.
- Actions add no permission or bypass: everything they do, the member could
  do through the full form.
- Delivery is sliced:
  - related-list New and global New, in the web app only;
  - then the quick-action metadata in both backends, the API, the Object
    Manager and the record page.
