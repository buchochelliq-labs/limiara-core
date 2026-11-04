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

## Release verification

For preview and production, verify the home page, a clean article URL, the custom 404, `/getting-started`, `/api`, `sitemap.xml`, `robots.txt`, `_headers` behavior, and `deployment.json`. Assets use immutable caching; HTML revalidates; the sitemap has a one-hour cache. A rollback must therefore replace HTML immediately while content-addressed assets remain safely cached.

Locally, reproduce the release artifact and serve it without Cloudflare:

```sh
pnpm install --frozen-lockfile
DOCS_DEPLOYMENT_ENV=production DOCS_SOURCE_REVISION=<commit> pnpm --filter docs build
node apps/docs/scripts/verify-portable-artifact.mjs
node apps/docs/scripts/verify-portable-artifact.mjs --serve
```

The third command performs an independent HTTP smoke test; the fourth exposes the artifact on `127.0.0.1:4175`. CI retains each successful publish artifact for 90 days. Download and extract `docs-static-site-<sha>` to serve the same directory with any conventional static host.

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
