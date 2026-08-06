"""The Providers page against a redirected pi config directory.

The page is the only thing in the app that writes credentials, so what is
pinned here is that it writes what pi expects, reports what is actually on
disk, and never puts a stored secret back on screen.
"""

import json

import pytest

from kraken.agent import pi_config
from kraken.shell.settings_providers import ProvidersPage


@pytest.fixture
def page(tmp_path, monkeypatch, qapp):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    page = ProvidersPage()
    yield page
    page.deleteLater()


def test_saving_an_openrouter_key_writes_it_and_clears_the_field(page):
    page._openrouter_key.setText("sk-or-v1-secret")
    page._save_openrouter()
    assert pi_config.credential_kind(pi_config.OPENROUTER) == "api_key"
    # The key is on disk now; leaving it in the box would only invite saving
    # it twice, and it is the one thing on this page worth not displaying.
    assert page._openrouter_key.text() == ""
    assert "auth.json" in page._openrouter_status.text()


def test_an_empty_key_is_refused_rather_than_stored(page):
    page._save_openrouter()
    assert pi_config.load_auth() == {}
    assert "Enter a key" in page._openrouter_status.text()


def test_forgetting_a_key_removes_it(page):
    page._openrouter_key.setText("sk-or-v1-secret")
    page._save_openrouter()
    page._forget_openrouter()
    assert pi_config.credential_kind(pi_config.OPENROUTER) is None
    assert not page._openrouter_forget.isEnabled()


def test_local_provider_round_trips_through_models_json(page):
    page._local_id.setText("locallm")
    page._local_url.setText("http://localhost:11434/v1")
    page._local_models.setText("llama3.1:8b, qwen2.5-coder:7b")
    page._save_local()
    provider = json.loads(pi_config.models_path().read_text())["providers"]["locallm"]
    assert provider["baseUrl"] == "http://localhost:11434/v1"
    assert [model["id"] for model in provider["models"]] == [
        "llama3.1:8b",
        "qwen2.5-coder:7b",
    ]
    assert "locallm" in page._local_status.text()
    assert page._local_remove.isEnabled()


def test_a_local_provider_without_models_is_refused(page):
    page._local_id.setText("locallm")
    page._local_url.setText("http://localhost:11434/v1")
    page._save_local()
    assert pi_config.load_providers() == {}
    assert "at least one model" in page._local_status.text()


def test_an_existing_local_server_is_prefilled_for_editing(tmp_path, monkeypatch, qapp):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    (tmp_path / "models.json").write_text(
        json.dumps(
            {
                "providers": {
                    "llamacpp": {
                        "baseUrl": "http://localhost:8080/v1",
                        "models": [{"id": "qwen"}],
                    }
                }
            }
        )
    )
    page = ProvidersPage()
    # Editing the server that is already configured, rather than silently
    # declaring a second one beside it.
    assert page._local_id.text() == "llamacpp"
    assert page._local_url.text() == "http://localhost:8080/v1"
    assert page._local_models.text() == "qwen"
    page.deleteLater()


def test_codex_reports_sign_in_state_and_can_sign_out(page):
    assert page._codex_status.text() == "Not signed in."
    assert not page._codex_signout.isEnabled()
    pi_config.auth_path().write_text(
        json.dumps({pi_config.CODEX: {"type": "oauth", "expires": 1786698578488}})
    )
    page.refresh()
    assert page._codex_status.text().startswith("Signed in.")
    assert page._codex_signout.isEnabled()
    page._signout_codex()
    assert pi_config.credential_kind(pi_config.CODEX) is None
    assert page._codex_status.text() == "Not signed in."


def test_signing_in_is_delegated_rather_than_attempted(page):
    # pi's OAuth flow only exists inside its own interactive UI, so the page
    # asks the window for a terminal instead of pretending it can log in.
    asked = []
    page.codex_signin_requested.connect(lambda: asked.append(True))
    page.codex_signin_requested.emit()
    assert asked == [True]
    assert pi_config.credential_kind(pi_config.CODEX) is None


def test_an_api_key_for_codex_is_not_reported_as_signed_out(page):
    # Put there by hand: pi accepts an API key for this provider too. The row
    # used to say "Not signed in." beside an enabled Sign out, offering to
    # delete a credential it had just denied having.
    pi_config.save_api_key(pi_config.CODEX, "sk-codex-by-hand")
    page.refresh()
    assert page._codex_signout.isEnabled()
    assert "API key" in page._codex_status.text()
    page._signout_codex()
    assert pi_config.credential_kind(pi_config.CODEX) is None
    assert page._codex_status.text() == "Not signed in."
