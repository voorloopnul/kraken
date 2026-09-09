//! RPC client for the Pi coding agent (`pi --mode rpc`).
//!
//! One agent process per workspace (cwd = the workspace folder), speaking the
//! JSONL protocol from pi's `docs/rpc.md`: commands go to stdin, responses and
//! events stream back on stdout. Framing is strict — records are split on LF
//! only, with a trailing CR stripped.
//!
//! The protocol itself lives in [`Protocol`], which is pure: it frames bytes,
//! hands out request ids, tracks whether pi is busy, and says what each record
//! means. [`PiAgent`] is the part that owns a child process and two reader
//! threads, and it does nothing the pure half could have done — so the
//! interesting behaviour is testable without spawning anything.
//!
//! Commands may carry an `id`; the matching response comes back on the event
//! channel as [`AgentRecord::Response`] under that same id, which is how a
//! caller correlates the answer with what it asked. Events (no id) are
//! broadcast. Extension UI dialog requests are auto-cancelled so a headless
//! extension can never deadlock the chat.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::remote::RemoteTarget;
use crate::state;

/// How long a terminated pi is given to go away before it is killed.
const TERMINATE_GRACE: Duration = Duration::from_millis(2000);
/// How long a killed pi is waited for, so `stop` never leaves a zombie behind.
const KILL_GRACE: Duration = Duration::from_millis(1000);

/// The variable the ssh routing extension reads its connection out of.
///
/// Named here rather than written into both halves: the extension is a
/// TypeScript file in the other crate, and when the two names drifted apart the
/// extension loaded, found nothing, registered no tools, and pi quietly ran
/// every command on the local machine instead of the remote one.
pub const SSH_ENV: &str = "KRAKEN_SSH";

/// Where the bundled ssh routing extension is unpacked, for `pi -e <path>`.
/// Only the path is needed here; unpacking the asset belongs to the remote
/// layer that ships it.
pub fn ssh_extension_path() -> PathBuf {
    state::extension_dir().join("ssh_remote.ts")
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// Everything that decides how the process is started.
///
/// For a remote workspace the process still runs locally, but is launched with
/// the bundled ssh routing extension and a `KRAKEN_SSH` environment descriptor,
/// so its read/write/edit/bash tools operate on the remote host over SSH. The
/// cwd is the local anchor folder, which is where pi runs and stores its
/// session files.
#[derive(Debug, Clone)]
pub struct Launch {
    pub cwd: PathBuf,
    /// When set, the process is launched already bound to this session file
    /// (`--session <path>`), so its history loads without a switch round-trip.
    pub session_path: Option<String>,
    /// pi's `--no-session`: the session is held in memory and never written.
    /// For an agent that is asked a question and stopped again, which has no
    /// conversation worth keeping and should leave no file behind.
    pub ephemeral: bool,
    /// Disable agent tools for a text-only request supplied with all its context.
    pub no_tools: bool,
    /// When set, tool execution is routed to this remote host over SSH; pi
    /// itself still runs locally in `cwd` (the workspace's local anchor).
    pub remote: Option<RemoteTarget>,
    /// The binary to run. Always `pi` in production; a test points it at
    /// something harmless.
    pub program: String,
}

impl Launch {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            session_path: None,
            ephemeral: false,
            no_tools: false,
            remote: None,
            program: "pi".to_string(),
        }
    }

    pub fn session(mut self, path: impl Into<String>) -> Self {
        self.session_path = Some(path.into());
        self
    }

    pub fn ephemeral(mut self) -> Self {
        self.ephemeral = true;
        self
    }

    pub fn no_tools(mut self) -> Self {
        self.no_tools = true;
        self
    }

    pub fn remote(mut self, remote: RemoteTarget) -> Self {
        self.remote = Some(remote);
        self
    }

    pub fn program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// The arguments after the program name.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--mode".to_string(), "rpc".to_string()];
        if self.remote.is_some() {
            // Load the ssh routing extension; the connection itself is
            // described to it through the environment (see `env`).
            args.push("-e".to_string());
            args.push(ssh_extension_path().to_string_lossy().into_owned());
        }
        if self.no_tools {
            args.push("--no-tools".to_string());
        }
        if self.ephemeral {
            args.push("--no-session".to_string());
        } else if let Some(path) = self.session_path.as_deref().filter(|p| !p.is_empty()) {
            args.push("--session".to_string());
            args.push(path.to_string());
        }
        args
    }

    /// Environment overrides for the child, on top of what it inherits.
    pub fn env(&self) -> Vec<(String, String)> {
        match &self.remote {
            Some(remote) => vec![(SSH_ENV.to_string(), remote.env_value())],
            None => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

/// One thing that reached us from the agent, in the terms its owner cares
/// about.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentRecord {
    /// The reply to a command sent with [`PiAgent::request`], under its id.
    Response { id: String, value: Value },
    /// Any agent event: `agent_start`, `message_update`, `tool_execution_*`, …
    Event(Value),
    /// An extension asked for a message to be shown: `(message, level)` where
    /// level is `"info" | "warning" | "error"`.
    Notify { message: String, level: String },
    /// The process could not be started, or a command could not be written.
    Failed(String),
    /// The process exited; `died_mid_turn` when a turn was still streaming.
    Finished { died_mid_turn: bool },
}

/// What the reader must do with one decoded record.
#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    /// Hand this to the owner.
    Deliver(AgentRecord),
    /// Write this command back to the agent before anything else — an
    /// extension is blocked waiting on it.
    Reply(Value),
    /// Nothing to do: a response nobody is waiting for, or a UI request that
    /// wants no answer.
    Drop,
}

