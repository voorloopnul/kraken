//! The file explorer's tree, and the rules a copy in or out of it obeys.
//!
//! The tree is the shape an editor's is: one row per entry, directories before
//! files, and a directory's children indented under it only while it is open.
//! Expansion is remembered per directory rather than per row, so a collapse and
//! a re-expand come back to the same open branches, and a refresh that finds a
//! new file leaves every other branch where the reader left it.
//!
//! Directories are read lazily — opening one is one listing, and the tree never
//! walks what nobody has looked at. That is the difference between a panel that
//! opens instantly on a repository with a `target/` in it and one that appears
//! to hang.
//!
//! The tree itself does no I/O at all. It holds what has been read and says
//! what it still wants ([`Tree::view`]); the caller fetches that and hands it
//! back ([`Tree::deliver`]). This is not ceremony — for a workspace on another
//! machine every listing is an SSH round trip, and a tree that read directories
//! from inside a property getter would freeze the window on each one. The same
//! seam makes the whole tree testable by handing it invented listings.
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
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::remote::{self, RemoteTarget};
use crate::util::shell_quote;

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
// Colours
// ---------------------------------------------------------------------------

/// What a row is drawn in, by what it is: `dir`, `exec`, `file`.
///
/// Three kinds and no more. Everything else about a row is already said by its
/// shape — the indent, the twisty, the icon, the leaning name of a symlink —
/// and colour is spent on the one question the shape does not answer: is this a
/// folder, something that runs, or a file to read. The blue and the green are
/// the two the diff pane already marks a moved and an added file with, so the
/// same two colours mean the same two things in both panes, and a plain file
/// keeps the pane's own text colour so most of the tree stays quiet.
pub fn kind_color(theme: &str, key: &str) -> &'static str {
    let dark = theme == "dark";
    match key {
        "dir" => if dark { "#61afef" } else { "#0184bc" },
        "exec" => if dark { "#98c379" } else { "#50a14f" },
        "file" => if dark { "#c8cad0" } else { "#4a4d55" },
        _ => "#ff00ff",
    }
}

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
    /// Whether it runs: an execute bit set on whatever the name resolves to.
    /// Asked only of files — on a directory the same bit means "may be entered"
    /// and every readable one has it, so colouring by it would light up the
    /// whole tree.
    pub executable: bool,
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

