# Documentation quality gates

Issue #127 makes the repository build, public API boundary, links, terminology, sensitive-content policy, and representative rendered behavior executable.

## Run the complete gate

Install locked dependencies and the Chromium test runtime once, then run one workspace command:

```sh
pnpm install --frozen-lockfile
pnpm exec playwright install chromium
pnpm docs:check
```

`pnpm docs:check` runs checker unit tests, validates all documentation sources and the public OpenAPI document, builds the production site, scans the output, exercises representative desktop and 320-CSS-pixel reflow scenarios, and checks external links with bounded retries.

## Source and style policy

- Every ordinary Markdown page has exactly one level-one heading. The VitePress home page supplies its heading through home-layout frontmatter.
- Images require useful alternative text. Vendor-specific GitBook blocks and trailing whitespace are rejected.
- Preferred terminology is stored in `apps/docs/policies/terminology.json`; additions should correct a recurring ambiguity rather than encode personal prose preferences.
- CSpell checks en-GB prose offline. Project and product names belong in `apps/docs/policies/words.txt`; do not add ordinary misspellings merely to silence the gate.
- Relative links must resolve in the repository. External links receive bounded attempts with the timeout, backoff, concurrency, release-critical patterns, and documented exceptions in `apps/docs/policies/external-links.json`.
- Stable external failures such as HTTP 404 fail required PR checks. Transient DNS, TLS, socket, timeout, rate-limit, and server failures are reported with their source page but do not make unrelated PRs flaky.
- A scheduled or manually dispatched live-link audit uses strict transient handling. It reports third-party availability separately from required PR checks.
- Add an `ignored` entry only when automation cannot access a reviewed link, and include a reason. Add a `critical` URL pattern when temporary unavailability must fail both PR and live audits. Diagnostics strip query strings and fragments so secrets cannot be printed.
- Likely private keys, provider tokens, bearer tokens, embedded URL credentials, and non-placeholder client secrets are rejected without echoing their values in diagnostics.

## Public API boundary

`docs/api/openapi.json` is a public specification. Every operation must have a unique `operationId`, a successful response, and `x-limiara-visibility: public`. Readiness, metrics, admin, debug, and internal routes are prohibited. The human-readable API reference must include every public path and operation ID.

An operation must move into a separate internal specification instead of weakening this policy. Interactive requests remain disabled until authentication, CORS, rate limits, and abuse controls are approved.

## Rendered baselines

Chromium checks representative home, search, API, article/navigation, and 404 behavior at desktop and 320 CSS pixels—the reflow width equivalent to zooming a 1280-pixel viewport to 400%. They cover skip-link focus, main/navigation landmarks, local search, mobile navigation, colour schemes, reduced-motion CSS, heading structure, and horizontal overflow.

These checks are a regression floor, not a complete accessibility audit. New component families need focused keyboard and assistive-technology review before publication.

## CI artifacts and failures

The `docs-quality` job uploads the already-scanned static output for seven days only after every gate passes. The separate `docs-publish-build` job prepares an environment-labelled portable artifact, verifies it through an independent Node HTTP server, and retains it for 90 days. Deployment remains disabled until the protected GitHub environments and scoped Cloudflare credentials in the [publishing runbook](../operations/docs-publishing.md) are approved.

CI diagnostics name the source page or API operation. It does not upload browser traces or reports by default because future documentation may contain unreleased material.
