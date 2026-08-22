//! The Files pane, as QML sees it: the workspace's tree, and the copies in and
//! out of it — on this machine or on one at the end of an SSH connection.
//!
//! Nothing here touches a filesystem on the UI thread. Every listing, every
//! transfer and every read for the preview runs on a worker and comes back
//! through a queued callback, because on a remote workspace each of them is an
//! SSH round trip and a window that stopped for one would stop for all of them.
//! Locally the same path costs a thread and gains a window that never blocks on
//! a slow disk.
//!
//! Which machine is answering is [`Place`]'s business, and it is the only thing
//! that differs: the tree, the sorting, the collision rules and the naming are
//! written once and do not know which they are looking at.
//!
//! The preview sheet reuses [`super::diff`]'s run builders and
//! `pupo_core::diff`'s sheet colours, so a file read here is lexed, coloured
//! and laid out exactly as the same file is when it is read as a diff. Two
//! sheets that showed the same code in different colours would be two sheets
//! nobody trusts.

use std::path::{Path, PathBuf};

use pupo_core::chat::highlight;
use pupo_core::files::{
    self, CopyReport, Empty, Entry, Load, OnCollision, Place, Preview, Refusal, Row, Tree,
};
use pupo_core::theme::DEFAULT_THEME;
use pupo_core::{diff, state};
use qmetaobject::*;

use super::diff::{push_run, runs_for};

/// A copy that has been checked and knows where it is going.
#[derive(Clone)]
struct Plan {
    /// Where the bytes come from. Local paths for an import, paths on the far
    /// side for an export out of a remote workspace.
    sources: Vec<PathBuf>,
    /// The directory they land in.
    dest: PathBuf,
    /// `Some` for an import — the direction that has to land inside the
    /// workspace, and the direction whose result the tree shows.
    root: Option<PathBuf>,
    note: String,
}

impl Plan {
    fn importing(&self) -> bool {
        self.root.is_some()
    }
}

/// A plan held up waiting for the reader to say what to do about names that are
/// already taken at the other end.
struct Pending {
    plan: Plan,
    /// Everything the destination already holds. Carried rather than re-read:
    /// it is what found the collision, and it is what names the copy if the
    /// answer turns out to be "keep both".
    taken: Vec<String>,
    /// The colliding names. Empty while the destination is still being listed,
    /// which is how "getting ready" is told from "waiting for an answer".
    names: Vec<String>,
}

/// What a finished copy said, and which way it went.
struct Outcome {
    message: String,
    /// Where the copy landed, when it landed inside the workspace — the tree
    /// opens down to it so the reader can see it arrived.
    reveal: Option<PathBuf>,
    failed: bool,
}

/// A file read for the sheet, with everything the view needs already measured.
struct Sheet {
    path: PathBuf,
    label: String,
    subtitle: String,
    body: Preview,
    /// One entry per line, from the lexer. Empty for a body that is not text.
    spans: Vec<Vec<highlight::Span>>,
    digits: i32,
    columns: i32,
    /// Where the picture is, for an image body: a local file's own path, or the
    /// cache file a remote one was fetched into.
    image: PathBuf,
}

#[derive(QObject, Default)]
pub struct FilesBridge {
    base: qt_base_class!(trait QObject),

    /// The workspace the tree is rooted at, bound to `App.current` in QML.
    workspace: qt_property!(QString; NOTIFY workspace_changed READ get_workspace WRITE set_workspace),
    /// "light" or "dark". The preview's colours are baked into its runs, so the
    /// sheet is rebuilt when this changes.
    theme: qt_property!(QString; NOTIFY theme_changed READ get_theme WRITE set_theme),

    /// One entry per visible line: `{ path, name, depth, is_dir, expanded,
    /// loading, size_label, symlink }`. Rebuilt whole rather than patched — the
    /// tree is a few hundred rows at the sizes anyone reads, and a list rebuilt
    /// whole is one that cannot disagree with itself.
    rows: qt_property!(QVariantList; NOTIFY rows_changed READ get_rows),
    /// The one sentence shown instead of rows: no workspace, an empty folder, a
    /// folder that would not be read. Empty while there are rows.
    message: qt_property!(QString; NOTIFY rows_changed READ get_message),
    /// Whether a listing is still on its way. On a remote workspace this is the
    /// difference between "empty" and "not here yet".
    listing: qt_property!(bool; NOTIFY rows_changed READ get_listing),
    /// Whether the workspace is on another machine. The pane reads it to know
    /// which gestures it can offer.
    remote: qt_property!(bool; NOTIFY workspace_changed READ get_remote),
    /// Whether dotfiles are listed.
    show_hidden: qt_property!(bool; NOTIFY rows_changed READ get_show_hidden WRITE set_show_hidden),
    /// Set while the tree stopped at `files::MAX_ROWS`.
    truncated: qt_property!(bool; NOTIFY rows_changed READ get_truncated),
    /// The row the reader last clicked, as a path.
    selected: qt_property!(QString; NOTIFY selected_changed READ get_selected WRITE set_selected),

