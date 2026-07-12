"""Vertical icon strip docked at the window's right edge, IDE tool-window
style: a top group of tool buttons and a bottom group pinned to the bottom.
Buttons are placeholders — connect to their `clicked` signals to add actions."""

from __future__ import annotations

from PySide6.QtCore import QPointF, QRectF, QSize, Qt
from PySide6.QtGui import (
    QColor,
    QIcon,
    QPainter,
    QPainterPath,
    QPen,
    QPixmap,
    QPolygonF,
)
from PySide6.QtWidgets import QToolButton, QVBoxLayout, QWidget

from kraken.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
#sideBar { background: #1b1c21; border-left: 1px solid #33353c; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #9a9da5; font-size: 15px; padding: 4px;
}
QToolButton:hover { background: #2c2e35; color: #ffffff; }
QToolButton:checked { background: #26282e; color: #ffffff; }
""",
    "light": """
#sideBar { background: #fafafa; border-left: 1px solid #e0e0e0; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #5a5d65; font-size: 15px; padding: 4px;
}
QToolButton:hover { background: #e8e8ec; color: #1b1d22; }
QToolButton:checked { background: #e0e0e5; color: #1b1d22; }
""",
}

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def _terminal_icon(color: str) -> QIcon:
    """Terminal window outline with a ">_" prompt, drawn on an 18x18 canvas."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.6)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    painter.drawRoundedRect(QRectF(1.5, 2.5, 15.0, 13.0), 2.5, 2.5)
    painter.drawPolyline(
        QPolygonF([QPointF(4.5, 6.5), QPointF(7.5, 9.0), QPointF(4.5, 11.5)])
    )
    painter.drawLine(QPointF(9.5, 12.0), QPointF(13.5, 12.0))
    painter.end()
    return QIcon(pixmap)


def _globe_icon(color: str) -> QIcon:
    """Globe: circle with a meridian ellipse and an equator line."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.4)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawEllipse(QPointF(9.0, 9.0), 6.5, 6.5)
    painter.drawEllipse(QPointF(9.0, 9.0), 3.0, 6.5)
    painter.drawLine(QPointF(2.5, 9.0), QPointF(15.5, 9.0))
    painter.end()
    return QIcon(pixmap)


def _git_icon(color: str) -> QIcon:
    """Git branch: a trunk with top/bottom nodes and a branch node curving in."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.4)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawEllipse(QPointF(5.1, 4.3), 2.0, 2.0)
    painter.drawEllipse(QPointF(5.1, 13.7), 2.0, 2.0)
    painter.drawEllipse(QPointF(12.9, 7.0), 2.0, 2.0)
    painter.drawLine(QPointF(5.1, 6.3), QPointF(5.1, 11.7))
    path = QPainterPath(QPointF(5.1, 11.3))
    path.cubicTo(QPointF(5.1, 9.0), QPointF(12.9, 10.7), QPointF(12.9, 9.0))
    painter.drawPath(path)
    painter.end()
    return QIcon(pixmap)


def _camera_icon(color: str) -> QIcon:
    """Camera: rounded body with a viewfinder bump and a lens circle."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.4)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    painter.drawRoundedRect(QRectF(2.0, 5.0, 14.0, 9.5), 2.0, 2.0)
    painter.drawPolyline(
        QPolygonF(
            [
                QPointF(6.5, 5.0),
                QPointF(7.5, 3.0),
                QPointF(10.5, 3.0),
                QPointF(11.5, 5.0),
            ]
        )
    )
    painter.drawEllipse(QPointF(9.0, 9.75), 2.6, 2.6)
    painter.end()
    return QIcon(pixmap)


# Buttons rendered with painted icons instead of text glyphs; the factory is
# re-run on theme change to recolor the icon.
_ICON_FACTORIES = {
    "Terminal Panel": _terminal_icon,
    "Browser Panel": _globe_icon,
    "Git Panel": _git_icon,
    "Screenshot": _camera_icon,
}

_BOTTOM_ITEMS = (
    ("", "Screenshot"),
)


class SideBar(QWidget):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("sideBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedWidth(40)
        self.buttons: dict[str, QToolButton] = {}

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 8, 4, 8)
        layout.setSpacing(6)
        for name in ("Terminal Panel", "Browser Panel", "Git Panel"):
            btn = self._make_button("", name)
            btn.setIconSize(QSize(18, 18))
            layout.addWidget(btn)
        layout.addStretch(1)
        for glyph, name in _BOTTOM_ITEMS:
            btn = self._make_button(glyph, name)
            if not glyph:
                btn.setIconSize(QSize(18, 18))
            layout.addWidget(btn)

        self.set_theme(DEFAULT_THEME)

    def _make_button(self, glyph: str, name: str) -> QToolButton:
        btn = QToolButton()
        btn.setText(glyph)
        btn.setToolTip(name)
        btn.setCursor(Qt.CursorShape.PointingHandCursor)
        btn.setFixedSize(28, 28)
        self.buttons[name] = btn
        return btn

    def set_theme(self, name: str) -> None:
        self.setStyleSheet(_STYLES[name])
        for button_name, factory in _ICON_FACTORIES.items():
            self.buttons[button_name].setIcon(factory(_ICON_COLORS[name]))
