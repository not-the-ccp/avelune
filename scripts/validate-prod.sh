#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
mkdir -p results

step=0
run() { step=$((step+1)); printf '\n[prod %d] %s\n' "$step" "$1"; shift; "$@"; }

run 'format' cargo fmt --all -- --check
run 'workspace clippy' cargo clippy --workspace --all-targets --locked -- -D warnings
run 'production release tests, differential tests, and hostile corpus' \
  cargo test -p avelune-prod -p avelune-prod-kernels --release --locked
run 'unsafe-location and assembly audit' ./scripts/audit-prod-unsafe.sh
run 'native/WASM instruction-selection disassembly audit' ./scripts/prod-disassembly-check.sh

WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  run 'production WASM scalar + SIMD128 builds' ./scripts/build-prod-wasm.sh
  run 'Node scalar WASM differential smoke' node scripts/prod-wasm-smoke.mjs web/player/avelune-prod-scalar.wasm web/player/demo.avl
  run 'Node SIMD128 WASM differential smoke' node scripts/prod-wasm-smoke.mjs web/player/avelune-prod-simd128.wasm web/player/demo.avl
  run 'browser loader HTTP Range smoke (Node WebAssembly)' node scripts/prod-browser-loader-smoke.mjs
  if command -v chromium >/dev/null 2>&1 || [[ -x ${CHROMIUM:-/usr/bin/chromium} ]]; then
    run 'actual Chromium scalar/SIMD execution smoke' node scripts/prod-browser-smoke.mjs web/player/demo.avl
  else
    echo 'SKIP: Chromium executable unavailable; browser runtime gate remains unexecuted'
  fi
else
  echo 'SKIP: wasm32-unknown-unknown stdlib unavailable; production WASM gates remain unexecuted'
fi

run 'CLI production verify' cargo run -p avelune-cli --release --locked -- --backend prod verify web/player/demo.avl
run 'CLI reference verify' cargo run -p avelune-cli --release --locked -- --backend reference verify web/player/demo.avl
run 'measured format-design proxies' python3 scripts/prod-format-experiments.py --out results/format-experiments.json

if rustc --print target-libdir --target aarch64-unknown-linux-gnu >/dev/null 2>&1; then
  AARCH64_LIBDIR=$(rustc --print target-libdir --target aarch64-unknown-linux-gnu)
  if [[ -d $AARCH64_LIBDIR ]]; then
    run 'AArch64 production cross-check' cargo check -p avelune-prod-kernels -p avelune-prod --target aarch64-unknown-linux-gnu --locked
  else
    echo 'SKIP: AArch64 target stdlib not installed; source exists but compile/runtime gates remain unexecuted'
  fi
else
  echo 'SKIP: AArch64 target stdlib not installed; source exists but compile/runtime gates remain unexecuted'
fi

BENCH_SCALE=${AVELUNE_BENCH_SCALE:-2}
run "production benchmark sanity (scale=$BENCH_SCALE)" cargo run -p avelune-prod-lab --release --locked -- \
  --scale "$BENCH_SCALE" --json results/prod-bench.json --csv results/prod-bench.csv
run 'benchmark JSON parse' python3 -c 'import json; json.load(open("results/prod-bench.json")); print("benchmark JSON valid")'

if [[ ${AVELUNE_FULL_CURVES:-0} == 1 ]]; then
  run 'reference/production ALV1 size-quality curves' python3 scripts/prod-encoder-curves.py --repeats "${AVELUNE_CURVE_REPEATS:-3}"
fi

printf '\nAvelune production validation PASS\n'
