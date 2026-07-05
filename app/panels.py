"""The three content panels. Replace the placeholder widgets with real content."""

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QFrame,
    QGraphicsDropShadowEffect,
    QLabel,
    QVBoxLayout,
    QWidget,
)

from app.themes import LIGHT, UI_COLORS


class Card(QFrame):
    """A rounded-border container to group panel content."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("card")
        self.set_colors("#%02X%02X%02X" % LIGHT.background, "#e0e0e0")
        shadow = QGraphicsDropShadowEffect(self)
        shadow.setBlurRadius(12)
        shadow.setOffset(0, 2)
        shadow.setColor(QColor(0, 0, 0, 40))
        self.setGraphicsEffect(shadow)

        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(12, 12, 12, 12)

    def set_colors(self, background: str, border: str) -> None:
        self.setStyleSheet(
            "#card {"
            f" background: {background};"
            f" border: 1px solid {border};"
            " border-radius: 8px;"
            "}"
        )

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)


class Panel(QWidget):
    """Base panel: a container with a vertical layout to add widgets to."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(8, 8, 8, 8)

    def add_widget(self, widget: QWidget, stretch: int = 0) -> None:
        self._layout.addWidget(widget, stretch)


class LeftPanel(Panel):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._card = Card()
        placeholder = QLabel("History")
        placeholder.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._card.add_widget(placeholder, stretch=1)
        self.add_widget(self._card, stretch=1)

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])


class CenterPanel(Panel):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        from app.chat_input import ChatInput

        placeholder = QLabel("Conversation")
        placeholder.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.add_widget(placeholder, stretch=1)
        self.chat = ChatInput()
        self.add_widget(self.chat)

    def set_theme(self, name: str) -> None:
        self.chat.set_theme(name)


class RightPanel(Panel):
    def __init__(self, parent: QWidget | None = None, cwd: str | None = None):
        super().__init__(parent)
        from app.terminal_tabs import TerminalTabs

        self._card = Card()
        self.terminals = TerminalTabs(self, cwd=cwd)
        self._card.add_widget(self.terminals, stretch=1)
        self.add_widget(self._card, stretch=1)

    @property
    def theme_name(self) -> str:
        return self.terminals.theme_name

    def set_theme(self, name: str) -> None:
        """Theme the card and everything inside it (tab strip, terminals)."""
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self.terminals.set_theme(name)
