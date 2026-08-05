"""Closing the last browser tab closes the panel and frees Chromium.

A hidden QWebEngineView keeps its renderer process alive, and that renderer is
where nearly all of the browser's memory lives — a debug log showed 202MB of
renderer plus 113MB of zygotes still resident long after the panel was hidden
and every tab "closed". Closing the last tab used to open a replacement one
immediately, so the view count never reached zero and the renderer never had a
moment with no live page to exit on.

So the last ✕ closes the panel and the panel destroys the tab strip. The tests
below fix the two halves of that (a replacement is not created; the strip is
really destroyed), the behaviour that must NOT change (closing one of several
tabs is ordinary), and the trap the teardown has to avoid: the dock's drag grip
is only lent to the strip, and destroying the strip while the grip is still
parented into it would take the dock's grip down with it.
"""

import pytest
import shiboken6
from PySide6.QtWebEngineCore import QWebEnginePage
from PySide6.QtWidgets import QWidget

from kraken.browser.tabs import BrowserTabs
from kraken.shell.panels.browser import BrowserPanel


@pytest.fixture
def tabs(qapp):
    """A bare tab strip, which starts with one tab."""
    return BrowserTabs()


@pytest.fixture
def panel(qapp):
    """A browser panel wired the way MainWindow wires it: `emptied` hides the
    panel, which is what lets the deferred teardown run."""
    p = BrowserPanel()
    p.emptied.connect(p.hide)
    return p


def close_tab(strip: BrowserTabs, index: int = 0) -> None:
    strip._close_browser(strip.browsers()[index])


# ---- The behaviour that must not change --------------------------------


def test_closing_one_of_two_tabs_leaves_the_other(tabs, settle):
    tabs.add_browser()
    assert len(tabs.browsers()) == 2

    seen = []
    tabs.emptied.connect(lambda: seen.append(True))
    close_tab(tabs)
    settle()

    assert len(tabs.browsers()) == 1
    assert seen == [], "a strip with tabs left must not ask to be closed"


# ---- The new behaviour --------------------------------------------------


def test_closing_the_last_tab_asks_to_be_closed(tabs, settle):
    seen = []
    tabs.emptied.connect(lambda: seen.append(True))

    close_tab(tabs)
    settle()

    assert seen == [True]
    assert tabs.browsers() == [], "no replacement tab; that is what pinned the renderer"


def test_the_panel_destroys_the_strip(panel, settle):
    panel.show()
    panel._ensure_tabs()
    strip = panel.browsers
    assert strip is not None

    close_tab(strip)
    settle()

    assert panel.browsers is None
    assert not shiboken6.isValid(strip), (
        "the strip must be destroyed, not just hidden — a live QWebEngineView "
        "keeps its renderer process"
    )


def test_the_dock_grip_survives_the_teardown(panel, settle):
    """The grip belongs to the dock and is only seated in the strip's layout.
    Destroying the strip with the grip still parented into it deletes the
    dock's grip, and the next mount touches a dead C++ object."""
    grip = QWidget()
    panel.mount_grip(grip)
    panel.show()
    panel._ensure_tabs()

    close_tab(panel.browsers)
    settle()

    assert shiboken6.isValid(grip), "the strip took the dock's grip down with it"


def test_showing_again_builds_a_fresh_strip(panel, settle):
    panel.show()
    panel._ensure_tabs()
    close_tab(panel.browsers)
    settle()
    assert panel.browsers is None

    panel.show()
    settle()

    assert panel.browsers is not None
    assert len(panel.browsers.browsers()) == 1


def test_a_hidden_tab_actually_freezes(tabs, settle):
    """Freezing is what suspends a background tab's timers and JS. Attempted
    from inside hideEvent it loses the race against the page's own visibility
    update, and Qt refuses it — "failed to transition from Active to Frozen
    state: page is visible" — so it silently never happened. Assert the state
    that results, not that the call was made."""
    tabs.show()
    background = tabs.browsers()[0]
    tabs.add_browser()  # the stack hides the first tab
    settle()

    assert not background.isVisible()
    assert (
        background.web.page().lifecycleState()
        == QWebEnginePage.LifecycleState.Frozen
    )


def test_a_panel_shown_again_before_the_teardown_keeps_a_tab(panel, settle):
    """The teardown is deferred, so a panel that comes back in between must be
    left usable rather than emptied out from under whoever re-opened it."""
    panel.show()
    panel._ensure_tabs()
    strip = panel.browsers

    close_tab(strip)  # emptied -> hide, teardown queued
    panel.show()  # ...but it is back before the queue turns
    settle()

    assert panel.browsers is strip
    assert shiboken6.isValid(strip)
    assert len(strip.browsers()) == 1
