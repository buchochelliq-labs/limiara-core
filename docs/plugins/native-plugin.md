# Native plugin guide
Create a manifest matching `plugin-contract`, declare permissions/events/capabilities, keep domain tables in a plugin-owned schema, use tenant-scoped DB abstractions, emit outbox events in the same transaction as mutations, and register UI through declarative contributions.

## Plugin execution context

Open every plugin transaction with `platform_db::begin_plugin_tx(pool, tenant_id, plugin_id)`, and give every table in your `p_*` schema a restrictive policy that requires `current_setting('app.plugin_id', true) = '<your plugin id>'` (see [ADR 0010](../adr/0010-plugin-execution-context.md) and `plugins/contacts/migrations/0005_plugin_execution_context.sql`). The tenant isolation harness fails if a plugin table lacks one. Inbox markers and outbox events are accepted only inside your plugin's namespace.

## Background work

Declare each worker contribution in the manifest, then implement it with `plugin_runtime_native::NativeWorkerPlugin`. The worker refuses to start if the declarations and the linked code differ. Contacts is the reference implementation (`plugins/contacts/backend/src/worker.rs`).

```toml
[entrypoints]
worker = true

[[events.consume]]
type = "platform.contacts.person-created.v1"

[capabilities]
event_consume = ["platform.contacts.person-created.v1"]

[[worker.consumers]]
id = "platform.example.timeline-v1"      # stable inbox identity; never reuse
events = ["platform.contacts.person-created.v1"]

[[worker.jobs]]
id = "platform.example.digest-v1"
interval_seconds = 3600                  # 60 seconds to 7 days
```

Handlers and jobs receive the tenant transaction. Write every effect through it, so effects commit with the inbox marker or the job checkpoint. A job returns its next checkpoint as a JSON object of at most 16 KiB. Failures are bounded machine codes, never free-form text. The platform runs work only for tenants whose installation is active (see [ADR 0009](../adr/0009-plugin-worker-contributions.md)). Run `bash scripts/test-plugin-runtime.sh` to check this behaviour.
