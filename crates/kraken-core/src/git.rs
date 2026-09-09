//! Git surfaces: the branch a workspace sits on, its local branches, and the
//! commit graph the history pane draws.
//!
//! Every git call is built as argv here and run through one [`GitRunner`], so a
//! local repository and a remote one reached over SSH follow the same code
//! path — the runner is the only thing that knows which it is. The parsing is
//! kept separate from the running: it takes the bytes git printed and returns
//! rows, which is what lets the interesting half be tested without a
//! repository, a network, or a display.
//!
//! [`crate::diff`] builds on the same runner for the diff pane.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::remote::{self, RemoteTarget};
use crate::util::shell_quote;

/// The history pane walks this far back and no further; a repo with tens of
/// thousands of commits would otherwise spend the whole refresh in `git log`.
pub const MAX_COMMITS: usize = 200;

/// What a command that only reads refs is given.
pub const QUICK: Duration = Duration::from_secs(5);
/// What a command that touches file contents (a diff, a checkout) is given.
pub const SLOW: Duration = Duration::from_secs(10);
/// Hooks and a network push can take much longer than a read of the worktree.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
/// A remote call carries a network round trip plus a login-shell spawn, so no
/// remote timeout is ever shorter than this.
pub const REMOTE_FLOOR: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// What a finished command left behind. `code` is `None` when a signal killed
/// it, which for our purposes reads the same as a non-zero exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Run `argv`, collect its output, and give up on it after `timeout`.
///
/// `wait_with_output` has no deadline of its own, so the wait happens on a
/// thread and the answer comes back over a channel. A call that runs out of
/// time is killed by pid rather than through the `Child` — the waiting thread
/// owns that, and it is also what reaps the corpse. Output is decoded lossily:
/// a diff carries whatever bytes the file had, and one invalid sequence must
/// not lose the whole reading.
pub fn run_argv(argv: &[String], timeout: Duration) -> Option<GitOutput> {
    let (program, rest) = argv.split_first()?;
    let child = Command::new(program)
        .args(rest)
        // Nothing here ever has an answer for a prompt, and a git that asks for
        // one (credentials, a passphrase) would hang until the timeout instead
        // of failing at once.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(output)) => Some(GitOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Ok(Err(_)) => None,
        Err(_) => {
            // SAFETY: `pid` is a child of this process that nothing has reaped
            // yet — the thread above still holds its `Child` and will reap it
            // once the kill lands, so the pid cannot have been recycled.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            None
        }
    }
}

/// Where git runs for one workspace: a directory on this machine, or a path on
/// a remote host reached over SSH.
#[derive(Debug, Clone)]
pub struct GitRunner {
    pub cwd: String,
    /// When set, git runs on the remote host instead of on `cwd`.
    pub remote: Option<RemoteTarget>,
}

impl GitRunner {
    pub fn local(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            remote: None,
        }
    }

    pub fn remote(cwd: impl Into<String>, target: RemoteTarget) -> Self {
        Self {
            cwd: cwd.into(),
            remote: Some(target),
        }
    }

    /// The runner for a workspace key: a remote one when the key names a remote
    /// workspace's anchor, a local one otherwise.
    pub fn for_workspace(path: &str) -> Self {
        match remote::resolve(path) {
            Some(target) => Self::remote(path, target),
            None => Self::local(path),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Full argv for `git <git_args>` in the workspace.
    pub fn argv(&self, git_args: &[String]) -> Vec<String> {
        match &self.remote {
            Some(target) => target.git_argv(git_args),
            None => {
                let mut argv = vec!["git".to_string(), "-C".to_string(), self.cwd.clone()];
                argv.extend(git_args.iter().cloned());
                argv
            }
        }
    }

    fn deadline(&self, base: Duration) -> Duration {
        if self.is_remote() {
            base.max(REMOTE_FLOOR)
        } else {
            base
        }
    }

    /// Run a git command, or `None` if it could not be started or ran out of
    /// time. A command that ran and failed comes back as a [`GitOutput`] with
    /// its exit code, because the stderr is often the answer we want.
    pub fn run(&self, git_args: &[String], timeout: Duration) -> Option<GitOutput> {
        run_argv(&self.argv(git_args), self.deadline(timeout))
    }

    /// stdout of a git command, or `None` if it couldn't run or failed.
    pub fn text(&self, git_args: &[String], timeout: Duration) -> Option<String> {
        self.run(git_args, timeout)
            .filter(GitOutput::ok)
            .map(|output| output.stdout)
    }

    /// stdout of a plain shell command on the remote host, or `""`. For the two
    /// things git cannot answer about a file it does not track: how many lines
    /// it has, and what is in it. `partial` keeps the output of a command that
    /// failed but printed usable results anyway. A local runner has no shell to
    /// run this on — it reads the filesystem directly instead — and answers "".
    pub fn remote_shell(&self, command: &str, timeout: Duration, partial: bool) -> String {
        let Some(target) = &self.remote else {
            return String::new();
        };
        let argv = target.ssh_argv(command, false);
        match run_argv(&argv, timeout.max(REMOTE_FLOOR)) {
            Some(output) if partial || output.ok() => output.stdout,
            _ => String::new(),
        }
    }
}

/// `["a", "b"]` as owned strings, for the argv builders below.
pub fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

// ---------------------------------------------------------------------------
// Paths and branches
// ---------------------------------------------------------------------------

/// `path` with the user's home folder written as `~`. The folder is a subtitle
/// in the title bar, under the conversation's own name, and the half of an
/// absolute path that is the same for every folder is the half worth losing.
pub fn home_relative(path: &str) -> String {
    match dirs::home_dir() {
        Some(home) => home_relative_to(path, &home.to_string_lossy()),
        None => path.to_string(),
    }
}

/// [`home_relative`] against an explicit home, which is what the tests drive.
pub fn home_relative_to(path: &str, home: &str) -> String {
    if path == home {
        return "~".to_string();
    }
    // The separator has to be part of the comparison: /Users/xylophone is not
    // inside /Users/x, however much of the prefix it shares.
    match path.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => path.to_string(),
    }
}

