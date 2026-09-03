//! The type scales the app is sized from.
//!
//! Two numbers are configurable — the transcript's base size in pixels and the
//! terminal's in points — and everything else in the conversation pane is
//! derived from the first of them here: the composer, the model and effort
//! labels, the attachment chips, the busy row, and the monospace detail under
//! an expanded tool call. A reader who bumps the base is asking for a bigger
//! conversation, not for bigger body text sitting in chrome that stayed put.

/// The conversation's type scale.
pub mod chat {
    pub const DEFAULT_SIZE: i32 = 13;
    /// Floors and ceilings for the setting. The floor is where the transcript's
    /// bubbles and code cards still have room for their padding; the ceiling is
    /// past any plausible reading size and only guards a bad stored value.
    pub const MIN_SIZE: i32 = 9;
    pub const MAX_SIZE: i32 = 28;

    /// Qt char formats take points, not pixels. At Qt's assumed 96dpi a pixel
    /// is 0.75pt, which is the conversion the detail sizes below go through.
    const PX_TO_PT: f64 = 0.75;

    /// A base size within the supported range, whatever the caller had.
    pub fn clamp(size: i32) -> i32 {
        size.clamp(MIN_SIZE, MAX_SIZE)
    }

    /// The same, for a value restored from disk: anything that is not a number
    /// at all falls back to the default rather than to a bound.
    pub fn clamp_opt(size: Option<i64>) -> i32 {
        match size {
            Some(value) => clamp(value.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
            None => DEFAULT_SIZE,
        }
    }

    /// Chrome around the message text: buttons, chips' labels, the model and
    /// effort labels, the busy row. One step under the body, never below the
    /// floor — at the smallest sizes everything collapses onto one size rather
    /// than shrinking the chrome into illegibility.
    pub fn secondary(base: i32) -> i32 {
        (base - 1).max(MIN_SIZE)
    }

    /// The smallest text in the pane: attachment chips and a code block's Copy
    /// button, which sit *on* content and have to stay out of its way.
    pub fn caption(base: i32) -> i32 {
        (base - 2).max(MIN_SIZE)
    }

    fn round2(value: f64) -> f64 {
        (value * 100.0).round() / 100.0
    }

    /// Point size for the monospace detail under an expanded tool call.
    pub fn detail_points(base: i32) -> f64 {
        round2(f64::from(secondary(base)) * PX_TO_PT)
    }

    /// Point size for the stats footer under a finished reply. It sits at the
    /// caption size because it annotates the reply above it rather than being
    /// part of it.
    pub fn footer_points(base: i32) -> f64 {
        round2(f64::from(caption(base)) * PX_TO_PT)
    }
}

/// Terminal font-size bounds and normalization.
pub mod terminal {
    pub const DEFAULT_SIZE: i32 = 13;
    pub const MIN_SIZE: i32 = 8;
    pub const MAX_SIZE: i32 = 32;

    pub fn clamp(size: i32) -> i32 {
        size.clamp(MIN_SIZE, MAX_SIZE)
    }

    pub fn clamp_opt(size: Option<i64>) -> i32 {
        match size {
            Some(value) => clamp(value.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
            None => DEFAULT_SIZE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_clamp_holds_the_bounds() {
        assert_eq!(chat::clamp(3), chat::MIN_SIZE);
        assert_eq!(chat::clamp(999), chat::MAX_SIZE);
        assert_eq!(chat::clamp(13), 13);
    }

    #[test]
    fn a_missing_stored_size_falls_back_to_the_default() {
        assert_eq!(chat::clamp_opt(None), chat::DEFAULT_SIZE);
        assert_eq!(terminal::clamp_opt(None), terminal::DEFAULT_SIZE);
        // A stored value out of range is still clamped, not defaulted.
        assert_eq!(chat::clamp_opt(Some(400)), chat::MAX_SIZE);
    }

    #[test]
    fn derived_sizes_step_down_but_never_below_the_floor() {
        assert_eq!(chat::secondary(13), 12);
        assert_eq!(chat::caption(13), 11);
        assert_eq!(chat::secondary(chat::MIN_SIZE), chat::MIN_SIZE);
        assert_eq!(chat::caption(chat::MIN_SIZE), chat::MIN_SIZE);
        assert_eq!(chat::caption(chat::MIN_SIZE + 1), chat::MIN_SIZE);
    }

    #[test]
    fn point_sizes_convert_at_qts_96dpi() {
        assert_eq!(chat::detail_points(13), 9.0);
        assert_eq!(chat::footer_points(13), 8.25);
        assert_eq!(chat::detail_points(20), 14.25);
    }

    #[test]
    fn terminal_clamp_holds_its_own_bounds() {
        assert_eq!(terminal::clamp(1), terminal::MIN_SIZE);
        assert_eq!(terminal::clamp(100), terminal::MAX_SIZE);
        assert_eq!(terminal::clamp(10), 10);
    }
}
