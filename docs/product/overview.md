# Product overview

Limiara is an open, composable business platform for customer relationship management and operational workflows. Its community data plane remains self-hostable; domains such as Contacts, field service, automation, analytics, and governed intelligence are delivered through versioned plugin contracts rather than privileged kernel modules.

## What exists today

The repository contains a runnable React demonstration, Rust service scaffolds, database migrations, plugin contracts, metadata-driven UI proofs, architecture decisions, and executable documentation gates. The [roadmap](../roadmap/README.md) is authoritative about which behavior is delivered, being built, or planned.

It is not yet a production-ready CRM. Durable authentication, database-backed Contacts behavior, complete plugin lifecycle orchestration, and the Gate 1 operational baseline remain active work.

## Design principles

- Keep the kernel domain-neutral and make business capabilities plugins.
- Preserve a self-hostable community data plane.
- Treat tenant isolation, authorization, audit, and plugin trust as release gates.
- Keep API, metadata, event, and UI extension contracts explicit and versioned.
- Prefer executable evidence over roadmap wording when describing delivery status.

Start with the [local development guide](../guide/getting-started.md), then use the [open-core boundary](../architecture/open-core.md) and [plugin guide](../plugins/native-plugin.md) to understand the extension model.
