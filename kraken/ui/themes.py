"""Terminal color themes.

A theme sets the terminal's default background/foreground and optionally
overrides the 16 ANSI colors. Indices 16-255 (color cube + gray ramp) are
generated with the same formulas libghostty uses, so only the named colors
differ between themes.
"""

from __future__ import annotations

from dataclasses import dataclass

Rgb = tuple[int, int, int]


@dataclass(frozen=True)
class TerminalTheme:
    name: str
    background: Rgb
    foreground: Rgb
    # 16 ANSI colors, or None to keep libghostty's built-in palette.
    ansi: tuple[Rgb, ...] | None = None

    def palette256(self) -> list[Rgb] | None:
        """Full 256-color palette, or None to use the built-in default."""
        if self.ansi is None:
            return None
        colors = list(self.ansi)
        for r in range(6):
            for g in range(6):
                for b in range(6):
                    colors.append(
                        (
                            0 if r == 0 else r * 40 + 55,
                            0 if g == 0 else g * 40 + 55,
                            0 if b == 0 else b * 40 + 55,
                        )
                    )
        for i in range(24):
            v = 8 + i * 10
            colors.append((v, v, v))
        return colors


# The original widget colors; ANSI palette stays libghostty's built-in
# (Tomorrow Night).
DARK = TerminalTheme(
    name="dark",
    background=(0x28, 0x2C, 0x34),
    foreground=(0xFF, 0xFF, 0xFF),
)

# One Half Light.
LIGHT = TerminalTheme(
    name="light",
    background=(0xFA, 0xFA, 0xFA),
    foreground=(0x38, 0x3A, 0x42),
    ansi=(
        (0x38, 0x3A, 0x42),  # black
        (0xE4, 0x56, 0x49),  # red
        (0x50, 0xA1, 0x4F),  # green
        (0xC1, 0x84, 0x01),  # yellow
        (0x01, 0x84, 0xBC),  # blue
        (0xA6, 0x26, 0xA4),  # magenta
        (0x09, 0x97, 0xB3),  # cyan
        (0xFA, 0xFA, 0xFA),  # white
        (0x4F, 0x52, 0x5E),  # bright black
        (0xE0, 0x6C, 0x75),  # bright red
        (0x98, 0xC3, 0x79),  # bright green
        (0xE5, 0xC0, 0x7B),  # bright yellow
        (0x61, 0xAF, 0xEF),  # bright blue
        (0xC6, 0x78, 0xDD),  # bright magenta
        (0x56, 0xB6, 0xC2),  # bright cyan
        (0xFF, 0xFF, 0xFF),  # bright white
    ),
)

THEMES: dict[str, TerminalTheme] = {t.name: t for t in (DARK, LIGHT)}
DEFAULT_THEME = LIGHT.name

# Application chrome per theme: window background, cards, and plain text.
# Card backgrounds match the terminal backgrounds so the right panel blends.
#
# The sidebar is the exception to the card treatment: it is a surface rather
# than an object on one, so it carries a background of its own and no border at
# all — the line beside it belongs to the dock's divider. The offset runs the
# same direction in both themes: a shade off the cards it sits beside, never
# far enough to read as a separate window.
UI_COLORS: dict[str, dict[str, str]] = {
    "dark": {
        "window": "#1f2127",
        "card": "#%02X%02X%02X" % DARK.background,
        "card_border": "#3a3f4a",
        "sidebar": "#23252b",
        "text": "#c8cad0",
    },
    "light": {
        "window": "#faf6ec",
        "card": "#%02X%02X%02X" % LIGHT.background,
        "card_border": "#e0e0e0",
        "sidebar": "#f5f4f1",
        "text": "#383a42",
    },
}
