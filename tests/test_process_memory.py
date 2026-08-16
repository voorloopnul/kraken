"""Kraken's process-memory snapshot and modal tree."""

from kraken import debug
from kraken.debug import ProcessMemory
from kraken.shell.process_dialog import ProcessDialog


class _Ps:
    pid = 999

    def communicate(self, timeout=None):
        root = debug.os.getpid()
        return (
            f"{root} 1 102400 /Applications/Kraken\n"
            f"{root + 1} {root} 51200 pi\n"
            f"{root + 2} {root + 1} 10240 bash\n"
            "777 1 999999 unrelated\n"
            "999 1 100 ps\n",
            "",
        )

    def kill(self):
        pass


def test_process_snapshot_contains_only_kraken_and_descendants(monkeypatch):
    monkeypatch.setattr(debug.subprocess, "Popen", lambda *args, **kwargs: _Ps())

    rows = debug.process_tree()

    root = debug.os.getpid()
    assert [(row.pid, row.depth, row.command) for row in rows] == [
        (root, 0, "/Applications/Kraken"),
        (root + 1, 1, "pi"),
        (root + 2, 2, "bash"),
    ]
    assert sum(row.rss for row in rows) == 163840 * 1024


def test_process_dialog_builds_a_hierarchy_and_total(qapp):
    rows = [
        ProcessMemory(10, 1, 100 * 1024**2, "/Applications/Kraken", 0),
        ProcessMemory(11, 10, 50 * 1024**2, "/usr/local/bin/pi", 1),
        ProcessMemory(12, 11, 25 * 1024**2, "/bin/zsh", 2),
    ]
    dialog = ProcessDialog(theme_name="dark", snapshot=lambda: rows)

    root = dialog._tree.topLevelItem(0)
    assert dialog._tree.topLevelItemCount() == 1
    assert root.text(0) == "Kraken"
    assert root.child(0).text(0) == "pi"
    assert root.child(0).child(0).text(0) == "zsh"
    assert root.child(0).text(2) == "50 MB"
    assert dialog._summary.text() == "3 processes · 175 MB resident"
