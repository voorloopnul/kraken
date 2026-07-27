"""The prompt box grows with its content instead of scrolling.

Height is asserted through "does this need a scrollbar", not through pixel
counts: the point of the feature is that typed lines stay visible, and a pixel
assertion would just re-encode the arithmetic under test.

The box is exercised inside a host widget with room to spare, because that is
the shape of the real layout — the prompt sits in a panel that can yield space
to it. Sizing the ChatInput directly would cap growth at whatever height the
test happened to pick and prove nothing about the feature.
"""

import pytest
from PySide6.QtWidgets import QVBoxLayout, QWidget

from kraken.chat.chat_input import ChatInput


@pytest.fixture
def host(qapp, settle):
    container = QWidget()
    container.resize(700, 600)
    layout = QVBoxLayout(container)
    layout.addStretch(1)
    layout.addWidget(ChatInput())
    container.show()
    settle()
    yield container
    container.close()
    container.deleteLater()


@pytest.fixture
def chat(host):
    return host.findChild(ChatInput)


def edit(chat):
    return chat._edit


def needs_scroll(chat) -> bool:
    return edit(chat).verticalScrollBar().maximum() > 0


def laid_out_height(box) -> float:
    """The pixel height Qt actually gave the document, margins included."""
    layout = box.document().documentLayout()
    total = 2 * box.document().documentMargin()
    block = box.document().firstBlock()
    while block.isValid():
        total += layout.blockBoundingRect(block).height()
        block = block.next()
    return total


def test_empty_box_keeps_its_resting_height(chat, settle):
    assert edit(chat).height() == edit(chat)._MIN_HEIGHT
    assert not needs_scroll(chat)


def test_a_short_prompt_does_not_change_the_height(chat, settle):
    resting = edit(chat).height()
    edit(chat).setPlainText("one line")
    settle()

    assert edit(chat).height() == resting
    assert not needs_scroll(chat)


def test_growing_line_by_line_never_needs_a_scrollbar(chat, settle):
    previous = edit(chat).height()
    for count in range(2, 13):
        edit(chat).setPlainText("\n".join(f"line{i}" for i in range(count)))
        settle()
        assert not needs_scroll(chat), f"{count} lines did not fit"
        assert edit(chat).height() >= previous, "the box shrank while gaining a line"
        previous = edit(chat).height()
    assert previous > edit(chat)._MIN_HEIGHT, "the box never grew at all"


def test_wrapped_text_grows_the_box_too(chat, settle):
    """One logical line long enough to wrap several times: the height has to
    come from laid-out lines, not from counting newlines."""
    edit(chat).setPlainText("wrap " * 400)
    settle()

    assert edit(chat).height() > edit(chat)._MIN_HEIGHT
    assert edit(chat).height() <= edit(chat)._MAX_HEIGHT


def test_growth_stops_at_the_cap_and_scrolls_instead(chat, settle):
    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(200)))
    settle()

    assert edit(chat).height() == edit(chat)._MAX_HEIGHT
    assert needs_scroll(chat), "past the cap the box must scroll rather than grow"


def test_the_box_shrinks_back_when_the_text_goes(chat, settle):
    resting = edit(chat).height()
    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(20)))
    settle()
    assert edit(chat).height() > resting

    edit(chat).clear()
    settle()
    assert edit(chat).height() == resting


def test_submitting_returns_the_box_to_its_resting_height(chat, settle):
    """_submit clears the text; the box has to come back down with it, or the
    next prompt starts in a tall empty field."""
    resting = edit(chat).height()
    sent = []
    chat.submitted.connect(lambda text, images, files: sent.append(text))

    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(10)))
    settle()
    chat._submit()
    settle()

    assert sent, "the prompt was not submitted"
    assert edit(chat).height() == resting


def test_a_tall_box_is_not_a_floor_for_its_container(chat, host, settle):
    """setFixedHeight pinned the minimum as well as the maximum, making a long
    prompt a hard lower bound the layout could not shrink — enough to force the
    whole window taller on a short screen."""
    resting_minimum = host.minimumSizeHint().height()

    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(200)))
    settle()

    assert edit(chat).height() == edit(chat)._MAX_HEIGHT
    assert host.minimumSizeHint().height() == resting_minimum


def test_a_cramped_container_squeezes_the_box(chat, host, settle):
    """Given less room than it wants, the box gives way and scrolls rather than
    pushing its container open."""
    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(200)))
    settle()
    assert edit(chat).height() == edit(chat)._MAX_HEIGHT

    host.resize(700, 200)
    settle()

    assert edit(chat).height() < edit(chat)._MAX_HEIGHT
    assert edit(chat).height() >= edit(chat)._MIN_HEIGHT
    assert needs_scroll(chat)


def test_the_box_is_no_taller_than_its_content_needs(chat, settle):
    """Chrome is measured off the widget and content summed from the laid-out
    blocks. Deriving either — frameWidth() plus contentsMargins() double-counts
    the stylesheet padding, and a line count times lineSpacing is not what Qt
    actually spends — was wrong in both directions at once."""
    edit(chat).setPlainText("\n".join(f"line{i}" for i in range(6)))
    settle()
    box = edit(chat)

    outside_viewport = box.height() - box.viewport().height()

    assert box.height() == pytest.approx(laid_out_height(box) + outside_viewport, abs=1)


def test_narrowing_the_box_rewraps_and_regrows(chat, host, settle):
    """Shrinking the width turns one line into several; the height has to
    follow or the tail of the text is hidden."""
    edit(chat).setPlainText("wrap " * 60)
    settle()
    wide = edit(chat).height()

    host.resize(300, 600)
    settle()

    assert edit(chat).height() >= wide
    assert not needs_scroll(chat)