/// Current branch of the repo containing `path`, or `""` when it is not in one.
/// Reads `.git/HEAD` directly (following the gitdir file a worktree or
/// submodule leaves behind), so it is cheap enough to poll without spawning
/// git.
pub fn git_branch(path: &str) -> String {
    let start = PathBuf::from(path);
    let mut candidate = Some(start.as_path());
    while let Some(dir) = candidate {
        candidate = dir.parent();
        let git = dir.join(".git");
        let head = if git.is_dir() {
            git.join("HEAD")
        } else if git.is_file() {
            let Some(gitdir) = fs::read_to_string(&git)
                .ok()
                .and_then(|text| text.split_once(':').map(|(_, rest)| rest.trim().to_string()))
            else {
                return String::new();
            };
            let linked = dir.join(gitdir);
            linked
                .canonicalize()
                .unwrap_or(linked)
                .join("HEAD")
        } else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&head) else {
            return String::new();
        };
        let content = content.trim();
        if let Some(reference) = content.strip_prefix("ref:") {
            let reference = reference.trim();
            return reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string();
        }
        // Detached HEAD: the short commit hash, which is what git itself shows.
        return content.chars().take(8).collect();
    }
    String::new()
}

pub fn branch_list_argv() -> Vec<String> {
    args(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])
}

/// One branch per line, but split on whitespace: a branch name can hold no
/// space, so this also drops the trailing blank line without a special case.
pub fn parse_branch_list(stdout: &str) -> Vec<String> {
    stdout.split_whitespace().map(str::to_string).collect()
}

pub fn checkout_argv(target: &str) -> Vec<String> {
    vec!["checkout".to_string(), target.to_string()]
}

pub fn current_branch_argv() -> Vec<String> {
    args(&["rev-parse", "--abbrev-ref", "HEAD"])
}

pub fn short_head_argv() -> Vec<String> {
    args(&["rev-parse", "--short", "HEAD"])
}

pub fn head_exists_argv() -> Vec<String> {
    args(&["rev-parse", "--verify", "--quiet", "HEAD"])
}

pub fn rev_list_argv(reference: &str) -> Vec<String> {
    vec!["rev-list".to_string(), reference.to_string(), "--".to_string()]
}

/// The repo's local branches, or an empty list if git could not answer.
pub fn local_branches(runner: &GitRunner) -> Vec<String> {
    match runner.text(&branch_list_argv(), QUICK) {
        Some(stdout) => parse_branch_list(&stdout),
        None => Vec::new(),
    }
}

/// The branch a remote workspace is on. The local case reads `.git/HEAD`
/// instead ([`git_branch`]); this is the round trip a remote one costs, so it
/// belongs off the UI thread.
pub fn current_branch(runner: &GitRunner) -> String {
    let current = runner
        .text(&current_branch_argv(), QUICK)
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    if current == "HEAD" {
        // Detached: show the short hash, the same as git_branch() does.
        return runner
            .text(&short_head_argv(), QUICK)
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
    }
    current
}

/// Check `target` (a branch or a commit) out. The error is what the caller puts
/// in front of the user, so it carries git's own words where there are any.
pub fn checkout(runner: &GitRunner, target: &str) -> Result<(), String> {
    match runner.run(&checkout_argv(target), SLOW) {
        Some(output) if output.ok() => Ok(()),
        Some(output) => {
            let message = output.stderr.trim();
            Err(if message.is_empty() {
                "git checkout failed".to_string()
            } else {
                message.to_string()
            })
        }
        None => Err("git checkout could not run".to_string()),
    }
}

/// Commit the checked files' working-tree contents, not the entire index.
/// Unchecked files stay out of the commit even if someone already staged them.
/// Paths are repository-relative, just like the Changes tab's status rows.
pub fn commit_selected(runner: &GitRunner, message: &str, paths: &[String]) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Enter a commit message.".into());
    }
    if paths.is_empty() {
        return Err("Select at least one changed file.".into());
    }
    // Read again at click time: a selected rename must include its old name,
    // and a stale row must not silently commit a different set of paths.
    let status = runner
        .run(&crate::diff::status_argv(), QUICK)
        .ok_or("Could not read Git status. Refresh the Changes tab and retry.")?;
    if !status.ok() {
        return Err(format!("Could not read Git status:\n{}", status.stderr.trim()));
    }
    let entries = crate::diff::parse_status(&status.stdout);
    let (stage, commit) = selected_paths(&entries, paths)?;
    if !stage.is_empty() {
        let mut argv = args(&["add", "--all", "--"]);
        argv.extend(stage.iter().map(|path| literal_path(path)));
        write_command(runner, &argv, "Stage")?;
    }
    // --only is essential: staging checked files and doing a plain commit
    // would also sweep in every unchecked file that was already staged.
    write_command(runner, &commit_argv(message, &commit), "Commit").map_err(|error| {
        format!("{error}\nThe index was not reset; review staged changes before retrying.")
    })
}

fn selected_paths(
    entries: &[crate::diff::StatusEntry],
    paths: &[String],
) -> Result<(Vec<String>, Vec<String>), String> {
    let selected: HashSet<&str> = paths.iter().map(String::as_str).collect();
    if selected
        .iter()
        .any(|path| !entries.iter().any(|entry| entry.path == *path))
    {
        return Err(
            "Selected files have changed or disappeared. Refresh the Changes tab and retry.".into(),
        );
    }
    let mut stage = std::collections::BTreeSet::new();
    let mut commit = std::collections::BTreeSet::new();
    for entry in entries
        .iter()
        .filter(|entry| selected.contains(entry.path.as_str()))
    {
        commit.insert(entry.path.clone());
        // A staged deletion has already left the index, so `git add` would
        // reject its path. `commit --only` can still find it in HEAD.
        if entry.xy != "D " {
            stage.insert(entry.path.clone());
        }
        if entry.xy.contains('R') {
            if let Some(origin) = &entry.orig {
                if entries.iter().any(|other| other.path == *origin)
                    && !selected.contains(origin.as_str())
                {
                    return Err(format!(
                        "The rename of {origin} overlaps an unchecked change at that path. \
                         Select both changes or adjust the rename before committing."
                    ));
                }
                commit.insert(origin.clone());
                if entry.xy.ends_with('R') {
                    stage.insert(origin.clone());
                }
            }
        }
        // A copy does not remove its source; do not pull in source edits.
    }
    Ok((stage.into_iter().collect(), commit.into_iter().collect()))
}

