//! The diff pane and the sheet it opens: what has changed since the last
//! commit, file by file, and one file's diff in full.
//!
//! The pane answers "what has changed since the last commit" — the question
//! you ask after an agent has been editing for a while. Each row is one file
//! with the lines added and removed in it, counted against HEAD, so a change
//! that is partly staged is still reported as one total per file rather than
//! split in two. Untracked files are listed too (an agent creates them
//! constantly); git's own diff never mentions them, so their additions are
//! counted here.
//!
//! Everything in this module is either a parse of git's output or a builder
//! for the command that produces it. The running goes through
//! [`crate::git::GitRunner`], which is what lets a local repository and a
//! remote one over SSH share this code.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::git::{self, escape_html, GitRunner, QUICK, SLOW};
use crate::util::shell_quote;

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

/// Row colours in the pane: `text`, `dim`, `add`, `del`.
pub fn panel_color(theme: &str, key: &str) -> &'static str {
    let dark = theme == "dark";
    match key {
        "text" => if dark { "#c8cad0" } else { "#4a4d55" },
        "dim" => if dark { "#7a7d85" } else { "#9a9da5" },
        "add" => if dark { "#98c379" } else { "#50a14f" },
        "del" => if dark { "#e06c75" } else { "#e45649" },
        _ => "#ff00ff",
    }
}

/// Row text and the tints behind changed lines in the sheet. The tints are the
/// card background nudged toward green/red, so they read as a wash over the
/// code rather than a block of colour competing with the syntax highlighting.
pub fn viewer_color(theme: &str, key: &str) -> &'static str {
    let dark = theme == "dark";
    match key {
        "text" => if dark { "#c8cad0" } else { "#383a42" },
        "dim" => if dark { "#7a7d85" } else { "#9a9da5" },
        "add" => if dark { "#98c379" } else { "#3c7d3b" },
        "del" => if dark { "#e06c75" } else { "#c93c36" },
        "add_bg" => if dark { "#2b3a2e" } else { "#e9f6e9" },
        "del_bg" => if dark { "#3a2b30" } else { "#fdeceb" },
        "hunk_bg" => if dark { "#2f333c" } else { "#f2f0ec" },
        "gutter" => if dark { "#6b6e77" } else { "#aeaba4" },
        _ => "#ff00ff",
    }
}

/// The scrim painted over the window behind the sheet, as RGBA. It is what
/// makes the sheet modal, so it is a colour with an alpha rather than a tint.
pub fn scrim(theme: &str) -> (u8, u8, u8, u8) {
    if theme == "dark" {
        (0, 0, 0, 165)
    } else {
        (24, 22, 18, 130)
    }
}

/// Accent per status letter, drawn from the same One Half palette as the
/// themes. The pane colours its rows from this and so does the sheet — one
/// table, so a file reads the same in the list and in what it opens.
pub fn letter_color(theme: &str, letter: char) -> &'static str {
    let dark = theme == "dark";
    match letter {
        'A' | '?' => if dark { "#98c379" } else { "#50a14f" },
        'M' => if dark { "#e5c07b" } else { "#c18401" },
        'D' | 'U' => if dark { "#e06c75" } else { "#e45649" },
        'R' | 'C' => if dark { "#61afef" } else { "#0184bc" },
        'T' => if dark { "#c678dd" } else { "#a626a4" },
        _ => panel_color(theme, "text"),
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// One record of `git status --porcelain`: the two-letter code, the path, and
/// where a renamed or copied file came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub xy: String,
    pub path: String,
    pub orig: Option<String>,
}

/// Parse `git status --porcelain=v1 -z` into entries.
///
/// Records are `XY PATH\0`; a rename or copy adds a second record naming where
/// the file came from, so those are consumed two at a time.
pub fn parse_status(raw: &str) -> Vec<StatusEntry> {
    let tokens: Vec<&str> = raw.split('\0').collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        index += 1;
        // "XY " plus at least one character of path; the trailing empty token
        // after the final NUL falls out here too.
        if token.len() < 4 {
            continue;
        }
        let xy = &token[..2];
        let path = &token[3..];
        let mut orig = None;
        if (xy.contains('R') || xy.contains('C'))
            && index < tokens.len() {
                orig = Some(tokens[index]).filter(|text| !text.is_empty()).map(str::to_string);
                index += 1;
            }
        entries.push(StatusEntry {
            xy: xy.to_string(),
            path: path.to_string(),
            orig,
        });
    }
    entries
}

/// One display letter for a two-letter porcelain code. The index side wins when
/// both sides changed — a staged rename modified afterwards reads better as "R"
/// than as "M" — except when the worktree says the file is gone, which is the
/// more useful thing to know.
pub fn status_letter(xy: &str) -> char {
    if xy == "??" {
        return '?';
    }
    if xy.contains('U') || xy == "AA" || xy == "DD" {
        return 'U'; // unmerged, whichever way the conflict is shaped
    }
    let mut chars = xy.chars();
    let index = chars.next().unwrap_or(' ');
    let work = chars.next().unwrap_or(' ');
    if work == 'D' {
        return 'D';
    }
    if index != ' ' {
        index
    } else {
        work
    }
}

/// Where the change sits relative to the index, in the words the row shows.
pub fn staged_note(xy: &str) -> &'static str {
    if xy == "??" {
        return "untracked";
    }
    let mut chars = xy.chars();
    let index = chars.next().unwrap_or(' ');
    let work = chars.next().unwrap_or(' ');
    if index != ' ' && work != ' ' {
        return "partly staged";
    }
    if index != ' ' {
        "staged"
    } else {
        "not staged"
    }
}

fn letter_name(letter: char) -> &'static str {
    match letter {
        'A' => "added",
        '?' => "untracked",
        'M' => "modified",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "type changed",
        'U' => "unmerged",
        _ => "changed",
    }
}

/// One changed file. `adds`/`dels` are `None` when the count is unavailable — a
/// binary file (git reports "-" for both) or a file that wasn't counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Raw two-letter porcelain code, e.g. `"A "`, `" M"`, `"??"`.
    pub xy: String,
    /// As git reports it: relative to the repo root.
    pub path: String,
    /// Where a renamed or copied file came from.
    pub orig: Option<String>,
    pub adds: Option<i64>,
    pub dels: Option<i64>,
    /// Whether the counts are missing because the file is not text. A count can
    /// also go missing for a file that simply wasn't counted — past the
    /// per-refresh cap, or too large to read — and that file still has a diff
    /// worth opening.
    pub binary: bool,
}

impl FileChange {
    pub fn letter(&self) -> char {
        status_letter(&self.xy)
    }
}

/// What happened to the file, in words: "modified · not staged".
pub fn describe(change: &FileChange) -> String {
    let name = letter_name(change.letter());
    let note = staged_note(&change.xy);
    if note == name {
        name.to_string()
    } else {
        format!("{name} · {note}")
    }
}

pub fn tooltip(change: &FileChange) -> String {
    let mut lines = vec![change.path.clone(), describe(change)];
    if let Some(orig) = &change.orig {
        lines.push(format!("from {orig}"));
    }
    if change.binary {
        lines.push("binary — no line count".to_string());
    } else if change.adds.is_none() || change.dels.is_none() {
        // Not counted rather than not countable: too large to read, or past the
        // cap on how many files one refresh counts. Its diff still opens.
        lines.push("lines not counted".to_string());
    } else {
        lines.push(format!(
            "+{}  −{}",
            change.adds.unwrap_or(0),
            change.dels.unwrap_or(0)
        ));
    }
    lines.push("Click to view the diff".to_string());
    lines.join("\n")
}

