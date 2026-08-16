"""Typography in the History pane follows the application mockup."""

from kraken.shell.panels.left import LeftPanel, _ROW_COLORS, _row_html
from kraken.ui.fonts import MONO_FAMILY, UI_SANS_FAMILY


def test_history_rows_use_sans_while_new_session_keeps_ui_mono(qapp):
    panel = LeftPanel()

    assert panel._list.font().family() == UI_SANS_FAMILY
    assert panel._list.font().pixelSize() == 13
    assert panel._new_button.font().family() == MONO_FAMILY
    assert panel._new_button.font().pixelSize() == 13


def test_history_title_uses_regular_weight():
    row = _row_html("A title", "Aug 16 · 2 messages")

    assert "<b>" not in row
    assert '<span class="title">A title</span>' in row
    assert '<span class="subtitle">Aug 16 · 2 messages</span>' in row


def test_light_history_colors_match_the_mockup():
    assert _ROW_COLORS["light"]["title"] == "#2a2824"
    assert _ROW_COLORS["light"]["subtitle"] == "#8e8b86"
