"""Shared building blocks for the workspace content panels.

The card and scrollbar styling the panels also share are not panel-specific and
live in kraken.ui.chrome.
"""

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QIcon, QPainter, QPixmap
from PySide6.QtWidgets import QVBoxLayout, QWidget


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


class Panel(QWidget):
    """Base panel: a container with a vertical layout to add widgets to."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._layout = QVBoxLayout(self)
        # Panels meet each other and the window edges directly; the only line
        # between two of them is the divider the dock's splitter paints, so a
        # margin here would open a gap of window colour beside it.
        self._layout.setContentsMargins(0, 0, 0, 0)

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
