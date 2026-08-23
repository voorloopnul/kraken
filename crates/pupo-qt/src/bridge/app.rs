//! The application object: what is open, what is showing, and the actions the
//! chrome fires.
//!
//! This is the one QObject the window's own furniture talks to. It owns the
//! list of workspaces, which one is current, and each workspace's panel
//! visibility — the last of those per workspace rather than globally, because
//! a pane you opened in one project is not a pane you asked for in the next.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pupo_core::{debug, external, git, remote, state, workspace};
use qmetaobject::*;
use serde_json::{json, Value};

/// The panes a workspace can show, in the order the side strip lists them.
/// "left" (History) is the one that starts open — it is the only pane that
/// costs nothing to have open and the only one you need before you have asked
/// the agent anything.
pub const PANEL_KEYS: [&str; 5] = ["left", "files", "browser", "git", "right"];

fn default_panels() -> HashMap<String, bool> {
    PANEL_KEYS
        .iter()
        .map(|key| ((*key).to_string(), *key == "left"))
        .collect()
}

#[derive(QObject, Default)]
pub struct App {
    base: qt_base_class!(trait QObject),

    /// One entry per open workspace: `{ key, label, tooltip, remote, colour
    /// faces, active }`. A plain list rather than a model — the strip holds a
    /// handful of tiles, and a list that is rebuilt whole is one that cannot
    /// disagree with itself.
    workspaces: qt_property!(QVariantList; NOTIFY workspaces_changed READ get_workspaces),
    /// The current workspace's key, or "" on the home screen.
    current: qt_property!(QString; NOTIFY current_changed READ get_current),
    /// The path shown under the conversation title: home-relative for a local
    /// folder, `user@host:/path` for a remote one.
    workspace_label: qt_property!(QString; NOTIFY current_changed READ get_workspace_label),
    /// Whether the current workspace lives on another machine.
    is_remote: qt_property!(bool; NOTIFY current_changed READ get_is_remote),
    /// The focused conversation's title, or "" when none is chosen.
    conversation: qt_property!(QString; NOTIFY conversation_changed READ get_conversation),
    /// The current workspace's git branch, or "" when it is not in a repo.
    branch: qt_property!(QString; NOTIFY chrome_changed READ get_branch),
    /// The process tree's resident set, as the title bar reports it.
    memory_label: qt_property!(QString; NOTIFY chrome_changed READ get_memory_label),
    /// The process table behind that number: `{ name, pid, memory }` per row,
    /// this process first and its children after it.
    ///
    /// Sampled on demand rather than kept fresh — walking `/proc` is not free,
    /// and nobody is reading the table while the dialog is closed.
    processes: qt_property!(QVariantList; NOTIFY processes_changed READ get_processes),
    /// The line under it: how many processes, and what they hold between them.
    process_summary: qt_property!(QString; NOTIFY processes_changed READ get_process_summary),

    workspaces_changed: qt_signal!(),
    current_changed: qt_signal!(),
    conversation_changed: qt_signal!(),
    panels_changed: qt_signal!(),
    chrome_changed: qt_signal!(),
    processes_changed: qt_signal!(),

    add_workspace: qt_method!(fn(&mut self, path: QString)),
    select_workspace: qt_method!(fn(&mut self, key: QString)),
    remove_workspace: qt_method!(fn(&mut self, key: QString)),
    set_workspace_active: qt_method!(fn(&mut self, key: QString, active: bool)),
    /// A tile's colour for one of its four faces, so QML never has to know the
    /// hue table.
    ///
    /// The theme is an argument rather than something this object remembers, so
    /// a binding on it repaints when the theme changes: QML records a dependency
    /// on a property it reads, and `Theme.name` in the call is that read. An
    /// object holding its own copy would repaint nothing.
    tile_color: qt_method!(fn(&self, key: QString, theme: QString, checked: bool, hovered: bool) -> QString),
    indicator_color: qt_method!(fn(&self, theme: QString) -> QString),

    is_panel_visible: qt_method!(fn(&self, side: QString) -> bool),
    set_panel_visible: qt_method!(fn(&mut self, side: QString, visible: bool)),
    toggle_panel: qt_method!(fn(&mut self, side: QString)),

