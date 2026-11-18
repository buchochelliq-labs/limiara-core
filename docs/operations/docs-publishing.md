# Documentation publishing and recovery

The repository builds a portable VitePress artifact before Cloudflare sees it. Cloudflare Pages is a replaceable static host, not the content authority. This runbook covers the controls introduced by issue #129 and the maintainer actions required to activate them.

## Current activation status

Repository build, preview policy, artifact retention, portable serving, redirects, headers, sitemap, social metadata, and 404 verification are implemented. Cloudflare project creation, GitHub environment approval, branch protection, and `docs.limiara.org` DNS remain pending maintainer approval. Until those controls exist, leave the repository variable `DOCS_DEPLOY_ENABLED` unset or set to `false`; `docs-publish-build` still proves the deployable artifact without contacting Cloudflare.

Analytics and feedback collection are disabled. Do not add either until a recorded privacy and data-flow review defines purpose, lawful basis, processor, fields, retention, consent where needed, deletion, and user notice.

## Hosting and access model

Use Cloudflare Pages Direct Upload. Create a custom Cloudflare API token with only account-level `Cloudflare Pages: Edit`, an expiry date, and no DNS permission. Cloudflare currently cannot restrict that permission to one Pages project, so use separate preview and production tokens and treat the remaining account scope as an accepted, reviewable limitation. Prefer account-owned service tokens when the account supports them so access is not tied to one maintainer.

Create these GitHub environments:

| Environment | Protection | Secrets | Variable |
| --- | --- | --- | --- |
| `docs-preview` | no production approval; trusted in-repository pull requests only | `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` | `CLOUDFLARE_PAGES_PROJECT` |
| `docs-production` | required maintainer approval; production branch only | a different `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` | `CLOUDFLARE_PAGES_PROJECT` |

The workflow never uses `pull_request_target`, and the preview deploy job rejects fork pull requests. Forks still receive the secret-free `docs-publish-build` and normal `docs-quality` checks. Protect `main` and require `docs-quality` plus `docs-publish-build`; add the deployment check only after it is stable.

## Activate Pages

1. Create the Direct Upload Pages project and record the generated `pages.dev` domain.
2. Configure the two GitHub environments and their distinct tokens.
3. Run the workflow manually with `deployment_environment=preview` and a reviewed `source_ref`.
4. Confirm `deployment.json` says `preview`, `robots.txt` disallows `/`, and the HTML contains `noindex, nofollow`.
5. Set the repository variable `DOCS_DEPLOY_ENABLED=true`.
6. Merge through protected `main`, approve `docs-production`, and verify the generated Pages domain.
7. Add `docs.limiara.org` only after DNS and brand approval. The canonical sitemap and social URL already target that domain, so do not announce it before DNS works.

## The Zensical site

The Zensical build of the same pages (`apps/docs-zensical`, under evaluation in [ADR 0008](../adr/0008-vitepress-documentation-platform.md)) is published to this repository's GitHub Pages site, `https://buchochelliq-labs.github.io/limiara-core/`, by its own workflow, `docs-zensical-deploy`. It needs no Cloudflare project, token or secret.

- **Pull requests** run `docs-zensical-publish-build`: the strict build, the link check and the deployment preparation, with read-only permissions and nothing deployed. GitHub Pages has no per-pull-request previews; download the `github-pages` artifact from the run to look at a pull request's site.
- **Pushes to `main`** that touch `docs/` or `apps/docs-zensical/` build and deploy through the `github-pages` environment, with `pages: write` and `id-token: write` given to the deploy job only.
- **A manual run** builds and deploys `source_ref`, which is how to roll back: run it with a known-good commit SHA. `deployment.json` and the artifact always record the commit that was built.

The site is never indexed. GitHub Pages sends no custom headers and serves the site under `/limiara-core/`, where `robots.txt` is not read, so `scripts/prepare_deployment.py` adds `<meta name="robots" content="noindex, nofollow">` to every page's `<head>`, removes `robots.txt`, and writes `deployment.json`; the sitemap stays because the theme's instant navigation loads it, and it lists only the Pages URLs. The build sets `ZENSICAL_SITE_URL` to the Pages address, so the 404 page and the canonical links carry the `/limiara-core/` prefix. Do not give the Pages site a custom domain while it is under evaluation.

