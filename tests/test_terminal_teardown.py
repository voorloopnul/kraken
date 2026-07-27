"""A terminal widget cleans up after itself however it is destroyed.

The deliberate paths (closing a tab, discarding a workspace) call shutdown()
explicitly, but a widget can also go down with its parent chain — and nothing
else in the process knows about its shell, its pty, or the libghostty handles
it holds. Qt also keeps delivering to the Python wrapper after the C++ half is
gone (the Wayland platform pumps the event loop while a window is torn down),
so the timer- and notifier-driven slots have to survive landing on a corpse.

Destruction is forced with shiboken6.delete rather than deleteLater: the tests
drive the loop with processEvents, which never delivers DeferredDelete.
"""

import os

import pytest
import shiboken6
from PySide6.QtWidgets import QVBoxLayout, QWidget

from kraken.terminal.widget import GhosttyTerminalWidget


@pytest.fixture
def term(qapp, settle):
    """A terminal inside a host widget, as it lives in the real layout — the
    host is what gets destroyed, so the terminal is never told directly."""
    host = QWidget()
    host.resize(600, 400)
    layout = QVBoxLayout(host)
    terminal = GhosttyTerminalWidget(host)
    layout.addWidget(terminal)
    host.show()
    settle()
    yield terminal
    if shiboken6.isValid(host):
        terminal.shutdown()
        shiboken6.delete(host)


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def test_destroyed_terminal_reaps_its_shell(term, settle):
    pid = term._child_pid
    assert alive(pid)

    shiboken6.delete(term.parentWidget())
    settle()

    assert not alive(pid)


def test_destroyed_terminal_releases_its_resources(term):
    shiboken6.delete(term.parentWidget())

    assert term._master_fd == -1  # pty master closed
    assert not term._term  # libghostty handles freed

    # A second teardown (shutdown() explicitly, then again from destroyed())
    # must not close the pty master's old fd number, which by now belongs to
    # whoever opened next.
    reused = os.open(os.devnull, os.O_RDONLY)
    try:
        term.shutdown()
        os.fstat(reused)  # raises if the terminal closed someone else's fd
    finally:
        os.close(reused)


def test_slots_are_inert_once_the_widget_is_destroyed(term):
    """Every entry point Qt can still call after destruction: the render tick,
    the pty notifier, the debounced resize, the autoscroll tick."""
    shiboken6.delete(term.parentWidget())

    term._sync_render_state()
    term._on_pty_readable()
    term._apply_resize()
    term._autoscroll_tick()
