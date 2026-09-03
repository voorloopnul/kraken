//! The pty half of the terminal: a shell in a session of its own.
//!
//! Everything here is about handing a child process a terminal that is really
//! *its own*: a pty it holds as its controlling terminal (so the kernel has
//! somewhere to send SIGWINCH and Ctrl-C), sized before the shell is born (so
//! its first prompt is laid out for the grid that is actually on screen), and
//! an environment describing the user's machine rather than the one Kraken is
//! running on.
//!
//! The work between `fork` and `exec` is the delicate part: only
//! async-signal-safe calls are allowed there, so every allocation — argv, envp,
//! the slave path, the working directory — happens in the parent, before the
//! fork, and the child does nothing but syscalls on what it was handed.
//!
//! Teardown escalates rather than trusting one signal: SIGHUP (what a real
//! terminal hanging up sends), then SIGTERM, then SIGKILL, reaping the child
//! and closing the master exactly once.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::remote::RemoteTarget;

/// Variables naming the Python *Kraken itself* may be running on. A shell opened
/// in a workspace must not inherit any of them: with `VIRTUAL_ENV` set, `uv`
/// and `pip` resolve against Kraken's virtualenv from any directory (it outranks
/// the `.venv` sitting right there), `PYTHONPATH` makes Kraken's own sources
/// importable from the user's project, and `PYTHONHOME` points a bare `python`
/// at Kraken's runtime instead of theirs.
const OWN_RUNTIME_VARS: [&str; 3] = ["VIRTUAL_ENV", "PYTHONHOME", "PYTHONPATH"];

/// What a bundled launcher (AppImage, or a wrapper script in a checkout) puts
/// into the environment so that Kraken's *own* process can find its bundled Qt
/// and libraries. Every one of them is a lie about the user's system: a shell
/// that keeps `LD_LIBRARY_PATH` links everything it runs against the bundle's
/// libstdc++, and one that keeps the Qt plugin paths hands them to any Qt
/// program the user starts from it.
///
/// PATH is deliberately *not* on this list — the launcher's additions to it
/// (Homebrew prefixes, the bundled `pi`) are for the terminal's benefit as much
/// as the agent's, and a bundle started from a desktop launcher inherits a bare
/// PATH without them.
const LAUNCHER_VARS: [&str; 9] = [
    "APPDIR",
    "APPIMAGE",
    "ARGV0",
    "OWD",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "QT_PLUGIN_PATH",
    "QML2_IMPORT_PATH",
    "QT_QPA_PLATFORM_PLUGIN_PATH",
];

/// The escalation a deliberate teardown walks. SIGHUP is what a real terminal
/// sends when it hangs up and what a shell is written to handle; SIGTERM is for
/// the child that ignores a hangup because it thinks it is being disconnected
/// rather than closed; SIGKILL is for the one that ignores both.
pub const TEARDOWN_SIGNALS: [c_int; 3] = [libc::SIGHUP, libc::SIGTERM, libc::SIGKILL];

/// How long each step of the escalation waits for the child to become reapable
/// before promoting to the next signal. Kept short because teardown runs while
/// a pane is closing: waiting longer will not move a shell that is ignoring the
/// signal, and the last step cannot be ignored at all.
const SIGNAL_GRACE: Duration = Duration::from_millis(200);

/// How long a reap on the EOF path retries. The child that just closed the
/// master is dying but not yet reapable, so a bare `waitpid(WNOHANG)` races it
/// and collects nothing.
const EOF_GRACE: Duration = Duration::from_millis(1000);

/// The directories ncurses itself searches for a terminfo entry. An entry lives
/// in a directory named for the first character of its name: the letter on
/// Linux, the character's hex byte on macOS ('x' -> 78).
const TERMINFO_ROOTS: [&str; 3] = ["/usr/share/terminfo", "/etc/terminfo", "/lib/terminfo"];

/// What Kraken claims to be. The VT engine in [`super::vt`] implements the xterm
/// feature set — 256 colours, truecolor SGR, the usual CSI repertoire — so this
/// is the honest name for it. Claiming something richer (`xterm-ghostty`, say)
/// would have the shell and its full-screen programs emit sequences the engine
/// does not implement.
const PREFERRED_TERM: &str = "xterm-256color";

