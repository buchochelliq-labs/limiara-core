# 0022: Record-triggered automation

## Status

Accepted (2026-10-12). Builds on ADR 0015 (object engine), ADR 0016
(sharing), ADR 0017 (field history), ADR 0018 (change data capture) and
ADR 0021 (formula language). Issue #298, part of epic #292. Implemented in
`platform_objects::automation`, PostgreSQL migration 0022 and SQLite tenant
step 0008.

## Context

Salesforce administrators automate most routine work without code through
record-triggered flows. A flow runs when a record is created or updated:
before the save it can fill in fields on that record ("fast field
updates"), and after the save it can create records and update related
ones. The flow's conditions and values use the same formula language as
validation rules.

Limiara has that language (ADR 0021). It needs automation built on it that
follows the engine's rules on every backend. Nothing an administrator
writes may reach SQL, run without bounds, read another workspace, or
bypass validation, audit, change data capture or field history.

## Decision

### 1. Automations are per-object metadata

An automation belongs to an object and has:

- a **name** (`[a-z][a-z0-9_]*`, unique per object). Automations run in
  name order, so administrators control the order;
- a **trigger**: `create`, `update` or `create_or_update`;
- a **timing**: `before_save` or `after_save`, fixed once created;
- an optional **condition**, a boolean formula. The automation runs only
  when it is true. It may use `ISNEW`, `ISCHANGED` and `PRIORVALUE`, so
  "only when the stage changes to won" is
  `AND(ISCHANGED(custom:stage), ISPICKVAL(custom:stage, 'won'))`;
- **actions**, each setting fields to the values of formulas. A formula is
  evaluated on the record that was saved, and is type-checked against the
  field it sets;
- whether it is **active**, an optional description, and a version.

The actions:

| Timing | Action | Effect |
| --- | --- | --- |
| `before_save` | `update_record` (exactly one) | Sets fields on the record being saved, before validation rules run and before anything is written. |
| `after_save` | `update_related` (lookup) | Updates the record a lookup on the saved record points to. Nothing happens when the lookup is empty or its target is in the recycle bin. |
| `after_save` | `create_record` (object) | Creates a record. The automation must set its name and every required field without a default. |

An after-save automation has 1 to 10 actions. Each action sets 1 to 50
fields. An object has at most 50 automations.

### 2. Where automations run in a save

A member's create or update now runs in this order, in one transaction:

1. The member's values are validated and applied (unchanged).
2. **Before-save automations** run in name order. Each sees the record as
   the previous ones left it. Their values go through the same field
   validation as a member's.
3. Lookups are resolved. A lookup the member set must point to a record
   the member can see. A lookup set by an automation only has to exist.
4. Validation rules run (ADR 0021), so they check what automations set.
5. The record, its indexes, evidence, change-log entry and history are
   written.
6. **After-save automations** run in name order. Each action saves its
   record through the same path, steps 1 to 6, so that record's own
   automations, rules, indexes, events, change data capture and history
   all apply.

If anything fails, the whole change rolls back and the member's save
returns `400 validation.failed` with one error:
`{"field": "record", "code": "records.automation_failed"}`. Its detail
names the automation and the reason, such as a validation rule on a created
record.

The system clearing of lookups when a target is deleted does not run
automations. Owner changes, deletes and restores do (§5).

### 3. System context, bounded

- **Automations run in system context.** They read lookups and write
  records whatever the saving member can see or create, as Salesforce
  record-triggered flows do by default. Only administrators with
  `objects.manage` can define them, and every record they write carries the
  member's id as the changer, with the member's correlation id, in audit,
  events, the change log and history.
- **Bounded:** automations that cause other automations to run stop at
  **5 levels**, and one change may make at most **100** automation saves.
  Past either limit the change fails with `records.automation_failed`.
- **Loops end on their own:** an automation's update that changes nothing
  is not written, so it does not run that record's automations again.
- Formulas are evaluated in Rust exactly as in validation rules (ADR 0021
  §2). They never become SQL.

### 4. Checked when saved, kept working

- An automation is compiled against current metadata when it is created or
  changed. Each problem is reported against the part it concerns, such as
  `actions[0].values[1].formula`. Codes: `automations.name_invalid`,
  `automations.description_invalid`, `automations.condition_invalid`,
  `automations.actions_invalid`, `automations.values_invalid`,
  `automations.lookup_invalid`, `automations.object_unknown`,
  `automations.field_invalid` (missing, or a formula field),
  `automations.field_duplicate`, `automations.formula_invalid`,
  `automations.value_required`, `automations.limit_reached`, and
  `409 automations.duplicate_name`.
- Deleting a field an active automation needs, on its object or on any
  object it writes, is `409 objects.field_in_use`, the same as for rules
  and formula fields. Deleting an object an active automation creates
  records of is `409 objects.object_in_use` (added with ADR 0021 §5). An
  automation that no longer compiles anyway blocks the saves it would run
  on, and the error tells the member to ask an administrator.
- Automations are versioned, audited and published as
  `platform.objects.automation-created.v1` / `-updated.v1` / `-deleted.v1`.
  Formulas stay out of event payloads. The `automations` table carries
  `tenant_id` with forced row-level security.

### 5. Owner change, delete and restore; evidence; run history (amendment)

Added 2026-10-12 with the automation follow-ups of #298.

- **Owner changes are updates.** `update` and `create_or_update`
  automations run when a record changes owner:
  - before-save automations can set fields, which `ISCHANGED(owner)`
    detects;
  - after-save automations then run as for any update.
- **Delete and restore.** The new triggers `delete` and `undelete` run
  after a record moves to or from the recycle bin. They are after-save
  only (`automations.trigger_invalid` otherwise). Their formulas read the
  record as it is in, or out of, the recycle bin. The lookups that deleting
  clears on other records are cleared by the system and do not run
  automations.
- **Changes are marked.** Records that after-save actions create or update
  carry `automation_id` in their events and audit entries, alongside the
  member's id and correlation id. Before-save changes are part of the
  member's own change.
- **Run history.** Each time an automation's condition holds, a run is
  recorded with:
  - the record, the event (`create`, `update`, `delete` or `undelete`);
  - the outcome and the number of records its actions saved;
  - when it ran, for whom, and the correlation id.

  A successful run is written in the change's transaction. When a run
  fails, its change rolls back and the run is written afterwards in a
  transaction of its own, with the reason. That write is best effort,
  since the member needs the save's error, not a logging error. Runs are
  kept for 30 days. The `automation_runs` table carries `tenant_id` with
  forced row-level security. Administrators read runs at
  `GET /objects/{object}/automations/{name}/runs`, newest first, with a
  cursor.

### 6. Background and scheduled automations (amendment)

Added 2026-10-12 with the automation follow-ups of #298.

- Two new timings run an automation's actions **after** the change, in the
  background, so the member's save does not wait for them:
  - `async` runs as soon as the worker picks it up;
  - `scheduled` runs `offset_minutes` after its time, or before it when
    negative, up to a year either way. Its time is a date or date-time
    field of the record (`schedule.from`), or the change itself.
- Background automations can update the record they ran for
  (`update_record`), update related records and create records. Scheduled
  ones fire on create and update only.
- **Queued with the change.** When the trigger fires and the condition
  holds, a job is written to `automation_jobs` in the change's own
  transaction, so a rolled-back change queues nothing and a committed one
  is never lost. A scheduled automation keeps one pending job per record:
  - a later change moves it to the new time;
  - a change that makes the condition false cancels it;
  - a blank date field cancels it too.
- **Run later, in the workspace.** The worker finds due jobs across
  workspaces only through the bounded `platform_due_automation_jobs`
  routine, as for extension purges (ADR 0012). It then claims each job in
  its workspace with a 10-minute lock and runs it in a transaction of its
  own:
  - it runs as the member whose change queued it, in system context;
  - the values are evaluated on the record as it is then;
  - the condition held when the job was queued and is not checked again,
    since `ISCHANGED` has no meaning later.

  A job whose automation was switched off or deleted, or whose record is
  gone, is cancelled. A worker that stops mid-job loses its lock, and the
  job is claimed again.
- **Retries.** A failed attempt is recorded as a failed run with the
  reason, and the job waits 1, 5 or 30 minutes, then 2 hours, before its
  next attempt. After 5 attempts it fails for good.
- **Operator controls.** Administrators list an automation's jobs. They can
  retry a failed job, which gets a fresh set of attempts, or cancel a
  pending one. Both actions are audited and published as
  `platform.objects.automation-job-retried.v1` / `-cancelled.v1`.
- **Bounded.**
  - A job's own changes do not queue that automation again for the same
    record.
  - Chains of jobs queuing jobs stop at 5 levels, with a failed run that
    says so.
  - The table carries `tenant_id` with forced row-level security.
  - The worker needs `platform.worker.tenant_database_url`. Without it,
    jobs wait and the worker logs that they will not run.

### 7. Decisions and workspace events (amendment)

Added 2026-10-12 with the automation follow-ups of #298.

- **Decisions.** Any action can have its own `when` formula. The action
  runs only when it holds, so one automation can branch:
  - give actions complementary conditions for an if-else;
  - give each action its own case for several branches.

  The formula uses the same language and functions as the automation's
  condition. A `when` that cannot be evaluated fails the run.
- **Workspace events.** The `publish_event` action publishes a
  workspace-defined platform event, with these rules:
  - It has an `event` name (`[a-z][a-z0-9_]*`) and up to 50 fields, each
    the value of a formula of any type. A field name follows the same rule.
  - It runs after saving, in the background or on a schedule, never before
    saving.
  - It is written to the outbox in the change's transaction, as
    `platform.objects.workspace-event.v1`, and delivered like the engine's
    other events (ADR 0010).
  - Its payload names the event, object, record and automation, and
    carries the fields. Unlike the engine's other events, it intentionally
    carries record values that the administrator chose.
  - The audit entry names the event without its fields.
  - A payload over 64 KB fails the run.
- Loops over related records are still left open.

## Alternatives considered

- **Run all after-save work asynchronously, from the outbox.** Durable
  retries and no added latency, but members would see partial results.
  After-save actions stay synchronous, matching Salesforce's
  same-transaction after-save flows. Background and scheduled work is a
  separate, opt-in timing with its own job table (§6), not the event
  stream, because jobs need claiming, retries and operator controls.
- **Run automations in the member's sharing context.** Then the same save
  could work for one member and fail for another, and automations could
  not maintain records members cannot edit, which is their most common use.
- **A general flow graph (decisions, loops, variables) from the start.**
  This is the larger Salesforce Flow surface. Conditions plus a list of
  actions cover most record-triggered automation, and keep cost and
  failure modes easy to reason about.

## Consequences

- Administrators can fill in fields, keep related records up to date and
  create follow-up records without code, on every backend.
- A member's save can now change records the member cannot see. This is
  intended, and evidence for those changes names the member.
- Saves of objects with after-save automations take longer, in proportion
  to the records the automations write.
- Left open on #298 and listed in the roadmap:
  - replay diagnostics (run history is in §5);
  - loops over related records;
  - actions that call out through connectors.
- Since the formula follow-ups (ADR 0021 §5), automations can use formula
  fields in their conditions and values.
