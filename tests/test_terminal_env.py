"""A terminal opens in the user's environment, not in Kraken's.

The app runs on a Python of its own — a virtualenv in a checkout, a bundled
runtime in the .app — and the shell it spawns used to inherit that wholesale.
The variables describing it then followed the user into their own project: uv
and pip resolved against Kraken's virtualenv from any directory, because
VIRTUAL_ENV outranks the .venv sitting right there; python found Kraken's
source importable through PYTHONPATH; and a bare `python3` was Kraken's
interpreter, since activation had put its bin first on PATH.

What is checked here is the environment the terminal is handed, not a live
shell: the leak was in what got copied, and it is visible in the dict.
"""

import os

import pytest

from kraken.terminal.widget import _shell_env

VENV = "/somewhere/kraken/.venv"


@pytest.fixture
def app_env(monkeypatch):
    """The environment of an app running inside its own virtualenv."""
    monkeypatch.setenv("VIRTUAL_ENV", VENV)
    monkeypatch.setenv("PYTHONPATH", "/somewhere/kraken")
    monkeypatch.setenv("PYTHONHOME", "/somewhere/runtime")
    monkeypatch.setenv("PATH", f"{VENV}/bin:/opt/homebrew/bin:/usr/bin:/bin")
    monkeypatch.setenv("EDITOR", "vim")


def test_the_shell_is_not_told_about_kraken_s_python(app_env):
    env = _shell_env()
    for name in ("VIRTUAL_ENV", "PYTHONHOME", "PYTHONPATH"):
        assert name not in env, f"{name} followed the user into their shell"


def test_the_virtualenv_s_bin_leaves_path_too(app_env):
    """Unsetting VIRTUAL_ENV is not enough on its own — a bare `python` goes
    through PATH, and activation put the virtualenv first on it."""
    parts = _shell_env()["PATH"].split(os.pathsep)
    assert f"{VENV}/bin" not in parts


def test_the_rest_of_path_is_left_alone(app_env):
    """Only the virtualenv's own entry goes. The launcher's additions are
    deliberate: a bundle opened from the Dock inherits launchd's bare PATH,
    and Homebrew and the bundled pi are put back on it on purpose."""
    parts = _shell_env()["PATH"].split(os.pathsep)
    assert parts == ["/opt/homebrew/bin", "/usr/bin", "/bin"]


def test_everything_else_survives(app_env):
    assert _shell_env()["EDITOR"] == "vim"


def test_an_app_outside_a_virtualenv_keeps_its_path(monkeypatch):
    """Nothing to strip, nothing stripped — the .app runs on a bundled
    runtime with no VIRTUAL_ENV set at all."""
    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    monkeypatch.delenv("PYTHONHOME", raising=False)
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    monkeypatch.setenv("PYTHONPATH", "/Kraken.app/Contents/Resources/share/kraken")
    env = _shell_env()
    assert env["PATH"] == "/usr/bin:/bin"
    # PYTHONPATH still goes: in the bundle it names Kraken's own source, which
    # a `python` in the user's project has no business importing.
    assert "PYTHONPATH" not in env
