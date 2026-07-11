"""Custom window decoration bar replacing the native title bar: workspace
folder and git branch on the left, the focused conversation's title in the
center, and memory usage plus the minimize/maximize/close buttons on the
right. Dragging the bar moves the window; double-clicking toggles maximize.
Shares the side bar's background so the chrome reads as one surface."""

from __future__ import annotations

import os
from pathlib import Path

from PySide6.QtCore import QPointF, QRectF, QSize, Qt, QTimer
from PySide6.QtGui import QColor, QIcon, QPainter, QPainterPath, QPen, QPixmap
from PySide6.QtWidgets import QHBoxLayout, QLabel, QToolButton, QWidget

from app.themes import DEFAULT_THEME

_STYLES = {
    "dark": """
#titleBar { background: #1b1c21; border-bottom: 1px solid #33353c; }
#folderLabel { color: #c8cad0; font-weight: 600; }
#branchLabel, #memoryLabel, #conversationLabel { color: #9a9da5; }
QToolButton { background: #2c2e35; border: none; border-radius: 11px; }
QToolButton:hover { background: #3a3d45; }
""",
    "light": """
#titleBar { background: #fafafa; border-bottom: 1px solid #e0e0e0; }
#folderLabel { color: #383a42; font-weight: 600; }
#branchLabel, #memoryLabel, #conversationLabel { color: #5a5d65; }
QToolButton { background: #ebebee; border: none; border-radius: 11px; }
QToolButton:hover { background: #dcdce1; }
""",
}

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def git_branch(path: str) -> str:
    """Current branch of the repo containing `path`, or "" when not in one.
    Reads .git/HEAD directly (following worktree/submodule gitdir files), so
    it's cheap enough to poll without spawning git."""
    for candidate in (Path(path), *Path(path).parents):
        git = candidate / ".git"
        if git.is_dir():
            head = git / "HEAD"
        elif git.is_file():
            try:
                gitdir = git.read_text().split(":", 1)[1].strip()
            except (OSError, IndexError):
                return ""
            head = (candidate / gitdir).resolve() / "HEAD"
        else:
            continue
        try:
            content = head.read_text().strip()
        except OSError:
            return ""
        if content.startswith("ref:"):
            ref = content.split(":", 1)[1].strip()
            return ref.removeprefix("refs/heads/")
        return content[:8]  # detached HEAD: short commit hash
    return ""


def process_tree_rss() -> int:
    """Resident set size in bytes of this process and all its descendants
    (QtWebEngine renderers, terminal shells, agent processes)."""
    total = 0
    stack = [os.getpid()]
    while stack:
        pid = stack.pop()
        proc = Path(f"/proc/{pid}")
        try:
            with open(proc / "status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        total += int(line.split()[1]) * 1024
                        break
            # Children are listed per thread, so walk every task dir.
            for task in (proc / "task").iterdir():
                children = (task / "children").read_text().split()
                stack.extend(int(child) for child in children)
        except OSError:
            continue  # process exited mid-scan
    return total


def format_bytes(size: int) -> str:
    if size >= 1024**3:
        return f"{size / 1024**3:.1f} GB"
    return f"{size // 1024**2} MB"


def _branch_icon(color: str) -> QIcon:
    """Git branch: a trunk with top/bottom nodes and a branch node curving in."""
    pixmap = QPixmap(32, 32)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.3)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawEllipse(QPointF(4.5, 3.8), 1.8, 1.8)
    painter.drawEllipse(QPointF(4.5, 12.2), 1.8, 1.8)
    painter.drawEllipse(QPointF(11.5, 6.2), 1.8, 1.8)
    painter.drawLine(QPointF(4.5, 5.6), QPointF(4.5, 10.4))
    path = QPainterPath(QPointF(4.5, 10.0))
    path.cubicTo(QPointF(4.5, 8.0), QPointF(11.5, 9.5), QPointF(11.5, 8.0))
    painter.drawPath(path)
    painter.end()
    return QIcon(pixmap)


