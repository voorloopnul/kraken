"""One live Pi session: its agent process plus the transcript widget showing
it. A workspace keeps several of these at once (one per session in flight), so
all per-session state — the agent, the conversation view, the tool-call block
map, the model label — lives here rather than on the workspace.

The controller translates raw agent events into transcript edits and re-emits
just the cross-cutting facts the workspace needs to coordinate the rest of the
UI: when streaming starts/stops (busy row, history "running" badge, retirement)
and when the session's file path and model become known.
"""

from __future__ import annotations

from PySide6.QtCore import QObject, Signal

from app.conversation import (
    ConversationView,
    _content_text,
    args_detail,
    args_summary,
    error_summary,
)
from app.pi_rpc import PiAgent


class SessionController(QObject):
    streaming_changed = Signal(bool)  # agent started (True) / finished (False)
    path_known = Signal(str)  # the session's on-disk file path, once discovered
    model_known = Signal(str)  # the model display name, once discovered
    title_known = Signal(str)  # the title, once a loaded session's messages arrive

    def __init__(
        self,
        cwd: str,
        theme_name: str,
        session_path: str | None = None,
        parent: QObject | None = None,
    ):
        super().__init__(parent)
        self.session_path = session_path
        self.model_name: str | None = None
        self.first_prompt: str | None = None  # first user message; the title
        self.conversation = ConversationView()
        self.conversation.set_theme(theme_name)
        self.agent = PiAgent(cwd, self, session_path=session_path)
        self._tool_blocks: dict[str, int] = {}  # toolCallId -> transcript block
        self._model_emitted = False

        self.agent.event.connect(self._on_event)
        self.agent.notify.connect(self._on_notify)
        self.agent.failed.connect(self._on_failed)

    # ---- State ----------------------------------------------------------

    @property
    def is_streaming(self) -> bool:
        return self.agent.is_streaming

    @property
    def running(self) -> bool:
        return self.agent.running

    @property
    def title(self) -> str:
        """History label for a still-live session, from its first prompt."""
        text = " ".join((self.first_prompt or "").split())
        if not text:
            return "New session"
        return text[:59] + "…" if len(text) > 60 else text

    def set_theme(self, name: str) -> None:
        self.conversation.set_theme(name)

    def stop(self) -> None:
        """Retire the session: kill its agent process. The transcript widget
        stays valid but stops updating; the workspace drops it separately."""
        self.agent.stop()

    # ---- Chat -> agent --------------------------------------------------

    def prompt(self, text: str, images: list | None = None) -> None:
        if self.first_prompt is None:
            self.first_prompt = text or "(image)"
        self.conversation.add_user(text or "(image)")
        if images:
            noun = "image" if len(images) == 1 else "images"
            self.conversation.add_info(f"({len(images)} {noun} attached)")
        was_streaming = self.agent.is_streaming
        self.agent.prompt(text, images=images, callback=self._on_prompt_response)
        if was_streaming:
            self.conversation.add_info("(queued: delivered after the current turn)")
        self._sync_state()

    def _on_prompt_response(self, response: dict) -> None:
        if not response.get("success"):
            self.conversation.add_info(
                f"Prompt rejected: {response.get('error', 'unknown error')}",
                error=True,
            )

    def load(self) -> None:
        """Render the bound session's existing messages into the transcript."""

        def on_messages(response: dict) -> None:
            if response.get("success"):
                messages = (response.get("data") or {}).get("messages") or []
                self.conversation.render_messages(messages)
                # A loaded session's title comes from its stored messages,
                # the same way History derives it (first user message).
                if self.first_prompt is None:
                    for message in messages:
                        if message.get("role") == "user":
                            text = _content_text(message.get("content"))
                            if text:
                                self.first_prompt = text
                                self.title_known.emit(self.title)
                                break

        self.agent.get_messages(on_messages)
        self._sync_state()

    def _sync_state(self) -> None:
        """Learn the session file path and model name from the live agent."""

        def on_state(response: dict) -> None:
            data = response.get("data") or {}
            session_file = data.get("sessionFile")
            if session_file and session_file != self.session_path:
                self.session_path = session_file
                self.path_known.emit(session_file)
            model = data.get("model") or {}
            if model.get("name") and not self._model_emitted:
                self._model_emitted = True
                self.model_name = model["name"]
                self.model_known.emit(model["name"])

        self.agent.get_state(on_state)

    # ---- Agent events -> conversation -----------------------------------

    def _on_event(self, event: dict) -> None:
        conversation = self.conversation
        kind = event.get("type")
        if kind == "agent_start":
            self.streaming_changed.emit(True)
        elif kind == "agent_end":
            # A request the provider rejected outright (e.g. HTTP 400) ends
            # the run with an errored assistant message but streams no
            # message_update events — without this the turn fails silently.
            for message in event.get("messages") or []:
                error = error_summary(message)
                if error:
                    conversation.add_info(f"Turn failed: {error}", error=True)
            self.streaming_changed.emit(False)
        elif kind == "message_update":
            delta = event.get("assistantMessageEvent") or {}
            if delta.get("type") == "text_delta":
                conversation.append_assistant_delta(delta.get("delta", ""))
            elif delta.get("type") == "error":
                reason = delta.get("reason", "error")
                conversation.add_info(f"({reason})", error=reason != "aborted")
        elif kind == "tool_execution_start":
            index = conversation.add_tool(
                event.get("toolName", "?"),
                args_summary(event.get("args")),
                detail=args_detail(event.get("args")),
            )
            if event.get("toolCallId"):
                self._tool_blocks[event["toolCallId"]] = index
        elif kind == "tool_execution_end":
            index = self._tool_blocks.pop(event.get("toolCallId"), None)
            result = event.get("result") or {}
            text = "\n".join(
                part.get("text", "")
                for part in result.get("content") or []
                if isinstance(part, dict) and part.get("type") == "text"
            ).strip()
            if index is not None and text:
                if event.get("isError"):
                    text = f"(error)\n{text}"
                conversation.append_tool_detail(index, text)

    def _on_notify(self, message: str, notify_type: str) -> None:
        self.conversation.add_info(message, error=notify_type == "error")

    def _on_failed(self, error: str) -> None:
        self.conversation.add_info(f"Pi agent unavailable: {error}", error=True)
        self.streaming_changed.emit(False)
