"""Chat input box: a bordered container with an attachment chip row, a
multiline prompt field, and a footer row (attach, dictate and model selector
on the left; effort selector and Send on the right). Emits `submitted` with
the text and any attached images."""

from __future__ import annotations

import base64
from pathlib import Path

from PySide6.QtCore import (
    QBuffer,
    QEvent,
    QIODevice,
    QMimeData,
    QPoint,
    Qt,
    QTimer,
    Signal,
)
from PySide6.QtGui import QImage, QImageReader, QPixmap
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

from kraken.ui.themes import DEFAULT_THEME

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


def _pasted_image_paths(source: QMimeData) -> list[str]:
    """Local image files carried by mime data — what a file manager puts on
    the clipboard when copying an image.

    All or nothing: if anything in the payload isn't a local image file, the
    whole paste falls through to Qt's default handling, so pasting a web link
    or a path still types text rather than silently attaching something. The
    format is sniffed from each file's header, matching _image_mime.
    """
    if not source.hasUrls():
        return []
    paths = []
    for url in source.urls():
        if not url.isLocalFile():
            return []
        path = url.toLocalFile()
        fmt = bytes(QImageReader(path).format()).decode("ascii", "replace").lower()
        if fmt not in _QT_FORMAT_MIMES:
            return []
        paths.append(path)
    return paths


class _PromptEdit(QPlainTextEdit):
    """The prompt field, with images routed to the attachment row.

    Qt funnels every paste (Ctrl+V, the context menu, middle-click) and every
    drop through insertFromMimeData, so overriding it here covers them all.
    Without it an image paste inserts nothing: QPlainTextEdit is text-only and
    silently drops the image data.

    It also sizes itself to its content, growing from _MIN_HEIGHT as lines are
    added and stopping at _MAX_HEIGHT, past which it scrolls like any text box.
    """

    image_pasted = Signal(QImage)
    files_pasted = Signal(list)

    # Three-ish lines empty, about sixteen before it stops growing. The cap
    # matters: the prompt shares the panel with the transcript, and a box that
    # grew without limit would push the conversation off the top of the screen.
    _MIN_HEIGHT = 64
    _MAX_HEIGHT = 320

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setFixedHeight(self._MIN_HEIGHT)
        # documentSizeChanged covers both new lines and rewrapping, which is
        # what a plain textChanged would miss when a long line reflows.
        self.document().documentLayout().documentSizeChanged.connect(self._fit_height)

    def _fit_height(self, *_size) -> None:
        document = self.document()
        layout = document.documentLayout()
        # Sum the laid-out blocks rather than multiplying a line count by a font
        # metric. QPlainTextDocumentLayout reports its documentSize height in
        # *lines*, not pixels, and lineSpacing is not what it actually spends
        # per line — deriving the pixels got it wrong in both directions at
        # once. blockBoundingRect is the height Qt really used, wrapping and all.
        content = 2 * document.documentMargin()
        block = document.firstBlock()
        while block.isValid():
            content += layout.blockBoundingRect(block).height()
            block = block.next()
        # Everything outside the viewport — frame, stylesheet padding, a
        # horizontal scrollbar if one shows — measured rather than derived.
        # frameWidth() and contentsMargins() both report the stylesheet's
        # padding, so adding them together counts it twice.
        chrome = self.height() - self.viewport().height()
        wanted = max(self._MIN_HEIGHT, min(round(content + chrome), self._MAX_HEIGHT))
        # Guard the resize: growing re-lays-out the viewport, which comes back
        # through documentSizeChanged.
        if wanted != self.height():
            self.setFixedHeight(wanted)

    def resizeEvent(self, event) -> None:
        # A narrower box rewraps its text into more lines, so the height has to
        # be recomputed from the width it actually ended up with.
        super().resizeEvent(event)
        self._fit_height()

    def canInsertFromMimeData(self, source: QMimeData) -> bool:
        # Also what enables Paste in the context menu.
        if source.hasImage() or _pasted_image_paths(source):
            return True
        return super().canInsertFromMimeData(source)

    def insertFromMimeData(self, source: QMimeData) -> None:
        # File paths win over any bitmap in the same payload: they carry the
        # original bytes and a real filename, where the bitmap would be a
        # re-encoded copy called "pasted".
        paths = _pasted_image_paths(source)
        if paths:
            self.files_pasted.emit(paths)
            return
        if source.hasImage():
            image = QImage(source.imageData())
            if not image.isNull():
                self.image_pasted.emit(image)
                return
        super().insertFromMimeData(source)


