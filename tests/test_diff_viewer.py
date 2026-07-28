"""The diff viewer: the sheet that opens on a row of the diff pane.

The parsing tests pin the two properties everything else rests on — that a row
knows its line number on each side, and that a line is colored from the file it
belongs to rather than from itself. The widget tests drive the real sheet: it is
built around a QPlainTextEdit whose block formats carry the add/remove tints, so
asserting on the document is asserting on what is drawn.
"""

import pytest
from PySide6.QtGui import QColor
from pygments.token import Token

from kraken.shell.diff_viewer import (
    DiffDocument,
    DiffViewer,
    line_spans,
    parse_diff,
)
from kraken.ui.highlight import TOKEN_COLORS, lexer_for_filename, resolve_token

# Built line by line so the marker column survives: a blank context line is a
# single space in a real diff, which any whitespace-trimming tool would eat out
# of a triple-quoted string.
DIFF = "\n".join(
    [
        "diff --git a/app.py b/app.py",
        "index 1234567..89abcde 100644",
        "--- a/app.py",
        "+++ b/app.py",
        "@@ -3,6 +3,7 @@ import os",
        " def start(name):",
        "     value = 1",
        "-    return value",
        "+    value += 1",
        "+    return value * 2",
        " ",
        " def stop():",
    ]
) + "\n"


def kinds(rows):
    return [row.kind for row in rows]


def test_rows_carry_their_line_number_on_each_side():
    rows = parse_diff(DIFF)

    # The header is dropped; the hunk header stays as a row of its own.
    assert kinds(rows) == [
        "hunk", "context", "context", "del", "add", "add", "context", "context"
    ]
    numbered = [(r.kind, r.old_no, r.new_no) for r in rows[1:]]
    assert numbered == [
        ("context", 3, 3),
        ("context", 4, 4),
        ("del", 5, None),  # only in the old file
        ("add", None, 5),  # only in the new one
        ("add", None, 6),
        ("context", 6, 7),  # the sides have drifted apart by one
        ("context", 7, 8),
    ]


def test_binary_and_no_newline_lines_are_kept_as_notes():
    rows = parse_diff(
        "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n"
    )
    assert kinds(rows) == ["note"]

    rows = parse_diff("@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n")
    assert kinds(rows) == ["hunk", "del", "add", "note"]


def test_a_blank_line_with_no_marker_still_counts_as_context():
    # git writes " " for a blank context line, but a diff that has been through
    # a whitespace-trimming tool arrives with the marker gone.
    rows = parse_diff("@@ -1,3 +1,3 @@\n one\n\n-two\n+three\n")

    assert kinds(rows) == ["hunk", "context", "context", "del", "add"]
    assert [(r.old_no, r.new_no) for r in rows[1:3]] == [(1, 1), (2, 2)]


def test_a_diff_past_the_row_cap_is_truncated():
    body = "".join(f"+line {i}\n" for i in range(50))
    rows = parse_diff(f"@@ -0,0 +1,50 @@\n{body}", max_rows=10)

    assert len(rows) == 11  # the cap, plus the note that says so
    assert rows[-1].kind == "note"
    assert "truncated" in rows[-1].text


# The claim worth a test of its own: a line is lexed with the file above it in
# hand, so text inside a docstring is a string even when it reads like code.
SOURCE = '''"""A docstring.

It mentions def stop() and class Thing, which are prose here.
"""

def stop():
    pass
'''


def test_a_line_inside_a_docstring_is_lexed_as_a_string():
    spans = line_spans(SOURCE, lexer_for_filename("app.py"))

    # Line 3 is the prose line: every run on it is part of the string, even the
    # ones spelling "def" and "class".
    prose = spans[2]
    assert prose, "the docstring line should carry tokens"
    assert all(token in Token.Literal.String for _, _, token in prose)

    # Line 6 is the real def, and it is a keyword there.
    real_def = spans[5]
    assert any(token in Token.Keyword for _, _, token in real_def)


def test_spans_carry_tokens_rather_than_colors():
    """A run holds the token, so the palette is applied at render time. Colors
    baked in here would survive a theme flip that re-colored everything around
    them — the sheet's own rebuild reuses these spans instead of lexing again."""
    spans = line_spans(SOURCE, lexer_for_filename("app.py"))

    tokens = {token for runs in spans for _, _, token in runs}
    assert tokens, "the source should lex into something"
    for token in tokens:
        assert not isinstance(token, str)
        # Every token resolves through the palette independently per theme.
        for theme in ("dark", "light"):
            resolved = resolve_token(TOKEN_COLORS[theme], token)
            assert resolved is None or resolved[0].startswith("#")


