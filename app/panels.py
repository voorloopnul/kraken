"""The three content panels. Replace the placeholder widgets with real content."""

import html
import subprocess

from PySide6.QtCore import Qt, QSize, Signal
from PySide6.QtGui import (
    QColor,
    QGuiApplication,
    QIcon,
    QPainter,
    QPixmap,
    QTextDocument,
)
from PySide6.QtWidgets import (
    QFrame,
    QGraphicsDropShadowEffect,
    QLabel,
    QHBoxLayout,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QScrollBar,
    QStackedWidget,
    QStyle,
    QStyledItemDelegate,
    QStyleOptionViewItem,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from app.themes import DEFAULT_THEME, LIGHT, UI_COLORS


def _dot_icon(color: str) -> QIcon:
    """A small filled circle used as a history-row status indicator."""
    size = 10
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(color))
    painter.drawEllipse(1, 1, size - 2, size - 2)
    painter.end()
    return QIcon(pixmap)


_HISTORY_STYLES = {
    "dark": """
QListWidget { background: transparent; border: none; color: #c8cad0; font-size: 12px; }
QListWidget::item { padding: 6px 8px; border-radius: 6px; }
QListWidget::item:hover { background: #2c2e35; }
QListWidget::item:selected { background: #1c2f50; color: #ffffff; }
""",
    "light": """
QListWidget { background: transparent; border: none; color: #4a4d55; font-size: 12px; }
QListWidget::item { padding: 6px 8px; border-radius: 6px; }
QListWidget::item:hover { background: #e8e8ec; }
QListWidget::item:selected { background: #dce8fb; color: #1b1d22; }
""",
}

_NEW_SESSION_STYLES = {
    "dark": """
QPushButton { background: #26282e; border: 1px solid #33353c; border-radius: 6px;
              color: #c8cad0; font-size: 12px; padding: 5px 8px; }
QPushButton:hover { background: #2c2e35; color: #ffffff; }
""",
    "light": """
QPushButton { background: #f5f5f6; border: 1px solid #d8d8dd; border-radius: 6px;
              color: #4a4d55; font-size: 12px; padding: 5px 8px; }
QPushButton:hover { background: #e8e8ec; color: #1b1d22; }
""",
}


class Card(QFrame):
    """A rounded-border container to group panel content.

    `shadow=False` matters for cards hosting a QWebEngineView: a graphics
    effect makes Qt render the subtree through a cached pixmap, freezing
    Chromium's composited output — the page looks unresponsive even though
    input still reaches it."""

    def __init__(self, parent: QWidget | None = None, shadow: bool = True):
        super().__init__(parent)
        self.setObjectName("card")
        self.set_colors("#%02X%02X%02X" % LIGHT.background, "#e0e0e0")
        if shadow:
            effect = QGraphicsDropShadowEffect(self)
            effect.setBlurRadius(12)
            effect.setOffset(0, 2)
            effect.setColor(QColor(0, 0, 0, 40))
            self.setGraphicsEffect(effect)

        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(12, 12, 12, 12)

    def set_colors(self, background: str, border: str) -> None:
        self.setStyleSheet(
            "#card {"
            f" background: {background};"
            f" border: 1px solid {border};"
            " border-radius: 8px;"
            "}"
        )

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)


