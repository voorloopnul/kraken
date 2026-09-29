#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
export ARCH=${ARCH:-x86_64}
TOOLS="$ROOT/.tools"
APPDIR="$ROOT/AppDir"
mkdir -p "$TOOLS" "$ROOT/dist"

fetch() {
    local url=$1 output=$2
    if [[ ! -x "$output" ]]; then
        curl -fL --retry 3 -o "$output" "$url"
        chmod +x "$output"
    fi
}

fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${ARCH}.AppImage" \
      "$TOOLS/linuxdeploy-${ARCH}.AppImage"
fetch "https://github.com/linuxdeploy/linuxdeploy-plugin-qt/releases/download/continuous/linuxdeploy-plugin-qt-${ARCH}.AppImage" \
      "$TOOLS/linuxdeploy-plugin-qt-${ARCH}.AppImage"
ln -sf "linuxdeploy-plugin-qt-${ARCH}.AppImage" "$TOOLS/linuxdeploy-plugin-qt"
# appimagetool fetches this itself, but its own download follows no redirects
# and fails on the 302 GitHub answers with. Fetched here, where curl does.
fetch "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-${ARCH}" \
      "$TOOLS/runtime-${ARCH}"
fetch "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage" \
      "$TOOLS/appimagetool-${ARCH}.AppImage"

QMAKE=${QMAKE:-$(command -v qmake6 || command -v qmake)} cargo build --release
rm -rf "$APPDIR"
rm -f "$ROOT"/Kraken-*.AppImage

export PATH="$TOOLS:$PATH"
export QMAKE=${QMAKE:-$(command -v qmake6 || command -v qmake)}
# The QML tree is compiled into the binary through qrc, so there is nothing on
# disk for the plugin to scan. It still needs a path to look at to decide which
# QML modules to bundle, and the sources are where the imports are written.
export QML_SOURCES_PATHS="$ROOT/crates/kraken-qt/qml"
# Allows the deployment tools to run on machines where FUSE is unavailable.
export APPIMAGE_EXTRACT_AND_RUN=1

"$TOOLS/linuxdeploy-${ARCH}.AppImage" \
    --appdir "$APPDIR" \
    --executable "$ROOT/target/release/kraken" \
    --desktop-file "$ROOT/packaging/kraken.desktop" \
    --icon-file "$ROOT/packaging/kraken.svg" \
    --plugin qt

# linuxdeploy intentionally excludes the GLVND loader libraries as host-side
# graphics components. Qt links to libOpenGL directly, however, and minimal
# distributions do not always provide it. Bundle the vendor-neutral loaders;
# actual GPU drivers remain supplied by the host.
copy_host_lib() {
    local name=$1 source
    source=$(ldconfig -p | awk -v name="$name" '$1 == name { print $NF; exit }')
    if [[ -z "$source" || ! -e "$source" ]]; then
        echo "Required host library not found: $name" >&2
        exit 1
    fi
    cp -L "$source" "$APPDIR/usr/lib/$name"
}
copy_host_lib libOpenGL.so.0
copy_host_lib libGLdispatch.so.0
copy_host_lib libGLX.so.0
copy_host_lib libEGL.so.1

# Everything Qt deployed that Kraken has no use for.
#
# The Qt plugin bundles all of Qt's own translations, on the assumption that
# the application is translated. Kraken is not: its interface is English, and
# the `qtbase_*.qm` catalogues only ever translate the standard dialog buttons.
rm -f "$APPDIR"/usr/translations/qtbase_*.qm

"$TOOLS/appimagetool-${ARCH}.AppImage" --runtime-file "$TOOLS/runtime-${ARCH}" \
    "$APPDIR" "$ROOT/dist/Kraken-${ARCH}.AppImage"

if [[ ! -f "$ROOT/dist/Kraken-${ARCH}.AppImage" ]]; then
    echo "AppImage output was not created" >&2
    exit 1
fi
chmod +x "$ROOT/dist/Kraken-${ARCH}.AppImage"
echo "Created dist/Kraken-${ARCH}.AppImage"
