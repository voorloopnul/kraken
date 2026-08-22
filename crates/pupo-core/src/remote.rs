//! Remote SSH workspaces: host profiles, connection targets, and the helpers
//! that run commands on the remote over SSH.
//!
//! The constraint the design follows is that `pi` always runs on the local
//! machine — only SSH is used to reach a remote host. A workspace that lives on
//! a remote machine is modelled in two levels:
//!
//! * an **SSH host profile** ([`SshHost`]): a reusable connection — hostname,
//!   user, port, identity file — that several workspaces can share. Profiles
//!   can be imported from `~/.ssh/config` aliases.
//! * a **remote workspace**: a host profile (by id) plus a path on that host.
//!
//! Each remote workspace also gets a local *anchor* directory under
//! `~/.pupo/remotes/`. The local `pi` process runs there, so it has a valid cwd
//! and a place to store its session files (which keeps the History pane working
//! unchanged); its tools operate on the remote path instead, via the bundled
//! ssh extension. The anchor's absolute path doubles as the workspace key, so
//! the rest of the app stays path-keyed.
//!
//! All SSH invocations share one multiplexed connection per host
//! (ControlMaster), so the many short commands the agent, terminal and git
//! panel issue don't each re-authenticate.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::state;
use crate::util::{shell_quote, slugify};

/// A reusable SSH connection profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHost {
    /// Stable key and display alias.
    pub host_id: String,
    /// The host actually reached (may itself be an ssh_config alias).
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity: Option<String>,
}

impl SshHost {
    pub fn from_value(host_id: &str, data: &Value) -> Self {
        let text = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            host_id: host_id.to_string(),
            hostname: text("hostname").unwrap_or_else(|| host_id.to_string()),
            user: text("user").unwrap_or_default(),
            port: data
                .get("port")
                .and_then(Value::as_u64)
                .filter(|p| *p > 0)
                .unwrap_or(22) as u16,
            identity: text("identity"),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "hostname": self.hostname,
            "user": self.user,
            "port": self.port,
            "identity": self.identity,
        })
    }

    /// The `user@host` (or bare host) argument passed to ssh.
    pub fn destination(&self) -> String {
        if self.user.is_empty() {
            self.hostname.clone()
        } else {
            format!("{}@{}", self.user, self.hostname)
        }
    }

    /// Common ssh options — everything but the destination and the command:
    /// port, identity, and connection multiplexing.
    ///
    /// `master` is whether this connection may become the shared one. Every
    /// short-lived command says yes; the interactive terminal says no, and the
    /// difference is not cosmetic.
    ///
    /// With `ControlMaster=auto`, whichever ssh starts first creates the socket
    /// and the ones that started alongside it find it already there and say so
    /// — on stderr, which for a session with a PTY is the pane the reader is
    /// looking at. Opening a workspace starts several at once (the file tree,
    /// git, pi's extension, the terminal), so this is a race the terminal loses
    /// often enough to be the first thing in every shell.
    ///
    /// `ControlMaster=no` does not mean "unshared": with a `ControlPath` set,
    /// ssh still uses an existing socket. It means "do not try to create one",
    /// which is the whole of what the terminal was doing wrong.
    pub fn ssh_base_args(&self, master: bool) -> Vec<String> {
        let control_dir = state::ssh_control_dir();
        let _ = fs::create_dir_all(&control_dir);
        let mut args = vec![
            "-p".into(),
            self.port.to_string(),
            "-o".into(),
            if master {
                "ControlMaster=auto".into()
            } else {
                "ControlMaster=no".to_string()
            },
            // %C is a short fixed-length hash of the connection tuple, so the
            // socket path stays well under the ~104-char AF_UNIX limit even for
            // long home dirs, usernames or hostnames (a literal %r@%h:%p can
            // overflow it and make ssh disable multiplexing silently).
            "-o".into(),
            format!("ControlPath={}/cm-%C", control_dir.display()),
            "-o".into(),
            "ControlPersist=120".into(),
            // Key-based auth only: never block the UI on an interactive prompt.
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            // A connection that has gone quiet is noticed and dropped, after
            // roughly a minute of silence. This is what makes a wedged transfer
            // end on its own, and it is the right measure for one: a copy has
            // no honest time limit — its length is its size over the link's
            // speed, and neither is knowable here — but "no bytes have moved
            // for a minute" says something has actually gone wrong, whether the
            // copy is a kilobyte or six gigabytes.
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=4".into(),
        ];
        if let Some(identity) = self.identity.as_deref().filter(|s| !s.is_empty()) {
            args.push("-i".into());
            args.push(expand_home(identity));
        }
        args
    }
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// A resolved remote workspace: a host profile plus a path on that host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTarget {
    pub host: SshHost,
    pub path: String,
}

