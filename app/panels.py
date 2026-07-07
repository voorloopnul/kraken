"""The three content panels. Replace the placeholder widgets with real content."""

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor
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
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from app.themes import DEFAULT_THEME, LIGHT, UI_COLORS

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
        self._card.add_widget(title)
        self._card.add_widget(self._new_button)
        self._card.add_widget(self._list, stretch=1)
        self.add_widget(self._card, stretch=1)
        self.refresh()

    def refresh(self) -> None:
        """Re-read the workspace's Pi sessions from disk."""
        from app.pi_sessions import sessions_for

        selected = self._selected_path()
        self._list.clear()
        sessions = sessions_for(self._cwd) if self._cwd else []
        if not sessions:
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
        if item is None or not item.data(Qt.ItemDataRole.UserRole):
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
        delete_session(item.data(Qt.ItemDataRole.UserRole))
        self.refresh()

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self._list.setStyleSheet(_HISTORY_STYLES[name])
        self._new_button.setStyleSheet(_NEW_SESSION_STYLES[name])


class CenterPanel(Panel):
    """Conversation pane: transcript, busy row (with Stop), and chat input."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        from app.chat_input import ChatInput
        from app.conversation import ConversationView

        self.conversation = ConversationView()
        self.add_widget(self.conversation, stretch=1)

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
        self.add_widget(busy_row)

        self.chat = ChatInput()
        self.add_widget(self.chat)

    def set_busy(self, busy: bool) -> None:
        self._busy_row.setVisible(busy)

    def set_theme(self, name: str) -> None:
        self.conversation.set_theme(name)
        self.chat.set_theme(name)
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

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        if self.browsers is not None:
            self.browsers.set_theme(name)


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
