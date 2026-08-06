"""Application settings: a navigable two-pane window in the style of Zed's
settings UI.

The left pane is a navbar — a search field that filters it, a tree of
categories, and a keycap hint pinned to the bottom. The right pane is one
scrolling page per category: a breadcrumb across the top, then the settings
themselves as sections of rows, each row a title and a description on the left
with its control on the right.

Pages are built through `settings_page.Page`'s helpers (`add_section`,
`add_group`, `add_row`, `add_actions`) rather than by hand, so the vertical
rhythm and the label/control split stay identical across pages as the settings
fill in. A page with enough of its own logic lives in its own module and is
assembled here (`settings_providers`); the short ones are built below.
"""

from __future__ import annotations

from collections.abc import Callable
from math import cos, radians, sin

from PySide6.QtCore import QPointF, QSize, Qt, Signal
from PySide6.QtGui import QColor, QIcon, QPainter, QPen, QPixmap
from PySide6.QtWidgets import (
    QComboBox,
    QDialog,
    QHBoxLayout,
    QLineEdit,
    QScrollArea,
    QSpinBox,
    QStackedWidget,
    QToolButton,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)
from shiboken6 import isValid

from kraken.chat.typography import DEFAULT_SIZE, MAX_SIZE, MIN_SIZE, clamp
from kraken.shell.settings_models import ModelsPage
from kraken.shell.settings_page import Page, label, stepper
from kraken.shell.settings_providers import ProvidersPage
from kraken.ui.chrome import SCROLLBAR_STYLES
from kraken.ui.fonts import MONO_FAMILY, UI_SANS_FAMILY
from kraken.ui.themes import DEFAULT_THEME, THEMES

# Width of the navbar. The control column's width lives with the rows, in
# settings_page.
_NAV_WIDTH = 208

# Settings text reads as prose, so the dialog overrides the app-wide mono for
# most of its widgets; the structural labels (breadcrumb, section and group
# headers) stay mono, which is what gives the page its scaffolded look. This
# goes through the style sheet rather than setFont(): a child built without a
# parent has already resolved the application font by the time a layout adopts
# it, and reparenting does not revisit that.
_FONT_STYLE = f"""
QDialog, QDialog * {{ font-family: '{UI_SANS_FAMILY}'; }}
QLabel[role="breadcrumb"], QLabel[role="section"], QLabel[role="group"],
QLabel[role="keycap"] {{ font-family: '{MONO_FAMILY}'; }}
"""

