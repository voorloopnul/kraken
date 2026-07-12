"""Self-installing launcher for Kraken, packaged as a zipapp.

This module runs in whatever Python invokes the ``.pyz``. It is deliberately
written to be compatible with older interpreters (>=3.8): the real app code
(which requires Python 3.14) is only imported *after* we have re-executed
ourselves under the managed virtualenv's interpreter.

Phases, in order:
  1. If PySide6 imports in the current interpreter, extract the bundled
     libghostty-vt library and run the app.
  2. Else, if a managed venv already exists with PySide6, re-exec this same
     archive with that venv's python.
  3. Else, install: ensure ``uv`` is present, create the venv (Python 3.14),
     install dependencies, extract the native library, copy this archive into
     place, drop a launcher shim, then re-exec.
"""

import os
import re
import shutil
import subprocess
import sys
import zipfile

APP_NAME = "kraken"
APP_DISPLAY = "Kraken"
# Third-party runtime dependencies (everything else the app uses is stdlib).
DEPS = ["pyside6>=6.11.1", "pygments>=2.20.0"]
PYTHON_VERSION = "3.14"
# The Pi coding agent is an npm CLI (node dist/cli.js). We provision it via npm
# into a private prefix rather than bundling its ~180MB of JS + native modules.
PI_PACKAGE = "@earendil-works/pi-coding-agent"
NODE_MIN = (22, 19, 0)
# Path inside the archive that holds the native terminal library.
BUNDLED_LIB = "data/libghostty-vt.so"
LIB_BASENAME = "libghostty-vt.so"
LIB_ENV = "KRAKEN_GHOSTTY_VT_LIB"

HOME = os.path.expanduser("~")
INSTALL_ROOT = os.path.join(HOME, ".local", "share", APP_NAME)
VENV_DIR = os.path.join(INSTALL_ROOT, "venv")
LIB_DIR = os.path.join(INSTALL_ROOT, "lib")
PI_DIR = os.path.join(INSTALL_ROOT, "pi")
PI_BIN_DIR = os.path.join(PI_DIR, "node_modules", ".bin")
INSTALLED_PYZ = os.path.join(INSTALL_ROOT, APP_NAME + ".pyz")
BIN_DIR = os.path.join(HOME, ".local", "bin")
LAUNCHER = os.path.join(BIN_DIR, APP_NAME)


# --------------------------------------------------------------------------
# Archive / bundled-data helpers
# --------------------------------------------------------------------------

def _archive_path():
    """Absolute path to the .pyz we are running from."""
    loader = globals().get("__loader__")
    archive = getattr(loader, "archive", None)
    if archive:
        return os.path.abspath(archive)
    return os.path.abspath(sys.argv[0])


def _read_bundled(name):
    with zipfile.ZipFile(_archive_path()) as zf:
        return zf.read(name)


def _venv_python():
    py = os.path.join(VENV_DIR, "bin", "python")
    return py if os.path.exists(py) else None


