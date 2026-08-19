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
///
/// The floor is well above that hazard, though, because clickability is not the
/// binding constraint — compositing cost is. A near-transparent borderless
/// window sitting over an animated backdrop makes DWM re-composite the window
/// against moving pixels for every damage rectangle. Ordinary use hides this;
/// streaming a screenful of text (a long table, a build log) turns it into
/// continuous full-screen recomposition and the display visibly tears and
/// flashes. Measured at 10%: unusable. The old floor made that reachable with
/// one keypress in the TUI, so it moved up to where the backdrop still reads
/// but the glyphs carry enough alpha to composite cheaply.
pub const MIN_PERCENT: u8 = 35;
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

    #[test]
    fn the_floor_stays_out_of_the_compositor_thrashing_range() {
        // Not a style preference. Below roughly a third, a borderless window
        // over an animated backdrop tears and flashes under sustained terminal
        // output -- see the constant's doc comment. Lowering this re-opens that
        // bug, so the number is pinned rather than merely documented.
        assert!(
            MIN_PERCENT >= 30,
            "a floor below 30% lets the compositor thrash on heavy terminal output"
        );
    }
}
