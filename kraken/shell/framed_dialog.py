"""A dialog wearing the app's own decoration instead of the desktop's.

The app is frameless — the main window paints its own title bar — and a dialog
that asked the window manager for a frame would arrive in the desktop's
colours, sitting a system-grey bar over a themed window: the one piece of the
app a theme could not reach. So the dialogs are frameless too, with the title
bar, the rounded shape and the edge grips gathered here rather than written out
once per dialog.

The styles come in two halves because that is how they are reused. `CHROME` is
the window itself — frame, title bar, close button — and is the same in every
dialog. `FORM` is the controls, which a dialog is free to override for a widget
of its own (the settings window's search field is a QLineEdit that is not a
setting's control). Both are appended to whatever the dialog adds; a subclass
calls `apply_theme` and then its own style sheet, or passes its extra rules in.
"""

from __future__ import annotations

from PySide6.QtCore import QSize, Qt
from PySide6.QtWidgets import (
    QDialog,
    QHBoxLayout,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from kraken.shell.settings_page import label
from kraken.shell.title_bar import window_icon
from kraken.ui.chrome import WINDOW_RADIUS, corner_style, edge_grips, place_edge_grips

# Height of our title bar, matching the main window's so every window of the
# app reads as the same piece of chrome.
TITLE_HEIGHT = 36

# The grey the dialogs paint their line-art icons in — the close glyph here,
# and whatever a subclass draws for itself, which has to be the same grey.
ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}

# The window's shape, which no theme changes: the frame carries the rounding,
# and the title bar, which reaches the window's top edge, rounds the two
# corners it owns. A dialog whose content reaches the bottom edge with a
# background of its own rounds those two itself. The dialog is transparent so
# the pixels outside the radius belong to nobody, as in the main window.
SHAPE = (
    f"QWidget#dialogFrame {{ border-radius: {WINDOW_RADIUS}px; }}"
    + corner_style("QWidget#dialogTitleBar", ("top-left", "top-right"), WINDOW_RADIUS)
)

CHROME = {
    "dark": """
QDialog { background: transparent; }
QWidget#dialogFrame { background: #1f2127; }
QWidget#dialogTitleBar { background: #1b1c21; border-bottom: 1px solid #33353c; }
QLabel { color: #c8cad0; }
QLabel[role="windowTitle"] { color: #c8cad0; font-size: 12px; }
QToolButton#dialogClose { background: #2c2e35; border: none; border-radius: 11px; }
QToolButton#dialogClose:hover { background: #3a3d45; }
""",
    "light": """
QDialog { background: transparent; }
QWidget#dialogFrame { background: #fafafa; }
QWidget#dialogTitleBar { background: #fafafa; border-bottom: 1px solid #e0e0e0; }
QLabel { color: #383a42; }
QLabel[role="windowTitle"] { color: #383a42; font-size: 12px; }
QToolButton#dialogClose { background: #ebebee; border: none; border-radius: 11px; }
QToolButton#dialogClose:hover { background: #dcdce1; }
""",
}

# Text fields carry role="control" rather than being styled as plain QLineEdits:
# a QSpinBox holds a QLineEdit of its own, and styling every one of them would
# draw a second border inside the box. settings_page tags a row's field as it
# places it; a dialog laying out its own form tags them itself.
FORM = {
    "dark": """
QComboBox, QSpinBox, QLineEdit[role="control"] {
    background: #26282e; border: 1px solid #3a3d45; border-radius: 6px;
    padding: 5px 8px; color: #c8cad0; font-size: 12px;
}
QComboBox:hover, QSpinBox:hover { border-color: #4a4e58; }
QComboBox:focus, QSpinBox:focus,
QLineEdit[role="control"]:focus { border-color: #4f83e0; }
QComboBox::drop-down { border: none; width: 18px; }
QSpinBox::up-button, QSpinBox::down-button {
    background: transparent; border: none; width: 14px;
}
QComboBox QAbstractItemView {
    background: #26282e; border: 1px solid #3a3d45;
    color: #c8cad0; selection-background-color: #2c2e35;
}
QPushButton {
    background: #2c2e35; border: 1px solid #3a3d45; border-radius: 6px;
    padding: 5px 10px; color: #e6e8ec; font-size: 12px;
}
QPushButton:hover { background: #363943; }
QPushButton:pressed { background: #26282e; }
QPushButton:default { border-color: #4f83e0; }
""",
    "light": """
QComboBox, QSpinBox, QLineEdit[role="control"] {
    background: #ffffff; border: 1px solid #d8d8dd; border-radius: 6px;
    padding: 5px 8px; color: #383a42; font-size: 12px;
}
QComboBox:hover, QSpinBox:hover { border-color: #c2c2c8; }
QComboBox:focus, QSpinBox:focus,
QLineEdit[role="control"]:focus { border-color: #4f83e0; }
QComboBox::drop-down { border: none; width: 18px; }
QSpinBox::up-button, QSpinBox::down-button {
    background: transparent; border: none; width: 14px;
}
QComboBox QAbstractItemView {
    background: #ffffff; border: 1px solid #d8d8dd;
    color: #383a42; selection-background-color: #e0e0e5;
}
QPushButton {
    background: #ffffff; border: 1px solid #d8d8dd; border-radius: 6px;
    padding: 5px 10px; color: #383a42; font-size: 12px;
}
QPushButton:hover { background: #ececef; }
QPushButton:pressed { background: #e0e0e5; }
QPushButton:default { border-color: #4f83e0; }
""",
}


