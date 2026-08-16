"""Opening a terminal for pi's sign-in flow, without opening two.

The terminal pane builds itself on first use and its tab strip opens a first
tab as it is constructed. So "build the pane" and "give me a terminal" are one
request, not two, and answering the second with a new tab would leave that
first shell idle behind the one the sign-in is typed into — a real process with
a real scrollback attached to it. Once the pane is up, though, a request is a
request: the user may be working in the tab that is already there, and typing
`pi` into it would be worse than an extra tab.

A stub stands in for the tab strip: the real one starts a shell through
libghostty and a pty, which is not what is under test here.
"""

import pytest
from PySide6.QtWidgets import QWidget

from kraken.shell.panels.right import RightPanel


class FakeTerminal(QWidget):
    pass


class FakeTabs(QWidget):
    """The tab strip's contract as this panel uses it: it opens a tab of its
    own on construction, hands out the current one, and adds more on request."""

    def __init__(self, parent=None, cwd=None, remote=None, font_size=13):
        super().__init__(parent)
        self.font_size = font_size
        self.tabs: list[FakeTerminal] = []
        self.add_terminal()

    def add_terminal(self) -> FakeTerminal:
        self.tabs.append(FakeTerminal(self))
        return self.tabs[-1]

    @property
    def current_terminal(self) -> FakeTerminal | None:
        return self.tabs[-1] if self.tabs else None

    def set_theme(self, name: str) -> None:
        pass

    def set_font_size(self, size: int) -> None:
        self.font_size = size

    def mount_grip(self, grip) -> None:
        pass


@pytest.fixture
def panel(qapp, monkeypatch):
    import kraken.terminal.tabs as tabs_module

    monkeypatch.setattr(tabs_module, "TerminalTabs", FakeTabs)
    panel = RightPanel()
    yield panel
    panel.deleteLater()


def test_a_first_terminal_is_the_one_the_pane_already_opened(panel):
    terminal = panel.open_terminal()
    assert len(panel.terminals.tabs) == 1
    assert terminal is panel.terminals.tabs[0]


def test_asking_again_opens_a_second_terminal(panel):
    first = panel.open_terminal()
    second = panel.open_terminal()
    # Once the pane exists, asking for a terminal means a new one — this is the
    # ordinary "open another tab" case, and it must keep working.
    assert [first, second] == panel.terminals.tabs


def test_a_pane_already_on_screen_is_not_taken_over(panel):
    # Shown by the user, so its first tab is theirs and may have something
    # running in it. A caller that needs a shell of its own gets a new tab.
    panel.show()
    assert len(panel.terminals.tabs) == 1
    terminal = panel.open_terminal()
    assert terminal is not panel.terminals.tabs[0]
    assert len(panel.terminals.tabs) == 2


def test_font_size_is_kept_for_lazy_terminal_creation(panel):
    panel.set_font_size(17)
    panel.open_terminal()

    assert panel.terminals.font_size == 17
