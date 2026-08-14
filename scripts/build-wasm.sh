#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
cd "$ROOT"
mkdir -p web/player

cargo build --release -p avelune-wasm --target wasm32-unknown-unknown --locked
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_wasm.wasm" web/player/avelune-scalar.wasm

RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+simd128" \
  cargo build --release -p avelune-wasm --target wasm32-unknown-unknown --locked
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_wasm.wasm" web/player/avelune-simd128.wasm

printf 'wrote %s and %s\n' web/player/avelune-scalar.wasm web/player/avelune-simd128.wasm
