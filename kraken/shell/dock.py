"""A draggable panel dock for the workspace.

The dock arranges content panels into columns laid out left to right in a
horizontal splitter. Each column is a vertical splitter holding one or two
panels, so the workspace can stack (for example) the terminal above the git
history in a single column. Every panel carries a slim header that doubles as
a drag handle: grab it and drop the panel beside a column (to open a new
column) or onto the top/bottom half of a column (to stack it there, up to the
two-panel limit).

`WorkspaceView` owns the content panels and registers each one here under a
short key ("left", "center", …); the dock only reparents those widgets between
splitters, so a panel's own state (terminals, browser, transcripts) survives a
move untouched."""

from __future__ import annotations

from PySide6.QtCore import QPoint, QRect, QRectF, Qt, Signal
from PySide6.QtGui import QColor, QFont, QPainter
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QSplitter,
    QSplitterHandle,
    QVBoxLayout,
    QWidget,
)

from kraken.ui.fonts import UI_SANS_FAMILY
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

# Preferred column widths per panel key, used to size a column when the dock
# structure changes (a panel shown, hidden, or dropped elsewhere).
_PREF_WIDTH = {"left": 300, "center": 700, "browser": 480, "git": 360, "right": 460}

# How far into a column counts as its side band (a drop there opens a new
# column) versus its middle (a drop there stacks into the column).
_EDGE_BAND = 0.28
# Pixels the header must travel before a click turns into a drag.
_DRAG_THRESHOLD = 6

# The grip sits inside the panel's card, so it paints transparent over the
# card and only carries a faint bottom rule to separate it from the content.
_HEADER_COLORS = {
    "dark": {"text": "#c8cad0", "grip": "#5a5d65", "rule": "#33353c", "hover": "#2c2e35"},
    "light": {"text": "#4a4d55", "grip": "#a9acb4", "rule": "#e4e4e8", "hover": "#ececef"},
}
_GRIP_COLORS = {"dark": "#3a3d45", "light": "#c9c4b4"}
# Drop indicator: a translucent fill with a solid accent border.
_DROP_FILL = QColor(79, 131, 224, 60)
_DROP_BORDER = QColor(79, 131, 224, 220)


class _GripHandle(QSplitterHandle):
    """A splitter handle that paints a short rounded grip bar; the stock dotted
    handle nearly vanishes against the light theme's background."""

    _LENGTH = 36.0
    _THICKNESS = 3.0

    def paintEvent(self, event) -> None:
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(self.splitter().grip_color)
        if self.orientation() == Qt.Orientation.Horizontal:
            rect = QRectF(
                (self.width() - self._THICKNESS) / 2,
                (self.height() - self._LENGTH) / 2,
                self._THICKNESS,
                self._LENGTH,
            )
        else:
            rect = QRectF(
                (self.width() - self._LENGTH) / 2,
                (self.height() - self._THICKNESS) / 2,
                self._LENGTH,
                self._THICKNESS,
            )
        painter.drawRoundedRect(rect, self._THICKNESS / 2, self._THICKNESS / 2)


class _GripSplitter(QSplitter):
    """Splitter whose handles paint a visible grip bar (see _GripHandle)."""

    def __init__(self, orientation: Qt.Orientation, parent: QWidget | None = None):
        super().__init__(orientation, parent)
        self.grip_color = QColor(_GRIP_COLORS[DEFAULT_THEME])
        self.setHandleWidth(8)
        self.setChildrenCollapsible(False)

    def set_grip_color(self, color: str) -> None:
        self.grip_color = QColor(color)
        for i in range(1, self.count()):
            self.handle(i).update()

    def createHandle(self) -> QSplitterHandle:
        return _GripHandle(self.orientation(), self)


