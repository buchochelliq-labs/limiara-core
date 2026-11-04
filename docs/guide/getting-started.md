# Get started

Limiara is a Rust and TypeScript monorepo. The current baseline uses Node.js 22, pnpm 9, Rust, Docker, and Docker Compose.

## Install dependencies

```sh
pnpm install --frozen-lockfile
cargo fetch --locked
```

## Run the web application

```sh
pnpm --filter web dev
```

The Vite development server prints the local URL. The current web application uses demonstration data while the durable Gate 1 platform services are completed.

## Run the documentation site

```sh
pnpm --filter docs dev
```

Before opening a pull request, run the same documentation checks used by the workspace:

```sh
pnpm --filter docs test
pnpm --filter docs build
```

## Understand the boundaries

The [roadmap](/roadmap/README) distinguishes implemented behavior from proofs and design contracts. The [open-core architecture](/architecture/open-core) explains what must remain available to a self-hosted community deployment.