/// The pure half of the client: framing, request ids, and what records mean.
#[derive(Debug, Default)]
pub struct Protocol {
    buffer: Vec<u8>,
    next_id: u64,
    /// Ids sent with a command whose response has not arrived yet. A response
    /// under any other id is dropped, which is what keeps a reply that arrives
    /// after `stop` cleared the slate from being routed to a caller that has
    /// moved on.
    pending: HashSet<String>,
    is_streaming: bool,
    is_compacting: bool,
}

impl Protocol {
    pub fn new() -> Self {
        Self::default()
    }

    /// A turn is in flight (between `agent_start` and `agent_end`).
    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Pi is busy summarizing the context away. Tracked beside `is_streaming`
    /// because it is the other way pi can be occupied: a threshold compaction
    /// runs *after* `agent_end`, so a session can be busy with no turn in
    /// flight.
    pub fn is_compacting(&self) -> bool {
        self.is_compacting
    }

    pub fn set_compacting(&mut self, compacting: bool) {
        self.is_compacting = compacting;
    }

    /// Claim the next `req-N` id and remember that a response is expected.
    pub fn next_request_id(&mut self) -> String {
        self.next_id += 1;
        let id = format!("req-{}", self.next_id);
        self.pending.insert(id.clone());
        id
    }

    /// Split whatever has arrived into whole records. A record can be cut in
    /// half by the pipe, so the tail stays buffered until its LF turns up.
    pub fn frame(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.buffer.extend_from_slice(chunk);
        let mut records = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            // A line we cannot read is skipped rather than fatal: pi's stdout
            // is the only channel the session has, and one bad record must not
            // cost the rest of the conversation.
            if let Ok(record) = serde_json::from_slice::<Value>(&line) {
                records.push(record);
            }
        }
        records
    }

    /// Classify one record, updating the busy flags as it goes.
    pub fn dispatch(&mut self, record: Value) -> Dispatch {
        match record.get("type").and_then(Value::as_str).unwrap_or("") {
            "response" => {
                let id = record
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if self.pending.remove(&id) {
                    Dispatch::Deliver(AgentRecord::Response { id, value: record })
                } else {
                    Dispatch::Drop
                }
            }
            "extension_ui_request" => self.ui_request(&record),
            kind => {
                match kind {
                    "agent_start" => self.is_streaming = true,
                    "agent_end" => self.is_streaming = false,
                    "compaction_start" => self.is_compacting = true,
                    "compaction_end" => self.is_compacting = false,
                    _ => {}
                }
                Dispatch::Deliver(AgentRecord::Event(record))
            }
        }
    }

    fn ui_request(&mut self, record: &Value) -> Dispatch {
        match record.get("method").and_then(Value::as_str).unwrap_or("") {
            "notify" => Dispatch::Deliver(AgentRecord::Notify {
                message: record
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                level: record
                    .get("notifyType")
                    .and_then(Value::as_str)
                    .unwrap_or("info")
                    .to_string(),
            }),
            // No dialog UI: cancel so extensions never block the agent.
            "select" | "confirm" | "input" | "editor" => Dispatch::Reply(json!({
                "type": "extension_ui_response",
                "id": record.get("id").cloned().unwrap_or(Value::Null),
                "cancelled": true,
            })),
            // setStatus / setWidget / setTitle / set_editor_text: fire-and-forget.
            _ => Dispatch::Drop,
        }
    }

    /// Frame and classify in one step — what the reader thread does per read.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Dispatch> {
        self.frame(chunk)
            .into_iter()
            .map(|record| self.dispatch(record))
            .collect()
    }

    /// Forget the half-record and the outstanding requests. Called when the
    /// process behind them is gone: their answers are never coming, and the
    /// bytes belong to a stream that ended.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.pending.clear();
        self.is_streaming = false;
    }
}

/// The `prompt` command for one message.
///
/// `streaming` and `compacting` are the agent's busy flags: pi rejects an
/// unqualified prompt while either is true, so a busy agent gets a queueing
/// behaviour instead of an error. Steering lands the message in the running
/// turn; a compaction has no assistant turn to steer, so the message waits for
/// pi to finish rather than trying to land between its tool calls.
pub fn prompt_command(
    message: &str,
    images: &[Value],
    streaming: bool,
    compacting: bool,
) -> Value {
    let mut command = json!({ "type": "prompt", "message": message });
    if !images.is_empty() {
        // Prompt-ready objects: {"type": "image", "data": <base64>,
        // "mimeType": ...}, per the pi RPC protocol.
        command["images"] = Value::Array(images.to_vec());
    }
    if streaming {
        command["streamingBehavior"] = json!("steer");
    } else if compacting {
        command["streamingBehavior"] = json!("followUp");
    }
    command
}

