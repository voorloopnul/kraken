"""Conversation and terminal share their working-surface background."""

from kraken.shell.panels.center import CenterPanel
from kraken.ui.themes import THEMES, UI_COLORS


def test_dark_conversation_matches_terminal_background(qapp):
    panel = CenterPanel()
    panel.set_theme("dark")

    terminal = "#%02X%02X%02X" % THEMES["dark"].background
    assert UI_COLORS["dark"]["card"] == terminal
    assert UI_COLORS["dark"]["window"] == terminal
    assert f"background: {terminal}" in panel.conversation_stack.styleSheet()
