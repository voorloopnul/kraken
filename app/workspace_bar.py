"""Vertical icon strip docked at the window's left edge for workspace
management: a "+" button to add a workspace/project folder, followed by one
button per open workspace showing a two-letter abbreviation of its folder
name. Emits `workspace_selected` when a workspace button is clicked."""

from __future__ import annotations

import re
from math import cos, radians, sin
from pathlib import Path

from PySide6.QtCore import QPointF, QRectF, QSize, Qt, Signal
from PySide6.QtGui import QColor, QIcon, QPainter, QPainterPath, QPen, QPixmap
from PySide6.QtWidgets import QButtonGroup, QToolButton, QVBoxLayout, QWidget

from app.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
#workspaceBar { background: #1b1c21; border-right: 1px solid #33353c; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #9a9da5; font-size: 11px; font-weight: 600; padding: 4px;
}
QToolButton:hover { background: #2c2e35; color: #ffffff; }
QToolButton:checked { background: #26282e; color: #ffffff; }
""",
    "light": """
#workspaceBar { background: #fafafa; border-right: 1px solid #e0e0e0; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #5a5d65; font-size: 11px; font-weight: 600; padding: 4px;
}
QToolButton:hover { background: #e8e8ec; color: #1b1d22; }
QToolButton:checked { background: #e0e0e5; color: #1b1d22; }
""",
}

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def _plus_icon(color: str) -> QIcon:
    """Plus sign, drawn on an 18x18 canvas."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.6)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawLine(QPointF(9.0, 3.5), QPointF(9.0, 14.5))
    painter.drawLine(QPointF(3.5, 9.0), QPointF(14.5, 9.0))
    painter.end()
    return QIcon(pixmap)


def _power_icon(color: str) -> QIcon:
    """Power symbol: a circle with a top gap and a vertical bar through it."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.6)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    # Arc angles are in 1/16 deg, 0 at 3 o'clock, CCW; leave a gap at the top.
    painter.drawArc(QRectF(3.5, 4.5, 11.0, 11.0), 120 * 16, 300 * 16)
    painter.drawLine(QPointF(9.0, 2.5), QPointF(9.0, 8.5))
    painter.end()
    return QIcon(pixmap)


def _sun_icon(color: str) -> QIcon:
    """Sun: small circle with eight rays."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.6)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawEllipse(QPointF(9.0, 9.0), 3.2, 3.2)
    for i in range(8):
        dx = cos(radians(i * 45.0))
        dy = sin(radians(i * 45.0))
        painter.drawLine(
            QPointF(9.0 + dx * 5.2, 9.0 + dy * 5.2),
            QPointF(9.0 + dx * 7.2, 9.0 + dy * 7.2),
        )
    painter.end()
    return QIcon(pixmap)


def _moon_icon(color: str) -> QIcon:
    """Crescent moon, filled."""
    pixmap = QPixmap(36, 36)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    full = QPainterPath()
    full.addEllipse(QRectF(3.5, 3.5, 11.0, 11.0))
    bite = QPainterPath()
    bite.addEllipse(QRectF(6.5, 1.5, 11.0, 11.0))
    painter.fillPath(full.subtracted(bite), QColor(color))
    painter.end()
    return QIcon(pixmap)


def abbreviation(folder_name: str) -> str:
    """Two-letter label for a workspace: initials of the first two words for
    multi-word names ("my-project" -> "MP"), else the first two letters."""
    words = [w for w in re.split(r"[^0-9A-Za-z]+", folder_name) if w]
    if len(words) >= 2:
        return (words[0][0] + words[1][0]).upper()
    name = words[0] if words else folder_name
    return name[:2].upper()


class WorkspaceBar(QWidget):
    workspace_selected = Signal(str)  # absolute path of the chosen workspace

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("workspaceBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedWidth(40)
        self._workspaces: dict[str, QToolButton] = {}
        self._group = QButtonGroup(self)
        self._group.setExclusive(True)
        # Bottom-pinned app actions (theme toggle, quit); MainWindow wires them.
        self.buttons: dict[str, QToolButton] = {}

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 8, 4, 8)
        layout.setSpacing(6)
        self.add_button = QToolButton()
        self.add_button.setToolTip("Add Workspace")
        self.add_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.add_button.setFixedSize(28, 28)
        self.add_button.setIconSize(QSize(18, 18))
        layout.addWidget(self.add_button)
        # Workspace buttons fill the gap above this stretch, top to bottom; the
        # action buttons below the stretch stay pinned to the bottom edge.
        layout.addStretch(1)
        for name in ("Toggle Theme", "Quit"):
            btn = QToolButton()
            btn.setToolTip(name)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(28, 28)
            btn.setIconSize(QSize(18, 18))
            layout.addWidget(btn)
            self.buttons[name] = btn
        self._layout = layout

        self.set_theme(DEFAULT_THEME)

    @property
    def workspaces(self) -> list[str]:
        return list(self._workspaces)

    def add_workspace(self, path: str, select: bool = True) -> None:
        """Add a button for the folder at `path`; re-select it if it exists."""
        path = str(Path(path).expanduser().resolve())
        btn = self._workspaces.get(path)
        if btn is None:
            btn = QToolButton()
            btn.setText(abbreviation(Path(path).name))
            btn.setToolTip(path)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(28, 28)
            btn.setCheckable(True)
            btn.clicked.connect(lambda _=False, p=path: self.workspace_selected.emit(p))
            self._group.addButton(btn)
            # Insert just above the stretch: after the add button and the
            # existing workspace buttons, before the bottom action group.
            self._layout.insertWidget(1 + len(self._workspaces), btn)
            self._workspaces[path] = btn
        if select:
            btn.setChecked(True)
            self.workspace_selected.emit(path)

    def set_theme(self, name: str) -> None:
        self.setStyleSheet(_STYLES[name])
        color = _ICON_COLORS[name]
        self.add_button.setIcon(_plus_icon(color))
        self.buttons["Quit"].setIcon(_power_icon(color))
        # Show the theme you'd switch to: sun in dark mode, moon in light.
        toggle_factory = _sun_icon if name == "dark" else _moon_icon
        self.buttons["Toggle Theme"].setIcon(toggle_factory(color))
