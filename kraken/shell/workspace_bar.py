"""Vertical icon strip docked at the window's left edge for workspace
management: a "+" button to add a workspace/project folder, followed by one
coloured tile per open workspace carrying a three-letter abbreviation of its
folder name. Emits `workspace_selected` when a workspace button is clicked."""

from __future__ import annotations

import re
import zlib
from math import cos, radians, sin
from pathlib import Path

from PySide6.QtCore import QRectF, QSize, Qt, QTimer, Signal
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
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS


def _sans_font(widget: QWidget) -> QFont:
    """The proportional (Roboto) UI font at the widget's inherited size — used
    for menu text, which reads as prose rather than code."""
    font = QFont(widget.font())
    font.setFamily(UI_SANS_FAMILY)
    return font

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def _style(theme: str) -> str:
    """The strip's surface and the buttons that are not workspaces: the "+" at
    the top and the app actions pinned to the bottom.

    The same strip the panel toggles live in on the other side of the window, so
    it wears the same surface and the same quiet grey. The workspace tiles in
    between are the exception — each carries a colour of its own (see
    `tile_style`), set on the button itself."""
    ui = UI_COLORS[theme]
    return f"""
#workspaceBar {{ background: {ui['sidebar']}; border-right: 1px solid {ui['card_border']}; }}
QToolButton {{
    background: transparent; border: none; border-radius: 6px;
    color: {_ICON_COLORS[theme]}; font-size: 11px; font-weight: 600; padding: 4px;
}}
QToolButton:hover {{ background: {ui['hover']}; color: {ui['text']}; }}
QToolButton::menu-indicator {{ image: none; }}
"""


def abbreviation(folder_name: str) -> str:
    """Three-letter label for a workspace: initials of the first three words for
    names that have them ("claude-code-sdk" -> "CCS"), else the first three
    letters ("kraken" -> "KRA", "my-project" -> "MYP").

    Three rather than two because the tile is coloured now, and a colour and two
    letters left too many folders looking alike — "kr" served Kraken and Kramer
    equally well."""
    words = [w for w in re.split(r"[^0-9A-Za-z]+", folder_name) if w]
    if len(words) >= 3:
        return "".join(w[0] for w in words[:3]).upper()
    return ("".join(words) or folder_name)[:3].upper()


# ---- Workspace tiles ------------------------------------------------------
#
# One hue per workspace, so a folder is recognisable in the strip before its
# letters are read. Eight of them, spaced far enough around the wheel that no
# two are mistakable at tile size, and each rendered twice: saturated for the
# workspace you are in, pale for the ones you are not. The letters are white on
# both — it is the tile's weight, not its hue, that says which is current, and
# a tile that changed hue on selection would stop being an identity.
_TILE_HUES = (12, 43, 90, 141, 186, 225, 268, 322)

# Saturation and lightness for a tile, per theme and state. On a light strip an
# idle tile is a pastel and the current one steps down into full colour; on a
# dark strip it has to run the other way, because a pastel on #23252b glares
# brighter than anything that could mark it as current.
_TILE_MIX = {
    "light": {
        (False, False): (0.45, 0.76),  # (checked, hovered)
        (False, True): (0.45, 0.70),
        (True, False): (0.48, 0.54),
        (True, True): (0.48, 0.48),
    },
    "dark": {
        (False, False): (0.32, 0.32),
        (False, True): (0.32, 0.38),
        (True, False): (0.52, 0.58),
        (True, True): (0.52, 0.64),
    },
}

# The bar down the strip's edge beside the current workspace, and the activity
# dot on a tile. Both have to read on any of the eight hues at either weight, so
# they are the strip's own extreme rather than anything from the wheel.
_INDICATOR = {"dark": "#e8e6e2", "light": "#2a2824"}

TILE_SIZE = 30
_TILE_RADIUS = 8


def tile_hue(key: str) -> int:
    """The hue a workspace keeps, chosen from its key so it is the same colour
    in every window and on every run. Python's hash() is salted per process and
    would hand the same folder a new colour each launch."""
    return _TILE_HUES[zlib.crc32(key.encode()) % len(_TILE_HUES)]


def _tile_color(hue: int, theme: str, checked: bool, hovered: bool) -> str:
    saturation, lightness = _TILE_MIX[theme][(checked, hovered)]
    color = QColor.fromHslF(hue / 360.0, saturation, lightness)
    return "#%02X%02X%02X" % (color.red(), color.green(), color.blue())


