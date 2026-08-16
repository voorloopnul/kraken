"""Custom window decoration bar replacing the native title bar: the window's
close/minimize/zoom lights at the far left, then the History toggle, and then
two stacked lines naming what is open — the focused conversation's title, with
a menu of the actions that apply to it, over the workspace folder and a git
branch switcher. Memory usage sits at the right. Dragging the bar moves the
window; double-clicking toggles maximize. Shares the side bar's background so
the chrome reads as one surface."""

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
    QVBoxLayout,
    QWidget,
)

from kraken import debug
from kraken.debug import format_bytes, process_tree_rss
from kraken.shell.async_run import run_async
from kraken.ui.chrome import corner_style
from kraken.ui.icons import icon, paint as paint_icon, toggle_icon
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}
# The idle fill for the bar's own round buttons.
_BUTTON_COLORS = {
    "dark": {"idle": "#2c2e35", "hover": "#3a3d45"},
    "light": {"idle": "#eeece8", "hover": "#e2e0db"},
}


def _style(theme: str) -> str:
    """The bar's surface, its two lines of text, and the controls on it.

    The bar sits on the base surface rather than on the shade the panel headers
    and the side strips wear: those run along one edge of the content and frame
    it, while this spans the whole window above all of it. The hairline along
    its bottom is what separates the two.

    The bare QToolButton rules are a leftover of the window buttons having been
    grey circles; the traffic lights that replaced them paint themselves, so
    they opt out along with the branch switcher and the panel toggle."""
    ui = UI_COLORS[theme]
    c = _BUTTON_COLORS[theme]
    return f"""
#titleBar {{ background: {ui['window']}; border-bottom: 1px solid {ui['card_border']}; }}
#conversationLabel {{ color: {ui['text']}; font-weight: 600; }}
#folderLabel, #memoryLabel {{ color: {_ICON_COLORS[theme]}; font-size: 11px; }}
QToolButton {{ background: {c['idle']}; border: none; border-radius: 11px; }}
QToolButton:hover {{ background: {c['hover']}; }}
#branchButton {{ background: transparent; border-radius: 5px; font-size: 11px;
                color: {_ICON_COLORS[theme]}; padding: 1px 6px 1px 4px; }}
#branchButton:hover {{ background: {ui['hover']}; }}
#branchButton::menu-indicator {{ image: none; }}
#panelButton, #sessionButton {{ background: transparent; border-radius: 6px; }}
#panelButton:hover, #sessionButton:hover {{ background: {ui['hover']}; }}
/* The History panel's toggle answers the same question the side strip's
   buttons do, so it is marked in the same blue — softened, because it sits
   among text rather than in a strip of its own. */
#panelButton:checked {{ background: {ui['accent_soft']}; }}
#sessionButton::menu-indicator {{ image: none; }}
#memoryLabel {{ background: transparent; border-radius: 5px; padding: 3px 6px; }}
#memoryLabel:hover {{ background: {ui['hover']}; }}
#trafficLight {{ background: transparent; border: none; border-radius: 0; }}
#trafficLight:hover {{ background: transparent; }}
"""

# Stands in for the focused conversation's title before one is chosen, so the
# bar's first line always names something rather than opening a hole above the
# folder it belongs to.
_NO_SESSION = "No session selected"


def home_relative(path: str) -> str:
    """`path` with the user's home folder written as `~`. The folder is a
    subtitle here, under the conversation's own name, and the half of an
    absolute path that is the same for every folder is the half worth losing."""
    home = str(Path.home())
    if path == home:
        return "~"
    if path.startswith(home + "/"):
        return "~" + path[len(home):]
    return path


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
    memory_requested = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("titleBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        # Two lines of text, so taller than the one-line bar it replaced.
        self.setFixedHeight(44)
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
        self.branch_button.setIconSize(QSize(14, 14))
        self.branch_button.setPopupMode(
            QToolButton.ToolButtonPopupMode.InstantPopup
        )
        self._branch_menu = QMenu(self.branch_button)
        self._branch_menu.aboutToShow.connect(self._populate_branch_menu)
        self.branch_button.setMenu(self._branch_menu)
        self.conversation_label = QLabel()
        self.conversation_label.setObjectName("conversationLabel")
        # The actions that apply to the conversation named beside it. The menu
        # is filled by whoever knows what is open (MainWindow) as it opens.
        self.session_button = QToolButton()
        self.session_button.setObjectName("sessionButton")
        self.session_button.setToolTip("Session actions")
        self.session_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.session_button.setFixedSize(20, 20)
        self.session_button.setIconSize(QSize(14, 14))
        self.session_button.setPopupMode(
            QToolButton.ToolButtonPopupMode.InstantPopup
        )
        self.session_menu = QMenu(self.session_button)
        self.session_button.setMenu(self.session_menu)
        self.memory_label = QToolButton()
        self.memory_label.setObjectName("memoryLabel")
        self.memory_label.setToolTip("Show Kraken processes and memory")
        self.memory_label.setCursor(Qt.CursorShape.PointingHandCursor)
        self.memory_label.setFocusPolicy(Qt.FocusPolicy.NoFocus)
        self.memory_label.clicked.connect(self.memory_requested.emit)

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

        # What is open, on two lines: its name, then where it lives. The rows
        # are laid out in a column of their own so both start at the same left
        # edge, just past the History toggle.
        title_row = QHBoxLayout()
        title_row.setContentsMargins(0, 0, 0, 0)
        title_row.setSpacing(4)
        title_row.addWidget(self.conversation_label)
        title_row.addWidget(self.session_button)
        title_row.addStretch(1)

        folder_row = QHBoxLayout()
        folder_row.setContentsMargins(0, 0, 0, 0)
        folder_row.setSpacing(4)
        folder_row.addWidget(self.folder_label)
        folder_row.addWidget(self.branch_button)
        folder_row.addStretch(1)

        column = QVBoxLayout()
        column.setContentsMargins(0, 0, 0, 0)
        column.setSpacing(0)
        column.addStretch(1)
        column.addLayout(title_row)
        column.addLayout(folder_row)
        column.addStretch(1)

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
        layout.addSpacing(2)
        layout.addLayout(column, stretch=1)
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
            folder = f"{remote.host.destination}:{remote.path}"
        else:
            folder = home_relative(path) if path else "Kraken"
        fm = self.folder_label.fontMetrics()
        self.folder_label.setText(
            fm.elidedText(folder, Qt.TextElideMode.ElideMiddle, 420)
        )
        self.folder_label.setToolTip(folder)
        self._refresh_branch()

    def set_conversation(self, title: str) -> None:
        fm = self.conversation_label.fontMetrics()
        self.conversation_label.setText(
            fm.elidedText(title or _NO_SESSION, Qt.TextElideMode.ElideRight, 420)
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
            _style(self._theme_name)
            + corner_style(
                "#titleBar", ("top-left", "top-right"), self._corner_radius
            )
        )

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self._apply_style()
        color = _ICON_COLORS[name]
        self.branch_button.setIcon(icon("git-branch", color, 14))
        # Blue on the accent tint once the History panel is showing, to match
        # the strip on the other side of the window.
        self.left_panel_toggle.setIcon(
            toggle_icon("panel-left", color, UI_COLORS[name]["accent_text"], 16)
        )
        self.session_button.setIcon(icon("ellipsis", color, 14))
        # The lights carry no theme colour of their own, but the maximized
        # state still has to be re-applied here.
        self.set_maximized(self._maximized)
