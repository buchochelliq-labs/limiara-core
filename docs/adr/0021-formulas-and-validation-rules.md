# 0021: Formula language and validation rules

## Status

Accepted (2026-10-12). Builds on ADR 0015 (object engine), ADR 0016
(sharing) and ADR 0019 (query language). Issue #298, part of epic #292.
Implemented in `platform_objects::formula`, migrations 0020 and 0021, the
validation rule API and formula fields.

## Context

Salesforce administrators express business logic without code through one
formula language: formula fields compute values from a record and its
parents, validation rules block saves that break a rule, and the same
expressions appear in automation. The language is side-effect free, typed
against field metadata, and bounded in size so it can run inside every save.

Limiara needs the same, on every storage backend, with the engine's rules:
nothing a workspace administrator writes may reach SQL, run unbounded, read
another workspace, or bypass audit.

## Decision

### 1. The language

Expressions in the familiar Salesforce shape:

```
AND(
  ISPICKVAL(custom:stage, 'won'),
  OR(ISBLANK(custom:amount), custom:amount <= 0)
)
```

- **Values and types:** number, text, boolean, date, date-time and `NULL`.
  Field references use API names (ADR 0015 §3) and the standard fields of
  ADR 0014; a dot follows a lookup to its target, up to three hops
  (`custom:account.custom:region`). Lookups are text (the target's public
  ID), picklists are text, multi-select picklists can only be tested with
  `INCLUDES` and `ISBLANK`.
- **Operators:** `+ - * /` on numbers; `&` joins text; date ± number of
  days, date − date in days, date-time − date-time in days; `= == != <> <
  <= > >=`; `&& || !`.
- **Functions:** `IF`, `CASE`, `AND`, `OR`, `NOT`, `ISBLANK`, `BLANKVALUE`,
  `ISPICKVAL`, `INCLUDES`, `LEN`, `LOWER`, `UPPER`, `TRIM`, `LEFT`, `RIGHT`,
  `MID`, `CONTAINS`, `BEGINS`, `SUBSTITUTE`, `TEXT`, `VALUE`, `ABS`,
  `FLOOR`, `CEILING`, `ROUND`, `MOD`, `MIN`, `MAX`, `TODAY`, `NOW`, `DATE`,
  `DATEVALUE`, `YEAR`, `MONTH`, `DAY`, `ADDMONTHS`, and in validation rules
  `ISNEW`, `ISCHANGED` and `PRIORVALUE`.
- **Type-checked when saved.** A formula is parsed and checked against the
  object's metadata when a rule is created or changed; unknown fields,
  wrong argument types and misplaced `ISCHANGED` are rejected with the
  position, so a rule that saves always evaluates to a boolean.
- **Blank values:** arithmetic with a blank value is blank; `&` treats
  blanks as empty text; `=` and `!=` compare blanks with each other; a
  blank condition counts as false.
- **Errors at run time** (division by zero, an invalid date, text over the
  long text limit) make the result an error, not a value.

### 2. Bounded and side-effect free

A formula has no loops, no user-defined functions, no I/O and no access to
anything but the record being saved, its previous values and the records
its lookups reach. Limits: 3,900 characters, 400 syntax nodes, nesting 40
deep, three hops, and text values up to the long-text limit. Evaluation is
therefore linear in the formula's size. Formulas never become SQL; they are
evaluated in Rust, identically on every backend.

### 3. Validation rules

- Each object has rules: a name, an optional description, a **condition**
  (a boolean formula that describes the error: the save is blocked when it
  is true), a message, an optional field to show the message on, and
  whether the rule is active.
- Active rules run on every create and update made by a member, including
  owner changes, inside the save transaction and before anything is
  written. Every rule whose condition is true is reported, as
  `400 validation.failed` with `code: records.validation_rule`, the rule's
  message, and the field it names or `record`. A rule whose condition
  cannot be evaluated blocks the save with
  `records.validation_rule_error`, because a rule that cannot be checked
  must not be silently skipped.
- System changes (lookups cleared when a target is deleted) and restoring from the recycle bin
  do not run rules, as in Salesforce.
- **Rules run in system context.** Lookup values are read whatever the
  saving member can see, so a rule gives the same answer for everyone; the
  only thing a member learns is the administrator's message.
- Rules are metadata: created, changed and deleted by administrators with
  `objects.manage`, versioned, audited, and published as
  `platform.objects.validation-rule-created.v1` / `-updated.v1` /
  `-deleted.v1` in the same transaction. Like other engine tables they
  carry `tenant_id` with forced row-level security.

### 4. Formula fields

