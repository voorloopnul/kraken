"""The remote-workspace dialog, which is the settings window's chrome around a
form: the app's own decoration instead of the desktop's, and fields dressed as
the settings pages dress theirs."""

import pytest
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QLabel

from kraken.agent import remote
from kraken.shell.remote_dialog import RemoteWorkspaceDialog


@pytest.fixture
def no_hosts(monkeypatch):
    """No saved profiles and no ~/.ssh/config: the host picker is built from
    both as the dialog opens, and neither belongs in a test's answer."""
    monkeypatch.setattr(remote, "load_hosts", dict)
    monkeypatch.setattr(remote, "parse_ssh_config", list)
    monkeypatch.setattr(remote, "resolve", lambda anchor: None)


def test_the_dialog_wears_the_apps_decoration(qapp, no_hosts):
    dialog = RemoteWorkspaceDialog(theme_name="dark")
    assert dialog.windowFlags() & Qt.WindowType.FramelessWindowHint
    assert dialog.windowTitle() == "Add Remote Workspace"
    assert "#dialogTitleBar { background: #1b1c21;" in dialog.styleSheet()
    assert not dialog.title_bar.close_button.icon().isNull()
    dialog.show()
    dialog.title_bar.close_button.click()
    assert not dialog.isVisible()


def test_the_theme_reaches_the_form_as_well_as_the_frame(qapp, no_hosts):
    """The dialog used to carry no styling at all, so it opened in the
    desktop's palette however the app was themed."""
    dialog = RemoteWorkspaceDialog(theme_name="light")
    style = dialog.styleSheet()
    assert "#dialogFrame { background: #faf6ec; }" in style
    assert 'QLineEdit[role="control"]' in style
    # The fields have to claim that role for the rule to reach them.
    assert dialog._hostname.property("role") == "control"
    assert dialog._path.property("role") == "control"


def test_editing_says_so_in_the_title(qapp, no_hosts):
    dialog = RemoteWorkspaceDialog(anchor="remote:box", theme_name="dark")
    assert dialog.windowTitle() == "Edit Remote Workspace"
    # Our bar carries the name, so the window title alone is no longer proof
    # the user can see which of the two dialogs this is.
    shown = [label.text() for label in dialog.title_bar.findChildren(QLabel)]
    assert "Edit Remote Workspace" in shown
