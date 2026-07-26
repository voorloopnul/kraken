"""Dialogs for the voice feature: the one-time download prompt and its
progress box. Both are modal — the mic can't do anything useful until the
model is on disk, so there's nothing to do in the window meanwhile."""

from __future__ import annotations

from PySide6.QtCore import QEventLoop, Qt
from PySide6.QtWidgets import QMessageBox, QProgressDialog, QWidget

from kraken.voice.models import PARAKEET
from kraken.voice.service import Downloader


def _confirm_download(parent: QWidget) -> bool:
    """Ask before spending several hundred megabytes of someone's bandwidth."""
    box = QMessageBox(parent)
    box.setWindowTitle("Voice input")
    box.setIcon(QMessageBox.Icon.Question)
    box.setText("Download the speech-to-text model?")
    box.setInformativeText(
        f"Dictation needs {PARAKEET.label} ({PARAKEET.download_label}), "
        "downloaded once into ~/.kraken/models.\n\n"
        "Your voice is then transcribed on this machine — nothing is uploaded."
    )
    download = box.addButton(
        f"Download ({PARAKEET.download_label})", QMessageBox.ButtonRole.AcceptRole
    )
    box.addButton(QMessageBox.StandardButton.Cancel)
    box.setDefaultButton(download)
    box.exec()
    return box.clickedButton() is download


def ensure_model(parent: QWidget) -> bool:
    """Make sure the model is on disk, asking and downloading if it isn't.
    False if it failed, or the user declined or cancelled."""
    if PARAKEET.is_installed():
        return True
    if not _confirm_download(parent):
        return False

    megabytes = PARAKEET.download_bytes / 1_000_000
    dialog = QProgressDialog(
        # Same shape as the progress label below, so the text doesn't jump
        # when the first block of bytes lands.
        f"Downloading {PARAKEET.label} — 0 of {megabytes:.0f} MB",
        "Cancel",
        0,
        1000,
        parent,
    )
    dialog.setWindowTitle("Voice input")
    dialog.setWindowModality(Qt.WindowModality.WindowModal)
    dialog.setMinimumDuration(0)
    # The dialog closes when the worker reports back, not when the bar fills:
    # the last file's bytes land slightly before the download call returns.
    dialog.setAutoClose(False)
    dialog.setAutoReset(False)
    dialog.setValue(0)

    downloader = Downloader(parent)
    outcome: dict[str, object] = {"ok": False, "error": ""}
    loop = QEventLoop()

    def on_progress(done: int, total: int) -> None:
        dialog.setLabelText(
            f"Downloading {PARAKEET.label} — "
            f"{done / 1_000_000:.0f} of {total / 1_000_000:.0f} MB"
        )
        dialog.setValue(int(done / max(total, 1) * 1000))

    def on_finished(ok: bool, error: str) -> None:
        outcome["ok"] = ok
        outcome["error"] = error
        loop.quit()

    downloader.progress.connect(on_progress)
    downloader.finished.connect(on_finished)
    dialog.canceled.connect(downloader.cancel)
    downloader.start(PARAKEET)
    loop.exec()
    dialog.close()

    if outcome["error"]:
        QMessageBox.warning(
            parent,
            "Voice input",
            f"Could not download {PARAKEET.label}.\n\n{outcome['error']}",
        )
    return bool(outcome["ok"])
