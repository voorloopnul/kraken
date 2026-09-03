//! Terminal colour themes and the application chrome palette.
//!
//! A theme sets the terminal's default background/foreground and optionally
//! overrides the 16 ANSI colours. Indices 16-255 (colour cube + gray ramp) are
//! generated with the same formulas libghostty uses, so only the named colours
//! differ between themes.

use std::collections::HashMap;

use once_cell::sync::Lazy;

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTheme {
    pub name: &'static str,
    pub background: Rgb,
    pub foreground: Rgb,
    /// 16 ANSI colours, or `None` to keep the built-in palette.
    pub ansi: Option<[Rgb; 16]>,
}

impl TerminalTheme {
    /// Full 256-colour palette, or `None` to use the built-in default.
    pub fn palette256(&self) -> Option<Vec<Rgb>> {
        let ansi = self.ansi?;
        let mut colors: Vec<Rgb> = ansi.to_vec();
        for r in 0..6u16 {
            for g in 0..6u16 {
                for b in 0..6u16 {
                    colors.push((
                        if r == 0 { 0 } else { (r * 40 + 55) as u8 },
                        if g == 0 { 0 } else { (g * 40 + 55) as u8 },
                        if b == 0 { 0 } else { (b * 40 + 55) as u8 },
                    ));
                }
            }
        }
        for i in 0..24u16 {
            let v = (8 + i * 10) as u8;
            colors.push((v, v, v));
        }
        Some(colors)
    }
}

/// The original widget colours; the ANSI palette stays libghostty's built-in
/// (Tomorrow Night).
pub const DARK: TerminalTheme = TerminalTheme {
    name: "dark",
    background: (0x28, 0x2C, 0x34),
    foreground: (0xFF, 0xFF, 0xFF),
    ansi: None,
};

/// One Half Light, on a white ground rather than the palette's own #FAFAFA. The
/// light theme is built on white — the cards, the window and the terminal are
/// one surface (see [`UI_COLORS`]), and everything that is *not* that surface is
/// a shade of warm grey off it.
pub const LIGHT: TerminalTheme = TerminalTheme {
    name: "light",
    background: (0xFF, 0xFF, 0xFF),
    foreground: (0x38, 0x3A, 0x42),
    ansi: Some([
        (0x38, 0x3A, 0x42), // black
        (0xE4, 0x56, 0x49), // red
        (0x50, 0xA1, 0x4F), // green
        (0xC1, 0x84, 0x01), // yellow
        (0x01, 0x84, 0xBC), // blue
        (0xA6, 0x26, 0xA4), // magenta
        (0x09, 0x97, 0xB3), // cyan
        (0xFA, 0xFA, 0xFA), // white
        (0x4F, 0x52, 0x5E), // bright black
        (0xE0, 0x6C, 0x75), // bright red
        (0x98, 0xC3, 0x79), // bright green
        (0xE5, 0xC0, 0x7B), // bright yellow
        (0x61, 0xAF, 0xEF), // bright blue
        (0xC6, 0x78, 0xDD), // bright magenta
        (0x56, 0xB6, 0xC2), // bright cyan
        (0xFF, 0xFF, 0xFF), // bright white
    ]),
};

pub const DEFAULT_THEME: &str = LIGHT.name;

pub fn terminal_theme(name: &str) -> &'static TerminalTheme {
    match name {
        "dark" => &DARK,
        _ => &LIGHT,
    }
}

pub fn hex(rgb: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2)
}