impl RemoteTarget {
    /// Full argv to run `remote_command` on the remote, from the workspace
    /// path. `remote_command` is a shell string.
    pub fn ssh_argv(&self, remote_command: &str, tty: bool) -> Vec<String> {
        let mut argv = vec!["ssh".to_string()];
        if tty {
            argv.push("-t".into());
        }
        argv.extend(self.host.ssh_base_args(!tty));
        argv.push(self.host.destination());
        argv.push(format!(
            "cd {} && {remote_command}",
            shell_quote(&self.path)
        ));
        argv
    }

    /// argv running `git <git_args>` in the workspace path on the remote. The
    /// whole remote command is one string ssh hands to the login shell, so each
    /// git argument is shell-quoted.
    pub fn git_argv(&self, git_args: &[String]) -> Vec<String> {
        let joined: Vec<String> = git_args.iter().map(|a| shell_quote(a)).collect();
        self.ssh_argv(&format!("git {}", joined.join(" ")), false)
    }

    /// argv for an interactive login shell on the remote, in the workspace path
    /// — what the terminal pane execs, with a PTY.
    pub fn terminal_argv(&self) -> Vec<String> {
        self.ssh_argv(r#"exec "${SHELL:-/bin/bash}" -l"#, true)
    }

    /// The JSON connection descriptor handed to the bundled pi ssh extension
    /// through the `PUPO_SSH` environment variable.
    pub fn env_value(&self) -> String {
        json!({
            "destination": self.host.destination(),
            "baseArgs": self.host.ssh_base_args(true),
            "remotePath": self.path,
        })
        .to_string()
    }
}

// ---------------------------------------------------------------------------
// State: host profiles and remote workspaces
// ---------------------------------------------------------------------------

pub fn load_hosts() -> BTreeMap<String, SshHost> {
    state::object("ssh_hosts")
        .iter()
        .map(|(host_id, data)| (host_id.clone(), SshHost::from_value(host_id, data)))
        .collect()
}

fn store_hosts(hosts: &BTreeMap<String, SshHost>) {
    let map: Map<String, Value> = hosts
        .iter()
        .map(|(id, host)| (id.clone(), host.to_value()))
        .collect();
    state::set("ssh_hosts", Value::Object(map));
}

pub fn save_host(host: SshHost) {
    let mut hosts = load_hosts();
    hosts.insert(host.host_id.clone(), host);
    store_hosts(&hosts);
}

pub fn delete_host(host_id: &str) {
    let mut hosts = load_hosts();
    if hosts.remove(host_id).is_some() {
        store_hosts(&hosts);
    }
}

/// Map of anchor path -> `{ host_id, path }` for every remote workspace.
pub fn load_remotes() -> Map<String, Value> {
    state::object("remotes")
}

/// The [`RemoteTarget`] for a workspace key, or `None` if it is a local folder
/// or its host profile has gone missing.
pub fn resolve(anchor: &str) -> Option<RemoteTarget> {
    let remotes = load_remotes();
    let entry = remotes.get(anchor)?;
    let host_id = entry.get("host_id").and_then(Value::as_str)?;
    let host = load_hosts().get(host_id).cloned()?;
    let path = entry
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or(".")
        .to_string();
    Some(RemoteTarget { host, path })
}

/// Register a remote workspace for `host_id` at `path`, creating its local
/// anchor directory. Returns the anchor's absolute path (the workspace key).
/// Re-registering the same host and path returns the existing anchor.
pub fn add_remote_workspace(host_id: &str, path: &str) -> String {
    let mut remotes = load_remotes();
    for (anchor, entry) in remotes.iter() {
        let same_host = entry.get("host_id").and_then(Value::as_str) == Some(host_id);
        let same_path = entry.get("path").and_then(Value::as_str) == Some(path);
        if same_host && same_path {
            let _ = fs::create_dir_all(anchor);
            return anchor.clone();
        }
    }

    // The slug is derived from host+path, so a leftover anchor directory with
    // this name is from this same workspace (removed earlier, its folder kept);
    // reuse it so a re-add resurfaces the stored sessions. Only bump the name
    // when the slug is an *active* key for a different workspace.
    let base = format!("{}-{}", slugify(host_id), slugify(path));
    let root = state::remotes_dir();
    let mut anchor = root.join(&base);
    let mut n = 2;
    while remotes.contains_key(anchor.to_string_lossy().as_ref()) {
        anchor = root.join(format!("{base}-{n}"));
        n += 1;
    }
    let _ = fs::create_dir_all(&anchor);
    let key = anchor.to_string_lossy().into_owned();
    remotes.insert(key.clone(), json!({ "host_id": host_id, "path": path }));
    state::set("remotes", Value::Object(remotes));
    key
}

pub fn remove_remote_workspace(anchor: &str) {
    let mut remotes = load_remotes();
    if remotes.remove(anchor).is_some() {
        state::set("remotes", Value::Object(remotes));
    }
}

// ---------------------------------------------------------------------------
// ~/.ssh/config import
// ---------------------------------------------------------------------------

/// One concrete `Host` alias read out of an ssh config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfigEntry {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity: Option<String>,
}