fn literal_path(path: &str) -> String {
    // Neither a leading '-' nor Git's wildcard/pathspec syntax is an option
    // to expand the selection. Anchor it at the root for nested workspaces.
    format!(":(top,literal){path}")
}

/// Both the message and each literal path remain single arguments over SSH.
pub fn commit_argv(message: &str, paths: &[String]) -> Vec<String> {
    let mut argv = args(&["commit", "--only", "-m", message, "--"]);
    argv.extend(paths.iter().map(|path| literal_path(path)));
    argv
}

/// Use Git's configured push destination and refspec. Never infer a remote,
/// set an upstream, force a push, or bypass hooks on the user's behalf.
pub fn push(runner: &GitRunner) -> Result<(), String> {
    write_command(runner, &args(&["push"]), "Push")
}

/// Do not ask for credentials in the terminal that happened to launch Kraken.
/// `env` also applies these settings on the far side of an SSH workspace.
fn write_argv(runner: &GitRunner, git_args: &[String]) -> Vec<String> {
    let mut prefix = args(&["env", "GIT_TERMINAL_PROMPT=0", "GCM_INTERACTIVE=never"]);
    match &runner.remote {
        Some(target) => {
            prefix.push("git".into());
            prefix.extend(git_args.iter().cloned());
            let command = prefix
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            target.ssh_argv(&command, false)
        }
        None => {
            prefix.extend(runner.argv(git_args));
            prefix
        }
    }
}

fn write_command(runner: &GitRunner, argv: &[String], action: &str) -> Result<(), String> {
    match run_argv(&write_argv(runner, argv), WRITE_TIMEOUT) {
        Some(output) if output.ok() => Ok(()),
        Some(output) => {
            // Hooks and "nothing to commit" can report on stdout rather than
            // stderr. Preserve both, as plain text, instead of hiding why.
            let detail = [output.stderr.trim(), output.stdout.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            Err(if detail.is_empty() {
                format!("{action} failed.")
            } else {
                format!("{action} failed:\n{detail}")
            })
        }
        None => Err(format!(
            "{action} could not start or timed out. Refresh the repository before retrying."
        )),
    }
}

pub fn head_exists(runner: &GitRunner) -> bool {
    runner
        .run(&head_exists_argv(), QUICK)
        .map(|output| output.ok())
        .unwrap_or(false)
}

/// Full hashes reachable from master (or main), or `None` when neither exists
/// to compare against — which is not the same as "nothing is on the main line"
/// and is why the rows carry a three-valued answer.
pub fn main_line_hashes(runner: &GitRunner) -> Option<HashSet<String>> {
    for reference in ["master", "main"] {
        // A git that could not run at all says nothing about either ref, so
        // trying the second one would only cost another timeout.
        let output = runner.run(&rev_list_argv(reference), QUICK)?;
        if output.ok() {
            return Some(output.stdout.split_whitespace().map(str::to_string).collect());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The commit graph
// ---------------------------------------------------------------------------

/// The log command the history pane renders.
///
/// Branches, tags, remotes and HEAD — not `--all`, which also walks tool-owned
/// refs (editor local-history, agent checkpoints) and shows "history" in repos
/// whose branches have no commits at all. HEAD is listed explicitly so a
/// detached checkout stays visible, but only when it resolves: an unborn
/// branch's HEAD would make `git log` fail outright.
pub fn log_argv(max_commits: usize, head_exists: bool) -> Vec<String> {
    let mut argv = args(&["log", "--graph", "--branches", "--tags", "--remotes"]);
    if head_exists {
        argv.push("HEAD".to_string());
    }
    // \x1f-separated fields after the graph prefix; continuation lines (pure
    // graph, like "|/") carry no record at all.
    argv.push("--format=%x1f%h%x1f%H%x1f%d%x1f%s%x1f%an%x1f%ar".to_string());
    argv.push(format!("-{max_commits}"));
    argv
}

/// The commit on a log row that has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogCommit {
    pub short_hash: String,
    pub full_hash: String,
    /// git's `%d`: the decoration, `" (HEAD -> main, tag: v1)"` and all.
    pub refs: String,
    pub subject: String,
    pub author: String,
    pub when: String,
    /// Branches decorating this commit, offered as attached checkouts.
    /// `HEAD -> x` is the branch we are already on; tags and a bare detached
    /// `HEAD` check out the same thing as the hash, so neither earns an entry.
    pub branches: Vec<String>,
    /// Whether this is the checked-out commit, which the row draws bold.
    pub is_head: bool,
    /// Whether the commit is reachable from master/main, or `None` when there
    /// is no such branch to compare against.
    pub on_main_line: Option<bool>,
}

impl LogCommit {
    pub fn tooltip(&self) -> String {
        format!(
            "{}\n{} — {}, {}",
            self.subject, self.short_hash, self.author, self.when
        )
    }
}

/// One rendered row: the graph columns, plus the commit when the row carries
/// one. A row without a commit is a pure graph line like `|/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub graph: String,
    pub commit: Option<LogCommit>,
}

/// Row text per theme, plus the hash accents: pastel green for commits on the
/// main line (reachable from master/main), light gray for side-branch work.
pub fn log_text_color(theme: &str) -> &'static str {
    if theme == "dark" {
        "#c8cad0"
    } else {
        "#4a4d55"
    }
}

pub fn main_hash_color(theme: &str) -> &'static str {
    if theme == "dark" {
        "#98c379"
    } else {
        "#50a14f"
    }
}

pub fn off_main_color(theme: &str) -> &'static str {
    if theme == "dark" {
        "#7a7d85"
    } else {
        "#9a9da5"
    }
}

/// The colour a row's hash is drawn in. With no master/main to compare against
/// every hash keeps the plain text colour, rather than every commit being
/// declared off the main line.
pub fn hash_color(theme: &str, on_main_line: Option<bool>) -> &'static str {
    match on_main_line {
        None => log_text_color(theme),
        Some(true) => main_hash_color(theme),
        Some(false) => off_main_color(theme),
    }
}

/// Escape one span of text for the rich-text row, as Python's `html.escape`
/// does — quotes included, since a commit subject is arbitrary user text.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// One coloured span of row text, with runs of spaces kept: the graph columns
/// are drawn out of spaces and pipes, and collapsing them would bend the graph.
pub fn mono(text: &str, color: &str) -> String {
    format!(
        "<span style=\"color: {color};\">{}</span>",
        escape_html(text).replace(' ', "&nbsp;")
    )
}

