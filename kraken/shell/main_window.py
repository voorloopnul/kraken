from importlib.resources import files
from pathlib import Path

from PySide6.QtCore import QEvent, QSize, Qt
from PySide6.QtGui import QAction, QGuiApplication, QIcon, QPixmap
from PySide6.QtOpenGLWidgets import QOpenGLWidget
from PySide6.QtWidgets import (
    QApplication,
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMessageBox,
    QStackedWidget,
    QToolTip,
    QVBoxLayout,
    QWidget,
)

from kraken import debug
from kraken.agent import remote as remote_mod
from kraken.agent.config import load_state, save_state
from kraken.shell.remote_dialog import RemoteWorkspaceDialog
from kraken.shell.side_bar import SideBar
from kraken.ui.chrome import WINDOW_RADIUS
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS
from kraken.shell.title_bar import TitleBar
from kraken.shell.workspace_bar import WorkspaceBar, abbreviation
from kraken.shell.workspace_view import WorkspaceView

# Grabbing within this many pixels of a window edge starts a resize; the
# custom title bar removes the native frame, and with it native resizing.
_RESIZE_MARGIN = 6
# Within this many pixels of a strip's end the grab is a corner resize.
_CORNER_MARGIN = 14

_EDGE_CURSORS = {
    Qt.Edge.LeftEdge: Qt.CursorShape.SizeHorCursor,
    Qt.Edge.RightEdge: Qt.CursorShape.SizeHorCursor,
    Qt.Edge.TopEdge: Qt.CursorShape.SizeVerCursor,
    Qt.Edge.BottomEdge: Qt.CursorShape.SizeVerCursor,
    Qt.Edge.TopEdge | Qt.Edge.LeftEdge: Qt.CursorShape.SizeFDiagCursor,
    Qt.Edge.BottomEdge | Qt.Edge.RightEdge: Qt.CursorShape.SizeFDiagCursor,
    Qt.Edge.TopEdge | Qt.Edge.RightEdge: Qt.CursorShape.SizeBDiagCursor,
    Qt.Edge.BottomEdge | Qt.Edge.LeftEdge: Qt.CursorShape.SizeBDiagCursor,
}


def _asset_pixmap(name: str) -> QPixmap:
    """Load a bundled theme image as a pixmap.

    Reads the bytes through importlib.resources, which locates them relative to
    the package itself — so a source checkout and the AppImage's bundled tree
    both work without assuming anything about the layout around them. Returns a
    null pixmap if the asset is missing, so callers can fall back gracefully.
    """
    pixmap = QPixmap()
    try:
        data = files("kraken.ui.assets").joinpath(f"{name}.png").read_bytes()
    except (FileNotFoundError, ModuleNotFoundError, OSError):
        return pixmap
    pixmap.loadFromData(data)
    return pixmap


class _HomeScreen(QWidget):
    """The logo shown when no workspace is open. It paints no background of its
    own: it fills the stack, which reaches the window's rounded bottom-right
    corner, and the window frame behind it already carries the window color."""

    def __init__(self):
        super().__init__()
        self._logo = QLabel()
        self._logo.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._logo.setScaledContents(False)
        self._logo.setMinimumSize(1, 1)
        self._pixmap = QPixmap()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.addStretch(1)
        layout.addWidget(self._logo)
        layout.addStretch(1)

    def set_theme(self, name: str) -> None:
        self._pixmap = _asset_pixmap(name)
        self._update_logo()

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._update_logo()

    def _update_logo(self) -> None:
        if self._pixmap.isNull():
            self._logo.setText("Kraken")
            return
        self._logo.setText("")
        target_w = max(240, min(self.width() - 96, 420))
        target_h = max(180, min(self.height() - 96, 420))
        self._logo.setPixmap(
            self._pixmap.scaled(
                target_w,
                target_h,
                Qt.AspectRatioMode.KeepAspectRatio,
                Qt.TransformationMode.SmoothTransformation,
            )
        )


