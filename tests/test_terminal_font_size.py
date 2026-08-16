"""Terminal font sizing is configurable, persistent, and live."""

import pytest

from kraken.terminal.typography import DEFAULT_SIZE, MAX_SIZE, MIN_SIZE, clamp
from kraken.terminal.widget import GhosttyTerminalWidget


def test_terminal_size_defaults_to_thirteen_points(qapp):
    terminal = GhosttyTerminalWidget()

    assert DEFAULT_SIZE == 13
    assert terminal._font_size == 13
    assert terminal.font().pointSizeF() == 13.0
    terminal.shutdown()


def test_terminal_size_changes_live(qapp):
    terminal = GhosttyTerminalWidget(font_size=15)
    old_cell_height = terminal._cell_h

    terminal.set_font_size(18)

    assert terminal._font_size == 18
    assert terminal.font().pointSizeF() == 18.0
    assert terminal._cell_h > old_cell_height
    terminal.shutdown()


@pytest.mark.parametrize(
    ("value", "expected"),
    [(0, MIN_SIZE), (999, MAX_SIZE), ("bad", DEFAULT_SIZE)],
)
def test_terminal_size_is_clamped(value, expected):
    assert clamp(value) == expected


def test_main_window_restores_persists_and_propagates_terminal_size(
    qapp, monkeypatch
):
    from kraken.shell import main_window as main_window_module

    saved = []
    monkeypatch.setattr(
        main_window_module,
        "load_state",
        lambda: {"terminal_font_size": 15, "workspaces": []},
    )
    monkeypatch.setattr(
        main_window_module, "save_state", lambda **changes: saved.append(changes)
    )
    window = main_window_module.MainWindow()
    applied = []
    window.views = {
        "fake": type(
            "View",
            (),
            {"set_terminal_font_size": lambda _self, size: applied.append(size)},
        )()
    }

    assert window._terminal_font_size == 15
    window.set_terminal_font_size(17)

    assert saved == [{"terminal_font_size": 17}]
    assert applied == [17]
    window.views = {}
    window.close()