def _can_import(python_exe, module):
    """True if `module` imports under the given interpreter."""
    try:
        subprocess.run(
            [python_exe, "-c", "import %s" % module],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return True
    except (subprocess.CalledProcessError, OSError):
        return False


def _pyside_here():
    try:
        import PySide6  # noqa: F401
        return True
    except Exception:
        return False


def _ensure_lib_extracted():
    """Extract the bundled native library to LIB_DIR (once) and return its path."""
    dest = os.path.join(LIB_DIR, LIB_BASENAME)
    try:
        bundled = _read_bundled(BUNDLED_LIB)
    except KeyError:
        # No bundled lib (e.g. running from a source checkout): let the app's
        # own resolver find the dev build tree.
        return None
    if not os.path.exists(dest) or os.path.getsize(dest) != len(bundled):
        os.makedirs(LIB_DIR, exist_ok=True)
        tmp = dest + ".tmp"
        with open(tmp, "wb") as fh:
            fh.write(bundled)
        os.chmod(tmp, 0o755)
        os.replace(tmp, dest)
    return dest


# --------------------------------------------------------------------------
# Phase 1: run the app (we are in a PySide6-capable interpreter)
# --------------------------------------------------------------------------

def _run_app():
    lib = _ensure_lib_extracted()
    if lib:
        os.environ.setdefault(LIB_ENV, lib)
    # Put the privately-installed Pi agent on PATH so the app's QProcess, which
    # spawns `pi` by name, resolves our managed copy.
    if os.path.isdir(PI_BIN_DIR):
        os.environ["PATH"] = PI_BIN_DIR + os.pathsep + os.environ.get("PATH", "")
    # Cap Chromium at one shared renderer process before QtWebEngine inits.
    os.environ.setdefault("QTWEBENGINE_CHROMIUM_FLAGS", "--renderer-process-limit=1")

    from PySide6.QtCore import Qt
    from PySide6.QtWidgets import QApplication

    from kraken.shell.main_window import MainWindow

    QApplication.setAttribute(Qt.ApplicationAttribute.AA_ShareOpenGLContexts)
    app = QApplication(sys.argv)
    app.setApplicationName(APP_DISPLAY)
    app.setApplicationDisplayName(APP_DISPLAY)
    window = MainWindow()
    window.show()
    return app.exec()


# --------------------------------------------------------------------------
# Phase 3: install
# --------------------------------------------------------------------------

def _assume_yes():
    return os.environ.get("KRAKEN_ASSUME_YES") == "1"


def _confirm(prompt):
    if _assume_yes():
        return True
    if not sys.stdin.isatty():
        return False
    try:
        reply = input(prompt + " [Y/n] ").strip().lower()
    except EOFError:
        return False
    return reply in ("", "y", "yes")


def _find_uv():
    uv = shutil.which("uv")
    if uv:
        return uv
    # The astral installer drops uv here; it may not be on PATH yet this session.
    for cand in (os.path.join(BIN_DIR, "uv"), os.path.join(HOME, ".cargo", "bin", "uv")):
        if os.path.exists(cand):
            return cand
    return None


def _ensure_uv():
    uv = _find_uv()
    if uv:
        return uv
    print("`uv` (the Python package/Python-version manager) was not found.")
    print("Kraken uses it to create an isolated environment with Python "
          + PYTHON_VERSION + " and PySide6.")
    if not _confirm("Install uv now from https://astral.sh/uv/install.sh ?"):
        print("\nAborted. Install uv yourself (https://docs.astral.sh/uv/) "
              "and re-run this file.", file=sys.stderr)
        sys.exit(1)
    installer = ""
    if shutil.which("curl"):
        installer = "curl -LsSf https://astral.sh/uv/install.sh | sh"
    elif shutil.which("wget"):
        installer = "wget -qO- https://astral.sh/uv/install.sh | sh"
    else:
        print("Neither curl nor wget is available to fetch uv.", file=sys.stderr)
        sys.exit(1)
    subprocess.run(installer, shell=True, check=True)
    uv = _find_uv()
    if not uv:
        print("uv installation did not produce a usable binary.", file=sys.stderr)
        sys.exit(1)
    return uv


def _node_version(node):
    try:
        out = subprocess.run(
            [node, "--version"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
        ).stdout.decode().strip()
    except OSError:
        return None
    m = re.match(r"v?(\d+)\.(\d+)\.(\d+)", out)
    return tuple(int(x) for x in m.groups()) if m else None


def _install_pi():
    """Install the Pi agent via npm into a private prefix.

    If Node is missing or too old, abort installation entirely; missing npm
    just skips the Pi agent install.
    """
    need = ".".join(str(n) for n in NODE_MIN)
    node = shutil.which("node")
    if not node:
        print("\n⚠ Node.js not found. The Pi agent needs Node >= %s." % need)
        print("  Install Node, then re-run Kraken.")
        raise SystemExit(1)
    version = _node_version(node)
    if version is None or version < NODE_MIN:
        have = ".".join(str(n) for n in version) if version else "unknown"
        print("\n⚠ Node %s is too old (need >= %s)." % (have, need))
        print("  Upgrade Node, then re-run Kraken.")
        raise SystemExit(1)
    npm = shutil.which("npm")
    if not npm:
        print("\n⚠ `npm` not found next to Node. Skipping Pi agent install.")
        return False
    print("Installing Pi coding agent (%s) via npm..." % PI_PACKAGE)
    os.makedirs(PI_DIR, exist_ok=True)
    try:
        subprocess.run(
            [npm, "install", "--prefix", PI_DIR, "--no-fund", "--no-audit", PI_PACKAGE],
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        print("\n⚠ Pi agent install failed. Kraken will still run; fix Node/npm "
              "and run `%s --reinstall`." % APP_NAME, file=sys.stderr)
        return False
    return True


def _install():
    need = ".".join(str(n) for n in NODE_MIN)
    node = shutil.which("node")
    if not node:
        print("\n⚠ Node.js not found. The Pi agent needs Node >= %s." % need)
        print("Aborting Kraken installation.")
        raise SystemExit(1)
    version = _node_version(node)
    if version is None or version < NODE_MIN:
        have = ".".join(str(n) for n in version) if version else "unknown"
        print("\n⚠ Node %s is too old (need >= %s)." % (have, need))
        print("Aborting Kraken installation.")
        raise SystemExit(1)

    print("Installing %s into %s ..." % (APP_DISPLAY, INSTALL_ROOT))
    uv = _ensure_uv()
    os.makedirs(INSTALL_ROOT, exist_ok=True)

    print("Creating virtualenv (Python %s)..." % PYTHON_VERSION)
    subprocess.run([uv, "venv", "--python", PYTHON_VERSION, VENV_DIR], check=True)
    venv_py = _venv_python()

    print("Installing dependencies: %s" % ", ".join(DEPS))
    subprocess.run([uv, "pip", "install", "--python", venv_py] + DEPS, check=True)

    print("Extracting native terminal library...")
    _ensure_lib_extracted()

    pi_ok = _install_pi()

    # Copy this archive into place (skip if we are already the installed copy).
    src = _archive_path()
    if os.path.abspath(src) != os.path.abspath(INSTALLED_PYZ):
        shutil.copyfile(src, INSTALLED_PYZ)
    os.chmod(INSTALLED_PYZ, 0o755)

    # Launcher shim: always run the archive with the venv's interpreter.
    os.makedirs(BIN_DIR, exist_ok=True)
    with open(LAUNCHER, "w") as fh:
        fh.write(
            "#!/bin/sh\n"
            'exec "%s" "%s" "$@"\n' % (venv_py, INSTALLED_PYZ)
        )
    os.chmod(LAUNCHER, 0o755)

    print("\n✅ Installed. Launch it with:  %s" % APP_NAME)
    if not pi_ok:
        print("   (Pi agent not installed — see the note above; the app still runs.)")
    if BIN_DIR not in os.environ.get("PATH", "").split(os.pathsep):
        print("\nNote: %s is not on your PATH. Add it, e.g.:" % BIN_DIR)
        print('  export PATH="%s:$PATH"' % BIN_DIR)


def _reexec_via_venv():
    venv_py = _venv_python()
    target = INSTALLED_PYZ if os.path.exists(INSTALLED_PYZ) else _archive_path()
    os.execv(venv_py, [venv_py, target] + sys.argv[1:])


# --------------------------------------------------------------------------

def main():
    args = sys.argv[1:]
    if "--uninstall" in args:
        for path in (LAUNCHER,):
            if os.path.exists(path):
                os.remove(path)
        shutil.rmtree(INSTALL_ROOT, ignore_errors=True)
        print("Removed %s and %s" % (INSTALL_ROOT, LAUNCHER))
        return 0
    force_install = "--install" in args or "--reinstall" in args

    if _pyside_here() and not force_install:
        # We're already in a capable interpreter (the venv, or a dev shell).
        return _run_app()

    venv_py = _venv_python()
    if venv_py and _can_import(venv_py, "PySide6") and not force_install:
        _reexec_via_venv()  # does not return

    _install()
    _reexec_via_venv()  # does not return


if __name__ == "__main__":
    raise SystemExit(main())
