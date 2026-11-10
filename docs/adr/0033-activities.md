# 0033: Activities

## Status

Accepted (2026-11-09). Builds on ADR 0015 (metadata-driven objects and
plugin namespaces), ADR 0028 (related lists), ADR 0032 (quick actions) and
ADR 0011 (the record page's Activity tab). Part of issue #179 ("Log a call")
and the CRM application roadmap. Slice 1's server side, the *related to*
field type (`FieldType::RelatedTo`), is implemented in `platform_objects`
(`values`, `service`, `related`, `quick_actions`, the query planner and the
replica definitions), the API's field and record routes with
`reference_objects`, `related_objects` and the candidates route's `object`
parameter, PostgreSQL migration 0036 and SQLite tenant step 0026. Lookup
filters on related-to fields and the web form follow (see
`docs/roadmap/README.md`). Slice 2's plugin-declared objects are
implemented by the manifest's `[[objects]]` declaration
(`plugin_contract::objects`), the engine's installer
(`platform_objects::plugin_objects`, `ObjectCaller::plugin_installer`), the
`objects.plugin_owned` guards in `platform_objects::service`, the enable,
disable and remove routes in `apps/api/src/plugins.rs`, PostgreSQL
migration 0037 and SQLite tenant step 0027, with the fixture
`fixtures/plugins/tracker`. Slice 3's Activities plugin is implemented by
the manifest-only first-party package `plugins/activities`
(`platform.activities`), the manifest's `[[objects.related_actions]]`
declaration (`plugin_contract::objects::RelatedActionDecl`), the engine's
`platform_objects::related_actions`, the allowlist exception to the
`objects.plugin_owned` guard in `ObjectService::update_field`, PostgreSQL
migration 0038 and SQLite tenant step 0028; the plugin is listed in
`config/default.yml` and exercised by `scripts/test-objects-acceptance.sh`.

## Context

Salesforce records the work done on a record as activities: tasks to do,
calls and emails logged, and meetings. Each one points at what it is about
(an account, a deal, a case: any object) and at who it involved (a contact).
Activities appear on every record page as a timeline, and "Log a call" is
one of the most used quick actions.

Limiara has none of this yet. Two platform pieces are missing:

- **A reference to any record.** A lookup points at one object (ADR 0015),
  but an activity must point at any of several.
- **Objects a plugin declares.** ADR 0015 reserves plugin namespaces and
  origins for engine objects, but nothing installs them yet, so a plugin
  cannot ship objects that workspaces then extend.

CLAUDE.md keeps business capabilities out of the kernel, so activities
themselves must be a plugin.

## Decision

### 1. A domain-neutral "any record" reference

The engine gains a field type, **related to**: a reference whose target
can be a record of any object in an allowlist set on the field. It stores
the record's id, and the record's public id already names its object
(ADR 0015 key prefixes). The rules match lookups:

- the member must be able to see the record they point at;
- deleting the target clears the reference;
- lookup filters (ADR 0028) apply per target object.

The field is indexed by target, so the target's related lists can find
what points at it. Nothing in it is about activities; any object may use it.

### 2. Plugins declare engine objects

A plugin manifest may declare objects and fields in its namespace, as
engine definitions: labels, field types and options. Installing or
upgrading the plugin creates or migrates them with `origin = plugin`. They
are governed like every engine object:

- sharing, permissions, RLS and audit apply;
- validation rules and automations work on them;
- workspace administrators may add `custom` fields, rules, automations,
  quick actions and layouts;
- administrators cannot change or delete the plugin's own fields;
- disabling the plugin hides its objects, and removing it follows the
  plugin removal rules, keeping records for export.

API names follow ADR 0015: the namespace is the plugin id, so the
Activities plugin's Task is `platform.activities:task`. The plugin id must
therefore be a valid namespace, and plugin objects take letter-first key
prefixes. How slice 2 implements this:

- **Install and upgrade.** The kernel installs a plugin's declared objects
  through the object engine as a *plugin installer* caller: it holds no
  member permissions, may change only what that plugin owns, and names the
  plugin (`"plugin"`) in every audit entry and event it causes, with the
  member who changed the plugin's state as actor. Installing is
  idempotent: it adds missing objects and fields and brings labels,
  descriptions, options and flags in line with the manifest. It never
  deletes a field the manifest dropped, which keeps its data; a dropped
  picklist option stays, inactive. A new field type, uniqueness, scale,
  lookup target or delete behaviour, or a new key prefix, is refused with a
  stable `plugin.objects_*` code, and the plugin is not enabled.
- **Transaction boundary.** The installation is one object-engine
  transaction and the plugin registry's transition another: the engine
  opens transactions through its storage port and stays independent of the
  registry's tables. The steps are ordered so every state in between is
  safe: install (new objects start suspended), then enable the plugin, then
  resume its objects. Disabling and removing transition first and suspend
  after. Every step is idempotent, so a failure leaves the plugin not
  enabled or its objects hidden, and repeating the request finishes it.
- **Disabling** sets a per-workspace `suspended` flag on the plugin's object
  definitions. A suspended object is not active, so every engine read
  (describe, lists, records, query, search, related lists, global New,
  navigation and device replicas) treats it as missing, without a check of
  its own. Lookup and related-to values pointing at its records stay and
  are saved again unchanged, but are not followed, and new values to them
  are refused. Enabling clears the flag and everything returns unchanged.
- **Governance.** Administrators may add `custom` fields, rules,
  automations, quick actions and object permissions, and choose the
  object's sharing model and hierarchy grant, which are the workspace's
  security posture; the manifest's sharing model is only the starting
  value. They cannot relabel, change or delete the plugin's fields or
  object (`objects.plugin_owned`): relabelling is the plugin's, through a
  new version. Role permissions start empty, as for a new workspace object.
- **Related-to allowlists are the workspace's** (product-owner decision,
  slice 3). A plugin's related-to field may be declared with an empty or
  partial `reference_objects` list, because a plugin cannot know which
  objects a workspace has. Administrators may change that field's
  allowlist: it is the one property of a plugin's field they may change,
  and a request that changes anything else with it is refused with
  `objects.plugin_owned`. The manifest's list is only the starting value:
  upgrades never overwrite an installed allowlist, as for the sharing
  model. A plugin's related-to field allows 0 to 10 objects, a workspace's
  1 to 10. While the allowlist is empty no value can be set
  (`records.reference_object_not_allowed`), the candidates route refuses
  every object, and device replicas keep the field with nothing allowed.
  Changing it runs slice 1's cascades.
