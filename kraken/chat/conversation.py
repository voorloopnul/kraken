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

from PySide6.QtCore import Qt, QRectF, QTimer, Signal
from PySide6.QtGui import (
    QColor,
    QFont,
    QPainter,
    QPen,
    QTextBlockFormat,
    QTextCharFormat,
    QTextCursor,
    QTextFormat,
)
from PySide6.QtWidgets import QApplication, QTextBrowser, QToolButton, QWidget
from pygments.lexers import get_lexer_by_name
from pygments.token import Token
from pygments.util import ClassNotFound

from kraken.chat.formatting import (
    _clip,
    _content_text,
    args_detail,
    args_summary,
    error_summary,
)
from kraken.ui.fonts import MONO_FAMILY
from kraken.ui.themes import DEFAULT_THEME

# The horizontal scrollbar (wide code blocks) matches the panel-edge
# vertical one: a bare rounded handle on a transparent track, no steppers.
_STYLES = {
    "dark": """
QTextBrowser { background: transparent; border: none; color: #d6d8dd;
               font-family: 'JetBrains Mono'; font-size: 13px; padding: 4px; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #3a3d45; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #4a4e58; }
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal { width: 0; }
QScrollBar::add-page:horizontal, QScrollBar::sub-page:horizontal { background: transparent; }
""",
    "light": """
QTextBrowser { background: transparent; border: none; color: #1a1c21;
               font-family: 'JetBrains Mono'; font-size: 13px; padding: 4px; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #c9c4b4; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #b3ae9e; }
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal { width: 0; }
QScrollBar::add-page:horizontal, QScrollBar::sub-page:horizontal { background: transparent; }
""",
}

# Color roles per theme. Light values are picked for contrast on the light
# window background (#ececee): body text near-black, secondary text dark
# enough to stay comfortably readable while still reading as secondary.
_PALETTE = {
    "dark": {
        "text": "#d6d8dd", "dim": "#7a7d85", "error": "#e06c75",
        "code_bg": "#17181d", "code_border": "#2c2e35",
        "user_bg": "#26282e", "user_border": "#33353c",
        "link": "#61afef",
        "inline_bg": "#3a3f4a", "inline_text": "#d19a66",
    },
    "light": {
        "text": "#1a1c21", "dim": "#5f6269", "error": "#a8232e",
        "code_bg": "#f0ebdc", "code_border": "#d8d3c4",
        "user_bg": "#dfe0e4", "user_border": "#c9cbd2",
        "link": "#02669c",
        "inline_bg": "#f0ebdc", "inline_text": "#8a5c00",
    },
}

# Marks the tool blocks' monospace detail text. It is fixed-pitch like a
# markdown code span, but it is not code in the transcript's sense and must
# not pick up the inline-code chip, so the pass below skips it by this flag.
_TOOL_DETAIL_PROPERTY = QTextFormat.Property.UserProperty + 1

