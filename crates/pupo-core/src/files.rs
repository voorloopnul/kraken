//! The file explorer's tree, and the rules a copy in or out of it obeys.
//!
//! The tree is the shape an editor's is: one row per entry, directories before
//! files, and a directory's children indented under it only while it is open.
//! Expansion is remembered per directory rather than per row, so a collapse and
//! a re-expand come back to the same open branches, and a refresh that finds a
//! new file leaves every other branch where the reader left it.
//!
//! Directories are read lazily — opening one is one `readdir`, and the tree
//! never walks what nobody has looked at. That is the difference between a
//! panel that opens instantly on a repository with a `target/` in it and one
//! that appears to hang.
//!
//! The other half of this file is copying. Every rule that decides whether a
//! copy is allowed, and what the copy ends up called, lives here rather than in
//! the panel: a copy is the one thing this feature does that writes to the
//! user's disk, and "would this overwrite something" is not a question to answer
//! from a drag handler.
//!
//! A name that is already taken is the reader's decision, not this file's. The
//! collision is found before anything is written ([`collisions`]) and the copy
//! waits for an answer ([`OnCollision`]) — replacing quietly would destroy work,
//! and renaming quietly leaves a `report 2.txt` nobody asked for and nobody
//! notices until the wrong one gets sent.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The most rows the tree will produce, however much is expanded.
///
/// A `node_modules` opened by accident is tens of thousands of entries, and a
/// list that long is not a thing anyone is reading — it is a scrollbar with no
/// travel and a panel that takes a second to lay out. The walk stops here and
/// says so, which is recoverable; the alternative is a window that stops
/// answering.
pub const MAX_ROWS: usize = 20_000;

/// The largest file the preview will read. Past this the sheet says how big the
/// file is instead: the point of a preview is a look, and a reader who wants to
/// page through forty megabytes wants an editor.
pub const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

/// The most lines the preview lays out.
///
/// The same cap the diff sheet uses, and for the same reason: both sheets lay
/// every row out rather than virtualising, so the number that keeps one of them
/// quick is the number that keeps the other quick. A generated file can be one
/// line short of a million, and every one of them would become a row.
pub const MAX_PREVIEW_LINES: usize = 4_000;

/// How much of a file is sniffed to decide whether it is text.
const SNIFF_BYTES: usize = 8192;

// ---------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------

/// One name in a directory, as the tree shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Whether the row opens. A symlink to a directory counts as one — it is
    /// what the reader sees when they follow it — which is also why the walk
    /// below has to guard against a link that points at its own ancestor.
    pub is_dir: bool,
    pub size: u64,
    pub symlink: bool,
}

