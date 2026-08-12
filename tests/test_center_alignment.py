"""The chat input sits directly under the transcript, and both sit centred.

The conversation pane centres two columns independently — the transcript in one
row, the input in another — and they line up only because both rows reserve the
same width for the scrollbar and divide the same remainder. Nothing in the
layout enforces that; it is two arithmetics that have to agree, and they have
drifted apart twice.

Once because the rows measured the scrollbar differently: the bar is styled to
10px and the row below reserved the platform's PM_ScrollBarExtent, 14. Once
because a default spacing falls between two widgets but not between a widget
and a spacer, so the row ending in the scrollbar divided six pixels less than
the row ending in a spacer. Both are invisible in the code and obvious on
screen, which is what this covers.
"""

import pytest

from kraken.shell.panels.center import CenterPanel

# Comfortably past MAX_CONTENT_WIDTH, so the columns are capped and centred
# rather than simply filling the panel.
PANEL_WIDTH = CenterPanel.MAX_CONTENT_WIDTH + 200


@pytest.fixture
def panel(qapp, settle):
    widget = CenterPanel()
    widget.resize(PANEL_WIDTH, 700)
    widget.show()
    settle()
    yield widget
    widget.hide()
    widget.deleteLater()


def span(panel: CenterPanel, widget) -> tuple[int, int]:
    """`widget`'s left and right edge in the panel's own coordinates."""
    left = widget.mapTo(panel, widget.rect().topLeft()).x()
    return left, left + widget.width()


def test_the_input_lines_up_under_the_transcript(panel):
    assert span(panel, panel.conversation_stack) == span(panel, panel.chat)


def test_the_column_is_centred_on_the_panel(panel):
    """The scrollbar's width is reserved whether or not it is showing, so it
    has to be reserved on both sides — held on one, it pushes everything a
    scrollbar's width off centre."""
    left, right = span(panel, panel.chat)
    assert left == panel.width() - right


def test_the_gutters_match_the_scrollbar_that_is_actually_drawn(panel):
    """Every gutter is the bar's own width, not the platform metric — which
    says something else entirely, and was what the input used to be indented
    by. Measured on the spacers rather than on the column's offset, since that
    offset is the gutter plus whatever the centring stretches take."""
    expected = panel._scrollbar.sizeHint().width()
    assert panel._gutters, "the rows reserve nothing for the scrollbar"
    for item in panel._gutters:
        assert item.sizeHint().width() == expected


@pytest.mark.parametrize("theme", ("light", "dark"))
def test_the_columns_still_agree_after_a_theme_change(panel, settle, theme):
    """A theme restyles the scrollbar, which is what sets its width, so the
    gutters standing in for it have to be re-measured with it."""
    panel.set_theme(theme)
    settle()
    assert span(panel, panel.conversation_stack) == span(panel, panel.chat)
