# Persistence and indexing strategy

## Phase 1 database design stress notes
PostgreSQL remains the system of record because Contacts, plugin installation state, audit records, idempotency keys, and the outbox require transactional consistency. Tenant-scoped tables must use composite access paths that start with `tenant_id`, so common workspace queries do not scan across tenants.

## Early indexes
Core tables should keep tenant-first indexes for membership checks, plugin state, audit review, outbox publishing, inbox dedupe, idempotency replay, and session expiry. Contacts adds tenant/name indexes for deterministic pagination and search prefixes; future generated columns or GIN indexes should be added only for frequently filtered custom fields.

## When to add other persistence stores
- Object storage: attachments, imports/exports, plugin artifacts, large audit evidence, and generated documents. Add when payloads exceed practical row storage or must be streamed.
- Key/value cache (Valkey): session acceleration, distributed rate limits, short-lived locks, and hot permission/plugin contribution caches. Add only when DB-backed latency or contention is measured.
- Search engine (OpenSearch): faceted or relevance-ranked cross-field search once PostgreSQL trigram/FTS is insufficient.
- Analytics store (ClickHouse): high-volume reporting, event analytics, and dashboard aggregates that should not compete with OLTP workloads.
- Graph database: only if relationship traversal becomes a first-class workload that cannot be modelled efficiently through plugin-owned relational tables/read models. Do not introduce a graph store for ordinary CRM relationships.

## Performance test gates
The performance test plan targets p95 latency, tenant-isolation query plans, outbox backlog drain rate, and index use. Any new plugin table must document expected list/detail/filter access patterns before migration approval.