/// Whether a name is one the tree hides unless asked. Dotfiles, by the same
/// rule every file manager on this platform uses.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Directories first, then by name, case-insensitively.
///
/// The tie-break on the raw name is not decoration: without it `README` and
/// `readme` compare equal, and an unstable order between two refreshes moves
/// rows under the pointer. Two entries in one directory can never tie on the
/// raw name, so the order is total.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// One directory's entries, sorted. An unreadable directory is an error rather
/// than an empty one — the panel says which it was, because "no permission" and
/// "nothing in it" look identical in a list and mean opposite things.
pub fn read_dir(dir: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && is_hidden(&name) {
            continue;
        }
        // `file_type` is the cheap one — it comes from the directory read
        // itself and does not follow the link. `metadata` does follow it, which
        // is what makes a symlink to a folder open like a folder.
        let link = item
            .file_type()
            .map(|kind| kind.is_symlink())
            .unwrap_or(false);
        let (is_dir, size) = match item.metadata() {
            Ok(meta) => (meta.is_dir(), meta.len()),
            // A broken symlink: it has a name and nothing behind it. Listing it
            // as a zero-byte file is the honest row — dropping it would leave a
            // name visible in every other tool missing from this one.
            Err(_) => (false, 0),
        };
        entries.push(Entry {
            name,
            is_dir,
            size,
            symlink: link,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

/// One visible line: an entry, plus where it sits in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// 0 for a child of the root, 1 for its children, and so on.
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub size: u64,
    pub symlink: bool,
}

/// Why a directory has no rows under it, when it is open and shows none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Empty {
    /// Nothing in it — or nothing but hidden entries, with hidden entries off.
    Nothing,
    /// It could not be read; the usual reason is permissions.
    Unreadable,
}

/// The workspace's tree: what is open, and what has been read.
#[derive(Debug, Default)]
pub struct Tree {
    root: PathBuf,
    /// Directories the reader has opened. Kept across a refresh and across a
    /// collapse, which is what makes re-opening a branch return to it rather
    /// than to a closed copy of it.
    expanded: BTreeSet<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    unreadable: BTreeSet<PathBuf>,
    show_hidden: bool,
    /// Set while the last walk stopped at [`MAX_ROWS`].
    truncated: bool,
}

impl Tree {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ..Default::default()
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Point the tree at another folder. Everything read and everything open
    /// belongs to the old one, so all of it goes.
    pub fn set_root(&mut self, root: impl Into<PathBuf>) {
        let root = root.into();
        if root == self.root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        self.children.clear();
        self.unreadable.clear();
        self.truncated = false;
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Show or hide dotfiles. Every cached listing was filtered under the old
    /// answer, so the cache goes; what is open does not.
    pub fn set_show_hidden(&mut self, show: bool) {
        if show == self.show_hidden {
            return;
        }
        self.show_hidden = show;
        self.children.clear();
        self.unreadable.clear();
    }

    /// Re-read every directory, keeping what is open. This is what a refresh
    /// and a completed copy both do: the reader's place in the tree is not
    /// something either of them has any business resetting.
    pub fn refresh(&mut self) {
        self.children.clear();
        self.unreadable.clear();
    }

    /// Forget one directory's listing, so the next walk re-reads it.
    pub fn invalidate(&mut self, dir: &Path) {
        self.children.remove(dir);
        self.unreadable.remove(dir);
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn expand(&mut self, path: &Path) {
        self.expanded.insert(path.to_path_buf());
    }

    pub fn collapse(&mut self, path: &Path) {
        self.expanded.remove(path);
    }

    /// Close every branch, leaving the top level. The way back from a tree
    /// that got away from the reader.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    pub fn toggle(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
    }

    /// Open every directory between the root and `path`, so a file the app
    /// wants to point at ends up on screen. Used after a copy in: the new file
    /// is the one thing the reader is looking for, and leaving it inside a
    /// closed branch is the one place they will not look.
    pub fn reveal(&mut self, path: &Path) {
        let mut cursor = path.parent();
        while let Some(dir) = cursor {
            if !dir.starts_with(&self.root) {
                break;
            }
            self.expanded.insert(dir.to_path_buf());
            if dir == self.root {
                break;
            }
            cursor = dir.parent();
        }
    }

    /// Why an open directory shows nothing, or `None` when it has rows.
    pub fn empty_reason(&self, dir: &Path) -> Option<Empty> {
        if self.unreadable.contains(dir) {
            return Some(Empty::Unreadable);
        }
        match self.children.get(dir) {
            Some(entries) if entries.is_empty() => Some(Empty::Nothing),
            _ => None,
        }
    }

    /// Whether the last walk hit [`MAX_ROWS`] and stopped short.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The visible rows, top to bottom. Reads whatever open directory has not
    /// been read yet, which is why this takes `&mut self`.
    pub fn rows(&mut self) -> Vec<Row> {
        self.truncated = false;
        let mut rows = Vec::new();
        if self.root.as_os_str().is_empty() {
            return rows;
        }
        let root = self.root.clone();
        // Guards against a symlink loop: a link pointing at one of its own
        // ancestors would otherwise be a branch that can be opened for ever.
        let mut open: Vec<PathBuf> = vec![root.clone()];
        self.walk(&root, 0, &mut open, &mut rows);
        rows
    }

    fn walk(&mut self, dir: &Path, depth: usize, open: &mut Vec<PathBuf>, rows: &mut Vec<Row>) {
        for entry in self.load(dir) {
            if rows.len() >= MAX_ROWS {
                self.truncated = true;
                return;
            }
            let path = dir.join(&entry.name);
            let expanded = entry.is_dir && self.expanded.contains(&path);
            rows.push(Row {
                path: path.clone(),
                name: entry.name,
                depth,
                is_dir: entry.is_dir,
                expanded,
                size: entry.size,
                symlink: entry.symlink,
            });
            if !expanded {
                continue;
            }
            // A link back up the branch we are standing on. Its row stays —
            // the link is really there — but it is not followed.
            let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if open.contains(&real) {
                continue;
            }
            open.push(real);
            self.walk(&path, depth + 1, open, rows);
            open.pop();
        }
    }

    /// One directory's entries, reading it the first time it is asked for.
    fn load(&mut self, dir: &Path) -> Vec<Entry> {
        if let Some(cached) = self.children.get(dir) {
            return cached.clone();
        }
        let entries = match read_dir(dir, self.show_hidden) {
            Ok(entries) => entries,
            Err(_) => {
                self.unreadable.insert(dir.to_path_buf());
                Vec::new()
            }
        };
        self.children.insert(dir.to_path_buf(), entries.clone());
        entries
    }
}

// ---------------------------------------------------------------------------
// Naming a copy
// ---------------------------------------------------------------------------

/// What to do about a destination name that is already taken.
///
/// There is no default. Both answers lose something — one overwrites work, the
/// other leaves a near-duplicate — so the caller has to have asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnCollision {
    /// Copy alongside, under a name that is not taken.
    KeepBoth,
    /// Delete what is there and put this in its place.
    Replace,
}

/// The names among `sources` that `dest_dir` already has, in the order they
/// were given. Empty when nothing is in the way, which is the case that needs
/// no question asked.
pub fn collisions(sources: &[PathBuf], dest_dir: &Path) -> Vec<String> {
    sources
        .iter()
        .filter_map(|source| source.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| dest_dir.join(name).exists())
        .collect()
}

/// The question the panel asks about a collision.
///
/// Names the single file where there is one, because "note.txt already exists"
/// is answerable and "1 item already exists" is not. Past one, the count is the
/// honest summary — a dialog listing forty names is a dialog nobody reads.
pub fn collision_message(names: &[String], destination: &str) -> String {
    let place = if destination.is_empty() {
        "the workspace".to_string()
    } else {
        format!("\u{201c}{destination}\u{201d}")
    };
    match names.len() {
        0 => String::new(),
        1 => format!(
            "\u{201c}{}\u{201d} already exists in {place}.",
            names[0]
        ),
        many => format!("{many} items already exist in {place}."),
    }
}

/// `name` with ` <n>` worked into it, for the nth copy of a file.
///
/// The number goes before the extension, because the extension is what decides
/// how the file opens and `report.txt 2` opens in nothing. A dotfile has no
/// extension to protect — Rust reads `.bashrc` as all stem — so its number goes
/// on the end, which is where it belongs.
pub fn numbered_name(name: &str, n: u32) -> String {
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => format!(
            "{} {}.{}",
            stem.to_string_lossy(),
            n,
            extension.to_string_lossy()
        ),
        _ => format!("{name} {n}"),
    }
}

