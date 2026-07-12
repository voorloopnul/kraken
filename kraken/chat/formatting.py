"""Formatting helpers for Pi message dicts.

Pure functions that turn Pi's message/tool payloads into the short strings
the transcript shows. Shared by the ConversationView renderer and the
SessionController that feeds it.
"""

from __future__ import annotations

import json


def error_summary(message: dict) -> str | None:
    """One-line summary of an errored assistant message, or None. Provider
    error payloads can be pages of repeated JSON; one clipped line keeps the
    transcript readable while naming the actual failure."""
    if message.get("role") != "assistant" or message.get("stopReason") != "error":
        return None
    text = " ".join((message.get("errorMessage") or "unknown error").split())
    return text[:299] + "…" if len(text) > 300 else text


def _content_text(content) -> str:
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        return " ".join(
            part.get("text", "")
            for part in content
            if isinstance(part, dict) and part.get("type") == "text"
        ).strip()
    return ""


def args_summary(args) -> str:
    if not isinstance(args, dict):
        return ""
    for key in ("command", "path", "file_path", "pattern", "url"):
        if isinstance(args.get(key), str):
            value = " ".join(args[key].split())
            return value if len(value) <= 80 else value[:79] + "…"
    return ""


def args_detail(args) -> str:
    """Full tool arguments for the expanded view."""
    if not isinstance(args, dict) or not args:
        return ""
    return json.dumps(args, indent=2, ensure_ascii=False)


def _clip(text: str, limit: int = 4000) -> str:
    text = text.strip()
    if len(text) > limit:
        return text[:limit] + "\n… (truncated)"
    return text
