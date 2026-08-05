"""Tabbed browser container: a tab strip with a '+' button over a stack
of BrowserWidget instances, one per tab."""

from __future__ import annotations

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import QHBoxLayout, QStackedWidget, QTabBar, QToolButton, QVBoxLayout, QWidget

from kraken.browser.widget import BrowserWidget
from kraken.ui.themes import DEFAULT_THEME

_TAB_ROW_STYLES = {
    "dark": """
#tabRow { background: transparent; }
QTabBar { background: transparent; }
QTabBar::tab {
    background: #26282e;
    color: #c8cad0;
    border: 1px solid #33353c;
    border-radius: 6px;
    font-size: 13px;
    /* Tight on the right: the ✕ button supplies the trailing space. */
    padding: 3px 0 3px 8px;
    margin: 2px 4px 2px 0;
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
    font-size: 13px;
    /* Tight on the right: the ✕ button supplies the trailing space. */
    padding: 3px 0 3px 8px;
    margin: 2px 4px 2px 0;
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


class BrowserTabs(QWidget):
    """Tab bar + '+' button on top, one browser per tab below."""

    # The last tab was closed. The panel reads this as "close me" rather than
    # opening a replacement tab.
    emptied = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._theme_name = DEFAULT_THEME
        self._counter = 0

        self._tab_bar = QTabBar()
        self._tab_bar.setExpanding(False)
        self._tab_bar.setDrawBase(False)
        self._tab_bar.setMovable(True)
        self._tab_bar.setUsesScrollButtons(False)
        self._tab_bar.currentChanged.connect(self._on_current_changed)
        self._tab_bar.tabMoved.connect(self._on_tab_moved)

        plus = QToolButton()
        plus.setText("+")
        plus.setToolTip("New browser tab")
        plus.setCursor(Qt.CursorShape.PointingHandCursor)
        plus.clicked.connect(self.add_browser)

        tab_row = QWidget()
        tab_row.setObjectName("tabRow")
        tab_row.setStyleSheet(_TAB_ROW_STYLES[self._theme_name])
        self._tab_row = tab_row
        row_layout = QHBoxLayout(tab_row)
        self._row_layout = row_layout
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

        self.add_browser()

    def mount_grip(self, grip: QWidget) -> None:
        """Seat the dock's drag grip at the head of the tab strip, ahead of the
        first tab."""
        self._row_layout.insertWidget(0, grip)
        self._row_layout.insertSpacing(1, 6)

    # ---- Tabs ----------------------------------------------------------

    @property
    def current_browser(self) -> BrowserWidget | None:
        widget = self._stack.currentWidget()
        return widget if isinstance(widget, BrowserWidget) else None

    def browsers(self) -> list[BrowserWidget]:
        return [self._stack.widget(i) for i in range(self._stack.count())]  # type: ignore[arg-type]

    def open_url(self, url: str) -> None:
        """Load a URL in the current tab (creating one if none exist)."""
        browser = self.current_browser or self.add_browser()
        browser.navigate(url)

    def add_browser(self) -> BrowserWidget:
        browser = BrowserWidget(self)
        if self._theme_name != DEFAULT_THEME:
            browser.set_theme(self._theme_name)
        self._stack.addWidget(browser)

        self._counter += 1
        label = "Browser" if self._counter == 1 else f"Browser#{self._counter}"
        index = self._tab_bar.addTab(label)

        close = QToolButton()
        close.setText("✕")
        close.setToolTip("Close browser tab")
        close.setCursor(Qt.CursorShape.PointingHandCursor)
        close.setFixedSize(16, 16)
        close.setStyleSheet(
            "QToolButton { font-size: 10px; padding: 0; border-radius: 8px; }"
        )
        close.clicked.connect(lambda: self._close_browser(browser))
        holder = QWidget()
        holder.setFixedSize(22, 16)
        holder_layout = QHBoxLayout(holder)
        holder_layout.setContentsMargins(0, 0, 6, 0)
        holder_layout.addWidget(close)
        self._tab_bar.setTabButton(index, QTabBar.ButtonPosition.RightSide, holder)

        self._tab_bar.setCurrentIndex(index)
        browser.setFocus()
        return browser

    def _close_browser(self, browser: BrowserWidget) -> None:
        index = self._stack.indexOf(browser)
        if index < 0:
            return
        self._stack.removeWidget(browser)
        browser.deleteLater()
        self._tab_bar.removeTab(index)
        if self._stack.count() == 0:
            # A replacement tab would pin a Chromium renderer (~200MB) for a
            # browser nobody asked to keep open, so the last ✕ closes the
            # panel instead and the panel drops the strip.
            self._counter = 0
            self.emptied.emit()
        else:
            current = self.current_browser
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

    # ---- Theme / lifecycle ---------------------------------------------

    @property
    def theme_name(self) -> str:
        return self._theme_name

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._tab_row.setStyleSheet(_TAB_ROW_STYLES[name])
        for browser in self.browsers():
            browser.set_theme(name)
