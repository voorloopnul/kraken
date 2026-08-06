"""Read and write pi's own configuration files.

Kraken drives pi through the RPC protocol, but pi's credentials and custom
providers live in files the RPC surface does not expose — there is no `login`
command to send. This module is the boundary that touches them, following the
contracts in pi's `docs/providers.md` and `docs/models.md`:

* `~/.pi/agent/auth.json` — one entry per provider, `{"type": "api_key",
  "key": ...}` or an OAuth record pi writes and refreshes itself. Created
  `0600`, and it takes priority over environment variables.
* `~/.pi/agent/models.json` — custom providers (Ollama, llama.cpp, LM Studio,
  vLLM) under `providers`, each with a `baseUrl`, an `api`, and its models.
* `~/.pi/agent/settings.json` — pi's own settings, of which we touch exactly
  one key: `enabledModels`, the model scope (`docs/settings.md`).

Both files belong to pi, not to us: every write merges into what is already
there and leaves keys we do not understand untouched. `PI_CODING_AGENT_DIR`
relocates the whole directory, so it is honoured here exactly as pi honours it.
"""

from __future__ import annotations

import json
import os
from fnmatch import fnmatchcase
from pathlib import Path

# pi's own override for its config directory (config.js: ENV_AGENT_DIR).
_ENV_AGENT_DIR = "PI_CODING_AGENT_DIR"

# What a local OpenAI-compatible server (llama.cpp, Ollama, LM Studio, vLLM)
# is declared as. pi treats every model as needing auth before it shows up in
# the picker, so a keyless local server still needs a placeholder key.
LOCAL_API = "openai-completions"
LOCAL_PLACEHOLDER_KEY = "local"
LOCAL_PROVIDER_ID = "locallm"
LOCAL_BASE_URL = "http://localhost:8080/v1"

# Providers this settings UI knows how to configure, and the environment
# variable pi accepts for each in place of an auth.json entry.
OPENROUTER = "openrouter"
OPENROUTER_ENV = "OPENROUTER_API_KEY"
CODEX = "openai-codex"


def agent_dir() -> Path:
    """pi's config directory: `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`."""
    override = os.environ.get(_ENV_AGENT_DIR)
    if override:
        return Path(override).expanduser()
    return Path.home() / ".pi" / "agent"


def auth_path() -> Path:
    return agent_dir() / "auth.json"


def models_path() -> Path:
    return agent_dir() / "models.json"


def settings_path() -> Path:
    return agent_dir() / "settings.json"


def _load(path: Path) -> dict:
    """The file's contents, or an empty mapping when it is missing or corrupt.

    A broken file is treated as absent rather than raised on: these are read to
    render a settings page, and pi itself still owns the repair."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _save(path: Path, data: dict, private: bool) -> None:
    """Write `data` as pi formats it. `private` files are created 0600, and an
    existing file's mode is tightened to match — pi's own guarantee for
    auth.json, which we must not weaken by rewriting it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if private:
        # Open first, tighten second, write last, so the key never exists on
        # disk under a laxer mode even briefly. The 0o600 passed to os.open
        # applies only when it creates the file: an auth.json already there at
        # 0644 — hand-edited, or restored from a backup — would otherwise take
        # the key world-readable and be tightened only afterwards. fchmod acts
        # on the descriptor we hold, so no one can swap the path underneath it.
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w") as handle:
            os.fchmod(handle.fileno(), 0o600)
            handle.write(json.dumps(data, indent=2) + "\n")
    else:
        path.write_text(json.dumps(data, indent=2) + "\n")


# ---- Credentials (auth.json) ---------------------------------------------


def literal_key(value: str) -> str:
    """Escape a pasted key so pi stores it verbatim.

    pi's `key` field is a small language: a leading `!` runs the value as a
    shell command, and `$NAME` interpolates the environment anywhere in the
    string. A key that happens to contain either would otherwise be resolved
    into something else — or executed."""
    escaped = value.replace("$", "$$")
    if escaped.startswith("!"):
        escaped = "$" + escaped  # "$!" is pi's escape for a literal "!"
    return escaped


def load_auth() -> dict:
    return _load(auth_path())


def save_api_key(provider_id: str, key: str) -> None:
    """Store an API key for `provider_id`, leaving every other provider's
    credential — including OAuth records pi refreshes on its own — in place."""
    auth = load_auth()
    auth[provider_id] = {"type": "api_key", "key": literal_key(key.strip())}
    _save(auth_path(), auth, private=True)


def remove_credential(provider_id: str) -> bool:
    """Forget a provider's credential; True when there was one to forget."""
    auth = load_auth()
    if provider_id not in auth:
        return False
    del auth[provider_id]
    _save(auth_path(), auth, private=True)
    return True


def credential_kind(provider_id: str) -> str | None:
    """`"api_key"`, `"oauth"`, or None when the provider has no stored
    credential. Never returns the credential itself: nothing in the UI has a
    reason to read a key back, and the ones pi stores are bearer tokens."""
    entry = load_auth().get(provider_id)
    if not isinstance(entry, dict):
        return None
    kind = entry.get("type")
    return kind if isinstance(kind, str) else None


def api_key_source(provider_id: str, env_var: str) -> str:
    """Where pi would find this provider's key: `"auth.json"`, `"env"`, or
    `"none"` — in pi's own precedence, the file first."""
    if credential_kind(provider_id) == "api_key":
        return "auth.json"
    if os.environ.get(env_var):
        return "env"
    return "none"


def oauth_expiry(provider_id: str) -> int | None:
    """Epoch milliseconds at which the stored OAuth token expires, when the
    record carries one. pi refreshes it in the background, so this says how
    fresh the sign-in is, not whether it still works."""
    entry = load_auth().get(provider_id)
    if not isinstance(entry, dict) or entry.get("type") != "oauth":
        return None
    expires = entry.get("expires")
    return expires if isinstance(expires, int) else None