class _PanelHeader(QWidget):
    """The slim bar atop each panel: a two-dot grip glyph plus the panel title.
    Pressing and dragging it moves the whole panel; the DockArea listens to
    these signals and drives the move."""

    pressed = Signal(QPoint)  # global cursor position at press
    moved = Signal(QPoint)  # global cursor position while the button is held
    released = Signal(QPoint)  # global cursor position at release

    def __init__(
        self, title: str, draggable: bool = True, parent: QWidget | None = None
    ):
        super().__init__(parent)
        # A styled background/border only paints on a plain QWidget with this.
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setFixedHeight(24)
        self.draggable = draggable
        if draggable:
            self.setCursor(Qt.CursorShape.OpenHandCursor)
        self._down = False
        font = QFont(UI_SANS_FAMILY)
        font.setPixelSize(12)
        font.setWeight(QFont.Weight.DemiBold)
        # A non-draggable header carries no grip glyph — there's nothing to grab.
        self._grip = QLabel("⠿" if draggable else "")
        self._title = QLabel(title)
        self._title.setFont(font)
        # The card already pads its edges, so the grip only needs a little
        # breathing room of its own.
        self._layout = QHBoxLayout(self)
        self._layout.setContentsMargins(2, 0, 2, 0)
        self._layout.setSpacing(6)
        self._layout.addWidget(self._grip)
        self._layout.addWidget(self._title)
        self._layout.addStretch(1)

    def add_trailing(self, widget: QWidget) -> None:
        """Add a control (e.g. a refresh button) at the far right of the grip
        row, past the stretch."""
        self._layout.addWidget(widget)

    def set_theme(self, name: str) -> None:
        c = _HEADER_COLORS[name]
        # Transparent over the card; a hover tint hints the drag affordance,
        # but only where there is actually something to grab.
        hover = (
            f" _PanelHeader:hover {{ background: {c['hover']}; }}"
            if self.draggable
            else ""
        )
        self.setStyleSheet(
            f"_PanelHeader {{ background: transparent;"
            f" border-bottom: 1px solid {c['rule']}; }}"
            f"{hover}"
            f" QLabel {{ color: {c['text']}; background: transparent;"
            " font-size: 12px; }"
        )
        self._grip.setStyleSheet(f"color: {c['grip']}; font-size: 13px;")

    # A left-button press starts a potential drag; Qt's implicit mouse grab
    # keeps the moves and the release coming here even over other widgets.
    def mousePressEvent(self, event) -> None:
        if not self.draggable:
            return super().mousePressEvent(event)
        if event.button() == Qt.MouseButton.LeftButton:
            self._down = True
            self.setCursor(Qt.CursorShape.ClosedHandCursor)
            self.pressed.emit(event.globalPosition().toPoint())
            event.accept()

    def mouseMoveEvent(self, event) -> None:
        if self._down:
            self.moved.emit(event.globalPosition().toPoint())
            event.accept()

    def mouseReleaseEvent(self, event) -> None:
        if self._down and event.button() == Qt.MouseButton.LeftButton:
            self._down = False
            self.setCursor(Qt.CursorShape.OpenHandCursor)
            self.released.emit(event.globalPosition().toPoint())
            event.accept()


class DockPanel(QWidget):
    """One dockable panel. Its drag-handle header lives *inside* the content
    panel (mounted into the card's top row), so the grip reads as part of the
    panel rather than a bar wrapped around it. The content is created and
    themed by WorkspaceView; the dock only moves the DockPanel between columns,
    so the content's live state is preserved."""

    def __init__(
        self, key: str, title: str, content: QWidget, draggable: bool = True
    ):
        super().__init__()
        self.key = key
        self.content = content
        self.draggable = draggable
        # Only a draggable panel carries a grip header; the fixed anchors
        # (History, Conversation) show no header at all.
        self.header = _PanelHeader(title, draggable=True) if draggable else None
        if self.header is not None:
            content.mount_grip(self.header)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(content, stretch=1)

    def set_theme(self, name: str) -> None:
        if self.header is not None:
            self.header.set_theme(name)


class _DockColumn(_GripSplitter):
    """A vertical splitter holding one or two DockPanels."""

    def __init__(self, parent: QWidget | None = None):
        super().__init__(Qt.Orientation.Vertical, parent)

    def panels(self) -> list[DockPanel]:
        return [self.widget(i) for i in range(self.count())]


class _DropOverlay(QWidget):
    """A transparent sheet over the dock that paints the pending drop zone
    while a panel is being dragged. It never takes mouse input, so the drag's
    implicit grab keeps flowing to the panel header underneath."""

    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)
        self.hide()
        self._rect = QRect()

    def show_zone(self, rect: QRect) -> None:
        self._rect = rect
        if not self.isVisible():
            self.show()
        self.raise_()
        self.update()

    def paintEvent(self, event) -> None:
        if self._rect.isNull():
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setBrush(_DROP_FILL)
        painter.setPen(_DROP_BORDER)
        painter.drawRoundedRect(self._rect.adjusted(1, 1, -1, -1), 6, 6)


