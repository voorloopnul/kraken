"""The diff pane lists the repo's uncommitted changes with per-file line counts.

The panel is driven against real `git` output in a real temporary repository,
because the whole feature is a parse of that output: a test with hand-written
fixture strings would only assert that the parser matches whatever this test
imagined git prints, which is the part most likely to be wrong.
"""

import subprocess

import pytest
from PySide6.QtCore import Qt

from kraken.shell.panels.diff import DiffPanel, parse_numstat, parse_status


def git(repo, *args):
    subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )


@pytest.fixture
def repo(tmp_path):
    """A repo with one commit: a file to modify, one to rename, one to delete."""
    path = tmp_path / "repo"
    path.mkdir()
    git(path, "init", "-q", "-b", "main")
    git(path, "config", "user.email", "test@example.com")
    git(path, "config", "user.name", "Test")
    (path / "keep.txt").write_text("a\nb\nc\n")
    (path / "move_me.txt").write_text("one\ntwo\nthree\n")
    (path / "delete_me.txt").write_text("gone\n")
    git(path, "add", "-A")
    git(path, "commit", "-qm", "init")
    return path


@pytest.fixture
def panel(qapp, settle):
    def _build(cwd):
        widget = DiffPanel(cwd=str(cwd))
        widget.resize(420, 500)
        widget.show()
        settle()
        return widget

    built = []
    yield lambda cwd: built.append(_build(cwd)) or built[-1]
    for widget in built:
        widget.close()
        widget.deleteLater()


def rows(widget):
    """The panel's rows as {path: (status letter, additions, deletions)}."""
    tree = widget._tree
    out = {}
    for i in range(tree.topLevelItemCount()):
        item = tree.topLevelItem(i)
        out[item.text(1)] = (item.text(0), item.text(2), item.text(3))
    return out


def test_clean_repo_reports_no_changes(repo, panel):
    widget = panel(repo)

    assert rows(widget) == {}
    assert "No changes" in widget._summary.text()


def test_counts_lines_added_and_removed_per_file(repo, panel):
    # Two lines added to a three-line file, and one of the originals dropped.
    (repo / "keep.txt").write_text("a\nc\nd\ne\n")

    widget = panel(repo)

    assert rows(widget)["keep.txt"] == ("M", "+2", "−1")


def test_staged_and_unstaged_edits_count_as_one_total(repo, panel):
    # The pane measures against HEAD, so a file edited on both sides of the
    # index is still one row carrying the whole change.
    (repo / "keep.txt").write_text("a\nb\nc\nstaged\n")
    git(repo, "add", "keep.txt")
    (repo / "keep.txt").write_text("a\nb\nc\nstaged\nunstaged\n")

    widget = panel(repo)

    assert rows(widget)["keep.txt"] == ("M", "+2", "−0")


def test_untracked_file_counts_its_lines_as_additions(repo, panel):
    (repo / "fresh.txt").write_text("one\ntwo\nthree\n")
    # A final line without a newline still counts, the way git counts it.
    (repo / "no_eol.txt").write_text("only")

    widget = panel(repo)
    listed = rows(widget)

    assert listed["fresh.txt"] == ("?", "+3", "−0")
    assert listed["no_eol.txt"] == ("?", "+1", "−0")


def test_deleted_and_renamed_files_are_listed(repo, panel):
    (repo / "delete_me.txt").unlink()
    git(repo, "mv", "move_me.txt", "moved.txt")

    widget = panel(repo)
    listed = rows(widget)

    assert listed["delete_me.txt"] == ("D", "+0", "−1")
    # The rename is one row under the new name, not an add plus a delete.
    assert "move_me.txt" not in listed
    assert listed["moved.txt"][0] == "R"


def test_binary_file_has_no_line_counts(repo, panel):
    # Both counts read as unknown: a zero for the removals would be a number
    # the panel made up, next to an addition count it admits it doesn't have.
    (repo / "untracked.bin").write_bytes(b"\x00\x01\x02binary\x00")
    (repo / "keep.txt").write_bytes(b"a\nb\n\x00tracked binary now\x00")

    widget = panel(repo)
    listed = rows(widget)

    assert listed["untracked.bin"] == ("?", "—", "—")
    assert listed["keep.txt"] == ("M", "—", "—")


def test_summary_totals_every_file(repo, panel):
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")  # +1
    (repo / "delete_me.txt").unlink()  # -1
    (repo / "fresh.txt").write_text("x\ny\n")  # +2

    widget = panel(repo)
    summary = widget._summary.text()

    assert "3 files changed" in summary
    assert "+3" in summary
    assert "−1" in summary


def test_refresh_picks_up_later_edits(repo, panel, settle):
    widget = panel(repo)
    assert rows(widget) == {}

    (repo / "keep.txt").write_text("a\nb\nc\nd\n")
    widget.refresh()
    settle()

    assert rows(widget)["keep.txt"] == ("M", "+1", "−0")


