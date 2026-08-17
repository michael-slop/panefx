//! Bounds for the background-opacity setting.
//!
//! This module used to apply a WHOLE-WINDOW alpha with
//! `SetLayeredWindowAttributes`, which was the wrong mechanism: it fades every
//! pixel including the terminal's text, so the backdrop bled through the glyphs
//! and the text became hard to read.
//!
//! Opacity is now driven through each terminal's own **per-pixel** alpha — see
//! [`crate::term_opacity`] — where the background fades and the glyphs stay
//! solid. Only the range survives here, because [`crate::config`] validates
//! against it and that runs on every platform.

/// Lowest opacity the UI will allow.
///
/// NOT zero. A fully transparent window is also unfocusable, which is an easy
/// way to lose a terminal with no way to click it back.
pub const MIN_PERCENT: u8 = 10;
pub const MAX_PERCENT: u8 = 100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_keeps_a_window_clickable() {
        assert!(MIN_PERCENT > 0, "0% would make the window unfocusable");
        assert!(MIN_PERCENT < MAX_PERCENT);
        assert_eq!(MAX_PERCENT, 100);
    }
}
