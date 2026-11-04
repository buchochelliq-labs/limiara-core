# First boot and recovery

The local integration stack has one setup and teardown contract. CI calls the
same entry point that contributors use:

```bash
bash scripts/integration-stack.sh up
bash scripts/integration-stack.sh status
bash scripts/integration-stack.sh down
```

Use `reset` instead of `down` to delete the named PostgreSQL, JetStream, and
Keycloak volumes. Set `LIMIARA_COMPOSE_PROJECT` to run isolated stacks in
parallel. The checked-in fixture provides two workspaces, two member users,
different workspace role assignments, and the discovered Contacts plugin. All
fixture credentials are local-only: `member`, `operator`, and `nonmember` use
`local-password`.

After explicitly building the images, `LIMIARA_SKIP_BUILD=1` can skip the
rebuild on a subsequent local `up`. CI and the documented first boot always
build from the checkout.

## Diagnose first boot

Run `bash scripts/integration-stack.sh status`, then inspect
`docker compose logs migrate api worker nats keycloak`. A healthy first boot has a
successful one-shot `migrate` container and healthy infrastructure and runtime
containers. `/health/live` proves that the API process is running;
`/health/ready` additionally requires PostgreSQL, migrations, NATS, and at
least one verified plugin candidate. Health responses deliberately omit
credentials, workspace ids, plugin manifests, and database errors.

The worker has separate internal liveness and readiness checks on its service
listener. Worker readiness requires PostgreSQL, the latest core migration, a
connected and compatible JetStream stream, and a recent successful outbox
claim cycle. The listener starts even when NATS is initially unavailable; the
worker remains live, reports not-ready, and retries broker and stream setup with
bounded backoff until it can start publishing. Docker Compose does not publish
the worker listener. Inspect it only from the local service network:

```bash
docker compose exec --no-TTY worker curl --fail --silent http://127.0.0.1:8080/health/ready
```

Do not route worker health or metrics through the public application origin.

Compose gives the worker its dedicated `platform_worker` database identity. A
standalone worker must override the shared default explicitly; never run it as
`platform_app`, `platform_migrator`, or the bootstrap role:

```bash
PLATFORM__PLATFORM__DATABASE__URL='postgres://platform_worker:<password>@<postgres-host>:5432/platform' cargo run -p worker
```

The worker role has no direct tenant-table access. Startup plugin recovery and
outbox claim, settlement, and backlog access are bounded database functions.

## Outbox publication and quarantine

The worker publishes committed outbox records with at-least-once semantics.
Its internal Prometheus text endpoint distinguishes the durable queue from
process-local activity:

```bash
docker compose exec --no-TTY worker curl --fail --silent http://127.0.0.1:8080/metrics
```

`limiara_outbox_backlog`, `limiara_outbox_claimable`,
`limiara_outbox_claimed`, `limiara_outbox_quarantined`, and the oldest pending
age are database-backed gauges. Attempt, failure, publication, and latency
counters are process-local; sum them across worker replicas. Metrics are
aggregate and deliberately have no tenant, event-type, subject, or payload
labels.

A transient NATS failure leaves `published_at` and `quarantined_at` empty,
records a bounded broker failure code, clears the live claim, and advances
`next_attempt_at`. Restore NATS and confirm that backlog and oldest-pending age
fall. A deterministically invalid envelope instead sets `quarantined_at`,
remains unpublished and unclaimed, and is excluded from retry. Inspect the
bounded evidence through approved operator database tooling without selecting
the envelope payload:

```sql
SELECT tenant_id, id, event_type, attempt_count, failure_count,
       quarantined_at, last_error_code
FROM outbox
WHERE quarantined_at IS NOT NULL
ORDER BY quarantined_at, tenant_id, id;
```

For a specific record, always filter on both `tenant_id` and `id`. An ID-only
query can match rows from more than one workspace after migration 0009:

```sql
SELECT tenant_id, id, event_type, published_at, quarantined_at, last_error_code
FROM outbox
WHERE tenant_id = :tenant_id AND id = :event_id;
```

Codes such as `invalid_envelope`, `envelope_mismatch`, `invalid_subject`, and
`oversized_envelope` identify the validation boundary without storing broker
details or payloads. Correct the producer and retain the quarantined row as
evidence. Do not edit its envelope, clear quarantine, publish it manually, or
reuse its event ID. Limiara does not yet ship a repair or replay command; that
future control must remain operator-only, authenticated, authorised, audited,
and must revalidate the canonical event contract before publication.

## Migrations

Migrations are checksum-ledgered and safe to rerun with
`docker compose run --rm migrate`. Never edit an applied migration; add a new
lexically ordered file. A checksum mismatch or failed statement stops startup
and leaves the failed migration transaction not applied. See
[Database migrations](/operations/migrations) for repair guidance.

Migration 0009 changes audit and outbox identity from global `id` uniqueness to
`(tenant_id, id)`; Contacts migration 0003 makes cursor-token identity tenant
scoped as well. Deploy the role-provisioning service, migrations, and worker
together. The worker's JetStream deduplication key changes to
`<tenant_id>:<id>`. A publication acknowledged immediately before rollout but
not recorded in PostgreSQL can be delivered again under the new key, so
consumers must retain the documented idempotent `(tenant_id, id)` handling.

## Identity recovery

The Keycloak realm is disposable. If its fixture import becomes inconsistent,
run `bash scripts/integration-stack.sh reset` and then `up`. This destroys only
the named local stack volumes. Verify the realm independently with
`bash scripts/test-keycloak-fixture.sh`.

## Plugin recovery

Discovery validates identity, source metadata, dependency ordering, and the
SHA-256 digest of the package artifact before a candidate becomes usable. A
rejected candidate writes only its stable control identifier to the audit log;
it cannot replace the last usable version. Correct the manifest or artifact and
restart the API to retry discovery.

Enable and disable operations are tenant-scoped, durable, idempotent, and
audited with the actor. On worker startup, an interrupted `enabling` transition
converges to disabled and an interrupted `disabling` transition converges to
enabled. Recovery clears transitional fields and records
`plugin.transition.recovered`, preventing lifecycle work from being repeated
silently.

The executable acceptance path is `bash scripts/test-compose-startup.sh`. It
covers clean boot, authentication boundaries, plugin enable/disable and audit,
restart persistence, interrupted-transition recovery, dependency recovery, and
failed migration startup. `bash scripts/test-outbox-publisher.sh` separately
covers concurrent outbox claims, acknowledged publication, broker failure and
retry, quarantine, aggregate metrics, and JetStream state.
