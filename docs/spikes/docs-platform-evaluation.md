# Documentation platform evaluation

## Outcome

Select **VitePress 1.6.4** with repository Markdown as the source of truth. Use Cloudflare Pages as the initial static deployment target, subject to a separate access and deployment setup review. Do not connect GitBook to the production repository.

## Mandatory criteria matrix

| Criterion | GitBook | VitePress + Cloudflare Pages | Docusaurus + static host |
| --- | --- | --- | --- |
| Docs as code | Bidirectional Git Sync; hosted edits can commit | Native: repository Markdown is input | Native: repository Markdown/MDX is input |
| Pull-request preview | Managed preview URLs | Pages preview per in-repository PR/branch | Host-dependent; Pages can provide it |
| Custom domain and redirects | Premium tier | Host custom domain; repo `_redirects` | Host custom domain; official redirects plugin |
| SEO and sitemap | Managed | Built-in sitemap, metadata, clean URLs | Official sitemap plugin |
| Responsive, accessible navigation/search | Managed theme and AI search on paid tier | Responsive default theme and local full-text search | Responsive theme; search needs a provider/plugin |
| Repository OpenAPI | Rich upload/URL/CLI blocks | Repository spec plus generated/static renderer | Repository spec plus generated/plugin renderer |
| Versioning | Site-space structure and plan features | Release-tag snapshots by policy | First-class docs versions |
| Export and backups | Markdown export depends on Git Sync; PDF is paid | Git history, Markdown, and static artifact | Git history, Markdown/MDX, and static artifact |
| Runtime ownership | Proprietary SaaS | Local open-source build; portable static files | Local open-source build; portable static files |
| Initial service cost | Premium required for mandatory features | $0 static assets within Pages free limits | $0 static assets within Pages free limits |

## Bounded proof

The proof builds the existing `docs/` tree directly; it does not copy production content into an app-specific directory. It adds:

- a responsive home and getting-started journey;
- keyboard-accessible default navigation, theme switching, and local search;
- a repository-owned OpenAPI 3.0 example and static reference page;
- an accessible SVG architecture diagram;
- custom-domain sitemap metadata and host-level redirects;
- source checks for broken relative links, vendor-specific GitBook markup, and OpenAPI/reference drift.

Run it with:

```sh
pnpm --filter docs test
pnpm --filter docs build
pnpm --filter docs dev
```

The equivalent GitBook import bundle is the same ordinary Markdown and OpenAPI source. A live GitBook workspace was deliberately not connected: the issue explicitly prohibits granting production repository write access without approval, and GitBook was already eliminated by mandatory-feature cost and repository-authority criteria. Official Git Sync documentation confirms that GitBook edits can be merged into commits on the selected branch.

## Cost and operational review