def tile_style(key: str, theme: str, remote: bool) -> str:
    """The four faces of one workspace tile — idle, hovered, current, and
    current-and-hovered — as a stylesheet for the button itself.

    All four are written at once rather than restyled on each toggle, so Qt
    switches them from the pseudo-states it already tracks. The selector is
    attribute-qualified to outrank the plain `QToolButton` rules the strip sets
    on every button in it."""
    hue = tile_hue(key)
    face = 'QToolButton[workspace="true"]'
    # A remote workspace keeps its accent bar down the left: the hue says which
    # folder, and the bar says it is not on this machine.
    edge = (
        f"{face} {{ border-left: 2px solid {UI_COLORS[theme]['accent']};"
        f" border-top-left-radius: 2px; border-bottom-left-radius: 2px; }}"
        if remote
        else ""
    )
    return f"""
{face} {{
    background: {_tile_color(hue, theme, False, False)};
    color: #ffffff; border: none; border-radius: {_TILE_RADIUS}px;
    font-size: 11px; font-weight: 600; padding: 0;
}}
{face}:hover {{ background: {_tile_color(hue, theme, False, True)}; }}
/* After :hover, so hovering the current workspace does not fade it back. */
{face}:checked {{ background: {_tile_color(hue, theme, True, False)}; }}
{face}:checked:hover {{ background: {_tile_color(hue, theme, True, True)}; }}
{edge}
"""


class _WorkspaceButton(QToolButton):
    """One workspace tile: its three letters on the hue that workspace keeps,
    pale while another workspace is current and in full colour once it is.

    It also paints a blinking dot in its top-right corner while that workspace
    has a Pi agent running, so a session processing in the background stays
    visible even when the workspace isn't on screen."""

    _DOT_RADIUS = 4

    def __init__(self, key: str, remote: bool = False, parent: QWidget | None = None):
        super().__init__(parent)
        self._key = key
        self._remote = remote
        # Lets tile_style's selector outrank the strip's own QToolButton rules,
        # and keeps the workspace tiles apart from the icon buttons around them.
        self.setProperty("workspace", "true")
        self._dot_color = QColor(_INDICATOR[DEFAULT_THEME])
        self._active = False
        self._blink_on = True
        self._timer = QTimer(self)
        self._timer.setInterval(600)
        self._timer.timeout.connect(self._toggle)
        self.set_theme(DEFAULT_THEME)

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
        painter.setBrush(self._dot_color)
        d = self._DOT_RADIUS * 2
        painter.drawEllipse(self.width() - d - 2, 2, d, d)

    def set_theme(self, name: str) -> None:
        self.setStyleSheet(tile_style(self._key, name, self._remote))
        self._dot_color = QColor(_INDICATOR[name])
        self.update()


class WorkspaceBar(QWidget):
    # The edge indicator: a hairline-ish bar, a little over half the tile tall,
    # flush with the strip's outer edge.
    _INDICATOR_WIDTH = 3.0
    _INDICATOR_SCALE = 0.6

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
        # The edge indicator is painted rather than laid out, so nothing else
        # notices that the current workspace moved.
        self._group.buttonToggled.connect(lambda *_: self.update())
        # Bottom-pinned app actions (theme toggle, quit); MainWindow wires them.
        self.buttons: dict[str, QToolButton] = {}

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 8, 4, 8)
        layout.setSpacing(8)
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
            btn = _WorkspaceButton(path, remote)
            # Workspaces are added long after the strip was first themed.
            btn.set_theme(self._theme_name)
            btn.setText(abbrev or abbreviation(Path(path).name))
            btn.setToolTip(tooltip or path)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(TILE_SIZE, TILE_SIZE)
            btn.setCheckable(True)
            btn.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
            btn.customContextMenuRequested.connect(
                lambda pos, p=path: self._on_workspace_menu(p, pos)
            )
            btn.clicked.connect(lambda _=False, p=path: self.workspace_selected.emit(p))
            if remote:
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

    def paintEvent(self, event) -> None:
        """Mark the current workspace with a bar down the strip's outer edge.

        The tile's own weight already says which workspace is current, but only
        against the other tiles — with one workspace open there is nothing to
        compare it to, and a lone saturated tile says nothing at all. The bar is
        painted here rather than laid out because it belongs to the strip's
        edge, outside the margin the buttons sit in."""
        super().paintEvent(event)
        current = next(
            (b for b in self._workspaces.values() if b.isChecked()), None
        )
        if current is None:
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(QColor(_INDICATOR[self._theme_name]))
        height = current.height() * self._INDICATOR_SCALE
        top = current.y() + (current.height() - height) / 2
        radius = self._INDICATOR_WIDTH / 2
        painter.drawRoundedRect(
            QRectF(0.0, top, self._INDICATOR_WIDTH, height), radius, radius
        )

    def set_corner_radius(self, radius: int) -> None:
        """Round the window's bottom-left corner, which is the strip's own."""
        self._corner_radius = radius
        self._apply_style()

    def _apply_style(self) -> None:
        self.setStyleSheet(
            _style(self._theme_name)
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
        for button in self._workspaces.values():
            button.set_theme(name)
