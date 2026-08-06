#!/bin/bash
# Build Kraken-x86_64.AppImage: a single self-contained file bundling a private
# Python 3.14 + PySide6/Qt, a Node.js runtime, the Pi coding agent, the
# libghostty-vt terminal core, and the kraken app source. The end user just
# downloads it, `chmod +x`, and runs it — no system Python, Node, or uv needed.
#
# Run from anywhere; paths resolve relative to the repo root.
#
#   packaging/appimage/build_appimage.sh
#
# Environment overrides:
#   NODE_MAJOR   Node major line to bundle (default 22, the Pi agent minimum).
#   ARCH         Target arch (default x86_64; only x86_64 is wired up here).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ARCH="${ARCH:-x86_64}"
NODE_MAJOR="${NODE_MAJOR:-22}"
PY_VERSION="3.14"
PI_PACKAGE="@earendil-works/pi-coding-agent"

BUILD="$REPO/build/appimage"
APPDIR="$BUILD/Kraken.AppDir"
TOOLS="$HERE/tools"                     # cached downloads (appimagetool)
OUT="$REPO/Kraken-$ARCH.AppImage"
GHOSTTY_LIB="$REPO/vendor/ghostty/zig-out/lib/libghostty-vt.so"

log() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

command -v uv >/dev/null || die "uv not found (https://docs.astral.sh/uv/)."
command -v curl >/dev/null || die "curl not found."
[ -e "$GHOSTTY_LIB" ] || die "libghostty-vt not built. Run: (cd vendor/ghostty && zig build -Demit-lib-vt)"
[ "$ARCH" = "x86_64" ] || die "only x86_64 is wired up (got $ARCH)."

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/lib" "$APPDIR/usr/share/kraken" "$TOOLS"

# --------------------------------------------------------------------------
# 1. Private Python + the project's locked dependencies
#    Copy a standalone (relocatable) CPython, then install the wheels into the
#    copy so the whole tree can move into the AppImage. python-build-standalone
#    locates itself relative to its binary, so it survives relocation.
# --------------------------------------------------------------------------
log "Provisioning Python $PY_VERSION (standalone)"
# Install into an isolated dir so we grab the relocatable standalone build and
# never the repo's own .venv (which `uv python find` would prefer, and which is
# not relocatable). The only cpython-* dir here is the one we just installed.
export UV_PYTHON_INSTALL_DIR="$BUILD/pythons"
rm -rf "$UV_PYTHON_INSTALL_DIR"
uv python install "$PY_VERSION"
# uv drops a stable-name symlink (cpython-3.14-...) next to the concrete build
# (cpython-3.14.6-...); resolve it to the real, self-contained directory.
PY_ROOT="$(readlink -f "$UV_PYTHON_INSTALL_DIR/cpython-$PY_VERSION-linux-x86_64-gnu")"
[ -x "$PY_ROOT/bin/python3" ] || die "standalone Python not found under $PY_ROOT"
cp -a "$PY_ROOT" "$APPDIR/usr/python"
APP_PY="$APPDIR/usr/python/bin/python3"
# uv marks its managed interpreters PEP-668 "externally managed" so pip refuses
# to touch them. This is our private, disposable copy, so lift the guard.
rm -f "$APPDIR"/usr/python/lib/python*/EXTERNALLY-MANAGED

log "Installing the project's dependencies into the bundled Python"
# From the lockfile, not a hand-written list. The list here used to be
# "pyside6 pygments", which quietly dropped onnx-asr — so the AppImage shipped
# without numpy/onnxruntime/huggingface_hub, and voice transcription died on
# `import numpy` at the moment someone spoke into it, having already recorded
# the audio. Only a build could show that: a source checkout has them from
# .venv. Exporting keeps the bundle honest to pyproject.toml by construction.
# --no-emit-project: the app runs from usr/share/kraken, not as an installed
# package, so we want its dependencies and not kraken itself.
# --system: the target is a base (non-venv) interpreter, so opt in explicitly.
# --project: uv would otherwise resolve from the caller's cwd, and this script
# promises to run from anywhere. Inside another uv project that would bundle
# *its* dependencies — the same silent-wrong-deps failure the export is here
# to prevent, wearing a different hat.
REQS="$BUILD/requirements.txt"
uv export --project "$REPO" --no-dev --no-emit-project \
  --format requirements-txt -o "$REQS" -q
