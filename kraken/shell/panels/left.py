"""History pane: previous Pi agent sessions for the workspace folder."""

import html

from PySide6.QtCore import Qt, QSize, Signal
from PySide6.QtGui import QFont, QIcon, QTextDocument, QTextOption
from PySide6.QtWidgets import (
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QStyle,
    QStyledItemDelegate,
    QStyleOptionViewItem,
    QWidget,
)

from kraken.shell.panels.base import Panel, _dot_icon
from kraken.ui.fonts import UI_SANS_FAMILY
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

# Total item padding from the stylesheet below ("6px 8px"), which the
# delegate has to account for itself when sizing rows.
_ITEM_PAD_X = 16
_ITEM_PAD_Y = 12


# Row backgrounds only: the text color lives in _ROW_COLORS, because the
# delegate paints the rows itself and a stylesheet `color` never reaches it —
# Qt resolves that inside drawControl, on its own copy of the style option.
_HISTORY_STYLES = {
    "dark": """
QListWidget { background: transparent; border: none; font-family: 'Roboto'; font-size: 13px; }
QListWidget::item { padding: 6px 8px; border-radius: 6px; }
QListWidget::item:hover { background: #2c2e35; }
QListWidget::item:selected { background: #1c2f50; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #3a3d45; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #4a4e58; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
""",
    "light": """
QListWidget { background: transparent; border: none; font-family: 'Roboto'; font-size: 13px; }
QListWidget::item { padding: 6px 8px; border-radius: 6px; }
QListWidget::item:hover { background: #e8e8ec; }
QListWidget::item:selected { background: #dce8fb; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #c9c4b4; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #b3ae9e; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
""",
}