/// The group to signal for `pid`, given the group it turned out to be in.
///
/// The leader check is not a formality: if `setsid` never ran, pi shares
/// Kraken's process group and signalling that group would take Kraken down with
/// it. A pi we cannot reach this way still gets its own signal; only its
/// children survive.
fn group_target(pid: i32, pgid: i32) -> Option<i32> {
    if pid > 0 && pgid == pid {
        Some(pid)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// The process
// ---------------------------------------------------------------------------

/// Where commands go.
enum Outbox {
    /// Not started yet.
    Idle,
    /// The child's stdin.
    Pipe(ChildStdin),
    /// No process at all: commands are recorded instead of written. Lets a
    /// caller (and every controller test) drive the protocol without a pi.
    Recorded(Vec<Value>),
}

impl Outbox {
    fn write(&mut self, command: &Value) -> io::Result<()> {
        match self {
            Outbox::Pipe(stdin) => {
                let mut line = command.to_string();
                line.push('\n');
                stdin.write_all(line.as_bytes())?;
                stdin.flush()
            }
            Outbox::Recorded(sent) => {
                sent.push(command.clone());
                Ok(())
            }
            Outbox::Idle => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "pi agent is not running",
            )),
        }
    }
}

/// One `pi --mode rpc` process bound to a workspace folder.
pub struct PiAgent {
    launch: Launch,
    offline: bool,
    protocol: Arc<Mutex<Protocol>>,
    outbox: Arc<Mutex<Outbox>>,
    child: Arc<Mutex<Option<Child>>>,
    /// Set by `stop`, so the reader threads of a process we are done with stay
    /// quiet: their last reads and the EOF that follows belong to a process the
    /// owner has already written off, and a stray `Finished` would clear a
    /// busy row that a replacement agent had just put up.
    detached: Arc<AtomicBool>,
    stderr: Arc<Mutex<Vec<String>>>,
    sender: Sender<AgentRecord>,
    events: Receiver<AgentRecord>,
    started: bool,
}

impl PiAgent {
    pub fn new(launch: Launch) -> Self {
        let (sender, events) = channel();
        Self {
            launch,
            offline: false,
            protocol: Arc::new(Mutex::new(Protocol::new())),
            outbox: Arc::new(Mutex::new(Outbox::Idle)),
            child: Arc::new(Mutex::new(None)),
            detached: Arc::new(AtomicBool::new(false)),
            stderr: Arc::new(Mutex::new(Vec::new())),
            sender,
            events,
            started: false,
        }
    }

    /// An agent with no process behind it: commands are recorded rather than
    /// sent, and records are fed in by hand. The whole protocol still runs.
    pub fn offline(launch: Launch) -> Self {
        let mut agent = Self::new(launch);
        agent.offline = true;
        agent
    }

    pub fn launch(&self) -> &Launch {
        &self.launch
    }

    /// Remember the session file pi bound itself to. The path is usually
    /// learned after launch (a fresh session names its own file), and a
    /// respawn after a mid-turn death has to resume *that* session — without it
    /// the next start would silently open an empty one while the transcript
    /// still shows the old conversation.
    pub fn set_session_path(&mut self, path: impl Into<String>) {
        self.launch.session_path = Some(path.into());
    }

    pub fn is_streaming(&self) -> bool {
        self.with_protocol(Protocol::is_streaming)
    }

    pub fn is_compacting(&self) -> bool {
        self.with_protocol(Protocol::is_compacting)
    }

    /// Force the compaction flag, for a `get_state` that reports a compaction
    /// already under way — no `compaction_start` is replayed for one we did
    /// not see begin.
    pub fn set_compacting(&self, compacting: bool) {
        let mut protocol = self.lock_protocol();
        protocol.set_compacting(compacting);
    }

    fn lock_protocol(&self) -> std::sync::MutexGuard<'_, Protocol> {
        self.protocol.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn with_protocol<T>(&self, read: impl Fn(&Protocol) -> T) -> T {
        read(&self.lock_protocol())
    }

