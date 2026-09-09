//! The theme, as QML sees it.
//!
//! One object holds the palette, the two font scales and the icon set, because
//! everything in the interface asks the same three questions of it: what colour
//! is this surface, how big is this text, and where is that glyph. Changing the
//! theme bumps a single notify signal, and every binding in the tree re-reads
//! from here — the widget port had to walk the tree by hand and repaint each
//! surface, which is what let two of them drift apart.
//!
//! The palettes are *properties* holding a map, not methods taking a key, and
//! that is not a style choice: QML records a dependency when a binding reads a
//! property and records nothing at all when it calls a method. A binding written
//! against a `color(key)` method paints correctly once and then never repaints,
//! which is exactly the drift this object exists to prevent.

use std::collections::HashMap;

use kraken_core::theme::{self, UI_COLORS};
use kraken_core::typography::{chat, terminal};
use kraken_core::util::base64;
use qmetaobject::*;
use serde_json::json;

use crate::resources::ICON_SOURCES;

// The three storage fields below are read through their `READ` accessors, which
// the derive generates as Qt metacalls rather than as Rust reads — so the
// compiler sees no reader and says so.
#[allow(dead_code)]
#[derive(QObject, Default)]
pub struct Theme {
    base: qt_base_class!(trait QObject),

    /// "light" or "dark". Everything else here is derived from it.
    name: qt_property!(QString; NOTIFY changed READ get_name WRITE set_name),
    /// The transcript's base size, in pixels.
    chat_font_size: qt_property!(i32; NOTIFY changed READ get_chat_font_size WRITE set_chat_font_size),
    /// The bounds the Settings steppers hold to, so no QML file carries a copy
    /// of the scale's arithmetic.
    chat_font_min: qt_property!(i32; NOTIFY families_changed READ get_chat_font_min),
    chat_font_max: qt_property!(i32; NOTIFY families_changed READ get_chat_font_max),
    terminal_font_min: qt_property!(i32; NOTIFY families_changed READ get_terminal_font_min),
    terminal_font_max: qt_property!(i32; NOTIFY families_changed READ get_terminal_font_max),
    /// Chrome around the message text: buttons, chips, the busy row.
    secondary_font_size: qt_property!(i32; NOTIFY changed READ get_secondary_font_size),
    /// The smallest text in the pane: chips and a code block's Copy button.
    caption_font_size: qt_property!(i32; NOTIFY changed READ get_caption_font_size),
    /// The terminal's size, in points.
    terminal_font_size: qt_property!(i32; NOTIFY changed READ get_terminal_font_size WRITE set_terminal_font_size),
    /// The bundled font families, published by whichever `FontLoader` finished
    /// registering them. JetBrains Mono is the face for the whole interface, so
    /// it looks the same on every machine rather than falling back to whatever
    /// the system happens to have; Roboto covers the few places a mono grid
    /// reads wrong (menus and the History pane, which are prose).
    mono_family: qt_property!(QString; NOTIFY families_changed),
    sans_family: qt_property!(QString; NOTIFY families_changed),

    families_changed: qt_signal!(),

    changed: qt_signal!(),

    /// The chrome palette: "window", "card", "card_border", "sidebar", "header",
    /// "hover", "home", "text", "accent", "accent_on", "accent_soft",
    /// "accent_text".
    colors: qt_property!(QVariantMap; NOTIFY changed READ get_colors),
    /// The transcript's own palette: "text", "dim", "error", "thinking_label",
    /// "thinking_text", "tool_detail", "tool_bg", "tool_border", "code_bg",
    /// "code_border", "user_bg", "user_border", "link", "inline_bg",
    /// "inline_text".
    ///
    /// Separate from [`colors`](Self::colors) because they are separate tables:
    /// the chrome's roles paint the window around the conversation, these paint
    /// the inside of it, and several of these exist nowhere else.
    chat_colors: qt_property!(QVariantMap; NOTIFY changed READ get_chat_colors),
    /// The terminal's own "background"/"foreground", which the cards match.
    terminal_colors: qt_property!(QVariantMap; NOTIFY changed READ get_terminal_colors),

    /// A Lucide glyph in `color`, as a `data:` URL an `Image` can load.
    ///
    /// A method rather than a table, because the colour is the caller's — every
    /// button tints its glyph for its own hover and checked states. A binding on
    /// it still repaints on a theme change: the colour it passes in is itself
    /// read from a property here.
    icon: qt_method!(fn(&self, name: QString, color: QString) -> QString),
    /// Swap to the other theme, and remember the choice.
    toggle: qt_method!(fn(&mut self)),

    theme_name: String,
    chat_size: i32,
    terminal_size: i32,
    icons: HashMap<String, String>,
}

impl Theme {
    pub fn new(theme_name: &str, chat_size: i32, terminal_size: i32) -> Self {
        Self {
            theme_name: theme_name.to_string(),
            chat_size: chat::clamp(chat_size),
            terminal_size: terminal::clamp(terminal_size),
            icons: ICON_SOURCES
                .iter()
                .map(|(name, src)| ((*name).to_string(), (*src).to_string()))
                .collect(),
            ..Default::default()
        }
    }

    fn get_name(&self) -> QString {
        self.theme_name.as_str().into()
    }

    fn set_name(&mut self, name: QString) {
        let name = name.to_string();
        if name == self.theme_name || !UI_COLORS.contains_key(name.as_str()) {
            return;
        }
        self.theme_name = name;
        self.remember();
        self.changed();
    }

