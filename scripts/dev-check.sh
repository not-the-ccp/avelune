#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

echo '[1/10] rustfmt'
cargo fmt --all -- --check

echo '[2/10] clippy'
cargo clippy --workspace --all-targets --locked -- -D warnings

echo '[3/10] tests + doctests'
cargo test --workspace --locked
cargo test --workspace --doc --locked

echo '[4/10] rustdoc'
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked

echo '[5/10] script syntax'
node --check web/player/player.js
node --check web/player/serve.mjs
node --check scripts/validate-content.mjs
python3 -m py_compile scripts/benchmark-real.py scripts/benchmark-real-audio.py scripts/benchmark-xiph.py scripts/check-site-links.py
bash -n scripts/build-wasm.sh scripts/build-site.sh scripts/fetch-xiph-corpus.sh scripts/validate-draft.sh

echo '[6/10] cli smoke + completions'
cargo build -p avelune-cli --locked
BIN=${CARGO_TARGET_DIR:-target}/debug/avelune
"$BIN" --version
"$BIN" --help >/dev/null
for sh in bash zsh fish powershell elvish; do "$BIN" completions "$sh" >/dev/null; done

echo '[7/10] wasm (when target installed)'
WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  ./scripts/build-wasm.sh
else
  echo 'SKIP: wasm32-unknown-unknown stdlib is not installed'
fi

echo '[8/10] AsciiDoc content'
npm run check:content

echo '[9/10] docs/site'
./scripts/build-site.sh --skip-wasm

echo '[10/10] generated-site links and fragments'
python3 scripts/check-site-links.py dist/site

echo 'Avelune development checks PASS'
