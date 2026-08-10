#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -n "${AVELUNE_RUST_PREFIX:-}" ]; then
  export PATH="$AVELUNE_RUST_PREFIX/bin:$PATH"
fi
exec "$@"