    fn get_chat_font_size(&self) -> i32 {
        self.chat_size
    }

    fn get_terminal_font_size(&self) -> i32 {
        self.terminal_size
    }

    fn get_chat_font_min(&self) -> i32 {
        chat::MIN_SIZE
    }

    fn get_chat_font_max(&self) -> i32 {
        chat::MAX_SIZE
    }

    fn get_terminal_font_min(&self) -> i32 {
        terminal::MIN_SIZE
    }

    fn get_terminal_font_max(&self) -> i32 {
        terminal::MAX_SIZE
    }

    fn get_secondary_font_size(&self) -> i32 {
        chat::secondary(self.chat_size)
    }

    fn get_caption_font_size(&self) -> i32 {
        chat::caption(self.chat_size)
    }

    /// Persist the reader's accommodations. All three are settings someone chose
    /// on purpose, and a choice that has to be made again on every launch is not
    /// a setting.
    fn remember(&self) {
        kraken_core::state::save([
            ("theme".to_string(), json!(self.theme_name)),
            ("chat_font_size".to_string(), json!(self.chat_size)),
            ("terminal_font_size".to_string(), json!(self.terminal_size)),
        ]);
    }

    fn set_chat_font_size(&mut self, size: i32) {
        let size = chat::clamp(size);
        if size != self.chat_size {
            self.chat_size = size;
            self.remember();
            self.changed();
        }
    }

    fn set_terminal_font_size(&mut self, size: i32) {
        let size = terminal::clamp(size);
        if size != self.terminal_size {
            self.terminal_size = size;
            self.remember();
            self.changed();
        }
    }

    fn get_colors(&self) -> QVariantMap {
        let table = UI_COLORS
            .get(self.theme_name.as_str())
            .unwrap_or(&UI_COLORS[theme::DEFAULT_THEME]);
        let mut map = QVariantMap::default();
        for (key, value) in table {
            map.insert((*key).into(), QVariant::from(QString::from(value.as_str())));
        }
        map
    }

    fn get_chat_colors(&self) -> QVariantMap {
        let table = kraken_core::chat::PALETTE
            .get(self.theme_name.as_str())
            .unwrap_or(&kraken_core::chat::PALETTE[theme::DEFAULT_THEME]);
        let mut map = QVariantMap::default();
        for (key, value) in table {
            map.insert((*key).into(), QVariant::from(QString::from(*value)));
        }
        map
    }

    fn get_terminal_colors(&self) -> QVariantMap {
        let palette = theme::terminal_theme(&self.theme_name);
        let mut map = QVariantMap::default();
        for (key, rgb) in [
            ("background", palette.background),
            ("foreground", palette.foreground),
        ] {
            map.insert(
                key.into(),
                QVariant::from(QString::from(theme::hex(rgb).as_str())),
            );
        }
        map
    }

    fn icon(&self, name: QString, color: QString) -> QString {
        icon_url(&self.icons, &name.to_string(), &color.to_string())
            .as_str()
            .into()
    }

    fn toggle(&mut self) {
        self.theme_name = theme::other_theme(&self.theme_name).to_string();
        self.remember();
        self.changed();
    }

}

/// A Lucide glyph recoloured and wrapped as a `data:` URL.
///
/// Lucide draws on a 24x24 grid with `stroke="currentColor"`, which SVG
/// resolves from the cascade and QSvgRenderer does not resolve at all — so the
/// colour is substituted into the source before it is handed over. An unknown
/// name yields an empty URL rather than a broken-image glyph.
pub fn icon_url(icons: &HashMap<String, String>, name: &str, color: &str) -> String {
    let Some(source) = icons.get(name) else {
        return String::new();
    };
    let painted = source.replace("currentColor", color);
    format!(
        "data:image/svg+xml;base64,{}",
        base64(painted.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons() -> HashMap<String, String> {
        ICON_SOURCES
            .iter()
            .map(|(n, s)| ((*n).to_string(), (*s).to_string()))
            .collect()
    }

    #[test]
    fn every_icon_the_chrome_asks_for_is_vendored() {
        let icons = icons();
        for name in [
            "square-terminal", "globe", "git-branch", "camera", "settings",
            "sun", "moon", "power", "plus", "minus", "x", "square", "copy",
            "search", "panel-left", "arrow-left", "ellipsis", "maximize-2",
            "minimize-2", "folder", "folder-open", "folder-tree", "file",
            "chevron-right", "chevron-down", "sparkles",
        ] {
            assert!(icons.contains_key(name), "missing icon {name}");
        }
    }

    #[test]
    fn icon_url_substitutes_the_colour() {
        let icons = icons();
        let url = icon_url(&icons, "x", "#ff0000");
        assert!(url.starts_with("data:image/svg+xml;base64,"));
        let payload = url.trim_start_matches("data:image/svg+xml;base64,");
        // Decode enough to see the colour landed; a round trip through the
        // encoder is the cheapest way to assert that.
        let expected = base64(
            icons["x"].replace("currentColor", "#ff0000").as_bytes(),
        );
        assert_eq!(payload, expected);
        assert!(!icons["x"].contains("#ff0000"));
    }

    #[test]
    fn an_unknown_icon_is_empty_rather_than_broken() {
        assert_eq!(icon_url(&icons(), "no-such-icon", "#000"), "");
    }
}
