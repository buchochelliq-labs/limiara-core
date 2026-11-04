# Contributing documentation

Documentation is reviewed as product behavior. The Markdown, API descriptions, images, redirects, inventory, ownership rules, and site configuration in this repository are canonical; a hosted editor must not become the source of truth.

## Choose the audience and status first

Every Markdown page must have an entry in `docs/inventory.json`. Select its audience, publication decision, product status, owner, edition, versioning policy, sensitivity, and relationship to other pages before writing prose.

- **Shipped** describes behavior covered by executable evidence in the repository.
- **Experimental** describes a runnable proof or unstable interface that may change without compatibility guarantees.
- **Planned** describes a decision or target that is not delivered. Use future tense and link to the roadmap or issue.
- **Mixed** is reserved for pages that deliberately compare delivered and planned behavior and must distinguish them locally.
- **Historical** preserves decision or research context and is not current product guidance.

Commercial-only behavior must be labelled `commercial`; content that applies to both editions uses `both`. Version-specific guidance goes under a release path and records the release in the inventory. Internal or security-sensitive operational material is excluded from the public build, not hidden by an unlinked URL.

## Voice and terminology

Write concise en-GB prose for the reader's task. Lead with current behavior, state prerequisites, and distinguish a proof, design, or future direction from shipped behavior. Prefer stable platform terms from the architecture and avoid marketing claims, unexplained acronyms, and competitor language as Limiara terminology.

The terminology policy is in `apps/docs/policies/terminology.json`; valid project words are in `apps/docs/policies/words.txt`. Add a project word only when it is a proper name or established technical term. Do not add an ordinary misspelling to silence CSpell.

## Commands and code samples

- Use commands that run from a fresh checkout and state the required working directory.
- Pin or derive tool versions from repository manifests; do not imply `latest` is reproducible.
- Use placeholders such as `<workspace-id>` and `${CLIENT_SECRET}`. Never include real credentials, tokens, tenant identifiers, or customer data.
- Show expected outcomes when success is otherwise ambiguous, and test important snippets where practical.
- Mark pseudocode and incomplete examples explicitly. Generated API reference content must come from checked-in specifications.

## Screenshots and diagrams

- Capture only seeded or synthetic data. Remove browser profiles, account names, local paths, tokens, and unrelated desktop chrome.
- Include useful alternative text that explains the evidence, not merely the colours or filename.
- Prefer a desktop view near 1280 CSS pixels and a mobile/reflow view at 390 CSS pixels when layout behavior matters.
- Store source assets beside the owning page, compress raster images, and prefer accessible SVG for diagrams.
- Record the feature state and owning issue in surrounding prose. Refresh an image when the interaction, theme, labels, or accessibility state changes materially.
- Do not use screenshots as the only source for commands, errors, or required text.

## Accessibility and localization

Use one level-one heading, a logical heading hierarchy, descriptive link text, labelled tables, language-neutral examples, and captions or text equivalents for complex diagrams. Do not encode meaning through colour alone. New interactions require keyboard, focus, screen-reader landmark, reduced-motion, zoom/reflow, and narrow-screen evidence.

English (`en-GB`) is the initial source locale. Keep headings stable for links, avoid text embedded in images, expand acronyms on first use, avoid string fragments that cannot be translated independently, and keep locale-specific screenshots replaceable. Translation is deferred until a release and ownership model exists; machine-generated translations must not publish without human review.

## Deprecation, redirects, and versions

Do not silently delete or rename a public page. Mark deprecated guidance, name its replacement and removal release, add a permanent redirect in `docs/public/_redirects`, and update inbound links in the same change. Temporary redirects require a documented expiry.

Current documentation describes the current supported development line. When the first compatibility-stable release ships, release snapshots are generated from tags under `/vX.Y/`; fixes to a supported snapshot are backported deliberately. Do not copy the entire tree by hand. Release notes identify breaking changes, deprecations, migrations, and documentation version availability.

## Ownership and review

`docs/owners.json` maps owner IDs to scopes and review expectations. Changes touching security design, public API/events, operations, commercial boundaries, or publication status require the specialist review recorded there in addition to the page owner. A page without a valid owner or inventory record fails the documentation gate.

## Validate the change

Install the pinned browser once and run the complete local gate:

```sh
pnpm install --frozen-lockfile
pnpm exec playwright install chromium
pnpm docs:check
```

See [documentation quality gates](../guide/documentation-quality.md) for individual checks and failure policy.
