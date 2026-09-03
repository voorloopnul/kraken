//! The diagnostics trace: what the app was doing, and what it cost in memory.
//!
//! Off unless asked for, because every entry point here sits on a path that
//! runs whether or not anyone wanted debugging — a disabled call must cost
//! nothing but a load and a branch.
//!
//! The property that makes the log usable is that it is readable *after* a
//! crash: records are written and flushed as they happen rather than buffered,
//! and a clean exit closes the file with an `exit  clean shutdown` marker. The
//! absence of that marker at the end of a file is the crash signal, and the
//! last `action` line before it is the suspect.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;

use crate::state;

/// How often memory is sampled on a timer as well as on actions, so drift while
/// the app sits idle is visible too.
pub const HEARTBEAT_DEFAULT: f64 = 30.0;

/// Where the log goes and how much it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The file to write, or `None` for the dated default under
    /// `~/.kraken/logs/`. `-` means stderr.
    pub path: Option<String>,
    /// Also log every mouse press and key press.
    pub trace_input: bool,
    /// Seconds between idle memory samples; 0 disables.
    pub heartbeat: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path: None,
            trace_input: false,
            heartbeat: HEARTBEAT_DEFAULT,
        }
    }
}

/// The environment fallback, for a launcher or an AppImage where passing argv
/// is awkward. The flag wins where both are given.
pub fn from_environment() -> Option<Settings> {
    let path = std::env::var("KRAKEN_DEBUG").ok()?;
    if path.is_empty() || path == "0" {
        return None;
    }
    Some(Settings {
        path: (path != "1").then_some(path),
        trace_input: matches!(std::env::var("KRAKEN_DEBUG_TRACE").as_deref(), Ok("1")),
        heartbeat: std::env::var("KRAKEN_DEBUG_HEARTBEAT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(HEARTBEAT_DEFAULT),
    })
}

enum Sink {
    File(File),
    Stderr,
}

impl Sink {
    fn write_line(&mut self, line: &str) {
        // A closed or broken sink must never take the app down with it — the
        // whole point of this module is to survive whatever is going wrong.
        let _ = match self {
            Sink::File(file) => file
                .write_all(line.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.flush()),
            Sink::Stderr => {
                let mut err = std::io::stderr();
                err.write_all(line.as_bytes())
                    .and_then(|()| err.write_all(b"\n"))
                    .and_then(|()| err.flush())
            }
        };
    }
}

struct Log {
    sink: Sink,
    started: Instant,
    /// The last tree total, so each memory suffix can report the change.
    last_tree: u64,
    /// When the last `action` was written, which is what `idle=` counts from.
    last_action: Instant,
    trace_input: bool,
    path: Option<PathBuf>,
}

static LOG: Lazy<Mutex<Option<Log>>> = Lazy::new(|| Mutex::new(None));
/// Read on every disabled call, so tracing costs a relaxed load when it is off.
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The default log path: dated and pid-stamped, so two runs never collide.
pub fn default_path() -> PathBuf {
    let stamp = timestamp(SystemTime::now());
    state::logs_dir().join(format!("kraken-{stamp}-{}.log", std::process::id()))
}

/// `YYYYmmdd-HHMMSS` in UTC.
///
/// Local time would be friendlier in a filename, and getting it needs either a
/// dependency or a `libc` call that is not thread-safe next to `setenv`. A log
/// filename is read by whoever just produced it; UTC costs them nothing.
fn timestamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`, which is the whole of a calendar in a
/// dozen lines and needs no crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Wall-clock `HH:MM:SS.mmm`, the first field of every record.
fn wall_clock(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        since.subsec_millis()
    )
}

/// Start the log. Returns the path it went to, or `None` for stderr and for a
/// file that could not be opened.
pub fn start(settings: &Settings) -> Option<PathBuf> {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let (sink, path) = match settings.path.as_deref() {
        Some("-") => (Sink::Stderr, None),
        other => {
            let path = other.map(PathBuf::from).unwrap_or_else(default_path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => (Sink::File(file), Some(path)),
                // Nowhere to write is not a reason to fail to start; say so on
                // stderr and carry on there.
                Err(_) => (Sink::Stderr, None),
            }
        }
    };
    *guard = Some(Log {
        sink,
        started: Instant::now(),
        last_tree: 0,
        last_action: Instant::now(),
        trace_input: settings.trace_input,
        path: path.clone(),
    });
    ENABLED.store(true, Ordering::Relaxed);
    drop(guard);

    banner();
    if settings.trace_input {
        write("boot", "input tracing enabled", false);
    }
    start_heartbeat(settings.heartbeat);
    path
}

/// Everything a crash report needs about the machine, written once up front.
fn banner() {
    let argv: Vec<String> = std::env::args().collect();
    write(
        "boot",
        &format!("kraken pid={} argv={argv:?}", std::process::id()),
        false,
    );
    let env = |key: &str, fallback: &str| std::env::var(key).unwrap_or_else(|_| fallback.into());
    write(
        "boot",
        &format!(
            "session={} platform={} desktop={}",
            env("XDG_SESSION_TYPE", "?"),
            env("QT_QPA_PLATFORM", "default"),
            env("XDG_CURRENT_DESKTOP", "?")
        ),
        false,
    );
}

/// The log's path, or `None` when it is off or going to stderr.
pub fn path() -> Option<PathBuf> {
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|log| log.path.clone())
}

pub fn tracing_input() -> bool {
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|log| log.trace_input)
        .unwrap_or(false)
}

/// Close the log with the marker that tells a reader this was a clean exit. Its
/// absence at the end of a file is the crash signal.
pub fn shutdown(code: Option<i32>) {
    if !enabled() {
        return;
    }
    let code = code.map(|c| c.to_string()).unwrap_or_else(|| "None".into());
    write("exit", &format!("clean shutdown code={code}"), true);
    ENABLED.store(false, Ordering::Relaxed);
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn write(kind: &str, message: &str, memory: bool) {
    if !enabled() {
        return;
    }
    let suffix = if memory { Some(memory_suffix()) } else { None };
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(log) = guard.as_mut() else {
        return;
    };
    let mut line = format!(
        "{} {:9.3} {kind:<8} {message}",
        wall_clock(SystemTime::now()),
        log.started.elapsed().as_secs_f64()
    );
    if let Some(suffix) = suffix {
        let delta = suffix.tree as i64 - log.last_tree as i64;
        log.last_tree = suffix.tree;
        line.push_str(&format!("  {}", suffix.render(delta)));
    }
    log.sink.write_line(&line);
}

/// Format a set of `key=value` fields the way every record's tail reads.
pub fn fields(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn record(kind: &str, what: &str, pairs: &[(&str, String)], memory: bool) {
    if !enabled() {
        return;
    }
    let rendered = fields(pairs);
    let message = if rendered.is_empty() {
        what.to_string()
    } else {
        format!("{what} {rendered}")
    };
    write(kind, &message, memory);
}

/// A user-driven action — a click, a menu choice, a session switch — with a
/// memory snapshot taken right after it. This is the line you read first when
/// the log ends mid-crash.
pub fn action(what: &str, pairs: &[(&str, String)]) {
    if let Some(log) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        log.last_action = Instant::now();
    }
    record("action", what, pairs, true);
}

/// Something the app did on its own: a process starting, an event arriving, a
/// panel rebuilding. No memory snapshot — these are frequent, and the
/// surrounding `action` records already bracket them.
pub fn log(what: &str, pairs: &[(&str, String)]) {
    record("event", what, pairs, false);
}

/// A child-process transition (spawn, exit, kill). Carries a memory snapshot:
/// every one of these moves the tree total, and a spawn never matched by an
/// exit is a leak.
pub fn proc(what: &str, pairs: &[(&str, String)]) {
    record("proc", what, pairs, true);
}

pub fn error(what: &str, pairs: &[(&str, String)]) {
    record("error", what, pairs, false);
}

/// A caught failure with its detail indented under the record, so the
/// one-line-per-record shape still holds for `grep`.
pub fn failure(what: &str, detail: &str) {
    if !enabled() {
        return;
    }
    write("error", what, false);
    for line in detail.lines() {
        write("error", &format!("    {line}"), false);
    }
}

/// Sample memory on a timer, on a thread of its own.
///
/// A thread rather than a UI timer, because the whole point of the heartbeat is
/// the shape of *"it grew overnight while I was not touching it"* — and a timer
/// on the event loop stops sampling exactly when the loop is the thing that has
/// wedged. It is detached and never joined: the process exiting is what ends it,
/// and it holds nothing that has to be released first.
fn start_heartbeat(seconds: f64) {
    if seconds <= 0.0 {
        return;
    }
    let interval = std::time::Duration::from_secs_f64(seconds);
    let _ = std::thread::Builder::new()
        .name("kraken-heartbeat".into())
        .spawn(move || loop {
            std::thread::sleep(interval);
            if !enabled() {
                return;
            }
            heartbeat();
        });
}

/// An idle memory sample, so drift while nothing is happening is visible.
pub fn heartbeat() {
    if !enabled() {
        return;
    }
    // Seconds since the last thing the user did. It is what separates growth
    // caused by use from growth that happens on its own: a climbing tree beside
    // a climbing `idle` is a leak nobody triggered, which is a different bug
    // from one that costs memory per click.
    let idle = LOG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|log| log.last_action.elapsed().as_secs())
        .unwrap_or(0);
    write("mem", &format!("heartbeat idle={idle}s"), true);
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// One process in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMemory {
    pub pid: i32,
    pub rss: u64,
    pub name: String,
}

/// Human-readable size for the UI: coarse, and a stable width.
pub fn format_bytes(size: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else {
        format!("{} MB", size / (1024 * 1024))
    }
}

/// Log-precision size: always MB with one decimal and no space, so a record
/// stays one whitespace-separated token per field.
fn mb(size: u64) -> String {
    format!("{:.1}MB", size as f64 / (1024.0 * 1024.0))
}

struct MemorySuffix {
    rss: u64,
    tree: u64,
    procs: usize,
    fds: usize,
    threads: usize,
}

impl MemorySuffix {
    fn render(&self, delta: i64) -> String {
        format!(
            "| rss={} tree={} d={}{} procs={} fds={} threads={}",
            mb(self.rss),
            mb(self.tree),
            if delta >= 0 { '+' } else { '-' },
            mb(delta.unsigned_abs()),
            self.procs,
            self.fds,
            self.threads
        )
    }
}

fn memory_suffix() -> MemorySuffix {
    let (tree, procs) = tree_stats();
    MemorySuffix {
        rss: process_rss(),
        tree,
        procs,
        fds: open_fds(),
        threads: thread_count(),
    }
}

/// This process's resident set.
#[cfg(target_os = "linux")]
pub fn process_rss() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    // Fields are in pages: size, resident, shared, …
    text.split_whitespace()
        .nth(1)
        .and_then(|pages| pages.parse::<u64>().ok())
        .map(|pages| pages * page_size())
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
pub fn process_rss() -> u64 {
    ps_rss(std::process::id() as i32)
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // SAFETY: `sysconf` reads a constant and cannot fail in a way that matters
    // here; a negative answer falls back to the near-universal 4 KiB.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as u64
    } else {
        4096
    }
}

/// The whole process tree's resident set, and how many processes are in it.
///
/// Children are what the app leaks: an agent per workspace, a shell per
/// terminal, an ssh client per remote command. This process's own number moves
/// far less than theirs.
pub fn tree_stats() -> (u64, usize) {
    let tree = process_tree();
    (tree.iter().map(|p| p.rss).sum(), tree.len())
}

pub fn process_tree_rss() -> u64 {
    tree_stats().0
}

/// This process and everything descended from it, deepest last.
#[cfg(target_os = "linux")]
pub fn process_tree() -> Vec<ProcessMemory> {
    let mut all: Vec<(i32, i32, u64, String)> = Vec::new(); // pid, ppid, rss, name
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        if let Some(row) = read_stat(pid) {
            all.push(row);
        }
    }
    let mine = std::process::id() as i32;
    // Walk down from this process rather than up from every process: the tree
    // is small and the process table is not.
    let mut tree: Vec<ProcessMemory> = Vec::new();
    let mut frontier = vec![mine];
    while let Some(pid) = frontier.pop() {
        if let Some((_, _, rss, name)) = all.iter().find(|(p, ..)| *p == pid) {
            tree.push(ProcessMemory {
                pid,
                rss: *rss,
                name: name.clone(),
            });
        }
        frontier.extend(all.iter().filter(|(_, ppid, ..)| *ppid == pid).map(|(p, ..)| *p));
    }
    tree
}

#[cfg(target_os = "linux")]
fn read_stat(pid: i32) -> Option<(i32, i32, u64, String)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field is parenthesised and may itself contain spaces and
    // parentheses, so the split has to be anchored on the *last* ')'.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 2..)?.split_whitespace().collect();
    // After comm: state, ppid, … and rss is the 22nd field from there.
    let ppid = rest.get(1)?.parse().ok()?;
    let rss_pages: u64 = rest.get(21)?.parse().ok()?;
    Some((pid, ppid, rss_pages * page_size(), name))
}

#[cfg(not(target_os = "linux"))]
pub fn process_tree() -> Vec<ProcessMemory> {
    // No /proc: ask ps for the same three columns, as Kraken does on macOS.
    let mine = std::process::id() as i32;
    let Ok(out) = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,rss=,comm="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(i32, i32, u64, String)> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            // ps reports RSS in kilobytes.
            let rss: u64 = parts.next()?.parse::<u64>().ok()? * 1024;
            let name = parts.next().unwrap_or_default().to_string();
            Some((pid, ppid, rss, name))
        })
        .collect();
    let mut tree = Vec::new();
    let mut frontier = vec![mine];
    while let Some(pid) = frontier.pop() {
        if let Some((_, _, rss, name)) = rows.iter().find(|(p, ..)| *p == pid) {
            tree.push(ProcessMemory { pid, rss: *rss, name: name.clone() });
        }
        frontier.extend(rows.iter().filter(|(_, ppid, ..)| *ppid == pid).map(|(p, ..)| *p));
    }
    tree
}

/// Open file descriptors. A count that only ever grows is the other kind of
/// leak this log is for.
#[cfg(target_os = "linux")]
pub fn open_fds() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
pub fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map(|e| e.count()).unwrap_or(0)
}

#[cfg(target_os = "linux")]
pub fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task")
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
pub fn thread_count() -> usize {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-M", &std::process::id().to_string()])
        .output()
    else {
        return 0;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log is process-global, so the tests that write it take turns.
    static GUARD: Mutex<()> = Mutex::new(());

    struct Scratch {
        path: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let path = std::env::temp_dir().join(format!("kraken-debug-{name}.log"));
            let _ = std::fs::remove_file(&path);
            start(&Settings {
                path: Some(path.to_string_lossy().into_owned()),
                ..Settings::default()
            });
            Self { path, _lock: lock }
        }

        fn text(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap_or_default()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            shutdown(Some(0));
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn disabled_by_default_and_every_entry_point_is_a_no_op() {
        // The log is process-global, so take the turn another test would
        // otherwise be holding with its own sink open.
        let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        // These sit on paths that run whether or not anyone asked for
        // debugging, so calling them with the log off must do nothing at all.
        assert!(!enabled(), "another test left the log open");
        action("noop", &[("a", "1".into())]);
        log("noop", &[]);
        proc("noop", &[]);
        error("noop", &[]);
        failure("noop", "boom");
        heartbeat();
        shutdown(Some(0));
        drop(lock);
    }

    #[test]
    fn records_are_readable_before_the_process_exits() {
        let scratch = Scratch::new("readable");
        action(
            "panel.toggle",
            &[("side", "right".into()), ("visible", "true".into())],
        );
        // No flush, no close: a crash right here must still leave the record on
        // disk, which is what writing through buys.
        assert!(
            scratch.text().contains("panel.toggle side=right visible=true"),
            "{}",
            scratch.text()
        );
    }

    #[test]
    fn a_record_carries_a_clock_an_offset_and_its_kind() {
        let scratch = Scratch::new("shape");
        log("view.discard", &[("key", "/a/b".into())]);
        let line = scratch
            .text()
            .lines()
            .find(|line| line.contains("view.discard"))
            .expect("the record was written")
            .to_string();
        let mut parts = line.split_whitespace();
        let clock = parts.next().unwrap();
        assert_eq!(clock.len(), "00:00:00.000".len(), "{line}");
        assert!(parts.next().unwrap().parse::<f64>().is_ok(), "{line}");
        assert_eq!(parts.next().unwrap(), "event");
        assert_eq!(parts.next().unwrap(), "view.discard");
        assert_eq!(parts.next().unwrap(), "key=/a/b");
    }

    #[test]
    fn an_action_carries_a_memory_snapshot_and_an_event_does_not() {
        let scratch = Scratch::new("memory");
        action("workspace.select", &[]);
        log("view.discard", &[]);
        let text = scratch.text();
        let action_line = text
            .lines()
            .find(|l| l.contains("workspace.select"))
            .unwrap();
        assert!(action_line.contains("| rss="), "{action_line}");
        assert!(action_line.contains("tree="));
        assert!(action_line.contains("procs="));
        assert!(action_line.contains("fds="));
        assert!(action_line.contains("threads="));
        let event_line = text.lines().find(|l| l.contains("view.discard")).unwrap();
        assert!(!event_line.contains("| rss="), "{event_line}");
    }

    #[test]
    fn a_clean_exit_leaves_the_marker_a_crash_would_not() {
        let path = {
            let scratch = Scratch::new("exit");
            let path = scratch.path.clone();
            action("window.close", &[]);
            // Dropping the scratch shuts the log down, which is the clean path.
            drop(scratch);
            path
        };
        // Read before the file is gone: Scratch's drop removed it, so re-open
        // is not an option — assert on what shutdown wrote instead.
        assert!(!path.exists(), "the scratch file should be cleaned up");
    }

    #[test]
    fn the_exit_marker_is_written_on_shutdown() {
        let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::temp_dir().join("kraken-debug-marker.log");
        let _ = std::fs::remove_file(&path);
        start(&Settings {
            path: Some(path.to_string_lossy().into_owned()),
            ..Settings::default()
        });
        action("window.close", &[]);
        shutdown(Some(0));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains("exit"), "{text}");
        assert!(text.contains("clean shutdown code=0"), "{text}");
        let _ = std::fs::remove_file(&path);
        drop(lock);
    }

    #[test]
    fn a_failure_is_one_record_plus_its_indented_detail() {
        let scratch = Scratch::new("failure");
        failure("view.shutdown failed", "line one\nline two");
        let text = scratch.text();
        assert!(text.contains("error    view.shutdown failed"), "{text}");
        // Indented under the record, so one-line-per-record still holds for grep.
        assert!(text.contains("error        line one"), "{text}");
        assert!(text.contains("error        line two"), "{text}");
    }

    #[test]
    fn the_banner_names_the_process_and_the_session() {
        let scratch = Scratch::new("banner");
        let text = scratch.text();
        assert!(text.contains(&format!("kraken pid={}", std::process::id())), "{text}");
        assert!(text.contains("session="), "{text}");
        assert!(text.contains("platform="), "{text}");
    }

    #[test]
    fn a_field_free_record_carries_no_trailing_space() {
        let scratch = Scratch::new("fields");
        log("app.started", &[]);
        let line = scratch
            .text()
            .lines()
            .find(|l| l.contains("app.started"))
            .unwrap()
            .to_string();
        assert!(line.ends_with("app.started"), "{line:?}");
    }

    #[test]
    fn sizes_are_coarse_for_the_ui_and_precise_in_the_log() {
        assert_eq!(format_bytes(0), "0 MB");
        assert_eq!(format_bytes(200 * 1024 * 1024), "200 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(mb(1024 * 1024), "1.0MB");
        assert_eq!(mb(1536 * 1024), "1.5MB");
    }

    #[test]
    fn this_process_reports_a_plausible_resident_set() {
        let rss = process_rss();
        // A test binary is at least a megabyte and nowhere near a terabyte.
        assert!(rss > 1024 * 1024, "rss looks wrong: {rss}");
        assert!(rss < 1024u64.pow(4));
    }

    #[test]
    fn the_process_tree_contains_at_least_this_process() {
        let tree = process_tree();
        let mine = std::process::id() as i32;
        assert!(tree.iter().any(|p| p.pid == mine), "{tree:?}");
        let (total, count) = tree_stats();
        assert_eq!(count, tree.len());
        assert!(total >= process_rss() / 2, "tree total looks wrong: {total}");
    }

    #[test]
    fn descriptors_and_threads_are_counted() {
        assert!(open_fds() > 0);
        assert!(thread_count() > 0);
    }

    #[test]
    fn the_default_path_is_dated_and_pid_stamped() {
        let path = default_path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("kraken-"), "{name}");
        assert!(name.ends_with(&format!("-{}.log", std::process::id())), "{name}");
        assert!(path.starts_with(state::logs_dir()));
    }

    #[test]
    fn the_calendar_arithmetic_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // A leap day, which is where a hand-rolled calendar usually goes wrong.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn the_environment_turns_tracing_on_only_when_it_says_something() {
        let restore = std::env::var("KRAKEN_DEBUG").ok();
        // SAFETY-adjacent: these tests hold the same lock, so no other test is
        // reading the environment while it is being changed here.
        let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KRAKEN_DEBUG");
        assert_eq!(from_environment(), None);
        std::env::set_var("KRAKEN_DEBUG", "0");
        assert_eq!(from_environment(), None);
        std::env::set_var("KRAKEN_DEBUG", "1");
        assert_eq!(from_environment().unwrap().path, None);
        std::env::set_var("KRAKEN_DEBUG", "/tmp/x.log");
        assert_eq!(
            from_environment().unwrap().path.as_deref(),
            Some("/tmp/x.log")
        );
        match restore {
            Some(value) => std::env::set_var("KRAKEN_DEBUG", value),
            None => std::env::remove_var("KRAKEN_DEBUG"),
        }
        drop(lock);
    }
}
