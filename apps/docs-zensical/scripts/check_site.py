"""Checks the built Zensical site: every page built, every local link resolves.

Run after `zensical build` in `build/`. It fails when the site has fewer
pages than the inventory marks public, or when a page links or loads a
local file that the build did not produce. The site's own 404 page is
skipped: it is served at any depth, so its links start at the site root.
"""

from __future__ import annotations

import json
import re
import sys
import urllib.parse
from pathlib import Path

APP = Path(__file__).resolve().parent.parent
SITE = APP / "build" / "site"
INVENTORY = APP.parent.parent / "docs" / "inventory.json"


def main() -> int:
    if not SITE.is_dir():
        print(f"check: {SITE} does not exist; build the site first", file=sys.stderr)
        return 1
    entries = json.loads(INVENTORY.read_text(encoding="utf-8"))["entries"]
    public = sum(1 for entry in entries if entry["publication"] == "public")
    pages = [
        page
        for page in SITE.rglob("index.html")
        if not page.relative_to(SITE).parts[0] in {"assets", "search"}
    ]
    problems: list[str] = []
    if len(pages) < public:
        problems.append(f"{len(pages)} pages built, {public} public pages in the inventory")
    for page in SITE.rglob("*.html"):
        if page == SITE / "404.html":
            continue
        text = page.read_text(encoding="utf-8", errors="replace")
        for target in re.findall(r'(?:href|src)="([^"#?]+)', text):
            if re.match(r"^[a-z][a-z0-9+.-]*:|^//", target):
                continue
            decoded = urllib.parse.unquote(target)
            resolved = (
                SITE / decoded.lstrip("/") if decoded.startswith("/") else page.parent / decoded
            ).resolve()
            if resolved.is_dir():
                resolved = resolved / "index.html"
            if not resolved.exists():
                problems.append(f"{page.relative_to(SITE)}: {target} does not exist")
    for problem in problems:
        print(f"check: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(f"Site checks passed ({len(pages)} pages, every local link resolves)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