def test_spans_are_columns_within_their_own_line():
    spans = line_spans(SOURCE, lexer_for_filename("app.py"))

    for line_number, runs in enumerate(spans, start=1):
        for start, end, _token in runs:
            assert 0 <= start < end
            # A run never reaches past the end of the line it belongs to.
            assert end <= len(SOURCE.splitlines()[line_number - 1])


@pytest.fixture
def host(qapp, settle):
    """A window-sized widget for the sheet to cover, like the real window."""
    from PySide6.QtWidgets import QWidget

    widget = QWidget()
    widget.resize(1000, 700)
    widget.show()
    settle()
    yield widget
    widget.close()
    widget.deleteLater()


def open_viewer(host, settle, document, theme="dark"):
    viewer = DiffViewer.open_on(host, theme, document)
    settle(300)  # let the fade finish
    return viewer


def document_for(diff=DIFF, **kwargs):
    return DiffDocument(
        path=kwargs.pop("path", "app.py"),
        letter=kwargs.pop("letter", "M"),
        diff_text=diff,
        **kwargs,
    )


def block_texts(viewer):
    document = viewer._body.document()
    return [
        document.findBlockByNumber(i).text() for i in range(document.blockCount())
    ]


def test_the_sheet_renders_one_block_per_diff_row(host, settle):
    viewer = open_viewer(host, settle, document_for())

    assert block_texts(viewer) == [
        "@@ -3,6 +3,7 @@ import os",
        " def start(name):",
        "     value = 1",
        "-    return value",
        "+    value += 1",
        "+    return value * 2",
        " ",
        " def stop():",
    ]


def test_added_and_removed_rows_are_tinted_apart(host, settle):
    from PySide6.QtCore import Qt

    viewer = open_viewer(host, settle, document_for())
    document = viewer._body.document()

    def brush(index):
        return document.findBlockByNumber(index).blockFormat().background()

    added, removed = brush(4).color(), brush(3).color()
    assert QColor(added).green() > QColor(added).red()  # green wash on additions
    assert QColor(removed).red() > QColor(removed).green()  # red on removals
    # A context row is left untinted, so the card shows through it.
    assert brush(2).style() == Qt.BrushStyle.NoBrush


def test_the_gutter_fits_the_longest_number_on_either_side(host, settle):
    # One context line, then a large insertion after it: the old side never gets
    # past 1 while the new side runs into four digits. The context row is the
    # only one carrying both numbers, so sizing the gutter from the widest *row*
    # rather than the widest number would settle on 1 and clip the rest.
    body = "".join(f"+line {i}\n" for i in range(1199))
    viewer = open_viewer(
        host, settle, document_for(f"@@ -1,1 +1,1200 @@\n context\n{body}")
    )

    assert viewer._body._digits == 4


def test_the_sheet_is_built_in_the_theme_it_is_given(host, settle):
    """The card's own colors, not just the text on it. Card ships with a light
    default and keeps it unless it is told otherwise, so a sheet that forgot to
    say leaves a light panel behind dark-theme code."""
    from kraken.ui.themes import UI_COLORS

    for theme in ("dark", "light"):
        viewer = open_viewer(host, settle, document_for(), theme=theme)
        assert UI_COLORS[theme]["card"].lower() in viewer._sheet.styleSheet().lower()
        viewer.close_view()
        settle(300)


def test_the_sheet_follows_a_theme_toggle_while_it_is_open(host, settle):
    from kraken.ui.themes import UI_COLORS

    viewer = open_viewer(host, settle, document_for(), theme="light")
    before = viewer._body.document().findBlockByNumber(4).blockFormat()

    viewer.set_theme("dark")
    settle()

    assert UI_COLORS["dark"]["card"].lower() in viewer._sheet.styleSheet().lower()
    # The row tints are formats inside the document, so they have to be rebuilt.
    after = viewer._body.document().findBlockByNumber(4).blockFormat()
    assert after.background().color() != before.background().color()
    # And the diff itself survives the rebuild.
    assert block_texts(viewer)[4] == "+    value += 1"


def row_colors(viewer, index):
    """The foreground colors along one rendered row, run by run."""
    block = viewer._body.document().findBlockByNumber(index)
    return [run.format.foreground().color().name() for run in block.textFormats()]


def test_syntax_colors_follow_a_theme_toggle(host, settle):
    """The regression the first theme fix missed: the row tints and base text
    changed while the keywords kept the palette they were lexed under, leaving
    light-theme purple on a dark card."""
    source = "import os\nvalue = 1\n"
    diff = "@@ -1,2 +1,2 @@\n import os\n-value = 0\n+value = 1\n"
    viewer = open_viewer(
        host,
        settle,
        document_for(diff, old_text=source, new_text=source),
        theme="light",
    )

    def keyword_color(theme):
        return resolve_token(TOKEN_COLORS[theme], Token.Keyword)[0]

    # Row 1 is " import os": the keyword is colored from the light palette.
    assert keyword_color("light") in row_colors(viewer, 1)

    viewer.set_theme("dark")
    settle()

    assert keyword_color("dark") in row_colors(viewer, 1)
    assert keyword_color("light") not in row_colors(viewer, 1)


