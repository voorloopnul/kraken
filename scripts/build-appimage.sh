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
rm -f "$ROOT"/Kraken-*.AppImage

export PATH="$TOOLS:$PATH"
export QMAKE=${QMAKE:-$(command -v qmake6 || command -v qmake)}
# The QML tree is compiled into the binary through qrc, so there is nothing on
# disk for the plugin to scan. It still needs a path to look at to decide which
# QML modules to bundle, and the sources are where the imports are written.
export QML_SOURCES_PATHS="$ROOT/crates/kraken-qt/qml"
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
    --executable "$ROOT/target/release/kraken" \
    --desktop-file "$ROOT/packaging/kraken.desktop" \
    --icon-file "$ROOT/packaging/kraken.svg" \
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

# The Pi coding agent, and the Node runtime that runs it.
#
# `PiAgent` spawns a bare `pi` (crates/kraken-core/src/pi/rpc.rs), so without
# this the AppImage only talks to an agent on machines where the user has
# already installed one. Pi ships as an npm package of JavaScript, so the whole
# package — its bundled `node_modules` included — is copied next to a copy of
# the build host's `node`, and `usr/bin/pi` is a launcher that pairs the two.
PI_PACKAGE=${PI_PACKAGE:-$(npm root -g 2>/dev/null)/@earendil-works/pi-coding-agent}
NODE_BIN=${NODE_BIN:-$(command -v node || true)}
if [[ -d $PI_PACKAGE && -x ${NODE_BIN:-} ]]; then
    mkdir -p "$APPDIR/usr/lib/pi"
    cp -a "$PI_PACKAGE/." "$APPDIR/usr/lib/pi/"
    cp -L "$NODE_BIN" "$APPDIR/usr/bin/node"
    cat > "$APPDIR/usr/bin/pi" <<'LAUNCHER'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/node" "$here/../lib/pi/dist/cli.js" "$@"
LAUNCHER
    chmod +x "$APPDIR/usr/bin/pi"
else
    echo "note: pi not installed; the AppImage will have no coding agent" >&2
fi

# Everything Qt deployed that Kraken has no use for.
#
# The Qt plugin bundles all of QtWebEngine's locales and all of Qt's own
# translations, on the assumption that the application is translated. Kraken is
# not: its interface is English, so the one locale Chromium falls back to is the
# only one worth carrying, and the `qtbase_*.qm` catalogues only ever translate
# the standard dialog buttons. The devtools pack is Chromium's inspector UI,
# which no panel opens.
KEEP_LOCALE=${KEEP_LOCALE:-en-US}
locales="$APPDIR/usr/translations/qtwebengine_locales"
if [[ -d $locales ]]; then
    find "$locales" -name '*.pak' ! -name "${KEEP_LOCALE}.pak" -delete
fi
rm -f "$APPDIR"/usr/translations/qtbase_*.qm
rm -f "$APPDIR/usr/resources/qtwebengine_devtools_resources.pak"

# linuxdeploy leaves AppRun as a symlink to the executable, which runs Kraken
# with the host's PATH and so never finds the bundled `pi`. A launcher puts the
# bundle's own bin directory first instead, so the agent that ships with the
# AppImage is the agent it runs.
rm -f "$APPDIR/AppRun"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
export PATH="$here/usr/bin:$PATH"
exec "$here/usr/bin/kraken" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

"$TOOLS/appimagetool-${ARCH}.AppImage" --runtime-file "$TOOLS/runtime-${ARCH}" \
    "$APPDIR" "$ROOT/dist/Kraken-${ARCH}.AppImage"

if [[ ! -f "$ROOT/dist/Kraken-${ARCH}.AppImage" ]]; then
    echo "AppImage output was not created" >&2
    exit 1
fi
chmod +x "$ROOT/dist/Kraken-${ARCH}.AppImage"
echo "Created dist/Kraken-${ARCH}.AppImage"
