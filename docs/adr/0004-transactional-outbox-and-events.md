# 0004-transactional-outbox-and-events: transactional-outbox-and-events

## Context
Gate 2 needs a secure event path without premature distributed-system complexity. PostgreSQL is the transaction boundary for domain changes, audit evidence, the outbox, inbox markers, and native consumer effects. JetStream provides at-least-once delivery, so acknowledgement cannot make the database and broker one atomic system.

## Decision

- Producers commit the domain change, audit record, and versioned outbox envelope in one tenant transaction. The worker publishes only committed envelopes and marks them published only after broker acknowledgement.
- A native consumer validates the bounded envelope and recomputes its canonical tenant-scoped subject before opening a database transaction.
- Each logical consumer has a stable, bounded ID. Its deduplication key is `(tenant_id, consumer_id, event_id)`.
- The platform inserts the inbox marker before invoking the handler and commits that marker in the same forced-RLS transaction as handler effects. It never performs a select-then-insert deduplication check.
- An already committed marker skips the handler. Applied and duplicate outcomes may be acknowledged only after the database transaction commits.
- Database and transient handler failures roll the transaction back and request bounded redelivery. Malformed, oversized, unsafe, subject-mismatched, or permanently rejected deliveries terminate without a marker. A temporarily unsupported schema may be retried during a bounded rolling-deployment window before termination.
- Consumer subscription registration, plugin enablement, filter authorization, delivery-attempt limits, and the NATS settlement loop are host responsibilities. The reusable inbox primitive does not create a public subscription surface.
- External service plugins cannot share the PostgreSQL transaction and must keep their own durable idempotency ledger behind the governed service protocol.

## Consequences

- Delivery remains at least once. A crash before commit leaves no marker or effect; a crash after commit but before acknowledgement redelivers and returns a duplicate outcome.
- Concurrent duplicates serialize on the inbox primary key. If the first transaction rolls back, a waiting delivery can acquire the marker and process the event.
- Consumer IDs are durable data identities. Renaming one intentionally creates a new idempotency scope and requires an explicit migration decision.
- Inbox markers are retained indefinitely until replay horizons and an audited retention policy are implemented. They must never be purged earlier than any supported replay window.
- Native handler timeouts must remain below the JetStream acknowledgement window. The platform does not extend unbounded work with progress acknowledgements.
- The platform remains extractable later while the first release stays operable as a modular monolith.

## Alternatives considered
Publishing before the producer commit, acknowledging before the consumer commit, select-then-insert deduplication, claiming exactly-once delivery, and granting external plugins a database transaction were rejected.

## Future revisit triggers
Revisit when scale, regulatory isolation, marketplace signing, or independent plugin release cadence creates measurable pressure.
