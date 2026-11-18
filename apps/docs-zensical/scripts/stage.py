"""Stages `docs/` for the Zensical build and writes its configuration.

The Zensical site is built from the same Markdown as the VitePress site
(`apps/docs`), so that the two can be compared page for page. This script:

1. copies every page the inventory (`docs/inventory.json`) marks public,
   with the files they reference, into `build/src`, and the contents of
   `docs/public` to its root, as VitePress serves them, with this app's
   `stylesheets/` (the VitePress site's brand colours);
2. turns the little VitePress-only syntax the pages use into Markdown
   Zensical reads: `::: kind Title` containers into admonitions, the home
   page's `layout: home` front matter into a heading, buttons and cards,
   and links that start at the site root (`/guide/...`) into relative
   links to the Markdown file;
3. writes `build/zensical.toml`: `zensical.toml` with a `nav` taken from
   the VitePress sidebar, so both sites list pages in the same order. A
   public page the sidebar leaves out is added to its folder's section
   under its inventory title, and a folder the sidebar leaves out gets a
   section of its own.

The environment variable `ZENSICAL_SITE_URL`, when set, replaces the
configuration's `site_url` for a deployment served at another address.

It fails, naming the page, when the sidebar links a page that is not
public or does not exist, or when a page uses a VitePress container this
script does not convert. Run it with any Python 3.11 or later; it needs
PyYAML, which Zensical installs.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
from pathlib import Path

import yaml

APP = Path(__file__).resolve().parent.parent
REPO = APP.parent.parent
DOCS = REPO / "docs"
BUILD = APP / "build"
SRC = BUILD / "src"
VITEPRESS_CONFIG = DOCS / ".vitepress" / "config.mts"
SKIPPED = {".vitepress", "public", "inventory.json", "owners.json"}

ADMONITIONS = {"tip": "tip", "info": "info", "warning": "warning", "danger": "danger"}


class StageError(Exception):
    pass


def public_pages() -> dict[str, str]:
    """Public pages by path, with their inventory titles."""
    inventory = json.loads((DOCS / "inventory.json").read_text(encoding="utf-8"))
    return {
        entry["path"]: entry["title"]
        for entry in inventory["entries"]
        if entry["publication"] == "public"
    }


def link_to_path(link: str) -> str:
    """`/guide/getting-started` -> `guide/getting-started.md`, `/admin/` -> `admin/index.md`."""
    path = link.lstrip("/")
    if path == "" or path.endswith("/"):
        return f"{path}index.md"
    return f"{path}.md"


def sidebar() -> list[tuple[str, list[tuple[str, str, list[tuple[str, str]]]]]]:
    """The VitePress sidebar: per folder, its groups and their (title, page) items."""
    config = VITEPRESS_CONFIG.read_text(encoding="utf-8")
    start = config.find("sidebar: {")
    end = config.find("search: {", start)
    if start < 0 or end < 0:
        raise StageError(f"{VITEPRESS_CONFIG}: no sidebar found")
    block = config[start:end]
    token = re.compile(
        r'"(?P<folder>/[^"]+/)": \['
        r'|text: "(?P<group>[^"]+)",(?:\s*collapsed: \w+,)?\s*items:'
        r'|\{ text: "(?P<title>[^"]+)", link: "(?P<link>[^"]+)" \}'
    )
    folders: list[tuple[str, list]] = []
    for match in token.finditer(block):
        if match["folder"]:
            folders.append((match["folder"].strip("/"), []))
        elif match["group"]:
            if not folders:
                raise StageError(f"{VITEPRESS_CONFIG}: a sidebar group outside a folder")
            folders[-1][1].append((match["group"], []))
        else:
            if not folders or not folders[-1][1]:
                raise StageError(f"{VITEPRESS_CONFIG}: a sidebar item outside a group")
            folders[-1][1][-1][1].append((match["title"], link_to_path(match["link"])))
    return folders


def top_menus() -> list[tuple[str, list[str]]]:
    """The VitePress top navigation menus, with the folders each links to."""
    config = VITEPRESS_CONFIG.read_text(encoding="utf-8")
    start = config.find("nav: [")
    end = config.find("sidebar: {", start)
    block = config[start:end]
    menus: list[tuple[str, list[str]]] = []
    for match in re.finditer(
        r'text: "(?P<menu>[^"]+)",\s*items:|link: "/(?P<folder>[^/"]+)/', block
    ):
        if match["menu"]:
            menus.append((match["menu"], []))
        elif menus and match["folder"] not in menus[-1][1]:
            menus[-1][1].append(match["folder"])
    return menus


def navigation(pages: dict[str, str]) -> list:
    """The nav: Home, then one tab per top menu, then `More` for the rest."""
    folders = sidebar()
    placed: set[str] = set()
    for _, groups in folders:
        for _, items in groups:
            for _, path in items:
                if path not in pages:
                    raise StageError(
                        f"the VitePress sidebar links {path}, which is not a public page"
                    )

    # Each folder goes to the first menu that links into it; a folder no
    # menu links to follows the folder before it in the sidebar.
    menus = top_menus()
    tab_of: dict[str, str] = {}
    for menu, menu_folders in menus:
        for folder in menu_folders:
            tab_of.setdefault(folder, menu)
    previous = None
    for folder, _ in folders:
        if folder not in tab_of and previous is not None:
            tab_of[folder] = tab_of[previous]
        previous = folder

    def section(folder: str, groups) -> list:
        entries = []
        for group, items in groups:
            listed = [{title: path} for title, path in items]
            placed.update(path for _, path in items)
            entries.append({group: listed})
        extra = sorted(
            path
            for path in pages
            if path.startswith(f"{folder}/") and path not in placed
        )
        if extra:
            placed.update(extra)
            target = entries[-1][next(iter(entries[-1]))] if entries else None
            listed = [{pages[path]: path} for path in extra]
            if target is None:
                entries.append({folder.replace("-", " ").capitalize(): listed})
            else:
                target.extend(listed)
        return entries

    tabs: dict[str, list] = {menu: [] for menu, _ in menus}
    tabs["More"] = []
    for folder, groups in folders:
        tabs[tab_of.get(folder, "More")].extend(section(folder, groups))
    remaining = sorted({path.split("/")[0] for path in pages if "/" in path} - {f for f, _ in folders})
    for folder in remaining:
        tabs["More"].extend(section(folder, []))

    nav: list = [{"Home": "index.md"}] if "index.md" in pages else []
    placed.add("index.md")
    for menu, entries in tabs.items():
        if entries:
            nav.append({menu: entries})
    missing = sorted(set(pages) - placed)
    if missing:
        raise StageError(f"pages left out of the navigation: {', '.join(missing)}")
    return nav


def relative_link(target: str, page: str) -> str:
    """A site-root link (`/guide/x#a`) as a link from `page` to the Markdown file."""
    path, _, anchor = target.partition("#")
    file = link_to_path(path) if not Path(path).suffix else path.lstrip("/")
    relative = os.path.relpath(file, start=os.path.dirname(page) or ".")
    return relative + (f"#{anchor}" if anchor else "")


def convert_links(text: str, page: str) -> str:
    text = re.sub(
        r"\]\((/[^)\s]*)\)",
        lambda match: f"]({relative_link(match[1], page)})",
        text,
    )
    return re.sub(
        r'href="(/[^"]*)"',
        lambda match: f'href="{relative_link(match[1], page)}"',
        text,
    )


def convert_containers(text: str, page: str) -> str:
    out: list[str] = []
    lines = text.split("\n")
    index = 0
    fence = None
    while index < len(lines):
        line = lines[index]
        stripped = line.strip()
        if stripped.startswith(("```", "~~~")):
            marker = stripped[:3]
            fence = None if fence == marker else (fence or marker)
            out.append(line)
            index += 1
            continue
        match = re.match(r"^:::\s*(\w[\w-]*)\s*(.*)$", line) if fence is None else None
        if match:
            kind = match[1]
            if kind not in ADMONITIONS:
                raise StageError(f"{page}: the VitePress container `::: {kind}` is not converted")
            title = match[2].strip()
            out.append(f'!!! {ADMONITIONS[kind]}' + (f' "{title}"' if title else ""))
            index += 1
            while index < len(lines) and lines[index].strip() != ":::":
                out.append(f"    {lines[index]}" if lines[index] else "")
                index += 1
            if index == len(lines):
                raise StageError(f"{page}: a `::: {kind}` container is not closed")
            index += 1
            continue
        out.append(line)
        index += 1
    return "\n".join(out)


def convert_home(text: str) -> str:
    """The VitePress home layout's hero and features, as Markdown."""
    match = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    if not match:
        return text
    front = yaml.safe_load(match[1]) or {}
    if front.get("layout") != "home":
        return text
    hero = front.get("hero", {})
    parts = [f"# {hero.get('name', 'Limiara')}", ""]
    if hero.get("text"):
        parts += [f"**{hero['text']}**", ""]
    if hero.get("tagline"):
        parts += [hero["tagline"], ""]
    buttons = []
    for action in hero.get("actions", []):
        style = ".md-button .md-button--primary" if action.get("theme") == "brand" else ".md-button"
        buttons.append(f"[{action['text']}]({action['link']}){{ {style} }}")
    if buttons:
        parts += [" ".join(buttons), ""]
    features = front.get("features", [])
    if features:
        parts.append('<div class="grid cards" markdown>')
        parts.append("")
        for feature in features:
            parts.append(f"-   **{feature['title']}**")
            parts.append("")
            parts.append(f"    {feature['details']}")
            parts.append("")
        parts.append("</div>")
        parts.append("")
    return "\n".join(parts) + text[match.end():]


