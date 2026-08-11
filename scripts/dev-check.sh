#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

echo '[1/11] rustfmt'
cargo fmt --all -- --check

echo '[2/11] clippy'
cargo clippy --workspace --all-targets --locked -- -D warnings

echo '[3/11] tests + doctests'
cargo test --workspace --locked
cargo test --workspace --doc --locked

echo '[4/11] rustdoc'
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked

echo '[5/11] script syntax'
node --check web/player/player.js
node --check web/player/avelune-prod-loader.js
node --check web/player/serve.mjs
node --check scripts/validate-content.mjs
node --check scripts/prod-wasm-smoke.mjs
node --check scripts/prod-browser-smoke.mjs
node --check scripts/prod-browser-loader-smoke.mjs
python3 -m py_compile scripts/benchmark-real.py scripts/benchmark-real-audio.py scripts/benchmark-xiph.py scripts/check-site-links.py scripts/prod-format-experiments.py scripts/prod-encoder-curves.py
bash -n scripts/build-wasm.sh scripts/build-prod-wasm.sh scripts/build-site.sh scripts/fetch-xiph-corpus.sh scripts/validate-draft.sh scripts/audit-prod-unsafe.sh scripts/prod-disassembly-check.sh scripts/validate-prod.sh

echo '[6/11] production safety audit'
./scripts/audit-prod-unsafe.sh

echo '[7/11] cli smoke + completions'
cargo build -p avelune-cli --locked
BIN=${CARGO_TARGET_DIR:-target}/debug/avelune
"$BIN" --version
"$BIN" --help >/dev/null
for sh in bash zsh fish powershell elvish; do "$BIN" completions "$sh" >/dev/null; done

echo '[8/11] wasm (when target installed)'
WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  ./scripts/build-wasm.sh
  ./scripts/build-prod-wasm.sh
  node scripts/prod-wasm-smoke.mjs web/player/avelune-prod-scalar.wasm web/player/demo.avl
  node scripts/prod-wasm-smoke.mjs web/player/avelune-prod-simd128.wasm web/player/demo.avl
else
  echo 'SKIP: wasm32-unknown-unknown stdlib is not installed'
fi

echo '[9/11] AsciiDoc content'
npm run check:content

echo '[10/11] docs/site'
./scripts/build-site.sh --skip-wasm

echo '[11/11] generated-site links and fragments'
python3 scripts/check-site-links.py dist/site

echo 'Avelune development checks PASS'
