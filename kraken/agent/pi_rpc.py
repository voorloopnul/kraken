"""RPC client for the Pi coding agent (`pi --mode rpc`).

Spawns one agent process per workspace (cwd = workspace folder) and speaks
the JSONL protocol from pi's docs/rpc.md: commands go to stdin, responses
and events stream back on stdout. Framing is strict: records are split on
LF only, with a trailing CR stripped.

Commands may carry an `id`; the matching response is routed to the callback
passed to `send()`. Events (no id) are re-emitted as Qt signals. Extension
UI dialog requests are auto-cancelled so a headless extension can never
deadlock the chat.
"""

from __future__ import annotations

import json
from typing import Callable, TYPE_CHECKING

from PySide6.QtCore import QObject, QProcess, QProcessEnvironment, Signal

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget


class PiAgent(QObject):
    """One `pi --mode rpc` process bound to a workspace folder.

    For a remote workspace the process still runs locally, but is launched with
    the bundled ssh routing extension (see remote.ssh_extension_path) and a
    KRAKEN_SSH environment descriptor, so its read/write/edit/bash tools operate
    on the remote host over SSH. The workspace cwd is the local anchor folder,
    which is where pi runs and stores its session files."""

    started = Signal()
    event = Signal(dict)  # every agent event (agent_start, message_update, ...)
    notify = Signal(str, str)  # extension notify: (message, "info"|"warning"|"error")
    failed = Signal(str)  # process could not start / crashed
    finished = Signal(bool)  # process exited; True if it died mid-turn

    def __init__(
        self,
        cwd: str,
        parent: QObject | None = None,
        session_path: str | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self._cwd = cwd
        # When set, the process is launched already bound to this session file
        # (`--session <path>`), so its history loads without a switch round-trip.
        self._session_path = session_path
        # When set, tool execution is routed to this remote host over SSH; pi
        # itself still runs locally in `cwd` (the workspace's local anchor).
        self._remote = remote
        self._proc: QProcess | None = None
        self._buffer = b""
        self._next_id = 0
        self._callbacks: dict[str, Callable[[dict], None]] = {}
        self.is_streaming = False

    # ---- Lifecycle ------------------------------------------------------

    @property
    def running(self) -> bool:
        return (
            self._proc is not None
            and self._proc.state() == QProcess.ProcessState.Running
        )

    def ensure_started(self) -> None:
        if self._proc is not None:
            return
        proc = QProcess(self)
        proc.setWorkingDirectory(self._cwd)
        proc.setProcessChannelMode(QProcess.ProcessChannelMode.SeparateChannels)
        proc.readyReadStandardOutput.connect(self._on_stdout)
        proc.readyReadStandardError.connect(self._on_stderr)
        proc.errorOccurred.connect(self._on_error)
        proc.finished.connect(self._on_finished)
        self._proc = proc
        args = ["--mode", "rpc"]
        if self._remote is not None:
            # Load the ssh routing extension and describe the connection to it
            # via the environment; pi runs locally in the anchor cwd but its
            # tools act on the remote.
            from kraken.agent.remote import ssh_extension_path

            args += ["-e", ssh_extension_path()]
            env = QProcessEnvironment.systemEnvironment()
            env.insert("KRAKEN_SSH", self._remote.env_value())
            proc.setProcessEnvironment(env)
        if self._session_path:
            args += ["--session", self._session_path]
        proc.start("pi", args)

    def stop(self) -> None:
        if self._proc is None:
            return
        proc, self._proc = self._proc, None
        self._callbacks.clear()
        proc.readyReadStandardOutput.disconnect(self._on_stdout)
        proc.readyReadStandardError.disconnect(self._on_stderr)
        proc.errorOccurred.disconnect(self._on_error)
        proc.finished.disconnect(self._on_finished)
        if proc.state() != QProcess.ProcessState.NotRunning:
            proc.terminate()
            if not proc.waitForFinished(2000):
                proc.kill()
                proc.waitForFinished(1000)

    # ---- Commands -------------------------------------------------------

    def send(self, command: dict, callback: Callable[[dict], None] | None = None) -> None:
        """Send one command; `callback` receives the matching response."""
        self.ensure_started()
        if callback is not None:
            self._next_id += 1
            request_id = f"req-{self._next_id}"
            command = {**command, "id": request_id}
            self._callbacks[request_id] = callback
        data = json.dumps(command) + "\n"
        self._proc.write(data.encode())

    def prompt(
        self,
        message: str,
        images: list | None = None,
        callback: Callable[[dict], None] | None = None,
    ) -> None:
        command: dict = {"type": "prompt", "message": message}
        if images:
            # Prompt-ready dicts: {"type": "image", "data": <base64>,
            # "mimeType": ...}, per the pi RPC protocol.
            command["images"] = images
        if self.is_streaming:
            # Queue instead of erroring while the agent is mid-response.
            command["streamingBehavior"] = "steer"
        self.send(command, callback)

    def abort(self) -> None:
        """Stop the current turn. A running bash tool is cancelled through pi's
        own `_bashAbortController`, which plain `abort` never touches: `abort`
        calls `agent.abort()` then awaits idle, but a blocked command keeps the
        session busy forever, so `abort` alone can't interrupt a hung command.
        Send `abort_bash` first to kill any in-flight command (a no-op when none
        is running), then `abort` to end the turn."""
        if self.running:
            self.send({"type": "abort_bash"})
            self.send({"type": "abort"})

    def get_state(self, callback: Callable[[dict], None]) -> None:
        self.send({"type": "get_state"}, callback)

    def get_available_models(self, callback: Callable[[dict], None]) -> None:
        self.send({"type": "get_available_models"}, callback)

    def set_model(
        self,
        provider: str,
        model_id: str,
        callback: Callable[[dict], None] | None = None,
    ) -> None:
        self.send(
            {"type": "set_model", "provider": provider, "modelId": model_id},
            callback,
        )

    def set_thinking_level(
        self, level: str, callback: Callable[[dict], None] | None = None
    ) -> None:
        self.send({"type": "set_thinking_level", "level": level}, callback)

    def get_messages(self, callback: Callable[[dict], None]) -> None:
        self.send({"type": "get_messages"}, callback)

    def switch_session(self, session_path: str, callback: Callable[[dict], None] | None = None) -> None:
        self.send({"type": "switch_session", "sessionPath": session_path}, callback)

    def new_session(self, callback: Callable[[dict], None] | None = None) -> None:
        self.send({"type": "new_session"}, callback)

    # ---- Stream handling -------------------------------------------------

    def _on_stdout(self) -> None:
        if self._proc is None:  # a late readyRead after the process was reaped
            return
        self._buffer += bytes(self._proc.readAllStandardOutput())
        while True:
            newline = self._buffer.find(b"\n")
            if newline < 0:
                break
            line = self._buffer[:newline].removesuffix(b"\r")
            self._buffer = self._buffer[newline + 1 :]
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            self._dispatch(record)

    def _dispatch(self, record: dict) -> None:
        kind = record.get("type")
        if kind == "response":
            callback = self._callbacks.pop(record.get("id"), None)
            if callback is not None:
                callback(record)
            return
        if kind == "extension_ui_request":
            self._handle_ui_request(record)
            return
        if kind == "agent_start":
            self.is_streaming = True
        elif kind == "agent_end":
            self.is_streaming = False
        self.event.emit(record)

    def _handle_ui_request(self, record: dict) -> None:
        method = record.get("method")
        if method == "notify":
            self.notify.emit(record.get("message", ""), record.get("notifyType", "info"))
        elif method in ("select", "confirm", "input", "editor"):
            # No dialog UI yet: cancel so extensions never block the agent.
            self.send({
                "type": "extension_ui_response",
                "id": record.get("id"),
                "cancelled": True,
            })
        # setStatus / setWidget / setTitle / set_editor_text: fire-and-forget.

    def _on_stderr(self) -> None:
        if self._proc is None:  # a late readyRead after the process was reaped
            return
        data = bytes(self._proc.readAllStandardError()).decode(errors="replace")
        if data.strip():
            print(f"[pi rpc stderr] {data.rstrip()}", flush=True)

    def _on_error(self, error) -> None:
        self.is_streaming = False
        self.failed.emit(self._proc.errorString() if self._proc else str(error))

    def _on_finished(self, *_args) -> None:
        # A clean exit (nonzero code, provider drop, SSH extension death on a
        # remote) fires only `finished`, never `errorOccurred`, so this is the
        # sole place a mid-turn death can be noticed. Report whether streaming
        # was still in flight so the controller can clear the busy row and say
        # the turn was cut short. A crash fires `errorOccurred` first, which
        # already reset is_streaming, so `died_mid_turn` is False and we don't
        # double-announce.
        died_mid_turn = self.is_streaming
        self.is_streaming = False
        # Drop the dead process and its orphaned callbacks so the next prompt
        # respawns via ensure_started (resuming the session with --session when
        # the path is known) instead of writing into a corpse.
        self._proc = None
        self._buffer = b""
        self._callbacks.clear()
        self.finished.emit(died_mid_turn)
