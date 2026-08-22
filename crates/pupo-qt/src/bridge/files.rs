//! The Files pane, as QML sees it: the workspace's tree, and the copies in and
//! out of it.
//!
//! Reading a directory is one `readdir` and happens on the UI thread — opening
//! a branch is not work worth a thread, and a tree that arrived a frame later
//! than the click that asked for it would flicker. Copying is the opposite: a
//! folder can be gigabytes, so every copy runs on a worker and reports back.
//!
//! The preview sheet is the diff sheet's twin, and deliberately so — it reuses
//! [`super::diff`]'s run builders and `pupo_core::diff`'s sheet colours, so a
//! file read here is lexed, coloured and laid out exactly as the same file is
//! when it is read as a diff. Two sheets that showed the same code in different
//! colours would be two sheets nobody trusts.

use std::path::{Path, PathBuf};

use pupo_core::chat::highlight;
use pupo_core::files::{self, CopyReport, Empty, OnCollision, Preview, Refusal, Row, Tree};
use pupo_core::theme::DEFAULT_THEME;
use pupo_core::{diff, remote};
use qmetaobject::*;

use super::diff::{push_run, runs_for};

/// A copy that is ready to run and waiting for the reader to say what to do
/// about the names already taken at the other end.
struct Pending {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    /// `Some` for an import, which is the direction that has to land inside the
    /// workspace and the direction whose result the tree shows.
    root: Option<PathBuf>,
    note: String,
    /// The colliding names, for the question.
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

#[derive(QObject, Default)]
pub struct FilesBridge {
    base: qt_base_class!(trait QObject),

    /// The workspace the tree is rooted at, bound to `App.current` in QML.
    workspace: qt_property!(QString; NOTIFY workspace_changed READ get_workspace WRITE set_workspace),
    /// "light" or "dark". The preview's colours are baked into its runs, so the
    /// sheet is rebuilt when this changes.
    theme: qt_property!(QString; NOTIFY theme_changed READ get_theme WRITE set_theme),

    /// One entry per visible line: `{ path, name, depth, is_dir, expanded,
    /// size_label, symlink }`. Rebuilt whole rather than patched — the tree is
    /// a few hundred rows at the sizes anyone reads, and a list rebuilt whole
    /// is one that cannot disagree with itself.
    rows: qt_property!(QVariantList; NOTIFY rows_changed READ get_rows),
    /// The one sentence shown instead of rows: an empty folder, a remote
    /// workspace, or no workspace at all. Empty while there are rows.
    message: qt_property!(QString; NOTIFY rows_changed READ get_message),
    /// Whether dotfiles are listed.
    show_hidden: qt_property!(bool; NOTIFY rows_changed READ get_show_hidden WRITE set_show_hidden),
    /// Set while the tree stopped at `files::MAX_ROWS`.
    truncated: qt_property!(bool; NOTIFY rows_changed READ get_truncated),
    /// The row the reader last clicked, as a path. Drives the highlight, and is
    /// the default target for a copy out.
    selected: qt_property!(QString; NOTIFY selected_changed READ get_selected WRITE set_selected),

    /// What the last copy did, or why it was refused. Shown in the panel's
    /// footer until the next one replaces it.
    status: qt_property!(QString; NOTIFY status_changed READ get_status),
    /// Whether that status is a refusal rather than a result, so the footer can
    /// colour it.
    status_failed: qt_property!(bool; NOTIFY status_changed READ get_status_failed),
    /// A copy is running. The panel says so and does not start a second.
    busy: qt_property!(bool; NOTIFY status_changed READ get_busy),

    /// Whether a copy is held up waiting for an answer about names that are
    /// already taken. The panel puts its question up while this is set.
    collision_open: qt_property!(bool; NOTIFY collision_changed READ get_collision_open),
    /// The question itself, naming the one file where there is one and counting
    /// them where there are more.
    collision_message: qt_property!(QString; NOTIFY collision_changed READ get_collision_message),

