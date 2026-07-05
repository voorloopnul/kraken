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

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import (
    QColor,
    QFont,
    QTextBlockFormat,
    QTextCharFormat,
    QTextCursor,
    QTextFormat,
)
from PySide6.QtWidgets import QApplication, QTextBrowser, QToolButton, QWidget
from pygments.lexers import get_lexer_by_name
from pygments.token import Token
from pygments.util import ClassNotFound

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
    "dark": {
        "user": "#61afef", "text": "#d6d8dd", "dim": "#7a7d85", "error": "#e06c75",
        "code_bg": "#17181d",
    },
    "light": {
        "user": "#02669c", "text": "#1a1c21", "dim": "#5f6269", "error": "#a8232e",
        "code_bg": "#f0ebdc",
    },
}

_COPY_STYLES = {
    "dark": """
QToolButton { background: #26282e; border: 1px solid #33353c; border-radius: 4px;
              color: #9a9da5; font-size: 11px; padding: 2px 8px; }
QToolButton:hover { background: #2c2e35; color: #ffffff; }
""",
    "light": """
QToolButton { background: #faf6ec; border: 1px solid #d8d3c4; border-radius: 4px;
              color: #5f6269; font-size: 11px; padding: 2px 8px; }
QToolButton:hover { background: #efeadb; color: #1b1d22; }
""",
}

# Syntax highlight colors per theme: token type -> (color, italic). Token
# types resolve through their parents, so Token.Literal.String.Doc finds
# Token.Literal.String. Dark leans on One Dark; light uses darkened One
# Light values that keep contrast on the tinted code background.
_HIGHLIGHT = {
    "dark": {
        Token.Keyword: ("#c678dd", False),
        Token.Keyword.Constant: ("#d19a66", False),
        Token.Operator.Word: ("#c678dd", False),
        Token.Literal.String: ("#98c379", False),
        Token.Literal.Number: ("#d19a66", False),
        Token.Comment: ("#7d818c", True),
        Token.Name.Function: ("#61afef", False),
        Token.Name.Class: ("#e5c07b", False),
        Token.Name.Builtin: ("#56b6c2", False),
        Token.Name.Decorator: ("#e5c07b", False),
        Token.Name.Tag: ("#e06c75", False),
        Token.Name.Attribute: ("#d19a66", False),
    },
    "light": {
        Token.Keyword: ("#96218f", False),
        Token.Keyword.Constant: ("#8a5c00", False),
        Token.Operator.Word: ("#96218f", False),
        Token.Literal.String: ("#3c7d3b", False),
        Token.Literal.Number: ("#8a5c00", False),
        Token.Comment: ("#75786f", True),
        Token.Name.Function: ("#2a5fd3", False),
        Token.Name.Class: ("#9c6d00", False),
        Token.Name.Builtin: ("#077a92", False),
        Token.Name.Decorator: ("#9c6d00", False),
        Token.Name.Tag: ("#a8232e", False),
        Token.Name.Attribute: ("#8a5c00", False),
    },
}

# Lexer lookup is not free; cache per fence language (None = no lexer).
_LEXERS: dict[str, object] = {}


