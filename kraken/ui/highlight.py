"""Syntax highlighting: the token palette and the lexer lookups around it.

Shared so that a snippet looks the same wherever the app shows code — a fenced
block in the transcript, a file in the diff viewer — rather than each surface
inventing its own colors.

The palette is keyed by pygments token type. Token types resolve through their
parents (`resolve_token` walks them), so an entry for Token.Literal.String also
catches Token.Literal.String.Doc and the rest of the family.
"""

from __future__ import annotations

from pygments.lexers import get_lexer_by_name, get_lexer_for_filename
from pygments.token import Token
from pygments.util import ClassNotFound

# Dark leans on One Dark; light uses darkened One Light values that keep
# contrast on the tinted code background.
TOKEN_COLORS = {
    "dark": {
        Token.Keyword: ("#c678dd", False),
        Token.Keyword.Constant: ("#d19a66", False),
        Token.Operator.Word: ("#c678dd", False),
        Token.Literal.String: ("#98c379", False),
        Token.Literal.Number: ("#d19a66", False),
        Token.Comment: ("#7d818c", True),
        Token.Name.Function: ("#61afef", False),
        Token.Name.Class: ("#e5c07b", False),
        Token.Name.Builtin: ("#56b6c2", False),
        Token.Name.Decorator: ("#e5c07b", False),
        Token.Name.Tag: ("#e06c75", False),
        Token.Name.Attribute: ("#d19a66", False),
    },
    "light": {
        Token.Keyword: ("#96218f", False),
        Token.Keyword.Constant: ("#8a5c00", False),
        Token.Operator.Word: ("#96218f", False),
        Token.Literal.String: ("#3c7d3b", False),
        Token.Literal.Number: ("#8a5c00", False),
        Token.Comment: ("#75786f", True),
        Token.Name.Function: ("#2a5fd3", False),
        Token.Name.Class: ("#9c6d00", False),
        Token.Name.Builtin: ("#077a92", False),
        Token.Name.Decorator: ("#9c6d00", False),
        Token.Name.Tag: ("#a8232e", False),
        Token.Name.Attribute: ("#8a5c00", False),
    },
}

# Lexer lookup is not free; cache per key (None = nothing matched).
_LEXERS: dict[str, object] = {}
_FILENAME_LEXERS: dict[str, object] = {}


def lexer_for(language: str):
    """The lexer for a fence language ("python", "ts"), or None."""
    if language not in _LEXERS:
        try:
            _LEXERS[language] = get_lexer_by_name(language)
        except ClassNotFound:
            _LEXERS[language] = None
    return _LEXERS[language]


def lexer_for_filename(path: str):
    """The lexer for a file's name, or None when nothing claims it. Keyed on the
    whole path, since pygments matches basenames as well as suffixes."""
    if path not in _FILENAME_LEXERS:
        try:
            # The lexer is chosen from the name alone: the content isn't at hand
            # for `guess_lexer`, and a wrong guess colors code as another
            # language, which reads worse than leaving it plain.
            _FILENAME_LEXERS[path] = get_lexer_for_filename(path)
        except ClassNotFound:
            _FILENAME_LEXERS[path] = None
    return _FILENAME_LEXERS[path]


def resolve_token(table: dict, token) -> tuple[str, bool] | None:
    """The (color, italic) a token should paint with, found by walking up to its
    nearest styled ancestor, or None when no ancestor is in the palette."""
    node = token
    while node is not None and node not in table:
        node = node.parent
    return table[node] if node is not None else None
