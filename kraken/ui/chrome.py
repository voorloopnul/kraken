"""Shared containers, scroll chrome, and the window's own shape.

The pieces of the app's look that are not specific to any one surface: the
rounded card the panels group their content in, the scrollbar styling their
scroll areas share, and the shape and edge grips every frameless window of ours
wears. The card lives here rather than with the panels because the diff viewer's
sheet is a card too, and it is not a panel; the window pieces live here because
the widgets that have to honour them — the title bar, both side strips, the
frame, the viewer's scrim over all of them, and the settings window, which is a
second frameless window — have nothing else in common.
"""

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QFrame,
    QGraphicsDropShadowEffect,
    QVBoxLayout,
    QWidget,
)

from kraken.ui.themes import LIGHT

# Corner rounding for the frameless window. There is no decoration to round, so
# the corner pixels belong to whichever widget sits in them — the title bar
# along the top, the workspace and side bars at the bottom — and each rounds its
# own outer corners to this. A maximized window is handed 0: there is nothing
# beside it to round against, and a gap at the screen corner reads as a glitch.
WINDOW_RADIUS = 10

# Grabbing within this many pixels of a window edge starts a resize; a custom
# title bar removes the native frame, and with it native resizing.
RESIZE_MARGIN = 6
# Within this many pixels of a strip's end the grab is a corner resize.
_CORNER_MARGIN = 14

_EDGE_CURSORS = {
    Qt.Edge.LeftEdge: Qt.CursorShape.SizeHorCursor,
    Qt.Edge.RightEdge: Qt.CursorShape.SizeHorCursor,
    Qt.Edge.TopEdge: Qt.CursorShape.SizeVerCursor,
    Qt.Edge.BottomEdge: Qt.CursorShape.SizeVerCursor,
    Qt.Edge.TopEdge | Qt.Edge.LeftEdge: Qt.CursorShape.SizeFDiagCursor,
    Qt.Edge.BottomEdge | Qt.Edge.RightEdge: Qt.CursorShape.SizeFDiagCursor,
    Qt.Edge.TopEdge | Qt.Edge.RightEdge: Qt.CursorShape.SizeBDiagCursor,
    Qt.Edge.BottomEdge | Qt.Edge.LeftEdge: Qt.CursorShape.SizeBDiagCursor,
}


class EdgeGrip(QWidget):
    """Invisible strip overlaid along one edge of a frameless window.
    Pressing it hands the drag to the window manager as a resize; the strip
    ends double as corner grips by adding the perpendicular edge. A plain
    child widget (not an application event filter) so QtWebEngine's internal
    QObjects never pass through Python during construction — wrapping those
    in an app-wide filter crashes PySide."""

    def __init__(self, edge: Qt.Edge, parent: QWidget):
        super().__init__(parent)
        self.edge = edge
        self._horizontal = edge in (Qt.Edge.TopEdge, Qt.Edge.BottomEdge)
        self.setMouseTracking(True)

    def _edges_at(self, pos) -> Qt.Edge:
        edges = self.edge
        along = pos.x() if self._horizontal else pos.y()
        length = self.width() if self._horizontal else self.height()
        if along <= _CORNER_MARGIN:
            edges |= Qt.Edge.TopEdge if not self._horizontal else Qt.Edge.LeftEdge
        elif along >= length - _CORNER_MARGIN:
            edges |= (
                Qt.Edge.BottomEdge if not self._horizontal else Qt.Edge.RightEdge
            )
        return edges

    def mouseMoveEvent(self, event) -> None:
        self.setCursor(_EDGE_CURSORS[self._edges_at(event.position().toPoint())])

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            self.window().windowHandle().startSystemResize(
                self._edges_at(event.position().toPoint())
            )
            event.accept()


def edge_grips(window: QWidget) -> list[EdgeGrip]:
    """One grip per edge of `window`. Give the result to `place_edge_grips`
    from the window's resizeEvent — the grips are overlaid rather than laid
    out, so nothing else moves them."""
    return [
        EdgeGrip(edge, window)
        for edge in (
            Qt.Edge.LeftEdge,
            Qt.Edge.RightEdge,
            Qt.Edge.TopEdge,
            Qt.Edge.BottomEdge,
        )
    ]


def place_edge_grips(grips: list[EdgeGrip], width: int, height: int) -> None:
    """Stretch each grip along its edge of a window that size, on top of
    whatever the layout has put there."""
    m = RESIZE_MARGIN
    rects = {
        Qt.Edge.LeftEdge: (0, 0, m, height),
        Qt.Edge.RightEdge: (width - m, 0, m, height),
        Qt.Edge.TopEdge: (0, 0, width, m),
        Qt.Edge.BottomEdge: (0, height - m, width, m),
    }
    for grip in grips:
        grip.setGeometry(*rects[grip.edge])
        grip.raise_()


def corner_style(selector: str, corners: tuple[str, ...], radius: int) -> str:
    """A stylesheet rule rounding only the named corners ("top-left", …) of a
    widget that sits at the window's edge.

    Appended to the widget's own style sheet rather than written into it, so the
    radius can change — on maximize, and back — without the rest of the style
    having to be rebuilt around it."""
    rules = "".join(f" border-{corner}-radius: {radius}px;" for corner in corners)
    return f"{selector} {{{rules} }}"


# Scrollbars for the app's scroll areas, matching the conversation panel's
# external one: a bare rounded handle on a transparent track, no stepper
# buttons. Append to a widget's own stylesheet.
SCROLLBAR_STYLES = {
    "dark": """
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #3a3d45; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #4a4e58; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #3a3d45; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #4a4e58; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
    "light": """
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #c9c4b4; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #b3ae9e; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #c9c4b4; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #b3ae9e; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
}


class Card(QFrame):
    """A rounded-border container to group content.

    `shadow=False` matters for cards hosting a QWebEngineView: a graphics
    effect makes Qt render the subtree through a cached pixmap, freezing
    Chromium's composited output — the page looks unresponsive even though
    input still reaches it. It matters again for a card that needs an effect of
    its own (the diff viewer fades one), since a widget gets only one."""

    def __init__(self, parent: QWidget | None = None, shadow: bool = True):
        super().__init__(parent)
        self.setObjectName("card")
        self.set_colors("#%02X%02X%02X" % LIGHT.background, "#e0e0e0")
        if shadow:
            effect = QGraphicsDropShadowEffect(self)
            effect.setBlurRadius(12)
            effect.setOffset(0, 2)
            effect.setColor(QColor(0, 0, 0, 40))
            self.setGraphicsEffect(effect)

        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(12, 12, 12, 12)

    def set_colors(self, background: str, border: str) -> None:
        self.setStyleSheet(
            "#card {"
            f" background: {background};"
            f" border: 1px solid {border};"
            " border-radius: 8px;"
            "}"
        )

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)

    def add_header(self, widget: QWidget) -> None:
        """Place a widget as the card's top row (above whatever was added)."""
        self._layout.insertWidget(0, widget)
