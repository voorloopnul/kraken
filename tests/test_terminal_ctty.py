"""The shell a terminal spawns owns the pty as its controlling terminal.

Without one the shell still runs — it reads and writes fd 0 like nothing is
wrong — so this fails silently rather than loudly. What breaks is everything
the kernel routes through the terminal's foreground process group: SIGWINCH
above all, which means the shell keeps whatever $COLUMNS it started with and
every later resize leaves it laying out lines for a grid that is no longer
there. Ctrl-C has nobody to interrupt for the same reason.

macOS is where this bit: Linux hands a session leader the tty it opens without
O_NOCTTY, BSD only ever does that on an explicit TIOCSCTTY, and posix_spawn
cannot ask for one at all.
"""

import fcntl
import os
import re
import struct
import termios

import pytest
import shiboken6
from PySide6.QtWidgets import QVBoxLayout, QWidget

from kraken.terminal.widget import GhosttyTerminalWidget


@pytest.fixture
def term(qapp, settle):
    host = QWidget()
    host.resize(900, 500)
    layout = QVBoxLayout(host)
    layout.setContentsMargins(0, 0, 0, 0)
    terminal = GhosttyTerminalWidget(host)
    layout.addWidget(terminal)
    host.show()
    settle(500)
    yield terminal
    if shiboken6.isValid(host):
        terminal.shutdown()
        shiboken6.delete(host)


def foreground_pgrp(fd: int) -> int:
    """The pty's foreground process group, or 0 when it has none — which is
    what a pty whose shell never claimed it as a controlling terminal reports."""
    return struct.unpack("i", fcntl.ioctl(fd, termios.TIOCGPGRP, b"\0" * 4))[0]


def ask_columns(term, settle) -> int:
    """What the shell itself believes the width is, which is the only number
    that matters here — the bug this covers kept every number the widget owns
    perfectly consistent and left only the shell's idea of the width stale."""
    os.write(term._master_fd, b"printf 'C=%s\\n' $COLUMNS\r")
    settle(700)
    # Rows come back padded out to the full grid width, so trim before
    # matching on a line that is only the shell's answer.
    rendered = "\n".join(
        "".join(cell[0] or " " for cell in row).rstrip() for row in term._row_model
    )
    reported = re.search(r"^C=(\d+)$", rendered, re.MULTILINE)
    assert reported, f"shell never answered; grid was:\n{rendered}"
    return int(reported.group(1))


def winsize_cols(fd: int) -> int:
    return struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))[1]


def test_shell_claims_the_pty_as_its_controlling_terminal(term):
    assert foreground_pgrp(term._master_fd) == term._child_pid


def test_shell_starts_at_the_grid_width(term, settle):
    """The shell has to be born knowing the width, not told afterwards.

    openpty() hands back a 0x0 terminal and a shell reading that falls back to
    terminfo's 80 columns, so a shell started before the widget has a geometry
    prints its first prompt for a screen that is not there. There is no second
    chance at that prompt: the correction arrives after it has been drawn.
    """
    assert winsize_cols(term._master_fd) == term._cols
    assert ask_columns(term, settle) == term._cols


def test_resize_reaches_the_shell(term, settle):
    host = term.parent()
    host.resize(640, 500)
    settle(500)

    assert winsize_cols(term._master_fd) == term._cols
    assert ask_columns(term, settle) == term._cols


def test_a_terminal_that_is_never_shown_starts_no_shell(qapp, settle):
    """The corollary of starting the shell at first show: a terminal built
    into a page nobody has opened yet holds no pty and no child. Closing one
    has to stay safe, which is what the -1 sentinels are for."""
    host = QWidget()
    host.resize(900, 500)
    layout = QVBoxLayout(host)
    hidden = GhosttyTerminalWidget(host)
    hidden.hide()
    layout.addWidget(hidden)
    settle(300)
    try:
        assert hidden._master_fd == -1
        assert hidden._child_pid == -1
        hidden.shutdown()
    finally:
        if shiboken6.isValid(host):
            shiboken6.delete(host)
