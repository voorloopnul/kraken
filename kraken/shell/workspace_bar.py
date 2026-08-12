"""Vertical icon strip docked at the window's left edge for workspace
management: a "+" button to add a workspace/project folder, followed by one
button per open workspace showing a two-letter abbreviation of its folder
name. Emits `workspace_selected` when a workspace button is clicked."""

from __future__ import annotations

import re
from math import cos, radians, sin
from pathlib import Path

from PySide6.QtCore import QSize, Qt, QTimer, Signal
from PySide6.QtGui import QColor, QFont, QPainter
from PySide6.QtWidgets import (
    QButtonGroup,
    QMenu,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken.ui.chrome import corner_style
from kraken.ui.fonts import UI_SANS_FAMILY
from kraken.ui.icons import icon
from kraken.ui.themes import DEFAULT_THEME


def _sans_font(widget: QWidget) -> QFont:
    """The proportional (Roboto) UI font at the widget's inherited size — used
    for menu text, which reads as prose rather than code."""
    font = QFont(widget.font())
    font.setFamily(UI_SANS_FAMILY)
    return font

# Remote workspaces carry a left accent bar so they read differently from
# local folders in the strip.
_STYLES = {
    "dark": """
#workspaceBar { background: #1b1c21; border-right: 1px solid #33353c; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #9a9da5; font-size: 11px; font-weight: 600; padding: 4px;
}
QToolButton:hover { background: #2c2e35; color: #ffffff; }
QToolButton:checked { background: #26282e; color: #ffffff; }
QToolButton[remote="true"] {
    border-left: 2px solid #4f83e0; border-top-left-radius: 2px;
    border-bottom-left-radius: 2px;
}
QToolButton::menu-indicator { image: none; }
""",
    "light": """
#workspaceBar { background: #fafafa; border-right: 1px solid #e0e0e0; }
QToolButton {
    background: transparent; border: none; border-radius: 6px;
    color: #5a5d65; font-size: 11px; font-weight: 600; padding: 4px;
}
QToolButton:hover { background: #e8e8ec; color: #1b1d22; }
QToolButton:checked { background: #e0e0e5; color: #1b1d22; }
QToolButton[remote="true"] {
    border-left: 2px solid #4f83e0; border-top-left-radius: 2px;
    border-bottom-left-radius: 2px;
}
QToolButton::menu-indicator { image: none; }
""",
}

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def abbreviation(folder_name: str) -> str:
    """Two-letter label for a workspace: initials of the first two words for
    multi-word names ("my-project" -> "MP"), else the first two letters."""
    words = [w for w in re.split(r"[^0-9A-Za-z]+", folder_name) if w]
    if len(words) >= 2:
        return (words[0][0] + words[1][0]).upper()
    name = words[0] if words else folder_name
    return name[:2].upper()


class _WorkspaceButton(QToolButton):
    """A workspace button that paints a blinking blue dot in its top-right
    corner while that workspace has a Pi agent running, so a session processing
    in the background stays visible even when the workspace isn't on screen."""

    _DOT_COLOR = QColor("#4f83e0")
    _DOT_RADIUS = 4

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._active = False
        self._blink_on = True
        self._timer = QTimer(self)
        self._timer.setInterval(600)
        self._timer.timeout.connect(self._toggle)

    def set_active(self, active: bool) -> None:
        if active == self._active:
            return
        self._active = active
        if active:
            self._blink_on = True
            self._timer.start()
        else:
            self._timer.stop()
        self.update()

    def _toggle(self) -> None:
        self._blink_on = not self._blink_on
        self.update()

    def paintEvent(self, event) -> None:
        super().paintEvent(event)
        if not (self._active and self._blink_on):
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(self._DOT_COLOR)
        d = self._DOT_RADIUS * 2
        painter.drawEllipse(self.width() - d - 2, 2, d, d)


class WorkspaceBar(QWidget):
    workspace_selected = Signal(str)  # key (local path or remote anchor) chosen
    add_local_requested = Signal()
    add_remote_requested = Signal()
    workspace_edit_requested = Signal(str)  # remote anchor to edit
    workspace_remove_requested = Signal(str)  # workspace key to remove

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("workspaceBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedWidth(40)
        self._theme_name = DEFAULT_THEME
        # Set by MainWindow, which owns the window's shape.
        self._corner_radius = 0
        self._workspaces: dict[str, QToolButton] = {}
        self._remote_keys: set[str] = set()
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
        # The "+" opens a menu: a local folder, or a remote SSH host.
        self.add_button.setPopupMode(QToolButton.ToolButtonPopupMode.InstantPopup)
        add_menu = QMenu(self.add_button)
        add_menu.setFont(_sans_font(self))
        add_menu.addAction("Add Local Folder…").triggered.connect(
            self.add_local_requested
        )
        add_menu.addAction("Add Remote Host…").triggered.connect(
            self.add_remote_requested
        )
        self.add_button.setMenu(add_menu)
        layout.addWidget(self.add_button)
        # Workspace buttons fill the gap above this stretch, top to bottom; the
        # action buttons below the stretch stay pinned to the bottom edge.
        layout.addStretch(1)
        for name in ("Toggle Theme", "Settings", "Quit"):
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

    def add_workspace(
        self,
        path: str,
        select: bool = True,
        remote: bool = False,
        tooltip: str | None = None,
        abbrev: str | None = None,
    ) -> None:
        """Add a button for the workspace keyed by `path`; re-select it if it
        already exists. Remote workspaces pass `remote=True` with an explicit
        `tooltip` (their `user@host:/path`) and `abbrev`, since `path` is only a
        local anchor folder whose name is meaningless to the user."""
        if not remote:
            path = str(Path(path).expanduser().resolve())
        btn = self._workspaces.get(path)
        if btn is None:
            btn = _WorkspaceButton()
            btn.setText(abbrev or abbreviation(Path(path).name))
            btn.setToolTip(tooltip or path)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(28, 28)
            btn.setCheckable(True)
            btn.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
            btn.customContextMenuRequested.connect(
                lambda pos, p=path: self._on_workspace_menu(p, pos)
            )
            btn.clicked.connect(lambda _=False, p=path: self.workspace_selected.emit(p))
            if remote:
                btn.setProperty("remote", "true")
                self._remote_keys.add(path)
            self._group.addButton(btn)
            # Insert just above the stretch: after the add button and the
            # existing workspace buttons, before the bottom action group.
            self._layout.insertWidget(1 + len(self._workspaces), btn)
            self._workspaces[path] = btn
        if select:
            btn.setChecked(True)
            self.workspace_selected.emit(path)

    def set_workspace_active(self, path: str, active: bool) -> None:
        """Show/hide the blinking activity dot on a workspace's button, keyed by
        the same path/anchor used to add it."""
        btn = self._workspaces.get(path)
        if isinstance(btn, _WorkspaceButton):
            btn.set_active(active)

    def select_workspace(self, path: str) -> None:
        """Programmatically select an existing workspace button."""
        btn = self._workspaces.get(path)
        if btn is not None:
            btn.setChecked(True)
            self.workspace_selected.emit(path)

    def remove_workspace(self, path: str) -> None:
        """Drop a workspace's button from the strip."""
        btn = self._workspaces.pop(path, None)
        if btn is None:
            return
        self._remote_keys.discard(path)
        self._group.removeButton(btn)
        self._layout.removeWidget(btn)
        btn.deleteLater()

    def _on_workspace_menu(self, path: str, pos) -> None:
        btn = self._workspaces.get(path)
        if btn is None:
            return
        menu = QMenu(self)
        menu.setFont(_sans_font(self))
        if path in self._remote_keys:
            menu.addAction("Edit…").triggered.connect(
                lambda: self.workspace_edit_requested.emit(path)
            )
        menu.addAction("Remove").triggered.connect(
            lambda: self.workspace_remove_requested.emit(path)
        )
        menu.exec(btn.mapToGlobal(pos))

    def set_corner_radius(self, radius: int) -> None:
        """Round the window's bottom-left corner, which is the strip's own."""
        self._corner_radius = radius
        self._apply_style()

    def _apply_style(self) -> None:
        self.setStyleSheet(
            _STYLES[self._theme_name]
            + corner_style("#workspaceBar", ("bottom-left",), self._corner_radius)
        )

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._apply_style()
        color = _ICON_COLORS[name]
        self.add_button.setIcon(icon("plus", color))
        self.buttons["Settings"].setIcon(icon("settings", color))
        self.buttons["Quit"].setIcon(icon("power", color))
        # Show the theme you'd switch to: sun in dark mode, moon in light.
        self.buttons["Toggle Theme"].setIcon(
            icon("sun" if name == "dark" else "moon", color)
        )
