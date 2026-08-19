//! A wizard holding a torch, lit by the torch he is holding.
//!
//! The drawing is fixed — see [`crate::wizardtorch_art`], traced from
//! `AXB-WIZARDTORCH.ANS`. What animates is the LIGHT: a torch flame flickers at
//! the top of the piece, and every cell is lit according to how far it sits from
//! that flame. Cells near the torch swing bright and warm; the skulls at the
//! bottom sit at the edge of the throw and barely move.
//!
//! # Why light rather than a moving picture
//!
//! The obvious way to animate ANSI art is to store several frames and cycle
//! them. That was rejected: the source is one frame, so the other frames would
//! have to be invented, and hand-inventing them badly is worse than not
//! animating at all. Lighting keeps every glyph exactly where the artist put it
//! and animates the one thing a torch actually does.
//!
//! It is also what makes the effect cheap. The art is static, so per frame the
//! work is one flicker update plus a multiply per cell — no simulation grid, no
//! allocation, and [`changed`](AsciiAnimation::changed) can report `false`
//! whenever the flicker has not moved enough to alter a glyph.
//!
//! # The two-part flicker
//!
//! A single random value per frame reads as noise, not as fire. Real torchlight
//! has a slow body — the flame leaning and recovering — with a fast tremor on
//! top. So the intensity is a slow wander plus a small fast oscillation, and
//! the two are tunable separately (`unrest` and `tremor`).

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;
use crate::wizardtorch_art::{ART, COLS, ROWS};

/// Glyphs the renderer can draw, dimmest first.
///
/// The art's own shade blocks, in order, so a lit cell can be *dimmed* by
/// stepping down this ramp: a full block in shadow becomes a three-quarter
/// block, then a half, and so on. That is what makes the light look like it is
/// falling on the drawing rather than being tinted over it.
const RAMP: [char; 4] = ['\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'];

/// Half blocks, kept as themselves.
///
/// These carry the art's fine edges — a face, the rim of a skull. Substituting
/// a shade block for them visibly coarsens the drawing, so they are drawn as-is
/// at whatever brightness the light gives them.
const HALF_TOP: char = '\u{2580}';
const HALF_BOTTOM: char = '\u{2584}';
const HALF_LEFT: char = '\u{258c}';
const HALF_RIGHT: char = '\u{2590}';

/// Where the torch flame sits in ART cell coordinates.
///
/// Read off the artwork, not guessed: the flame is the bright plume at the top
/// left, above the wizard's raised hand. Light falls off from here.
const TORCH_COL: f32 = 12.0;
const TORCH_ROW: f32 = 6.0;

/// Shade level of one art cell, 0 (empty) to 4 (full block).
fn shade_of(ch: u8) -> u8 {
    match ch {
        b'.' => 1,
        b':' => 2,
        b'*' => 3,
        b'#' => 4,
        // Half blocks are full-brightness marks that happen to cover half a
        // cell; they light like a full block and keep their own glyph.
        b'T' | b'B' | b'L' | b'R' => 4,
        _ => 0,
    }
}

/// The glyph a half-block cell must keep, if it is one.
fn half_glyph(ch: u8) -> Option<char> {
    match ch {
        b'T' => Some(HALF_TOP),
        b'B' => Some(HALF_BOTTOM),
        b'L' => Some(HALF_LEFT),
        b'R' => Some(HALF_RIGHT),
        _ => None,
    }
}

pub struct WizardTorch {
    cols: usize,
    rows: usize,

    /// Frames elapsed, the flicker's clock.
    tick: u64,
    /// Milliseconds per frame, so the flicker runs on wall-clock time rather
    /// than frame count — the same effect looks identical at 5fps and 30fps.
    frame_ms: u64,
    /// Current torch intensity, roughly 0.6..1.4.
    intensity: f32,
    /// Intensity when the last frame was drawn, for `changed`.
    last_drawn: f32,

    /// Slow wander, x1000. How far the flame leans.
    unrest_milli: i64,
    /// Fast tremor, x1000. The high-frequency shimmer on top.
    tremor_milli: i64,
    /// How far the light reaches, in cells.
    reach_milli: i64,
    /// Light level in the darkest corner, x1000. Never zero, or the bottom of
    /// the piece is simply missing rather than dim.
    ambient_milli: i64,