    /// What the last copy did, or why it was refused. Shown in the panel's
    /// footer until the next one replaces it.
    status: qt_property!(QString; NOTIFY status_changed READ get_status),
    /// Whether that status is a refusal rather than a result, so the footer can
    /// colour it.
    status_failed: qt_property!(bool; NOTIFY status_changed READ get_status_failed),
    /// A copy is running, or is being got ready. The panel says so and does not
    /// start a second.
    busy: qt_property!(bool; NOTIFY status_changed READ get_busy),

    /// Whether a copy is held up waiting for an answer about names that are
    /// already taken. The panel puts its question up while this is set.
    collision_open: qt_property!(bool; NOTIFY collision_changed READ get_collision_open),
    /// The question itself, naming the one file where there is one and counting
    /// them where there are more.
    collision_message: qt_property!(QString; NOTIFY collision_changed READ get_collision_message),

    // ---- The preview sheet ----
    preview_open: qt_property!(bool; NOTIFY preview_changed READ get_preview_open),
    /// The previewed file's path within the workspace, for the sheet's title.
    preview_path: qt_property!(QString; NOTIFY preview_changed READ get_preview_path),
    /// Its size, as the sheet's subtitle.
    preview_subtitle: qt_property!(QString; NOTIFY preview_changed READ get_preview_subtitle),
    /// "loading", "text", "image", or "message" — which of the sheet's bodies
    /// to show. One string rather than four booleans that could all be true.
    preview_kind: qt_property!(QString; NOTIFY preview_changed READ get_preview_kind),
    /// The sentence shown for a file there is nothing to lay out for.
    preview_message: qt_property!(QString; NOTIFY preview_changed READ get_preview_message),
    /// A `file://` URL for the image body.
    preview_url: qt_property!(QString; NOTIFY preview_changed READ get_preview_url),
    /// One entry per line: `{ no, runs }`, the runs already coloured.
    preview_rows: qt_property!(QVariantList; NOTIFY preview_changed READ get_preview_rows),
    /// How wide the line-number gutter has to be, in digits.
    preview_digits: qt_property!(i32; NOTIFY preview_changed READ get_preview_digits),
    /// The longest line, in characters, so the body scrolls to fit it.
    preview_columns: qt_property!(i32; NOTIFY preview_changed READ get_preview_columns),
    preview_gutter_color: qt_property!(QString; NOTIFY preview_changed READ get_preview_gutter_color),
    preview_scrim: qt_property!(QString; NOTIFY preview_changed READ get_preview_scrim),

    /// The subdued text colour, for the pane's own furniture as much as the
    /// sheet's. One property rather than one per surface, so they cannot drift.
    dim_color: qt_property!(QString; NOTIFY theme_changed READ get_dim_color),
    /// What a refusal is written in — the same red the diff sheet marks a
    /// removed line with, because it is the same "this did not happen".
    alert_color: qt_property!(QString; NOTIFY theme_changed READ get_alert_color),

    workspace_changed: qt_signal!(),
    theme_changed: qt_signal!(),
    rows_changed: qt_signal!(),
    selected_changed: qt_signal!(),
    status_changed: qt_signal!(),
    collision_changed: qt_signal!(),
    preview_changed: qt_signal!(),

    /// Re-read every open directory, keeping the branches that are open.
    refresh: qt_method!(fn(&mut self)),
    /// Open or close a directory row.
    toggle: qt_method!(fn(&mut self, path: QString)),
    /// Close every branch.
    collapse_all: qt_method!(fn(&mut self)),
    /// The directory a drop on this row lands in: the row itself when it is a
    /// folder, its parent when it is a file. Answered here rather than in QML
    /// because it is the same rule the row menu obeys, and because on a remote
    /// workspace QML has no filesystem to ask.
    drop_target: qt_method!(fn(&mut self, path: QString) -> QString),
    /// A `file://` URL for a row. Empty on a remote workspace, whose paths name
    /// nothing on this machine.
    url_for: qt_method!(fn(&mut self, path: QString) -> QString),

    /// Copy the files named by a `text/uri-list` payload into `dest`.
    copy_in: qt_method!(fn(&mut self, payload: QString, dest: QString)),
    /// Copy one row out to a folder the reader picked, named by a URL.
    copy_out: qt_method!(fn(&mut self, path: QString, dest_url: QString)),
    /// Answer the collision question: "replace", "keep_both" or "cancel".
    resolve_collision: qt_method!(fn(&mut self, choice: QString)),

    /// Read a file into the sheet.
    open_preview: qt_method!(fn(&mut self, path: QString)),
    close_preview: qt_method!(fn(&mut self)),

    /// Which machine the tree is reading.
    place: Place,
    tree: Tree,
    entries: Vec<Row>,
    note: String,
    failed: bool,
    running: usize,
    pending: Option<Pending>,
    sheet: Option<Sheet>,
    /// Bumped whenever a copy or a preview starts; an answer from one the
    /// reader has since navigated away from is dropped rather than shown.
    generation: u64,
    /// Counts the cache files a remote image preview is fetched into.
    fetched: u64,
}

