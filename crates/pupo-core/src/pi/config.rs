//! Read and write pi's own configuration files.
//!
//! Pupo drives pi through the RPC protocol, but pi's credentials and custom
//! providers live in files the RPC surface does not expose — there is no
//! `login` command to send. This module is the boundary that touches them,
//! following the contracts in pi's `docs/providers.md` and `docs/models.md`:
//!
//! * `~/.pi/agent/auth.json` — one entry per provider, `{"type": "api_key",
//!   "key": ...}` or an OAuth record pi writes and refreshes itself. Created
//!   `0600`, and it takes priority over environment variables.
//! * `~/.pi/agent/models.json` — custom providers (Ollama, llama.cpp, LM
//!   Studio, vLLM) under `providers`, each with a `baseUrl`, an `api`, and its
//!   models.
//! * `~/.pi/agent/settings.json` — pi's own settings, of which exactly one key
//!   is touched here: `enabledModels`, the model scope (`docs/settings.md`).
//!
//! These files belong to pi, not to us: every write merges into what is already
//! there and leaves keys we do not understand untouched. `PI_CODING_AGENT_DIR`
//! relocates the whole directory, so it is honoured here exactly as pi honours
//! it, and [`set_agent_dir`] does the same for tests.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde_json::{json, Map, Value};

/// pi's own override for its config directory (its `config.js: ENV_AGENT_DIR`).
const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";
/// Pupo's override for the `~/.pi` root, which covers the sessions directory
/// too — one variable that keeps a test (or a second install) off the
/// developer's real agent.
const ENV_PI_HOME: &str = "PUPO_PI_HOME";

/// What a local OpenAI-compatible server (llama.cpp, Ollama, LM Studio, vLLM)
/// is declared as. pi treats every model as needing auth before it shows up in
/// the picker, so a keyless local server still needs a placeholder key.
pub const LOCAL_API: &str = "openai-completions";
pub const LOCAL_PLACEHOLDER_KEY: &str = "local";
pub const LOCAL_PROVIDER_ID: &str = "locallm";
pub const LOCAL_BASE_URL: &str = "http://localhost:8080/v1";

/// Providers the settings UI knows how to configure, and the environment
/// variable pi accepts for each in place of an auth.json entry.
pub const OPENROUTER: &str = "openrouter";
pub const OPENROUTER_ENV: &str = "OPENROUTER_API_KEY";
pub const CODEX: &str = "openai-codex";

/// The scope key in settings.json.
pub const ENABLED_MODELS: &str = "enabledModels";

/// Set by [`set_agent_dir`]; overrides both environment variables.
static OVERRIDE: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

/// pi's config directory: an explicit override, else `$PI_CODING_AGENT_DIR`,
/// else `$PUPO_PI_HOME/agent`, else `~/.pi/agent`.
pub fn agent_dir() -> PathBuf {
    if let Some(path) = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return path;
    }
    if let Some(path) = env_path(ENV_AGENT_DIR) {
        return path;
    }
    if let Some(path) = env_path(ENV_PI_HOME) {
        return path.join("agent");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi")
        .join("agent")
}

