//! ASCII rain — ported from Michael's own `createRain`.
//!
//! Source read before porting: `createRain` from michaelslop.org's own
//! `static/app.js`, the renderer behind that site's boot screen. (That file is
//! not public; the algorithm is reproduced faithfully here and the comments
//! carry the reasoning that mattered.)
//!
//! Kept from the original:
//!   * the exact CP437 character set, block glyphs included. It is ASCII-not-
//!     katakana on purpose: BigBlueTerm is a DOS face with no katakana, and
//!     "ASCII rain" ought to be ascii.
//!   * one head per column, each starting at a NEGATIVE row so columns are
//!     staggered rather than falling in a sheet
//!   * pale head (`#c8ffc8`) with a green cell (`#22cc44`) immediately above
//!   * respawn above the top once the head passes the bottom, at a random
//!     negative offset
//!   * the stepped ~55ms tick — the original's comment is explicit that a CRT
//!     reads better stepped than smooth, so this does NOT run at panel fps
//!
//! Deliberately NOT ported (michaelslop.org furniture, per instruction):
//!   the skeleton silhouette mask, the "michael.slop" text flourish, and the
//!   wizard lightning bolts.
//!
//! One thing had to change rather than be copied. The original is a canvas: it
//! paints a translucent black rect each frame so old glyphs fade, and the
//! trail is a side effect of that persistence. This renderer is a character
//! grid that is cleared every frame, so the trail is modelled explicitly as a
//! per-cell brightness that decays. Same look, different mechanism.

use crate::animation::AsciiAnimation;
use crate::palette::Rgb;

/// The site's exact set, block glyphs included.
pub const CHARS: &str = "01ABCDEFGHIJKLMNOPQRSTUVWXYZ#$%&@*+=<>?!;:░▒▓";

/// Milliseconds per rain step. From the original's `TICK = 55`.
///
/// The original's comment is emphatic that this number is a choice: "chunky on
/// purpose - a CRT reads better stepped than smooth - but 66 was far enough
/// into slideshow territory to look broken rather than retro. 55 is ~18fps."
/// Do not smooth it out.
pub const TICK_MS: u64 = 55;

/// Cell size the rain is DESIGNED for, in pixels (`const CELL = 16`).
///
/// This is not the same as the Alacritty text cell (10x15 for BigBlueTerm437
/// at 11pt/96dpi). Drawing the rain on the terminal's grid makes it denser and
/// finer than the site version, so run it with `PANEFX_CELL_W=11
/// PANEFX_CELL_H=16` (or the equivalent config keys) to match michaelslop.org.
///
/// Kept as documentation of the original's intent; the supervisor does not read
/// it, because cell size is a whole-panel property shared by every effect.
pub const DESIGN_CELL: i32 = 16;

/// Head colour — pale, near-white green. Original `#c8ffc8`.
pub const C_HEAD: Rgb = Rgb(0xc8, 0xff, 0xc8);
/// Trail colour — the original's `#22cc44`.
pub const C_TRAIL: Rgb = Rgb(0x22, 0xcc, 0x44);

/// How much a trail cell dims per step. Chosen so a trail runs ~10 cells,
/// which matches the canvas version's 0.22-alpha fade.
pub const DECAY: u8 = 26;

pub const BACKGROUND: Rgb = Rgb(0x00, 0x00, 0x00);

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

pub struct Rain {
    cols: usize,
    rows: usize,
    /// Head row per column. Signed: a negative value means the head has not
    /// entered the top of the panel yet, which is what staggers the columns.
    drops: Vec<i32>,
    /// Glyph currently shown in each cell.
    glyph: Vec<char>,
    /// Brightness 0..=255 per cell; 255 is a fresh head. Decays each step.
    bright: Vec<u8>,
    chars: Vec<char>,
    rng: Rng,
    /// Accumulated milliseconds, so the rain steps at TICK_MS regardless of
    /// the panel's frame rate.
    accum_ms: u64,
    /// Milliseconds one panel frame represents, set from the configured fps.
    frame_ms: u64,
    /// Did the last `step()` call actually advance the rain? The 55ms tick is
    /// independent of the panel fps, so many frames are no-ops.
    dirty: bool,
    // ---- live-tunable parameters (defaults are the consts above) ----
    /// Rain step interval. The original is emphatic that 55ms is a deliberate
    /// stepped look, so the default stays there — but it is tunable now.
    tick_ms: u64,
    /// Per-step dimming of trail cells. Higher = shorter trails.
    decay: u8,
    head: Rgb,
    trail: Rgb,
    bg: Rgb,
}

