"""Panels are surfaces divided by a hairline, not cards floating in a gutter.

The dock used to inset its columns and let each panel draw a rounded border of
its own, so what separated two panels was a band of window colour. Now the
panels run edge to edge and the only thing between two of them is the line
their splitter handle paints — which means every inset along that path has to
stay at zero, and the handle has to carry both colours it needs: the line, and
the surface either side of it.

The handle is deliberately wider than the line it draws, since a 1px drag
target cannot be hit. That is why the surface colour matters: those extra
pixels are painted, not left to show the window through.
"""

import pytest
from PySide6.QtWidgets import QLabel

from kraken.shell.dock import _GRIP_COLORS, DockArea, DockPanel
from kraken.shell.panels.base import Panel
from kraken.shell.panels.left import LeftPanel
from kraken.ui.themes import UI_COLORS

KEYS = ("left", "center", "git")


class _SidebarPanel(Panel):
    """Stands in for the history pane: a panel that is not a card."""

    SURFACE_KEY = "sidebar"


@pytest.fixture
def dock(qapp):
    area = DockArea(order=list(KEYS), stretch_key="center", fixed_keys={"left"})
    for key in KEYS:
        content = _SidebarPanel() if key == "left" else Panel()
        content.add_widget(QLabel(key))
        area.register(DockPanel(key, content, draggable=key != "left"))
    area.set_layout([["left"], ["center"]])
    return area


def test_the_dock_insets_nothing(dock):
    margins = dock.layout().contentsMargins()
    assert (
        margins.left(),
        margins.top(),
        margins.right(),
        margins.bottom(),
    ) == (0, 0, 0, 0)


def test_panels_inset_nothing(qapp):
    """Panel's own margin would open a gap of window colour beside the
    divider, which is exactly what the divider replaced."""
    panel = Panel()  # held: a temporary takes its layout down with it
    margins = panel.layout().contentsMargins()
    assert (
        margins.left(),
        margins.top(),
        margins.right(),
        margins.bottom(),
    ) == (0, 0, 0, 0)


def test_the_divider_is_narrower_than_the_handle_carrying_it(dock):
    """If the line ever grew to the handle's width it would read as a gutter
    again, and if the handle shrank to the line's the divider could not be
    dragged."""
    from kraken.shell.dock import _GripHandle, _GripSplitter

    assert _GripHandle._LINE < _GripSplitter._HANDLE


@pytest.mark.parametrize("theme", ("light", "dark"))
def test_a_theme_change_recolors_the_line(dock, theme):
    # QColor.name() is lower case; the palette builds its hexes with %02X.
    dock.set_theme(theme)
    for splitter in [dock._splitter, *dock._columns()]:
        assert splitter.grip_color.name().lower() == _GRIP_COLORS[theme].lower()


@pytest.mark.parametrize("theme", ("light", "dark"))
def test_a_divider_takes_its_colour_from_each_side(dock, theme):
    """The bug this covers: the handle filled itself with one colour for both
    sides, always the card's. Beside the history pane — which is a shade off
    the cards — that painted a stripe of card colour, reinstating the gutter
    the divider exists to replace.
    """
    dock.set_theme(theme)
    splitter = dock._splitter
    before, after = splitter.neighbours(splitter.handle(1))
    ui = UI_COLORS[theme]
    assert splitter.surface_color(before).name().lower() == ui["sidebar"].lower()
    assert splitter.surface_color(after).name().lower() == ui["card"].lower()
    # The two really are different, or this test would pass on the old code.
    assert ui["sidebar"].lower() != ui["card"].lower()


def test_the_history_pane_declares_itself_a_sidebar():
    """The link between the mechanism above and the real panel: the dividers
    resolve a colour from this, so the pane going back to a card's surface
    without saying so would be silent."""
    assert LeftPanel.SURFACE_KEY == "sidebar"
    assert Panel.SURFACE_KEY == "card"


def test_the_divider_matches_what_a_card_border_would_have_been():
    """The line is the border the panels stopped drawing, so it has to stay
    the same colour — a divider picked independently would drift from the
    borders still drawn elsewhere in the app."""
    for name, ui in UI_COLORS.items():
        assert _GRIP_COLORS[name] == ui["card_border"]
