"""Writing pi's own config files.

These are pi's files, not ours: the tests below pin the parts of the contract
that a careless write would break — the 0600 on auth.json, the credentials and
provider settings we did not set surviving a save, and the escaping that keeps
a pasted key from being read as a shell command.

Every test redirects PI_CODING_AGENT_DIR, which is pi's own override, so
nothing here can reach the real ~/.pi.
"""

import json
import os
import stat

import pytest

from kraken.agent import pi_config


@pytest.fixture
def agent_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    return tmp_path


def read(path):
    return json.loads(path.read_text())


def test_the_env_override_is_honoured(agent_dir):
    assert pi_config.agent_dir() == agent_dir
    assert pi_config.auth_path() == agent_dir / "auth.json"


def test_auth_file_is_private(agent_dir):
    pi_config.save_api_key(pi_config.OPENROUTER, "sk-or-v1-secret")
    mode = stat.S_IMODE(os.stat(pi_config.auth_path()).st_mode)
    assert mode == 0o600
    assert read(pi_config.auth_path())[pi_config.OPENROUTER] == {
        "type": "api_key",
        "key": "sk-or-v1-secret",
    }


def test_saving_a_key_leaves_other_credentials_alone(agent_dir):
    # An OAuth record pi wrote and refreshes itself.
    pi_config.auth_path().write_text(
        json.dumps({"openai-codex": {"type": "oauth", "access": "t", "expires": 1}})
    )
    pi_config.save_api_key(pi_config.OPENROUTER, "sk-or-v1-x")
    auth = read(pi_config.auth_path())
    assert auth["openai-codex"] == {"type": "oauth", "access": "t", "expires": 1}
    assert auth[pi_config.OPENROUTER]["type"] == "api_key"


def test_a_pasted_key_is_stored_literally(agent_dir):
    # pi resolves "!cmd" as a shell command and "$NAME" from the environment,
    # so a key containing either has to be escaped on the way in.
    pi_config.save_api_key("openai", "!rm -rf /")
    pi_config.save_api_key("anthropic", "sk-$HOME-1")
    auth = read(pi_config.auth_path())
    assert auth["openai"]["key"] == "$!rm -rf /"
    assert auth["anthropic"]["key"] == "sk-$$HOME-1"


def test_credential_state_is_reported_without_reading_the_secret(agent_dir):
    assert pi_config.credential_kind(pi_config.OPENROUTER) is None
    assert (
        pi_config.api_key_source(pi_config.OPENROUTER, pi_config.OPENROUTER_ENV)
        == "none"
    )
    pi_config.save_api_key(pi_config.OPENROUTER, "sk-or-v1-x")
    assert pi_config.credential_kind(pi_config.OPENROUTER) == "api_key"
    assert (
        pi_config.api_key_source(pi_config.OPENROUTER, pi_config.OPENROUTER_ENV)
        == "auth.json"
    )
    assert pi_config.remove_credential(pi_config.OPENROUTER) is True
    assert pi_config.remove_credential(pi_config.OPENROUTER) is False


def test_the_env_var_is_reported_only_as_a_fallback(agent_dir, monkeypatch):
    monkeypatch.setenv(pi_config.OPENROUTER_ENV, "sk-or-v1-from-env")
    assert (
        pi_config.api_key_source(pi_config.OPENROUTER, pi_config.OPENROUTER_ENV)
        == "env"
    )
    # auth.json wins in pi's own resolution order, so it must win here too.
    pi_config.save_api_key(pi_config.OPENROUTER, "sk-or-v1-stored")
    assert (
        pi_config.api_key_source(pi_config.OPENROUTER, pi_config.OPENROUTER_ENV)
        == "auth.json"
    )


def test_oauth_expiry_is_read_only_from_an_oauth_record(agent_dir):
    pi_config.auth_path().write_text(
        json.dumps({pi_config.CODEX: {"type": "oauth", "expires": 1786698578488}})
    )
    assert pi_config.oauth_expiry(pi_config.CODEX) == 1786698578488
    pi_config.save_api_key(pi_config.CODEX, "not-an-oauth-record")
    assert pi_config.oauth_expiry(pi_config.CODEX) is None


def test_saving_a_local_provider_keeps_what_it_did_not_set(agent_dir):
    # A provider configured by hand: a compat flag the server needs, a real
    # key, and a model carrying its own display name.
    pi_config.models_path().write_text(
        json.dumps(
            {
                "providers": {
                    "llamacpp": {
                        "baseUrl": "http://localhost:8080/v1",
                        "api": "openai-completions",
                        "apiKey": "hand-written",
                        "compat": {"supportsDeveloperRole": False},
                        "models": [{"id": "qwen", "name": "Qwen 3", "input": ["text"]}],
                    }
                },
                "somethingElse": {"kept": True},
            }
        )
    )
    pi_config.save_local_provider(
        "llamacpp", "http://localhost:9090/v1", ["qwen", "llama3.1:8b"]
    )
    data = read(pi_config.models_path())
    provider = data["providers"]["llamacpp"]
    assert provider["baseUrl"] == "http://localhost:9090/v1"
    assert provider["apiKey"] == "hand-written"
    assert provider["compat"] == {"supportsDeveloperRole": False}
    # The surviving model keeps its own fields; the new one is just an id.
    assert provider["models"] == [
        {"id": "qwen", "name": "Qwen 3", "input": ["text"]},
        {"id": "llama3.1:8b"},
    ]
    assert data["somethingElse"] == {"kept": True}