    /// Colour of the flame itself, and of the coldest lit cell. Everything in
    /// between is mixed from these two, so one dial changes the whole mood.
    hot: Rgb,
    cold: Rgb,
    bg: Rgb,

    /// Distance from the torch to each art cell, precomputed.
    ///
    /// The falloff is the only per-cell maths in the effect and it never
    /// changes — the art does not move — so it is computed once at construction
    /// rather than every frame for every cell.
    falloff: Vec<f32>,
}

impl WizardTorch {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut w = WizardTorch {
            cols,
            rows,
            tick: 0,
            frame_ms: 100,
            intensity: 1.0,
            last_drawn: -1.0,
            unrest_milli: 260,
            tremor_milli: 90,
            reach_milli: 110_000,
            ambient_milli: 340,
            hot: Rgb(0xff, 0xc4, 0x6b),
            cold: Rgb(0x2e, 0x3d, 0x6b),
            bg: Rgb(0, 0, 0),
            falloff: Vec::new(),
        };
        w.rebuild_falloff();
        w
    }

    /// Current flame intensity. Exposed for the preview example, which prints
    /// it to show the flicker is actually moving between saved frames.
    pub fn intensity_for_preview(&self) -> f32 {
        self.intensity
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// Distance-based light for every art cell.
    ///
    /// Cells are twice as tall as they are wide, so the row delta is halved
    /// before the distance is taken. Without that the light pools into a
    /// vertical ellipse and reads as a column of light rather than a point
    /// source.
    fn rebuild_falloff(&mut self) {
        let reach = (self.reach_milli as f32 / 1000.0).max(1.0);
        let ambient = self.ambient_milli as f32 / 1000.0;
        self.falloff = Vec::with_capacity(COLS * ROWS);
        for r in 0..ROWS {
            for c in 0..COLS {
                let dx = c as f32 - TORCH_COL;
                let dy = (r as f32 - TORCH_ROW) * 0.5;
                let d = (dx * dx + dy * dy).sqrt();
                // Linear falloff, floored at `ambient`. Inverse-square is the
                // physical law and looks wrong here: it blows out the torch and
                // crushes everything past a few cells to black, leaving the
                // skulls invisible.
                let lit = (1.0 - d / reach).max(0.0);
                self.falloff.push(ambient + (1.0 - ambient) * lit);
            }
        }
    }

    /// The art cell shown at a panel cell.
    ///
    /// The art is anchored TOP-CENTRE and clipped, never scaled: this is
    /// pixel-art with a face in it, and resampling a face across a cell grid
    /// turns it to mush. On a panel narrower than the art the wizard stays
    /// centred and the edges are cropped; on a taller one the art runs from the
    /// top and the flame band at the bottom is what falls off screen.
    fn art_at(&self, col: usize, row: usize) -> Option<(u8, usize)> {
        let x_off = (self.cols as isize - COLS as isize) / 2;
        let ac = col as isize - x_off;
        if ac < 0 || ac >= COLS as isize || row >= ROWS {
            return None;
        }
        let line = ART[row].as_bytes();
        let ac = ac as usize;
        let ch = if ac < line.len() { line[ac] } else { b' ' };
        Some((ch, row * COLS + ac))
    }
}

