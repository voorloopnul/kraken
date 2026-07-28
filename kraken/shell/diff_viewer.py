"""One file's diff, syntax highlighted, in a modal over the dimmed app.

The viewer is a child widget of the window rather than a top-level dialog: the
app is frameless, and an in-window sheet keeps its own chrome instead of asking
the window manager for a second decorated window. Covering the window also *is*
the modality — every click and wheel event lands on the scrim — and it lets the
dimming be a plain paint over the app rather than a compositor trick.

Lines are colored from the file they belong to, not from the diff line alone.
A lexer handed one line at a time has no idea it is inside a docstring or a
multi-line string, and colors it as code; so each side of the diff is lexed as
a whole document and the hunks index into the result by line number.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, replace

from PySide6.QtCore import (
    QEasingCurve,
    QEvent,
    QPropertyAnimation,
    QSize,
    Property,
    Qt,
)
from PySide6.QtGui import (
    QColor,
    QFont,
    QKeySequence,
    QPainter,
    QShortcut,
    QTextBlockFormat,
    QTextCharFormat,
    QTextCursor,
)
from PySide6.QtWidgets import (
    QGraphicsOpacityEffect,
    QHBoxLayout,
    QLabel,
    QPlainTextEdit,
    QSizePolicy,
    QToolButton,
    QWidget,
)

from kraken.ui.chrome import SCROLLBAR_STYLES, WINDOW_RADIUS, Card
from kraken.ui.fonts import MONO_FAMILY
from kraken.ui.highlight import TOKEN_COLORS, lexer_for_filename, resolve_token
from kraken.ui.themes import UI_COLORS

# Row text and the tints behind changed lines. The tints are the card
# background nudged toward green/red, so they read as a wash over the code
# rather than a block of color competing with the syntax highlighting.
_COLORS = {
    "dark": {
        "text": "#c8cad0",
        "dim": "#7a7d85",
        "add": "#98c379",
        "del": "#e06c75",
        "add_bg": "#2b3a2e",
        "del_bg": "#3a2b30",
        "hunk_bg": "#2f333c",
        "gutter": "#6b6e77",
        "scrim": QColor(0, 0, 0, 165),
    },
    "light": {
        "text": "#383a42",
        "dim": "#9a9da5",
        "add": "#3c7d3b",
        "del": "#c93c36",
        "add_bg": "#e9f6e9",
        "del_bg": "#fdeceb",
        "hunk_bg": "#eeeef2",
        "gutter": "#a9acb4",
        "scrim": QColor(24, 22, 18, 130),
    },
}

# Accent per status letter, drawn from the same One Half palette as the themes.
# The diff pane colors its rows from this too — one table, so a file reads the
# same in the list and in the sheet it opens.
LETTER_COLORS = {
    "dark": {
        "A": "#98c379", "?": "#98c379", "M": "#e5c07b", "D": "#e06c75",
        "R": "#61afef", "C": "#61afef", "T": "#c678dd", "U": "#e06c75",
    },
    "light": {
        "A": "#50a14f", "?": "#50a14f", "M": "#c18401", "D": "#e45649",
        "R": "#0184bc", "C": "#0184bc", "T": "#a626a4", "U": "#e45649",
    },
}

_HUNK_HEADER = re.compile(r"^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@")


@dataclass(frozen=True)
class DiffRow:
    """One line of the rendered diff. `old_no`/`new_no` are the line's number on
    each side, absent where it does not exist on that side."""

    kind: str  # "add" | "del" | "context" | "hunk" | "note"
    text: str  # the diff line, marker character included
    old_no: int | None = None
    new_no: int | None = None


@dataclass(frozen=True)
class DiffDocument:
    """Everything the viewer needs about one file. Plain data with no Qt in it,
    so a remote workspace can gather it on a worker thread.

    `old_text`/`new_text` are the whole file on each side, used only to lex it;
    either is empty when the file doesn't exist there (added, or deleted)."""

    path: str
    letter: str
    diff_text: str = ""
    old_text: str = ""
    new_text: str = ""
    subtitle: str = ""
    # Set when there is no text diff to show: a binary file, or a failure.
    message: str = ""


