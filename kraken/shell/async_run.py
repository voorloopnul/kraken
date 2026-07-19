"""Run blocking work off the GUI thread and deliver the result back on it.

Qt's UI is single-threaded: a slow subprocess — a remote git command over SSH,
an SSH connection test — run inline on the event loop freezes the whole window.
These helpers run such work on a QThreadPool worker and hand the result to a
callback on the GUI thread through a queued signal.

The callback is delivered by a small QObject parented to the caller-supplied
`receiver`. If the receiver is destroyed while the work is still running (e.g.
the panel is closed mid-fetch), Qt drops the pending delivery instead of firing
into a dead widget. The worker function must not touch Qt objects — it runs on
a background thread; return plain data and do the widget work in the callback.
"""

from __future__ import annotations

import subprocess
from typing import Callable

from PySide6.QtCore import QObject, QRunnable, QThreadPool, Signal


class _Failure:
    """Sentinel wrapping an exception raised by the worker function."""

    def __init__(self, exc: BaseException):
        self.exc = exc


class _ResultSignal(QObject):
    done = Signal(object)


class _FnRunnable(QRunnable):
    def __init__(self, fn: Callable[[], object], signal: _ResultSignal):
        super().__init__()
        self._fn = fn
        self._signal = signal

    def run(self) -> None:
        try:
            result: object = self._fn()
        except BaseException as exc:  # noqa: BLE001 - reported to the callback
            result = _Failure(exc)
        # Emitted from the worker thread; the receiver lives in the GUI thread,
        # so Qt queues this onto the GUI event loop.
        self._signal.done.emit(result)


class _Caller(QObject):
    def __init__(self, on_result: Callable[[object], None], parent: QObject):
        super().__init__(parent)
        self._on_result = on_result

    def deliver(self, result: object) -> None:
        try:
            self._on_result(None if isinstance(result, _Failure) else result)
        finally:
            self.deleteLater()


def run_async(
    fn: Callable[[], object],
    on_result: Callable[[object], None],
    receiver: QObject,
) -> None:
    """Run `fn()` on a worker thread; call `on_result(value)` on the GUI thread
    with its return value (or None if it raised). `receiver` owns the callback:
    if it is destroyed before `fn` finishes, the result is dropped."""
    signal = _ResultSignal()
    caller = _Caller(on_result, receiver)
    # Keep the signal alive on the GUI side until delivery; the runnable also
    # holds it for the duration of the worker run.
    caller._signal = signal  # type: ignore[attr-defined]
    signal.done.connect(caller.deliver)
    QThreadPool.globalInstance().start(_FnRunnable(fn, signal))


def run_command_async(
    argv: list[str],
    on_result: Callable[[object], None],
    receiver: QObject,
    **run_kwargs,
) -> None:
    """Convenience wrapper: run `subprocess.run(argv, **run_kwargs)` off-thread
    and deliver the CompletedProcess (or None on OSError/timeout) to
    `on_result` on the GUI thread."""

    def work() -> object:
        try:
            return subprocess.run(argv, **run_kwargs)
        except (OSError, subprocess.TimeoutExpired):
            return None

    run_async(work, on_result, receiver)
