# 0008: VitePress for repository-owned documentation

## Status

Accepted by spike #125 on 2026-07-16. Deployment remains a separate implementation slice.

## Context

Limiara needs public product, contributor, operator, and API documentation. Documentation must be version-controlled, previewable, accessible, searchable, exportable, and buildable without depending on the hosted Limiara control plane. A custom domain, redirects, sitemap, mobile navigation, and a repository-owned OpenAPI source are mandatory.

The spike compared GitBook, VitePress, and Docusaurus. The detailed evidence and cost/security review are in the [platform evaluation](../spikes/docs-platform-evaluation.md).

## Decision

Use VitePress 1.6.4 with Markdown under `docs/` as the canonical source. Build the static site in the pnpm workspace and deploy it to Cloudflare Pages after a separate deployment review.

- Pull requests build and validate the complete site. Cloudflare Pages can provide isolated, `noindex` preview deployments for repository branches.
- The production site will use `docs.limiara.org`, with redirects stored in `docs/public/_redirects` and a generated sitemap.
- Search uses VitePress local search initially, so public documentation is not sent to a hosted search index.
- The public OpenAPI description remains in the repository. Static reference pages are acceptable until authenticated interactive requests have an explicit threat model.
- Current-release documentation stays unversioned. Version snapshots begin only when Limiara makes a compatibility promise, and are generated from release tags under `/vX.Y/`.
- Source Markdown and Git history are the export, backup, and disaster-recovery format. Static build output may also be retained as a release artifact.

## Consequences

Documentation changes use the same review, provenance, and branch protection as code. The site is inexpensive to move between static hosts and remains fully buildable locally. Limiara owns accessibility and theme maintenance, and VitePress does not provide GitBook's visual editor, managed analytics, AI search, or polished interactive OpenAPI blocks.

Cloudflare Pages Git integration requires scoped repository access. Before production setup, maintainers must choose either the narrowly scoped GitHub App or a GitHub Actions deployment using a scoped Cloudflare API token, document secret rotation and offboarding, and enable branch protection. No hosted documentation or deployment service receives production-repository write access as part of this spike.

## Alternatives considered

### GitBook

GitBook has strong authoring, bidirectional Git Sync, pull-request previews, and an integrated OpenAPI experience. It was not selected because the mandatory custom domain, redirects, and PDF export require at least the paid Premium tier; Git Sync makes a hosted editor capable of committing to the selected branch; direct Markdown export relies on Git Sync; and GitBook-specific blocks can export as HTML. Those trade-offs are not justified while the contributor group is small and technical.

### Docusaurus

Docusaurus has mature documentation versioning and official sitemap and redirect plugins. It was not selected because its React application/plugin surface is larger than this documentation set requires, while neither first-class versioning nor a React theme ecosystem is presently a release need. It remains the preferred revisit candidate if complex multi-product or multi-version documentation becomes dominant.

## Revisit triggers

- Non-technical contributors cannot complete routine documentation changes with the GitHub editing flow.
- More than two supported documentation versions make generated snapshots difficult to navigate or maintain.
- Local search no longer meets relevance, language, or scale needs.
- API consumers require a supported interactive console and its authentication/CORS threat model is approved.
- Hosting reliability, preview access control, or build limits exceed the selected static host.
