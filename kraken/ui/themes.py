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

# One Half Light, on a white ground rather than the palette's own #FAFAFA. The
# light theme is built on white — the cards, the window, and the terminal are
# one surface (see UI_COLORS), and everything that is *not* that surface is a
# shade of warm grey off it. A near-white ground gave the chrome nothing to be
# a shade off of.
LIGHT = TerminalTheme(
    name="light",
    background=(0xFF, 0xFF, 0xFF),
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
#
# Every panel's top strip — a tab row, or the plain row the drag grip rides in —
# is painted in "header": the same shade as the sidebar, because it is the same
# kind of thing. Chrome that frames content sits a step off the surface the
# content itself is on, whichever edge it runs along. The window's own title bar
# is the exception: it spans the whole window rather than running along one edge
# of the content, so it stays on the base surface and is closed with a hairline.
#
# "accent" is the one blue the app marks a current or active thing in: a tool
# button for a panel that is open, the selected tab, a focused field, the drop
# indicator. It is a fill, so it carries "accent_on" for whatever sits on top of
# it. "accent_soft" is the same blue as a tint, for a mark that should not shout
# (a tab pill), with "accent_text" for the text and glyphs on it — in the dark
# theme that has to be lighter than the fill, which is why the two differ.
UI_COLORS: dict[str, dict[str, str]] = {
    "dark": {
        # Match the terminal/card working surface. The conversation column is
        # width-capped and centered, so its surrounding application gutters
        # must carry this same color or they show as darker vertical bands.
        "window": "#%02X%02X%02X" % DARK.background,
        "card": "#%02X%02X%02X" % DARK.background,
        "card_border": "#3a3f4a",
        "sidebar": "#23252b",
        "header": "#23252b",
        "hover": "#2c2e35",
        "home": "#1f2127",
        "text": "#c8cad0",
        "accent": "#4f77d4",
        "accent_on": "#ffffff",
        "accent_soft": "#26365e",
        "accent_text": "#8ab4f8",
    },
    "light": {
        # The same value as the cards, and written the same way so the two
        # cannot drift: with the panels running edge to edge there is no gutter
        # left for a window colour to show in, and anywhere it still surfaces
        # (behind the conversation, under a hidden panel) it should read as
        # more of the same surface rather than as a second one.
        "window": "#%02X%02X%02X" % LIGHT.background,
        "card": "#%02X%02X%02X" % LIGHT.background,
        "card_border": "#e6e4e0",
        "sidebar": "#faf9f7",
        "header": "#faf9f7",
        # A hover on a chrome strip: one step further from the base than the
        # strip itself, stopping short of the hairline that closes it.
        "hover": "#f1efeb",
        # The home screen keeps the cream the rest of the app used to be
        # painted in: it is one logo on an empty window rather than a working
        # surface, and it is the one place the warmth was worth keeping.
        "home": "#faf6ec",
        "text": "#383a42",
        "accent": "#496ecf",
        "accent_on": "#ffffff",
        "accent_soft": "#dfe6f8",
        "accent_text": "#496ecf",
    },
}
