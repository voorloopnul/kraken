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

from kraken.shell.dock import _GRIP_COLORS, _SURFACE_COLORS, DockArea, DockPanel
from kraken.shell.panels.base import Panel

KEYS = ("left", "center", "git")


@pytest.fixture
def dock(qapp):
    area = DockArea(order=list(KEYS), stretch_key="center", fixed_keys={"left"})
    for key in KEYS:
        content = Panel()
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
def test_a_theme_change_recolors_line_and_surface(dock, theme):
    # QColor.name() is lower case; the palette builds its card hex with %02X.
    dock.set_theme(theme)
    for splitter in [dock._splitter, *dock._columns()]:
        assert splitter.grip_color.name().lower() == _GRIP_COLORS[theme].lower()
        assert splitter.surface_color.name().lower() == _SURFACE_COLORS[theme].lower()


def test_the_divider_matches_what_a_card_border_would_have_been():
    """The line is the border the panels stopped drawing, so it has to stay
    the same colour — a divider picked independently would drift from the
    borders still drawn elsewhere in the app."""
    from kraken.ui.themes import UI_COLORS

    for name, ui in UI_COLORS.items():
        assert _GRIP_COLORS[name] == ui["card_border"]
        assert _SURFACE_COLORS[name] == ui["card"]
