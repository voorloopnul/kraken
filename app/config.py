"""Persistent app state in ~/.kraken/: config, preferences, and project
tracking. State is one JSON object; save_state merges partial updates so
independent features can persist their keys without clobbering others."""

from __future__ import annotations

import json
from pathlib import Path

CONFIG_DIR = Path.home() / ".kraken"
_STATE_FILE = CONFIG_DIR / "state.json"
_LEGACY_STATE_FILE = Path.home() / ".alpine" / "state.json"


def load_state() -> dict:
    # Preserve existing settings on the first run after the rename. New saves
    # are always written to ~/.kraken.
    source = _STATE_FILE if _STATE_FILE.exists() else _LEGACY_STATE_FILE
    try:
        return json.loads(source.read_text())
    except (OSError, ValueError):
        return {}


def save_state(**changes) -> None:
    state = load_state()
    state.update(changes)
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    _STATE_FILE.write_text(json.dumps(state, indent=2) + "\n")
