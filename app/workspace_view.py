"""One workspace's set of panes — history (left), conversation (center), and
terminals (right) in a splitter. MainWindow keeps one WorkspaceView per open
workspace in a stack and switches between them, so each workspace keeps its
own pane state, chat input, and running shells (spawned in the workspace
folder)."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QSplitter, QVBoxLayout, QWidget

from app.panels import CenterPanel, LeftPanel, RightPanel
from app.themes import UI_COLORS


class WorkspaceView(QWidget):
    def __init__(self, path: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.path = path

        self.left_panel = LeftPanel()
        self.center_panel = CenterPanel()
        self.right_panel = RightPanel(cwd=path)

        splitter = QSplitter(Qt.Orientation.Horizontal)
        splitter.addWidget(self.left_panel)
        splitter.addWidget(self.center_panel)
        splitter.addWidget(self.right_panel)

        # Center panel absorbs extra space when the window resizes.
        splitter.setStretchFactor(0, 0)
        splitter.setStretchFactor(1, 1)
        splitter.setStretchFactor(2, 0)
        splitter.setSizes([220, 500, 480])
        splitter.setChildrenCollapsible(False)
        self._splitter = splitter

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(splitter)

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        # Styling the splitter covers the view background and cascades text
        # color to every panel label.
        self._splitter.setStyleSheet(
            f"QSplitter {{ background: {ui['window']}; }}"
            f" QLabel {{ color: {ui['text']}; }}"
        )
        self.left_panel.set_theme(name)
        self.center_panel.set_theme(name)
        self.right_panel.set_theme(name)

    def shutdown(self) -> None:
        self.right_panel.terminals.shutdown_all()