# ---- Custom providers (models.json) ---------------------------------------


def load_providers() -> dict:
    providers = _load(models_path()).get("providers")
    return providers if isinstance(providers, dict) else {}


def local_providers() -> dict:
    """The custom providers that look like a local server — an OpenAI-
    compatible API on a loopback or private address. These are the ones this
    UI offers to edit; a proxy to a paid cloud endpoint is left to models.json,
    where whoever wrote it can see everything it sets."""
    found = {}
    for provider_id, config in load_providers().items():
        if not isinstance(config, dict):
            continue
        url = str(config.get("baseUrl") or "")
        if any(host in url for host in ("localhost", "127.0.0.1", "0.0.0.0", "::1")):
            found[provider_id] = config
    return found


def save_local_provider(
    provider_id: str, base_url: str, model_ids: list[str]
) -> None:
    """Declare (or update) a local OpenAI-compatible provider.

    Everything already configured for it survives: `compat` flags a server
    needs, the real API key if one was set, and each surviving model's own
    fields (its display name, its input modalities). Only the base URL and the
    set of model ids are ours to set — the rest is the user's, written by hand
    or by pi, and rewriting it from a two-field form would silently drop it."""
    data = _load(models_path())
    providers = data.get("providers")
    if not isinstance(providers, dict):
        providers = {}
    existing = providers.get(provider_id)
    config = dict(existing) if isinstance(existing, dict) else {}
    previous = {
        model["id"]: model
        for model in config.get("models", [])
        if isinstance(model, dict) and "id" in model
    }
    config["baseUrl"] = base_url.strip()
    config.setdefault("api", LOCAL_API)
    config.setdefault("apiKey", LOCAL_PLACEHOLDER_KEY)
    config["models"] = [
        previous.get(model_id, {"id": model_id}) for model_id in model_ids
    ]
    providers[provider_id] = config
    data["providers"] = providers
    _save(models_path(), data, private=False)


def remove_local_provider(provider_id: str) -> bool:
    """Drop a custom provider from models.json; True when one was there."""
    data = _load(models_path())
    providers = data.get("providers")
    if not isinstance(providers, dict) or provider_id not in providers:
        return False
    del providers[provider_id]
    data["providers"] = providers
    _save(models_path(), data, private=False)
    return True


# ---- Model scope (settings.json) ------------------------------------------
#
# pi already has a way to narrow the models a session offers: `enabledModels`
# in settings.json, a list of patterns (the `--models` flag in file form). pi
# resolves it at session start for its own picker and Ctrl+P cycling, but the
# RPC `get_available_models` answers with the whole catalogue regardless — so
# the scope is stored where pi reads it and applied here as well.

ENABLED_MODELS = "enabledModels"

# A pattern may pin a thinking level with a trailing `:level` (`anthropic/*:high`).
# It says nothing about which models are in scope, so it is stripped before
# matching. (session_controller keeps the same list for the effort picker.)
_THINKING_LEVELS = frozenset(
    {"off", "minimal", "low", "medium", "high", "xhigh", "max"}
)

_GLOB_CHARS = "*?["


def load_settings() -> dict:
    return _load(settings_path())


def enabled_models() -> list[str]:
    """The configured scope, or `[]` when there is none — which is pi's own
    reading of an absent `enabledModels`: every model is in scope."""
    patterns = load_settings().get(ENABLED_MODELS)
    if not isinstance(patterns, list):
        return []
    return [pattern for pattern in patterns if isinstance(pattern, str) and pattern]


def save_enabled_models(patterns: list[str]) -> None:
    """Set the scope, leaving every other pi setting alone. An empty list
    removes the key rather than writing `[]`: to pi they mean the same thing,
    and the absent key is the one a later pi release will keep meaning."""
    settings = load_settings()
    if patterns:
        settings[ENABLED_MODELS] = list(patterns)
    else:
        settings.pop(ENABLED_MODELS, None)
    _save(settings_path(), settings, private=False)


def model_ref(model: dict) -> str:
    """A model as pi's canonical `provider/id` reference — what we write into
    `enabledModels`, since it names one model and nothing else."""
    return f"{model.get('provider', '')}/{model.get('id', '')}"


def matches_pattern(model: dict, pattern: str) -> bool:
    """Whether one `enabledModels` pattern covers this model, following pi's
    model-resolver: a glob is matched against `provider/id` and against the
    bare id, anything else is an exact reference, then a substring of the id or
    the display name.

    Where pi's resolver narrows an ambiguous substring to a single best model,
    this keeps every match. The difference only ever shows a model the user's
    own pattern named — the safe direction for a filter whose job is hiding."""
    pattern = pattern.strip()
    head, _, tail = pattern.rpartition(":")
    if head and tail in _THINKING_LEVELS:
        pattern = head.strip()
    if not pattern:
        return False
    needle = pattern.lower()
    ref = model_ref(model).lower()
    model_id = str(model.get("id") or "").lower()
    name = str(model.get("name") or "").lower()
    if any(char in pattern for char in _GLOB_CHARS):
        return fnmatchcase(ref, needle) or fnmatchcase(model_id, needle)
    if needle in (ref, model_id):
        return True
    return bool(model_id) and (needle in model_id or (bool(name) and needle in name))


def in_scope(models: list[dict], patterns: list[str] | None = None) -> list[dict]:
    """`models` narrowed to the configured scope, in the order pi listed them.
    An empty scope selects everything, as it does for pi."""
    if patterns is None:
        patterns = enabled_models()
    if not patterns:
        return list(models)
    return [
        model
        for model in models
        if any(matches_pattern(model, pattern) for pattern in patterns)
    ]