/// A count as the row shows it: an em dash where there is no number, rather
/// than a zero the pane would be inventing.
pub fn count_text(count: Option<i64>, sign: &str) -> String {
    match count {
        None => "—".to_string(),
        Some(value) => format!("{sign}{value}"),
    }
}

/// The pane's one-line total, as rich text. Also the only place the empty and
/// error states are shown — no placeholder row is faked into the list.
pub fn summary_html(theme: &str, files: &[FileChange], omitted: usize) -> String {
    let adds: i64 = files.iter().map(|c| c.adds.unwrap_or(0)).sum();
    let dels: i64 = files.iter().map(|c| c.dels.unwrap_or(0)).sum();
    let count = files.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut parts = vec![
        format!(
            "<span style=\"color: {};\">{count} file{plural} changed</span>",
            panel_color(theme, "text")
        ),
        format!(
            "<span style=\"color: {};\">+{adds}</span>",
            panel_color(theme, "add")
        ),
        format!(
            "<span style=\"color: {};\">−{dels}</span>",
            panel_color(theme, "del")
        ),
    ];
    if omitted > 0 {
        parts.push(format!(
            "<span style=\"color: {};\">(+{omitted} more)</span>",
            panel_color(theme, "dim")
        ));
    }
    parts.join("&nbsp;&nbsp;")
}

/// The pane's message line: the empty state, and every failure.
pub fn message_html(theme: &str, message: &str) -> String {
    format!(
        "<span style=\"color: {};\">{}</span>",
        panel_color(theme, "dim"),
        escape_html(message)
    )
}

// ---------------------------------------------------------------------------
// Line counts
// ---------------------------------------------------------------------------

/// Parse `git diff --numstat -z` into `{path: (additions, deletions)}`.
///
/// Records are `<adds>\t<dels>\t<path>\0`. A rename leaves the path field empty
/// and follows with two records of its own — the old path, then the new one,
/// which is the path the file is keyed under (matching what `git status`
/// reports for it). Binary files carry "-" for both counts, which comes back as
/// `None`.
pub fn parse_numstat(raw: &str) -> BTreeMap<String, (Option<i64>, Option<i64>)> {
    let tokens: Vec<&str> = raw.split('\0').filter(|token| !token.is_empty()).collect();
    let mut counts = BTreeMap::new();
    let mut index = 0;
    while index < tokens.len() {
        let fields: Vec<&str> = tokens[index].split('\t').collect();
        index += 1;
        if fields.len() < 3 {
            continue;
        }
        // Only the first two tabs are separators: the record is NUL-terminated,
        // so anything after them is the path — tabs in the filename included.
        let (adds, dels) = (fields[0], fields[1]);
        let mut path = fields[2..].join("\t");
        if path.is_empty() {
            // A rename: the two following tokens are the old path, then the new.
            if index + 1 >= tokens.len() {
                continue;
            }
            path = tokens[index + 1].to_string();
            index += 2;
        }
        counts.insert(path, (as_count(adds), as_count(dels)));
    }
    counts
}

/// A numstat field as a number, or `None` for the "-" git writes for a binary
/// file, which has no line counts at all.
fn as_count(field: &str) -> Option<i64> {
    field.parse().ok()
}

/// Sum two counts, where an unknown one (a binary file) poisons the total.
pub fn add(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    Some(left? + right?)
}

/// `(lines, binary)` for a file. `lines` is `None` when it can't be counted —
/// unreadable, not text, or larger than `max_bytes` — and `binary` separates
/// the one of those that means "there is no text diff to show" from the two
/// that only mean "we didn't count". Read in chunks so a big file isn't held in
/// memory whole, and counted git's way: a final line without a newline counts.
pub fn count_lines(path: &Path, max_bytes: u64) -> (Option<i64>, bool) {
    let Ok(mut handle) = File::open(path) else {
        return (None, false);
    };
    let mut buffer = vec![0u8; 64 * 1024];
    let mut lines = 0i64;
    let mut read: u64 = 0;
    let mut tail = 0u8;
    let mut any = false;
    loop {
        let count = match handle.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => return (None, false),
        };
        let chunk = &buffer[..count];
        read += count as u64;
        if read > max_bytes {
            return (None, false);
        }
        if chunk.contains(&0) {
            return (None, true); // binary, like git's own numstat
        }
        lines += chunk.iter().filter(|byte| **byte == b'\n').count() as i64;
        tail = chunk[count - 1];
        any = true;
    }
    if any && tail != b'\n' {
        lines += 1;
    }
    (Some(lines), false)
}

/// `wc -l` output back into per-path counts.
///
/// `absolute` maps the absolute name each path was asked about to the path git
/// reported. `wc` appends a "total" line when it is given several files, which
/// is no path of ours and falls out of the lookup.
pub fn parse_wc_output(
    output: &str,
    absolute: &BTreeMap<String, String>,
) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for line in output.lines() {
        let line = line.trim();
        let Some((number, name)) = line.split_once(' ') else {
            continue;
        };
        let Some(path) = absolute.get(name.trim()) else {
            continue;
        };
        if let Ok(value) = number.parse::<i64>() {
            counts.insert(path.clone(), value);
        }
    }
    counts
}

// ---------------------------------------------------------------------------
// The unified diff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Add,
    Del,
    Hunk,
    /// A line that is neither code nor a hunk header: git's sentence about a
    /// binary file, "\ No newline at end of file", or the truncation notice.
    Note,
}

impl RowKind {
    /// The name the view styles a row by.
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Context => "context",
            RowKind::Add => "add",
            RowKind::Del => "del",
            RowKind::Hunk => "hunk",
            RowKind::Note => "note",
        }
    }
}

/// One line of the rendered diff. `old_no`/`new_no` are the line's number on
/// each side, absent where it does not exist on that side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    /// The diff line, marker character included.
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

impl DiffRow {
    fn new(kind: RowKind, text: &str, old_no: Option<u32>, new_no: Option<u32>) -> Self {
        Self {
            kind,
            text: text.to_string(),
            old_no,
            new_no,
        }
    }
}

/// Which side of the file a row is coloured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

/// The side and zero-based line index a row takes its syntax colours from — a
/// removed line only exists in the old file, everything else is read from the
/// new one.
///
/// This is the seam for highlighting: `chat::highlight` lexes each side of the
/// file as one document and produces per-line spans, and the view indexes that
/// table with this. Lexing the file whole is what keeps a hunk that starts
/// inside a docstring from being coloured as if it were code — a lexer handed
/// one line at a time has no idea it is inside a string. A row whose index
/// falls outside the table simply renders uncoloured: that means the file moved
/// under us between two git calls.
pub fn span_source(row: &DiffRow) -> Option<(Side, usize)> {
    let (side, number) = match row.kind {
        RowKind::Del => (Side::Old, row.old_no),
        _ => (Side::New, row.new_no),
    };
    match number {
        Some(number) if number > 0 => Some((side, number as usize - 1)),
        _ => None,
    }
}

/// How many rows one sheet will lay out before it stops.
pub const MAX_ROWS: usize = 4000;

/// `@@ -3,6 +3,7 @@ import os` → the first line number on each side.
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = take_range(rest)?;
    let (new, rest) = take_range(rest.strip_prefix(" +")?)?;
    // The header's closing "@@" is what tells it apart from a removed line that
    // happens to start with "@@ -".
    rest.strip_prefix(" @@")?;
    Some((old, new))
}