def test_a_new_local_provider_gets_the_defaults_pi_needs(agent_dir):
    pi_config.save_local_provider("locallm", "http://localhost:11434/v1", ["llama3"])
    provider = read(pi_config.models_path())["providers"]["locallm"]
    assert provider["api"] == pi_config.LOCAL_API
    # pi hides models whose provider has no auth at all, so a keyless local
    # server still needs the placeholder.
    assert provider["apiKey"] == pi_config.LOCAL_PLACEHOLDER_KEY


def test_only_local_providers_are_offered_for_editing(agent_dir):
    pi_config.models_path().write_text(
        json.dumps(
            {
                "providers": {
                    "local": {"baseUrl": "http://127.0.0.1:8080/v1"},
                    "some-proxy": {"baseUrl": "https://api.example.com/v1"},
                }
            }
        )
    )
    assert set(pi_config.local_providers()) == {"local"}
    assert pi_config.remove_local_provider("local") is True
    assert pi_config.remove_local_provider("local") is False
    assert set(read(pi_config.models_path())["providers"]) == {"some-proxy"}


def test_no_scope_means_every_model(agent_dir):
    models = [
        {"provider": "openrouter", "id": "moonshotai/kimi-k3"},
        {"provider": "openai-codex", "id": "gpt-5.5"},
    ]
    assert pi_config.enabled_models() == []
    assert pi_config.in_scope(models) == models


def test_the_scope_is_written_where_pi_reads_it(agent_dir):
    pi_config.settings_path().write_text(json.dumps({"theme": "dark"}))
    pi_config.save_enabled_models(["openrouter/moonshotai/kimi-k3"])
    settings = read(pi_config.settings_path())
    assert settings[pi_config.ENABLED_MODELS] == ["openrouter/moonshotai/kimi-k3"]
    # It is pi's file: every other setting it holds survives the write.
    assert settings["theme"] == "dark"
    # An empty scope is an absent key, not an empty list — to pi they mean the
    # same thing, and only one of them says it plainly.
    pi_config.save_enabled_models([])
    assert pi_config.ENABLED_MODELS not in read(pi_config.settings_path())
    assert read(pi_config.settings_path())["theme"] == "dark"


def test_scope_patterns_follow_pi_s_matching(agent_dir):
    opus = {"provider": "anthropic", "id": "claude-opus-4-8", "name": "Claude Opus"}
    kimi = {"provider": "openrouter", "id": "moonshotai/kimi-k3", "name": "Kimi K3"}
    # A canonical provider/id reference, which is what the settings page writes.
    assert pi_config.matches_pattern(opus, "anthropic/claude-opus-4-8")
    assert not pi_config.matches_pattern(kimi, "anthropic/claude-opus-4-8")
    # A bare id, a glob against either form, and a substring of id or name.
    assert pi_config.matches_pattern(opus, "claude-opus-4-8")
    assert pi_config.matches_pattern(opus, "claude-*")
    assert pi_config.matches_pattern(kimi, "openrouter/*")
    assert pi_config.matches_pattern(opus, "opus")
    assert pi_config.matches_pattern(kimi, "Kimi")
    # A pinned thinking level says nothing about which models are in scope.
    assert pi_config.matches_pattern(opus, "anthropic/*:high")
    assert pi_config.matches_pattern(opus, "claude-opus-4-8:xhigh")
    assert not pi_config.matches_pattern(kimi, "anthropic/*:high")


def test_a_stored_scope_hides_the_models_outside_it(agent_dir):
    models = [
        {"provider": "anthropic", "id": "claude-opus-4-8"},
        {"provider": "openrouter", "id": "moonshotai/kimi-k3"},
        {"provider": "openai-codex", "id": "gpt-5.5"},
    ]
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["anthropic/claude-opus-4-8", "gpt-5.5"]})
    )
    assert [model["id"] for model in pi_config.in_scope(models)] == [
        "claude-opus-4-8",
        "gpt-5.5",
    ]


def test_a_corrupt_file_reads_as_absent(agent_dir):
    pi_config.auth_path().write_text("{ not json")
    assert pi_config.load_auth() == {}
    assert pi_config.credential_kind(pi_config.OPENROUTER) is None


def test_a_lax_auth_file_is_tightened_before_the_key_lands(agent_dir):
    # An auth.json already there at 0644 — hand-edited, or restored from a
    # backup. os.open's mode applies only when it creates the file, so without
    # the explicit tightening the key would be written world-readable first.
    path = pi_config.auth_path()
    path.write_text("{}")
    os.chmod(path, 0o644)
    pi_config.save_api_key(pi_config.OPENROUTER, "sk-or-v1-secret")
    assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