    set_conversation: qt_method!(fn(&mut self, title: QString)),
    /// Re-read the live readouts. Called from a slow timer in QML: memory
    /// always drifts, and the branch can change under us — a checkout in the
    /// terminal is the usual way.
    refresh_chrome: qt_method!(fn(&mut self)),
    /// Re-walk the process tree. The dialog calls it as it opens and on its own
    /// Refresh; children are what the app leaks — an agent per workspace, a
    /// shell per terminal — so the number that matters is the tree's, not ours.
    sample_processes: qt_method!(fn(&mut self)),
    /// The repository's local branches, for the switcher's menu.
    /// The applications the "open in" menu offers, and whether this workspace
    /// is one they could be pointed at — a remote workspace's anchor is a local
    /// stand-in with none of the project in it.
    external_apps: qt_method!(fn(&self) -> QVariantList),
    can_open_externally: qt_method!(fn(&self) -> bool),
    /// Hand the workspace folder to another application. Detached: it is
    /// somebody else's window from the moment it starts, and a child that
    /// outlives us is better than one we have to reap.
    open_externally: qt_method!(fn(&mut self, app: QString)),

    branches: qt_method!(fn(&self) -> QVariantList),
    /// Check one out. Returns "" on success, or the message git failed with —
    /// a dirty worktree is the common case and the reader needs to be told.
    checkout: qt_method!(fn(&mut self, branch: QString) -> QString),

    entries: Vec<Entry>,
    current_key: String,
    conversation_title: String,
    /// Per workspace key, per panel key.
    panels: HashMap<String, HashMap<String, bool>>,
    branch_text: String,
    memory_text: String,
    /// The last process sample, held so the property getter is a copy rather
    /// than a walk of `/proc` on every read.
    process_rows: Vec<debug::ProcessMemory>,
}