impl FilesBridge {
    pub fn new() -> Self {
        Self {
            theme: DEFAULT_THEME.into(),
            ..Default::default()
        }
    }

    // ---- Workspace and theme ----------------------------------------------

    fn get_workspace(&self) -> QString {
        self.workspace.clone()
    }

    /// The workspace key as a plain string. The Qt property is the storage, so
    /// there is one copy of it rather than a shadow field to keep in step.
    fn cwd(&self) -> String {
        self.workspace.to_string()
    }

    fn theme_name(&self) -> String {
        self.theme.to_string()
    }

    fn get_remote(&self) -> bool {
        self.place.is_remote()
    }

    fn set_workspace(&mut self, path: QString) {
        let path = path.to_string();
        if path == self.cwd() {
            return;
        }
        self.workspace = path.as_str().into();
        // A sheet open on the old workspace's file has nothing to do with this
        // one, and its path would read as belonging here. Nor does a question
        // about a folder that has just gone off screen.
        self.sheet = None;
        self.pending = None;
        self.note.clear();
        self.failed = false;
        self.selected = QString::default();
        self.generation += 1;

        self.place = if path.is_empty() {
            Place::Local
        } else {
            Place::for_workspace(&path)
        };
        // A remote workspace's key is a local stand-in folder with none of the
        // project in it; the files are at the path on the far side.
        self.tree.set_root(self.place.root_for(&path));

        self.workspace_changed();
        self.selected_changed();
        self.collision_changed();
        self.preview_changed();
        self.status_changed();
        self.rebuild();
    }

    fn get_theme(&self) -> QString {
        self.theme.clone()
    }

    fn set_theme(&mut self, name: QString) {
        let name = name.to_string();
        if name == self.theme_name() {
            return;
        }
        self.theme = name.as_str().into();
        self.theme_changed();
        // The sheet's runs carry their colours; the tree's do not.
        self.relex();
        self.preview_changed();
    }

    // ---- The tree ---------------------------------------------------------

    /// Redraw from what has been read, and fetch whatever that turned out to
    /// need. This is the whole loop: a listing arriving calls it again, and it
    /// settles when the tree wants nothing more.
    fn rebuild(&mut self) {
        let view = self.tree.view();
        self.entries = view.rows;
        for load in view.wanted {
            self.fetch(load);
        }
        self.rows_changed();
    }

