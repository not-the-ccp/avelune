#!/usr/bin/env bash
# Optional external benchmark corpus. These downloads are not redistributed by Avelune.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$ROOT/tests/corpus/xiph"}
mkdir -p "$OUT"
fetch() {
  local url=$1 out=$2
  if command -v curl >/dev/null; then curl -fL --retry 3 --continue-at - "$url" -o "$out"
  elif command -v wget >/dev/null; then wget -c "$url" -O "$out"
  else echo "need curl or wget" >&2; exit 1; fi
}
fetch https://media.xiph.org/video/derf/y4m/bus_qcif_15fps.y4m "$OUT/bus_qcif_15fps.y4m"
fetch https://media.xiph.org/video/derf/y4m/foreman_qcif.y4m "$OUT/foreman_qcif.y4m"
cat > "$OUT/PROVENANCE.txt" <<'TXT'
Downloaded from Xiph.org's Derf video test-media collection.
Consult https://media.xiph.org/video/derf/ and the per-sequence readmes/copyright files before redistribution.
Bus is listed as a 150-frame test sequence; this script selects its 15-fps QCIF Y4M.
Foreman is listed as a 300-frame test sequence; this script selects its QCIF Y4M.
TXT
printf 'downloaded Xiph corpus to %s\n' "$OUT"