/// Point the module at another agent directory — for tests, so nothing here
/// can reach the real `~/.pi`. `None` restores the environment's answer.
pub fn set_agent_dir(path: Option<&Path>) {
    *OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = path.map(Path::to_path_buf);
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn auth_path() -> PathBuf {
    agent_dir().join("auth.json")
}

pub fn models_path() -> PathBuf {
    agent_dir().join("models.json")
}

pub fn settings_path() -> PathBuf {
    agent_dir().join("settings.json")
}

/// The file's contents, or an empty object when it is missing or corrupt.
///
/// A broken file is treated as absent rather than raised on: these are read to
/// render a settings page, and pi itself still owns the repair.
fn load(path: &Path) -> Map<String, Value> {
    match fs::read_to_string(path).ok().as_deref().map(serde_json::from_str) {
        Some(Ok(Value::Object(data))) => data,
        _ => Map::new(),
    }
}

/// Write `data` as pi formats it. A `private` file is created 0600, and an
/// existing file's mode is tightened to match — pi's own guarantee for
/// auth.json, which we must not weaken by rewriting it.
fn store(path: &Path, data: &Map<String, Value>, private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(data.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    if !private {
        return fs::write(path, text);
    }
    // Open first, tighten second, write last, so the key never exists on disk
    // under a laxer mode even briefly. The 0o600 passed at open applies only
    // when it creates the file: an auth.json already there at 0644 —
    // hand-edited, or restored from a backup — would otherwise take the key
    // world-readable and be tightened only afterwards. The tightening acts on
    // the handle we hold, so no one can swap the path underneath it.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
}

// ---------------------------------------------------------------------------
// Credentials (auth.json)
// ---------------------------------------------------------------------------

/// Escape a pasted key so pi stores it verbatim.
///
/// pi's `key` field is a small language: a leading `!` runs the value as a
/// shell command, and `$NAME` interpolates the environment anywhere in the
/// string. A key that happens to contain either would otherwise be resolved
/// into something else — or executed.
pub fn literal_key(value: &str) -> String {
    let escaped = value.replace('$', "$$");
    if escaped.starts_with('!') {
        // "$!" is pi's escape for a literal "!".
        format!("${escaped}")
    } else {
        escaped
    }
}

pub fn load_auth() -> Map<String, Value> {
    load(&auth_path())
}

/// Store an API key for `provider_id`, leaving every other provider's
/// credential — including OAuth records pi refreshes on its own — in place.
pub fn save_api_key(provider_id: &str, key: &str) -> std::io::Result<()> {
    let mut auth = load_auth();
    auth.insert(
        provider_id.to_string(),
        json!({ "type": "api_key", "key": literal_key(key.trim()) }),
    );
    store(&auth_path(), &auth, true)
}

/// Forget a provider's credential; `true` when there was one to forget.
pub fn remove_credential(provider_id: &str) -> bool {
    let mut auth = load_auth();
    if auth.remove(provider_id).is_none() {
        return false;
    }
    store(&auth_path(), &auth, true).is_ok()
}

/// `"api_key"`, `"oauth"`, or `None` when the provider has no stored
/// credential. Never returns the credential itself: nothing in the UI has a
/// reason to read a key back, and the ones pi stores are bearer tokens.
pub fn credential_kind(provider_id: &str) -> Option<String> {
    load_auth()
        .get(provider_id)?
        .get("type")?
        .as_str()
        .map(str::to_string)
}

/// Where the key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// A key pi reads out of auth.json.
    AuthFile,
    /// The provider's environment variable, which pi falls back to.
    Env,
    /// Nothing configured.
    None,
}

/// Where pi would find this provider's key — in pi's own precedence, the file
/// first.
pub fn api_key_source(provider_id: &str, env_var: &str) -> KeySource {
    if credential_kind(provider_id).as_deref() == Some("api_key") {
        return KeySource::AuthFile;
    }
    match std::env::var(env_var) {
        Ok(value) if !value.is_empty() => KeySource::Env,
        _ => KeySource::None,
    }
}

/// Epoch milliseconds at which the stored OAuth token expires, when the record
/// carries one. pi refreshes it in the background, so this says how fresh the
/// sign-in is, not whether it still works.
pub fn oauth_expiry(provider_id: &str) -> Option<i64> {
    let auth = load_auth();
    let entry = auth.get(provider_id)?;
    if entry.get("type").and_then(Value::as_str) != Some("oauth") {
        return None;
    }
    entry.get("expires")?.as_i64()
}

// ---------------------------------------------------------------------------
// Custom providers (models.json)
// ---------------------------------------------------------------------------

pub fn load_providers() -> Map<String, Value> {
    match load(&models_path()).get("providers") {
        Some(Value::Object(providers)) => providers.clone(),
        _ => Map::new(),
    }
}

/// The custom providers that look like a local server — an OpenAI-compatible
/// API on a loopback or private address. These are the ones this UI offers to
/// edit; a proxy to a paid cloud endpoint is left to models.json, where whoever
/// wrote it can see everything it sets.
pub fn local_providers() -> Map<String, Value> {
    load_providers()
        .into_iter()
        .filter(|(_, config)| {
            let url = config.get("baseUrl").and_then(Value::as_str).unwrap_or("");
            ["localhost", "127.0.0.1", "0.0.0.0", "::1"]
                .iter()
                .any(|host| url.contains(host))
        })
        .collect()
}

/// Declare (or update) a local OpenAI-compatible provider.
///
/// Everything already configured for it survives: `compat` flags a server
/// needs, the real API key if one was set, and each surviving model's own
/// fields (its display name, its input modalities). Only the base URL and the
/// set of model ids are ours to set — the rest is the user's, written by hand
/// or by pi, and rewriting it from a two-field form would silently drop it.
pub fn save_local_provider(
    provider_id: &str,
    base_url: &str,
    model_ids: &[String],
) -> std::io::Result<()> {
    let mut data = load(&models_path());
    let mut providers = match data.get("providers") {
        Some(Value::Object(providers)) => providers.clone(),
        _ => Map::new(),
    };
    let mut config = match providers.get(provider_id) {
        Some(Value::Object(config)) => config.clone(),
        _ => Map::new(),
    };
    let previous: Map<String, Value> = match config.get("models") {
        Some(Value::Array(models)) => models
            .iter()
            .filter_map(|model| {
                let id = model.get("id")?.as_str()?;
                Some((id.to_string(), model.clone()))
            })
            .collect(),
        _ => Map::new(),
    };
    config.insert("baseUrl".into(), json!(base_url.trim()));
    config.entry("api").or_insert_with(|| json!(LOCAL_API));
    config
        .entry("apiKey")
        .or_insert_with(|| json!(LOCAL_PLACEHOLDER_KEY));
    config.insert(
        "models".into(),
        Value::Array(
            model_ids
                .iter()
                .map(|id| {
                    previous
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| json!({ "id": id }))
                })
                .collect(),
        ),
    );
    providers.insert(provider_id.to_string(), Value::Object(config));
    data.insert("providers".into(), Value::Object(providers));
    store(&models_path(), &data, false)
}

