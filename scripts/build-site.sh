#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
SKIP_WASM=0
[[ ${1:-} == --skip-wasm ]] && SKIP_WASM=1
command -v pandoc >/dev/null || { echo 'pandoc is required to build the static site' >&2; exit 1; }
OUT=dist/site
rm -rf "$OUT"
mkdir -p "$OUT/assets" "$OUT/docs" "$OUT/spec" "$OUT/demo"
cp site/style.css "$OUT/assets/style.css"

render() {
  local src=$1 out=$2 title=$3 rootrel=$4
  mkdir -p "$(dirname "$out")"
  pandoc "$src" --from=gfm --to=html5 --standalone --template=site/template.html \
    --metadata="title:$title" --metadata="root:$rootrel" -o "$out"
}

render README.md "$OUT/index.html" 'Avelune' '.'

# Human-facing docs.
cat > "$OUT/docs/index.md" <<'DOC'
# Documentation

<ul class="cards">
<li><a href="../STATUS.html"><strong>Project status</strong></a><br>Current scope and limitations.</li>
<li><a href="user/CLI.html"><strong>CLI guide</strong></a><br>Encoding, decoding, inspection and completions.</li>
<li><a href="development/VERSIONING.html"><strong>Versioning</strong></a><br>Draft vs stable compatibility policy.</li>
<li><a href="development/REFERENCE_IMPLEMENTATION.html"><strong>Reference implementation</strong></a><br>What the Rust implementation is and is not.</li>
<li><a href="browser/streaming-contract.html"><strong>Browser streaming</strong></a><br>Range-fetch and WASM contract.</li>
<li><a href="browser/webgpu-backend.html"><strong>WebGPU direction</strong></a><br>Experimental hybrid GPU backend research.</li>
</ul>
DOC
render "$OUT/docs/index.md" "$OUT/docs/index.html" 'Documentation' '..'
rm "$OUT/docs/index.md"
while IFS= read -r f; do
  rel=${f#docs/}; out="$OUT/docs/${rel%.md}.html"
  depth=$(awk -F/ '{print NF}' <<<"$rel")
  rootrel=$(printf '../%.0s' $(seq 1 $depth)); rootrel=${rootrel%/}
  render "$f" "$out" "$(basename "${rel%.md}")" "$rootrel"
done < <(find docs -type f -name '*.md' | sort)
render STATUS.md "$OUT/STATUS.html" 'Project status' '.'
render CONTRIBUTING.md "$OUT/CONTRIBUTING.html" 'Contributing' '.'
render AGENTS.md "$OUT/AGENTS.html" 'Agent/contributor guidance' '.'
render SECURITY.md "$OUT/SECURITY.html" 'Security' '.'

# Normative draft index + documents.
cat > "$OUT/spec/index.md" <<'SPEC'
# Normative draft specification

These documents describe **Draft Generation 1**. They are not frozen and may change incompatibly.

- [Specification overview](README.html)
- [Baseline profile](001-v1-baseline-profile.html)
- [Entropy coding](common/001-entropy-v1.html)
- [Container](container/001-container-v1.html)
- [Video](video/001-video-v1.html)
- [Audio](audio/001-audio-v1.html)
- [Conformance](conformance/001-v1.html)
SPEC
render "$OUT/spec/index.md" "$OUT/spec/index.html" 'Draft specification' '..'
rm "$OUT/spec/index.md"
while IFS= read -r f; do
  rel=${f#spec/}; out="$OUT/spec/${rel%.md}.html"
  depth=$(awk -F/ '{print NF}' <<<"$rel")
  rootrel=$(printf '../%.0s' $(seq 1 $depth)); rootrel=${rootrel%/}
  render "$f" "$out" "$(basename "${rel%.md}")" "$rootrel"
done < <(find spec -type f -name '*.md' | sort)

# Rust API reference.
cargo doc --workspace --no-deps
DOCROOT=${CARGO_TARGET_DIR:-target}/doc
cp -a "$DOCROOT" "$OUT/api"
cat > "$OUT/api/index.md" <<'API'
# Rust API reference

The [`avelune` facade](avelune/index.html) is the recommended entry point for experiments.
Component/reference crates remain public for low-level work and conformance testing:

- [`avelune`](avelune/index.html) — thin facade over the draft components;
- [`avelune_container_v1`](avelune_container_v1/index.html) — incremental container parser/writer;
- [`avelune_video_v1`](avelune_video_v1/index.html) — Draft Generation 1 video reference codec;
- [`avelune_audio_v1`](avelune_audio_v1/index.html) — Draft Generation 1 audio reference codec;
- [`avelune_video_ref_v1`](avelune_video_ref_v1/index.html) — source-separated scalar video decoder;
- [`avelune_bitstream`](avelune_bitstream/index.html) — common bitstream/entropy primitives;
- [`avelune_wasm_v1`](avelune_wasm_v1/index.html) — raw WASM ABI used by the browser POC.

The API is experimental and may change incompatibly while the software is `0.x`.
API
render "$OUT/api/index.md" "$OUT/api/index.html" 'Rust API reference' '..'
rm "$OUT/api/index.md"

# Browser demo. The demo fixture is committed; the WASM binary is generated.
if [[ $SKIP_WASM -eq 0 ]]; then ./scripts/build-wasm.sh; fi
# The local Range-test server is a development helper, not part of the static Pages artifact.
cp web/player/index.html web/player/player.js web/player/demo.avl "$OUT/demo/"
if [[ -f web/player/avelune.wasm ]]; then cp web/player/avelune.wasm "$OUT/demo/"; fi
cp -a web/webgpu "$OUT/demo/webgpu"

python3 scripts/site-fix-links.py "$OUT"
find "$OUT" -type f | sort > "$OUT/FILES.txt"
echo "built $OUT"
