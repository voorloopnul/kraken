"""Shared containers and scroll chrome.

The pieces of the app's look that are not specific to any one surface: the
rounded card the panels group their content in, and the scrollbar styling their
scroll areas share. They live here rather than with the panels because the diff
viewer's sheet is a card too, and it is not a panel.
"""

from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QFrame,
    QGraphicsDropShadowEffect,
    QVBoxLayout,
    QWidget,
)

from kraken.ui.themes import LIGHT

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
