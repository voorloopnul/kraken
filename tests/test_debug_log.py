"""The diagnostics log.

These tests care about the properties that make the log usable *after* a
crash — that it is off by default, that records reach the file immediately
rather than sitting in a buffer, and that a clean exit is distinguishable from
a crash — rather than about the exact wording of any record.
"""

import subprocess
import sys

import pytest

from kraken import debug


@pytest.fixture
def log_path(tmp_path):
    """Start the log into a temp file, and always tear it down: the module
    keeps its sink in a global, so a test that left it open would write the
    next test's records into a deleted file."""
    path = tmp_path / "kraken.log"
    debug.start(str(path))
    yield path
    debug.shutdown(0)


def test_disabled_by_default_and_costs_nothing():
    # No start() call: every entry point must be a silent no-op, since these
    # sit on paths that run whether or not anyone asked for debugging.
    assert not debug.enabled()
    debug.action("noop", a=1)
    debug.log("noop")
    debug.proc("noop")
    debug.error("noop")
    debug.exception("noop", RuntimeError("boom"))


def test_records_are_readable_before_the_process_exits(log_path):
    debug.action("panel.toggle", side="right", visible=True)
    # No flush, no close: a crash right here must still leave the record on
    # disk, which is what line buffering buys.
    text = log_path.read_text()
    assert "panel.toggle side=right visible=True" in text


def test_actions_carry_a_memory_snapshot(log_path):
    debug.action("workspace.select", path="/tmp/x")
    line = [l for l in log_path.read_text().splitlines() if "workspace.select" in l][0]
    fields = dict(
        token.split("=", 1) for token in line.split("| ")[1].split() if "=" in token
    )
    assert set(fields) == {"rss", "tree", "d", "procs", "fds", "threads"}
    assert fields["rss"].endswith("MB")
    assert int(fields["procs"]) >= 1
    assert int(fields["threads"]) >= 1


def test_plain_events_skip_the_snapshot(log_path):
    debug.log("session.create", path="x")
    line = [l for l in log_path.read_text().splitlines() if "session.create" in l][0]
    assert "rss=" not in line


def test_exception_records_keep_the_traceback(log_path):
    try:
        raise ValueError("bad")
    except ValueError as exc:
        debug.exception("thing.failed", exc)
    text = log_path.read_text()
    assert "thing.failed ValueError: bad" in text
    assert "raise ValueError" in text  # the traceback body came along


def test_clean_exit_is_marked_and_a_crash_is_not(tmp_path):
    path = tmp_path / "clean.log"
    debug.start(str(path))
    debug.action("window.close")
    debug.shutdown(0)
    assert "clean shutdown code=0" in path.read_text()

    # A crash is the same log without that last line; nothing else marks it,
    # which is exactly why the marker has to be written on the way out.
    crashed = tmp_path / "crashed.log"
    debug.start(str(crashed))
    debug.action("window.close")
    text = crashed.read_text()
    debug.shutdown(0)
    assert "clean shutdown" not in text


def test_start_is_idempotent(tmp_path):
    first = tmp_path / "first.log"
    debug.start(str(first))
    try:
        assert debug.start(str(tmp_path / "second.log")) == first
        debug.action("only.here")
        assert "only.here" in first.read_text()
        assert not (tmp_path / "second.log").exists()
    finally:
        debug.shutdown(0)


def test_from_environment_reads_the_launcher_variables(monkeypatch):
    for name in ("KRAKEN_DEBUG", "KRAKEN_DEBUG_TRACE", "KRAKEN_DEBUG_HEARTBEAT"):
        monkeypatch.delenv(name, raising=False)
    assert debug.from_environment() is None

    monkeypatch.setenv("KRAKEN_DEBUG", "1")
    assert debug.from_environment() == debug.Settings(
        path=None, trace_input=False, heartbeat=debug.HEARTBEAT_DEFAULT
    )

    monkeypatch.setenv("KRAKEN_DEBUG", "/tmp/kraken.log")
    monkeypatch.setenv("KRAKEN_DEBUG_TRACE", "1")
    monkeypatch.setenv("KRAKEN_DEBUG_HEARTBEAT", "10")
    assert debug.from_environment() == debug.Settings(
        path="/tmp/kraken.log", trace_input=True, heartbeat=10.0
    )

    # A launcher exporting nonsense must not stop the app from starting.
    monkeypatch.setenv("KRAKEN_DEBUG_HEARTBEAT", "soon")
    assert debug.from_environment().heartbeat == debug.HEARTBEAT_DEFAULT

    monkeypatch.setenv("KRAKEN_DEBUG", "0")
    assert debug.from_environment() is None


