//! The diff pane and the sheet it opens, as QML sees them.
//!
//! One object carries both because they are one conversation: the pane lists
//! the changed files, and a click on a row loads that file's diff into the same
//! object's viewer half. Splitting them would mean handing the sheet a file
//! reference the pane could invalidate underneath it.
//!
//! Every git call happens on a worker thread. A remote workspace runs git over
//! SSH, and a round trip on the UI thread is a frozen window; the local case
//! goes the same way rather than keeping two code paths, and the pane simply
//! keeps what it is showing until the new rows land.
//!
//! The parsing, the colours and the command building all live in
//! [`pupo_core::diff`]. What is here is the conversion: core data in, Qt
//! properties out.

use pupo_core::chat::highlight::{line_spans, syntax_for_filename, Span};
use pupo_core::debug;
use pupo_core::diff::{self, DiffBody, DiffDocument, DiffLimits, FileChange, RowKind};
use pupo_core::git::GitRunner;
use pupo_core::theme::DEFAULT_THEME;
use qmetaobject::*;

/// A tab is drawn as this many spaces. The widget port set a tab stop four
/// characters wide; a `Text` item has no tab stops at all, so the substitution
/// happens in the runs instead — after they are sliced, so the lexer's byte
/// offsets still line up with the line they came from.
const TAB_WIDTH: usize = 4;

/// One file's diff, loaded and lexed. Held so a theme flip can recolour the
/// sheet without going back to git for a diff it already has.
struct Sheet {
    document: DiffDocument,
    body: DiffBody,
    /// Per-line syntax spans for each side of the file, indexed from zero.
    old_spans: Vec<Vec<Span>>,
    new_spans: Vec<Vec<Span>>,
}

#[derive(QObject, Default)]
pub struct DiffBridge {
    base: qt_base_class!(trait QObject),

    /// The workspace key git runs against. Bound to `App.current` in QML, so a
    /// workspace switch empties the pane here rather than in every binding that
    /// reads it.
    workspace: qt_property!(QString; NOTIFY workspace_changed READ get_workspace WRITE set_workspace),
    /// "light" or "dark". Row colours are baked into the lists below, so the
    /// pane and an open sheet are both rebuilt when this changes.
    theme: qt_property!(QString; NOTIFY theme_changed READ get_theme WRITE set_theme),

    /// One entry per changed file: `{ letter, letter_color, path, path_color,
    /// adds, adds_color, dels, dels_color, tooltip }`. A plain list rather than
    /// a model — a refresh replaces the lot, and a list rebuilt whole is one
    /// that cannot disagree with itself.
    files: qt_property!(QVariantList; NOTIFY files_changed READ get_files),
    /// The totals line, as rich text: the file count, `+`/`−`, and the "(+N
    /// more)" notice when a refresh left rows out. Carries the empty and error
    /// states too — no placeholder row is faked into the list.
    summary: qt_property!(QString; NOTIFY files_changed READ get_summary),
    /// Whether a gather is in flight with nothing on screen to keep showing.
    loading: qt_property!(bool; NOTIFY files_changed READ get_loading),

    /// Whether the sheet is up. Everything below it is only meaningful then.
    viewer_open: qt_property!(bool; NOTIFY viewer_changed READ get_viewer_open),
    viewer_path: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_path),
    viewer_letter: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_letter),
    viewer_letter_color: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_letter_color),
    viewer_subtitle: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_subtitle),
    /// Set instead of rows when there is no text diff: a binary file, an
    /// unreadable one, or a diff that turned out to hold no lines.
    viewer_message: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_message),
    /// One entry per diff line: `{ old_no, new_no, mark, mark_color, background,
    /// runs }`, where `runs` are the `{ text, color, italic }` pieces the line
    /// is drawn from.
    viewer_rows: qt_property!(QVariantList; NOTIFY viewer_changed READ get_viewer_rows),
    /// How many digits wide each half of the line-number gutter has to be.
    viewer_digits: qt_property!(i32; NOTIFY viewer_changed READ get_viewer_digits),
    /// The longest row in characters, which is what the body has to be able to
    /// scroll across. Lines never wrap in a diff.
    viewer_columns: qt_property!(i32; NOTIFY viewer_changed READ get_viewer_columns),
    viewer_gutter_color: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_gutter_color),
    viewer_dim_color: qt_property!(QString; NOTIFY viewer_changed READ get_viewer_dim_color),
    /// The scrim over the app behind the sheet, alpha included: it is what
    /// makes the sheet modal, so it is a colour with transparency in it rather
    /// than a tint.
    viewer_scrim: qt_property!(QString; NOTIFY theme_changed READ get_viewer_scrim),

    workspace_changed: qt_signal!(),
    theme_changed: qt_signal!(),
    files_changed: qt_signal!(),
    viewer_changed: qt_signal!(),

    refresh: qt_method!(fn(&mut self)),
    open_file: qt_method!(fn(&mut self, index: i32)),
    close_viewer: qt_method!(fn(&mut self)),
    /// The path on one row, for the context menu's "Copy path".
    path_at: qt_method!(fn(&self, index: i32) -> QString),

    /// Bumped on every refresh; a result from an earlier gather that finished
    /// after a newer one started is dropped rather than rendered.
    generation: u64,
    entries: Vec<FileChange>,
    omitted: usize,
    /// The repo's top level, learned on each refresh. git reports porcelain
    /// paths relative to it, which is not where the workspace necessarily sits.
    root: String,
    /// The empty state and every failure, in one sentence.
    message: String,
    busy: bool,
    /// A click taken but not yet shown. Until the sheet lands there is nothing
    /// to refuse a second click, which is what a double-click sends.
    opening: bool,
    sheet: Option<Sheet>,
}