impl Rain {
    pub fn new(cols: usize, rows: usize, seed: u64, chars: Option<&str>) -> Self {
        let set: Vec<char> = chars
            .filter(|s| !s.is_empty())
            .unwrap_or(CHARS)
            .chars()
            .collect();
        let mut r = Rain {
            cols,
            rows,
            drops: Vec::new(),
            glyph: vec![' '; cols * rows],
            bright: vec![0; cols * rows],
            chars: if set.is_empty() {
                CHARS.chars().collect()
            } else {
                set
            },
            rng: Rng::new(seed),
            accum_ms: 0,
            frame_ms: 50,
            dirty: true,
            tick_ms: TICK_MS,
            decay: DECAY,
            head: C_HEAD,
            trail: C_TRAIL,
            bg: BACKGROUND,
        };
        r.reset_drops();
        r
    }

    /// Tell the effect how long one panel frame is, so its fixed 55ms rain
    /// tick stays correct whatever fps the panel runs at.
    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// `drops = Array.from({length: cols}, () => -floor(random()*rows*1.5))`
    fn reset_drops(&mut self) {
        let rows = self.rows;
        let cols = self.cols;
        self.drops = (0..cols)
            .map(|_| {
                let span = (rows as f32 * 1.5) as usize;
                -(self.rng.below(span.max(1)) as i32)
            })
            .collect();
    }

    #[inline]
    fn idx(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    fn random_glyph(&mut self) -> char {
        let i = self.rng.below(self.chars.len());
        self.chars[i]
    }

    /// One rain step — the body of the original's `tick`.
    fn advance(&mut self) {
        // Fade everything. Stands in for the canvas' translucent black rect.
        for b in self.bright.iter_mut() {
            *b = b.saturating_sub(self.decay);
        }

        for c in 0..self.cols {
            let r = self.drops[c];

            if r >= 0 && (r as usize) < self.rows {
                let row = r as usize;
                let g = self.random_glyph();
                let i = self.idx(c, row);
                self.glyph[i] = g;
                self.bright[i] = 255;

                // `if (r > 0)` — the green cell one above the pale head.
                if row > 0 {
                    let g2 = self.random_glyph();
                    let i2 = self.idx(c, row - 1);
                    self.glyph[i2] = g2;
                    // Just under the head-colour threshold so it renders as
                    // trail green rather than a second head.
                    self.bright[i2] = 200;
                }
            }

            // `drops[c] = r > rows + random()*30 ? -((random()*20)|0) : r + 1`
            let limit = self.rows as f32 + self.rng.next_f32() * 30.0;
            self.drops[c] = if (r as f32) > limit {
                -(self.rng.below(20) as i32)
            } else {
                r + 1
            };
        }
    }
}

impl AsciiAnimation for Rain {
    fn name(&self) -> &'static str {
        "rain"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.glyph = vec![' '; cols * rows];
        self.bright = vec![0; cols * rows];
        self.reset_drops();
        self.dirty = true;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        // The original ticks at a fixed 55ms because "a CRT reads better
        // stepped than smooth". The panel may run faster or slower, so
        // accumulate and step only when enough time has passed.
        //
        // Frame duration is derived from the configured fps rather than a
        // clock read, so this stays deterministic under test.
        self.dirty = false;
        self.accum_ms += self.frame_ms;
        while self.accum_ms >= self.tick_ms {
            self.accum_ms -= self.tick_ms;
            self.advance();
            self.dirty = true;
        }
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        let i = self.idx(col, row);
        let b = self.bright[i];
        if b == 0 {
            return None;
        }
        let g = self.glyph[i];
        if g == ' ' {
            return None;
        }
        // Fresh head is pale; everything behind it is trail green, dimmed by
        // how far it has decayed.
        let colour = if b >= 250 {
            self.head
        } else {
            let f = b as f32 / 255.0;
            Rgb(
                (self.trail.0 as f32 * f) as u8,
                (self.trail.1 as f32 * f) as u8,
                (self.trail.2 as f32 * f) as u8,
            )
        };
        Some((g, colour))
    }

