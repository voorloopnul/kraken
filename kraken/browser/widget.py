"""A single browser tab: small nav toolbar + QWebEngineView."""

from __future__ import annotations

import time

from PySide6.QtCore import Qt, QUrl
from PySide6.QtGui import QColor
from PySide6.QtWebEngineCore import QWebEnginePage
from PySide6.QtWebEngineWidgets import QWebEngineView
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken import debug
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

_BLANK_URLS = ("", "about:blank")

_BROWSER_STYLES = {
    "dark": """
QToolButton {
    background: #26282e; border: 1px solid #33353c; border-radius: 4px;
    color: #c8cad0; font-size: 12px; padding: 2px 8px;
}
QToolButton:hover { background: #2c2e35; color: #ffffff; }
QLineEdit {
    background: #26282e; border: 1px solid #33353c; border-radius: 4px;
    color: #c8cad0; font-size: 12px; padding: 4px 8px;
}
""",
    "light": """
QToolButton {
    background: #f5f5f6; border: 1px solid #d8d8dd; border-radius: 4px;
    color: #4a4d55; font-size: 12px; padding: 2px 8px;
}
QToolButton:hover { background: #e8e8ec; color: #1b1d22; }
QLineEdit {
    background: #f5f5f6; border: 1px solid #d8d8dd; border-radius: 4px;
    color: #4a4d55; font-size: 12px; padding: 4px 8px;
}
""",
}


