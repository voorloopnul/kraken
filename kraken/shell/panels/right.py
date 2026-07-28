"""Terminal pane, the sibling of the browser pane."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtWidgets import QWidget

from kraken.shell.panels.base import Panel
from kraken.ui.chrome import Card
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget


class RightPanel(Panel):
    """Terminal pane. The tabbed terminal (and its shell/SSH child process) is
    only created the first time the panel becomes visible, so a workspace you
    never open a terminal in costs no shell and no scrollback buffer."""

    def __init__(
        self,
        parent: QWidget | None = None,
        cwd: str | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self._card = Card()
        self._theme_name = DEFAULT_THEME
        self._cwd = cwd
        self._remote = remote
        self.terminals = None
        self._creating = False
        # Held (unparented, so nothing paints) until the tab strip it rides in
        # exists; the dock hands it over long before the panel is first shown.
        self._grip = None
        self.add_widget(self._card, stretch=1)

    def mount_grip(self, grip: QWidget) -> bool:
        """The grip rides at the head of the terminal's own tab strip."""
        self._grip = grip
        if self.terminals is not None:
            self.terminals.mount_grip(grip)
        return True

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self._ensure_terminals()

    def _ensure_terminals(self) -> None:
        # Re-entrancy guard mirrors BrowserPanel: terminal construction can
        # pump the event loop, so a second showEvent must not stack a
        # duplicate terminal into the card.
        if self.terminals is not None or self._creating:
            return
        self._creating = True
        try:
            from kraken.terminal.tabs import TerminalTabs

            terminals = TerminalTabs(self, cwd=self._cwd, remote=self._remote)
            terminals.set_theme(self._theme_name)
            if self._grip is not None:
                terminals.mount_grip(self._grip)
            self._card.add_widget(terminals, stretch=1)
            self.terminals = terminals
        finally:
            self._creating = False

    @property
    def theme_name(self) -> str:
        return self._theme_name

    def set_theme(self, name: str) -> None:
        """Theme the card and everything inside it (tab strip, terminals)."""
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        if self.terminals is not None:
            self.terminals.set_theme(name)

    def shutdown(self) -> None:
        if self.terminals is not None:
            self.terminals.shutdown_all()
