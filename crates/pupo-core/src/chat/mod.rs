//! The chat pipeline: turning agent output into something readable.
//!
//! [`transcript`] holds the semantic model — an ordered list of blocks that the
//! UI repaints from — while [`formatting`] turns Pi's message payloads into the
//! short strings those blocks carry, [`markdown`] renders an assistant reply
//! into the HTML subset QML's rich text understands, and [`highlight`] colours
//! the code inside it.
//!
//! Nothing here paints; the model is kept whole so a theme switch is a repaint
//! from the same blocks rather than a re-read of text that was already styled
//! for the colours it is leaving.

use std::collections::HashMap;

use once_cell::sync::Lazy;

pub mod formatting;
pub mod highlight;
pub mod markdown;
pub mod transcript;

/// Colour roles for the conversation pane, per theme.
///
/// Separate from [`crate::theme::UI_COLORS`], which paints the app's chrome:
/// these are the roles *inside* a transcript, and several of them (the muted
/// reasoning text, the tinted inline-code chip) exist only here. Light values
/// are picked for contrast on the light window background: body text near
/// black, secondary text dark enough to stay comfortably readable while still
/// reading as secondary.
pub static PALETTE: Lazy<HashMap<&'static str, HashMap<&'static str, &'static str>>> =
    Lazy::new(|| {
        let dark: HashMap<&'static str, &'static str> = [
            ("text", "#d6d8dd"),
            ("dim", "#7a7d85"),
            ("error", "#e06c75"),
            ("thinking_label", "#9a9da5"),
            ("thinking_text", "#7a7d85"),
            ("tool_detail", "#9a9da5"),
            ("tool_bg", "#17181d"),
            ("tool_border", "#2c2e35"),
            ("code_bg", "#17181d"),
            ("code_border", "#2c2e35"),
            ("user_bg", "#26282e"),
            ("user_border", "#33353c"),
            ("link", "#61afef"),
            ("inline_bg", "#3a3f4a"),
            ("inline_text", "#d19a66"),
        ]
        .into_iter()
        .collect();
        let light: HashMap<&'static str, &'static str> = [
            ("text", "#1a1c21"),
            ("dim", "#5f6269"),
            ("error", "#a8232e"),
            ("thinking_label", "#8e8b86"),
            ("thinking_text", "#b6b3ae"),
            ("tool_detail", "#8e8b86"),
            ("tool_bg", "#f5f3ef"),
            ("tool_border", "#e1ded8"),
            ("code_bg", "#f4f4f5"),
            ("code_border", "#e0e0e0"),
            ("user_bg", "#f1efea"),
            ("user_border", "#e1ded8"),
            ("link", "#02669c"),
            ("inline_bg", "#f4f4f5"),
            ("inline_text", "#8a5c00"),
        ]
        .into_iter()
        .collect();
        [("dark", dark), ("light", light)].into_iter().collect()
    });

/// One transcript colour, falling back to the light theme for an unknown name
/// and to magenta for an unknown key (which is a bug, and should look like one).
pub fn color(theme: &str, key: &str) -> &'static str {
    let table = PALETTE.get(theme).unwrap_or(&PALETTE["light"]);
    table.get(key).copied().unwrap_or("#ff00ff")
}

/// The monospace stack every code surface in the pane asks for. The bundled
/// family first, then whatever the system calls monospace — a code span that
/// falls back to the proportional body font lands visibly off-baseline in the
/// middle of its own sentence.
pub const MONO_STACK: &str = "'JetBrains Mono',monospace";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_user_bubble_uses_the_mockup_colors() {
        assert_eq!(color("light", "user_bg"), "#f1efea");
        assert_eq!(color("light", "user_border"), "#e1ded8");
    }

    #[test]
    fn light_output_card_uses_the_mockup_colors() {
        assert_eq!(color("light", "tool_bg"), "#f5f3ef");
        assert_eq!(color("light", "tool_border"), "#e1ded8");
        assert_eq!(color("light", "tool_detail"), "#8e8b86");
    }

    #[test]
    fn the_thinking_label_is_stronger_than_its_muted_summary() {
        assert_eq!(color("light", "thinking_label"), "#8e8b86");
        assert_eq!(color("light", "thinking_text"), "#b6b3ae");
    }

    #[test]
    fn every_theme_defines_every_role() {
        let keys: Vec<&str> = PALETTE["light"].keys().copied().collect();
        for theme in ["light", "dark"] {
            for key in &keys {
                assert!(PALETTE[theme].contains_key(key), "{theme} is missing {key}");
            }
        }
    }

    #[test]
    fn an_unknown_role_is_loud_rather_than_silent() {
        assert_eq!(color("light", "nonsense"), "#ff00ff");
        // An unknown theme still answers, from the light table.
        assert_eq!(color("sepia", "text"), color("light", "text"));
    }
}