class _TitleBar(QWidget):
    """A dialog's decoration: its name centered, a close button on the right,
    and a drag anywhere else that moves the window. The left spacer is the
    close button's width, so the title sits centered on the window rather than
    on the space the button leaves.

    Only close: a dialog has no state to minimize to and nothing to gain from
    filling the screen, so the two buttons that would say otherwise are left
    off rather than drawn dead."""

    def __init__(self, title: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("dialogTitleBar")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedHeight(TITLE_HEIGHT)

        self.close_button = QToolButton()
        self.close_button.setObjectName("dialogClose")
        self.close_button.setToolTip("Close")
        self.close_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.close_button.setFixedSize(22, 22)
        self.close_button.setIconSize(QSize(12, 12))

        layout = QHBoxLayout(self)
        layout.setContentsMargins(8, 0, 8, 0)
        layout.setSpacing(6)
        layout.addSpacing(self.close_button.width())
        layout.addStretch(1)
        layout.addWidget(label(title, "windowTitle"))
        layout.addStretch(1)
        layout.addWidget(self.close_button)

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            self.window().windowHandle().startSystemMove()
            event.accept()


class FramedDialog(QDialog):
    """Base for the app's dialogs: a frameless window holding our title bar
    over whatever `set_body` is given.

    Subclasses build their content, hand it over, and call `apply_theme` with
    their own style sheet; everything above — the shape, the bar, the grips
    that restore the edge resizing a frameless window loses — is settled here.
    """

    def __init__(self, title: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.setWindowTitle(title)
        # Our own title bar replaces the native decoration, and the corners
        # outside the frame's radius have to be able to stay empty rather than
        # being painted by the window.
        self.setWindowFlag(Qt.WindowType.FramelessWindowHint, True)
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground, True)

        self.title_bar = _TitleBar(title)
        self.title_bar.close_button.clicked.connect(self.reject)

        # The frame behind every child: with a translucent window, the corner a
        # rounded child leaves open would come out transparent, so this carries
        # the window colour under them all at the same radius.
        self._frame = QWidget()
        self._frame.setObjectName("dialogFrame")
        self._frame.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self._frame_layout = QVBoxLayout(self._frame)
        self._frame_layout.setContentsMargins(0, 0, 0, 0)
        self._frame_layout.setSpacing(0)
        self._frame_layout.addWidget(self.title_bar)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(self._frame)

        # Frameless windows lose native edge resizing; grip strips overlaid on
        # the borders bring it back.
        self._grips = edge_grips(self)

    def set_body(self, body: QWidget) -> None:
        """Everything under the title bar. Called once, as the subclass
        finishes building itself."""
        self._frame_layout.addWidget(body, stretch=1)

    def apply_theme(self, theme_name: str, extra: str = "") -> None:
        """Dress the window in `theme_name`, with the subclass's own rules
        appended so they can override the shared ones."""
        self.setStyleSheet(CHROME[theme_name] + FORM[theme_name] + SHAPE + extra)
        self.title_bar.close_button.setIcon(
            window_icon("close", ICON_COLORS[theme_name])
        )

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        # A minimum or fixed size set during construction resizes the dialog
        # before the grips exist to be placed.
        if hasattr(self, "_grips"):
            place_edge_grips(self._grips, self.width(), self.height())
