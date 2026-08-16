"""Terminal font-size bounds and normalization."""

DEFAULT_SIZE = 13
MIN_SIZE = 8
MAX_SIZE = 32


def clamp(size: int) -> int:
    try:
        size = int(size)
    except (TypeError, ValueError):
        return DEFAULT_SIZE
    return max(MIN_SIZE, min(size, MAX_SIZE))