/// One directory's entries, sorted, hidden ones included.
///
/// Filtering happens in the walk rather than here, so that showing dotfiles is
/// a redraw rather than a re-read. On a remote workspace that is the difference
/// between a toggle and a round trip.
///
/// An unreadable directory is an error rather than an empty one — the panel
/// says which it was, because "no permission" and "nothing in it" look
/// identical in a list and mean opposite things.
pub fn read_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        // `file_type` is the cheap one — it comes from the directory read
        // itself and does not follow the link. `metadata` does follow it, which
        // is what makes a symlink to a folder open like a folder.
        let link = item
            .file_type()
            .map(|kind| kind.is_symlink())
            .unwrap_or(false);
        let (is_dir, size, executable) = match item.metadata() {
            Ok(meta) => (
                meta.is_dir(),
                meta.len(),
                meta.permissions().mode() & 0o111 != 0,
            ),
            // A broken symlink: it has a name and nothing behind it. Listing it
            // as a zero-byte file is the honest row — dropping it would leave a
            // name visible in every other tool missing from this one.
            Err(_) => (false, 0, false),
        };
        entries.push(Entry {
            name,
            is_dir,
            size,
            symlink: link,
            executable,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Where a tree lives
// ---------------------------------------------------------------------------

/// The machine a workspace's files are on.
///
/// The two differ in one thing only — how a directory is listed and how bytes
/// are moved — so everything above this (the tree, the sort, the collision
/// rules, the naming) is written once and does not know which it is looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Local,
    Remote(Box<RemoteTarget>),
}

impl Default for Place {
    fn default() -> Self {
        Place::Local
    }
}

impl Place {
    /// The place a workspace key names: a remote one when the key is a remote
    /// workspace's anchor, local otherwise. The same lookup `GitRunner` does.
    pub fn for_workspace(path: &str) -> Self {
        match remote::resolve(path) {
            Some(target) => Place::Remote(Box::new(target)),
            None => Place::Local,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Place::Remote(_))
    }

    /// The directory a tree rooted at this workspace starts from. A remote
    /// workspace's key is a local stand-in folder; the files are at the path on
    /// the far side.
    pub fn root_for(&self, workspace: &str) -> PathBuf {
        match self {
            Place::Local => PathBuf::from(workspace),
            Place::Remote(target) => PathBuf::from(&target.path),
        }
    }
}

/// How long a remote listing may take before it is given up on.
///
/// A listing is a bounded thing — one `find` at one level — so a wall-clock cap
/// is a fair question to ask of it, and the pane has to be able to say "this is
/// not answering" rather than showing a branch that never opens. Generous
/// because the first one also pays for opening the SSH connection; every one
/// after it rides the multiplexed channel and returns in milliseconds.
pub const REMOTE_LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// The shell command that lists one directory on the far side.
///
/// One `find`, so one round trip. The fields are whether the entry runs, the
/// entry's own type, the type of whatever it points at, the size and the
/// basename, NUL-terminated per entry — a name may contain a newline or a tab,
/// and NUL is the one byte a path cannot hold.
///
/// `%y` is `l` for a symlink while `%Y` is what it resolves to, which is how a
/// link to a folder comes to open like a folder and a broken one still gets a
/// row.
///
/// The execute flag is a predicate rather than a `%m` field on purpose: `-perm`
/// and `%m` both describe the link itself, and a symlink's own mode is `777` —
/// every link in the tree would come back executable. `-executable` asks about
/// what the name resolves to, which is what the row is about.
pub fn list_command(dir: &Path) -> String {
    format!(
        "find {} -mindepth 1 -maxdepth 1 \\( -executable -printf 'x' -o -printf '-' \\) \
         -printf '\\t%y\\t%Y\\t%s\\t%f\\0'",
        shell_quote(&dir.to_string_lossy())
    )
}

/// The entries in what [`list_command`] printed.
///
/// Anything malformed is dropped rather than failing the listing: one entry the
/// remote `find` could not describe should cost that row, not the directory.
pub fn parse_listing(stdout: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for record in stdout.split('\0') {
        if record.is_empty() {
            continue;
        }
        // Five fields, and the name is last because it is the one that can
        // contain a tab.
        let mut fields = record.splitn(5, '\t');
        let (Some(runs), Some(kind), Some(target), Some(size), Some(name)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        entries.push(Entry {
            name: name.to_string(),
            is_dir: target == "d",
            size: size.parse().unwrap_or(0),
            symlink: kind == "l",
            executable: runs == "x",
        });
    }
    sort_entries(&mut entries);
    entries
}

/// List one directory, wherever it is.
///
/// The error is the sentence the panel shows, so it says what actually went
/// wrong rather than "failed": a directory that is not there and a host that
/// will not answer look the same in an empty list and are not the same problem.
pub fn list(place: &Place, dir: &Path) -> Result<Vec<Entry>, String> {
    match place {
        Place::Local => read_dir(dir).map_err(|error| error.to_string()),
        Place::Remote(target) => {
            let argv = target.ssh_argv(&list_command(dir), false);
            // `run_argv` is git's only by where it lives; it is a plain timed
            // process runner and this is the second thing that wants one.
            match crate::git::run_argv(&argv, REMOTE_LIST_TIMEOUT) {
                Some(output) if output.ok() => Ok(parse_listing(&output.stdout)),
                Some(output) => Err(remote_error(&output.stderr)),
                None => Err("The host did not answer.".to_string()),
            }
        }
    }
}

/// A remote command's stderr, cut down to something a one-line footer can hold.
fn remote_error(stderr: &str) -> String {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("The command failed on the remote host.");
    if line.len() > 200 {
        format!("{}…", &line[..line.floor_char_boundary(200)])
    } else {
        line.to_string()
    }
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
    /// Open, but its listing has not arrived yet. Only ever true for long
    /// enough to see on a remote workspace, which is exactly where it matters.
    pub loading: bool,
    pub size: u64,
    pub symlink: bool,
    /// Whether the file runs. Always false for a directory; see [`Entry`].
    pub executable: bool,
}

/// Why a directory has no rows under it, when it is open and shows none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Empty {
    /// Nothing in it at all.
    Nothing,
    /// Nothing in it but hidden entries, with hidden entries turned off. Worth
    /// telling apart from the above: the toggle that fixes it is right there.
    HiddenOnly,
    /// It could not be read, and why.
    Unreadable(String),
}

/// One directory the tree wants read, and the state of the tree when it asked.
///
/// The generation is what keeps a slow listing honest. A remote `find` can take
/// seconds, and in that time the reader can switch workspace or hit refresh;
/// the answer to the old question must not be filed as the answer to the new
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Load {
    pub dir: PathBuf,
    pub generation: u64,
}

/// What the panel draws, and what it must fetch before it can draw more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub rows: Vec<Row>,
    pub wanted: Vec<Load>,
}

