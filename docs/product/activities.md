# Using Activities

**Status:** experimental. Design: [ADR 0033 §3 and §4](../adr/0033-activities.md).
Plugin: `platform.activities`, in `plugins/activities`.

Activities record the work done on a record: tasks to do, calls and emails
that happened, and meetings. Each one can point at the record it is about
and at the person it involved. The Activities plugin ships two objects and
a **Log a call** quick action, and lists them in an **Activity** section on
the pages of the records they point at.

## Turning it on

A workspace administrator enables the plugin
(`POST /api/v1/workspaces/{workspace}/plugins/platform.activities/enable`).
Tasks and Events then appear in navigation for anyone who may read them.
Like any new object, no role has access until an administrator grants it in
the object's permissions, and records start private: each workspace chooses
the sharing model.

Disabling the plugin hides tasks and events everywhere, with the Log a call
actions that make them. Nothing is deleted. Enabling it again brings
everything back unchanged.

## Tasks

| Field | What it holds |
| --- | --- |
| Subject | What the task is. Required. |
| Due date | When it is due. |
| Status | Open (the default) or Completed. |
| Priority | High, Normal (the default) or Low. |
| Type | To do (the default), Call, Email, Meeting or Other. |
| Description | Notes, as long text. |
| Related to | The record the task is about: a record of any object the administrator allows. |
| Who | The person the task involved: a record of any object the administrator allows. |

## Events

| Field | What it holds |
| --- | --- |
| Subject | What the event is. Required. |
| Start | When it starts. Required. |
| End | When it ends. |
| Location | Where it takes place. |
| Description | Notes, as long text. |
| Related to, Who | As on tasks. |

Nothing checks yet that an event ends after it starts. A workspace can add
that as a validation rule, for example `platform.activities:ends_at <
platform.activities:starts_at`.

## Choosing what activities can point at

*Related to* and *Who* start with no objects allowed, so they cannot hold a
value until an administrator chooses their objects. Choose them through the
objects API (an editor in the Object Manager follows):

```http
PATCH /api/v1/workspaces/{workspace}/objects/platform.activities:task/fields/platform.activities:related_to
{"expected_version": 1, "reference_objects": ["custom:account", "custom:deal"]}
```

Up to 10 objects can be allowed, and the list can be emptied again. This
list is the only part of the plugin's fields a workspace can change; labels,
types and options belong to the plugin. Upgrading the plugin never changes
the workspace's list.

Point *Who* at the objects that hold people in your workspace, such as a
Contact object. Contacts' Person will join once Contacts moves onto the
object engine.

Taking an object out of a list keeps the values that already point at its
records, but no new ones can be set.

## Log a call

Every object added to Task's *Related to* list gets a **Log a call** quick
action on its record pages. It asks for a subject and a description, and
saves a completed task of type Call with *Related to* set to the record.

- It is an ordinary quick action: administrators may relabel it, change its
  fields and presets, reorder it or delete it, and add more of their own.
- A deleted Log a call stays deleted while the object stays in the list. It
  comes back only if the object is taken out of the list and added again.
- It is not added if the object already has an action named `log_a_call` or
  already has 10 quick actions. The audit log records why
  (`objects.quick_action.skipped`), and the list change still saves.
- Taking the object out of the list removes its Log a call, with any other
  action that creates tasks through *Related to*.
- It grants nothing: only members who may create tasks can use it, and it
  saves through the same checks as the full task form.

## The Activity section

Every record page that tasks or events can point at, through *Related to*
or *Who*, shows an **Activity** section after the related lists.

- **Open and upcoming** lists open tasks and events that start today or
  later, soonest first, then open tasks without a due date. An open task
  whose due date has passed is marked **Overdue**. Up to 50 are shown.
- **Past** lists completed tasks and past events, newest first. A task
  without a due date, such as a logged call, sits where it was logged.
  **Show more** loads 20 more at a time.
- Each entry shows whether it is a task or an event, links to it, and shows
  its date and its status, priority and type.
- The section offers the page's quick actions that create tasks or events,
  such as **Log a call**. Saving one refreshes the section.
- You see only the tasks and events you may see: with private sharing,
  other members' activities stay theirs.
- A record that no activity can point at shows no Activity section, and
  neither does any page while the plugin is disabled.
- An activity whose object was taken out of *Related to*'s list still
  appears on the record it points at.

Not yet: tabs on the record page, completing a task from the section, and
emails. See the [roadmap](../roadmap/README.md).

