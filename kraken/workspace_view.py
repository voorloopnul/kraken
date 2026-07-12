"""One workspace's set of panes — history (left), conversation (center), and
terminals (right) in a splitter. Unlike a single agent per workspace, this
runs one Pi session per `SessionController`: the focused session shows in the
center panel while others keep streaming in the background, so a running task
never blocks starting or opening another session. MainWindow keeps one
WorkspaceView per open workspace in a stack and switches between them."""

from __future__ import annotations

from PySide6.QtCore import QRectF, Qt, Signal
from PySide6.QtGui import QColor, QPainter
from PySide6.QtWidgets import QSplitter, QSplitterHandle, QVBoxLayout, QWidget

from kraken.panels import BrowserPanel, CenterPanel, GitPanel, LeftPanel, RightPanel
from kraken.session_controller import SessionController
from kraken.themes import DEFAULT_THEME, UI_COLORS


class _GripSplitter(QSplitter):
    """Splitter whose handles paint a short rounded grip bar; the stock
    dotted handle nearly vanishes against the light theme's background."""

    def __init__(self, orientation: Qt.Orientation, parent: QWidget | None = None):
        super().__init__(orientation, parent)
        self.grip_color = QColor("#c9c4b4")
        self.setHandleWidth(8)

    def set_grip_color(self, color: str) -> None:
        self.grip_color = QColor(color)
        for i in range(1, self.count()):
            self.handle(i).update()

    def createHandle(self) -> QSplitterHandle:
        return _GripHandle(self.orientation(), self)


class _GripHandle(QSplitterHandle):
    _LENGTH = 36.0
    _THICKNESS = 3.0

    def paintEvent(self, event) -> None:
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(self.splitter().grip_color)
        if self.orientation() == Qt.Orientation.Horizontal:
            rect = QRectF(
                (self.width() - self._THICKNESS) / 2,
                (self.height() - self._LENGTH) / 2,
                self._THICKNESS,
                self._LENGTH,
            )
        else:
            rect = QRectF(
                (self.width() - self._LENGTH) / 2,
                (self.height() - self._THICKNESS) / 2,
                self._LENGTH,
                self._THICKNESS,
            )
        painter.drawRoundedRect(rect, self._THICKNESS / 2, self._THICKNESS / 2)


