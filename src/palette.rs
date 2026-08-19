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

/// Fast sine by table lookup.
///
/// Kept for what it is -- a cheap sine -- and NOT for the reason it was added.
/// It went in believing per-cell `sin` was why `plasma` and `tunnel` cost ~100%
/// of a core; swapping it in moved that by under 1%. The real cost was draw
/// calls (see `quantise`). This stays because it is free and correct, but do
/// not expect it to buy performance.
///
/// 4096 entries is the accuracy/size trade: the error is under 0.001, which is
/// far below one step of any glyph ramp here, and the table is 16KB -- small
/// enough to stay in L1 across a frame.
mod fasttrig {
    pub const BITS: usize = 12;
    pub const SIZE: usize = 1 << BITS;
    pub const MASK: usize = SIZE - 1;

    /// `sin(i / SIZE * TAU)` for each `i`.
    pub static TABLE: std::sync::LazyLock<[f32; SIZE]> = std::sync::LazyLock::new(|| {
        let mut t = [0.0f32; SIZE];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f32 / SIZE as f32 * std::f32::consts::TAU).sin();
        }
        t
    });
}

/// Table-lookup sine. Argument in radians, any magnitude.
#[inline]
pub fn fsin(x: f32) -> f32 {
    let idx = (x * (fasttrig::SIZE as f32 / std::f32::consts::TAU)) as isize;
    fasttrig::TABLE[(idx as usize) & fasttrig::MASK]
}

/// Table-lookup cosine.
#[inline]
pub fn fcos(x: f32) -> f32 {
    fsin(x + std::f32::consts::FRAC_PI_2)
}

#[cfg(test)]
mod fasttrig_tests {
    use super::*;

    #[test]
    fn the_table_matches_the_real_sine() {
        // The whole point is that it is indistinguishable at ramp resolution.
        // A ramp step here is at worst 1/4 (the block ramps), so 0.01 is two
        // orders of margin.
        let mut worst = 0.0f32;
        for i in 0..2000 {
            let x = (i as f32 / 2000.0) * 40.0 - 20.0;
            worst = worst.max((fsin(x) - x.sin()).abs());
        }
        assert!(worst < 0.01, "worst sine error {worst}");
    }

    #[test]
    fn it_handles_negative_and_large_arguments() {
        // The index is masked, so it must wrap rather than panic or clamp --
        // a plasma phase grows without bound and goes negative on some terms.
        for x in [-1000.0f32, -0.5, 0.0, 12345.0] {
            let v = fsin(x);
            assert!(v.is_finite() && (-1.0..=1.0).contains(&v), "fsin({x}) = {v}");
        }
    }

    #[test]
    fn cosine_leads_sine_by_a_quarter_turn() {
        for i in 0..100 {
            let x = i as f32 * 0.1;
            assert!((fcos(x) - x.cos()).abs() < 0.01, "fcos({x})");
        }
    }
}

/// Snap a colour to a coarse grid, so neighbouring cells share one.
///
/// **This is the single most expensive decision an effect makes.** The renderer
/// issues one `ExtTextOutW` PER DISTINCT COLOUR PER ROW (see `render.rs`), so
/// the draw-call bill is the number of distinct colours, not the number of lit
/// cells. Measured on a 128x62 grid across pHub's four monitors:
///
/// | effect | lit | colours/row | draw calls | CPU |
/// |---|---|---|---|---|
/// | `waves`  | 46% |  6.0 |  370 |  35% |
/// | `plasma` | 44% | 33.6 | 2081 | 100% |
///
/// Same lit fraction, 5.6x the draw calls, 3x the CPU. A smooth per-cell colour
/// blend is what does it: every cell gets its own RGB and no two share a bucket.
///
/// `levels` is how many steps each channel is allowed. 8 is invisible on a
/// wallpaper -- the ramp glyph already carries most of the shading -- and cuts
/// the distinct colours per row to a handful.
#[inline]
pub fn quantise(c: Rgb, levels: u8) -> Rgb {
    let l = levels.max(2) as u32;
    let q = |v: u8| -> u8 {
        // Round to the nearest of `l` levels, then map that level back across
        // the FULL 0..255 range.
        //
        // Not `level * (255 / (l-1))`: integer division truncates the step, so
        // at 8 levels the top bucket lands on 252 and white quietly turns grey.
        // Multiplying first and dividing last keeps both ends exact.
        let level = ((v as u32 * (l - 1) + 127) / 255).min(l - 1);
        ((level * 255) / (l - 1)) as u8
    };
    Rgb(q(c.0), q(c.1), q(c.2))
}

#[cfg(test)]
mod quantise_tests {
    use super::*;

    #[test]
    fn it_collapses_near_colours_together() {
        // The whole point: adjacent cells of a smooth gradient must land in the
        // same bucket, or the renderer pays a draw call for each.
        let a = quantise(Rgb(100, 100, 100), 8);
        let b = quantise(Rgb(103, 101, 99), 8);
        assert_eq!(a, b, "near colours must share a bucket");
    }

    #[test]
    fn the_ends_of_the_range_are_preserved() {
        // Black must stay black and white white, or every effect's background
        // and highlights shift.
        assert_eq!(quantise(Rgb(0, 0, 0), 8), Rgb(0, 0, 0));
        assert_eq!(quantise(Rgb(255, 255, 255), 8), Rgb(255, 255, 255));
    }

    #[test]
    fn more_levels_means_more_distinct_colours() {
        let count = |levels: u8| {
            let mut seen: Vec<Rgb> = Vec::new();
            for v in 0..=255u8 {
                let q = quantise(Rgb(v, 0, 0), levels);
                if !seen.contains(&q) {
                    seen.push(q);
                }
            }
            seen.len()
        };
        assert!(count(16) > count(4), "{} !> {}", count(16), count(4));
    }

    #[test]
    fn a_degenerate_level_count_does_not_divide_by_zero() {
        // `levels` comes from a config knob, so 0 and 1 must be survivable.
        for l in [0u8, 1, 2] {
            let q = quantise(Rgb(128, 128, 128), l);
            assert!(q.0 <= 255);
        }
    }
}
