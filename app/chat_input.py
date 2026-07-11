"""Chat input box: a bordered container with an attachment chip row, a
multiline prompt field, and a footer row (attach, model selector on the
left; Ask mode and Send on the right). Emits `submitted` with the text and
any attached images."""

from __future__ import annotations

import base64
import mimetypes
from pathlib import Path

from PySide6.QtCore import QBuffer, QIODevice, Qt, Signal
from PySide6.QtGui import QPixmap
from PySide6.QtWidgets import (
    QFileDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QPlainTextEdit,
    QToolButton,
    QVBoxLayout,
    QWidget,
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
#attachChip { background: #26282e; border: 1px solid #33353c; border-radius: 6px; }
#attachChip QLabel { color: #c8cad0; font-size: 11px; }
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
#attachChip { background: #f2f2f4; border: 1px solid #d8d8dd; border-radius: 6px; }
#attachChip QLabel { color: #4a4d55; font-size: 11px; }
""",
}


class ChatInput(QFrame):
    """The prompt box; Send (or Ctrl+Enter) emits `submitted` and clears."""

    # (text, images) — images are prompt-ready dicts:
    # {"type": "image", "data": <base64>, "mimeType": "image/png"}.
    submitted = Signal(str, list)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("chatBox")
        # Pending attachments: (payload, chip widget). The payload is a
        # prompt-ready image dict for images, or an absolute path string for
        # other files (folded into the message text on submit, since the pi
        # RPC prompt only carries images).
        self._attachments: list[tuple[dict | str, QWidget]] = []

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
        self._attach.setToolTip("Attach images or files")
        self._attach.clicked.connect(self._pick_files)
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

        # Attachment chips (thumbnail + name + remove) above the text field;
        # the row stays hidden while nothing is attached.
        self._attach_row = QWidget()
        self._attach_layout = QHBoxLayout(self._attach_row)
        self._attach_layout.setContentsMargins(2, 0, 2, 0)
        self._attach_layout.setSpacing(4)
        self._attach_layout.addStretch(1)
        self._attach_row.hide()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 4)
        layout.setSpacing(4)
        layout.addWidget(self._attach_row)
        layout.addWidget(self._edit)
        layout.addLayout(footer)

        self.set_theme(DEFAULT_THEME)

    def set_theme(self, name: str) -> None:
        self.setStyleSheet(_STYLES[name])

    # ---- Attachments -------------------------------------------------------

    # Image formats the model providers accept in the prompt payload; other
    # image types travel as plain file paths like any other file.
    _IMAGE_MIMES = {"image/png", "image/jpeg", "image/gif", "image/webp"}

    def attach_image(self, pixmap: QPixmap, name: str = "screenshot.png") -> None:
        """Queue an image to be sent with the next prompt, shown as a chip."""
        buffer = QBuffer()
        buffer.open(QIODevice.OpenModeFlag.WriteOnly)
        pixmap.save(buffer, "PNG")
        image = {
            "type": "image",
            "data": base64.b64encode(bytes(buffer.data())).decode("ascii"),
            "mimeType": "image/png",
        }
        self._add_attachment(image, name, thumbnail=pixmap)

    def _pick_files(self) -> None:
        paths, _filter = QFileDialog.getOpenFileNames(
            self,
            "Attach images or files",
            "",
            "All files (*);;Images (*.png *.jpg *.jpeg *.gif *.webp)",
        )
        for path in paths:
            self._attach_path(path)

    def _attach_path(self, path: str) -> None:
        """Attach one file: images go into the prompt's image payload, any
        other file rides along as a path reference in the message text."""
        file = Path(path)
        mime = mimetypes.guess_type(file.name)[0]
        if mime in self._IMAGE_MIMES:
            pixmap = QPixmap(str(file))
            if not pixmap.isNull():
                image = {
                    "type": "image",
                    "data": base64.b64encode(file.read_bytes()).decode("ascii"),
                    "mimeType": mime,
                }
                self._add_attachment(image, file.name, thumbnail=pixmap)
                return
        self._add_attachment(str(file.absolute()), file.name)

    def _add_attachment(
        self,
        payload: dict | str,
        name: str,
        thumbnail: QPixmap | None = None,
    ) -> None:
        chip = QFrame()
        chip.setObjectName("attachChip")
        chip_layout = QHBoxLayout(chip)
        chip_layout.setContentsMargins(4, 2, 2, 2)
        chip_layout.setSpacing(4)
        icon = QLabel()
        if thumbnail is not None:
            icon.setPixmap(
                thumbnail.scaledToHeight(24, Qt.TransformationMode.SmoothTransformation)
            )
        else:
            icon.setText("📄")
        chip_layout.addWidget(icon)
        chip_layout.addWidget(QLabel(name))
        remove = QToolButton()
        remove.setText("✕")
        remove.setToolTip("Remove attachment")
        remove.setCursor(Qt.CursorShape.PointingHandCursor)
        remove.clicked.connect(lambda: self._remove_attachment(chip))
        chip_layout.addWidget(remove)

        self._attachments.append((payload, chip))
        self._attach_layout.insertWidget(self._attach_layout.count() - 1, chip)
        self._attach_row.show()
        self._on_text_changed()
        self._edit.setFocus()

    def _remove_attachment(self, chip: QWidget) -> None:
        self._attachments = [(i, c) for i, c in self._attachments if c is not chip]
        chip.deleteLater()
        if not self._attachments:
            self._attach_row.hide()
        self._on_text_changed()

    def _clear_attachments(self) -> None:
        for _image, chip in self._attachments:
            chip.deleteLater()
        self._attachments.clear()
        self._attach_row.hide()
        self._on_text_changed()

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
        self._send.setEnabled(bool(self.text().strip()) or bool(self._attachments))

    def _submit(self) -> None:
        text = self.text().strip()
        if not text and not self._attachments:
            return
        images = [p for p, _chip in self._attachments if isinstance(p, dict)]
        files = [p for p, _chip in self._attachments if isinstance(p, str)]
        if files:
            refs = "\n".join(f"- {path}" for path in files)
            text = (f"{text}\n\n" if text else "") + f"Attached files:\n{refs}"
        self.submitted.emit(text, images)
        self._edit.clear()
        self._clear_attachments()
