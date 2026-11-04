# GitHub Copilot instructions for Limiara

Limiara is a rebrandable, open-core, plugin-native customer operations platform. The codebase should remain a modular monolith first, with explicit boundaries that can later be extracted into services.

## Architecture rules

- Keep the kernel domain-neutral. Do not put contacts, leads, deals, jobs, visits, assets, quotes, invoices, or workflows into platform kernel crates.
- Implement business capabilities as plugins with manifests, permissions, capabilities, migrations, events, lifecycle state, and UI contributions.
- Keep crate names, schemas, API paths, container names, and infrastructure names brand-neutral when possible.
- Use `/api/v1` for REST APIs; do not place the provisional brand in API paths.
- Use PostgreSQL as the transactional source of truth and NATS JetStream for durable event/work delivery in the first vertical slice.

## Security rules

- User permissions, plugin runtime capabilities, and commercial entitlements are separate concepts.
- Never use entitlements as authorization decisions.
- Never hand plugins raw database DSNs, unrestricted object-store credentials, ambient filesystem/environment access, or unrestricted network access.
- Tenant-scoped tables require `tenant_id`, tenant-first indexes, and forced PostgreSQL RLS.
- Application code should establish tenant context with `SET LOCAL app.tenant_id = ...` only after membership verification.
- Browser auth should be OIDC authorization code + PKCE with backend-managed HttpOnly sessions; do not store access tokens in local storage.
- Do not load arbitrary third-party plugin JavaScript into the authenticated web origin.
- Wasm plugins are default-deny and must run with fuel/time/memory/output limits.

## Rust style

- Keep `#![forbid(unsafe_code)]` unless a future ADR explicitly justifies an exception.
- Avoid `unwrap` and `expect` in production code; tests and compile-time-infallible setup are acceptable.
- Prefer typed errors and stable error codes.
- Avoid circular crate dependencies.
- Keep domain types inside their plugin crate.

## Frontend style

- Use strict TypeScript.
- The shell should consume UI contributions from the backend and not hardcode first-party plugins into navigation.
- Render plugin data safely as data, not executable code.
- Include accessible labels, validation states, loading states, permission-denied states, and conflict states.

## Validation

When possible, run:

```bash
cargo build --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm build
```

If dependency registries are unavailable, report the exact command and blocker.
