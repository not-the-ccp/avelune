#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

SKIP_WASM=0
[[ ${1:-} == --skip-wasm ]] && SKIP_WASM=1

command -v node >/dev/null || { echo 'Node.js is required to build the static site' >&2; exit 1; }
command -v npm >/dev/null || { echo 'npm is required to build the static site' >&2; exit 1; }
[[ -x node_modules/.bin/astro ]] || {
  echo 'Site dependencies are not installed; run npm ci explicitly before building' >&2
  exit 1
}

npm run check:content

if [[ $SKIP_WASM -eq 0 ]]; then
  ./scripts/build-wasm.sh
fi

cargo doc --workspace --no-deps --locked
npm run build

OUT=dist/site
DOCROOT=${CARGO_TARGET_DIR:-target}/doc
mkdir -p "$OUT/api/rust" "$OUT/demo"
cp -a "$DOCROOT"/. "$OUT/api/rust/"
cp web/player/player.js web/player/demo.avl "$OUT/demo/"
if [[ -f web/player/avelune.wasm ]]; then
  cp web/player/avelune.wasm "$OUT/demo/"
fi
cp -a web/webgpu "$OUT/demo/webgpu"

python3 scripts/check-site-links.py "$OUT"
find "$OUT" -type f | sort > "$OUT/FILES.txt"
echo "built $OUT"