    /// Read one directory on a worker: one SSH round trip on a remote
    /// workspace, one `readdir` locally, and neither on the UI thread.
    fn fetch(&mut self, load: Load) {
        let place = self.place.clone();
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |(load, outcome): (Load, Result<Vec<Entry>, String>)| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_listed(load, outcome);
            }
        });
        std::thread::spawn(move || {
            let outcome = files::list(&place, &load.dir);
            deliver((load, outcome));
        });
    }

    fn on_listed(&mut self, load: Load, outcome: Result<Vec<Entry>, String>) {
        // The tree drops a listing from a generation it has moved past, so a
        // slow answer about a workspace the reader has left goes nowhere.
        self.tree.deliver(&load, outcome);
        self.rebuild();
    }

    fn refresh(&mut self) {
        self.tree.refresh();
        self.rebuild();
    }

    fn toggle(&mut self, path: QString) {
        self.tree.toggle(Path::new(&path.to_string()));
        self.rebuild();
    }

    fn collapse_all(&mut self) {
        self.tree.collapse_all();
        self.rebuild();
    }

    fn get_show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The Qt property is the storage and the tree is told; keeping the answer
    /// in both and reading it from the tree is how the two come to disagree.
    fn set_show_hidden(&mut self, show: bool) {
        if show == self.show_hidden {
            return;
        }
        self.show_hidden = show;
        self.tree.set_show_hidden(show);
        // A redraw, not a re-read: the listings are kept whole.
        self.rebuild();
    }

    fn get_truncated(&self) -> bool {
        self.tree.truncated()
    }

    fn get_listing(&self) -> bool {
        self.tree.loading()
    }

    fn get_selected(&self) -> QString {
        self.selected.clone()
    }

    fn set_selected(&mut self, path: QString) {
        if path.to_string() == self.selected.to_string() {
            return;
        }
        self.selected = path;
        self.selected_changed();
    }

    fn get_message(&self) -> QString {
        if self.cwd().is_empty() {
            return "No workspace open.".into();
        }
        if !self.entries.is_empty() {
            return QString::default();
        }
        match self.tree.empty_reason(self.tree.root()) {
            Some(Empty::Unreadable(reason)) => reason.as_str().into(),
            Some(Empty::Nothing) => "This folder is empty.".into(),
            Some(Empty::HiddenOnly) => "Nothing here but hidden files.".into(),
            // Not read yet. On a remote workspace this is the first thing the
            // pane says, and it must not say "empty" about a folder nobody has
            // looked in.
            None if self.tree.loading() => "Reading…".into(),
            None => QString::default(),
        }
    }

    fn get_rows(&self) -> QVariantList {
        let root = self.tree.root();
        let mut list = QVariantList::default();
        for row in &self.entries {
            let mut map = QVariantMap::default();
            let put = |map: &mut QVariantMap, key: &str, value: &str| {
                map.insert(key.into(), QVariant::from(QString::from(value)));
            };
            put(&mut map, "path", &row.path.to_string_lossy());
            put(&mut map, "name", &row.name);
            put(&mut map, "relative", &files::relative_to(root, &row.path));
            map.insert("depth".into(), (row.depth as i32).into());
            map.insert("is_dir".into(), row.is_dir.into());
            map.insert("expanded".into(), row.expanded.into());
            map.insert("loading".into(), row.loading.into());
            map.insert("symlink".into(), row.symlink.into());
            // A directory's size is the size of the directory entry itself,
            // which is a number about the filesystem rather than about the
            // project. The column stays empty for one.
            put(
                &mut map,
                "size_label",
                &if row.is_dir {
                    String::new()
                } else {
                    files::format_size(row.size)
                },
            );
            list.push(map.into());
        }
        list
    }

    /// The row for a path. It carries the size and the type, which is what
    /// keeps the preview from needing a round trip to learn them.
    fn row_for(&self, path: &Path) -> Option<&Row> {
        self.entries.iter().find(|row| row.path == path)
    }

    fn drop_target(&mut self, path: QString) -> QString {
        let path = path.to_string();
        // An empty path is the panel's own background: the workspace root.
        if path.is_empty() {
            return self.tree.root().to_string_lossy().as_ref().into();
        }
        // Answered from the row rather than from the disk, because on a remote
        // workspace the disk is not here to ask.
        let target = match self.row_for(Path::new(&path)) {
            Some(row) if row.is_dir => PathBuf::from(&path),
            Some(row) => row.path.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => PathBuf::from(&path),
        };
        target.to_string_lossy().as_ref().into()
    }

    fn url_for(&mut self, path: QString) -> QString {
        // A remote path names nothing on this machine, so there is no URL to
        // hand another application.
        if self.place.is_remote() {
            return QString::default();
        }
        files::file_url(Path::new(&path.to_string()))
            .as_str()
            .into()
    }

    // ---- Copying ----------------------------------------------------------

    fn get_status(&self) -> QString {
        self.note.as_str().into()
    }

    fn get_status_failed(&self) -> bool {
        self.failed
    }

    fn get_busy(&self) -> bool {
        self.running > 0
    }

    fn say(&mut self, message: impl Into<String>, failed: bool) {
        self.note = message.into();
        self.failed = failed;
        self.status_changed();
    }

    /// Copy files in from outside the project.
    ///
    /// The sources are always on this machine — they came from a drop or from
    /// the chooser — while the destination is wherever the workspace is.
    fn copy_in(&mut self, payload: QString, dest: QString) {
        if self.running > 0 || self.pending.is_some() {
            return;
        }
        let sources = files::paths_from_uri_list(&payload.to_string());
        if sources.is_empty() {
            return self.say("Nothing there that is a file on this machine.", true);
        }
        let root = self.tree.root().to_path_buf();
        let dest = PathBuf::from(dest.to_string());
        let into = files::relative_to(&root, &dest);
        let where_ = if into.is_empty() {
            "the workspace".to_string()
        } else {
            into
        };

        // Whichever machine the destination is on, the copy has to land inside
        // the workspace. The rest of the rules only mean something when both
        // ends are on one machine — a local process cannot stat a remote
        // directory, and two paths on two machines are never the same file.
        for source in &sources {
            let outcome = if self.place.is_remote() {
                files::check_across(source, &dest, Some(&root))
            } else {
                files::check_copy(source, &dest, Some(&root), OnCollision::KeepBoth)
            };
            if let Err(refusal) = outcome {
                return self.say(refusal.message(), true);
            }
        }
        self.prepare(Plan {
            sources,
            dest,
            root: Some(root),
            note: format!("Copying into {where_}…"),
        });
    }

    /// Copy one row out to a folder on this machine.
    fn copy_out(&mut self, path: QString, dest_url: QString) {
        if self.running > 0 || self.pending.is_some() {
            return;
        }
        let source = PathBuf::from(path.to_string());
        let Some(dest) = files::path_from_url(&dest_url.to_string()) else {
            return self.say("That destination is not a folder on this machine.", true);
        };
        if !dest.is_dir() {
            return self.say(Refusal::NotADirectory.message(), true);
        }
        // No root: leaving the workspace is the point of an export. A source on
        // the far side cannot be stat-ed from here, so a local export is the
        // only one with anything more to check.
        if !self.place.is_remote() {
            if let Err(refusal) = files::check_copy(&source, &dest, None, OnCollision::KeepBoth) {
                return self.say(refusal.message(), true);
            }
        }
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.prepare(Plan {
            sources: vec![source],
            dest,
            root: None,
            note: format!("Copying {name}…"),
        });
    }

    /// Find out what the destination already holds, then either run the copy or
    /// stop and ask.
    ///
    /// One listing rather than one `exists` per name: on a remote destination
    /// that is a single round trip instead of one per file, and it is the same
    /// list that names the copy if the answer turns out to be "keep both".
    fn prepare(&mut self, plan: Plan) {
        self.generation += 1;
        let generation = self.generation;
        self.running += 1;
        self.say(plan.note.clone(), false);

        // The destination is on the workspace's machine for an import, and on
        // this one for an export.
        let place = if plan.importing() {
            self.place.clone()
        } else {
            Place::Local
        };
        let dest = plan.dest.clone();
        self.pending = Some(Pending {
            plan,
            taken: Vec::new(),
            names: Vec::new(),
        });

        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |taken: Result<Vec<String>, String>| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_prepared(generation, taken);
            }
        });
        std::thread::spawn(move || {
            let taken = files::list(&place, &dest)
                .map(|entries| entries.into_iter().map(|entry| entry.name).collect());
            deliver(taken);
        });
    }

    fn on_prepared(&mut self, generation: u64, taken: Result<Vec<String>, String>) {
        self.running = self.running.saturating_sub(1);
        if generation != self.generation {
            return;
        }
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let taken = match taken {
            Ok(taken) => taken,
            Err(reason) => {
                self.collision_changed();
                return self.say(reason, true);
            }
        };

        let names = files::collisions_among(&pending.plan.sources, &taken);
        if names.is_empty() {
            // Nothing in the way, so there is nothing to ask and the mode
            // cannot matter.
            self.collision_changed();
            return self.run(pending.plan, taken, OnCollision::KeepBoth);
        }
        // The footer says nothing: the dialog is about to ask the same question
        // in larger type, and the status line's job starts again when there is
        // an outcome to report.
        self.say("", false);
        pending.taken = taken;
        pending.names = names;
        self.pending = Some(pending);
        self.collision_changed();
    }

    fn get_collision_open(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| !pending.names.is_empty())
    }

    fn get_collision_message(&self) -> QString {
        let Some(pending) = &self.pending else {
            return QString::default();
        };
        let where_ = match &pending.plan.root {
            Some(root) => files::relative_to(root, &pending.plan.dest),
            None => pending
                .plan
                .dest
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        files::collision_message(&pending.names, &where_)
            .as_str()
            .into()
    }

    /// The reader's answer. Taking the pending copy out first means a second
    /// click on a button whose dialog is already closing cannot start the copy
    /// twice.
    fn resolve_collision(&mut self, choice: QString) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.collision_changed();

        let mode = match choice.to_string().as_str() {
            "replace" => OnCollision::Replace,
            "keep_both" => OnCollision::KeepBoth,
            // Including "cancel", and including anything unrecognised: an
            // answer nobody meant is not one to act on when acting deletes
            // files.
            _ => return self.say("Copy cancelled.", false),
        };

        // Replace deletes before it writes, so it refuses destinations the
        // first pass allowed. Only checkable when both ends are on one machine:
        // across the wire the two paths belong to different filesystems and
        // cannot be the same file however alike they look.
        if mode == OnCollision::Replace && !self.place.is_remote() {
            for source in &pending.plan.sources {
                if let Err(refusal) = files::check_copy(
                    source,
                    &pending.plan.dest,
                    pending.plan.root.as_deref(),
                    OnCollision::Replace,
                ) {
                    return self.say(refusal.message(), true);
                }
            }
        }
        self.run(pending.plan, pending.taken, mode);
    }

    /// Run one batch of copies on a worker and report back.
    fn run(&mut self, plan: Plan, taken: Vec<String>, mode: OnCollision) {
        self.generation += 1;
        let generation = self.generation;
        self.running += 1;
        self.say(plan.note.clone(), false);

        let place = self.place.clone();
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |outcome: Outcome| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_copied(generation, outcome);
            }
        });
        std::thread::spawn(move || {
            deliver(carry_out(&place, plan, taken, mode));
        });
    }

    fn on_copied(&mut self, generation: u64, outcome: Outcome) {
        self.running = self.running.saturating_sub(1);
        self.say(outcome.message, outcome.failed);
        // A copy into a workspace the reader has since left changed a folder
        // that is no longer on screen; saying what it did is still right, and
        // re-reading a tree rooted somewhere else is not.
        if generation != self.generation {
            return;
        }
        if let Some(landed) = outcome.reveal {
            self.tree.reveal(&landed);
            self.tree.refresh();
            self.rebuild();
            self.set_selected(landed.to_string_lossy().as_ref().into());
        }
    }

    // ---- The preview sheet ------------------------------------------------

    fn open_preview(&mut self, path: QString) {
        let path = PathBuf::from(path.to_string());
        // The row carries the size and the type. Over SSH that is the
        // difference between one round trip and two.
        let Some(row) = self.row_for(&path) else {
            return;
        };
        let (size, is_dir) = (row.size, row.is_dir);
        if is_dir {
            return;
        }

        self.generation += 1;
        let generation = self.generation;
        // Up straight away, saying it is reading. On a remote workspace the
        // bytes are a round trip away, and a sheet that appeared only once they
        // landed would read as a click that did nothing.
        self.sheet = Some(Sheet {
            label: files::relative_to(self.tree.root(), &path),
            subtitle: files::format_size(size),
            path: path.clone(),
            body: Preview::Unreadable(String::new()),
            spans: Vec::new(),
            digits: 2,
            columns: 0,
            image: PathBuf::new(),
        });
        self.preview_changed();

        let place = self.place.clone();
        // A fresh cache name each time: Qt caches an image by its URL, and
        // reusing one would redraw the file before this one.
        self.fetched += 1;
        let cache = image_cache_path(&path, self.fetched);
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |(body, image): (Preview, PathBuf)| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_previewed(generation, body, image);
            }
        });
        std::thread::spawn(move || match &place {
            Place::Local => {
                let body = files::preview(&path);
                deliver((body, path.clone()));
            }
            Place::Remote(target) => {
                let body = files::preview_remote(target, &path, size, is_dir);
                // An image has to exist as a file before anything can draw it,
                // so this is the one preview that fetches rather than reads.
                if body == Preview::Image {
                    return match files::fetch_to(target, &path, &cache) {
                        Ok(()) => deliver((body, cache)),
                        Err(reason) => deliver((Preview::Unreadable(reason), PathBuf::new())),
                    };
                }
                deliver((body, PathBuf::new()));
            }
        });
    }

    fn on_previewed(&mut self, generation: u64, body: Preview, image: PathBuf) {
        if generation != self.generation {
            return;
        }
        let Some(sheet) = &mut self.sheet else {
            return;
        };
        sheet.body = body;
        sheet.image = image;
        if let Preview::TooLarge(size) = &sheet.body {
            sheet.subtitle = files::format_size(*size);
        }
        self.relex();
        self.preview_changed();
    }

    /// Lex the sheet's text and measure it. Run when a file arrives, and again
    /// when the theme changes, because the runs carry their own colours.
    fn relex(&mut self) {
        let theme = self.theme_name();
        let Some(sheet) = &mut self.sheet else {
            return;
        };
        let Preview::Text { lines, .. } = &sheet.body else {
            sheet.spans = Vec::new();
            sheet.digits = 2;
            sheet.columns = 0;
            return;
        };
        // A syntax the set does not know comes back as no spans at all, which
        // renders as plain text rather than as an error.
        let syntax = highlight::syntax_for_filename(&sheet.path.to_string_lossy());
        sheet.spans = highlight::line_spans(&lines.join("\n"), syntax, &theme);
        sheet.digits = lines.len().to_string().len().max(2) as i32;
        sheet.columns = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as i32;
    }

    fn close_preview(&mut self) {
        let Some(sheet) = &self.sheet else {
            return;
        };
        // A fetched picture has done its job, and it is the reader's disk. A
        // local one is the file itself and stays where it is.
        if !sheet.image.as_os_str().is_empty() && sheet.image != sheet.path {
            let _ = std::fs::remove_file(&sheet.image);
        }
        self.sheet = None;
        self.preview_changed();
    }

    fn get_preview_open(&self) -> bool {
        self.sheet.is_some()
    }

    fn get_preview_path(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.label.as_str().into(),
            None => QString::default(),
        }
    }

    fn get_preview_subtitle(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.subtitle.as_str().into(),
            None => QString::default(),
        }
    }

    fn get_preview_kind(&self) -> QString {
        match self.sheet.as_ref().map(|sheet| &sheet.body) {
            Some(Preview::Text { .. }) => "text".into(),
            Some(Preview::Image) => "image".into(),
            // The placeholder a sheet is opened with carries an empty reason,
            // which is how "still reading" is told from "would not read".
            Some(Preview::Unreadable(reason)) if reason.is_empty() => "loading".into(),
            Some(_) => "message".into(),
            None => QString::default(),
        }
    }

    fn get_preview_message(&self) -> QString {
        let Some(sheet) = &self.sheet else {
            return QString::default();
        };
        match &sheet.body {
            Preview::Binary => "This is a binary file.".into(),
            Preview::Empty => "This file is empty.".into(),
            Preview::TooLarge(size) => format!(
                "This file is {} — too large to preview. The limit is {}.",
                files::format_size(*size),
                files::format_size(files::MAX_PREVIEW_BYTES)
            )
            .as_str()
            .into(),
            Preview::Unreadable(reason) => reason.as_str().into(),
            Preview::Text { truncated: true, .. } => {
                format!("Showing the first {} lines.", files::MAX_PREVIEW_LINES)
                    .as_str()
                    .into()
            }
            _ => QString::default(),
        }
    }

    fn get_preview_url(&self) -> QString {
        match &self.sheet {
            Some(sheet) if sheet.body == Preview::Image => {
                files::file_url(&sheet.image).as_str().into()
            }
            _ => QString::default(),
        }
    }

    fn get_preview_digits(&self) -> i32 {
        self.sheet.as_ref().map(|sheet| sheet.digits).unwrap_or(2)
    }

    fn get_preview_columns(&self) -> i32 {
        self.sheet.as_ref().map(|sheet| sheet.columns).unwrap_or(0)
    }

    fn get_preview_gutter_color(&self) -> QString {
        diff::viewer_color(&self.theme_name(), "gutter").into()
    }

    fn get_dim_color(&self) -> QString {
        diff::viewer_color(&self.theme_name(), "dim").into()
    }

    fn get_alert_color(&self) -> QString {
        diff::viewer_color(&self.theme_name(), "del").into()
    }

    fn get_preview_scrim(&self) -> QString {
        let (r, g, b, a) = diff::scrim(&self.theme_name());
        format!("#{a:02x}{r:02x}{g:02x}{b:02x}").as_str().into()
    }

    fn get_preview_rows(&self) -> QVariantList {
        let mut list = QVariantList::default();
        let Some(sheet) = &self.sheet else {
            return list;
        };
        let Preview::Text { lines, .. } = &sheet.body else {
            return list;
        };
        let base = diff::viewer_color(&self.theme_name(), "text");
        for (index, line) in lines.iter().enumerate() {
            let mut map = QVariantMap::default();
            map.insert(
                "no".into(),
                QVariant::from(QString::from((index + 1).to_string().as_str())),
            );
            let runs = match sheet.spans.get(index) {
                Some(spans) => runs_for(line, spans, base),
                // The lexer gave up part way — a file it could not finish still
                // reads, just without colour past that point.
                None => {
                    let mut runs = QVariantList::default();
                    push_run(&mut runs, line, base, false);
                    runs
                }
            };
            map.insert("runs".into(), QVariant::from(runs));
            list.push(map.into());
        }
        list
    }
}

