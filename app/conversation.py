"""Conversation transcript: a read-only view of the chat with the Pi agent.

Renders user prompts, streamed assistant text (as markdown), tool-call
notes, and info lines. Content is kept as a list of semantic blocks and
repainted from that model on theme switches, so text written in dark mode
never lingers unreadable in light mode.

Assistant blocks are markdown sources: streaming deltas can split markdown
syntax mid-token, so instead of appending raw text, the trailing assistant
block is re-rendered in place (from its recorded start position) with the
accumulated source on every delta.
"""

from __future__ import annotations

from PySide6.QtGui import (
    QColor,
    QFont,
    QTextBlockFormat,
    QTextCharFormat,
    QTextCursor,
)
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

# One span of styled text inside a non-assistant block:
# (text, color_role, bold, italic). Assistant blocks carry markdown source
# strings instead.
_Span = tuple[str, str, bool, bool]


class ConversationView(QTextBrowser):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setOpenExternalLinks(False)
        # The transcript model: (kind, payload) per block. `kind` drives
        # block spacing; payload is a list of markdown source chunks for
        # assistant blocks, a list of _Span for everything else.
        self._blocks: list[tuple[str, list]] = []
        self._last_kind: str | None = None
        # Document position where the trailing assistant block's markdown
        # starts; None when the last block isn't assistant.
        self._assistant_start: int | None = None
        self.set_theme(DEFAULT_THEME)

    # ---- Theme -----------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._colors = _PALETTE[name]
        self.setStyleSheet(_STYLES[name])
        # Repaint existing content with the new palette.
        self.clear()
        self._last_kind = None
        self._assistant_start = None
        for kind, payload in self._blocks:
            self._paint(kind, payload)

    def _format(self, role: str, bold: bool, italic: bool) -> QTextCharFormat:
        fmt = QTextCharFormat()
        fmt.setForeground(QColor(self._colors[role]))
        if bold:
            fmt.setFontWeight(QFont.Weight.DemiBold)
        fmt.setFontItalic(italic)
        return fmt

    # ---- Writing ---------------------------------------------------------

    def _separator(self, cursor: QTextCursor, kind: str) -> None:
        """Blocks of different kinds get a blank line between them,
        consecutive tool/info lines just a newline. Inserted with default
        formats so markdown block styles (lists, headings) don't leak."""
        if self._last_kind is None:
            return
        block_fmt, char_fmt = QTextBlockFormat(), QTextCharFormat()
        if kind != self._last_kind:
            cursor.insertBlock(block_fmt, char_fmt)
            cursor.insertBlock(block_fmt, char_fmt)
        elif kind in ("tool", "info"):
            cursor.insertBlock(block_fmt, char_fmt)

    def _paint(self, kind: str, payload: list) -> None:
        scrollbar = self.verticalScrollBar()
        follow = scrollbar.value() >= scrollbar.maximum() - 4
        cursor = self.textCursor()
        cursor.movePosition(QTextCursor.MoveOperation.End)
        if kind == "assistant" and self._assistant_start is not None:
            # Streaming continuation: re-render the whole trailing assistant
            # block so markdown split across deltas parses correctly.
            cursor.setPosition(self._assistant_start)
            cursor.movePosition(
                QTextCursor.MoveOperation.End, QTextCursor.MoveMode.KeepAnchor
            )
            cursor.removeSelectedText()
            cursor.insertMarkdown("".join(payload))
        else:
            self._separator(cursor, kind)
            if kind == "assistant":
                self._assistant_start = cursor.position()
                cursor.insertMarkdown("".join(payload))
            else:
                self._assistant_start = None
                for text, role, bold, italic in payload:
                    cursor.insertText(text, self._format(role, bold, italic))
        self._last_kind = kind
        if follow:
            scrollbar.setValue(scrollbar.maximum())

    def _write(self, kind: str, payload: list) -> None:
        # Merge consecutive assistant deltas into one block: its full
        # markdown source must be re-rendered together.
        if kind == "assistant" and self._blocks and self._blocks[-1][0] == "assistant":
            self._blocks[-1][1].extend(payload)
            self._paint(kind, self._blocks[-1][1])
        else:
            self._blocks.append((kind, list(payload)))
            self._paint(kind, payload)

    def add_user(self, text: str) -> None:
        self._write("user", [("You\n", "user", True, False), (text, "text", False, False)])

    def append_assistant_delta(self, delta: str) -> None:
        self._write("assistant", [delta])

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
        self._assistant_start = None

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