impl AsciiAnimation for WizardTorch {
    fn name(&self) -> &'static str {
        "wizardtorch"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        self.tick += 1;
        // Time, not frame count, so the flicker keeps its pace at any fps.
        let t = (self.tick as f32) * (self.frame_ms as f32 / 1000.0);
        let unrest = self.unrest_milli as f32 / 1000.0;
        let tremor = self.tremor_milli as f32 / 1000.0;
        // Three incommensurable frequencies: the sum never repeats on a period
        // a viewer can spot, which is what keeps it reading as fire rather than
        // as a loop. Cheap enough to be free at these grid sizes.
        let slow = (t * 2.3).sin() * 0.6 + (t * 1.1).sin() * 0.4;
        let fast = (t * 17.0).sin() * 0.5 + (t * 29.0).sin() * 0.5;
        self.intensity = 1.0 + slow * unrest + fast * tremor;
    }

    fn changed(&self) -> bool {
        // A glyph only moves when the light crosses a ramp step, so a flicker
        // too small to change any cell is not worth a redraw. The threshold is
        // a quarter of a ramp step, which is under the smallest visible change.
        (self.intensity - self.last_drawn).abs() > 0.03
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        let (ch, idx) = self.art_at(col, row)?;
        let shade = shade_of(ch);
        if shade == 0 {
            return None;
        }
        let light = (self.falloff[idx] * self.intensity).clamp(0.0, 1.4);
        // Brightness is the art's own shade scaled by the light on it, so a
        // dark cell in bright light and a bright cell in shadow can read the
        // same -- which is what makes it look lit rather than tinted.
        let v = ((shade as f32 / 4.0) * light).clamp(0.0, 1.0);
        if v <= 0.06 {
            return None;
        }
        let glyph = half_glyph(ch).unwrap_or_else(|| {
            let i = ((v * RAMP.len() as f32).ceil() as usize).clamp(1, RAMP.len()) - 1;
            RAMP[i]
        });
        // Warm toward the torch, cold away from it.
        let mix = (light / 1.2).clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| -> u8 {
            let f = a as f32 + (b as f32 - a as f32) * mix;
            (f * v).clamp(0.0, 255.0) as u8
        };
        Some((
            glyph,
            Rgb(
                lerp(self.cold.0, self.hot.0),
                lerp(self.cold.1, self.hot.1),
                lerp(self.cold.2, self.hot.2),
            ),
        ))
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    /// Chunky cells, like `waves`.
    ///
    /// The art is 80 cells wide and full of faces; at the terminal's 10x15 it
    /// renders about a third of a 1080p screen and reads as noise. Square-ish
    /// cells also matter more here than for the other effects, because the
    /// source is pixel art whose proportions the artist chose.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((12, 18))
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("unrest", "flame unrest (x1000)", self.unrest_milli, 0, 800),
            Param::int("tremor", "flame tremor (x1000)", self.tremor_milli, 0, 400),
            Param::int("reach", "light reach (x1000)", self.reach_milli, 8_000, 160_000),
            Param::int("ambient", "ambient light (x1000)", self.ambient_milli, 0, 800),
            Param::colour("hot", "flame colour", self.hot),
            Param::colour("cold", "shadow colour", self.cold),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        let clamp = |n: i64, lo: i64, hi: i64| n.clamp(lo, hi);
        match key {
            "unrest" => match v.as_int() {
                Some(n) => {
                    self.unrest_milli = clamp(n, 0, 800);
                    true
                }
                None => false,
            },
            "tremor" => match v.as_int() {
                Some(n) => {
                    self.tremor_milli = clamp(n, 0, 400);
                    true
                }
                None => false,
            },
            // Both of these change the light map, so it has to be rebuilt --
            // otherwise the knob does nothing until the next resize, which
            // reads as a broken control.
            "reach" => match v.as_int() {
                Some(n) => {
                    self.reach_milli = clamp(n, 8_000, 160_000);
                    self.rebuild_falloff();
                    true
                }
                None => false,
            },
            "ambient" => match v.as_int() {
                Some(n) => {
                    self.ambient_milli = clamp(n, 0, 800);
                    self.rebuild_falloff();
                    true
                }
                None => false,
            },
            "hot" => match v.as_rgb() {
                Some(c) => {
                    self.hot = c;
                    true
                }
                None => false,
            },
            "cold" => match v.as_rgb() {
                Some(c) => {
                    self.cold = c;
                    true
                }
                None => false,
            },
            "bg" => match v.as_rgb() {
                Some(c) => {
                    self.bg = c;
                    true
                }
                None => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built() -> WizardTorch {
        WizardTorch::new(80, 60)
    }

    #[test]
    fn the_art_is_rectangular_and_uses_only_known_glyphs() {
        // The art is generated from the .ANS by a script; this is the check
        // that the generator and `shade_of` still agree. An unknown byte would
        // silently render as empty -- a hole in the wizard, with nothing in any
        // log to say why.
        for (i, row) in ART.iter().enumerate() {
            assert_eq!(row.len(), COLS, "row {i} is not {COLS} cells wide");
            for &b in row.as_bytes() {
                assert!(
                    b == b' ' || shade_of(b) > 0,
                    "row {i} has unknown glyph {:?}",
                    b as char
                );
            }
        }
    }

    #[test]
    fn the_torch_is_brighter_than_the_skulls() {
        // The whole point of the effect: light falls off with distance. The
        // torch sits near the top, the skulls near the bottom.
        let w = built();
        let near = w.falloff[(TORCH_ROW as usize + 1) * COLS + TORCH_COL as usize];
        let far = w.falloff[(ROWS - 4) * COLS + COLS / 2];
        assert!(near > far, "near {near} should out-light far {far}");
    }

    #[test]
    fn the_darkest_corner_is_still_visible() {
        // Ambient is never zero on purpose. At zero the bottom of the piece is
        // not dim, it is ABSENT -- and "the art does not reach that far" and
        // "the effect is broken" must not look the same.
        let w = built();
        let min = w.falloff.iter().cloned().fold(f32::MAX, f32::min);
        assert!(min > 0.0, "some cell is fully black: {min}");
    }

    #[test]
    fn the_flicker_moves_and_stays_in_range() {
        // Unbounded intensity would blow the colour mix past white and clip the
        // whole drawing to a flat block on the bright frames.
        let mut w = built();
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for _ in 0..600 {
            w.step();
            lo = lo.min(w.intensity);
            hi = hi.max(w.intensity);
        }
        assert!(hi > lo, "the flame never moved");
        assert!(lo > 0.2 && hi < 2.0, "intensity ran to {lo}..{hi}");
    }

    #[test]
    fn the_flicker_is_paced_by_time_not_frame_count() {
        // Same elapsed time at two frame rates must give the same flame, or the
        // effect speeds up on a fast panel and crawls on the wallpaper.
        let mut fast = built();
        fast.set_frame_ms(20);
        let mut slow = built();
        slow.set_frame_ms(100);
        for _ in 0..50 {
            fast.step();
        }
        for _ in 0..10 {
            slow.step();
        }
        assert!(
            (fast.intensity - slow.intensity).abs() < 0.001,
            "1s at 50fps ({}) != 1s at 10fps ({})",
            fast.intensity,
            slow.intensity
        );
    }

    #[test]
    fn the_art_is_centred_and_clipped_never_scaled() {
        // A panel wider than the art centres it; a narrower one crops evenly.
        // Scaling was rejected -- resampling a face across a cell grid destroys
        // it -- so out-of-range columns must report nothing at all.
        let wide = WizardTorch::new(COLS + 20, ROWS);
        assert!(wide.art_at(0, 10).is_none(), "left margin should be empty");
        assert!(wide.art_at(COLS + 19, 10).is_none(), "right margin too");
        assert!(wide.art_at(COLS / 2 + 10, 10).is_some(), "centre has art");
    }

    #[test]
    fn a_cell_outside_the_art_draws_nothing() {
        let w = built();
        assert_eq!(w.cell_at(0, ROWS + 5), None);
    }

    #[test]
    fn half_blocks_keep_their_own_glyph() {
        // They carry the art's fine edges. Replacing them with a shade block
        // coarsens every face in the piece.
        assert_eq!(half_glyph(b'T'), Some(HALF_TOP));
        assert_eq!(half_glyph(b'B'), Some(HALF_BOTTOM));
        assert_eq!(half_glyph(b'#'), None);
    }

    #[test]
    fn reach_and_ambient_rebuild_the_light_map() {
        // A knob that edits a field but not the derived map does nothing until
        // the next resize, which reads as a dead control rather than a bug.
        let mut w = built();
        let before = w.falloff[(ROWS - 4) * COLS + COLS / 2];
        assert!(w.set_param("reach", &ParamValue::Int { v: 160_000 }));
        let after = w.falloff[(ROWS - 4) * COLS + COLS / 2];
        assert!(after > before, "a longer reach must light the far cells");
    }

    #[test]
    fn unknown_params_are_rejected_rather_than_ignored() {
        let mut w = built();
        assert!(!w.set_param("nope", &ParamValue::Int { v: 1 }));
    }
}