class WorkspaceView(QWidget):
    # A transcript link asks MainWindow to show the browser panel.
    browser_requested = Signal()
    # Focused conversation's title, for the window title bar.
    title_changed = Signal(str)

    def __init__(self, path: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.path = path
        self._theme_name = DEFAULT_THEME

        self.left_panel = LeftPanel(cwd=path)
        self.left_panel.setMinimumWidth(280)
        self.left_panel.setMaximumWidth(380)
        self.center_panel = CenterPanel()
        # Per-workspace browser, like the terminals; it's lazily populated
        # on first show, so hidden panels cost no Chromium processes.
        self.browser_panel = BrowserPanel()
        self.git_panel = GitPanel(cwd=path)
        self.git_panel.setMinimumWidth(320)
        self.right_panel = RightPanel(cwd=path)
        # The terminal panel never shrinks below a usable width; the splitter
        # stops the drag here rather than letting it collapse to a sliver.
        self.right_panel.setMinimumWidth(380)

        # Terminals and git history share the rightmost column, terminals on
        # top; the column drops out entirely when both are toggled off (see
        # set_panel_visible).
        self._right_column = _GripSplitter(Qt.Orientation.Vertical)
        self._right_column.addWidget(self.right_panel)
        self._right_column.addWidget(self.git_panel)
        self._right_column.setChildrenCollapsible(False)
        self._right_column.setSizes([420, 300])

        splitter = _GripSplitter(Qt.Orientation.Horizontal)
        splitter.addWidget(self.left_panel)
        splitter.addWidget(self.center_panel)
        splitter.addWidget(self.browser_panel)
        splitter.addWidget(self._right_column)

        # Center panel absorbs extra space when the window resizes.
        splitter.setStretchFactor(0, 0)
        splitter.setStretchFactor(1, 1)
        splitter.setStretchFactor(2, 0)
        splitter.setStretchFactor(3, 0)
        splitter.setSizes([300, 500, 480, 480])
        splitter.setChildrenCollapsible(False)
        self._splitter = splitter

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(splitter)

        # Live sessions (one agent + transcript each). Agents start lazily on
        # the first prompt, so opening a workspace stays instant. `unseen`
        # holds sessions that finished in the background and haven't been
        # opened since.
        self.controllers: list[SessionController] = []
        self.focused: SessionController | None = None
        self.unseen: set[str] = set()

        self.center_panel.chat.submitted.connect(self._on_prompt)
        self.center_panel.chat.model_menu_requested.connect(self._on_model_menu)
        self.center_panel.chat.model_selected.connect(self._on_model_selected)
        self.center_panel.stop_button.clicked.connect(self._on_stop)
        self.left_panel.session_selected.connect(self._load_session)
        self.left_panel.new_session_requested.connect(self._new_session)
        self.left_panel.session_removed.connect(self._on_session_removed)

        # Start on a fresh, empty session ready to type in.
        self._focus(self._new_controller())

    # ---- Session pool ----------------------------------------------------

    def _new_controller(self, session_path: str | None = None) -> SessionController:
        controller = SessionController(
            self.path, self._theme_name, session_path=session_path, parent=self
        )
        controller.streaming_changed.connect(
            lambda streaming, c=controller: self._on_streaming_changed(c, streaming)
        )
        controller.path_known.connect(
            lambda p, c=controller: self._on_path_known(c, p)
        )
        controller.model_known.connect(
            lambda name, c=controller: self._on_model_known(c, name)
        )
        # A loaded session learns its title asynchronously; resyncing
        # refreshes the History row and the window title bar.
        controller.title_known.connect(lambda _title: self._sync_history())
        self.controllers.append(controller)
        controller.conversation.link_clicked.connect(self._open_link)
        self.center_panel.add_conversation(controller.conversation)
        return controller

    def _open_link(self, url: str) -> None:
        self.browser_requested.emit()
        self.browser_panel.open_url(url)

    def _controller_key(self, controller: SessionController) -> str:
        """A stable history key for a live session: its file path once Pi has
        assigned one, else a temporary id so it's still listed and clickable."""
        return controller.session_path or f"live:{id(controller)}"

    def _controller_for_key(self, key: str) -> SessionController | None:
        for controller in self.controllers:
            if self._controller_key(controller) == key:
                return controller
        return None

    def _focus(self, controller: SessionController) -> None:
        previous = self.focused
        self.focused = controller
        self.center_panel.set_focused_conversation(controller.conversation)
        self.center_panel.set_busy(controller.is_streaming)
        self.center_panel.chat.set_model_label(controller.model_label)
        # Retire the session we're leaving unless it's still streaming (then it
        # stays live in the background so you can flip back and watch it).
        if previous is not None and previous is not controller and not previous.is_streaming:
            self._retire(previous)
        self._sync_history()

    def _retire(self, controller: SessionController) -> None:
        controller.stop()
        self.center_panel.remove_conversation(controller.conversation)
        controller.conversation.deleteLater()
        if controller in self.controllers:
            self.controllers.remove(controller)
        controller.deleteLater()

    def _sync_history(self) -> None:
        """Push the current live sessions and their running/unseen state to the
        History panel and rebuild it."""
        entries = [
            (self._controller_key(c), c.title, c.session_path, c.is_streaming)
            for c in self.controllers
            if c.first_prompt is not None
        ]
        self.left_panel.set_live_sessions(entries)
        running = {
            self._controller_key(c) for c in self.controllers if c.is_streaming
        }
        self.left_panel.set_session_status(running, self.unseen)
        self.title_changed.emit(
            self.focused.title if self.focused is not None else ""
        )

    # ---- Chat / controls -------------------------------------------------

    def _on_prompt(
        self, text: str, images: list | None = None, files: list | None = None
    ) -> None:
        if self.focused is None:
            self._focus(self._new_controller())
        self.focused.prompt(text, images, files)
        # Surface the just-started session in History right away, so it's
        # reachable even before Pi writes its file to disk.
        self._sync_history()

    def _on_stop(self) -> None:
        if self.focused is not None:
            self.focused.agent.abort()

    def _on_model_menu(self) -> None:
        if self.focused is None:
            return
        controller = self.focused

        def show(models: list, provider: str | None, model_id: str | None) -> None:
            # The fetch is async; only act if this session is still on screen.
            if controller is not self.focused:
                return
            if models:
                self.center_panel.chat.show_model_menu(models, provider, model_id)
            else:
                # Empty means the fetch failed or the agent has no models
                # configured; say so instead of leaving a dead button.
                self.center_panel.chat.notify_model("No models available")

        controller.request_models(show)

    def _on_model_selected(self, provider: str, model_id: str) -> None:
        if self.focused is not None:
            self.focused.set_model(provider, model_id)

    def _new_session(self) -> None:
        # Reuse the current session if it's already a fresh, unused one.
        if (
            self.focused is not None
            and not self.focused.is_streaming
            and self.focused.session_path is None
            and self.focused.conversation.is_empty
        ):
            self.left_panel.clear_selection()
            return
        self._focus(self._new_controller())
        self.left_panel.clear_selection()

    def _on_session_removed(self, path: str) -> None:
        # A session was archived or deleted from History. Drop its live agent
        # if it has one; if it was the focused session, its transcript is now
        # stale, so replace it with a fresh, empty session.
        self.unseen.discard(path)
        controller = self._controller_for_key(path)
        if controller is not None:
            was_focused = controller is self.focused
            self._retire(controller)
            if was_focused:
                self.focused = None
                self._focus(self._new_controller())
        self._sync_history()

    def _load_session(self, key: str) -> None:
        # Opening a session clears its "finished, unread" badge.
        self.unseen.discard(key)
        # A live session (running or in-flight, maybe not yet on disk) is keyed
        # by path-or-temp-id; jump straight to it without touching its agent.
        live = self._controller_for_key(key)
        if live is not None:
            self._focus(live)
            return
        # Otherwise it's a persisted session on disk: bind a fresh agent to it.
        controller = self._new_controller(session_path=key)
        self._focus(controller)
        controller.load()

    # ---- Controller signals ----------------------------------------------

    def _on_streaming_changed(self, controller: SessionController, streaming: bool) -> None:
        if controller is self.focused:
            self.center_panel.set_busy(streaming)
        if not streaming:
            # A turn finished; its messages are now on disk.
            if controller is not self.focused:
                # Background session completed: flag it unread, then retire it.
                if controller.session_path:
                    self.unseen.add(controller.session_path)
                self._retire(controller)
        self._sync_history()

    def _on_path_known(self, controller: SessionController, path: str) -> None:
        # A brand-new session's file path just became known; re-key its History
        # row (and show its persisted row once Pi writes the file).
        self._sync_history()

    def _on_model_known(self, controller: SessionController, name: str) -> None:
        if controller is self.focused:
            self.center_panel.chat.set_model_label(name)

    # ---- Panels ------------------------------------------------------------

    def set_panel_visible(self, side: str, visible: bool) -> None:
        panel = {
            "left": self.left_panel,
            "browser": self.browser_panel,
            "git": self.git_panel,
            "right": self.right_panel,
        }[side]
        panel.setVisible(visible)
        # The right column only earns splitter space while it has a visible
        # pane; a hidden QSplitter child gives up its slot, but the column
        # widget itself would still claim one.
        self._right_column.setVisible(
            not self.right_panel.isHidden() or not self.git_panel.isHidden()
        )

    # ---- Theme / lifecycle -----------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        # Styling the splitter covers the view background and cascades text
        # color to every panel label.
        self._splitter.setStyleSheet(
            f"QSplitter {{ background: {ui['window']}; }}"
            f" QLabel {{ color: {ui['text']}; }}"
        )
        # Same handle colors as the conversation scrollbar, per theme.
        grip = "#3a3d45" if name == "dark" else "#c9c4b4"
        self._splitter.set_grip_color(grip)
        self._right_column.set_grip_color(grip)
        self.left_panel.set_theme(name)
        self.center_panel.set_theme(name)
        self.browser_panel.set_theme(name)
        self.git_panel.set_theme(name)
        self.right_panel.set_theme(name)
        for controller in self.controllers:
            controller.set_theme(name)

    def shutdown(self) -> None:
        for controller in self.controllers:
            controller.stop()
        self.right_panel.terminals.shutdown_all()