/// A name for `name` inside `dir` that is not taken.
///
/// A copy never overwrites. That is the whole rule, and it is here rather than
/// at the call sites because there are three of them — a drop, a paste and an
/// import — and two out of three getting it right is a feature that eats files.
pub fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    for n in 2..1000 {
        let candidate = numbered_name(name, n);
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    // A thousand copies of one name is not a case worth a cleverer answer, but
    // it still must not return a name that exists.
    numbered_name(name, std::process::id())
}

// ---------------------------------------------------------------------------
// Whether a copy is allowed
// ---------------------------------------------------------------------------

/// Why a copy was refused before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The source is gone — a stale row, or something moved under us.
    Missing,
    /// The destination is not a directory that can be written into.
    NotADirectory,
    /// A directory copied into itself or into something inside it. Left to run,
    /// this copies for ever and fills the disk.
    IntoItself,
    /// An import whose destination is outside the workspace. The panel's job is
    /// this project; a drop that lands elsewhere is one nobody asked for.
    OutsideWorkspace,
    /// A replace whose destination is the source itself, or a folder holding
    /// it. Replacing means deleting what is there first, so left to run this
    /// deletes the very thing it was about to copy.
    OntoItself,
}

impl Refusal {
    /// The sentence the panel shows. Written here so the same refusal reads the
    /// same way whichever gesture caused it.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::Missing => "That file is no longer there.",
            Refusal::NotADirectory => "That is not a folder to copy into.",
            Refusal::IntoItself => "A folder cannot be copied into itself.",
            Refusal::OutsideWorkspace => "That destination is outside the workspace.",
            Refusal::OntoItself => "That would replace the file with itself.",
        }
    }
}

/// `path` with `.` and `..` resolved textually, without touching the disk.
///
/// Lexical on purpose: `canonicalize` resolves symlinks too, and a workspace
/// reached through a symlinked home would then fail every containment check
/// against the path the app was given.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `path` is `root` or sits under it.
pub fn within(root: &Path, path: &Path) -> bool {
    normalize(path).starts_with(normalize(root))
}

