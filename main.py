import os
import sys

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication

from kraken.main_window import MainWindow


def main() -> int:
    # Cap Chromium at one shared renderer process: by default every browser
    # tab gets its own (~80MB apiece). Must be in the environment before
    # QtWebEngine initializes; an externally set value is respected.
    os.environ.setdefault(
        "QTWEBENGINE_CHROMIUM_FLAGS", "--renderer-process-limit=1"
    )
    # Required before the QApplication exists for QtWebEngine (the browser
    # panel imports it lazily, after app startup).
    QApplication.setAttribute(Qt.ApplicationAttribute.AA_ShareOpenGLContexts)
    app = QApplication(sys.argv)
    app.setApplicationName("Kraken")
    app.setApplicationDisplayName("Kraken")
    window = MainWindow()
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
