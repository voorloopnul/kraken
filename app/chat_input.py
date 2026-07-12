"""Chat input box: a bordered container with an attachment chip row, a
multiline prompt field, and a footer row (attach, model selector on the
left; Ask mode and Send on the right). Emits `submitted` with the text and
any attached images."""

from __future__ import annotations

import base64
from pathlib import Path

from PySide6.QtCore import QBuffer, QEvent, QIODevice, QPoint, Qt, Signal
from PySide6.QtGui import QImageReader, QPixmap
from PySide6.QtWidgets import (
    QFileDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QPlainTextEdit,
    QToolButton,
    QToolTip,
    QVBoxLayout,
    QWidget,
)

from app.themes import DEFAULT_THEME

# Image formats model providers accept in the prompt payload, keyed by the
# format name Qt sniffs from the file's content. Anything else travels as a
# plain path reference like any other file.
_QT_FORMAT_MIMES = {
    "png": "image/png",
    "jpeg": "image/jpeg",
    "gif": "image/gif",
    "webp": "image/webp",
}


def _image_mime(data: bytes) -> str | None:
    """The provider mime type for raw image bytes, detected from the content
    itself (not the filename, which can lie — a JPEG saved as .png would else
    ship mislabeled and get rejected). None if it isn't a supported image."""
    buffer = QBuffer()
    buffer.setData(data)
    buffer.open(QIODevice.OpenModeFlag.ReadOnly)
    fmt = bytes(QImageReader(buffer).format()).decode("ascii", "replace").lower()
    return _QT_FORMAT_MIMES.get(fmt)


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
#modelMenu { background: #1b1c1f; border: 1px solid #33353c; border-radius: 8px; }
#modelMenu QLineEdit {
    background: #131417; border: 1px solid #33353c; border-radius: 4px;
    color: #d6d8dd; font-size: 12px; padding: 3px 6px;
}
#modelMenu QListWidget { background: transparent; border: none; color: #c8cad0; font-size: 12px; }
#modelMenu QListWidget::item { padding: 3px 6px; border-radius: 4px; }
#modelMenu QListWidget::item:selected { background: #26282e; color: #ffffff; }
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
#modelMenu { background: #ffffff; border: 1px solid #d8d8dd; border-radius: 8px; }
#modelMenu QLineEdit {
    background: #f2f2f4; border: 1px solid #d8d8dd; border-radius: 4px;
    color: #26282e; font-size: 12px; padding: 3px 6px;
}
#modelMenu QListWidget { background: transparent; border: none; color: #4a4d55; font-size: 12px; }
#modelMenu QListWidget::item { padding: 3px 6px; border-radius: 4px; }
#modelMenu QListWidget::item:selected { background: #e8e8ec; color: #1b1d22; }
#sep { color: #d0d0d5; }
#attachChip { background: #f2f2f4; border: 1px solid #d8d8dd; border-radius: 6px; }
#attachChip QLabel { color: #4a4d55; font-size: 11px; }
""",
}


class _ModelPopup(QFrame):
    """Searchable model picker for the chat footer: a filter field over the
    agent's configured model list. Closes on outside click, Esc, or pick."""

    selected = Signal(str, str)  # (provider, model_id)

    def __init__(
        self,
        models: list[dict],
        current_provider: str | None,
        current_id: str | None,
        parent: QWidget,
    ):
        super().__init__(parent, Qt.WindowType.Popup)
        self.setObjectName("modelMenu")
        self._models = models
        self._current = (current_provider, current_id)

        self._search = QLineEdit()
        self._search.setPlaceholderText("Search models…")
        self._search.textChanged.connect(self._refilter)
        self._search.installEventFilter(self)
        self._list = QListWidget()
        self._list.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._list.setTextElideMode(Qt.TextElideMode.ElideRight)
        self._list.itemClicked.connect(self._choose)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 6)
        layout.setSpacing(4)
        layout.addWidget(self._search)
        layout.addWidget(self._list)
        self.setFixedSize(400, 320)
        self.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)

        self._refilter("")
        self._search.setFocus()

    def _refilter(self, text: str) -> None:
        words = text.strip().lower().split()
        self._list.clear()
        current_row = None
        for model in self._models:
            provider = model.get("provider", "")
            model_id = model.get("id", "")
            haystack = f"{model_id} {model.get('name', '')} {provider}".lower()
            if not all(word in haystack for word in words):
                continue
            is_current = (provider, model_id) == self._current
            mark = "✓ " if is_current else "   "
            item = QListWidgetItem(f"{mark}{model_id}  [{provider}]")
            item.setData(Qt.ItemDataRole.UserRole, (provider, model_id))
            self._list.addItem(item)
            if is_current:
                current_row = self._list.count() - 1
        if self._list.count():
            self._list.setCurrentRow(current_row if current_row is not None else 0)

    def _choose(self, item: QListWidgetItem) -> None:
        provider, model_id = item.data(Qt.ItemDataRole.UserRole)
        self.close()
        self.selected.emit(provider, model_id)

    def eventFilter(self, obj, event) -> bool:
        # The search field keeps focus; arrows/Enter drive the list from it.
        if obj is self._search and event.type() == QEvent.Type.KeyPress:
            key = event.key()
            if key == Qt.Key.Key_Down:
                row = min(self._list.currentRow() + 1, self._list.count() - 1)
                self._list.setCurrentRow(row)
                return True
            if key == Qt.Key.Key_Up:
                self._list.setCurrentRow(max(self._list.currentRow() - 1, 0))
                return True
            if key in (Qt.Key.Key_Return, Qt.Key.Key_Enter):
                if self._list.currentItem() is not None:
                    self._choose(self._list.currentItem())
                return True
            if key == Qt.Key.Key_Escape:
                self.close()
                return True
        return super().eventFilter(obj, event)


