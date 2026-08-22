//! The Settings window, as QML sees it.
//!
//! Two of its four pages have nothing to hold here — General says there is
//! nothing to set yet, and Theme edits the [`Theme`](super::theme::Theme) object
//! directly, which is where the theme and the two font scales already live. What
//! is left is Providers and Models, and both of them edit files that belong to
//! **pi** rather than to us: `auth.json` for credentials, `models.json` for
//! custom servers, `settings.json` for the model scope. [`pupo_core::pi::config`]
//! holds those contracts and [`pupo_core::settings`] holds the tree's rules;
//! this is the seam between them and the window.
//!
//! Every page restates itself from disk after every write rather than from what
//! was typed. These are the same files a pi user edits by hand, and a page that
//! believed its own last write would quietly disagree with the file it shares.

use std::path::Path;

use pupo_core::pi::config::KeySource;
use pupo_core::pi::{catalogue, config};
use pupo_core::settings::{Check, ModelTree};
use qmetaobject::*;
use serde_json::Value;

#[derive(QObject, Default)]
pub struct SettingsBridge {
    base: qt_base_class!(trait QObject),

    /// Whether the window is up. QML binds its overlay to this rather than
    /// keeping a flag of its own, so the workspace bar's button, Escape and a
    /// sign-in hand-off all close the same one thing.
    open: qt_property!(bool; NOTIFY open_changed READ get_open),

    // ---- Providers ---------------------------------------------------------
    /// One sentence each on where the three credentials stand.
    openrouter_status: qt_property!(QString; NOTIFY providers_changed READ get_openrouter_status),
    /// Whether there is a stored key to forget — as opposed to one in the
    /// environment, which is pi's to read and not ours to delete.
    openrouter_stored: qt_property!(bool; NOTIFY providers_changed READ get_openrouter_stored),
    local_status: qt_property!(QString; NOTIFY providers_changed READ get_local_status),
    /// The single configured local server, prefilled so an edit changes that
    /// server rather than quietly declaring a second one. Empty when there is
    /// none, or when there are several and no one of them is the obvious one.
    local_id: qt_property!(QString; NOTIFY providers_changed READ get_local_id),
    local_url: qt_property!(QString; NOTIFY providers_changed READ get_local_url),
    local_models: qt_property!(QString; NOTIFY providers_changed READ get_local_models),
    codex_status: qt_property!(QString; NOTIFY providers_changed READ get_codex_status),
    codex_signed_in: qt_property!(bool; NOTIFY providers_changed READ get_codex_signed_in),
    /// Where the keys are written, as the reader would write it themselves.
    auth_path: qt_property!(QString; NOTIFY providers_changed READ get_auth_path),

    // ---- Models ------------------------------------------------------------
    /// One entry per visible row: `{ index, depth, label, group, expanded,
    /// state }`, `state` being "off", "partial" or "on".
    model_rows: qt_property!(QVariantList; NOTIFY models_changed READ get_model_rows),
    /// What the page says under the tree: the saved scope, or why there is none.
    models_status: qt_property!(QString; NOTIFY models_changed READ get_models_status),
    /// Whether the catalogue is still being fetched, so the page can say so
    /// rather than looking like a catalogue with nothing in it.
    models_loading: qt_property!(bool; NOTIFY models_changed READ get_models_loading),

    open_changed: qt_signal!(),
    providers_changed: qt_signal!(),
    models_changed: qt_signal!(),
    /// The user asked to sign in to their ChatGPT plan. Only pi's own
    /// interactive OAuth flow can do it — there is no RPC command and no
    /// headless CLI behind it — so the window is asked to hand over a terminal
    /// already running it.
    codex_signin_requested: qt_signal!(),

    show: qt_method!(fn(&mut self)),
    hide: qt_method!(fn(&mut self)),

