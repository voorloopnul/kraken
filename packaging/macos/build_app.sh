#!/bin/bash
# Build Kraken-<version>.dmg: a self-contained Apple silicon app bundle with a
# private Python 3.14 + PySide6/Qt, a Node.js runtime, the Pi coding agent,
# the libghostty-vt terminal core, and the kraken app source, packed into a
# compressed disk image. The end user opens the .dmg and drags Kraken.app to
# /Applications — no system Python, Node, or uv needed. The uncompressed
# build/macos/Kraken.app underneath is kept around too, for local runs.
#
# The AppImage script is the sibling of this one and the two stage the same
# things; what differs is the container (an app bundle + dmg rather than a
# squashfs image), the Info.plist macOS needs to treat it as an app, and the
# ad-hoc signature Apple silicon needs to run it at all.
#
# Must run ON an Apple silicon Mac: PySide6 ships Qt as prebuilt frameworks per
# platform, and codesign/sips/iconutil/clang are macOS-only. The one part that
# does cross-compile is libghostty-vt — see GHOSTTY_LIB below.
#
#   packaging/macos/build_app.sh
#
# Environment overrides:
#   NODE_MAJOR    Node major line to bundle (default 22, the Pi agent minimum).
#   GHOSTTY_LIB   Path to a prebuilt libghostty-vt.dylib. Defaults to the
#                 vendored build tree. A Linux box with zig can produce one:
#                 (cd vendor/ghostty && zig build -Demit-lib-vt -Dtarget=aarch64-macos)
#   SIGN_IDENTITY Codesigning identity (default "-", i.e. ad-hoc). A Developer
#                 ID is only needed to hand the bundle to someone else.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
NODE_MAJOR="${NODE_MAJOR:-22}"
PY_VERSION="3.14"
PI_PACKAGE="@earendil-works/pi-coding-agent"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"

BUILD="$REPO/build/macos"
APP="$BUILD/Kraken.app"
CONTENTS="$APP/Contents"
RES="$CONTENTS/Resources"
GHOSTTY_LIB="${GHOSTTY_LIB:-$REPO/vendor/ghostty/zig-out/lib/libghostty-vt.dylib}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO/pyproject.toml" | head -1)"

log() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "this builds a macOS bundle and must run on macOS."
[ "$(uname -m)" = "arm64" ] || die "Apple silicon only (got $(uname -m))."
command -v uv >/dev/null || die "uv not found (https://docs.astral.sh/uv/)."
command -v curl >/dev/null || die "curl not found."
command -v clang >/dev/null || die "clang not found (install the Xcode Command Line Tools: xcode-select --install)."
[ -e "$GHOSTTY_LIB" ] || die "libghostty-vt.dylib not found at $GHOSTTY_LIB. Run: (cd vendor/ghostty && zig build -Demit-lib-vt)"
[ -n "$VERSION" ] || die "could not read version from pyproject.toml."

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$RES/lib" "$RES/share/kraken"

# --------------------------------------------------------------------------
# 1. Private Python + the project's locked dependencies
#    Copy a standalone (relocatable) CPython, then install the wheels into the
#    copy so the whole tree can move into the bundle. python-build-standalone
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
# `cd && pwd -P` rather than readlink -f, which BSD readlink lacks.
PY_ROOT="$(cd "$UV_PYTHON_INSTALL_DIR/cpython-$PY_VERSION-macos-aarch64-none" && pwd -P)"
[ -x "$PY_ROOT/bin/python3" ] || die "standalone Python not found under $PY_ROOT"
cp -a "$PY_ROOT" "$RES/python"
APP_PY="$RES/python/bin/python3"
# uv marks its managed interpreters PEP-668 "externally managed" so pip refuses
# to touch them. This is our private, disposable copy, so lift the guard.
rm -f "$RES"/python/lib/python*/EXTERNALLY-MANAGED

log "Installing the project's dependencies into the bundled Python"
# From the lockfile, not a hand-written list, so the bundle stays honest to
# pyproject.toml by construction — see the AppImage script for the bug that
# taught us this. --no-emit-project: the app runs from Resources/share/kraken,
# not as an installed package. --project: uv would otherwise resolve from the
# caller's cwd, and this script promises to run from anywhere.
REQS="$BUILD/requirements.txt"
uv export --project "$REPO" --no-dev --no-emit-project \
  --format requirements-txt -o "$REQS" -q
uv pip install --system --python "$APP_PY" -r "$REQS"

# PyPI has no arm64-only macOS wheel for Qt, so PySide6 installs as universal2
# (x86_64 + arm64) — every .dylib/.so/framework binary in it carries a whole
# extra copy of itself compiled for x86_64, dead weight on the Apple-silicon-
# only bundle this script produces (see the uname check above). Thinning them
# to arm64 cuts PySide6 roughly in half; `file`/`perm -111` catches the
# non-suffixed Mach-O binaries frameworks keep under Versions/A alongside the
# .so/.dylib ones.
log "Thinning universal2 binaries to arm64"
THINNED=0
while IFS= read -r -d '' f; do
  arches="$(lipo -archs "$f" 2>/dev/null || true)"
  # A single-arch file (or something lipo can't read at all) reports no space.
  [[ "$arches" == *" "* ]] || continue
  mode="$(stat -f '%A' "$f")"
  lipo "$f" -thin arm64 -output "$f.thin"
  mv "$f.thin" "$f"
  chmod "$mode" "$f"
  THINNED=$((THINNED + 1))
done < <(find "$RES/python" -type f \( -name '*.so' -o -name '*.dylib' -o -perm -111 \) -print0)
log "Thinned $THINNED universal2 binaries"

