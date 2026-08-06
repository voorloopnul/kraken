"""One workspace's set of panes — history (left), conversation (center), and
terminals (right) in a splitter. Unlike a single agent per workspace, this
runs one Pi session per `SessionController`: the focused session shows in the
center panel while others keep streaming in the background, so a running task
never blocks starting or opening another session. MainWindow keeps one
WorkspaceView per open workspace in a stack and switches between them."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import QVBoxLayout, QWidget

from kraken import debug
from kraken.shell.dock import DockArea, DockPanel
from kraken.shell.panels import (
    BrowserPanel,
    CenterPanel,
    DiffPanel,
    GitPanel,
    LeftPanel,
    RightPanel,
)
from kraken.agent.session_controller import SessionController
from kraken.ui.themes import DEFAULT_THEME

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget


class WorkspaceView(QWidget):
    # A transcript link asks MainWindow to show the browser panel.
    browser_requested = Signal()
    # The last browser tab was closed; MainWindow hides the panel.
    browser_closed = Signal()
    # Focused conversation's title, for the window title bar.
    title_changed = Signal(str)
    # True while any of this workspace's sessions has an agent streaming, so the
    # workspace-bar button can blink its activity dot (even in the background).
    activity_changed = Signal(bool)

    def __init__(
        self,
        path: str,
        parent: QWidget | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self.path = path
        self._theme_name = DEFAULT_THEME
        # When set, this workspace lives on a remote host: `path` is the local
        # anchor folder (where pi runs and stores sessions), while the agent,
        # terminal, and the git and diff panels all operate on the remote over
        # SSH.
        self.remote = remote

        self.left_panel = LeftPanel(cwd=path)
        # Only a floor: the History panel grows to fill its column so its
        # resize grip stays flush with the panel edge rather than detaching
        # once the content hits a maximum.
        self.left_panel.setMinimumWidth(280)
        self.center_panel = CenterPanel()
        # Per-workspace browser, like the terminals; it's lazily populated
        # on first show, so hidden panels cost no Chromium processes.
        self.browser_panel = BrowserPanel()
        self.browser_panel.emptied.connect(self.browser_closed)
        self.diff_panel = DiffPanel(cwd=path, remote=remote)
        self.diff_panel.setMinimumWidth(300)
        self.git_panel = GitPanel(cwd=path, remote=remote)
        self.git_panel.setMinimumWidth(320)
        self.right_panel = RightPanel(cwd=path, remote=remote)
        # The terminal panel never shrinks below a usable width; the splitter
        # stops the drag here rather than letting it collapse to a sliver.
        self.right_panel.setMinimumWidth(380)

        # Every panel lives in the dock, wrapped in a draggable header. The
        # dock arranges them into columns (up to two panels stacked per column)
        # and lets the user drag a panel's header to re-column or stack it.
        self._dock = DockArea(
            order=["left", "center", "browser", "diff", "git", "right"],
            stretch_key="center",
            fixed_keys={"left"},
            no_stack_keys={"center"},
        )
        # History is a fixed anchor on the far left: not draggable, and nothing
        # may stack with it or open a column to its left. The conversation is
        # also un-draggable and refuses stacking, but columns may still open on
        # either side of it.
        anchors = {"left", "center"}
        for key, content in (
            ("left", self.left_panel),
            ("center", self.center_panel),
            ("browser", self.browser_panel),
            ("diff", self.diff_panel),
            ("git", self.git_panel),
            ("right", self.right_panel),
        ):
            panel = DockPanel(key, content, draggable=key not in anchors)
            # The git and diff panels' refresh buttons ride in the grip row
            # rather than a row of their own inside the card.
            if key in ("diff", "git"):
                panel.header.add_trailing(content.refresh_button)
            self._dock.register(panel)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self._dock)

        # Panel visibility is per-workspace, not global: a new workspace opens
        # with History and the conversation showing; the terminal, browser, diff,
        # and git panes stay closed (and their lazy shell/Chromium processes
        # unspun) until toggled open here, independently of other workspaces.
        self._panel_visible = {
            "left": True,
            "browser": False,
            "diff": False,
            "git": False,
            "right": False,
        }
        self._dock.set_layout([["left"], ["center"]])

        # Live sessions (one agent + transcript each). Agents start lazily on
        # the first prompt, so opening a workspace stays instant. `unseen`
        # holds sessions that finished in the background and haven't been
        # opened since.
        self.controllers: list[SessionController] = []
        self.focused: SessionController | None = None
        self.unseen: set[str] = set()
        # Last emitted activity state (any session streaming), to fire
        # activity_changed only on transitions.
        self._active = False

        self.center_panel.chat.submitted.connect(self._on_prompt)
        self.center_panel.chat.model_menu_requested.connect(self._on_model_menu)
        self.center_panel.chat.model_selected.connect(self._on_model_selected)
        self.center_panel.chat.effort_menu_requested.connect(self._on_effort_menu)
        self.center_panel.chat.effort_selected.connect(self._on_effort_selected)
        self.center_panel.stop_button.clicked.connect(self._on_stop)
        self.left_panel.session_selected.connect(self._load_session)
        self.left_panel.new_session_requested.connect(self._new_session)
        self.left_panel.session_removed.connect(self._on_session_removed)

        # Start on a fresh, empty session ready to type in.
        self._focus(self._new_controller())

    # ---- Session pool ----------------------------------------------------

    def _new_controller(self, session_path: str | None = None) -> SessionController:
        controller = SessionController(
            self.path,
            self._theme_name,
            session_path=session_path,
            parent=self,
            remote=self.remote,
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
        controller.thinking_known.connect(
            lambda _level, c=controller: self._on_thinking_known(c)
        )
        # A loaded session learns its title asynchronously; resyncing
        # refreshes the History row and the window title bar.
        controller.title_known.connect(lambda _title: self._sync_history())
        self.controllers.append(controller)
        controller.conversation.link_clicked.connect(self._open_link)
        self.center_panel.add_conversation(controller.conversation)
        debug.log(
            "session.create",
            path=session_path or "(new)",
            live=len(self.controllers),
        )
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
        self.center_panel.set_busy(controller.is_streaming, controller.streaming_since)
        self.center_panel.chat.set_model_label(controller.model_label)
        self._apply_effort(controller)
        # Retire the session we're leaving unless it's still streaming (then it
        # stays live in the background so you can flip back and watch it).
        if previous is not None and previous is not controller and not previous.is_streaming:
            self._retire(previous)
        self._sync_history()

    def _retire(self, controller: SessionController) -> None:
        # Retirement kills a pi process and deletes a transcript widget; if the
        # tree RSS on this line never comes back down, that is the leak.
        debug.proc(
            "session.retire",
            path=controller.session_path or "(unsaved)",
            live=len(self.controllers) - 1,
        )
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
        active = bool(running)
        if active != self._active:
            self._active = active
            self.activity_changed.emit(active)
        self.title_changed.emit(
            self.focused.title if self.focused is not None else ""
        )

    # ---- Chat / controls -------------------------------------------------

    def _on_prompt(
        self, text: str, images: list | None = None, files: list | None = None
    ) -> None:
        if self.focused is None:
            self._focus(self._new_controller())
        # Logged once the controller exists, so the turn can name its model.
        # This is the agent's own reported model (folded in from get_state),
        # not the picker's idea of it — the two disagreeing is exactly the
        # thing worth being able to see, so logging the picker would defeat
        # the purpose. "(pending)" until the first get_state comes back.
        debug.action(
            "chat.submit",
            chars=len(text),
            images=len(images or []),
            files=len(files or []),
            provider=self.focused.model_provider or "?",
            model=self.focused.model_id or "(pending)",
        )
        self.focused.prompt(text, images, files)
        # Surface the just-started session in History right away, so it's
        # reachable even before Pi writes its file to disk.
        self._sync_history()

    def _on_stop(self) -> None:
        debug.action("chat.stop")
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
        debug.action("model.select", provider=provider, model=model_id)
        if self.focused is not None:
            self.focused.set_model(provider, model_id)

    def _on_effort_menu(self) -> None:
        if self.focused is None:
            return
        controller = self.focused

        def show(levels: list, current: str | None) -> None:
            # The fetch is async; only act if this session is still on screen.
            if controller is not self.focused:
                return
            if levels:
                self.center_panel.chat.show_effort_menu(levels, current)
            else:
                self.center_panel.chat.notify_effort(
                    "This model has no reasoning levels"
                )

        controller.request_thinking(show)

    def _on_effort_selected(self, level: str) -> None:
        debug.action("effort.select", level=level)
        if self.focused is not None:
            self.focused.set_thinking_level(level)

    def _apply_effort(self, controller: SessionController) -> None:
        # Hide the selector only once we've confirmed the model has no levels
        # (empty list); an unknown (None) state still shows the placeholder.
        supported = controller.thinking_levels != []
        self.center_panel.chat.set_effort_label(controller.thinking_level, supported)

    def _on_thinking_known(self, controller: SessionController) -> None:
        if controller is self.focused:
            self._apply_effort(controller)

    def _new_session(self) -> None:
        debug.action("session.new")
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
        debug.action("session.remove", path=path)
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
        debug.action("session.open", key=key)
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
        # A turn boundary is the natural sampling point for growth: the memory
        # snapshot here shows what a whole agent turn cost.
        debug.proc(
            "session.streaming",
            streaming=streaming,
            focused=controller is self.focused,
            model=controller.model_id or "(pending)",
            path=controller.session_path or "(unsaved)",
        )
        if controller is self.focused:
            self.center_panel.set_busy(streaming, controller.streaming_since)
        if not streaming:
            # A turn finished; its messages are now on disk, and whatever the
            # agent edited is on disk too — so the diff pane is stale.
            if self.diff_panel.isVisible():
                self.diff_panel.refresh()
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

    def is_panel_visible(self, side: str) -> bool:
        return self._panel_visible[side]

    def set_panel_visible(self, side: str, visible: bool) -> None:
        self._panel_visible[side] = visible
        if visible:
            self._dock.show_panel(side)
        else:
            self._dock.hide_panel(side)

    # ---- Theme / lifecycle -----------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        # The dock paints the view background, colors the splitter grips, and
        # themes each panel's drag header.
        self._dock.set_theme(name)
        self.left_panel.set_theme(name)
        self.center_panel.set_theme(name)
        self.browser_panel.set_theme(name)
        self.diff_panel.set_theme(name)
        self.git_panel.set_theme(name)
        self.right_panel.set_theme(name)
        for controller in self.controllers:
            controller.set_theme(name)

    def shutdown(self) -> None:
        # Reap every agent process even if one teardown misbehaves: a raised
        # exception here must not skip the remaining controllers (or, up in
        # MainWindow, the remaining workspaces) and leave a live `pi` behind.
        debug.log("view.shutdown", path=self.path, sessions=len(self.controllers))
        for controller in self.controllers:
            try:
                controller.stop()
            except Exception as exc:
                debug.exception("controller.stop failed", exc)
        try:
            self.right_panel.shutdown()
        except Exception as exc:
            debug.exception("right_panel.shutdown failed", exc)
