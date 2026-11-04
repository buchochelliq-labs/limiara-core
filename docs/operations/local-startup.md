# Local startup and diagnostics

From a configured checkout, build and start the complete local stack with the
same command used by CI:

```bash
bash scripts/integration-stack.sh up
```

Compose starts PostgreSQL, NATS JetStream, and the local Keycloak fixture first.
The one-shot migration service must then finish successfully before the API and
worker are allowed to start. The web shell waits for the API health endpoint.
PostgreSQL, NATS, and Keycloak use named volumes so recreating runtime containers
does not remove durable local state.

Inspect service health and one-shot job state with:

```bash
docker compose ps --all
docker compose logs migrate api worker
```

PostgreSQL, NATS, Keycloak, API, worker, plugin runner, and web checks probe
their native or HTTP readiness. API and worker recheck PostgreSQL, the core
migration ledger, and NATS on every readiness request; API also checks plugin
registry initialization. The plugin runner reports ready only after its runtime
registry initializes. The worker binds its internal health and metrics listener
before broker setup and retries NATS and JetStream initialization with bounded
backoff. A dependency can therefore make a live process not-ready and later
recover without a restart. A failed migration remains visible as an exited
`migrate` container, and Compose will not start the dependent services.

PostgreSQL reports healthy only after first-run initialization has finished.
The image runs its init scripts against a temporary server that listens on the
Unix socket only, so `infra/postgres/healthcheck.sh` connects over TCP and
requires the `platform_migrator`, `platform_app`, and `platform_worker` roles.
A plain socket `pg_isready` can pass against the temporary server and let
`migrate` connect just before that server stops. The cold-start regression is
`bash scripts/test-postgres-readiness.sh`.

The web container is the browser's single origin (`http://localhost:3000`).
Its nginx configuration (`infra/web/default.conf.template`) serves the shell
for every route, so deep links such as `/contacts/people/{id}` load the app.
Missing hashed assets under `/assets/` still return 404. It proxies `/api/`
and `/auth/` to the API so the shell's relative API calls and the session
cookie stay same-origin. `API_UPSTREAM` (default `api:8080`) and
`NGINX_RESOLVER` (default `127.0.0.11`, Docker's embedded DNS) configure the
proxy. The API name is re-resolved, so restarting or recreating the API does
not require restarting the web container.

Health responses contain component names and `healthy`/`unavailable` status
only. Connection strings, credentials, tenant identifiers, and plugin data are
never returned.

Restart stateless runtime processes without recreating infrastructure:

```bash
docker compose restart api worker plugin-runner web
docker compose up --detach --wait
```

To reset every local durable fixture, including PostgreSQL, JetStream, and
Keycloak state, explicitly remove volumes:

```bash
docker compose down --volumes
```

Use `bash scripts/integration-stack.sh down` for normal teardown or `reset` to
delete local volumes. The [first boot and recovery runbook](./first-boot-and-recovery.md)
covers migration, identity, plugin, and health failures. Run the automated
acceptance path with `bash scripts/test-compose-startup.sh`.
