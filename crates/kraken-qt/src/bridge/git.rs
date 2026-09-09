//! The git pane, as QML sees it: the workspace repo's commit graph.
//!
//! Rows come out of the core already rendered to rich text, because a row is
//! not one colour — the hash is green on the main line and grey off it, while
//! the graph columns and the subject keep the plain text colour. One string per
//! row is what lets the view stay a plain list instead of a per-row layout that
//! would have to know that rule.
//!
//! Like the diff pane, every git call runs on a worker thread: `git log` on a
//! remote workspace is an SSH round trip, and a checkout is one that also
//! writes files. Commit and push share that worker path and keep their draft,
//! progress and result with the workspace that requested them.

use std::collections::HashMap;

use kraken_core::git::{self, GitRunner, LogData, LogRow, MAX_COMMITS};
use kraken_core::theme::DEFAULT_THEME;
use qmetaobject::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mutation {
    Commit {
        message: String,
        paths: Vec<String>,
    },
    Push,
    Checkout(String),
}

impl Mutation {
    fn progress(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "Committing…",
            Self::Push => "Pushing…",
            Self::Checkout(_) => "Checking out…",
        }
    }

    fn success(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "Committed selected files.",
            Self::Push => "Push completed.",
            Self::Checkout(_) => "Checkout completed.",
        }
    }

    fn run(&self, runner: &GitRunner) -> Result<(), String> {
        match self {
            Self::Commit { message, paths } => git::commit_selected(runner, message, paths),
            Self::Push => git::push(runner),
            Self::Checkout(target) => git::checkout(runner, target),
        }
    }
}

/// Per-workspace rather than per-view: switching away during a push must not
/// clear another repository's draft or unlock a second write to the first one.
#[derive(Default)]
struct Actions {
    draft: String,
    active: Option<Mutation>,
    status: String,
    error: bool,
}

impl Actions {
    fn begin(&mut self, operation: Mutation) -> bool {
        if self.active.is_some() {
            return false;
        }
        if matches!(&operation, Mutation::Commit { message, paths } if message.trim().is_empty() || paths.is_empty()) {
            return false;
        }
        self.status = operation.progress().into();
        self.error = false;
        self.active = Some(operation);
        true
    }

    fn finish(&mut self, outcome: &Result<(), String>) -> Option<Mutation> {
        let operation = self.active.take()?;
        self.error = outcome.is_err();
        self.status = match outcome {
            Ok(()) => {
                if let Mutation::Commit { message, .. } = &operation {
                    // Do not erase an edit made after the request was sent.
                    if self.draft == *message {
                        self.draft.clear();
                    }
                }
                operation.success().into()
            }
            Err(error) => error.clone(),
        };
        Some(operation)
    }
}

#[derive(QObject, Default)]
pub struct GitBridge {
    base: qt_base_class!(trait QObject),

    /// The workspace key git runs against, bound to `App.current` in QML.
    workspace: qt_property!(QString; NOTIFY workspace_changed READ get_workspace WRITE set_workspace),
    /// "light" or "dark". Row colours are baked into each row's rich text, so
    /// the list is rebuilt when this changes.
    theme: qt_property!(QString; NOTIFY theme_changed READ get_theme WRITE set_theme),

    /// One entry per `git log --graph` line: `{ html, tooltip, short_hash,
    /// full_hash, branches, is_head }`. A row with an empty `short_hash` is a
    /// pure graph line like `|/`, which carries nothing to act on.
    rows: qt_property!(QVariantList; NOTIFY rows_changed READ get_rows),
    /// The one sentence shown instead of rows: no HEAD yet, or not a repo.
    /// Empty while there are rows to show.
    message: qt_property!(QString; NOTIFY rows_changed READ get_message),
    message_color: qt_property!(QString; NOTIFY rows_changed READ get_message_color),

    /// The branch the workspace is on, or "" outside a repository. Read from
    /// `.git/HEAD`, which is cheap enough to poll — that is how both panes
    /// notice a branch switched from the terminal rather than from here.
    branch: qt_property!(QString; NOTIFY branch_changed READ get_branch),

    // QML's property declaration needs a field, but its value lives in the
    // per-workspace draft rather than in this generated storage slot.
    #[allow(dead_code)]
    commit_message: qt_property!(QString; NOTIFY action_changed READ get_commit_message WRITE set_commit_message),
    action_busy: qt_property!(bool; NOTIFY action_changed READ get_action_busy),
    action_status: qt_property!(QString; NOTIFY action_changed READ get_action_status),
    action_error: qt_property!(bool; NOTIFY action_changed READ get_action_error),
    action_changed: qt_signal!(),
    /// A write completed, even if it failed after staging or a hook ran.
    repository_changed: qt_signal!(),
    commit: qt_method!(fn(&mut self, workspace: QString, paths: QVariantList)),
    push: qt_method!(fn(&mut self)),