# A run of one token within a line: (start column, end column, token). The token
# rather than a color: the palette belongs to the theme, and a run that had a
# color baked into it would keep the old one through a theme flip — while the
# text around it changed, which is worse than not following at all.
Span = tuple[int, int, object]


def parse_diff(diff_text: str, max_rows: int = 4000) -> list[DiffRow]:
    """Rows from a unified diff. The file header is dropped, each `@@` header is
    kept as its own row, and every body line carries the line numbers it holds
    on each side — which is what lets a row be colored from its own file."""
    rows: list[DiffRow] = []
    old_no = new_no = 0
    in_body = False
    for line in diff_text.splitlines():
        if len(rows) >= max_rows:
            rows.append(DiffRow("note", "… diff truncated"))
            break
        header = _HUNK_HEADER.match(line)
        if header:
            old_no, new_no = int(header.group(1)), int(header.group(2))
            rows.append(DiffRow("hunk", line))
            in_body = True
            continue
        if not in_body:
            # git reports a binary change instead of a hunk; that line is the
            # only thing it will say about the file, so it is worth keeping.
            if line.startswith("Binary files ") or line.startswith("Binary file "):
                rows.append(DiffRow("note", line))
            continue
        if line.startswith("+"):
            rows.append(DiffRow("add", line, None, new_no))
            new_no += 1
        elif line.startswith("-"):
            rows.append(DiffRow("del", line, old_no, None))
            old_no += 1
        elif line.startswith("\\"):  # "\ No newline at end of file"
            rows.append(DiffRow("note", line))
        else:
            # Context. A body line is " text"; a stripped-down diff can leave a
            # blank line for a blank context line, which lands here too.
            rows.append(DiffRow("context", line, old_no, new_no))
            old_no += 1
            new_no += 1
    return rows


def line_spans(text: str, lexer) -> list[list[Span]]:
    """Token runs for every line of `text`, indexed from zero.

    Lexing the file whole and slicing the result per line is what keeps a hunk
    that starts inside a docstring from being colored as if it were code."""
    if lexer is None or not text:
        return []
    lines: list[list[Span]] = []
    current: list[Span] = []
    column = 0
    for _, token, value in lexer.get_tokens_unprocessed(text):
        for index, piece in enumerate(value.split("\n")):
            if index:  # the newline ended the line before this piece
                lines.append(current)
                current = []
                column = 0
            if not piece:
                continue
            # Whitespace carries no color, and skipping it keeps the runs from
            # painting a background-colored gap between two tokens.
            if piece.strip():
                current.append((column, column + len(piece), token))
            column += len(piece)
    lines.append(current)
    return lines


class _Gutter(QWidget):
    """The line-number margin. It is painted by the view (which owns the block
    geometry) and kept out of the document, so copying a diff line out of the
    viewer yields the line, not the line with numbers glued to its front."""

    def __init__(self, view: "_DiffText"):
        super().__init__(view)
        self._view = view

    def sizeHint(self) -> QSize:
        return QSize(self._view.gutter_width(), 0)

    def paintEvent(self, event) -> None:
        self._view.paint_gutter(event)


