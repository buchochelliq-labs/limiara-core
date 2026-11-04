# Events reference

The Contacts v1 events on this page are Limiara's first repository-owned public event contracts. Every successful Contacts mutation writes one audit record and one event envelope to the transactional outbox in the same PostgreSQL transaction as the domain change. A failed domain, audit, or outbox write rolls the complete transaction back.

The worker publishes committed envelopes to NATS JetStream with at-least-once semantics. The platform provides a tenant-scoped transactional inbox primitive for native consumers; subscription authorization, plugin lifecycle wiring, retention policy, and operator replay remain planned. Broker publication does not yet provide a public external subscription surface. The architecture boundary is recorded in [ADR 0004](../adr/0004-transactional-outbox-and-events.md).

## Envelope

Every catalogue entry uses schema version `1` and the following envelope:

| Field | Type | Contacts contract |
| --- | --- | --- |
| `id` | UUID | Tenant-local event identifier. At-least-once consumers must use `(tenant_id, id)` for deduplication. |
| `event_type` | string | Stable namespaced type from the catalogue below. |
| `schema_version` | integer | `1` for every event on this page. |
| `source_plugin` | string | `platform.contacts`. |
| `tenant_id` | UUID | Workspace boundary for the mutation and event. |
| `actor_id` | UUID | Authenticated principal that performed the Contacts mutation. |
| `correlation_id` | UUID | Correlates the mutation with its originating request or workflow. |
| `causation_id` | UUID or null | Identifier of the event that caused this mutation, when applicable. |
| `occurred_at` | timestamp | UTC time at which the event evidence was created. |
| `payload` | object | One of the bounded payloads documented below. |

The audit record carries the same tenant, actor, correlation, causation, entity identifier, resulting version, and event identifier. Audit records are operational evidence, not integration events.

Event and outbox identity is the pair `(tenant_id, id)`, not `id` alone. The
same UUID may exist safely in two workspaces. Database lookups, operator tools,
consumer ledgers, and plugin-owned references must therefore carry both values.

## Delivery semantics

The worker claims bounded due batches from PostgreSQL using expiring ownership leases. Concurrent workers skip rows already locked by another claimant. Each claimed row is validated and encoded as the canonical v1 envelope before it is published to the configured JetStream stream on this subject:

```text
platform.events.v1.tenant.<tenant_id>.<event_type>
```

The stable JetStream message identity is `<tenant_id>:<id>`. The worker waits for the broker acknowledgement before setting `published_at`. A broker error or timeout leaves the row unpublished, clears its live claim, records a bounded failure code, and schedules a bounded exponential retry. The platform stream has a finite file-storage byte ceiling and rejects new messages when full, preserving them in the retryable database outbox instead of silently discarding older broker events.

A deterministically invalid stored envelope is not sent or retried indefinitely. The worker leaves it unpublished, clears the claim, records a bounded error code, sets `quarantined_at`, and excludes it from later claims. There is no public repair or replay operation. Quarantined records are retained for operator-only diagnosis; an authenticated, audited repair and replay workflow remains future work.

Delivery is not exactly once. A process can receive the broker acknowledgement and stop before it records `published_at`; the same event may then be published again after its lease expires. JetStream duplicate suppression reduces duplicates within its configured window, but every consumer must make repeated `(tenant_id, id)` pairs harmless. Ordering across tenants or concurrent mutations is not guaranteed.

The worker reports aggregate backlog, claim, quarantine, attempt, failure, publication, oldest-pending-age, and publish-latency metrics. These metrics have no tenant, event-type, subject, or payload labels. Worker readiness and metrics are internal operational surfaces, not public application or plugin APIs.

## Consumer transaction contract

Before selecting a tenant transaction, a consumer host rejects an envelope larger than the platform limit, deserializes the canonical envelope, recomputes its JetStream subject, and requires that subject to match the broker subject exactly. This binds the envelope tenant, event type, source plugin, and schema version to the received subject before database work begins.

Each logical native consumer uses an immutable, namespaced consumer ID. The host begins a transaction with the validated envelope tenant and attempts this marker first:

```text
(tenant_id, consumer_id, event_id)
```

The marker table has forced RLS and is append-only to the application role. `INSERT ... ON CONFLICT DO NOTHING` is the concurrency boundary: one delivery owns the marker, while a committed conflict is a duplicate and skips the handler. The marker and handler effects commit together. A handler error or database failure rolls both back, so a later delivery can try again.

The pure processing result maps to broker settlement as follows:

