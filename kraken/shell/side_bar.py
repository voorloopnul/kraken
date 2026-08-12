"""Vertical icon strip docked at the window's right edge, IDE tool-window
style: a top group of tool buttons and a bottom group pinned to the bottom.
Buttons are placeholders — connect to their `clicked` signals to add actions."""

from __future__ import annotations

from PySide6.QtCore import QSize, Qt
from PySide6.QtWidgets import QToolButton, QVBoxLayout, QWidget

from kraken.ui.chrome import corner_style
from kraken.ui.icons import icon
from kraken.ui.themes import DEFAULT_THEME

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
            _STYLES[self._theme_name]
            + corner_style("#sideBar", ("bottom-right",), self._corner_radius)
        )

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._apply_style()
        for button_name, icon_name in _ICONS.items():
            self.buttons[button_name].setIcon(icon(icon_name, _ICON_COLORS[name]))
