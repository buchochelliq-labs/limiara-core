# Metadata-bound forms and actions

Issue #117 adds a host-owned interaction runtime to the metadata page renderer.
The same page tree supports create, edit, view, quick-create, wizard, embedded,
and related-record modes; plugins provide definitions and operation adapters,
not bespoke form components.

## Trust boundary

The client evaluates only the versioned condition AST. It does not execute
scripts, expressions, HTML, or plugin callbacks. Rust contract validation rejects
unknown field/data-source bindings, empty conditions, unsupported operators,
and cyclic data-source dependencies before publication. The Rust resolver and
TypeScript evaluator carry the same truth-table tests for the supported subset.

The browser's required, disabled, hidden, and read-only states are presentation
and usability features only. Resource APIs remain authoritative for permission,
validation, duplicate detection, and concurrency. Action triggers resolve to a
filtered `ActionDefinition` and are handed to an injected adapter.

## Form behavior

The runtime owns typed values, descriptor defaults, dirty tracking, unload and
cancel protection, loading state, field/form errors, retry-safe retained input,
and ETag concurrency state. Server field errors focus the matching named native
control. A conflict offers two explicit hooks: review the latest values or keep
the caller's changes while advancing to the latest ETag.

Submission payloads include only writable descriptor fields. Fields hidden by a
condition are omitted unless their descriptor explicitly sets
`submit_when_hidden`; this exception is visible in the resource contract and is
therefore reviewable. View mode forces controls read-only regardless of metadata.

`platform.ui.form`, `platform.ui.wizard`, bound field controls, action bars, and
action buttons use native form, input, select, button, heading, status, and alert
semantics. Wizards expose progress through a polite live region, compact layouts
stack actions, and the runtime introduces no motion that needs to be disabled.
