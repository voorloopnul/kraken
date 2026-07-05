from PySide6.QtCore import Qt
from PySide6.QtGui import QAction, QActionGroup
from PySide6.QtWidgets import QHBoxLayout, QMainWindow, QSplitter, QWidget

from app.panels import CenterPanel, LeftPanel, RightPanel
from app.side_bar import SideBar
from app.themes import DEFAULT_THEME, UI_COLORS


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Alpine")
        self.resize(1200, 720)

        self.left_panel = LeftPanel()
        self.center_panel = CenterPanel()
        self.right_panel = RightPanel()

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

        self.side_bar = SideBar()
        central = QWidget()
        central_layout = QHBoxLayout(central)
        central_layout.setContentsMargins(0, 0, 0, 0)
        central_layout.setSpacing(0)
        central_layout.addWidget(splitter, stretch=1)
        central_layout.addWidget(self.side_bar)

        self.setCentralWidget(central)
        self._splitter = splitter
        self.set_theme(DEFAULT_THEME)

        self._create_menus()

    def set_theme(self, name: str) -> None:
        """Theme the whole application; the menu bar keeps its native look."""
        self._theme_name = name
        ui = UI_COLORS[name]
        # Styling the splitter (the central widget) covers the window
        # background and cascades text color to every panel label, while
        # leaving the menu and status bars alone.
        self._splitter.setStyleSheet(
            f"QSplitter {{ background: {ui['window']}; }}"
            f" QLabel {{ color: {ui['text']}; }}"
        )
        self.left_panel.set_theme(name)
        self.center_panel.set_theme(name)
        self.right_panel.set_theme(name)
        self.side_bar.set_theme(name)
        # Keep the View > Theme radio items in sync (empty until menus exist).
        for action_name, action in getattr(self, "_theme_actions", {}).items():
            action.setChecked(action_name == name)

    def closeEvent(self, event) -> None:
        self.right_panel.terminals.shutdown_all()
        super().closeEvent(event)

    def _create_menus(self) -> None:
        file_menu = self.menuBar().addMenu("&File")
        quit_action = QAction("&Quit", self)
        quit_action.setShortcut("Ctrl+Q")
        quit_action.triggered.connect(self.close)
        file_menu.addAction(quit_action)

        view_menu = self.menuBar().addMenu("&View")
        for label, panel in (
            ("Left Panel", self.left_panel),
            ("Right Panel", self.right_panel),
        ):
            action = QAction(label, self, checkable=True, checked=True)
            action.toggled.connect(panel.setVisible)
            view_menu.addAction(action)
            if label == "Right Panel":
                right_panel_action = action

        # First side-bar icon toggles the terminal (right) panel, staying in
        # sync with the View menu checkbox.
        toggle = self.side_bar.buttons["Terminal Panel"]
        toggle.setCheckable(True)
        toggle.setChecked(True)
        toggle.toggled.connect(right_panel_action.setChecked)
        right_panel_action.toggled.connect(toggle.setChecked)

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
