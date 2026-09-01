#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# Keep this audit self-contained. `dev-check.sh` already requires Python, while
# ripgrep is not guaranteed to exist on clean CI images. More importantly, a
# missing search executable must never be mistaken for "no unsafe code found".
python3 - <<'PY'
from pathlib import Path
import re
import sys

UNSAFE = re.compile(r"\bunsafe\s+(?:fn|impl|trait)\b|\bunsafe\s*\{")
ASSEMBLY = re.compile(r"\b(?:global_asm|asm)!\s*\(")
OUTSIDE_ROOTS = [
    Path("crates/avelune"),
    Path("crates/avelune-reference"),
    Path("crates/avelune-wasm"),
    Path("tools"),
    Path("fuzz"),
]
KERNEL_ROOT = Path("crates/avelune-kernels")
ALLOWED = {
    Path("crates/avelune-kernels/src/aarch64/mod.rs"),
    Path("crates/avelune-kernels/src/lib.rs"),
    Path("crates/avelune-kernels/src/wasm/mod.rs"),
    Path("crates/avelune-kernels/src/x86/mod.rs"),
}


def rust_files(root: Path):
    if not root.exists():
        return
    yield from root.rglob("*.rs")


def matches(path: Path, pattern: re.Pattern[str]):
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        print(f"ERROR: cannot audit {path}: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
    for line_no, line in enumerate(text.splitlines(), 1):
        if pattern.search(line):
            yield line_no, line


outside_hits: list[tuple[Path, int, str]] = []
for root in OUTSIDE_ROOTS:
    for path in rust_files(root):
        outside_hits.extend((path, line_no, line) for line_no, line in matches(path, UNSAFE))
if outside_hits:
    print("ERROR: executable unsafe Rust found outside crates/avelune-kernels:", file=sys.stderr)
    for path, line_no, line in outside_hits:
        print(f"{path}:{line_no}:{line}", file=sys.stderr)
    raise SystemExit(1)

assembly_hits: list[tuple[Path, int, str]] = []
for path in rust_files(KERNEL_ROOT):
    assembly_hits.extend((path, line_no, line) for line_no, line in matches(path, ASSEMBLY))
if assembly_hits:
    print("ERROR: assembly appeared in the kernel crate without a dedicated assembly review:", file=sys.stderr)
    for path, line_no, line in assembly_hits:
        print(f"{path}:{line_no}:{line}", file=sys.stderr)
    raise SystemExit(1)

unsafe_kernel_files = {
    path
    for path in rust_files(KERNEL_ROOT)
    if any(matches(path, UNSAFE))
}
unreviewed = sorted(unsafe_kernel_files - ALLOWED)
if unreviewed:
    for path in unreviewed:
        print(f"ERROR: unsafe kernel code appeared in unreviewed location: {path}", file=sys.stderr)
    raise SystemExit(1)

for safe in [Path("crates/avelune/src/lib.rs"), Path("crates/avelune-reference/src/lib.rs")]:
    try:
        text = safe.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        print(f"ERROR: cannot audit {safe}: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
    if "#![forbid(unsafe_code)]" not in text:
        print(f"ERROR: {safe} no longer forbids unsafe code", file=sys.stderr)
        raise SystemExit(1)

print(f"unsafe audit PASS ({len(unsafe_kernel_files)} allowlisted kernel source files; no assembly)")
PY
