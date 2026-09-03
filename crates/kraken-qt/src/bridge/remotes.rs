//! The Add / Edit Remote Workspace dialog, as QML sees it.
//!
//! A remote workspace is a folder on another machine that pi works in over SSH.
//! What is stored for it is two things: a reusable connection profile (an
//! [`SshHost`]) and a local *anchor* folder that stands in for the remote path —
//! pi runs in the anchor and routes its tools over the connection.
//! [`kraken_core::remote`] holds both contracts; this is the form in front of them.
//!
//! The host picker offers the machine's own `~/.ssh/config` beside the profiles
//! already saved here, because a host worth opening a workspace on is usually
//! one ssh already knows about — and retyping a hostname that is written down
//! two directories away is how a typo becomes a workspace that never connects.

use std::time::Duration;

use kraken_core::{git, remote, state};
use qmetaobject::*;
use serde_json::{json, Value};

/// How long the probe waits for `ssh … true` to come back. Generous: a first
/// connection may be agreeing a key exchange with a machine that is asleep.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(QObject, Default)]
pub struct RemotesBridge {
    base: qt_base_class!(trait QObject),

    /// Whether the dialog is up.
    open: qt_property!(bool; NOTIFY form_changed READ get_open),
    /// Whether it is editing an existing workspace rather than adding one. The
    /// difference matters on save: an edit keeps its anchor, so the workspace
    /// stays the same workspace with the same history.
    editing: qt_property!(bool; NOTIFY form_changed READ get_editing),

    /// What the host picker offers: `{ id, hostname, user, port, identity,
    /// saved }` per entry, the profiles saved here first and then whatever
    /// `~/.ssh/config` knows that they do not already cover.
    hosts: qt_property!(QVariantList; NOTIFY form_changed READ get_hosts),

    /// The form's fields, prefilled when a host is picked or a workspace is
    /// being edited. QML owns them from then on and hands them back on save —
    /// a bridge that mirrored every keystroke would be a second copy of the
    /// form, and the two would disagree.
    form_name: qt_property!(QString; NOTIFY form_changed READ get_form_name),
    form_hostname: qt_property!(QString; NOTIFY form_changed READ get_form_hostname),
    form_user: qt_property!(QString; NOTIFY form_changed READ get_form_user),
    form_port: qt_property!(i32; NOTIFY form_changed READ get_form_port),
    form_identity: qt_property!(QString; NOTIFY form_changed READ get_form_identity),
    form_path: qt_property!(QString; NOTIFY form_changed READ get_form_path),

    /// The one line under the form: what the probe found, or why a save was
    /// refused.
    status: qt_property!(QString; NOTIFY status_changed READ get_status),
    /// Set while a probe is in flight, so a second click cannot stack another
    /// ssh on top of the first.
    probing: qt_property!(bool; NOTIFY status_changed READ get_probing),

    form_changed: qt_signal!(),
    status_changed: qt_signal!(),
    /// A workspace was added or edited; the anchor is what names it.
    saved: qt_signal!(anchor: QString),

    /// Open the dialog. An empty anchor adds a workspace; anything else edits
    /// the one it names.
    show: qt_method!(fn(&mut self, anchor: QString)),
    hide: qt_method!(fn(&mut self)),
    /// Fill the form from one of the picker's entries.
    pick_host: qt_method!(fn(&mut self, id: QString)),
    test_connection: qt_method!(fn(&mut self, name: QString, hostname: QString, user: QString, port: i32, identity: QString)),
    /// Write the profile and the workspace. Returns "" on success, or the
    /// reason it was refused.
    save: qt_method!(fn(&mut self, name: QString, hostname: QString, user: QString, port: i32, identity: QString, path: QString) -> QString),

    visible: bool,
    anchor: String,
    entries: Vec<remote::SshHost>,
    /// Which of `entries` came from `~/.ssh/config` rather than from our state.
    from_config: Vec<String>,
    form: Form,
    note: String,
    busy: bool,
    /// Bumped per probe, so an answer from one the reader has moved on from is
    /// dropped rather than written over a newer one.
    generation: u64,
}

