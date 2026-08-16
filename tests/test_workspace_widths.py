"""Panel widths must not force the workspace beyond the display.

The dock already gives each panel a preferred width. Setting hard minimums on
the content as well makes those values additive across horizontal columns. If
their sum is wider than a fullscreen window, Qt can constrain the splitter
wrappers without constraining the content inside them; controls then paint in
one place while receiving pointer events in another.
"""

import pytest

from kraken.shell.workspace_view import WorkspaceView


@pytest.fixture
def view(qapp, tmp_path):
    workspace = WorkspaceView(str(tmp_path))
    yield workspace
    workspace.shutdown()
    workspace.deleteLater()
    qapp.processEvents()


def test_panel_widths_are_preferences_instead_of_hard_minimums(view):
    """The dock may shrink every horizontal panel to fit the real viewport."""
    assert {
        key: panel.minimumWidth()
        for key, panel in (
            ("history", view.left_panel),
            ("browser", view.browser_panel),
            ("diff", view.diff_panel),
            ("git", view.git_panel),
            ("terminal", view.right_panel),
        )
    } == {
        "history": 0,
        "browser": 0,
        "diff": 0,
        "git": 0,
        "terminal": 0,
    }
