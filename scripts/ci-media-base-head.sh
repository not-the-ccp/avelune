#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
BASE_SHA=${BASE_SHA:?set BASE_SHA to the PR base commit}
OUT=${AVELUNE_CI_MEDIA_OUT:-results/ci-media}; mkdir -p "$OUT"
TMP=$(mktemp -d); trap 'git worktree remove --force "$TMP/base" >/dev/null 2>&1 || true; rm -rf "$TMP"' EXIT
git worktree add --detach "$TMP/base" "$BASE_SHA" >/dev/null
CARGO_TARGET_DIR="$TMP/base-target" cargo build --manifest-path "$TMP/base/Cargo.toml" -p avelune-cli --release --locked
CARGO_TARGET_DIR="$TMP/head-target" cargo build -p avelune-cli --release --locked
for side in base head; do
  cli="$TMP/$side-target/release/avelune"; [[ -x "$cli" ]] || cli="$TMP/$side-target/release/avelune.exe"
  python3 scripts/ci-media-regression.py --cli "$cli" --backend prod --repeats "${AVELUNE_CI_MEDIA_REPEATS:-2}" --out-json "$OUT/$side.json" --out-csv "$OUT/$side.csv"
done
python3 scripts/compare-ci-media.py "$OUT/base.json" "$OUT/head.json" --json "$OUT/comparison.json"