uv pip install --system --python "$APP_PY" -r "$REQS"

# --------------------------------------------------------------------------
# 2. Node.js runtime
# --------------------------------------------------------------------------
log "Resolving latest Node $NODE_MAJOR.x"
NODE_VER="$(curl -fsSL https://nodejs.org/dist/index.json \
  | grep -oE "\"version\":\"v$NODE_MAJOR\.[0-9]+\.[0-9]+\"" \
  | head -1 | grep -oE "v$NODE_MAJOR\.[0-9]+\.[0-9]+")"
[ -n "$NODE_VER" ] || die "could not resolve a Node $NODE_MAJOR.x version."
NODE_TARBALL="node-$NODE_VER-linux-x64.tar.xz"
log "Fetching $NODE_TARBALL"
curl -fsSL "https://nodejs.org/dist/$NODE_VER/$NODE_TARBALL" -o "$BUILD/$NODE_TARBALL"
mkdir -p "$APPDIR/usr/node"
tar -xJf "$BUILD/$NODE_TARBALL" -C "$APPDIR/usr/node" --strip-components=1
rm -f "$BUILD/$NODE_TARBALL"

# --------------------------------------------------------------------------
# 3. Pi coding agent (installed with the bundled npm into a private prefix)
# --------------------------------------------------------------------------
log "Installing Pi agent ($PI_PACKAGE)"
mkdir -p "$APPDIR/usr/pi"
PATH="$APPDIR/usr/node/bin:$PATH" \
  "$APPDIR/usr/node/bin/npm" install \
    --prefix "$APPDIR/usr/pi" --no-fund --no-audit "$PI_PACKAGE"

# --------------------------------------------------------------------------
# 4. Native terminal library + app source + entry point
# --------------------------------------------------------------------------
log "Staging libghostty-vt, kraken source, and entry point"
cp -L "$GHOSTTY_LIB" "$APPDIR/usr/lib/libghostty-vt.so"
cp -r "$REPO/kraken" "$APPDIR/usr/share/kraken/kraken"
find "$APPDIR/usr/share/kraken/kraken" -name '__pycache__' -type d -prune -exec rm -rf {} +
find "$APPDIR/usr/share/kraken/kraken" -name '*.py[co]' -delete
cp "$REPO/main.py" "$APPDIR/usr/share/kraken/main.py"

# --------------------------------------------------------------------------
# 5. AppDir metadata: AppRun, desktop entry, icon
# --------------------------------------------------------------------------
log "Adding AppRun, desktop entry, and icon"
cp "$HERE/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"
cp "$HERE/kraken.desktop" "$APPDIR/kraken.desktop"
cp "$REPO/kraken/ui/assets/dark.png" "$APPDIR/kraken.png"
# appimagetool also looks for the icon under the hicolor theme.
install -Dm644 "$APPDIR/kraken.png" \
  "$APPDIR/usr/share/icons/hicolor/1024x1024/apps/kraken.png"

# --------------------------------------------------------------------------
# 6. Pack the AppDir into a single AppImage
# --------------------------------------------------------------------------
APPIMAGETOOL="$TOOLS/appimagetool-x86_64.AppImage"
if [ ! -x "$APPIMAGETOOL" ]; then
  log "Fetching appimagetool"
  curl -fsSL \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
    -o "$APPIMAGETOOL"
  chmod +x "$APPIMAGETOOL"
fi

log "Packing AppImage"
# --appimage-extract-and-run avoids needing FUSE on the build host.
ARCH="$ARCH" "$APPIMAGETOOL" --appimage-extract-and-run "$APPDIR" "$OUT"

log "Done"
ls -lh "$OUT"
