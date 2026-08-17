//! Colour ramp: dark -> light green over a grey background.
//!
//! The original `asciifire.py` (mhearse) uses curses colour pairs, which can offer only
//! flat terminal colours (its "green" is a single colour). Rendering into our
//! own window frees us to use real RGB, so the 8 ramp steps each get their own
//! shade — this is the gradient curses could not express.

/// Background grey the panel clears to before drawing glyphs.
pub const BACKGROUND: Rgb = Rgb(0x1c, 0x1c, 0x1c);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parse `#rrggbb` or `rrggbb`. Returns None on anything malformed, so a
    /// bad value from the control channel is rejected rather than silently
    /// becoming black.
    pub fn parse_hex(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(Rgb(
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Pack into a Win32 COLORREF (0x00BBGGRR — byte order is reversed
    /// relative to the usual web #RRGGBB, which is an easy bug to write).
    #[inline]
    pub fn colorref(self) -> u32 {
        (self.0 as u32) | ((self.1 as u32) << 8) | ((self.2 as u32) << 16)
    }
}

/// One colour per glyph in `fire::RAMP`, coldest first.
///
/// These are deliberately BRIGHT at the cool end. The panel is only ever seen
/// through Alacritty at `opacity = 0.6`, which blends every colour most of the
/// way back toward the terminal's own dark background — a ramp that starts at
/// near-black (the obvious choice on paper) renders as invisible-to-faint for
/// its lower two thirds, so only the flame roots show. Measured: with
/// a near-black cool end, only `$`/`#` survived the blend.
pub const GREEN_RAMP: [Rgb; 8] = [
    Rgb(0x14, 0x28, 0x18), // ' '  darkest, still above the terminal bg
    Rgb(0x1e, 0x5c, 0x28), // '.'
    Rgb(0x28, 0x8c, 0x36), // ':'
    Rgb(0x34, 0xb4, 0x44), // '*'
    Rgb(0x46, 0xd2, 0x56), // 's'
    Rgb(0x62, 0xe6, 0x70), // 'S'
    Rgb(0x8e, 0xf5, 0x98), // '#'
    Rgb(0xc8, 0xff, 0xd0), // '$'  near-white green
];

#[inline]
pub fn color_for(index: usize) -> Rgb {
    GREEN_RAMP[index.min(GREEN_RAMP.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fire::RAMP;

    #[test]
    fn ramp_lengths_match() {
        assert_eq!(GREEN_RAMP.len(), RAMP.len());
    }

    #[test]
    fn ramp_gets_brighter() {
        // Perceived luminance must increase monotonically, otherwise the fire
        // reads as noise rather than a gradient.
        let lum = |c: Rgb| 0.2126 * c.0 as f32 + 0.7152 * c.1 as f32 + 0.0722 * c.2 as f32;
        for w in GREEN_RAMP.windows(2) {
            assert!(lum(w[1]) > lum(w[0]), "ramp not monotonic: {:?} -> {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn ramp_is_actually_green() {
        // Green must dominate at every step, or "greenish spectrum" is a lie.
        for c in GREEN_RAMP {
            assert!(c.1 > c.0 && c.1 > c.2, "not green-dominant: {c:?}");
        }
    }

    #[test]
    fn ramp_survives_the_opacity_blend() {
        // The panel is only ever seen THROUGH Alacritty at opacity 0.6, so
        // every colour is blended toward the terminal's dark background before
        // the user sees it. A ramp whose cool end is near-black renders as
        // nothing. Require that even the coldest drawn glyph stays clearly
        // above the terminal background after blending.
        const ALACRITTY_OPACITY: f32 = 0.6;
        // seoul256's background is a dark grey around #3a3a3a; use the darker
        // #1c1c1c to be conservative.
        let term_bg = 0x1c as f32;

        // Index 0 is the blank glyph and is never drawn, so start at 1.
        for (i, c) in GREEN_RAMP.iter().enumerate().skip(1) {
            let blended_g = c.1 as f32 * ALACRITTY_OPACITY + term_bg * (1.0 - ALACRITTY_OPACITY);
            assert!(
                blended_g - term_bg > 18.0,
                "ramp[{i}] {c:?} vanishes at {ALACRITTY_OPACITY} opacity \
                 (green {blended_g:.0} vs bg {term_bg:.0})"
            );
        }
    }

    #[test]
    fn colorref_byte_order_is_bgr() {
        // #b4ffbe -> 0x00BEFFB4
        assert_eq!(Rgb(0xb4, 0xff, 0xbe).colorref(), 0x00BE_FFB4);
    }

    #[test]
    fn color_for_saturates() {
        assert_eq!(color_for(99), GREEN_RAMP[GREEN_RAMP.len() - 1]);
    }
}
