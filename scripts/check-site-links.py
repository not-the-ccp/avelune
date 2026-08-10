#!/usr/bin/env python3
"""Fail when generated Pages HTML references a missing local resource."""

from __future__ import annotations

from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import sys


class LinkParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.links: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag not in {"a", "link", "script", "img"}:
            return
        values = dict(attrs)
        for key in ("href", "src"):
            value = values.get(key)
            if value:
                self.links.append(value)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/site").resolve()
    if not root.is_dir():
        print(f"site directory does not exist: {root}", file=sys.stderr)
        return 2

    missing: list[tuple[Path, str]] = []
    checked = 0
    for html in root.rglob("*.html"):
        parser = LinkParser()
        parser.feed(html.read_text(encoding="utf-8", errors="replace"))
        for link in parser.links:
            if link.startswith(("#", "mailto:", "javascript:", "data:")):
                continue
            url = urlsplit(link)
            if url.scheme or url.netloc or not url.path:
                continue
            path = unquote(url.path)
            target = root / path.lstrip("/") if path.startswith("/") else html.parent / path
            if target.is_dir():
                target /= "index.html"
            if not target.exists():
                missing.append((html.relative_to(root), link))
            checked += 1

    if missing:
        for source, link in missing:
            print(f"missing local link: {source} -> {link}", file=sys.stderr)
        print(f"site link check FAILED ({len(missing)} missing)", file=sys.stderr)
        return 1

    print(f"site local-link check PASS ({checked} references)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
