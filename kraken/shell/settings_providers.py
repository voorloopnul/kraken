"""The Providers settings page: OpenRouter, a local model server, and the
ChatGPT (Codex) plan sign-in.

Every row here edits a file that belongs to pi — `auth.json` for credentials,
`models.json` for custom providers — through `kraken.agent.pi_config`, which
holds the contracts. The page states which file it is writing, because these
are the same files a pi user edits by hand and the two must not surprise each
other.

Codex is the odd one out: its sign-in is an OAuth flow that pi only exposes
through `/login` in its own interactive TUI, with no RPC command and no
headless CLI behind it. So the button here does not sign anyone in — it emits
`codex_signin_requested`, and the window hands the user a terminal already
running the flow. Signing *out* is just dropping the stored credential, which
is ours to do.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QLabel, QLineEdit

from kraken.agent import pi_config
from kraken.shell.settings_page import Page, chip, label, row_of


def _placeholder_url() -> str:
    return pi_config.LOCAL_BASE_URL


def _short_path(path) -> str:
    """The path as the user would write it: `~/.pi/agent/auth.json`."""
    try:
        return f"~/{path.relative_to(Path.home())}"
    except ValueError:
        return str(path)


class ProvidersPage(Page):
    """Reads its state from pi's config on construction and after every write,
    so what it shows is what is on disk rather than what was typed."""

    # The user asked to sign in to their ChatGPT plan; only pi's interactive
    # flow can do it, so the window is asked to start one.
    codex_signin_requested = Signal()

    def __init__(self):
        super().__init__("Settings", "Providers")
        self.add_section("Credentials")
        self.add_note(
            f"Keys are stored in pi's own {_short_path(pi_config.auth_path())}, "
            "created 0600, and custom servers in models.json beside it. Both "
            "files stay editable by hand; nothing here rewrites what it did "
            "not set."
        )

        self._build_openrouter()
        self._build_local()
        self._build_codex()
        # finish() is the dialog's to call, once, for every page it hosts.
        self.refresh()

    # ---- OpenRouter ------------------------------------------------------

    def _build_openrouter(self) -> None:
        self.add_group("OpenRouter")
        self._openrouter_status = label("", "description", wrap=True)
        self._openrouter_key = QLineEdit()
        self._openrouter_key.setEchoMode(QLineEdit.EchoMode.Password)
        self._openrouter_key.setPlaceholderText("sk-or-v1-…")
        self._openrouter_key.returnPressed.connect(self._save_openrouter)
        self.add_row(
            "API Key",
            "Billed from your OpenRouter credits. Pasted here it is written to "
            f"auth.json, which pi prefers over ${pi_config.OPENROUTER_ENV}.",
            self._openrouter_key,
        )
        save = chip("Save")
        save.clicked.connect(self._save_openrouter)
        self._openrouter_forget = chip("Forget")
        self._openrouter_forget.clicked.connect(self._forget_openrouter)
        self.add_actions(save, self._openrouter_forget)
        self.add_status(self._openrouter_status)

    def _save_openrouter(self) -> None:
        key = self._openrouter_key.text().strip()
        if not key:
            self._openrouter_status.setText("Enter a key first.")
            return
        pi_config.save_api_key(pi_config.OPENROUTER, key)
        # The field is cleared rather than left holding the key: it is stored
        # now, and a password field that stays full invites a second save of
        # something already saved.
        self._openrouter_key.clear()
        self.refresh()

    def _forget_openrouter(self) -> None:
        pi_config.remove_credential(pi_config.OPENROUTER)
        self.refresh()

    # ---- Local models ----------------------------------------------------

    def _build_local(self) -> None:
        self.add_group("Local Models")
        self._local_status = label("", "description", wrap=True)
        self._local_id = QLineEdit()
        self._local_id.setPlaceholderText(pi_config.LOCAL_PROVIDER_ID)
        self.add_row(
            "Provider ID",
            "The name this server appears under in the model picker.",
            self._local_id,
        )
        self._local_url = QLineEdit()
        self._local_url.setPlaceholderText(_placeholder_url())
        self.add_row(
            "Server URL",
            "An OpenAI-compatible endpoint — llama.cpp, Ollama, LM Studio, "
            "vLLM. Include the /v1.",
            self._local_url,
        )
        self._local_models = QLineEdit()
        self._local_models.setPlaceholderText("qwen2.5-coder:7b, llama3.1:8b")
        self.add_row(
            "Models",
            "Model ids the server exposes, comma separated. Anything else "
            "already configured for these models is kept.",
            self._local_models,
        )
        save = chip("Save")
        save.clicked.connect(self._save_local)
        self._local_remove = chip("Remove")
        self._local_remove.clicked.connect(self._remove_local)
        self.add_actions(save, self._local_remove)
        self.add_status(self._local_status)

    def _save_local(self) -> None:
        provider_id = self._local_id.text().strip() or pi_config.LOCAL_PROVIDER_ID
        url = self._local_url.text().strip() or _placeholder_url()
        models = [
            model.strip()
            for model in self._local_models.text().split(",")
            if model.strip()
        ]
        if not models:
            self._local_status.setText(
                "List at least one model id — pi only offers models a provider "
                "declares."
            )
            return
        pi_config.save_local_provider(provider_id, url, models)
        self.refresh()

    def _remove_local(self) -> None:
        provider_id = self._local_id.text().strip()
        if provider_id:
            pi_config.remove_local_provider(provider_id)
        self.refresh()

    # ---- Codex -----------------------------------------------------------

    def _build_codex(self) -> None:
        self.add_group("ChatGPT (Codex)")
        self._codex_status = label("", "description", wrap=True)
        signin = chip("Sign in…")
        signin.clicked.connect(self.codex_signin_requested)
        self._codex_signout = chip("Sign out")
        self._codex_signout.clicked.connect(self._signout_codex)
        self.add_row(
            "ChatGPT Plan",
            "Uses a ChatGPT Plus or Pro subscription. The sign-in is pi's own "
            "OAuth flow, so this opens a terminal running it; the token it "
            "stores is refreshed by pi from then on.",
            row_of(signin, self._codex_signout),
        )
        self.add_status(self._codex_status)

    def _signout_codex(self) -> None:
        pi_config.remove_credential(pi_config.CODEX)
        self.refresh()

    # ---- State -----------------------------------------------------------

    def refresh(self) -> None:
        """Re-read pi's config and restate every group from it."""
        self._refresh_openrouter()
        self._refresh_local()
        self._refresh_codex()

    def _refresh_openrouter(self) -> None:
        source = pi_config.api_key_source(
            pi_config.OPENROUTER, pi_config.OPENROUTER_ENV
        )
        stored = source == "auth.json"
        self._openrouter_forget.setEnabled(stored)
        if stored:
            self._openrouter_status.setText("Key saved in auth.json.")
        elif source == "env":
            self._openrouter_status.setText(
                f"No key here, but ${pi_config.OPENROUTER_ENV} is set in the "
                "environment and pi will use that."
            )
        else:
            self._openrouter_status.setText("No key configured.")

    def _refresh_local(self) -> None:
        configured = pi_config.local_providers()
        # Prefill from what is already there when it is unambiguous, so an
        # edit changes that server rather than quietly declaring a second one.
        if not self._local_id.text().strip() and len(configured) == 1:
            provider_id, config = next(iter(configured.items()))
            self._local_id.setText(provider_id)
            self._local_url.setText(str(config.get("baseUrl") or ""))
            self._local_models.setText(
                ", ".join(
                    str(model.get("id"))
                    for model in config.get("models", [])
                    if isinstance(model, dict) and model.get("id")
                )
            )
        for field in (self._local_url, self._local_models):
            field.setCursorPosition(0)
        current = self._local_id.text().strip()
        self._local_remove.setEnabled(bool(current) and current in configured)
        if configured:
            names = ", ".join(sorted(configured))
            self._local_status.setText(f"Configured in models.json: {names}.")
        else:
            self._local_status.setText("No local server configured.")

    def _refresh_codex(self) -> None:
        kind = pi_config.credential_kind(pi_config.CODEX)
        self._codex_signout.setEnabled(kind is not None)
        if kind is None:
            self._codex_status.setText("Not signed in.")
            return
        if kind != "oauth":
            # An API key against the Codex provider, put there by hand. Saying
            # "Not signed in." beside a live Sign out would offer to delete a
            # credential the page had just denied having.
            self._codex_status.setText(
                "An API key is configured for this provider, rather than a "
                "ChatGPT plan sign-in. Sign out removes it."
            )
            return
        expires = pi_config.oauth_expiry(pi_config.CODEX)
        if expires is None:
            self._codex_status.setText("Signed in.")
            return
        when = datetime.fromtimestamp(expires / 1000).strftime("%d %b %H:%M")
        # An expired token is not a signed-out one: pi refreshes it on use, so
        # this reads as the age of the sign-in rather than a warning.
        self._codex_status.setText(f"Signed in. Token valid until {when}.")
