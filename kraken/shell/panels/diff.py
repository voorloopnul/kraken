"""Diff pane: the workspace repo's uncommitted changes, file by file.

The pane answers "what has changed since the last commit" — the question you
ask after an agent has been editing for a while. Each row is one file with the
lines added and removed in it, counted against HEAD, so a change that is partly
staged is still reported as one total per file rather than split in two.
Untracked files are listed too (an agent creates them constantly); git's own
diff never mentions them, so their additions are counted here.
"""

from __future__ import annotations

import html
import shlex
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QGuiApplication
from PySide6.QtWidgets import (
    QHeaderView,
    QLabel,
    QMenu,
    QToolButton,
    QTreeWidget,
    QTreeWidgetItem,
    QWidget,
)

from kraken.shell.async_run import run_async
from kraken.shell.panels.base import SCROLLBAR_STYLES, Card, Panel
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

_TREE_STYLES = {
    "dark": """
QTreeWidget { background: transparent; border: none; color: #c8cad0;
              font-family: 'JetBrains Mono', monospace; font-size: 11px; }
QTreeWidget::item { padding: 2px 0; }
QTreeWidget::item:hover { background: #2c2e35; }
QTreeWidget::item:selected { background: #333640; color: #e8eaf0; }
""",
    "light": """
QTreeWidget { background: transparent; border: none; color: #4a4d55;
              font-family: 'JetBrains Mono', monospace; font-size: 11px; }
QTreeWidget::item { padding: 2px 0; }
QTreeWidget::item:hover { background: #e8e8ec; }
QTreeWidget::item:selected { background: #dfe3ea; color: #21232a; }
""",
}

_TEXT_COLORS = {"dark": "#c8cad0", "light": "#4a4d55"}
_DIM_COLORS = {"dark": "#7a7d85", "light": "#9a9da5"}
_ADD_COLORS = {"dark": "#98c379", "light": "#50a14f"}
_DEL_COLORS = {"dark": "#e06c75", "light": "#e45649"}

# Accent per status letter, drawn from the same One Half palette as the themes.
_LETTER_COLORS = {
    "dark": {
        "A": "#98c379", "?": "#98c379", "M": "#e5c07b", "D": "#e06c75",
        "R": "#61afef", "C": "#61afef", "T": "#c678dd", "U": "#e06c75",
    },
    "light": {
        "A": "#50a14f", "?": "#50a14f", "M": "#c18401", "D": "#e45649",
        "R": "#0184bc", "C": "#0184bc", "T": "#a626a4", "U": "#e45649",
    },
}

_LETTER_NAMES = {
    "A": "added", "?": "untracked", "M": "modified", "D": "deleted",
    "R": "renamed", "C": "copied", "T": "type changed", "U": "unmerged",
}

# Columns.
_COL_STATUS, _COL_PATH, _COL_ADDS, _COL_DELS = range(4)


@dataclass(frozen=True)
class FileChange:
    """One changed file. `adds`/`dels` are None when the count is unavailable —
    a binary file (git reports "-" for both) or an untracked file that couldn't
    be read."""

    xy: str  # raw two-letter porcelain code, e.g. "A ", " M", "??"
    path: str  # as git reports it: relative to the repo root
    orig: str | None  # where a renamed or copied file came from
    adds: int | None
    dels: int | None

    @property
    def letter(self) -> str:
        return status_letter(self.xy)


def parse_status(raw: str) -> list[tuple[str, str, str | None]]:
    """Parse `git status --porcelain=v1 -z` into (XY, path, orig) entries.

    Records are "XY PATH\\0"; a rename or copy adds a second record naming
    where the file came from, so those are consumed two at a time."""
    tokens = raw.split("\0")
    entries: list[tuple[str, str, str | None]] = []
    i = 0
    while i < len(tokens):
        token = tokens[i]
        i += 1
        # "XY " plus at least one character of path; the trailing empty token
        # after the final NUL falls out here too.
        if len(token) < 4:
            continue
        xy, path = token[:2], token[3:]
        orig = None
        if "R" in xy or "C" in xy:
            if i < len(tokens):
                orig = tokens[i] or None
                i += 1
        entries.append((xy, path, orig))
    return entries


