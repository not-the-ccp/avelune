#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

outside=$(mktemp); assembly=$(mktemp)
trap 'rm -f "$outside" "$assembly"' EXIT
if rg -n '\bunsafe\s+(fn|impl|trait)\b|\bunsafe\s*\{' crates/avelune crates/avelune-reference crates/avelune-wasm tools fuzz > "$outside"; then
  echo 'ERROR: executable unsafe Rust found outside crates/avelune-kernels:' >&2
  cat "$outside" >&2
  exit 1
fi
if rg -n '\b(global_asm|asm)!\s*\(' crates/avelune-kernels > "$assembly"; then
  echo 'ERROR: assembly appeared in the kernel crate without a dedicated assembly review:' >&2
  cat "$assembly" >&2
  exit 1
fi
mapfile -t files < <(rg -l '\bunsafe\s+(fn|impl|trait)\b|\bunsafe\s*\{' crates/avelune-kernels | sort)
allowed=(
  crates/avelune-kernels/src/aarch64/mod.rs
  crates/avelune-kernels/src/lib.rs
  crates/avelune-kernels/src/wasm/mod.rs
  crates/avelune-kernels/src/x86/mod.rs
)
for file in "${files[@]}"; do
  ok=0
  for expected in "${allowed[@]}"; do [[ "$file" == "$expected" ]] && ok=1; done
  [[ $ok -eq 1 ]] || { echo "ERROR: unsafe kernel code appeared in unreviewed location: $file" >&2; exit 1; }
done
for safe in crates/avelune/src/lib.rs crates/avelune-reference/src/lib.rs; do
  grep -q '#!\[forbid(unsafe_code)\]' "$safe" || { echo "ERROR: $safe no longer forbids unsafe code" >&2; exit 1; }
done
echo "unsafe audit PASS (${#files[@]} allowlisted kernel source files; no assembly)"