/// The environment for a shell in a workspace: this process's, less the parts
/// that describe the runtime Kraken is itself running on and the bundle it was
/// launched from.
///
/// Split from [`shell_env`] so it can be tested on a made-up environment rather
/// than the developer's own: the leak this guards against is in what gets
/// copied, and it is visible in the map.
pub fn scrub_env<I>(vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env: BTreeMap<String, String> = vars.into_iter().collect();
    let venv = env.get("VIRTUAL_ENV").cloned().filter(|v| !v.is_empty());
    for name in OWN_RUNTIME_VARS.iter().chain(LAUNCHER_VARS.iter()) {
        env.remove(*name);
    }
    if let Some(venv) = venv {
        // Unsetting VIRTUAL_ENV is not enough on its own: activation also puts
        // the virtualenv's bin first on PATH, and that is what a bare `python`
        // resolves through.
        let bin = format!("{}/bin", venv.trim_end_matches('/'));
        if let Some(path) = env.get("PATH").cloned() {
            let kept: Vec<&str> = path.split(':').filter(|entry| *entry != bin).collect();
            env.insert("PATH".to_string(), kept.join(":"));
        }
    }
    env
}

/// This process's environment, scrubbed for a child shell.
pub fn shell_env() -> BTreeMap<String, String> {
    scrub_env(std::env::vars())
}

/// The TERM to advertise, checked against the terminfo database the child will
/// actually search. Claiming a name that is not installed leaves the shell with
/// an unknown terminal — no colours, no line editing worth the name — so fall
/// back to the entry every system has.
pub fn pick_term() -> &'static str {
    let mut roots: Vec<PathBuf> = TERMINFO_ROOTS.iter().map(PathBuf::from).collect();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".terminfo"));
    }
    for root in roots {
        for first in ["x", "78"] {
            if root.join(first).join(PREFERRED_TERM).exists() {
                return PREFERRED_TERM;
            }
        }
    }
    "xterm"
}

/// What to exec on the pty, and what it should see around itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Where the child starts. The chdir happens *in the child* — Kraken's own
    /// working directory is shared by every thread in the process, so moving it
    /// around a spawn would be visible to anything else running at the time.
    pub cwd: Option<String>,
}

impl Command {
    /// The user's login shell, in `cwd`.
    pub fn local_shell(cwd: Option<&str>) -> Self {
        // A desktop launcher hands the app a bare environment that may carry no
        // SHELL at all, so the fallback has to be a shell that is certainly
        // installed rather than nothing.
        let default_shell = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        let program = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_shell.to_string());
        let mut env = shell_env();
        env.insert("TERM".to_string(), pick_term().to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        Self {
            argv: vec![program.clone()],
            program,
            env,
            cwd: cwd.map(str::to_string),
        }
    }

    /// An `ssh` client on the local pty, opening an interactive login shell on
    /// the remote host in the workspace path.
    pub fn remote_shell(target: &RemoteTarget) -> Self {
        let mut env = shell_env();
        // The remote host is unlikely to have anything but the common entries,
        // and ssh forwards TERM to the pty it allocates over there.
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let argv = target.terminal_argv();
        Self {
            program: which("ssh").unwrap_or_else(|| "/usr/bin/ssh".to_string()),
            argv,
            env,
            // ssh does not care where it is started from, and the remote
            // command already begins with a `cd`.
            cwd: None,
        }
    }
}

/// The grid, in cells and in pixels. The pixel dimensions are what full-screen
/// programs use to size images and sixel output; they cost nothing to pass on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Grid {
    pub fn new(cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            cell_w,
            cell_h,
        }
    }
}

/// A running child on a pty of its own.
pub struct Pty {
    master: c_int,
    child: libc::pid_t,
    /// The child's end has hung up; stop writing to the pty.
    exited: bool,
    /// The child's exit status has actually been collected. Distinct from
    /// `exited`: a reap can give up before the child is reapable, and teardown
    /// must retry rather than leave a zombie for the life of the process.
    reaped: bool,
}