class ChatInput(QFrame):
    """The prompt box; Send (or Ctrl+Enter) emits `submitted` and clears."""

    # (text, images, files) — images are prompt-ready dicts
    # {"type": "image", "data": <base64>, "mimeType": "image/png"}; files are
    # absolute path strings. The owner composes the wire message; keeping them
    # separate lets it title the session from the real text, not a path blob.
    submitted = Signal(str, list, list)
    # The model button was clicked; the owner fetches the agent's model list
    # and calls show_model_menu with it.
    model_menu_requested = Signal()
    # (provider, model_id) — the user picked a model from the menu.
    model_selected = Signal(str, str)

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
        self._model = tool_button("Model  ⌄")
        self._model.setToolTip("Switch model")
        self._model.clicked.connect(self.model_menu_requested)
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
        try:
            data = file.read_bytes()
        except OSError:
            # Vanished or unreadable between the dialog and now; fall back to a
            # path reference rather than crashing the slot.
            self._add_attachment(str(file.absolute()), file.name)
            return
        mime = _image_mime(data)
        if mime is not None:
            pixmap = QPixmap()
            if pixmap.loadFromData(data):
                image = {
                    "type": "image",
                    "data": base64.b64encode(data).decode("ascii"),
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

    def set_model_label(self, text: str | None) -> None:
        self._model.setText(f"{text or 'Model'}  ⌄")

    def notify_model(self, text: str) -> None:
        """Transient tooltip at the model button — feedback for when the menu
        can't open (e.g. the model list came back empty)."""
        pos = self._model.mapToGlobal(self._model.rect().center())
        QToolTip.showText(pos, text, self._model)

    def show_model_menu(
        self,
        models: list[dict],
        current_provider: str | None,
        current_id: str | None,
    ) -> None:
        """Pop a searchable model list at the model button. Entries mirror
        Pi's /model selector: the model id with a [provider] badge, the
        current selection marked."""
        popup = _ModelPopup(models, current_provider, current_id, self)
        popup.selected.connect(self.model_selected)
        corner = self._model.mapToGlobal(QPoint(0, 0))
        screen = self._model.screen().availableGeometry()
        x = min(corner.x(), screen.right() - popup.width())
        # The chat box sits at the bottom of the window, so prefer opening
        # upward; fall back to below the button when there's no room.
        y = corner.y() - popup.height() - 4
        if y < screen.top():
            y = corner.y() + self._model.height() + 4
        popup.move(x, y)
        popup.show()

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
        self.submitted.emit(text, images, files)
        self._edit.clear()
        self._clear_attachments()
