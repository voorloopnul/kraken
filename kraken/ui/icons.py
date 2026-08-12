"""The app's line-art icons, rendered from vendored Lucide SVGs.

Every icon in the chrome used to be drawn by hand in QPainter — a dozen little
routines placing arcs and polylines on an 18x18 canvas, each one its own set of
magic coordinates and each one drifting from the others in weight and corner
radius. They are Lucide now: one set, drawn to one grid, at one stroke weight.

The SVGs are vendored under assets/icons rather than fetched or installed. The
AppImage and the .app bundle carry the package's own tree and nothing else, so
an icon that needed a network or a node_modules would be an icon the packaged
app did not have. Lucide is ISC licensed (see assets/icons/ISC.txt), the same
arrangement the bundled fonts are under.

Lucide draws on a 24x24 grid with `stroke="currentColor"`, which SVG resolves
from the cascade and QSvgRenderer does not resolve at all — so the colour is
substituted into the source before rendering. Sizes are the *logical* ones the
buttons ask for; the pixmap behind them is rendered at twice that, so the
strokes stay clean where the desktop is scaled.
"""

from __future__ import annotations

from functools import lru_cache
from importlib.resources import files

from PySide6.QtCore import QByteArray, QRectF, Qt
from PySide6.QtGui import QIcon, QPainter, QPixmap
from PySide6.QtSvg import QSvgRenderer

# Rendered at twice the logical size, as the hand-drawn icons were: the pixmap
# carries the ratio, so Qt paints it at the right size on either kind of
# display and it stays sharp on the scaled one.
_SCALE = 2.0


@lru_cache(maxsize=None)
def _source(name: str) -> bytes:
    """The vendored SVG for `name`, read through importlib.resources so a
    source checkout and a packaged tree both find it."""
    return files("kraken.ui.assets").joinpath("icons", f"{name}.svg").read_bytes()


@lru_cache(maxsize=None)
def _renderer(name: str, color: str) -> QSvgRenderer:
    """A renderer for `name` in `color`. Cached with the parsed document, which
    is the expensive half of drawing one of these."""
    return QSvgRenderer(QByteArray(_source(name).replace(b"currentColor", color.encode())))


def paint(painter: QPainter, rect: QRectF, name: str, color: str) -> None:
    """Draw an icon into `rect` of a painter that is already going.

    For an icon that lands on top of something else drawn here — the glyph
    inside a window button, say. Going through a pixmap would rasterize the
    glyph at its own small size and then scale that into place; this keeps it
    vector all the way down, which at a handful of pixels across is the
    difference between a glyph and a smudge."""
    _renderer(name, color).render(painter, rect)


@lru_cache(maxsize=None)
def icon(name: str, color: str, size: int = 18) -> QIcon:
    """A Lucide icon in `color`, sized for a button that asks for `size`.

    Cached: a theme change rebuilds every icon in the window at once, and the
    same handful of (name, colour, size) triples come back each time."""
    pixmap = QPixmap(int(size * _SCALE), int(size * _SCALE))
    pixmap.setDevicePixelRatio(_SCALE)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    # In logical coordinates: the pixmap's ratio already scales the painter.
    paint(painter, QRectF(0, 0, size, size), name, color)
    painter.end()
    return QIcon(pixmap)