impl LogRow {
    /// The row as rich text: the graph, then the hash in its own colour, then
    /// the decoration and subject. One string per row, so the view stays a
    /// plain list.
    pub fn html(&self, theme: &str) -> String {
        let text = log_text_color(theme);
        match &self.commit {
            None => mono(self.graph.trim_end(), text),
            Some(commit) => {
                mono(&self.graph, text)
                    + &mono(&commit.short_hash, hash_color(theme, commit.on_main_line))
                    + &mono(&format!("{}  {}", commit.refs, commit.subject), text)
            }
        }
    }
}

/// Rows from `git log --graph` output. `main_line` is the set of hashes on
/// master/main, or `None` when there is no such branch.
pub fn parse_log(stdout: &str, main_line: Option<&HashSet<String>>) -> Vec<LogRow> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let Some((graph, record)) = line.split_once('\u{1f}') else {
            rows.push(LogRow {
                graph: line.to_string(),
                commit: None,
            });
            continue;
        };
        let fields: Vec<&str> = record.split('\u{1f}').collect();
        if fields.len() != 6 {
            // A record we cannot read is still a graph line worth drawing:
            // dropping it would break the columns of every row below it.
            rows.push(LogRow {
                graph: line.to_string(),
                commit: None,
            });
            continue;
        }
        let refs = fields[2];
        let ref_names: Vec<&str> = refs
            .trim_matches(|c| c == ' ' || c == '(' || c == ')')
            .split(", ")
            .filter(|name| !name.is_empty())
            .collect();
        let branches = ref_names
            .iter()
            .filter(|name| {
                **name != "HEAD" && !name.starts_with("HEAD -> ") && !name.starts_with("tag: ")
            })
            .map(|name| (*name).to_string())
            .collect();
        let is_head = ref_names
            .iter()
            .any(|name| *name == "HEAD" || name.starts_with("HEAD -> "));
        rows.push(LogRow {
            graph: graph.to_string(),
            commit: Some(LogCommit {
                short_hash: fields[0].to_string(),
                full_hash: fields[1].to_string(),
                refs: refs.to_string(),
                subject: fields[3].to_string(),
                author: fields[4].to_string(),
                when: fields[5].to_string(),
                branches,
                is_head,
                on_main_line: main_line.map(|hashes| hashes.contains(fields[1])),
            }),
        });
    }
    rows
}

/// What the history pane has to render: rows, or the one sentence that explains
/// why there are none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogData {
    Rows(Vec<LogRow>),
    Message(String),
}

/// Run the log commands and return plain data for the view. No UI here — this
/// runs on a worker thread for remote workspaces.
pub fn gather_log(runner: &GitRunner, max_commits: usize) -> LogData {
    let Some(output) = runner.run(&log_argv(max_commits, head_exists(runner)), QUICK) else {
        return LogData::Message("No commits".to_string());
    };
    if !output.ok() {
        return LogData::Message(log_failure_message(&output.stderr));
    }
    let rows = parse_log(&output.stdout, main_line_hashes(runner).as_ref());
    if rows.is_empty() {
        // rc 0 but nothing listed: branches exist but are unborn.
        return LogData::Message("No commits".to_string());
    }
    LogData::Rows(rows)
}

