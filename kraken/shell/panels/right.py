"""Terminal pane, the sibling of the browser pane."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtWidgets import QWidget

from kraken.shell.panels.base import Card, Panel
from kraken.ui.themes import UI_COLORS

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget


class RightPanel(Panel):
    def __init__(
        self,
        parent: QWidget | None = None,
        cwd: str | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        from kraken.terminal.tabs import TerminalTabs

        self._card = Card()
        self.terminals = TerminalTabs(self, cwd=cwd, remote=remote)
        self._card.add_widget(self.terminals, stretch=1)
        self.add_widget(self._card, stretch=1)

    @property
    def theme_name(self) -> str:
        return self.terminals.theme_name

    def set_theme(self, name: str) -> None:
        """Theme the card and everything inside it (tab strip, terminals)."""
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self.terminals.set_theme(name)
