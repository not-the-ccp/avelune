#!/usr/bin/env python3
from pathlib import Path
import re, sys
root = Path(sys.argv[1])
for path in root.rglob('*.html'):
    text = path.read_text(encoding='utf-8')
    text = re.sub(r'href="([^"?#]+)\.md([?#][^"]*)?"', lambda m: f'href="{m.group(1)}.html{m.group(2) or ""}"', text)
    # Repository-directory links are meaningful on Git hosting but are not copied verbatim to Pages.
    if path == root / 'index.html':
        text = text.replace('href="impl/"', 'href="docs/development/REFERENCE_IMPLEMENTATION.html"')
        text = text.replace('href="prod/"', 'href="docs/development/PRODUCTION_BACKENDS.html"')
    path.write_text(text, encoding='utf-8')
