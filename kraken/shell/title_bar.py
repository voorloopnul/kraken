"""Custom window decoration bar replacing the native title bar: workspace
folder and a git branch switcher on the left, the focused conversation's
title in the center, and memory usage plus the minimize/maximize/close
buttons on the right. Dragging the bar moves the window; double-clicking
toggles maximize. Shares the side bar's background so the chrome reads as
one surface."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

from PySide6.QtCore import QPointF, QRectF, QSize, Qt, QTimer, Signal
from PySide6.QtGui import QColor, QIcon, QPainter, QPainterPath, QPen, QPixmap
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QMenu,
    QMessageBox,
    QToolButton,
    QWidget,
)

from kraken import debug
from kraken.debug import format_bytes, process_tree_rss
from kraken.shell.async_run import run_async
from kraken.ui.chrome import corner_style
from kraken.ui.themes import DEFAULT_THEME

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

# The bare QToolButton rules style the circular window buttons; the branch
# switcher opts out via its id selector.
_STYLES = {
    "dark": """
#titleBar { background: #1b1c21; border-bottom: 1px solid #33353c; }
#folderLabel { color: #c8cad0; font-weight: 600; }
#memoryLabel, #conversationLabel { color: #9a9da5; }
QToolButton { background: #2c2e35; border: none; border-radius: 11px; }
QToolButton:hover { background: #3a3d45; }
#branchButton { background: transparent; border-radius: 6px;
                color: #9a9da5; padding: 2px 8px 2px 6px; }
#branchButton:hover { background: #2c2e35; }
#branchButton::menu-indicator { image: none; }
#panelButton { background: transparent; border-radius: 6px; }
#panelButton:hover { background: #2c2e35; }
#panelButton:checked { background: #26282e; }
""",
    "light": """
#titleBar { background: #fafafa; border-bottom: 1px solid #e0e0e0; }
#folderLabel { color: #383a42; font-weight: 600; }
#memoryLabel, #conversationLabel { color: #5a5d65; }
QToolButton { background: #ebebee; border: none; border-radius: 11px; }
QToolButton:hover { background: #dcdce1; }
#branchButton { background: transparent; border-radius: 6px;
                color: #5a5d65; padding: 2px 8px 2px 6px; }
#branchButton:hover { background: #e8e8ec; }
#branchButton::menu-indicator { image: none; }
#panelButton { background: transparent; border-radius: 6px; }
#panelButton:hover { background: #e8e8ec; }
#panelButton:checked { background: #e0e0e5; }
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


# process_tree_rss / format_bytes live in kraken.debug: the diagnostics log
# reports the same numbers after every action, and one implementation keeps the
# label and the log from disagreeing.


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


def _sidebar_icon(color: str) -> QIcon:
    """Window outline with a divided left column: the History panel toggle."""
    pixmap = QPixmap(32, 32)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.2)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    painter.drawRoundedRect(QRectF(2.0, 3.0, 12.0, 10.0), 2.0, 2.0)
    painter.drawLine(QPointF(6.5, 3.0), QPointF(6.5, 13.0))
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
    # HEAD moved via the branch dropdown; lets the window refresh git views.
    branch_changed = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("titleBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedHeight(36)
        self._theme_name = DEFAULT_THEME
        # Set by MainWindow, which owns the window's shape.
        self._corner_radius = 0
        self._workspace_path: str | None = None
        # Set for a remote workspace: git runs on the host, and the folder
        # label shows the remote destination rather than the local anchor.
        self._remote: "RemoteTarget | None" = None
        # Cached remote branch state, filled by an async fetch (a remote git
        # call would block the UI). _branch_gen invalidates it on each switch.
        self._remote_branch: str = ""
        self._remote_branch_list: list[str] = []
        self._branch_gen = 0
        self._maximized = False

        # Shows/hides the History (left) panel; MainWindow keeps it in sync
        # with the panel action.
        self.left_panel_toggle = QToolButton()
        self.left_panel_toggle.setObjectName("panelButton")
        self.left_panel_toggle.setToolTip("Toggle History Panel")
        self.left_panel_toggle.setCursor(Qt.CursorShape.PointingHandCursor)
        self.left_panel_toggle.setCheckable(True)
        self.left_panel_toggle.setFixedSize(24, 24)
        self.left_panel_toggle.setIconSize(QSize(16, 16))

        self.folder_label = QLabel()
        self.folder_label.setObjectName("folderLabel")
        # Branch switcher: shows the current branch, clicking opens a menu of
        # the repo's local branches; picking one checks it out.
        self.branch_button = QToolButton()
        self.branch_button.setObjectName("branchButton")
        self.branch_button.setToolTip("Switch branch")
        self.branch_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.branch_button.setToolButtonStyle(
            Qt.ToolButtonStyle.ToolButtonTextBesideIcon
        )
        self.branch_button.setIconSize(QSize(16, 16))
        self.branch_button.setPopupMode(
            QToolButton.ToolButtonPopupMode.InstantPopup
        )
        self._branch_menu = QMenu(self.branch_button)
        self._branch_menu.aboutToShow.connect(self._populate_branch_menu)
        self.branch_button.setMenu(self._branch_menu)
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
        layout.setContentsMargins(8, 0, 8, 0)
        layout.setSpacing(6)
        layout.addWidget(self.left_panel_toggle)
        layout.addWidget(self.folder_label)
        layout.addSpacing(4)
        layout.addWidget(self.branch_button)
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

    def set_workspace(
        self, path: str | None, remote: "RemoteTarget | None" = None
    ) -> None:
        self._workspace_path = path or None
        self._remote = remote
        # New workspace: drop any in-flight/cached remote branch info.
        self._branch_gen += 1
        self._remote_branch = ""
        self._remote_branch_list = []
        if remote is not None:
            self.folder_label.setText(f"{remote.host.destination}:{remote.path}")
        else:
            self.folder_label.setText(path if path else "Kraken")
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
        # A remote branch lookup is a network round trip, too costly for the
        # 3s poll; it refreshes on workspace switch and checkout instead.
        if self._remote is None:
            self._refresh_branch()

    def _git_argv(self, git_args: list[str]) -> list[str]:
        """argv to run git for the current workspace, local or over SSH."""
        if self._remote is not None:
            return self._remote.git_argv(git_args)
        return ["git", "-C", self._workspace_path, *git_args]

    def _remote_git(self, git_args: list[str]) -> str:
        """stdout of a remote git command (stripped), or "" on failure. Blocking
        — call only from a worker thread (see _fetch_branch_info)."""
        try:
            result = subprocess.run(
                self._git_argv(git_args),
                capture_output=True,
                text=True,
                errors="replace",
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired):
            return ""
        return result.stdout.strip() if result.returncode == 0 else ""

    def _local_branches(self) -> list[str]:
        try:
            result = subprocess.run(
                self._git_argv(
                    ["for-each-ref", "--format=%(refname:short)", "refs/heads"]
                ),
                capture_output=True,
                text=True,
                errors="replace",
                timeout=15 if self._remote is not None else 5,
            )
        except (OSError, subprocess.TimeoutExpired):
            return []
        if result.returncode != 0:
            return []
        return result.stdout.split()

    def _refresh_branch(self) -> None:
        # Local: a cheap .git/HEAD read, done inline. Remote: fetch off-thread
        # so the branch lookup can't freeze a workspace switch.
        if self._remote is None:
            branch = git_branch(self._workspace_path) if self._workspace_path else ""
            self.branch_button.setText(branch)
            self.branch_button.setVisible(bool(branch))
            return
        gen = self._branch_gen
        run_async(
            self._fetch_branch_info,
            lambda info, g=gen: self._on_branch_info(g, info),
            self,
        )

    def _fetch_branch_info(self) -> tuple[str, list[str]]:
        """(current branch, local branch list) from the remote. Worker-thread
        safe: subprocess + parsing only, no Qt."""
        current = self._remote_git(["rev-parse", "--abbrev-ref", "HEAD"])
        if current == "HEAD":  # detached: show the short hash, like git_branch()
            current = self._remote_git(["rev-parse", "--short", "HEAD"])
        return current, self._local_branches()

    def _on_branch_info(self, gen: int, info) -> None:
        if gen != self._branch_gen or info is None:
            return  # superseded by a workspace switch, or the fetch failed
        current, branches = info
        self._remote_branch = current
        self._remote_branch_list = branches
        self.branch_button.setText(current)
        self.branch_button.setVisible(bool(current))

    # ---- Branch switching ---------------------------------------------------

    def _populate_branch_menu(self) -> None:
        """Rebuild the dropdown as it opens. Local git is read inline; the
        remote path uses the branch list cached by the last async fetch, so the
        menu opens instantly instead of blocking on an SSH round trip."""
        self._branch_menu.clear()
        if self._remote is not None:
            current, branches = self._remote_branch, self._remote_branch_list
            if not branches and not current:
                self._branch_menu.addAction("Loading…").setEnabled(False)
                self._refresh_branch()  # fill the cache for the next open
                return
        else:
            current = git_branch(self._workspace_path) if self._workspace_path else ""
            branches = self._local_branches()
        if not branches:
            self._branch_menu.addAction("No branches").setEnabled(False)
            return
        for name in branches:
            action = self._branch_menu.addAction(name)
            action.setCheckable(True)
            action.setChecked(name == current)
            if name != current:
                action.triggered.connect(
                    lambda _=False, b=name: self._switch_branch(b)
                )

    def _switch_branch(self, branch: str) -> None:
        debug.action(
            "branch.checkout", branch=branch, remote=self._remote is not None
        )
        # Remote checkout is a blocking round trip; run it off-thread.
        if self._remote is None:
            self._after_switch(self._run_checkout(branch))
        else:
            run_async(lambda: self._run_checkout(branch), self._after_switch, self)

    def _run_checkout(self, branch: str):
        """Run the checkout; return the CompletedProcess or the exception if it
        couldn't run. Worker-thread safe (no Qt)."""
        try:
            return subprocess.run(
                self._git_argv(["checkout", branch]),
                capture_output=True,
                text=True,
                errors="replace",
                timeout=15 if self._remote is not None else 10,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return exc

    def _after_switch(self, result) -> None:
        if isinstance(result, (OSError, subprocess.TimeoutExpired)):
            debug.error("branch.checkout failed", detail=str(result))
            QMessageBox.warning(self, "Checkout failed", str(result))
            return
        if result is None or result.returncode != 0:
            QMessageBox.warning(
                self,
                "Checkout failed",
                (result.stderr.strip() if result else "") or "git checkout failed",
            )
            return
        self._refresh_branch()
        self.branch_changed.emit()

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

    def set_corner_radius(self, radius: int) -> None:
        """Round the window's top corners, which are the title bar's own."""
        self._corner_radius = radius
        self._apply_style()

    def _apply_style(self) -> None:
        self.setStyleSheet(
            _STYLES[self._theme_name]
            + corner_style(
                "#titleBar", ("top-left", "top-right"), self._corner_radius
            )
        )

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._apply_style()
        color = _ICON_COLORS[name]
        self.branch_button.setIcon(_branch_icon(color))
        self.left_panel_toggle.setIcon(_sidebar_icon(color))
        self.buttons["Minimize"].setIcon(_window_icon("min", color))
        self.buttons["Close"].setIcon(_window_icon("close", color))
        self.set_maximized(self._maximized)
