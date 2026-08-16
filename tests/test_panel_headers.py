"""The strip along the top of every panel.

A panel's header — the terminal's and browser's tab rows, and the plain grip row
on the panels without tabs — is one surface running the panel's full width, with
the content inset underneath it. Both halves of that are easy to lose silently:
a header added inside the card's padding still lays out and still paints, just
with a stripe of card colour down either side of it, and a strip whose theme is
never pushed through paints nothing at all.
"""

import pytest
from PySide6.QtWidgets import QLabel, QWidget

from kraken.shell.dock import DockPanel, _PanelHeader, PanelGrip
from kraken.ui.chrome import PANEL_HEADER_HEIGHT, Card
from kraken.ui.themes import UI_COLORS


@pytest.fixture
def card(qapp):
    widget = Card(flat=True)
    header = QLabel("header")
    body = QLabel("body")
    widget.add_header(header)
    widget.add_widget(body, stretch=1)
    widget.resize(300, 200)
    widget.layout().activate()
    yield widget, header, body
    widget.deleteLater()


def test_a_card_header_runs_the_full_width(card):
    widget, header, _ = card

    assert header.geometry().left() == 0
    assert header.geometry().right() == widget.width() - 1
    assert header.geometry().top() == 0


def test_the_content_under_it_stays_inset(card):
    widget, header, body = card

    assert body.geometry().left() > 0
    assert body.geometry().top() > header.geometry().bottom()


def test_a_second_header_lands_under_the_first_and_over_the_content(card):
    widget, header, body = card
    second = QLabel("second")
    widget.add_header(second)
    widget.layout().activate()

    assert header.geometry().bottom() <= second.geometry().top()
    assert second.geometry().bottom() < body.geometry().top()


class _NoStripPanel(QWidget):
    """A content panel with nowhere of its own for the grip, so the dock wraps
    it in a header row — what git and diff do."""

    def __init__(self):
        super().__init__()
        self.rows: list[QWidget] = []

    def mount_grip(self, grip) -> bool:
        return False

    def mount_grip_row(self, row) -> None:
        self.rows.append(row)


def test_a_panel_without_a_strip_is_given_one(qapp):
    panel = DockPanel("git", _NoStripPanel())

    assert isinstance(panel.header, _PanelHeader)
    assert panel.header.height() == PANEL_HEADER_HEIGHT
    assert panel.content.rows == [panel.header]


def test_a_theme_change_reaches_the_strip(qapp):
    """The dock themes its panels; before the header was a surface there was
    nothing on it to paint, so nothing noticed that it was never asked."""
    panel = DockPanel("git", _NoStripPanel())

    for theme in ("dark", "light"):
        panel.set_theme(theme)
        style = panel.header.styleSheet()

        assert UI_COLORS[theme]["header"] in style
        assert UI_COLORS[theme]["card_border"] in style


def test_the_grip_row_is_the_height_of_a_tab_strip(qapp):
    """Both kinds of header are the same strip, so a panel beside another lines
    up with it whichever kind each one has."""
    from kraken.ui.chrome import tab_strip_style

    header = _PanelHeader(PanelGrip())

    assert header.height() == PANEL_HEADER_HEIGHT
    # And the tab strips paint themselves the same way.
    assert UI_COLORS["light"]["header"] in tab_strip_style("light")
