#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"

echo '[1/9] rustfmt'
cargo fmt --all -- --check

echo '[2/9] clippy'
cargo clippy --workspace --all-targets --locked -- -D warnings

echo '[3/9] tests + doctests'
cargo test --workspace --locked
cargo test --workspace --doc --locked

echo '[4/9] canonical rustdoc'
RUSTDOCFLAGS='-D warnings' cargo doc -p avelune --no-deps --locked

echo '[5/9] script/source syntax'
node --check web/player/player.js
node --check web/player/avelune-loader.js
node --check web/player/renderers.js
node --check scripts/browser-loader-smoke.mjs
node --check scripts/wasm-smoke.mjs
node --check scripts/wasm-encoder-smoke.mjs
node --check scripts/chromium-wasm-smoke.mjs
node --check scripts/site-browser-smoke.mjs
node --check scripts/browser-media-import-smoke.mjs
node --check scripts/check-demo-defaults.mjs
node --check scripts/validate-content.mjs
node scripts/check-demo-defaults.mjs
python3 -m py_compile scripts/benchmark-real.py scripts/benchmark-real-audio.py scripts/benchmark-xiph.py scripts/check-site-links.py scripts/format-experiments.py scripts/encoder-curves.py scripts/ci-media-regression.py scripts/compare-ci-media.py scripts/test-ci-regression-tools.py scripts/generate-demo-fixtures.py scripts/generate-showcase-media.py
python3 scripts/test-ci-regression-tools.py
bash -n scripts/build-wasm.sh scripts/build-site.sh scripts/fetch-xiph-corpus.sh scripts/audit-unsafe.sh scripts/disassembly-check.sh scripts/validate-release.sh scripts/ci-media-base-head.sh scripts/install.sh

echo '[6/9] unsafe boundary'
./scripts/audit-unsafe.sh

echo '[7/9] CLI fixture'
cargo build -p avelune-cli --locked
BIN=${CARGO_TARGET_DIR:-target}/debug/avelune
"$BIN" --version
"$BIN" --help >/dev/null
for sh in bash zsh fish powershell elvish; do "$BIN" completions "$sh" >/dev/null; done
"$BIN" verify web/player/demo.avl >/dev/null
python3 scripts/generate-demo-fixtures.py --cli "$BIN" --check

echo '[8/9] WASM + browser transport (when target installed)'
WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  ./scripts/build-wasm.sh
  node scripts/wasm-smoke.mjs web/player/avelune-scalar.wasm web/player/demo.avl
  node scripts/wasm-smoke.mjs web/player/avelune-simd128.wasm web/player/demo.avl
  node scripts/wasm-encoder-smoke.mjs web/player/avelune-scalar.wasm
  node scripts/wasm-encoder-smoke.mjs web/player/avelune-simd128.wasm
  node scripts/browser-loader-smoke.mjs
  if [[ -f node_modules/@ffmpeg/core/dist/esm/ffmpeg-core.js ]]; then node scripts/browser-media-import-smoke.mjs; else echo 'SKIP: @ffmpeg/core is not installed for media-import smoke'; fi
else
  echo 'SKIP: wasm32-unknown-unknown stdlib is not installed'
fi

echo '[9/9] content/site when npm dependencies are installed'
if [[ -x node_modules/.bin/astro ]]; then
  npm run check:content
  ./scripts/build-site.sh --skip-wasm
  python3 scripts/check-site-links.py dist/site
  node scripts/site-browser-smoke.mjs dist/site
else
  echo 'SKIP: npm dependency tree is not installed; run npm ci explicitly'
fi

echo 'Avelune development checks PASS'