class Panel(QWidget):
    """Base panel: a container with a vertical layout to add widgets to."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(8, 8, 8, 8)

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)


class LeftPanel(Panel):
    """History pane: previous Pi agent sessions recorded for the workspace
    folder. Emits `session_selected` with the session file path on click."""

    session_selected = Signal(str)
    new_session_requested = Signal()
    session_removed = Signal(str)  # a session's path was archived or deleted

    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        self._cwd = cwd
        self._card = Card()
        title = QLabel("History")
        self._new_button = QPushButton("＋  New Session")
        self._new_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self._new_button.clicked.connect(self.new_session_requested)
        self._list = QListWidget()
        self._list.setWordWrap(True)
        self._list.setHorizontalScrollBarPolicy(
            Qt.ScrollBarPolicy.ScrollBarAlwaysOff
        )
        self._list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        self._list.itemClicked.connect(self._on_item_clicked)
        self._list.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._list.customContextMenuRequested.connect(self._on_context_menu)
        # Per-session status shown as a coloured dot to the left of each row:
        # amber = a turn is streaming now, green = finished with a result the
        # user hasn't opened yet. Both are keyed by session file path.
        self._list.setIconSize(QSize(10, 10))
        self._running: set[str] = set()
        self._unseen: set[str] = set()
        # Live, in-flight sessions the workspace supplies: (key, title,
        # path-or-None, running). They're listed even before Pi has written
        # the session file to disk, so a running session is always reachable.
        self._live: list[tuple[str, str, str | None, bool]] = []
        self._running_icon = _dot_icon("#e0a030")
        self._unseen_icon = _dot_icon("#2ea043")
        self._card.add_widget(title)
        self._card.add_widget(self._new_button)
        self._card.add_widget(self._list, stretch=1)
        self.add_widget(self._card, stretch=1)
        self.refresh()

    def refresh(self) -> None:
        """Rebuild the list from live in-flight sessions plus the ones Pi has
        written to disk (deduped by path)."""
        from app.pi_sessions import sessions_for

        selected = self._selected_path()
        self._list.clear()
        sessions = sessions_for(self._cwd) if self._cwd else []
        disk_paths = {str(s.path) for s in sessions}

        # Live sessions first: those not yet on disk get their own row so the
        # user can jump back to a running session mid-turn. A live session that
        # Pi has already persisted is left to its disk row below (the running
        # dot still marks it).
        for key, title, path, _running in self._live:
            if path and path in disk_paths:
                continue
            item = QListWidgetItem(f"{title}\nrunning…")
            item.setData(Qt.ItemDataRole.UserRole, key)
            self._list.addItem(item)
            if key == selected:
                item.setSelected(True)

        if not sessions and not self._live:
            item = QListWidgetItem("No previous sessions")
            item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._list.addItem(item)
            return

        for session in sessions:
            noun = "message" if session.message_count == 1 else "messages"
            item = QListWidgetItem(
                f"{session.title}\n{session.started:%b %d, %H:%M}"
                f" · {session.message_count} {noun}"
            )
            item.setData(Qt.ItemDataRole.UserRole, str(session.path))
            item.setData(Qt.ItemDataRole.UserRole + 1, session.session_id)
            item.setToolTip(str(session.path))
            self._list.addItem(item)
            if str(session.path) == selected:
                item.setSelected(True)
        self._apply_status()

    def set_live_sessions(self, entries: list[tuple[str, str, str | None, bool]]) -> None:
        """Supply the workspace's live, in-flight sessions (key, title, path,
        running). Stored for the next refresh."""
        self._live = list(entries)

    def set_session_status(self, running: set[str], unseen: set[str]) -> None:
        """Update which sessions show the running / done-unseen dot. Called by
        the workspace as agents start, finish, and get opened. A full refresh
        keeps live-session rows in sync with their running state."""
        self._running = set(running)
        self._unseen = set(unseen)
        self.refresh()

    def _apply_status(self) -> None:
        for i in range(self._list.count()):
            item = self._list.item(i)
            path = item.data(Qt.ItemDataRole.UserRole)
            if path in self._running:
                item.setIcon(self._running_icon)
            elif path in self._unseen:
                item.setIcon(self._unseen_icon)
            else:
                item.setIcon(QIcon())

    def clear_selection(self) -> None:
        self._list.clearSelection()

    def _selected_path(self) -> str | None:
        items = self._list.selectedItems()
        return items[0].data(Qt.ItemDataRole.UserRole) if items else None

    def _on_item_clicked(self, item: QListWidgetItem) -> None:
        path = item.data(Qt.ItemDataRole.UserRole)
        if path:
            self.session_selected.emit(path)

    def _on_context_menu(self, pos) -> None:
        item = self._list.itemAt(pos)
        # Only persisted sessions (which carry a session id) can be archived or
        # deleted; live in-flight rows have no file to act on.
        if item is None or not item.data(Qt.ItemDataRole.UserRole + 1):
            return
        menu = QMenu(self._list)
        archive_action = menu.addAction("Archive")
        delete_action = menu.addAction("Delete")
        chosen = menu.exec(self._list.viewport().mapToGlobal(pos))
        if chosen is archive_action:
            self._archive(item)
        elif chosen is delete_action:
            self._delete(item)

    def _archive(self, item: QListWidgetItem) -> None:
        from app.pi_sessions import archive_session

        session_id = item.data(Qt.ItemDataRole.UserRole + 1)
        archive_session(session_id)
        self.refresh()
        self.session_removed.emit(item.data(Qt.ItemDataRole.UserRole))

    def _delete(self, item: QListWidgetItem) -> None:
        from app.pi_sessions import delete_session

        if (
            QMessageBox.question(
                self,
                "Delete session",
                "Permanently delete this session? This cannot be undone.",
            )
            != QMessageBox.StandardButton.Yes
        ):
            return
        path = item.data(Qt.ItemDataRole.UserRole)
        delete_session(path)
        self.refresh()
        self.session_removed.emit(path)

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self._list.setStyleSheet(_HISTORY_STYLES[name])
        self._new_button.setStyleSheet(_NEW_SESSION_STYLES[name])


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
        from app.chat_input import ChatInput

        self._scrollbar = QScrollBar(Qt.Orientation.Vertical)
        # Keep the transcript row's width stable when the bar hides.
        policy = self._scrollbar.sizePolicy()
        policy.setRetainSizeWhenHidden(True)
        self._scrollbar.setSizePolicy(policy)
        self._scrollbar.hide()
        self._bound: QScrollBar | None = None  # mirrored inner scrollbar

        self.conversation_stack = QStackedWidget()
        self.conversation_stack.setMaximumWidth(self.MAX_CONTENT_WIDTH)
        top = QHBoxLayout()
        top.addStretch()
        top.addWidget(self.conversation_stack, stretch=1)
        top.addStretch()
        top.addWidget(self._scrollbar)
        self._layout.addLayout(top, stretch=1)

        bottom_content = QWidget()
        bottom_content.setMaximumWidth(self.MAX_CONTENT_WIDTH)
        column = QVBoxLayout(bottom_content)
        column.setContentsMargins(0, 0, 0, 0)

        self._busy_label = QLabel("Pi is working…")
        self.stop_button = QToolButton()
        self.stop_button.setText("Stop")
        self.stop_button.setCursor(Qt.CursorShape.PointingHandCursor)
        busy_row = QWidget()
        self._busy_row = busy_row
        row_layout = QHBoxLayout(busy_row)
        row_layout.setContentsMargins(4, 0, 4, 2)
        row_layout.addWidget(self._busy_label)
        row_layout.addStretch(1)
        row_layout.addWidget(self.stop_button)
        busy_row.setVisible(False)
        column.addWidget(busy_row)

        self.chat = ChatInput()
        column.addWidget(self.chat)

        bottom = QHBoxLayout()
        bottom.addStretch()
        bottom.addWidget(bottom_content, stretch=1)
        bottom.addStretch()
        bottom.addSpacing(
            self.style().pixelMetric(QStyle.PixelMetric.PM_ScrollBarExtent)
        )
        self._layout.addLayout(bottom)

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

    def set_busy(self, busy: bool) -> None:
        self._busy_row.setVisible(busy)

    def set_theme(self, name: str) -> None:
        # The transcripts paint transparent, so the stack that hosts them must
        # carry the window background; the SessionControllers theme the text.
        self.conversation_stack.setStyleSheet(
            f"QStackedWidget {{ background: {UI_COLORS[name]['window']}; }}"
        )
        self.chat.set_theme(name)
        handle, handle_hover = (
            ("#3a3d45", "#4a4e58") if name == "dark" else ("#c9c4b4", "#b3ae9e")
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
        dim, hover = ("#7a7d85", "#2c2e35") if name == "dark" else ("#5f6269", "#e0e0e4")
        self._busy_row.setStyleSheet(
            f"QLabel {{ color: {dim}; font-size: 12px; font-style: italic; }}"
            f" QToolButton {{ background: transparent; border: none; border-radius: 4px;"
            f" color: {dim}; font-size: 12px; padding: 2px 6px; }}"
            f" QToolButton:hover {{ background: {hover}; }}"
        )


class BrowserPanel(Panel):
    """Web browser pane, the sibling of the terminal pane. The tabbed
    browser (and its Chromium processes) is only created the first time the
    panel becomes visible, so hidden panels cost nothing."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._card = Card(shadow=False)
        self._theme_name = DEFAULT_THEME
        self.browsers = None
        self._creating = False
        self.add_widget(self._card, stretch=1)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self._ensure_tabs()

    def _ensure_tabs(self) -> None:
        # Re-entrancy guard: web view construction can pump the event loop
        # (Chromium/GL init), letting a second showEvent arrive while the
        # first BrowserTabs is still mid-construction — without the flag
        # that stacked a duplicate browser into the card.
        if self.browsers is not None or self._creating:
            return
        self._creating = True
        try:
            from app.browser_tabs import BrowserTabs

            tabs = BrowserTabs(self)
            tabs.set_theme(self._theme_name)
            self._card.add_widget(tabs, stretch=1)
            self.browsers = tabs
        finally:
            self._creating = False

    def open_url(self, url: str) -> None:
        """Load a URL in the panel's current tab, revealing the panel (and
        building the lazily-created tabs) if needed."""
        self.show()
        self._ensure_tabs()
        if self.browsers is not None:
            self.browsers.open_url(url)

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        if self.browsers is not None:
            self.browsers.set_theme(name)