def _window_icon(kind: str, color: str) -> QIcon:
    """Minimize / maximize / restore / close glyphs on a 12x12 canvas."""
    pixmap = QPixmap(24, 24)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.2)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    if kind == "min":
        painter.drawLine(QPointF(2.5, 8.5), QPointF(9.5, 8.5))
    elif kind == "max":
        painter.drawRect(QRectF(2.5, 2.5, 7.0, 7.0))
    elif kind == "restore":
        painter.drawRect(QRectF(2.5, 4.5, 5.0, 5.0))
        painter.drawPolyline(
            [QPointF(4.5, 2.5), QPointF(9.5, 2.5), QPointF(9.5, 7.5)]
        )
    elif kind == "close":
        painter.drawLine(QPointF(3.0, 3.0), QPointF(9.0, 9.0))
        painter.drawLine(QPointF(9.0, 3.0), QPointF(3.0, 9.0))
    painter.end()
    return QIcon(pixmap)


class TitleBar(QWidget):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("titleBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedHeight(36)
        self._workspace_path: str | None = None
        self._maximized = False

        self.folder_label = QLabel()
        self.folder_label.setObjectName("folderLabel")
        self._branch_icon = QLabel()
        self.branch_label = QLabel()
        self.branch_label.setObjectName("branchLabel")
        self.conversation_label = QLabel()
        self.conversation_label.setObjectName("conversationLabel")
        self.memory_label = QLabel()
        self.memory_label.setObjectName("memoryLabel")

        self.buttons: dict[str, QToolButton] = {}
        for name in ("Minimize", "Maximize", "Close"):
            btn = QToolButton()
            btn.setToolTip(name)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(22, 22)
            btn.setIconSize(QSize(12, 12))
            self.buttons[name] = btn

        layout = QHBoxLayout(self)
        layout.setContentsMargins(12, 0, 8, 0)
        layout.setSpacing(6)
        layout.addWidget(self.folder_label)
        layout.addSpacing(4)
        layout.addWidget(self._branch_icon)
        layout.addWidget(self.branch_label)
        layout.addStretch(1)
        layout.addWidget(self.conversation_label)
        layout.addStretch(1)
        layout.addWidget(self.memory_label)
        layout.addSpacing(6)
        for btn in self.buttons.values():
            layout.addWidget(btn)

        # One slow tick keeps the live readouts fresh: memory always drifts,
        # and the branch can change under us (checkout in a terminal).
        self._timer = QTimer(self)
        self._timer.setInterval(3000)
        self._timer.timeout.connect(self._refresh)
        self._timer.start()

        self.set_theme(DEFAULT_THEME)
        self._refresh()

    # ---- Content ----------------------------------------------------------

    def set_workspace(self, path: str) -> None:
        self._workspace_path = path
        self.folder_label.setText(Path(path).name)
        self._refresh_branch()

    def set_conversation(self, title: str) -> None:
        fm = self.conversation_label.fontMetrics()
        self.conversation_label.setText(
            fm.elidedText(title, Qt.TextElideMode.ElideRight, 420)
        )

    def set_maximized(self, maximized: bool) -> None:
        """Swap the maximize button's glyph to "restore" while maximized."""
        self._maximized = maximized
        color = _ICON_COLORS[self._theme_name]
        kind = "restore" if maximized else "max"
        self.buttons["Maximize"].setIcon(_window_icon(kind, color))
        self.buttons["Maximize"].setToolTip("Restore" if maximized else "Maximize")

    def _refresh(self) -> None:
        self.memory_label.setText(format_bytes(process_tree_rss()))
        self._refresh_branch()

    def _refresh_branch(self) -> None:
        branch = git_branch(self._workspace_path) if self._workspace_path else ""
        self.branch_label.setText(branch)
        self._branch_icon.setVisible(bool(branch))

    # ---- Window dragging ----------------------------------------------------

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            self.window().windowHandle().startSystemMove()
            event.accept()

    def mouseDoubleClickEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            window = self.window()
            if window.isMaximized():
                window.showNormal()
            else:
                window.showMaximized()
            event.accept()

    # ---- Theme --------------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self.setStyleSheet(_STYLES[name])
        color = _ICON_COLORS[name]
        pixmap = _branch_icon(color).pixmap(QSize(16, 16))
        self._branch_icon.setPixmap(pixmap)
        self.buttons["Minimize"].setIcon(_window_icon("min", color))
        self.buttons["Close"].setIcon(_window_icon("close", color))
        self.set_maximized(self._maximized)