_STYLES = {
    "dark": """
#chatBox { background: #1b1c1f; border: 1px solid #33353c; border-radius: 10px; }
QPlainTextEdit {
    background: transparent; border: none; border-radius: 6px;
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
#modelMenu QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
#modelMenu QScrollBar::handle:vertical { background: #3a3d45; border-radius: 5px; min-height: 24px; }
#modelMenu QScrollBar::handle:vertical:hover { background: #4a4e58; }
#modelMenu QScrollBar::add-line, #modelMenu QScrollBar::sub-line { width: 0; height: 0; }
#modelMenu QScrollBar::add-page, #modelMenu QScrollBar::sub-page { background: transparent; }
#sep { color: #3a3c42; }
#attachChip { background: #26282e; border: 1px solid #33353c; border-radius: 6px; }
#attachChip QLabel { color: #c8cad0; font-size: 11px; }
""",
    "light": """
#chatBox { background: #ffffff; border: 1px solid #d8d8dd; border-radius: 10px; }
QPlainTextEdit {
    background: transparent; border: none; border-radius: 6px;
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
#modelMenu QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
#modelMenu QScrollBar::handle:vertical { background: #c9c4b4; border-radius: 5px; min-height: 24px; }
#modelMenu QScrollBar::handle:vertical:hover { background: #b3ae9e; }
#modelMenu QScrollBar::add-line, #modelMenu QScrollBar::sub-line { width: 0; height: 0; }
#modelMenu QScrollBar::add-page, #modelMenu QScrollBar::sub-page { background: transparent; }
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


class _EffortPopup(QFrame):
    """Small reasoning-effort picker for the chat footer: the current model's
    supported thinking levels, current marked. Closes on outside click, Esc,
    or pick. Reuses the model menu's styling via the shared object name."""

    selected = Signal(str)  # thinking level

    def __init__(self, levels: list[str], current: str | None, parent: QWidget):
        super().__init__(parent, Qt.WindowType.Popup)
        self.setObjectName("modelMenu")
        self._list = QListWidget()
        self._list.itemClicked.connect(self._choose)
        self._list.itemActivated.connect(self._choose)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 6)
        layout.setSpacing(4)
        layout.addWidget(self._list)
        self.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)

        current_row = None
        for level in levels:
            mark = "✓ " if level == current else "   "
            item = QListWidgetItem(f"{mark}{level.title()}")
            item.setData(Qt.ItemDataRole.UserRole, level)
            self._list.addItem(item)
            if level == current:
                current_row = self._list.count() - 1
        if self._list.count():
            self._list.setCurrentRow(current_row if current_row is not None else 0)
        self.setFixedWidth(180)
        self.setFixedHeight(min(max(self._list.count(), 1), 7) * 26 + 12)
        self._list.setFocus()

    def _choose(self, item: QListWidgetItem) -> None:
        level = item.data(Qt.ItemDataRole.UserRole)
        self.close()
        self.selected.emit(level)

    def keyPressEvent(self, event) -> None:
        if event.key() == Qt.Key.Key_Escape:
            self.close()
            return
        super().keyPressEvent(event)


