# Runtime configuration

API, worker, and plugin runner load the same `platform-config::Settings`
contract before binding a listener. Missing or invalid required fields stop the
process with a field name; configuration values and secrets are not included in
validation errors.

Sources are applied in this order, with later values taking precedence:

1. Required YAML defaults from `config/default.yml`, or the path in
   `PLATFORM_DEFAULT_CONFIG_FILE`.
2. An optional YAML override named by `PLATFORM_CONFIG_FILE`.
3. Environment variables beginning with `PLATFORM__`.

Environment nesting uses a double underscore for every path segment. For
example:

```text
PLATFORM__PLATFORM__DATABASE__URL=postgres://...
PLATFORM__PLATFORM__OIDC__ISSUER_URL=https://identity.example/realms/company
PLATFORM__PLATFORM__SESSIONS__COOKIE_SECURE=true
```

Compose mounts `./config` at `/config` read-only and sets
`PLATFORM_DEFAULT_CONFIG_FILE=/config/default.yml`. To enable the checked-in
local override template:

```bash
cp config/local.yml.example config/local.yml
PLATFORM_CONFIG_FILE=/config/local.yml docker compose up --build --wait
```

## Outbox publisher settings

`platform.nats.stream` names the JetStream stream. The nested
`platform.nats.outbox` settings control claim batch size, maximum in-flight
publications, polling, claim leases, publication timeout, retry backoff, and
JetStream duplicate suppression. `stream_max_bytes` is a finite file-storage
ceiling; when it is reached, JetStream rejects new messages so their outbox
rows remain retryable instead of silently discarding older events. Use the checked-in values in
`config/default.yml` unless measured backlog and broker evidence justify a
change.

These values are validated as one safety envelope. Maximum in-flight work
cannot exceed the batch size or exhaust the worker database pool; retry base
cannot exceed retry maximum; and a claim lease must cover every bounded
publication wave, database timeout, and safety margin. An existing stream must
use file storage, own exactly the Limiara v1 subject filter, and match the
configured duplicate window and byte ceiling. Subject transforms, republish,
mirror, and source imports are rejected. At startup, an unavailable broker or
unsafe stream combination keeps the worker not-ready and prevents publication
while its internal health and metrics listener remains available. The worker
retries the validated connection with bounded backoff instead of weakening
tenant routing, lease ownership, storage safety, or duplicate suppression.

## Database pool settings

`platform.database.acquire_timeout_ms` (default 3000) bounds how long a
request or worker task waits for a pooled connection. During a database
outage, API requests then fail closed with 503 within that bound and readiness
reports the dependency as unavailable. Without the bound they would hang for
the driver's 30-second default. It must be non-zero.

## Plugin worker runtime settings

`platform.worker` controls plugin-contributed event consumers and scheduled
jobs (see [ADR 0009](../adr/0009-plugin-worker-contributions.md)).
`tenant_database_url` names the RLS-bound `platform_app` role used for tenant
transactions, while `platform.database.url` keeps the `platform_worker` role
for bounded cross-tenant routines. Compose sets
`PLATFORM__PLATFORM__WORKER__TENANT_DATABASE_URL`. If a linked plugin
contributes background work and this URL is absent, the worker reports
`plugin_runtime: unavailable` and stays not-ready rather than skipping work.
The remaining keys bound reconciliation and polling intervals, job batches,
leases and retry backoff, and consumer acknowledgement and retry timing. A job
lease must be longer than `platform.database.statement_timeout_ms`. The worker also runs background and scheduled automations (ADR 0022 §6)
and bulk jobs (ADR 0023 §2) through it; without it, those jobs wait and the worker logs that they will
not run.

## Push notifications

`platform.devices.push` configures the worker's "workspace changed"
notifications to device apps (ADR 0030). Each carries only the workspace's
id. With neither provider configured, the worker sends none and devices sync
on their own schedule.

- `quiet_seconds` (default 180, at most 3,600): the least time between two
  notifications to one device, so a burst of changes costs one.
- `apns`: `key_file` (the `.p8` signing key), `key_id`, `team_id`, `topic`
  (the app's bundle id) and `sandbox` (Apple's development environment).
- `fcm`: `project_id` and `service_account_file` (the service account's
  JSON key).

Keys stay in files the worker reads at startup, mounted as deployment
secrets; configuration names only their paths. A file that cannot be read
or parsed stops the worker. Notifications are recorded through
`platform.worker.tenant_database_url`; without it, none are sent.

Production configuration must use HTTPS issuer/redirect URLs, `tls://` NATS
transport, and secure cookies. `SameSite=None` is rejected unless
`cookie_secure` is true. Session identifiers and OIDC tokens must never be
placed in YAML, logs, URLs, or error responses; use the browser-managed
HTTP-only cookie and deployment secret injection for confidential client
credentials.