/// A `<start>` or `<start>,<count>` field, and whatever follows it.
fn take_range(text: &str) -> Option<(u32, &str)> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let start: u32 = text[..digits].parse().ok()?;
    let rest = &text[digits..];
    match rest.strip_prefix(',') {
        Some(after) => {
            let more = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if more == 0 {
                return None;
            }
            Some((start, &after[more..]))
        }
        None => Some((start, rest)),
    }
}

/// Rows from a unified diff. The file header is dropped, each `@@` header is
/// kept as its own row, and every body line carries the line numbers it holds
/// on each side — which is what lets a row be coloured from its own file.
pub fn parse_diff(diff_text: &str, max_rows: usize) -> Vec<DiffRow> {
    let mut rows: Vec<DiffRow> = Vec::new();
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut in_body = false;
    for line in diff_text.lines() {
        if rows.len() >= max_rows {
            rows.push(DiffRow::new(RowKind::Note, "… diff truncated", None, None));
            break;
        }
        if let Some((old, new)) = parse_hunk_header(line) {
            old_no = old;
            new_no = new;
            rows.push(DiffRow::new(RowKind::Hunk, line, None, None));
            in_body = true;
            continue;
        }
        if !in_body {
            // git reports a binary change instead of a hunk; that line is the
            // only thing it will say about the file, so it is worth keeping.
            if line.starts_with("Binary files ") || line.starts_with("Binary file ") {
                rows.push(DiffRow::new(RowKind::Note, line, None, None));
            }
            continue;
        }
        if line.starts_with('+') {
            rows.push(DiffRow::new(RowKind::Add, line, None, Some(new_no)));
            new_no += 1;
        } else if line.starts_with('-') {
            rows.push(DiffRow::new(RowKind::Del, line, Some(old_no), None));
            old_no += 1;
        } else if line.starts_with('\\') {
            // "\ No newline at end of file"
            rows.push(DiffRow::new(RowKind::Note, line, None, None));
        } else {
            // Context. A body line is " text"; a stripped-down diff can leave a
            // blank line for a blank context line, which lands here too.
            rows.push(DiffRow::new(
                RowKind::Context,
                line,
                Some(old_no),
                Some(new_no),
            ));
            old_no += 1;
            new_no += 1;
        }
    }
    rows
}

/// Everything the sheet needs about one file. Plain data with no view in it, so
/// a remote workspace can gather it on a worker thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffDocument {
    pub path: String,
    pub letter: char,
    pub diff_text: String,
    /// The whole file on each side, used only to lex it; either is empty when
    /// the file does not exist there (added, or deleted).
    pub old_text: String,
    pub new_text: String,
    pub subtitle: String,
    /// Set when there is no text diff to show: a binary file, or a failure.
    pub message: String,
}

/// What the sheet puts up: rows, or one sentence instead of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBody {
    pub rows: Vec<DiffRow>,
    pub message: Option<String>,
}

impl DiffDocument {
    pub fn body(&self) -> DiffBody {
        let mut rows = parse_diff(&self.diff_text, MAX_ROWS);
        let mut message = self.message.clone();
        // A diff of nothing but notes ("Binary files … differ") has no lines to
        // lay out, and git's own sentence about it is the whole story — so it is
        // shown as the message rather than as a one-row body.
        let notes = rows.iter().filter(|row| row.kind == RowKind::Note).count();
        if !rows.is_empty() && notes == rows.len() {
            message = rows
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            rows.clear();
        }
        if !message.is_empty() || rows.is_empty() {
            return DiffBody {
                rows: Vec::new(),
                message: Some(if message.is_empty() {
                    "No textual changes".to_string()
                } else {
                    message
                }),
            };
        }
        DiffBody {
            rows,
            message: None,
        }
    }
}

/// How many digits wide the line-number gutter has to be.
///
/// Taken over both sides together: a large insertion pushes the new side's
/// numbers well past the old side's, and the gutter has to fit whichever ends
/// up longer.
pub fn gutter_digits(rows: &[DiffRow]) -> usize {
    let widest = rows
        .iter()
        .flat_map(|row| [row.old_no, row.new_no])
        .flatten()
        .max()
        .unwrap_or(0);
    widest.to_string().len().max(2)
}

// ---------------------------------------------------------------------------
// Gathering
// ---------------------------------------------------------------------------

/// Ceilings on the work one refresh will do. A repo with an unignored build
/// directory can have thousands of untracked files; the pane stays responsive
/// and says how many rows it left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLimits {
    pub max_files: usize,
    pub max_counted_untracked: usize,
    /// Untracked files are read to count their lines; stop at this much and
    /// report no count rather than slurping a huge blob into memory.
    pub max_read_bytes: u64,
    /// A file this size is fetched whole to syntax-highlight the sheet's diff.
    /// Past it the diff still opens, uncoloured: lexing megabytes would cost
    /// more of a pause than the colours are worth, and no source file is this
    /// big.
    pub max_highlight_bytes: u64,
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_files: 500,
            max_counted_untracked: 200,
            max_read_bytes: 4 * 1024 * 1024,
            max_highlight_bytes: 512 * 1024,
        }
    }
}

pub fn status_argv() -> Vec<String> {
    git::args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])
}

pub fn numstat_head_argv() -> Vec<String> {
    git::args(&["diff", "--numstat", "-z", "HEAD"])
}

pub fn numstat_cached_argv() -> Vec<String> {
    git::args(&["diff", "--numstat", "-z", "--cached"])
}

pub fn numstat_worktree_argv() -> Vec<String> {
    git::args(&["diff", "--numstat", "-z"])
}

pub fn repo_root_argv() -> Vec<String> {
    git::args(&["rev-parse", "--show-toplevel"])
}

/// The unified diff for one file, against HEAD. `:(top)` anchors the pathspec
/// to the repo root, which is what git reported the path relative to; a bare
/// path would be read relative to git's cwd and quietly match nothing when the
/// workspace is a subdirectory.
pub fn file_diff_argv(change: &FileChange) -> Vec<String> {
    let mut argv = git::args(&["diff", "HEAD", "--"]);
    argv.push(format!(":(top){}", change.path));
    if let Some(orig) = &change.orig {
        // Without the old path in the pathspec, a rename reads as an add.
        argv.push(format!(":(top){orig}"));
    }
    argv
}

/// A file with no counterpart in the repo, diffed against nothing so it shows
/// as all additions.
pub fn new_file_diff_argv(root: &str, path: &str) -> Vec<String> {
    let mut argv = git::args(&["diff", "--no-index", "--", "/dev/null"]);
    argv.push(format!("{root}/{path}"));
    argv
}

/// The file as HEAD has it. `HEAD:<path>` is resolved from the repo root unless
/// it starts with "./", so the porcelain path works as it stands.
pub fn head_text_argv(path: &str) -> Vec<String> {
    vec!["show".to_string(), format!("HEAD:{path}")]
}

/// Why `git status` failed, in the words the pane shows.
pub fn status_failure_message(stderr: &str) -> String {
    if stderr.to_lowercase().contains("not a git repository") {
        "Not a git repository".to_string()
    } else {
        "Could not read the working tree".to_string()
    }
}

/// What the pane has to render: rows, or the one sentence that explains why
/// there are none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffData {
    Files {
        files: Vec<FileChange>,
        /// How many changed files were left out of the list.
        omitted: usize,
        /// The repo's top level, which the paths above are relative to.
        root: String,
    },
    Message(String),
}

/// The repo's top level. Porcelain paths are relative to it, while a pathspec
/// and a filesystem read are relative to where git ran — so a workspace opened
/// on a subdirectory needs this to bridge the two.
pub fn repo_root(runner: &GitRunner) -> String {
    match runner.text(&repo_root_argv(), QUICK) {
        Some(stdout) => stdout.trim().to_string(),
        None => runner.cwd.clone(),
    }
}