def _lexer_for(language: str):
    if language not in _LEXERS:
        try:
            _LEXERS[language] = get_lexer_by_name(language)
        except ClassNotFound:
            _LEXERS[language] = None
    return _LEXERS[language]


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
        # Floating "Copy" buttons, one per code block; _code_ranges holds
        # the matching (first, last) text-block numbers.
        self._code_ranges: list[tuple[int, int]] = []
        self._copy_buttons: list[QToolButton] = []
        self.verticalScrollBar().valueChanged.connect(self._layout_copy_buttons)
        self.set_theme(DEFAULT_THEME)

    # ---- Theme -----------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._colors = _PALETTE[name]
        self._copy_style = _COPY_STYLES[name]
        self.setStyleSheet(_STYLES[name])
        for button in self._copy_buttons:
            button.setStyleSheet(self._copy_style)
        # Repaint existing content with the new palette.
        self.clear()
        self._last_kind = None
        self._assistant_start = None
        for kind, payload in self._blocks:
            self._paint(kind, payload)
        self._sync_code_ui()

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
        # Everything before this document position survives the paint, so
        # code styling only needs re-applying from here on.
        changed_from = cursor.position()
        if kind == "assistant" and self._assistant_start is not None:
            changed_from = self._assistant_start
            # Streaming continuation: re-render the whole trailing assistant
            # block so markdown split across deltas parses correctly.
            cursor.setPosition(self._assistant_start)
            cursor.movePosition(
                QTextCursor.MoveOperation.End, QTextCursor.MoveMode.KeepAnchor
            )
            cursor.removeSelectedText()
            # The surviving block keeps the previous render's block format;
            # reset it so stale code-block properties (BlockCodeLanguage,
            # background) don't contaminate the re-rendered content.
            cursor.setBlockFormat(QTextBlockFormat())
            cursor.setCharFormat(QTextCharFormat())
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
        self._sync_code_ui(changed_from)
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
        self._sync_code_ui()

    # ---- Code blocks: background + copy buttons ----------------------------

    def _sync_code_ui(self, changed_from: int = 0) -> None:
        """Re-scan the document for code blocks (markdown import marks them
        with BlockCodeLanguage), tint them, syntax-highlight them, and give
        each one a Copy button. Ranges entirely before `changed_from` keep
        their existing highlighting."""
        document = self.document()
        ranges: list[tuple[int, int]] = []
        start: int | None = None
        block = document.firstBlock()
        while block.isValid():
            is_code = (
                block.blockFormat().property(QTextFormat.Property.BlockCodeLanguage)
                is not None
            )
            if is_code and start is None:
                start = block.blockNumber()
            elif not is_code and start is not None:
                ranges.append((start, block.blockNumber() - 1))
                start = None
            block = block.next()
        if start is not None:
            ranges.append((start, document.blockCount() - 1))
        self._code_ranges = ranges

        background = QColor(self._colors["code_bg"])
        for first, last in ranges:
            block = document.findBlockByNumber(first)
            while block.isValid() and block.blockNumber() <= last:
                if block.blockFormat().background().color() != background:
                    fmt = QTextBlockFormat()
                    fmt.setBackground(background)
                    QTextCursor(block).mergeBlockFormat(fmt)
                block = block.next()
            end_block = document.findBlockByNumber(last)
            if end_block.position() + end_block.length() >= changed_from:
                self._highlight_range(first, last)

        while len(self._copy_buttons) < len(ranges):
            self._copy_buttons.append(self._make_copy_button())
        self._layout_copy_buttons()

    def _highlight_range(self, first: int, last: int) -> None:
        """Apply pygments token colors to one code block. The fence language
        is on the blocks' BlockCodeLanguage property; offsets into the
        newline-joined source map linearly onto document positions because
        each block boundary occupies exactly one position."""
        document = self.document()
        first_block = document.findBlockByNumber(first)
        language = first_block.blockFormat().property(
            QTextFormat.Property.BlockCodeLanguage
        )
        lexer = _lexer_for(language) if language else None
        if lexer is None:
            return
        lines = []
        block = first_block
        while block.isValid() and block.blockNumber() <= last:
            lines.append(block.text())
            block = block.next()
        code = "\n".join(lines)

        table = _HIGHLIGHT[self._theme_name]
        base = first_block.position()
        cursor = QTextCursor(document)
        for offset, token, value in lexer.get_tokens_unprocessed(code):
            if not value.strip():
                continue
            node = token
            while node is not None and node not in table:
                node = node.parent
            if node is None:
                continue
            color, italic = table[node]
            fmt = QTextCharFormat()
            fmt.setForeground(QColor(color))
            fmt.setFontItalic(italic)
            cursor.setPosition(base + offset)
            cursor.setPosition(
                min(base + offset + len(value), base + len(code)),
                QTextCursor.MoveMode.KeepAnchor,
            )
            cursor.mergeCharFormat(fmt)

    def _make_copy_button(self) -> QToolButton:
        button = QToolButton(self.viewport())
        button.setText("Copy")
        button.setCursor(Qt.CursorShape.PointingHandCursor)
        button.setFocusPolicy(Qt.FocusPolicy.NoFocus)
        button.setStyleSheet(self._copy_style)
        button.clicked.connect(lambda _=False, b=button: self._copy_code(b))
        return button

    def _copy_code(self, button: QToolButton) -> None:
        index = self._copy_buttons.index(button)
        if index >= len(self._code_ranges):
            return
        first, last = self._code_ranges[index]
        document = self.document()
        lines = []
        block = document.findBlockByNumber(first)
        while block.isValid() and block.blockNumber() <= last:
            # Within one block, soft line breaks come through as U+2028.
            lines.append(block.text().replace("\u2028", "\n"))
            block = block.next()
        QApplication.clipboard().setText("\n".join(lines))
        button.setText("Copied ✓")
        self._layout_copy_buttons()

        def restore() -> None:
            button.setText("Copy")
            self._layout_copy_buttons()

        QTimer.singleShot(1500, restore)

    def _layout_copy_buttons(self) -> None:
        document = self.document()
        layout = document.documentLayout()
        scroll = self.verticalScrollBar().value()
        viewport = self.viewport()
        for index, button in enumerate(self._copy_buttons):
            if index >= len(self._code_ranges):
                button.hide()
                continue
            block = document.findBlockByNumber(self._code_ranges[index][0])
            if not block.isValid():
                button.hide()
                continue
            rect = layout.blockBoundingRect(block)
            button.adjustSize()
            x = viewport.width() - button.width() - 10
            y = int(rect.top()) - scroll + 2
            visible = y + button.height() > 0 and y < viewport.height()
            button.move(x, y)
            button.setVisible(visible)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._layout_copy_buttons()

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
