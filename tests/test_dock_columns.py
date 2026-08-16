"""The dock's columns must stay reachable from Python.

A column is built in Python and handed to the splitter, which owns it in C++.
Nothing else referred to one, so a column was reachable only through
`splitter.widget(i)` — and once the collector took it, Qt deleted every panel
inside it while they were on screen and taking events. The crash that followed
landed nowhere near the dock: a click in a panel's tree segfaulted inside
QAbstractItemView::mouseReleaseEvent, seconds after the drop that orphaned it.

What that failure needs to happen is a reference cycle spanning the column, and
those are built by the live panels (a diff sheet opening and closing leaves
thousands of collectable objects behind). Reproducing it here would mean
recreating a cycle this test would have to guess at, and asserting on when the
collector happens to run — a flaky test guarding the wrong thing.

So the guard is the invariant instead: no column is reachable only from C++.
That is the property the fix establishes, it holds independently of GC timing,
and it fails loudly if a future column is built outside `_acquire_column`.
"""

import gc

import pytest
import shiboken6
from PySide6.QtWidgets import QLabel

from kraken.shell.dock import DockArea, DockPanel
from kraken.shell.panels.base import Panel

KEYS = ("left", "center", "diff", "git")


@pytest.fixture
def dock(qapp):
    """A dock with four draggable panels, none of them placed yet."""
    area = DockArea(order=list(KEYS), stretch_key="center")
    for key in KEYS:
        content = Panel()
        content.add_widget(QLabel(key))
        area.register(DockPanel(key, content))
    return area


def assert_every_column_referenced(dock) -> None:
    """No column may be reachable only through the splitter."""
    held = {id(column) for column in dock._column_refs}
    assert held, "the dock holds no column references at all"
    for column in dock._columns():
        assert id(column) in held, (
            f"column holding {[p.key for p in column.panels()]} is reachable "
            "only from C++, so a collection can delete the panels inside it"
        )


def test_initial_columns_are_referenced(dock):
    dock.set_layout([["left"], ["center"]])
    assert_every_column_referenced(dock)


def test_columns_opened_by_show_panel_are_referenced(dock):
    """Revealing a panel that has never been placed builds a fresh column."""
    dock.set_layout([["left"], ["center"]])
    dock.show_panel("diff")
    dock.show_panel("git")
    assert_every_column_referenced(dock)


def test_columns_are_referenced_after_a_stacking_drop(dock):
    """git dropped into diff's column — the move that preceded the crash. Both
    panels then live in the destination, which is why collecting that one column
    killed a view in each of two different panels at the same instant."""
    dock.set_layout([["left"], ["center"]])
    dock.show_panel("diff")
    dock.show_panel("git")

    destination = dock._panels["diff"].parentWidget()
    assert dock._panels["git"].parentWidget() is not destination
    dock._apply_drop(dock._panels["git"], ("stack_bottom", destination))

    assert dock._panels["git"].parentWidget() is destination
    assert_every_column_referenced(dock)


def test_columns_are_referenced_after_a_new_column_drop(dock):
    """The other drop shape: a stacked panel pulled back out into its own
    column, which is the path that allocates one through _acquire_column."""
    dock.set_layout([["left"], ["center"]])
    dock.show_panel("diff")
    dock.show_panel("git")

    dock._apply_drop(
        dock._panels["git"], ("stack_bottom", dock._panels["diff"].parentWidget())
    )
    dock._apply_drop(
        dock._panels["git"], ("new_after", dock._panels["diff"].parentWidget())
    )

    assert dock._panels["git"].parentWidget() is not dock._panels["diff"].parentWidget()
    assert_every_column_referenced(dock)


def test_panels_survive_a_collection(dock):
    """The symptom the invariant exists to prevent, asserted directly.

    This passes with the columns unreferenced too — the cycle that makes one
    collectable is not present here — so it is a floor, not the guard. It would
    still catch an outright ownership regression that deletes panels eagerly.
    """
    dock.set_layout([["left"], ["center"]])
    dock.show_panel("diff")
    dock.show_panel("git")
    dock._apply_drop(
        dock._panels["git"], ("stack_bottom", dock._panels["diff"].parentWidget())
    )
    gc.collect()

    assert {key: shiboken6.isValid(p) for key, p in sorted(dock._panels.items())} == {
        key: True for key in sorted(KEYS)
    }


def test_extra_side_panels_stack_from_right_to_left(qapp):
    """Three side columns is the horizontal limit. Panels four through six
    fill the lower half of those columns, starting at the right edge."""
    anchors = ("left", "center")
    side_keys = tuple(f"side-{number}" for number in range(1, 7))
    area = DockArea(
        order=[*anchors, *side_keys],
        stretch_key="center",
        fixed_keys={"left"},
        no_stack_keys={"center"},
        max_side_columns=3,
    )
    for key in (*anchors, *side_keys):
        content = Panel()
        content.add_widget(QLabel(key))
        area.register(DockPanel(key, content, draggable=key in side_keys))
    area.set_layout([["left"], ["center"]])

    for key in side_keys:
        area.show_panel(key)

    side_columns = [
        [panel.key for panel in column.panels()]
        for column in area._active_side_columns()
    ]
    assert side_columns == [
        ["side-1", "side-6"],
        ["side-2", "side-5"],
        ["side-3", "side-4"],
    ]


def test_side_panel_cannot_be_dragged_into_a_fourth_column(qapp):
    """The cap still holds when a panel is pulled out of an automatic stack."""
    keys = ("center", "one", "two", "three", "four")
    area = DockArea(
        order=list(keys),
        stretch_key="center",
        no_stack_keys={"center"},
        max_side_columns=3,
    )
    for key in keys:
        content = Panel()
        content.add_widget(QLabel(key))
        area.register(DockPanel(key, content, draggable=key != "center"))
    area.set_layout([["center"]])
    for key in keys[1:]:
        area.show_panel(key)

    panel = area._panels["four"]
    source = panel.parentWidget()
    destination = area._panels["one"].parentWidget()
    area._apply_drop(panel, ("new_after", destination))

    assert panel.parentWidget() is source
    assert len(area._active_side_columns()) == 3
