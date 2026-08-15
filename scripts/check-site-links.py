#!/usr/bin/env python3
"""Fail when generated Pages HTML contains a missing local file or fragment."""

from __future__ import annotations

from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import sys


RESOURCE_TAGS = {"a", "area", "audio", "embed", "iframe", "img", "link", "object", "script", "source", "track", "video"}
SKIPPED_SCHEMES = {"data", "javascript", "mailto", "tel"}


class LinkParser(HTMLParser):
    """Collect links, anchors, and semantic HTML facts from one generated page."""

    def __init__(self) -> None:
        super().__init__()
        self.links: list[str] = []
        self.anchors: set[str] = set()
        self.duplicate_anchors: set[str] = set()
        self.counts: dict[str, int] = {}
        self.lang: str | None = None
        self.has_description = False
        self.has_canonical = False
        self.control_ids: set[str] = set()
        self.label_targets: set[str] = set()
        self.images_without_alt = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        self.counts[tag] = self.counts.get(tag, 0) + 1
        if tag == "html":
            self.lang = values.get("lang")
        if tag == "meta" and values.get("name") == "description" and values.get("content"):
            self.has_description = True
        if tag == "link" and values.get("rel") == "canonical" and values.get("href"):
            self.has_canonical = True
        if tag == "label" and values.get("for"):
            self.label_targets.add(values["for"])
        if tag in {"input", "select", "textarea"} and values.get("id") and values.get("type") != "hidden":
            self.control_ids.add(values["id"])
        if tag == "img" and "alt" not in values:
            self.images_without_alt += 1
        anchor = values.get("id") or (values.get("name") if tag == "a" else None)
        if anchor:
            if anchor in self.anchors:
                self.duplicate_anchors.add(anchor)
            self.anchors.add(anchor)
        if tag not in RESOURCE_TAGS:
            return
        for key in ("href", "src", "data"):
            value = values.get(key)
            if value:
                self.links.append(value)


def parse_html(path: Path) -> LinkParser:
    """Parse one generated HTML file into a populated :class:`LinkParser`."""
    parser = LinkParser()
    parser.feed(path.read_text(encoding="utf-8", errors="replace"))
    return parser


def destination(root: Path, source: Path, url_path: str, site_base: str) -> Path:
    """Resolve a Pages URL to a built file, accounting for Astro's /avelune base."""
    path_value = unquote(url_path)
    base_prefix = site_base.rstrip("/")
    if path_value == base_prefix:
        path_value = "/"
    elif path_value.startswith(f"{base_prefix}/"):
        path_value = path_value[len(base_prefix):]
    target = root / path_value.lstrip("/") if path_value.startswith("/") else source.parent / path_value
    if target.is_dir():
        target /= "index.html"
    return target


def is_rustdoc(root: Path, path: Path) -> bool:
    """Rustdoc owns its source-link graph, which is not part of the Pages publication."""
    try:
        return path.resolve().relative_to(root).parts[:2] == ("api", "rust")
    except ValueError:
        return False


def main() -> int:
    """Check every generated page for missing files, fragments, and semantic issues."""
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/site").resolve()
    site_base = sys.argv[2] if len(sys.argv) > 2 else "/avelune"
    if not root.is_dir():
        print(f"site directory does not exist: {root}", file=sys.stderr)
        return 2

    missing: list[tuple[Path, str, str]] = []
    semantic: list[tuple[Path, str]] = []
    parsers: dict[Path, LinkParser] = {}
    checked = 0
    for html in root.rglob("*.html"):
        if is_rustdoc(root, html):
            continue
        parser = parse_html(html)
        relative_html = html.relative_to(root)
        if parser.lang != "en":
            semantic.append((relative_html, "document language must be en"))
        if parser.counts.get("main") != 1:
            semantic.append((relative_html, "document must contain exactly one main landmark"))
        if parser.counts.get("h1") != 1:
            semantic.append((relative_html, "document must contain exactly one h1"))
        if not parser.has_description:
            semantic.append((relative_html, "document is missing a meta description"))
        if not parser.has_canonical:
            semantic.append((relative_html, "document is missing a canonical link"))
        for anchor in sorted(parser.duplicate_anchors):
            semantic.append((relative_html, f"duplicate id/name: {anchor}"))
        for control in sorted(parser.control_ids - parser.label_targets):
            semantic.append((relative_html, f"form control has no explicit label: {control}"))
        if parser.images_without_alt:
            semantic.append((relative_html, f"{parser.images_without_alt} image(s) missing alt"))
        parsers[html.resolve()] = parser
        for link in parser.links:
            url = urlsplit(link)
            if url.scheme.lower() in SKIPPED_SCHEMES or url.scheme or url.netloc:
                continue
            if not url.path and not url.fragment:
                continue
            target = destination(root, html, url.path, site_base) if url.path else html
            if is_rustdoc(root, target):
                continue
            if not target.exists():
                missing.append((html.relative_to(root), link, "file"))
            elif url.fragment and target.suffix.lower() in {".htm", ".html"}:
                parsed_target = parsers.setdefault(target.resolve(), parse_html(target))
                if unquote(url.fragment) not in parsed_target.anchors:
                    missing.append((html.relative_to(root), link, "fragment"))
            checked += 1

    if missing or semantic:
        for source, link, kind in missing:
            print(f"missing local {kind}: {source} -> {link}", file=sys.stderr)
        for source, problem in semantic:
            print(f"site semantic error: {source} -> {problem}", file=sys.stderr)
        print(f"site check FAILED ({len(missing)} missing link/fragment, {len(semantic)} semantic)", file=sys.stderr)
        return 1

    print(f"site local-link check PASS ({checked} references)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