class _EdgeGrip(QWidget):
    """Invisible strip overlaid along one edge of the frameless window.
    Pressing it hands the drag to the window manager as a resize; the strip
    ends double as corner grips by adding the perpendicular edge. A plain
    child widget (not an application event filter) so QtWebEngine's internal
    QObjects never pass through Python during construction — wrapping those
    in an app-wide filter crashes PySide."""

    def __init__(self, edge: Qt.Edge, parent: QWidget):
        super().__init__(parent)
        self._edge = edge
        self._horizontal = edge in (Qt.Edge.TopEdge, Qt.Edge.BottomEdge)
        self.setMouseTracking(True)

    def _edges_at(self, pos) -> Qt.Edge:
        edges = self._edge
        along = pos.x() if self._horizontal else pos.y()
        length = self.width() if self._horizontal else self.height()
        if along <= _CORNER_MARGIN:
            edges |= Qt.Edge.TopEdge if not self._horizontal else Qt.Edge.LeftEdge
        elif along >= length - _CORNER_MARGIN:
            edges |= (
                Qt.Edge.BottomEdge if not self._horizontal else Qt.Edge.RightEdge
            )
        return edges

    def mouseMoveEvent(self, event) -> None:
        self.setCursor(_EDGE_CURSORS[self._edges_at(event.position().toPoint())])

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            self.window().windowHandle().startSystemResize(
                self._edges_at(event.position().toPoint())
            )
            event.accept()


def _theme_icon(name: str) -> QIcon:
    return QIcon(_asset_pixmap(name))


def _looks_blank(pixmap: QPixmap) -> bool:
    """A GPU-composited QWebEngineView grab can come back a valid-but-uniform
    image instead of the rendered page. Sampling a grid across it catches that
    (a real page is never a single flat color) so a blank capture isn't
    silently attached and sent."""
    image = pixmap.toImage()
    if image.isNull():
        return True
    w, h = image.width(), image.height()
    first = image.pixel(0, 0)
    cols, rows = min(32, w), min(32, h)
    for r in range(rows):
        y = r * (h - 1) // max(1, rows - 1)
        for c in range(cols):
            x = c * (w - 1) // max(1, cols - 1)
            if image.pixel(x, y) != first:
                return False
    return True


