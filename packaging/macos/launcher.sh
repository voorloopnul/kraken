#!/bin/bash
# Contents/Resources/launcher.sh: the bundle's real entry point, exec'd by the
# native stub at Contents/MacOS/kraken (see launcher.c for why that stub
# exists rather than this script sitting there directly). The macOS
# counterpart of the AppImage's AppRun — everything Kraken needs (a private
# Python 3.14 + PySide6, a Node runtime, the Pi coding agent, and
# libghostty-vt) lives alongside this script under Contents/Resources, so it
# only points the app at those bundled copies and execs it.
set -euo pipefail

RES="$(cd "$(dirname "$0")" && pwd)"

# An app opened from the Dock or Finder inherits launchd's PATH, not the one
# from the user's shell: /usr/bin:/bin:/usr/sbin:/sbin and nothing else. The
# Homebrew prefixes are where a Mac keeps the tools the agent shells out to
# (git beyond Xcode's, ripgrep, language runtimes), so put them back.
export PATH="$RES/node/bin:$RES/pi/node_modules/.bin:$PATH:/opt/homebrew/bin:/usr/local/bin"

# The terminal backend loads this via ctypes; ghostty_vt.py honours the override.
export KRAKEN_GHOSTTY_VT_LIB="$RES/lib/libghostty-vt.dylib"

# Make `import kraken` resolve to the bundled source tree.
export PYTHONPATH="$RES/share/kraken${PYTHONPATH:+:$PYTHONPATH}"

# Cap Chromium at one shared renderer process. Unlike the AppImage, the sandbox
# stays on: macOS's Chromium sandbox needs no setuid helper, so there is
# nothing to disable.
export QTWEBENGINE_CHROMIUM_FLAGS="--renderer-process-limit=1 ${QTWEBENGINE_CHROMIUM_FLAGS:-}"

# Unlike the AppImage's read-only squashfs, Contents/Resources is a plain
# writable directory — CPython would otherwise drop __pycache__/*.pyc into it
# on first launch, and those files aren't in build_app.sh's codesign seal, so
# the very first run invalidates the bundle's signature.
export PYTHONDONTWRITEBYTECODE=1

exec "$RES/python/bin/python3" "$RES/share/kraken/main.py" "$@"
