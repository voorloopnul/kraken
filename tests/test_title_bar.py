"""The title bar's two-line layout.

The bar names what is open on two lines — the conversation over the folder it
belongs to — and everything after the window's traffic lights hangs off that
column. Geometry is what those tests assert, because the failure this can have
is a silent one: a row that lands beside the title instead of under it, or a
second line that starts at a different left edge from the first, still lays out
and still paints.
"""

import pytest

from kraken.shell import main_window as main_window_module
from kraken.shell.title_bar import TitleBar, _external_app_argv, home_relative


@pytest.fixture
def bar(qapp, tmp_path):
    # A folder the branch switcher has something to say about: git_branch reads
    # .git/HEAD itself, so a written ref is enough to put the button on screen.
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
    widget = TitleBar()
    widget.resize(800, widget.height())
    widget.set_workspace(str(tmp_path))
    widget.set_conversation("A conversation")
    # Layouts are computed lazily; the geometry assertions need them settled.
    widget.layout().activate()
    yield widget
    widget.close()
    widget.deleteLater()


def test_the_folder_sits_under_the_conversation(bar):
    title = bar.conversation_label.geometry()
    folder = bar.folder_label.geometry()

    assert folder.top() >= title.bottom()
    # Both lines start at the same left edge, just past the History toggle.
    assert title.left() == folder.left()
    assert title.left() > bar.left_panel_toggle.geometry().right()


def test_the_branch_switcher_shares_the_folder_line(bar):
    folder = bar.folder_label.geometry()
    branch = bar.branch_button.geometry()

    assert branch.left() >= folder.right()
    assert abs(branch.center().y() - folder.center().y()) <= 2


def test_the_session_menu_opens_beside_the_title(bar):
    session = bar.session_button.geometry()
    title = bar.conversation_label.geometry()

    assert session.left() >= title.right()
    assert abs(session.center().y() - title.center().y()) <= 2


def test_the_memory_readout_stays_at_the_far_right(bar):
    assert bar.memory_label.geometry().left() > bar.session_button.geometry().right()


def test_the_external_launcher_sits_immediately_before_memory(bar):
    launcher = bar.external_open_button.geometry()
    memory = bar.memory_label.geometry()

    assert launcher.right() < memory.left()
    assert launcher.left() > bar.session_button.geometry().right()


def test_the_external_launcher_offers_the_first_three_apps(bar):
    assert [a.text() for a in bar.external_open_menu.actions()] == [
        "Ghostty",
        "Terminal",
        "Zed",
    ]


def test_external_apps_receive_the_workspace_path_on_macos(monkeypatch):
    monkeypatch.setattr("kraken.shell.title_bar.sys.platform", "darwin")
    path = "/Users/x/Workspace/a project"

    assert _external_app_argv("Ghostty", path) == [
        "/usr/bin/open",
        "-na",
        "Ghostty.app",
        "--args",
        f"--working-directory={path}",
    ]
    assert _external_app_argv("Terminal", path) == [
        "/usr/bin/open",
        "-a",
        "Terminal",
        path,
    ]
    assert _external_app_argv("Zed", path) == [
        "/usr/bin/open",
        "-a",
        "Zed",
        path,
    ]


def test_the_external_launcher_is_hidden_without_a_workspace(qapp):
    bar = TitleBar()
    try:
        bar.set_workspace(None)
        assert bar.external_open_button.isHidden()
    finally:
        bar.close()
        bar.deleteLater()


def test_the_memory_readout_requests_the_process_modal(bar):
    requested = []
    bar.memory_requested.connect(lambda: requested.append(True))

    assert bar.memory_label.toolTip() == "Show Kraken processes and memory"
    bar.memory_label.click()

    assert requested == [True]


def test_both_lines_fit_the_bar(bar):
    assert bar.folder_label.geometry().bottom() <= bar.height()


def test_an_unfocused_bar_still_names_its_first_line(bar):
    """With no session chosen the line says so, rather than leaving a hole
    above the folder it belongs to."""
    bar.set_conversation("")

    assert bar.conversation_label.text() == "No session selected"


def test_the_folder_line_shortens_the_home_folder(monkeypatch):
    monkeypatch.setattr("pathlib.Path.home", lambda: __import__("pathlib").Path("/Users/x"))

    assert home_relative("/Users/x/Workspace/kraken") == "~/Workspace/kraken"
    assert home_relative("/Users/x") == "~"
    assert home_relative("/opt/kraken") == "/opt/kraken"
    # A folder that merely starts with the same characters is not inside it.
    assert home_relative("/Users/xylophone/kraken") == "/Users/xylophone/kraken"


def test_the_session_menu_says_when_there_is_nothing_to_act_on(qapp, monkeypatch):
    """The menu is filled as it opens, from whatever session is focused. On the
    home screen there is neither, so it opens with one dead entry rather than
    with actions that would have nothing to archive or delete."""
    monkeypatch.setattr(main_window_module, "load_state", lambda: {})
    monkeypatch.setattr(main_window_module, "save_state", lambda **kwargs: None)
    window = main_window_module.MainWindow()
    try:
        window.title_bar.session_menu.aboutToShow.emit()
        actions = window.title_bar.session_menu.actions()

        assert [a.text() for a in actions] == ["No saved session"]
        assert not actions[0].isEnabled()
    finally:
        window.close()
        window.deleteLater()
