//! Persistent app state in `~/.kraken/`: preferences and project tracking.
//!
//! State is one JSON object; [`save`] merges partial updates so independent
//! features can persist their keys without clobbering each other's. Every read
//! is tolerant — a missing, unreadable or corrupt file reads as an empty object
//! rather than stopping a window that has not been built yet.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde_json::{Map, Value};

/// Overridden by tests (and by `KRAKEN_HOME`) so state work never touches the
/// developer's own `~/.kraken`.
static ROOT: Lazy<Mutex<PathBuf>> = Lazy::new(|| Mutex::new(default_root()));

fn default_root() -> PathBuf {
    if let Ok(path) = std::env::var("KRAKEN_HOME") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kraken")
}

/// The configuration directory. Everything the app writes lives under it:
/// `state.json`, the speech model cache, SSH control sockets, remote anchors,
/// the unpacked pi extension, and debug logs.
pub fn config_dir() -> PathBuf {
    ROOT.lock().expect("state root").clone()
}

/// Point the whole module at another directory. For tests, and for `KRAKEN_HOME`.
pub fn set_config_dir(path: impl AsRef<Path>) {
    *ROOT.lock().expect("state root") = path.as_ref().to_path_buf();
}

pub fn state_path() -> PathBuf {
    config_dir().join("state.json")
}

pub fn models_dir() -> PathBuf {
    config_dir().join("models")
}

pub fn remotes_dir() -> PathBuf {
    config_dir().join("remotes")
}

pub fn ssh_control_dir() -> PathBuf {
    config_dir().join("ssh")
}

pub fn extension_dir() -> PathBuf {
    config_dir().join("ext")
}

pub fn logs_dir() -> PathBuf {
    config_dir().join("logs")
}

/// The whole state object.
pub fn load() -> Map<String, Value> {
    match fs::read_to_string(state_path()) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

/// Merge `changes` into the stored state, leaving every other key alone.
pub fn save(changes: impl IntoIterator<Item = (String, Value)>) {
    let mut state = load();
    for (key, value) in changes {
        state.insert(key, value);
    }
    let dir = config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut text) = serde_json::to_string_pretty(&Value::Object(state)) {
        text.push('\n');
        let _ = fs::write(state_path(), text);
    }
}

/// Store one key.
pub fn set(key: &str, value: Value) {
    save([(key.to_string(), value)]);
}

pub fn get(key: &str) -> Option<Value> {
    load().get(key).cloned()
}

pub fn string(key: &str) -> Option<String> {
    match get(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

pub fn integer(key: &str) -> Option<i64> {
    get(key).and_then(|value| value.as_i64())
}

pub fn bool_flag(key: &str) -> bool {
    matches!(get(key), Some(Value::Bool(true)))
}

/// A list-of-strings key, with anything that is not a string dropped.
pub fn string_list(key: &str) -> Vec<String> {
    match get(key) {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An object-valued key, or an empty map.
pub fn object(key: &str) -> Map<String, Value> {
    match get(key) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// State is process-global, so the tests that write it take turns.
    static GUARD: Mutex<()> = Mutex::new(());

    struct Scratch {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("kraken-state-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            set_config_dir(&dir);
            Self { dir, _lock: lock }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
            set_config_dir(default_root());
        }
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        let _scratch = Scratch::new("missing");
        assert!(load().is_empty());
        assert_eq!(string("current_workspace"), None);
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_rather_than_failing() {
        let scratch = Scratch::new("corrupt");
        fs::write(scratch.dir.join("state.json"), "{ not json at all").unwrap();
        assert!(load().is_empty());
        // A JSON document that is not an object is equally unusable.
        fs::write(scratch.dir.join("state.json"), "[1, 2, 3]").unwrap();
        assert!(load().is_empty());
    }

    #[test]
    fn saving_one_key_leaves_the_others_alone() {
        let _scratch = Scratch::new("merge");
        set("chat_font_size", json!(17));
        set("workspaces", json!(["/a", "/b"]));
        set("chat_font_size", json!(19));
        assert_eq!(integer("chat_font_size"), Some(19));
        assert_eq!(string_list("workspaces"), vec!["/a", "/b"]);
    }

    #[test]
    fn typed_readers_ignore_values_of_the_wrong_shape() {
        let _scratch = Scratch::new("typed");
        set("workspaces", json!(["/a", 7, null, "/b"]));
        set("chat_font_size", json!("thirteen"));
        set("remotes", json!("not an object"));
        assert_eq!(string_list("workspaces"), vec!["/a", "/b"]);
        assert_eq!(integer("chat_font_size"), None);
        assert!(object("remotes").is_empty());
    }

    #[test]
    fn the_file_round_trips_as_pretty_json_with_a_trailing_newline() {
        let scratch = Scratch::new("format");
        set("current_workspace", json!("/home/pascal/Workspace/kraken"));
        let text = fs::read_to_string(scratch.dir.join("state.json")).unwrap();
        assert!(text.ends_with("}\n"), "state file should end with a newline");
        assert!(text.contains("\n  \"current_workspace\""));
    }
}
