"""Prepares the built Zensical site for Cloudflare Pages.

Run after `build.sh`, with the deployment environment and the source
revision:

    python3 scripts/prepare_deployment.py preview <sha>

The Zensical site is under evaluation next to the VitePress site, which
is the canonical one (ADR 0008). So every deployment of it, preview and
production alike, asks not to be indexed:

- `robots.txt` disallows everything;
- `_headers` sends `X-Robots-Tag: noindex, nofollow` on every response,
  with the same security headers as the VitePress site
  (`docs/public/_headers`), and caches only the fingerprinted theme
  assets for good;
- `sitemap.xml` is removed: it lists the canonical domain's URLs, which
  serve the VitePress site, not this one.

It also writes `deployment.json`, as the VitePress deployment does, so a
deployed site says what it is and which commit it was built from.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent.parent / "build" / "site"
ENVIRONMENTS = {"local", "preview", "production"}

HEADERS = """/*
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin
  X-Frame-Options: DENY
  X-Robots-Tag: noindex, nofollow
  Cache-Control: public, max-age=0, must-revalidate

/assets/stylesheets/*
  Cache-Control: public, max-age=31536000, immutable

/assets/javascripts/*
  Cache-Control: public, max-age=31536000, immutable
"""


def prepare(site: Path, environment: str, revision: str) -> None:
    if environment not in ENVIRONMENTS:
        raise ValueError(f"unsupported deployment environment: {environment}")
    if not (site / "index.html").is_file():
        raise ValueError(f"{site} has no index.html; build the site first")
    (site / "robots.txt").write_text("User-agent: *\nDisallow: /\n", encoding="utf-8")
    (site / "_headers").write_text(HEADERS, encoding="utf-8")
    (site / "sitemap.xml").unlink(missing_ok=True)
    (site / "sitemap.xml.gz").unlink(missing_ok=True)
    deployment = {
        "schemaVersion": 1,
        "site": "zensical",
        "environment": environment,
        "sourceRevision": revision,
        "indexable": False,
        "portableStaticSite": True,
        "analyticsEnabled": False,
        "feedbackEnabled": False,
    }
    (site / "deployment.json").write_text(json.dumps(deployment, indent=2) + "\n", encoding="utf-8")


def main(arguments: list[str]) -> int:
    if len(arguments) != 2:
        print("usage: prepare_deployment.py <local|preview|production> <revision>", file=sys.stderr)
        return 2
    try:
        prepare(SITE, arguments[0], arguments[1])
    except ValueError as error:
        print(f"prepare: {error}", file=sys.stderr)
        return 1
    print(f"Prepared {SITE.name} for {arguments[0]} at {arguments[1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