- **Removal** leaves the objects suspended with their records, for export;
  reinstalling resumes them. Purging them after the retention window is a
  follow-up (`docs/roadmap/README.md`).

### 3. An Activities plugin

A first-party plugin, `platform.activities`, declares:

- **Task** (`platform.activities:task`): subject, due date, status (open,
  completed), priority, type (to do, call, email, meeting or other),
  description, *related to*, and *who*. *Who* is a related-to reference
  whose allowlist is people-like objects such as Contacts' Person.
- **Event** (`platform.activities:event`): subject, start, end, location,
  description, *related to* and *who*.

The plugin also ships a default **Log a call** quick action (ADR 0032)
for every object its *related to* allowlist names. It creates a completed
Task of type call, with *related to* set to the current record. Workspaces
may edit it, remove it or add more.

As implemented in slice 3:

- **Names.** Task's key prefix is `tsk` and Event's `evt`. The subject is
  each object's name field, labelled "Subject". Task's fields are
  `due_date`, `status`, `priority`, `type`, `description`, `related_to`
  and `who`; Event's are `starts_at` (required), `ends_at`, `location`,
  `description`, `related_to` and `who`. Both start with the `private`
  sharing model, which each workspace may open up.
- **Packaging.** The plugin is manifest-only: it links no code, and its
  artifact is a marker the registry verifies by digest like any package,
  as a `core-oob` distribution in the reserved `platform` namespace.
  Everything it does goes through the object engine's installer, so
  tenancy, permissions, audit and events apply. It declares no
  permissions or capabilities: access to tasks and events is the
  workspace's object permissions, which start empty.
- ***Who* starts empty too** (product-owner decision). Its allowlist,
  like *related to*'s, starts empty and administrators point it at
  person-like workspace objects. Contacts' Person joins once Contacts
  moves onto the object engine (roadmap).
- **Related actions.** The kernel stays domain-neutral: the manifest
  declares, per object, generic *related actions*
  (`[[objects.related_actions]]`: a related-to field, a name, a label, the
  form's fields and constant presets), checked when the manifest is read.
  Whenever an object is in that field's allowlist it carries a create quick
  action making a record of the declaring object with the field set to the
  current record; Log a call is one, asking for the subject and
  description and presetting type `call` and status `completed`. The due
  date is not asked for: a logged call's date can default later, once
  presets can be computed.
- **Created on entry only.** The engine keeps each declaration per
  workspace with the objects it was offered to, and offers it when an
  object enters the allowlist: once the plugin's objects are in service
  (each resume, for the starting list and for declarations a new version
  adds) and when an administrator adds the object. An object leaving the
  allowlist is forgotten and loses the create actions that depend on the
  field (ADR 0032 §4), so an action an administrator deleted is not
  recreated until its object leaves and re-enters. The action is created
  only if the object has no action of that name and fewer than 10;
  otherwise it is skipped and audited (`objects.quick_action.skipped`),
  and the change that caused it still succeeds. Once created it is an
  ordinary workspace action, and it grants nothing (ADR 0032 §3).
- **Disabling** the plugin hides, with its objects, the create actions on
  other objects that make their records; enabling brings them back.
- **Not yet.** Plugin-shipped validation rules, such as an event ending
  after it starts, are a roadmap follow-up.

### 4. The Activity timeline

The record page's Activity tab (ADR 0011) lists, newest first, the open
tasks and upcoming events, then the past activities that point at the
record through *related to* or *who*. Like related lists, it runs as the
viewer, so it shows only what the viewer may see. It also offers the
record's activity quick actions.

## Alternatives considered

- **Activities in the kernel.** This is simpler, but it breaks the plugin
  boundary and the kernel's neutrality, and other plugins could not
  shape their own activity kinds.
- **One lookup per target object.** A Task with an Account lookup, a Deal
  lookup and so on grows with every object and cannot follow
  workspace-defined objects.
- **Extending Contacts' change timeline.** That timeline records what
  happened to a record. Activities are records people create and own;
  the two are different things.

## Consequences

Delivery is sliced:

1. the *related to* field type, in both stores, the API and the web form;
2. plugin-declared engine objects and the install and upgrade path;
3. the Activities plugin, with its default "Log a call" action;
4. the Activity timeline on the record page.

Also:

- Each slice is useful alone. *Related to* and plugin objects serve any
  plugin, not just activities.
- Device replicas get activities like other objects, under the sharing
  rules (ADR 0024).
