"""Expanded tool output uses the mock's padded output card."""

from PySide6.QtGui import QColor

from kraken.chat.conversation import (
    ConversationView,
    _PALETTE,
    _TOOL_CARD_PAD_X,
    _TOOL_BLOCK_MARGIN,
)


def expanded_bash(qapp) -> ConversationView:
    view = ConversationView()
    view.set_theme("light")
    index = view.add_tool("bash", "ls -la", '{\n  "command": "ls -la"\n}')
    view.append_tool_detail(index, "total 232\ndrwxr-xr-x  20 pascal  staff  640 .")
    view._blocks[index][1]["expanded"] = True
    view._repaint_all()
    return view


def test_expanded_output_gets_its_own_padded_block(qapp):
    view = expanded_bash(qapp)

    assert len(view._tool_detail_ranges) == 1
    first, last = view._tool_detail_ranges[0]
    assert first < last
    first_block = view.document().findBlockByNumber(first)
    last_block = view.document().findBlockByNumber(last)
    assert first_block.blockFormat().leftMargin() == _TOOL_CARD_PAD_X
    assert first_block.blockFormat().topMargin() == _TOOL_BLOCK_MARGIN
    assert last_block.blockFormat().leftMargin() == _TOOL_CARD_PAD_X
    assert last_block.blockFormat().bottomMargin() == _TOOL_BLOCK_MARGIN
    assert first_block.text() == "{"
    assert "drwxr-xr-x" in last_block.text()


def test_light_output_card_uses_the_mockup_colors():
    colors = _PALETTE["light"]

    assert colors["tool_bg"] == "#f5f3ef"
    assert colors["tool_border"] == "#e1ded8"
    assert colors["tool_detail"] == "#8e8b86"


def test_expanded_output_text_is_muted_monospace(qapp):
    view = expanded_bash(qapp)
    first, _ = view._tool_detail_ranges[0]
    fragment = view.document().findBlockByNumber(first).begin().fragment()
    fmt = fragment.charFormat()

    assert fmt.fontFixedPitch()
    assert fmt.foreground().color() == QColor("#8e8b86")