/// Drop a custom provider from models.json; `true` when one was there.
pub fn remove_local_provider(provider_id: &str) -> bool {
    let mut data = load(&models_path());
    let mut providers = match data.get("providers") {
        Some(Value::Object(providers)) => providers.clone(),
        _ => return false,
    };
    if providers.remove(provider_id).is_none() {
        return false;
    }
    data.insert("providers".into(), Value::Object(providers));
    store(&models_path(), &data, false).is_ok()
}

// ---------------------------------------------------------------------------
// Model scope (settings.json)
// ---------------------------------------------------------------------------
//
// pi already has a way to narrow the models a session offers: `enabledModels`
// in settings.json, a list of patterns (the `--models` flag in file form). pi
// resolves it at session start for its own picker and Ctrl+P cycling, but the
// RPC `get_available_models` answers with the whole catalogue regardless — so
// the scope is stored where pi reads it and applied here as well.

/// A pattern may pin a thinking level with a trailing `:level`
/// (`anthropic/*:high`). It says nothing about which models are in scope, so it
/// is stripped before matching. (The controller keeps the same list for the
/// effort picker.)
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn load_settings() -> Map<String, Value> {
    load(&settings_path())
}

/// The configured scope, or `[]` when there is none — which is pi's own reading
/// of an absent `enabledModels`: every model is in scope.
pub fn enabled_models() -> Vec<String> {
    match load_settings().get(ENABLED_MODELS) {
        Some(Value::Array(patterns)) => patterns
            .iter()
            .filter_map(Value::as_str)
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Set the scope, leaving every other pi setting alone. An empty list removes
/// the key rather than writing `[]`: to pi they mean the same thing, and the
/// absent key is the one a later pi release will keep meaning.
pub fn save_enabled_models(patterns: &[String]) -> std::io::Result<()> {
    let mut settings = load_settings();
    if patterns.is_empty() {
        settings.remove(ENABLED_MODELS);
    } else {
        settings.insert(ENABLED_MODELS.into(), json!(patterns));
    }
    store(&settings_path(), &settings, false)
}

/// A model as pi's canonical `provider/id` reference — what is written into
/// `enabledModels`, since it names one model and nothing else.
pub fn model_ref(model: &Value) -> String {
    let field = |key: &str| model.get(key).and_then(Value::as_str).unwrap_or_default();
    format!("{}/{}", field("provider"), field("id"))
}

/// Whether one `enabledModels` pattern covers this model, following pi's
/// model-resolver: a glob is matched against `provider/id` and against the bare
/// id, anything else is an exact reference, then a substring of the id or the
/// display name.
///
/// Where pi's resolver narrows an ambiguous substring to a single best model,
/// this keeps every match. The difference only ever shows a model the user's
/// own pattern named — the safe direction for a filter whose job is hiding.
pub fn matches_pattern(model: &Value, pattern: &str) -> bool {
    let mut pattern = pattern.trim();
    if let Some((head, tail)) = pattern.rsplit_once(':') {
        if !head.is_empty() && THINKING_LEVELS.contains(&tail) {
            pattern = head.trim();
        }
    }
    if pattern.is_empty() {
        return false;
    }
    let needle = pattern.to_lowercase();
    let reference = model_ref(model).to_lowercase();
    let field = |key: &str| {
        model
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase()
    };
    let model_id = field("id");
    let name = field("name");
    if pattern.contains(['*', '?', '[']) {
        return glob_match(&needle, &reference) || glob_match(&needle, &model_id);
    }
    if needle == reference || needle == model_id {
        return true;
    }
    !model_id.is_empty()
        && (model_id.contains(&needle) || (!name.is_empty() && name.contains(&needle)))
}

/// `models` narrowed to the configured scope, in the order pi listed them. An
/// empty scope selects everything, as it does for pi.
pub fn in_scope(models: &[Value], patterns: Option<&[String]>) -> Vec<Value> {
    let configured;
    let patterns = match patterns {
        Some(patterns) => patterns,
        None => {
            configured = enabled_models();
            &configured
        }
    };
    if patterns.is_empty() {
        return models.to_vec();
    }
    models
        .iter()
        .filter(|model| {
            patterns
                .iter()
                .any(|pattern| matches_pattern(model, pattern))
        })
        .cloned()
        .collect()
}

/// Shell-style matching over the whole string: `*`, `?` and `[…]` classes, the
/// dialect pi's resolver uses. Case is already folded by the caller.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    // Iterative backtracking rather than recursion: a pattern is user input,
    // and a `*` would otherwise cost one stack frame per candidate split.
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);
    while t < text.len() {
        if pattern.get(p) == Some(&'*') {
            star = Some(p);
            p += 1;
            resume = t;
            continue;
        }
        let step = match pattern.get(p) {
            Some('?') => Some((p + 1, t + 1)),
            Some('[') => match class_match(&pattern, p, text[t]) {
                Some((true, next)) => Some((next, t + 1)),
                Some((false, _)) => None,
                // A bracket that never closes is a literal `[`, as in fnmatch.
                None => (text[t] == '[').then_some((p + 1, t + 1)),
            },
            Some(character) if *character == text[t] => Some((p + 1, t + 1)),
            _ => None,
        };
        match (step, star) {
            (Some((next_pattern, next_text)), _) => {
                p = next_pattern;
                t = next_text;
            }
            // The last `*` swallows one more character and the tail is retried.
            (None, Some(index)) => {
                p = index + 1;
                resume += 1;
                t = resume;
            }
            (None, None) => return false,
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

/// Match one `[…]` class at `start` against `candidate`. Returns whether it
/// matched and where the pattern continues, or `None` when the bracket never
/// closes — fnmatch reads that as a literal `[`.
fn class_match(pattern: &[char], start: usize, candidate: char) -> Option<(bool, usize)> {
    let mut index = start + 1;
    let negated = matches!(pattern.get(index), Some('!') | Some('^'));
    if negated {
        index += 1;
    }
    let first = index;
    let mut matched = false;
    while index < pattern.len() {
        let character = pattern[index];
        if character == ']' && index > first {
            return Some((matched != negated, index + 1));
        }
        if pattern.get(index + 1) == Some(&'-') && pattern.get(index + 2).is_some_and(|c| *c != ']')
        {
            let (low, high) = (character, pattern[index + 2]);
            matched |= low <= candidate && candidate <= high;
            index += 3;
            continue;
        }
        matched |= character == candidate;
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent directory and the environment are process-global, so the tests
    /// that change them take turns.
    static GUARD: Mutex<()> = Mutex::new(());

    struct Scratch {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let lock = GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("pupo-pi-config-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a scratch agent dir");
            set_agent_dir(Some(&dir));
            Self { dir, _lock: lock }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
            set_agent_dir(None);
            std::env::remove_var(OPENROUTER_ENV);
            std::env::remove_var(ENV_AGENT_DIR);
            std::env::remove_var(ENV_PI_HOME);
        }
    }

    fn read(path: PathBuf) -> Value {
        serde_json::from_str(&fs::read_to_string(path).expect("the file exists")).expect("json")
    }

    fn mode(path: PathBuf) -> u32 {
        fs::metadata(path).expect("stat").permissions().mode() & 0o777
    }

    #[test]
    fn pis_own_override_names_the_directory_and_ours_names_its_root() {
        let scratch = Scratch::new("override");
        assert_eq!(agent_dir(), scratch.dir);
        assert_eq!(auth_path(), scratch.dir.join("auth.json"));
        set_agent_dir(None);
        std::env::set_var(ENV_AGENT_DIR, "/somewhere/agent");
        assert_eq!(agent_dir(), PathBuf::from("/somewhere/agent"));
        // PUPO_PI_HOME points at the `.pi` root, the way `~/.pi` is written.
        std::env::remove_var(ENV_AGENT_DIR);
        std::env::set_var(ENV_PI_HOME, "/somewhere/pi");
        assert_eq!(agent_dir(), PathBuf::from("/somewhere/pi/agent"));
    }

    #[test]
    fn the_auth_file_is_private() {
        let _scratch = Scratch::new("private");
        save_api_key(OPENROUTER, "sk-or-v1-secret").expect("saved");
        assert_eq!(mode(auth_path()), 0o600);
        assert_eq!(
            read(auth_path())[OPENROUTER],
            json!({"type": "api_key", "key": "sk-or-v1-secret"})
        );
    }

    #[test]
    fn a_lax_auth_file_is_tightened_before_the_key_lands() {
        // An auth.json already there at 0644 — hand-edited, or restored from a
        // backup. The mode passed at open applies only when it creates the
        // file, so without the explicit tightening the key would be written
        // world-readable first.
        let _scratch = Scratch::new("tighten");
        fs::write(auth_path(), "{}").expect("a pre-existing file");
        fs::set_permissions(auth_path(), fs::Permissions::from_mode(0o644)).expect("chmod");
        save_api_key(OPENROUTER, "sk-or-v1-secret").expect("saved");
        assert_eq!(mode(auth_path()), 0o600);
    }

    #[test]
    fn saving_a_key_leaves_other_credentials_alone() {
        let _scratch = Scratch::new("merge-auth");
        // An OAuth record pi wrote and refreshes itself.
        fs::write(
            auth_path(),
            json!({CODEX: {"type": "oauth", "access": "t", "expires": 1}}).to_string(),
        )
        .expect("written");
        save_api_key(OPENROUTER, "sk-or-v1-x").expect("saved");
        let auth = read(auth_path());
        assert_eq!(auth[CODEX], json!({"type": "oauth", "access": "t", "expires": 1}));
        assert_eq!(auth[OPENROUTER]["type"], "api_key");
    }

    #[test]
    fn a_pasted_key_is_stored_literally() {
        // pi resolves "!cmd" as a shell command and "$NAME" from the
        // environment, so a key containing either has to be escaped on the way
        // in.
        let _scratch = Scratch::new("literal");
        save_api_key("openai", "!rm -rf /").expect("saved");
        save_api_key("anthropic", "sk-$HOME-1").expect("saved");
        let auth = read(auth_path());
        assert_eq!(auth["openai"]["key"], "$!rm -rf /");
        assert_eq!(auth["anthropic"]["key"], "sk-$$HOME-1");
    }

    #[test]
    fn credential_state_is_reported_without_reading_the_secret() {
        let _scratch = Scratch::new("state");
        assert_eq!(credential_kind(OPENROUTER), None);
        assert_eq!(api_key_source(OPENROUTER, OPENROUTER_ENV), KeySource::None);
        save_api_key(OPENROUTER, "sk-or-v1-x").expect("saved");
        assert_eq!(credential_kind(OPENROUTER).as_deref(), Some("api_key"));
        assert_eq!(
            api_key_source(OPENROUTER, OPENROUTER_ENV),
            KeySource::AuthFile
        );
        assert!(remove_credential(OPENROUTER));
        assert!(!remove_credential(OPENROUTER));
    }

    #[test]
    fn the_env_var_is_reported_only_as_a_fallback() {
        let _scratch = Scratch::new("env-fallback");
        std::env::set_var(OPENROUTER_ENV, "sk-or-v1-from-env");
        assert_eq!(api_key_source(OPENROUTER, OPENROUTER_ENV), KeySource::Env);
        // auth.json wins in pi's own resolution order, so it must win here too.
        save_api_key(OPENROUTER, "sk-or-v1-stored").expect("saved");
        assert_eq!(
            api_key_source(OPENROUTER, OPENROUTER_ENV),
            KeySource::AuthFile
        );
    }

    #[test]
    fn an_expiry_is_read_only_from_an_oauth_record() {
        let _scratch = Scratch::new("oauth");
        fs::write(
            auth_path(),
            json!({CODEX: {"type": "oauth", "expires": 1786698578488i64}}).to_string(),
        )
        .expect("written");
        assert_eq!(oauth_expiry(CODEX), Some(1786698578488));
        save_api_key(CODEX, "not-an-oauth-record").expect("saved");
        assert_eq!(oauth_expiry(CODEX), None);
    }

    #[test]
    fn a_corrupt_file_reads_as_absent() {
        let _scratch = Scratch::new("corrupt");
        fs::write(auth_path(), "{ not json").expect("written");
        assert!(load_auth().is_empty());
        assert_eq!(credential_kind(OPENROUTER), None);
    }

    #[test]
    fn saving_a_local_provider_keeps_what_it_did_not_set() {
        // A provider configured by hand: a compat flag the server needs, a real
        // key, and a model carrying its own display name.
        let _scratch = Scratch::new("providers");
        fs::write(
            models_path(),
            json!({
                "providers": {
                    "llamacpp": {
                        "baseUrl": "http://localhost:8080/v1",
                        "api": "openai-completions",
                        "apiKey": "hand-written",
                        "compat": {"supportsDeveloperRole": false},
                        "models": [{"id": "qwen", "name": "Qwen 3", "input": ["text"]}],
                    }
                },
                "somethingElse": {"kept": true},
            })
            .to_string(),
        )
        .expect("written");
        save_local_provider(
            "llamacpp",
            "http://localhost:9090/v1",
            &["qwen".to_string(), "llama3.1:8b".to_string()],
        )
        .expect("saved");
        let data = read(models_path());
        let provider = &data["providers"]["llamacpp"];
        assert_eq!(provider["baseUrl"], "http://localhost:9090/v1");
        assert_eq!(provider["apiKey"], "hand-written");
        assert_eq!(provider["compat"], json!({"supportsDeveloperRole": false}));
        // The surviving model keeps its own fields; the new one is just an id.
        assert_eq!(
            provider["models"],
            json!([
                {"id": "qwen", "name": "Qwen 3", "input": ["text"]},
                {"id": "llama3.1:8b"}
            ])
        );
        assert_eq!(data["somethingElse"], json!({"kept": true}));
    }

    #[test]
    fn a_new_local_provider_gets_the_defaults_pi_needs() {
        let _scratch = Scratch::new("defaults");
        save_local_provider(
            LOCAL_PROVIDER_ID,
            "http://localhost:11434/v1",
            &["llama3".to_string()],
        )
        .expect("saved");
        let provider = read(models_path())["providers"][LOCAL_PROVIDER_ID].clone();
        assert_eq!(provider["api"], LOCAL_API);
        // pi hides models whose provider has no auth at all, so a keyless local
        // server still needs the placeholder.
        assert_eq!(provider["apiKey"], LOCAL_PLACEHOLDER_KEY);
    }

    #[test]
    fn only_local_providers_are_offered_for_editing() {
        let _scratch = Scratch::new("local-only");
        fs::write(
            models_path(),
            json!({"providers": {
                "local": {"baseUrl": "http://127.0.0.1:8080/v1"},
                "some-proxy": {"baseUrl": "https://api.example.com/v1"},
            }})
            .to_string(),
        )
        .expect("written");
        let local: Vec<String> = local_providers().keys().cloned().collect();
        assert_eq!(local, ["local"]);
        assert!(remove_local_provider("local"));
        assert!(!remove_local_provider("local"));
        let left: Vec<String> = read(models_path())["providers"]
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(left, ["some-proxy"]);
    }

    fn models() -> Vec<Value> {
        vec![
            json!({"provider": "anthropic", "id": "claude-opus-4-8", "name": "Claude Opus"}),
            json!({"provider": "openrouter", "id": "moonshotai/kimi-k3", "name": "Kimi K3"}),
            json!({"provider": "openai-codex", "id": "gpt-5.5"}),
        ]
    }

    #[test]
    fn no_scope_means_every_model() {
        let _scratch = Scratch::new("no-scope");
        assert!(enabled_models().is_empty());
        assert_eq!(in_scope(&models(), None), models());
    }

    #[test]
    fn the_scope_is_written_where_pi_reads_it() {
        let _scratch = Scratch::new("scope-file");
        fs::write(settings_path(), json!({"theme": "dark"}).to_string()).expect("written");
        save_enabled_models(&["openrouter/moonshotai/kimi-k3".to_string()]).expect("saved");
        let settings = read(settings_path());
        assert_eq!(settings[ENABLED_MODELS], json!(["openrouter/moonshotai/kimi-k3"]));
        // It is pi's file: every other setting it holds survives the write.
        assert_eq!(settings["theme"], "dark");
        // An empty scope is an absent key, not an empty list — to pi they mean
        // the same thing, and only one of them says it plainly.
        save_enabled_models(&[]).expect("saved");
        assert!(read(settings_path()).get(ENABLED_MODELS).is_none());
        assert_eq!(read(settings_path())["theme"], "dark");
    }

    #[test]
    fn scope_patterns_follow_pis_matching() {
        let opus = &models()[0];
        let kimi = &models()[1];
        // A canonical provider/id reference, which is what the settings page
        // writes.
        assert!(matches_pattern(opus, "anthropic/claude-opus-4-8"));
        assert!(!matches_pattern(kimi, "anthropic/claude-opus-4-8"));
        // A bare id, a glob against either form, and a substring of id or name.
        assert!(matches_pattern(opus, "claude-opus-4-8"));
        assert!(matches_pattern(opus, "claude-*"));
        assert!(matches_pattern(kimi, "openrouter/*"));
        assert!(matches_pattern(opus, "opus"));
        assert!(matches_pattern(kimi, "Kimi"));
        // A pinned thinking level says nothing about which models are in scope.
        assert!(matches_pattern(opus, "anthropic/*:high"));
        assert!(matches_pattern(opus, "claude-opus-4-8:xhigh"));
        assert!(!matches_pattern(kimi, "anthropic/*:high"));
        // A glob is anchored: it has to cover the whole reference, where a
        // plain substring does not.
        assert!(matches_pattern(opus, "claude-opus-4"));
        assert!(!matches_pattern(opus, "claude-*-4"));
        assert!(matches_pattern(opus, "claude-opus-4-?"));
        assert!(matches_pattern(opus, "*/claude-[a-z]*-4-8"));
        assert!(!matches_pattern(opus, "*/claude-[0-9]*-4-8"));
        // A pattern that is nothing but a level pin selects nothing.
        assert!(!matches_pattern(opus, ":high"));
        assert!(!matches_pattern(opus, "   "));
    }

    #[test]
    fn a_stored_scope_hides_the_models_outside_it() {
        let _scratch = Scratch::new("scope-filter");
        fs::write(
            settings_path(),
            json!({ENABLED_MODELS: ["anthropic/claude-opus-4-8", "gpt-5.5"]}).to_string(),
        )
        .expect("written");
        let ids: Vec<String> = in_scope(&models(), None)
            .iter()
            .map(|model| model["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(ids, ["claude-opus-4-8", "gpt-5.5"]);
    }

    #[test]
    fn a_reference_names_a_provider_and_a_model() {
        assert_eq!(model_ref(&models()[0]), "anthropic/claude-opus-4-8");
        // A half-known model still produces a reference rather than nothing.
        assert_eq!(model_ref(&json!({"id": "gpt-5.5"})), "/gpt-5.5");
    }
}
