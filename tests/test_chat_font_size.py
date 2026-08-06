"""One setting sizes the whole conversation.

The base size reaches the transcript's body text, the sizes written into char
formats rather than style sheets (the monospace detail under a tool call), the
chrome around the messages, and the composer below them — and a session opened
after the change starts at the chosen size rather than the default.
"""

import pytest

from kraken.chat import typography
from kraken.chat.chat_input import ChatInput
from kraken.chat.conversation import ConversationView
from kraken.shell.panels.center import CenterPanel


def test_scale_is_derived_from_the_base():
    base = typography.DEFAULT_SIZE
    assert typography.secondary(base) == base - 1
    assert typography.caption(base) == base - 2
    # The value the tool detail used before the size was configurable, which
    # is what pins the px->pt conversion to the size it was chosen against.
    assert typography.detail_points(base) == 9.0
    # Chrome collapses onto the body size at the floor instead of shrinking
    # below what the setting itself allows.
    assert typography.secondary(typography.MIN_SIZE) == typography.MIN_SIZE
    assert typography.caption(typography.MIN_SIZE) == typography.MIN_SIZE


def test_clamp_survives_a_bad_stored_value():
    assert typography.clamp(typography.MAX_SIZE + 20) == typography.MAX_SIZE
    assert typography.clamp(0) == typography.MIN_SIZE
    # State restored from disk is whatever is in the file.
    assert typography.clamp("huge") == typography.DEFAULT_SIZE
    assert typography.clamp(None) == typography.DEFAULT_SIZE


@pytest.fixture
def view(qapp):
    view = ConversationView()
    view.set_theme("dark")
    yield view
    view.deleteLater()


def test_transcript_body_follows_the_setting(view):
    view.set_font_size(19)
    assert "font-size: 19px" in view.styleSheet()
    # The rule has to come after the theme sheet's own font-size, or the
    # default would keep winning on equal specificity.
    assert view.styleSheet().rindex("font-size: 19px") > view.styleSheet().index(
        "QTextBrowser"
    )


def test_tool_detail_follows_the_setting(view):
    index = view.add_tool("bash", "ls", "total 24")
    view._blocks[index][1]["expanded"] = True
    view.set_font_size(20)

    sizes = set()
    block = view.document().firstBlock()
    while block.isValid():
        for fragment in (it.fragment() for it in block):
            if "total 24" in fragment.text():
                sizes.add(round(fragment.charFormat().fontPointSize(), 2))
        block = block.next()
    # The detail is sized in points through a char format, so it only tracks
    # the setting because set_font_size rebuilds the document.
    assert sizes == {typography.detail_points(20)}


def test_composer_and_busy_row_follow_the_setting(qapp):
    panel = CenterPanel()
    panel.set_theme("dark")
    panel.set_font_size(17)
    assert "font-size: 17px" in panel.chat.styleSheet()
    assert f"font-size: {typography.secondary(17)}px" in panel._busy_row.styleSheet()
    panel.deleteLater()


def test_chat_input_keeps_its_size_across_a_theme_switch(qapp):
    chat = ChatInput()
    chat.set_font_size(16)
    chat.set_theme("light")
    assert "font-size: 16px" in chat.styleSheet()
    chat.deleteLater()


def test_a_new_session_starts_at_the_chosen_size(qapp, tmp_path):
    from kraken.agent.session_controller import SessionController

    controller = SessionController(str(tmp_path), "dark", font_size=21)
    assert controller.conversation._font_size == 21
    assert "font-size: 21px" in controller.conversation.styleSheet()
    controller.stop()


@pytest.fixture
def window(qapp, settle, monkeypatch):
    """A real MainWindow, kept away from the user's stored state — the picker's
    coalescing lives there, not in the widgets the size ends up on."""
    from kraken.shell import main_window as main_window_module

    saved = []
    monkeypatch.setattr(main_window_module, "load_state", lambda: {})
    monkeypatch.setattr(
        main_window_module, "save_state", lambda **kwargs: saved.append(kwargs)
    )
    win = main_window_module.MainWindow()
    win.saved = saved
    yield win
    win.close()
    win.deleteLater()


def test_a_run_of_sizes_is_applied_once(window, settle):
    # What a spin box emits while "24" is typed, or a step button is held.
    for size in (2, 24, 20, 19):
        window.set_chat_font_size(size)
    # Nothing applied yet, but the size is already the window's: a workspace
    # opened at this moment opens at it.
    assert window.saved == []
    assert window._font_size == typography.clamp(19)
    settle(400)
    # One save, of the size the picker settled on — not four, each re-rendering
    # every open transcript.
    assert window.saved == [{"chat_font_size": 19}]


def test_a_size_chosen_and_left_is_not_lost(window):
    window.set_chat_font_size(21)
    # Closing the picker (or the window) does not wait for the timer: a setting
    # the user chose and never saw applied is worse than one applied early.
    window.flush_font_size()
    assert window.saved == [{"chat_font_size": 21}]
    # And the flush is idempotent — the timer it pre-empted must not fire too.
    window.flush_font_size()
    assert window.saved == [{"chat_font_size": 21}]
