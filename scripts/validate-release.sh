#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
mkdir -p results
step=0
run() { step=$((step+1)); printf '\n[release %d] %s\n' "$step" "$1"; shift; "$@"; }

run 'development gate' ./scripts/dev-check.sh
run 'release workspace tests' cargo test --workspace --release --locked
run 'instruction-selection disassembly audit' ./scripts/disassembly-check.sh

WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  run 'canonical scalar + SIMD128 WASM builds' ./scripts/build-wasm.sh
  run 'scalar WASM smoke' node scripts/wasm-smoke.mjs web/player/avelune-scalar.wasm web/player/demo.avl
  run 'SIMD128 WASM smoke' node scripts/wasm-smoke.mjs web/player/avelune-simd128.wasm web/player/demo.avl
  run 'scalar WASM encoder roundtrip' node scripts/wasm-encoder-smoke.mjs web/player/avelune-scalar.wasm
  run 'SIMD128 WASM encoder roundtrip' node scripts/wasm-encoder-smoke.mjs web/player/avelune-simd128.wasm
  run 'Range/Blob/adversarial browser-loader smoke' node scripts/browser-loader-smoke.mjs
  if command -v chromium >/dev/null 2>&1 || [[ -x ${CHROMIUM:-/usr/bin/chromium} ]]; then
    run 'actual Chromium scalar/SIMD execution smoke' node scripts/chromium-wasm-smoke.mjs web/player/demo.avl
  else
    echo 'SKIP: Chromium executable unavailable'
  fi
fi

run 'CLI canonical verify' cargo run -p avelune-cli --release --locked -- verify web/player/demo.avl
run 'format-design measurement proxies' python3 scripts/format-experiments.py --out results/format-experiments.json

AARCH64_LIBDIR=$(rustc --print target-libdir --target aarch64-unknown-linux-gnu 2>/dev/null || true)
if [[ -n $AARCH64_LIBDIR && -d $AARCH64_LIBDIR ]]; then
  run 'AArch64 canonical cross-check' cargo check -p avelune-kernels -p avelune --target aarch64-unknown-linux-gnu --locked
else
  echo 'SKIP: AArch64 target stdlib not installed'
fi

BENCH_SCALE=${AVELUNE_BENCH_SCALE:-2}
run "implementation lab sanity (scale=$BENCH_SCALE)" cargo run -p avelune-lab --release --locked -- \
  --scale "$BENCH_SCALE" --json results/canonical-bench.json --csv results/canonical-bench.csv
run 'benchmark JSON parse' python3 -c 'import json; json.load(open("results/canonical-bench.json")); print("benchmark JSON valid")'

if [[ ${AVELUNE_FULL_CURVES:-0} == 1 ]]; then
  run 'canonical ALV1 size-quality curves' python3 scripts/encoder-curves.py --repeats "${AVELUNE_CURVE_REPEATS:-3}"
fi
printf '\nAvelune release validation PASS\n'