    /// Whether a process is alive behind this agent.
    pub fn running(&self) -> bool {
        if self.offline {
            return self.started;
        }
        let mut slot = self.child.lock().unwrap_or_else(|error| error.into_inner());
        match slot.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Records from the agent, oldest first. Non-blocking.
    pub fn drain(&self) -> Vec<AgentRecord> {
        self.events.try_iter().collect()
    }

    /// The channel itself, for a caller that wants to block on it.
    pub fn events(&self) -> &Receiver<AgentRecord> {
        &self.events
    }

    /// The last lines pi wrote to stderr — the only explanation available when
    /// it exits without saying anything on the protocol channel.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Commands sent by an [`offline`](Self::offline) agent, in order.
    pub fn recorded(&self) -> Vec<Value> {
        match &*self.outbox.lock().unwrap_or_else(|error| error.into_inner()) {
            Outbox::Recorded(sent) => sent.clone(),
            _ => Vec::new(),
        }
    }

    // ---- Lifecycle ------------------------------------------------------

    pub fn ensure_started(&mut self) -> io::Result<()> {
        if self.started && (self.offline || self.running()) {
            return Ok(());
        }
        if self.offline {
            *self.outbox.lock().unwrap_or_else(|e| e.into_inner()) = Outbox::Recorded(Vec::new());
            self.started = true;
            return Ok(());
        }
        self.spawn()
    }

    fn spawn(&mut self) -> io::Result<()> {
        let mut command = Command::new(&self.launch.program);
        command
            .args(self.launch.args())
            .current_dir(&self.launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in self.launch.env() {
            command.env(key, value);
        }
        // Put pi in a session (and so a process group) of its own, so `stop`
        // can signal the whole tree rather than just the leader — pi's tools
        // leave children behind (an ssh client per remote command, a shell per
        // local one) and those outlive a plain terminate, holding their
        // connections open.
        unsafe {
            // Between fork and exec only async-signal-safe calls are allowed.
            // setsid is one of them, and it is all this closure does.
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("could not start {}: {error}", self.launch.program);
                let _ = self.sender.send(AgentRecord::Failed(message));
                return Err(error);
            }
        };

        crate::debug::proc(
            "pi.start",
            &[
                ("pid", child.id().to_string()),
                ("cwd", self.launch.cwd.display().to_string()),
                ("remote", self.launch.remote.is_some().to_string()),
            ],
        );
        self.detached.store(false, Ordering::SeqCst);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        if let Some(stdin) = child.stdin.take() {
            *self.outbox.lock().unwrap_or_else(|e| e.into_inner()) = Outbox::Pipe(stdin);
        }
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        self.started = true;

        if let Some(stdout) = stdout {
            self.read_stdout(stdout);
        }
        if let Some(stderr) = stderr {
            self.read_stderr(stderr);
        }
        Ok(())
    }

    fn read_stdout(&self, mut stdout: std::process::ChildStdout) {
        let protocol = Arc::clone(&self.protocol);
        let outbox = Arc::clone(&self.outbox);
        let child = Arc::clone(&self.child);
        let detached = Arc::clone(&self.detached);
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                let read = match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                // The lock is held only for the framing, never across the
                // write below: an extension reply goes through the same outbox
                // a `send` on the owner's thread uses.
                let dispatches = {
                    let mut protocol = protocol.lock().unwrap_or_else(|e| e.into_inner());
                    protocol.feed(&chunk[..read])
                };
                if detached.load(Ordering::SeqCst) {
                    break;
                }
                for dispatch in dispatches {
                    match dispatch {
                        Dispatch::Deliver(record) => {
                            if sender.send(record).is_err() {
                                return; // the owner is gone
                            }
                        }
                        Dispatch::Reply(command) => {
                            let mut outbox = outbox.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = outbox.write(&command);
                        }
                        Dispatch::Drop => {}
                    }
                }
            }

            // A clean exit (nonzero code, provider drop, ssh extension death on
            // a remote) says nothing on the protocol channel, so EOF here is
            // the only place a mid-turn death can be noticed. Report whether a
            // turn was still in flight, so the owner can clear the busy row and
            // say the turn was cut short.
            let died_mid_turn = {
                let mut protocol = protocol.lock().unwrap_or_else(|e| e.into_inner());
                let streaming = protocol.is_streaming();
                protocol.reset();
                streaming
            };
            // Reap, so a finished pi does not sit around as a zombie until
            // someone calls stop. Bounded: a child that closed stdout but is
            // still alive must not hold this thread (and the child lock) open.
            let deadline = Instant::now() + KILL_GRACE;
            while Instant::now() < deadline {
                let mut slot = child.lock().unwrap_or_else(|e| e.into_inner());
                match slot.as_mut().map(Child::try_wait) {
                    Some(Ok(None)) => {}
                    _ => break,
                }
                drop(slot);
                std::thread::sleep(Duration::from_millis(20));
            }
            if !detached.load(Ordering::SeqCst) {
                let _ = sender.send(AgentRecord::Finished { died_mid_turn });
            }
        });
    }

    fn read_stderr(&self, stderr: std::process::ChildStderr) {
        let tail = Arc::clone(&self.stderr);
        std::thread::spawn(move || {
            // Drained even though nothing reads most of it: a stderr pipe
            // nobody empties fills up, and pi blocks writing to it.
            let mut reader = io::BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = line.trim_end().to_string();
                if text.is_empty() {
                    continue;
                }
                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                if tail.len() == 20 {
                    tail.remove(0);
                }
                tail.push(text);
            }
        });
    }

    /// Kill the agent and everything it spawned.
    pub fn stop(&mut self) {
        // Whatever was in flight ends here, and nothing else will clear the
        // flag: a flag left set would make the next prompt steer itself into a
        // session that no longer exists. Cleared before the early return, so
        // "stopped" always means "not streaming", whatever state the process
        // was in.
        self.lock_protocol().reset();
        self.detached.store(true, Ordering::SeqCst);
        self.started = false;
        // Closing stdin is the polite half of the ask: a pi still reading
        // commands sees its input end.
        *self.outbox.lock().unwrap_or_else(|e| e.into_inner()) = Outbox::Idle;

        let mut child = match self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(child) => child,
            None => return,
        };
        let pid = child.id() as i32;
        crate::debug::proc("pi.terminate", &[("pid", pid.to_string())]);
        signal_group(pid, libc::SIGTERM);
        // The leader signal above may not have reached it (no setsid), so the
        // process itself is always signalled too.
        let _ = child.kill_with(libc::SIGTERM);
        if !wait_for(&mut child, TERMINATE_GRACE) {
            // SIGTERM was ignored — worth knowing, since a pi that will not die
            // is also one that keeps its memory and its SSH connection.
            crate::debug::proc("pi.kill", &[("pid", pid.to_string())]);
            signal_group(pid, libc::SIGKILL);
            let _ = child.kill();
            wait_for(&mut child, KILL_GRACE);
            return;
        }
        crate::debug::proc("pi.exit", &[("pid", pid.to_string())]);
    }
}

