//! Heat-dissipation fire simulation.
//!
//! Ported from `mhearse/asciifire` (`asciifire.py`), which is itself a port of
//! Thiemo Mattig's JavaScript at http://maettig.com/code/javascript/asciifire.html
//!
//! The original is a curses program: it owns a TTY, sizes itself via
//! `getmaxyx()`, and paints with five flat curses color pairs. Only the
//! algorithm is kept here. This module is deliberately free of any Windows or
//! rendering dependency so it can be tested standalone.
//!
//! The algorithm:
//!   * seed the bottom row with random heat each frame
//!   * every other cell becomes the average of four neighbours below it,
//!     which both spreads heat sideways and carries it upward
//!   * a decay term keeps the fire from saturating
//!   * map the resulting value to an index in an 8-glyph ramp

/// The character ramp from the original, coldest first.
pub const RAMP: [char; 8] = [' ', '.', ':', '*', 's', 'S', '#', '$'];

/// Xorshift RNG. The original used Python's `random`; we want determinism for
/// tests and no external crate, so we carry our own.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // A zero state would lock xorshift at zero forever.
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

    /// Uniform in [0.0, 1.0).
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits gives a clean mantissa-sized integer.
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

/// Per-row cooling tuned so flames reach a similar FRACTION of the panel
/// height whatever its size.
///
/// A fixed decay burns out after a roughly fixed number of rows: tuned on an
/// 80x25 grid it looks right there, then fills only the bottom sixth of a
/// full-height terminal (measured on pHub: 1274px tall ≈ 85 rows, flames died
/// around row 70 of 85, leaving the top two thirds empty).
///
/// Calibrated so that flames reach roughly 60-70% of the way up, which reads
/// as fire rather than as a stripe along the bottom.
pub fn decay_for_rows(rows: usize) -> f32 {
    if rows == 0 {
        return 0.14;
    }
    // ~15 rows of visible flame at the reference size, scaled by height.
    (2.6 / rows as f32).clamp(0.02, 0.30)
}

/// How much the glyph threshold is dithered, in ramp buckets. ~1 bucket of
/// spread is enough to shatter the flat bands without visibly softening the
/// bright core of the fire.
const DITHER_STRENGTH: f32 = 1.0;

/// Stable per-cell value in [0,1), hashed from the coordinates.
///
/// Must be a function of position only. A per-frame random here would make
/// every cell in the sparse tail flicker independently, which looks like TV
/// static rather than fire.
#[inline]
fn cell_dither(col: usize, row: usize) -> f32 {
    // Cheap integer hash (xorshift-mix on the packed coordinate pair).
    let mut h = (col as u32).wrapping_mul(0x9E37_79B9) ^ (row as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h >> 8) as f32 / ((1u32 << 24) as f32)
}

/// How many rows at the top of the grid are faded to nothing.
///
/// Deliberately generous: the fade must be wide enough that heat is already
/// near zero *before* it reaches row 0, otherwise the boundary line reappears
/// at the top of the fade instead of the top of the grid.
const TOP_FADE_ROWS: usize = 6;

/// Multiplier that ramps from 0.0 at row 0 to 1.0 by `TOP_FADE_ROWS`.
#[inline]
fn top_fade(row: usize, rows: usize) -> f32 {
    // On a very short panel, fading a fixed 6 rows would erase most of the
    // fire, so scale the band down for small grids.
    let band = TOP_FADE_ROWS.min(rows / 3).max(1);
    if row >= band {
        return 1.0;
    }
    // Squared so the last row or two go properly dark rather than merely dim.
    let t = row as f32 / band as f32;
    t * t
}

pub struct Fire {
    pub cols: usize,
    pub rows: usize,
    /// Heat per cell in [0.0, 1.0], row-major, row 0 is the TOP of the screen.
    ///
    /// Holds `rows + 1` rows: the extra final row is the *off-screen* seed row.
    /// Mättig's original description is explicit that the random source row is
    /// off-screen; drawing it would show a solid wall of hot glyphs pinned to
    /// the bottom edge instead of flame roots.
    cells: Vec<f32>,
    rng: Rng,
    /// Cooling factor per row of rise; higher = shorter flames.
    decay: f32,
}

impl Fire {
    pub fn new(cols: usize, rows: usize, seed: u64) -> Self {
        Fire {
            cols,
            rows,
            cells: vec![0.0; cols * (rows + 1)],
            rng: Rng::new(seed),
            decay: decay_for_rows(rows),
        }
    }