class ChatInput(QFrame):
    """The prompt box; Send (or Enter) emits `submitted` and clears."""

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
    # The effort button was clicked; the owner fetches the model's thinking
    # levels and calls show_effort_menu with them.
    effort_menu_requested = Signal()
    # The user picked a reasoning effort level from the menu.
    effort_selected = Signal(str)

    # Mic button faces. Recording swaps in an elapsed timer, transcribing a
    # static marker — the button is the only progress indicator the feature
    # needs, so it never grows a spinner of its own.
    _MIC_IDLE = "🎙"
    _MIC_BUSY = "🎙 …"
    _MIC_COLOR = "#e0574f"

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("chatBox")
        # Pending attachments: (payload, chip widget). The payload is a
        # prompt-ready image dict for images, or an absolute path string for
        # other files (folded into the message text on submit, since the pi
        # RPC prompt only carries images).
        self._attachments: list[tuple[dict | str, QWidget]] = []

        # Voice input state. The recorder and transcriber are built on the
        # first mic click so a session that never dictates never imports
        # onnxruntime or opens an audio device.
        self._recorder = None
        self._transcriber = None
        self._voice_peak = 0.0
        # Set when the recording was closed by Enter, which means "send this
        # once you've transcribed it" rather than just "stop".
        self._submit_after_transcribe = False
        self._mic_timer = QTimer(self)
        self._mic_timer.setInterval(200)
        self._mic_timer.timeout.connect(self._tick_recording)

        self._edit = _PromptEdit()
        self._edit.setPlaceholderText(
            "Follow-up on this task, @ for mentions, / for commands"
        )
        # Sizes itself to its content; the scrollbar only appears at its cap.
        self._edit.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._edit.textChanged.connect(self._on_text_changed)
        self._edit.image_pasted.connect(self._attach_pasted_image)
        self._edit.files_pasted.connect(self._attach_paths)
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
        self._mic = tool_button(self._MIC_IDLE)
        self._mic.setToolTip("Dictate a prompt")
        self._mic.clicked.connect(self._toggle_recording)
        self._model = tool_button("Model  ⌄")
        self._model.setToolTip("Switch model")
        self._model.clicked.connect(self.model_menu_requested)
        self._effort = tool_button("Effort  ⌄")
        self._effort.setToolTip("Reasoning effort")
        self._effort.clicked.connect(self.effort_menu_requested)
        self._effort_sep = separator()
        self._send = tool_button("Send")
        self._send.setEnabled(False)
        self._send.clicked.connect(self._submit)

        footer = QHBoxLayout()
        footer.setContentsMargins(4, 0, 4, 2)
        footer.setSpacing(6)
        footer.addWidget(self._attach)
        footer.addWidget(self._mic)
        footer.addWidget(separator())
        footer.addWidget(self._model)
        footer.addStretch(1)
        footer.addWidget(self._effort)
        footer.addWidget(self._effort_sep)
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

    def _attach_pasted_image(self, image: QImage) -> None:
        """A bitmap pasted from the clipboard (a screenshot, a browser's
        "Copy image"). It has no filename of its own, so the chip gets a
        generic one."""
        self.attach_image(QPixmap.fromImage(image), "pasted.png")

    def _attach_paths(self, paths: list[str]) -> None:
        for path in paths:
            self._attach_path(path)

    def _pick_files(self) -> None:
        paths, _filter = QFileDialog.getOpenFileNames(
            self,
            "Attach images or files",
            "",
            "All files (*);;Images (*.png *.jpg *.jpeg *.gif *.webp)",
        )
        self._attach_paths(paths)

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

    # ---- Voice input -------------------------------------------------------

    def _notify_mic(self, text: str) -> None:
        QToolTip.showText(self._mic.mapToGlobal(self._mic.rect().center()), text, self._mic)

    def _toggle_recording(self) -> None:
        """Click to start, click again to transcribe. The model is downloaded
        on the first click ever, before the mic is opened — asking
        mid-recording would throw away whatever was said during the dialog."""
        if self._recorder is not None and self._recorder.is_recording:
            self._finish_recording()
            return

        from kraken.voice import ui as voice_ui
        from kraken.voice.recorder import Recorder, has_input_device

        if not has_input_device():
            self._notify_mic("No microphone found")
            return
        if not voice_ui.ensure_model(self):
            return

        if self._recorder is None:
            self._recorder = Recorder(self)
            self._recorder.level.connect(self._on_level)
        self._voice_peak = 0.0
        if not self._recorder.start():
            self._notify_mic("Could not open the microphone")
            return
        self._mic.setStyleSheet(f"color: {self._MIC_COLOR};")
        self._tick_recording()
        self._mic_timer.start()
        self._edit.setFocus()

    def _on_level(self, level: float) -> None:
        self._voice_peak = max(self._voice_peak, level)

    def _tick_recording(self) -> None:
        seconds = int(self._recorder.seconds)
        self._mic.setText(f"⏺ {seconds // 60}:{seconds % 60:02d}")

    def _finish_recording(self) -> None:
        pcm = self._recorder.stop()
        self._mic_timer.stop()
        # A take with no signal at all is almost always a muted or misrouted
        # device; transcribing it would just return nothing with no clue why.
        if self._voice_peak < 0.01:
            self._reset_mic()
            self._notify_mic("No sound was recorded — check your microphone")
            return

        from kraken.voice.service import Transcriber

        if self._transcriber is None:
            self._transcriber = Transcriber(self)
            self._transcriber.finished.connect(self._on_transcribed)
        self._mic.setEnabled(False)
        self._mic.setText(self._MIC_BUSY)
        self._transcriber.start(pcm)

    def _cancel_recording(self) -> None:
        if self._recorder is not None and self._recorder.is_recording:
            self._recorder.cancel()
            self._mic_timer.stop()
            self._reset_mic()

    def _reset_mic(self) -> None:
        # Back to idle, so any send the recording still owed is off too — the
        # one caller that honours it reads the flag before resetting.
        self._submit_after_transcribe = False
        self._mic.setEnabled(True)
        self._mic.setStyleSheet("")
        self._mic.setText(self._MIC_IDLE)

    def _on_transcribed(self, text: str, error: str) -> None:
        submit = self._submit_after_transcribe
        self._reset_mic()
        if error:
            self._notify_mic(f"Transcription failed: {error}")
            return
        if not text:
            self._notify_mic("Nothing was said")
            return
        # Insert rather than replace: dictation composes with what's already
        # typed, and lands where the cursor is.
        cursor = self._edit.textCursor()
        before = self._edit.toPlainText()[: cursor.selectionStart()]
        cursor.insertText(text if not before or before[-1].isspace() else " " + text)
        self._edit.setTextCursor(cursor)
        self._edit.setFocus()
        if submit:
            self._submit()

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

    def set_effort_label(self, level: str | None, supported: bool = True) -> None:
        """Show the current effort on the button. `supported=False` hides the
        selector entirely (the model has no thinking levels); otherwise a None
        level shows the placeholder until the level is known."""
        self._effort.setVisible(supported)
        self._effort_sep.setVisible(supported)
        self._effort.setText(f"{level.title()}  ⌄" if level else "Effort  ⌄")

    def notify_effort(self, text: str) -> None:
        """Transient tooltip at the effort button — feedback for when the menu
        can't open (e.g. the model has no thinking levels)."""
        pos = self._effort.mapToGlobal(self._effort.rect().center())
        QToolTip.showText(pos, text, self._effort)

    def show_effort_menu(self, levels: list[str], current: str | None) -> None:
        """Pop the reasoning-effort list at the effort button."""
        popup = _EffortPopup(levels, current, self)
        popup.selected.connect(self.effort_selected)
        corner = self._effort.mapToGlobal(QPoint(0, 0))
        screen = self._effort.screen().availableGeometry()
        x = min(corner.x(), screen.right() - popup.width())
        y = corner.y() - popup.height() - 4
        if y < screen.top():
            y = corner.y() + self._effort.height() + 4
        popup.move(x, y)
        popup.show()

    def text(self) -> str:
        return self._edit.toPlainText()

    def eventFilter(self, obj, event) -> bool:
        if obj is not self._edit or event.type() != QEvent.Type.KeyPress:
            return super().eventFilter(obj, event)
        if event.key() in (Qt.Key.Key_Return, Qt.Key.Key_Enter):
            # Enter sends, Shift+Enter breaks the line.
            if event.modifiers() & Qt.KeyboardModifier.ShiftModifier:
                return super().eventFilter(obj, event)
            # Enter while dictating means "that's the prompt": close the
            # recording and let the send wait for the transcript, which arrives
            # from a worker thread some time after _finish_recording returns.
            if self._recorder is not None and self._recorder.is_recording:
                self._submit_after_transcribe = True
                self._finish_recording()
                return True
            self._submit()
            return True
        if event.key() == Qt.Key.Key_Escape and self._recorder is not None:
            if self._recorder.is_recording:
                self._cancel_recording()
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
