"""Persistent app state in ~/.alpine/: config, preferences, and project
tracking. State is one JSON object; save_state merges partial updates so
independent features can persist their keys without clobbering others."""

from __future__ import annotations

import json
from pathlib import Path

CONFIG_DIR = Path.home() / ".alpine"
_STATE_FILE = CONFIG_DIR / "state.json"


def load_state() -> dict:
    try:
        return json.loads(_STATE_FILE.read_text())
    except (OSError, ValueError):
        return {}


def save_state(**changes) -> None:
    state = load_state()
    state.update(changes)
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    _STATE_FILE.write_text(json.dumps(state, indent=2) + "\n")