def test_heartbeat_samples_while_nothing_happens(qapp, settle, tmp_path):
    """The point of the heartbeat: memory is sampled on a timer, so growth
    while the app sits idle is visible between actions instead of invisible."""
    path = tmp_path / "beat.log"
    debug.start(str(path), heartbeat=0.05)
    try:
        debug.install(qapp)
        settle(300)
        samples = [l for l in path.read_text().splitlines() if " sample " in l]
    finally:
        debug.shutdown(0)

    assert len(samples) >= 2, "the timer should have ticked more than once"
    assert "heartbeat idle=" in samples[0]
    assert "rss=" in samples[0] and "tree=" in samples[0]


def test_heartbeat_idle_resets_on_an_action(qapp, settle, tmp_path):
    """`idle=` is what separates growth caused by use from growth that happens
    on its own, so it has to be measured from the last action, not from
    start-up."""
    path = tmp_path / "idle.log"
    debug.start(str(path), heartbeat=0.05)
    try:
        debug.install(qapp)
        settle(200)
        debug.action("panel.toggle", side="right", visible=True)
        settle(120)
        lines = path.read_text().splitlines()
    finally:
        debug.shutdown(0)

    after = [l for l in lines[_index_of(lines, "panel.toggle") :] if " sample " in l]
    assert after, "expected a heartbeat after the action"
    idle = float(after[0].split("idle=")[1].split("s")[0])
    assert idle < 1.0


def _index_of(lines: list[str], needle: str) -> int:
    return next(i for i, line in enumerate(lines) if needle in line)


def test_heartbeat_can_be_switched_off(qapp, settle, tmp_path):
    path = tmp_path / "off.log"
    debug.start(str(path), heartbeat=0)
    try:
        debug.install(qapp)
        settle(200)
        text = path.read_text()
    finally:
        debug.shutdown(0)
    assert " sample " not in text
    assert debug._heartbeat_timer is None


def test_input_tracing_records_clicks_but_not_what_was_typed(qapp, tmp_path):
    """The opt-in tracer has to name the widget that was clicked — that's the
    whole point — while counting plain keystrokes rather than recording them.
    A log people paste into bug reports must not be a keylogger."""
    from PySide6.QtCore import QEvent, QPoint, QPointF, Qt
    from PySide6.QtGui import QKeyEvent, QMouseEvent
    from PySide6.QtWidgets import QWidget

    path = tmp_path / "trace.log"
    debug.start(str(path), trace_input=True)
    debug.install(qapp)
    widget = QWidget()
    widget.setObjectName("victim")
    try:
        press = QMouseEvent(
            QEvent.Type.MouseButtonPress,
            QPointF(3, 4),
            QPointF(30, 40),
            Qt.MouseButton.LeftButton,
            Qt.MouseButton.LeftButton,
            Qt.KeyboardModifier.NoModifier,
        )
        qapp.sendEvent(widget, press)
        for letter in "hunter2":
            qapp.sendEvent(
                widget,
                QKeyEvent(
                    QEvent.Type.KeyPress,
                    Qt.Key.Key_A,
                    Qt.KeyboardModifier.NoModifier,
                    letter,
                ),
            )
        # A non-printable key flushes the pending run of typing.
        qapp.sendEvent(
            widget,
            QKeyEvent(
                QEvent.Type.KeyPress, Qt.Key.Key_Return, Qt.KeyboardModifier.NoModifier
            ),
        )
        text = path.read_text()
    finally:
        qapp.removeEventFilter(debug._tracer)
        debug._tracer = None
        debug.shutdown(0)
        widget.deleteLater()

    assert "click button=left at=30,40" in text
    assert "QWidget#victim" in text
    assert "key.typing keys=7" in text
    assert "hunter2" not in text  # counted, never recorded
    assert "hunter" not in text


def test_metrics_describe_this_process():
    # Deliberately not asserting tree >= rss: the two are separate reads of a
    # live process, and normal allocation between them makes that flap. That
    # the tree covers descendants is what test_tree_rss_counts_a_child checks.
    assert debug.process_rss() > 0
    tree, count = debug.tree_stats()
    assert tree > 0
    assert count >= 1
    assert debug.open_fds() > 0
    assert debug.thread_count() >= 1


def test_tree_rss_counts_a_child(log_path):
    """The tree total is the number worth watching, so it has to actually
    include descendants — a per-process reading would miss the renderers,
    shells and agents that make up most of Kraken's footprint."""
    alone = debug.process_tree_rss()
    # A child that blocks reading stdin: alive for the measurement, and gone as
    # soon as the pipe closes. Spawned rather than forked — forking a process
    # that holds a Qt application is its own source of crashes.
    child = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdin.read()"],
        stdin=subprocess.PIPE,
    )
    try:
        with_child = debug.process_tree_rss()
        _, count = debug.tree_stats()
        assert with_child > alone
        assert count >= 2
    finally:
        child.stdin.close()
        child.wait(timeout=10)