/// Per-file line counts for every tracked change, staged or not, against HEAD.
/// A repo whose HEAD is unborn has no such revision to diff against, so its
/// index and worktree diffs are summed instead.
pub fn numstat(runner: &GitRunner) -> BTreeMap<String, (Option<i64>, Option<i64>)> {
    if let Some(stdout) = runner.text(&numstat_head_argv(), QUICK) {
        return parse_numstat(&stdout);
    }
    let mut counts: BTreeMap<String, (Option<i64>, Option<i64>)> = BTreeMap::new();
    for argv in [numstat_cached_argv(), numstat_worktree_argv()] {
        let Some(stdout) = runner.text(&argv, QUICK) else {
            continue;
        };
        for (path, (adds, dels)) in parse_numstat(&stdout) {
            match counts.get(&path).copied() {
                None => {
                    counts.insert(path, (adds, dels));
                }
                Some(have) => {
                    counts.insert(path, (add(have.0, adds), add(have.1, dels)));
                }
            }
        }
    }
    counts
}

/// `(lines, binary)` for each untracked file — its additions, since every line
/// is new. git's diff doesn't cover untracked files, so this counts them
/// itself.
pub fn untracked_counts(
    runner: &GitRunner,
    paths: &[String],
    root: &str,
    limits: &DiffLimits,
) -> BTreeMap<String, (Option<i64>, bool)> {
    if paths.is_empty() {
        return BTreeMap::new();
    }
    if runner.is_remote() {
        return remote_line_counts(runner, paths, root);
    }
    let base = PathBuf::from(root);
    paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                count_lines(&base.join(path), limits.max_read_bytes),
            )
        })
        .collect()
}

