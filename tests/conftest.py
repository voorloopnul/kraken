"""Shared fixtures for the Qt widget tests.

Everything here runs on Qt's "offscreen" platform plugin: real widgets, real
layout, real signals, no window server. The platform is chosen before PySide is
imported, because it is read when the QGuiApplication is constructed.
"""

import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication


@pytest.fixture(scope="session")
def qapp():
    """One QApplication for the whole run — Qt allows only a single instance,
    and tearing it down mid-session tends to take the platform plugin with it."""
    app = QApplication.instance() or QApplication([])
    yield app
    app.processEvents()


@pytest.fixture
def settle(qapp):
    """Run the event loop for a moment.

    Widget code under test is full of deferred work — single-shot timers,
    coalesced repaints, lazy document layout — so assertions that follow a
    change need the loop to actually turn first.
    """

    def _settle(ms: int = 120) -> None:
        done = []
        QTimer.singleShot(ms, lambda: done.append(True))
        while not done:
            qapp.processEvents()

    return _settle
