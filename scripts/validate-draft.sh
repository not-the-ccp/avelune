#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
BIN="$CARGO_TARGET_DIR/release/avelune"
cd "$ROOT"

cargo fmt --all -- --check
cargo test --workspace --locked
cargo build --release --workspace --locked
./scripts/build-wasm.sh

rm -rf dist/conformance
"$BIN" conformance dist/conformance
"$BIN" verify dist/conformance/video-lossless.avl
"$BIN" verify dist/conformance/video-lossy.avl
if "$BIN" verify dist/conformance/reject-corrupt.avl >/dev/null 2>&1; then
  echo 'corrupt conformance file was accepted' >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
ffmpeg -y -loglevel error -f lavfi -i 'testsrc2=size=320x180:rate=30' \
  -f lavfi -i 'sine=frequency=997:sample_rate=48000' -t 2 -pix_fmt yuv420p \
  -c:v libx264 -preset ultrafast -c:a aac -ac 2 "$TMP/source.mp4"
"$BIN" encode "$TMP/source.mp4" web/player/demo.avl --seconds 2 --size 320x180 \
  --video-q 128 --audio-q 1 --epoch 30 --preset balanced
"$BIN" verify web/player/demo.avl
node scripts/http-wasm-range-smoke.mjs web/player/demo.avl web/player/avelune.wasm | tee dist/http-range-smoke.json
"$BIN" reindex web/player/demo.avl "$TMP/reindexed.avl"
"$BIN" verify "$TMP/reindexed.avl"
"$BIN" fuzz-smoke web/player/demo.avl 5000 | tee dist/fuzz-smoke.txt

echo 'Avelune Draft Generation 1 deep validation PASS'
