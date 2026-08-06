"""The pieces a settings page is built from.

Separate from the dialog that hosts them so a page can live in its own module
(see `settings_providers`) without importing the window that shows it. The
style sheet stays with the dialog and cascades down: every widget here is
tagged with a `role` property for it to find, and none carries colours of its
own.
"""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QSizePolicy,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

# The column the controls line up in. Fixed so every row on a page shares one
# right edge; a ragged control column is what makes a settings page look
# unplanned.
CONTROL_WIDTH = 250


def label(text: str, role: str, wrap: bool = False) -> QLabel:
    """A label the style sheet can find by role, which is how every piece of
    settings text gets its size and colour without inline styling."""
    widget = QLabel(text)
    widget.setProperty("role", role)
    widget.setWordWrap(wrap)
    return widget


def rule() -> QFrame:
    line = QFrame()
    line.setProperty("role", "rule")
    line.setFixedHeight(1)
    return line


def chip(text: str) -> QPushButton:
    """A button sized for the control column, where several often share a row."""
    button = QPushButton(text)
    button.setCursor(Qt.CursorShape.PointingHandCursor)
    button.setAutoDefault(False)
    return button


def row_of(*widgets: QWidget) -> QWidget:
    """Several controls as one row's control, right-aligned together."""
    holder = QWidget()
    layout = QHBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    layout.setSpacing(6)
    layout.addStretch(1)
    for widget in widgets:
        layout.addWidget(widget)
    return holder


def stepper(spin: QSpinBox) -> QWidget:
    """A numeric setting as a value between two step buttons.

    A styled QSpinBox loses its own arrows — they are drawn from images the
    style sheet has no way to supply — and a bare number field gives no sign
    it can be nudged. Two buttons say it plainly, and they are the same chip
    every other button on the page is."""
    spin.setButtonSymbols(QSpinBox.ButtonSymbols.NoButtons)
    spin.setAlignment(Qt.AlignmentFlag.AlignCenter)
    spin.setFixedWidth(74)
    holder = QWidget()
    layout = QHBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    layout.setSpacing(6)
    down, up = chip("−"), chip("+")
    for button, step in ((down, spin.stepDown), (up, spin.stepUp)):
        button.setFixedSize(26, 26)
        # Nudges, not destinations: taking focus would put a ring on one of
        # them the moment the page opens, and the value already takes the
        # keyboard (arrows, page keys, typing).
        button.setFocusPolicy(Qt.FocusPolicy.NoFocus)
        button.clicked.connect(step)
    layout.addWidget(down)
    layout.addWidget(spin)
    layout.addWidget(up)
    return holder


class Page(QWidget):
    """One settings page. Built by calling the `add_*` helpers in reading
    order; they carry the spacing between blocks, so a page body is a list of
    what it contains rather than a pile of margins."""

    def __init__(self, *breadcrumb: str):
        super().__init__()
        self.breadcrumb = breadcrumb
        self._layout = QVBoxLayout(self)
        self._layout.setContentsMargins(26, 4, 26, 28)
        self._layout.setSpacing(0)
        self._first = True

    def add_section(self, text: str) -> None:
        """A quiet header over a run of related rows ("Active Provider")."""
        self._layout.addSpacing(0 if self._first else 24)
        self._first = False
        self._layout.addWidget(label(text, "section"))
        self._layout.addSpacing(4)

    def add_group(self, title: str) -> None:
        """A heavier header with a rule under it, for a page's major divisions
        — one per provider, per panel, per whatever the page is a list of."""
        self._layout.addSpacing(0 if self._first else 26)
        self._first = False
        self._layout.addWidget(label(title, "group"))
        self._layout.addSpacing(6)
        self._layout.addWidget(rule())

    def add_row(
        self, title: str, description: str, control: QWidget | None = None
    ) -> None:
        """A setting: its name and explanation on the left, its control right.

        The control sits in a fixed-width column so every row on the page
        agrees on one right edge; a text field fills that column while a button
        or a dropdown keeps its natural width, as in Zed."""
        self._layout.addSpacing(16)
        self._first = False
        row = QWidget()
        layout = QHBoxLayout(row)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(16)

        text = QVBoxLayout()
        text.setContentsMargins(0, 0, 0, 0)
        text.setSpacing(3)
        text.addWidget(label(title, "title"))
        if description:
            text.addWidget(label(description, "description", wrap=True))
        layout.addLayout(text, stretch=1)

        if control is not None:
            holder = QWidget()
            holder.setFixedWidth(CONTROL_WIDTH)
            holder_layout = QHBoxLayout(holder)
            holder_layout.setContentsMargins(0, 0, 0, 0)
            holder_layout.addStretch(1)
            if isinstance(control, QLineEdit):
                control.setProperty("role", "control")
                control.setSizePolicy(
                    QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed
                )
                # A value longer than the field arrives scrolled to its end,
                # which reads as a truncated setting; show it from the start.
                control.setCursorPosition(0)
            holder_layout.addWidget(control)
            layout.addWidget(holder, alignment=Qt.AlignmentFlag.AlignTop)
        self._layout.addWidget(row)

    def add_widget(self, widget: QWidget) -> None:
        """A control wide enough to be its own block — a list of everything a
        provider offers, say. The label/control split of a row would squeeze it
        into a quarter of the page for no gain."""
        self._layout.addSpacing(12)
        self._first = False
        self._layout.addWidget(widget)

    def add_actions(self, *widgets: QWidget) -> None:
        """Buttons that act on the rows above them, aligned under the control
        column. A row with an empty title would leave a gap where a setting's
        name belongs, which reads as one gone missing."""
        self._layout.addSpacing(12)
        self._first = False
        holder = QWidget()
        layout = QHBoxLayout(holder)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(6)
        layout.addStretch(1)
        for widget in widgets:
            layout.addWidget(widget)
        self._layout.addWidget(holder)

    def add_note(self, text: str) -> None:
        """Prose that is not a setting — what a still-empty page has to say,
        and what a page says about the file it is editing."""
        self._layout.addSpacing(12)
        self._first = False
        self._layout.addWidget(label(text, "description", wrap=True))

    def add_status(self, widget: QLabel) -> None:
        """A status line the page updates in place after an action."""
        self._layout.addSpacing(10)
        self._first = False
        self._layout.addWidget(widget)

    def finish(self) -> None:
        """Take up the slack, so a short page stays top-aligned."""
        self._layout.addStretch(1)