Pricing was checked on 2026-07-16. [GitBook pricing](https://www.gitbook.com/pricing) lists Free at $0 with Git Sync and preview deployments, but custom domains, redirects, PDF export, and advanced branding start at Premium: $65 per site per month plus $12 per user per month when billed annually. Costs grow with editors and the public site remains SaaS-hosted.

VitePress is MIT-licensed and has no service fee. [Cloudflare Pages limits](https://developers.cloudflare.com/pages/platform/limits/) currently include 500 builds per month, one concurrent build, 20,000 files, and up to 100 custom domains on the Free plan. [Static asset requests are free and unlimited](https://developers.cloudflare.com/pages/functions/pricing/); this proof uses no Pages Functions. Domain registration is the only expected mandatory external cost. Build minutes and limits must be reviewed if traffic or contributor activity changes materially.

Docusaurus is also open source and has the same broad static-host cost profile, but would add more theme/plugin dependencies and maintenance than the current site needs.

## Security and privacy review

### GitBook

[Git Sync is bidirectional](https://gitbook.com/docs/integrations/git-sync), and enabling GitHub Sync installs a GitHub App scoped to selected or all repositories. That is convenient but introduces a hosted content editor with commit authority. GitBook's [content export guidance](https://gitbook.com/docs/help-center/editing-content/managing-your-content) says Markdown export uses Git Sync and warns that custom blocks may export as HTML. Its [privacy statement](https://gitbook.com/docs/policies/privacy-and-security/statement) describes processing service and visitor data in the United States and through subprocessors; public content can be indexed or scraped. GitBook states in its [AI policy](https://gitbook.com/docs/policies/privacy-and-security/security/ai-policy) that customer data is not shared with OpenAI or used to train AI models. These are manageable SaaS risks, but unnecessary for the current contributor model.

### VitePress and Cloudflare Pages

The build has no runtime database, identity, cookies, or server-side functions. Local search keeps the search index in the static bundle. Cloudflare will necessarily process visitor IP and request metadata as the hosting/CDN provider; a public privacy notice and retention/configuration review are required before analytics or additional scripts are added.

[Cloudflare's GitHub integration](https://developers.cloudflare.com/pages/configuration/git-integration/github-integration/) can read source and publish check/deployment state. Production setup must scope the integration to this repository, prohibit arbitrary build secrets, protect the production branch, and document account offboarding. [Preview deployments](https://developers.cloudflare.com/pages/configuration/preview-deployments/) are public by default but carry `X-Robots-Tag: noindex`; Cloudflare Access can restrict them if documentation starts containing pre-release confidential material. Fork pull requests do not receive automatic previews, so CI must still build them without deployment secrets.

No secrets, private endpoint descriptions, customer data, workspace identifiers, or internal readiness state may be included in public documentation. Interactive API execution remains disabled until authentication, CORS, abuse controls, and example-data handling are designed.

## Quality and portability gates

- **Broken links:** the repository checker validates relative Markdown targets; VitePress fails unresolved site links during build.
- **Accessibility/mobile:** the default theme provides a skip link, semantic navigation, responsive sidebar, keyboard focus, and colour-scheme control. Custom styles retain visible focus and responsive layout. Browser-level checks should be added to the deployment issue for regression coverage.
- **Performance:** static output has no application backend or client analytics. The deployment slice should record Lighthouse mobile accessibility and performance scores and set budgets before launch.
- **Export:** canonical Markdown, OpenAPI JSON, SVG, redirects, and configuration are all in Git. A fresh clone rebuilds the site; no proprietary block has to be transformed.
- **Recovery:** pin dependencies in `pnpm-lock.yaml`; retain successful production output as an artifact; a different static host can serve the same `apps/docs/.vitepress/dist` directory.

### Proof results

Recorded locally on 2026-07-16 against the production build:

- all 43 Markdown files and the public OpenAPI path passed the repository source checker;
- VitePress built every page and generated the sitemap with no unresolved site link;
- desktop rendering had one H1, no horizontal overflow, a skip target, labelled search, labelled theme control, and labelled primary navigation;
- local search returned relevant repository pages for `metadata` and exposed keyboard navigation instructions;
- at a 375-by-844 mobile viewport, desktop navigation was hidden, the labelled mobile menu expanded, and the page had no horizontal overflow;
- the colour-scheme switch applied dark mode, and the API page retained a `main` landmark and accessible heading hierarchy;
- the custom home layout is assigned a `main` role by the theme so its skip link has a landmark target.
- the output contained 44 HTML pages and 156 files totalling 2,348,800 bytes; the largest asset was the 208,466-byte local-search index. These are baseline transfer/build metrics, not Lighthouse scores.

## Primary product evidence

- [VitePress site configuration](https://vitepress.dev/reference/site-config)
- [VitePress local search](https://vitepress.dev/reference/default-theme-search)
- [VitePress sitemap generation](https://vitepress.dev/guide/sitemap-generation)
- [VitePress deployment guide](https://vitepress.dev/guide/deploy)
- [Docusaurus documentation introduction](https://docusaurus.io/docs/docs-introduction)
- [Docusaurus official plugins](https://docusaurus.io/docs/api/plugins)
- [GitBook Git Sync](https://gitbook.com/docs/integrations/git-sync)
- [GitBook OpenAPI support](https://gitbook.com/docs/api-references/openapi)
- [GitBook pricing](https://www.gitbook.com/pricing)
- [Cloudflare Pages preview deployments](https://developers.cloudflare.com/pages/configuration/preview-deployments/)