/// Application chrome per theme: window background, cards, and plain text.
///
/// Card backgrounds match the terminal backgrounds so the right panel blends.
/// The sidebar is the exception to the card treatment: it is a surface rather
/// than an object on one, so it carries a background of its own and no border —
/// the line beside it belongs to the dock's divider.
///
/// `accent` is the one blue the app marks a current or active thing in. It is a
/// fill, so it carries `accent_on` for whatever sits on top of it.
/// `accent_soft` is the same blue as a tint, with `accent_text` for the text and
/// glyphs on it.
pub static UI_COLORS: Lazy<HashMap<&'static str, HashMap<&'static str, String>>> =
    Lazy::new(|| {
        let dark: HashMap<&'static str, String> = [
            // Match the terminal/card working surface: the conversation column
            // is width-capped and centred, so its gutters must carry this same
            // colour or they show as darker vertical bands.
            ("window", hex(DARK.background)),
            ("card", hex(DARK.background)),
            ("card_border", "#3a3f4a".into()),
            ("sidebar", "#23252b".into()),
            ("header", "#23252b".into()),
            ("hover", "#2c2e35".into()),
            ("home", "#1f2127".into()),
            ("text", "#c8cad0".into()),
            ("accent", "#4f77d4".into()),
            ("accent_on", "#ffffff".into()),
            ("accent_soft", "#26365e".into()),
            ("accent_text", "#8ab4f8".into()),
        ]
        .into_iter()
        .collect();
        let light: HashMap<&'static str, String> = [
            // The same value as the cards, and written the same way so the two
            // cannot drift.
            ("window", hex(LIGHT.background)),
            ("card", hex(LIGHT.background)),
            ("card_border", "#e6e4e0".into()),
            ("sidebar", "#faf9f7".into()),
            ("header", "#faf9f7".into()),
            // A hover on a chrome strip: one step further from the base than
            // the strip itself, stopping short of the closing hairline.
            ("hover", "#f1efeb".into()),
            // The home screen keeps the cream the rest of the app used to be
            // painted in: one logo on an empty window rather than a working
            // surface.
            ("home", "#faf6ec".into()),
            ("text", "#383a42".into()),
            ("accent", "#496ecf".into()),
            ("accent_on", "#ffffff".into()),
            ("accent_soft", "#dfe6f8".into()),
            ("accent_text", "#496ecf".into()),
        ]
        .into_iter()
        .collect();
        [("dark", dark), ("light", light)].into_iter().collect()
    });

/// One chrome colour, falling back to the light theme for an unknown name and
/// to magenta for an unknown key (which is a bug, and should look like one).
pub fn ui_color(theme: &str, key: &str) -> String {
    let table = UI_COLORS
        .get(theme)
        .unwrap_or_else(|| &UI_COLORS["light"]);
    table
        .get(key)
        .cloned()
        .unwrap_or_else(|| "#ff00ff".to_string())
}

/// The two themes, in the order the toggle steps through them.
pub const THEME_NAMES: [&str; 2] = ["light", "dark"];

/// The other theme, for the toggle.
pub fn other_theme(name: &str) -> &'static str {
    if name == "dark" {
        "light"
    } else {
        "dark"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_keeps_the_builtin_ansi_palette() {
        assert!(DARK.palette256().is_none());
    }

    #[test]
    fn light_palette_is_256_entries() {
        let palette = LIGHT.palette256().expect("light overrides ANSI");
        assert_eq!(palette.len(), 256);
    }

    #[test]
    fn palette_cube_and_ramp_match_libghostty_formulas() {
        let palette = LIGHT.palette256().unwrap();
        // First cube entry is pure black, last is (255, 255, 255).
        assert_eq!(palette[16], (0, 0, 0));
        assert_eq!(palette[231], (255, 255, 255));
        // A middle cube entry: index 16 + 36*r + 6*g + b with r=1, g=2, b=3.
        assert_eq!(palette[16 + 36 + 12 + 3], (95, 135, 175));
        // The gray ramp runs 8, 18, ... 238.
        assert_eq!(palette[232], (8, 8, 8));
        assert_eq!(palette[255], (238, 238, 238));
    }

    #[test]
    fn light_is_the_default_theme() {
        assert_eq!(DEFAULT_THEME, "light");
        assert_eq!(terminal_theme(DEFAULT_THEME).name, "light");
    }

    #[test]
    fn window_and_card_share_the_terminal_background() {
        for name in THEME_NAMES {
            let bg = hex(terminal_theme(name).background);
            assert_eq!(ui_color(name, "window"), bg);
            assert_eq!(ui_color(name, "card"), bg);
        }
    }

    #[test]
    fn every_theme_defines_every_key() {
        let keys: Vec<&str> = UI_COLORS["light"].keys().copied().collect();
        for name in THEME_NAMES {
            for key in &keys {
                assert!(
                    UI_COLORS[name].contains_key(key),
                    "{name} is missing {key}"
                );
            }
        }
    }

    #[test]
    fn toggle_alternates() {
        assert_eq!(other_theme("light"), "dark");
        assert_eq!(other_theme("dark"), "light");
    }
}