#[derive(Debug, Clone)]
struct Entry {
    key: String,
    label: String,
    tooltip: String,
    remote: bool,
    active: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            ..Default::default()
        };
        app.restore();
        app.reread_chrome();
        app
    }

    fn reread_chrome(&mut self) {
        self.branch_text = if self.current_key.is_empty() {
            String::new()
        } else {
            // Reads .git/HEAD directly rather than spawning git, which is what
            // makes it cheap enough to poll.
            git::git_branch(&self.current_key)
        };
        self.memory_text = debug::format_bytes(debug::process_tree_rss());
    }

    fn refresh_chrome(&mut self) {
        let branch = self.branch_text.clone();
        let memory = self.memory_text.clone();
        self.reread_chrome();
        if branch != self.branch_text || memory != self.memory_text {
            self.chrome_changed();
        }
    }

    fn get_branch(&self) -> QString {
        self.branch_text.as_str().into()
    }

    fn get_memory_label(&self) -> QString {
        self.memory_text.as_str().into()
    }

    fn external_apps(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for app in external::APPS {
            list.push(QVariant::from(QString::from(app)));
        }
        list
    }

    fn can_open_externally(&self) -> bool {
        external::openable(&self.current_key)
    }

    fn open_externally(&mut self, app: QString) {
        let app = app.to_string();
        let Some(argv) = external::app_argv(&app, &self.current_key) else {
            return;
        };
        let Some((program, args)) = argv.split_first() else {
            return;
        };
        debug::action("workspace.open-external", &[("app", app)]);
        match std::process::Command::new(program)
            .args(args)
            .current_dir(&self.current_key)
            .spawn()
        {
            // Not waited on: it is somebody else's window from here, and the
            // zombie it leaves is reaped by init once we are not looking.
            Ok(_) => {}
            Err(error) => debug::error(
                "workspace.open-external",
                &[("error", error.to_string())],
            ),
        }
    }

    fn sample_processes(&mut self) {
        self.process_rows = debug::process_tree();
        self.processes_changed();
    }

    fn get_processes(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for (index, row) in self.process_rows.iter().enumerate() {
            let mut map = QVariantMap::default();
            // The first row is this process, which is the app rather than one
            // of the programs it started.
            let name = if index == 0 { "Pupo" } else { row.name.as_str() };
            map.insert("name".into(), QVariant::from(QString::from(name)));
            map.insert("pid".into(), QVariant::from(row.pid));
            map.insert(
                "memory".into(),
                QVariant::from(QString::from(debug::format_bytes(row.rss).as_str())),
            );
            list.push(map.into());
        }
        list
    }

    fn get_process_summary(&self) -> QString {
        let total: u64 = self.process_rows.iter().map(|row| row.rss).sum();
        let count = self.process_rows.len();
        format!(
            "{count} {} · {} resident",
            if count == 1 { "process" } else { "processes" },
            debug::format_bytes(total)
        )
        .as_str()
        .into()
    }

    fn runner(&self) -> git::GitRunner {
        git::GitRunner::for_workspace(&self.current_key)
    }

    fn branches(&self) -> QVariantList {
        let mut list = QVariantList::default();
        if self.current_key.is_empty() {
            return list;
        }
        for branch in git::local_branches(&self.runner()) {
            list.push(QVariant::from(QString::from(branch.as_str())));
        }
        list
    }

    fn checkout(&mut self, branch: QString) -> QString {
        let branch = branch.to_string();
        if self.current_key.is_empty() || branch.is_empty() {
            return "".into();
        }
        let argv = git::checkout_argv(&branch);
        let result = self
            .runner()
            .run(&argv, std::time::Duration::from_secs(20));
        let error = match result {
            Some(output) if output.ok() => String::new(),
            Some(output) => {
                let text = output.stderr.trim();
                if text.is_empty() {
                    "git could not switch branch.".to_string()
                } else {
                    text.to_string()
                }
            }
            None => "git could not be run.".to_string(),
        };
        if error.is_empty() {
            self.reread_chrome();
            self.chrome_changed();
        }
        error.as_str().into()
    }

    /// Reopen the workspaces from the last run. Folders that have vanished are
    /// dropped; a first launch falls back to the launch directory, so the
    /// window starts with something live in it rather than on an empty screen
    /// the user has to populate before anything works.
    fn restore(&mut self) {
        let mut keys: Vec<String> = state::string_list("workspaces")
            .into_iter()
            .filter(|p| Path::new(p).is_dir())
            .collect();
        // A first run opens the folder it was started in, which is almost
        // always the project someone means. An *empty* list is not that: it is
        // someone who removed their last workspace, and re-adding the cwd would
        // undo it every launch. The difference is whether the key exists at all.
        if keys.is_empty() && state::get("workspaces").is_none() {
            if let Ok(cwd) = std::env::current_dir() {
                keys.push(cwd.to_string_lossy().into_owned());
            }
        }
        for key in keys {
            self.push_entry(&key);
        }
        let stored = state::string("current_workspace").unwrap_or_default();
        let current = if self.entries.iter().any(|e| e.key == stored) {
            stored
        } else {
            self.entries.first().map(|e| e.key.clone()).unwrap_or_default()
        };
        self.current_key = current;
        self.load_panels();
    }

    fn load_panels(&mut self) {
        let stored = state::object("panels");
        self.panels = stored
            .iter()
            .map(|(key, value)| {
                let mut sides = default_panels();
                if let Value::Object(map) = value {
                    for side in PANEL_KEYS {
                        if let Some(Value::Bool(open)) = map.get(side) {
                            sides.insert(side.to_string(), *open);
                        }
                    }
                }
                (key.clone(), sides)
            })
            .collect();
    }

    fn save_panels(&self) {
        let map: serde_json::Map<String, Value> = self
            .panels
            .iter()
            .map(|(key, sides)| {
                let entry: serde_json::Map<String, Value> = sides
                    .iter()
                    .map(|(side, open)| (side.clone(), json!(open)))
                    .collect();
                (key.clone(), Value::Object(entry))
            })
            .collect();
        state::set("panels", Value::Object(map));
    }

    fn push_entry(&mut self, key: &str) {
        if self.entries.iter().any(|e| e.key == key) {
            return;
        }
        let entry = match remote::resolve(key) {
            Some(target) => Entry {
                key: key.to_string(),
                label: workspace::abbreviation(&target.host.host_id),
                tooltip: format!("{}:{}", target.host.destination(), target.path),
                remote: true,
                active: false,
            },
            None => Entry {
                key: key.to_string(),
                label: workspace::abbreviation_for_path(key),
                tooltip: key.to_string(),
                remote: false,
                active: false,
            },
        };
        self.entries.push(entry);
    }

    fn persist(&self) {
        let keys: Vec<Value> = self.entries.iter().map(|e| json!(e.key)).collect();
        state::save([
            ("workspaces".to_string(), Value::Array(keys)),
            (
                "current_workspace".to_string(),
                if self.current_key.is_empty() {
                    Value::Null
                } else {
                    json!(self.current_key)
                },
            ),
        ]);
    }

    fn get_workspaces(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for entry in &self.entries {
            let mut map = QVariantMap::default();
            let text = |value: &str| QVariant::from(QString::from(value));
            map.insert("key".into(), text(&entry.key));
            map.insert("label".into(), text(&entry.label));
            map.insert("tooltip".into(), text(&entry.tooltip));
            map.insert("remote".into(), entry.remote.into());
            map.insert("active".into(), entry.active.into());
            map.insert("current".into(), (entry.key == self.current_key).into());
            list.push(map.into());
        }
        list
    }

    fn get_current(&self) -> QString {
        self.current_key.as_str().into()
    }

    fn get_is_remote(&self) -> bool {
        self.entries
            .iter()
            .find(|e| e.key == self.current_key)
            .map(|e| e.remote)
            .unwrap_or(false)
    }

    fn get_workspace_label(&self) -> QString {
        match self.entries.iter().find(|e| e.key == self.current_key) {
            Some(entry) if entry.remote => entry.tooltip.as_str().into(),
            Some(entry) => git::home_relative(&entry.key).as_str().into(),
            None => "".into(),
        }
    }

    fn get_conversation(&self) -> QString {
        self.conversation_title.as_str().into()
    }

    fn set_conversation(&mut self, title: QString) {
        let title = title.to_string();
        if title != self.conversation_title {
            self.conversation_title = title;
            self.conversation_changed();
        }
    }

    fn add_workspace(&mut self, path: QString) {
        let path = path.to_string();
        let path = path.strip_prefix("file://").unwrap_or(&path).to_string();
        let key = PathBuf::from(&path)
            .canonicalize()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(path);
        if key.is_empty() {
            return;
        }
        self.push_entry(&key);
        self.current_key = key;
        self.persist();
        self.reread_chrome();
        self.workspaces_changed();
        self.current_changed();
        self.chrome_changed();
    }

    fn select_workspace(&mut self, key: QString) {
        let key = key.to_string();
        if key == self.current_key || !self.entries.iter().any(|e| e.key == key) {
            return;
        }
        self.current_key = key;
        self.persist();
        self.reread_chrome();
        self.workspaces_changed();
        self.current_changed();
        self.panels_changed();
        self.chrome_changed();
    }

    fn remove_workspace(&mut self, key: QString) {
        let key = key.to_string();
        let before = self.entries.len();
        self.entries.retain(|e| e.key != key);
        if self.entries.len() == before {
            return;
        }
        self.panels.remove(&key);
        // A remote workspace is also dropped from stored state; its local
        // anchor folder is left in place, so a re-add reuses any sessions
        // already stored under it.
        if remote::resolve(&key).is_some() {
            remote::remove_remote_workspace(&key);
        }
        if self.current_key == key {
            self.current_key = self.entries.first().map(|e| e.key.clone()).unwrap_or_default();
        }
        self.persist();
        self.save_panels();
        self.workspaces_changed();
        self.current_changed();
        self.panels_changed();
    }

    fn set_workspace_active(&mut self, key: QString, active: bool) {
        let key = key.to_string();
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            if entry.active != active {
                entry.active = active;
                self.workspaces_changed();
            }
        }
    }

    fn tile_color(&self, key: QString, theme: QString, checked: bool, hovered: bool) -> QString {
        workspace::tile_color(&key.to_string(), &theme.to_string(), checked, hovered)
            .as_str()
            .into()
    }

    fn indicator_color(&self, theme: QString) -> QString {
        workspace::indicator_color(&theme.to_string()).into()
    }

    fn is_panel_visible(&self, side: QString) -> bool {
        self.panel_state(&side.to_string())
    }

    fn panel_state(&self, side: &str) -> bool {
        self.panels
            .get(&self.current_key)
            .and_then(|sides| sides.get(side).copied())
            .unwrap_or_else(|| side == "left")
    }

    fn set_panel_visible(&mut self, side: QString, visible: bool) {
        let side = side.to_string();
        if self.current_key.is_empty() || !PANEL_KEYS.contains(&side.as_str()) {
            return;
        }
        let sides = self
            .panels
            .entry(self.current_key.clone())
            .or_insert_with(default_panels);
        if sides.get(&side).copied() == Some(visible) {
            return;
        }
        sides.insert(side, visible);
        self.save_panels();
        self.panels_changed();
    }

    fn toggle_panel(&mut self, side: QString) {
        let open = self.panel_state(&side.to_string());
        self.set_panel_visible(side, !open);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_the_only_pane_that_starts_open() {
        let panels = default_panels();
        assert_eq!(panels["left"], true);
        for side in ["files", "browser", "git", "right"] {
            assert_eq!(panels[side], false, "{side} should start closed");
        }
        assert_eq!(panels.len(), PANEL_KEYS.len());
    }
}
