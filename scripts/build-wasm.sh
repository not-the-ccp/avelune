#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
cd "$ROOT"
mkdir -p web/player

build_with_feature() {
  local feature=$1
  local encoded=${CARGO_ENCODED_RUSTFLAGS-}
  local plain=${RUSTFLAGS-}
  if [[ ${CARGO_ENCODED_RUSTFLAGS+x} ]]; then
    local sep=''
    [[ -n $encoded ]] && sep=$'\x1f'
    CARGO_ENCODED_RUSTFLAGS="${encoded}${sep}-Ctarget-feature=${feature}" \
      cargo build --release -p avelune-wasm --target wasm32-unknown-unknown --locked
  else
    RUSTFLAGS="${plain}${plain:+ }-C target-feature=${feature}" \
      cargo build --release -p avelune-wasm --target wasm32-unknown-unknown --locked
  fi
}

build_with_feature -simd128
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_wasm.wasm" web/player/avelune-scalar.wasm

build_with_feature +simd128
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/avelune_wasm.wasm" web/player/avelune-simd128.wasm

printf 'wrote %s and %s\n' web/player/avelune-scalar.wasm web/player/avelune-simd128.wasm