/// Why `git log` failed, in the words the pane shows. Only one failure has an
/// explanation worth giving; everything else reads as an empty history.
pub fn log_failure_message(stderr: &str) -> String {
    if stderr.to_lowercase().contains("not a git repository") {
        "Not a git repository".to_string()
    } else {
        "No commits".to_string()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    // ---- Fixtures ---------------------------------------------------------

    /// A throwaway repository under the system temp dir, isolated from the
    /// developer's git configuration and removed when the test ends.
    pub(crate) struct TestRepo {
        pub path: PathBuf,
    }

    impl TestRepo {
        pub fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("kraken-git-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp repo");
            let repo = Self { path };
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        /// Run git in the repo. The global and system configs are pointed at
        /// nothing: the developer's own settings (signing, hooks, a default
        /// branch name) must not decide whether a test passes.
        pub fn git(&self, argv: &[&str]) -> GitOutput {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.path)
                .arg("-c")
                .arg("user.email=test@example.com")
                .arg("-c")
                .arg("user.name=Test")
                .arg("-c")
                .arg("commit.gpgsign=false")
                .args(argv)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00 +0000")
                .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00 +0000")
                .output()
                .expect("git runs");
            GitOutput {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        }

        pub fn write(&self, name: &str, text: &str) {
            let target = self.path.join(name);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("parent dir");
            }
            fs::write(target, text).expect("write file");
        }

        pub fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-qm", message]);
        }

        pub fn runner(&self) -> GitRunner {
            GitRunner::local(self.path.to_string_lossy().into_owned())
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // ---- Writes -----------------------------------------------------------

    // Select every current row for fixtures that need a complete commit.
    fn commit_all(runner: &GitRunner, message: &str) -> Result<(), String> {
        let status = runner.run(&crate::diff::status_argv(), QUICK).unwrap();
        let paths = crate::diff::parse_status(&status.stdout)
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        commit_selected(runner, message, &paths)
    }

    fn writable_repo(name: &str) -> TestRepo {
        let repo = TestRepo::new(name);
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/test-hooks"),
            ("core.excludesFile", "/dev/null"),
            ("push.default", "simple"),
            ("push.autoSetupRemote", "false"),
            ("push.gpgSign", "false"),
        ] {
            assert!(repo.git(&["config", "--local", key, value]).ok());
        }
        repo
    }

    #[test]
    fn commit_all_handles_new_modified_deleted_and_partially_staged_files() {
        let repo = writable_repo("commit-all");
        repo.write("changed.txt", "original\n");
        repo.write("gone.txt", "delete me\n");
        commit_all(&repo.runner(), "Initial commit").expect("initial commit");
        repo.write("changed.txt", "staged\n");
        assert!(repo.git(&["add", "changed.txt"]).ok());
        repo.write("changed.txt", "working copy\n");
        fs::remove_file(repo.path.join("gone.txt")).unwrap();
        repo.write("sub/new.txt", "new\n");
        repo.write(".gitignore", "ignored.txt\n");
        repo.write("ignored.txt", "leave me out\n");

        // A workspace inside the repository still commits the whole Changes
        // listing, not just the paths beneath that subdirectory.
        let runner = GitRunner::local(repo.path.join("sub").to_string_lossy());
        let message = "Fix 'quotes' and $variables\n\nKeep a detailed body; $(not a command).";
        commit_all(&runner, message).expect("commit changes");
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B"]).stdout.trim(),
            message
        );
        assert_eq!(
            repo.git(&["show", "HEAD:changed.txt"]).stdout,
            "working copy\n"
        );
        assert_eq!(repo.git(&["show", "HEAD:sub/new.txt"]).stdout, "new\n");
        assert!(!repo.git(&["cat-file", "-e", "HEAD:gone.txt"]).ok());
        assert!(!repo.git(&["cat-file", "-e", "HEAD:ignored.txt"]).ok());
        assert!(repo.git(&["status", "--porcelain"]).stdout.is_empty());
    }

    #[test]
    fn unchecked_staged_and_unstaged_edits_stay_out_of_a_selected_commit() {
        let repo = writable_repo("commit-subset");
        repo.write("checked", "original\n");
        repo.write("unchecked", "original\n");
        commit_all(&repo.runner(), "Initial").unwrap();
        repo.write("unchecked", "staged\n");
        assert!(repo.git(&["add", "unchecked"]).ok());
        repo.write("unchecked", "unstaged\n");
        repo.write("checked", "checked edits\n");
        repo.write("untracked", "leave out\n");

        commit_selected(&repo.runner(), "Selected only", &args(&["checked"])).unwrap();
        assert_eq!(repo.git(&["show", "HEAD:checked"]).stdout, "checked edits\n");
        assert_eq!(repo.git(&["show", "HEAD:unchecked"]).stdout, "original\n");
        assert_eq!(repo.git(&["show", ":unchecked"]).stdout, "staged\n");
        assert_eq!(fs::read_to_string(repo.path.join("unchecked")).unwrap(), "unstaged\n");
        assert!(!repo.git(&["cat-file", "-e", "HEAD:untracked"]).ok());
    }

    #[test]
    fn an_initial_commit_also_excludes_unchecked_staged_files() {
        let repo = writable_repo("commit-subset-initial");
        repo.write("checked", "checked\n");
        repo.write("unchecked", "unchecked\n");
        assert!(repo.git(&["add", "unchecked"]).ok());
        commit_selected(&repo.runner(), "First", &args(&["checked"])).unwrap();
        assert_eq!(repo.git(&["ls-tree", "--name-only", "HEAD"]).stdout, "checked\n");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]).stdout, "unchecked\n");
    }

    #[test]
    fn a_checked_rename_commits_both_names_but_not_other_staged_files() {
        let repo = writable_repo("commit-selected-rename");
        repo.write("old", "original\n");
        repo.write("unchecked", "original\n");
        commit_all(&repo.runner(), "Initial").unwrap();
        assert!(repo.git(&["mv", "old", "new"]).ok());
        repo.write("unchecked", "staged\n");
        assert!(repo.git(&["add", "unchecked"]).ok());
        commit_selected(&repo.runner(), "Rename", &args(&["new"])).unwrap();
        assert!(!repo.git(&["cat-file", "-e", "HEAD:old"]).ok());
        assert_eq!(repo.git(&["show", "HEAD:new"]).stdout, "original\n");
        assert_eq!(repo.git(&["show", "HEAD:unchecked"]).stdout, "original\n");
        assert_eq!(repo.git(&["show", ":unchecked"]).stdout, "staged\n");
    }

    #[test]
    fn checked_deletions_work_whether_staged_or_not() {
        let repo = writable_repo("commit-selected-deletions");
        for path in ["staged-delete", "unstaged-delete", "unchecked-delete"] {
            repo.write(path, "original\n");
        }
        commit_all(&repo.runner(), "Initial").unwrap();
        assert!(repo.git(&["rm", "staged-delete", "unchecked-delete"]).ok());
        fs::remove_file(repo.path.join("unstaged-delete")).unwrap();
        commit_selected(&repo.runner(), "Delete selected", &args(&["staged-delete", "unstaged-delete"])).unwrap();
        assert_eq!(repo.git(&["ls-tree", "--name-only", "HEAD"]).stdout, "unchecked-delete\n");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]).stdout, "unchecked-delete\n");
    }

    #[test]
    fn pathspec_magic_and_unusual_names_cannot_expand_the_selection() {
        let repo = writable_repo("commit-literal-selection");
        let names = ["*.txt", "-leading[1].txt", ":(glob)**", "with space\tand\nnewline"];
        for name in names { repo.write(name, "checked\n"); }
        repo.write("other.txt", "unchecked\n");
        commit_selected(&repo.runner(), "Literal names", &args(&names)).unwrap();
        for name in names {
            assert!(repo.git(&["cat-file", "-e", &format!("HEAD:{name}")]).ok());
        }
        assert!(!repo.git(&["cat-file", "-e", "HEAD:other.txt"]).ok());
        assert!(!repo.git(&["cat-file", "-e", ":other.txt"]).ok());
    }

    #[test]
    fn an_empty_or_stale_selection_never_stages_anything() {
        let repo = writable_repo("commit-empty-selection");
        repo.write("new", "new\n");
        assert!(commit_selected(&repo.runner(), "Empty", &[]).unwrap_err().contains("Select at least one"));
        assert!(commit_selected(&repo.runner(), "Stale", &args(&["new", "gone"])).unwrap_err().contains("Refresh"));
        assert!(repo.git(&["ls-files"]).stdout.is_empty());
        assert!(!head_exists(&repo.runner()));
    }

    #[test]
    fn a_checked_copy_does_not_pull_in_its_unchecked_source() {
        let entries = crate::diff::parse_status("C  copy\0source\0 M source\0");
        let (stage, commit) = selected_paths(&entries, &args(&["copy"])).unwrap();
        assert_eq!(stage, args(&["copy"]));
        assert_eq!(commit, args(&["copy"]));
    }

    #[test]
    fn a_rename_cannot_sweep_in_an_unchecked_recreated_source() {
        let entries = crate::diff::parse_status("R  new\0old\0?? old\0");
        assert!(selected_paths(&entries, &args(&["new"])).unwrap_err().contains("unchecked"));
        assert!(selected_paths(&entries, &args(&["new", "old"])).is_ok());
    }

    #[test]
    fn an_empty_message_does_not_even_stage_changes() {
        let repo = writable_repo("commit-empty-message");
        repo.write("new.txt", "new\n");
        assert_eq!(
            commit_all(&repo.runner(), " \n\t "),
            Err("Enter a commit message.".into())
        );
        assert!(repo.git(&["ls-files"]).stdout.is_empty());
        assert!(!head_exists(&repo.runner()));
    }

    #[test]
    fn a_clean_tree_does_not_create_an_empty_commit() {
        let repo = writable_repo("commit-clean");
        repo.write("new.txt", "new\n");
        commit_all(&repo.runner(), "First").unwrap();
        let error = commit_all(&repo.runner(), "Nothing changed").unwrap_err();
        assert!(error.contains("Select at least one changed file"), "{error}");
        assert_eq!(
            repo.git(&["rev-list", "--count", "HEAD"]).stdout.trim(),
            "1"
        );
    }

    #[test]
    fn a_rejected_commit_keeps_the_index_and_reports_the_hook_output() {
        use std::os::unix::fs::PermissionsExt;
        let repo = writable_repo("commit-hook");
        repo.write("new.txt", "new\n");
        repo.write(
            ".git/test-hooks/pre-commit",
            "#!/bin/sh\necho 'hook refused this commit'\nexit 1\n",
        );
        fs::set_permissions(
            repo.path.join(".git/test-hooks/pre-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let error = commit_all(&repo.runner(), "Rejected").unwrap_err();
        assert!(error.contains("hook refused this commit"), "{error}");
        assert!(error.contains("index was not reset"), "{error}");
        assert_eq!(repo.git(&["ls-files"]).stdout.trim(), "new.txt");
        assert!(!head_exists(&repo.runner()));
    }

    #[test]
    fn a_staging_failure_never_attempts_a_commit() {
        let repo = writable_repo("commit-index-lock");
        repo.write("new.txt", "new\n");
        repo.write(".git/index.lock", "held\n");
        let error = commit_all(&repo.runner(), "Blocked").unwrap_err();
        assert!(error.starts_with("Stage failed:"), "{error}");
        assert!(!head_exists(&repo.runner()));
        assert!(!repo.path.join(".git/index").exists());
    }

    #[test]
    fn push_uses_the_configured_upstream_and_never_forces_a_rejection() {
        let remote = writable_repo("push-remote");
        assert!(remote.git(&["config", "core.bare", "true"]).ok());
        let repo = writable_repo("push-local");
        repo.write("file.txt", "first\n");
        commit_all(&repo.runner(), "First").unwrap();
        assert!(repo
            .git(&[
                "remote",
                "add",
                "origin",
                remote.path.join(".git").to_str().unwrap()
            ])
            .ok());
        assert!(repo.git(&["config", "branch.main.remote", "origin"]).ok());
        assert!(repo
            .git(&["config", "branch.main.merge", "refs/heads/main"])
            .ok());
        push(&repo.runner()).expect("initial push");
        assert_eq!(
            repo.git(&["rev-parse", "HEAD"]).stdout,
            remote.git(&["rev-parse", "main"]).stdout
        );

        repo.write("file.txt", "second\n");
        commit_all(&repo.runner(), "Second").unwrap();
        push(&repo.runner()).expect("follow-up push");
        let pushed = remote.git(&["rev-parse", "main"]).stdout;
        assert!(repo.git(&["reset", "--hard", "HEAD~1"]).ok());
        repo.write("file.txt", "divergent\n");
        commit_all(&repo.runner(), "Divergent").unwrap();
        let error = push(&repo.runner()).unwrap_err();
        assert!(error.contains("rejected"), "{error}");
        assert_eq!(remote.git(&["rev-parse", "main"]).stdout, pushed);
    }

    #[test]
    fn push_without_a_destination_reports_gits_refusal() {
        let repo = writable_repo("push-no-remote");
        repo.write("file.txt", "first\n");
        commit_all(&repo.runner(), "First").unwrap();
        let error = push(&repo.runner()).unwrap_err();
        assert!(error.starts_with("Push failed:"), "{error}");
        assert!(error.contains("No configured push destination"), "{error}");
    }

    #[test]
    fn a_remote_commit_quotes_the_message_and_disables_terminal_prompts() {
        let runner = GitRunner::remote(
            "/anchor",
            RemoteTarget {
                host: crate::remote::SshHost {
                    host_id: "test".into(),
                    hostname: "host".into(),
                    user: "user".into(),
                    port: 22,
                    identity: None,
                },
                path: "/repo with spaces".into(),
            },
        );
        let message = "-subject 'quoted'\n\n$(touch /tmp/should-not-exist); $HOME";
        let paths = args(&["-file[1].txt"]);
        let argv = write_argv(&runner, &commit_argv(message, &paths));
        assert_eq!(argv[0], "ssh");
        let command = argv.last().unwrap();
        assert!(
            command.contains("env GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never git commit --only -m "),
            "{command}"
        );
        assert!(command.contains(&shell_quote(message)), "{command}");
        assert!(command.ends_with(&shell_quote(":(top,literal)-file[1].txt")), "{command}");
        assert_eq!(commit_argv(message, &paths), args(&["commit", "--only", "-m", message, "--", ":(top,literal)-file[1].txt"]));
    }

    // ---- Argv -------------------------------------------------------------

    #[test]
    fn a_local_runner_puts_git_in_the_workspace_with_dash_c() {
        let runner = GitRunner::local("/home/pascal/Workspace/kraken");
        assert_eq!(
            runner.argv(&args(&["status", "-z"])),
            vec!["git", "-C", "/home/pascal/Workspace/kraken", "status", "-z"]
        );
    }

    #[test]
    fn a_remote_runner_sends_the_same_arguments_over_ssh() {
        let target = RemoteTarget {
            host: crate::remote::SshHost {
                host_id: "pi5".into(),
                hostname: "purplenode.local".into(),
                user: "pascal".into(),
                port: 22,
                identity: None,
            },
            path: "/home/pascal/app".into(),
        };
        let runner = GitRunner::remote("/anchor", target);
        let argv = runner.argv(&args(&["status", "-z"]));
        assert_eq!(argv[0], "ssh");
        assert!(argv.last().expect("a command").ends_with("&& git status -z"));
        // A remote call never gets less than the network floor.
        assert!(runner.deadline(QUICK) >= REMOTE_FLOOR);
        assert_eq!(GitRunner::local("/x").deadline(QUICK), QUICK);
    }

    #[test]
    fn the_log_command_lists_refs_explicitly_and_never_uses_all() {
        let argv = log_argv(200, true);
        assert!(!argv.contains(&"--all".to_string()));
        assert_eq!(
            argv,
            vec![
                "log",
                "--graph",
                "--branches",
                "--tags",
                "--remotes",
                "HEAD",
                "--format=%x1f%h%x1f%H%x1f%d%x1f%s%x1f%an%x1f%ar",
                "-200",
            ]
        );
        // An unborn HEAD would make git log fail outright, so it is left out.
        assert!(!log_argv(200, false).contains(&"HEAD".to_string()));
    }

    #[test]
    fn a_command_that_outlives_its_timeout_is_abandoned() {
        let argv = args(&["sleep", "30"]);
        let started = std::time::Instant::now();
        assert_eq!(run_argv(&argv, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_that_does_not_exist_is_a_missing_answer_not_a_panic() {
        assert_eq!(
            run_argv(&args(&["kraken-no-such-binary", "x"]), QUICK),
            None
        );
        assert_eq!(run_argv(&[], QUICK), None);
    }

    // ---- Paths ------------------------------------------------------------

    #[test]
    fn the_folder_line_shortens_the_home_folder() {
        assert_eq!(
            home_relative_to("/Users/x/Workspace/kraken", "/Users/x"),
            "~/Workspace/kraken"
        );
        assert_eq!(home_relative_to("/Users/x", "/Users/x"), "~");
        assert_eq!(home_relative_to("/opt/kraken", "/Users/x"), "/opt/kraken");
        // A folder that merely starts with the same characters is not inside it.
        assert_eq!(
            home_relative_to("/Users/xylophone/kraken", "/Users/x"),
            "/Users/xylophone/kraken"
        );
    }

    // ---- git_branch -------------------------------------------------------

    #[test]
    fn the_branch_is_read_out_of_head_without_spawning_git() {
        let dir = std::env::temp_dir().join(format!("kraken-head-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).expect("fake repo");
        fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        assert_eq!(git_branch(&dir.to_string_lossy()), "main");

        // A subdirectory finds the repo by walking up.
        let nested = dir.join("pkg").join("deep");
        fs::create_dir_all(&nested).expect("nested");
        assert_eq!(git_branch(&nested.to_string_lossy()), "main");

        // Detached HEAD shows the short hash instead of a branch name.
        fs::write(
            dir.join(".git").join("HEAD"),
            "9f1c0a3b4d5e6f708192a3b4c5d6e7f8091a2b3c\n",
        )
        .expect("HEAD");
        assert_eq!(git_branch(&dir.to_string_lossy()), "9f1c0a3b");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worktrees_gitdir_file_is_followed_to_its_head() {
        let root = std::env::temp_dir().join(format!("kraken-worktree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let real = root.join("store").join("worktrees").join("wt");
        fs::create_dir_all(&real).expect("gitdir");
        fs::write(real.join("HEAD"), "ref: refs/heads/feature\n").expect("HEAD");
        let checkout = root.join("checkout");
        fs::create_dir_all(&checkout).expect("checkout");
        fs::write(
            checkout.join(".git"),
            format!("gitdir: {}\n", real.to_string_lossy()),
        )
        .expect(".git file");

        assert_eq!(git_branch(&checkout.to_string_lossy()), "feature");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_outside_a_repository_has_no_branch() {
        assert_eq!(git_branch("/"), "");
    }

    // ---- Log parsing ------------------------------------------------------

    /// Real `git log --graph` output, with the \x1f separators written out.
    fn log_fixture() -> String {
        [
            "* \u{1f}a1b2c3d\u{1f}a1b2c3d0000000000000000000000000000000\u{1f} (HEAD -> main, origin/main)\u{1f}Add the diff pane\u{1f}Pascal\u{1f}2 hours ago",
            "* \u{1f}b2c3d4e\u{1f}b2c3d4e0000000000000000000000000000000\u{1f} (tag: v0.1)\u{1f}Release\u{1f}Pascal\u{1f}3 days ago",
            "|\\  ",
            "| * \u{1f}c3d4e5f\u{1f}c3d4e5f0000000000000000000000000000000\u{1f} (feature)\u{1f}Side work\u{1f}Ada\u{1f}4 days ago",
            "|/  ",
            "* \u{1f}d4e5f60\u{1f}d4e5f600000000000000000000000000000000\u{1f}\u{1f}First commit\u{1f}Pascal\u{1f}5 days ago",
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn graph_continuation_lines_carry_no_commit() {
        let rows = parse_log(&log_fixture(), None);
        assert_eq!(rows.len(), 6);
        assert!(rows[2].commit.is_none());
        assert_eq!(rows[2].graph, "|\\  ");
        assert!(rows[4].commit.is_none());
        assert!(rows[3].commit.is_some());
        assert_eq!(rows[3].graph, "| * ");
    }

    #[test]
    fn a_commit_row_splits_into_its_six_fields() {
        let rows = parse_log(&log_fixture(), None);
        let head = rows[0].commit.as_ref().expect("a commit");
        assert_eq!(head.short_hash, "a1b2c3d");
        assert_eq!(head.full_hash, "a1b2c3d0000000000000000000000000000000");
        assert_eq!(head.subject, "Add the diff pane");
        assert_eq!(head.author, "Pascal");
        assert_eq!(head.when, "2 hours ago");
        assert_eq!(
            head.tooltip(),
            "Add the diff pane\na1b2c3d — Pascal, 2 hours ago"
        );
    }

    #[test]
    fn only_branches_are_offered_as_checkouts() {
        let rows = parse_log(&log_fixture(), None);
        // "HEAD -> main" is where we already are; the branch itself is offered.
        assert_eq!(
            rows[0].commit.as_ref().expect("commit").branches,
            vec!["origin/main"]
        );
        assert!(rows[0].commit.as_ref().expect("commit").is_head);
        // A tag checks out the same thing as the hash, so it earns no entry.
        assert!(rows[1]
            .commit
            .as_ref()
            .expect("commit")
            .branches
            .is_empty());
        assert!(!rows[1].commit.as_ref().expect("commit").is_head);
        assert_eq!(
            rows[3].commit.as_ref().expect("commit").branches,
            vec!["feature"]
        );
    }

    #[test]
    fn hashes_are_coloured_by_whether_they_are_on_the_main_line() {
        let main: HashSet<String> = ["a1b2c3d0000000000000000000000000000000".to_string()]
            .into_iter()
            .collect();
        let rows = parse_log(&log_fixture(), Some(&main));
        assert_eq!(rows[0].commit.as_ref().expect("commit").on_main_line, Some(true));
        assert_eq!(rows[3].commit.as_ref().expect("commit").on_main_line, Some(false));
        assert_eq!(hash_color("dark", Some(true)), "#98c379");
        assert_eq!(hash_color("light", Some(true)), "#50a14f");
        assert_eq!(hash_color("dark", Some(false)), "#7a7d85");
        assert_eq!(hash_color("light", Some(false)), "#9a9da5");
    }

    #[test]
    fn with_no_master_or_main_every_hash_keeps_the_plain_text_colour() {
        let rows = parse_log(&log_fixture(), None);
        assert_eq!(rows[0].commit.as_ref().expect("commit").on_main_line, None);
        assert_eq!(hash_color("dark", None), log_text_color("dark"));
        assert_eq!(hash_color("light", None), "#4a4d55");
    }

    #[test]
    fn a_row_is_three_coloured_spans_with_its_graph_columns_intact() {
        let main: HashSet<String> = ["a1b2c3d0000000000000000000000000000000".to_string()]
            .into_iter()
            .collect();
        let rows = parse_log(&log_fixture(), Some(&main));
        assert_eq!(
            rows[0].html("dark"),
            "<span style=\"color: #c8cad0;\">*&nbsp;</span>\
             <span style=\"color: #98c379;\">a1b2c3d</span>\
             <span style=\"color: #c8cad0;\">&nbsp;(HEAD&nbsp;-&gt;&nbsp;main,&nbsp;origin/main)\
             &nbsp;&nbsp;Add&nbsp;the&nbsp;diff&nbsp;pane</span>"
        );
        // A continuation row is the graph alone, with its trailing space gone.
        assert_eq!(
            rows[4].html("light"),
            "<span style=\"color: #4a4d55;\">|/</span>"
        );
    }

    #[test]
    fn a_subject_that_looks_like_markup_is_escaped() {
        let line = "* \u{1f}abc1234\u{1f}abc1234full\u{1f}\u{1f}Fix <script> & \"quotes\"\u{1f}Ada\u{1f}now";
        let rows = parse_log(line, None);
        let html = rows[0].html("dark");
        assert!(html.contains("Fix&nbsp;&lt;script&gt;&nbsp;&amp;&nbsp;&quot;quotes&quot;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn a_failure_only_explains_itself_when_it_is_not_a_repository() {
        assert_eq!(
            log_failure_message("fatal: not a git repository (or any of the parent directories)"),
            "Not a git repository"
        );
        assert_eq!(log_failure_message("fatal: bad revision"), "No commits");
    }

    // ---- Against a real repository ----------------------------------------

    #[test]
    fn a_real_repository_reports_its_branches_and_head() {
        let repo = TestRepo::new("branches");
        let runner = repo.runner();
        assert!(!head_exists(&runner), "an unborn branch has no HEAD");
        assert_eq!(main_line_hashes(&runner), None);

        repo.write("a.txt", "one\n");
        repo.commit("init");
        repo.git(&["branch", "feature"]);

        assert!(head_exists(&runner));
        assert_eq!(local_branches(&runner), vec!["feature", "main"]);
        assert_eq!(git_branch(&repo.path.to_string_lossy()), "main");
        assert_eq!(current_branch(&runner), "main");

        // main is the main line, so its one commit is on it.
        let hashes = main_line_hashes(&runner).expect("main exists");
        assert_eq!(hashes.len(), 1);
    }

    #[test]
    fn checking_out_a_branch_moves_head_and_a_bad_target_says_why() {
        let repo = TestRepo::new("checkout");
        repo.write("a.txt", "one\n");
        repo.commit("init");
        repo.git(&["branch", "feature"]);
        let runner = repo.runner();

        assert_eq!(checkout(&runner, "feature"), Ok(()));
        assert_eq!(git_branch(&repo.path.to_string_lossy()), "feature");

        let failure = checkout(&runner, "no-such-branch").expect_err("no such branch");
        assert!(failure.contains("no-such-branch"), "{failure}");
    }

    #[test]
    fn the_history_of_a_real_repository_renders_row_by_row() {
        let repo = TestRepo::new("history");
        repo.write("a.txt", "one\n");
        repo.commit("first commit");
        repo.write("a.txt", "one\ntwo\n");
        repo.commit("second commit");

        match gather_log(&repo.runner(), MAX_COMMITS) {
            LogData::Rows(rows) => {
                assert_eq!(rows.len(), 2);
                let head = rows[0].commit.as_ref().expect("a commit");
                assert_eq!(head.subject, "second commit");
                assert!(head.is_head, "the checked-out commit is marked");
                // The decoration is "(HEAD -> main)": the branch we are already
                // on is not offered as somewhere to check out.
                assert!(head.branches.is_empty(), "{:?}", head.branches);
                assert_eq!(head.on_main_line, Some(true));
                assert_eq!(head.author, "Test");
                assert!(head.short_hash.len() >= 7);
                assert!(head.full_hash.starts_with(&head.short_hash));
                assert_eq!(rows[1].commit.as_ref().expect("commit").subject, "first commit");
            }
            other => panic!("expected rows, got {other:?}"),
        }
    }

    #[test]
    fn a_repository_with_no_commits_yet_says_so() {
        let repo = TestRepo::new("unborn");
        assert_eq!(
            gather_log(&repo.runner(), MAX_COMMITS),
            LogData::Message("No commits".to_string())
        );
    }

    #[test]
    fn a_folder_that_is_not_a_repository_says_that_instead() {
        let dir = std::env::temp_dir().join(format!("kraken-not-a-repo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let runner = GitRunner::local(dir.to_string_lossy().into_owned());
        assert_eq!(
            gather_log(&runner, MAX_COMMITS),
            LogData::Message("Not a git repository".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
