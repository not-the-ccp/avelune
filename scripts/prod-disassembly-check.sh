#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
mkdir -p results

cargo test -p avelune-prod-kernels --release --locked --no-run >/tmp/avelune-kernel-build.log 2>&1
bin=$(find target/release/deps -maxdepth 1 -type f -name 'avelune_prod_kernels-*' -perm -111 | head -n1)
[[ -n ${bin:-} ]] || { echo 'ERROR: kernel test executable not found' >&2; exit 1; }

native=$(mktemp)
trap 'rm -f "$native" "$wasm_dump"' EXIT
objdump -d -C "$bin" > "$native"

need_native() {
  local label=$1 pattern=$2
  if ! grep -E "$pattern" "$native" >/dev/null; then
    echo "ERROR: native disassembly missing $label ($pattern)" >&2
    exit 1
  fi
}
need_native 'SSE4.2 CRC32C' '\bcrc32[bqlw]?\b'
need_native 'x86 packed SAD' '\bpsadbw\b'
need_native 'AVX2 integer vector work' '\bvp(add|sub)d\b'

{
  echo '# Avelune production instruction-selection evidence'
  echo "kernel_test_binary=$bin"
  echo "objdump=$(objdump --version | head -n1)"
  echo
  echo '## Native x86 excerpts'
  grep -E -m 30 '\b(crc32[bqlw]?|psadbw|vp(add|sub)d|vmovdqu)\b' "$native"
} > results/disassembly-evidence.txt

# WASM SIMD is a separate artifact with a compile-time feature requirement. If the target exists,
# prove that the retained artifact actually contains the arithmetic used by our byte-SAD kernel.
wasm_dump=$(mktemp)
if rustc --print target-libdir --target wasm32-unknown-unknown >/dev/null 2>&1 \
  && [[ -d $(rustc --print target-libdir --target wasm32-unknown-unknown) ]]; then
  ./scripts/build-prod-wasm.sh >/tmp/avelune-wasm-build.log 2>&1
  llvm_objdump=${LLVM_OBJDUMP:-$(command -v llvm-objdump || true)}
  if [[ -z $llvm_objdump && -x /usr/local/swift/usr/bin/llvm-objdump ]]; then
    llvm_objdump=/usr/local/swift/usr/bin/llvm-objdump
  fi
  if [[ -n $llvm_objdump ]]; then
    "$llvm_objdump" -d web/player/avelune-prod-simd128.wasm > "$wasm_dump"
    grep -q 'i8x16.sub_sat_u' "$wasm_dump" || {
      echo 'ERROR: SIMD128 artifact lacks expected saturating byte subtraction' >&2; exit 1;
    }
    grep -q 'i16x8.extadd_pairwise_i8x16_u' "$wasm_dump" || {
      echo 'ERROR: SIMD128 artifact lacks expected pairwise widening reduction' >&2; exit 1;
    }
    {
      echo
      echo '## WASM SIMD128 excerpts'
      echo "llvm_objdump=$($llvm_objdump --version | head -n1)"
      grep -E -m 20 'i8x16.sub_sat_u|i16x8.extadd_pairwise_i8x16_u|i32x4.extadd_pairwise_i16x8_u' "$wasm_dump"
    } >> results/disassembly-evidence.txt
  else
    echo 'NOTE: llvm-objdump unavailable; WASM instruction disassembly gate skipped' >> results/disassembly-evidence.txt
  fi
else
  echo 'NOTE: wasm32 stdlib unavailable; WASM instruction disassembly gate skipped' >> results/disassembly-evidence.txt
fi

echo 'production disassembly inspection PASS'