To activate it, once: in **Settings → Pages**, set **Source** to **GitHub Actions**. Until then a deploy fails at its first step and nothing is published. Then run `docs-zensical-deploy` manually with `source_ref=main`, open the Pages address, and check that `deployment.json` says `zensical`, `github-pages` and the commit, and that a page's source carries the `noindex` tag.

To take it down, turn GitHub Pages off in **Settings → Pages**; to stop deploying without taking it down, disable the `docs-zensical-publish` workflow under **Actions**.

## Release verification

For preview and production, verify the home page, a clean article URL, the custom 404, `/getting-started`, `/api`, `sitemap.xml`, `robots.txt`, `_headers` behavior, and `deployment.json`. Assets use immutable caching; HTML revalidates; the sitemap has a one-hour cache. A rollback must therefore replace HTML immediately while content-addressed assets remain safely cached.

Locally, reproduce the release artifact and serve it without Cloudflare:

```sh
pnpm install --frozen-lockfile
DOCS_DEPLOYMENT_ENV=production DOCS_SOURCE_REVISION=<commit> pnpm --filter docs build
node apps/docs/scripts/verify-portable-artifact.mjs
node apps/docs/scripts/verify-portable-artifact.mjs --serve
```

The third command performs an independent HTTP smoke test; the fourth exposes the artifact on `127.0.0.1:4175`. CI retains each successful publish artifact for 90 days. Download and extract `docs-static-site-<sha>` to serve the same directory with any conventional static host. The `<sha>` in an artifact's name and `sourceRevision` in its `deployment.json` are always the commit that was built, even when a manual run named a branch or tag as `source_ref`; both deploy workflows resolve it after checkout.

## Rollback and clean rebuild

Find the last known-good production deployment SHA and its retained artifact. Prefer redeploying that exact artifact. If it is unavailable, dispatch `docs-publish` with `deployment_environment=production` and `source_ref=<known-good-sha>`; the protected production environment still requires approval. Confirm `deployment.json`, smoke-test the Pages URL, then record the failed and restored SHAs in the incident.

For a clean rebuild, use a fresh checkout of the selected SHA, Node 22, pnpm 9.12.0, and `pnpm install --frozen-lockfile`. Run `pnpm docs:check` before deployment. A lockfile mismatch or failed quality gate blocks publication rather than falling back to unlocked dependencies.

## Incident response

1. Disable `DOCS_DEPLOY_ENABLED` for a compromised workflow or token; revoke the affected Cloudflare token immediately.
2. Roll back to the last known-good artifact for bad content or broken navigation.
3. Remove DNS or serve the retained artifact from another static host for a Cloudflare outage that exceeds the agreed tolerance.
4. Preserve workflow, deployment, DNS, and token-audit evidence without copying secret values.
5. Rotate credentials, review repository/environment access, patch the cause, rebuild from a reviewed SHA, and document the recovery.

## Rotation, offboarding, and updates

Rotate each API token before expiry and after maintainer offboarding. Create and verify the replacement, update only the matching GitHub environment secret, run a preview or approved production deployment, then revoke the old token. Quarterly, review Cloudflare account members, GitHub environment reviewers, token age/permissions, Pages projects, custom domains, and deployment history.

Dependency updates use an ordinary pull request with locked versions and all docs gates. Review changes to VitePress, Wrangler, GitHub Actions, and generated output before merging. Do not give a hosted editor repository write access.

## Backup and provider exit

Git history is the primary source backup; the 90-day static artifact is the rapid-recovery copy. Periodically download a production artifact and execute the portable smoke test. To leave Cloudflare, deploy the unchanged artifact to another static host that supports clean URLs, redirects, headers, and a custom 404; verify the release checklist; lower DNS TTL; switch DNS; monitor; then revoke Cloudflare tokens and remove the Pages project only after the rollback window closes.