# Scrollbars match the conversation panel's external one: a bare rounded
# handle on a transparent track, no stepper buttons.
_GIT_LOG_STYLES = {
    "dark": """
QListWidget { background: transparent; border: none; color: #c8cad0;
              font-family: monospace; font-size: 11px; }
QListWidget::item { padding: 2px 4px; border-radius: 4px; }
QListWidget::item:hover { background: #2c2e35; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #3a3d45; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #4a4e58; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #3a3d45; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #4a4e58; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
    "light": """
QListWidget { background: transparent; border: none; color: #4a4d55;
              font-family: monospace; font-size: 11px; }
QListWidget::item { padding: 2px 4px; border-radius: 4px; }
QListWidget::item:hover { background: #e8e8ec; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #c9c4b4; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #b3ae9e; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #c9c4b4; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #b3ae9e; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
}


class _RichTextDelegate(QStyledItemDelegate):
    """Paints item text as HTML so parts of a row (the commit hash) can be
    colored independently; backgrounds still come from the widget style."""

    def paint(self, painter, option, index) -> None:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        doc = self._document(opt)
        opt.text = ""
        style = opt.widget.style()
        style.drawControl(
            QStyle.ControlElement.CE_ItemViewItem, opt, painter, opt.widget
        )
        rect = style.subElementRect(
            QStyle.SubElement.SE_ItemViewItemText, opt, opt.widget
        )
        painter.save()
        painter.translate(
            rect.left(), rect.top() + (rect.height() - doc.size().height()) / 2
        )
        doc.drawContents(painter)
        painter.restore()

    def sizeHint(self, option, index) -> QSize:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        doc = self._document(opt)
        # Mirror the stylesheet's 2px/4px item padding.
        return QSize(int(doc.idealWidth()) + 8, int(doc.size().height()) + 4)

    @staticmethod
    def _document(opt: QStyleOptionViewItem) -> QTextDocument:
        doc = QTextDocument()
        doc.setDocumentMargin(0)
        doc.setDefaultFont(opt.font)
        doc.setHtml(opt.text)
        return doc


def _mono(text: str, color: str) -> str:
    """Escape one span of row text, keeping runs of spaces (graph columns)."""
    return (
        f'<span style="color: {color};">'
        f'{html.escape(text).replace(" ", "&nbsp;")}</span>'
    )


# Row text per theme, plus the hash accents: pastel green for commits on the
# main line (reachable from master/main), light gray for side-branch work.
_GIT_TEXT_COLORS = {"dark": "#c8cad0", "light": "#4a4d55"}
_MAIN_HASH_COLORS = {"dark": "#98c379", "light": "#50a14f"}
_OFF_MAIN_COLORS = {"dark": "#7a7d85", "light": "#9a9da5"}


class GitPanel(Panel):
    """Git history pane: the workspace repo's commit graph, one row per
    `git log --graph` line. Refreshes whenever the panel becomes visible —
    which covers toggling it on and switching workspaces — plus a manual
    refresh button for commits made while it's showing."""

    _MAX_COMMITS = 200

    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        self._cwd = cwd
        self._theme_name = DEFAULT_THEME
        self._card = Card()

        header = QWidget()
        header_layout = QHBoxLayout(header)
        header_layout.setContentsMargins(0, 0, 0, 0)
        self.refresh_button = QToolButton()
        self.refresh_button.setText("↻")
        self.refresh_button.setToolTip("Refresh")
        self.refresh_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.refresh_button.clicked.connect(self.refresh)
        header_layout.addWidget(QLabel("Git History"))
        header_layout.addStretch(1)
        header_layout.addWidget(self.refresh_button)

        # Rows act through their context menu only, so no selection; the
        # monospace font keeps the graph columns aligned across rows.
        self._list = QListWidget()
        self._list.setSelectionMode(QListWidget.SelectionMode.NoSelection)
        self._list.setWordWrap(False)
        self._list.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._list.customContextMenuRequested.connect(self._on_context_menu)
        # Rows are HTML (see _RichTextDelegate) so the hash can carry its
        # own color while the rest of the row keeps the theme text color.
        self._list.setItemDelegate(_RichTextDelegate(self._list))

        self._card.add_widget(header)
        self._card.add_widget(self._list, stretch=1)
        self.add_widget(self._card, stretch=1)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self.refresh()

    def refresh(self) -> None:
        self._list.clear()
        if not self._cwd:
            return
        # Branches, tags, remotes, and HEAD — not --all, which also walks
        # tool-owned refs (editor local-history, agent checkpoints) and shows
        # "history" in repos whose branches have no commits at all. HEAD is
        # listed explicitly so a detached checkout (from the context menu)
        # stays visible, but only when it resolves: an unborn branch's HEAD
        # would make git log fail outright.
        revs = ["--branches", "--tags", "--remotes"]
        if self._head_exists():
            revs.append("HEAD")
        # \x1f-separated fields after the graph prefix; continuation lines
        # (pure graph, like "|/") carry no record at all.
        try:
            result = subprocess.run(
                [
                    "git", "-C", self._cwd, "log", "--graph", *revs,
                    "--format=%x1f%h%x1f%H%x1f%d%x1f%s%x1f%an%x1f%ar",
                    f"-{self._MAX_COMMITS}",
                ],
                capture_output=True,
                text=True,
                errors="replace",
                timeout=5,
            )
        except (OSError, subprocess.TimeoutExpired):
            result = None
        text_color = _GIT_TEXT_COLORS[self._theme_name]
        if result is None or result.returncode != 0:
            stderr = result.stderr.lower() if result is not None else ""
            message = (
                "Not a git repository"
                if "not a git repository" in stderr
                else "No commits"
            )
            item = QListWidgetItem(_mono(message, text_color))
            item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._list.addItem(item)
            return
        main_line = self._main_line_hashes()
        for line in result.stdout.splitlines():
            graph, sep, record = line.partition("\x1f")
            if sep:
                short_hash, full_hash, refs, subject, author, when = record.split(
                    "\x1f"
                )
                if main_line is None:
                    hash_color = text_color  # no master/main to compare with
                elif full_hash in main_line:
                    hash_color = _MAIN_HASH_COLORS[self._theme_name]
                else:
                    hash_color = _OFF_MAIN_COLORS[self._theme_name]
                item = QListWidgetItem(
                    _mono(graph, text_color)
                    + _mono(short_hash, hash_color)
                    + _mono(f"{refs}  {subject}", text_color)
                )
                item.setToolTip(f"{subject}\n{short_hash} — {author}, {when}")
                item.setData(Qt.ItemDataRole.UserRole, short_hash)
                item.setData(Qt.ItemDataRole.UserRole + 2, full_hash)
                # Branches decorating this commit, offered as attached
                # checkouts in the context menu. "HEAD -> x" is the branch
                # we're already on; tags and a bare detached "HEAD" checkout
                # the same as the hash, so neither earns an entry.
                ref_names = [r for r in refs.strip(" ()").split(", ") if r]
                branches = [
                    ref
                    for ref in ref_names
                    if ref != "HEAD" and not ref.startswith(("HEAD -> ", "tag: "))
                ]
                item.setData(Qt.ItemDataRole.UserRole + 1, branches)
                # Bold the checked-out commit so the current state stands out.
                if any(r == "HEAD" or r.startswith("HEAD -> ") for r in ref_names):
                    font = item.font()
                    font.setBold(True)
                    item.setFont(font)
            else:
                item = QListWidgetItem(_mono(graph.rstrip(), text_color))
            self._list.addItem(item)
        if self._list.count() == 0:
            # rc 0 but nothing listed: branches exist but are unborn.
            item = QListWidgetItem(_mono("No commits", text_color))
            item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._list.addItem(item)

    def _head_exists(self) -> bool:
        try:
            return (
                subprocess.run(
                    ["git", "-C", self._cwd, "rev-parse", "--verify",
                     "--quiet", "HEAD"],
                    capture_output=True,
                    timeout=5,
                ).returncode
                == 0
            )
        except (OSError, subprocess.TimeoutExpired):
            return False

    def _main_line_hashes(self) -> set[str] | None:
        """Full hashes reachable from master (or main), or None when neither
        exists to compare against."""
        for ref in ("master", "main"):
            try:
                result = subprocess.run(
                    ["git", "-C", self._cwd, "rev-list", ref, "--"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
            except (OSError, subprocess.TimeoutExpired):
                return None
            if result.returncode == 0:
                return set(result.stdout.split())
        return None

    def _on_context_menu(self, pos) -> None:
        item = self._list.itemAt(pos)
        # Graph continuation rows ("|/") carry no commit to act on.
        commit = item.data(Qt.ItemDataRole.UserRole) if item is not None else None
        if not commit:
            return
        menu = QMenu(self._list)
        targets = {}
        for branch in item.data(Qt.ItemDataRole.UserRole + 1) or []:
            targets[menu.addAction(f"Checkout {branch}")] = branch
        targets[menu.addAction(f"Checkout {commit}")] = commit
        menu.addSeparator()
        copy_action = menu.addAction("Copy hash")
        chosen = menu.exec(self._list.viewport().mapToGlobal(pos))
        if chosen is copy_action:
            # The full hash: short ones go ambiguous as a repo grows.
            QGuiApplication.clipboard().setText(
                item.data(Qt.ItemDataRole.UserRole + 2)
            )
        elif chosen is not None:
            self._checkout(targets[chosen])

    def _checkout(self, commit: str) -> None:
        try:
            result = subprocess.run(
                ["git", "-C", self._cwd, "checkout", commit],
                capture_output=True,
                text=True,
                errors="replace",
                timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            QMessageBox.warning(self, "Checkout failed", str(exc))
            return
        if result.returncode != 0:
            QMessageBox.warning(
                self,
                "Checkout failed",
                result.stderr.strip() or "git checkout failed",
            )
            return
        # HEAD moved: redraw so the (HEAD -> …) decoration follows.
        self.refresh()

    def set_theme(self, name: str) -> None:
        changed = name != self._theme_name
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self._list.setStyleSheet(_GIT_LOG_STYLES[name])
        # Row colors are baked into each item's HTML, so re-render on a
        # theme flip (hidden panels re-render on their next showEvent).
        if changed and self.isVisible():
            self.refresh()
        dim, hover = (
            ("#7a7d85", "#2c2e35") if name == "dark" else ("#5f6269", "#e8e8ec")
        )
        self.refresh_button.setStyleSheet(
            f"QToolButton {{ background: transparent; border: none;"
            f" border-radius: 4px; color: {dim}; font-size: 13px;"
            f" padding: 0 4px; }}"
            f" QToolButton:hover {{ background: {hover}; }}"
        )


class RightPanel(Panel):
    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        from app.terminal_tabs import TerminalTabs

        self._card = Card()
        self.terminals = TerminalTabs(self, cwd=cwd)
        self._card.add_widget(self.terminals, stretch=1)
        self.add_widget(self._card, stretch=1)

    @property
    def theme_name(self) -> str:
        return self.terminals.theme_name

    def set_theme(self, name: str) -> None:
        """Theme the card and everything inside it (tab strip, terminals)."""
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self.terminals.set_theme(name)