/// Where a remote image is fetched to before it is drawn.
fn image_cache_path(source: &Path, serial: u64) -> PathBuf {
    let extension = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    state::config_dir()
        .join("cache")
        .join(format!("preview-{serial}{extension}"))
}

/// Do the copies, wherever the two ends are.
///
/// The three cases differ only in which function moves the bytes; the naming,
/// the replacing and the refusing are the same rules in all of them.
fn carry_out(place: &Place, plan: Plan, mut taken: Vec<String>, mode: OnCollision) -> Outcome {
    let importing = plan.importing();
    let mut reports: Vec<CopyReport> = Vec::new();
    let mut refusal: Option<Refusal> = None;

    for source in &plan.sources {
        let attempt = match (place, importing) {
            (Place::Local, _) => files::copy_into(source, &plan.dest, plan.root.as_deref(), mode),
            (Place::Remote(target), true) => {
                files::copy_up(target, source, &plan.dest, &taken, mode)
            }
            (Place::Remote(target), false) => files::copy_down(target, source, &plan.dest, mode),
        };
        match attempt {
            Ok(report) => {
                // The next source in the batch must not be handed a name this
                // one has just taken — nobody is going to re-list the far side
                // between two files of one drop.
                if let Some(name) = report.destination.file_name() {
                    taken.push(name.to_string_lossy().into_owned());
                }
                reports.push(report);
            }
            // Checked before the batch started, so reaching here means the
            // source went away while the copy was running.
            Err(reason) => {
                refusal = Some(reason);
                break;
            }
        }
    }
    summarise(reports, refusal, importing)
}