class DockArea(QWidget):
    """Hosts the columns and runs drag-and-drop between them.

    `order` is the canonical left-to-right key order; a panel shown from hidden
    slots into a fresh column at the position that order implies. `stretch_key`
    names the panel whose column absorbs spare width (the conversation)."""

    def __init__(
        self,
        order: list[str],
        stretch_key: str,
        fixed_keys: set[str] | None = None,
        no_stack_keys: set[str] | None = None,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self._order = order
        self._stretch_key = stretch_key
        # Fixed panels are hard anchors: never stacked with, and nothing may
        # open a column to their left. No-stack panels are softer: nothing may
        # stack with them either, but columns can still open on either side.
        # (Both are made non-draggable by WorkspaceView.) Fixed implies
        # no-stack.
        self._fixed = set(fixed_keys or ())
        self._no_stack = set(no_stack_keys or ()) | self._fixed
        self._theme_name = DEFAULT_THEME
        self._panels: dict[str, DockPanel] = {}

        self._splitter = _GripSplitter(Qt.Orientation.Horizontal)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self._splitter)

        self._overlay = _DropOverlay(self)

        # Keys currently shown (logical visibility, independent of whether the
        # whole view is on screen). Panels stay parented in their column even
        # while hidden — show/hide only flips visibility, never reparents or
        # deletes, so nothing can be used-after-free while a heavy panel's
        # reparent pumps a nested event loop.
        self._shown: set[str] = set()

        # Drag state, live only between a header press and its release.
        self._drag_panel: DockPanel | None = None
        self._drag_origin: QPoint | None = None
        self._dragging = False
        self._drop_target: tuple[str, _DockColumn] | None = None

    # ---- Registration & layout -------------------------------------------

    def register(self, panel: DockPanel) -> None:
        """Make a panel known to the dock (hidden until placed). Wires its
        header so a drag on it is routed here."""
        self._panels[panel.key] = panel
        if not panel.draggable:
            return  # anchored panel: no header, no drag wiring
        panel.header.pressed.connect(lambda pos, p=panel: self._on_press(p, pos))
        panel.header.moved.connect(self._on_move)
        panel.header.released.connect(self._on_release)

    def set_layout(self, columns: list[list[str]]) -> None:
        """Build the initial column layout from a list of columns, each a list
        of panel keys (top to bottom)."""
        for keys in columns:
            column = self._acquire_column()
            for key in keys:
                column.addWidget(self._panels[key])
                self._shown.add(key)
            self._splitter.insertWidget(self._splitter.count(), column)
            column.setVisible(True)
        self._reflow()

    def is_visible(self, key: str) -> bool:
        return key in self._shown

    def show_panel(self, key: str) -> None:
        """Reveal a panel. It reappears in whatever column it last lived in; a
        panel that has never been placed gets a fresh column positioned to
        match the canonical order."""
        if key in self._shown:
            return
        pre = self._snapshot()
        self._shown.add(key)
        panel = self._panels[key]
        column = panel.parentWidget()
        if not isinstance(column, _DockColumn):
            column = self._acquire_column()
            column.addWidget(panel)
            self._splitter.insertWidget(self._insertion_index(key), column)
        panel.setVisible(True)
        column.setVisible(True)
        self._reflow(pre)

    def hide_panel(self, key: str) -> None:
        """Hide a panel in place — it stays parented in its column so a later
        show can restore it. A column left with nothing visible is hidden too
        (kept, empty, for reuse) rather than destroyed."""
        if key not in self._shown:
            return
        pre = self._snapshot()
        self._shown.discard(key)
        panel = self._panels[key]
        panel.setVisible(False)
        column = panel.parentWidget()
        if isinstance(column, _DockColumn) and not self._column_has_shown(column):
            column.setVisible(False)
        self._reflow(pre)

    def _insertion_index(self, key: str) -> int:
        """Splitter index for a fresh column holding `key`, placed before the
        first active column that ranks after it in the canonical order."""
        rank = self._order.index(key)
        for column in self._columns():
            reps = [
                self._order.index(p.key)
                for p in column.panels()
                if p.key in self._shown
            ]
            if reps and min(reps) > rank:
                return self._splitter.indexOf(column)
        return self._splitter.count()

    # ---- Column bookkeeping ----------------------------------------------

    def _columns(self) -> list[_DockColumn]:
        return [self._splitter.widget(i) for i in range(self._splitter.count())]

    def _active_columns(self) -> list[_DockColumn]:
        return [c for c in self._columns() if self._column_has_shown(c)]

    def _column_has_shown(self, column: _DockColumn) -> bool:
        return any(p.key in self._shown for p in column.panels())

    def _acquire_column(self) -> _DockColumn:
        """A column to place a panel in: reuse an emptied (childless) one if the
        dock has one lying around, else make a new one. Reusing keeps the dock
        from having to delete columns at all."""
        for column in self._columns():
            if column.count() == 0:
                return column
        return _DockColumn()

    def _snapshot(self) -> dict[_DockColumn, int]:
        """Current pixel widths of the visible columns, captured before a
        structural change so the change can preserve them."""
        return {c: c.width() for c in self._active_columns()}

    def _pref_width(self, column: _DockColumn) -> int:
        return max(
            (_PREF_WIDTH.get(p.key, 400) for p in column.panels()), default=400
        )

    def _reflow(self, pre: dict[_DockColumn, int] | None = None) -> None:
        """Recompute per-column stretch, grip colors, and widths after a
        structural or visibility change. Each surviving column keeps the width
        it had in `pre`; a column that is newly shown takes its preferred width;
        the stretch column (conversation) absorbs the slack so the row still
        fills the splitter. With no `pre`, every column starts at preferred."""
        pre = pre or {}
        columns = self._columns()
        for i, column in enumerate(columns):
            keys = [p.key for p in column.panels()]
            self._splitter.setStretchFactor(i, 1 if self._stretch_key in keys else 0)
            column.set_grip_color(_GRIP_COLORS[self._theme_name])
        self._splitter.set_grip_color(_GRIP_COLORS[self._theme_name])

        active = [c for c in columns if self._column_has_shown(c)]
        if not active:
            return
        stretch_col = next(
            (c for c in active if any(p.key == self._stretch_key for p in c.panels())),
            None,
        )
        width: dict[_DockColumn, int] = {}
        fixed_total = 0
        for column in active:
            if column is stretch_col:
                continue
            w = pre.get(column) or 0
            width[column] = w if w > 0 else self._pref_width(column)
            fixed_total += width[column]
        if stretch_col is not None:
            avail = self._splitter.width()
            # The stretch column takes whatever is left, shrinking (down to a
            # floor; Qt still enforces its real minimum) so the preserved
            # columns keep their exact widths rather than being scaled down.
            width[stretch_col] = (
                max(avail - fixed_total, 1) if avail > 0 else self._pref_width(stretch_col)
            )
        self._splitter.setSizes([width.get(c, 0) for c in columns])

    # ---- Drag & drop ------------------------------------------------------

    def _on_press(self, panel: DockPanel, pos: QPoint) -> None:
        self._drag_panel = panel
        self._drag_origin = pos
        self._dragging = False

    def _on_move(self, pos: QPoint) -> None:
        if self._drag_panel is None or self._drag_origin is None:
            return
        if not self._dragging:
            if (pos - self._drag_origin).manhattanLength() < _DRAG_THRESHOLD:
                return
            self._dragging = True
        target = self._hit_test(pos)
        self._drop_target = target
        if target is None:
            self._overlay.hide()
        else:
            self._overlay.setGeometry(self._splitter.geometry())
            self._overlay.show_zone(self._zone_rect(target))

    def _on_release(self, pos: QPoint) -> None:
        panel, target = self._drag_panel, self._drop_target
        dragging = self._dragging
        self._drag_panel = None
        self._drag_origin = None
        self._dragging = False
        self._drop_target = None
        self._overlay.hide()
        if dragging and panel is not None and target is not None:
            self._apply_drop(panel, target)

    def _hit_test(self, global_pos: QPoint) -> tuple[str, _DockColumn] | None:
        """Map the cursor to a drop descriptor: a ("mode", column) pair where
        mode is one of new_before / new_after / stack_top / stack_bottom.
        Returns None when the drop would be a no-op or exceeds two per column."""
        columns = self._active_columns()
        if not columns:
            return None
        local = self._splitter.mapFromGlobal(global_pos)
        column = self._column_at(local.x(), columns)
        keys = [p.key for p in column.panels()]
        # A fixed panel hard-anchors its column: no new column opens to its
        # left. A no-stack panel (which every fixed one also is) only refuses
        # to be stacked with.
        fixed_col = any(k in self._fixed for k in keys)
        nostack_col = any(k in self._no_stack for k in keys)
        rect = column.geometry()
        rel_x = (local.x() - rect.x()) / max(rect.width(), 1)
        if rel_x < _EDGE_BAND:
            if fixed_col:
                return None
            return self._collapse_noop("new_before", column)
        if rel_x > 1 - _EDGE_BAND:
            return self._collapse_noop("new_after", column)
        # Middle band: stack into this column, unless it refuses stacking or
        # already holds two panels other than the one being dragged. The limit
        # counts all children (a hidden co-panel still occupies the column).
        panels = column.panels()
        if nostack_col or (len(panels) >= 2 and self._drag_panel not in panels):
            return None
        rel_y = (local.y() - rect.y()) / max(rect.height(), 1)
        return ("stack_top", column) if rel_y < 0.5 else ("stack_bottom", column)

    def _collapse_noop(
        self, mode: str, column: _DockColumn
    ) -> tuple[str, _DockColumn] | None:
        """Drop a "new column beside" target that would just put the dragged
        panel back where it already sits alone — those moves change nothing."""
        shown = [p for p in column.panels() if p.key in self._shown]
        if shown == [self._drag_panel]:
            return None
        return (mode, column)

    def _column_at(self, x: int, columns: list[_DockColumn]) -> _DockColumn:
        for column in columns:
            if x < column.geometry().right():
                return column
        return columns[-1]

    def _zone_rect(self, target: tuple[str, _DockColumn]) -> QRect:
        mode, column = target
        rect = column.geometry()
        if mode == "new_before":
            return QRect(rect.x(), rect.y(), int(rect.width() * 0.4), rect.height())
        if mode == "new_after":
            w = int(rect.width() * 0.4)
            return QRect(rect.right() - w, rect.y(), w, rect.height())
        if mode == "stack_top":
            return QRect(rect.x(), rect.y(), rect.width(), rect.height() // 2)
        return QRect(
            rect.x(), rect.y() + rect.height() // 2, rect.width(), rect.height() // 2
        )

    def _apply_drop(self, panel: DockPanel, target: tuple[str, _DockColumn]) -> None:
        mode, dest = target
        # Widths captured before the move so an unstacked column keeps the width
        # the stack had, rather than snapping back to its preferred size.
        pre = self._snapshot()
        source = panel.parentWidget()
        if not isinstance(source, _DockColumn):
            source = None
        if mode in ("stack_top", "stack_bottom"):
            if source is dest and dest.count() <= 1:
                return  # already the lone panel here; nothing to reorder
            panel.setParent(None)  # detach before re-inserting
            at = 0 if mode == "stack_top" else dest.count()
            dest.insertWidget(min(at, dest.count()), panel)
            # A freshly stacked column splits its height evenly.
            half = max(dest.height() // 2, 1)
            dest.setSizes([half] * dest.count())
        else:
            column = self._acquire_column()
            column.addWidget(panel)  # reparents out of source
            at = self._splitter.indexOf(dest)
            self._splitter.insertWidget(
                at if mode == "new_before" else at + 1, column
            )
            column.setVisible(True)
        dest.setVisible(True)
        panel.setVisible(True)
        # An emptied source is hidden (and kept for reuse), never deleted.
        if source is not None and source is not dest and not self._column_has_shown(source):
            source.setVisible(False)
        self._reflow(pre)

    # ---- Theme ------------------------------------------------------------

    def set_theme(self, name: str) -> None:
        self._theme_name = name
        ui = UI_COLORS[name]
        # Styling the top splitter covers the gaps behind the columns and
        # cascades a default text color to panel labels (headers override it).
        self._splitter.setStyleSheet(
            f"QSplitter {{ background: {ui['window']}; }}"
            f" QLabel {{ color: {ui['text']}; }}"
        )
        grip = _GRIP_COLORS[name]
        self._splitter.set_grip_color(grip)
        for column in self._columns():
            column.set_grip_color(grip)
        for panel in self._panels.values():
            panel.set_theme(name)