def test_unborn_head_still_reports_staged_and_untracked(tmp_path, panel):
    """A repo with no commit yet has no HEAD to diff against; the pane falls
    back to the index and worktree instead of showing nothing."""
    path = tmp_path / "empty"
    path.mkdir()
    git(path, "init", "-q", "-b", "main")
    (path / "staged.txt").write_text("a\nb\n")
    git(path, "add", "staged.txt")
    (path / "loose.txt").write_text("c\n")

    widget = panel(path)
    listed = rows(widget)

    assert listed["staged.txt"] == ("A", "+2", "−0")
    assert listed["loose.txt"] == ("?", "+1", "−0")


def test_outside_a_repository_says_so(tmp_path, panel):
    widget = panel(tmp_path)

    assert rows(widget) == {}
    assert "Not a git repository" in widget._summary.text()


def viewer_for(widget, row_index, settle):
    """Click a row the way the tree does, and return the sheet it opened."""
    item = widget._tree.topLevelItem(row_index)
    widget._tree.itemClicked.emit(item, 1)
    settle(300)  # let the fade finish
    return widget._viewer


def test_clicking_a_row_opens_the_file_diff(repo, panel, settle):
    (repo / "keep.txt").write_text("a\nb\nc\nadded here\n")

    widget = panel(repo)
    viewer = viewer_for(widget, 0, settle)

    assert viewer is not None
    body = viewer._body.document()
    lines = [body.findBlockByNumber(i).text() for i in range(body.blockCount())]
    assert "+added here" in lines
    # Context comes through as well, so the change is readable in place.
    assert " a" in lines


def test_the_sheet_is_not_built_inside_the_click(repo, panel, settle):
    """The click may take the request, but must not do the work.

    The slot runs nested inside QAbstractItemView::mouseReleaseEvent, which goes
    on using the view after the slot returns. Building the sheet there means
    lexing a whole file for its colors — enough allocation to run the collector
    under a widget Qt is still holding, and the click segfaults on the way back
    out. So nothing may exist until the event loop has turned.
    """
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")
    widget = panel(repo)

    widget._tree.itemClicked.emit(widget._tree.topLevelItem(0), 1)

    # Still on the click's stack: the request is taken, nothing is built.
    assert widget._viewer is None
    assert widget._opening is True

    settle(300)
    assert widget._viewer is not None
    assert widget._opening is False


def test_a_double_click_reads_the_diff_once(repo, panel, settle):
    """A double-click sends itemClicked and itemActivated back to back, both
    before the deferred open has run — so the open sheet cannot be what refuses
    the second one. Without the pending flag each request would read the diff
    (a git round trip apiece) only for one of the sheets to be thrown away."""
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")
    widget = panel(repo)

    reads = []
    original = widget._diff_document
    widget._diff_document = lambda *a, **k: (reads.append(1), original(*a, **k))[1]

    item = widget._tree.topLevelItem(0)
    widget._tree.itemClicked.emit(item, 1)
    widget._tree.itemActivated.emit(item, 1)
    settle(300)

    assert widget._viewer is not None
    assert len(reads) == 1


def test_a_second_click_does_not_stack_another_sheet(repo, panel, settle):
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")

    widget = panel(repo)
    first = viewer_for(widget, 0, settle)
    second = viewer_for(widget, 0, settle)

    assert first is second


def test_closing_the_sheet_lets_the_next_click_open_one(repo, panel, settle):
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")

    widget = panel(repo)
    first = viewer_for(widget, 0, settle)
    first.close_view()
    settle(300)
    assert widget._viewer is None

    assert viewer_for(widget, 0, settle) is not None


def test_an_untracked_file_opens_as_all_additions(repo, panel, settle):
    (repo / "fresh.py").write_text("import os\n\n\ndef main():\n    return os\n")

    widget = panel(repo)
    viewer = viewer_for(widget, 0, settle)

    body = viewer._body.document()
    lines = [body.findBlockByNumber(i).text() for i in range(body.blockCount())]
    assert "+import os" in lines
    assert "+def main():" in lines


def test_a_binary_file_says_so_instead_of_opening_a_diff(repo, panel, settle):
    (repo / "blob.bin").write_bytes(b"\x00\x01binary\x00")

    widget = panel(repo)
    viewer = viewer_for(widget, 0, settle)

    assert viewer is not None
    assert viewer._body is None  # a message, not a diff


def test_the_panels_theme_reaches_an_open_sheet(repo, panel, settle):
    """The app's theme toggle runs through every panel's set_theme; the sheet is
    a surface of its own, and nothing else reaches into it once it is up."""
    (repo / "keep.txt").write_text("a\nb\nc\nd\n")

    widget = panel(repo)
    viewer = viewer_for(widget, 0, settle)
    assert viewer._theme == "light"

    widget.set_theme("dark")
    settle()

    assert viewer._theme == "dark"


