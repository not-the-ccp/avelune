#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
cd "$ROOT"
cargo build --release -p avelune-wasm-v1 --target wasm32-unknown-unknown "$@"
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_wasm_v1.wasm" web/player/avelune.wasm
echo "wrote web/player/avelune.wasm"
