"""Shared building blocks for the workspace content panels."""

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QIcon, QPainter, QPixmap
from PySide6.QtWidgets import (
    QFrame,
    QGraphicsDropShadowEffect,
    QVBoxLayout,
    QWidget,
)

from kraken.ui.themes import LIGHT


def _dot_icon(color: str) -> QIcon:
    """A small filled circle used as a history-row status indicator."""
    size = 10
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(color))
    painter.drawEllipse(1, 1, size - 2, size - 2)
    painter.end()
    return QIcon(pixmap)


class Card(QFrame):
    """A rounded-border container to group panel content.

    `shadow=False` matters for cards hosting a QWebEngineView: a graphics
    effect makes Qt render the subtree through a cached pixmap, freezing
    Chromium's composited output — the page looks unresponsive even though
    input still reaches it."""

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


class Panel(QWidget):
    """Base panel: a container with a vertical layout to add widgets to."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(2, 2, 2, 2)

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)

    def mount_grip(self, grip: QWidget) -> bool:
        """Take the dock's bare drag grip into a strip this panel already has
        (a tab row, say). Returning False means there is nowhere for it to
        ride, so the dock will wrap it in a row of its own and hand that back
        through mount_grip_row()."""
        return False

    def mount_grip_row(self, row: QWidget) -> None:
        """Host the dock's grip row inside this panel: at the top of the card
        when the panel has one (so the row sits within the rounded border),
        otherwise at the very top of the panel's own layout."""
        card = getattr(self, "_card", None)
        if card is not None:
            card.add_header(row)
        else:
            self._layout.insertWidget(0, row)
