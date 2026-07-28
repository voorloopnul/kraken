"""Web browser pane, the sibling of the terminal pane."""

from PySide6.QtWidgets import QWidget

from kraken.shell.panels.base import Panel
from kraken.ui.chrome import Card
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS


class BrowserPanel(Panel):
    """Web browser pane, the sibling of the terminal pane. The tabbed
    browser (and its Chromium processes) is only created the first time the
    panel becomes visible, so hidden panels cost nothing."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._card = Card(shadow=False)
        self._theme_name = DEFAULT_THEME
        self.browsers = None
        self._creating = False
        # Held (unparented, so nothing paints) until the tab strip it rides in
        # exists; the dock hands it over long before the panel is first shown.
        self._grip = None
        self.add_widget(self._card, stretch=1)

    def mount_grip(self, grip: QWidget) -> bool:
        """The grip rides at the head of the browser's own tab strip."""
        self._grip = grip
        if self.browsers is not None:
            self.browsers.mount_grip(grip)
        return True

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self._ensure_tabs()

    def _ensure_tabs(self) -> None:
        # Re-entrancy guard: web view construction can pump the event loop
        # (Chromium/GL init), letting a second showEvent arrive while the
        # first BrowserTabs is still mid-construction — without the flag
        # that stacked a duplicate browser into the card.
        if self.browsers is not None or self._creating:
            return
        self._creating = True
        try:
            from kraken.browser.tabs import BrowserTabs

            tabs = BrowserTabs(self)
            tabs.set_theme(self._theme_name)
            if self._grip is not None:
                tabs.mount_grip(self._grip)
            self._card.add_widget(tabs, stretch=1)
            self.browsers = tabs
        finally:
            self._creating = False

    def open_url(self, url: str) -> None:
        """Load a URL in the panel's current tab, revealing the panel (and
        building the lazily-created tabs) if needed."""
        self.show()
        self._ensure_tabs()
        if self.browsers is not None:
            self.browsers.open_url(url)

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        if self.browsers is not None:
            self.browsers.set_theme(name)
