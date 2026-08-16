"""Modal process/memory view for the tree spawned by Kraken."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QPushButton,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from kraken.debug import ProcessMemory, format_bytes, process_tree
from kraken.shell.framed_dialog import FramedDialog
from kraken.ui.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
QTreeWidget#processTree { background: #282c34; border: 1px solid #3a3f4a;
    border-radius: 7px; color: #c8cad0; outline: none; }
QTreeWidget#processTree::item { padding: 5px 4px; }
QTreeWidget#processTree::item:selected { background: #26365e; }
QHeaderView::section { background: #23252b; color: #9a9da5;
    border: none; border-bottom: 1px solid #3a3f4a; padding: 6px; }
QLabel#processSummary { color: #9a9da5; }
""",
    "light": """
QTreeWidget#processTree { background: #ffffff; border: 1px solid #e1ded8;
    border-radius: 7px; color: #383a42; outline: none; }
QTreeWidget#processTree::item { padding: 5px 4px; }
QTreeWidget#processTree::item:selected { background: #dfe6f8; }
QHeaderView::section { background: #faf9f7; color: #8e8b86;
    border: none; border-bottom: 1px solid #e1ded8; padding: 6px; }
QLabel#processSummary { color: #8e8b86; }
""",
}


class ProcessDialog(FramedDialog):
    def __init__(
        self,
        parent: QWidget | None = None,
        theme_name: str = DEFAULT_THEME,
        snapshot: Callable[[], list[ProcessMemory]] = process_tree,
    ):
        super().__init__("Process Memory", parent)
        self._theme_name = theme_name
        self._snapshot = snapshot
        self.setMinimumSize(620, 420)

        self._summary = QLabel()
        self._summary.setObjectName("processSummary")
        self._tree = QTreeWidget()
        self._tree.setObjectName("processTree")
        self._tree.setHeaderLabels(["Process", "PID", "Memory"])
        self._tree.setRootIsDecorated(True)
        self._tree.setUniformRowHeights(True)
        self._tree.setAlternatingRowColors(False)
        self._tree.setColumnWidth(0, 360)
        self._tree.setColumnWidth(1, 90)

        refresh = QPushButton("Refresh")
        refresh.clicked.connect(self.refresh)
        actions = QHBoxLayout()
        actions.addWidget(self._summary)
        actions.addStretch(1)
        actions.addWidget(refresh)

        body = QWidget()
        layout = QVBoxLayout(body)
        layout.setContentsMargins(18, 16, 18, 18)
        layout.setSpacing(12)
        layout.addLayout(actions)
        layout.addWidget(self._tree, stretch=1)
        self.set_body(body)
        self.apply_theme(theme_name, _STYLES[theme_name])
        self.refresh()

    def refresh(self) -> None:
        rows = self._snapshot()
        self._tree.clear()
        items: dict[int, QTreeWidgetItem] = {}
        for row in rows:
            name = "Kraken" if row.depth == 0 else Path(row.command).name
            values = [name or row.command, str(row.pid), format_bytes(row.rss)]
            parent = items.get(row.ppid)
            item = (
                QTreeWidgetItem(parent, values)
                if parent is not None
                else QTreeWidgetItem(self._tree, values)
            )
            item.setToolTip(0, row.command)
            item.setTextAlignment(1, Qt.AlignmentFlag.AlignRight)
            item.setTextAlignment(2, Qt.AlignmentFlag.AlignRight)
            items[row.pid] = item
        self._tree.expandAll()
        total = sum(row.rss for row in rows)
        noun = "process" if len(rows) == 1 else "processes"
        self._summary.setText(f"{len(rows)} {noun} · {format_bytes(total)} resident")
