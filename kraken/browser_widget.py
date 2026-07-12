"""A single browser tab: small nav toolbar + QWebEngineView."""

from __future__ import annotations

from PySide6.QtCore import Qt, QUrl
from PySide6.QtWebEngineWidgets import QWebEngineView
from PySide6.QtWidgets import QHBoxLayout, QLineEdit, QToolButton, QVBoxLayout, QWidget

from kraken.themes import DEFAULT_THEME

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
        self.web.load(QUrl(initial_url))

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addLayout(top)
        layout.addWidget(self.web, 1)

        back.clicked.connect(self.web.back)
        forward.clicked.connect(self.web.forward)
        reload.clicked.connect(self.web.reload)
        go.clicked.connect(self._go)
        self._url_bar.returnPressed.connect(self._go)
        self.web.urlChanged.connect(self._update_url)

        self._theme_name = DEFAULT_THEME
        self.set_theme(DEFAULT_THEME)

    # ---- Navigation ----------------------------------------------------

    def _go(self) -> None:
        self.navigate(self._url_bar.text())

    def navigate(self, url: str) -> None:
        url = url.strip()
        if not url:
            return
        if not url.startswith(("http://", "https://")):
            url = "https://" + url
        self._url_bar.setText(url)
        self.web.load(QUrl(url))

    def _update_url(self, url: QUrl) -> None:
        text = url.toString()
        self._url_bar.setText("" if text == "about:blank" else text)

    # ---- Theme ---------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self.setStyleSheet(_BROWSER_STYLES[name])

    @property
    def theme_name(self) -> str:
        return self._theme_name