impl Pty {
    /// Open a pty, fork, and exec `cmd` on the far end of it.
    pub fn spawn(cmd: &Command, grid: Grid) -> io::Result<Self> {
        let master = open_master()?;
        let mut pty = Pty {
            master,
            child: -1,
            exited: false,
            reaped: true,
        };

        // Size the pty before anything is born onto it. A fresh pty is 0x0, and
        // a shell that reads that falls back to terminfo's 80 columns — which
        // it would use for its first prompt and only correct once a resize
        // arrives, one prompt too late to lay out.
        pty.resize(grid);

        // Everything the child needs is built here, in the parent: after the
        // fork the child may only make async-signal-safe calls, and allocating
        // a CString is not one of them.
        let slave = slave_path(master)?;
        let program = cstring(&cmd.program)?;
        let argv: Vec<CString> = cmd
            .argv
            .iter()
            .map(|a| cstring(a))
            .collect::<io::Result<_>>()?;
        let envp: Vec<CString> = cmd
            .env
            .iter()
            .map(|(k, v)| cstring(&format!("{k}={v}")))
            .collect::<io::Result<_>>()?;
        let cwd = match cmd.cwd.as_deref() {
            Some(dir) => Some(cstring(dir)?),
            None => None,
        };
        let argv_ptrs = null_terminated(&argv);
        let envp_ptrs = null_terminated(&envp);

        // SAFETY: fork() itself is safe to call; what follows it in the child
        // is the part that has to be async-signal-safe, and `child_exec` is
        // written to be exactly that. It never returns.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            let err = io::Error::last_os_error();
            pty.close_master();
            return Err(err);
        }
        if pid == 0 {
            // SAFETY: the child of a fork in a possibly-threaded process. Only
            // syscalls from here on, on memory laid out before the fork.
            unsafe {
                child_exec(
                    master,
                    &slave,
                    cwd.as_deref(),
                    &program,
                    argv_ptrs.as_ptr(),
                    envp_ptrs.as_ptr(),
                )
            }
        }
        pty.child = pid;
        pty.reaped = false;
        Ok(pty)
    }

    pub fn child_pid(&self) -> libc::pid_t {
        self.child
    }

    pub fn master_fd(&self) -> c_int {
        self.master
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// A second file descriptor for the same pty, for the reader thread. It
    /// holds its own so that closing the master here cannot pull a descriptor
    /// out from under a blocked `read` — and so that a reused fd number can
    /// never be read from by mistake.
    pub fn dup_master(&self) -> io::Result<c_int> {
        if self.master < 0 {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        // SAFETY: dup on a descriptor this struct owns.
        let fd = unsafe { libc::dup(self.master) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    /// Tell the pty its grid. Safe before the child exists (it is how the child
    /// learns its size to begin with) and after it is gone, where the ioctl
    /// simply fails on a dead descriptor.
    pub fn resize(&self, grid: Grid) {
        if self.master < 0 {
            return;
        }
        let size = libc::winsize {
            ws_row: grid.rows,
            ws_col: grid.cols,
            ws_xpixel: grid.cols.saturating_mul(grid.cell_w),
            ws_ypixel: grid.rows.saturating_mul(grid.cell_h),
        };
        // SAFETY: TIOCSWINSZ takes a `winsize` by pointer; the descriptor is
        // ours and the struct outlives the call.
        unsafe {
            libc::ioctl(self.master, libc::TIOCSWINSZ, &size);
        }
    }

    /// Everything the child has written, or `Ok(0)` at end of file.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if self.master < 0 {
            return Ok(0);
        }
        read_fd(self.master, buf)
    }

    /// Write to the child, retrying short writes. A pty whose child has gone is
    /// not an error worth surfacing: keystrokes into a dead shell are dropped,
    /// not reported.
    pub fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        if self.master < 0 || self.exited || data.is_empty() {
            return Ok(());
        }
        while !data.is_empty() {
            // SAFETY: writing `data.len()` bytes from a slice we hold.
            let written = unsafe {
                libc::write(
                    self.master,
                    data.as_ptr() as *const libc::c_void,
                    data.len(),
                )
            };
            if written < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            data = &data[written as usize..];
        }
        Ok(())
    }

    /// The child hung up on us. Stop writing, and collect it if it is already
    /// reapable — but do not escalate: a child that closed its end is on its
    /// way out, and a signal would only race its own exit.
    pub fn note_exit(&mut self) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.reap_within(EOF_GRACE);
    }

    /// Terminate the child and release the pty. Safe to call more than once: a
    /// pane closing calls it explicitly and `Drop` calls it again.
    pub fn shutdown(&mut self) {
        if self.child > 0 && !self.reaped {
            if self.exited {
                // It exited on its own but an earlier reap gave up before it
                // became reapable; finish collecting it so it cannot linger as
                // a zombie for the life of the process.
                self.reap_within(SIGNAL_GRACE);
            } else {
                self.escalate();
            }
        }
        self.exited = true;
        // Also on the EOF path: the child hanging up closed its end, not ours.
        self.close_master();
    }

    /// Walk [`TEARDOWN_SIGNALS`] until the child is collected.
    fn escalate(&mut self) -> Vec<c_int> {
        let child = self.child;
        let send = |signal: c_int| {
            // SAFETY: a pid we forked and have not reaped, so the number cannot
            // have been recycled onto somebody else's process.
            unsafe { libc::kill(child, signal) == 0 }
        };
        let sent = escalate_signals(send, || self.reap_within(SIGNAL_GRACE));
        // A `kill` that failed means the pid is already gone; make sure the
        // status is collected either way.
        if !self.reaped {
            self.reap_within(SIGNAL_GRACE);
        }
        sent
    }

    /// Poll for the child's exit status until `grace` runs out. A bare
    /// `waitpid(WNOHANG)` races the child: right after a hangup the process is
    /// dying but not yet reapable, so it returns 0 and the status is never
    /// collected. Bounded rather than blocking, so a child stuck in
    /// uninterruptible sleep cannot hang a pane that is closing.
    fn reap_within(&mut self, grace: Duration) -> bool {
        if self.reaped || self.child <= 0 {
            // A pid of -1 means the spawn never happened; waiting on it would
            // reap an unrelated child.
            return self.reaped;
        }
        let deadline = Instant::now() + grace;
        loop {
            let mut status: c_int = 0;
            // SAFETY: waitpid on our own child, non-blocking.
            let result = unsafe { libc::waitpid(self.child, &mut status, libc::WNOHANG) };
            if result == self.child {
                self.reaped = true;
                self.exited = true;
                return true;
            }
            if result < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                // ECHILD: somebody else collected it, or it never existed.
                self.reaped = true;
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Close the pty master exactly once. Teardown can run twice (explicitly,
    /// then again from `Drop`), and by the second pass the fd number may
    /// already have been handed to something else.
    fn close_master(&mut self) {
        if self.master < 0 {
            return;
        }
        let fd = std::mem::replace(&mut self.master, -1);
        // SAFETY: closing a descriptor this struct owns, exactly once.
        unsafe {
            libc::close(fd);
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The escalation itself, with the signalling and the waiting handed in: the
/// order is the part worth testing, and it should not need a stubborn child to
/// test it against. Returns the signals actually sent.
fn escalate_signals(
    mut send: impl FnMut(c_int) -> bool,
    mut reaped: impl FnMut() -> bool,
) -> Vec<c_int> {
    let mut sent = Vec::new();
    for signal in TEARDOWN_SIGNALS {
        if !send(signal) {
            break; // already gone
        }
        sent.push(signal);
        if reaped() {
            break;
        }
    }
    sent
}

// ---------------------------------------------------------------------------
// libc plumbing
// ---------------------------------------------------------------------------

fn cstring(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "argument contains an interior NUL",
        )
    })
}

fn null_terminated(values: &[CString]) -> Vec<*const c_char> {
    let mut ptrs: Vec<*const c_char> = values.iter().map(|v| v.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    ptrs
}

/// A pty master, unlocked and ready for its slave to be opened.
fn open_master() -> io::Result<c_int> {
    // SAFETY: the three calls a pty master needs, in the order POSIX requires.
    // O_NOCTTY on the master matters: this process is not the one adopting the
    // terminal, the child is.
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::grantpt(fd) < 0 || libc::unlockpt(fd) < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        Ok(fd)
    }
}

/// The device path of the master's slave, which the child opens by name.
fn slave_path(master: c_int) -> io::Result<CString> {
    let mut buf = [0 as c_char; 128];
    // SAFETY: ptsname_r writes a NUL-terminated name into our buffer, bounded
    // by the length we pass. The reentrant form is required: this may run while
    // other threads are alive, and plain ptsname() returns a shared static.
    let result = unsafe { libc::ptsname_r(master, buf.as_mut_ptr(), buf.len()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above NUL-terminated the buffer.
    let name = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Ok(name.to_owned())
}

fn read_fd(fd: c_int, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: reading at most `buf.len()` bytes into a slice we hold.
        let count = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if count >= 0 {
            return Ok(count as usize);
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            // A pty master whose slave has been closed reports EIO rather than
            // end of file. For a terminal that *is* end of file.
            Some(libc::EIO) => return Ok(0),
            _ => return Err(err),
        }
    }
}

/// The child side of the fork: give ourselves a session, adopt the pty as our
/// controlling terminal, and exec.
///
/// Every call below is async-signal-safe, and nothing here allocates or takes a
/// lock — the fork may have happened while another thread held one, and that
/// lock will never be released in this process.
///
/// # Safety
///
/// Must only be called in the child of a `fork`, with pointers that outlive the
/// call. Never returns.
unsafe fn child_exec(
    master: c_int,
    slave: &CStr,
    cwd: Option<&CStr>,
    program: &CStr,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> ! {
    // A session of our own. Without it the shell would share Kraken's process
    // group, and its job control would fight the app's.
    if libc::setsid() < 0 {
        libc::_exit(127);
    }
    // Deliberately without O_NOCTTY: Linux hands a session leader the tty it
    // opens that way.
    let fd = libc::open(slave.as_ptr(), libc::O_RDWR);
    if fd < 0 {
        libc::_exit(127);
    }
    // BSD only ever grants a controlling terminal on an explicit TIOCSCTTY; on
    // Linux the open above already did it and this is a no-op that succeeds.
    // Without a controlling terminal the shell still reads and writes fd 0 as
    // if nothing were wrong, but the pty has no foreground process group, so
    // the kernel has nobody to send SIGWINCH (or SIGINT) to: the shell keeps
    // whatever $COLUMNS it started with and Ctrl-C interrupts nothing.
    libc::ioctl(fd, libc::TIOCSCTTY, 0);
    if libc::dup2(fd, 0) < 0 || libc::dup2(fd, 1) < 0 || libc::dup2(fd, 2) < 0 {
        libc::_exit(127);
    }
    if fd > 2 {
        libc::close(fd);
    }
    // The master is Kraken's end. A child holding it open would keep the pty
    // alive after its own exit, so the reader would never see the hangup that
    // tells the app the shell is gone.
    libc::close(master);
    if let Some(dir) = cwd {
        if libc::chdir(dir.as_ptr()) < 0 {
            libc::_exit(127);
        }
    }
    // Rust ignores SIGPIPE process-wide at startup, and exec does not reset an
    // *ignored* disposition. Left alone, every program the user pipes in this
    // shell would survive a closed pipe and read EPIPE instead of dying — `yes
    // | head` would never end.
    libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    let mut mask: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut mask);
    libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());

    libc::execve(program.as_ptr(), argv, envp);
    // exec only returns on failure, and there is nothing left to report to.
    libc::_exit(127)
}

/// The first executable named `name` on PATH.
pub fn which(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_env() -> Vec<(String, String)> {
        [
            ("VIRTUAL_ENV", "/somewhere/kraken/.venv"),
            ("PYTHONPATH", "/somewhere/kraken"),
            ("PYTHONHOME", "/somewhere/runtime"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_kraken/usr/lib"),
            ("APPDIR", "/tmp/.mount_kraken"),
            ("PATH", "/somewhere/kraken/.venv/bin:/opt/homebrew/bin:/usr/bin:/bin"),
            ("EDITOR", "vim"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn the_shell_is_not_told_about_krakens_own_python() {
        let env = scrub_env(app_env());
        for name in OWN_RUNTIME_VARS {
            assert!(!env.contains_key(name), "{name} followed the user's shell");
        }
    }

    #[test]
    fn the_virtualenvs_bin_leaves_path_too() {
        let env = scrub_env(app_env());
        let parts: Vec<&str> = env["PATH"].split(':').collect();
        assert!(!parts.contains(&"/somewhere/kraken/.venv/bin"));
    }

    #[test]
    fn the_rest_of_path_is_left_alone() {
        let env = scrub_env(app_env());
        assert_eq!(env["PATH"], "/opt/homebrew/bin:/usr/bin:/bin");
    }

    #[test]
    fn what_the_launcher_injected_does_not_follow_the_user() {
        let env = scrub_env(app_env());
        assert!(!env.contains_key("LD_LIBRARY_PATH"));
        assert!(!env.contains_key("APPDIR"));
    }

    #[test]
    fn everything_else_survives() {
        assert_eq!(scrub_env(app_env())["EDITOR"], "vim");
    }

    #[test]
    fn an_app_outside_a_virtualenv_keeps_its_path() {
        let vars = [("PATH", "/usr/bin:/bin"), ("PYTHONPATH", "/opt/kraken/share")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()));
        let env = scrub_env(vars);
        assert_eq!(env["PATH"], "/usr/bin:/bin");
        // PYTHONPATH still goes: in a bundle it names Kraken's own sources, which
        // a `python` in the user's project has no business importing.
        assert!(!env.contains_key("PYTHONPATH"));
    }

    #[test]
    fn the_term_advertised_is_one_the_child_can_look_up() {
        let term = pick_term();
        assert!(term == PREFERRED_TERM || term == "xterm");
    }

    #[test]
    fn teardown_escalates_from_hangup_to_kill() {
        let mut sent_to = Vec::new();
        // A child that ignores everything until it is killed.
        let mut waits = 0;
        let sent = escalate_signals(
            |signal| {
                sent_to.push(signal);
                true
            },
            || {
                waits += 1;
                waits >= 3
            },
        );
        assert_eq!(sent, vec![libc::SIGHUP, libc::SIGTERM, libc::SIGKILL]);
        assert_eq!(sent_to, sent);
    }

    #[test]
    fn a_shell_that_takes_the_hangup_is_never_escalated() {
        let sent = escalate_signals(|_| true, || true);
        assert_eq!(sent, vec![libc::SIGHUP]);
    }

    #[test]
    fn a_child_that_is_already_gone_is_not_signalled_twice() {
        let sent = escalate_signals(|_| false, || false);
        assert!(sent.is_empty());
    }

    #[test]
    fn a_spawned_child_owns_the_pty_as_its_controlling_terminal() {
        let mut cmd = Command::local_shell(None);
        // Never an interactive shell in a test: `cat` reads the pty, keeps it
        // open, and exits the moment it is signalled.
        cmd.program = "/bin/cat".to_string();
        cmd.argv = vec!["/bin/cat".to_string()];
        let mut pty = Pty::spawn(&cmd, Grid::new(80, 24, 8, 16)).expect("spawn");

        // The pty's foreground process group is the child's own session, which
        // is only true if it claimed the terminal. The claim happens in the
        // forked child, so the parent can reach here first and read a group of
        // 0 — poll rather than asserting into the race.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut pgrp: libc::pid_t = 0;
        while std::time::Instant::now() < deadline {
            // SAFETY: TIOCGPGRP writes one pid_t through the pointer.
            let ok = unsafe { libc::ioctl(pty.master_fd(), libc::TIOCGPGRP, &mut pgrp) };
            if ok == 0 && pgrp == pty.child_pid() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(pgrp, pty.child_pid(), "the child never claimed the pty");

        pty.shutdown();
    }

    #[test]
    fn the_child_starts_at_the_grid_size_it_was_given() {
        let mut cmd = Command::local_shell(None);
        cmd.program = "/bin/cat".to_string();
        cmd.argv = vec!["/bin/cat".to_string()];
        let mut pty = Pty::spawn(&cmd, Grid::new(97, 31, 8, 16)).expect("spawn");

        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGWINSZ fills the struct we pass.
        unsafe {
            libc::ioctl(pty.master_fd(), libc::TIOCGWINSZ, &mut size);
        }
        assert_eq!((size.ws_col, size.ws_row), (97, 31));
        assert_eq!(size.ws_xpixel, 97 * 8);

        pty.shutdown();
    }

    #[test]
    fn a_pty_round_trips_bytes_through_its_child() {
        let mut cmd = Command::local_shell(None);
        cmd.program = "/bin/cat".to_string();
        cmd.argv = vec!["/bin/cat".to_string()];
        let mut pty = Pty::spawn(&cmd, Grid::new(80, 24, 8, 16)).expect("spawn");

        pty.write(b"kraken\n").expect("write");
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !seen.contains("kraken") && Instant::now() < deadline {
            let mut buf = [0u8; 256];
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => break,
            }
        }
        assert!(seen.contains("kraken"), "child never echoed; saw {seen:?}");

        pty.shutdown();
    }

    #[test]
    fn teardown_reaps_the_child_and_closes_the_master_once() {
        let mut cmd = Command::local_shell(None);
        cmd.program = "/bin/cat".to_string();
        cmd.argv = vec!["/bin/cat".to_string()];
        let mut pty = Pty::spawn(&cmd, Grid::new(80, 24, 8, 16)).expect("spawn");
        let pid = pty.child_pid();

        pty.shutdown();

        assert_eq!(pty.master_fd(), -1, "the pty master is closed");
        // SAFETY: signal 0 only probes for the pid's existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the shell was left behind");

        // A second teardown must not close whatever fd number the first one
        // released, which by now may belong to somebody else.
        // SAFETY: opening and closing a descriptor of our own.
        let reused = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
        pty.shutdown();
        let mut probe: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fstat on the descriptor just opened; fails if it was closed.
        let still_open = unsafe { libc::fstat(reused, &mut probe) } == 0;
        unsafe { libc::close(reused) };
        assert!(still_open, "teardown closed someone else's descriptor");
    }

    #[test]
    fn a_child_that_ignores_the_hangup_is_killed_anyway() {
        let mut cmd = Command::local_shell(None);
        cmd.program = "/bin/sh".to_string();
        cmd.argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "trap '' HUP TERM; while :; do sleep 0.05; done".to_string(),
        ];
        let mut pty = Pty::spawn(&cmd, Grid::new(80, 24, 8, 16)).expect("spawn");
        let pid = pty.child_pid();
        // Let the shell install its traps before the hangup arrives.
        std::thread::sleep(Duration::from_millis(200));

        pty.shutdown();

        // SAFETY: signal 0 only probes for the pid's existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "a signal-ignoring shell survived teardown");
    }

    #[test]
    fn a_remote_terminal_execs_ssh_with_the_workspace_command() {
        use crate::remote::{RemoteTarget, SshHost};

        let target = RemoteTarget {
            host: SshHost {
                host_id: "pi5".into(),
                hostname: "purplenode.local".into(),
                user: "pascal".into(),
                port: 22,
                identity: None,
            },
            path: "/home/pascal/Workspace/app".into(),
        };
        let cmd = Command::remote_shell(&target);
        assert!(cmd.program.ends_with("ssh"));
        assert_eq!(cmd.argv[0], "ssh");
        assert_eq!(cmd.argv[1], "-t");
        assert!(cmd.argv.last().expect("remote command").contains("exec"));
        assert_eq!(cmd.env["TERM"], "xterm-256color");
        assert!(cmd.cwd.is_none());
    }
}
