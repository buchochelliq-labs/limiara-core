# 0009-plugin-worker-contributions: plugin-worker-contributions

## Context

Gate 2 requires tenant plugin activation to govern worker consumers and jobs as well as API and UI surfaces. The `platform_worker` role has no tenant-table access by design.

## Decision

- **One activation rule.** API and worker both call `plugin_registry::activation::resolve_active_manifest`. A plugin is active when the installation is enabled, the exact installed version is `health_checked`, and the manifest id, version, and digest match the verified record.
- **Declared, linked, verified contributions.** Manifests declare `[[worker.consumers]]` and `[[worker.jobs]]` with plugin-namespaced ids. Consumed events must also be in `events.consume` and `capabilities.event_consume`. The worker refuses to start if a linked `NativeWorkerPlugin` does not match its declarations exactly.
- **Activation is re-checked inside each unit of work.** Every delivery and job run locks the tenant installation `FOR SHARE` in its own RLS-bound transaction. Version skew is deferred.
  - A disable waits for in-flight work to finish.
  - Later work sees the disabled state.
  - If the lifecycle store is unavailable, the work fails closed and is retried.
- **Disabled deliveries are skipped.** They are acknowledged with `consumer.plugin_inactive`, with no effects and no inbox marker. Outages retry without limit; retryable handler failures terminate at `max_deliver`.
- **Leased job schedules.** `plugin_job_schedules` (forced RLS) keeps one row per tenant, plugin, and job.
  - Bounded routines register, claim, fail, and release schedules, one run per lease.
  - Disabled schedules pause with their checkpoint kept.
  - `platform_complete_plugin_job` commits the effects, checkpoint, and next run together, and only while the lease is live. A run with an expired lease rolls back.
- **Two roles.** The worker uses `platform_worker` for cross-tenant routines and an RLS-bound `platform_app` pool (`platform.worker.tenant_database_url`) for tenant work.

## Consequences

- Disablement keeps data, markers, schedules, and checkpoints. Re-enable restores one registration per contribution.
- In-flight work can delay a disable by up to the statement timeout, so job leases must exceed that timeout.
- Events delivered while a plugin was disabled are not replayed.
- Metrics are aggregate and carry no tenant labels.

## Alternatives considered

- **Per-tenant durable consumers.** Broker state grows with the number of tenants.
- **Deferring disabled deliveries.** Causes unbounded retention pressure.
- **Tenant policies for `platform_worker`.** Breaks the worker boundary.
- **Checking activation outside the work transaction.** Races with disable.

## Future revisit triggers

Revisit for event backfill across disablement, Wasm or service-plugin background work, or measurable activation-read cost.
