# 0010-plugin-execution-context: plugin-execution-context

## Context

All trusted native plugins share the RLS-bound `platform_app` role, so tenant isolation is enforced but plugin isolation is not. A bug in one plugin, or a confused kernel code path, could:

- read another plugin's private tables;
- write inbox markers under another plugin's consumer identity;
- publish events as another plugin;
- settle another plugin's job schedule.

Per-plugin database roles would close this, but they multiply provisioning, connection pools, grants, and credential rotation for every plugin.

## Decision

- **A transaction-local plugin execution context.** The host sets `app.plugin_id` with `platform_db::begin_plugin_tx(pool, tenant_id, plugin_id)` before running plugin code, next to `app.tenant_id`. The identifier is validated before any statement runs.
  - The Contacts service opens every transaction this way.
  - The worker opens every consumer delivery and job run in the owning plugin's context.
  - A consumer id must sit inside that plugin's namespace.
- **Restrictive RLS policies enforce it.** Each policy is ANDed with the tenant policy, so plugin work needs both the right workspace and the right plugin:
  - **Plugin tables:** every table in a `p_*` schema requires its owning plugin's context for reads and writes. The tenant harness fails if any plugin table lacks such a policy.
  - **Job schedules:** reads, writes, and lease settlement are limited to the current plugin.
  - **Inbox:** a marker can only be written under the current plugin's consumer namespace.
  - **Outbox:** an event can only be written with the current plugin as `source_plugin`, and in its event-type namespace.
  - Reads of the inbox and outbox stay tenant-scoped, for diagnostics.
- **Kernel code without a plugin context sees no plugin rows.**

## Consequences

- A plugin's data, events, and background state can only be used inside that plugin's context, in the right tenant. Cross-plugin access must go through public APIs, events, or governed read models.
- New plugins must add a restrictive `app.plugin_id` policy to each table and open transactions with `begin_plugin_tx`.
- The boundary guards against bugs and confused deputies, not hostile native code: in-process code can set the variable itself. Isolation from hostile code remains the job of the Wasm and external-service runtimes (ADR 0002).
- There is still one runtime role, one pool, and no extra credentials.

## Alternatives considered

- **Per-plugin database roles.** Strongest, but operationally heavy at this stage.
- **Schema `USAGE` grants per role.** Still requires per-plugin roles.
- **Rust-only checks.** Bypassed by any raw query.

## Future revisit triggers

Revisit when third-party native plugins are allowed, when audit needs per-plugin database identity, or when a plugin needs a governed read of another plugin's data.
