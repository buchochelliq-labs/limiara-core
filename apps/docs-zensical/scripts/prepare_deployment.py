"""Prepares the built Zensical site for GitHub Pages.

Run after `build.sh`, with the deployment environment and the commit
that was built:

    python3 scripts/prepare_deployment.py production <sha>

The Zensical site is under evaluation next to the VitePress site, which
is the canonical one (ADR 0008). So every deployment of it asks not to
be indexed. GitHub Pages sends no custom headers and serves the site
under `/<repository>/`, where a `robots.txt` is not read, so:

- every HTML page gets `<meta name="robots" content="noindex, nofollow">`
  in its `<head>`; a page without a `<head>` fails the preparation;
- `robots.txt` (copied from `docs/public`) is removed: it is not read
  below a host's root and names the canonical site's sitemap. The
  site's own `sitemap.xml` stays, since the theme loads it for instant
  navigation; with `ZENSICAL_SITE_URL` set it lists the Pages URLs,
  each of which is marked noindex;
- `deployment.json` says which site, environment and commit it is.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent.parent / "build" / "site"
ENVIRONMENTS = {"local", "production"}
NOINDEX = '<meta name="robots" content="noindex, nofollow">'


def mark_noindex(page: Path) -> None:
    html = page.read_text(encoding="utf-8")
    if NOINDEX in html:
        return
    marked, count = re.subn(r"<head(\s[^>]*)?>", lambda match: match[0] + NOINDEX, html, count=1)
    if count != 1:
        raise ValueError(f"{page.relative_to(SITE)} has no <head>")
    page.write_text(marked, encoding="utf-8")


def prepare(site: Path, environment: str, revision: str) -> int:
    if environment not in ENVIRONMENTS:
        raise ValueError(f"unsupported deployment environment: {environment}")
    if not (site / "index.html").is_file():
        raise ValueError(f"{site} has no index.html; build the site first")
    pages = sorted(site.rglob("*.html"))
    for page in pages:
        mark_noindex(page)
    (site / "robots.txt").unlink(missing_ok=True)
    deployment = {
        "schemaVersion": 1,
        "site": "zensical",
        "host": "github-pages",
        "environment": environment,
        "sourceRevision": revision,
        "indexable": False,
        "portableStaticSite": True,
        "analyticsEnabled": False,
        "feedbackEnabled": False,
    }
    (site / "deployment.json").write_text(json.dumps(deployment, indent=2) + "\n", encoding="utf-8")
    return len(pages)


def main(arguments: list[str]) -> int:
    if len(arguments) != 2:
        print("usage: prepare_deployment.py <local|production> <revision>", file=sys.stderr)
        return 2
    try:
        count = prepare(SITE, arguments[0], arguments[1])
    except ValueError as error:
        print(f"prepare: {error}", file=sys.stderr)
        return 1
    print(f"Prepared {count} pages for {arguments[0]} at {arguments[1]} (noindex)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