| Result | Broker action | Database state |
| --- | --- | --- |
| Applied | ACK after commit | Marker and effects committed once |
| Duplicate | ACK after confirming the committed marker | Existing marker/effects retained; handler not called |
| Database or transient handler failure | NAK with bounded retry policy | Marker and attempted effects rolled back |
| Temporarily unsupported schema | NAK during a bounded rollout window, then terminate | No marker or effects |
| Malformed, oversized, unsafe, subject-mismatched, or permanently rejected event | TERM | No marker or effects |

The `platform-inbox` crate defines this transaction and disposition contract. Mapping an unsupported schema error to a broker action requires the host to state whether its bounded compatibility retry budget remains; there is no implicit unbounded retry default. Issue #32 supplies authorised, enablement-aware subscription registration and that delivery-attempt policy; issue #37 proves the complete API, PostgreSQL, NATS, acknowledgement, redelivery, and lifecycle path. Until then there is no automatically registered public consumer.

A process failure before database commit leaves no durable marker or effect and the unacknowledged message is redelivered. A failure after commit but before ACK also redelivers, but the committed marker returns the duplicate outcome without repeating effects. This is idempotent at-least-once processing, not exactly-once delivery.

Inbox markers currently have indefinite retention. A future cleanup policy must be tied to the longest broker retention and supported replay horizon. External service plugins cannot atomically share this PostgreSQL transaction and must implement their own durable idempotency ledger.

## Contacts event catalogue

| Event type | Trigger | Payload |
| --- | --- | --- |
| `platform.contacts.person-created.v1` | A person is created. | Person lifecycle payload |
| `platform.contacts.person-updated.v1` | A person is updated. | Person lifecycle payload |
| `platform.contacts.person-archived.v1` | A person is archived. | Person lifecycle payload |
| `platform.contacts.person-deleted.v1` | A person is moved to the recycle bin. | Person lifecycle payload |
| `platform.contacts.person-undeleted.v1` | A person is restored from the recycle bin. | Person lifecycle payload |
| `platform.contacts.organisation-created.v1` | An organisation is created. | Organisation lifecycle payload |
| `platform.contacts.organisation-updated.v1` | An organisation is updated. | Organisation lifecycle payload |
| `platform.contacts.organisation-archived.v1` | An organisation is archived. | Organisation lifecycle payload |
| `platform.contacts.organisation-deleted.v1` | An organisation is moved to the recycle bin. | Organisation lifecycle payload |
| `platform.contacts.organisation-undeleted.v1` | An organisation is restored from the recycle bin. | Organisation lifecycle payload |
| `platform.contacts.custom-field-definition-created.v1` | A custom-field definition is created. | Custom-field definition lifecycle payload |
| `platform.contacts.custom-field-definition-updated.v1` | A custom-field definition is updated. | Custom-field definition lifecycle payload |
| `platform.contacts.custom-field-definition-disabled.v1` | A custom-field definition is disabled. | Custom-field definition lifecycle payload |
| `platform.contacts.custom-field-definition-archived.v1` | A custom-field definition is archived. | Custom-field definition lifecycle payload |

### Person lifecycle payload

| Field | Type | Meaning |
| --- | --- | --- |
| `entity_id` | string | Stable public person identifier, such as `per_...`. |
| `version` | integer | Resulting optimistic-concurrency version. |

### Organisation lifecycle payload

| Field | Type | Meaning |
| --- | --- | --- |
| `entity_id` | string | Stable public organisation identifier, such as `org_...`. |
| `version` | integer | Resulting optimistic-concurrency version. |

### Custom-field definition lifecycle payload

| Field | Type | Meaning |
| --- | --- | --- |
| `entity_id` | UUID | Identifier of the changed definition. |
| `applies_to` | string | `person` or `organisation`. |
| `version` | integer | Resulting optimistic-concurrency version. |

## Compatibility and privacy

The `.v1` suffix and `schema_version: 1` define the compatibility boundary. A documented payload schema may gain optional fields in a compatible release, so consumers must ignore payload fields they do not recognise. The top-level envelope is canonical: the worker removes unknown stored top-level keys, and a new top-level field requires coordinated platform-contract support. Removing a documented field, changing its type or meaning, or making an optional field mandatory requires a new event type and schema version.

Payloads are change notifications rather than row snapshots. They intentionally exclude names, email addresses, telephone numbers, tags, custom-field values, definition API names, labels and descriptions, options, validation rules, data classifications, purpose, retention policy, and encryption settings. An authorised consumer that needs current details must read the owning Contacts API. Never infer exactly-once delivery, global ordering, retention, or replay guarantees from this contract.