#[derive(Debug, Clone, Default)]
struct Form {
    name: String,
    hostname: String,
    user: String,
    port: u16,
    identity: String,
    path: String,
}

impl RemotesBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_open(&self) -> bool {
        self.visible
    }

    fn get_editing(&self) -> bool {
        !self.anchor.is_empty()
    }

    fn show(&mut self, anchor: QString) {
        self.anchor = anchor.to_string();
        self.visible = true;
        self.note.clear();
        self.busy = false;
        self.reload_hosts();
        self.form = Form {
            port: 22,
            ..Default::default()
        };
        if !self.anchor.is_empty() {
            self.load_existing();
        }
        self.status_changed();
        self.form_changed();
    }

    fn hide(&mut self) {
        self.visible = false;
        // A probe still running answers into a dialog nobody is looking at;
        // dropping the generation is what makes that answer a no-op.
        self.generation += 1;
        self.busy = false;
        self.form_changed();
        self.status_changed();
    }

    /// The saved profiles, then whatever the machine's ssh config knows that
    /// they do not already name.
    fn reload_hosts(&mut self) {
        let saved = remote::load_hosts();
        let mut entries: Vec<remote::SshHost> = saved.values().cloned().collect();
        let known: Vec<String> = entries.iter().map(|host| host.host_id.clone()).collect();
        let mut from_config = Vec::new();
        for entry in remote::parse_ssh_config(None) {
            if known.contains(&entry.alias) {
                continue;
            }
            from_config.push(entry.alias.clone());
            entries.push(remote::SshHost {
                host_id: entry.alias,
                hostname: entry.hostname,
                user: entry.user,
                port: entry.port,
                identity: entry.identity,
            });
        }
        self.entries = entries;
        self.from_config = from_config;
    }

    /// Prefill from the workspace being edited: its host profile and the remote
    /// path the anchor stands in for.
    fn load_existing(&mut self) {
        let remotes = remote::load_remotes();
        let Some(entry) = remotes.get(&self.anchor) else {
            return;
        };
        let host_id = entry
            .get("host_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.form.path = entry
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(host) = remote::load_hosts().get(host_id) {
            self.fill_from(host);
        } else {
            self.form.name = host_id.to_string();
        }
    }

    fn fill_from(&mut self, host: &remote::SshHost) {
        self.form.name = host.host_id.clone();
        self.form.hostname = host.hostname.clone();
        self.form.user = host.user.clone();
        self.form.port = host.port;
        self.form.identity = host.identity.clone().unwrap_or_default();
    }

    fn pick_host(&mut self, id: QString) {
        let id = id.to_string();
        if let Some(host) = self.entries.iter().find(|host| host.host_id == id).cloned() {
            self.fill_from(&host);
            self.form_changed();
        }
    }

    fn get_hosts(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for host in &self.entries {
            let mut map = QVariantMap::default();
            let put = |map: &mut QVariantMap, key: &str, value: &str| {
                map.insert(key.into(), QVariant::from(QString::from(value)));
            };
            put(&mut map, "id", &host.host_id);
            put(&mut map, "hostname", &host.hostname);
            put(&mut map, "user", &host.user);
            put(&mut map, "identity", host.identity.as_deref().unwrap_or(""));
            map.insert("port".into(), QVariant::from(i32::from(host.port)));
            map.insert(
                "saved".into(),
                QVariant::from(!self.from_config.contains(&host.host_id)),
            );
            list.push(map.into());
        }
        list
    }

    fn get_form_name(&self) -> QString {
        self.form.name.as_str().into()
    }
    fn get_form_hostname(&self) -> QString {
        self.form.hostname.as_str().into()
    }
    fn get_form_user(&self) -> QString {
        self.form.user.as_str().into()
    }
    fn get_form_port(&self) -> i32 {
        i32::from(self.form.port.max(1))
    }
    fn get_form_identity(&self) -> QString {
        self.form.identity.as_str().into()
    }
    fn get_form_path(&self) -> QString {
        self.form.path.as_str().into()
    }

    fn get_status(&self) -> QString {
        self.note.as_str().into()
    }

    fn get_probing(&self) -> bool {
        self.busy
    }

    /// The profile the form describes, or the reason it does not describe one.
    fn host_from(
        name: QString,
        hostname: QString,
        user: QString,
        port: i32,
        identity: QString,
    ) -> Result<remote::SshHost, &'static str> {
        let name = name.to_string().trim().to_string();
        let hostname = hostname.to_string().trim().to_string();
        if name.is_empty() || hostname.is_empty() {
            return Err("Name and hostname are required.");
        }
        let identity = identity.to_string().trim().to_string();
        Ok(remote::SshHost {
            host_id: name,
            hostname,
            user: user.to_string().trim().to_string(),
            port: port.clamp(1, 65535) as u16,
            identity: (!identity.is_empty()).then_some(identity),
        })
    }

    fn test_connection(
        &mut self,
        name: QString,
        hostname: QString,
        user: QString,
        port: i32,
        identity: QString,
    ) {
        if self.busy {
            return;
        }
        let host = match Self::host_from(name, hostname, user, port, identity) {
            Ok(host) => host,
            Err(problem) => {
                self.note = problem.to_string();
                self.status_changed();
                return;
            }
        };
        self.busy = true;
        self.note = "Testing…".to_string();
        self.generation += 1;
        let generation = self.generation;
        self.status_changed();

        let destination = host.destination();
        let mut argv = vec!["ssh".to_string()];
        argv.extend(host.ssh_base_args(true));
        argv.push(destination.clone());
        // The cheapest command that proves a login shell was reached.
        argv.push("true".to_string());

        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |note: String| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_probe(generation, note);
            }
        });
        // `git::run_argv` is the app's one "run this and give up after N"
        // helper; the probe is not a git command but it is exactly that shape,
        // and a second implementation of the timeout would be a second thing to
        // get wrong.
        std::thread::spawn(move || {
            let note = match git::run_argv(&argv, PROBE_TIMEOUT) {
                None => "✗ Connection failed (timed out or ssh unavailable).".to_string(),
                Some(output) if output.ok() => format!("✓ Connected to {destination}."),
                Some(output) => {
                    let reason = output.stderr.trim();
                    if reason.is_empty() {
                        match output.code {
                            Some(code) => format!("✗ Failed: ssh exited {code}"),
                            None => "✗ Failed: ssh was killed before it answered.".to_string(),
                        }
                    } else {
                        format!("✗ Failed: {reason}")
                    }
                }
            };
            deliver(note);
        });
    }

    fn on_probe(&mut self, generation: u64, note: String) {
        if generation != self.generation {
            return;
        }
        self.busy = false;
        self.note = note;
        self.status_changed();
    }

    fn save(
        &mut self,
        name: QString,
        hostname: QString,
        user: QString,
        port: i32,
        identity: QString,
        path: QString,
    ) -> QString {
        let host = match Self::host_from(name, hostname, user, port, identity) {
            Ok(host) => host,
            Err(problem) => return problem.into(),
        };
        let path = path.to_string().trim().to_string();
        if path.is_empty() {
            return "A remote path is required.".into();
        }
        let host_id = host.host_id.clone();
        remote::save_host(host);
        let anchor = if self.anchor.is_empty() {
            remote::add_remote_workspace(&host_id, &path)
        } else {
            // Editing in place: the same anchor, with its path and host
            // reference brought up to date. A new anchor would be a new
            // workspace, and the one being edited would still be in the bar.
            let mut remotes = remote::load_remotes();
            let entry = remotes
                .entry(self.anchor.clone())
                .or_insert_with(|| json!({}));
            entry["host_id"] = json!(host_id);
            entry["path"] = json!(path);
            state::set("remotes", Value::Object(remotes));
            self.anchor.clone()
        };
        self.visible = false;
        self.form_changed();
        self.saved(anchor.as_str().into());
        "".into()
    }
}
