# Local identity fixture

Local development uses a pinned Keycloak container with the `local` realm
imported from `infra/keycloak/realm-local.json`. Keycloak is a disposable test
fixture only. Application identity settings remain provider-neutral, and a
production deployment must provide its own OIDC issuer and credentials.

Start or inspect the provider:

```bash
docker compose up --detach --wait keycloak
docker compose logs keycloak
```

The local issuer is `http://auth.localhost:8081/realms/local`. The
`auth.localhost` name resolves to loopback in browsers and is also a Compose
network alias, so front-channel and back-channel issuer validation agree. The
administrative console is at `http://auth.localhost:8081/admin/` with
`local-admin` / `local-admin`.
These checked-in credentials are deliberately public and must never be reused
outside an isolated development environment.

Two enabled identities support membership and denial tests:

| Username | Password | Intended fixture role |
| --- | --- | --- |
| `member` | `local-password` | User with a seeded workspace membership |
| `nonmember` | `local-password` | Authenticated user without membership |

The public `platform-web` client enables the authorization-code flow, requires
PKCE S256, and disables direct password grants and service accounts. Start a
login at `http://localhost:8080/auth/oidc/login`. The API validates state,
nonce, issuer, audience, signature, token timing, and PKCE before linking the
external identity and issuing a server-side session cookie.

The seeded `member` identity is linked to the local workspace and administrator
role. `nonmember` is created or linked on first login but intentionally has no
workspace membership. Log out with `POST http://localhost:8080/auth/logout`;
this revokes the durable application session and expires the HTTP-only cookie.

Realm import occurs only when the named Keycloak data volume is empty. Reset the
fixture, including any changes made through the admin console, with:

```bash
docker compose down --volumes
docker compose up --detach --wait keycloak
```

Run the executable fixture check with `bash scripts/test-keycloak-fixture.sh`.
It verifies readiness, discovery metadata, authorization-code entry, client
security settings, callback registration, and both seeded users.
