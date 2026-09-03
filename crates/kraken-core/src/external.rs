//! Opening the workspace folder in another application.
//!
//! Three targets, named the same way in the menu and in the code so the visible
//! order and the launch behaviour stay in one small list. Only the argv is here;
//! spawning it is the bridge's, and what the menu offers is what this file says
//! it does.
//!
//! A remote workspace has nothing to open: its anchor is a local stand-in folder
//! with none of the project in it, and handing that to an editor would open an
//! empty directory that looks like the project and is not.

use std::path::Path;

/// The applications the menu offers, in the order it offers them.
pub const APPS: [&str; 3] = ["Ghostty", "Terminal", "Zed"];

/// The command that opens `path` in `app`, or `None` for a name that is not one
/// of [`APPS`].
///
/// macOS goes through `open` so this works from a bundled build, whose PATH does
/// not necessarily hold an app's optional CLI helper. Ghostty is the exception
/// to the usual folder-as-a-document convention: its own CLI directs macOS
/// callers to pass configuration through `open --args`.
///
/// The Linux forms keep the same menu useful in a development build, and the
/// generic terminal is built by [`terminal_argv`] because what it stands for
/// decides whether the folder has to be named.
pub fn app_argv(app: &str, path: &str) -> Option<Vec<String>> {
    let argv: Vec<&str> = if cfg!(target_os = "macos") {
        match app {
            "Ghostty" => vec!["/usr/bin/open", "-na", "Ghostty.app", "--args"],
            "Terminal" | "Zed" => vec!["/usr/bin/open", "-a", app, path],
            _ => return None,
        }
    } else {
        match app {
            "Ghostty" => vec!["ghostty"],
            "Terminal" => return Some(terminal_argv(path)),
            "Zed" => vec!["zed", path],
            _ => return None,
        }
    };
    let mut argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
    if app == "Ghostty" {
        argv.push(format!("--working-directory={path}"));
    }
    Some(argv)
}

/// The name a Debian-style system points at whichever terminal is installed.
const GENERIC_TERMINAL: &str = "x-terminal-emulator";

/// The argv that opens the generic terminal on `path`.
///
/// A terminal that puts up its own window inherits `path` from the spawn and
/// needs no flag. The GTK ones hand the request to an already-running instance
/// of themselves, which opens the window from wherever that instance was
/// started — the home folder, in the usual case of one started from the
/// desktop — and the spawn's working directory is thrown away with the process
/// that carried it. Those have to be told the folder outright, which they all
/// spell the same way; Ptyxis reads the flag only once it knows it is opening a
/// window.
fn terminal_argv(path: &str) -> Vec<String> {
    let mut argv = vec![GENERIC_TERMINAL.to_string()];
    match terminal_behind_the_generic_name().as_deref() {
        Some("ptyxis") => argv.push("--new-window".to_string()),
        Some("gnome-terminal" | "kgx" | "tilix") => {}
        // An unknown terminal is left to inherit: a flag it does not take is
        // worse than one it did not need, since it would fail to open at all.
        _ => return argv,
    }
    argv.push(format!("--working-directory={path}"));
    argv
}

/// The file name of the program [`GENERIC_TERMINAL`] resolves to, following the
/// alternatives symlinks to whatever is really there.
fn terminal_behind_the_generic_name() -> Option<String> {
    let found = crate::terminal::pty::which(GENERIC_TERMINAL)?;
    let real = std::fs::canonicalize(found).ok()?;
    Some(real.file_name()?.to_string_lossy().into_owned())
}

/// Whether a folder is one another application could be pointed at.
pub fn openable(path: &str) -> bool {
    !path.is_empty() && Path::new(path).is_dir() && crate::remote::resolve(path).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_app_builds_a_command() {
        for app in APPS {
            let argv = app_argv(app, "/tmp/project").expect("an argv");
            assert!(!argv.is_empty());
            // Every form carries the folder somewhere, whether as an argument
            // or as a flag. The generic terminal is the one exception, and only
            // where it turns out to be one that inherits the folder instead.
            assert!(
                argv.iter().any(|part| part.contains("/tmp/project"))
                    || (app == "Terminal" && !cfg!(target_os = "macos")),
                "{app} loses the path"
            );
        }
    }

    #[test]
    fn an_unknown_application_builds_nothing() {
        assert!(app_argv("Emacs", "/tmp/project").is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ghostty_takes_the_folder_as_a_flag_rather_than_an_argument() {
        assert_eq!(
            app_argv("Ghostty", "/tmp/p").unwrap(),
            vec!["ghostty", "--working-directory=/tmp/p"]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_terminal_that_opens_windows_from_elsewhere_is_told_the_folder() {
        let argv = terminal_argv("/tmp/p");
        // Whether the flag is there depends on what is installed, so what is
        // checked is the pairing: named folder, or nothing at all.
        match terminal_behind_the_generic_name().as_deref() {
            Some("ptyxis") => assert_eq!(
                argv,
                vec![
                    "x-terminal-emulator",
                    "--new-window",
                    "--working-directory=/tmp/p"
                ]
            ),
            Some("gnome-terminal" | "kgx" | "tilix") => assert_eq!(
                argv,
                vec!["x-terminal-emulator", "--working-directory=/tmp/p"]
            ),
            _ => assert_eq!(argv, vec!["x-terminal-emulator"]),
        }
    }

    #[test]
    fn a_path_that_is_not_a_folder_is_not_openable() {
        assert!(!openable(""));
        assert!(!openable("/definitely/not/here"));
        assert!(openable("/tmp"));
    }
}