class BrowserWidget(QWidget):
    """One browser instance with back/forward/reload/address bar."""

    # A second crash this soon after an automatic reload is a page that kills
    # the renderer every time it renders, not a one-off fault.
    _RELOAD_GRACE = 10.0

    # New tabs start on about:blank: a blank page costs ~0 memory, while a
    # real start page costs its full content (~40MB for duckduckgo.com) in
    # every open tab.
    def __init__(self, parent: QWidget | None = None, initial_url: str = "about:blank"):
        super().__init__(parent)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

        back = QToolButton()
        back.setText("←")
        back.setToolTip("Back")
        back.setFixedSize(28, 24)
        back.setCursor(Qt.CursorShape.PointingHandCursor)

        forward = QToolButton()
        forward.setText("→")
        forward.setToolTip("Forward")
        forward.setFixedSize(28, 24)
        forward.setCursor(Qt.CursorShape.PointingHandCursor)

        reload = QToolButton()
        reload.setText("↻")
        reload.setToolTip("Reload")
        reload.setFixedSize(28, 24)
        reload.setCursor(Qt.CursorShape.PointingHandCursor)

        self._url_bar = QLineEdit(
            "" if initial_url == "about:blank" else initial_url
        )
        self._url_bar.setPlaceholderText("Enter address")
        self._url_bar.setCursor(Qt.CursorShape.IBeamCursor)

        go = QToolButton()
        go.setText("Go")
        go.setToolTip("Navigate")
        go.setFixedSize(36, 24)
        go.setCursor(Qt.CursorShape.PointingHandCursor)

        top = QHBoxLayout()
        top.setContentsMargins(6, 4, 6, 4)
        top.setSpacing(4)
        top.addWidget(back)
        top.addWidget(forward)
        top.addWidget(reload)
        top.addWidget(self._url_bar, 1)
        top.addWidget(go)

        self.web = QWebEngineView()

        # Shown in the web view's place when a crashed page is not worth
        # retrying. Built up front so the crash path never has to allocate.
        self._crash_notice = QWidget()
        self._crash_notice.setObjectName("crashNotice")
        self._crash_notice.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        crash_label = QLabel(
            "This page stopped responding and was closed.\n"
            "Reloading it crashed the browser again."
        )
        crash_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        retry = QToolButton()
        retry.setText("Reload")
        retry.setCursor(Qt.CursorShape.PointingHandCursor)
        retry.clicked.connect(self._reload_page)
        crash_layout = QVBoxLayout(self._crash_notice)
        crash_layout.setAlignment(Qt.AlignmentFlag.AlignCenter)
        crash_layout.setSpacing(12)
        crash_layout.addWidget(crash_label)
        crash_layout.addWidget(retry, alignment=Qt.AlignmentFlag.AlignCenter)
        self._crash_notice.hide()

        # Crash bookkeeping: when the last automatic reload went out, and
        # whether a tab that crashed while hidden still owes one.
        self._last_auto_reload = 0.0
        self._reload_pending = False

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addLayout(top)
        layout.addWidget(self.web, 1)
        layout.addWidget(self._crash_notice, 1)

        back.clicked.connect(self.web.back)
        forward.clicked.connect(self.web.forward)
        reload.clicked.connect(self._reload_page)
        go.clicked.connect(self._go)
        self._url_bar.returnPressed.connect(self._go)
        self.web.urlChanged.connect(self._update_url)
        self.web.page().renderProcessTerminated.connect(self._on_render_crash)

        self._theme_name = DEFAULT_THEME
        self.set_theme(DEFAULT_THEME)
        # Loaded last so the blank page is already tinted when it paints.
        self.web.load(QUrl(initial_url))

    # ---- Navigation ----------------------------------------------------

    def _go(self) -> None:
        self.navigate(self._url_bar.text())

    def navigate(self, url: str) -> None:
        url = url.strip()
        if not url:
            return
        if not url.startswith(("http://", "https://")):
            url = "https://" + url
        debug.action("browser.navigate", url=url)
        self._url_bar.setText(url)
        self._show_crash_notice(False)
        self.web.load(QUrl(url))

    def _update_url(self, url: QUrl) -> None:
        text = url.toString()
        self._url_bar.setText("" if text == "about:blank" else text)
        self._apply_page_background()

    # ---- State ---------------------------------------------------------

    @property
    def is_blank(self) -> bool:
        """True when no page is loaded — a fresh tab sits on about:blank."""
        return self.web.url().toString() in _BLANK_URLS

    @property
    def crashed(self) -> bool:
        """True while the tab is showing the renderer-crash notice, i.e. its
        page is gone and only an explicit reload will bring it back."""
        return self._crash_notice.isVisible()

    # ---- Renderer crashes ----------------------------------------------

    # main.py caps Chromium at a single renderer process for the whole app, so
    # a page that kills it blanks every open tab at once, in every workspace.
    # Qt reports the death and does nothing else — it never respawns, paints no
    # error page, and url() keeps returning the address that died — so without
    # the handling below a crash leaves the tab showing a white void under a
    # perfectly normal-looking address bar, forever.

    def _on_render_crash(self, status, exit_code: int) -> None:
        debug.error(
            "browser.render-terminated",
            status=int(status),
            code=exit_code,
            visible=self.isVisible(),
        )
        normal = QWebEnginePage.RenderProcessTerminationStatus.NormalTerminationStatus
        if status == normal:
            return  # an orderly shutdown (page closed), not a failure
        if self.crashed:
            # Already given up on this one: its page is hidden behind the notice
            # and only the user asking can bring it back. This death is some
            # other tab's doing, and reloading here would put a live page back
            # underneath a notice that says the tab is gone.
            return
        if not self.isVisible():
            # Hidden tabs are deliberately frozen to keep Chromium's footprint
            # down (see the lifecycle section below). Reviving all of them at
            # once — they all just died together — would undo that and pile
            # every reload into the one shared renderer, so a background tab
            # waits until someone actually looks at it.
            self._reload_pending = True
            return
        # An empty tab holds no content that could have killed anything — it
        # died only because it shares the one renderer with whichever tab did.
        # Never accuse it: just put a live page back underneath it.
        if self.is_blank:
            # Rate-limited all the same: whatever is killing the renderer will
            # keep killing it, and an unthrottled blank tab would just spin on
            # crash-reload-crash. Sitting still costs nothing — it's blank.
            if time.monotonic() - self._last_auto_reload < self._RELOAD_GRACE:
                return
            self._last_auto_reload = time.monotonic()
            self.web.reload()
            return
        # A rendering fault is usually transient, so the first one costs a
        # single silent reload. One that comes straight back is reproducible,
        # and retrying it forever would just wedge the browser.
        if time.monotonic() - self._last_auto_reload < self._RELOAD_GRACE:
            self._show_crash_notice(True)
            return
        self._last_auto_reload = time.monotonic()
        self.web.reload()

    def _reload_page(self) -> None:
        """Reload on the user's say-so — the toolbar button and the crash
        notice's Reload are the same action. Every reload has to take the
        notice down with it, or the page comes back invisible underneath it."""
        # An explicit retry earns a fresh automatic attempt next time.
        self._last_auto_reload = 0.0
        self._show_crash_notice(False)
        self.web.reload()

    def _show_crash_notice(self, crashed: bool) -> None:
        self._crash_notice.setVisible(crashed)
        self.web.setVisible(not crashed)

    # ---- Lifecycle -----------------------------------------------------

    # A tab is only ever the visible one when its panel is open and it's the
    # current tab in the stack; every other tab (background tab, or any tab
    # while the panel is hidden) is not visible, so freeze it. Qt forces a
    # visible page to Active and lets a hidden one be Frozen, which suspends
    # its timers/JS and lets Chromium reclaim work — so we just follow the
    # widget's own show/hide events.

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self._set_lifecycle(QWebEnginePage.LifecycleState.Active)
        if self._reload_pending:
            # Crashed while hidden; this is the first time it's been looked at.
            # It may have died once before while visible, so the notice can
            # already be up over the page this reload is about to bring back.
            self._reload_pending = False
            self._last_auto_reload = time.monotonic()
            self._show_crash_notice(False)
            self.web.reload()

    def hideEvent(self, event) -> None:
        super().hideEvent(event)
        self._set_lifecycle(QWebEnginePage.LifecycleState.Frozen)

    def _set_lifecycle(self, state: QWebEnginePage.LifecycleState) -> None:
        # Guard the teardown path: a hide can arrive as the C++ page is being
        # destroyed, where touching it raises RuntimeError.
        try:
            self.web.page().setLifecycleState(state)
        except RuntimeError:
            pass

    # ---- Theme ---------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        self.setStyleSheet(
            _BROWSER_STYLES[name]
            + f"#crashNotice {{ background: {ui['card']}; }}"
            f" #crashNotice QLabel {{ color: {ui['text']}; font-size: 13px; }}"
        )
        self._apply_page_background()

    def _apply_page_background(self) -> None:
        """Paint the canvas behind the document. A new tab sits on about:blank,
        which declares no background of its own, so Chromium's default shows
        through — a sheet of white in the middle of a dark window. Tint it to
        match the card instead.

        Only while blank, though: hand the white back the moment a real page
        loads. A site that declares no background expects the canvas to be
        white and picks its text colors against it, so keeping the dark tint
        would leave those pages dark-on-dark.
        """
        color = UI_COLORS[self._theme_name]["card"] if self.is_blank else "#ffffff"
        try:
            self.web.page().setBackgroundColor(QColor(color))
        except RuntimeError:  # page torn down mid-teardown, as in _set_lifecycle
            pass

    @property
    def theme_name(self) -> str:
        return self._theme_name