    /// Resize preserving nothing — callers resize on terminal geometry changes,
    /// where a garbage-preserving copy would look worse than a clean restart.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.cells = vec![0.0; cols * (rows + 1)];
        // Must re-tune: keeping the old decay after a resize leaves flames the
        // wrong height for the new panel.
        self.decay = decay_for_rows(rows);
    }

    #[inline]
    fn idx(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    #[inline]
    pub fn heat_at(&self, col: usize, row: usize) -> f32 {
        self.cells[self.idx(col, row)]
    }

    /// Heat value -> index into `RAMP`.
    ///
    /// The mapping is DITHERED, and it has to be. In the sparse tail of the
    /// fire the heat field is smooth and nearly flat, so a plain `floor()`
    /// puts a whole horizontal swathe of cells in the same bucket at once and
    /// they all draw the same glyph — long unbroken runs of `.` stretching
    /// 40-100 characters wide. On screen that reads as horizontal lines
    /// through the dying part of the flames, which is the single most visible
    /// artifact this renderer produces.
    ///
    /// Adding a stable per-cell offset before flooring breaks the tie: cells
    /// sitting either side of a threshold scatter instead of flipping in
    /// unison. The offset is a hash of (col, row), NOT a random number, so it
    /// is constant across frames — a per-frame random would make every cell
    /// in the tail strobe.
    #[inline]
    pub fn glyph_index_at(&self, col: usize, row: usize) -> usize {
        let h = self.heat_at(col, row).clamp(0.0, 1.0);
        let scaled = h * RAMP.len() as f32;
        // Dither by up to one bucket, centred so the mean brightness is
        // unchanged.
        let d = (cell_dither(col, row) - 0.5) * DITHER_STRENGTH;
        let idx = (scaled + d).max(0.0) as usize;
        idx.min(RAMP.len() - 1)
    }

    #[inline]
    pub fn glyph_at(&self, col: usize, row: usize) -> char {
        RAMP[self.glyph_index_at(col, row)]
    }

    /// Advance one frame.
    pub fn step(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }

        // Seed the OFF-SCREEN row (index `rows`) with fresh random heat. Some
        // cells are seeded cold, which is what carves the gaps between flame
        // tongues — a uniformly hot base gives an unbroken sheet of fire.
        let seed_row = self.rows;
        for col in 0..self.cols {
            let v = self.rng.next_f32();
            let i = self.idx(col, seed_row);
            self.cells[i] = if v < 0.25 { v * 0.7 } else { 0.75 + v * 0.25 };
        }

        // Propagate upward, walking top-down so each row reads the
        // not-yet-updated row beneath it.
        //
        // The kernel is CENTRE-WEIGHTED. An even left/centre/right average is a
        // strong horizontal blur applied every frame: it smears vertical
        // structure away within a row or two, leaving flat (or merely
        // dithered) horizontal bands. Weighting the cell directly below far
        // more heavily lets heat climb in columns, and the lighter side terms
        // let those columns lean and merge like real flames.
        for row in 0..self.rows {
            for col in 0..self.cols {
                let below = row + 1;
                let left = if col == 0 { self.cols - 1 } else { col - 1 };
                let right = if col + 1 == self.cols { 0 } else { col + 1 };

                let c = self.cells[self.idx(col, below)];
                let l = self.cells[self.idx(left, below)];
                let r = self.cells[self.idx(right, below)];

                // Weights 6:1:1 — dominated by straight-up rise.
                let avg = (c * 6.0 + l + r) / 8.0;

                // Random per-cell cooling. Uniform decay would let the (small)
                // sideways term equalise each row over time; the jitter keeps
                // neighbouring columns at genuinely different heights.
                let jitter = 1.0 - self.decay * (0.2 + self.rng.next_f32() * 1.6);

                // Extra cooling over the topmost rows so the fire fades out
                // instead of being sliced off at the grid boundary.
                //
                // Row 0 has no neighbour above it, so nothing cools it further
                // and whatever heat reaches it renders as a hard flat line
                // across the full width — a horizontal streak, not a flame.
                // (Measured on pHub: the top ~3 rows of every panel showed
                // ramp-coloured pixels in straight lines.) This is the mirror
                // of the off-screen seed row at the bottom.
                let fade = top_fade(row, self.rows);

                let i = self.idx(col, row);
                self.cells[i] = (avg * jitter * fade).clamp(0.0, 1.0);
            }
        }
    }
}

