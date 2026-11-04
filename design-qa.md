# Design QA — issue #97 application shell

## Comparison target

- Source visual truth: `docs/product/assets/contacts-people-workspace-direction.png`
- Desktop implementation: `docs/product/assets/app-shell-desktop.jpg`
- Mobile implementation: `docs/product/assets/app-shell-mobile.jpg`
- Mobile navigation: `docs/product/assets/app-shell-mobile-drawer.jpg`
- Desktop viewport/state: 1280 × 800, Contacts contribution active, Northstar Services workspace, light theme.
- Mobile viewport/state: 390 × 844, same route/workspace/theme with drawer closed and open.

## Full-view comparison evidence

The source and final desktop capture were opened together in one comparison pass. The implementation deliberately stops at the shell/content-slot boundary: the People list and record preview in the source belong to #98 and #100, while issue #97’s content canvas demonstrates a contribution-owned destination.

- **Fonts and typography:** the implementation retains the source’s editorial Georgia display hierarchy and compact platform sans UI text. Heading weights, line height, and small uppercase module status preserve the same optical split without adding another font family.
- **Spacing and layout rhythm:** the 112px ink rail, 72px workspace bar, active-module band, thin separators, workspace/search/account alignment, and 32px content inset closely follow the source. The empty right canvas is expected at this issue boundary rather than a missing shell region.
- **Colors and visual tokens:** all surfaces use #96 semantic tokens: chalk canvas, ink rail/text, forest actions, sage active/focus accents, and restrained borders. No new feature-local palette was introduced.
- **Image quality and asset fidelity:** the Limiara mark is a project-bound raster asset derived from the selected source, placed on the same ink background. Phosphor supplies all interface icons; no CSS art, emoji, hand-authored SVG, or placeholder imagery appears.
- **Copy and content:** shell copy names the active contribution and explains the host/plugin boundary without pretending the Contacts feature screens are implemented. Fixture names match the selected Northstar direction.
- **Icons:** rail, workspace, search, help, account, action, and drawer icons use one consistent regular-stroke family, with the compact active state matching the source’s visual weight.

## Focused comparison evidence

The desktop full view resolves rail, workspace bar, page header, active state, icon weight, and primary-action details clearly. The separate mobile drawer capture is the focused navigation comparison: it verifies the generated brand asset, selected contribution, compact workspace control, backdrop, focus ring, and comfortably sized targets. The closed mobile capture verifies the page action remains above the fold and that content adapts rather than compressing the desktop rail.

## Interaction and accessibility evidence

- Contribution link navigation updates browser history, `aria-current`, document title, and focuses the new level-one heading.
- Workspace switching clears navigation, announces `Riverview Property Group loaded. 1 module available.`, and focuses the Contacts heading after the new contribution state resolves.
- Mobile drawer moves focus inside, traps Tab/Shift+Tab, closes with Escape, and restores focus to `Open navigation`.
- Account disclosure opens and closes with Escape while returning focus to its trigger.
- Active workspace is a labelled native select; global search is labelled; icon-only actions are labelled.
- 390px evidence reports `scrollWidth` equal to `innerWidth` with no horizontal page overflow.
- Browser console errors and warnings: none.

## Findings

No actionable P0/P1/P2 findings remain.

## Open questions

None for #97. Typed route registration, session expiry, and authoritative permission decisions remain explicitly coordinated with #33 and Gate 1 APIs.

## Comparison history

- Pass 1: the shell matched the source hierarchy, but programmatic heading focus appeared on initial load and the primary action wrapped more tightly than the source. Initial-load focus was suppressed while route/workspace focus was preserved, and the page-action layout now enforces a single row.
- Pass 2: the mobile drawer returned focus on Escape but did not yet keep Tab focus within its modal boundary. A keyboard focus loop was added for the drawer’s links, buttons, and workspace select.
- Pass 3: no actionable P0/P1/P2 differences remained in the issue #97 shell scope; the desktop, mobile, and drawer files above are the final evidence.

## Implementation checklist

- [x] Plugin contribution navigation with active-route semantics.
- [x] Stable workspace, search, page action, account, and host navigation regions.
- [x] Loading, empty, denied, error, and unavailable-route coverage.
- [x] Workspace invalidation, stale-response protection, announcements, and focus restoration.
- [x] Accessible mobile drawer and overflow-free responsive layouts.
- [x] Desktop/mobile visual evidence and interaction verification.

## Follow-up polish

- P3: replace the generated raster mark if a canonical brand-source file becomes available; the current asset matches the accepted direction but is not a long-term master logo.

final result: passed