    fn changed(&self) -> bool {
        self.dirty
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    fn params(&self) -> Vec<crate::animation::Param> {
        use crate::animation::Param;
        vec![
            Param::int("tick_ms", "step (ms)", self.tick_ms as i64, 10, 300),
            Param::int("decay", "trail decay", self.decay as i64, 1, 128),
            Param::text("chars", "glyphs", &self.chars.iter().collect::<String>()),
            Param::colour("head", "head colour", self.head),
            Param::colour("trail", "trail colour", self.trail),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &crate::animation::ParamValue) -> bool {
        use crate::animation::clamp_int;
        match key {
            "tick_ms" => match v.as_int() {
                Some(n) => {
                    self.tick_ms = clamp_int(n, 10, 300) as u64;
                    true
                }
                None => false,
            },
            "decay" => match v.as_int() {
                Some(n) => {
                    self.decay = clamp_int(n, 1, 128) as u8;
                    true
                }
                None => false,
            },
            "chars" => match v.as_text() {
                // An empty set would panic the modulo in `random_glyph`, so
                // fall back rather than accept it.
                Some(s) => {
                    let set: Vec<char> = s.chars().collect();
                    self.chars = if set.is_empty() {
                        CHARS.chars().collect()
                    } else {
                        set
                    };
                    true
                }
                None => false,
            },
            "head" => match v.as_rgb() {
                Some(c) => {
                    self.head = c;
                    true
                }
                None => false,
            },
            "trail" => match v.as_rgb() {
                Some(c) => { self.trail = c; true }
                None => false,
            },
            "bg" => match v.as_rgb() {
                Some(c) => { self.bg = c; true }
                None => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stepped(r: &mut Rain, n: usize) {
        for _ in 0..n {
            r.advance();
        }
    }

    #[test]
    fn charset_is_the_sites_exact_set() {
        assert!(CHARS.starts_with("01ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
        assert!(CHARS.contains('░') && CHARS.contains('▒') && CHARS.contains('▓'));
        // ASCII-not-katakana is deliberate: BigBlueTerm has no katakana.
        assert!(!CHARS.chars().any(|c| ('\u{30A0}'..='\u{30FF}').contains(&c)));
    }

    #[test]
    fn columns_start_staggered() {
        // If every column started at row 0 the rain would fall as one sheet.
        let r = Rain::new(60, 30, 42, None);
        let distinct: std::collections::HashSet<i32> = r.drops.iter().copied().collect();
        assert!(distinct.len() > 5, "drops not staggered: {distinct:?}");
        assert!(r.drops.iter().all(|d| *d <= 0), "drops should start above the top");
    }

    #[test]
    fn heads_fall_and_respawn() {
        let mut r = Rain::new(20, 15, 7, None);
        stepped(&mut r, 400);
        // Every column must still hold a plausible head position.
        for d in &r.drops {
            assert!(*d >= -25 && *d <= r.rows as i32 + 35, "runaway drop {d}");
        }
    }

    #[test]
    fn something_is_actually_drawn() {
        let mut r = Rain::new(60, 30, 11, None);
        stepped(&mut r, 60);
        let lit = (0..30)
            .flat_map(|row| (0..60).map(move |c| (c, row)))
            .filter(|(c, row)| r.cell_at(*c, *row).is_some())
            .count();
        assert!(lit > 10, "rain renders almost nothing: {lit}");
    }

    #[test]
    fn trails_fade_rather_than_vanish() {
        // A trail must span several cells, otherwise it reads as unconnected
        // dots rather than falling rain.
        let mut r = Rain::new(1, 40, 5, None);
        stepped(&mut r, 60);
        let lit = (0..40).filter(|row| r.cell_at(0, *row).is_some()).count();
        assert!(lit >= 3, "trail too short ({lit} cells)");
    }

    #[test]
    fn custom_charset_is_used() {
        let mut r = Rain::new(20, 10, 3, Some("XY"));
        stepped(&mut r, 40);
        for row in 0..10 {
            for c in 0..20 {
                if let Some((g, _)) = r.cell_at(c, row) {
                    assert!(g == 'X' || g == 'Y', "unexpected glyph {g:?}");
                }
            }
        }
    }

    #[test]
    fn empty_charset_falls_back() {
        let r = Rain::new(5, 5, 1, Some(""));
        assert!(!r.chars.is_empty(), "empty charset would panic on modulo");
    }

    #[test]
    fn params_round_trip_and_clamp() {
        use crate::animation::{AsciiAnimation, ParamValue};
        let mut r = Rain::new(20, 10, 1, None);

        // Every declared param must be settable by its own key, or the TUI
        // shows a control that does nothing.
        for p in r.params() {
            assert!(
                r.set_param(&p.key, &p.value),
                "declared param '{}' is not settable",
                p.key
            );
        }

        // Out-of-range values must clamp, not corrupt.
        assert!(r.set_param("tick_ms", &ParamValue::Int { v: 99999 }));
        assert!(r.tick_ms <= 300, "tick_ms not clamped: {}", r.tick_ms);
        assert!(r.set_param("decay", &ParamValue::Int { v: -50 }));
        assert!(r.decay >= 1, "decay not clamped: {}", r.decay);

        // Wrong type and unknown key are both rejected.
        assert!(!r.set_param("tick_ms", &ParamValue::Text { v: "x".into() }));
        assert!(!r.set_param("nonexistent", &ParamValue::Int { v: 1 }));
    }

    #[test]
    fn empty_charset_param_falls_back() {
        use crate::animation::{AsciiAnimation, ParamValue};
        let mut r = Rain::new(20, 10, 1, None);
        assert!(r.set_param("chars", &ParamValue::Text { v: String::new() }));
        // An empty set would panic the modulo in random_glyph.
        assert!(!r.chars.is_empty());
        for _ in 0..20 {
            r.advance();
        }
    }

    #[test]
    fn resize_does_not_panic() {
        let mut r = Rain::new(10, 10, 1, None);
        r.resize(0, 0);
        r.step();
        r.resize(80, 25);
        stepped(&mut r, 30);
    }
}