def _default_size() -> QSize:
    """Preferred opening size, clamped to the screen. The window is frameless,
    so one that opens larger than the display has no native decoration to drag
    it back by — only the edge grips, which would be off-screen too."""
    preferred = QSize(1920, 1080)
    screen = QGuiApplication.primaryScreen()
    if screen is None:
        return preferred
    return preferred.boundedTo(screen.availableGeometry().size())


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Kraken")
        # The TitleBar widget replaces the native decoration.
        self.setWindowFlag(Qt.WindowType.FramelessWindowHint, True)
        # Rounded corners mean the pixels outside the radius belong to nobody, so
        # the window has to be able to leave them empty rather than painting them.
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground, True)
        self.resize(_default_size())
        self.current_workspace: str | None = None
        # Set properly by set_theme below; seeded because a window-state change
        # can arrive while the window is still being built.
        self._theme_name = DEFAULT_THEME
        # The radius the corner widgets were last given, to skip the work when a
        # state change does not actually change the shape.
        self._corners_applied: int | None = None
        # Set while pushing a workspace's stored panel state into the toggle
        # buttons on a switch, so the resulting `toggled` signals only update
        # the button visuals and don't re-apply onto the view (which would
        # clobber the state we're reading).
        self._syncing_panels = False

        # One WorkspaceView (history/conversation/terminal panes) per open
        # workspace, keyed by absolute path; the stack shows the current one.
        self.views: dict[str, WorkspaceView] = {}
        self._view_stack = QStackedWidget()
        # Transparent rather than window-colored, and set once since it carries
        # no color: on the home screen the stack reaches into the window's
        # rounded bottom-right corner (the side bar is hidden there), and an
        # opaque background would square that corner off again. The frame behind
        # it holds the window color for both.
        self._view_stack.setStyleSheet("QStackedWidget { background: transparent; }")
        self._home_screen = _HomeScreen()
        self._view_stack.addWidget(self._home_screen)
        self._view_stack.setCurrentWidget(self._home_screen)

        self.side_bar = SideBar()
        self.workspace_bar = WorkspaceBar()
        self.workspace_bar.add_local_requested.connect(self._add_workspace)
        self.workspace_bar.add_remote_requested.connect(self._add_remote_workspace)
        self.workspace_bar.workspace_selected.connect(self._on_workspace_selected)
        self.workspace_bar.workspace_edit_requested.connect(self._edit_remote_workspace)
        self.workspace_bar.workspace_remove_requested.connect(self._remove_workspace)
        self.title_bar = TitleBar()
        self.title_bar.buttons["Minimize"].clicked.connect(self.showMinimized)
        self.title_bar.buttons["Maximize"].clicked.connect(self._toggle_maximized)
        self.title_bar.buttons["Close"].clicked.connect(self.close)
        self.title_bar.branch_changed.connect(self._on_branch_switched)

        content = QWidget()
        content_layout = QHBoxLayout(content)
        content_layout.setContentsMargins(0, 0, 0, 0)
        content_layout.setSpacing(0)
        content_layout.addWidget(self.workspace_bar)
        content_layout.addWidget(self._view_stack, stretch=1)
        content_layout.addWidget(self.side_bar)

        # The frame behind every child. With a translucent window, anything no
        # child paints would come out transparent — including the corner a
        # rounded child leaves open — so this carries the window color under
        # them all, rounded to the same radius.
        central = QWidget()
        central.setObjectName("windowFrame")
        central.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self._frame = central
        central_layout = QVBoxLayout(central)
        central_layout.setContentsMargins(0, 0, 0, 0)
        central_layout.setSpacing(0)
        central_layout.addWidget(self.title_bar)
        central_layout.addWidget(content, stretch=1)

        # Adding the first render-to-texture widget (the browser panel's
        # QWebEngineView) to a window whose native handle already exists
        # makes Qt destroy and recreate that handle, so the compositor
        # unmaps and remaps the window — a visible flash that can also
        # move it. This 1px GL widget, clipped outside the viewport,
        # makes the window GL-composited from first creation instead.
        self._gl_warmup = QOpenGLWidget(central)
        self._gl_warmup.setFixedSize(1, 1)
        self._gl_warmup.move(-2, -2)

        self.setCentralWidget(central)
        self.set_theme(DEFAULT_THEME)
        self._apply_corners()
        self.title_bar.set_workspace(None)
        self.title_bar.set_conversation("")

        # Frameless windows lose native edge resizing; thin grip strips
        # overlaid on the window borders bring it back (see _EdgeGrip).
        self._grips = [
            _EdgeGrip(edge, self)
            for edge in (
                Qt.Edge.LeftEdge,
                Qt.Edge.RightEdge,
                Qt.Edge.TopEdge,
                Qt.Edge.BottomEdge,
            )
        ]

        # Safety net: some exit paths (app.quit(), the last window closing via
        # the WM) don't deliver closeEvent, so reap the agent processes when the
        # event loop is about to end too. Idempotent with closeEvent.
        app = QApplication.instance()
        if app is not None:
            app.aboutToQuit.connect(self._shutdown_all_views)

        self._create_actions()
        # Reopen the workspaces from the last run (folders that vanished
        # since are dropped); first launch falls back to the launch
        # directory so the window starts with a live set of panes.
        state = load_state()
        workspaces = [p for p in state.get("workspaces", []) if Path(p).is_dir()]
        if not workspaces:
            workspaces = [str(Path.cwd())]
        for path in workspaces:
            target = remote_mod.resolve(path)
            if target is not None:
                self._add_remote_button(path, target, select=False)
            else:
                self.workspace_bar.add_workspace(path, select=False)
        self._sync_chrome()

    @property
    def current_view(self) -> WorkspaceView | None:
        widget = self._view_stack.currentWidget()
        return widget if isinstance(widget, WorkspaceView) else None

    def _sync_chrome(self) -> None:
        """The side bar and the title-bar history toggle both act on a
        workspace's panels, so hide them on the home screen where there's
        nothing to toggle."""
        on_workspace = self.current_view is not None
        self.side_bar.setVisible(on_workspace)
        self.title_bar.left_panel_toggle.setVisible(on_workspace)

    def _corner_radius(self) -> int:
        """The window's corner radius as things stand. A maximized window is
        square: there is nothing beside it to round against, and a gap at the
        screen's own corner reads as a glitch."""
        return 0 if self.isMaximized() else WINDOW_RADIUS

    def _apply_corners(self) -> None:
        """Push the radius into the widgets that own the window's corners — each
        corner is rounded by whatever sits in it, with the frame underneath
        matching so the curves coincide rather than one showing past the other.

        Returns early when the shape has not moved. Every widget here rebuilds
        its style sheet, and one on the frame repolishes the whole tree beneath
        it (transcript, terminal, web view); minimize and restore both arrive
        here without changing the radius at all."""
        radius = self._corner_radius()
        if radius == self._corners_applied:
            return
        self._corners_applied = radius
        self.title_bar.set_corner_radius(radius)
        self.workspace_bar.set_corner_radius(radius)
        self.side_bar.set_corner_radius(radius)
        self._apply_frame()

    def _apply_frame(self) -> None:
        self._frame.setStyleSheet(
            f"#windowFrame {{ background: {UI_COLORS[self._theme_name]['window']};"
            f" border-radius: {self._corner_radius()}px; }}"
        )

    def set_theme(self, name: str) -> None:
        """Apply the selected theme throughout the application."""
        debug.action("theme.set", name=name, views=len(self.views))
        self._theme_name = name
        icon = _theme_icon(name)
        self.setWindowIcon(icon)
        app = QApplication.instance()
        if app is not None:
            app.setWindowIcon(icon)
        self._apply_frame()
        for view in self.views.values():
            view.set_theme(name)
        self._home_screen.set_theme(name)
        self.side_bar.set_theme(name)
        self.workspace_bar.set_theme(name)
        self.title_bar.set_theme(name)

    def _add_workspace(self) -> None:
        path = QFileDialog.getExistingDirectory(self, "Add Workspace")
        debug.action("workspace.add", path=path or "(cancelled)")
        if path:
            self.workspace_bar.add_workspace(path)

    def _add_remote_button(self, anchor: str, target, select: bool) -> None:
        """Add a workspace-bar button for a remote workspace. `anchor` is the
        local key; the visible label/tooltip come from the remote target."""
        self.workspace_bar.add_workspace(
            anchor,
            select=select,
            remote=True,
            tooltip=f"{target.host.destination}:{target.path}",
            abbrev=abbreviation(target.host.host_id),
        )

    def _add_remote_workspace(self) -> None:
        debug.action("workspace.add-remote.open")
        dialog = RemoteWorkspaceDialog(self)
        if dialog.exec() and dialog.anchor:
            debug.action("workspace.add-remote", anchor=dialog.anchor)
            target = remote_mod.resolve(dialog.anchor)
            if target is not None:
                self._add_remote_button(dialog.anchor, target, select=True)

    def _edit_remote_workspace(self, anchor: str) -> None:
        debug.action("workspace.edit-remote", anchor=anchor)
        dialog = RemoteWorkspaceDialog(self, anchor=anchor)
        if not dialog.exec():
            return
        # Connection details changed: drop the open view and rebuild its button
        # so a fresh selection binds the agent/terminal/git to the new target.
        self._discard_view(anchor)
        self.workspace_bar.remove_workspace(anchor)
        target = remote_mod.resolve(anchor)
        if target is not None:
            self._add_remote_button(anchor, target, select=True)

    def _remove_workspace(self, key: str) -> None:
        """Remove a workspace from the strip. Remote workspaces are also dropped
        from persisted state (their local anchor folder is left in place, so a
        re-add reuses any stored sessions)."""
        debug.action("workspace.remove", key=key)
        self._discard_view(key)
        self.workspace_bar.remove_workspace(key)
        if remote_mod.resolve(key) is not None:
            remote_mod.remove_remote_workspace(key)
        remaining = self.workspace_bar.workspaces
        save_state(
            workspaces=remaining,
            current_workspace=remaining[0] if remaining else None,
        )
        if remaining:
            self.workspace_bar.select_workspace(remaining[0])
        else:
            self._view_stack.setCurrentWidget(self._home_screen)
            self.current_workspace = None
            self.setWindowTitle("Kraken")
            self.title_bar.set_workspace(None)
            self.title_bar.set_conversation("")
            self._sync_chrome()

    def _discard_view(self, key: str) -> None:
        """Shut down and drop a cached workspace view, if one exists."""
        view = self.views.pop(key, None)
        if view is None:
            return
        debug.log("view.discard", key=key, remaining=len(self.views))
        view.shutdown()
        self._view_stack.removeWidget(view)
        view.deleteLater()
        if self.current_workspace == key:
            self.current_workspace = None

    def _on_workspace_selected(self, path: str) -> None:
        """Show the workspace's panes, creating them on first selection."""
        target = remote_mod.resolve(path)
        view = self.views.get(path)
        debug.action(
            "workspace.select",
            path=path,
            remote=target is not None,
            new=view is None,
        )
        if view is None:
            view = WorkspaceView(path, remote=target)
            view.set_theme(self._theme_name)
            # A clicked transcript link opens the browser panel through the
            # same action used by its side-bar toggle.
            view.browser_requested.connect(
                lambda: self._panel_actions["browser"].setChecked(True)
            )
            # Closing the last tab closes the panel, through the same action,
            # so the toggle and the panel.toggle record stay in step.
            view.browser_closed.connect(
                lambda: self._panel_actions["browser"].setChecked(False)
            )
            # The title bar shows the focused conversation's title; only the
            # visible workspace's view gets to set it.
            view.title_changed.connect(
                lambda title, v=view: (
                    self.title_bar.set_conversation(title)
                    if v is self.current_view
                    else None
                )
            )
            # Blink the workspace's bar button while any of its sessions is
            # streaming — visible even when another workspace is on screen.
            view.activity_changed.connect(
                lambda active, p=path: self.workspace_bar.set_workspace_active(
                    p, active
                )
            )
            # A new workspace keeps WorkspaceView's own defaults (only History
            # open); the toggle buttons are synced to it just below.
            self.views[path] = view
            self._view_stack.addWidget(view)
        else:
            view.left_panel.refresh()
        self._view_stack.setCurrentWidget(view)
        # Reflect this workspace's own open panes in the toggle buttons.
        self._sync_panel_toggles(view)
        self._sync_chrome()
        self.current_workspace = path
        if target is not None:
            name = Path(target.path).name or target.host.host_id
            self.setWindowTitle(f"Kraken — {name} ({target.host.destination})")
        else:
            self.setWindowTitle(f"Kraken — {Path(path).name}")
        self.title_bar.set_workspace(path, remote=target)
        self.title_bar.set_conversation(
            view.focused.title if view.focused is not None else ""
        )
        save_state(
            workspaces=self.workspace_bar.workspaces, current_workspace=path
        )

    def _shutdown_all_views(self) -> None:
        """Stop every workspace's agent processes. Guarded per view so one
        workspace's failing teardown can't leave another's `pi` running, and
        idempotent (a second pass finds already-stopped agents) so wiring it to
        both closeEvent and aboutToQuit is safe."""
        if self.views:
            debug.action("app.shutdown-views", views=len(self.views))
        for view in list(self.views.values()):
            try:
                view.shutdown()
            except Exception as exc:
                # Swallowed so one workspace can't block the others, but a
                # failure here is exactly what leaves a `pi` or a shell behind.
                debug.exception("view.shutdown failed", exc)

    def closeEvent(self, event) -> None:
        debug.action("window.close")
        self._shutdown_all_views()
        super().closeEvent(event)

    def _screenshot_browser(self) -> None:
        """Capture the current browser tab's rendered page and attach it to
        the chat input, ready to send with the next prompt."""
        button = self.side_bar.buttons["Screenshot"]
        pos = button.mapToGlobal(button.rect().center())
        view = self.current_view
        browser = None
        if view is not None and view.browser_panel.browsers is not None:
            browser = view.browser_panel.browsers.current_browser
        if browser is None or not view.browser_panel.isVisible():
            QToolTip.showText(pos, "Open the browser panel first", button)
            return
        # Both of these would grab a blank image and land on the "still
        # rendering" advice below, which is wrong for either: a crashed page
        # never finishes rendering, and an empty tab has nothing to render.
        if browser.crashed:
            QMessageBox.warning(
                self,
                "Screenshot failed",
                "This page crashed and was closed. Reload it in the browser "
                "panel, then try again.",
            )
            return
        if browser.is_blank:
            QToolTip.showText(pos, "This tab has no page loaded", button)
            return
        # Grabbing a GPU-composited web view reaches into Chromium; log around
        # it so a crash here is distinguishable from one in the chat input.
        debug.action("browser.screenshot")
        pixmap = browser.web.grab()
        if pixmap.isNull() or _looks_blank(pixmap):
            QMessageBox.warning(
                self,
                "Screenshot failed",
                "The browser produced a blank capture. Give the page a moment "
                "to finish rendering, then try again.",
            )
            return
        view.center_panel.chat.attach_image(pixmap)
        QToolTip.showText(pos, "Screenshot attached to the message", button)

    def _on_branch_switched(self) -> None:
        """A title-bar branch switch moved HEAD; redraw the visible git and diff
        panels, whose contents are both relative to it."""
        view = self.current_view
        if view is None:
            return
        if view.git_panel.isVisible():
            view.git_panel.refresh()
        if view.diff_panel.isVisible():
            view.diff_panel.refresh()

    def _toggle_maximized(self) -> None:
        if self.isMaximized():
            self.showNormal()
        else:
            self.showMaximized()

    def changeEvent(self, event) -> None:
        # Keep the title bar's maximize/restore glyph in sync however the
        # state changes (button, double-click, or the window manager), round or
        # square the corners to match, and drop the resize grips while
        # maximized. State changes do arrive mid-__init__, so the guard names the
        # last of the three to be built rather than the first.
        if event.type() == QEvent.Type.WindowStateChange and hasattr(self, "_grips"):
            debug.action("window.state", maximized=self.isMaximized())
            self.title_bar.set_maximized(self.isMaximized())
            self._apply_corners()
            for grip in self._grips:
                grip.setVisible(not self.isMaximized())
        super().changeEvent(event)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        if not hasattr(self, "_grips"):
            return
        m, w, h = _RESIZE_MARGIN, self.width(), self.height()
        rects = {
            Qt.Edge.LeftEdge: (0, 0, m, h),
            Qt.Edge.RightEdge: (w - m, 0, m, h),
            Qt.Edge.TopEdge: (0, 0, w, m),
            Qt.Edge.BottomEdge: (0, h - m, w, m),
        }
        for grip in self._grips:
            grip.setGeometry(*rects[grip._edge])
            grip.raise_()

    def _set_panel_visible(self, side: str, visible: bool) -> None:
        """A pane toggle applies only to the current workspace; each workspace
        keeps its own set of open panes."""
        if self._syncing_panels:
            return
        view = self.current_view
        if view is not None:
            # Showing a panel is what spins up its native machinery — Chromium
            # for the browser, a shell and a libghostty terminal for the right
            # panel — so these lines bracket the app's riskiest transitions.
            debug.action("panel.toggle", side=side, visible=visible)
            view.set_panel_visible(side, visible)

    def _sync_panel_toggles(self, view: WorkspaceView) -> None:
        """Point the toggle buttons at `view`'s stored panel state. Guarded so
        the `toggled` signals refresh the button visuals without looping back
        through _set_panel_visible onto the view."""
        self._syncing_panels = True
        try:
            for side, action in self._panel_actions.items():
                action.setChecked(view.is_panel_visible(side))
        finally:
            self._syncing_panels = False

    def _create_actions(self) -> None:
        # Keep keyboard shortcuts available without exposing a menu bar.
        add_workspace_action = QAction("Add Workspace", self)
        add_workspace_action.setShortcut("Ctrl+O")
        add_workspace_action.triggered.connect(self._add_workspace)
        self.addAction(add_workspace_action)
        quit_action = QAction("Quit", self)
        quit_action.setShortcut("Ctrl+Q")
        quit_action.triggered.connect(self.close)
        self.addAction(quit_action)

        self._panel_actions: dict[str, QAction] = {}
        for label, side in (
            ("Left Panel", "left"),
            ("Browser Panel", "browser"),
            ("Diff Panel", "diff"),
            ("Git Panel", "git"),
            ("Right Panel", "right"),
        ):
            action = QAction(label, self, checkable=True, checked=True)
            action.toggled.connect(
                lambda checked, s=side: self._set_panel_visible(s, checked)
            )
            self._panel_actions[side] = action

        # Side-bar icons toggle the terminal (right), browser, diff, and git
        # panels. They are hidden by default.
        for button_name, side in (
            ("Terminal Panel", "right"),
            ("Browser Panel", "browser"),
            ("Diff Panel", "diff"),
            ("Git Panel", "git"),
        ):
            toggle = self.side_bar.buttons[button_name]
            toggle.setCheckable(True)
            toggle.toggled.connect(self._panel_actions[side].setChecked)
            self._panel_actions[side].toggled.connect(toggle.setChecked)
            self._panel_actions[side].setChecked(False)

        # The title-bar button toggles the History (left) panel, which is
        # visible by default.
        history_toggle = self.title_bar.left_panel_toggle
        history_toggle.setChecked(self._panel_actions["left"].isChecked())
        history_toggle.toggled.connect(self._panel_actions["left"].setChecked)
        self._panel_actions["left"].toggled.connect(history_toggle.setChecked)

        self.side_bar.buttons["Screenshot"].clicked.connect(self._screenshot_browser)
        self.workspace_bar.buttons["Quit"].clicked.connect(self.close)
        self.workspace_bar.buttons["Toggle Theme"].clicked.connect(
            lambda: self.set_theme("light" if self._theme_name == "dark" else "dark")
        )