def parse_numstat(raw: str) -> dict[str, tuple[int | None, int | None]]:
    """Parse `git diff --numstat -z` into {path: (additions, deletions)}.

    Records are "<adds>\\t<dels>\\t<path>\\0". A rename leaves the path field
    empty and follows with two records of its own — the old path, then the new
    one, which is the path the file is keyed under (matching what `git status`
    reports for it). Binary files carry "-" for both counts, which comes back
    as None."""
    tokens = [t for t in raw.split("\0") if t]
    counts: dict[str, tuple[int | None, int | None]] = {}
    i = 0
    while i < len(tokens):
        fields = tokens[i].split("\t")
        i += 1
        if len(fields) < 3:
            continue
        adds, dels, path = fields[0], fields[1], fields[2]
        if not path:  # rename: the two following tokens are old, then new
            if i + 1 >= len(tokens):
                continue
            path = tokens[i + 1]
            i += 2
        counts[path] = (_as_count(adds), _as_count(dels))
    return counts


def _as_count(field: str) -> int | None:
    try:
        return int(field)
    except ValueError:
        return None  # "-": a binary file, which has no line counts


def status_letter(xy: str) -> str:
    """One display letter for a two-letter porcelain code. The index side wins
    when both sides changed — a staged rename modified afterwards reads better
    as "R" than as "M" — except when the worktree says the file is gone, which
    is the more useful thing to know."""
    if xy == "??":
        return "?"
    if "U" in xy or xy in ("AA", "DD"):
        return "U"  # unmerged, whichever way the conflict is shaped
    if xy[1] == "D":
        return "D"
    return xy[0] if xy[0] != " " else xy[1]


def _staged_note(xy: str) -> str:
    if xy == "??":
        return "untracked"
    if xy[0] != " " and xy[1] != " ":
        return "partly staged"
    return "staged" if xy[0] != " " else "not staged"