# Row text per theme, picked for contrast against the backgrounds above: the
# selected row sits on a tinted band and needs its own value.
_ROW_COLORS = {
    "dark": {"text": "#c8cad0", "selected": "#ffffff"},
    "light": {"text": "#4a4d55", "selected": "#1b1d22"},
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


def _row_html(title: str, subtitle: str) -> str:
    """One history row: the session title in bold over its subtitle line."""
    return f"<b>{html.escape(title)}</b><br>{html.escape(subtitle)}"


class _SessionDelegate(QStyledItemDelegate):
    """Paints a history row as HTML so the title can be bold while the
    subtitle stays regular — an item's own font would apply to both lines.

    Wrapping is done here rather than by the view: QListWidget's word wrap
    only applies to its own plain-text painting, so the document is given the
    row's text width and the row is sized from the height that produces.
    """

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._colors = _ROW_COLORS[DEFAULT_THEME]

    def set_theme(self, name: str) -> None:
        self._colors = _ROW_COLORS[name]

    def paint(self, painter, option, index) -> None:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        style = opt.widget.style()
        selected = bool(opt.state & QStyle.StateFlag.State_Selected)
        # Draw the row itself (background, selection, status dot) without its
        # text, then lay the document into the rect the style reserved for it.
        text = opt.text
        opt.text = ""
        style.drawControl(
            QStyle.ControlElement.CE_ItemViewItem, opt, painter, opt.widget
        )
        rect = style.subElementRect(
            QStyle.SubElement.SE_ItemViewItemText, opt, opt.widget
        )
        doc = self._document(text, opt, rect.width(), selected)
        painter.save()
        painter.translate(rect.left(), rect.top())
        doc.drawContents(painter)
        painter.restore()

    def sizeHint(self, option, index) -> QSize:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        width = self._text_width(opt)
        doc = self._document(opt.text, opt, width, False)
        return QSize(int(width), int(doc.size().height()) + _ITEM_PAD_Y)

    def _text_width(self, opt: QStyleOptionViewItem) -> float:
        """Width available to the text. The option rect is not laid out yet
        when the view asks for a size hint, so measure from the viewport and
        leave room for the padding and the status dot."""
        view = self.parent()
        width = view.viewport().width() - _ITEM_PAD_X
        if not opt.icon.isNull():
            width -= opt.decorationSize.width() + _ITEM_PAD_X / 2
        return max(width, 1)

    def _document(
        self, text: str, opt: QStyleOptionViewItem, width: float, selected: bool
    ) -> QTextDocument:
        doc = QTextDocument()
        doc.setDocumentMargin(0)
        doc.setDefaultFont(opt.font)
        option = QTextOption()
        option.setWrapMode(QTextOption.WrapMode.WrapAtWordBoundaryOrAnywhere)
        doc.setDefaultTextOption(option)
        color = self._colors["selected" if selected else "text"]
        doc.setDefaultStyleSheet(f"body {{ color: {color}; }}")
        doc.setHtml(f"<body>{text}</body>")
        doc.setTextWidth(width)
        return doc


class _SessionList(QListWidget):
    """The history list. Row heights depend on the viewport width, since the
    delegate wraps long titles, and the view caches its size hints — so a
    resize has to force a fresh layout or rows keep the height they were
    given at the previous width."""

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self.scheduleDelayedItemsLayout()


class LeftPanel(Panel):
    """History pane: previous Pi agent sessions recorded for the workspace
    folder. Emits `session_selected` with the session file path on click."""

    session_selected = Signal(str)
    new_session_requested = Signal()
    session_removed = Signal(str)  # a session's path was archived or deleted

    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        self._cwd = cwd
        # A surface, not a card: the panel paints its own background out to its
        # edges, so it reads as part of the window rather than as something
        # resting on it. The hairline separating it from the conversation is
        # the dock's divider, the same one between any two panels — drawing a
        # border here too would double it.
        self.setObjectName("sidebar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self._layout.setContentsMargins(12, 12, 12, 12)
        # The History pane uses the proportional Roboto face rather than the
        # app-wide mono. The delegate paints rows from the list's own font, so
        # setting an explicit pixel size here (not just in the stylesheet) is
        # what fixes the row size the delegate measures and paints with.
        sans = QFont(UI_SANS_FAMILY)
        sans.setPixelSize(13)
        self._new_button = QPushButton("＋  New Session")
        self._new_button.setFont(sans)
        self._new_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self._new_button.clicked.connect(self.new_session_requested)
        self._list = _SessionList()
        self._list.setFont(sans)
        self._list.setWordWrap(True)
        # Rows are HTML (see _SessionDelegate) so the title can be bold while
        # the subtitle line stays regular.
        self._delegate = _SessionDelegate(self._list)
        self._list.setItemDelegate(self._delegate)
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
        self.add_widget(self._new_button)
        self.add_widget(self._list, stretch=1)
        self.refresh()

    def refresh(self) -> None:
        """Rebuild the list from live in-flight sessions plus the ones Pi has
        written to disk (deduped by path)."""
        from kraken.agent.pi_sessions import sessions_for

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
            item = QListWidgetItem(_row_html(title, "running…"))
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
                _row_html(
                    session.title,
                    f"{session.started:%b %d, %H:%M}"
                    f" · {session.message_count} {noun}",
                )
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
        if item is None:
            return
        # Keep plain Python values across menu/dialog nested event loops. A
        # concurrent refresh can delete the underlying QListWidgetItem.
        path = item.data(Qt.ItemDataRole.UserRole)
        session_id = item.data(Qt.ItemDataRole.UserRole + 1)
        if not session_id:
            return
        menu = QMenu(self._list)
        archive_action = menu.addAction("Archive")
        delete_action = menu.addAction("Delete")
        chosen = menu.exec(self._list.viewport().mapToGlobal(pos))
        if chosen is archive_action:
            self._archive(path, session_id)
        elif chosen is delete_action:
            self._delete(path)

    def _archive(self, path: str, session_id: str) -> None:
        from kraken.agent.pi_sessions import archive_session

        archive_session(session_id)
        self.refresh()
        self.session_removed.emit(path)

    def _delete(self, path: str) -> None:
        from kraken.agent.pi_sessions import delete_session

        if (
            QMessageBox.question(
                self,
                "Delete session",
                "Permanently delete this session? This cannot be undone.",
            )
            != QMessageBox.StandardButton.Yes
        ):
            return
        delete_session(path)
        self.refresh()
        self.session_removed.emit(path)

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        self.setStyleSheet(f"#sidebar {{ background: {ui['sidebar']}; }}")
        self._list.setStyleSheet(_HISTORY_STYLES[name])
        self._new_button.setStyleSheet(_NEW_SESSION_STYLES[name])
        # The delegate paints the row text, so it needs the new colors too;
        # the stylesheet change alone would repaint only the backgrounds.
        self._delegate.set_theme(name)
        self._list.viewport().update()
