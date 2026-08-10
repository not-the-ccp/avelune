#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

echo '[1/9] rustfmt'
cargo fmt --all -- --check

echo '[2/9] clippy'
cargo clippy --workspace --all-targets --locked -- -D warnings

echo '[3/9] tests + doctests'
cargo test --workspace --locked
cargo test --workspace --doc --locked

echo '[4/9] rustdoc'
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked

echo '[5/9] script syntax'
node --check web/player/player.js
node --check web/player/serve.mjs
python3 -m py_compile scripts/benchmark-real.py scripts/benchmark-real-audio.py scripts/benchmark-xiph.py scripts/site-fix-links.py scripts/check-site-links.py
bash -n scripts/build-wasm.sh scripts/build-site.sh scripts/fetch-xiph-corpus.sh scripts/validate-draft.sh

echo '[6/9] cli smoke + completions'
cargo build -p avelune-cli --locked
BIN=${CARGO_TARGET_DIR:-target}/debug/avelune
"$BIN" --version
"$BIN" --help >/dev/null
for sh in bash zsh fish powershell elvish; do "$BIN" completions "$sh" >/dev/null; done

echo '[7/9] wasm (when target installed)'
WASM_LIBDIR=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
if [[ -n $WASM_LIBDIR && -d $WASM_LIBDIR ]]; then
  ./scripts/build-wasm.sh
else
  echo 'SKIP: wasm32-unknown-unknown stdlib is not installed'
fi

echo '[8/9] docs/site'
./scripts/build-site.sh --skip-wasm

echo '[9/9] generated-site links'
python3 scripts/check-site-links.py dist/site

echo 'Avelune development checks PASS'
