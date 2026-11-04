# Documentation workflow

Limiara documentation is docs-as-code. Markdown, images, redirects, and the public OpenAPI description are reviewed in the same pull requests as the behavior they describe.

![Documentation sources pass through checks and a static build before preview and production deployment.](/architecture-overview.svg)

## Authoring rules

- Keep canonical content under `docs/`; do not author production content in a hosted editor.
- Use ordinary Markdown and relative links where possible.
- Put public API operations in `docs/api/openapi.json`, then update the human-readable reference in the same change.
- Do not document internal-only endpoints, credentials, private tenant examples, or unreleased security behavior.
- Run `pnpm --filter docs test` and `pnpm --filter docs build` before review.

## Preview and release

The selected deployment shape is a repository-built static VitePress artifact uploaded to Cloudflare Pages. Pull requests from trusted repository branches receive isolated preview URLs; forks run the same secret-free build but do not deploy. The protected production environment publishes the approved branch. Preview output is generated with both a `noindex` meta directive and a disallowing robots policy.

The complete activation, release, rollback, credential, incident, and provider-exit procedure is in [Documentation publishing and recovery](../operations/docs-publishing.md).

## Versioning

The site documents the current supported release by default. When Limiara publishes its first compatibility-stable release, snapshots will be generated from release tags under `/vX.Y/`. We will not duplicate every page into version folders before compatibility promises exist.

## Export and recovery

The Git repository is the backup and export format. A clean checkout plus locked dependencies can rebuild the site without a hosted documentation account. Static output can also be archived as a release artifact for disaster recovery.