/// Whether `source` may be copied into `dest_dir` under `mode`.
///
/// `root` is the workspace, and `Some` of it means this is an import that has
/// to land inside the project. An export — copying out to anywhere the reader
/// picked — passes `None`, because the whole point of it is to leave.
///
/// The mode matters because [`OnCollision::Replace`] deletes before it writes,
/// which makes destinations legal under `KeepBoth` fatal under `Replace`.
pub fn check_copy(
    source: &Path,
    dest_dir: &Path,
    root: Option<&Path>,
    mode: OnCollision,
) -> Result<(), Refusal> {
    if !source.exists() {
        return Err(Refusal::Missing);
    }
    if !dest_dir.is_dir() {
        return Err(Refusal::NotADirectory);
    }
    if let Some(root) = root {
        if !within(root, dest_dir) {
            return Err(Refusal::OutsideWorkspace);
        }
    }
    // Only a directory can contain its own destination. A file copied beside
    // itself is a duplicate, which is a thing people mean to do.
    if source.is_dir() && within(source, dest_dir) {
        return Err(Refusal::IntoItself);
    }
    // Under Replace the destination is deleted first. If the thing that would
    // be deleted is the source — or a folder with the source inside it — the
    // copy destroys its own input and there is nothing left to write.
    if mode == OnCollision::Replace {
        if let Some(name) = source.file_name() {
            if within(&dest_dir.join(name), source) {
                return Err(Refusal::OntoItself);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Doing the copy
// ---------------------------------------------------------------------------

/// What a finished copy moved, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub files: usize,
    pub bytes: u64,
    /// One line per thing that failed, naming it. A copy of a tree is many
    /// operations and some of them fail on their own — an unreadable file, a
    /// socket, a device node — and stopping the whole copy at the first is
    /// worse than finishing it and saying what was left behind.
    pub failures: Vec<String>,
    /// Where the copy landed, once the collision rule has had its say.
    pub destination: PathBuf,
    /// Whether something was deleted to make room. Worth reporting: it is the
    /// one outcome here that cannot be undone.
    pub replaced: bool,
}

impl CopyReport {
    /// The sentence the panel shows when the copy is done.
    pub fn summary(&self) -> String {
        let name = self
            .destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let verb = if self.replaced { "Replaced" } else { "Copied" };
        let head = if self.files == 1 {
            format!("{verb} {name}")
        } else {
            format!(
                "{verb} {name} — {} files, {}",
                self.files,
                format_size(self.bytes)
            )
        };
        if self.failures.is_empty() {
            head
        } else {
            format!("{head} ({} skipped)", self.failures.len())
        }
    }
}

/// Copy `source` into `dest_dir`, resolving a taken name the way `mode` says.
///
/// Directories come across whole. Symlinks are copied as what they point at
/// rather than re-created as links: a link into the source tree would dangle
/// once the copy is somewhere else, and a link out of it would be a surprise in
/// a folder the reader thinks they now own outright.
pub fn copy_into(
    source: &Path,
    dest_dir: &Path,
    root: Option<&Path>,
    mode: OnCollision,
) -> Result<CopyReport, Refusal> {
    check_copy(source, dest_dir, root, mode)?;
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or(Refusal::Missing)?;

    let destination = match mode {
        OnCollision::KeepBoth => dest_dir.join(unique_name(dest_dir, &name)),
        OnCollision::Replace => dest_dir.join(&name),
    };
    let mut report = CopyReport {
        destination: destination.clone(),
        ..Default::default()
    };

    // Clear the way, and only then. A failed removal must not be followed by a
    // copy that half-merges the new tree into the old one — `fs::copy` would
    // overwrite the files that match and leave every file the old tree had and
    // the new one does not, which is neither of the two things anyone asked
    // for and is indistinguishable from success.
    if mode == OnCollision::Replace && destination.exists() {
        let removed = if destination.is_dir() {
            fs::remove_dir_all(&destination)
        } else {
            fs::remove_file(&destination)
        };
        if let Err(error) = removed {
            report
                .failures
                .push(format!("{}: {error}", destination.display()));
            return Ok(report);
        }
        report.replaced = true;
    }

    copy_tree(source, &destination, &mut report);
    Ok(report)
}

fn copy_tree(source: &Path, destination: &Path, report: &mut CopyReport) {
    let meta = match fs::metadata(source) {
        Ok(meta) => meta,
        Err(error) => return report.failures.push(format!("{}: {error}", source.display())),
    };
    if meta.is_dir() {
        if let Err(error) = fs::create_dir_all(destination) {
            return report
                .failures
                .push(format!("{}: {error}", destination.display()));
        }
        let entries = match fs::read_dir(source) {
            Ok(entries) => entries,
            Err(error) => {
                return report.failures.push(format!("{}: {error}", source.display()))
            }
        };
        for item in entries.flatten() {
            copy_tree(&item.path(), &destination.join(item.file_name()), report);
        }
        return;
    }
    if !meta.is_file() {
        // A socket, a fifo, a device node. There is nothing to copy and no
        // sensible stand-in, so it is named rather than silently dropped.
        return report
            .failures
            .push(format!("{}: not a regular file", source.display()));
    }
    match fs::copy(source, destination) {
        Ok(bytes) => {
            report.files += 1;
            report.bytes += bytes;
        }
        Err(error) => report.failures.push(format!("{}: {error}", source.display())),
    }
}

// ---------------------------------------------------------------------------
// Previewing a file
// ---------------------------------------------------------------------------

/// What the preview sheet can make of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// Text, split into lines. `truncated` is set when the file had more than
    /// [`MAX_PREVIEW_LINES`] and the tail was dropped.
    Text {
        lines: Vec<String>,
        truncated: bool,
    },
    /// An image, to be shown rather than decoded here.
    Image,
    /// Readable, but not text — the sheet says so instead of drawing mojibake.
    Binary,
    /// Bigger than [`MAX_PREVIEW_BYTES`].
    TooLarge(u64),
    /// Nothing in it. Distinct from binary so the sheet can say which.
    Empty,
    /// It could not be read, with the reason.
    Unreadable(String),
}

/// The extensions the sheet shows as a picture. Qt's image plugins read more
/// than this, but a list is what keeps a `.ico` from being sniffed as binary
/// and a `.svg` from being drawn as its own source — which, for an SVG, is a
/// real choice rather than a fallback.
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg"];

pub fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
}

/// Whether a file's opening bytes say it is not text.
///
/// A NUL byte is the test every tool from `grep` to `git` uses, and it is the
/// right one: no text encoding this app will meet puts a NUL in the middle of a
/// line, and every binary format worth refusing has one early.
pub fn looks_binary(head: &[u8]) -> bool {
    head.contains(&0)
}

/// Read a file for the sheet.
pub fn preview(path: &Path) -> Preview {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) => return Preview::Unreadable(error.to_string()),
    };
    if meta.is_dir() {
        return Preview::Unreadable("That is a folder.".to_string());
    }
    if meta.len() == 0 {
        return Preview::Empty;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Images are decided by name before size: a photograph is routinely past
    // the text budget and is exactly the kind of file a preview is for.
    if is_image(&name) {
        return Preview::Image;
    }
    if meta.len() > MAX_PREVIEW_BYTES {
        return Preview::TooLarge(meta.len());
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Preview::Unreadable(error.to_string()),
    };
    if looks_binary(&bytes[..bytes.len().min(SNIFF_BYTES)]) {
        return Preview::Binary;
    }
    // Lossy rather than strict: a file that is text apart from one bad byte is
    // still a file worth reading, and a refusal would say "binary" about
    // something the reader can see is not.
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let truncated = lines.len() > MAX_PREVIEW_LINES;
    lines.truncate(MAX_PREVIEW_LINES);
    Preview::Text { lines, truncated }
}

// ---------------------------------------------------------------------------
// Odds and ends the view needs
// ---------------------------------------------------------------------------

/// A byte count as a row shows it. Binary units under decimal-looking names,
/// which is what every file manager on this platform does and so what the
/// numbers here have to match to agree with the one beside it.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    // One decimal under ten, none above: "9.4 MB" is worth the character,
    // "473.2 MB" is not.
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{:.0} {}", value.round(), UNITS[unit])
    }
}

