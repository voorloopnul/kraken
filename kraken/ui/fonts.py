"""Bundled UI fonts.

JetBrains Mono ships with the app (OFL, see assets/fonts/OFL.txt) and is the
font for the whole Qt interface, so the UI looks the same on every machine
rather than falling back to whatever the system happens to have.

The terminal is deliberately not covered: it picks its own font from the
system's fixed-pitch default (see kraken/terminal/widget.py), which overrides
the application font for that widget.
"""

from __future__ import annotations

from importlib.resources import files

from PySide6.QtGui import QFontDatabase
from PySide6.QtWidgets import QApplication

MONO_FAMILY = "JetBrains Mono"

# Regular/italic/bold/bold-italic cover everything the UI asks for: body and
# tool lines (regular, italic), markdown emphasis and headings, and the
# semibold labels in the chrome, which Qt renders from the bold face. Other
# weights would only add megabytes.
_FONT_FILES = (
    "JetBrainsMono-Regular.ttf",
    "JetBrainsMono-Italic.ttf",
    "JetBrainsMono-Bold.ttf",
    "JetBrainsMono-BoldItalic.ttf",
)

_loaded = False


def load_fonts() -> None:
    """Register the bundled fonts with Qt. Call once, after the QApplication
    exists and before any widget is built.

    Reads the bytes through importlib.resources so it works both from a source
    checkout and from inside the packaged .pyz (where a filesystem path into
    the archive would fail). A font that fails to load is skipped: Qt then
    falls back to the system's default mono font for it.
    """
    global _loaded
    if _loaded:
        return
    _loaded = True
    for name in _FONT_FILES:
        try:
            data = files("kraken.ui.assets").joinpath("fonts", name).read_bytes()
        except (FileNotFoundError, ModuleNotFoundError, OSError):
            continue
        QFontDatabase.addApplicationFontFromData(data)


def apply_ui_font(app: QApplication) -> None:
    """Make the bundled font the application default. Call after load_fonts()
    and before any widget is built, so every widget inherits it.

    Only the family is swapped: the size stays on Qt's platform default, and
    the widgets that care about their own size set it in their stylesheets.
    If the font failed to register, this is a no-op rather than a request for
    a family that does not exist.
    """
    if MONO_FAMILY not in QFontDatabase.families():
        return
    font = app.font()
    font.setFamily(MONO_FAMILY)
    app.setFont(font)