    save_openrouter_key: qt_method!(fn(&mut self, key: QString) -> QString),
    forget_openrouter_key: qt_method!(fn(&mut self)),
    save_local_provider: qt_method!(fn(&mut self, id: QString, url: QString, models: QString) -> QString),
    remove_local_provider: qt_method!(fn(&mut self, id: QString)),
    request_codex_signin: qt_method!(fn(&mut self)),
    sign_out_codex: qt_method!(fn(&mut self)),

    /// Ask pi for its catalogue. Blocking, on a worker thread; the page fills in
    /// when the answer arrives.
    load_models: qt_method!(fn(&mut self)),
    toggle_model: qt_method!(fn(&mut self, index: i32)),
    expand_model_group: qt_method!(fn(&mut self, index: i32, open: bool)),
    set_all_models: qt_method!(fn(&mut self, checked: bool)),
    filter_models: qt_method!(fn(&mut self, query: QString)),
    save_models: qt_method!(fn(&mut self)),

    tree: ModelTree,
    /// Set while a fetch is in flight; a second click while one is running is a
    /// second throwaway pi for an answer already on its way.
    loading: bool,
    /// Bumped per fetch, so a reply from one the window has moved on from is
    /// dropped rather than rendered.
    generation: u64,
    models_note: String,
    visible: bool,
}