impl DiffBridge {
    pub fn new() -> Self {
        Self {
            theme: DEFAULT_THEME.into(),
            ..Default::default()
        }
    }

    // ---- Workspace and theme ---------------------------------------------

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
        // A sheet showing a file from the workspace we just left would be a
        // surprise, and its "copy path" would name a file that is no longer in
        // front of anyone.
        self.drop_sheet();
        self.entries.clear();
        self.omitted = 0;
        self.root.clear();
        self.message.clear();
        // Nothing gathered yet for this workspace, so the pane must not keep
        // showing the last one's rows while the first refresh runs.
        self.generation += 1;
        self.workspace_changed();
        self.files_changed();
        self.viewer_changed();
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
        if let Some(sheet) = &self.sheet {
            // The spans carry a colour each, so they are only right for the
            // theme they were lexed under.
            let syntax = syntax_for_filename(&sheet.document.path);
            let old_spans = line_spans(&sheet.document.old_text, syntax, &self.theme_name());
            let new_spans = line_spans(&sheet.document.new_text, syntax, &self.theme_name());
            if let Some(sheet) = &mut self.sheet {
                sheet.old_spans = old_spans;
                sheet.new_spans = new_spans;
            }
        }
        self.theme_changed();
        self.files_changed();
        self.viewer_changed();
    }

    // ---- Refresh ----------------------------------------------------------

    /// Re-read the working tree. Called when the pane becomes visible — which
    /// covers toggling it on and switching workspaces — and by the refresh
    /// button, a branch switch, and the end of an agent turn.
    fn refresh(&mut self) {
        if self.cwd().is_empty() {
            self.entries.clear();
            self.message = "No workspace".to_string();
            self.busy = false;
            self.generation += 1;
            self.files_changed();
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        // "Loading…" only while there is nothing to look at. Replacing a list
        // that is already right with a word, for the fraction of a second a
        // local gather takes, reads as a flicker rather than as progress.
        self.busy = self.entries.is_empty();
        self.files_changed();

        let runner = GitRunner::for_workspace(&self.cwd());
        let limits = DiffLimits::default();
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |data: diff::DiffData| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_gathered(generation, data);
            }
        });
        std::thread::spawn(move || deliver(diff::gather(&runner, &limits)));
    }

    fn on_gathered(&mut self, generation: u64, data: diff::DiffData) {
        // A result a newer refresh — or a workspace switch — obsoleted.
        if generation != self.generation {
            return;
        }
        self.busy = false;
        match data {
            diff::DiffData::Message(message) => {
                self.entries.clear();
                self.omitted = 0;
                self.message = message;
            }
            diff::DiffData::Files {
                files,
                omitted,
                root,
            } => {
                self.root = root;
                self.omitted = omitted;
                self.message = if files.is_empty() {
                    "No changes".to_string()
                } else {
                    String::new()
                };
                self.entries = files;
            }
        }
        self.files_changed();
    }

    fn get_loading(&self) -> bool {
        self.busy
    }

    fn get_summary(&self) -> QString {
        if self.busy {
            return diff::message_html(&self.theme_name(), "Loading…").as_str().into();
        }
        if self.entries.is_empty() {
            let message = if self.message.is_empty() {
                "No changes"
            } else {
                &self.message
            };
            return diff::message_html(&self.theme_name(), message).as_str().into();
        }
        diff::summary_html(&self.theme_name(), &self.entries, self.omitted)
            .as_str()
            .into()
    }

    fn get_files(&self) -> QVariantList {
        let theme = &self.theme_name();
        let text = diff::panel_color(theme, "text");
        let dim = diff::panel_color(theme, "dim");
        let mut list = QVariantList::default();
        for change in &self.entries {
            let letter = change.letter();
            let mut map = QVariantMap::default();
            let put = |map: &mut QVariantMap, key: &str, value: &str| {
                map.insert(key.into(), QVariant::from(QString::from(value)));
            };
            put(&mut map, "letter", &letter.to_string());
            put(&mut map, "letter_color", diff::letter_color(theme, letter));
            put(&mut map, "path", &change.path);
            // A deleted file's path is dimmed: it is no longer there to open.
            put(&mut map, "path_color", if letter == 'D' { dim } else { text });
            put(&mut map, "adds", &diff::count_text(change.adds, "+"));
            put(&mut map, "dels", &diff::count_text(change.dels, "−"));
            let counted = |count: Option<i64>, colour: &'static str| -> &'static str {
                if count.unwrap_or(0) != 0 {
                    colour
                } else {
                    dim
                }
            };
            put(&mut map, "adds_color", counted(change.adds, diff::panel_color(theme, "add")));
            put(&mut map, "dels_color", counted(change.dels, diff::panel_color(theme, "del")));
            put(&mut map, "tooltip", &diff::tooltip(change));
            list.push(map.into());
        }
        list
    }

    fn path_at(&self, index: i32) -> QString {
        match self.entries.get(index.max(0) as usize) {
            Some(change) => change.path.as_str().into(),
            None => "".into(),
        }
    }

    // ---- The sheet --------------------------------------------------------

    /// Load one file's diff and put the sheet up. One sheet at a time, so a
    /// double-click cannot stack two.
    fn open_file(&mut self, index: i32) {
        if self.opening || self.sheet.is_some() || index < 0 {
            return;
        }
        let Some(change) = self.entries.get(index as usize).cloned() else {
            return;
        };
        self.opening = true;
        let root = if self.root.is_empty() {
            self.cwd()
        } else {
            self.root.clone()
        };
        let runner = GitRunner::for_workspace(&self.cwd());
        let limits = DiffLimits::default();
        let theme = self.theme_name();
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |sheet: Box<Sheet>| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_document(*sheet);
            }
        });
        // The diff itself, both sides of the file, and the lexing of them: for
        // a remote workspace that is three SSH round trips, and even locally
        // the lexing of a large file is more than a click should cost.
        std::thread::spawn(move || {
            let document = diff::diff_document(&runner, &change, &root, &limits);
            let body = document.body();
            let syntax = syntax_for_filename(&document.path);
            let sheet = Sheet {
                old_spans: line_spans(&document.old_text, syntax, &theme),
                new_spans: line_spans(&document.new_text, syntax, &theme),
                body,
                document,
            };
            deliver(Box::new(sheet));
        });
    }

    fn on_document(&mut self, sheet: Sheet) {
        // This is where the click stops being pending, whether or not the sheet
        // ends up going up.
        self.opening = false;
        // The workspace changed while the diff was in flight, so this is a file
        // from a repository nobody is looking at any more.
        if self.cwd().is_empty() || self.sheet.is_some() {
            return;
        }
        debug::action(
            "diff.open",
            &[
                ("path", sheet.document.path.clone()),
                ("bytes", sheet.document.diff_text.len().to_string()),
            ],
        );
        self.sheet = Some(sheet);
        self.viewer_changed();
    }

    fn close_viewer(&mut self) {
        if self.sheet.is_none() {
            return;
        }
        self.drop_sheet();
        self.viewer_changed();
    }

    fn drop_sheet(&mut self) {
        self.sheet = None;
        // A click already taken is not one this sheet's closing should honour:
        // the document it asked for would open over a pane the reader has
        // moved on from.
        self.opening = false;
    }

    fn get_viewer_open(&self) -> bool {
        self.sheet.is_some()
    }

    fn get_viewer_path(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.document.path.as_str().into(),
            None => "".into(),
        }
    }

    fn get_viewer_letter(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.document.letter.to_string().as_str().into(),
            None => "".into(),
        }
    }

    fn get_viewer_letter_color(&self) -> QString {
        let letter = self.sheet.as_ref().map(|s| s.document.letter).unwrap_or(' ');
        diff::letter_color(&self.theme_name(), letter).into()
    }

    fn get_viewer_subtitle(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.document.subtitle.as_str().into(),
            None => "".into(),
        }
    }

    fn get_viewer_message(&self) -> QString {
        match &self.sheet {
            Some(sheet) => sheet.body.message.clone().unwrap_or_default().as_str().into(),
            None => "".into(),
        }
    }

    fn get_viewer_digits(&self) -> i32 {
        match &self.sheet {
            Some(sheet) => diff::gutter_digits(&sheet.body.rows) as i32,
            None => 2,
        }
    }

    fn get_viewer_columns(&self) -> i32 {
        match &self.sheet {
            Some(sheet) => sheet
                .body
                .rows
                .iter()
                .map(|row| row.text.chars().count())
                .max()
                .unwrap_or(0) as i32,
            None => 0,
        }
    }

    fn get_viewer_gutter_color(&self) -> QString {
        diff::viewer_color(&self.theme_name(), "gutter").into()
    }

    fn get_viewer_dim_color(&self) -> QString {
        diff::viewer_color(&self.theme_name(), "dim").into()
    }

    fn get_viewer_scrim(&self) -> QString {
        let (r, g, b, a) = diff::scrim(&self.theme_name());
        format!("#{a:02x}{r:02x}{g:02x}{b:02x}").as_str().into()
    }

    fn get_viewer_rows(&self) -> QVariantList {
        let Some(sheet) = &self.sheet else {
            return QVariantList::default();
        };
        let theme = &self.theme_name();
        let base = diff::viewer_color(theme, "text");
        let dim = diff::viewer_color(theme, "dim");
        let mut list = QVariantList::default();
        for row in &sheet.body.rows {
            let mut map = QVariantMap::default();
            let put = |map: &mut QVariantMap, key: &str, value: &str| {
                map.insert(key.into(), QVariant::from(QString::from(value)));
            };
            put(&mut map, "old_no", &number(row.old_no));
            put(&mut map, "new_no", &number(row.new_no));
            put(&mut map, "background", tint(theme, row.kind));
            match row.kind {
                // Structural lines carry no code to highlight, so the whole
                // line goes in as one dim run.
                RowKind::Hunk | RowKind::Note => {
                    put(&mut map, "mark", "");
                    put(&mut map, "mark_color", dim);
                    map.insert("runs".into(), QVariant::from(one_run(&row.text, dim)));
                }
                _ => {
                    let mut chars = row.text.chars();
                    let mark = chars.next().map(String::from).unwrap_or_default();
                    let body = chars.as_str();
                    put(&mut map, "mark", &mark);
                    put(&mut map, "mark_color", mark_color(theme, row.kind));
                    let spans = match diff::span_source(row) {
                        Some((diff::Side::Old, index)) => sheet.old_spans.get(index),
                        Some((diff::Side::New, index)) => sheet.new_spans.get(index),
                        None => None,
                    };
                    let runs = runs_for(body, spans.map(Vec::as_slice).unwrap_or(&[]), base);
                    map.insert("runs".into(), QVariant::from(runs));
                }
            }
            list.push(map.into());
        }
        list
    }
}

