"""Custom window decoration bar replacing the native title bar: the window's
close/minimize/zoom lights at the far left, then the History toggle, the
workspace folder and a git branch switcher; the focused conversation's title
in the center; memory usage on the right. Dragging the bar moves the window;
double-clicking toggles maximize. Shares the side bar's background so the
chrome reads as one surface."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

from PySide6.QtCore import QRectF, QSize, Qt, QTimer, Signal
from PySide6.QtGui import QColor, QIcon, QPainter, QPixmap
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
from kraken.ui.icons import icon, paint as paint_icon
from kraken.ui.themes import DEFAULT_THEME

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

# The bare QToolButton rules are a leftover of the window buttons having been
# grey circles; the traffic lights that replaced them paint themselves, so they
# opt out along with the branch switcher and the panel toggle.
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
#trafficLight { background: transparent; border: none; border-radius: 0; }
#trafficLight:hover { background: transparent; }
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
#trafficLight { background: transparent; border: none; border-radius: 0; }
#trafficLight:hover { background: transparent; }
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


# The window's own buttons are the platform's traffic lights: a filled circle
# each, carrying its glyph only while the pointer is over the group. The colours
# are the same in both themes, as macOS's are — they are the one piece of chrome
# that reads as the window rather than as the app.
_TRAFFIC_SIZE = 12
_TRAFFIC = {
    "close": ("#ff5f57", "#6b0500", "x"),
    "min": ("#febc2e", "#7d4900", "minus"),
    "max": ("#28c840", "#0a5c14", "maximize-2"),
    # The green light says what the click will do, so it inverts once the
    # window is already filling the screen.
    "restore": ("#28c840", "#0a5c14", "minimize-2"),
}
# The glyph sits inside the circle rather than filling it.
_TRAFFIC_GLYPH_SCALE = 0.62


def window_icon(kind: str, hovered: bool = False) -> QIcon:
    """One traffic light: its circle, and its glyph when `hovered`."""
    fill, glyph_color, glyph = _TRAFFIC[kind]
    scale = 2.0
    pixmap = QPixmap(int(_TRAFFIC_SIZE * scale), int(_TRAFFIC_SIZE * scale))
    pixmap.setDevicePixelRatio(scale)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(fill))
    painter.drawEllipse(QRectF(0, 0, _TRAFFIC_SIZE, _TRAFFIC_SIZE))
    if hovered:
        inner = _TRAFFIC_SIZE * _TRAFFIC_GLYPH_SCALE
        offset = (_TRAFFIC_SIZE - inner) / 2
        paint_icon(painter, QRectF(offset, offset, inner, inner), glyph, glyph_color)
    painter.end()
    return QIcon(pixmap)


class _TrafficLight(QToolButton):
    """A window button that reports the pointer entering and leaving it.

    The three light up together, as the platform's do: hovering any one of
    them shows the glyphs on all three, so the group reads as one control."""

    hovered = Signal(bool)

    def enterEvent(self, event) -> None:
        super().enterEvent(event)
        self.hovered.emit(True)

    def leaveEvent(self, event) -> None:
        super().leaveEvent(event)
        self.hovered.emit(False)


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

        # Whether the pointer is over any of the three, which is what decides
        # if the glyphs are showing.
        self._traffic_hovered = False
        self.buttons: dict[str, QToolButton] = {}
        for name in ("Close", "Minimize", "Maximize"):
            btn = _TrafficLight()
            btn.setObjectName("trafficLight")
            btn.setToolTip(name)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setFixedSize(_TRAFFIC_SIZE, _TRAFFIC_SIZE)
            btn.setIconSize(QSize(_TRAFFIC_SIZE, _TRAFFIC_SIZE))
            btn.hovered.connect(self._on_traffic_hover)
            self.buttons[name] = btn

        layout = QHBoxLayout(self)
        # The lights sit further from the left edge than the rest of the bar's
        # contents: they are what the window's rounded corner curves around.
        layout.setContentsMargins(14, 0, 8, 0)
        layout.setSpacing(6)
        # Close, minimize, zoom — the platform's order, at the platform's
        # spacing, which is wider than this bar's own.
        for name in ("Close", "Minimize", "Maximize"):
            layout.addWidget(self.buttons[name])
            layout.addSpacing(2)
        layout.addSpacing(8)
        layout.addWidget(self.left_panel_toggle)
        layout.addWidget(self.folder_label)
        layout.addSpacing(4)
        layout.addWidget(self.branch_button)
        layout.addStretch(1)
        layout.addWidget(self.conversation_label)
        layout.addStretch(1)
        layout.addWidget(self.memory_label)

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
        """Swap the zoom light's glyph to "restore" while maximized."""
        self._maximized = maximized
        self._refresh_traffic()
        self.buttons["Maximize"].setToolTip("Restore" if maximized else "Maximize")

    def _on_traffic_hover(self, hovered: bool) -> None:
        if hovered == self._traffic_hovered:
            return
        self._traffic_hovered = hovered
        self._refresh_traffic()

    def _refresh_traffic(self) -> None:
        """Repaint all three lights. They show their glyphs together, so a
        pointer entering any one of them redraws the group."""
        kinds = {
            "Close": "close",
            "Minimize": "min",
            "Maximize": "restore" if self._maximized else "max",
        }
        for name, kind in kinds.items():
            self.buttons[name].setIcon(window_icon(kind, self._traffic_hovered))

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
        self.branch_button.setIcon(icon("git-branch", color, 16))
        self.left_panel_toggle.setIcon(icon("panel-left", color, 16))
        # The lights carry no theme colour of their own, but the maximized
        # state still has to be re-applied here.
        self.set_maximized(self._maximized)
