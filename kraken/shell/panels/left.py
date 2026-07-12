"""History pane: previous Pi agent sessions for the workspace folder."""

from PySide6.QtCore import Qt, QSize, Signal
from PySide6.QtGui import QIcon
from PySide6.QtWidgets import (
    QLabel,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QWidget,
)

from kraken.shell.panels.base import Card, Panel, _dot_icon
from kraken.ui.themes import UI_COLORS

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
        self._card.set_colors(ui["card"], ui["card_border"])
        self._list.setStyleSheet(_HISTORY_STYLES[name])
        self._new_button.setStyleSheet(_NEW_SESSION_STYLES[name])