/// The same counts for a remote workspace, in one round trip: `wc -l` over the
/// whole list. It counts newlines, so a file that doesn't end in one comes out
/// a line short of what git would say — a better answer than no answer. A host
/// without `wc` leaves every count unknown, and nothing here can tell a binary
/// file from a text one.
///
/// `wc` exits non-zero if *any* path failed while still printing counts for the
/// rest, so the output is read whatever the exit status: one file deleted
/// between the status call and this one must not blank out every count.
pub fn remote_line_counts(
    runner: &GitRunner,
    paths: &[String],
    root: &str,
) -> BTreeMap<String, (Option<i64>, bool)> {
    let absolute: BTreeMap<String, String> = paths
        .iter()
        .map(|path| (format!("{root}/{path}"), path.clone()))
        .collect();
    let command = format!(
        "wc -l -- {}",
        absolute
            .keys()
            .map(|name| shell_quote(name))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let output = runner.remote_shell(&command, SLOW, true);
    let counts = parse_wc_output(&output, &absolute);
    paths
        .iter()
        .map(|path| (path.clone(), (counts.get(path).copied(), false)))
        .collect()
}

/// Run the git commands and return plain data for the view. No UI here — this
/// runs on a worker thread for remote workspaces.
pub fn gather(runner: &GitRunner, limits: &DiffLimits) -> DiffData {
    let Some(status) = runner.run(&status_argv(), QUICK) else {
        return DiffData::Message("Could not run git".to_string());
    };
    if !status.ok() {
        return DiffData::Message(status_failure_message(&status.stderr));
    }
    let entries = parse_status(&status.stdout);
    let shown = &entries[..entries.len().min(limits.max_files)];
    let counts = numstat(runner);
    let root = repo_root(runner);
    // Only the rows that will be listed are worth counting lines for.
    let untracked: Vec<String> = shown
        .iter()
        .filter(|entry| entry.xy == "??")
        .map(|entry| entry.path.clone())
        .take(limits.max_counted_untracked)
        .collect();
    let added = untracked_counts(runner, &untracked, &root, limits);

    let mut files = Vec::with_capacity(shown.len());
    for entry in shown {
        let (adds, dels, binary) = if entry.xy == "??" {
            // Every line of a new file is an addition and none are removals —
            // unless the additions couldn't be counted at all, in which case
            // claiming zero removals would be inventing a number.
            let (adds, binary) = added.get(&entry.path).copied().unwrap_or((None, false));
            let dels = if adds.is_some() { Some(0) } else { None };
            (adds, dels, binary)
        } else {
            let listed = counts.get(&entry.path).copied();
            let (adds, dels) = listed.unwrap_or((Some(0), Some(0)));
            // git reporting "-" for both counts is what makes it binary; a path
            // missing from numstat entirely is unchanged, not binary.
            let binary = listed == Some((None, None));
            (adds, dels, binary)
        };
        files.push(FileChange {
            xy: entry.xy.clone(),
            path: entry.path.clone(),
            orig: entry.orig.clone(),
            adds,
            dels,
            binary,
        });
    }
    DiffData::Files {
        files,
        omitted: entries.len().saturating_sub(limits.max_files),
        root,
    }
}

// ---------------------------------------------------------------------------
// One file's diff
// ---------------------------------------------------------------------------

fn file_diff(runner: &GitRunner, change: &FileChange, root: &str) -> Option<String> {
    if change.xy == "??" {
        return new_file_diff(runner, root, &change.path);
    }
    if let Some(stdout) = runner.text(&file_diff_argv(change), SLOW) {
        return Some(stdout);
    }
    // No HEAD to diff against: an unborn branch, where every line is new.
    new_file_diff(runner, root, &change.path)
}

/// `--no-index` exits 1 when its two inputs differ, which for a file with any
/// content at all is the normal outcome.
fn new_file_diff(runner: &GitRunner, root: &str, path: &str) -> Option<String> {
    let output = runner.run(&new_file_diff_argv(root, path), SLOW)?;
    match output.code {
        Some(0) | Some(1) => Some(output.stdout),
        _ => None,
    }
}

fn head_text(runner: &GitRunner, path: &str) -> String {
    runner.text(&head_text_argv(path), SLOW).unwrap_or_default()
}

/// The file as it is on disk now. Returns "" when it is too big to be worth
/// lexing, or unreadable — the diff still opens, just uncoloured.
fn worktree_text(runner: &GitRunner, path: &str, root: &str, limits: &DiffLimits) -> String {
    if runner.is_remote() {
        // head -c caps the transfer; a file that big only loses the colours on
        // the lines past the cap.
        let command = format!(
            "head -c {} -- {}",
            limits.max_highlight_bytes,
            shell_quote(&format!("{root}/{path}"))
        );
        return runner.remote_shell(&command, SLOW, false);
    }
    let target = Path::new(root).join(path);
    match std::fs::metadata(&target) {
        Ok(meta) if meta.len() > limits.max_highlight_bytes => String::new(),
        Ok(_) => match std::fs::read(&target) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

/// One file's diff plus both sides of the file whole, which is what lets the
/// view colour a line with the context above it in hand. No UI here — this runs
/// on a worker thread for remote workspaces.
pub fn diff_document(
    runner: &GitRunner,
    change: &FileChange,
    root: &str,
    limits: &DiffLimits,
) -> DiffDocument {
    let mut subtitle = describe(change);
    if let (Some(adds), Some(dels)) = (change.adds, change.dels) {
        subtitle = format!("{subtitle} · +{adds} −{dels}");
    }
    let letter = change.letter();
    if change.binary {
        // Only a file git (or the untracked-file read) called binary skips the
        // diff. A count that merely went missing — past the refresh cap, or too
        // big to read — still has a diff worth showing.
        return DiffDocument {
            path: change.path.clone(),
            letter,
            subtitle,
            message: "Binary file — no text diff".to_string(),
            ..DiffDocument::default()
        };
    }
    let Some(diff_text) = file_diff(runner, change, root) else {
        return DiffDocument {
            path: change.path.clone(),
            letter,
            subtitle,
            message: "Could not read the diff".to_string(),
            ..DiffDocument::default()
        };
    };
    DiffDocument {
        path: change.path.clone(),
        letter,
        diff_text,
        // A file that is new on this side has nothing to read on the other.
        old_text: if letter == 'A' || letter == '?' {
            String::new()
        } else {
            head_text(runner, change.orig.as_deref().unwrap_or(&change.path))
        },
        new_text: if letter == 'D' {
            String::new()
        } else {
            worktree_text(runner, &change.path, root, limits)
        },
        subtitle,
        message: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::TestRepo;

    fn change(xy: &str, path: &str) -> FileChange {
        FileChange {
            xy: xy.to_string(),
            path: path.to_string(),
            orig: None,
            adds: Some(3),
            dels: Some(1),
            binary: false,
        }
    }

    // ---- Status -----------------------------------------------------------

    #[test]
    fn parse_status_pairs_a_rename_with_its_origin() {
        let entries = parse_status("RM new.txt\0old.txt\0 M other.txt\0");
        assert_eq!(
            entries,
            vec![
                StatusEntry {
                    xy: "RM".into(),
                    path: "new.txt".into(),
                    orig: Some("old.txt".into())
                },
                StatusEntry {
                    xy: " M".into(),
                    path: "other.txt".into(),
                    orig: None
                },
            ]
        );
    }

    #[test]
    fn parse_status_skips_the_empty_token_after_the_last_nul() {
        let entries = parse_status("?? a.txt\0?? b.txt\0");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].path, "b.txt");
        assert!(parse_status("").is_empty());
    }

    #[test]
    fn the_display_letter_prefers_the_index_side_unless_the_file_is_gone() {
        assert_eq!(status_letter("??"), '?');
        assert_eq!(status_letter(" M"), 'M');
        assert_eq!(status_letter("M "), 'M');
        assert_eq!(status_letter("A "), 'A');
        // A staged rename modified afterwards reads better as a rename.
        assert_eq!(status_letter("RM"), 'R');
        // Except when the worktree says it is gone, which is the more useful
        // thing to know.
        assert_eq!(status_letter("MD"), 'D');
        assert_eq!(status_letter(" D"), 'D');
        // Unmerged, whichever way the conflict is shaped.
        assert_eq!(status_letter("UU"), 'U');
        assert_eq!(status_letter("AU"), 'U');
        assert_eq!(status_letter("AA"), 'U');
        assert_eq!(status_letter("DD"), 'U');
    }

    #[test]
    fn the_staged_note_says_which_side_of_the_index_the_change_is_on() {
        assert_eq!(staged_note("??"), "untracked");
        assert_eq!(staged_note("M "), "staged");
        assert_eq!(staged_note(" M"), "not staged");
        assert_eq!(staged_note("MM"), "partly staged");
    }

    #[test]
    fn a_change_describes_itself_in_words() {
        assert_eq!(describe(&change(" M", "a.txt")), "modified · not staged");
        assert_eq!(describe(&change("A ", "a.txt")), "added · staged");
        assert_eq!(describe(&change("MM", "a.txt")), "modified · partly staged");
        assert_eq!(describe(&change(" D", "a.txt")), "deleted · not staged");
        // "untracked · untracked" would be saying it twice.
        assert_eq!(describe(&change("??", "a.txt")), "untracked");
    }

    #[test]
    fn the_tooltip_says_what_happened_and_what_it_cost() {
        assert_eq!(
            tooltip(&change(" M", "src/app.rs")),
            "src/app.rs\nmodified · not staged\n+3  −1\nClick to view the diff"
        );
    }

    #[test]
    fn a_renamed_file_names_where_it_came_from() {
        let mut renamed = change("R ", "new.txt");
        renamed.orig = Some("old.txt".into());
        assert_eq!(
            tooltip(&renamed),
            "new.txt\nrenamed · staged\nfrom old.txt\n+3  −1\nClick to view the diff"
        );
    }

    #[test]
    fn an_uncounted_text_file_is_not_called_binary() {
        let mut uncounted = change("??", "big.txt");
        uncounted.adds = None;
        uncounted.dels = None;
        let text = tooltip(&uncounted);
        assert!(text.contains("lines not counted"), "{text}");
        assert!(!text.contains("binary"), "{text}");

        let mut binary = uncounted.clone();
        binary.binary = true;
        assert!(tooltip(&binary).contains("binary — no line count"));
    }

    #[test]
    fn a_missing_count_shows_as_a_dash_rather_than_a_made_up_zero() {
        assert_eq!(count_text(Some(3), "+"), "+3");
        assert_eq!(count_text(Some(0), "−"), "−0");
        assert_eq!(count_text(None, "+"), "—");
    }

    #[test]
    fn the_summary_totals_every_file_and_says_what_it_left_out() {
        let mut files = vec![change(" M", "a.txt"), change("??", "b.txt")];
        files[1].adds = Some(2);
        files[1].dels = Some(0);
        assert_eq!(
            summary_html("dark", &files, 0),
            "<span style=\"color: #c8cad0;\">2 files changed</span>&nbsp;&nbsp;\
             <span style=\"color: #98c379;\">+5</span>&nbsp;&nbsp;\
             <span style=\"color: #e06c75;\">−1</span>"
        );
        assert_eq!(
            summary_html("light", &files[..1], 7),
            "<span style=\"color: #4a4d55;\">1 file changed</span>&nbsp;&nbsp;\
             <span style=\"color: #50a14f;\">+3</span>&nbsp;&nbsp;\
             <span style=\"color: #e45649;\">−1</span>&nbsp;&nbsp;\
             <span style=\"color: #9a9da5;\">(+7 more)</span>"
        );
    }

    #[test]
    fn an_unknown_count_never_drags_the_total_below_what_is_known() {
        let mut files = vec![change(" M", "a.txt")];
        files[0].adds = None;
        files[0].dels = None;
        assert!(summary_html("dark", &files, 0).contains(">+0<"));
    }

    #[test]
    fn a_files_letter_carries_its_own_accent() {
        assert_eq!(letter_color("dark", 'M'), "#e5c07b");
        assert_eq!(letter_color("light", 'D'), "#e45649");
        assert_eq!(letter_color("dark", '?'), letter_color("dark", 'A'));
        // An unknown letter falls back to the plain row colour.
        assert_eq!(letter_color("light", 'X'), panel_color("light", "text"));
    }

    // ---- numstat ----------------------------------------------------------

    #[test]
    fn parse_numstat_keeps_a_tab_inside_a_path() {
        // The record is NUL-terminated, so only the first two tabs separate.
        let counts = parse_numstat("3\t1\tta\tb.txt\0");
        assert_eq!(counts.get("ta\tb.txt"), Some(&(Some(3), Some(1))));
        assert_eq!(counts.len(), 1);
    }

    #[test]
    fn parse_numstat_keys_a_rename_under_its_new_path() {
        let counts =
            parse_numstat("3\t1\tplain.txt\05\t2\t\0old.txt\0new.txt\0-\t-\tblob.bin\0");
        assert_eq!(counts.len(), 3);
        assert_eq!(counts["plain.txt"], (Some(3), Some(1)));
        assert_eq!(counts["new.txt"], (Some(5), Some(2)));
        // "-" for both is git saying the file is binary.
        assert_eq!(counts["blob.bin"], (None, None));
        assert!(!counts.contains_key("old.txt"));
    }

    #[test]
    fn an_unknown_count_poisons_a_sum() {
        assert_eq!(add(Some(2), Some(3)), Some(5));
        assert_eq!(add(None, Some(3)), None);
        assert_eq!(add(Some(2), None), None);
    }

    // ---- Counting lines ---------------------------------------------------

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pupo-diff-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn counting_lines_follows_gits_rule_for_a_missing_final_newline() {
        let dir = scratch("count");
        std::fs::write(dir.join("three.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("no_eol.txt"), "only").unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        assert_eq!(count_lines(&dir.join("three.txt"), 1 << 20), (Some(3), false));
        assert_eq!(count_lines(&dir.join("no_eol.txt"), 1 << 20), (Some(1), false));
        assert_eq!(count_lines(&dir.join("empty.txt"), 1 << 20), (Some(0), false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nul_byte_makes_a_file_binary_but_a_big_one_is_only_uncounted() {
        let dir = scratch("binary");
        std::fs::write(dir.join("blob.bin"), b"\x00\x01binary\x00").unwrap();
        std::fs::write(dir.join("big.txt"), "x\n".repeat(100)).unwrap();
        assert_eq!(count_lines(&dir.join("blob.bin"), 1 << 20), (None, true));
        // Past the cap: no count, but not binary — its diff still opens.
        assert_eq!(count_lines(&dir.join("big.txt"), 8), (None, false));
        // And an unreadable file is neither counted nor called binary.
        assert_eq!(count_lines(&dir.join("gone.txt"), 1 << 20), (None, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_counts_survive_one_unreadable_file() {
        // `wc -l` exits non-zero if any argument fails, while still printing
        // counts for the rest. A file that vanished between the status call and
        // this one is routine while an agent works, and must not blank out
        // every other count.
        let absolute: BTreeMap<String, String> = [
            ("/repo/here.txt".to_string(), "here.txt".to_string()),
            ("/repo/gone.txt".to_string(), "gone.txt".to_string()),
        ]
        .into_iter()
        .collect();
        let counts = parse_wc_output(
            "       2 /repo/here.txt\n       2 total\n",
            &absolute,
        );
        assert_eq!(counts.get("here.txt"), Some(&2));
        // The "total" line is no path of ours, and the missing file has no count.
        assert_eq!(counts.get("gone.txt"), None);
        assert_eq!(counts.len(), 1);
    }

    // ---- parse_diff -------------------------------------------------------

    /// Built line by line so the marker column survives: a blank context line is
    /// a single space in a real diff, which any whitespace-trimming tool would
    /// eat out of a multi-line literal.
    fn diff_fixture() -> String {
        [
            "diff --git a/app.py b/app.py",
            "index 1234567..89abcde 100644",
            "--- a/app.py",
            "+++ b/app.py",
            "@@ -3,6 +3,7 @@ import os",
            " def start(name):",
            "     value = 1",
            "-    return value",
            "+    value += 1",
            "+    return value * 2",
            " ",
            " def stop():",
        ]
        .join("\n")
            + "\n"
    }

    fn kinds(rows: &[DiffRow]) -> Vec<&'static str> {
        rows.iter().map(|row| row.kind.as_str()).collect()
    }

    #[test]
    fn rows_carry_their_line_number_on_each_side() {
        let rows = parse_diff(&diff_fixture(), MAX_ROWS);
        // The file header is dropped; the hunk header stays as a row of its own.
        assert_eq!(
            kinds(&rows),
            ["hunk", "context", "context", "del", "add", "add", "context", "context"]
        );
        let numbered: Vec<(&str, Option<u32>, Option<u32>)> = rows[1..]
            .iter()
            .map(|row| (row.kind.as_str(), row.old_no, row.new_no))
            .collect();
        assert_eq!(
            numbered,
            [
                ("context", Some(3), Some(3)),
                ("context", Some(4), Some(4)),
                ("del", Some(5), None),   // only in the old file
                ("add", None, Some(5)),   // only in the new one
                ("add", None, Some(6)),
                ("context", Some(6), Some(7)), // the sides have drifted apart
                ("context", Some(7), Some(8)),
            ]
        );
        assert_eq!(rows[0].text, "@@ -3,6 +3,7 @@ import os");
        assert_eq!(rows[4].text, "+    value += 1");
    }

    #[test]
    fn binary_and_no_newline_lines_are_kept_as_notes() {
        let rows = parse_diff(
            "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n",
            MAX_ROWS,
        );
        assert_eq!(kinds(&rows), ["note"]);

        let rows = parse_diff("@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n", MAX_ROWS);
        assert_eq!(kinds(&rows), ["hunk", "del", "add", "note"]);
    }

    #[test]
    fn a_blank_line_with_no_marker_still_counts_as_context() {
        // git writes " " for a blank context line, but a diff that has been
        // through a whitespace-trimming tool arrives with the marker gone.
        let rows = parse_diff("@@ -1,3 +1,3 @@\n one\n\n-two\n+three\n", MAX_ROWS);
        assert_eq!(kinds(&rows), ["hunk", "context", "context", "del", "add"]);
        assert_eq!(
            rows[1..3]
                .iter()
                .map(|row| (row.old_no, row.new_no))
                .collect::<Vec<_>>(),
            [(Some(1), Some(1)), (Some(2), Some(2))]
        );
    }

    #[test]
    fn a_diff_past_the_row_cap_is_truncated() {
        let body: String = (0..50).map(|i| format!("+line {i}\n")).collect();
        let rows = parse_diff(&format!("@@ -0,0 +1,50 @@\n{body}"), 10);
        assert_eq!(rows.len(), 11); // the cap, plus the note that says so
        assert_eq!(rows[10].kind, RowKind::Note);
        assert!(rows[10].text.contains("truncated"));
    }

    #[test]
    fn a_hunk_header_needs_both_sides_and_its_closing_marker() {
        // Both sides' *start* lines, which is what numbers the rows that
        // follow; the counts after the commas are not needed and not returned.
        assert_eq!(parse_hunk_header("@@ -3,6 +3,7 @@ import os"), Some((3, 3)));
        assert_eq!(parse_hunk_header("@@ -12,4 +40,9 @@"), Some((12, 40)));
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
        // A removed line that merely starts the same way is not a header.
        assert_eq!(parse_hunk_header("@@ -3,6 +3,7 no marker"), None);
        assert_eq!(parse_hunk_header("-    return value"), None);
        assert_eq!(parse_hunk_header("@@ -x +y @@"), None);
    }

    #[test]
    fn the_gutter_fits_the_longest_number_on_either_side() {
        // One context line, then a large insertion after it: the old side never
        // gets past 1 while the new side runs into four digits. The context row
        // is the only one carrying both numbers, so sizing from the widest *row*
        // rather than the widest number would settle on 1 and clip the rest.
        let body: String = (0..1199).map(|i| format!("+line {i}\n")).collect();
        let rows = parse_diff(&format!("@@ -1,1 +1,1200 @@\n context\n{body}"), MAX_ROWS);
        assert_eq!(gutter_digits(&rows), 4);
        // Two digits is the floor, so a one-line diff's gutter is not a sliver.
        assert_eq!(gutter_digits(&parse_diff("@@ -1 +1 @@\n+x\n", MAX_ROWS)), 2);
    }

    #[test]
    fn a_row_is_coloured_from_the_side_it_belongs_to() {
        let rows = parse_diff(&diff_fixture(), MAX_ROWS);
        // A removed line only exists in the old file.
        assert_eq!(span_source(&rows[3]), Some((Side::Old, 4)));
        assert_eq!(span_source(&rows[4]), Some((Side::New, 4)));
        assert_eq!(span_source(&rows[1]), Some((Side::New, 2)));
        // Structural rows carry no code to highlight.
        assert_eq!(span_source(&rows[0]), None);
    }

    // ---- The sheet's body -------------------------------------------------

    #[test]
    fn a_diff_that_is_only_a_binary_note_reads_as_a_message() {
        // git says this and nothing else about a binary change; one row of body
        // for it would be a worse way to show the same sentence.
        let document = DiffDocument {
            path: "x.png".into(),
            letter: 'M',
            diff_text: "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n"
                .into(),
            ..DiffDocument::default()
        };
        let body = document.body();
        assert!(body.rows.is_empty());
        assert_eq!(
            body.message.as_deref(),
            Some("Binary files a/x.png and b/x.png differ")
        );
    }

    #[test]
    fn a_document_with_a_message_never_shows_a_body() {
        let document = DiffDocument {
            path: "logo.png".into(),
            letter: '?',
            message: "Binary file — no text diff".into(),
            diff_text: diff_fixture(),
            ..DiffDocument::default()
        };
        assert_eq!(
            document.body().message.as_deref(),
            Some("Binary file — no text diff")
        );
    }

    #[test]
    fn an_empty_diff_says_so_rather_than_showing_nothing() {
        let body = DiffDocument::default().body();
        assert!(body.rows.is_empty());
        assert_eq!(body.message.as_deref(), Some("No textual changes"));
    }

    #[test]
    fn a_real_diff_keeps_its_rows_and_no_message() {
        let document = DiffDocument {
            diff_text: diff_fixture(),
            ..DiffDocument::default()
        };
        let body = document.body();
        assert_eq!(body.message, None);
        assert_eq!(body.rows.len(), 8);
    }

    // ---- Command builders -------------------------------------------------

    #[test]
    fn a_pathspec_is_anchored_to_the_repo_root() {
        // A bare path would be read relative to git's cwd and quietly match
        // nothing when the workspace is a subdirectory of the repo.
        let mut renamed = change("R ", "pkg/new.txt");
        renamed.orig = Some("pkg/old.txt".into());
        assert_eq!(
            file_diff_argv(&renamed),
            vec![
                "diff",
                "HEAD",
                "--",
                ":(top)pkg/new.txt",
                ":(top)pkg/old.txt"
            ]
        );
        assert_eq!(
            file_diff_argv(&change(" M", "a.txt")),
            vec!["diff", "HEAD", "--", ":(top)a.txt"]
        );
    }

    #[test]
    fn a_new_file_is_diffed_against_nothing() {
        assert_eq!(
            new_file_diff_argv("/repo", "fresh.txt"),
            vec!["diff", "--no-index", "--", "/dev/null", "/repo/fresh.txt"]
        );
        assert_eq!(head_text_argv("pkg/a.txt"), vec!["show", "HEAD:pkg/a.txt"]);
        assert_eq!(
            status_argv(),
            vec!["status", "--porcelain=v1", "-z", "--untracked-files=all"]
        );
    }

    #[test]
    fn a_failure_only_explains_itself_when_it_is_not_a_repository() {
        assert_eq!(
            status_failure_message("fatal: not a git repository (or any parent)"),
            "Not a git repository"
        );
        assert_eq!(
            status_failure_message("fatal: unable to read index"),
            "Could not read the working tree"
        );
    }

    // ---- Against a real repository ----------------------------------------

    /// A repo with one commit: a file to modify, one to rename, one to delete.
    fn repo_with_history(name: &str) -> TestRepo {
        let repo = TestRepo::new(name);
        repo.write("keep.txt", "a\nb\nc\n");
        repo.write("move_me.txt", "one\ntwo\nthree\n");
        repo.write("delete_me.txt", "gone\n");
        repo.commit("init");
        repo
    }

    /// The pane's rows as `{path: (letter, adds, dels)}`, as they are drawn.
    fn rows_of(data: &DiffData) -> BTreeMap<String, (char, String, String)> {
        match data {
            DiffData::Files { files, .. } => files
                .iter()
                .map(|change| {
                    (
                        change.path.clone(),
                        (
                            change.letter(),
                            count_text(change.adds, "+"),
                            count_text(change.dels, "−"),
                        ),
                    )
                })
                .collect(),
            DiffData::Message(message) => panic!("expected files, got {message:?}"),
        }
    }

    #[test]
    fn a_clean_repository_reports_no_changes() {
        let repo = repo_with_history("clean");
        assert!(rows_of(&gather(&repo.runner(), &DiffLimits::default())).is_empty());
    }

    #[test]
    fn lines_added_and_removed_are_counted_per_file() {
        let repo = repo_with_history("counts");
        // Two lines added to a three-line file, and one of the originals dropped.
        repo.write("keep.txt", "a\nc\nd\ne\n");
        let rows = rows_of(&gather(&repo.runner(), &DiffLimits::default()));
        assert_eq!(rows["keep.txt"], ('M', "+2".into(), "−1".into()));
    }

    #[test]
    fn staged_and_unstaged_edits_count_as_one_total() {
        // The pane measures against HEAD, so a file edited on both sides of the
        // index is still one row carrying the whole change.
        let repo = repo_with_history("both-sides");
        repo.write("keep.txt", "a\nb\nc\nstaged\n");
        repo.git(&["add", "keep.txt"]);
        repo.write("keep.txt", "a\nb\nc\nstaged\nunstaged\n");
        let rows = rows_of(&gather(&repo.runner(), &DiffLimits::default()));
        assert_eq!(rows["keep.txt"], ('M', "+2".into(), "−0".into()));
    }

    #[test]
    fn an_untracked_file_counts_its_lines_as_additions() {
        let repo = repo_with_history("untracked");
        repo.write("fresh.txt", "one\ntwo\nthree\n");
        // A final line without a newline still counts, the way git counts it.
        repo.write("no_eol.txt", "only");
        let rows = rows_of(&gather(&repo.runner(), &DiffLimits::default()));
        assert_eq!(rows["fresh.txt"], ('?', "+3".into(), "−0".into()));
        assert_eq!(rows["no_eol.txt"], ('?', "+1".into(), "−0".into()));
    }

    #[test]
    fn deleted_and_renamed_files_are_listed() {
        let repo = repo_with_history("moves");
        std::fs::remove_file(repo.path.join("delete_me.txt")).unwrap();
        repo.git(&["mv", "move_me.txt", "moved.txt"]);
        let rows = rows_of(&gather(&repo.runner(), &DiffLimits::default()));
        assert_eq!(rows["delete_me.txt"], ('D', "+0".into(), "−1".into()));
        // The rename is one row under the new name, not an add plus a delete.
        assert!(!rows.contains_key("move_me.txt"));
        assert_eq!(rows["moved.txt"].0, 'R');
    }

    #[test]
    fn a_binary_file_has_no_line_counts() {
        // Both counts read as unknown: a zero for the removals would be a number
        // the pane made up, next to an addition count it admits it doesn't have.
        let repo = repo_with_history("binary");
        std::fs::write(repo.path.join("untracked.bin"), b"\x00\x01\x02binary\x00").unwrap();
        std::fs::write(repo.path.join("keep.txt"), b"a\nb\n\x00tracked binary now\x00").unwrap();
        let data = gather(&repo.runner(), &DiffLimits::default());
        let rows = rows_of(&data);
        assert_eq!(rows["untracked.bin"], ('?', "—".into(), "—".into()));
        assert_eq!(rows["keep.txt"], ('M', "—".into(), "—".into()));
        // git's "-" for the tracked file, the NUL byte for the untracked one.
        match &data {
            DiffData::Files { files, .. } => {
                for change in files {
                    assert!(change.binary, "{} should be binary", change.path);
                }
            }
            other => panic!("expected files, got {other:?}"),
        }
    }

    #[test]
    fn an_unborn_head_still_reports_staged_and_untracked_files() {
        // A repo with no commit yet has no HEAD to diff against; the pane falls
        // back to the index and worktree instead of showing nothing.
        let repo = TestRepo::new("unborn-diff");
        repo.write("staged.txt", "a\nb\n");
        repo.git(&["add", "staged.txt"]);
        repo.write("loose.txt", "c\n");
        let rows = rows_of(&gather(&repo.runner(), &DiffLimits::default()));
        assert_eq!(rows["staged.txt"], ('A', "+2".into(), "−0".into()));
        assert_eq!(rows["loose.txt"], ('?', "+1".into(), "−0".into()));
    }

    #[test]
    fn outside_a_repository_the_pane_says_so() {
        let dir = scratch("no-repo");
        let runner = GitRunner::local(dir.to_string_lossy().into_owned());
        assert_eq!(
            gather(&runner, &DiffLimits::default()),
            DiffData::Message("Not a git repository".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_on_a_subdirectory_still_finds_its_files() {
        // git reports paths relative to the repo root, while a pathspec and a
        // file read are relative to where git ran. A workspace opened on a
        // subdirectory is where those two disagree, and both the counts and the
        // diff depend on it.
        let repo = TestRepo::new("subdir");
        repo.write("pkg/tracked.txt", "one\ntwo\n");
        repo.commit("init");
        repo.write("pkg/tracked.txt", "one\ntwo\nthree\n");
        repo.write("pkg/fresh.txt", "new\nlines\n");
        std::fs::create_dir_all(repo.path.join("pkg/deep")).unwrap();

        let runner = GitRunner::local(repo.path.join("pkg/deep").to_string_lossy().into_owned());
        let data = gather(&runner, &DiffLimits::default());
        let rows = rows_of(&data);
        assert_eq!(rows["pkg/tracked.txt"], ('M', "+1".into(), "−0".into()));
        // The untracked count is a filesystem read, which is the half that
        // breaks when the root is assumed to be the workspace.
        assert_eq!(rows["pkg/fresh.txt"], ('?', "+2".into(), "−0".into()));

        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let tracked = files
            .iter()
            .find(|change| change.path == "pkg/tracked.txt")
            .expect("the tracked file");
        let document = diff_document(&runner, tracked, root, &DiffLimits::default());
        let body = document.body();
        assert!(
            body.rows.iter().any(|row| row.text == "+three"),
            "{:?}",
            body.rows
        );
        // Both sides of the file come along, for the highlighter to lex.
        assert_eq!(document.old_text, "one\ntwo\n");
        assert_eq!(document.new_text, "one\ntwo\nthree\n");
        assert_eq!(document.subtitle, "modified · not staged · +1 −0");
    }

    #[test]
    fn an_untracked_file_opens_as_all_additions() {
        let repo = repo_with_history("new-file-diff");
        repo.write("fresh.rs", "fn main() {}\n");
        let runner = repo.runner();
        let data = gather(&runner, &DiffLimits::default());
        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let document = diff_document(&runner, &files[0], root, &DiffLimits::default());
        assert_eq!(document.letter, '?');
        // Nothing to read on the other side of a file that is new here.
        assert_eq!(document.old_text, "");
        let body = document.body();
        assert!(body.rows.iter().any(|row| row.text == "+fn main() {}"));
    }

    #[test]
    fn a_binary_file_says_so_instead_of_opening_a_diff() {
        let repo = repo_with_history("binary-open");
        std::fs::write(repo.path.join("blob.bin"), b"\x00\x01binary\x00").unwrap();
        let runner = repo.runner();
        let data = gather(&runner, &DiffLimits::default());
        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let document = diff_document(&runner, &files[0], root, &DiffLimits::default());
        assert_eq!(document.message, "Binary file — no text diff");
        assert!(document.diff_text.is_empty());
    }

    #[test]
    fn a_deleted_file_diffs_against_head_with_nothing_on_the_new_side() {
        let repo = repo_with_history("deleted-open");
        std::fs::remove_file(repo.path.join("delete_me.txt")).unwrap();
        let runner = repo.runner();
        let data = gather(&runner, &DiffLimits::default());
        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let document = diff_document(&runner, &files[0], root, &DiffLimits::default());
        assert_eq!(document.letter, 'D');
        assert_eq!(document.old_text, "gone\n");
        assert_eq!(document.new_text, "");
        assert!(document
            .body()
            .rows
            .iter()
            .any(|row| row.kind == RowKind::Del && row.text == "-gone"));
    }

    #[test]
    fn a_rename_opens_as_one_diff_rather_than_an_add_and_a_delete() {
        let repo = repo_with_history("rename-open");
        repo.git(&["mv", "move_me.txt", "moved.txt"]);
        repo.write("moved.txt", "one\ntwo\nthree\nfour\n");
        let runner = repo.runner();
        let data = gather(&runner, &DiffLimits::default());
        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let renamed = files
            .iter()
            .find(|change| change.path == "moved.txt")
            .expect("the renamed file");
        assert_eq!(renamed.orig.as_deref(), Some("move_me.txt"));
        let document = diff_document(&runner, renamed, root, &DiffLimits::default());
        // The old name's content is what HEAD has, and the diff is one file's.
        assert_eq!(document.old_text, "one\ntwo\nthree\n");
        assert!(document.diff_text.contains("rename"), "{}", document.diff_text);
    }

    #[test]
    fn a_file_past_the_untracked_cap_is_uncounted_but_still_openable() {
        // A count can go missing for a file that simply wasn't counted — past
        // the per-refresh cap, or too large to read. Such a file still has a
        // diff, and reporting it as binary would both mislabel the row and
        // refuse to open it.
        let repo = repo_with_history("cap");
        repo.write("a_first.txt", "only counted one\n");
        repo.write("b_beyond_cap.txt", "still text\nand still openable\n");
        let limits = DiffLimits {
            max_counted_untracked: 1,
            ..DiffLimits::default()
        };
        let runner = repo.runner();
        let data = gather(&runner, &limits);
        let rows = rows_of(&data);
        assert_eq!(rows["b_beyond_cap.txt"], ('?', "—".into(), "—".into()));

        let DiffData::Files { files, root, .. } = &data else {
            panic!("expected files");
        };
        let beyond = files
            .iter()
            .find(|change| change.path == "b_beyond_cap.txt")
            .expect("the uncounted file");
        assert!(!beyond.binary);
        assert!(tooltip(beyond).contains("lines not counted"));
        let document = diff_document(&runner, beyond, root, &limits);
        assert_eq!(document.message, "");
        assert!(document
            .body()
            .rows
            .iter()
            .any(|row| row.text == "+still text"));
    }

    #[test]
    fn the_file_cap_is_reported_rather_than_silently_applied() {
        let repo = repo_with_history("omitted");
        for index in 0..5 {
            repo.write(&format!("extra{index}.txt"), "x\n");
        }
        let limits = DiffLimits {
            max_files: 2,
            ..DiffLimits::default()
        };
        match gather(&repo.runner(), &limits) {
            DiffData::Files { files, omitted, .. } => {
                assert_eq!(files.len(), 2);
                assert_eq!(omitted, 3);
            }
            other => panic!("expected files, got {other:?}"),
        }
    }
}
