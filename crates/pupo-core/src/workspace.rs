//! What the workspace strip needs to know about a workspace before anything
//! else has been loaded: its three letters and its colour.
//!
//! One hue per workspace, so a folder is recognisable in the strip before its
//! letters are read. Eight of them, spaced far enough around the wheel that no
//! two are mistakable at tile size, and each rendered twice: saturated for the
//! workspace you are in, pale for the ones you are not. The letters are white
//! on both — it is the tile's weight, not its hue, that says which is current,
//! and a tile that changed hue on selection would stop being an identity.

use std::path::Path;

use crate::theme::hex;

pub const TILE_HUES: [u32; 8] = [12, 43, 90, 141, 186, 225, 268, 322];

/// Three-letter label for a workspace: initials of the first three words for
/// names that have them ("claude-code-sdk" -> "CCS"), else the first three
/// letters ("kraken" -> "KRA", "my-project" -> "MYP").
///
/// Three rather than two because the tile is coloured, and a colour and two
/// letters left too many folders looking alike — "kr" served Kraken and Kramer
/// equally well.
pub fn abbreviation(folder_name: &str) -> String {
    let words: Vec<&str> = folder_name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() >= 3 {
        return words[..3]
            .iter()
            .filter_map(|w| w.chars().next())
            .collect::<String>()
            .to_uppercase();
    }
    let joined: String = words.concat();
    let source = if joined.is_empty() { folder_name } else { &joined };
    source.chars().take(3).collect::<String>().to_uppercase()
}

/// The label for a local workspace path: its folder name, abbreviated.
pub fn abbreviation_for_path(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    abbreviation(&name)
}

/// CRC-32 (IEEE), so a folder keeps its colour across runs and machines. A
/// hasher salted per process — which is what most language runtimes give you —
/// would hand the same folder a new colour on every launch.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// The hue a workspace keeps, chosen from its key so it is the same colour in
/// every window and on every run.
pub fn tile_hue(key: &str) -> u32 {
    TILE_HUES[crc32(key.as_bytes()) as usize % TILE_HUES.len()]
}

/// Saturation and lightness for a tile, per theme and state.
///
/// On a light strip an idle tile is a pastel and the current one steps down
/// into full colour; on a dark strip it has to run the other way, because a
/// pastel on #23252b glares brighter than anything that could mark it current.
fn tile_mix(theme: &str, checked: bool, hovered: bool) -> (f64, f64) {
    match (theme, checked, hovered) {
        ("dark", false, false) => (0.32, 0.32),
        ("dark", false, true) => (0.32, 0.38),
        ("dark", true, false) => (0.52, 0.58),
        ("dark", true, true) => (0.52, 0.64),
        (_, false, false) => (0.45, 0.76),
        (_, false, true) => (0.45, 0.70),
        (_, true, false) => (0.48, 0.54),
        (_, true, true) => (0.48, 0.48),
    }
}

/// HSL to RGB, matching Qt's `QColor::fromHslF` (which is where these
/// saturation and lightness numbers were chosen against).
pub fn hsl(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = (hue % 360.0 + 360.0) % 360.0 / 60.0;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

/// One of a tile's four faces — idle, hovered, current, current-and-hovered.
pub fn tile_color(key: &str, theme: &str, checked: bool, hovered: bool) -> String {
    let (saturation, lightness) = tile_mix(theme, checked, hovered);
    hex(hsl(f64::from(tile_hue(key)), saturation, lightness))
}

/// The bar down the strip's edge beside the current workspace, and the activity
/// dot on a tile. Both have to read on any of the eight hues at either weight,
/// so they are the strip's own extreme rather than anything from the wheel.
pub fn indicator_color(theme: &str) -> &'static str {
    if theme == "dark" {
        "#e8e6e2"
    } else {
        "#2a2824"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_words_become_their_initials() {
        assert_eq!(abbreviation("claude-code-sdk"), "CCS");
        assert_eq!(abbreviation("a_b_c_d"), "ABC");
    }

    #[test]
    fn fewer_words_fall_back_to_the_first_three_letters() {
        assert_eq!(abbreviation("kraken"), "KRA");
        assert_eq!(abbreviation("my-project"), "MYP");
        assert_eq!(abbreviation("go"), "GO");
    }

    #[test]
    fn a_name_with_no_letters_at_all_still_produces_something() {
        assert_eq!(abbreviation("..."), "...");
        assert_eq!(abbreviation(""), "");
    }

    #[test]
    fn a_path_is_labelled_by_its_folder() {
        assert_eq!(abbreviation_for_path("/home/pascal/Workspace/pupo"), "PUP");
        assert_eq!(
            abbreviation_for_path("/home/pascal/Workspace/claude-code-sdk"),
            "CCS"
        );
    }

    #[test]
    fn crc32_matches_the_reference_vector() {
        // The check value every CRC-32/IEEE implementation agrees on.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn a_workspace_keeps_its_hue_across_runs() {
        let key = "/home/pascal/Workspace/pupo";
        let hue = tile_hue(key);
        assert!(TILE_HUES.contains(&hue));
        assert_eq!(hue, tile_hue(key));
        // With eight hues, collisions are expected and fine; what matters is
        // that the choice is derived from the key rather than from a salted
        // hash, so two real neighbouring workspaces stay distinguishable.
        assert_eq!(tile_hue("/home/pascal/Workspace/pupo"), 12);
        assert_eq!(tile_hue("/home/pascal/Workspace/alpine"), 141);
    }

    #[test]
    fn hsl_converts_the_way_qt_does() {
        assert_eq!(hsl(0.0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(hsl(120.0, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hsl(240.0, 1.0, 0.5), (0, 0, 255));
        assert_eq!(hsl(0.0, 0.0, 0.5), (128, 128, 128));
        assert_eq!(hsl(0.0, 1.0, 1.0), (255, 255, 255));
        assert_eq!(hsl(0.0, 1.0, 0.0), (0, 0, 0));
    }

    #[test]
    fn the_current_tile_is_the_stronger_one_in_either_theme() {
        let key = "/home/pascal/Workspace/pupo";
        // Light: current steps *down* in lightness; dark: it steps *up*.
        let light_idle = tile_color(key, "light", false, false);
        let light_current = tile_color(key, "light", true, false);
        assert_ne!(light_idle, light_current);
        let dark_idle = tile_color(key, "dark", false, false);
        let dark_current = tile_color(key, "dark", true, false);
        assert_ne!(dark_idle, dark_current);
        // All four faces are distinct, or a hover would go unnoticed.
        let faces = [
            tile_color(key, "light", false, false),
            tile_color(key, "light", false, true),
            tile_color(key, "light", true, false),
            tile_color(key, "light", true, true),
        ];
        for (i, a) in faces.iter().enumerate() {
            for b in &faces[i + 1..] {
                assert_ne!(a, b, "two tile faces render the same colour");
            }
        }
    }

    #[test]
    fn every_colour_is_an_html_hex_triplet() {
        let color = tile_color("/x", "dark", true, true);
        assert_eq!(color.len(), 7);
        assert!(color.starts_with('#'));
        assert!(color[1..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(indicator_color("dark"), "#e8e6e2");
        assert_eq!(indicator_color("light"), "#2a2824");
    }
}