impl SettingsBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_open(&self) -> bool {
        self.visible
    }

    fn show(&mut self) {
        if self.visible {
            return;
        }
        self.visible = true;
        // Read from disk on the way up rather than held from last time: these
        // files are shared with pi and with whoever edits them by hand.
        self.providers_changed();
        self.open_changed();
        self.load_models();
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.open_changed();
    }

    // ---- Providers ---------------------------------------------------------

    fn get_auth_path(&self) -> QString {
        short_path(&config::auth_path()).as_str().into()
    }

    fn get_openrouter_status(&self) -> QString {
        match config::api_key_source(config::OPENROUTER, config::OPENROUTER_ENV) {
            KeySource::AuthFile => "Key saved in auth.json.".into(),
            KeySource::Env => format!(
                "No key here, but ${} is set in the environment and pi will use that.",
                config::OPENROUTER_ENV
            )
            .as_str()
            .into(),
            KeySource::None => "No key configured.".into(),
        }
    }

    fn get_openrouter_stored(&self) -> bool {
        config::api_key_source(config::OPENROUTER, config::OPENROUTER_ENV) == KeySource::AuthFile
    }

    fn save_openrouter_key(&mut self, key: QString) -> QString {
        let key = key.to_string();
        let key = key.trim();
        if key.is_empty() {
            return "Enter a key first.".into();
        }
        if let Err(error) = config::save_api_key(config::OPENROUTER, key) {
            return format!("Could not write auth.json: {error}").as_str().into();
        }
        self.providers_changed();
        "".into()
    }

    fn forget_openrouter_key(&mut self) {
        config::remove_credential(config::OPENROUTER);
        self.providers_changed();
    }

    /// The one configured local server, or nothing. Prefilled only when it is
    /// unambiguous: with two configured, filling the fields from either would
    /// make the next Save edit a server the reader did not choose.
    fn local(&self) -> Option<(String, Value)> {
        let configured = config::local_providers();
        if configured.len() != 1 {
            return None;
        }
        configured
            .into_iter()
            .next()
    }

    fn get_local_id(&self) -> QString {
        self.local().map(|(id, _)| id).unwrap_or_default().as_str().into()
    }

    fn get_local_url(&self) -> QString {
        self.local()
            .and_then(|(_, value)| {
                value
                    .get("baseUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
            .as_str()
            .into()
    }

    fn get_local_models(&self) -> QString {
        self.local()
            .and_then(|(_, value)| value.get("models").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(", ")
            .as_str()
            .into()
    }

    fn get_local_status(&self) -> QString {
        let configured = config::local_providers();
        if configured.is_empty() {
            return "No local server configured.".into();
        }
        let mut names: Vec<&str> = configured.keys().map(String::as_str).collect();
        names.sort_unstable();
        format!("Configured in models.json: {}.", names.join(", "))
            .as_str()
            .into()
    }

    fn save_local_provider(&mut self, id: QString, url: QString, models: QString) -> QString {
        let id = id.to_string();
        let id = id.trim();
        let provider = if id.is_empty() {
            config::LOCAL_PROVIDER_ID
        } else {
            id
        };
        let url = url.to_string();
        let url = url.trim();
        let base = if url.is_empty() {
            config::LOCAL_BASE_URL
        } else {
            url
        };
        let models = models.to_string();
        let ids: Vec<String> = models
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        if ids.is_empty() {
            return "List at least one model id — pi only offers models a provider declares."
                .into();
        }
        if let Err(error) = config::save_local_provider(provider, base, &ids) {
            return format!("Could not write models.json: {error}").as_str().into();
        }
        self.providers_changed();
        "".into()
    }

    fn remove_local_provider(&mut self, id: QString) {
        let id = id.to_string();
        let id = id.trim();
        if !id.is_empty() {
            config::remove_local_provider(id);
            self.providers_changed();
        }
    }

    fn get_codex_status(&self) -> QString {
        let Some(kind) = config::credential_kind(config::CODEX) else {
            return "Not signed in.".into();
        };
        if kind != "oauth" {
            // An API key against the Codex provider, put there by hand. Saying
            // "Not signed in." beside a live Sign out would offer to delete a
            // credential the page had just denied having.
            return "An API key is configured for this provider, rather than a ChatGPT plan \
                    sign-in. Sign out removes it."
                .into();
        }
        match config::oauth_expiry(config::CODEX) {
            // An expired token is not a signed-out one: pi refreshes it on use,
            // so this reads as the age of the sign-in rather than a warning.
            Some(expiry) => format!("Signed in. Token valid until {}.", stamp(expiry))
                .as_str()
                .into(),
            None => "Signed in.".into(),
        }
    }

    fn get_codex_signed_in(&self) -> bool {
        config::credential_kind(config::CODEX).is_some()
    }

    fn request_codex_signin(&mut self) {
        // Closing first: a modal window is exactly what stands between the user
        // and the flow it has just asked to start.
        self.hide();
        self.codex_signin_requested();
    }

    fn sign_out_codex(&mut self) {
        config::remove_credential(config::CODEX);
        self.providers_changed();
    }

    // ---- Models ------------------------------------------------------------

    fn get_models_loading(&self) -> bool {
        self.loading
    }

    fn get_models_status(&self) -> QString {
        self.models_note.as_str().into()
    }

    fn get_model_rows(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for row in self.tree.rows() {
            let mut map = QVariantMap::default();
            map.insert("index".into(), QVariant::from(row.index as i32));
            map.insert("depth".into(), QVariant::from(i32::from(row.depth)));
            map.insert("label".into(), text(&row.label));
            map.insert("group".into(), QVariant::from(row.group));
            map.insert("expanded".into(), QVariant::from(row.expanded));
            map.insert(
                "state".into(),
                text(match row.state {
                    Check::Off => "off",
                    Check::Partial => "partial",
                    Check::On => "on",
                }),
            );
            list.push(map.into());
        }
        list
    }

    fn load_models(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.generation += 1;
        let generation = self.generation;
        self.models_note = "Asking pi for its model list…".to_string();
        self.models_changed();

        let pointer = QPointer::from(&*self);
        let deliver = queued_callback(move |models: Vec<Value>| {
            if let Some(this) = pointer.as_pinned() {
                this.borrow_mut().on_models(generation, models);
            }
        });
        // A throwaway `pi --mode rpc --no-session` in the user's home, stopped
        // as soon as it has answered. It blocks for as long as pi takes, which
        // on a first run is a network fetch — hence the thread.
        std::thread::spawn(move || {
            deliver(catalogue::query_models(catalogue::TIMEOUT));
        });
    }

    fn on_models(&mut self, generation: u64, models: Vec<Value>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        let patterns = config::enabled_models();
        self.tree = ModelTree::from_models(&models, &patterns);
        self.report(&patterns);
    }

    /// What the page says under the tree, from the scope as it is on disk.
    fn report(&mut self, patterns: &[String]) {
        self.models_note = if self.tree.model_count() == 0 && patterns.is_empty() {
            "pi listed no models. Configure a provider first — the catalogue is pi's, and \
             it only lists what it has credentials for."
                .to_string()
        } else if patterns.is_empty() {
            "No scope saved, so every model pi offers is in scope.".to_string()
        } else {
            format!(
                "{} {} in scope, from enabledModels in settings.json.",
                patterns.len(),
                if patterns.len() == 1 {
                    "pattern"
                } else {
                    "patterns"
                }
            )
        };
        self.models_changed();
    }

    fn toggle_model(&mut self, index: i32) {
        if let Ok(index) = usize::try_from(index) {
            self.tree.toggle(index);
            self.models_changed();
        }
    }

    fn expand_model_group(&mut self, index: i32, open: bool) {
        if let Ok(index) = usize::try_from(index) {
            self.tree.set_expanded(index, open);
            self.models_changed();
        }
    }

    fn set_all_models(&mut self, checked: bool) {
        self.tree.set_all(checked);
        self.models_changed();
    }

    fn filter_models(&mut self, query: QString) {
        self.tree.set_filter(&query.to_string());
        self.models_changed();
    }

    fn save_models(&mut self) {
        let checked = self.tree.checked_refs();
        if checked.is_empty() {
            // An empty list means "every model" to pi, which is the opposite of
            // what an empty page says. Nothing is written until a box goes back
            // on; the scope on disk stays as it was.
            self.models_note = "Nothing checked. Check at least one model — an empty list is \
                                how pi says “no scope at all”, so the saved scope is left as \
                                it was until then."
                .to_string();
            self.models_changed();
            return;
        }
        let stored = config::enabled_models();
        let kept = self.tree.orphan_patterns(&stored);
        // Everything checked and nothing else in the file: drop the setting
        // rather than pin today's catalogue, so a model added later shows up.
        let write = if self.tree.model_count() > 0
            && checked.len() == self.tree.model_count()
            && kept.is_empty()
        {
            Vec::new()
        } else {
            let mut all = checked;
            all.extend(kept);
            all
        };
        if let Err(error) = config::save_enabled_models(&write) {
            self.models_note = format!("Could not write settings.json: {error}");
            self.models_changed();
            return;
        }
        let patterns = config::enabled_models();
        self.report(&patterns);
    }
}

fn text(value: &str) -> QVariant {
    QVariant::from(QString::from(value))
}

/// A path as the reader would write it: `~/.pi/agent/auth.json`.
fn short_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// An OAuth expiry, in epoch milliseconds, as `21 Aug 20:34` in local time.
fn stamp(millis: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = (millis / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes into a struct we own and reads a plain
    // time_t; it is the reentrant form precisely so it is safe beside threads.
    if unsafe { libc::localtime_r(&time, &mut tm).is_null() } {
        return String::new();
    }
    let month = MONTHS
        .get(tm.tm_mon.clamp(0, 11) as usize)
        .copied()
        .unwrap_or("");
    format!("{:02} {month} {:02}:{:02}", tm.tm_mday, tm.tm_hour, tm.tm_min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_under_home_is_written_with_a_tilde() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(short_path(&home.join("a/b")), "~/a/b");
        assert_eq!(short_path(Path::new("/etc/hosts")), "/etc/hosts");
    }

    #[test]
    fn an_expiry_is_stamped_in_local_time() {
        // Only the shape is asserted: the value depends on the machine's zone,
        // and a test that pinned it would fail on a laptop that travelled.
        let stamped = stamp(1_755_000_000_000);
        assert_eq!(stamped.len(), "21 Aug 20:34".len());
    }
}
