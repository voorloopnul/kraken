"""Tabbed terminal container: a tab strip with a "+" button over a stack
of GhosttyTerminalWidget instances, one shell per tab."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QHBoxLayout,
    QStackedWidget,
    QTabBar,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken.terminal_widget import GhosttyTerminalWidget
from kraken.themes import DEFAULT_THEME

_TAB_ROW_STYLES = {
    "dark": """
#tabRow { background: transparent; }
QTabBar { background: transparent; }
QTabBar::tab {
    background: #26282e;
    color: #c8cad0;
    border: 1px solid #33353c;
    border-radius: 6px;
    padding: 4px 10px;
    margin: 2px 6px 2px 0;
}
QTabBar::tab:selected {
    background: #1c2f50;
    color: #ffffff;
    border: 1px solid #4f83e0;
}
QToolButton {
    background: transparent;
    color: #9a9da5;
    border: none;
    border-radius: 4px;
    font-size: 14px;
    padding: 2px 6px;
}
QToolButton:hover { background: #2c2e35; color: #ffffff; }
""",
    "light": """
#tabRow { background: transparent; }
QTabBar { background: transparent; }
QTabBar::tab {
    background: #f5f5f6;
    color: #4a4d55;
    border: 1px solid #d8d8dd;
    border-radius: 6px;
    padding: 4px 10px;
    margin: 2px 6px 2px 0;
}
QTabBar::tab:selected {
    background: #dce8fb;
    color: #1b1d22;
    border: 1px solid #4f83e0;
}
QToolButton {
    background: transparent;
    color: #6a6d75;
    border: none;
    border-radius: 4px;
    font-size: 14px;
    padding: 2px 6px;
}
QToolButton:hover { background: #dedee2; color: #1b1d22; }
""",
}


class TerminalTabs(QWidget):
    """Tab bar + "+" button on top, one terminal per tab below."""

    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        self._theme_name = DEFAULT_THEME
        self._counter = 0
        self._cwd = cwd

        self._tab_bar = QTabBar()
        self._tab_bar.setExpanding(False)
        self._tab_bar.setDrawBase(False)
        self._tab_bar.setMovable(True)
        # Without this the bar reserves ~30px for scroll buttons, leaving a
        # dead gap between the last tab and the "+" button.
        self._tab_bar.setUsesScrollButtons(False)
        self._tab_bar.currentChanged.connect(self._on_current_changed)
        self._tab_bar.tabMoved.connect(self._on_tab_moved)

        plus = QToolButton()
        plus.setText("+")
        plus.setToolTip("New terminal")
        plus.setCursor(Qt.CursorShape.PointingHandCursor)
        plus.clicked.connect(self.add_terminal)

        tab_row = QWidget()
        tab_row.setObjectName("tabRow")
        tab_row.setStyleSheet(_TAB_ROW_STYLES[self._theme_name])
        self._tab_row = tab_row
        row_layout = QHBoxLayout(tab_row)
        row_layout.setContentsMargins(6, 4, 6, 4)
        row_layout.setSpacing(0)
        row_layout.addWidget(self._tab_bar)
        row_layout.addWidget(plus)
        row_layout.addStretch(1)

        self._stack = QStackedWidget()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(tab_row)
        layout.addWidget(self._stack, stretch=1)

        self.add_terminal()

    # ---- Tabs ----------------------------------------------------------

    @property
    def current_terminal(self) -> GhosttyTerminalWidget | None:
        widget = self._stack.currentWidget()
        return widget if isinstance(widget, GhosttyTerminalWidget) else None

    def terminals(self) -> list[GhosttyTerminalWidget]:
        return [self._stack.widget(i) for i in range(self._stack.count())]

    def add_terminal(self) -> GhosttyTerminalWidget:
        term = GhosttyTerminalWidget(self, cwd=self._cwd)
        if self._theme_name != DEFAULT_THEME:
            term.set_theme(self._theme_name)
        self._stack.addWidget(term)

        self._counter += 1
        label = "Terminal" if self._counter == 1 else f"Terminal ({self._counter})"
        index = self._tab_bar.addTab(label)

        close = QToolButton()
        close.setText("✕")
        close.setToolTip("Close terminal")
        close.setCursor(Qt.CursorShape.PointingHandCursor)
        close.setFixedSize(16, 16)
        close.setStyleSheet(
            "QToolButton { font-size: 10px; padding: 0; border-radius: 8px; }"
        )
        close.clicked.connect(lambda: self._close_terminal(term))
        # Wrapper keeps the button inside the pill: the tab bar positions
        # side-buttons relative to the tab rect, which includes the pill's
        # 6px margin-right, so without the inset the ✕ lands on the border.
        holder = QWidget()
        holder.setFixedSize(26, 16)
        holder_layout = QHBoxLayout(holder)
        holder_layout.setContentsMargins(0, 0, 10, 0)
        holder_layout.addWidget(close)
        self._tab_bar.setTabButton(index, QTabBar.ButtonPosition.RightSide, holder)

        self._tab_bar.setCurrentIndex(index)
        term.setFocus()
        return term

    def _close_terminal(self, term: GhosttyTerminalWidget) -> None:
        index = self._stack.indexOf(term)
        if index < 0:
            return
        term.shutdown()
        self._stack.removeWidget(term)
        term.deleteLater()
        self._tab_bar.removeTab(index)
        if self._stack.count() == 0:
            self._counter = 0
            self.add_terminal()
        else:
            current = self.current_terminal
            if current is not None:
                current.setFocus()

    def _on_current_changed(self, index: int) -> None:
        if 0 <= index < self._stack.count():
            self._stack.setCurrentIndex(index)
            self._stack.widget(index).setFocus()

    def _on_tab_moved(self, from_index: int, to_index: int) -> None:
        widget = self._stack.widget(from_index)
        self._stack.removeWidget(widget)
        self._stack.insertWidget(to_index, widget)
        self._stack.setCurrentIndex(self._tab_bar.currentIndex())

    # ---- Theme / lifecycle ----------------------------------------------

    @property
    def theme_name(self) -> str:
        return self._theme_name

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._tab_row.setStyleSheet(_TAB_ROW_STYLES[name])
        for term in self.terminals():
            term.set_theme(name)

    def shutdown_all(self) -> None:
        for term in self.terminals():
            term.shutdown()
