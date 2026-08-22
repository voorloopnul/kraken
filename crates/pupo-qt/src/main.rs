//! Pupo — a Qt/QML desktop front end for the Pi coding agent.

mod bridge;
mod resources;

use std::cell::RefCell;
use std::rc::Rc;

use qmetaobject::*;

use bridge::app::App;
use bridge::browser::BrowserBridge;
use bridge::diff::DiffBridge;
use bridge::dock::DockModel;
use bridge::files::FilesBridge;
use bridge::git::GitBridge;
use bridge::history::HistoryBridge;
use bridge::remotes::RemotesBridge;
use bridge::session::SessionBridge;
use bridge::settings::SettingsBridge;
use bridge::terminal::TerminalBridge;
use bridge::theme::Theme;
use pupo_core::theme::DEFAULT_THEME;
use pupo_core::typography::{chat, terminal};
use pupo_core::{debug, state};

/// The ssh routing extension pi is launched with for a remote workspace.
///
/// Compiled in like the rest of `assets/`, but it cannot stay there: pi takes
/// `-e <path>` and opens it itself, so a resource inside our binary is no use
/// to it. It has to exist as a file, which is what `install_ssh_extension`
/// below is for.
const SSH_EXTENSION: &str = include_str!("../assets/agent/ssh_remote.ts");

/// Write the ssh routing extension out where pi can load it.
///
/// Without this every remote workspace fails at the first launch — pi refuses
/// to start at all when `-e` names a path that is not there, so the agent never
/// comes up, and the symptom is an empty model selector rather than anything
/// naming a missing file.
///
/// Rewritten whenever the contents differ, so an upgrade replaces the copy a
/// previous version left behind.
fn install_ssh_extension() {
    write_ssh_extension(&pupo_core::pi::rpc::ssh_extension_path());
}

/// The half of the above that knows nothing about where the file goes, so a
/// test can point it somewhere harmless.
fn write_ssh_extension(path: &std::path::Path) {
    let Some(folder) = path.parent() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(folder) {
        debug::error(
            "pi.ssh-extension-dir-failed",
            &[("path", folder.display().to_string()), ("error", error.to_string())],
        );
        return;
    }
    if std::fs::read_to_string(path).is_ok_and(|current| current == SSH_EXTENSION) {
        return;
    }
    if let Err(error) = std::fs::write(path, SSH_EXTENSION) {
        debug::error(
            "pi.ssh-extension-write-failed",
            &[("path", path.display().to_string()), ("error", error.to_string())],
        );
    }
}

/// Everything QML can reach, by the name it reaches it under. Registering them
/// all here rather than as each panel is built keeps `Main.qml` free of
/// conditionals about which halves of the app happen to exist.
macro_rules! publish {
    ($engine:expr, $($name:literal => $object:expr),+ $(,)?) => {{
        $(
            let boxed = Rc::new(RefCell::new(QObjectBox::new($object)));
            $engine.set_object_property($name.into(), boxed.borrow().pinned());
            // Held for the life of `main`: a context property is not an owning
            // reference, and an object collected out from under QML takes the
            // window with it.
            std::mem::forget(boxed);
        )+
    }};
}

fn main() {
    let settings = parse_args();
    if let Some(settings) = settings {
        if let Some(path) = debug::start(&settings) {
            eprintln!("pupo: debug log -> {}", path.display());
        }
    }

    resources::register_resources();
    install_ssh_extension();

    // The sizes and the theme are a reader's accommodation: ones that reset on
    // every launch would have to be set again every time, so all three are
    // restored here and the whole tree is built at them.
    let theme_name = state::string("theme").unwrap_or_else(|| DEFAULT_THEME.to_string());
    let chat_size = chat::clamp_opt(state::integer("chat_font_size"));
    let terminal_size = terminal::clamp_opt(state::integer("terminal_font_size"));

    // The History pane opens on whatever workspace was current last time.
    let mut history = HistoryBridge::new();
    if let Some(current) = state::string("current_workspace") {
        history.set_cwd(&current);
    }

    let mut engine = QmlEngine::new();
    publish!(
        engine,
        "Theme" => Theme::new(&theme_name, chat_size, terminal_size),
        "App" => App::new(),
        "DockModel" => DockModel::new(),
        "History" => history,
        "Remotes" => RemotesBridge::new(),
        "Session" => SessionBridge::new(),
        "TerminalTabs" => TerminalBridge::new(),
        "Browser" => BrowserBridge::new(),
        "Diff" => DiffBridge::new(),
        "Git" => GitBridge::new(),
        "Files" => FilesBridge::new(),
        "Settings" => SettingsBridge::new(),
    );

    debug::log("app.started", &[]);
    engine.load_file("qrc:/qml/Main.qml".into());
    engine.exec();
    debug::shutdown(Some(0));
}

/// `--debug [FILE]`, `--debug-trace` and `--debug-heartbeat SECONDS`, with the
/// environment as a fallback for a launcher where passing argv is awkward. The
/// flag wins where both are given. Anything else is left alone: Qt reads its
/// own arguments straight out of the command line.
fn parse_args() -> Option<debug::Settings> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut settings: Option<debug::Settings> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let value = |at: usize| args.get(at + 1).filter(|v| !v.starts_with('-')).cloned();
        match arg {
            "--debug" => {
                let path = value(index);
                if path.is_some() {
                    index += 1;
                }
                settings.get_or_insert_with(debug::Settings::default).path = path;
            }
            "--debug-trace" => {
                settings.get_or_insert_with(debug::Settings::default).trace_input = true;
            }
            "--debug-heartbeat" => {
                if let Some(seconds) = value(index).and_then(|v| v.parse().ok()) {
                    index += 1;
                    settings.get_or_insert_with(debug::Settings::default).heartbeat = seconds;
                }
            }
            _ => {}
        }
        index += 1;
    }
    settings.or_else(debug::from_environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pupo-ext-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// pi will not start at all when `-e` names a path that is not there, and
    /// what the user sees when it does not start is an empty model selector —
    /// nothing that mentions a file. So the one thing worth pinning is that the
    /// extension actually reaches the disk.
    #[test]
    fn the_ssh_extension_is_written_where_pi_is_told_to_look() {
        let dir = scratch("written");
        let path = dir.join("ssh_remote.ts");
        write_ssh_extension(&path);
        let written = std::fs::read_to_string(&path).expect("extension written");
        assert_eq!(written, SSH_EXTENSION);
        assert!(!written.trim().is_empty(), "the bundled extension is empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The launcher sets one environment variable and the extension reads
    /// another, and they live in different crates in different languages, so
    /// nothing but this notices when they stop matching. When they did, the
    /// extension registered no tools at all and pi ran every command on the
    /// local machine while claiming to be on the remote one.
    #[test]
    fn the_extension_reads_the_variable_the_launcher_sets() {
        let reads = format!("process.env.{}", pupo_core::pi::rpc::SSH_ENV);
        assert!(
            SSH_EXTENSION.contains(&reads),
            "the bundled extension never reads {}",
            pupo_core::pi::rpc::SSH_ENV
        );
    }

    /// An upgrade has to replace the copy the previous version left behind.
    #[test]
    fn a_stale_extension_is_replaced() {
        let dir = scratch("stale");
        let path = dir.join("ssh_remote.ts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "// left over from an older build\n").unwrap();
        write_ssh_extension(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SSH_EXTENSION);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
