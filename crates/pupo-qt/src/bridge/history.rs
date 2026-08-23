//! The History pane's list of sessions.
//!
//! Two sources, deduplicated by path: the sessions pi has written to disk for
//! this workspace, and the live ones the workspace is holding. A session that pi
//! has not persisted yet still needs a row — otherwise a turn that is streaming
//! right now would be unreachable the moment you clicked away from it.

use std::path::Path;

use pupo_core::pi::sessions::{self, PiSession};
use qmetaobject::*;
use serde_json::Value;

/// A live, in-flight session the workspace is holding, before or beside its
/// file on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Live {
    key: String,
    title: String,
    /// Empty until pi names the session's file.
    path: String,
    running: bool,
}

#[derive(QObject, Default)]
pub struct HistoryBridge {
    base: qt_base_class!(trait QObject),

    /// One entry per row: `{ key, title, subtitle, tooltip, session_id, live,
    /// status, pinned }`, where `status` is "running", "unseen" or "".
    ///
    /// The pinned rows are not in here — they are their own list, under their
    /// own heading, so the pane can show the two groups without asking the
    /// delegate to know which group it is in.
    sessions: qt_property!(QVariantList; NOTIFY sessions_changed READ get_sessions),
    /// The pinned rows, same shape, newest first among themselves.
    pinned: qt_property!(QVariantList; NOTIFY sessions_changed READ get_pinned),
    /// The row that is open, by key, or "".
    selected: qt_property!(QString; NOTIFY selection_changed READ get_selected),

    sessions_changed: qt_signal!(),
    selection_changed: qt_signal!(),
    /// A row was clicked: load that session.
    session_activated: qt_signal!(key: QString),
    /// A session was archived or deleted; the workspace drops it if it is open.
    session_removed: qt_signal!(path: QString),
    new_session_requested: qt_signal!(),

    refresh: qt_method!(fn(&mut self)),
    /// Point the pane at a workspace. The cwd is the local anchor even for a
    /// remote workspace: pi runs there and writes its session files there, so
    /// the history is the same question either way.
    set_workspace: qt_method!(fn(&mut self, cwd: QString)),
    activate: qt_method!(fn(&mut self, key: QString)),
    request_new_session: qt_method!(fn(&mut self)),
    archive: qt_method!(fn(&mut self, key: QString)),
    /// Pin a session above the rest, or drop it back among them. Both take the
    /// row key (a path), since that is what the row has to hand.
    pin: qt_method!(fn(&mut self, key: QString)),
    unpin: qt_method!(fn(&mut self, key: QString)),
    remove: qt_method!(fn(&mut self, key: QString)),
    clear_selection: qt_method!(fn(&mut self)),
    /// The workspace's live sessions, as `Session.live_sessions` publishes them:
    /// a JSON array of `{ key, title, path, running, unseen }`.
    ///
    /// Handed over from QML rather than read from the other bridge: each object
    /// here has one owner and no opinion about which others exist, and the
    /// workspace view is the thing that knows both (see WorkspaceView.qml). JSON
    /// rather than a `QVariantList` for the same reason the dock reports its
    /// geometry that way — one shape to parse instead of a walk over variants.
    set_live_sessions: qt_method!(fn(&mut self, json: QString)),
    /// Mark the row that is open, by key.
    set_selected_key: qt_method!(fn(&mut self, key: QString)),
    /// The id recorded for a session file, or "" when it is not one of this
    /// workspace's persisted sessions. The title bar's session menu acts on
    /// whatever is focused rather than on a clicked row, so it has a path and
    /// needs the id that archiving takes.
    session_id_for: qt_method!(fn(&self, path: QString) -> QString),

    cwd: String,
    rows: Vec<PiSession>,
    /// Read once per reload rather than per row: the ids live in the state file
    /// and every row would otherwise re-read it.
    pinned_ids: std::collections::HashSet<String>,
    live: Vec<Live>,
    running: Vec<String>,
    unseen: Vec<String>,
    selected_key: String,
}

