//! Finding the `pi` the user installed, when Kraken was not started from a shell.
//!
//! pi is an npm package, so it usually lives somewhere only a shell's rc files
//! put on PATH — nvm's `~/.nvm/versions/node/<v>/bin`, a Homebrew prefix,
//! `~/.local/bin`. Started from a terminal, Kraken inherits that PATH and a bare
//! `pi` resolves. Started from a desktop launcher or an AppImage, it inherits the
//! session's bare PATH instead, and the agent never comes up.
//!
//! So when `pi` is not on the PATH Kraken was given, the user's login shell is
//! asked for its own, once, at startup, and whatever it adds is appended. The
//! process-wide PATH is what changes, so the agent, the model catalogue, the
//! commit drafts and the terminals all see the same one.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the login shell gets. nvm alone costs a few hundred milliseconds;
/// a shell that has not answered in this long is stuck on something, and a
/// window that has not opened is worse than an agent that is not found.
const SHELL_DEADLINE: Duration = Duration::from_secs(5);

/// Printed around the shell's environment, so whatever the rc files print on
/// their way in — a greeting, a fortune, an nvm notice — is not read as PATH.
const BEGIN: &str = "__KRAKEN_ENV_BEGIN__";
const END: &str = "__KRAKEN_ENV_END__";

/// The executable `program` resolves to on `path`, the way `execvp` would find
/// it: the first directory holding an executable file of that name.
pub fn find_on_path(program: &str, path: &str) -> Option<PathBuf> {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// `current`, followed by every entry of `login` it does not already have.
///
/// Appended rather than prepended: the PATH Kraken was started with is the one
/// the user chose for it, and the shell's is only there to fill in what is
/// missing.
pub fn merge_paths(current: &str, login: &str) -> String {
    let mut entries: Vec<&str> = current.split(':').filter(|e| !e.is_empty()).collect();
    for entry in login.split(':').filter(|e| !e.is_empty()) {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries.join(":")
}

/// The `PATH=` line between the markers of the shell's output.
pub fn parse_env_output(output: &str) -> Option<String> {
    let body = output.split_once(BEGIN)?.1.split_once(END)?.0;
    body.lines()
        .find_map(|line| line.strip_prefix("PATH="))
        .map(str::to_string)
        .filter(|path| !path.is_empty())
}

/// The PATH the user's login shell sets up, or `None` if it could not be had.
///
/// Interactive as well as login: nvm's installer writes to `.bashrc`, which a
/// login bash only reads through `.bash_profile`, and which most distributions
/// guard with an early return for non-interactive shells. `env` rather than
/// `echo $PATH` because fish prints its PATH space-separated.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;
    let script = format!("echo {BEGIN}; env; echo {END}");
    let mut command = Command::new(&shell);
    command
        .args(["-i", "-l", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A session of its own, so an interactive shell never reaches for the
    // terminal Kraken may have been started from.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        output
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < SHELL_DEADLINE => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    parse_env_output(&reader.join().ok()?)
}

/// Make `pi` findable if the login shell knows where it is.
///
/// Must run before any other thread starts: it changes the process
/// environment. Returns what happened, for the caller to log once logging is
/// up — `None` when there was nothing to do.
pub fn adopt_login_path() -> Option<String> {
    let current = std::env::var("PATH").unwrap_or_default();
    if find_on_path("pi", &current).is_some() {
        return None;
    }
    let Some(login) = login_shell_path() else {
        return Some("pi not on PATH, and the login shell gave no PATH".to_string());
    };
    let merged = merge_paths(&current, &login);
    std::env::set_var("PATH", &merged);
    Some(match find_on_path("pi", &merged) {
        Some(pi) => format!("pi found through the login shell at {}", pi.display()),
        None => "pi is on neither PATH nor the login shell's".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_shell_only_fills_in_what_is_missing() {
        assert_eq!(
            merge_paths("/usr/bin:/bin", "/home/u/.nvm/bin:/usr/bin:/home/u/.local/bin"),
            "/usr/bin:/bin:/home/u/.nvm/bin:/home/u/.local/bin"
        );
        assert_eq!(merge_paths("", "/a:/b"), "/a:/b");
    }

    #[test]
    fn whatever_the_rc_files_print_is_not_read_as_path() {
        let output = format!(
            "Welcome!\nPATH=/not/this\n{BEGIN}\nHOME=/home/u\nPATH=/home/u/.nvm/bin:/usr/bin\n{END}\n"
        );
        assert_eq!(parse_env_output(&output).as_deref(), Some("/home/u/.nvm/bin:/usr/bin"));
        assert_eq!(parse_env_output("PATH=/usr/bin\n"), None);
    }

    #[test]
    fn only_an_executable_file_is_found() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("kraken-login-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("pi");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        let path = format!("/nonexistent:{}", dir.display());
        assert_eq!(find_on_path("pi", &path), None, "not executable yet");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_on_path("pi", &path), Some(program));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