    workspace_changed: qt_signal!(),
    theme_changed: qt_signal!(),
    rows_changed: qt_signal!(),
    /// HEAD moved. The diff pane listens for this too: its whole answer is
    /// "since the last commit", and a checkout changes what that means.
    branch_changed: qt_signal!(),
    /// A checkout that git refused, carrying git's own words.
    checkout_failed: qt_signal!(message: QString),

    refresh: qt_method!(fn(&mut self)),
    /// Re-read `.git/HEAD` and announce a move. Polled while the pane is up.
    poll_branch: qt_method!(fn(&mut self)),
    /// Check a branch or a commit out; `checkout_failed` carries the refusal.
    checkout: qt_method!(fn(&mut self, target: QString)),

    /// Bumped on every refresh; a result from an earlier gather that finished
    /// after a newer one started is dropped rather than rendered.
    generation: u64,
    entries: Vec<LogRow>,
    note: String,
    branch_name: String,
    actions: HashMap<String, Actions>,
}

impl GitBridge {
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
        self.entries.clear();
        self.note.clear();
        // The old repo's rows must not sit under the new workspace's name
        // while the first gather runs.
        self.generation += 1;
        self.workspace_changed();
        self.action_changed();
        self.rows_changed();
        self.poll_branch();
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
        self.rows_changed();
    }

    // ---- Refresh ----------------------------------------------------------

    /// Re-read the log. Called when the pane becomes visible — which covers
    /// toggling it on and switching workspaces — and by the refresh button and
    /// a checkout made from the row menu.
    fn refresh(&mut self) {
        if self.cwd().is_empty() {
            self.entries.clear();
            self.note.clear();
            self.generation += 1;
            self.rows_changed();
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        if self.entries.is_empty() {
            // "Loading…" only while there is nothing to look at: swapping a
            // list that is already right for a word reads as a flicker.
            self.note = "Loading…".to_string();
            self.rows_changed();
        }
        let runner = GitRunner::for_workspace(&self.cwd());
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |data: LogData| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_gathered(generation, data);
            }
        });
        std::thread::spawn(move || deliver(git::gather_log(&runner, MAX_COMMITS)));
    }

    fn on_gathered(&mut self, generation: u64, data: LogData) {
        // A result a newer refresh — or a workspace switch — obsoleted.
        if generation != self.generation {
            return;
        }
        match data {
            LogData::Message(message) => {
                self.entries.clear();
                self.note = message;
            }
            LogData::Rows(rows) => {
                self.entries = rows;
                self.note.clear();
            }
        }
        self.rows_changed();
    }

    fn get_message(&self) -> QString {
        if self.entries.is_empty() && self.note.is_empty() {
            return "No commits".into();
        }
        self.note.as_str().into()
    }

    fn get_message_color(&self) -> QString {
        git::log_text_color(&self.theme_name()).into()
    }

    fn get_rows(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for row in &self.entries {
            let mut map = QVariantMap::default();
            let put = |map: &mut QVariantMap, key: &str, value: &str| {
                map.insert(key.into(), QVariant::from(QString::from(value)));
            };
            put(&mut map, "html", &row.html(&self.theme_name()));
            match &row.commit {
                Some(commit) => {
                    put(&mut map, "tooltip", &commit.tooltip());
                    put(&mut map, "short_hash", &commit.short_hash);
                    put(&mut map, "full_hash", &commit.full_hash);
                    map.insert("is_head".into(), commit.is_head.into());
                    let mut branches = QVariantList::default();
                    for branch in &commit.branches {
                        branches.push(QVariant::from(QString::from(branch.as_str())));
                    }
                    map.insert("branches".into(), QVariant::from(branches));
                }
                None => {
                    put(&mut map, "tooltip", "");
                    put(&mut map, "short_hash", "");
                    put(&mut map, "full_hash", "");
                    map.insert("is_head".into(), false.into());
                    map.insert("branches".into(), QVariant::from(QVariantList::default()));
                }
            }
            list.push(map.into());
        }
        list
    }

    // ---- Branch -----------------------------------------------------------

    fn get_branch(&self) -> QString {
        self.branch_name.as_str().into()
    }

    fn poll_branch(&mut self) {
        // A remote workspace has no `.git` on this machine to read, and one SSH
        // round trip per tick is not a poll anyone wants; its branch moves only
        // through the checkout below, which announces itself.
        let branch = if self.cwd().is_empty() {
            String::new()
        } else {
            git::git_branch(&self.cwd())
        };
        if branch != self.branch_name {
            self.branch_name = branch;
            self.branch_changed();
        }
    }

    fn checkout(&mut self, target: QString) {
        self.start_mutation(Mutation::Checkout(target.to_string()));
    }

    // ---- Commit and push -------------------------------------------------

    fn get_commit_message(&self) -> QString {
        self.actions
            .get(&self.cwd())
            .map_or("", |state| state.draft.as_str())
            .into()
    }

    fn set_commit_message(&mut self, message: QString) {
        let cwd = self.cwd();
        if cwd.is_empty() {
            return;
        }
        let state = self.actions.entry(cwd).or_default();
        let message = message.to_string();
        if state.draft != message {
            state.draft = message;
            self.action_changed();
        }
    }

    fn get_action_busy(&self) -> bool {
        self.actions
            .get(&self.cwd())
            .is_some_and(|state| state.active.is_some())
    }

    fn get_action_status(&self) -> QString {
        self.actions
            .get(&self.cwd())
            .map_or("", |state| state.status.as_str())
            .into()
    }

    fn get_action_error(&self) -> bool {
        self.actions
            .get(&self.cwd())
            .is_some_and(|state| state.error)
    }

    fn commit(&mut self, workspace: QString, paths: QVariantList) {
        if workspace.to_string() != self.cwd() {
            return; // The selection must belong to the repository being committed.
        }
        let paths = (0..paths.len())
            .map(|index| paths[index].to_qstring().to_string())
            .collect();
        self.start_mutation(Mutation::Commit {
            message: self.get_commit_message().to_string(),
            paths,
        });
    }

    fn push(&mut self) {
        self.start_mutation(Mutation::Push);
    }

    fn start_mutation(&mut self, operation: Mutation) {
        let cwd = self.cwd();
        if cwd.is_empty()
            || !self
                .actions
                .entry(cwd.clone())
                .or_default()
                .begin(operation.clone())
        {
            return;
        }
        self.action_changed();
        let runner = GitRunner::for_workspace(&cwd);
        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |outcome: Result<(), String>| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_mutation(&cwd, outcome);
            }
        });
        std::thread::spawn(move || deliver(operation.run(&runner)));
    }

    fn on_mutation(&mut self, cwd: &str, outcome: Result<(), String>) {
        let operation = self
            .actions
            .get_mut(cwd)
            .and_then(|state| state.finish(&outcome));
        if cwd != self.cwd() {
            return; // The result belongs to the workspace that started it.
        }
        self.action_changed();
        if matches!(operation, Some(Mutation::Checkout(_))) {
            if let Err(message) = &outcome {
                self.checkout_failed(message.as_str().into());
            }
        }
        // A failed hook may still have staged or written files. Refresh both
        // views after any outcome, not just a successful change to HEAD.
        self.poll_branch();
        self.repository_changed();
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_request(message: &str) -> Mutation {
        Mutation::Commit {
            message: message.into(),
            paths: vec!["file.txt".into()],
        }
    }

    #[test]
    fn writes_are_serialized_until_the_worker_finishes() {
        let mut state = Actions::default();
        assert!(state.begin(Mutation::Push));
        assert_eq!(state.status, "Pushing…");
        assert!(!state.begin(commit_request("message")));
        assert!(!state.begin(Mutation::Checkout("main".into())));
        state.finish(&Ok(()));
        assert!(state.active.is_none());
        assert!(state.begin(commit_request("message")));
    }

    #[test]
    fn blank_messages_never_start_a_write() {
        let mut state = Actions::default();
        assert!(!state.begin(commit_request(" \n\t")));
        assert!(!state.begin(Mutation::Commit {
            message: "Message".into(),
            paths: Vec::new(),
        }));
        assert!(state.active.is_none());
    }

    #[test]
    fn only_a_successful_commit_clears_its_original_draft() {
        let mut state = Actions {
            draft: "Subject\n\nBody".into(),
            ..Default::default()
        };
        state.begin(commit_request(&state.draft));
        state.finish(&Err("hook rejected".into()));
        assert_eq!(state.draft, "Subject\n\nBody");
        assert!(state.error);
        assert_eq!(state.status, "hook rejected");

        state.begin(Mutation::Push);
        assert!(!state.error);
        state.finish(&Ok(()));
        assert_eq!(state.draft, "Subject\n\nBody");

        state.begin(commit_request(&state.draft));
        state.finish(&Ok(()));
        assert!(state.draft.is_empty());
        assert_eq!(state.status, "Committed selected files.");
    }

    #[test]
    fn a_completed_commit_does_not_erase_a_newer_draft() {
        let mut state = Actions {
            draft: "First".into(),
            ..Default::default()
        };
        state.begin(commit_request(&state.draft));
        state.draft = "Next".into();
        state.finish(&Ok(()));
        assert_eq!(state.draft, "Next");
    }

    #[test]
    fn a_background_result_stays_with_its_workspace() {
        let mut workspaces = HashMap::<String, Actions>::new();
        let first = workspaces.entry("/first".into()).or_default();
        first.draft = "First repository".into();
        first.begin(commit_request(&first.draft));
        let second = workspaces.entry("/second".into()).or_default();
        second.draft = "Second repository".into();
        second.begin(Mutation::Push);

        workspaces.get_mut("/first").unwrap().finish(&Ok(()));
        assert!(workspaces["/first"].draft.is_empty());
        assert!(workspaces["/first"].active.is_none());
        assert_eq!(workspaces["/second"].draft, "Second repository");
        assert_eq!(workspaces["/second"].active, Some(Mutation::Push));
        assert_eq!(workspaces["/second"].status, "Pushing…");
    }
}
