#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
ARCH=${ARCH:-x86_64}
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
rm -f "$ROOT"/Pupo-*.AppImage

export PATH="$TOOLS:$PATH"
export QMAKE=${QMAKE:-$(command -v qmake6 || command -v qmake)}
# The QML tree is compiled into the binary through qrc, so there is nothing on
# disk for the plugin to scan. It still needs a path to look at to decide which
# QML modules to bundle, and the sources are where the imports are written.
export QML_SOURCES_PATHS="$ROOT/crates/pupo-qt/qml"
# The browser's engine, which the plugin cannot infer.
#
# `BrowserPanel.qml` builds its `WebEngineView` with `Qt.createQmlObject`, so
# the only `import QtWebEngine` in the tree is inside a string — deliberately,
# because a real import would take the whole panel down on a machine without
# the module. The plugin parses imports properly and therefore never sees it,
# and an AppImage built without this line silently ships no engine at all and
# shows the "No browser engine" notice on every machine but the build host.
QT_WEBENGINE_QML="$(dirname "$(qmake6 -query QT_INSTALL_QML 2>/dev/null)/QtWebEngine")/QtWebEngine"
if [[ -d $QT_WEBENGINE_QML ]]; then
    export EXTRA_QT_MODULES="webenginecore;webchannel;positioning"
else
    echo "note: QtWebEngine not installed; the AppImage will have no browser engine" >&2
fi
# Allows the deployment tools to run on machines where FUSE is unavailable.
export APPIMAGE_EXTRACT_AND_RUN=1

"$TOOLS/linuxdeploy-${ARCH}.AppImage" \
    --appdir "$APPDIR" \
    --executable "$ROOT/target/release/pupo" \
    --desktop-file "$ROOT/packaging/pupo.desktop" \
    --icon-file "$ROOT/packaging/pupo.svg" \
    --plugin qt

# The QtWebEngine QML module, and the plugin's own dependencies.
#
# `EXTRA_QT_MODULES` above deploys Chromium, the helper process and the
# resources, but not the QML module that `import QtWebEngine` resolves to — the
# plugin only deploys QML modules it found an import for. Copied by hand, then
# handed back to linuxdeploy with `--deploy-deps-only` so its libraries are
# pulled in *and its RUNPATH is rewritten*. Without that second step the plugin
# still resolves `libQt6WebEngineCore` against /usr/lib on the build host and
# the bundle only works on a machine that already has QtWebEngine.
if [[ -d ${QT_WEBENGINE_QML:-} ]]; then
    qml_root=$(dirname "$QT_WEBENGINE_QML")
    mkdir -p "$APPDIR/usr/qml"
    cp -r "$QT_WEBENGINE_QML" "$APPDIR/usr/qml/"
    [[ -d $qml_root/QtWebChannel ]] && cp -r "$qml_root/QtWebChannel" "$APPDIR/usr/qml/"
    "$TOOLS/linuxdeploy-${ARCH}.AppImage" --appdir "$APPDIR" \
        --deploy-deps-only "$APPDIR/usr/qml/QtWebEngine" \
        --deploy-deps-only "$APPDIR/usr/qml/QtWebChannel"
fi

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

"$TOOLS/appimagetool-${ARCH}.AppImage" --runtime-file "$TOOLS/runtime-${ARCH}" \
    "$APPDIR" "$ROOT/dist/Pupo-${ARCH}.AppImage"

if [[ ! -f "$ROOT/dist/Pupo-${ARCH}.AppImage" ]]; then
    echo "AppImage output was not created" >&2
    exit 1
fi
chmod +x "$ROOT/dist/Pupo-${ARCH}.AppImage"
echo "Created dist/Pupo-${ARCH}.AppImage"
