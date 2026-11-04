# Releases and documentation versions

Limiara has not published a compatibility-stable release. The repository and documentation describe a changing development baseline; no current page should be interpreted as a long-term support promise.

## First-release policy

When the first supported release is tagged:

1. publish release notes with breaking changes, migrations, deprecations, and security-relevant operator actions;
2. generate a documentation snapshot from the same tag under `/vX.Y/`;
3. keep `/` on the current supported release and expose an explicit version selector;
4. backport critical documentation fixes to supported snapshots without silently changing their product behavior;
5. redirect removed current-version pages only when the replacement is semantically equivalent.

Release artifacts and documentation builds must remain reproducible from the tag and locked dependencies. The [documentation platform ADR](../adr/0008-vitepress-documentation-platform.md) records the hosting-independent approach.