_STYLES = {
    "dark": """
QDialog { background: #1f2127; }
QLabel { color: #c8cad0; }
QWidget#settingsNav { background: #1b1c21; border-right: 1px solid #33353c; }
QLineEdit#settingsSearch {
    background: #26282e; border: 1px solid #33353c; border-radius: 6px;
    padding: 5px 22px 5px 26px; color: #c8cad0;
}
QLineEdit#settingsSearch:focus { border-color: #4f83e0; }
QTreeWidget#settingsTree {
    background: transparent; border: none; outline: none;
    color: #9a9da5; font-size: 13px;
}
QTreeWidget#settingsTree::item { padding: 6px 6px; border-radius: 6px; }
QScrollArea#settingsScroll,
QScrollArea#settingsScroll > QWidget > QWidget,
QStackedWidget#settingsPages { background: transparent; }
QTreeWidget#settingsTree::item:hover { background: #26282e; color: #ffffff; }
QTreeWidget#settingsTree::item:selected { background: #2c2e35; color: #ffffff; }
QTreeWidget#modelTree {
    background: #1b1c21; border: 1px solid #33353c; border-radius: 6px;
    outline: none; color: #c8cad0; font-size: 12px; padding: 4px;
}
QTreeWidget#modelTree::item { padding: 3px 4px; border-radius: 4px; }
QTreeWidget#modelTree::item:hover { background: #26282e; }
QTreeWidget#modelTree::item:has-children { color: #8a8f99; }
QLabel[role="keycap"] {
    background: #26282e; border: 1px solid #33353c; border-radius: 4px;
    padding: 1px 5px; color: #9a9da5; font-size: 11px;
}
QLabel[role="hint"] { color: #6f737c; font-size: 11px; }
QLabel[role="breadcrumb"] { color: #7c8089; font-size: 12px; }
QLabel[role="section"] { color: #8a8f99; font-size: 12px; }
QLabel[role="group"] { color: #c8cad0; font-size: 13px; font-weight: 600; }
QLabel[role="title"] { color: #e6e8ec; font-size: 13px; font-weight: 600; }
QLabel[role="description"] { color: #7c8089; font-size: 12px; }
QFrame[role="rule"] { background: #33353c; border: none; }
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
QToolButton#settingsBack { border: none; border-radius: 4px; padding: 2px; }
QToolButton#settingsBack:hover { background: #2c2e35; }
QToolButton#settingsBack:disabled { opacity: 0.4; }
""",
    "light": """
QDialog { background: #faf6ec; }
QLabel { color: #383a42; }
QWidget#settingsNav { background: #fafafa; border-right: 1px solid #e0e0e0; }
QLineEdit#settingsSearch {
    background: #ffffff; border: 1px solid #d9d5c9; border-radius: 6px;
    padding: 5px 22px 5px 26px; color: #383a42;
}
QLineEdit#settingsSearch:focus { border-color: #4f83e0; }
QTreeWidget#settingsTree {
    background: transparent; border: none; outline: none;
    color: #5a5d65; font-size: 13px;
}
QTreeWidget#settingsTree::item { padding: 6px 6px; border-radius: 6px; }
QScrollArea#settingsScroll,
QScrollArea#settingsScroll > QWidget > QWidget,
QStackedWidget#settingsPages { background: transparent; }
QTreeWidget#settingsTree::item:hover { background: #e8e8ec; color: #1b1d22; }
QTreeWidget#settingsTree::item:selected { background: #e0e0e5; color: #1b1d22; }
QTreeWidget#modelTree {
    background: #ffffff; border: 1px solid #d9d5c9; border-radius: 6px;
    outline: none; color: #383a42; font-size: 12px; padding: 4px;
}
QTreeWidget#modelTree::item { padding: 3px 4px; border-radius: 4px; }
QTreeWidget#modelTree::item:hover { background: #f2eee2; }
QTreeWidget#modelTree::item:has-children { color: #7a7d85; }
QLabel[role="keycap"] {
    background: #ffffff; border: 1px solid #d9d5c9; border-radius: 4px;
    padding: 1px 5px; color: #5a5d65; font-size: 11px;
}
QLabel[role="hint"] { color: #8a8d95; font-size: 11px; }
QLabel[role="breadcrumb"] { color: #8a8d95; font-size: 12px; }
QLabel[role="section"] { color: #7a7d85; font-size: 12px; }
QLabel[role="group"] { color: #1b1d22; font-size: 13px; font-weight: 600; }
QLabel[role="title"] { color: #1b1d22; font-size: 13px; font-weight: 600; }
QLabel[role="description"] { color: #8a8d95; font-size: 12px; }
QFrame[role="rule"] { background: #e0e0e0; border: none; }
QComboBox, QSpinBox, QLineEdit[role="control"] {
    background: #ffffff; border: 1px solid #d9d5c9; border-radius: 6px;
    padding: 5px 8px; color: #383a42; font-size: 12px;
}
QComboBox:hover, QSpinBox:hover { border-color: #c2bdae; }
QComboBox:focus, QSpinBox:focus,
QLineEdit[role="control"]:focus { border-color: #4f83e0; }
QComboBox::drop-down { border: none; width: 18px; }
QSpinBox::up-button, QSpinBox::down-button {
    background: transparent; border: none; width: 14px;
}
QComboBox QAbstractItemView {
    background: #ffffff; border: 1px solid #d9d5c9;
    color: #383a42; selection-background-color: #e0e0e5;
}
QPushButton {
    background: #ffffff; border: 1px solid #d9d5c9; border-radius: 6px;
    padding: 5px 10px; color: #383a42; font-size: 12px;
}
QPushButton:hover { background: #f2eee2; }
QPushButton:pressed { background: #e8e4d8; }
QToolButton#settingsBack { border: none; border-radius: 4px; padding: 2px; }
QToolButton#settingsBack:hover { background: #e8e8ec; }
""",
}

