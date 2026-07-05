from pathlib import Path

from PySide6.QtGui import QAction, QActionGroup
from PySide6.QtWidgets import (
    QFileDialog,
    QHBoxLayout,
    QMainWindow,
    QStackedWidget,
    QWidget,
)

from app.side_bar import SideBar
from app.themes import DEFAULT_THEME, UI_COLORS
from app.workspace_bar import WorkspaceBar
from app.workspace_view import WorkspaceView


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Alpine")
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
        central = QWidget()
        central_layout = QHBoxLayout(central)
        central_layout.setContentsMargins(0, 0, 0, 0)
        central_layout.setSpacing(0)
        central_layout.addWidget(self.workspace_bar)
        central_layout.addWidget(self._view_stack, stretch=1)
        central_layout.addWidget(self.side_bar)

        self.setCentralWidget(central)
        self.set_theme(DEFAULT_THEME)

        self._create_menus()
        # Open the launch directory as the first workspace so the window
        # starts with a live set of panes.
        self.workspace_bar.add_workspace(str(Path.cwd()))

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
            # New views follow the current pane-visibility toggles.
            view.left_panel.setVisible(self._panel_actions["left"].isChecked())
            view.right_panel.setVisible(self._panel_actions["right"].isChecked())
            self.views[path] = view
            self._view_stack.addWidget(view)
        else:
            view.left_panel.refresh()
        self._view_stack.setCurrentWidget(view)
        self.current_workspace = path
        self.setWindowTitle(f"Alpine — {Path(path).name}")

    def closeEvent(self, event) -> None:
        for view in self.views.values():
            view.shutdown()
        super().closeEvent(event)

    def _set_panel_visible(self, side: str, visible: bool) -> None:
        """Pane visibility toggles are global: they apply to every workspace."""
        for view in self.views.values():
            panel = view.left_panel if side == "left" else view.right_panel
            panel.setVisible(visible)

    def _create_menus(self) -> None:
        file_menu = self.menuBar().addMenu("&File")
        add_workspace_action = QAction("&Add Workspace…", self)
        add_workspace_action.setShortcut("Ctrl+O")
        add_workspace_action.triggered.connect(self._add_workspace)
        file_menu.addAction(add_workspace_action)
        quit_action = QAction("&Quit", self)
        quit_action.setShortcut("Ctrl+Q")
        quit_action.triggered.connect(self.close)
        file_menu.addAction(quit_action)

        view_menu = self.menuBar().addMenu("&View")
        self._panel_actions: dict[str, QAction] = {}
        for label, side in (("Left Panel", "left"), ("Right Panel", "right")):
            action = QAction(label, self, checkable=True, checked=True)
            action.toggled.connect(
                lambda checked, s=side: self._set_panel_visible(s, checked)
            )
            view_menu.addAction(action)
            self._panel_actions[side] = action

        # First side-bar icon toggles the terminal (right) panel, staying in
        # sync with the View menu checkbox.
        toggle = self.side_bar.buttons["Terminal Panel"]
        toggle.setCheckable(True)
        toggle.setChecked(True)
        toggle.toggled.connect(self._panel_actions["right"].setChecked)
        self._panel_actions["right"].toggled.connect(toggle.setChecked)

        self.side_bar.buttons["Quit"].clicked.connect(self.close)
        menu_toggle = self.side_bar.buttons["Menu Bar"]
        menu_toggle.setCheckable(True)
        menu_toggle.setChecked(True)
        menu_toggle.toggled.connect(self.menuBar().setVisible)
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