impl Drop for PiAgent {
    fn drop(&mut self) {
        // An agent that goes out of scope must not leave a pi (and its ssh
        // clients) running behind it.
        self.stop();
    }
}

/// `Child::kill` with a signal of our choosing.
trait KillWith {
    fn kill_with(&mut self, signal: i32) -> io::Result<()>;
}

impl KillWith for Child {
    fn kill_with(&mut self, signal: i32) -> io::Result<()> {
        let pid = self.id() as i32;
        if pid <= 0 {
            return Ok(());
        }
        // Safe as long as the pid is still ours: `Child` has not been reaped
        // (we never call wait without dropping the handle), so the pid cannot
        // have been recycled onto someone else's process.
        if unsafe { libc::kill(pid, signal) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn signal_group(pid: i32, signal: i32) {
    // getpgid/killpg on a pid we own; both are read-only about our own state.
    let pgid = unsafe { libc::getpgid(pid) };
    if let Some(group) = group_target(pid, pgid) {
        unsafe {
            libc::killpg(group, signal);
        }
    }
}

fn wait_for(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

impl PiAgent {
    /// Send one command, wanting no reply.
    pub fn send(&mut self, command: Value) -> io::Result<()> {
        self.ensure_started()?;
        let result = {
            let mut outbox = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
            outbox.write(&command)
        };
        if let Err(error) = &result {
            let _ = self
                .sender
                .send(AgentRecord::Failed(format!("pi agent write failed: {error}")));
        }
        result
    }

    /// Send one command and return the id its response will carry.
    pub fn request(&mut self, mut command: Value) -> io::Result<String> {
        self.ensure_started()?;
        let id = self.lock_protocol().next_request_id();
        command["id"] = Value::String(id.clone());
        self.send(command)?;
        Ok(id)
    }

    pub fn prompt(&mut self, message: &str, images: &[Value]) -> io::Result<String> {
        let (streaming, compacting) = {
            let protocol = self.lock_protocol();
            (protocol.is_streaming(), protocol.is_compacting())
        };
        self.request(prompt_command(message, images, streaming, compacting))
    }

    /// Stop the current turn.
    ///
    /// A running bash tool is cancelled through pi's own `_bashAbortController`,
    /// which plain `abort` never touches: `abort` calls `agent.abort()` then
    /// awaits idle, but a blocked command keeps the session busy forever, so
    /// `abort` alone can't interrupt a hung command. `abort_bash` goes first to
    /// kill any in-flight command (a no-op when none is running), then `abort`
    /// ends the turn.
    pub fn abort(&mut self) -> io::Result<()> {
        if !self.running() {
            return Ok(());
        }
        self.send(json!({ "type": "abort_bash" }))?;
        self.send(json!({ "type": "abort" }))
    }

    pub fn get_state(&mut self) -> io::Result<String> {
        self.request(json!({ "type": "get_state" }))
    }

    /// Token totals, cost, and the current context-window usage. Like every
    /// command this starts pi if it isn't running, so callers that only want to
    /// read a number check [`running`](Self::running) first.
    pub fn get_session_stats(&mut self) -> io::Result<String> {
        self.request(json!({ "type": "get_session_stats" }))
    }

    pub fn get_available_models(&mut self) -> io::Result<String> {
        self.request(json!({ "type": "get_available_models" }))
    }

    pub fn set_model(&mut self, provider: &str, model_id: &str) -> io::Result<String> {
        self.request(json!({
            "type": "set_model", "provider": provider, "modelId": model_id
        }))
    }

    pub fn set_thinking_level(&mut self, level: &str) -> io::Result<String> {
        self.request(json!({ "type": "set_thinking_level", "level": level }))
    }

    pub fn get_messages(&mut self) -> io::Result<String> {
        self.request(json!({ "type": "get_messages" }))
    }

    pub fn switch_session(&mut self, session_path: &str) -> io::Result<String> {
        self.request(json!({ "type": "switch_session", "sessionPath": session_path }))
    }

    pub fn new_session(&mut self) -> io::Result<String> {
        self.request(json!({ "type": "new_session" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::SshHost;

    fn protocol() -> Protocol {
        Protocol::new()
    }

    fn kinds(dispatches: &[Dispatch]) -> Vec<&str> {
        dispatches
            .iter()
            .map(|dispatch| match dispatch {
                Dispatch::Deliver(AgentRecord::Event(_)) => "event",
                Dispatch::Deliver(AgentRecord::Response { .. }) => "response",
                Dispatch::Deliver(AgentRecord::Notify { .. }) => "notify",
                Dispatch::Deliver(_) => "other",
                Dispatch::Reply(_) => "reply",
                Dispatch::Drop => "drop",
            })
            .collect()
    }

    #[test]
    fn a_record_split_across_two_reads_is_held_until_its_newline_arrives() {
        let mut protocol = protocol();
        assert!(protocol.frame(br#"{"type":"agent_"#).is_empty());
        let records = protocol.frame(b"start\"}\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "agent_start");
    }

    #[test]
    fn several_records_in_one_read_come_back_in_order() {
        let mut protocol = protocol();
        let records = protocol.frame(b"{\"type\":\"a\"}\n{\"type\":\"b\"}\n{\"type\":\"c\"}\n");
        let types: Vec<&str> = records
            .iter()
            .map(|record| record["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(types, ["a", "b", "c"]);
    }

    #[test]
    fn crlf_endings_do_not_leave_a_stray_carriage_return_in_the_json() {
        let mut protocol = protocol();
        let records = protocol.frame(b"{\"type\":\"agent_end\"}\r\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "agent_end");
    }

    #[test]
    fn blank_and_undecodable_lines_are_skipped_rather_than_fatal() {
        let mut protocol = protocol();
        let records = protocol.frame(b"\n   \nnot json at all\n{\"type\":\"ok\"}\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "ok");
        // Invalid UTF-8 is just another line we cannot read.
        let records = protocol.frame(b"\xff\xfe\n{\"type\":\"after\"}\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "after");
    }

    #[test]
    fn a_response_is_routed_to_the_request_that_asked_for_it() {
        let mut protocol = protocol();
        let first = protocol.next_request_id();
        let second = protocol.next_request_id();
        assert_eq!(first, "req-1");
        assert_eq!(second, "req-2");
        let dispatch = protocol.dispatch(json!({"type": "response", "id": "req-2"}));
        match dispatch {
            Dispatch::Deliver(AgentRecord::Response { id, .. }) => assert_eq!(id, "req-2"),
            other => panic!("expected the response, got {other:?}"),
        }
        // Answered once and only once; the still-pending request is untouched.
        assert_eq!(
            protocol.dispatch(json!({"type": "response", "id": "req-2"})),
            Dispatch::Drop
        );
        assert!(matches!(
            protocol.dispatch(json!({"type": "response", "id": "req-1"})),
            Dispatch::Deliver(AgentRecord::Response { .. })
        ));
    }

    #[test]
    fn a_response_nobody_is_waiting_for_is_dropped() {
        let mut protocol = protocol();
        assert_eq!(
            protocol.dispatch(json!({"type": "response", "id": "req-9"})),
            Dispatch::Drop
        );
        // ...including one that arrives after the slate was cleared.
        protocol.next_request_id();
        protocol.reset();
        assert_eq!(
            protocol.dispatch(json!({"type": "response", "id": "req-1"})),
            Dispatch::Drop
        );
    }

    #[test]
    fn streaming_and_compacting_track_their_own_events() {
        let mut protocol = protocol();
        assert!(!protocol.is_streaming() && !protocol.is_compacting());
        protocol.feed(b"{\"type\":\"agent_start\"}\n");
        assert!(protocol.is_streaming());
        // A compaction can run after the turn ended, so the two flags are
        // independent.
        protocol.feed(b"{\"type\":\"agent_end\"}\n{\"type\":\"compaction_start\"}\n");
        assert!(!protocol.is_streaming());
        assert!(protocol.is_compacting());
        protocol.feed(b"{\"type\":\"compaction_end\"}\n");
        assert!(!protocol.is_compacting());
    }

    #[test]
    fn a_reset_after_the_process_dies_clears_the_streaming_flag() {
        // reset() belongs to a dead process: the turn it was streaming is over.
        let mut protocol = protocol();
        protocol.feed(b"{\"type\":\"agent_start\"}\n");
        protocol.reset();
        assert!(!protocol.is_streaming());
    }

    #[test]
    fn a_dialog_request_is_cancelled_so_an_extension_cannot_deadlock_the_chat() {
        let mut protocol = protocol();
        for method in ["select", "confirm", "input", "editor"] {
            let dispatch = protocol.dispatch(json!({
                "type": "extension_ui_request", "method": method, "id": "ui-7"
            }));
            assert_eq!(
                dispatch,
                Dispatch::Reply(json!({
                    "type": "extension_ui_response", "id": "ui-7", "cancelled": true
                })),
                "{method} should be auto-cancelled"
            );
        }
    }

    #[test]
    fn a_notify_request_is_surfaced_and_a_status_update_is_ignored() {
        let mut protocol = protocol();
        let dispatch = protocol.dispatch(json!({
            "type": "extension_ui_request", "method": "notify",
            "message": "build failed", "notifyType": "error"
        }));
        assert_eq!(
            dispatch,
            Dispatch::Deliver(AgentRecord::Notify {
                message: "build failed".into(),
                level: "error".into()
            })
        );
        // A notify without a type is informational, and fire-and-forget
        // methods want no answer at all.
        assert_eq!(
            protocol.dispatch(json!({"type": "extension_ui_request", "method": "notify"})),
            Dispatch::Deliver(AgentRecord::Notify {
                message: String::new(),
                level: "info".into()
            })
        );
        assert_eq!(
            protocol.dispatch(json!({"type": "extension_ui_request", "method": "setStatus"})),
            Dispatch::Drop
        );
    }

    #[test]
    fn events_reach_the_owner_while_protocol_traffic_does_not() {
        let mut protocol = protocol();
        protocol.next_request_id();
        let dispatches = protocol.feed(
            b"{\"type\":\"agent_start\"}\n\
              {\"type\":\"response\",\"id\":\"req-1\"}\n\
              {\"type\":\"extension_ui_request\",\"method\":\"confirm\",\"id\":\"u1\"}\n\
              {\"type\":\"message_update\"}\n",
        );
        assert_eq!(kinds(&dispatches), ["event", "response", "reply", "event"]);
    }

    #[test]
    fn a_prompt_sent_to_a_busy_agent_says_how_it_should_be_queued() {
        let plain = prompt_command("hi", &[], false, false);
        assert_eq!(plain["message"], "hi");
        assert!(plain.get("streamingBehavior").is_none());
        assert!(plain.get("images").is_none());

        assert_eq!(
            prompt_command("hi", &[], true, false)["streamingBehavior"],
            "steer"
        );
        // Compaction has no assistant turn to steer, so the message follows up.
        assert_eq!(
            prompt_command("hi", &[], false, true)["streamingBehavior"],
            "followUp"
        );
        // A turn in flight wins: steering is what lands the message soonest.
        assert_eq!(
            prompt_command("hi", &[], true, true)["streamingBehavior"],
            "steer"
        );

        let image = json!({"type": "image", "data": "AAA", "mimeType": "image/png"});
        let with_image = prompt_command("", std::slice::from_ref(&image), false, false);
        assert_eq!(with_image["images"], json!([image]));
    }

    // ---- Launch ---------------------------------------------------------

    #[test]
    fn a_local_workspace_launches_a_plain_rpc_agent() {
        let launch = Launch::new("/home/pascal/Workspace/kraken");
        assert_eq!(launch.args(), ["--mode", "rpc"]);
        assert!(launch.env().is_empty());
    }

    #[test]
    fn a_resumed_session_is_bound_at_launch_and_an_ephemeral_one_is_never_written() {
        let resumed = Launch::new("/tmp").session("/home/pascal/.pi/agent/sessions/x.jsonl");
        assert_eq!(
            resumed.args(),
            [
                "--mode",
                "rpc",
                "--session",
                "/home/pascal/.pi/agent/sessions/x.jsonl"
            ]
        );
        let ephemeral = Launch::new("/tmp").ephemeral();
        assert_eq!(ephemeral.args(), ["--mode", "rpc", "--no-session"]);
        // --no-session wins: an ephemeral agent has no file to resume, and
        // passing both would ask pi for two contradictory things.
        let both = Launch::new("/tmp").session("/x.jsonl").ephemeral();
        assert_eq!(both.args(), ["--mode", "rpc", "--no-session"]);
    }

    #[test]
    fn a_remote_workspace_loads_the_ssh_extension_and_describes_the_connection() {
        let remote = RemoteTarget {
            host: SshHost {
                host_id: "pi5".into(),
                hostname: "purplenode.local".into(),
                user: "pascal".into(),
                port: 22,
                identity: None,
            },
            path: "/home/pascal/Workspace/app1".into(),
        };
        let launch = Launch::new("/home/pascal/.kraken/remotes/pi5-app1").remote(remote);
        let args = launch.args();
        let extension = args.iter().position(|arg| arg == "-e").expect("-e is passed");
        assert!(args[extension + 1].ends_with("ssh_remote.ts"));
        let env = launch.env();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "KRAKEN_SSH");
        let descriptor: Value = serde_json::from_str(&env[0].1).expect("a JSON descriptor");
        assert_eq!(descriptor["destination"], "pascal@purplenode.local");
        assert_eq!(descriptor["remotePath"], "/home/pascal/Workspace/app1");
    }

    // ---- Signalling -----------------------------------------------------

    #[test]
    fn the_group_signal_refuses_to_fire_at_krakens_own_group() {
        // Not a leader: setsid never ran, so this group is ours as well.
        assert_eq!(group_target(4242, 4200), None);
        assert_eq!(group_target(4242, 4242), Some(4242));
        assert_eq!(group_target(-1, -1), None);
        // getpgid failing (-1) must not be read as "the whole world".
        assert_eq!(group_target(4242, -1), None);
    }

    // ---- Commands -------------------------------------------------------

    fn offline() -> PiAgent {
        PiAgent::offline(Launch::new("/tmp"))
    }

    #[test]
    fn every_command_that_wants_an_answer_carries_its_own_id() {
        let mut agent = offline();
        let state = agent.get_state().expect("sent");
        let models = agent.get_available_models().expect("sent");
        assert_eq!((state.as_str(), models.as_str()), ("req-1", "req-2"));
        let sent = agent.recorded();
        assert_eq!(sent[0], json!({"type": "get_state", "id": "req-1"}));
        assert_eq!(
            sent[1],
            json!({"type": "get_available_models", "id": "req-2"})
        );
    }

    #[test]
    fn the_commands_pi_understands_are_sent_in_its_own_spelling() {
        let mut agent = offline();
        agent.set_model("anthropic", "claude-opus-4-8").expect("sent");
        agent.set_thinking_level("high").expect("sent");
        agent.switch_session("/tmp/s.jsonl").expect("sent");
        agent.new_session().expect("sent");
        agent.get_messages().expect("sent");
        agent.get_session_stats().expect("sent");
        let sent = agent.recorded();
        assert_eq!(sent[0]["type"], "set_model");
        assert_eq!(sent[0]["modelId"], "claude-opus-4-8");
        assert_eq!(sent[1], json!({"type": "set_thinking_level", "level": "high", "id": "req-2"}));
        assert_eq!(sent[2]["sessionPath"], "/tmp/s.jsonl");
        assert_eq!(sent[3]["type"], "new_session");
        assert_eq!(sent[4]["type"], "get_messages");
        assert_eq!(sent[5]["type"], "get_session_stats");
    }

    #[test]
    fn an_abort_kills_the_running_command_before_it_ends_the_turn() {
        let mut agent = offline();
        agent.get_state().expect("starts the agent");
        agent.abort().expect("aborted");
        let types: Vec<String> = agent
            .recorded()
            .iter()
            .map(|command| command["type"].to_string())
            .collect();
        assert_eq!(types, ["\"get_state\"", "\"abort_bash\"", "\"abort\""]);
    }

    #[test]
    fn aborting_an_agent_that_never_started_sends_nothing() {
        let mut agent = offline();
        agent.abort().expect("no-op");
        assert!(agent.recorded().is_empty());
    }

    #[test]
    fn stopping_clears_the_streaming_flag_even_with_no_process_to_kill() {
        // stop() is the only thing that clears it, and a flag left set would
        // make the next prompt steer itself into a session that is gone.
        let mut agent = offline();
        agent.get_state().expect("starts the agent");
        agent.lock_protocol().feed(b"{\"type\":\"agent_start\"}\n");
        assert!(agent.is_streaming());
        agent.stop();
        assert!(!agent.is_streaming());
        assert!(!agent.running());
    }

    #[test]
    fn a_prompt_to_a_streaming_agent_steers_the_turn_in_flight() {
        let mut agent = offline();
        agent.get_state().expect("starts the agent");
        agent.lock_protocol().feed(b"{\"type\":\"agent_start\"}\n");
        agent.prompt("and also this", &[]).expect("sent");
        let prompt = agent.recorded().pop().expect("a prompt");
        assert_eq!(prompt["streamingBehavior"], "steer");
        assert_eq!(prompt["id"], "req-2");
    }

    // ---- With a real child ----------------------------------------------
    //
    // A shell script that echoes its input is a stand-in for pi: it reads
    // commands from stdin and writes them straight back, which exercises the
    // pipes, the framing, the reader thread and the kill path without a real
    // agent. It has to be a script rather than `cat` itself because the launch
    // arguments (`--mode rpc`) are passed to whatever is run.

    fn echo_agent(name: &str) -> PiAgent {
        let path = std::env::temp_dir().join(format!("kraken-fake-pi-{name}.sh"));
        std::fs::write(&path, "#!/bin/sh\nexec cat\n").expect("the stand-in is written");
        let mut mode = std::fs::metadata(&path).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&path, mode).expect("the stand-in is executable");
        PiAgent::new(Launch::new("/tmp").program(path.to_string_lossy().into_owned()))
    }

    #[test]
    fn a_record_written_by_the_child_arrives_on_the_event_channel() {
        let mut agent = echo_agent("echo");
        agent
            .send(json!({"type": "agent_start"}))
            .expect("the command reaches the child");
        let record = agent
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("the echo comes back");
        assert_eq!(record, AgentRecord::Event(json!({"type": "agent_start"})));
        assert!(agent.is_streaming(), "the flag follows the record");
        agent.stop();
    }

    #[test]
    fn the_child_leads_its_own_process_group_so_its_tree_can_be_signalled() {
        let mut agent = echo_agent("group");
        agent.send(json!({"type": "get_state"})).expect("started");
        let pid = agent
            .child
            .lock()
            .unwrap()
            .as_ref()
            .map(|child| child.id() as i32)
            .expect("a running child");
        // setsid ran, so pi is its own leader — which is the precondition
        // signal_group checks before it fires at a whole group.
        assert_eq!(unsafe { libc::getpgid(pid) }, pid);
        agent.stop();
        assert!(!agent.running());
        // The pid is gone: `kill(pid, 0)` on a reaped child fails with ESRCH.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn a_program_that_does_not_exist_is_reported_rather_than_panicking() {
        let mut agent = PiAgent::new(Launch::new("/tmp").program("/nonexistent/pi"));
        assert!(agent.get_state().is_err());
        assert!(matches!(
            agent.drain().first(),
            Some(AgentRecord::Failed(_))
        ));
    }
}