/// Split an ssh_config line into (key, value). Options may be written as
/// `Key value` or `Key=value`; keys are case-insensitive.
fn split_option(line: &str) -> (String, String) {
    if let Some((head, tail)) = line.split_once('=') {
        if !head.trim().contains(char::is_whitespace) {
            return (head.trim().to_lowercase(), tail.trim().to_string());
        }
    }
    match line.split_once(char::is_whitespace) {
        Some((key, value)) => (key.trim().to_lowercase(), value.trim().to_string()),
        None => (line.trim().to_lowercase(), String::new()),
    }
}

/// Read an ssh config and return one entry per concrete `Host` alias (wildcard
/// patterns skipped). Best-effort: an unreadable file yields an empty list.
///
/// A `Host a b` line may name several aliases; the option lines that follow
/// apply to every alias in that block until the next `Host` line.
pub fn parse_ssh_config(path: Option<&Path>) -> Vec<SshConfigEntry> {
    let path: PathBuf = match path {
        Some(path) => path.to_path_buf(),
        None => match dirs::home_dir() {
            Some(home) => home.join(".ssh").join("config"),
            None => return Vec::new(),
        },
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut entries: Vec<SshConfigEntry> = Vec::new();
    // Indices into `entries` the current option lines apply to.
    let mut group: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = split_option(line);
        if key == "host" {
            group.clear();
            for alias in value.split_whitespace() {
                if alias.contains(['*', '?', '!']) {
                    continue;
                }
                group.push(entries.len());
                entries.push(SshConfigEntry {
                    alias: alias.to_string(),
                    hostname: alias.to_string(),
                    user: String::new(),
                    port: 22,
                    identity: None,
                });
            }
            continue;
        }
        for index in &group {
            let entry = &mut entries[*index];
            match key.as_str() {
                "hostname" => entry.hostname = value.clone(),
                "user" => entry.user = value.clone(),
                "port" => {
                    if let Ok(port) = value.parse() {
                        entry.port = port;
                    }
                }
                "identityfile" => entry.identity = Some(value.clone()),
                _ => {}
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> SshHost {
        SshHost {
            host_id: "pi5".into(),
            hostname: "purplenode.local".into(),
            user: "pascal".into(),
            port: 22,
            identity: None,
        }
    }

    fn target() -> RemoteTarget {
        RemoteTarget {
            host: host(),
            path: "/home/pascal/Workspace/app 1".into(),
        }
    }

    #[test]
    fn a_userless_profile_ssh_es_to_the_bare_host() {
        let mut bare = host();
        bare.user = String::new();
        assert_eq!(bare.destination(), "purplenode.local");
        assert_eq!(host().destination(), "pascal@purplenode.local");
    }

    #[test]
    fn base_args_multiplex_and_never_prompt() {
        let args = host().ssh_base_args(true).join(" ");
        assert!(args.contains("ControlMaster=auto"));
        assert!(args.contains("ControlPersist=120"));
        assert!(args.contains("BatchMode=yes"));
        assert!(args.contains("ConnectTimeout=10"));
        // What ends a wedged transfer. There is no wall-clock cap on a copy —
        // its length is its size over the link's speed — so silence is the
        // thing measured instead, and this is what measures it.
        assert!(args.contains("ServerAliveInterval=15"));
        assert!(args.contains("ServerAliveCountMax=4"));
        // The socket name is the hashed connection tuple, not the literal one.
        assert!(args.contains("cm-%C"));
        assert!(!args.contains("%r@%h"));
    }

    #[test]
    fn only_the_interactive_terminal_declines_to_own_the_shared_connection() {
        // Opening a workspace starts several ssh clients at once. Whichever
        // creates the control socket first wins, and the rest print "already
        // exists, disabling multiplexing" on stderr — which, for the one
        // session that has a PTY, is the pane the reader is looking at.
        let terminal = target().terminal_argv().join(" ");
        assert!(terminal.contains("ControlMaster=no"), "{terminal}");
        assert!(!terminal.contains("ControlMaster=auto"), "{terminal}");

        // Everything else still shares: the file tree opens a listing per
        // branch, and paying for a connection each time is the cost this whole
        // arrangement exists to avoid.
        let command = target().ssh_argv("ls", false).join(" ");
        assert!(command.contains("ControlMaster=auto"), "{command}");
        assert!(target().git_argv(&["status".into()]).join(" ").contains("ControlMaster=auto"));
        assert!(target().env_value().contains("ControlMaster=auto"));

        // Declining to create the socket is not declining to use one: the
        // ControlPath is still passed, so the terminal rides a connection that
        // is already up.
        assert!(terminal.contains("cm-%C"), "{terminal}");
    }

    #[test]
    fn an_identity_is_expanded_and_passed_with_dash_i() {
        let mut with_key = host();
        with_key.identity = Some("~/.ssh/id_ed25519".into());
        let args = with_key.ssh_base_args(true);
        let index = args.iter().position(|a| a == "-i").expect("-i is passed");
        assert!(!args[index + 1].starts_with('~'), "~ should be expanded");
        assert!(args[index + 1].ends_with(".ssh/id_ed25519"));
    }

    #[test]
    fn the_remote_command_cds_into_a_quoted_workspace_path() {
        let argv = target().ssh_argv("ls", false);
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[argv.len() - 2], "pascal@purplenode.local");
        assert_eq!(
            argv[argv.len() - 1],
            "cd '/home/pascal/Workspace/app 1' && ls"
        );
        assert!(!argv.contains(&"-t".to_string()));
    }

    #[test]
    fn git_arguments_are_quoted_one_by_one() {
        let argv = target().git_argv(&[
            "log".into(),
            "--format=%s".into(),
            "a branch".into(),
        ]);
        let command = argv.last().unwrap();
        assert!(command.ends_with("&& git log --format=%s 'a branch'"), "{command}");
    }

    #[test]
    fn a_terminal_asks_for_a_tty_and_a_login_shell() {
        let argv = target().terminal_argv();
        assert_eq!(argv[1], "-t");
        assert!(argv.last().unwrap().contains(r#"exec "${SHELL:-/bin/bash}" -l"#));
    }

    #[test]
    fn the_extension_descriptor_carries_destination_args_and_path() {
        let value: Value = serde_json::from_str(&target().env_value()).unwrap();
        assert_eq!(value["destination"], "pascal@purplenode.local");
        assert_eq!(value["remotePath"], "/home/pascal/Workspace/app 1");
        assert!(value["baseArgs"].as_array().unwrap().len() >= 10);
    }

    #[test]
    fn a_profile_round_trips_through_state_json() {
        let stored = host().to_value();
        assert_eq!(SshHost::from_value("pi5", &stored), host());
        // A profile written without a hostname falls back to its own alias.
        let sparse = json!({ "user": "", "port": null, "identity": null });
        let parsed = SshHost::from_value("lenovo", &sparse);
        assert_eq!(parsed.hostname, "lenovo");
        assert_eq!(parsed.port, 22);
        assert!(parsed.user.is_empty());
        assert_eq!(parsed.identity, None);
    }

    fn parse(text: &str) -> Vec<SshConfigEntry> {
        let path = std::env::temp_dir().join(format!(
            "pupo-ssh-config-{}",
            text.len() as u64 * 2654435761 % 100_000
        ));
        fs::write(&path, text).unwrap();
        let entries = parse_ssh_config(Some(&path));
        let _ = fs::remove_file(&path);
        entries
    }

    #[test]
    fn ssh_config_options_apply_to_every_alias_in_their_block() {
        let entries = parse(
            "# a comment\n\
             Host alpha beta\n\
             \tHostName box.local\n\
             \tUser pascal\n\
             \tPort 2222\n\
             \n\
             Host gamma\n\
             \tIdentityFile ~/.ssh/other\n",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].alias, "alpha");
        assert_eq!(entries[1].alias, "beta");
        for entry in &entries[..2] {
            assert_eq!(entry.hostname, "box.local");
            assert_eq!(entry.user, "pascal");
            assert_eq!(entry.port, 2222);
        }
        assert_eq!(entries[2].alias, "gamma");
        // Defaults survive a block that sets nothing else.
        assert_eq!(entries[2].hostname, "gamma");
        assert_eq!(entries[2].port, 22);
        assert_eq!(entries[2].identity.as_deref(), Some("~/.ssh/other"));
    }

    #[test]
    fn wildcard_hosts_are_not_offered_as_workspaces() {
        let entries = parse("Host *\n\tUser everyone\n\nHost real\n\tUser pascal\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].alias, "real");
        assert_eq!(entries[0].user, "pascal");
    }

    #[test]
    fn options_may_be_written_with_an_equals_sign() {
        let entries = parse("Host box\n  HostName=box.example\n  Port=2200\n");
        assert_eq!(entries[0].hostname, "box.example");
        assert_eq!(entries[0].port, 2200);
    }

    #[test]
    fn an_unreadable_config_is_an_empty_list() {
        assert!(parse_ssh_config(Some(Path::new("/nope/nothing/here"))).is_empty());
    }

    #[test]
    fn a_bad_port_leaves_the_default_in_place() {
        let entries = parse("Host box\n  Port notanumber\n");
        assert_eq!(entries[0].port, 22);
    }
}
