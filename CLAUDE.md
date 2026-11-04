# Claude contributor guide for Limiara

This repository is the beginning of Limiara, a rebrandable, open-core, plugin-native customer operations platform. The internal architecture is currently called Trameia, but crate names, schemas, API paths, and infrastructure identifiers should stay brand-neutral.

## Non-negotiable boundaries

- Treat the kernel as domain-neutral platform machinery only.
- Implement business capabilities as plugins, including first-party capabilities such as Contacts.
- Do not let plugin code bypass tenancy, permissions, audit, lifecycle, capabilities, or event policy.
- Keep user permissions, plugin runtime capabilities, and commercial entitlements separate.
- Do not add a password system; browser authentication is intended to use OIDC authorization code + PKCE with backend-managed sessions.
- Do not load arbitrary third-party JavaScript in the authenticated web origin.
- Do not give Wasm or external service plugins raw database credentials, raw object-store credentials, ambient filesystem access, ambient environment access, or unrestricted network access.

## Rust conventions

- Run `cargo build --workspace --locked`, `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked` when dependency access is available.
- Keep `#![forbid(unsafe_code)]` in crates unless a future ADR explicitly justifies otherwise.
- Avoid `unwrap` and `expect` in production paths. Tests and provably infallible compile-time setup are acceptable.
- Prefer typed errors and stable API error codes over stringly ad-hoc errors.
- Keep domain types inside plugin crates.
- Avoid circular crate dependencies.

## TypeScript/frontend conventions

- Keep TypeScript strict.
- The app shell must discover navigation from the UI contribution endpoint rather than hardcoding a business plugin.
- Render untrusted plugin data as inert data, not executable code.
- Use accessible form labels, error states, loading states, and permission-denied states.

## Database and tenancy

- Every tenant-scoped table must include `tenant_id`.
- Use tenant-first indexes for tenant-scoped reads.
- Enable and force PostgreSQL RLS for tenant-scoped plugin tables.
- Application code should set tenant context transaction-locally via `app.tenant_id` after verifying membership.
- Cross-plugin interaction should use public plugin APIs, versioned events, governed read models, or kernel metadata services, not private table reads.

## Documentation expectations

- Record architecture choices in ADRs when they change platform boundaries.
- Keep deferred work in `docs/roadmap/` or an explicit follow-up section; do not leave silent TODOs in production paths.
- If a validation command cannot run because of environment limits, record the exact command and the observed blocker.
- Validate frontend changes with `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test`, and `pnpm build`.