    // ---- The preview sheet ----
    /// Whether the sheet is up.
    preview_open: qt_property!(bool; NOTIFY preview_changed READ get_preview_open),
    /// The previewed file's path within the workspace, for the sheet's title.
    preview_path: qt_property!(QString; NOTIFY preview_changed READ get_preview_path),
    /// Its size, as the sheet's subtitle.
    preview_subtitle: qt_property!(QString; NOTIFY preview_changed READ get_preview_subtitle),
    /// "text", "image", or "message" — which of the sheet's three bodies to
    /// show. One string rather than three booleans that could all be true.
    preview_kind: qt_property!(QString; NOTIFY preview_changed READ get_preview_kind),
    /// The sentence shown for a file there is nothing to lay out for: binary,
    /// empty, too large, unreadable.
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
    /// sheet's: the size column, the twisty, the footer, the empty sentence.
    /// One property rather than one per surface, so they cannot drift.
    dim_color: qt_property!(QString; NOTIFY theme_changed READ get_dim_color),
    /// What a refusal is written in. The same red the diff sheet marks a
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
    /// Close every branch. The way back from a tree that got away from the
    /// reader — an accidental `node_modules` is otherwise a lot of clicking.
    collapse_all: qt_method!(fn(&mut self)),
    /// The directory a drop on this row lands in: the row itself when it is a
    /// folder, its parent when it is a file. QML asks rather than deciding,
    /// because "drop on a file" meaning "into the folder holding it" is the
    /// same rule a copy out of the panel obeys and it is written once.
    drop_target: qt_method!(fn(&mut self, path: QString) -> QString),
    /// A `file://` URL for a row, for the drag payload and for "copy path".
    url_for: qt_method!(fn(&mut self, path: QString) -> QString),

    /// Copy the files named by a `text/uri-list` payload into `dest` — a drop
    /// from another application, or the file chooser's answer.
    copy_in: qt_method!(fn(&mut self, payload: QString, dest: QString)),
    /// Copy one row out to a folder the reader picked, named by a URL.
    copy_out: qt_method!(fn(&mut self, path: QString, dest_url: QString)),
    /// Answer the collision question: "replace", "keep_both" or "cancel".
    /// Anything else cancels, because an answer nobody recognises is not an
    /// answer to act on when the act deletes files.
    resolve_collision: qt_method!(fn(&mut self, choice: QString)),

    /// Read a file into the sheet.
    open_preview: qt_method!(fn(&mut self, path: QString)),
    close_preview: qt_method!(fn(&mut self)),

    tree: Tree,
    entries: Vec<Row>,
    /// Set when the workspace lives on another machine, which this pane cannot
    /// read. Kept as a field so the message survives a refresh.
    remote_workspace: bool,
    note: String,
    failed: bool,
    running: usize,
    pending: Option<Pending>,
    sheet: Option<Sheet>,
    /// Bumped whenever a copy starts; a result from a copy the reader has since
    /// navigated away from is applied to the status but not to the tree.
    generation: u64,
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
        self.collision_changed();
        self.note.clear();
        self.failed = false;
        self.selected = QString::default();
        // A remote workspace's anchor is a local stand-in folder with none of
        // the project in it. Listing it would show an empty folder and call it
        // the project, which is worse than saying there is nothing to show.
        self.remote_workspace = !path.is_empty() && remote::resolve(&path).is_some();
        self.tree.set_root(if self.remote_workspace {
            String::new()
        } else {
            path
        });
        self.workspace_changed();
        self.selected_changed();
        self.preview_changed();
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
        self.preview_changed();
    }

    // ---- The tree ---------------------------------------------------------

    fn rebuild(&mut self) {
        self.entries = self.tree.rows();
        self.rows_changed();
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
        self.rebuild();
    }

