"""Reasoning is surfaced as the mock's collapsible Thinking activity row."""

from PySide6.QtGui import QColor

from kraken.chat.conversation import ConversationView
from kraken.agent.session_controller import SessionController


def test_streamed_thinking_stays_in_one_collapsible_row(qapp):
    view = ConversationView()
    view.append_thinking_delta("**Preparing concise ")
    view.append_thinking_delta("run instructions**")

    assert [kind for kind, _ in view._blocks] == ["thinking"]
    assert view.toPlainText() == "› ✧ Thinking  Preparing concise run instructions"

    view._blocks[0][1]["expanded"] = True
    view._repaint_all()
    assert view.toPlainText() == "▾ ✧ Thinking\nPreparing concise run instructions"

    thinking_weight = None
    header_weight = None
    block = view.document().firstBlock()
    while block.isValid():
        for fragment in (it.fragment() for it in block):
            if "Thinking" in fragment.text():
                header_weight = fragment.charFormat().fontWeight()
            if "Preparing concise" in fragment.text():
                thinking_weight = fragment.charFormat().fontWeight()
        block = block.next()
    assert header_weight < thinking_weight


def test_stored_thinking_parts_are_rendered(qapp):
    view = ConversationView()
    view.render_messages(
        [{"role": "assistant", "content": [{"type": "thinking", "thinking": "Plan"}]}]
    )

    assert "Thinking  Plan" in view.toPlainText()


def test_live_thinking_deltas_reach_the_transcript(qapp, tmp_path):
    controller = SessionController(str(tmp_path), "light")

    controller._on_event(
        {
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "thinking_delta",
                "delta": "Checking the implementation",
            },
        }
    )

    assert "Thinking  Checking the implementation" in controller.conversation.toPlainText()
    controller.stop()


def test_thinking_label_is_stronger_than_its_muted_summary(qapp):
    view = ConversationView()
    view.set_theme("light")
    view.append_thinking_delta("Inspecting project environment files")

    colors = {}
    for fragment in (it.fragment() for it in view.document().firstBlock()):
        if "Thinking" in fragment.text():
            colors["label"] = fragment.charFormat().foreground().color()
        if "Inspecting" in fragment.text():
            colors["summary"] = fragment.charFormat().foreground().color()

    assert colors == {
        "label": QColor("#8e8b86"),
        "summary": QColor("#b6b3ae"),
    }


def test_thinking_and_tool_rows_have_one_consistent_gap(qapp):
    view = ConversationView()
    view.append_thinking_delta("Inspecting")
    view.add_tool("bash", "ls -la")
    view.add_tool("bash", "find .")
    view.append_thinking_delta("Planning")

    visible_blocks = []
    block = view.document().firstBlock()
    while block.isValid():
        if block.text():
            visible_blocks.append(block.blockNumber())
        block = block.next()

    assert visible_blocks == [0, 1, 2, 3]
    for number in visible_blocks[1:]:
        block = view.document().findBlockByNumber(number)
        assert block.blockFormat().topMargin() == 8.0