_ICON_COLORS = {"dark": "#9a9da5", "light": "#5a5d65"}


def _search_icon(color: str) -> QIcon:
    """Magnifier for the navbar's search field, on a 16x16 canvas."""
    pixmap = QPixmap(32, 32)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.4)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    painter.setPen(pen)
    painter.drawEllipse(QPointF(6.8, 6.8), 4.2, 4.2)
    painter.drawLine(QPointF(9.9, 9.9), QPointF(13.4, 13.4))
    painter.end()
    return QIcon(pixmap)


def _back_icon(color: str) -> QIcon:
    """Left arrow for the breadcrumb, on a 16x16 canvas."""
    pixmap = QPixmap(32, 32)
    pixmap.setDevicePixelRatio(2.0)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    pen = QPen(QColor(color))
    pen.setWidthF(1.4)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    painter.setPen(pen)
    painter.drawLine(QPointF(3.0, 8.0), QPointF(13.0, 8.0))
    for dy in (-1.0, 1.0):
        angle = radians(30.0)
        painter.drawLine(
            QPointF(3.0, 8.0),
            QPointF(3.0 + cos(angle) * 4.6, 8.0 + dy * sin(angle) * 9.2),
        )
    painter.end()
    return QIcon(pixmap)


class SettingsDialog(QDialog):
    """The settings window. `theme_selected` fires the moment a theme is
    picked — the window applies it everywhere, and the dialog restyles itself
    along with the rest of the app."""

    theme_selected = Signal(str)
    font_size_selected = Signal(int)
    codex_signin_requested = Signal()

    def __init__(
        self,
        parent: QWidget | None = None,
        theme_name: str = DEFAULT_THEME,
        font_size: int = DEFAULT_SIZE,
        fetch_models: Callable[[Callable[[list], None]], None] | None = None,
    ):
        super().__init__(parent)
        self._theme_name = theme_name
        self._font_size = clamp(font_size)
        # How the Models page gets pi's catalogue. The window supplies it,
        # since only it knows which session is on screen to ask.
        self._fetch_models = fetch_models
        self._history: list[int] = []  # page indices, for the breadcrumb's back arrow
        self._going_back = False
        self._search_action = None
        self.setWindowTitle("Settings")
        self.setMinimumSize(QSize(760, 500))

        self._pages = QStackedWidget()
        self._search = QLineEdit()
        self._tree = QTreeWidget()
        self._breadcrumb = label("", "breadcrumb")
        self._back = QToolButton()

        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(self._build_nav())
        layout.addWidget(self._build_content(), stretch=1)

        for page in (
            self._general_page(),
            self._theme_page(),
            self._models_page(),
            self._providers_page(),
        ):
            page.finish()
            QTreeWidgetItem(self._tree, [page.breadcrumb[-1]])
            self._pages.addWidget(page)
        self._tree.setCurrentItem(self._tree.topLevelItem(0))

        self.set_theme(theme_name)

    # ---- Chrome ----------------------------------------------------------

    def _build_nav(self) -> QWidget:
        nav = QWidget()
        nav.setObjectName("settingsNav")
        # QWidget subclasses ignore stylesheet backgrounds without this.
        nav.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        nav.setFixedWidth(_NAV_WIDTH)

        self._search.setObjectName("settingsSearch")
        self._search.setPlaceholderText("Search settings")
        self._search.setClearButtonEnabled(True)
        self._search.textChanged.connect(self._filter_nav)

        self._tree.setObjectName("settingsTree")
        self._tree.setHeaderHidden(True)
        self._tree.setFrameShape(QTreeWidget.Shape.NoFrame)
        self._tree.setIndentation(12)
        self._tree.setUniformRowHeights(True)
        # No branch column: with every category at the top level there is
        # nothing to expand, and the strip the style reserves for the missing
        # chevron shows as a stub of highlight beside the selected row.
        self._tree.setRootIsDecorated(False)
        self._tree.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._tree.currentItemChanged.connect(self._on_nav_changed)

        # Pinned to the navbar's foot, where Zed puts its focus hint: the one
        # keystroke worth advertising here is the one that closes the window.
        hint = QHBoxLayout()
        hint.setContentsMargins(0, 0, 0, 0)
        hint.setSpacing(6)
        hint.addWidget(label("Esc", "keycap"))
        hint.addWidget(label("Close", "hint"))
        hint.addStretch(1)

        layout = QVBoxLayout(nav)
        layout.setContentsMargins(10, 12, 10, 10)
        layout.setSpacing(10)
        layout.addWidget(self._search)
        layout.addWidget(self._tree, stretch=1)
        layout.addLayout(hint)
        return nav

    def _build_content(self) -> QWidget:
        self._back.setObjectName("settingsBack")
        self._back.setCursor(Qt.CursorShape.PointingHandCursor)
        self._back.setIconSize(QSize(16, 16))
        self._back.setToolTip("Back")
        self._back.setEnabled(False)
        self._back.clicked.connect(self._go_back)

        crumbs = QHBoxLayout()
        crumbs.setContentsMargins(26, 18, 26, 10)
        crumbs.setSpacing(8)
        crumbs.addWidget(self._back)
        crumbs.addWidget(self._breadcrumb)
        crumbs.addStretch(1)

        # The pages scroll, the breadcrumb does not: it names where you are,
        # which is exactly what a long page scrolls away from.
        self._pages.setObjectName("settingsPages")
        self._scroll = QScrollArea()
        self._scroll.setObjectName("settingsScroll")
        self._scroll.setWidgetResizable(True)
        self._scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        self._scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._scroll.setWidget(self._pages)

        content = QWidget()
        layout = QVBoxLayout(content)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addLayout(crumbs)
        layout.addWidget(self._scroll, stretch=1)
        return content

    # ---- Navigation ------------------------------------------------------

    def _on_nav_changed(
        self, current: QTreeWidgetItem | None, previous: QTreeWidgetItem | None
    ) -> None:
        if current is None:
            return
        index = self._tree.indexOfTopLevelItem(current)
        if index < 0 or index == self._pages.currentIndex():
            return
        if previous is not None and not self._going_back:
            self._history.append(self._pages.currentIndex())
            self._back.setEnabled(True)
        self._pages.setCurrentIndex(index)
        self._scroll.verticalScrollBar().setValue(0)
        self._update_breadcrumb()

    def _go_back(self) -> None:
        if not self._history:
            return
        index = self._history.pop()
        self._back.setEnabled(bool(self._history))
        # Move the selection, not the stack: the navbar has to keep agreeing
        # with the page. The flag keeps the resulting change out of the
        # history, which would otherwise push the page we are leaving and make
        # Back bounce between two pages forever.
        item = self._tree.topLevelItem(index)
        if item is None:
            return
        self._going_back = True
        try:
            self._tree.setCurrentItem(item)
        finally:
            self._going_back = False

    def _filter_nav(self, query: str) -> None:
        """Hide the categories that don't match, leaving the current page
        alone — filtering the navbar is not a reason to navigate away."""
        needle = query.strip().lower()
        for i in range(self._tree.topLevelItemCount()):
            item = self._tree.topLevelItem(i)
            item.setHidden(bool(needle) and needle not in item.text(0).lower())

    def _update_breadcrumb(self) -> None:
        page = self._pages.currentWidget()
        if not isinstance(page, Page):
            return
        trail, current = page.breadcrumb[:-1], page.breadcrumb[-1]
        colors = {"dark": "#c8cad0", "light": "#383a42"}
        lead = "".join(f"{part} / " for part in trail)
        self._breadcrumb.setText(
            f"{lead}<span style='color: {colors[self._theme_name]}'>{current}</span>"
        )

    # ---- Pages -----------------------------------------------------------

    def _general_page(self) -> Page:
        page = Page("Settings", "General")
        page.add_section("General")
        page.add_note("No general settings yet.")
        return page

    def _theme_page(self) -> Page:
        page = Page("Settings", "Theme")
        page.add_section("Appearance")
        self._theme_picker = QComboBox()
        self._font_picker = QSpinBox()
        for name in THEMES:
            self._theme_picker.addItem(name.capitalize(), name)
        self._theme_picker.setCurrentIndex(
            self._theme_picker.findData(self._theme_name)
        )
        self._theme_picker.currentIndexChanged.connect(
            lambda _: self._on_theme_picked(self._theme_picker.currentData())
        )
        page.add_row(
            "Theme",
            "Colours for the whole app: chrome, panels, and the terminal "
            "palette. Applies immediately, to every open workspace.",
            self._theme_picker,
        )
        self._font_picker.setRange(MIN_SIZE, MAX_SIZE)
        self._font_picker.setSuffix(" px")
        self._font_picker.setValue(self._font_size)
        self._font_picker.valueChanged.connect(self._on_font_size_picked)
        page.add_row(
            "Conversation Font Size",
            "Base size for the chat. Message text is set at it, and the rest "
            "of the pane — headings, code, the composer, tool detail — is "
            "sized from it. Applies to open transcripts as well as new ones.",
            stepper(self._font_picker),
        )
        return page

    def _providers_page(self) -> Page:
        page = ProvidersPage()
        # Sign-in needs a terminal, which is the window's to give: pass it on
        # and close, since a modal dialog is exactly what stands between the
        # user and the flow it just started.
        page.codex_signin_requested.connect(self._on_codex_signin)
        return page

    def _models_page(self) -> Page:
        return ModelsPage(self._guarded_fetch)

    def _guarded_fetch(self, callback: Callable[[list], None]) -> None:
        """Ask the window for pi's models, and answer the page only while there
        is still a page to answer.

        The fetch outlives the dialog by design — it answers on a timeout even
        when pi never does — and the dialog is deleted when it closes, so
        without this the reply would write into a destroyed widget."""
        if self._fetch_models is None:
            callback([])
            return
        self._fetch_models(lambda models: isValid(self) and callback(models))

    # ---- Theme -----------------------------------------------------------

    def _on_codex_signin(self) -> None:
        self.codex_signin_requested.emit()
        self.accept()

    def _on_font_size_picked(self, size: int) -> None:
        if size == self._font_size:
            return
        self._font_size = size
        self.font_size_selected.emit(size)

    def _on_theme_picked(self, name: str | None) -> None:
        if not name or name == self._theme_name:
            return
        # Restyle first: the window's set_theme repaints everything except this
        # dialog, which is not part of its widget tree.
        self.set_theme(name)
        self.theme_selected.emit(name)

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        self.setStyleSheet(_STYLES[name] + SCROLLBAR_STYLES[name] + _FONT_STYLE)
        color = _ICON_COLORS[name]
        # The icons are painted in the theme's own grey, so they are recoloured
        # rather than re-added; a second addAction would stack two magnifiers.
        if self._search_action is None:
            self._search_action = self._search.addAction(
                _search_icon(color), QLineEdit.ActionPosition.LeadingPosition
            )
        else:
            self._search_action.setIcon(_search_icon(color))
        self._back.setIcon(_back_icon(color))
        self._update_breadcrumb()