def test_the_sheet_keeps_the_readers_place_across_a_theme_toggle(host, settle):
    body = "".join(f"+line {i}\n" for i in range(400))
    viewer = open_viewer(host, settle, document_for(f"@@ -0,0 +1,400 @@\n{body}"))
    bar = viewer._body.verticalScrollBar()
    bar.setValue(bar.maximum() // 2)
    where = bar.value()
    assert where > 0

    viewer.set_theme("light")
    settle()

    assert viewer._body.verticalScrollBar().value() == where


def test_the_sheet_covers_its_host_and_fades_in(host, settle):
    viewer = open_viewer(host, settle, document_for())

    assert viewer.geometry() == host.rect()
    assert viewer._fade == pytest.approx(1.0)
    # The card is inset from the scrim on every side, and centred in it.
    sheet = viewer._sheet.geometry()
    assert sheet.left() > 0 and sheet.right() < viewer.width()
    assert abs(sheet.center().x() - viewer.rect().center().x()) <= 1


def test_the_sheet_follows_a_resize_of_the_window(host, settle):
    viewer = open_viewer(host, settle, document_for())

    host.resize(1300, 900)
    settle()

    assert viewer.geometry() == host.rect()


def test_a_short_diff_gets_a_sheet_the_size_of_the_diff(host, settle):
    short = open_viewer(host, settle, document_for())
    height = short._sheet.height()
    short.close_view()
    settle(300)

    long_body = "".join(f"+line {i}\n" for i in range(400))
    tall = open_viewer(host, settle, document_for(f"@@ -0,0 +1,400 @@\n{long_body}"))

    assert height < tall._sheet.height()
    # And the tall one still stays inside the window.
    assert tall._sheet.height() <= host.height()


def test_a_message_shows_instead_of_a_body_when_there_is_no_text_diff(host, settle):
    viewer = open_viewer(
        host,
        settle,
        DiffDocument(path="logo.png", letter="?", message="Binary file — no text diff"),
    )

    assert viewer._body is None


def test_a_message_only_sheet_is_still_modal(host, settle):
    """It has no diff body to hand focus to, and Esc is scoped to the sheet and
    its children — so a sheet that took no focus would advertise "Esc" while
    leaving the key, and every other keystroke, going to the app behind it."""
    from PySide6.QtCore import Qt
    from PySide6.QtGui import QKeyEvent
    from PySide6.QtWidgets import QApplication

    viewer = open_viewer(
        host, settle, DiffDocument(path="logo.png", letter="?", message="Binary file")
    )

    focused = QApplication.focusWidget()
    assert focused is viewer or (focused is not None and viewer.isAncestorOf(focused))

    closed = []
    viewer.destroyed.connect(lambda *_: closed.append(True))
    QApplication.sendEvent(
        focused,
        QKeyEvent(QKeyEvent.Type.KeyPress, Qt.Key.Key_Escape, Qt.KeyboardModifier.NoModifier),
    )
    settle(300)

    assert closed == [True], "Esc should close a sheet that has no diff body"


def test_a_diff_that_is_only_a_binary_note_reads_as_a_message(host, settle):
    # git says this and nothing else about a binary change; one row of body for
    # it would be a worse way to show the same sentence.
    viewer = open_viewer(
        host,
        settle,
        document_for("diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n"),
    )

    assert viewer._body is None
    assert "Binary files" in viewer._message_label.text()


def test_closing_fades_out_and_deletes_the_sheet(host, settle):
    viewer = open_viewer(host, settle, document_for())
    closed = []
    viewer.destroyed.connect(lambda *_: closed.append(True))

    viewer.close_view()
    settle(300)

    assert closed == [True]


def test_a_click_on_the_scrim_closes_the_sheet(host, settle):
    from PySide6.QtCore import QEvent, QPointF, Qt
    from PySide6.QtGui import QMouseEvent

    viewer = open_viewer(host, settle, document_for())
    closed = []
    viewer.destroyed.connect(lambda *_: closed.append(True))

    # The top-left corner is scrim: the card is inset from every edge.
    corner = QPointF(4, 4)
    viewer.mousePressEvent(
        QMouseEvent(
            QEvent.Type.MouseButtonPress,
            corner,
            corner,
            Qt.MouseButton.LeftButton,
            Qt.MouseButton.LeftButton,
            Qt.KeyboardModifier.NoModifier,
        )
    )
    settle(300)

    assert closed == [True]
