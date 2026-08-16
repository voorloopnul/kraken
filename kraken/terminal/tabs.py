"""Tabbed terminal container: a tab strip with a "+" button over a stack
of GhosttyTerminalWidget instances, one shell per tab."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QHBoxLayout,
    QStackedWidget,
    QTabBar,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken.terminal.widget import GhosttyTerminalWidget
from kraken.terminal.typography import DEFAULT_SIZE, clamp
from kraken.ui.chrome import PANEL_HEADER_HEIGHT, tab_strip_style
from kraken.ui.themes import DEFAULT_THEME

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

# What the card used to pad around the whole pane. The strip runs full width
# now, so the padding belongs to what is under it instead.
_CONTENT_PADDING = 12


class TerminalTabs(QWidget):
    """Tab bar + "+" button on top, one terminal per tab below."""

    def __init__(
        self,
        parent: QWidget | None = None,
        cwd: str | None = None,
        remote: "RemoteTarget | None" = None,
        font_size: int = DEFAULT_SIZE,
    ):
        super().__init__(parent)
        self._theme_name = DEFAULT_THEME
        self._counter = 0
        self._cwd = cwd
        self._remote = remote
        self._font_size = clamp(font_size)

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
        # QWidget subclasses ignore stylesheet backgrounds without this; the
        # strip has one of its own now (see chrome.header_style).
        tab_row.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        tab_row.setFixedHeight(PANEL_HEADER_HEIGHT)
        tab_row.setStyleSheet(tab_strip_style(self._theme_name))
        self._tab_row = tab_row
        row_layout = QHBoxLayout(tab_row)
        self._row_layout = row_layout
        row_layout.setContentsMargins(8, 0, 8, 0)
        row_layout.setSpacing(0)
        row_layout.addWidget(self._tab_bar)
        row_layout.addWidget(plus)
        row_layout.addStretch(1)

        self._stack = QStackedWidget()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(tab_row)
        # The strip is full-bleed and the terminals under it are inset, so the
        # padding sits here rather than around the pane as a whole.
        body = QVBoxLayout()
        body.setContentsMargins(
            _CONTENT_PADDING, _CONTENT_PADDING, _CONTENT_PADDING, _CONTENT_PADDING
        )
        body.addWidget(self._stack)
        layout.addLayout(body, stretch=1)

        self.add_terminal()

    def mount_grip(self, grip: QWidget) -> None:
        """Seat the dock's drag grip at the head of the tab strip, ahead of the
        first tab."""
        self._row_layout.insertWidget(0, grip)
        self._row_layout.insertSpacing(1, 6)

    # ---- Tabs ----------------------------------------------------------

    @property
    def current_terminal(self) -> GhosttyTerminalWidget | None:
        widget = self._stack.currentWidget()
        return widget if isinstance(widget, GhosttyTerminalWidget) else None

    def terminals(self) -> list[GhosttyTerminalWidget]:
        return [self._stack.widget(i) for i in range(self._stack.count())]

    def add_terminal(self) -> GhosttyTerminalWidget:
        term = GhosttyTerminalWidget(
            self,
            cwd=self._cwd,
            remote=self._remote,
            font_size=self._font_size,
        )
        if self._theme_name != DEFAULT_THEME:
            term.set_theme(self._theme_name)
        self._stack.addWidget(term)

        self._counter += 1
        label = "Terminal" if self._counter == 1 else f"Terminal#{self._counter}"
        index = self._tab_bar.addTab(label)

        close = QToolButton()
        # Styled by the strip (#tabClose) rather than here, so it follows a
        # theme change like everything else in the row.
        close.setObjectName("tabClose")
        close.setText("✕")
        close.setToolTip("Close terminal")
        close.setCursor(Qt.CursorShape.PointingHandCursor)
        close.setFixedSize(16, 16)
        close.clicked.connect(lambda: self._close_terminal(term))
        # Wrapper keeps the button inside the pill: the tab bar positions
        # side-buttons relative to the tab rect, which includes the pill's
        # 4px margin-right, so without the inset the ✕ lands on the border.
        holder = QWidget()
        holder.setFixedSize(22, 16)
        holder_layout = QHBoxLayout(holder)
        holder_layout.setContentsMargins(0, 0, 6, 0)
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
        self._tab_row.setStyleSheet(tab_strip_style(name))
        for term in self.terminals():
            term.set_theme(name)

    def set_font_size(self, size: int) -> None:
        self._font_size = clamp(size)
        for term in self.terminals():
            term.set_font_size(self._font_size)

    def shutdown_all(self) -> None:
        for term in self.terminals():
            term.shutdown()