class DiffPanel(Panel):
    """Diff pane: one row per file with uncommitted changes, showing the lines
    added and removed in it. Refreshes whenever the panel becomes visible —
    which covers toggling it on and switching workspaces — plus a manual
    refresh button, a branch switch, and the end of an agent turn."""

    # Ceilings on the work one refresh will do. A repo with an unignored build
    # directory can have thousands of untracked files; the pane stays responsive
    # and says how many rows it left out.
    _MAX_FILES = 500
    _MAX_COUNTED_UNTRACKED = 200
    # Untracked files are read to count their lines; stop at this much and
    # report no count rather than slurping a huge blob into memory.
    _MAX_READ_BYTES = 4 * 1024 * 1024

    def __init__(
        self,
        parent: QWidget | None = None,
        cwd: str | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self._cwd = cwd
        # When set, git runs on the remote host over SSH instead of on `cwd`.
        self._remote = remote
        # Bumped on every refresh; a stale async result (an earlier refresh that
        # finished after a newer one started) is discarded rather than rendered.
        self._refresh_gen = 0
        self._theme_name = DEFAULT_THEME
        self._card = Card()

        # The refresh button lives in the dock's drag-grip row (see
        # WorkspaceView), which is mounted as the card's top header.
        self.refresh_button = QToolButton()
        self.refresh_button.setText("↻")
        self.refresh_button.setToolTip("Refresh")
        self.refresh_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.refresh_button.clicked.connect(self.refresh)

        # Totals for the whole diff, and the only place the empty and error
        # states are shown — no placeholder row is faked into the tree.
        self._summary = QLabel()
        self._summary.setTextFormat(Qt.TextFormat.RichText)
        self._summary.setContentsMargins(2, 0, 2, 4)

        self._tree = QTreeWidget()
        self._tree.setColumnCount(4)
        self._tree.setHeaderHidden(True)
        self._tree.setRootIsDecorated(False)
        self._tree.setIndentation(0)
        self._tree.setUniformRowHeights(True)
        self._tree.setSelectionMode(QTreeWidget.SelectionMode.SingleSelection)
        self._tree.setSelectionBehavior(QTreeWidget.SelectionBehavior.SelectRows)
        # Full-row hover rather than just the cell under the cursor.
        self._tree.setAllColumnsShowFocus(True)
        # Paths are long and their tail is the informative end, so eat into the
        # leading directories instead of hiding the filename.
        self._tree.setTextElideMode(Qt.TextElideMode.ElideLeft)
        self._tree.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._tree.customContextMenuRequested.connect(self._on_context_menu)
        header = self._tree.header()
        header.setStretchLastSection(False)
        header.setSectionResizeMode(
            _COL_STATUS, QHeaderView.ResizeMode.ResizeToContents
        )
        header.setSectionResizeMode(_COL_PATH, QHeaderView.ResizeMode.Stretch)
        header.setSectionResizeMode(_COL_ADDS, QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(_COL_DELS, QHeaderView.ResizeMode.ResizeToContents)

        self._card.add_widget(self._summary)
        self._card.add_widget(self._tree, stretch=1)
        self.add_widget(self._card, stretch=1)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self.refresh()

    # ---- Refresh ----------------------------------------------------------

    def refresh(self) -> None:
        if not self._cwd:
            self._render(None)
            return
        # A remote gather is a blocking SSH round trip; run it off the GUI
        # thread and render when it returns, so the window never freezes. The
        # local case stays synchronous — it's fast and instant feedback is nice.
        self._refresh_gen += 1
        if self._remote is None:
            self._render(self._gather_data())
            return
        gen = self._refresh_gen
        self._show_message("Loading…")
        run_async(
            self._gather_data,
            lambda data, g=gen: self._on_gathered(g, data),
            self,
        )

    def _on_gathered(self, gen: int, data) -> None:
        # Ignore a result that a newer refresh (or a workspace switch) obsoleted.
        if gen != self._refresh_gen:
            return
        self._render(data)

    def _run_git(self, git_args: list[str], timeout: float, **kwargs):
        """Run `git <git_args>` in the workspace, locally or on the remote host.
        Remote calls carry a network round trip (plus a login-shell spawn), so
        their timeout is widened."""
        if self._remote is None:
            argv = ["git", "-C", self._cwd, *git_args]
        else:
            argv = self._remote.git_argv(git_args)
            timeout = max(timeout, 15)
        return subprocess.run(argv, timeout=timeout, **kwargs)

    def _git_text(self, git_args: list[str], timeout: float = 5):
        """stdout of a git command, or None if it couldn't run or failed."""
        try:
            result = self._run_git(
                git_args,
                timeout=timeout,
                capture_output=True,
                text=True,
                errors="replace",
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        return result if result.returncode == 0 else None

    def _gather_data(self) -> dict:
        """Run the git commands and return plain data for _render. No Qt here —
        this runs on a worker thread for remote workspaces."""
        try:
            status = self._run_git(
                ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                timeout=5,
                capture_output=True,
                text=True,
                errors="replace",
            )
        except (OSError, subprocess.TimeoutExpired):
            return {"message": "Could not run git"}
        if status.returncode != 0:
            return {
                "message": (
                    "Not a git repository"
                    if "not a git repository" in status.stderr.lower()
                    else "Could not read the working tree"
                )
            }
        entries = parse_status(status.stdout)
        shown = entries[: self._MAX_FILES]
        counts = self._numstat()
        # Only the rows that will be listed are worth counting lines for.
        untracked = [path for xy, path, _ in shown if xy == "??"]
        added = self._untracked_counts(untracked[: self._MAX_COUNTED_UNTRACKED])
        files = []
        for xy, path, orig in shown:
            if xy == "??":
                # Every line of a new file is an addition and none are removals
                # — unless the additions couldn't be counted at all, in which
                # case claiming zero removals would be inventing a number.
                adds = added.get(path)
                dels = 0 if adds is not None else None
            else:
                adds, dels = counts.get(path, (0, 0))
            files.append(FileChange(xy=xy, path=path, orig=orig, adds=adds, dels=dels))
        return {"files": files, "omitted": max(len(entries) - self._MAX_FILES, 0)}

    def _numstat(self) -> dict[str, tuple[int | None, int | None]]:
        """Per-file line counts for every tracked change, staged or not, against
        HEAD. A repo whose HEAD is unborn has no such revision to diff against,
        so its index and worktree diffs are summed instead."""
        result = self._git_text(["diff", "--numstat", "-z", "HEAD"])
        if result is not None:
            return parse_numstat(result.stdout)
        counts: dict[str, tuple[int | None, int | None]] = {}
        for args in (["diff", "--numstat", "-z", "--cached"], ["diff", "--numstat", "-z"]):
            part = self._git_text(args)
            if part is None:
                continue
            for path, (adds, dels) in parse_numstat(part.stdout).items():
                have = counts.get(path)
                if have is None:
                    counts[path] = (adds, dels)
                else:
                    counts[path] = (_add(have[0], adds), _add(have[1], dels))
        return counts

    def _untracked_counts(self, paths: list[str]) -> dict[str, int | None]:
        """Lines in each untracked file — its additions, since every line is
        new. git's diff doesn't cover untracked files, so this counts them."""
        if not paths:
            return {}
        if self._remote is not None:
            return self._remote_line_counts(paths)
        root = Path(self._cwd)
        return {path: _count_lines(root / path, self._MAX_READ_BYTES) for path in paths}

    def _remote_line_counts(self, paths: list[str]) -> dict[str, int | None]:
        """The same counts for a remote workspace, in one round trip: `wc -l`
        over the whole list. It counts newlines, so a file that doesn't end in
        one comes out a line short of what git would say — a better answer than
        no answer. A host without `wc` leaves every count unknown."""
        command = "wc -l -- " + " ".join(shlex.quote(p) for p in paths)
        try:
            result = subprocess.run(
                self._remote.ssh_argv(command),
                timeout=20,
                capture_output=True,
                text=True,
                errors="replace",
            )
        except (OSError, subprocess.TimeoutExpired):
            return {}
        counts: dict[str, int | None] = {}
        for line in result.stdout.splitlines():
            number, _, name = line.strip().partition(" ")
            name = name.strip()
            # `wc` appends a "total" line for multiple files, which is not a path.
            if name in counts or not name:
                continue
            try:
                counts[name] = int(number)
            except ValueError:
                continue
        return {path: counts.get(path) for path in paths}

    # ---- Rendering --------------------------------------------------------

    def _show_message(self, message: str) -> None:
        self._tree.clear()
        self._summary.setText(
            f'<span style="color: {_DIM_COLORS[self._theme_name]};">'
            f"{html.escape(message)}</span>"
        )

    def _render(self, data: dict | None) -> None:
        if not data or "files" not in data:
            self._show_message((data or {}).get("message") or "No changes")
            return
        files: list[FileChange] = data["files"]
        if not files:
            self._show_message("No changes")
            return
        self._render_summary(files, data.get("omitted", 0))
        self._tree.clear()
        letters = _LETTER_COLORS[self._theme_name]
        text = QColor(_TEXT_COLORS[self._theme_name])
        dim = QColor(_DIM_COLORS[self._theme_name])
        add_color = QColor(_ADD_COLORS[self._theme_name])
        del_color = QColor(_DEL_COLORS[self._theme_name])
        for change in files:
            letter = change.letter
            item = QTreeWidgetItem(
                [
                    letter,
                    change.path,
                    _count_text(change.adds, "+"),
                    _count_text(change.dels, "−"),
                ]
            )
            item.setForeground(
                _COL_STATUS, QColor(letters.get(letter, _TEXT_COLORS[self._theme_name]))
            )
            # A deleted file's path is dimmed: it is no longer there to open.
            item.setForeground(_COL_PATH, dim if letter == "D" else text)
            item.setForeground(_COL_ADDS, add_color if change.adds else dim)
            item.setForeground(_COL_DELS, del_color if change.dels else dim)
            for column in (_COL_ADDS, _COL_DELS):
                item.setTextAlignment(
                    column, Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter
                )
            item.setToolTip(_COL_PATH, _tooltip(change))
            item.setData(_COL_PATH, Qt.ItemDataRole.UserRole, change.path)
            self._tree.addTopLevelItem(item)

    def _render_summary(self, files: list[FileChange], omitted: int) -> None:
        adds = sum(c.adds or 0 for c in files)
        dels = sum(c.dels or 0 for c in files)
        count = len(files)
        theme = self._theme_name
        parts = [
            f'<span style="color: {_TEXT_COLORS[theme]};">'
            f'{count} file{"" if count == 1 else "s"} changed</span>',
            f'<span style="color: {_ADD_COLORS[theme]};">+{adds}</span>',
            f'<span style="color: {_DEL_COLORS[theme]};">−{dels}</span>',
        ]
        if omitted:
            parts.append(
                f'<span style="color: {_DIM_COLORS[theme]};">'
                f"(+{omitted} more)</span>"
            )
        self._summary.setText("&nbsp;&nbsp;".join(parts))

    # ---- Actions ----------------------------------------------------------

    def _on_context_menu(self, pos) -> None:
        item = self._tree.itemAt(pos)
        if item is None:
            return
        path = item.data(_COL_PATH, Qt.ItemDataRole.UserRole)
        if not path:
            return
        menu = QMenu(self._tree)
        copy_action = menu.addAction("Copy path")
        if menu.exec(self._tree.viewport().mapToGlobal(pos)) is copy_action:
            QGuiApplication.clipboard().setText(path)

    # ---- Theme ------------------------------------------------------------

    def set_theme(self, name: str) -> None:
        changed = name != self._theme_name
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self._tree.setStyleSheet(_TREE_STYLES[name] + SCROLLBAR_STYLES[name])
        # Row and summary colors are set per item, so re-render on a theme flip
        # (hidden panels re-render on their next showEvent).
        if changed and self.isVisible():
            self.refresh()
        dim, hover = (
            ("#7a7d85", "#2c2e35") if name == "dark" else ("#5f6269", "#e8e8ec")
        )
        self.refresh_button.setStyleSheet(
            f"QToolButton {{ background: transparent; border: none;"
            f" border-radius: 4px; color: {dim}; font-size: 13px;"
            f" padding: 0 4px; }}"
            f" QToolButton:hover {{ background: {hover}; }}"
        )


def _add(left: int | None, right: int | None) -> int | None:
    """Sum two counts, where an unknown one (a binary file) poisons the total."""
    if left is None or right is None:
        return None
    return left + right


def _count_text(count: int | None, sign: str) -> str:
    return "—" if count is None else f"{sign}{count}"


def _count_lines(path: Path, max_bytes: int) -> int | None:
    """Lines in a file, or None when it can't be counted: unreadable, binary, or
    larger than `max_bytes`. Read in chunks so a big file isn't held in memory
    whole, and counted git's way — a final line without a newline still counts."""
    lines = 0
    read = 0
    tail = b""
    try:
        with path.open("rb") as handle:
            while True:
                chunk = handle.read(64 * 1024)
                if not chunk:
                    break
                read += len(chunk)
                if read > max_bytes:
                    return None
                if b"\0" in chunk:
                    return None  # binary, like git's own numstat
                lines += chunk.count(b"\n")
                tail = chunk[-1:]
    except OSError:
        return None
    if tail and tail != b"\n":
        lines += 1
    return lines


def _tooltip(change: FileChange) -> str:
    name = _LETTER_NAMES.get(change.letter, "changed")
    note = _staged_note(change.xy)
    lines = [change.path, name if note == name else f"{name} · {note}"]
    if change.orig:
        lines.append(f"from {change.orig}")
    if change.adds is None or change.dels is None:
        lines.append("binary — no line count")
    else:
        lines.append(f"+{change.adds}  −{change.dels}")
    return "\n".join(lines)