- A field of type `formula` has a formula and a result type (text, number,
  currency, percent, boolean, date or date-time; numbers round to the
  field's decimal places). The formula is checked against the object when
  the field is created or its formula changes, with the same positioned
  errors as rules.
- Values are **computed whenever a record is read** (record reads, lists,
  the responses to saves, and queries) and never stored, so they are always
  current and cost nothing on write. Writing one is
  `records.field_read_only`. A formula that cannot be evaluated, and empty
  text, read as blank.
- Unlike rules, formula fields are shown to members, so **references
  through lookups apply the reader's sharing** (ADR 0016): a target the
  reader cannot see, or one in the recycle bin, reads as blank and its
  values never reach the reader.
- Formula fields cannot be required, unique, indexed, searchable, tracked
  in history or defaulted. Formulas can use other formula fields (§5).
  Queries can select them but not filter, sort or group on them
  (`query.field_not_indexed`); that needs materialised values and is left
  open.
- Deleting a field that a formula field or an active rule needs, on any
  object, is `409 objects.field_in_use`.

### 5. Formulas that use formulas, and more functions (amendment)

Added 2026-10-12 with the formula follow-ups of #298.

- A formula, rule condition or automation value can name a **formula
  field**, on its own object or through lookups. The field's formula is
  checked on the object it belongs to and **inlined**, so evaluation still
  reads only stored fields and lookups still apply the reader's sharing.
  Rules:
  - a formula that comes back to itself through other formulas is refused
    as a cycle, including when an edit would create one;
  - formula fields can use one another at most 10 levels deep, and the
    formulas a formula inlines can add at most 2,000 nodes together, so
    a few formulas that reuse each other cannot grow without bound;
  - lookups count across inlined formulas: at most three in all;
  - `ISCHANGED` and `PRIORVALUE` need stored fields.
- Changing a formula is refused with `409 objects.field_in_use` when it
  would break a formula, rule or automation that uses it. Deleting an object
  is refused with `409 objects.object_in_use` while an automation on another
  object creates its records. Lookups already block deleting the objects
  formulas read through.
- New functions:
  - `REGEX(text, 'pattern')` matches the whole text. The pattern must be
    written in quotes. It is compiled when the formula is checked, with a
    size limit, and matching takes time linear in the text.
  - `ISNUMBER`, `FIND`, `LPAD` and `RPAD` for text.
  - `SQRT`, `EXP`, `LN`, `LOG` and `TRUNC` for numbers. Invalid input,
    such as the square root of a negative number, is an error.
  - `WEEKDAY` (1 is Sunday) and `DATETIMEVALUE` for dates.

### 6. Filtering on formula values (amendment)

Added 2026-10-12, closing the last formula follow-up of #298.

- A formula field whose formula reads only its own record's stored fields
  has a **stored value**. Formula fields it uses count too. It may read the
  record's ID, name and creation stamps, but nothing reached through a
  lookup, and it may not use `TODAY` or `NOW`. The value is written to the
  record's typed index rows with the record, typed by the result: text
  (case-insensitive), number, date-time, or true/false.
- Stored values are rewritten:
  - whenever the record is saved, deleted or restored;
  - for every record of the object when a formula field is added or its
    formula changes, which also covers formulas that use it.
- **Formula fields that predate stored values** have none. Each rebuild of
  an object's index marks the object in `formula_index_marks`, which carries
  `tenant_id` with forced row-level security. The first query that needs
  stored values from an unmarked object rebuilds that object's index once,
  in the query's transaction, before it filters.
- The query language can **filter, sort and aggregate** on formula fields
  with stored values, through the same SQL as indexed fields.
- Formulas that follow lookups or use the time are still **computed when
  read**, with the reader's sharing. Using one in `WHERE`, `ORDER BY`,
  `GROUP BY` or an aggregate is refused with `query.formula_not_filterable`.
  A stored value for these would go stale when a related record changed or
  a day passed. Salesforce draws the same line for which formulas it can
  index.

## Alternatives considered

- **Compile formulas to SQL.** Faster for filters, but two dialects, a much
  larger attack surface, and functions that differ by backend. Evaluating
  in Rust keeps one implementation; filtering on formula values can use
  materialised values later.
- **An existing expression engine or embedded scripting** (CEL, Lua,
  JavaScript). Powerful, but either the semantics differ from what
  Salesforce administrators know, or bounding cost and sandboxing become the
  main work. The language is small enough to own.
- **Validation rules that run in the saving member's sharing context.**
  The same save could then pass for one member and fail for another, which
  makes rules hard to reason about.

## Consequences

- Administrators can enforce data quality per object today. Record-triggered
  automation reuses the evaluator ([ADR 0022](0022-record-triggered-automation.md)).
- Formulas that follow lookups or use the time cannot be filtered on;
  that would need values kept current from related records' changes and
  a schedule.

## Reading rules (2026-11-02)

A synced device checks a member's edits offline against the server's rules
(ADR 0024). Until now only administrators could list rules, so a member's
device never had them and learned of a refused edit only when it pushed.

- Anyone who can read an object can list its **active** rules with `GET
  /objects/{object}/validation-rules`. Rules are not secret from the people
  they constrain: a refused save already shows the rule's message and field,
  and records have no field-level security that a condition could reveal.
- Administrators with `objects.manage` still see every rule, inactive ones
  included. Only they create, change or delete rules.
- To a member who cannot read the object it is unknown: `404`, as for
  `GET /objects/{object}`.
- Automations stay administrator-only: they name actions and other objects
  a member may not see, and nothing on a device runs them.
- A synced device leaves out a rule whose condition names a field it does
  not hold, such as a lookup to an object the member cannot read. It could
  not check that rule, and the rule would refuse every save, so the server
  checks it on push (`DefinitionsApplied::deferred_rules`).
