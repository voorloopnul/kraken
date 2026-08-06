"""Ask pi which models it offers, without a workspace to ask through.

The Models settings page needs the catalogue whether or not a session is open,
and pi keeps that answer to itself: the models a provider exposes are fetched
and cached by pi, merged with what models.json declares, and then narrowed to
the providers that actually have credentials. Reading those files ourselves
would be reimplementing the part of pi most likely to change.

So the question is put to pi itself. A short-lived `pi --mode rpc --no-session`
in the user's home folder answers `get_available_models` and is stopped as soon
as it has — no session file, no workspace, nothing left running.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from PySide6.QtCore import QObject, QTimer

from kraken import debug
from kraken.agent.pi_rpc import PiAgent

# How long to wait for the answer. Generous, because a first run may be
# fetching a provider's model list over the network, and this is not blocking
# anything: the page fills in when it arrives.
_TIMEOUT_MS = 20_000


def deliver_once(callback: Callable[[list], None]) -> Callable[[list], None]:
    """Wrap a model-list callback so it runs exactly once, and runs for certain.

    An answer can go missing without a word: a pi that dies drops the callbacks
    waiting on it (`PiAgent._on_finished`), and one that never starts never had
    them. A page waiting on this would sit on "asking…" for the rest of the run,
    so the wrapper answers `[]` itself once the timeout passes — whichever comes
    first wins, and the loser is ignored."""
    answered: list[bool] = []

    def deliver(models: list) -> None:
        if answered:
            return
        answered.append(True)
        callback(models)

    QTimer.singleShot(_TIMEOUT_MS, lambda: deliver([]))
    return deliver


class ModelQuery(QObject):
    """One throwaway pi, asked for its model list.

    Delivers exactly once — with the models, or with `[]` when pi could not be
    started, died first, or took too long. Keep a reference until it answers;
    parenting it to the widget that asked is enough."""

    def __init__(self, callback: Callable[[list], None], parent: QObject | None = None):
        super().__init__(parent)
        self._callback = callback
        self._deliver = deliver_once(self._finish)
        # Home, not a workspace: this pi is asked one question about global
        # configuration, and no folder it could be started in makes it a better
        # answer — while a project folder would drag in project settings.
        self._agent = PiAgent(str(Path.home()), parent=self, ephemeral=True)
        self._agent.failed.connect(self._on_failed)
        self._agent.finished.connect(lambda _: self._deliver([]))
        debug.proc("models.query")
        self._agent.get_available_models(self._on_models)

    def _on_models(self, response: dict) -> None:
        models = (response.get("data") or {}).get("models") or []
        self._deliver(models)

    def _on_failed(self, error: str) -> None:
        debug.proc("models.query failed", error=error)
        self._deliver([])

    def _finish(self, models: list) -> None:
        debug.proc("models.query answered", count=len(models))
        # Stop before the callback: the page may put a dialog on screen, and
        # this process has nothing left to do either way.
        self._agent.stop()
        self.deleteLater()
        self._callback(models)
