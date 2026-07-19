"""Git history pane: the workspace repo's commit graph."""

from __future__ import annotations

import html
import subprocess
from typing import TYPE_CHECKING

from PySide6.QtCore import Qt, QSize
from PySide6.QtGui import QGuiApplication, QTextDocument
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QStyle,
    QStyledItemDelegate,
    QStyleOptionViewItem,
    QToolButton,
    QWidget,
)

from kraken.shell.async_run import run_async
from kraken.shell.panels.base import Card, Panel
from kraken.ui.themes import DEFAULT_THEME, UI_COLORS

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

# Scrollbars match the conversation panel's external one: a bare rounded
# handle on a transparent track, no stepper buttons.
_GIT_LOG_STYLES = {
    "dark": """
QListWidget { background: transparent; border: none; color: #c8cad0;
              font-family: 'JetBrains Mono', monospace; font-size: 11px; }
QListWidget::item { padding: 2px 4px; border-radius: 4px; }
QListWidget::item:hover { background: #2c2e35; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #3a3d45; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #4a4e58; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #3a3d45; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #4a4e58; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
    "light": """
QListWidget { background: transparent; border: none; color: #4a4d55;
              font-family: 'JetBrains Mono', monospace; font-size: 11px; }
QListWidget::item { padding: 2px 4px; border-radius: 4px; }
QListWidget::item:hover { background: #e8e8ec; }
QScrollBar:horizontal { background: transparent; border: none; height: 10px; margin: 0; }
QScrollBar::handle:horizontal { background: #c9c4b4; border-radius: 5px; min-width: 24px; }
QScrollBar::handle:horizontal:hover { background: #b3ae9e; }
QScrollBar:vertical { background: transparent; border: none; width: 10px; margin: 0; }
QScrollBar::handle:vertical { background: #c9c4b4; border-radius: 5px; min-height: 24px; }
QScrollBar::handle:vertical:hover { background: #b3ae9e; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QAbstractScrollArea::corner { background: transparent; border: none; }
""",
}


class _RichTextDelegate(QStyledItemDelegate):
    """Paints item text as HTML so parts of a row (the commit hash) can be
    colored independently; backgrounds still come from the widget style."""

    def paint(self, painter, option, index) -> None:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        doc = self._document(opt)
        opt.text = ""
        style = opt.widget.style()
        style.drawControl(
            QStyle.ControlElement.CE_ItemViewItem, opt, painter, opt.widget
        )
        rect = style.subElementRect(
            QStyle.SubElement.SE_ItemViewItemText, opt, opt.widget
        )
        painter.save()
        painter.translate(
            rect.left(), rect.top() + (rect.height() - doc.size().height()) / 2
        )
        doc.drawContents(painter)
        painter.restore()

    def sizeHint(self, option, index) -> QSize:
        opt = QStyleOptionViewItem(option)
        self.initStyleOption(opt, index)
        doc = self._document(opt)
        # Mirror the stylesheet's 2px/4px item padding.
        return QSize(int(doc.idealWidth()) + 8, int(doc.size().height()) + 4)

    @staticmethod
    def _document(opt: QStyleOptionViewItem) -> QTextDocument:
        doc = QTextDocument()
        doc.setDocumentMargin(0)
        doc.setDefaultFont(opt.font)
        doc.setHtml(opt.text)
        return doc


def _mono(text: str, color: str) -> str:
    """Escape one span of row text, keeping runs of spaces (graph columns)."""
    return (
        f'<span style="color: {color};">'
        f'{html.escape(text).replace(" ", "&nbsp;")}</span>'
    )


# Row text per theme, plus the hash accents: pastel green for commits on the
# main line (reachable from master/main), light gray for side-branch work.
_GIT_TEXT_COLORS = {"dark": "#c8cad0", "light": "#4a4d55"}
_MAIN_HASH_COLORS = {"dark": "#98c379", "light": "#50a14f"}
_OFF_MAIN_COLORS = {"dark": "#7a7d85", "light": "#9a9da5"}


class GitPanel(Panel):
    """Git history pane: the workspace repo's commit graph, one row per
    `git log --graph` line. Refreshes whenever the panel becomes visible —
    which covers toggling it on and switching workspaces — plus a manual
    refresh button for commits made while it's showing."""

    _MAX_COMMITS = 200

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

        header = QWidget()
        header_layout = QHBoxLayout(header)
        header_layout.setContentsMargins(0, 0, 0, 0)
        self.refresh_button = QToolButton()
        self.refresh_button.setText("↻")
        self.refresh_button.setToolTip("Refresh")
        self.refresh_button.setCursor(Qt.CursorShape.PointingHandCursor)
        self.refresh_button.clicked.connect(self.refresh)
        header_layout.addWidget(QLabel("Git History"))
        header_layout.addStretch(1)
        header_layout.addWidget(self.refresh_button)

        # Rows act through their context menu only, so no selection; the
        # monospace font keeps the graph columns aligned across rows.
        self._list = QListWidget()
        self._list.setSelectionMode(QListWidget.SelectionMode.NoSelection)
        self._list.setWordWrap(False)
        self._list.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._list.customContextMenuRequested.connect(self._on_context_menu)
        # Rows are HTML (see _RichTextDelegate) so the hash can carry its
        # own color while the rest of the row keeps the theme text color.
        self._list.setItemDelegate(_RichTextDelegate(self._list))

        self._card.add_widget(header)
        self._card.add_widget(self._list, stretch=1)
        self.add_widget(self._card, stretch=1)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self.refresh()

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

    def refresh(self) -> None:
        if not self._cwd:
            self._list.clear()
            return
        # A remote gather is a blocking SSH round trip; run it off the GUI
        # thread and render when it returns, so the window never freezes. The
        # local case stays synchronous — it's fast and instant feedback is nice.
        self._refresh_gen += 1
        if self._remote is None:
            self._render_rows(self._gather_data())
            return
        gen = self._refresh_gen
        self._show_message("Loading…")
        run_async(
            self._gather_data,
            lambda data, g=gen: self._on_gathered(g, data),
            self,
        )

    def _show_message(self, message: str) -> None:
        self._list.clear()
        item = QListWidgetItem(_mono(message, _GIT_TEXT_COLORS[self._theme_name]))
        item.setFlags(Qt.ItemFlag.NoItemFlags)
        self._list.addItem(item)

    def _on_gathered(self, gen: int, data) -> None:
        # Ignore a result that a newer refresh (or a workspace switch) obsoleted.
        if gen != self._refresh_gen:
            return
        self._render_rows(data)

    def _gather_data(self) -> dict:
        """Run the git commands and return plain data for _render_rows. No Qt
        here — this runs on a worker thread for remote workspaces."""
        # Branches, tags, remotes, and HEAD — not --all, which also walks
        # tool-owned refs (editor local-history, agent checkpoints) and shows
        # "history" in repos whose branches have no commits at all. HEAD is
        # listed explicitly so a detached checkout (from the context menu)
        # stays visible, but only when it resolves: an unborn branch's HEAD
        # would make git log fail outright.
        revs = ["--branches", "--tags", "--remotes"]
        if self._head_exists():
            revs.append("HEAD")
        # \x1f-separated fields after the graph prefix; continuation lines
        # (pure graph, like "|/") carry no record at all.
        try:
            result = self._run_git(
                [
                    "log", "--graph", *revs,
                    "--format=%x1f%h%x1f%H%x1f%d%x1f%s%x1f%an%x1f%ar",
                    f"-{self._MAX_COMMITS}",
                ],
                timeout=5,
                capture_output=True,
                text=True,
                errors="replace",
            )
        except (OSError, subprocess.TimeoutExpired):
            result = None
        if result is None or result.returncode != 0:
            stderr = result.stderr.lower() if result is not None else ""
            message = (
                "Not a git repository"
                if "not a git repository" in stderr
                else "No commits"
            )
            return {"message": message}
        return {"stdout": result.stdout, "main_line": self._main_line_hashes()}

    def _render_rows(self, data: dict) -> None:
        self._list.clear()
        text_color = _GIT_TEXT_COLORS[self._theme_name]
        if not data or "stdout" not in data:
            item = QListWidgetItem(
                _mono(data.get("message", "No commits") if data else "No commits", text_color)
            )
            item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._list.addItem(item)
            return
        main_line = data["main_line"]
        for line in data["stdout"].splitlines():
            graph, sep, record = line.partition("\x1f")
            if sep:
                short_hash, full_hash, refs, subject, author, when = record.split(
                    "\x1f"
                )
                if main_line is None:
                    hash_color = text_color  # no master/main to compare with
                elif full_hash in main_line:
                    hash_color = _MAIN_HASH_COLORS[self._theme_name]
                else:
                    hash_color = _OFF_MAIN_COLORS[self._theme_name]
                item = QListWidgetItem(
                    _mono(graph, text_color)
                    + _mono(short_hash, hash_color)
                    + _mono(f"{refs}  {subject}", text_color)
                )
                item.setToolTip(f"{subject}\n{short_hash} — {author}, {when}")
                item.setData(Qt.ItemDataRole.UserRole, short_hash)
                item.setData(Qt.ItemDataRole.UserRole + 2, full_hash)
                # Branches decorating this commit, offered as attached
                # checkouts in the context menu. "HEAD -> x" is the branch
                # we're already on; tags and a bare detached "HEAD" checkout
                # the same as the hash, so neither earns an entry.
                ref_names = [r for r in refs.strip(" ()").split(", ") if r]
                branches = [
                    ref
                    for ref in ref_names
                    if ref != "HEAD" and not ref.startswith(("HEAD -> ", "tag: "))
                ]
                item.setData(Qt.ItemDataRole.UserRole + 1, branches)
                # Bold the checked-out commit so the current state stands out.
                if any(r == "HEAD" or r.startswith("HEAD -> ") for r in ref_names):
                    font = item.font()
                    font.setBold(True)
                    item.setFont(font)
            else:
                item = QListWidgetItem(_mono(graph.rstrip(), text_color))
            self._list.addItem(item)
        if self._list.count() == 0:
            # rc 0 but nothing listed: branches exist but are unborn.
            item = QListWidgetItem(_mono("No commits", text_color))
            item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._list.addItem(item)

    def _head_exists(self) -> bool:
        try:
            return (
                self._run_git(
                    ["rev-parse", "--verify", "--quiet", "HEAD"],
                    timeout=5,
                    capture_output=True,
                ).returncode
                == 0
            )
        except (OSError, subprocess.TimeoutExpired):
            return False

    def _main_line_hashes(self) -> set[str] | None:
        """Full hashes reachable from master (or main), or None when neither
        exists to compare against."""
        for ref in ("master", "main"):
            try:
                result = self._run_git(
                    ["rev-list", ref, "--"],
                    timeout=5,
                    capture_output=True,
                    text=True,
                )
            except (OSError, subprocess.TimeoutExpired):
                return None
            if result.returncode == 0:
                return set(result.stdout.split())
        return None

    def _on_context_menu(self, pos) -> None:
        item = self._list.itemAt(pos)
        # Graph continuation rows ("|/") carry no commit to act on.
        commit = item.data(Qt.ItemDataRole.UserRole) if item is not None else None
        if not commit:
            return
        # The list may refresh while QMenu.exec runs its nested event loop.
        branches = item.data(Qt.ItemDataRole.UserRole + 1) or []
        full_hash = item.data(Qt.ItemDataRole.UserRole + 2)
        menu = QMenu(self._list)
        targets = {}
        for branch in branches:
            targets[menu.addAction(f"Checkout {branch}")] = branch
        targets[menu.addAction(f"Checkout {commit}")] = commit
        menu.addSeparator()
        copy_action = menu.addAction("Copy hash")
        chosen = menu.exec(self._list.viewport().mapToGlobal(pos))
        if chosen is copy_action:
            # The full hash: short ones go ambiguous as a repo grows.
            QGuiApplication.clipboard().setText(full_hash)
        elif chosen is not None:
            self._checkout(targets[chosen])

    def _checkout(self, commit: str) -> None:
        # Remote checkout is a blocking SSH round trip; run it off-thread.
        if self._remote is None:
            self._after_checkout(self._run_checkout(commit))
        else:
            run_async(lambda: self._run_checkout(commit), self._after_checkout, self)

    def _run_checkout(self, commit: str):
        """Run the checkout; return the CompletedProcess, or the exception if it
        couldn't run. Safe on a worker thread (no Qt)."""
        try:
            return self._run_git(
                ["checkout", commit],
                timeout=10,
                capture_output=True,
                text=True,
                errors="replace",
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return exc

    def _after_checkout(self, result) -> None:
        if isinstance(result, (OSError, subprocess.TimeoutExpired)):
            QMessageBox.warning(self, "Checkout failed", str(result))
            return
        if result is None or result.returncode != 0:
            QMessageBox.warning(
                self,
                "Checkout failed",
                (result.stderr.strip() if result else "") or "git checkout failed",
            )
            return
        # HEAD moved: redraw so the (HEAD -> …) decoration follows.
        self.refresh()

    def set_theme(self, name: str) -> None:
        changed = name != self._theme_name
        self._theme_name = name
        ui = UI_COLORS[name]
        self._card.set_colors(ui["card"], ui["card_border"])
        self._list.setStyleSheet(_GIT_LOG_STYLES[name])
        # Row colors are baked into each item's HTML, so re-render on a
        # theme flip (hidden panels re-render on their next showEvent).
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
