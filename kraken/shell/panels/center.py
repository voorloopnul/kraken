"""Conversation pane: the stack of per-session transcripts plus chat input."""

import time

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QScrollBar,
    QSizePolicy,
    QSpacerItem,
    QStackedWidget,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken.chat.typography import DEFAULT_SIZE, clamp, secondary
from kraken.shell.panels.base import Panel
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS


class CenterPanel(Panel):
    """Conversation pane: a stack of per-session transcripts (only the focused
    one is shown), a busy row (with Stop), and the chat input. The workspace
    owns one transcript widget per live session and flips between them here.

    The content is capped at MAX_CONTENT_WIDTH and centred: it takes all
    width until it hits its maximum, then the zero-stretch side spacers split
    the leftover equally. The transcripts' own scrollbars are hidden; one
    external scrollbar at the panel's right edge mirrors whichever transcript
    is focused, so the bar sits on the panel rather than the content column.
    The rows below reserve the scrollbar's width so their centring matches
    the transcript row's."""

    MAX_CONTENT_WIDTH = 1000

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        from kraken.chat.chat_input import ChatInput

        self._theme_name = DEFAULT_THEME
        self._font_size = DEFAULT_SIZE
        self._scrollbar = QScrollBar(Qt.Orientation.Vertical)
        # Keep the transcript row's width stable when the bar hides.
        policy = self._scrollbar.sizePolicy()
        policy.setRetainSizeWhenHidden(True)
        self._scrollbar.setSizePolicy(policy)
        self._scrollbar.hide()
        self._bound: QScrollBar | None = None  # mirrored inner scrollbar

        self.conversation_stack = QStackedWidget()
        self.conversation_stack.setMaximumWidth(self.MAX_CONTENT_WIDTH)
        # Both rows are built the same way: a gutter the width of the
        # scrollbar on each side, and the column centred in what is left. The
        # matching gutter on the left is what keeps the column centred on the
        # panel rather than pushed off it — the bar is reserved whether or not
        # it is showing (see retainSizeWhenHidden above), so reserving it on
        # one side only left everything sitting a scrollbar's width to port.
        #
        # Spacing is zeroed for the same reason the two rows mirror each
        # other: a default spacing lands between two *widgets* but not between
        # a widget and a spacer, so the row holding the scrollbar would divide
        # a different width from the row holding a spacer in its place.
        self._gutters: list[QSpacerItem] = []
        top = QHBoxLayout()
        top.setSpacing(0)
        top.addItem(self._new_gutter())
        top.addStretch()
        top.addWidget(self.conversation_stack, stretch=1)
        top.addStretch()
        top.addWidget(self._scrollbar)
        self._top_row = top
        self._layout.addLayout(top, stretch=1)

        bottom_content = QWidget()
        bottom_content.setMaximumWidth(self.MAX_CONTENT_WIDTH)
        column = QVBoxLayout(bottom_content)
        column.setContentsMargins(0, 0, 0, 0)

        self._busy_label = QLabel("Pi is working…")
        # Elapsed time of the current turn, ticked once a second while busy.
        self._elapsed_label = QLabel("")
        self._elapsed_started: float | None = None
        self._elapsed_timer = QTimer(self)
        self._elapsed_timer.setInterval(1000)
        self._elapsed_timer.timeout.connect(self._refresh_elapsed)
        self.stop_button = QToolButton()
        self.stop_button.setText("Stop")
        self.stop_button.setCursor(Qt.CursorShape.PointingHandCursor)
        busy_row = QWidget()
        self._busy_row = busy_row
        row_layout = QHBoxLayout(busy_row)
        row_layout.setContentsMargins(4, 0, 4, 2)
        row_layout.addWidget(self._busy_label)
        row_layout.addSpacing(8)
        row_layout.addWidget(self._elapsed_label)
        row_layout.addStretch(1)
        row_layout.addWidget(self.stop_button)
        busy_row.setVisible(False)
        column.addWidget(busy_row)

        self.chat = ChatInput()
        column.addWidget(self.chat)

        bottom = QHBoxLayout()
        bottom.setSpacing(0)
        bottom.addItem(self._new_gutter())
        bottom.addStretch()
        bottom.addWidget(bottom_content, stretch=1)
        bottom.addStretch()
        # Stands in for the scrollbar the row above ends with. Its width comes
        # from that scrollbar rather than from the style's PM_ScrollBarExtent:
        # the bar is styled to 10px and the platform metric says 14, so
        # measuring it the second way left the input four pixels left of the
        # transcript it is supposed to line up under.
        bottom.addItem(self._new_gutter())
        self._bottom_row = bottom
        self._layout.addLayout(bottom)
        self._sync_gutter()

    def _new_gutter(self) -> QSpacerItem:
        """A spacer kept at the scrollbar's width by _sync_gutter."""
        item = QSpacerItem(0, 0, QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Minimum)
        self._gutters.append(item)
        return item

    def _sync_gutter(self) -> None:
        """Hold every gutter to the width the scrollbar actually occupies.
        Called again whenever the bar is restyled, since that is what sets its
        width."""
        width = self._scrollbar.sizeHint().width()
        for item in self._gutters:
            item.changeSize(
                width, 0, QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Minimum
            )
        self._top_row.invalidate()
        self._bottom_row.invalidate()

    def add_conversation(self, view: QWidget) -> None:
        if self.conversation_stack.indexOf(view) < 0:
            # The external panel-edge scrollbar replaces the built-in one.
            view.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
            self.conversation_stack.addWidget(view)

    def remove_conversation(self, view: QWidget) -> None:
        self.conversation_stack.removeWidget(view)

    def set_focused_conversation(self, view: QWidget) -> None:
        self.add_conversation(view)
        self.conversation_stack.setCurrentWidget(view)
        self._bind_scrollbar(view.verticalScrollBar())

    def _bind_scrollbar(self, inner: QScrollBar) -> None:
        """Mirror the focused transcript's (hidden) scrollbar onto the
        external one: ranges and values stay in sync both ways. setValue is
        a no-op at the current value, so the mutual connection can't loop."""
        if inner is self._bound:
            return
        if self._bound is not None:
            self._bound.rangeChanged.disconnect(self._on_inner_range)
            self._bound.valueChanged.disconnect(self._scrollbar.setValue)
            self._scrollbar.valueChanged.disconnect(self._bound.setValue)
        self._bound = inner
        inner.rangeChanged.connect(self._on_inner_range)
        inner.valueChanged.connect(self._scrollbar.setValue)
        self._scrollbar.valueChanged.connect(inner.setValue)
        self._on_inner_range(inner.minimum(), inner.maximum())
        self._scrollbar.setValue(inner.value())

    def _on_inner_range(self, minimum: int, maximum: int) -> None:
        self._scrollbar.setRange(minimum, maximum)
        if self._bound is not None:
            self._scrollbar.setPageStep(self._bound.pageStep())
            self._scrollbar.setSingleStep(self._bound.singleStep())
        self._scrollbar.setVisible(maximum > minimum)

    def set_busy(self, busy: bool, started: float | None = None) -> None:
        self._busy_row.setVisible(busy)
        if busy:
            # Anchor to the turn's real start (monotonic) when the controller
            # knows it, so switching to an already-running session shows its
            # true elapsed rather than restarting from zero.
            self._elapsed_started = started if started is not None else time.monotonic()
            self._refresh_elapsed()
            self._elapsed_timer.start()
        else:
            self._elapsed_timer.stop()
            self._elapsed_started = None
            self._elapsed_label.clear()

    def _refresh_elapsed(self) -> None:
        if self._elapsed_started is None:
            return
        self._elapsed_label.setText(
            self._format_elapsed(int(time.monotonic() - self._elapsed_started))
        )

    @staticmethod
    def _format_elapsed(seconds: int) -> str:
        seconds = max(seconds, 0)
        if seconds < 60:
            return f"{seconds}s"
        minutes, secs = divmod(seconds, 60)
        if minutes < 60:
            return f"{minutes}:{secs:02d}"
        hours, minutes = divmod(minutes, 60)
        return f"{hours}:{minutes:02d}:{secs:02d}"

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        # The transcripts paint transparent, so the stack that hosts them must
        # carry the window background; the SessionControllers theme the text.
        self.conversation_stack.setStyleSheet(
            f"QStackedWidget {{ background: {UI_COLORS[name]['window']}; }}"
        )
        self.chat.set_theme(name)
        handle, handle_hover = (
            ("#3a3d45", "#4a4e58") if name == "dark" else ("#c9c9ce", "#b3b4b9")
        )
        self._scrollbar.setStyleSheet(
            "QScrollBar:vertical { background: transparent; border: none;"
            " width: 10px; margin: 0; }"
            f" QScrollBar::handle:vertical {{ background: {handle};"
            " border-radius: 5px; min-height: 24px; }"
            f" QScrollBar::handle:vertical:hover {{ background: {handle_hover}; }}"
            " QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical"
            " { height: 0; }"
            " QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical"
            " { background: transparent; }"
        )
        # The rule above is what gives the bar its width, so the gutter that
        # mirrors it is re-measured here rather than only at construction.
        self._sync_gutter()
        self._apply_busy_style()

    def set_font_size(self, size: int) -> None:
        """Scale the whole conversation pane with the transcript: the composer
        below it and the busy row between them, which would otherwise stay at
        their own fixed sizes while the messages moved."""
        self._font_size = clamp(size)
        self.chat.set_font_size(self._font_size)
        self._apply_busy_style()

    def _apply_busy_style(self) -> None:
        dark = self._theme_name == "dark"
        dim, hover = ("#7a7d85", "#2c2e35") if dark else ("#5f6269", "#e0e0e4")
        size = secondary(self._font_size)
        self._busy_row.setStyleSheet(
            f"QLabel {{ color: {dim}; font-size: {size}px; font-style: italic; }}"
            f" QToolButton {{ background: transparent; border: none; border-radius: 4px;"
            f" color: {dim}; font-size: {size}px; padding: 2px 6px; }}"
            f" QToolButton:hover {{ background: {hover}; }}"
        )
