# 0033: Activities

## Status

Proposed (2026-11-09). Builds on ADR 0015 (metadata-driven objects and
plugin namespaces), ADR 0028 (related lists), ADR 0032 (quick actions) and
ADR 0011 (the record page's Activity tab). Part of issue #179 ("Log a call")
and the CRM application roadmap.

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

### 3. An Activities plugin

A first-party plugin, `platform.activities`, declares:

- **Task** (`activities:task`): subject, due date, status (open,
  completed), priority, type (to do, call, email, meeting or other),
  description, *related to*, and *who*. *Who* is a related-to reference
  whose allowlist is people-like objects such as Contacts' Person.
- **Event** (`activities:event`): subject, start, end, location,
  description, *related to* and *who*.

The plugin also ships a default **Log a call** quick action (ADR 0032)
for every object its *related to* allowlist names. It creates a completed
Task of type call, with *related to* set to the current record. Workspaces
may edit it, remove it or add more.

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
