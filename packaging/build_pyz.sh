#!/bin/bash
# Build kraken.pyz: bundle the kraken package, the bootstrap __main__, and the
# native libghostty-vt library into a single self-installing zipapp.
#
# Run from anywhere; paths are resolved relative to the repo root.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
STAGE="$(mktemp -d)"
OUT="$REPO/kraken.pyz"
LIB="$REPO/vendor/ghostty/zig-out/lib/libghostty-vt.so"

trap 'rm -rf "$STAGE"' EXIT

if [ ! -e "$LIB" ]; then
    echo "error: $LIB not found. Build libghostty-vt first" \
         "(cd vendor/ghostty && zig build -Demit-lib-vt)." >&2
    exit 1
fi

mkdir -p "$STAGE/data"

# App code (exclude bytecode caches).
cp -r "$REPO/kraken" "$STAGE/kraken"
find "$STAGE/kraken" -name '__pycache__' -type d -prune -exec rm -rf {} +
find "$STAGE/kraken" -name '*.py[co]' -delete

# Bootstrap entry point.
cp "$HERE/__main__.py" "$STAGE/__main__.py"

# Native library (resolve the symlink to the real file).
cp -L "$LIB" "$STAGE/data/libghostty-vt.so"

# Build the zipapp with a portable shebang, compressed.
python3 -m zipapp "$STAGE" \
    --output "$OUT" \
    --python "/usr/bin/env python3" \
    --compress

chmod +x "$OUT"
echo "Built $OUT"
ls -lh "$OUT"
