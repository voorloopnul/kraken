from pathlib import Path

from PySide6.QtCore import QEvent, Qt
from PySide6.QtGui import QAction, QIcon, QPixmap
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

from app.config import load_state, save_state
from app.side_bar import SideBar
from app.themes import DEFAULT_THEME, UI_COLORS
from app.title_bar import TitleBar
from app.workspace_bar import WorkspaceBar
from app.workspace_view import WorkspaceView

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


class _HomeScreen(QWidget):
    def __init__(self):
        super().__init__()
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self._logo = QLabel()
        self._logo.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._logo.setScaledContents(False)
        self._logo.setMinimumSize(1, 1)
        self._logo_dir = Path(__file__).resolve().parents[1]
        self._pixmap = QPixmap()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.addStretch(1)
        layout.addWidget(self._logo)
        layout.addStretch(1)

    def set_theme(self, name: str) -> None:
        ui = UI_COLORS[name]
        self.setStyleSheet(f"background: {ui['window']};")
        self._pixmap = QPixmap(str(self._logo_dir / f"{name}.png"))
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
    return QIcon(str(Path(__file__).resolve().parents[1] / f"{name}.png"))


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


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Kraken")
        # The TitleBar widget replaces the native decoration.
        self.setWindowFlag(Qt.WindowType.FramelessWindowHint, True)
        self.resize(1200, 720)
        self.current_workspace: str | None = None

        # One WorkspaceView (history/conversation/terminal panes) per open
        # workspace, keyed by absolute path; the stack shows the current one.
        self.views: dict[str, WorkspaceView] = {}
        self._view_stack = QStackedWidget()
        self._home_screen = _HomeScreen()
        self._view_stack.addWidget(self._home_screen)
        self._view_stack.setCurrentWidget(self._home_screen)

        self.side_bar = SideBar()
        self.workspace_bar = WorkspaceBar()
        self.workspace_bar.add_button.clicked.connect(self._add_workspace)
        self.workspace_bar.workspace_selected.connect(self._on_workspace_selected)
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

        central = QWidget()
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

        self._create_actions()
        # Reopen the workspaces from the last run (folders that vanished
        # since are dropped); first launch falls back to the launch
        # directory so the window starts with a live set of panes.
        state = load_state()
        workspaces = [p for p in state.get("workspaces", []) if Path(p).is_dir()]
        if not workspaces:
            workspaces = [str(Path.cwd())]
        for path in workspaces:
            self.workspace_bar.add_workspace(path, select=False)

    @property
    def current_view(self) -> WorkspaceView | None:
        widget = self._view_stack.currentWidget()
        return widget if isinstance(widget, WorkspaceView) else None

    def set_theme(self, name: str) -> None:
        """Apply the selected theme throughout the application."""
        self._theme_name = name
        ui = UI_COLORS[name]
        icon = _theme_icon(name)
        self.setWindowIcon(icon)
        app = QApplication.instance()
        if app is not None:
            app.setWindowIcon(icon)
        self._view_stack.setStyleSheet(
            f"QStackedWidget {{ background: {ui['window']}; }}"
        )
        for view in self.views.values():
            view.set_theme(name)
        self._home_screen.set_theme(name)
        self.side_bar.set_theme(name)
        self.workspace_bar.set_theme(name)
        self.title_bar.set_theme(name)

    def _add_workspace(self) -> None:
        path = QFileDialog.getExistingDirectory(self, "Add Workspace")
        if path:
            self.workspace_bar.add_workspace(path)

    def _on_workspace_selected(self, path: str) -> None:
        """Show the workspace's panes, creating them on first selection."""
        view = self.views.get(path)
        if view is None:
            view = WorkspaceView(path)
            view.set_theme(self._theme_name)
            # A clicked transcript link opens the browser panel through the
            # same action used by its side-bar toggle.
            view.browser_requested.connect(
                lambda: self._panel_actions["browser"].setChecked(True)
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
            # New views follow the current pane-visibility toggles.
            for side, action in self._panel_actions.items():
                view.set_panel_visible(side, action.isChecked())
            self.views[path] = view
            self._view_stack.addWidget(view)
        else:
            view.left_panel.refresh()
        self._view_stack.setCurrentWidget(view)
        self.current_workspace = path
        self.setWindowTitle(f"Kraken — {Path(path).name}")
        self.title_bar.set_workspace(path)
        self.title_bar.set_conversation(
            view.focused.title if view.focused is not None else ""
        )
        save_state(
            workspaces=self.workspace_bar.workspaces, current_workspace=path
        )

    def closeEvent(self, event) -> None:
        for view in self.views.values():
            view.shutdown()
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
        """A title-bar branch switch moved HEAD; redraw the visible git panel."""
        view = self.current_view
        if view is not None and view.git_panel.isVisible():
            view.git_panel.refresh()

    def _toggle_maximized(self) -> None:
        if self.isMaximized():
            self.showNormal()
        else:
            self.showMaximized()

    def changeEvent(self, event) -> None:
        # Keep the title bar's maximize/restore glyph in sync however the
        # state changes (button, double-click, or the window manager), and
        # drop the resize grips while maximized. The hasattr guards state
        # changes delivered mid-__init__.
        if event.type() == QEvent.Type.WindowStateChange and hasattr(self, "title_bar"):
            self.title_bar.set_maximized(self.isMaximized())
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
        """Pane visibility toggles are global: they apply to every workspace."""
        for view in self.views.values():
            view.set_panel_visible(side, visible)

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
            ("Git Panel", "git"),
            ("Right Panel", "right"),
        ):
            action = QAction(label, self, checkable=True, checked=True)
            action.toggled.connect(
                lambda checked, s=side: self._set_panel_visible(s, checked)
            )
            self._panel_actions[side] = action

        # Side-bar icons toggle the terminal (right), browser, and git panels.
        # They are hidden by default.
        for button_name, side in (
            ("Terminal Panel", "right"),
            ("Browser Panel", "browser"),
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
        self.side_bar.buttons["Quit"].clicked.connect(self.close)
        self.side_bar.buttons["Toggle Theme"].clicked.connect(
            lambda: self.set_theme("light" if self._theme_name == "dark" else "dark")
        )
