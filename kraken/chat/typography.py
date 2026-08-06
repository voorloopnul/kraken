"""The chat's type scale.

One number is configurable — the transcript's base size, in pixels — and
everything else in the conversation pane is derived from it here: the composer,
the model and effort labels, the attachment chips, the busy row, and the
monospace detail under an expanded tool call. A reader who bumps the base is
asking for a bigger conversation, not for bigger body text sitting in chrome
that stayed put.

Markdown headings need nothing from this module: Qt sizes them with a relative
adjustment over the document's own font, so they follow the base by themselves.
"""

from __future__ import annotations

DEFAULT_SIZE = 13
# Floors and ceilings for the setting. The floor is where the transcript's
# bubbles and code cards still have room for their padding; the ceiling is
# past any plausible reading size and only guards against a bad stored value.
MIN_SIZE = 9
MAX_SIZE = 28

# Qt char formats take points, not pixels. At Qt's assumed 96dpi a pixel is
# 0.75pt, which is the conversion the detail size below goes through.
_PX_TO_PT = 0.75


def clamp(size: int) -> int:
    """A base size within the supported range, whatever the caller had.

    Also the boundary for the value restored from disk, so anything that is
    not a number at all falls back to the default rather than raising on the
    way to a window that has not been built yet."""
    try:
        size = int(size)
    except (TypeError, ValueError):
        return DEFAULT_SIZE
    return max(MIN_SIZE, min(size, MAX_SIZE))


def secondary(base: int) -> int:
    """Chrome around the message text: buttons, chips' labels, the model and
    effort labels, the busy row. One step under the body, never below the
    floor — at the smallest sizes everything collapses onto one size rather
    than shrinking the chrome into illegibility."""
    return max(MIN_SIZE, base - 1)


def caption(base: int) -> int:
    """The smallest text in the pane: attachment chips and a code block's Copy
    button, which sit *on* content and have to stay out of its way."""
    return max(MIN_SIZE, base - 2)


def detail_points(base: int) -> float:
    """Point size for the monospace detail under an expanded tool call, which
    is written into a QTextCharFormat rather than a style sheet."""
    return round(secondary(base) * _PX_TO_PT, 2)
