#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
cd "$ROOT"
mkdir -p web/player
cargo build --release -p avelune-prod-wasm --target wasm32-unknown-unknown --locked
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_prod_wasm.wasm" web/player/avelune-prod-scalar.wasm
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+simd128" cargo build --release -p avelune-prod-wasm --target wasm32-unknown-unknown --locked
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_prod_wasm.wasm" web/player/avelune-prod-simd128.wasm
printf 'wrote %s and %s\n' web/player/avelune-prod-scalar.wasm web/player/avelune-prod-simd128.wasm
