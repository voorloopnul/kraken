"""The frameless window's rounded corners.

There is no decoration to round, so each corner belongs to the widget sitting in
it, and every one of those widgets rebuilds its whole style sheet on a theme
change. That is the interaction worth pinning: a radius applied once and then
dropped by the next theme flip is the shape this went wrong in while it was
being written.

The window-level tests drive a real MainWindow, because the radius is decided
there and pushed outwards — the maximize flip and the frame behind the corner
widgets have no other home to be tested in.
"""

import pytest
from PySide6.QtCore import Qt

from kraken.shell import main_window as main_window_module
from kraken.shell.side_bar import SideBar
from kraken.shell.title_bar import TitleBar
from kraken.shell.workspace_bar import WorkspaceBar
from kraken.ui.chrome import WINDOW_RADIUS, corner_style
from kraken.ui.themes import UI_COLORS


def test_corner_style_rounds_only_the_corners_it_is_given():
    rule = corner_style("#sideBar", ("bottom-right",), 10)

    assert "#sideBar" in rule
    assert "border-bottom-right-radius: 10px;" in rule
    for other in ("top-left", "top-right", "bottom-left"):
        assert other not in rule


# Each bar with the corners it owns: the title bar spans the top, and the two
# vertical strips take the bottom corners they sit in.
BARS = {
    "title bar": (TitleBar, ("top-left", "top-right")),
    "workspace bar": (WorkspaceBar, ("bottom-left",)),
    "side bar": (SideBar, ("bottom-right",)),
}


@pytest.fixture(params=list(BARS), ids=list(BARS))
def bar(qapp, request):
    """One of the three widgets that own a window corner, with the corners it is
    responsible for. Torn down here so a failed assertion doesn't leak it."""
    factory, corners = BARS[request.param]
    widget = factory()
    yield widget, corners
    widget.close()
    widget.deleteLater()


def test_a_bar_rounds_the_corners_it_owns(bar):
    widget, corners = bar
    widget.set_corner_radius(WINDOW_RADIUS)

    style = widget.styleSheet()
    for corner in corners:
        assert f"border-{corner}-radius: {WINDOW_RADIUS}px;" in style


def test_a_theme_change_keeps_the_radius(bar):
    """set_theme rewrites the whole style sheet, so the radius has to be composed
    back into it rather than appended once and forgotten."""
    widget, corners = bar
    widget.set_corner_radius(WINDOW_RADIUS)

    for theme in ("dark", "light", "dark"):
        widget.set_theme(theme)
        style = widget.styleSheet()
        for corner in corners:
            assert f"border-{corner}-radius: {WINDOW_RADIUS}px;" in style, theme
        # And the theme's own colors are still there.
        assert "background:" in style


def test_a_radius_of_zero_squares_the_corner_off(bar):
    """What a maximized window is handed: there is nothing beside it to round
    against, and a gap at the screen's own corner reads as a glitch."""
    widget, corners = bar
    widget.set_corner_radius(WINDOW_RADIUS)
    widget.set_corner_radius(0)

    style = widget.styleSheet()
    for corner in corners:
        assert f"border-{corner}-radius: 0px;" in style
        assert f"border-{corner}-radius: {WINDOW_RADIUS}px;" not in style


@pytest.fixture
def window(qapp, settle, monkeypatch):
    """A real MainWindow, kept away from the user's stored workspaces."""
    monkeypatch.setattr(main_window_module, "load_state", lambda: {})
    monkeypatch.setattr(main_window_module, "save_state", lambda **kwargs: None)
    win = main_window_module.MainWindow()
    win.resize(700, 500)
    win.show()
    settle()
    yield win
    win.close()
    win.deleteLater()


def test_the_window_is_translucent_so_its_corners_can_be_empty(window):
    # Without this the pixels outside the radius would be painted, not absent.
    assert window.testAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)


def test_the_frame_and_the_corner_widgets_agree_on_the_radius(window):
    """The frame sits under the corner widgets at the same radius, so the two
    curves coincide instead of one showing past the other."""
    assert f"border-radius: {WINDOW_RADIUS}px" in window._frame.styleSheet()
    for widget in (window.title_bar, window.workspace_bar, window.side_bar):
        assert f"{WINDOW_RADIUS}px" in widget.styleSheet()


def test_maximizing_squares_the_window_and_restoring_rounds_it(window, settle):
    window.showMaximized()
    settle()

    assert "border-radius: 0px" in window._frame.styleSheet()
    assert "border-top-left-radius: 0px;" in window.title_bar.styleSheet()

    window.showNormal()
    settle()

    assert f"border-radius: {WINDOW_RADIUS}px" in window._frame.styleSheet()
    assert f"border-top-left-radius: {WINDOW_RADIUS}px;" in window.title_bar.styleSheet()


def test_a_theme_change_keeps_the_frame_rounded(window, settle):
    window.set_theme("dark")
    settle()

    style = window._frame.styleSheet()
    assert f"border-radius: {WINDOW_RADIUS}px" in style
    assert UI_COLORS["dark"]["window"].lower() in style.lower()


def test_an_open_diff_sheet_leaves_the_corners_empty(window, settle):
    """The sheet covers the whole window, corners included, so it has to respect
    the same rounding. Filling its plain rect painted the four corners the frame
    deliberately leaves empty — squaring the window off, over the desktop rather
    than over the app, for as long as a diff was open."""
    from kraken.shell.diff_viewer import DiffDocument, DiffViewer

    def image():
        return window.grab().toImage()

    def corner_alphas(shot):
        w, h = shot.width(), shot.height()
        return [
            shot.pixelColor(x, y).alpha()
            for x, y in ((0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1))
        ]

    before = image()
    assert corner_alphas(before) == [0, 0, 0, 0]
    middle = (window.width() // 2, window.height() // 2)

    viewer = DiffViewer.open_on(
        window,
        "light",
        DiffDocument(path="a.py", letter="M", diff_text="@@ -1,1 +1,1 @@\n-a\n+b\n"),
    )
    settle(300)
    after = image()

    assert corner_alphas(after) == [0, 0, 0, 0]
    # The scrim is genuinely there: everything between the corners is dimmed.
    assert after.pixelColor(*middle) != before.pixelColor(*middle)

    viewer.close_view()
    settle(300)


def test_a_state_change_that_keeps_the_shape_restyles_nothing(window, monkeypatch):
    """Restyling the frame repolishes every widget under it — the transcript, the
    terminal, the web view. Minimize and restore both come through here without
    changing the shape, so they must not pay for it."""
    applied = []
    monkeypatch.setattr(window._frame, "setStyleSheet", lambda sheet: applied.append(sheet))

    window._apply_corners()  # the radius is already what it should be

    assert applied == []