class _DiffText(QPlainTextEdit):
    """The diff body: one block per row, tinted by kind, with syntax colors
    inside it. QPlainTextEdit rather than a rich-text view because a diff is a
    long list of unwrapped lines, which is exactly what it is built for."""

    def __init__(self, theme: str, parent: QWidget | None = None):
        super().__init__(parent)
        self._theme = theme
        self._rows: list[DiffRow] = []
        self._old_spans: list[list[Span]] = []
        self._new_spans: list[list[Span]] = []
        self._digits = 3
        self.setReadOnly(True)
        self.setLineWrapMode(QPlainTextEdit.LineWrapMode.NoWrap)
        self.setFrameShape(QPlainTextEdit.Shape.NoFrame)
        self.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse
            | Qt.TextInteractionFlag.TextSelectableByKeyboard
        )
        # Keyboard scrolling moves the cursor, so it stays selectable — but a
        # blinking caret in a viewer reads as an invitation to type.
        self.setCursorWidth(0)
        font = QFont(MONO_FAMILY)
        font.setPixelSize(12)
        self.setFont(font)
        self.setTabStopDistance(4 * self.fontMetrics().horizontalAdvance(" "))
        self._gutter = _Gutter(self)
        self.updateRequest.connect(self._on_update_request)
        self._apply_theme()

    # ---- Content ----------------------------------------------------------

    def set_rows(
        self,
        rows: list[DiffRow],
        old_spans: list[list[Span]],
        new_spans: list[list[Span]],
    ) -> None:
        colors = _COLORS[self._theme]
        self._rows = rows
        self._old_spans = old_spans
        self._new_spans = new_spans
        # The widest number either side will show. Taken over both sides
        # together: a large insertion pushes the new side's numbers well past the
        # old side's, and the gutter has to fit whichever ends up longer.
        widest = max(
            (n for row in rows for n in (row.old_no, row.new_no) if n is not None),
            default=0,
        )
        self._digits = max(2, len(str(widest)))
        base = QTextCharFormat()
        base.setForeground(QColor(colors["text"]))
        marks = {
            "add": self._mark_format(colors["add"]),
            "del": self._mark_format(colors["del"]),
            "hunk": self._mark_format(colors["dim"]),
            "note": self._mark_format(colors["dim"]),
        }
        table = TOKEN_COLORS[self._theme]
        tints = {
            "add": colors["add_bg"],
            "del": colors["del_bg"],
            "hunk": colors["hunk_bg"],
        }
        self.clear()
        cursor = QTextCursor(self.document())
        cursor.beginEditBlock()
        for index, row in enumerate(rows):
            if index:
                cursor.insertBlock()
            block_format = QTextBlockFormat()
            if row.kind in tints:
                block_format.setBackground(QColor(tints[row.kind]))
            cursor.setBlockFormat(block_format)
            if row.kind in ("hunk", "note"):
                # Structural lines carry no code to highlight.
                cursor.insertText(row.text, marks[row.kind])
                continue
            spans = _spans_for(row, old_spans, new_spans)
            self._insert_code(
                cursor, row, spans, base, marks.get(row.kind, base), table
            )
        cursor.endEditBlock()
        self.moveCursor(QTextCursor.MoveOperation.Start)
        self._sync_gutter_width()

    def _mark_format(self, color: str) -> QTextCharFormat:
        fmt = QTextCharFormat()
        fmt.setForeground(QColor(color))
        return fmt

    @staticmethod
    def _insert_code(
        cursor: QTextCursor,
        row: DiffRow,
        spans: list[Span],
        base: QTextCharFormat,
        marker: QTextCharFormat,
        table: dict,
    ) -> None:
        """Write one code row: its +/-/space marker, then the line itself split
        into the runs the lexer found, colored for the current theme. Inserting
        the runs as we go costs one pass; re-formatting afterwards would mean
        seeking back over every line of the diff."""
        cursor.insertText(row.text[:1], marker)
        body = row.text[1:]
        column = 0
        for start, end, token in spans:
            if start >= len(body):
                break
            style = resolve_token(table, token)
            if style is None:
                continue  # no color for this token; the base run below covers it
            color, italic = style
            if start > column:
                cursor.insertText(body[column:start], base)
            run = QTextCharFormat(base)
            run.setForeground(QColor(color))
            run.setFontItalic(italic)
            cursor.insertText(body[start:end], run)
            column = min(end, len(body))
        if column < len(body):
            cursor.insertText(body[column:], base)

    def content_height(self) -> int:
        """Pixel height the whole diff occupies. Lines never wrap here, so one
        block is one line and the arithmetic is exact."""
        return (
            self.blockCount() * self.fontMetrics().lineSpacing()
            + 2 * int(self.document().documentMargin())
            + 4
        )

    # ---- Gutter -----------------------------------------------------------

    def gutter_width(self) -> int:
        # Two numbers side by side (old, new), each `_digits` wide, plus padding.
        return 10 + 2 * (self._digits * self.fontMetrics().horizontalAdvance("9") + 8)

    def _sync_gutter_width(self) -> None:
        width = self.gutter_width()
        self.setViewportMargins(width, 0, 0, 0)
        rect = self.contentsRect()
        self._gutter.setGeometry(rect.x(), rect.y(), width, rect.height())
        self._gutter.update()

    def _on_update_request(self, rect, dy: int) -> None:
        if dy:
            self._gutter.scroll(0, dy)
        else:
            self._gutter.update(0, rect.y(), self._gutter.width(), rect.height())

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._sync_gutter_width()

    def paint_gutter(self, event) -> None:
        colors = _COLORS[self._theme]
        painter = QPainter(self._gutter)
        painter.setFont(self.font())
        painter.setPen(QColor(colors["gutter"]))
        column = (self._gutter.width() - 10) // 2
        block = self.firstVisibleBlock()
        number = block.blockNumber()
        top = self.blockBoundingGeometry(block).translated(self.contentOffset()).top()
        while block.isValid() and top <= event.rect().bottom():
            height = self.blockBoundingRect(block).height()
            if block.isVisible() and top + height >= event.rect().top():
                row = self._rows[number] if number < len(self._rows) else None
                if row is not None:
                    flags = Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter
                    for slot, value in ((0, row.old_no), (1, row.new_no)):
                        if value is None:
                            continue
                        painter.drawText(
                            slot * column, int(top), column - 6, int(height), flags,
                            str(value),
                        )
            block = block.next()
            number += 1
            top += height

    # ---- Theme ------------------------------------------------------------

    def set_theme(self, name: str) -> None:
        """Re-color for a theme flip. Row colors and tints are written into the
        document as formats, so it is built again from the rows and spans already
        in hand — the file is not lexed twice. The scroll position is kept so the
        reader does not lose their place."""
        self._theme = name
        self._apply_theme()
        position = self.verticalScrollBar().value()
        self.set_rows(self._rows, self._old_spans, self._new_spans)
        self.verticalScrollBar().setValue(position)

    def _apply_theme(self) -> None:
        colors = _COLORS[self._theme]
        self.setStyleSheet(
            f"QPlainTextEdit {{ background: transparent; border: none;"
            f" color: {colors['text']}; }}" + SCROLLBAR_STYLES[self._theme]
        )