impl HistoryBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn set_workspace(&mut self, cwd: QString) {
        self.set_cwd(&cwd.to_string());
    }

    pub fn set_cwd(&mut self, cwd: &str) {
        if cwd == self.cwd {
            return;
        }
        self.cwd = cwd.to_string();
        self.selected_key.clear();
        self.reload();
        self.selection_changed();
    }

    fn set_live_sessions(&mut self, json: QString) {
        let parsed: Vec<Value> = serde_json::from_str(&json.to_string()).unwrap_or_default();
        let mut live = Vec::new();
        let mut running = Vec::new();
        let mut unseen = Vec::new();
        for entry in &parsed {
            let field = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let flag = |key: &str| entry.get(key).and_then(Value::as_bool).unwrap_or(false);
            let row = Live {
                key: field("key"),
                title: field("title"),
                path: field("path"),
                running: flag("running"),
            };
            // The dots are keyed the same way the rows are, so a session that
            // has a file on disk lights the disk row rather than a second one.
            if row.running {
                running.push(row.key.clone());
            } else if flag("unseen") {
                unseen.push(row.key.clone());
            }
            live.push(row);
        }
        // Nothing changed, and a reload is a directory walk: a pane rebuilt on
        // every pump is a pane that flickers and a disk that never rests.
        if live == self.live && running == self.running && unseen == self.unseen {
            return;
        }
        let relist = live != self.live;
        self.live = live;
        self.running = running;
        self.unseen = unseen;
        if relist {
            self.reload();
        } else {
            self.sessions_changed();
        }
    }

    fn set_selected_key(&mut self, key: QString) {
        let key = key.to_string();
        if key != self.selected_key {
            self.selected_key = key;
            self.selection_changed();
            self.sessions_changed();
        }
    }

    fn reload(&mut self) {
        self.rows = if self.cwd.is_empty() {
            Vec::new()
        } else {
            sessions::sessions_for(&self.cwd)
        };
        self.pinned_ids = sessions::pinned_ids();
        self.sessions_changed();
    }

    fn refresh(&mut self) {
        self.reload();
    }

    fn status_of(&self, key: &str) -> &'static str {
        if self.running.iter().any(|k| k == key) {
            "running"
        } else if self.unseen.iter().any(|k| k == key) {
            "unseen"
        } else {
            ""
        }
    }

    fn get_sessions(&self) -> QVariantList {
        let text = |value: &str| QVariant::from(QString::from(value));
        let mut list = QVariantList::default();
        let on_disk: Vec<String> = self
            .rows
            .iter()
            .map(|s| s.path.to_string_lossy().into_owned())
            .collect();

        // Live sessions first, and only the ones pi has not written yet: one it
        // has already persisted is left to its disk row below, where the
        // running dot still marks it.
        for live in &self.live {
            if !live.path.is_empty() && on_disk.contains(&live.path) {
                continue;
            }
            let mut row = QVariantMap::default();
            row.insert("key".into(), text(&live.key));
            row.insert(
                "title".into(),
                text(if live.title.is_empty() {
                    "New session"
                } else {
                    live.title.as_str()
                }),
            );
            // A live row has no file behind it to date or count messages from,
            // so it says what it is instead.
            row.insert(
                "subtitle".into(),
                text(if live.running { "running…" } else { "unsaved" }),
            );
            row.insert("tooltip".into(), text(""));
            row.insert("session_id".into(), text(""));
            row.insert("live".into(), true.into());
            row.insert("status".into(), text(self.status_of(&live.key)));
            row.insert("selected".into(), (live.key == self.selected_key).into());
            // Nothing to pin until pi has written the file that carries the id.
            row.insert("pinned".into(), false.into());
            list.push(row.into());
        }

        for session in &self.rows {
            if self.is_pinned(session) {
                continue; // it has its own list, above
            }
            list.push(self.row_for(session).into());
        }
        list
    }

    fn get_pinned(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for session in &self.rows {
            if self.is_pinned(session) {
                list.push(self.row_for(session).into());
            }
        }
        list
    }

    fn is_pinned(&self, session: &PiSession) -> bool {
        self.pinned_ids.contains(&session.session_id)
    }

    /// One persisted session's row, the same shape in either list.
    fn row_for(&self, session: &PiSession) -> QVariantMap {
        let text = |value: &str| QVariant::from(QString::from(value));
        let path = session.path.to_string_lossy().into_owned();
        let noun = if session.message_count == 1 {
            "message"
        } else {
            "messages"
        };
        let mut row = QVariantMap::default();
        row.insert("key".into(), text(&path));
        row.insert("title".into(), text(&session.title));
        row.insert(
            "subtitle".into(),
            text(&format!(
                "{} · {} {noun}",
                started_label(session.started),
                session.message_count
            )),
        );
        row.insert("tooltip".into(), text(&path));
        row.insert("session_id".into(), text(&session.session_id));
        row.insert("live".into(), false.into());
        row.insert("status".into(), text(self.status_of(&path)));
        row.insert("selected".into(), (path == self.selected_key).into());
        row.insert("pinned".into(), self.is_pinned(session).into());
        row
    }

    fn get_selected(&self) -> QString {
        self.selected_key.as_str().into()
    }

    fn activate(&mut self, key: QString) {
        let key = key.to_string();
        self.selected_key = key.clone();
        self.selection_changed();
        self.sessions_changed();
        self.session_activated(key.as_str().into());
    }

    fn request_new_session(&mut self) {
        self.new_session_requested();
    }

    fn clear_selection(&mut self) {
        self.selected_key.clear();
        self.selection_changed();
        self.sessions_changed();
    }

    fn archive(&mut self, key: QString) {
        let key = key.to_string();
        let Some(id) = self.id_for(&key) else {
            return; // a live row has no file to act on
        };
        sessions::archive_session(&id);
        self.reload();
        self.session_removed(key.as_str().into());
    }

    fn pin(&mut self, key: QString) {
        let Some(id) = self.id_for(&key.to_string()) else {
            return; // a live row has no file to pin
        };
        sessions::pin_session(&id);
        self.reload();
    }

    fn unpin(&mut self, key: QString) {
        let Some(id) = self.id_for(&key.to_string()) else {
            return;
        };
        sessions::unpin_session(&id);
        self.reload();
    }

    fn remove(&mut self, key: QString) {
        let key = key.to_string();
        if self.id_for(&key).is_none() {
            return;
        }
        sessions::delete_session(Path::new(&key));
        self.reload();
        self.session_removed(key.as_str().into());
    }

    fn id_for(&self, path: &str) -> Option<String> {
        self.rows
            .iter()
            .find(|session| session.path.to_string_lossy() == path)
            .map(|session| session.session_id.clone())
    }

    fn session_id_for(&self, path: QString) -> QString {
        self.id_for(&path.to_string()).unwrap_or_default().as_str().into()
    }
}

/// A session's start, as a history row says it: `Aug 21, 20:34`.
///
/// Local time, because the reader is looking for the conversation they had this
/// afternoon. The offset comes from `localtime_r`, which is the only part of
/// this that needs the C library at all.
fn started_label(epoch: f64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = epoch as libc::time_t;
    // SAFETY: `localtime_r` writes into the struct we own and reads a plain
    // time_t; it is the reentrant form precisely so it is safe beside threads.
    let ok = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if !ok {
        return String::new();
    }
    let month = MONTHS.get(tm.tm_mon.clamp(0, 11) as usize).copied().unwrap_or("");
    format!(
        "{month} {:02}, {:02}:{:02}",
        tm.tm_mday, tm.tm_hour, tm.tm_min
    )
}
