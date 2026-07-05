"""Conversation transcript: a read-only view of the chat with the Pi agent.

Renders user prompts, streamed assistant text, tool-call notes, and info
lines. Content is kept as a list of semantic blocks (kind + color role per
span) and painted with explicit character formats, so streaming deltas are
cheap appends and set_theme can repaint the whole transcript in the new
palette — text written in dark mode never lingers unreadable in light mode.
"""

from __future__ import annotations

from PySide6.QtGui import QColor, QFont, QTextCharFormat, QTextCursor
from PySide6.QtWidgets import QTextBrowser, QWidget

from app.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
QTextBrowser { background: transparent; border: none; color: #d6d8dd;
               font-size: 15px; padding: 4px; }
""",
    "light": """
QTextBrowser { background: transparent; border: none; color: #1a1c21;
               font-size: 15px; padding: 4px; }
""",
}

# Color roles per theme. Light values are picked for contrast on the light
# window background (#ececee): body text near-black, secondary text dark
# enough to stay comfortably readable while still reading as secondary.
_PALETTE = {
    "dark": {"user": "#61afef", "text": "#d6d8dd", "dim": "#7a7d85", "error": "#e06c75"},
    "light": {"user": "#02669c", "text": "#1a1c21", "dim": "#5f6269", "error": "#a8232e"},
}

# One span of styled text inside a block: (text, color_role, bold, italic).
_Span = tuple[str, str, bool, bool]


class ConversationView(QTextBrowser):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setOpenExternalLinks(False)
        # The transcript model: (kind, spans) per block. `kind` drives block
        # spacing and lets consecutive streaming deltas merge.
        self._blocks: list[tuple[str, list[_Span]]] = []
        self._last_kind: str | None = None
        self.set_theme(DEFAULT_THEME)

    # ---- Theme -----------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._colors = _PALETTE[name]
        self.setStyleSheet(_STYLES[name])
        # Repaint existing content with the new palette.
        self.clear()
        self._last_kind = None
        for kind, spans in self._blocks:
            self._paint(kind, spans)

    def _format(self, role: str, bold: bool, italic: bool) -> QTextCharFormat:
        fmt = QTextCharFormat()
        fmt.setForeground(QColor(self._colors[role]))
        if bold:
            fmt.setFontWeight(QFont.Weight.DemiBold)
        fmt.setFontItalic(italic)
        return fmt

    # ---- Writing ---------------------------------------------------------

    def _paint(self, kind: str, spans: list[_Span]) -> None:
        """Append one block to the document; blocks of different kinds get a
        blank line between them, consecutive tool/info lines just a newline,
        and consecutive assistant writes flow together (streaming deltas)."""
        scrollbar = self.verticalScrollBar()
        follow = scrollbar.value() >= scrollbar.maximum() - 4
        cursor = self.textCursor()
        cursor.movePosition(QTextCursor.MoveOperation.End)
        if self._last_kind is not None:
            if kind != self._last_kind:
                cursor.insertText("\n\n")
            elif kind in ("tool", "info"):
                cursor.insertText("\n")
        for text, role, bold, italic in spans:
            cursor.insertText(text, self._format(role, bold, italic))
        self._last_kind = kind
        if follow:
            scrollbar.setValue(scrollbar.maximum())

    def _write(self, kind: str, spans: list[_Span]) -> None:
        # Merge consecutive assistant deltas into one block so the model
        # stays small during streaming.
        if kind == "assistant" and self._blocks and self._blocks[-1][0] == "assistant":
            self._blocks[-1][1].extend(spans)
        else:
            self._blocks.append((kind, list(spans)))
        self._paint(kind, spans)

    def add_user(self, text: str) -> None:
        self._write("user", [("You\n", "user", True, False), (text, "text", False, False)])

    def append_assistant_delta(self, delta: str) -> None:
        self._write("assistant", [(delta, "text", False, False)])

    def add_tool(self, name: str, summary: str) -> None:
        line = f"⚒ {name}"
        if summary:
            line += f"  {summary}"
        self._write("tool", [(line, "dim", False, True)])

    def add_info(self, text: str, error: bool = False) -> None:
        self._write("info", [(text, "error" if error else "dim", False, True)])

    def clear_conversation(self) -> None:
        self.clear()
        self._blocks = []
        self._last_kind = None

    # ---- Rendering stored messages ----------------------------------------

    def render_messages(self, messages: list[dict]) -> None:
        """Replace the transcript with a session's stored messages."""
        self.clear_conversation()
        for message in messages:
            role = message.get("role")
            if role == "user":
                text = _content_text(message.get("content"))
                if text:
                    self.add_user(text)
            elif role == "assistant":
                for part in message.get("content") or []:
                    if not isinstance(part, dict):
                        continue
                    if part.get("type") == "text" and part.get("text"):
                        self.append_assistant_delta(part["text"])
                    elif part.get("type") == "toolCall":
                        self.add_tool(part.get("name", "?"), args_summary(part.get("arguments")))
            elif role == "bashExecution":
                self.add_tool("bash", message.get("command", ""))


def _content_text(content) -> str:
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        return " ".join(
            part.get("text", "")
            for part in content
            if isinstance(part, dict) and part.get("type") == "text"
        ).strip()
    return ""


def args_summary(args) -> str:
    if not isinstance(args, dict):
        return ""
    for key in ("command", "path", "file_path", "pattern", "url"):
        if isinstance(args.get(key), str):
            value = " ".join(args[key].split())
            return value if len(value) <= 80 else value[:79] + "…"
    return ""