def _spans_for(
    row: DiffRow, old_spans: list[list[Span]], new_spans: list[list[Span]]
) -> list[Span]:
    """The color runs for a row, taken from whichever side it belongs to. A
    removed line only exists in the old file; everything else is read from the
    new one. Out-of-range means the file moved under us between the two git
    calls, and the row simply renders uncolored."""
    table, number = (
        (old_spans, row.old_no) if row.kind == "del" else (new_spans, row.new_no)
    )
    if number is None or not 0 < number <= len(table):
        return []
    return table[number - 1]


class _ElidedLabel(QLabel):
    """A one-line label that shortens its text to the width it is handed, so a
    long path elides instead of stretching the header row."""

    def __init__(self, mode=Qt.TextElideMode.ElideLeft, parent: QWidget | None = None):
        super().__init__(parent)
        self._mode = mode
        self._full = ""
        # Ignored horizontally: the label takes the width the row has left over
        # and never reports one of its own, which is also what keeps re-eliding
        # on resize from feeding a new size hint back into the layout.
        self.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Preferred)

    def set_full_text(self, text: str) -> None:
        self._full = text
        self.setToolTip(text)
        self._elide()

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._elide()

    def _elide(self) -> None:
        self.setText(
            self.fontMetrics().elidedText(self._full, self._mode, max(self.width(), 40))
        )