def stage(pages: dict[str, str]) -> None:
    if SRC.exists():
        shutil.rmtree(SRC)
    SRC.mkdir(parents=True)
    for item in (DOCS / "public").iterdir():
        if item.is_file() and not item.name.startswith("_"):
            shutil.copy2(item, SRC / item.name)
    shutil.copytree(APP / "stylesheets", SRC / "stylesheets")
    for source in sorted(DOCS.rglob("*")):
        relative = source.relative_to(DOCS)
        if relative.parts[0] in SKIPPED or source.is_dir():
            continue
        target = SRC / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.suffix == ".md":
            page = relative.as_posix()
            if page not in pages:
                continue
            text = source.read_text(encoding="utf-8")
            if page == "index.md":
                text = convert_home(text)
            text = convert_links(convert_containers(text, page), page)
            target.write_text(text, encoding="utf-8")
        else:
            shutil.copy2(source, target)


def toml_value(value) -> str:
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{json.dumps(k, ensure_ascii=False)} = {toml_value(v)}" for k, v in value.items()) + " }"
    raise StageError(f"cannot write {value!r} as TOML")


def write_config(nav: list) -> None:
    template = (APP / "zensical.toml").read_text(encoding="utf-8")
    marker = "# nav: generated by scripts/stage.py from the VitePress sidebar."
    if marker not in template:
        raise StageError("zensical.toml: the nav marker is missing")
    config = template.replace(marker, f"nav = {toml_value(nav)}")
    # A deployment to another address (GitHub Pages serves the site under
    # /<repository>/) passes its URL, so the 404 page's links and the
    # canonical links match where the site is served.
    site_url = os.environ.get("ZENSICAL_SITE_URL")
    if site_url:
        if not re.fullmatch(r"https://[^\s\"]+/", site_url):
            raise StageError(f"ZENSICAL_SITE_URL must be an https URL ending in /: {site_url!r}")
        config, count = re.subn(
            r'^site_url = ".*"$', f"site_url = {json.dumps(site_url)}", config, flags=re.M
        )
        if count != 1:
            raise StageError("zensical.toml: no site_url line to replace")
    (BUILD / "zensical.toml").write_text(config, encoding="utf-8")


def main() -> int:
    try:
        pages = public_pages()
        nav = navigation(pages)
        stage(pages)
        write_config(nav)
    except StageError as error:
        print(f"stage: {error}", file=sys.stderr)
        return 1
    print(f"Staged {len(pages)} public pages into {SRC.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