    fn get_truncated(&self) -> bool {
        self.tree.truncated()
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
        if self.remote_workspace {
            return "This workspace is on another machine. Its files are not browsable here yet."
                .into();
        }
        if self.cwd().is_empty() {
            return "No workspace open.".into();
        }
        if !self.entries.is_empty() {
            return QString::default();
        }
        match self.tree.empty_reason(self.tree.root()) {
            Some(Empty::Unreadable) => "This folder could not be read.".into(),
            _ if self.show_hidden => "This folder is empty.".into(),
            // Worth saying which, because the toggle is right there and a
            // dotfile-only folder looks identical to an empty one.
            _ => "Nothing here but hidden files.".into(),
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

    fn drop_target(&mut self, path: QString) -> QString {
        let path = path.to_string();
        // An empty path is the panel's own background: the workspace root.
        if path.is_empty() {
            return self.tree.root().to_string_lossy().as_ref().into();
        }
        let path = PathBuf::from(path);
        let target = if path.is_dir() {
            path
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        target.to_string_lossy().as_ref().into()
    }

    fn url_for(&mut self, path: QString) -> QString {
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
    /// The whole payload is checked before any of it is copied, so a drop of
    /// five files where one is refused copies none of them: half a drop is a
    /// state nobody can see and nobody asked for.
    fn copy_in(&mut self, payload: QString, dest: QString) {
        if self.running > 0 || self.pending.is_some() {
            return;
        }
        let sources = files::paths_from_uri_list(&payload.to_string());
        if sources.is_empty() {
            return self.say("Nothing there that is a file on this machine.", true);
        }
        let root = PathBuf::from(self.cwd());
        let dest = PathBuf::from(dest.to_string());
        let into = files::relative_to(&root, &dest);
        let where_ = if into.is_empty() {
            "the workspace".to_string()
        } else {
            into.clone()
        };
        self.begin(sources, dest, Some(root), format!("Copying into {where_}…"));
    }

    /// Copy one row out to a folder outside the project.
    fn copy_out(&mut self, path: QString, dest_url: QString) {
        if self.running > 0 || self.pending.is_some() {
            return;
        }
        let source = PathBuf::from(path.to_string());
        let Some(dest) = files::path_from_url(&dest_url.to_string()) else {
            return self.say("That destination is not a folder on this machine.", true);
        };
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // No root: leaving the workspace is the point of an export.
        self.begin(vec![source], dest, None, format!("Copying {name}…"));
    }

    /// Check a batch, then either run it or stop and ask.
    ///
    /// The structural checks run under `KeepBoth`, which is the mode that
    /// refuses least — a batch that fails one of them fails whatever the reader
    /// would have answered, so it is refused before they are asked anything.
    fn begin(
        &mut self,
        sources: Vec<PathBuf>,
        dest: PathBuf,
        root: Option<PathBuf>,
        note: String,
    ) {
        for source in &sources {
            if let Err(refusal) =
                files::check_copy(source, &dest, root.as_deref(), OnCollision::KeepBoth)
            {
                return self.say(refusal.message(), true);
            }
        }
        let names = files::collisions(&sources, &dest);
        if names.is_empty() {
            // Nothing in the way, so there is nothing to ask and the mode
            // cannot matter.
            return self.run_copy(sources, dest, root, note, OnCollision::KeepBoth);
        }
        // The footer says nothing: the dialog is about to ask the same
        // question in larger type, and the status line's job starts again when
        // there is an outcome to report.
        self.pending = Some(Pending {
            sources,
            dest,
            root,
            note,
            names,
        });
        self.collision_changed();
    }

    fn get_collision_open(&self) -> bool {
        self.pending.is_some()
    }

    fn get_collision_message(&self) -> QString {
        match &self.pending {
            Some(pending) => {
                let where_ = match &pending.root {
                    Some(root) => files::relative_to(root, &pending.dest),
                    None => pending
                        .dest
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                };
                files::collision_message(&pending.names, &where_)
                    .as_str()
                    .into()
            }
            None => QString::default(),
        }
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
        // first pass allowed. Checked here rather than at the end of the
        // worker, where the deletion would already have happened.
        if mode == OnCollision::Replace {
            for source in &pending.sources {
                if let Err(refusal) = files::check_copy(
                    source,
                    &pending.dest,
                    pending.root.as_deref(),
                    OnCollision::Replace,
                ) {
                    return self.say(refusal.message(), true);
                }
            }
        }
        self.run_copy(
            pending.sources,
            pending.dest,
            pending.root,
            pending.note,
            mode,
        );
    }

    /// Run one batch of copies on a worker and report back.
    ///
    /// `root` is `Some` for an import, which is the direction that has to land
    /// inside the workspace — and the direction whose result the tree shows.
    fn run_copy(
        &mut self,
        sources: Vec<PathBuf>,
        dest: PathBuf,
        root: Option<PathBuf>,
        note: String,
        mode: OnCollision,
    ) {
        self.generation += 1;
        let generation = self.generation;
        self.running += 1;
        self.say(note, false);

        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |outcome: Outcome| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_copied(generation, outcome);
            }
        });
        std::thread::spawn(move || {
            let mut reports: Vec<CopyReport> = Vec::new();
            let mut refusal: Option<Refusal> = None;
            for source in &sources {
                match files::copy_into(source, &dest, root.as_deref(), mode) {
                    Ok(report) => reports.push(report),
                    // Checked before the batch started, so reaching here means
                    // the source went away while the copy was running.
                    Err(reason) => {
                        refusal = Some(reason);
                        break;
                    }
                }
            }
            deliver(summarise(reports, refusal, root.is_some()));
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
        let body = files::preview(&path);
        let label = files::relative_to(self.tree.root(), &path);
        let subtitle = match &body {
            Preview::TooLarge(size) => files::format_size(*size),
            _ => std::fs::metadata(&path)
                .map(|meta| files::format_size(meta.len()))
                .unwrap_or_default(),
        };

        // Lexing is the expensive half and it is only ever wanted for text. A
        // syntax the set does not know comes back as no spans at all, which
        // renders as plain text rather than as an error.
        let (spans, digits, columns) = match &body {
            Preview::Text { lines, .. } => {
                let source = lines.join("\n");
                let syntax = highlight::syntax_for_filename(&path.to_string_lossy());
                let spans = highlight::line_spans(&source, syntax, &self.theme_name());
                let digits = lines.len().to_string().len().max(2) as i32;
                let columns = lines
                    .iter()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0) as i32;
                (spans, digits, columns)
            }
            _ => (Vec::new(), 2, 0),
        };

        self.sheet = Some(Sheet {
            path,
            label,
            subtitle,
            body,
            spans,
            digits,
            columns,
        });
        self.preview_changed();
    }

    fn close_preview(&mut self) {
        if self.sheet.is_none() {
            return;
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
            Preview::Text { truncated: true, .. } => format!(
                "Showing the first {} lines.",
                files::MAX_PREVIEW_LINES
            )
            .as_str()
            .into(),
            _ => QString::default(),
        }
    }

    fn get_preview_url(&self) -> QString {
        match &self.sheet {
            Some(sheet) => files::file_url(&sheet.path).as_str().into(),
            None => QString::default(),
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

/// Turn a batch of copy reports into the one sentence the footer shows.
fn summarise(reports: Vec<CopyReport>, refusal: Option<Refusal>, importing: bool) -> Outcome {
    if let Some(refusal) = refusal {
        return Outcome {
            message: refusal.message().to_string(),
            reveal: None,
            failed: true,
        };
    }
    // The tree only shows the workspace, so only an import has anything to
    // reveal — and only the first of a batch, since revealing the last would
    // scroll away from the rest.
    let reveal = importing
        .then(|| reports.first().map(|report| report.destination.clone()))
        .flatten();
    let failures: usize = reports.iter().map(|report| report.failures.len()).sum();
    let message = match reports.len() {
        0 => "Nothing was copied.".to_string(),
        1 => reports[0].summary(),
        many => {
            let files: usize = reports.iter().map(|report| report.files).sum();
            let bytes: u64 = reports.iter().map(|report| report.bytes).sum();
            format!(
                "Copied {many} items — {files} files, {}",
                files::format_size(bytes)
            )
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
    fn a_copy_that_skipped_something_says_so_and_reads_as_failed() {
        // It happened, and it is not the copy that was asked for.
        let outcome = summarise(vec![report("/w/tree", 4, 100, 2)], None, true);
        assert!(outcome.message.contains("2 skipped"));
        assert!(outcome.failed);
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
}
