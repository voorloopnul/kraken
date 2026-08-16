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

from kraken.ui.themes import UI_COLORS

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


# Every panel wears the same strip along its top, whatever it puts in it: the
# terminal's and the browser's tab rows, and the plain row the drag grip rides
# in on the panels with no tabs of their own. The strip is a surface a shade off
# the panel below it, closed with the hairline a card border would have drawn,
# so a panel reads as content under a header rather than as one flat sheet. It
# runs the panel's full width — the card's padding starts underneath it.
PANEL_HEADER_HEIGHT = 32


def header_style(selector: str, theme: str) -> str:
    """The strip's own surface and its closing hairline, for a widget that is
    one. Append a widget's own rules to it."""
    ui = UI_COLORS[theme]
    return (
        f"{selector} {{ background: {ui['header']};"
        f" border-bottom: 1px solid {ui['card_border']}; }}"
    )


# The pills in a tab strip. Selected is the accent tint the app uses for a
# current thing throughout — the same blue the side strip fills an open panel's
# button with, softened, because a tab is one of several rather than the answer
# to the strip's whole question. The rest of the strip is quiet: an unselected
# tab is text on the strip itself, and the ✕ and + are quieter still, since
# neither is where the eye should land.
_TAB_COLORS = {
    "dark": {"idle": "#9a9da5", "close": "#7a7d85"},
    "light": {"idle": "#6a6d75", "close": "#a9a7a3"},
}


def _tab_colors(theme: str) -> dict[str, str]:
    ui = UI_COLORS[theme]
    return {
        **_TAB_COLORS[theme],
        "hover": ui["hover"],
        "selected": ui["accent_soft"],
        "accent": ui["accent_text"],
    }


def tab_strip_style(theme: str) -> str:
    """Stylesheet for a panel's tab strip: the header surface it rides on, the
    pills in it, and the ✕ and + buttons beside them.

    Shared verbatim by the terminal and the browser, which are the same strip
    twice — the two had drifted apart once already while they were two copies.
    """
    c = _tab_colors(theme)
    return header_style("#tabRow", theme) + f"""
QTabBar {{ background: transparent; }}
QTabBar::tab {{
    background: transparent;
    color: {c['idle']};
    border: none;
    border-radius: 6px;
    font-size: 13px;
    /* Tight on the right: the ✕ button supplies the trailing space. */
    padding: 5px 0 5px 10px;
    margin: 0 4px 0 0;
}}
QTabBar::tab:hover {{ background: {c['hover']}; }}
/* After :hover, so hovering the current tab does not tint it grey. */
QTabBar::tab:selected {{ background: {c['selected']}; color: {c['accent']}; }}
QToolButton {{
    background: transparent;
    color: {c['idle']};
    border: none;
    border-radius: 4px;
    font-size: 14px;
    padding: 2px 6px;
}}
QToolButton:hover {{ background: {c['hover']}; }}
#tabClose {{ color: {c['close']}; font-size: 10px; padding: 0;
             border-radius: 8px; }}
#tabClose:hover {{ background: {c['hover']}; color: {c['accent']}; }}
"""


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
QScrollBar::handle:horizontal { background: #ccc9c3; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #b6b3ac; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #ccc9c3; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #b6b3ac; }
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
    its own (the diff viewer fades one), since a widget gets only one.

    `flat=True` makes it a surface instead: the same background, but no border,
    no radius and no shadow, for a card that meets its neighbours edge to edge
    and lets the divider between them draw the only line. It implies
    `shadow=False` — an effect on a full-bleed surface has nowhere to fall.

    A header added with `add_header` sits outside the padding rather than in it:
    a panel's top strip is a surface of its own and runs the full width, while
    everything below it stays inset. `padding=0` is for a card whose content
    brings its own (the tab strips, which pad only what is under the strip).
    """

    def __init__(
        self,
        parent: QWidget | None = None,
        shadow: bool = True,
        flat: bool = False,
        padding: int = 12,
    ):
        super().__init__(parent)
        self.setObjectName("card")
        self._flat = flat
        self.set_colors(UI_COLORS["light"]["card"], UI_COLORS["light"]["card_border"])
        if shadow and not flat:
            effect = QGraphicsDropShadowEffect(self)
            effect.setBlurRadius(12)
            effect.setOffset(0, 2)
            effect.setColor(QColor(0, 0, 0, 40))
            self.setGraphicsEffect(effect)

        # Two layers: headers go in the outer one, which has no padding of its
        # own, and everything else in the inner one, which carries all of it.
        self._outer = QVBoxLayout(self)
        self._outer.setContentsMargins(0, 0, 0, 0)
        self._outer.setSpacing(0)
        self._layout = QVBoxLayout()
        self._layout.setContentsMargins(padding, padding, padding, padding)
        self._outer.addLayout(self._layout, stretch=1)
        self._headers = 0

    def set_colors(self, background: str, border: str) -> None:
        if self._flat:
            self.setStyleSheet(f"#card {{ background: {background}; border: none; }}")
            return
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
        """Place a widget as the card's top strip: above the content and
        outside its padding, so it runs the card's full width."""
        self._outer.insertWidget(self._headers, widget)
        self._headers += 1
