"""Chat input box: a bordered container with a multiline prompt field and a
footer row (attach, model selector on the left; Ask mode and Send on the
right). Emits `submitted` with the text; no backend wired yet."""

from __future__ import annotations

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QPlainTextEdit,
    QToolButton,
    QVBoxLayout,
)

from app.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
#chatBox { background: #1b1c1f; border: 1px solid #4f83e0; border-radius: 10px; }
QPlainTextEdit {
    background: #131417; border: none; border-radius: 6px;
    color: #d6d8dd; padding: 4px 6px;
}
QToolButton {
    background: transparent; border: none; border-radius: 4px;
    color: #9a9da5; font-size: 12px; padding: 2px 6px;
}
QToolButton:hover { background: #26282e; color: #ffffff; }
QToolButton:disabled { color: #55575d; }
#sep { color: #3a3c42; }
""",
    "light": """
#chatBox { background: #ffffff; border: 1px solid #4f83e0; border-radius: 10px; }
QPlainTextEdit {
    background: #f2f2f4; border: none; border-radius: 6px;
    color: #26282e; padding: 4px 6px;
}
QToolButton {
    background: transparent; border: none; border-radius: 4px;
    color: #5a5d65; font-size: 12px; padding: 2px 6px;
}
QToolButton:hover { background: #e8e8ec; color: #1b1d22; }
QToolButton:disabled { color: #b0b2b8; }
#sep { color: #d0d0d5; }
""",
}


class ChatInput(QFrame):
    """The prompt box; Send (or Ctrl+Enter) emits `submitted` and clears."""

    submitted = Signal(str)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("chatBox")

        self._edit = QPlainTextEdit()
        self._edit.setPlaceholderText(
            "Follow-up on this task, @ for mentions, / for commands"
        )
        self._edit.setFixedHeight(64)
        self._edit.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._edit.textChanged.connect(self._on_text_changed)
        self._edit.installEventFilter(self)

        def tool_button(text: str) -> QToolButton:
            btn = QToolButton()
            btn.setText(text)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            return btn

        def separator() -> QLabel:
            sep = QLabel("|")
            sep.setObjectName("sep")
            return sep

        self._attach = tool_button("+")
        self._model = tool_button("Opus 4.6 (1M)  ⌄")
        self._mode = tool_button("Ask  ⌄")
        self._send = tool_button("Send")
        self._send.setEnabled(False)
        self._send.clicked.connect(self._submit)

        footer = QHBoxLayout()
        footer.setContentsMargins(4, 0, 4, 2)
        footer.setSpacing(6)
        footer.addWidget(self._attach)
        footer.addWidget(separator())
        footer.addWidget(self._model)
        footer.addStretch(1)
        footer.addWidget(self._mode)
        footer.addWidget(separator())
        footer.addWidget(self._send)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 4)
        layout.setSpacing(4)
        layout.addWidget(self._edit)
        layout.addLayout(footer)

        self.set_theme(DEFAULT_THEME)

    def set_theme(self, name: str) -> None:
        self.setStyleSheet(_STYLES[name])

    def set_model_label(self, text: str) -> None:
        self._model.setText(f"{text}  ⌄")

    def text(self) -> str:
        return self._edit.toPlainText()

    def eventFilter(self, obj, event) -> bool:
        from PySide6.QtCore import QEvent

        if (
            obj is self._edit
            and event.type() == QEvent.Type.KeyPress
            and event.key() in (Qt.Key.Key_Return, Qt.Key.Key_Enter)
            and event.modifiers() & Qt.KeyboardModifier.ControlModifier
        ):
            self._submit()
            return True
        return super().eventFilter(obj, event)

    def _on_text_changed(self) -> None:
        self._send.setEnabled(bool(self.text().strip()))

    def _submit(self) -> None:
        text = self.text().strip()
        if not text:
            return
        self.submitted.emit(text)
        self._edit.clear()
