"""One workspace's set of panes — history (left), conversation (center), and
terminals (right) in a splitter — plus its own Pi agent (RPC subprocess
running in the workspace folder). MainWindow keeps one WorkspaceView per open
workspace in a stack and switches between them, so each workspace keeps its
own pane state, agent conversation, and running shells."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QSplitter, QVBoxLayout, QWidget

from app.panels import BrowserPanel, CenterPanel, LeftPanel, RightPanel
from app.pi_rpc import PiAgent
from app.themes import UI_COLORS


class WorkspaceView(QWidget):
    def __init__(self, path: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.path = path

        self.left_panel = LeftPanel(cwd=path)
        self.center_panel = CenterPanel()
        # Per-workspace browser, like the terminals; it's lazily populated
        # on first show, so hidden panels cost no Chromium processes.
        self.browser_panel = BrowserPanel()
        self.right_panel = RightPanel(cwd=path)

        splitter = QSplitter(Qt.Orientation.Horizontal)
        splitter.addWidget(self.left_panel)
        splitter.addWidget(self.center_panel)
        splitter.addWidget(self.browser_panel)
        splitter.addWidget(self.right_panel)

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

        # The agent process starts lazily on the first prompt or history
        # click, so opening a workspace stays instant.
        self.agent = PiAgent(path, self)
        self._model_label_set = False
        self.agent.event.connect(self._on_agent_event)
        self.agent.notify.connect(self._on_agent_notify)
        self.agent.failed.connect(self._on_agent_failed)
        self.center_panel.chat.submitted.connect(self._on_prompt)
        self.center_panel.stop_button.clicked.connect(self.agent.abort)
        self.left_panel.session_selected.connect(self._load_session)
        self.left_panel.new_session_requested.connect(self._new_session)

    # ---- Chat -> agent ---------------------------------------------------

    def _on_prompt(self, text: str) -> None:
        conversation = self.center_panel.conversation
        conversation.add_user(text)
        was_streaming = self.agent.is_streaming
        self.agent.prompt(text, self._on_prompt_response)
        if was_streaming:
            conversation.add_info("(queued: delivered after the current turn)")
        self._sync_model_label()

    def _on_prompt_response(self, response: dict) -> None:
        if not response.get("success"):
            self.center_panel.conversation.add_info(
                f"Prompt rejected: {response.get('error', 'unknown error')}", error=True
            )

    def _sync_model_label(self) -> None:
        if self._model_label_set:
            return
        self._model_label_set = True

        def on_state(response: dict) -> None:
            model = (response.get("data") or {}).get("model") or {}
            if model.get("name"):
                self.center_panel.chat.set_model_label(model["name"])

        self.agent.get_state(on_state)

    # ---- Agent events -> conversation -------------------------------------

    def _on_agent_event(self, event: dict) -> None:
        conversation = self.center_panel.conversation
        kind = event.get("type")
        if kind == "agent_start":
            self.center_panel.set_busy(True)
        elif kind == "agent_end":
            self.center_panel.set_busy(False)
            # The finished run was appended to the workspace's session file.
            self.left_panel.refresh()
        elif kind == "message_update":
            delta = event.get("assistantMessageEvent") or {}
            if delta.get("type") == "text_delta":
                conversation.append_assistant_delta(delta.get("delta", ""))
            elif delta.get("type") == "error":
                reason = delta.get("reason", "error")
                conversation.add_info(f"({reason})", error=reason != "aborted")
        elif kind == "tool_execution_start":
            from app.conversation import args_summary

            conversation.add_tool(
                event.get("toolName", "?"), args_summary(event.get("args"))
            )

    def _on_agent_notify(self, message: str, notify_type: str) -> None:
        self.center_panel.conversation.add_info(message, error=notify_type == "error")

    def _on_agent_failed(self, error: str) -> None:
        self.center_panel.set_busy(False)
        self.center_panel.conversation.add_info(
            f"Pi agent unavailable: {error}", error=True
        )

    # ---- History -> agent --------------------------------------------------

    def _new_session(self) -> None:
        conversation = self.center_panel.conversation
        if self.agent.is_streaming:
            conversation.add_info("Agent is busy — stop it before starting a new session.")
            return
        # No agent process yet means the transcript is already a fresh
        # session; just make sure it's empty.
        if not self.agent.running:
            conversation.clear_conversation()
            self.left_panel.clear_selection()
            return

        def on_new(response: dict) -> None:
            if not response.get("success"):
                conversation.add_info(
                    f"Could not start a new session: {response.get('error', 'unknown error')}",
                    error=True,
                )
                return
            if (response.get("data") or {}).get("cancelled"):
                conversation.add_info("New session cancelled by an extension.")
                return
            conversation.clear_conversation()
            self.left_panel.clear_selection()
            self.left_panel.refresh()

        self.agent.new_session(on_new)

    def _load_session(self, session_path: str) -> None:
        conversation = self.center_panel.conversation
        if self.agent.is_streaming:
            conversation.add_info("Agent is busy — stop it before switching sessions.")
            return

        def on_messages(response: dict) -> None:
            if response.get("success"):
                messages = (response.get("data") or {}).get("messages") or []
                conversation.render_messages(messages)

        def on_switched(response: dict) -> None:
            if not response.get("success"):
                conversation.add_info(
                    f"Could not load session: {response.get('error', 'unknown error')}",
                    error=True,
                )
                return
            if (response.get("data") or {}).get("cancelled"):
                conversation.add_info("Session switch cancelled by an extension.")
                return
            self.agent.get_messages(on_messages)

        self.agent.switch_session(session_path, on_switched)
        self._sync_model_label()

    # ---- Theme / lifecycle ---------------------------------------------------

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        # Styling the splitter covers the view background and cascades text
        # color to every panel label.
        self._splitter.setStyleSheet(
            f"QSplitter {{ background: {ui['window']}; }}"
            f" QLabel {{ color: {ui['text']}; }}"
        )
        self.left_panel.set_theme(name)
        self.center_panel.set_theme(name)
        self.browser_panel.set_theme(name)
        self.right_panel.set_theme(name)

    def shutdown(self) -> None:
        self.agent.stop()
        self.right_panel.terminals.shutdown_all()
