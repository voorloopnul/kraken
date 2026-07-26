"""Off-thread plumbing for voice input.

Both halves of the feature block for a long time: a model download runs for
tens of seconds to minutes, and loading a model costs a second or two before
the first transcription. Neither may run on the GUI thread, and both need to
report back to it — downloads with progress, transcription with text — so
each gets a small QObject that owns a QRunnable and re-enters the GUI thread
through queued signals.

This mirrors kraken.shell.async_run, but keeps voice free of a dependency on
the shell package and adds the progress channel run_async has no room for.
"""

from __future__ import annotations

from typing import Callable

from PySide6.QtCore import QObject, QRunnable, QThreadPool, Signal

from kraken.voice import engine, models
from kraken.voice.models import VoiceModel


class _Job(QRunnable):
    def __init__(self, fn: Callable[[], None]):
        super().__init__()
        self._fn = fn

    def run(self) -> None:
        self._fn()


class Downloader(QObject):
    """Fetches a model, reporting progress on the GUI thread."""

    progress = Signal(int, int)  # bytes done, bytes total
    finished = Signal(bool, str)  # ok, error message ("" when cancelled)

    def __init__(self, parent: QObject | None = None):
        super().__init__(parent)
        self._cancelled = False

    def cancel(self) -> None:
        """Ask the worker to stop. It notices on the next block of bytes, so
        `finished` still arrives — with ok=False and no error text."""
        self._cancelled = True

    def start(self, model: VoiceModel) -> None:
        def work() -> None:
            try:
                models.download(
                    model,
                    on_progress=self.progress.emit,
                    should_cancel=lambda: self._cancelled,
                )
            except models.Cancelled:
                self.finished.emit(False, "")
            except BaseException as exc:  # noqa: BLE001 - surfaced in the UI
                self.finished.emit(False, str(exc))
            else:
                self.finished.emit(True, "")

        QThreadPool.globalInstance().start(_Job(work))


class Transcriber(QObject):
    """Turns a PCM buffer into text without blocking the window."""

    finished = Signal(str, str)  # text, error message

    def start(self, pcm: bytes) -> None:
        def work() -> None:
            try:
                text = engine.transcribe(pcm)
            except BaseException as exc:  # noqa: BLE001 - surfaced in the UI
                self.finished.emit("", str(exc))
            else:
                self.finished.emit(text, "")

        QThreadPool.globalInstance().start(_Job(work))