# --------------------------------------------------------------------------
# 2. Node.js runtime
# --------------------------------------------------------------------------
log "Resolving latest Node $NODE_MAJOR.x"
NODE_VER="$(curl -fsSL https://nodejs.org/dist/index.json \
  | grep -oE "\"version\":\"v$NODE_MAJOR\.[0-9]+\.[0-9]+\"" \
  | head -1 | grep -oE "v$NODE_MAJOR\.[0-9]+\.[0-9]+")"
[ -n "$NODE_VER" ] || die "could not resolve a Node $NODE_MAJOR.x version."
NODE_TARBALL="node-$NODE_VER-darwin-arm64.tar.xz"
log "Fetching $NODE_TARBALL"
curl -fsSL "https://nodejs.org/dist/$NODE_VER/$NODE_TARBALL" -o "$BUILD/$NODE_TARBALL"
mkdir -p "$RES/node"
tar -xJf "$BUILD/$NODE_TARBALL" -C "$RES/node" --strip-components=1
rm -f "$BUILD/$NODE_TARBALL"

# --------------------------------------------------------------------------
# 3. Pi coding agent (installed with the bundled npm into a private prefix)
# --------------------------------------------------------------------------
log "Installing Pi agent ($PI_PACKAGE)"
mkdir -p "$RES/pi"
PATH="$RES/node/bin:$PATH" \
  "$RES/node/bin/npm" install \
    --prefix "$RES/pi" --no-fund --no-audit "$PI_PACKAGE"

# --------------------------------------------------------------------------
# 4. Native terminal library + app source + entry point
# --------------------------------------------------------------------------
log "Staging libghostty-vt, kraken source, and entry point"
cp -L "$GHOSTTY_LIB" "$RES/lib/libghostty-vt.dylib"
cp -r "$REPO/kraken" "$RES/share/kraken/kraken"
find "$RES/share/kraken/kraken" -name '__pycache__' -type d -prune -exec rm -rf {} +
find "$RES/share/kraken/kraken" -name '*.py[co]' -delete
cp "$REPO/main.py" "$RES/share/kraken/main.py"
cp "$HERE/launcher.sh" "$RES/launcher.sh"
chmod +x "$RES/launcher.sh"
# Contents/MacOS/kraken must be a real Mach-O binary, not launcher.sh itself:
# LaunchServices decides whether an app is native by reading that file's
# arch load commands, and a shell script has none — it reads as non-native
# and macOS offers to install Rosetta before running something that, once
# running, never executes an x86_64 instruction. This stub just execs
# launcher.sh; see launcher.c.
clang -O2 -arch arm64 -o "$CONTENTS/MacOS/kraken" "$HERE/launcher.c"

# --------------------------------------------------------------------------
# 5. Bundle metadata: Info.plist and the icon
# --------------------------------------------------------------------------
log "Writing Info.plist and icon"
sed "s/@VERSION@/$VERSION/g" "$HERE/Info.plist" > "$CONTENTS/Info.plist"

# .icns is the only icon format the Dock and Finder read, and iconutil builds
# one from a directory of the sizes macOS asks for at each scale factor.
ICONSET="$BUILD/kraken.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$REPO/kraken/ui/assets/dark.png" \
    --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$REPO/kraken/ui/assets/dark.png" \
    --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$RES/kraken.icns"
rm -rf "$ICONSET"

# --------------------------------------------------------------------------
# 6. Sign
#    Apple silicon refuses to run unsigned native code, so this is not a
#    distribution step you can skip — the bundle will not launch without it.
#    Everything we copied in (CPython, the Qt frameworks, the .dylib) carries
#    its own signature already; --deep would re-sign all of it, which is both
#    slow and the documented way to break Qt's framework layout, so only the
#    bundle itself is signed here.
#
#    An ad-hoc identity is per-build: macOS sees each rebuild as a different
#    app, so the microphone permission is asked for again after every one.
# --------------------------------------------------------------------------
log "Signing (identity: $SIGN_IDENTITY)"
# libghostty-vt is the one binary we did not get from a signed distribution —
# a cross-compile can arrive without a signature, and dlopen would then fail
# at the first terminal. Nested code is signed before the bundle that seals it.
codesign --force --sign "$SIGN_IDENTITY" "$RES/lib/libghostty-vt.dylib"
codesign --force --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --verbose=2 "$APP"

# --------------------------------------------------------------------------
# 7. Pack into a compressed .dmg
#    Kraken.app above is a plain directory — nothing on disk compresses it,
#    so it reports its full uncompressed size. hdiutil's UDZO format is a
#    zlib-compressed, read-only disk image: what actually ships. The
#    Applications symlink alongside the app is what makes Finder show the
#    drag-to-install arrow.
# --------------------------------------------------------------------------
log "Packing Kraken-$VERSION.dmg"
DMG_STAGE="$BUILD/dmg"
rm -rf "$DMG_STAGE"
mkdir -p "$DMG_STAGE"
cp -R "$APP" "$DMG_STAGE/"
ln -s /Applications "$DMG_STAGE/Applications"
DMG="$REPO/Kraken-$VERSION.dmg"
rm -f "$DMG"
hdiutil create -volname "Kraken" -srcfolder "$DMG_STAGE" -fs HFS+ \
  -format UDZO -imagekey zlib-level=9 -ov -quiet "$DMG"
rm -rf "$DMG_STAGE"

log "Done"
du -sh "$APP"
du -sh "$DMG"
printf 'open %s\n' "$APP"
printf '%s\n' "$DMG"
