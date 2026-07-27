"""The diff pane lists the repo's uncommitted changes with per-file line counts.

The panel is driven against real `git` output in a real temporary repository,
because the whole feature is a parse of that output: a test with hand-written
fixture strings would only assert that the parser matches whatever this test
imagined git prints, which is the part most likely to be wrong.
"""

import subprocess

import pytest

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
