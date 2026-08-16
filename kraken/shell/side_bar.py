"""Vertical icon strip docked at the window's right edge, IDE tool-window
style: a top group of tool buttons and a bottom group pinned to the bottom.
Buttons are placeholders — connect to their `clicked` signals to add actions."""

from __future__ import annotations

from PySide6.QtCore import QSize, Qt
from PySide6.QtWidgets import QToolButton, QVBoxLayout, QWidget

from kraken.ui.chrome import corner_style
from kraken.ui.icons import toggle_icon
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS


_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def _style(theme: str) -> str:
    """The strip's surface and the buttons on it.

    A button whose panel is open is filled with the accent and carries a white
    glyph — the one thing in this strip that is not grey, because "which panels
    are open" is the only question the strip answers. It used to be marked with
    a grey a shade off the strip itself, which said the same thing so quietly
    that the checked button and its neighbours read as one block."""
    ui = UI_COLORS[theme]
    return f"""
#sideBar {{ background: {ui['sidebar']}; border-left: 1px solid {ui['card_border']}; }}
QToolButton {{
    background: transparent; border: none; border-radius: 6px;
    color: {_ICON_COLORS[theme]}; font-size: 15px; padding: 4px;
}}
QToolButton:hover {{ background: {ui['hover']}; color: {ui['text']}; }}
/* After :hover, so hovering an open panel's button does not grey it out. */
QToolButton:checked {{ background: {ui['accent']}; color: {ui['accent_on']}; }}
"""


# Buttons rendered with icons instead of text glyphs; each is re-rendered on
# a theme change to recolor it.
_ICONS = {
    "Terminal Panel": "square-terminal",
    "Browser Panel": "globe",
    "Diff Panel": "diff",
    "Git Panel": "git-branch",
    "Screenshot": "camera",
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
        self._theme_name = DEFAULT_THEME
        # Set by MainWindow, which owns the window's shape.
        self._corner_radius = 0
        self.buttons: dict[str, QToolButton] = {}

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 8, 4, 8)
        layout.setSpacing(6)
        for name in ("Terminal Panel", "Browser Panel", "Diff Panel", "Git Panel"):
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

    def set_corner_radius(self, radius: int) -> None:
        """Round the window's bottom-right corner, which is the strip's own
        whenever it is showing."""
        self._corner_radius = radius
        self._apply_style()

    def _apply_style(self) -> None:
        self.setStyleSheet(
            _style(self._theme_name)
            + corner_style("#sideBar", ("bottom-right",), self._corner_radius)
        )

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._apply_style()
        ui = UI_COLORS[name]
        for button_name, icon_name in _ICONS.items():
            # Every button gets both glyphs, including the ones that are not
            # checkable: the strip is themed once at construction and again
            # once MainWindow has wired it up, and only the second of those
            # happens after the panel toggles have been made checkable. A
            # button that never checks simply never reaches the second glyph.
            self.buttons[button_name].setIcon(
                toggle_icon(icon_name, _ICON_COLORS[name], ui["accent_on"])
            )