# The button is borderless; its background matches the code card (_PALETTE
# code_bg) so it reads as flat against the card yet still hides the first code
# line it overlaps. Its :hover firms up the background as a click affordance.
_COPY_STYLES = {
    "dark": """
QToolButton { background: #17181d; border: none; border-radius: 4px;
              color: #7d818c; font-size: 11px; padding: 2px 8px; }
QToolButton:hover { background: #2c2e35; color: #ffffff; }
""",
    "light": """
QToolButton { background: #f0ebdc; border: none; border-radius: 4px;
              color: #85887f; font-size: 11px; padding: 2px 8px; }
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

# insertMarkdown lays out headings and code fences with no vertical
# margins; these are merged into the block formats after each render.
_HEADING_TOP_MARGIN = 14.0
# Qt sizes markdown headings with a relative FontSizeAdjustment step over
# the 13px body font (its defaults: h1=+3≈26px, h2=+2≈19px, h3=+1≈16px).
# An absolute FontPixelSize is ignored by the layout — the default font's
# pixel size wins the resolve — so headings are toned down by lowering the
# adjustment itself, one step gentler than Qt's default. Levels past 3 sit
# at body size but bold.
_HEADING_ADJUST = {1: 2, 2: 1, 3: 0}
_HEADING_DEFAULT_ADJUST = 0
# Code blocks render as a rounded, bordered card (painted in paintEvent)
# with the text inset inside it. The fence blocks carry margins that leave
# room for the card: the top/bottom fence margin is the outer gap to the
# surrounding text plus the interior vertical padding, and the left/right
# margin insets the text horizontally from the card border.
_CODE_CARD_RADIUS = 8.0
_CODE_CARD_GAP = 6.0          # gap between the card edge and neighbouring text
_CODE_CARD_PAD_Y = 10.0       # vertical breathing room inside the card
_CODE_CARD_PAD_X = 14.0       # horizontal inset of the code text inside the card
_CODE_BLOCK_MARGIN = _CODE_CARD_GAP + _CODE_CARD_PAD_Y

# User messages render right-aligned inside a rounded bubble (painted in
# paintEvent, like the code cards). The bubble hugs the text: its left edge
# follows the widest laid-out line, and the block margins reserve the
# interior padding plus a gap to neighbouring blocks.
_BUBBLE_RADIUS = 10.0
_BUBBLE_GAP = 6.0
_BUBBLE_PAD_Y = 8.0
_BUBBLE_PAD_X = 14.0
_BUBBLE_MIN_LEFT = 48.0       # keeps a bubble from spanning the full width
_BUBBLE_MARGIN = _BUBBLE_GAP + _BUBBLE_PAD_Y

# Bubbles and code cards paint their interior padding *above* the first block
# and *below* the last. The first/last block's own top/bottom margin collapses
# out through the root frame, so it reserves no space and the top bubble/card
# gets clipped against the viewport top. A fixed top/bottom margin on the root
# frame guarantees room; it must clear the largest interior pad plus a gap.
_DOC_MARGIN_Y = _BUBBLE_GAP + max(_BUBBLE_PAD_Y, _CODE_CARD_PAD_Y)

# How far off the bottom the view can sit and still count as "at the bottom",
# so a stray pixel of rounding doesn't read as the reader having scrolled up.
_STICK_SLACK = 4

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
    # A web link in the transcript was clicked; the workspace routes it to
    # the embedded browser panel.
    link_clicked = Signal(str)

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setOpenExternalLinks(False)
        # Anchor clicks toggle tool blocks; without this QTextBrowser would
        # try to navigate to the anchor and clear the document.
        self.setOpenLinks(False)
        self.anchorClicked.connect(self._on_anchor_clicked)
        # The transcript model: (kind, payload) per block. `kind` drives
        # block spacing; payload is a list of markdown source chunks for
        # assistant blocks, a dict for collapsible tool blocks, and a list
        # of _Span for everything else.
        self._blocks: list[tuple[str, list | dict]] = []
        self._last_kind: str | None = None
        # Whitespace-only assistant deltas held back until the message
        # proves to have visible text; see append_assistant_delta.
        self._pending_assistant = ""
        # Document position where the trailing assistant block's markdown
        # starts; None when the last block isn't assistant.
        self._assistant_start: int | None = None
        # Streaming deltas accumulate in the block model but repaint at most
        # once per interval: re-rendering the whole assistant block on every
        # token is O(message) per token, i.e. quadratic over a long reply.
        self._assistant_dirty = False
        self._flush_timer = QTimer(self)
        self._flush_timer.setSingleShot(True)
        self._flush_timer.setInterval(50)
        self._flush_timer.timeout.connect(self._flush_assistant)
        # Floating "Copy" buttons, one per code block; _code_ranges holds
        # the matching (first, last) text-block numbers.
        self._code_ranges: list[tuple[int, int]] = []
        # (first, last) text-block numbers of each user message, for the
        # bubbles painted behind them in paintEvent.
        self._user_ranges: list[tuple[int, int]] = []
        self._copy_buttons: list[QToolButton] = []
        # Sticky bottom: the transcript follows new content while the reader is
        # already at the end, and stays put once they scroll up to read.
        # _stick_pending coalesces the deferred pins (see _follow_bottom).
        self._stick_to_bottom = True
        self._stick_pending = False
        self._rebuilding = False
        scrollbar = self.verticalScrollBar()
        scrollbar.valueChanged.connect(self._layout_copy_buttons)
        scrollbar.valueChanged.connect(self._update_stick)
        scrollbar.rangeChanged.connect(self._follow_bottom)
        # set_theme() below repaints (clearing and re-applying the frame
        # margins), so no separate _apply_frame_margins() is needed here.
        self.set_theme(DEFAULT_THEME)

    # ---- Sticky bottom -----------------------------------------------------

    # Following has to key off the scroll *range*, not off painting. Plenty of
    # things grow the range without painting a block — above all the busy row,
    # which appears right after a prompt is sent and takes its height out of the
    # transcript's viewport. Deciding "am I at the bottom?" only inside _paint
    # left the view stranded those few dozen pixels short, and every later paint
    # then read that gap as the reader having scrolled up: the reply streamed in
    # below the fold and following never resumed. rangeChanged catches the lot.

    def _update_stick(self, value: int) -> None:
        """Follow only from the bottom. Fires for our own pinning too, which
        lands exactly at maximum and so keeps following."""
        if self._rebuilding:
            return
        self._stick_to_bottom = value >= self.verticalScrollBar().maximum() - _STICK_SLACK

    def _follow_bottom(self, *_range) -> None:
        if self._rebuilding:
            return
        # Deferred, not immediate: the panel-edge scrollbar mirrors this one and
        # syncs its own range from this same signal, so scrolling now is mirrored
        # out to a bar still holding the old maximum, clamped there, and mirrored
        # straight back — which pins the transcript near the top instead of the
        # bottom. Yielding lets every handler settle first, and collapses a burst
        # of range changes into one pin.
        if not self._stick_to_bottom or self._stick_pending:
            return
        self._stick_pending = True
        QTimer.singleShot(0, self._scroll_to_bottom)

    def _scroll_to_bottom(self) -> None:
        self._stick_pending = False
        if not self._stick_to_bottom:
            return  # the reader scrolled away while the pin was pending
        scrollbar = self.verticalScrollBar()
        scrollbar.setValue(scrollbar.maximum())

    def _apply_frame_margins(self) -> None:
        """Reserve fixed vertical space inside the root frame so the first
        bubble/card isn't clipped at the top (nor the last at the bottom)."""
        frame = self.document().rootFrame()
        fmt = frame.frameFormat()
        fmt.setTopMargin(_DOC_MARGIN_Y)
        fmt.setBottomMargin(_DOC_MARGIN_Y)
        frame.setFrameFormat(fmt)

    # ---- Theme -----------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._colors = _PALETTE[name]
        self._copy_style = _COPY_STYLES[name]
        self.setStyleSheet(_STYLES[name])
        for button in self._copy_buttons:
            button.setStyleSheet(self._copy_style)
        self._repaint_all()

    def _repaint_all(self) -> None:
        """Re-render the whole document from the block model (theme change,
        collapse toggle), keeping the scroll position."""
        # The model already holds any pending streamed text; a full repaint
        # supersedes the deferred single-block flush.
        self._flush_timer.stop()
        self._assistant_dirty = False
        scroll = self.verticalScrollBar().value()
        # Emptying and refilling the document churns the scroll range and value;
        # shut the sticky-bottom logic out of it, or the momentarily empty
        # document reads as the reader sitting at the end and the rebuild
        # overrides where they actually were.
        self._rebuilding = True
        self.clear()
        # clear() resets the root frame to Qt's default margins, dropping the
        # top/bottom space that keeps the first/last bubble from clipping.
        self._apply_frame_margins()
        self._last_kind = None
        self._assistant_start = None
        self._user_ranges = []
        for kind, payload in self._blocks:
            self._paint(kind, payload)
        self._sync_code_ui()
        self._rebuilding = False
        if self._stick_to_bottom:
            self._follow_bottom()
        else:
            self.verticalScrollBar().setValue(scroll)

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
        consecutive tool/info lines just a newline. Consecutive user
        messages also get the full break: without it they'd land in the
        same text block and render as one merged bubble. Inserted with
        default formats so markdown block styles (lists, headings) don't
        leak."""
        if self._last_kind is None:
            return
        block_fmt, char_fmt = QTextBlockFormat(), QTextCharFormat()
        if kind != self._last_kind or kind == "user":
            cursor.insertBlock(block_fmt, char_fmt)
            cursor.insertBlock(block_fmt, char_fmt)
        elif kind in ("tool", "info"):
            cursor.insertBlock(block_fmt, char_fmt)

    def _paint(self, kind: str, payload: list) -> None:
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
            elif kind == "tool":
                self._assistant_start = None
                self._paint_tool(cursor, payload)
            elif kind == "user":
                self._assistant_start = None
                self._paint_user(cursor, payload)
            else:
                self._assistant_start = None
                for text, role, bold, italic in payload:
                    cursor.insertText(text, self._format(role, bold, italic))
        self._last_kind = kind
        self._sync_code_ui(changed_from)
        self._follow_bottom()

    def _paint_tool(self, cursor: QTextCursor, payload: dict) -> None:
        """One collapsible line: a clickable '▸/▾ ⚒ tool  summary' header;
        expanded, the full detail (args, output) follows in monospace."""
        arrow = "▾" if payload["expanded"] else "▸"
        header = self._format("dim", False, True)
        header.setAnchor(True)
        header.setAnchorHref(f"tool:{payload['index']}")
        cursor.insertText(f"{arrow} {payload['line']}", header)
        if payload["expanded"] and payload["detail"]:
            detail = self._format("dim", False, False)
            detail.setFontFamilies([MONO_FAMILY, "monospace"])
            detail.setFontFixedPitch(True)
            detail.setFontPointSize(9.0)
            detail.setProperty(_TOOL_DETAIL_PROPERTY, True)
            cursor.insertText("\n" + payload["detail"], detail)

    def _paint_user(self, cursor: QTextCursor, payload: list) -> None:
        """Right-aligned message in a bubble. The side margins inset the text
        from the bubble edges; the gap+pad vertical margins go on the first
        and last block only so a multi-line message stays compact."""
        base = QTextBlockFormat()
        base.setAlignment(Qt.AlignmentFlag.AlignRight)
        base.setLeftMargin(_BUBBLE_MIN_LEFT)
        base.setRightMargin(_BUBBLE_PAD_X)
        cursor.setBlockFormat(base)
        first = cursor.blockNumber()
        for text, role, bold, italic in payload:
            cursor.insertText(text, self._format(role, bold, italic))
        last = cursor.blockNumber()
        document = self.document()
        top = QTextBlockFormat()
        top.setTopMargin(_BUBBLE_MARGIN)
        QTextCursor(document.findBlockByNumber(first)).mergeBlockFormat(top)
        bottom = QTextBlockFormat()
        bottom.setBottomMargin(_BUBBLE_MARGIN)
        QTextCursor(document.findBlockByNumber(last)).mergeBlockFormat(bottom)
        self._user_ranges.append((first, last))

    def _on_anchor_clicked(self, url) -> None:
        target = url.toString()
        if target.startswith("tool:"):
            index = int(target.split(":", 1)[1])
            if 0 <= index < len(self._blocks) and self._blocks[index][0] == "tool":
                payload = self._blocks[index][1]
                payload["expanded"] = not payload["expanded"]
                self._repaint_all()
        elif url.scheme() in ("http", "https"):
            self.link_clicked.emit(target)

    def _write(self, kind: str, payload: list) -> None:
        # A non-assistant block ends any assistant message in progress, so
        # held-back whitespace belonged to a text part that never became
        # visible — drop it, and flush any pending streamed text so the
        # finished reply is painted before the new block.
        if kind != "assistant":
            self._pending_assistant = ""
            self._flush_assistant()
        # Merge consecutive assistant deltas into one block: its full
        # markdown source must be re-rendered together.
        if kind == "assistant" and self._blocks and self._blocks[-1][0] == "assistant":
            self._blocks[-1][1].extend(payload)
            self._schedule_assistant_flush()
        else:
            self._blocks.append((kind, list(payload)))
            self._paint(kind, payload)

    def _schedule_assistant_flush(self) -> None:
        """Mark the trailing assistant block dirty and coalesce repaints:
        one paint per timer interval rather than one per delta."""
        self._assistant_dirty = True
        if not self._flush_timer.isActive():
            self._flush_timer.start()

    def _flush_assistant(self) -> None:
        self._flush_timer.stop()
        if not self._assistant_dirty:
            return
        self._assistant_dirty = False
        if self._blocks and self._blocks[-1][0] == "assistant":
            self._paint("assistant", self._blocks[-1][1])

    def add_user(self, text: str) -> None:
        self._write("user", [(text, "text", False, False)])

    def append_assistant_delta(self, delta: str) -> None:
        # Models often emit a whitespace-only text part (e.g. a single
        # space) between thinking and tool calls; opening an assistant
        # block for it renders nothing but still costs the blank-line
        # separators on both sides. Until an assistant block is open,
        # hold whitespace back and only write once visible text arrives.
        if not (self._blocks and self._blocks[-1][0] == "assistant"):
            if not (self._pending_assistant + delta).strip():
                self._pending_assistant += delta
                return
            delta = self._pending_assistant + delta
            self._pending_assistant = ""
        self._write("assistant", [delta])

    def add_tool(self, name: str, summary: str, detail: str = "") -> int:
        """Add a collapsible tool line; returns its block index so callers
        can append the result once the tool finishes."""
        self._flush_assistant()
        line = f"⚒ {name}"
        if summary:
            line += f"  {summary}"
        payload = {
            "line": line,
            "detail": _clip(detail),
            "expanded": False,
            "index": len(self._blocks),
        }
        self._blocks.append(("tool", payload))
        self._paint("tool", payload)
        return payload["index"]

    def append_tool_detail(self, index: int, text: str) -> None:
        """Append to a tool block's hidden detail (e.g. its result)."""
        if not (0 <= index < len(self._blocks)) or self._blocks[index][0] != "tool":
            return
        payload = self._blocks[index][1]
        joined = f"{payload['detail']}\n\n{text}" if payload["detail"] else text
        payload["detail"] = _clip(joined)
        if payload["expanded"]:
            self._repaint_all()

    def add_info(self, text: str, error: bool = False) -> None:
        self._write("info", [(text, "error" if error else "dim", False, True)])

    @property
    def is_empty(self) -> bool:
        return not self._blocks

    def clear_conversation(self) -> None:
        self._flush_timer.stop()
        self._assistant_dirty = False
        self.clear()
        self._apply_frame_margins()
        self._blocks = []
        self._last_kind = None
        self._assistant_start = None
        self._pending_assistant = ""
        self._user_ranges = []
        # A fresh or newly loaded transcript opens at its end.
        self._stick_to_bottom = True
        self._sync_code_ui()

    # ---- Code blocks: background + copy buttons ----------------------------

    def _sync_code_ui(self, changed_from: int = 0) -> None:
        """Give headings/code blocks their margins, tint and syntax-highlight
        code blocks, and give each one a Copy button. Only blocks from
        `changed_from` onward are re-examined; everything before it survived
        the paint untouched, so its cached ranges and styling are reused —
        without this, a streamed reply would re-scan the whole document on
        every repaint (quadratic over the message)."""
        document = self.document()
        changed_block = document.findBlock(changed_from).blockNumber()
        # A cached code range straddling the boundary must be re-scanned from
        # its start; only ranges wholly before that are kept as-is.
        rescan_from = changed_block
        for first, last in self._code_ranges:
            if first < changed_block <= last:
                rescan_from = min(rescan_from, first)
        prefix = [r for r in self._code_ranges if r[1] < rescan_from]

        tail: list[tuple[int, int]] = []
        start: int | None = None
        block = document.findBlockByNumber(rescan_from)
        while block.isValid():
            fmt = block.blockFormat()
            level = fmt.headingLevel()
            if level and fmt.topMargin() != _HEADING_TOP_MARGIN:
                margin = QTextBlockFormat()
                margin.setTopMargin(_HEADING_TOP_MARGIN)
                QTextCursor(block).mergeBlockFormat(margin)
                # Rein in Qt's oversized heading font to our own scale by
                # lowering its size-adjustment step (see _HEADING_ADJUST).
                adjust = _HEADING_ADJUST.get(level, _HEADING_DEFAULT_ADJUST)
                char = QTextCharFormat()
                char.setProperty(QTextFormat.Property.FontSizeAdjustment, adjust)
                cursor = QTextCursor(block)
                cursor.movePosition(QTextCursor.MoveOperation.StartOfBlock)
                cursor.movePosition(
                    QTextCursor.MoveOperation.EndOfBlock,
                    QTextCursor.MoveMode.KeepAnchor,
                )
                cursor.mergeCharFormat(char)
            self._retint_links(block)
            is_code = (
                fmt.property(QTextFormat.Property.BlockCodeLanguage) is not None
            )
            if not is_code:
                self._restyle_inline_code(block)
            if is_code and start is None:
                start = block.blockNumber()
            elif not is_code and start is not None:
                tail.append((start, block.blockNumber() - 1))
                start = None
            block = block.next()
        if start is not None:
            tail.append((start, document.blockCount() - 1))
        self._code_ranges = prefix + tail

        # Only the freshly scanned tail needs margins/highlighting; the prefix
        # kept its styling from earlier paints. The card fill and border are
        # painted in paintEvent; here we only inset the text so it sits inside
        # that card.
        for first, last in tail:
            block = document.findBlockByNumber(first)
            while block.isValid() and block.blockNumber() <= last:
                fmt = block.blockFormat()
                edges = QTextBlockFormat()
                # Horizontal inset applies to every line of the block.
                if fmt.leftMargin() != _CODE_CARD_PAD_X:
                    edges.setLeftMargin(_CODE_CARD_PAD_X)
                    edges.setRightMargin(_CODE_CARD_PAD_X)
                # Vertical margins go on the fence edges only; putting them on
                # every block would space out the individual code lines.
                if (
                    block.blockNumber() == first
                    and fmt.topMargin() != _CODE_BLOCK_MARGIN
                ):
                    edges.setTopMargin(_CODE_BLOCK_MARGIN)
                if (
                    block.blockNumber() == last
                    and fmt.bottomMargin() != _CODE_BLOCK_MARGIN
                ):
                    edges.setBottomMargin(_CODE_BLOCK_MARGIN)
                if edges.propertyCount():
                    QTextCursor(block).mergeBlockFormat(edges)
                block = block.next()
            self._restyle_code_font(first, last)
            self._highlight_range(first, last)

        while len(self._copy_buttons) < len(self._code_ranges):
            self._copy_buttons.append(self._make_copy_button())
        self._layout_copy_buttons()

    def _restyle_code_font(self, first: int, last: int) -> None:
        """Give a fenced block the body font at the body size.

        Same story as _restyle_inline_code: insertMarkdown pins code to a
        generic "monospace" at a fixed 9 point, so the cards would render in
        the system's mono font at a size of their own while the rest of the
        transcript is the bundled one. Only the font is touched — the tint and
        border are the card painted in paintEvent.

        Runs before _highlight_range: the token colors are merged on top of
        these formats, and replacing a format afterwards would drop them.
        """
        document = self.document()
        block = document.findBlockByNumber(first)
        while block.isValid() and block.blockNumber() <= last:
            spans: list[tuple[int, int, QTextCharFormat]] = []
            it = block.begin()
            while not it.atEnd():
                fragment = it.fragment()
                fmt = fragment.charFormat()
                # fontPointSize() reads 0 once the property is cleared, which
                # is what keeps this pass idempotent across repaints.
                if fmt.fontPointSize():
                    styled = QTextCharFormat(fmt)
                    styled.clearProperty(QTextFormat.Property.FontPointSize)
                    styled.setFontFamilies([MONO_FAMILY, "monospace"])
                    spans.append((fragment.position(), fragment.length(), styled))
                it += 1
            for position, length, styled in spans:
                cursor = QTextCursor(document)
                cursor.setPosition(position)
                cursor.setPosition(position + length, QTextCursor.MoveMode.KeepAnchor)
                cursor.setCharFormat(styled)
            block = block.next()

    def _restyle_inline_code(self, block) -> None:
        """Give markdown code spans the body font at the body size, on a
        tinted chip.

        insertMarkdown styles a code span as generic "monospace" at a fixed 9
        point — a different family and size from the body text, both resolved
        from whatever the system offers, so the span lands visibly off-size
        and off-baseline against its own sentence. Clearing the point size
        lets it inherit the document's, which is what keeps it matched to the
        surrounding words.

        Fenced code blocks are excluded by the caller (they are carded and
        syntax-highlighted instead), and the tool blocks' detail text opts out
        with _TOOL_DETAIL_PROPERTY. Already-styled spans are skipped, keeping
        the pass idempotent across repaints.
        """
        chip = QColor(self._colors["inline_bg"])
        spans: list[tuple[int, int, QTextCharFormat]] = []
        it = block.begin()
        while not it.atEnd():
            fragment = it.fragment()
            fmt = fragment.charFormat()
            if (
                fmt.fontFixedPitch()
                and not fmt.property(_TOOL_DETAIL_PROPERTY)
                and fmt.background().color() != chip
            ):
                styled = QTextCharFormat(fmt)
                # Not merged: the point size has to be cleared, and a merge
                # can only add properties.
                styled.clearProperty(QTextFormat.Property.FontPointSize)
                styled.setFontFamilies([MONO_FAMILY, "monospace"])
                styled.setBackground(chip)
                styled.setForeground(QColor(self._colors["inline_text"]))
                spans.append((fragment.position(), fragment.length(), styled))
            it += 1
        for position, length, styled in spans:
            cursor = QTextCursor(self.document())
            cursor.setPosition(position)
            cursor.setPosition(position + length, QTextCursor.MoveMode.KeepAnchor)
            cursor.setCharFormat(styled)

    def _retint_links(self, block) -> None:
        """Recolor markdown anchors to the theme's link color. insertMarkdown
        stamps them with the application palette's link blue, which is far
        too dark to read on the dark background. Tool-header anchors carry
        their own color and are skipped. Already-recolored fragments are
        skipped too, keeping the pass idempotent across repaints."""
        link = QColor(self._colors["link"])
        spans: list[tuple[int, int]] = []
        it = block.begin()
        while not it.atEnd():
            fragment = it.fragment()
            fmt = fragment.charFormat()
            if (
                fmt.isAnchor()
                and not (fmt.anchorHref() or "").startswith("tool:")
                and fmt.foreground().color() != link
            ):
                spans.append((fragment.position(), fragment.length()))
            it += 1
        recolor = QTextCharFormat()
        recolor.setForeground(link)
        for position, length in spans:
            cursor = QTextCursor(self.document())
            cursor.setPosition(position)
            cursor.setPosition(position + length, QTextCursor.MoveMode.KeepAnchor)
            cursor.mergeCharFormat(recolor)

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
        # Tuck the button into the card's top-right corner, inset equally from
        # the card's border on both axes so it reads as part of the card. The
        # card edges are derived exactly as _paint_code_cards draws them.
        margin = document.documentMargin()
        inset = 6
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
            card_right = viewport.width() - margin
            card_top = rect.top() - scroll - _CODE_CARD_PAD_Y
            x = int(card_right - button.width() - inset)
            y = int(card_top + inset)
            visible = y + button.height() > 0 and y < viewport.height()
            button.move(x, y)
            button.setVisible(visible)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._layout_copy_buttons()

    def paintEvent(self, event) -> None:
        # Draw the code-block cards and user bubbles behind the text:
        # super().paintEvent paints the (transparent) document background and
        # the text on top, so the rounded, bordered shapes show through
        # underneath.
        if self._code_ranges:
            self._paint_code_cards()
        if self._user_ranges:
            self._paint_user_bubbles()
        super().paintEvent(event)

    def _paint_code_cards(self) -> None:
        """Paint a rounded, bordered card behind each code block. Qt's text
        block backgrounds are flat, square, and full-width, so the card look
        (border, radius, side inset) is drawn here rather than via block
        formats; the text is inset into it by the code-block margins."""
        document = self.document()
        layout = document.documentLayout()
        viewport = self.viewport()
        scroll = self.verticalScrollBar().value()
        margin = document.documentMargin()
        left = margin
        right = viewport.width() - margin
        painter = QPainter(viewport)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        pen = QPen(QColor(self._colors["code_border"]))
        pen.setWidthF(1.0)
        painter.setPen(pen)
        painter.setBrush(QColor(self._colors["code_bg"]))
        for first, last in self._code_ranges:
            top_block = document.findBlockByNumber(first)
            bottom_block = document.findBlockByNumber(last)
            if not top_block.isValid() or not bottom_block.isValid():
                continue
            # blockBoundingRect covers the text lines only, not the fence
            # margins, so the card is grown past the text by the interior pad;
            # the fence margins (gap + pad) reserve room so it clears neighbours.
            top = layout.blockBoundingRect(top_block).top() - scroll - _CODE_CARD_PAD_Y
            bottom = (
                layout.blockBoundingRect(bottom_block).bottom() - scroll + _CODE_CARD_PAD_Y
            )
            if bottom < 0 or top > viewport.height():
                continue
            # Half-pixel inset keeps the 1px stroke on a crisp pixel boundary.
            rect = QRectF(
                left + 0.5, top + 0.5, (right - left) - 1.0, (bottom - top) - 1.0
            )
            painter.drawRoundedRect(rect, _CODE_CARD_RADIUS, _CODE_CARD_RADIUS)
        painter.end()

    def _paint_user_bubbles(self) -> None:
        """Paint a rounded bubble behind each user message, flush right and
        hugging the text: the left edge tracks the leftmost laid-out line
        across the message's blocks."""
        document = self.document()
        layout = document.documentLayout()
        viewport = self.viewport()
        scroll = self.verticalScrollBar().value()
        margin = document.documentMargin()
        right = viewport.width() - margin
        painter = QPainter(viewport)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        pen = QPen(QColor(self._colors["user_border"]))
        pen.setWidthF(1.0)
        painter.setPen(pen)
        painter.setBrush(QColor(self._colors["user_bg"]))
        for first, last in self._user_ranges:
            top_block = document.findBlockByNumber(first)
            bottom_block = document.findBlockByNumber(last)
            if not top_block.isValid() or not bottom_block.isValid():
                continue
            top = layout.blockBoundingRect(top_block).top() - scroll - _BUBBLE_PAD_Y
            bottom = (
                layout.blockBoundingRect(bottom_block).bottom() - scroll + _BUBBLE_PAD_Y
            )
            if bottom < 0 or top > viewport.height():
                continue
            left = right
            block = top_block
            while block.isValid() and block.blockNumber() <= last:
                text_layout = block.layout()
                x = text_layout.position().x()
                for i in range(text_layout.lineCount()):
                    line_rect = text_layout.lineAt(i).naturalTextRect()
                    left = min(left, x + line_rect.left())
                block = block.next()
            left -= _BUBBLE_PAD_X
            rect = QRectF(
                left + 0.5, top + 0.5, (right - left) - 1.0, (bottom - top) - 1.0
            )
            painter.drawRoundedRect(rect, _BUBBLE_RADIUS, _BUBBLE_RADIUS)
        painter.end()

    # ---- Rendering stored messages ----------------------------------------

    def render_messages(self, messages: list[dict]) -> None:
        """Replace the transcript with a session's stored messages."""
        self.clear_conversation()
        tool_blocks: dict[str, int] = {}  # toolCallId -> block index
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
                        index = self.add_tool(
                            part.get("name", "?"),
                            args_summary(part.get("arguments")),
                            detail=args_detail(part.get("arguments")),
                        )
                        if part.get("id"):
                            tool_blocks[part["id"]] = index
                error = error_summary(message)
                if error:
                    self.add_info(f"Turn failed: {error}", error=True)
            elif role == "toolResult":
                index = tool_blocks.get(message.get("toolCallId"))
                result = _content_text(message.get("content"))
                if index is not None and result:
                    self.append_tool_detail(index, result)
            elif role == "bashExecution":
                self.add_tool(
                    "bash", message.get("command", ""),
                    detail=message.get("output", ""),
                )
        # Paint a trailing assistant reply now rather than after the timer.
        self._flush_assistant()