class DiffViewer(QWidget):
    """The modal sheet: a scrim over the whole window with the diff card on top.

    Closes on Esc, on the ✕, and on a click in the scrim — the three things a
    reader reaches for. Created with `open_on()`, which also runs the fade in."""

    _MARGIN = 28
    _MAX_WIDTH = 1200
    _FADE_MS = 140

    def __init__(self, host: QWidget, theme: str, document: DiffDocument):
        super().__init__(host)
        self._host = host
        self._theme = theme
        self._document = document
        self._fade = 0.0
        self._closing = False
        self._animation: QPropertyAnimation | None = None
        # Focusable in its own right, for the sheets that have no diff body to
        # give focus to (a binary file, or a diff that could not be read).
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

        # The sheet is a card without the drop shadow the panels give theirs. A
        # widget gets one graphics effect, and the fade needs it: nesting an
        # opacity effect over a shadow makes Qt render one effect inside the
        # other's active painter, which it warns about and paints wrong. Over
        # the scrim a shadow would separate nothing anyway.
        card = Card(self, shadow=False)
        self._sheet = card
        self._opacity = QGraphicsOpacityEffect(card)
        self._opacity.setOpacity(0.0)
        card.setGraphicsEffect(self._opacity)

        self._letter_label = QLabel(document.letter)
        self._path_label = _ElidedLabel()
        self._path_label.set_full_text(document.path)
        self._subtitle_label = QLabel(document.subtitle)
        self._hint_label = QLabel("Esc")
        self._close_button = QToolButton()
        self._close_button.setText("✕")
        self._close_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self._close_button.setToolTip("Close")
        self._close_button.clicked.connect(self.close_view)

        header = QHBoxLayout()
        header.setContentsMargins(2, 0, 2, 6)
        header.setSpacing(8)
        header.addWidget(self._letter_label)
        header.addWidget(self._path_label, stretch=1)
        header.addWidget(self._subtitle_label)
        header.addWidget(self._hint_label)
        header.addWidget(self._close_button)
        self._header_row = QWidget()
        self._header_row.setLayout(header)
        card.add_widget(self._header_row)

        rows = parse_diff(document.diff_text)
        # A diff of nothing but notes ("Binary files ... differ") has no lines to
        # lay out, and git's own sentence about it is the whole story — so it is
        # shown as the message rather than as a one-row body.
        notes = [row.text for row in rows if row.kind == "note"]
        if rows and len(notes) == len(rows):
            document = replace(document, message=" ".join(notes))
            rows = []
        self._message_label: QLabel | None = None
        self._body: _DiffText | None = None
        if document.message or not rows:
            self._message_label = QLabel(document.message or "No textual changes")
            self._message_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
            card.add_widget(self._message_label, stretch=1)
        else:
            # Lexed once, on the way in: the spans are line/column runs over
            # tokens, so a theme flip re-colors them without lexing again.
            lexer = lexer_for_filename(document.path)
            self._body = _DiffText(theme)
            self._body.set_rows(
                rows,
                line_spans(document.old_text, lexer),
                line_spans(document.new_text, lexer),
            )
            card.add_widget(self._body, stretch=1)
        self._apply_theme()

        # Esc from anywhere inside the sheet, including the focused diff body.
        shortcut = QShortcut(QKeySequence(Qt.Key.Key_Escape), self)
        shortcut.setContext(Qt.ShortcutContext.WidgetWithChildrenShortcut)
        shortcut.activated.connect(self.close_view)

        self.setGeometry(host.rect())
        # The window can be resized under the sheet (a tiling shortcut, say);
        # Qt drops this filter for us when the sheet is destroyed.
        host.installEventFilter(self)

    # ---- Theme ------------------------------------------------------------

    def set_theme(self, name: str) -> None:
        """Follow the app's theme toggle. The sheet is modal, so the toggle can
        only be reached once it has closed — but a theme change while it is open
        must not leave it the only surface in the old colors."""
        if name == self._theme:
            return
        self._theme = name
        self._apply_theme()

    def _apply_theme(self) -> None:
        colors = _COLORS[self._theme]
        ui = UI_COLORS[self._theme]
        # Without this the sheet keeps Card's built-in light default, which is
        # the app's light card color whatever the theme says.
        self._sheet.set_colors(ui["card"], ui["card_border"])
        accent = LETTER_COLORS[self._theme].get(
            self._document.letter, colors["text"]
        )
        self._letter_label.setStyleSheet(
            f"color: {accent}; font-family: '{MONO_FAMILY}'; font-weight: bold;"
        )
        self._path_label.setStyleSheet(
            f"color: {colors['text']}; font-family: '{MONO_FAMILY}';"
        )
        self._subtitle_label.setStyleSheet(f"color: {colors['dim']};")
        self._hint_label.setStyleSheet(f"color: {colors['dim']}; font-size: 11px;")
        self._close_button.setStyleSheet(
            f"QToolButton {{ background: transparent; border: none;"
            f" border-radius: 4px; color: {colors['dim']}; font-size: 13px;"
            f" padding: 0 6px; }}"
            f" QToolButton:hover {{ color: {colors['text']}; }}"
        )
        if self._message_label is not None:
            self._message_label.setStyleSheet(f"color: {colors['dim']};")
        if self._body is not None:
            self._body.set_theme(self._theme)
        self.update()  # the scrim is painted from the same table

    # ---- Open / close -----------------------------------------------------

    @classmethod
    def open_on(
        cls, host: QWidget, theme: str, document: DiffDocument
    ) -> "DiffViewer":
        viewer = cls(host, theme, document)
        viewer.show()
        viewer.raise_()
        # Something inside the sheet must hold focus: the Esc shortcut is scoped
        # to the sheet and its children, and a sheet that took no focus would
        # leave keystrokes going to the app behind the scrim.
        (viewer._body or viewer).setFocus()
        viewer._animate_to(1.0)
        return viewer

    def close_view(self) -> None:
        if self._closing:
            return
        self._closing = True
        self._opacity.setEnabled(True)
        self._animate_to(0.0, on_done=self.deleteLater)

    def _animate_to(self, end: float, on_done=None) -> None:
        animation = QPropertyAnimation(self, b"fade", self)
        animation.setDuration(self._FADE_MS)
        animation.setStartValue(self._fade)
        animation.setEndValue(end)
        animation.setEasingCurve(QEasingCurve.Type.InOutQuad)
        if on_done is not None:
            animation.finished.connect(on_done)
        elif end >= 1.0:
            # Fully faded in: take the effect back out of the picture. Leaving it
            # installed would keep the whole card painting through the effect's
            # cached pixmap for as long as the viewer is open.
            animation.finished.connect(lambda: self._opacity.setEnabled(False))
        animation.start()
        self._animation = animation

    def _get_fade(self) -> float:
        return self._fade

    def _set_fade(self, value: float) -> None:
        self._fade = value
        self._opacity.setOpacity(value)
        self.update()  # the scrim's alpha follows the same value

    fade = Property(float, _get_fade, _set_fade)

    # ---- Geometry / input -------------------------------------------------

    def eventFilter(self, watched, event) -> bool:
        if watched is self._host and event.type() == QEvent.Type.Resize:
            self.setGeometry(self._host.rect())
        return super().eventFilter(watched, event)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        width = min(self._MAX_WIDTH, max(self.width() - 2 * self._MARGIN, 240))
        height = self._wanted_height(max(self.height() - 2 * self._MARGIN, 160))
        self._sheet.setGeometry(
            (self.width() - width) // 2, (self.height() - height) // 2, width, height
        )

    def _wanted_height(self, available: int) -> int:
        """How tall the sheet should be: enough for the whole diff, capped by the
        window. A three-line change gets a sheet the size of a three-line change
        rather than a full-height card with an empty half."""
        if self._body is None:
            return min(available, 200)
        # Card padding above and below, plus the header row and its bottom gap.
        chrome = 24 + self._header_row.sizeHint().height() + 6
        return min(available, max(240, self._body.content_height() + chrome))

    def paintEvent(self, event) -> None:
        scrim = QColor(_COLORS[self._theme]["scrim"])
        scrim.setAlpha(int(scrim.alpha() * self._fade))
        painter = QPainter(self)
        # Rounded like the window it covers. Filling the plain rect would paint
        # the corners the frame deliberately leaves empty, squaring the window
        # off for as long as a diff is open — and over the desktop rather than
        # over the app, since those pixels are transparent.
        radius = 0 if self._host.isMaximized() else WINDOW_RADIUS
        if radius:
            painter.setRenderHint(QPainter.RenderHint.Antialiasing)
            painter.setPen(Qt.PenStyle.NoPen)
            painter.setBrush(scrim)
            painter.drawRoundedRect(self.rect(), radius, radius)
        else:
            painter.fillRect(self.rect(), scrim)
        painter.end()

    def mousePressEvent(self, event) -> None:
        # Only scrim clicks reach here; anything on the card is the card's.
        self.close_view()
        event.accept()

    def wheelEvent(self, event) -> None:
        # Swallow scrolling so the panes behind the scrim stay put.
        event.accept()