impl crate::animation::AsciiAnimation for Fire {
    fn name(&self) -> &'static str {
        "fire"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        Fire::resize(self, cols, rows);
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        Fire::step(self);
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, crate::palette::Rgb)> {
        // One dithered index lookup serves both the glyph and the colour.
        let idx = self.glyph_index_at(col, row);
        // Index 0 is the blank glyph. Returning None (rather than Some(' '))
        // tells the renderer to skip the cell entirely — important behind a
        // transparent terminal, where a painted space is a visible smudge.
        if idx == 0 {
            None
        } else {
            Some((RAMP[idx], crate::palette::color_for(idx)))
        }
    }

    fn background(&self) -> crate::palette::Rgb {
        crate::palette::BACKGROUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32());
            assert!((0.0..1.0).contains(&x), "rng out of range: {x}");
        }
    }

    #[test]
    fn rng_survives_zero_seed() {
        let mut r = Rng::new(0);
        // A naive xorshift seeded at zero emits only zeros forever.
        assert!((0..50).any(|_| r.next_f32() > 0.0));
    }

    #[test]
    fn values_stay_in_range_and_finite() {
        let mut f = Fire::new(80, 25, 7);
        for _ in 0..500 {
            f.step();
            for row in 0..f.rows {
                for col in 0..f.cols {
                    let h = f.heat_at(col, row);
                    assert!(h.is_finite(), "non-finite heat at {col},{row}");
                    assert!((0.0..=1.0).contains(&h), "heat {h} out of range");
                }
            }
        }
    }

    #[test]
    fn fire_actually_rises() {
        // After enough frames heat should have climbed well above the seed row.
        let mut f = Fire::new(80, 25, 3);
        for _ in 0..200 {
            f.step();
        }
        let mid = f.rows / 2;
        let any_heat_at_mid = (0..f.cols).any(|c| f.heat_at(c, mid) > 0.0);
        assert!(any_heat_at_mid, "fire never reached the middle row");
    }

    #[test]
    fn bottom_is_hotter_than_top() {
        let mut f = Fire::new(80, 25, 11);
        for _ in 0..300 {
            f.step();
        }
        let row_avg = |r: usize| -> f32 {
            (0..f.cols).map(|c| f.heat_at(c, r)).sum::<f32>() / f.cols as f32
        };
        assert!(
            row_avg(f.rows - 1) > row_avg(0),
            "fire is upside down: bottom {} vs top {}",
            row_avg(f.rows - 1),
            row_avg(0)
        );
    }

    #[test]
    fn top_of_fire_goes_cold() {
        // Regression: the first kernel produced a smooth heat gradient with no
        // cold region at all — the top row was solid mid-ramp glyphs. It looked
        // nothing like fire, yet passed every "bottom hotter than top" check.
        // Fire must actually burn out before it reaches the top.
        let mut f = Fire::new(78, 20, 1234);
        for _ in 0..300 {
            f.step();
        }
        let top_avg: f32 = (0..f.cols).map(|c| f.heat_at(c, 0)).sum::<f32>() / f.cols as f32;
        assert!(top_avg < 0.12, "fire never cools: top row avg heat {top_avg}");
    }

    #[test]
    fn has_cold_gaps_not_just_a_gradient() {
        // A real fire has dark space between tongues. Require that a good
        // fraction of the upper half is fully cold (empty glyph).
        let mut f = Fire::new(78, 20, 99);
        for _ in 0..300 {
            f.step();
        }
        let mut cold = 0usize;
        let mut total = 0usize;
        for row in 0..f.rows / 2 {
            for col in 0..f.cols {
                total += 1;
                if f.glyph_index_at(col, row) == 0 {
                    cold += 1;
                }
            }
        }
        let frac = cold as f32 / total as f32;
        assert!(frac > 0.5, "upper half is not mostly empty (cold fraction {frac})");
    }

    #[test]
    fn rows_are_not_flat_bands() {
        // Regression: with uniform decay, sideways averaging smoothed each row
        // into a single value, producing flat horizontal stripes — every cell
        // in a row the same glyph. It was cold on top and hot on the bottom,
        // so the earlier tests all passed, but it looked like a layer cake
        // rather than fire. Require real horizontal variety in the active band.
        let mut f = Fire::new(78, 20, 1234);
        for _ in 0..300 {
            f.step();
        }
        let mut varied_rows = 0;
        for row in 0..f.rows {
            let distinct = (0..f.cols)
                .map(|c| f.glyph_index_at(c, row))
                .collect::<std::collections::HashSet<_>>()
                .len();
            if distinct >= 2 {
                varied_rows += 1;
            }
        }
        assert!(
            varied_rows >= f.rows / 2,
            "fire is banded: only {varied_rows}/{} rows have >1 glyph",
            f.rows
        );
    }

    #[test]
    fn flames_have_vertical_structure() {
        // Regression: an evenly-weighted left/centre/right kernel blurs each
        // row horizontally every frame, so the fire had no tongues — just
        // stacked bands (later, dithered bands). Cold/hot must vary along a
        // COLUMN's height differently for different columns, i.e. flames reach
        // noticeably different heights across the grid.
        let mut f = Fire::new(78, 20, 1234);
        for _ in 0..400 {
            f.step();
        }
        // Height of each column = topmost row with any heat.
        let heights: Vec<usize> = (0..f.cols)
            .map(|c| {
                (0..f.rows)
                    .find(|&r| f.glyph_index_at(c, r) > 0)
                    .map_or(0, |top| f.rows - top)
            })
            .collect();
        let max = *heights.iter().max().unwrap();
        let min = *heights.iter().min().unwrap();
        assert!(
            max - min >= 3,
            "no vertical structure: flame heights span only {min}..{max}"
        );
    }

    #[test]
    fn seed_row_is_off_screen() {
        // The random source row must not be visible, or the bottom line of the
        // panel is a solid wall of hot glyphs rather than flame roots.
        let mut f = Fire::new(40, 10, 5);
        for _ in 0..100 {
            f.step();
        }
        // Visible bottom row must not be uniformly maxed out.
        let all_hot = (0..f.cols).all(|c| f.glyph_index_at(c, f.rows - 1) == RAMP.len() - 1);
        assert!(!all_hot, "bottom visible row is a solid seed wall");
    }

    #[test]
    fn flames_scale_with_panel_height() {
        // Regression: a fixed decay burned out after ~15 rows regardless of
        // panel size. On a real 85-row terminal that left the top two thirds
        // completely empty — the fire looked like a stripe along the bottom
        // edge. Flames must reach a similar FRACTION of the height at any size.
        for rows in [25usize, 50, 85] {
            let mut f = Fire::new(80, rows, 4242);
            for _ in 0..(rows * 12) {
                f.step();
            }
            // Topmost row containing any lit glyph.
            let top_lit = (0..f.rows).find(|&r| (0..f.cols).any(|c| f.glyph_index_at(c, r) > 0));
            let reach = match top_lit {
                Some(top) => (f.rows - top) as f32 / f.rows as f32,
                None => 0.0,
            };
            assert!(
                reach > 0.45,
                "flames only reach {:.0}% of a {rows}-row panel",
                reach * 100.0
            );
            assert!(
                reach < 0.98,
                "flames fill {:.0}% of a {rows}-row panel — no cold sky left",
                reach * 100.0
            );
        }
    }

    #[test]
    fn top_rows_are_completely_dark() {
        // Regression: row 0 has no neighbour above it, so nothing cooled it and
        // whatever heat arrived there rendered as a hard flat line across the
        // full panel width. On screen this read as horizontal streaks at the
        // top of every terminal, not as fire. The topmost rows must be FULLY
        // dark, not merely dim — a single lit cell in row 0 is a visible line.
        for rows in [25usize, 50, 85] {
            let mut f = Fire::new(120, rows, 777);
            for _ in 0..(rows * 12) {
                f.step();
            }
            for row in 0..2 {
                let lit = (0..f.cols).filter(|&c| f.glyph_index_at(c, row) > 0).count();
                assert_eq!(
                    lit, 0,
                    "row {row} of a {rows}-row panel has {lit} lit cells (should be 0)"
                );
            }
        }
    }

    #[test]
    fn glyph_index_never_exceeds_ramp() {
        let mut f = Fire::new(40, 12, 99);
        for _ in 0..200 {
            f.step();
            for row in 0..f.rows {
                for col in 0..f.cols {
                    assert!(f.glyph_index_at(col, row) < RAMP.len());
                }
            }
        }
    }

    #[test]
    fn zero_sized_does_not_panic() {
        let mut f = Fire::new(0, 0, 1);
        f.step();
        f.resize(10, 10);
        f.step();
    }
}