/// `path` written relative to the workspace, for a subtitle. An absolute path
/// that happens to be outside comes back whole.
pub fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rest| rest.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// A `file://` URL for a path, for the drag payload and the image preview.
///
/// Percent-encoding is minimal on purpose: the characters that must be escaped
/// for a receiving application to parse the URL back into the same path, and
/// nothing else. Escaping more is not safer — it just produces a URL that
/// some receivers hand back with the escapes still in them.
pub fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The path behind a `file://` URL, or `None` for a URL naming another scheme.
///
/// This is the other half of a drop: what arrives from another application is
/// `text/uri-list`, and a drop of anything that is not a local file — a link
/// from a browser, say — has no file to copy and must be declined rather than
/// turned into a file named `https:`.
pub fn path_from_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://host/path` is legal and not ours; `file:///path` leaves an empty
    // host, which is the only one that names a path on this machine.
    let rest = match rest.find('/') {
        Some(0) => rest,
        _ => return None,
    };
    let mut out = Vec::new();
    let mut bytes = rest.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            out.push(byte);
            continue;
        }
        let hex: String = bytes.by_ref().take(2).map(char::from).collect();
        match u8::from_str_radix(&hex, 16) {
            Ok(decoded) => out.push(decoded),
            // A stray `%` that is not an escape. Keeping it is what makes a
            // path containing one survive a round trip through a naive sender.
            Err(_) => {
                out.push(b'%');
                out.extend(hex.bytes());
            }
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// The paths in a `text/uri-list` payload, dropping every line that is not a
/// local file. Blank lines and `#` comments are part of the format.
pub fn paths_from_uri_list(payload: &str) -> Vec<PathBuf> {
    payload
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(path_from_url)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Sorting ----------------------------------------------------------

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir,
            size: 0,
            symlink: false,
        }
    }

    #[test]
    fn directories_sort_above_files_whatever_they_are_called() {
        let mut entries = vec![
            entry("apple.txt", false),
            entry("zebra", true),
            entry("banana.txt", false),
            entry("alpha", true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zebra", "apple.txt", "banana.txt"]);
    }

    #[test]
    fn names_sort_case_insensitively_but_the_order_is_still_total() {
        // Case-insensitively these three interleave; without the tie-break on
        // the raw name, README and readme would compare equal and could swap
        // between two refreshes — which moves rows under the pointer.
        let mut entries = vec![
            entry("readme", false),
            entry("Makefile", false),
            entry("README", false),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Makefile", "README", "readme"]);
    }

    // ---- Naming a copy ----------------------------------------------------

    #[test]
    fn a_number_goes_before_the_extension_not_after_it() {
        // The extension decides how the file opens, so `report.txt 2` is a file
        // that opens in nothing.
        assert_eq!(numbered_name("report.txt", 2), "report 2.txt");
        assert_eq!(numbered_name("archive.tar.gz", 3), "archive.tar 3.gz");
    }

    #[test]
    fn a_name_with_no_extension_to_protect_takes_the_number_on_the_end() {
        assert_eq!(numbered_name("Makefile", 2), "Makefile 2");
        // A dotfile is all stem — there is no extension here to get in front of.
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc 2");
        assert_eq!(numbered_name(".config", 4), ".config 4");
    }

    #[test]
    fn a_free_name_is_left_alone_and_a_taken_one_counts_up() {
        let dir = tempdir();
        assert_eq!(unique_name(&dir, "report.txt"), "report.txt");
        fs::write(dir.join("report.txt"), b"x").unwrap();
        assert_eq!(unique_name(&dir, "report.txt"), "report 2.txt");
        fs::write(dir.join("report 2.txt"), b"x").unwrap();
        assert_eq!(unique_name(&dir, "report.txt"), "report 3.txt");
    }

    // ---- Whether a copy is allowed ----------------------------------------

    #[test]
    fn normalize_resolves_dot_and_dotdot_without_touching_the_disk() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/a/b/../..")), PathBuf::from("/"));
    }

    #[test]
    fn a_folder_cannot_be_copied_into_itself_or_into_anything_inside_it() {
        let root = tempdir();
        let outer = root.join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(&inner).unwrap();

        assert_eq!(
            check_copy(&outer, &outer, None, OnCollision::KeepBoth),
            Err(Refusal::IntoItself)
        );
        // The one that actually eats a disk: the copy's own output becomes more
        // to copy, for ever.
        assert_eq!(
            check_copy(&outer, &inner, None, OnCollision::KeepBoth),
            Err(Refusal::IntoItself)
        );
        // The other way round is an ordinary copy.
        assert!(check_copy(&inner, &root, None, OnCollision::KeepBoth).is_ok());
    }

    #[test]
    fn a_file_beside_itself_is_a_duplicate_rather_than_a_refusal() {
        let dir = tempdir();
        let file = dir.join("note.txt");
        fs::write(&file, b"x").unwrap();
        assert!(check_copy(&file, &dir, None, OnCollision::KeepBoth).is_ok());
    }

    #[test]
    fn an_import_must_land_inside_the_workspace() {
        let root = tempdir();
        let inside = root.join("src");
        fs::create_dir(&inside).unwrap();
        let outside = tempdir();
        let source = outside.join("note.txt");
        fs::write(&source, b"x").unwrap();

        assert!(check_copy(&source, &inside, Some(&root), OnCollision::KeepBoth).is_ok());
        assert_eq!(
            check_copy(&source, &outside, Some(&root), OnCollision::KeepBoth),
            Err(Refusal::OutsideWorkspace)
        );
        // An export names no root, because leaving is the point of it.
        assert!(check_copy(&source, &outside, None, OnCollision::KeepBoth).is_ok());
    }

    #[test]
    fn a_source_that_is_gone_is_refused_before_anything_is_written() {
        let dir = tempdir();
        assert_eq!(
            check_copy(&dir.join("ghost"), &dir, None, OnCollision::KeepBoth),
            Err(Refusal::Missing)
        );
    }

    // ---- Doing the copy ---------------------------------------------------

    #[test]
    fn keeping_both_leaves_what_was_already_there() {
        let source_dir = tempdir();
        let dest = tempdir();
        fs::write(source_dir.join("note.txt"), b"new").unwrap();
        fs::write(dest.join("note.txt"), b"original").unwrap();

        let report = copy_into(
            &source_dir.join("note.txt"),
            &dest,
            None,
            OnCollision::KeepBoth,
        )
        .unwrap();
        assert_eq!(report.destination, dest.join("note 2.txt"));
        assert!(!report.replaced);
        assert_eq!(fs::read(dest.join("note.txt")).unwrap(), b"original");
        assert_eq!(fs::read(dest.join("note 2.txt")).unwrap(), b"new");
    }

    #[test]
    fn a_directory_comes_across_whole() {
        let source_dir = tempdir();
        let dest = tempdir();
        let tree = source_dir.join("project");
        fs::create_dir_all(tree.join("src/deep")).unwrap();
        fs::write(tree.join("README.md"), b"hello").unwrap();
        fs::write(tree.join("src/deep/main.rs"), b"fn main() {}").unwrap();

        let report = copy_into(&tree, &dest, None, OnCollision::KeepBoth).unwrap();
        assert_eq!(report.files, 2);
        assert_eq!(report.bytes, 5 + 12);
        assert!(report.failures.is_empty());
        assert_eq!(
            fs::read(dest.join("project/src/deep/main.rs")).unwrap(),
            b"fn main() {}"
        );
    }

    #[test]
    fn replacing_puts_the_new_file_where_the_old_one_was() {
        let source_dir = tempdir();
        let dest = tempdir();
        fs::write(source_dir.join("note.txt"), b"new").unwrap();
        fs::write(dest.join("note.txt"), b"original").unwrap();

        let report = copy_into(
            &source_dir.join("note.txt"),
            &dest,
            None,
            OnCollision::Replace,
        )
        .unwrap();
        assert_eq!(report.destination, dest.join("note.txt"));
        assert!(report.replaced);
        assert_eq!(fs::read(dest.join("note.txt")).unwrap(), b"new");
        // No stray second copy left beside it.
        assert!(!dest.join("note 2.txt").exists());
    }

    #[test]
    fn replacing_a_folder_removes_it_rather_than_merging_into_it() {
        // The failure this rules out is subtle: copying over the top would
        // overwrite the files that match and leave every file the old tree had
        // and the new one does not, which is neither tree.
        let source_dir = tempdir();
        let dest = tempdir();
        fs::create_dir(source_dir.join("tree")).unwrap();
        fs::write(source_dir.join("tree/new.txt"), b"new").unwrap();
        fs::create_dir(dest.join("tree")).unwrap();
        fs::write(dest.join("tree/stale.txt"), b"stale").unwrap();

        let report = copy_into(
            &source_dir.join("tree"),
            &dest,
            None,
            OnCollision::Replace,
        )
        .unwrap();
        assert!(report.replaced);
        assert!(dest.join("tree/new.txt").exists());
        assert!(!dest.join("tree/stale.txt").exists());
    }

    #[test]
    fn replacing_a_file_with_itself_is_refused_before_it_is_deleted() {
        // Beside itself is a duplicate under KeepBoth and suicide under
        // Replace: the destination is the source, so clearing the way for the
        // copy destroys what was about to be copied.
        let dir = tempdir();
        let file = dir.join("note.txt");
        fs::write(&file, b"precious").unwrap();

        assert!(check_copy(&file, &dir, None, OnCollision::KeepBoth).is_ok());
        assert_eq!(
            check_copy(&file, &dir, None, OnCollision::Replace),
            Err(Refusal::OntoItself)
        );
        assert_eq!(
            copy_into(&file, &dir, None, OnCollision::Replace),
            Err(Refusal::OntoItself)
        );
        assert_eq!(fs::read(&file).unwrap(), b"precious");
    }

    #[test]
    fn collisions_are_the_names_already_taken_and_nothing_else() {
        let source_dir = tempdir();
        let dest = tempdir();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(source_dir.join(name), b"x").unwrap();
        }
        fs::write(dest.join("a.txt"), b"x").unwrap();
        fs::write(dest.join("c.txt"), b"x").unwrap();

        let sources: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|name| source_dir.join(name))
            .collect();
        assert_eq!(collisions(&sources, &dest), ["a.txt", "c.txt"]);
        // Nothing in the way is the case that needs no question asked.
        assert!(collisions(&sources, &tempdir()).is_empty());
    }

    #[test]
    fn the_collision_question_names_one_file_and_counts_the_rest() {
        // "note.txt already exists" is answerable; "1 item already exists" is
        // not.
        assert_eq!(
            collision_message(&["note.txt".into()], "docs"),
            "\u{201c}note.txt\u{201d} already exists in \u{201c}docs\u{201d}."
        );
        assert_eq!(
            collision_message(&["a".into(), "b".into(), "c".into()], "docs"),
            "3 items already exist in \u{201c}docs\u{201d}."
        );
        // The workspace root has no name to quote.
        assert_eq!(
            collision_message(&["a".into(), "b".into()], ""),
            "2 items already exist in the workspace."
        );
        assert_eq!(collision_message(&[], "docs"), "");
    }

    // ---- The tree ---------------------------------------------------------

    /// A small tree: two folders, a file in each, and a file at the top.
    fn sample_tree() -> PathBuf {
        let root = tempdir();
        fs::create_dir_all(root.join("src/inner")).unwrap();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("README.md"), b"hi").unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        fs::write(root.join("src/inner/deep.rs"), b"deep").unwrap();
        fs::write(root.join("docs/guide.md"), b"guide").unwrap();
        fs::write(root.join(".hidden"), b"secret").unwrap();
        root
    }

    fn names(rows: &[Row]) -> Vec<(usize, String)> {
        rows.iter()
            .map(|row| (row.depth, row.name.clone()))
            .collect()
    }

    #[test]
    fn a_closed_tree_shows_only_the_top_level() {
        let mut tree = Tree::new(sample_tree());
        assert_eq!(
            names(&tree.rows()),
            [
                (0, "docs".into()),
                (0, "src".into()),
                (0, "README.md".into())
            ]
        );
    }

    #[test]
    fn an_expanded_directory_indents_its_children_under_it() {
        let root = sample_tree();
        let mut tree = Tree::new(&root);
        tree.expand(&root.join("src"));
        assert_eq!(
            names(&tree.rows()),
            [
                (0, "docs".into()),
                (0, "src".into()),
                (1, "inner".into()),
                (1, "main.rs".into()),
                (0, "README.md".into())
            ]
        );
    }

    #[test]
    fn collapsing_a_branch_remembers_what_was_open_inside_it() {
        // The behaviour every editor has, and the reason expansion is keyed by
        // directory rather than by row: re-opening returns to the branch the
        // reader left, not to a closed copy of it.
        let root = sample_tree();
        let mut tree = Tree::new(&root);
        tree.expand(&root.join("src"));
        tree.expand(&root.join("src/inner"));
        assert_eq!(tree.rows().len(), 6);

        tree.collapse(&root.join("src"));
        assert_eq!(tree.rows().len(), 3);

        tree.expand(&root.join("src"));
        assert_eq!(
            names(&tree.rows()),
            [
                (0, "docs".into()),
                (0, "src".into()),
                (1, "inner".into()),
                (2, "deep.rs".into()),
                (1, "main.rs".into()),
                (0, "README.md".into())
            ]
        );
    }

    #[test]
    fn hidden_entries_appear_only_when_they_are_asked_for() {
        let mut tree = Tree::new(sample_tree());
        assert!(!tree.rows().iter().any(|row| row.name == ".hidden"));
        tree.set_show_hidden(true);
        assert!(tree.rows().iter().any(|row| row.name == ".hidden"));
    }

    #[test]
    fn a_refresh_finds_a_new_file_and_leaves_the_open_branches_alone() {
        let root = sample_tree();
        let mut tree = Tree::new(&root);
        tree.expand(&root.join("src"));
        assert_eq!(tree.rows().len(), 5);

        fs::write(root.join("src/added.rs"), b"new").unwrap();
        // Without the refresh the listing is the cached one, which is the whole
        // point of caching it.
        assert_eq!(tree.rows().len(), 5);

        tree.refresh();
        let rows = tree.rows();
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().any(|row| row.name == "added.rs"));
        assert!(tree.is_expanded(&root.join("src")));
    }

    #[test]
    fn reveal_opens_every_branch_down_to_a_file() {
        let root = sample_tree();
        let mut tree = Tree::new(&root);
        tree.reveal(&root.join("src/inner/deep.rs"));
        assert!(tree.is_expanded(&root.join("src")));
        assert!(tree.is_expanded(&root.join("src/inner")));
        assert!(tree.rows().iter().any(|row| row.name == "deep.rs"));
    }

    #[test]
    fn reveal_stops_at_the_workspace_and_does_not_walk_out_of_it() {
        let root = sample_tree();
        let mut tree = Tree::new(root.join("src"));
        tree.reveal(&root.join("src/inner/deep.rs"));
        assert!(tree.is_expanded(&root.join("src/inner")));
        // The parent of the root is outside the panel's world.
        assert!(!tree.is_expanded(&root));
    }

    #[test]
    fn an_open_directory_says_whether_it_is_empty_or_unreadable() {
        let root = tempdir();
        let hollow = root.join("hollow");
        fs::create_dir(&hollow).unwrap();
        let mut tree = Tree::new(&root);
        tree.expand(&hollow);
        tree.rows();
        assert_eq!(tree.empty_reason(&hollow), Some(Empty::Nothing));

        // A directory that was never read is not "empty" — nothing is known
        // about it yet, and saying "empty" would be a claim.
        let mut fresh = Tree::new(&root);
        assert_eq!(fresh.empty_reason(&hollow), None);
        fresh.rows();
        assert_eq!(fresh.empty_reason(&hollow), None);
    }

    #[test]
    fn changing_the_root_forgets_everything_about_the_old_one() {
        let first = sample_tree();
        let second = tempdir();
        let mut tree = Tree::new(&first);
        tree.expand(&first.join("src"));
        assert_eq!(tree.rows().len(), 5);

        tree.set_root(&second);
        assert!(tree.rows().is_empty());
        assert!(!tree.is_expanded(&first.join("src")));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_back_up_its_own_branch_is_not_followed() {
        // Left to run this is a branch that opens for ever. The link keeps its
        // row — it is really there — but the walk does not descend through it.
        let root = tempdir();
        let inner = root.join("inner");
        fs::create_dir(&inner).unwrap();
        std::os::unix::fs::symlink(&root, inner.join("loop")).unwrap();

        let mut tree = Tree::new(&root);
        tree.expand(&inner);
        tree.expand(&inner.join("loop"));
        tree.expand(&inner.join("loop/inner"));
        let rows = tree.rows();
        assert!(rows.iter().any(|row| row.name == "loop"));
        assert!(rows.len() < 10);
    }

    // ---- Previewing -------------------------------------------------------

    #[test]
    fn text_and_binary_are_told_apart_by_a_nul_byte() {
        assert!(!looks_binary(b"fn main() {}\n"));
        assert!(looks_binary(b"\x7fELF\x02\x01\x01\0"));
        assert!(!looks_binary(&[]));
    }

    #[test]
    fn a_file_reads_as_its_lines_and_an_empty_one_says_so() {
        let dir = tempdir();
        fs::write(dir.join("a.txt"), b"one\ntwo\n").unwrap();
        fs::write(dir.join("empty.txt"), b"").unwrap();

        assert_eq!(
            preview(&dir.join("a.txt")),
            Preview::Text {
                lines: vec!["one".into(), "two".into()],
                truncated: false
            }
        );
        assert_eq!(preview(&dir.join("empty.txt")), Preview::Empty);
    }

    #[test]
    fn an_image_is_shown_rather_than_sniffed() {
        // By name and before the size check: a photograph is routinely past the
        // text budget and is exactly what a preview is for.
        let dir = tempdir();
        fs::write(dir.join("shot.PNG"), b"\x89PNG\r\n\x1a\n").unwrap();
        assert_eq!(preview(&dir.join("shot.PNG")), Preview::Image);
        assert!(is_image("a.jpeg"));
        assert!(!is_image("a.rs"));
        assert!(!is_image("png"));
    }

    #[test]
    fn a_binary_file_is_named_rather_than_drawn_as_mojibake() {
        let dir = tempdir();
        fs::write(dir.join("a.bin"), b"\x7fELF\0\0\0\0").unwrap();
        assert_eq!(preview(&dir.join("a.bin")), Preview::Binary);
    }

    #[test]
    fn a_file_past_the_budget_is_measured_rather_than_read() {
        let dir = tempdir();
        let big = dir.join("big.log");
        fs::write(&big, vec![b'x'; (MAX_PREVIEW_BYTES + 1) as usize]).unwrap();
        assert_eq!(preview(&big), Preview::TooLarge(MAX_PREVIEW_BYTES + 1));
    }

    #[test]
    fn a_very_long_file_is_cut_and_says_it_was() {
        let dir = tempdir();
        let long = dir.join("long.txt");
        let body: String = (0..MAX_PREVIEW_LINES + 50).map(|i| format!("{i}\n")).collect();
        fs::write(&long, body).unwrap();
        match preview(&long) {
            Preview::Text { lines, truncated } => {
                assert_eq!(lines.len(), MAX_PREVIEW_LINES);
                assert!(truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    // ---- URLs -------------------------------------------------------------

    #[test]
    fn a_path_survives_a_round_trip_through_a_file_url() {
        for path in [
            "/home/pascal/note.txt",
            "/home/pascal/my notes/a b.txt",
            "/tmp/100% done/#1.rs",
        ] {
            let url = file_url(Path::new(path));
            assert!(!url.contains(' '), "{url} still has a space in it");
            assert_eq!(path_from_url(&url), Some(PathBuf::from(path)));
        }
    }

    #[test]
    fn a_url_that_is_not_a_local_file_has_no_path_to_copy() {
        // A link dragged from a browser. There is nothing on disk behind it, so
        // it must be declined rather than turned into a file named `https:`.
        assert_eq!(path_from_url("https://example.com/a.txt"), None);
        // `file://host/path` names a file on another machine.
        assert_eq!(path_from_url("file://server/share/a.txt"), None);
        assert_eq!(
            path_from_url("file:///srv/a.txt"),
            Some(PathBuf::from("/srv/a.txt"))
        );
    }

    #[test]
    fn a_uri_list_drops_the_lines_that_are_not_local_files() {
        let payload = "# comment\nfile:///a/one.txt\n\nhttps://example.com\nfile:///a/two%20x.txt\n";
        assert_eq!(
            paths_from_uri_list(payload),
            [PathBuf::from("/a/one.txt"), PathBuf::from("/a/two x.txt")]
        );
    }

    // ---- Sizes ------------------------------------------------------------

    #[test]
    fn sizes_read_the_way_a_file_manager_writes_them() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        // Past ten the decimal stops earning its character.
        assert_eq!(format_size(20 * 1024), "20 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    // ---- A scratch directory ----------------------------------------------

    /// A fresh directory under the system temp folder, unique per call.
    ///
    /// Hand-rolled rather than a dev-dependency: the crate has none, and this
    /// is the whole of what one would be pulled in for.
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "pupo-files-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
