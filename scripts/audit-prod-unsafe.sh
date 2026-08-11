#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# Rust 2024 requires #[unsafe(no_mangle)] for exported WASM symbols. Those attributes do not
# permit unsafe operations. Actual unsafe blocks/functions/impls are restricted to kernels.
if rg -n '\bunsafe\s+(fn|impl|trait)\b|\bunsafe\s*\{' prod/rust/src prod/rust/wasm prod/rust/lab prod/rust/tests > /tmp/avelune-prod-unsafe-outside-kernels.txt; then
  echo 'ERROR: executable unsafe Rust found outside prod/rust/kernels:' >&2
  cat /tmp/avelune-prod-unsafe-outside-kernels.txt >&2
  exit 1
fi

# Assembly has a higher acceptance bar and is intentionally absent at this stage.
if rg -n '\b(global_asm|asm)!\s*\(' prod/rust/kernels > /tmp/avelune-prod-asm.txt; then
  echo 'ERROR: assembly present in production kernels without a dedicated assembly audit gate:' >&2
  cat /tmp/avelune-prod-asm.txt >&2
  exit 1
fi

# Keep the physical unsafe boundary small and explicit. Comments/SAFETY prose may mention unsafe;
# executable occurrences must remain in this allowlisted set of kernel files.
mapfile -t files < <(rg -l '\bunsafe\s+(fn|impl|trait)\b|\bunsafe\s*\{' prod/rust/kernels | sort)
allowed=(
  prod/rust/kernels/src/aarch64/mod.rs
  prod/rust/kernels/src/lib.rs
  prod/rust/kernels/src/wasm/mod.rs
  prod/rust/kernels/src/x86/mod.rs
)
for file in "${files[@]}"; do
  ok=0
  for a in "${allowed[@]}"; do [[ "$file" == "$a" ]] && ok=1; done
  if [[ $ok -ne 1 ]]; then
    echo "ERROR: unsafe kernel code appeared in unreviewed location: $file" >&2
    exit 1
  fi
done

# The safe production engine itself has a compile-time lint backstop as well.
grep -q '#!\[forbid(unsafe_code)\]' prod/rust/src/lib.rs || {
  echo 'ERROR: avelune-prod no longer forbids unsafe code' >&2; exit 1;
}

echo "production unsafe audit PASS (${#files[@]} allowlisted kernel source files; no assembly)"