/// A line number as the gutter shows it, or "" for the side the line does not
/// exist on.
fn number(value: Option<u32>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

/// The wash behind a changed line, or "" for one that is drawn on the card
/// itself.
fn tint(theme: &str, kind: RowKind) -> &'static str {
    match kind {
        RowKind::Add => diff::viewer_color(theme, "add_bg"),
        RowKind::Del => diff::viewer_color(theme, "del_bg"),
        RowKind::Hunk => diff::viewer_color(theme, "hunk_bg"),
        _ => "",
    }
}

/// The colour of the `+`/`-` in the first column, which is the one part of a
/// code row that says what happened to it.
fn mark_color(theme: &str, kind: RowKind) -> &'static str {
    match kind {
        RowKind::Add => diff::viewer_color(theme, "add"),
        RowKind::Del => diff::viewer_color(theme, "del"),
        _ => diff::viewer_color(theme, "text"),
    }
}

fn one_run(text: &str, color: &str) -> QVariantList {
    let mut runs = QVariantList::default();
    push_run(&mut runs, text, color, false);
    runs
}

fn push_run(runs: &mut QVariantList, text: &str, color: &str, italic: bool) {
    if text.is_empty() {
        return;
    }
    let mut map = QVariantMap::default();
    map.insert(
        "text".into(),
        QVariant::from(QString::from(text.replace('\t', &" ".repeat(TAB_WIDTH)))),
    );
    map.insert("color".into(), QVariant::from(QString::from(color)));
    map.insert("italic".into(), italic.into());
    runs.push(map.into());
}

/// One line split into the pieces it is drawn from: the lexer's runs, and the
/// gaps between them in the plain text colour.
///
/// The gaps have to be carried through as runs of their own — a list that held
/// only the coloured stretches would draw a line with holes in it, since the
/// view lays the pieces out one after another rather than positioning each at
/// its own column.
fn runs_for(body: &str, spans: &[Span], base: &str) -> QVariantList {
    let mut runs = QVariantList::default();
    let mut column = 0usize;
    for span in spans {
        if span.start >= body.len() {
            break;
        }
        let end = span.end.min(body.len());
        // A span that does not land on a character boundary means the file
        // changed under us between two git calls; the line still renders, just
        // without that run's colour.
        if span.start < column || !body.is_char_boundary(span.start) || !body.is_char_boundary(end) {
            continue;
        }
        push_run(&mut runs, &body[column..span.start], base, false);
        push_run(&mut runs, &body[span.start..end], span.color, span.italic);
        column = end;
    }
    push_run(&mut runs, &body[column..], base, false);
    runs
}
