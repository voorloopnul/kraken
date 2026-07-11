from pathlib import Path

from PySide6.QtCore import QEvent, Qt
from PySide6.QtGui import QAction, QActionGroup
from PySide6.QtOpenGLWidgets import QOpenGLWidget
from PySide6.QtWidgets import (
    QFileDialog,
    QHBoxLayout,
    QMainWindow,
    QMenuBar,
    QStackedWidget,
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


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Alpine")
        # The TitleBar widget replaces the native decoration.
        self.setWindowFlag(Qt.WindowType.FramelessWindowHint, True)
        self.resize(1200, 720)
        self.current_workspace: str | None = None

        # One WorkspaceView (history/conversation/terminal panes) per open
        # workspace, keyed by absolute path; the stack shows the current one.
        self.views: dict[str, WorkspaceView] = {}
        self._view_stack = QStackedWidget()

        self.side_bar = SideBar()
        self.workspace_bar = WorkspaceBar()
        self.workspace_bar.add_button.clicked.connect(self._add_workspace)
        self.workspace_bar.workspace_selected.connect(self._on_workspace_selected)
        self.title_bar = TitleBar()
        self.title_bar.buttons["Minimize"].clicked.connect(self.showMinimized)
        self.title_bar.buttons["Maximize"].clicked.connect(self._toggle_maximized)
        self.title_bar.buttons["Close"].clicked.connect(self.close)

        # The frameless window hosts its own menu bar in the layout (below
        # the title bar) instead of QMainWindow's built-in slot, which would
        # sit above it.
        self._menu_bar = QMenuBar()
        self._menu_bar.setNativeMenuBar(False)

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
        central_layout.addWidget(self._menu_bar)
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

        self._create_menus()
        # Reopen the workspaces from the last run (folders that vanished
        # since are dropped); first launch falls back to the launch
        # directory so the window starts with a live set of panes.
        state = load_state()
        workspaces = [p for p in state.get("workspaces", []) if Path(p).is_dir()]
        if not workspaces:
            workspaces = [str(Path.cwd())]
        for path in workspaces:
            self.workspace_bar.add_workspace(path, select=False)
        current = state.get("current_workspace")
        self.workspace_bar.add_workspace(
            current if current in workspaces else workspaces[0]
        )

    @property
    def current_view(self) -> WorkspaceView | None:
        widget = self._view_stack.currentWidget()
        return widget if isinstance(widget, WorkspaceView) else None

    def set_theme(self, name: str) -> None:
        """Theme the whole application; the menu bar keeps its native look."""
        self._theme_name = name
        ui = UI_COLORS[name]
        self._view_stack.setStyleSheet(
            f"QStackedWidget {{ background: {ui['window']}; }}"
        )
        for view in self.views.values():
            view.set_theme(name)
        self.side_bar.set_theme(name)
        self.workspace_bar.set_theme(name)
        self.title_bar.set_theme(name)
        # Keep the View > Theme radio items in sync (empty until menus exist).
        for action_name, action in getattr(self, "_theme_actions", {}).items():
            action.setChecked(action_name == name)

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
            # A clicked transcript link opens the browser panel; checking the
            # menu action shows the panel and keeps the toggles in sync.
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
            view.left_panel.setVisible(self._panel_actions["left"].isChecked())
            view.browser_panel.setVisible(self._panel_actions["browser"].isChecked())
            view.right_panel.setVisible(self._panel_actions["right"].isChecked())
            self.views[path] = view
            self._view_stack.addWidget(view)
        else:
            view.left_panel.refresh()
        self._view_stack.setCurrentWidget(view)
        self.current_workspace = path
        self.setWindowTitle(f"Alpine — {Path(path).name}")
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
            panel = {
                "left": view.left_panel,
                "browser": view.browser_panel,
                "right": view.right_panel,
            }[side]
            panel.setVisible(visible)

    def _create_menus(self) -> None:
        file_menu = self._menu_bar.addMenu("&File")
        add_workspace_action = QAction("&Add Workspace…", self)
        add_workspace_action.setShortcut("Ctrl+O")
        add_workspace_action.triggered.connect(self._add_workspace)
        file_menu.addAction(add_workspace_action)
        quit_action = QAction("&Quit", self)
        quit_action.setShortcut("Ctrl+Q")
        quit_action.triggered.connect(self.close)
        file_menu.addAction(quit_action)

        view_menu = self._menu_bar.addMenu("&View")
        self._panel_actions: dict[str, QAction] = {}
        for label, side in (
            ("Left Panel", "left"),
            ("Browser Panel", "browser"),
            ("Right Panel", "right"),
        ):
            action = QAction(label, self, checkable=True, checked=True)
            action.toggled.connect(
                lambda checked, s=side: self._set_panel_visible(s, checked)
            )
            view_menu.addAction(action)
            self._panel_actions[side] = action

        # First side-bar icon toggles the terminal (right) panel, staying in
        # sync with the View menu checkbox. Hidden by default.
        toggle = self.side_bar.buttons["Terminal Panel"]
        toggle.setCheckable(True)
        toggle.toggled.connect(self._panel_actions["right"].setChecked)
        self._panel_actions["right"].toggled.connect(toggle.setChecked)
        self._panel_actions["right"].setChecked(False)

        # Same deal for the browser panel and its globe icon.
        browser_toggle = self.side_bar.buttons["Browser Panel"]
        browser_toggle.setCheckable(True)
        browser_toggle.toggled.connect(self._panel_actions["browser"].setChecked)
        self._panel_actions["browser"].toggled.connect(browser_toggle.setChecked)
        self._panel_actions["browser"].setChecked(False)

        self.side_bar.buttons["Quit"].clicked.connect(self.close)
        # Menu bar starts hidden; setVisible(False) is explicit because
        # setChecked(False) on a fresh action doesn't emit toggled.
        menu_toggle = self.side_bar.buttons["Menu Bar"]
        menu_toggle.setCheckable(True)
        menu_toggle.toggled.connect(self._menu_bar.setVisible)
        self._menu_bar.setVisible(False)
        self.side_bar.buttons["Toggle Theme"].clicked.connect(
            lambda: self.set_theme("light" if self._theme_name == "dark" else "dark")
        )

        theme_menu = view_menu.addMenu("Theme")
        theme_group = QActionGroup(self)
        self._theme_actions = {}
        for label, name in (("Dark", "dark"), ("Light", "light")):
            action = QAction(label, self, checkable=True)
            action.setActionGroup(theme_group)
            action.setChecked(name == self._theme_name)
            action.triggered.connect(lambda _=False, n=name: self.set_theme(n))
            theme_menu.addAction(action)
            self._theme_actions[name] = action