/// The workspace's tree: what is open, and what has been read.
///
/// Does no I/O. See the module header — that is what lets the same type serve a
/// local folder and a machine at the end of an SSH connection, and what lets
/// every rule below be tested by handing it invented listings.
#[derive(Debug, Default)]
pub struct Tree {
    root: PathBuf,
    /// Directories the reader has opened. Kept across a refresh and across a
    /// collapse, which is what makes re-opening a branch return to it rather
    /// than to a closed copy of it.
    expanded: BTreeSet<PathBuf>,
    /// Listings as they arrived, hidden entries and all — see [`read_dir`].
    children: HashMap<PathBuf, Vec<Entry>>,
    unreadable: HashMap<PathBuf, String>,
    /// Directories already asked for, so a slow listing is not asked for again
    /// on every repaint.
    requested: BTreeSet<PathBuf>,
    show_hidden: bool,
    /// Bumped whenever what has been read stops being valid.
    generation: u64,
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

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Point the tree at another folder. Everything read and everything open
    /// belongs to the old one, so all of it goes — and the generation moves, so
    /// a listing still in flight for the old root is discarded when it lands.
    pub fn set_root(&mut self, root: impl Into<PathBuf>) {
        let root = root.into();
        if root == self.root {
            return;
        }
        self.root = root;
        self.expanded.clear();
        self.forget();
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Show or hide dotfiles.
    ///
    /// A redraw, not a re-read: listings are kept whole and filtered in the
    /// walk. On a remote workspace re-reading would be a round trip per open
    /// branch, for a question already answered.
    pub fn set_show_hidden(&mut self, show: bool) {
        self.show_hidden = show;
    }

    /// Re-read every directory, keeping the branches that are open. This is
    /// what a refresh and a completed copy both do: the reader's place in the
    /// tree is not something either of them has any business resetting.
    pub fn refresh(&mut self) {
        self.forget();
    }

    fn forget(&mut self) {
        self.children.clear();
        self.unreadable.clear();
        self.requested.clear();
        self.generation += 1;
        self.truncated = false;
    }

    /// Forget one directory's listing, so the next view asks for it again.
    pub fn invalidate(&mut self, dir: &Path) {
        self.children.remove(dir);
        self.unreadable.remove(dir);
        self.requested.remove(dir);
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

    /// File a listing the caller fetched.
    ///
    /// A load from an earlier generation is dropped: it answers a question
    /// about a workspace or a state of the tree that is no longer on screen.
    pub fn deliver(&mut self, load: &Load, outcome: Result<Vec<Entry>, String>) {
        if load.generation != self.generation {
            return;
        }
        self.requested.remove(&load.dir);
        match outcome {
            Ok(mut entries) => {
                sort_entries(&mut entries);
                self.unreadable.remove(&load.dir);
                self.children.insert(load.dir.clone(), entries);
            }
            Err(reason) => {
                self.unreadable.insert(load.dir.clone(), reason);
                self.children.insert(load.dir.clone(), Vec::new());
            }
        }
    }

    /// Whether anything is still on its way. The panel says so rather than
    /// showing a tree that is quietly incomplete.
    pub fn loading(&self) -> bool {
        !self.requested.is_empty()
    }

    /// Why an open directory shows nothing, or `None` when it has rows or has
    /// not been read yet — nothing is known about an unread directory, and
    /// "empty" would be a claim.
    pub fn empty_reason(&self, dir: &Path) -> Option<Empty> {
        if let Some(reason) = self.unreadable.get(dir) {
            return Some(Empty::Unreadable(reason.clone()));
        }
        let entries = self.children.get(dir)?;
        if entries.is_empty() {
            return Some(Empty::Nothing);
        }
        if !self.show_hidden && entries.iter().all(|entry| is_hidden(&entry.name)) {
            return Some(Empty::HiddenOnly);
        }
        None
    }

    /// Whether the last walk hit [`MAX_ROWS`] and stopped short.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The visible rows, and the directories that have to be read before any
    /// more of them exist.
    ///
    /// Asking marks them asked, so a listing that takes a while is fetched once
    /// rather than started again on every repaint.
    pub fn view(&mut self) -> View {
        let mut view = self.look();
        view.wanted.retain(|load| !self.requested.contains(&load.dir));
        for load in &view.wanted {
            self.requested.insert(load.dir.clone());
        }
        view
    }

    /// The rows as they stand, asking for nothing.
    ///
    /// Deliberately free of [`view`](Self::view)'s side effect: a repaint that
    /// only wants to draw should not be able to start an SSH round trip, and a
    /// test that reads the tree should not consume the request it is about to
    /// assert on.
    pub fn rows(&mut self) -> Vec<Row> {
        self.look().rows
    }

    /// Walk the cache: the rows it can draw, and every directory it would need
    /// read to draw more. Marks nothing.
    fn look(&mut self) -> View {
        self.truncated = false;
        let mut view = View::default();
        if self.root.as_os_str().is_empty() {
            return view;
        }
        let root = self.root.clone();
        self.want(&root, &mut view);
        // Guards against a symlink loop: a link pointing at one of its own
        // ancestors would otherwise be a branch that can be opened for ever.
        let mut open: Vec<PathBuf> = vec![root.clone()];
        self.walk(&root, 0, &mut open, &mut view);
        view
    }

    /// Note that `dir` has not been read. Whether it has also already been
    /// asked for is [`view`](Self::view)'s business, not this walk's.
    fn want(&mut self, dir: &Path, view: &mut View) {
        if self.children.contains_key(dir) {
            return;
        }
        view.wanted.push(Load {
            dir: dir.to_path_buf(),
            generation: self.generation,
        });
    }

    fn walk(&mut self, dir: &Path, depth: usize, open: &mut Vec<PathBuf>, view: &mut View) {
        let entries = match self.children.get(dir) {
            Some(entries) => entries.clone(),
            // Not read yet. Its own row already says it is loading; there is
            // nothing under it to draw until the listing lands.
            None => return,
        };
        for entry in entries {
            if !self.show_hidden && is_hidden(&entry.name) {
                continue;
            }
            if view.rows.len() >= MAX_ROWS {
                self.truncated = true;
                return;
            }
            let path = dir.join(&entry.name);
            let expanded = entry.is_dir && self.expanded.contains(&path);
            let loading = expanded && !self.children.contains_key(&path);
            view.rows.push(Row {
                path: path.clone(),
                name: entry.name,
                depth,
                is_dir: entry.is_dir,
                expanded,
                loading,
                size: entry.size,
                symlink: entry.symlink,
                executable: !entry.is_dir && entry.executable,
            });
            if !expanded {
                continue;
            }
            // A link back up the branch we are standing on. Its row stays —
            // the link is really there — but it is not followed. Only local
            // paths can be resolved here; a remote loop is caught by the row
            // cap instead, which is the honest limit when the filesystem is on
            // the other side of a wire.
            let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if open.contains(&real) {
                continue;
            }
            self.want(&path, view);
            open.push(real);
            self.walk(&path, depth + 1, open, view);
            open.pop();
        }
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

/// The names among `sources` that are already in `taken`, in the order given.
///
/// Off a list rather than the filesystem, because the destination may be on
/// another machine — one listing answers this for the whole batch, where one
/// `exists` per name would be one round trip per name.
pub fn collisions_among(sources: &[PathBuf], taken: &[String]) -> Vec<String> {
    sources
        .iter()
        .filter_map(|source| source.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| taken.iter().any(|entry| entry == name))
        .collect()
}

/// The names among `sources` that a local `dest_dir` already has.
pub fn collisions(sources: &[PathBuf], dest_dir: &Path) -> Vec<String> {
    let taken: Vec<String> = read_dir(dest_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    collisions_among(sources, &taken)
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

/// A name not in `taken`, counting up from `name`.
///
/// Takes the list rather than the directory because a remote directory cannot
/// be asked "does this exist" cheaply — it has already been listed, and one
/// listing is the round trip that answers this for every name in the batch.
pub fn unique_name_among(taken: &[String], name: &str) -> String {
    let free = |candidate: &str| !taken.iter().any(|entry| entry == candidate);
    if free(name) {
        return name.to_string();
    }
    for n in 2..1000 {
        let candidate = numbered_name(name, n);
        if free(&candidate) {
            return candidate;
        }
    }
    // A thousand copies of one name is not a case worth a cleverer answer, but
    // it still must not return a name that is taken.
    numbered_name(name, std::process::id())
}

/// A name for `name` inside a local `dir` that is not taken.
pub fn unique_name(dir: &Path, name: &str) -> String {
    let taken: Vec<String> = read_dir(dir)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    unique_name_among(&taken, name)
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

/// The checks that still mean something when the two ends are on different
/// machines.
///
/// Only reach: a copy into the workspace has to land inside it, and that is a
/// question about the shape of the path rather than about what is on any disk.
/// The rest of [`check_copy`] cannot apply — a local process cannot stat a
/// remote directory, and "a folder inside itself" is not a thing two machines
/// can be, however alike their paths look.
pub fn check_across(source: &Path, dest_dir: &Path, root: Option<&Path>) -> Result<(), Refusal> {
    if !source.exists() {
        return Err(Refusal::Missing);
    }
    if let Some(root) = root {
        if !within(root, dest_dir) {
            return Err(Refusal::OutsideWorkspace);
        }
    }
    Ok(())
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
        // A transfer over SSH is a tar stream and reports no counts, so zero
        // means "not measured" rather than "nothing" — and a single file has
        // nothing worth measuring either way.
        let head = if self.files <= 1 {
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
// Moving bytes to and from another machine
// ---------------------------------------------------------------------------

/// A copy across the wire is a `tar` stream through the same `ssh` invocation
/// everything else on the remote goes through, so it rides the multiplexed
/// connection that is already open and needs no `scp` on either side.
///
/// `-h` dereferences symlinks, which matches what a local copy does: a link
/// into the source tree would dangle once the copy is elsewhere, and a link out
/// of it would be a surprise in a folder the reader thinks they own outright.
pub fn tar_send_argv(parent: &Path, name: &str) -> Vec<String> {
    vec![
        "tar".into(),
        "-chf".into(),
        "-".into(),
        "-C".into(),
        parent.to_string_lossy().into_owned(),
        "--".into(),
        name.to_string(),
    ]
}

/// The same, as a shell command for the far side.
pub fn tar_send_command(parent: &Path, name: &str) -> String {
    format!(
        "tar -chf - -C {} -- {}",
        shell_quote(&parent.to_string_lossy()),
        shell_quote(name)
    )
}

/// The local argv that unpacks a stream into `into`.
pub fn tar_receive_argv(into: &Path) -> Vec<String> {
    vec![
        "tar".into(),
        "-xf".into(),
        "-".into(),
        "-C".into(),
        into.to_string_lossy().into_owned(),
    ]
}

/// The shell command that lands an incoming stream in `dest_dir` under
/// `final_name`.
///
/// It unpacks into a staging directory beside the destination and moves the
/// result into place, for two reasons. A rename within one directory is atomic,
/// so a reader watching the tree never sees a half-written folder appear under
/// the name they are waiting for. And it is what lets the copy land under a
/// name of our choosing — `tar` extracts whatever name the archive carries, and
/// "keep both" needs a different one.
///
/// The staging directory is removed whether or not the unpack worked, and the
/// exit status carried through, so a failure is a failure rather than a
/// half-finished folder nobody knows about.
pub fn tar_receive_command(
    dest_dir: &Path,
    name: &str,
    final_name: &str,
    mode: OnCollision,
) -> String {
    let dest = shell_quote(&dest_dir.to_string_lossy());
    let landed = shell_quote(&dest_dir.join(final_name).to_string_lossy());
    let clear = match mode {
        OnCollision::Replace => format!("rm -rf -- {landed} && "),
        OnCollision::KeepBoth => String::new(),
    };
    format!(
        "stage=$(mktemp -d {dest}/.kraken-XXXXXX) || exit 1; \
         {{ tar -xf - -C \"$stage\" && {clear}mv -- \"$stage\"/{} {landed}; }}; \
         rc=$?; rm -rf \"$stage\"; exit $rc",
        shell_quote(name)
    )
}

/// Run `producer`, feeding its stdout to `consumer`, and wait for both.
///
/// There is no deadline here on purpose. A transfer's length is its size over
/// the link's speed and neither is knowable from here, so any number picked
/// would be wrong for somebody — and it would be wrong in the worst direction,
/// killing a copy that was working. What ends a *wedged* transfer is ssh's own
/// keepalive (see `SshHost::ssh_base_args`): about a minute of silence and the
/// connection drops, the pipe closes, and both ends fall out of their waits.
/// That measures the thing actually worth measuring — that nothing is moving —
/// rather than how long the job has been running.
///
/// The producer's failure is reported before the consumer's: a `tar` that could
/// not read the source and an `ssh` that got no bytes are the same event, and
/// the first of them is the one that says what actually went wrong.
fn pipe(producer: &[String], consumer: &[String]) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let (head, rest) = producer.split_first().ok_or("nothing to run")?;
    let mut source = Command::new(head)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{head}: {error}"))?;
    let stream = source.stdout.take().ok_or("no stream to read")?;

    let (head, rest) = consumer.split_first().ok_or("nothing to run")?;
    let sink = Command::new(head)
        .args(rest)
        .stdin(Stdio::from(stream))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{head}: {error}"))?;

    // The source is drained by the sink and ends when the pipe closes, so
    // waiting on the sink first is what lets the pair finish in either order.
    let sink = sink
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let source = source
        .wait_with_output()
        .map_err(|error| error.to_string())?;

    if !source.status.success() {
        return Err(remote_error(&String::from_utf8_lossy(&source.stderr)));
    }
    if !sink.status.success() {
        return Err(remote_error(&String::from_utf8_lossy(&sink.stderr)));
    }
    Ok(())
}

/// Copy a local file or folder into a directory on the remote host.
///
/// `taken` is what the destination directory already holds, which the caller
/// has listed anyway to find the collision in the first place.
pub fn copy_up(
    target: &RemoteTarget,
    source: &Path,
    dest_dir: &Path,
    taken: &[String],
    mode: OnCollision,
) -> Result<CopyReport, Refusal> {
    if !source.exists() {
        return Err(Refusal::Missing);
    }
    let (parent, name) = split_source(source)?;
    let final_name = match mode {
        OnCollision::KeepBoth => unique_name_among(taken, &name),
        OnCollision::Replace => name.clone(),
    };
    let replaced = mode == OnCollision::Replace && taken.contains(&final_name);

    let command = tar_receive_command(dest_dir, &name, &final_name, mode);
    let mut report = CopyReport {
        destination: dest_dir.join(&final_name),
        replaced,
        ..Default::default()
    };
    if let Err(reason) = pipe(
        &tar_send_argv(&parent, &name),
        &target.ssh_argv(&command, false),
    ) {
        report.failures.push(reason);
        report.replaced = false;
    }
    Ok(report)
}

/// Copy a file or folder from the remote host into a local directory.
///
/// The bytes land in a staging directory next to the destination and are then
/// placed by [`copy_into`], so the naming and replacing rules that apply to a
/// local copy apply to this one too — written once, tested once.
pub fn copy_down(
    target: &RemoteTarget,
    source: &Path,
    dest_dir: &Path,
    mode: OnCollision,
) -> Result<CopyReport, Refusal> {
    if !dest_dir.is_dir() {
        return Err(Refusal::NotADirectory);
    }
    let (parent, name) = split_source(source)?;

    let stage = dest_dir.join(format!(".kraken-stage-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    if let Err(error) = fs::create_dir_all(&stage) {
        return Ok(CopyReport {
            destination: dest_dir.join(&name),
            failures: vec![format!("{}: {error}", stage.display())],
            ..Default::default()
        });
    }

    let outcome = pipe(
        &target.ssh_argv(&tar_send_command(&parent, &name), false),
        &tar_receive_argv(&stage),
    );
    let report = match outcome {
        Err(reason) => CopyReport {
            destination: dest_dir.join(&name),
            failures: vec![reason],
            ..Default::default()
        },
        // Placed by the local rules, which is the whole reason for staging.
        Ok(()) => copy_into(&stage.join(&name), dest_dir, None, mode)
            .unwrap_or_else(|refusal| CopyReport {
                destination: dest_dir.join(&name),
                failures: vec![refusal.message().to_string()],
                ..Default::default()
            }),
    };
    let _ = fs::remove_dir_all(&stage);
    Ok(report)
}

/// A source split into the directory holding it and its own name — what `tar`
/// needs, and what a path with no name at all (a bare `/`) has none of.
fn split_source(source: &Path) -> Result<(PathBuf, String), Refusal> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or(Refusal::Missing)?;
    let parent = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    Ok((parent, name))
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

/// Read a file on the far side for the sheet.
///
/// `size` and `is_dir` come from the row the reader clicked, which the tree
/// already listed — so this is one round trip, not a `stat` followed by a
/// `cat`. Nothing but the bytes has to cross the wire.
pub fn preview_remote(target: &RemoteTarget, path: &Path, size: u64, is_dir: bool) -> Preview {
    if is_dir {
        return Preview::Unreadable("That is a folder.".to_string());
    }
    if size == 0 {
        return Preview::Empty;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Decided by name and before the size check, exactly as locally: a
    // photograph is routinely past the text budget and is what a preview is for.
    if is_image(&name) {
        return Preview::Image;
    }
    if size > MAX_PREVIEW_BYTES {
        return Preview::TooLarge(size);
    }

    let command = format!(
        "head -c {MAX_PREVIEW_BYTES} -- {}",
        shell_quote(&path.to_string_lossy())
    );
    let argv = target.ssh_argv(&command, false);
    let output = match crate::git::run_argv(&argv, REMOTE_LIST_TIMEOUT) {
        Some(output) if output.ok() => output.stdout,
        Some(output) => return Preview::Unreadable(remote_error(&output.stderr)),
        None => return Preview::Unreadable("The host did not answer.".to_string()),
    };
    // NUL survives the lossy decode — it is valid UTF-8 — so the same test that
    // sorts text from binary locally works on what came back over the wire.
    if output.chars().take(SNIFF_BYTES).any(|ch| ch == '\0') {
        return Preview::Binary;
    }
    let mut lines: Vec<String> = output.lines().map(str::to_string).collect();
    let truncated = lines.len() > MAX_PREVIEW_LINES;
    lines.truncate(MAX_PREVIEW_LINES);
    Preview::Text { lines, truncated }
}

/// Copy one remote file to a local path, byte for byte.
///
/// For the one thing the sheet cannot render from text: an image has to exist
/// as a file before anything can draw it.
pub fn fetch_to(target: &RemoteTarget, source: &Path, into: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};

    if let Some(parent) = into.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = fs::File::create(into).map_err(|error| format!("{}: {error}", into.display()))?;
    let argv = target.ssh_argv(
        &format!("cat -- {}", shell_quote(&source.to_string_lossy())),
        false,
    );
    let (head, rest) = argv.split_first().ok_or("nothing to run")?;
    let child = Command::new(head)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{head}: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let _ = fs::remove_file(into);
    Err(remote_error(&String::from_utf8_lossy(&output.stderr)))
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
            executable: false,
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

    /// Do what the panel does: ask the tree what it wants, read it, hand it
    /// back, and go round again until it wants nothing more.
    ///
    /// The tree does no I/O of its own, so nothing in it draws until something
    /// plays this part. Here that is the local filesystem; in the app it is a
    /// worker thread that may be talking to another machine.
    fn settle(tree: &mut Tree) -> Vec<Row> {
        for _ in 0..64 {
            let view = tree.view();
            if view.wanted.is_empty() {
                return view.rows;
            }
            for load in &view.wanted {
                tree.deliver(load, list(&Place::Local, &load.dir));
            }
        }
        panic!("tree never settled");
    }

    #[test]
    fn a_closed_tree_shows_only_the_top_level() {
        let mut tree = Tree::new(sample_tree());
        assert_eq!(
            names(&settle(&mut tree)),
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
            names(&settle(&mut tree)),
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
        assert_eq!(settle(&mut tree).len(), 6);

        tree.collapse(&root.join("src"));
        assert_eq!(settle(&mut tree).len(), 3);

        tree.expand(&root.join("src"));
        assert_eq!(
            names(&settle(&mut tree)),
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
        assert!(!settle(&mut tree).iter().any(|row| row.name == ".hidden"));
        tree.set_show_hidden(true);
        assert!(settle(&mut tree).iter().any(|row| row.name == ".hidden"));
    }

    #[test]
    fn a_refresh_finds_a_new_file_and_leaves_the_open_branches_alone() {
        let root = sample_tree();
        let mut tree = Tree::new(&root);
        tree.expand(&root.join("src"));
        assert_eq!(settle(&mut tree).len(), 5);

        fs::write(root.join("src/added.rs"), b"new").unwrap();
        // Without the refresh the listing is the cached one, which is the whole
        // point of caching it.
        assert_eq!(settle(&mut tree).len(), 5);

        tree.refresh();
        let rows = settle(&mut tree);
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
        assert!(settle(&mut tree).iter().any(|row| row.name == "deep.rs"));
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
        settle(&mut tree);
        assert_eq!(tree.empty_reason(&hollow), Some(Empty::Nothing));

        // A directory that was never read is not "empty" — nothing is known
        // about it yet, and saying "empty" would be a claim.
        let mut fresh = Tree::new(&root);
        assert_eq!(fresh.empty_reason(&hollow), None);
        settle(&mut fresh);
        assert_eq!(fresh.empty_reason(&hollow), None);
    }

    #[test]
    fn changing_the_root_forgets_everything_about_the_old_one() {
        let first = sample_tree();
        let second = tempdir();
        let mut tree = Tree::new(&first);
        tree.expand(&first.join("src"));
        assert_eq!(settle(&mut tree).len(), 5);

        tree.set_root(&second);
        assert!(settle(&mut tree).is_empty());
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
        let rows = settle(&mut tree);
        assert!(rows.iter().any(|row| row.name == "loop"));
        assert!(rows.len() < 10);
    }

    #[test]
    fn a_file_that_runs_is_told_apart_from_one_that_does_not() {
        // What the pane colours a row from. A directory carries the same
        // execute bit — it is how "may be entered" is written — so a folder
        // must not come back marked, or the whole tree would be green.
        let root = tempdir();
        fs::create_dir(root.join("bin")).unwrap();
        fs::write(root.join("build.sh"), b"#!/bin/sh\n").unwrap();
        fs::write(root.join("notes.txt"), b"hi").unwrap();
        fs::set_permissions(root.join("build.sh"), fs::Permissions::from_mode(0o755)).unwrap();

        let rows = settle(&mut Tree::new(&root));
        let runs = |name: &str| rows.iter().find(|row| row.name == name).unwrap().executable;
        assert!(runs("build.sh"));
        assert!(!runs("notes.txt"));
        assert!(!runs("bin"));
    }

    // ---- Listing another machine ------------------------------------------

    /// One record as the remote `find` prints it, for something that does not
    /// run.
    fn record(kind: &str, target: &str, size: &str, name: &str) -> String {
        flagged("-", kind, target, size, name)
    }

    /// The same, for one the far side reported as executable.
    fn runnable(kind: &str, target: &str, size: &str, name: &str) -> String {
        flagged("x", kind, target, size, name)
    }

    fn flagged(runs: &str, kind: &str, target: &str, size: &str, name: &str) -> String {
        format!("{runs}\t{kind}\t{target}\t{size}\t{name}\0")
    }

    #[test]
    fn a_remote_listing_reads_types_sizes_and_names() {
        let stdout = format!(
            "{}{}{}",
            record("d", "d", "4096", "src"),
            record("f", "f", "1234", "main.rs"),
            record("f", "f", "0", "empty"),
        );
        let entries = parse_listing(&stdout);
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.name.as_str(), e.is_dir, e.size))
                .collect::<Vec<_>>(),
            [("src", true, 4096), ("empty", false, 0), ("main.rs", false, 1234)]
        );
    }

    #[test]
    fn a_remote_symlink_is_typed_by_what_it_points_at() {
        // %y is the link, %Y is its target. A link to a folder has to open like
        // a folder, and a broken one still gets a row.
        let stdout = format!(
            "{}{}",
            record("l", "d", "7", "vendor"),
            record("l", "N", "9", "dangling"),
        );
        let entries = parse_listing(&stdout);
        let vendor = entries.iter().find(|e| e.name == "vendor").unwrap();
        assert!(vendor.is_dir);
        assert!(vendor.symlink);
        let dangling = entries.iter().find(|e| e.name == "dangling").unwrap();
        assert!(!dangling.is_dir);
        assert!(dangling.symlink);
    }

    #[test]
    fn a_remote_listing_says_which_entries_run() {
        // The flag is `-executable`, which asks about what the name resolves
        // to: a link to a script is marked, a link to a text file is not.
        let stdout = format!(
            "{}{}{}",
            record("f", "f", "10", "notes.txt"),
            runnable("f", "f", "20", "build.sh"),
            runnable("l", "f", "8", "link-to-build"),
        );
        let entries = parse_listing(&stdout);
        let runs = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .executable
        };
        assert!(runs("build.sh"));
        assert!(runs("link-to-build"));
        assert!(!runs("notes.txt"));
    }

    #[test]
    fn a_remote_name_may_hold_a_tab_or_a_newline() {
        // Which is why the records are NUL-terminated and the name is the last
        // field: NUL is the one byte a path cannot contain.
        let stdout = format!(
            "{}{}",
            record("f", "f", "1", "od\td name"),
            record("f", "f", "2", "two\nlines"),
        );
        let names: Vec<String> = parse_listing(&stdout)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"od\td name".to_string()));
        assert!(names.contains(&"two\nlines".to_string()));
    }

    #[test]
    fn a_malformed_record_costs_its_own_row_and_no_more() {
        let stdout = format!(
            "{}{}{}",
            record("f", "f", "1", "good.rs"),
            "nonsense-with-no-fields\0",
            record("f", "f", "2", "also-good.rs"),
        );
        let names: Vec<String> = parse_listing(&stdout)
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, ["also-good.rs", "good.rs"]);
    }

    #[test]
    fn the_listing_command_quotes_the_directory_it_is_given() {
        // It is pasted into a shell command line on the far side, so a space or
        // a quote in the path must not end the argument.
        let command = list_command(Path::new("/srv/my project"));
        assert!(command.contains("'/srv/my project'"), "{command}");
        assert!(command.contains("-maxdepth 1"));
        // NUL-terminated records, name last.
        assert!(command.contains("%f"));
        // The flag that dereferences, rather than a mode field that would
        // report every symlink's own 777.
        assert!(command.contains("-executable"), "{command}");
        assert!(!command.contains("%m"), "{command}");
    }

    // ---- Listings arriving late -------------------------------------------

    #[test]
    fn a_listing_from_before_a_refresh_is_dropped() {
        // A remote find can take seconds, and in that time the reader can hit
        // refresh or switch workspace. Filing the old answer under the new
        // question is how a tree comes to show another machine's folders.
        let mut tree = Tree::new("/w");
        let view = tree.view();
        let stale = view.wanted[0].clone();

        tree.refresh();
        tree.deliver(&stale, Ok(vec![entry("ghost", false)]));
        assert!(tree.rows().is_empty());

        // The re-asked load is a new generation, and that one lands.
        let view = tree.view();
        assert_eq!(view.wanted.len(), 1);
        assert!(view.rows.is_empty());
        assert_ne!(view.wanted[0].generation, stale.generation);
        tree.deliver(&view.wanted[0].clone(), Ok(vec![entry("real", false)]));
        assert_eq!(names(&tree.rows()), [(0, "real".to_string())]);
    }

    #[test]
    fn a_directory_is_asked_for_once_however_often_it_is_drawn() {
        // Otherwise every repaint starts another SSH round trip for a listing
        // that is already on its way.
        let mut tree = Tree::new("/w");
        assert_eq!(tree.view().wanted.len(), 1);
        assert!(tree.view().wanted.is_empty());
        assert!(tree.loading());
    }

    #[test]
    fn an_open_directory_says_it_is_loading_until_its_listing_lands() {
        let mut tree = Tree::new("/w");
        let load = tree.view().wanted[0].clone();
        tree.deliver(&load, Ok(vec![entry("src", true)]));
        tree.expand(Path::new("/w/src"));

        let view = tree.view();
        assert!(view.rows[0].loading, "the open row should say it is waiting");
        assert_eq!(view.wanted.len(), 1);
        assert_eq!(view.wanted[0].dir, PathBuf::from("/w/src"));

        tree.deliver(&view.wanted[0].clone(), Ok(vec![entry("main.rs", false)]));
        let rows = tree.rows();
        assert!(!rows[0].loading);
        assert_eq!(rows[1].name, "main.rs");
    }

    #[test]
    fn a_directory_that_would_not_be_read_says_why() {
        // "No permission" and "nothing in it" look identical in a list and mean
        // opposite things, and over SSH there is a third answer — the host did
        // not reply — that must not read as either.
        let mut tree = Tree::new("/w");
        let load = tree.view().wanted[0].clone();
        tree.deliver(&load, Err("The host did not answer.".into()));
        assert_eq!(
            tree.empty_reason(Path::new("/w")),
            Some(Empty::Unreadable("The host did not answer.".into()))
        );
    }

    #[test]
    fn showing_hidden_entries_is_a_redraw_rather_than_a_re_read() {
        // On a remote workspace a re-read is a round trip per open branch, for
        // a question that was already answered.
        let mut tree = Tree::new("/w");
        let load = tree.view().wanted[0].clone();
        tree.deliver(&load, Ok(vec![entry(".env", false), entry("main.rs", false)]));

        assert_eq!(names(&tree.rows()), [(0, "main.rs".to_string())]);
        assert_eq!(tree.empty_reason(Path::new("/w")), None);

        tree.set_show_hidden(true);
        // Nothing to fetch: the listing never left.
        assert!(tree.view().wanted.is_empty());
        assert_eq!(tree.rows().len(), 2);
    }

    #[test]
    fn a_folder_of_nothing_but_dotfiles_is_told_apart_from_an_empty_one() {
        let mut tree = Tree::new("/w");
        let load = tree.view().wanted[0].clone();
        tree.deliver(&load, Ok(vec![entry(".env", false)]));
        assert_eq!(tree.empty_reason(Path::new("/w")), Some(Empty::HiddenOnly));

        let mut bare = Tree::new("/b");
        let load = bare.view().wanted[0].clone();
        bare.deliver(&load, Ok(vec![]));
        assert_eq!(bare.empty_reason(Path::new("/b")), Some(Empty::Nothing));
    }

    // ---- Moving bytes across the wire -------------------------------------

    #[test]
    fn a_send_dereferences_links_so_a_copy_matches_a_local_one() {
        // -h. Without it the copy arrives full of links pointing at paths that
        // exist only on the machine it came from.
        let argv = tar_send_argv(Path::new("/w"), "src");
        assert_eq!(argv[0], "tar");
        assert!(argv.contains(&"-chf".to_string()));
        // `--` before the name, so a file called `-C` is a file and not a flag.
        let end = &argv[argv.len() - 2..];
        assert_eq!(end, ["--", "src"]);
    }

    #[test]
    fn a_send_command_quotes_both_the_directory_and_the_name() {
        let command = tar_send_command(Path::new("/srv/my project"), "notes' file");
        assert!(command.contains("'/srv/my project'"), "{command}");
        // The apostrophe has to survive being pasted into a shell command line.
        assert!(command.contains(r#"'notes'\'' file'"#), "{command}");
    }

    #[test]
    fn a_receive_stages_then_moves_so_a_half_written_folder_never_appears() {
        let command = tar_receive_command(
            Path::new("/w/docs"),
            "notes",
            "notes",
            OnCollision::KeepBoth,
        );
        // Staged beside the destination, so the move into place is a rename on
        // the same filesystem rather than a second copy.
        assert!(command.contains("mktemp -d /w/docs/.kraken-XXXXXX"), "{command}");
        assert!(command.contains("tar -xf - -C \"$stage\""), "{command}");
        assert!(command.contains("mv --"), "{command}");
        // Cleaned up and the status carried through, whether or not it worked.
        assert!(command.contains("rc=$?"), "{command}");
        assert!(command.contains("rm -rf \"$stage\""), "{command}");
        assert!(command.trim_end().ends_with("exit $rc"), "{command}");
        // Nothing is deleted unless replacing was asked for.
        assert!(!command.contains("rm -rf -- "), "{command}");
    }

    #[test]
    fn a_receive_that_replaces_clears_the_way_first() {
        let command = tar_receive_command(
            Path::new("/w"),
            "notes",
            "notes",
            OnCollision::Replace,
        );
        assert!(command.contains("rm -rf -- /w/notes"), "{command}");
    }

    #[test]
    fn keeping_both_lands_under_the_free_name_not_the_archived_one() {
        // tar extracts whatever name the archive carries, so the move is what
        // makes "keep both" possible at all on the far side.
        let command = tar_receive_command(
            Path::new("/w"),
            "notes.txt",
            "notes 2.txt",
            OnCollision::KeepBoth,
        );
        assert!(command.contains(r#""$stage"/notes.txt"#), "{command}");
        assert!(command.contains("'/w/notes 2.txt'"), "{command}");
    }

    #[test]
    fn a_free_remote_name_is_left_alone_and_a_taken_one_counts_up() {
        let taken = vec!["report.txt".to_string(), "report 2.txt".to_string()];
        assert_eq!(unique_name_among(&taken, "notes.txt"), "notes.txt");
        assert_eq!(unique_name_among(&taken, "report.txt"), "report 3.txt");
        assert!(unique_name_among(&[], "anything.txt") == "anything.txt");
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
            "kraken-files-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