/// Turn a batch of copy reports into the one sentence the footer shows.
fn summarise(reports: Vec<CopyReport>, refusal: Option<Refusal>, importing: bool) -> Outcome {
    if let Some(refusal) = refusal {
        return Outcome {
            message: refusal.message().to_string(),
            reveal: None,
            failed: true,
        };
    }
    let failures: usize = reports.iter().map(|report| report.failures.len()).sum();
    // The tree only shows the workspace, so only an import has anything to
    // reveal — and only the first of a batch, since revealing the last would
    // scroll away from the rest. A copy that failed has nothing to point at.
    let reveal = (importing && failures == 0)
        .then(|| reports.first().map(|report| report.destination.clone()))
        .flatten();
    let message = match reports.len() {
        0 => "Nothing was copied.".to_string(),
        // One thing, and none of it arrived. A transfer across the wire is a
        // single operation, so its one failure is the whole story — "Copied src
        // (1 skipped)" would be a claim about a file that never landed.
        1 if failures > 0 && reports[0].files == 0 => reports[0].failures[0].clone(),
        1 => reports[0].summary(),
        many => {
            let files: usize = reports.iter().map(|report| report.files).sum();
            let bytes: u64 = reports.iter().map(|report| report.bytes).sum();
            if files > 0 {
                format!(
                    "Copied {many} items — {files} files, {}",
                    files::format_size(bytes)
                )
            } else {
                format!("Copied {many} items")
            }
        }
    };
    let message = if failures > 0 && reports.len() > 1 {
        format!("{message} ({failures} skipped)")
    } else {
        message
    };
    Outcome {
        message,
        reveal,
        // Files skipped inside an otherwise finished copy are worth colouring:
        // the copy happened, and it is not the copy that was asked for.
        failed: failures > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(name: &str, files: usize, bytes: u64, failures: usize) -> CopyReport {
        CopyReport {
            files,
            bytes,
            failures: (0..failures).map(|i| format!("skipped {i}")).collect(),
            destination: PathBuf::from(name),
            replaced: false,
        }
    }

    #[test]
    fn one_copy_reports_what_it_moved() {
        let outcome = summarise(vec![report("/w/note.txt", 1, 12, 0)], None, true);
        assert_eq!(outcome.message, "Copied note.txt");
        assert!(!outcome.failed);
        assert_eq!(outcome.reveal, Some(PathBuf::from("/w/note.txt")));
    }

    #[test]
    fn a_batch_totals_its_files_and_bytes() {
        let outcome = summarise(
            vec![report("/w/a", 2, 1024, 0), report("/w/b", 3, 1024, 0)],
            None,
            true,
        );
        assert_eq!(outcome.message, "Copied 2 items — 5 files, 2.0 KB");
        // The first, not the last: revealing the last would scroll away from
        // the rest of the batch.
        assert_eq!(outcome.reveal, Some(PathBuf::from("/w/a")));
    }

    #[test]
    fn a_transfer_that_counts_nothing_still_names_what_it_moved() {
        // A copy over SSH is a tar stream and counts no files, so zero means
        // "not measured" rather than "nothing happened".
        let outcome = summarise(vec![report("/w/src", 0, 0, 0)], None, true);
        assert_eq!(outcome.message, "Copied src");
        assert!(!outcome.failed);

        let batch = summarise(
            vec![report("/w/a", 0, 0, 0), report("/w/b", 0, 0, 0)],
            None,
            true,
        );
        assert_eq!(batch.message, "Copied 2 items");
    }

    #[test]
    fn a_failed_transfer_says_what_went_wrong_rather_than_claiming_a_copy() {
        let mut failed = report("/w/src", 0, 0, 0);
        failed.failures = vec!["ssh: connect to host: No route to host".into()];
        let outcome = summarise(vec![failed], None, true);
        assert_eq!(outcome.message, "ssh: connect to host: No route to host");
        assert!(outcome.failed);
        // Nothing landed, so there is nothing in the tree to point at.
        assert_eq!(outcome.reveal, None);
    }

    #[test]
    fn a_local_copy_that_skipped_something_says_so_and_reads_as_failed() {
        // It happened, and it is not the copy that was asked for. Distinct from
        // the case above: files did land, so the summary still counts them.
        let outcome = summarise(vec![report("/w/tree", 4, 100, 2)], None, true);
        assert!(outcome.message.contains("2 skipped"), "{}", outcome.message);
        assert!(outcome.failed);
        assert_eq!(outcome.reveal, None);
    }

    #[test]
    fn a_refusal_beats_whatever_was_copied_before_it() {
        let outcome = summarise(
            vec![report("/w/a", 1, 10, 0)],
            Some(Refusal::IntoItself),
            true,
        );
        assert_eq!(outcome.message, Refusal::IntoItself.message());
        assert!(outcome.failed);
        // Nothing to point at: the batch did not finish.
        assert_eq!(outcome.reveal, None);
    }

    #[test]
    fn an_export_has_nothing_in_the_tree_to_reveal() {
        // It landed outside the workspace, which is the whole point of it.
        let outcome = summarise(vec![report("/elsewhere/note.txt", 1, 12, 0)], None, false);
        assert_eq!(outcome.reveal, None);
        assert!(!outcome.failed);
    }

    #[test]
    fn a_batch_never_hands_two_sources_the_same_landing_name() {
        // Two files with one basename, dropped together. The second has to see
        // the name the first has just taken, and on a remote destination nobody
        // is going to re-list the far side between them.
        let mut running: Vec<String> = Vec::new();
        for landed in ["note.txt", "note 2.txt", "note 3.txt"] {
            assert_eq!(files::unique_name_among(&running, "note.txt"), landed);
            running.push(landed.to_string());
        }
    }

    #[test]
    fn an_image_cache_name_keeps_the_extension_and_changes_every_time() {
        // Qt caches an image by its URL, so reusing a name would redraw the
        // file before this one.
        let first = image_cache_path(Path::new("/w/shot.png"), 1);
        let second = image_cache_path(Path::new("/w/shot.png"), 2);
        assert_ne!(first, second);
        assert!(first.to_string_lossy().ends_with("preview-1.png"));
        // A file with no extension still gets a name.
        assert!(image_cache_path(Path::new("/w/shot"), 3)
            .to_string_lossy()
            .ends_with("preview-3"));
    }
}
