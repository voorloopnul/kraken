"""Read Pi agent sessions (~/.pi/agent/sessions) for a project folder.

Each session is a JSONL file whose first line is a header like
{"type": "session", "id": ..., "timestamp": ..., "cwd": ...} followed by
event lines ({"type": "message", ...} among others). Session files are
grouped in one directory per project, named after a munged cwd; rather than
reproducing the munging, directories are matched by the header's cwd.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from kraken import config

SESSIONS_ROOT = Path.home() / ".pi" / "agent" / "sessions"


@dataclass(frozen=True)
class PiSession:
    path: Path
    session_id: str
    started: datetime  # local time
    title: str  # first user message, or "(empty session)"
    message_count: int


def _read_header(path: Path) -> dict | None:
    try:
        with open(path, encoding="utf-8") as f:
            header = json.loads(f.readline())
    except (OSError, json.JSONDecodeError):
        return None
    return header if header.get("type") == "session" else None


def _load_session(path: Path, header: dict) -> PiSession:
    title = ""
    count = 0
    try:
        with open(path, encoding="utf-8") as f:
            next(f, None)  # header
            for line in f:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if event.get("type") != "message":
                    continue
                message = event.get("message", {})
                if message.get("role") not in ("user", "assistant"):
                    continue
                count += 1
                if not title and message.get("role") == "user":
                    text = " ".join(
                        part.get("text", "")
                        for part in message.get("content", [])
                        if isinstance(part, dict) and part.get("type") == "text"
                    ).strip()
                    title = " ".join(text.split())
    except OSError:
        pass

    started = datetime.now()
    try:
        started = datetime.fromisoformat(header["timestamp"]).astimezone()
    except (KeyError, ValueError):
        pass
    if len(title) > 60:
        title = title[:59] + "…"
    return PiSession(
        path=path,
        session_id=header.get("id", path.stem),
        started=started,
        title=title or "(empty session)",
        message_count=count,
    )


def _archived_ids() -> set[str]:
    return set(config.load_state().get("archived_sessions", []))


def archive_session(session_id: str) -> None:
    """Hide a session from the history without deleting its file on disk."""
    archived = _archived_ids()
    archived.add(session_id)
    config.save_state(archived_sessions=sorted(archived))


def delete_session(path: Path | str) -> None:
    """Permanently remove a session's file from disk."""
    try:
        Path(path).unlink()
    except OSError:
        pass


def sessions_for(cwd: str, root: Path | None = None) -> list[PiSession]:
    """All Pi sessions recorded for the project folder `cwd`, newest first.
    Sessions the user has archived are omitted."""
    root = root or SESSIONS_ROOT
    target = str(Path(cwd).expanduser().resolve())
    archived = _archived_ids()
    sessions: list[PiSession] = []
    if not root.is_dir():
        return sessions
    for directory in root.iterdir():
        if not directory.is_dir():
            continue
        for path in directory.glob("*.jsonl"):
            header = _read_header(path)
            if header is None or header.get("cwd") != target:
                continue
            if header.get("id", path.stem) in archived:
                continue
            sessions.append(_load_session(path, header))
    sessions.sort(key=lambda s: s.started, reverse=True)
    return sessions
