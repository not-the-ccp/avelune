#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
BASE_SHA=${BASE_SHA:?set BASE_SHA to the PR base commit}
OUT=${AVELUNE_CI_MEDIA_OUT:-results/ci-media}; mkdir -p "$OUT"
TMP=$(mktemp -d); trap 'git worktree remove --force "$TMP/base" >/dev/null 2>&1 || true; rm -rf "$TMP"' EXIT
git worktree add --detach "$TMP/base" "$BASE_SHA" >/dev/null
CARGO_TARGET_DIR="$TMP/base-target" cargo build --manifest-path "$TMP/base/Cargo.toml" -p avelune-cli --release --locked
CARGO_TARGET_DIR="$TMP/head-target" cargo build -p avelune-cli --release --locked
base_bin="$TMP/base-target/release/avelune"; [[ -x "$base_bin" ]] || base_bin="$TMP/base-target/release/avelune.exe"
head_bin="$TMP/head-target/release/avelune"; [[ -x "$head_bin" ]] || head_bin="$TMP/head-target/release/avelune.exe"
base_cli="$base_bin"
# Historical bases may expose the old dual-backend CLI. Wrap only the historical binary so the
# regression workload still targets its production implementation; the current CLI has no backend switch.
if "$base_bin" --help 2>&1 | grep -q -- '--backend'; then
  cat > "$TMP/base-wrapper" <<WRAP
#!/usr/bin/env bash
exec "$base_bin" --backend prod "\$@"
WRAP
  chmod +x "$TMP/base-wrapper"
  base_cli="$TMP/base-wrapper"
fi
python3 scripts/ci-media-regression.py --cli "$base_cli" --repeats "${AVELUNE_CI_MEDIA_REPEATS:-2}" --out-json "$OUT/base.json" --out-csv "$OUT/base.csv"
python3 scripts/ci-media-regression.py --cli "$head_bin" --repeats "${AVELUNE_CI_MEDIA_REPEATS:-2}" --out-json "$OUT/head.json" --out-csv "$OUT/head.csv"
python3 scripts/compare-ci-media.py "$OUT/base.json" "$OUT/head.json" --json "$OUT/comparison.json"