def test_a_workspace_on_a_subdirectory_still_finds_its_files(tmp_path, panel, settle):
    """git reports paths relative to the repo root, while a pathspec and a file
    read are relative to where git ran. A workspace opened on a subdirectory is
    where those two disagree, and both the counts and the diff depend on it."""
    root = tmp_path / "repo"
    (root / "pkg" / "deep").mkdir(parents=True)
    git(root, "init", "-q", "-b", "main")
    git(root, "config", "user.email", "test@example.com")
    git(root, "config", "user.name", "Test")
    (root / "pkg" / "tracked.txt").write_text("one\ntwo\n")
    git(root, "add", "-A")
    git(root, "commit", "-qm", "init")
    (root / "pkg" / "tracked.txt").write_text("one\ntwo\nthree\n")
    (root / "pkg" / "fresh.txt").write_text("new\nlines\n")

    # The workspace is two levels below the root.
    widget = panel(root / "pkg" / "deep")
    listed = rows(widget)

    assert listed["pkg/tracked.txt"] == ("M", "+1", "−0")
    # The untracked count is a filesystem read, which is the half that breaks
    # when the root is assumed to be the workspace.
    assert listed["pkg/fresh.txt"] == ("?", "+2", "−0")

    viewer = viewer_for(widget, 0, settle)
    body = viewer._body.document()
    lines = [body.findBlockByNumber(i).text() for i in range(body.blockCount())]
    assert "+three" in lines


def test_an_uncounted_text_file_is_not_called_binary(repo, panel, settle, monkeypatch):
    """A count can go missing for a file that simply wasn't counted — past the
    per-refresh cap, or too large to read. Such a file still has a diff, and
    reporting it as binary both mislabels the row and refuses to open it."""
    monkeypatch.setattr(DiffPanel, "_MAX_COUNTED_UNTRACKED", 1)
    (repo / "a_first.txt").write_text("only counted one\n")
    (repo / "b_beyond_cap.txt").write_text("still text\nand still openable\n")

    widget = panel(repo)
    listed = rows(widget)

    # The row past the cap has no count, and says so with a dash.
    assert listed["b_beyond_cap.txt"] == ("?", "—", "—")
    item = widget._tree.topLevelItem(1)
    change = item.data(1, Qt.ItemDataRole.UserRole + 1)
    assert change.path == "b_beyond_cap.txt"
    assert change.binary is False
    # And the tooltip says what actually happened rather than calling it binary.
    assert "not counted" in item.toolTip(1)
    assert "binary" not in item.toolTip(1)

    # And clicking it opens the real diff rather than a "binary" message.
    viewer = viewer_for(widget, 1, settle)
    assert viewer._body is not None, "an uncounted text file should still open"
    body = viewer._body.document()
    lines = [body.findBlockByNumber(i).text() for i in range(body.blockCount())]
    assert "+still text" in lines


def test_a_binary_file_is_still_marked_binary(repo, panel):
    (repo / "untracked.bin").write_bytes(b"\x00\x01binary\x00")
    (repo / "keep.txt").write_bytes(b"a\nb\n\x00tracked binary now\x00")

    widget = panel(repo)
    changes = {
        widget._tree.topLevelItem(i).data(1, Qt.ItemDataRole.UserRole + 1).path:
        widget._tree.topLevelItem(i).data(1, Qt.ItemDataRole.UserRole + 1).binary
        for i in range(widget._tree.topLevelItemCount())
    }

    # git's "-" for a tracked file, and the NUL byte for an untracked one.
    assert changes == {"keep.txt": True, "untracked.bin": True}


class _LocalShell:
    """Stands in for a remote target, running the command locally instead of
    over SSH so the panel's remote path can be exercised."""

    def ssh_argv(self, command: str) -> list[str]:
        return ["sh", "-c", command]


def test_remote_counts_survive_one_unreadable_file(tmp_path, qapp):
    """`wc -l` exits non-zero if any argument fails, while still printing counts
    for the rest. A file that vanished between the status call and this one is
    routine while an agent works, and must not blank out every other count."""
    (tmp_path / "here.txt").write_text("one\ntwo\n")
    widget = DiffPanel(cwd=str(tmp_path), remote=_LocalShell())

    counts = widget._remote_line_counts(["here.txt", "gone.txt"], str(tmp_path))

    assert counts["here.txt"] == (2, False)
    assert counts["gone.txt"] == (None, False)
    widget.deleteLater()


def test_parse_numstat_keeps_a_tab_inside_a_path():
    # The record is NUL-terminated, so only the first two tabs are separators.
    counts = parse_numstat("3\t1\tta\tb.txt\0")

    assert counts == {"ta\tb.txt": (3, 1)}


def test_parse_status_pairs_a_rename_with_its_origin():
    entries = parse_status("RM new.txt\0old.txt\0 M other.txt\0")

    assert entries == [("RM", "new.txt", "old.txt"), (" M", "other.txt", None)]


def test_parse_numstat_keys_a_rename_under_its_new_path():
    counts = parse_numstat("3\t1\tplain.txt\0" "5\t2\t\0old.txt\0new.txt\0" "-\t-\tblob.bin\0")

    assert counts == {
        "plain.txt": (3, 1),
        "new.txt": (5, 2),
        "blob.bin": (None, None),
    }
