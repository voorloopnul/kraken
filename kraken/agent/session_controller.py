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

from pathlib import Path
from typing import TYPE_CHECKING

from PySide6.QtCore import QObject, Signal

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

from kraken.chat.conversation import ConversationView
from kraken.chat.formatting import (
    _content_text,
    args_detail,
    args_summary,
    error_summary,
)
from kraken.agent.pi_rpc import PiAgent


def _files_title(files: list[str]) -> str:
    """Short session title for a message that carries only file attachments:
    the first file's name, plus a count of the rest."""
    first = Path(files[0]).name or files[0]
    extra = len(files) - 1
    if extra:
        return f"{first} (+{extra} file{'s' if extra > 1 else ''})"
    return first


class SessionController(QObject):
    streaming_changed = Signal(bool)  # agent started (True) / finished (False)
    path_known = Signal(str)  # the session's on-disk file path, once discovered
    model_known = Signal(str)  # the footer label, whenever the model resolves
    title_known = Signal(str)  # the title, once a loaded session's messages arrive

    def __init__(
        self,
        cwd: str,
        theme_name: str,
        session_path: str | None = None,
        parent: QObject | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self.session_path = session_path
        self.model_name: str | None = None
        self.model_provider: str | None = None
        self.model_id: str | None = None
        self._last_model_label: str | None = None  # last label emitted
        self.first_prompt: str | None = None  # first user message; the title
        self.conversation = ConversationView()
        self.conversation.set_theme(theme_name)
        self.agent = PiAgent(cwd, self, session_path=session_path, remote=remote)
        self._tool_blocks: dict[str, int] = {}  # toolCallId -> transcript block
        # An error was already surfaced during the current turn (reset each
        # agent_start); keeps agent_end from reprinting the same failure that
        # streamed in as a message_update.
        self._turn_had_error = False
        # Identities of errored messages already reported, so a replayed
        # agent_end payload can't re-announce an earlier turn's failure.
        self._reported_errors: set[str] = set()

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

    def prompt(
        self,
        text: str,
        images: list | None = None,
        files: list | None = None,
    ) -> None:
        images = images or []
        files = files or []

        # Wire message: pi's prompt carries images structurally but has no file
        # channel, so file paths ride along in the text. Providers reject an
        # empty message, so an image-only turn still needs a carrier line.
        message = text
        if files:
            refs = "\n".join(f"- {path}" for path in files)
            message = (f"{message}\n\n" if message else "") + f"Attached files:\n{refs}"
        if not message and images:
            message = "(see attached image)"

        # Title and transcript stay the user's intent, never the path blob.
        display = text or (_files_title(files) if files else "(image)")
        if self.first_prompt is None:
            self.first_prompt = display
        self.conversation.add_user(display)
        for noun, items in (("image", images), ("file", files)):
            if items:
                plural = noun if len(items) == 1 else f"{noun}s"
                self.conversation.add_info(f"({len(items)} {plural} attached)")
        was_streaming = self.agent.is_streaming
        self.agent.prompt(message, images=images, callback=self._on_prompt_response)
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
            self._set_current_model(data.get("model") or {})

        self.agent.get_state(on_state)

    @property
    def model_label(self) -> str | None:
        """Footer text for the current model: the friendly name if known, else
        the raw id (still concrete), else None once nothing is known yet."""
        return self.model_name or self.model_id

    def _set_current_model(self, model: dict) -> None:
        self.model_provider = model.get("provider") or self.model_provider
        self.model_id = model.get("id") or self.model_id
        if model.get("name"):
            self.model_name = model["name"]
        # Emit whenever the effective label changes, so a session that only
        # knows its id (name not yet resolved) still shows a concrete model
        # instead of staying on the "Model" placeholder.
        label = self.model_label
        if label and label != self._last_model_label:
            self._last_model_label = label
            self.model_known.emit(label)

    # ---- Model selection --------------------------------------------------

    def request_models(self, callback) -> None:
        """Fetch the agent's configured models (Pi's /model list) plus the
        current selection; `callback(models, provider, model_id)`."""
        # Refresh the current selection first; the agent answers commands in
        # order, so it lands before the model list does.
        self._sync_state()

        def on_models(response: dict) -> None:
            models = (response.get("data") or {}).get("models") or []
            callback(models, self.model_provider, self.model_id)

        self.agent.get_available_models(on_models)

    def set_model(self, provider: str, model_id: str) -> None:
        def on_response(response: dict) -> None:
            if not response.get("success"):
                self.conversation.add_info(
                    f"Model switch failed: {response.get('error', 'unknown error')}",
                    error=True,
                )
                return
            # We already know the chosen provider/id, so record them for the
            # picker checkmark; refresh the display name from the agent's
            # authoritative state rather than assuming set_model's reply shape.
            self.model_provider = provider
            self.model_id = model_id
            self._sync_state()

        self.agent.set_model(provider, model_id, on_response)

    # ---- Agent events -> conversation -----------------------------------

    def _mark_error_reported(self, message: dict) -> bool:
        """True the first time an errored message is seen, False on repeats, so
        a replayed agent_end payload doesn't re-announce old failures."""
        key = str(
            message.get("responseId")
            or message.get("timestamp")
            or message.get("errorMessage")
            or ""
        )
        if key in self._reported_errors:
            return False
        self._reported_errors.add(key)
        return True

    def _on_event(self, event: dict) -> None:
        conversation = self.conversation
        kind = event.get("type")
        if kind == "agent_start":
            self._turn_had_error = False
            self.streaming_changed.emit(True)
        elif kind == "agent_end":
            # A request the provider rejected outright (e.g. HTTP 400) ends
            # the run with an errored assistant message but streams no
            # message_update events — without this the turn fails silently.
            # Skip it when streaming already surfaced the error, and never
            # report the same errored message twice (agent_end may replay
            # earlier turns' messages).
            if not self._turn_had_error:
                for message in event.get("messages") or []:
                    error = error_summary(message)
                    if error and self._mark_error_reported(message):
                        conversation.add_info(f"Turn failed: {error}", error=True)
            self.streaming_changed.emit(False)
        elif kind == "message_update":
            delta = event.get("assistantMessageEvent") or {}
            if delta.get("type") == "text_delta":
                conversation.append_assistant_delta(delta.get("delta", ""))
            elif delta.get("type") == "error":
                self._turn_had_error = True
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
