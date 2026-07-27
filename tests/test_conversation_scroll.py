"""The transcript's sticky bottom: it follows new content while the reader is
at the end, and holds still once they scroll up to read.

Every test drives a real CenterPanel rather than a bare ConversationView. That
is not incidental. The panel hides the transcript's own scrollbar and mirrors
it onto one at the panel's edge, and it owns the busy row whose appearance
resizes the transcript's viewport. Both live outside the widget, and both are
involved in the bug this file exists to pin down — a ConversationView tested on
its own scrolls correctly even with the bug present.
"""

import time

import pytest

from kraken.chat.conversation import ConversationView
from kraken.shell.panels.center import CenterPanel

# Matches the view's own tolerance for "close enough to the bottom".
SLACK = 4


@pytest.fixture
def panel(qapp, settle):
    """A focused transcript wired up exactly as the running app wires it."""
    panel = CenterPanel()
    panel.resize(700, 500)
    view = ConversationView()
    panel.add_conversation(view)
    panel.set_focused_conversation(view)
    panel.show()
    settle()
    yield panel, view
    panel.close()
    panel.deleteLater()


def scrollbar(view):
    return view.verticalScrollBar()


def at_bottom(view) -> bool:
    bar = scrollbar(view)
    return bar.value() >= bar.maximum() - SLACK


def fill(view, settle, exchanges: int = 12) -> None:
    """Enough conversation to overflow the viewport several times over."""
    for i in range(exchanges):
        view.add_user(f"user message {i}")
        view.append_assistant_delta(f"reply {i}\n\nwith a second paragraph.\n")
    settle()


def test_follows_new_content_from_the_bottom(panel, settle):
    _panel, view = panel
    fill(view, settle)
    assert at_bottom(view)

    view.add_user("one more")
    settle()
    assert at_bottom(view)


def test_busy_row_does_not_strand_the_newest_message(panel, settle):
    """The regression: sending a prompt shows the busy row, which takes its
    height out of the transcript's viewport and grows the scroll range. Nothing
    painted, so a follow decision made only at paint time never fires and the
    just-sent message sits clipped at the bottom edge."""
    _panel_widget, view = panel
    fill(view, settle)

    view.add_user("tell me a joke")
    _panel_widget.set_busy(True, time.monotonic())
    settle()

    assert at_bottom(view), "the sent message was left under the busy row"


def test_reply_stays_in_view_while_streaming(panel, settle):
    """The other half of the regression: once stranded, every later paint read
    the gap as "the reader scrolled up", so the whole reply arrived off-screen."""
    _panel_widget, view = panel
    fill(view, settle)
    view.add_user("tell me a joke")
    _panel_widget.set_busy(True, time.monotonic())
    settle()

    for chunk in ["Why did ", "the chicken ", "cross the road?\n\n", "To get to " * 40]:
        view.append_assistant_delta(chunk)
        settle(20)  # deltas arrive over time, not in one burst
    assert at_bottom(view)

    _panel_widget.set_busy(False)
    settle()
    assert at_bottom(view), "the view fell behind when the busy row went away"


def test_large_delta_scrolls_all_the_way(panel, settle):
    """A single insert far taller than the viewport is where a stale maximum
    would show up most."""
    _panel, view = panel
    fill(view, settle)

    view.append_assistant_delta("\n\n" + "long tail paragraph. " * 400)
    settle()
    assert at_bottom(view)


def test_resizing_keeps_the_bottom(panel, settle):
    """Another range change with no paint behind it."""
    panel_widget, view = panel
    fill(view, settle)
    assert at_bottom(view)

    panel_widget.resize(700, 300)
    settle()
    assert at_bottom(view)


def test_growing_the_prompt_box_keeps_the_bottom(panel, settle):
    """The prompt box grows into the transcript's space as lines are typed —
    the same kind of range change as the busy row, from the other direction."""
    panel_widget, view = panel
    fill(view, settle)
    assert at_bottom(view)

    panel_widget.chat._edit.setPlainText("\n".join(f"line{i}" for i in range(10)))
    settle()

    assert at_bottom(view)


def test_scrolled_up_reader_is_not_yanked(panel, settle):
    _panel, view = panel
    fill(view, settle)
    held = scrollbar(view).maximum() // 2
    scrollbar(view).setValue(held)
    settle()

    view.add_user("this should not yank the view")
    view.append_assistant_delta("nor should this reply " * 50)
    settle()

    assert scrollbar(view).value() == held
    assert not at_bottom(view)


def test_following_resumes_after_returning_to_the_bottom(panel, settle):
    _panel, view = panel
    fill(view, settle)
    scrollbar(view).setValue(scrollbar(view).maximum() // 2)
    settle()

    scrollbar(view).setValue(scrollbar(view).maximum())
    settle()
    view.append_assistant_delta("\n\nand following resumes. " * 30)
    settle()

    assert at_bottom(view)


def test_rebuild_keeps_a_scrolled_up_reader_in_place(panel, settle):
    """Expanding a tool block re-renders the whole document. The reader's
    position has to survive the document being emptied and refilled."""
    _panel, view = panel
    fill(view, settle)
    view.add_tool("bash", "echo hi", detail="hi\n" * 30)
    settle()
    before = scrollbar(view).maximum() // 3
    scrollbar(view).setValue(before)
    settle()

    view._repaint_all()
    settle()

    assert abs(scrollbar(view).value() - before) <= SLACK


def test_rebuild_keeps_a_following_reader_at_the_bottom(panel, settle):
    _panel, view = panel
    fill(view, settle)
    view.add_tool("bash", "echo hi", detail="hi\n" * 30)
    settle()
    assert at_bottom(view)

    view._repaint_all()
    settle()

    assert at_bottom(view)


def test_a_failed_rebuild_does_not_disable_following(panel, settle, monkeypatch):
    """_repaint_all fences the sticky-bottom logic off while it empties and
    refills the document. Painting parses markdown and lexes code, so it can
    raise — and a fence left standing switches following off for the life of
    the widget, with nothing else to show for it."""
    _panel, view = panel
    fill(view, settle)
    painted = []
    original = view._paint

    def exploding(kind, payload):
        painted.append(kind)
        if len(painted) == 3:
            raise RuntimeError("a paint blew up mid-rebuild")
        return original(kind, payload)

    monkeypatch.setattr(view, "_paint", exploding)
    with pytest.raises(RuntimeError):
        view._repaint_all()
    monkeypatch.undo()
    settle()

    view.append_assistant_delta("content arriving after the failure. " * 300)
    settle()

    # The partial rebuild leaves a short document, so guard against passing
    # vacuously: there has to be something to scroll before "at the bottom"
    # says anything at all.
    assert scrollbar(view).maximum() > 0, "the transcript never overflowed"
    assert at_bottom(view), "following stayed off after a rebuild raised"


def test_loaded_session_opens_at_the_end(panel, settle):
    _panel, view = panel
    view.render_messages(
        [
            {"role": "user", "content": [{"type": "text", "text": f"message {i}"}]}
            for i in range(60)
        ]
    )
    settle()

    assert at_bottom(view)


def test_loaded_session_resets_a_scrolled_up_position(panel, settle):
    """Switching sessions should show the newest messages, not inherit where
    the reader happened to be in the previous transcript."""
    _panel, view = panel
    fill(view, settle)
    scrollbar(view).setValue(0)
    settle()

    view.render_messages(
        [
            {"role": "user", "content": [{"type": "text", "text": f"message {i}"}]}
            for i in range(60)
        ]
    )
    settle()

    assert at_bottom(view)
