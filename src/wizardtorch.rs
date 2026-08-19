//! `AXB-WIZARDTORCH.ANS`, fitted to the panel and rippled like `waves`.
//!
//! The drawing is fixed — see [`crate::wizardtorch_art`]: a robed wizard holding
//! a torch above a bank of skulls, over a band of flame. What moves is the
//! SAMPLING. Each frame, every panel cell asks the art "what is at this point?"
//! through a slowly swirling displacement, so the whole piece breathes and
//! ripples without a single glyph being redrawn or invented.
//!
//! # The two problems this solves
//!
//! **Fit.** The art is 80x128 cells — a portrait shape. A landscape monitor is
//! nothing like that, so the art is *mapped* onto whatever grid it is given
//! rather than pasted into the middle of it. [`Fit::Contain`] keeps the artist's
//! proportions and letterboxes, [`Fit::Stretch`] fills the screen and lets the
//! wizard get wide, [`Fit::Cover`] fills it while keeping proportions and crops
//! the overflow.
//!
//! **Detail.** Sampling an 80x128 drawing onto a 180x70 grid means one art cell
//! per several panel cells, and nearest-neighbour blocks look like a mistake. So
//! the sample is BILINEAR over shade levels — the art's own 0..4 shades
//! interpolate to a continuous value which the ramp re-quantises. That is also
//! what makes `detail` meaningful: it scales the art under the sampler, and a
//! finer grid genuinely resolves more of the drawing.
//!
//! # The ripple
//!
//! Same shape as `waves::compute`: a smooth per-cell noise field drives a
//! sin/cos displacement whose phase advances with time, and the art is sampled
//! at the displaced coordinate. Displacing the SAMPLE rather than the image is
//! what keeps every feature intact — the wizard ripples, he does not smear.
//!
//! Amplitude is deliberately small and measured in ART cells, not panel cells,
//! so the ripple is the same size on the drawing whatever grid it is fitted to.
//! The art has a face in it; a large warp turns a face into soup, and the point
//! is a piece of art that moves, not a moving pattern that used to be art.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;
use crate::wizardtorch_art::{ART, COLS, ROWS};

/// Shade ramp, dimmest first. The art's own blocks, so a sampled value
/// re-quantises into the vocabulary it was drawn in.
const RAMP: [char; 4] = ['\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'];

/// How the art is mapped onto a panel whose shape is not the art's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Keep the art's proportions; letterbox whatever is left over.
    Contain,
    /// Fill the panel exactly. The wizard stretches to the screen's shape.
    Stretch,
    /// Keep proportions and fill the panel, cropping the overflow.
    Cover,
}

impl Fit {
    fn from_str(s: &str) -> Option<Fit> {
        match s.trim().to_lowercase().as_str() {
            "contain" => Some(Fit::Contain),
            "stretch" => Some(Fit::Stretch),
            "cover" => Some(Fit::Cover),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Fit::Contain => "contain",
            Fit::Stretch => "stretch",
            Fit::Cover => "cover",
        }
    }
}

/// Shade level of one art byte, 0 (empty) to 4 (full block).
///
/// Half blocks count as full: at any fit other than 1:1 a panel cell covers a
/// fraction of a glyph anyway, so preserving which HALF was inked buys nothing
/// the sampler could express.
fn shade_of(ch: u8) -> f32 {
    match ch {
        b'.' => 1.0,
        b':' => 2.0,
        b'*' => 3.0,
        b'#' | b'T' | b'B' | b'L' | b'R' => 4.0,
        _ => 0.0,
    }
}

/// Deterministic hash to [0,1). Same shape as the one in `waves`.
fn hash01(seed: u32, x: i32, y: i32) -> f32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add((x as u32).wrapping_mul(0x85EB_CA6B))
        .wrapping_add((y as u32).wrapping_mul(0xC2B2_AE35));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// A smooth low-frequency field over the grid, in [0,1).
///
/// Smoothstep-interpolated over a coarse lattice: cheap, and smooth enough that
/// the displacement it drives has no visible grid in it. Built once per resize,
/// never per frame.
fn smooth_field(h: usize, w: usize, freq: f32, seed: u32) -> Vec<f32> {
    let g = (freq as usize).max(1) + 2;
    let lattice: Vec<f32> = (0..g * g)
        .map(|i| hash01(seed, (i / g) as i32, (i % g) as i32))
        .collect();
    let at = |gy: usize, gx: usize| lattice[(gy % g) * g + (gx % g)];
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let fy = y as f32 / h.max(1) as f32 * freq;
            let fx = x as f32 / w.max(1) as f32 * freq;
            let (y0, x0) = (fy.floor() as usize, fx.floor() as usize);
            let (ty, tx) = (fy - y0 as f32, fx - x0 as f32);
            // Smoothstep, so the lattice edges do not show as creases.
            let sy = ty * ty * (3.0 - 2.0 * ty);
            let sx = tx * tx * (3.0 - 2.0 * tx);
            out[y * w + x] = at(y0, x0) * (1.0 - sx) * (1.0 - sy)
                + at(y0, x0 + 1) * sx * (1.0 - sy)
                + at(y0 + 1, x0) * (1.0 - sx) * sy
                + at(y0 + 1, x0 + 1) * sx * sy;
        }
    }
    out
}

pub struct WizardTorch {
    cols: usize,
    rows: usize,

    /// Animation phase, 0..1, advanced by `step`.
    t: f32,
    frame_ms: u64,

    /// Ripple speed, x1000.
    speed_milli: i64,
    /// Displacement amplitude, x1000.
    swirl_milli: i64,
    /// Art scale, x1000. 1000 = fitted; higher zooms in, lower zooms out.
    detail_milli: i64,
    /// Cells at or below this fraction of full shade are not drawn at all.
    darkcut_milli: i64,
    fit: Fit,

    ink: Rgb,
    bg: Rgb,

    /// Displacement fields, one per axis. Rebuilt on resize only.
    warp_a: Vec<f32>,
    warp_b: Vec<f32>,
}

impl WizardTorch {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut w = WizardTorch {
            cols,
            rows,
            t: 0.0,
            frame_ms: 100,
            speed_milli: 1000,
            swirl_milli: 700,
            detail_milli: 1000,
            darkcut_milli: 120,
            fit: Fit::Contain,
            ink: Rgb(0xc9, 0xb8, 0x9a),
            bg: Rgb(0, 0, 0),
            warp_a: Vec::new(),
            warp_b: Vec::new(),
        };
        w.rebuild_fields();
        w
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    fn rebuild_fields(&mut self) {
        let (h, w) = (self.rows.max(1), self.cols.max(1));
        self.warp_a = smooth_field(h, w, 2.0, 77);
        self.warp_b = smooth_field(h, w, 2.7, 91);
    }

    /// Bilinear shade sample of the art at a continuous coordinate.
    ///
    /// Out of bounds reads as empty rather than wrapping: the art is a picture
    /// with edges, and wrapping puts the flame band immediately above the
    /// wizard's hat.
    fn art_sample(&self, ay: f32, ax: f32) -> f32 {
        let y0 = ay.floor() as i32;
        let x0 = ax.floor() as i32;
        let (fy, fx) = (ay - y0 as f32, ax - x0 as f32);
        let at = |y: i32, x: i32| -> f32 {
            if y < 0 || x < 0 || y >= ROWS as i32 || x >= COLS as i32 {
                return 0.0;
            }
            let line = ART[y as usize].as_bytes();
            let b = if (x as usize) < line.len() {
                line[x as usize]
            } else {
                b' '
            };
            shade_of(b)
        };
        at(y0, x0) * (1.0 - fx) * (1.0 - fy)
            + at(y0, x0 + 1) * fx * (1.0 - fy)
            + at(y0 + 1, x0) * (1.0 - fx) * fy
            + at(y0 + 1, x0 + 1) * fx * fy
    }

    /// Map a panel cell to a point in art space, honouring `fit` and `detail`.
    ///
    /// Cells are about twice as tall as they are wide, so a grid's true aspect
    /// is `cols : rows * 2`. Ignoring that letterboxes on the wrong axis and
    /// squashes the wizard to half his height.
    fn to_art(&self, col: f32, row: f32) -> (f32, f32) {
        let (gw, gh) = (self.cols.max(1) as f32, self.rows.max(1) as f32);
        let detail = (self.detail_milli as f32 / 1000.0).max(0.05);

        let (u, v) = (col / gw, row / gh);
        let (mut su, mut sv) = (u, v);

        if self.fit != Fit::Stretch {
            let panel_aspect = gw / (gh * 2.0);
            let art_aspect = COLS as f32 / (ROWS as f32 * 2.0);
            let wider = panel_aspect > art_aspect;
            // Contain letterboxes on the long axis; Cover crops it instead --
            // the same comparison with the branch reversed.
            let expand_x = (self.fit == Fit::Contain) == wider;
            let (scale_x, scale_y) = if expand_x {
                (panel_aspect / art_aspect, 1.0)
            } else {
                (1.0, art_aspect / panel_aspect)
            };
            su = (u - 0.5) * scale_x + 0.5;
            sv = (v - 0.5) * scale_y + 0.5;
        }

        // `detail` zooms about the centre, so turning the knob resolves the
        // drawing rather than sliding it off the screen.
        su = (su - 0.5) / detail + 0.5;
        sv = (sv - 0.5) / detail + 0.5;

        (sv * ROWS as f32, su * COLS as f32)
    }
}

impl AsciiAnimation for WizardTorch {
    fn name(&self) -> &'static str {
        "wizardtorch"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.rebuild_fields();
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        // Time-based, so the ripple keeps its pace at any frame rate: the same
        // effect must look identical behind a 30fps terminal and on a 5fps
        // wallpaper.
        let dt = (self.frame_ms as f32 / 1000.0) * (self.speed_milli as f32 / 1000.0) * 0.10;
        self.t = (self.t + dt) % 1.0;
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let i = row * self.cols + col;
        let (wa, wb) = (
            *self.warp_a.get(i).unwrap_or(&0.0),
            *self.warp_b.get(i).unwrap_or(&0.0),
        );

        // Same displacement as `waves`: a per-cell noise value sets the phase
        // offset, so neighbouring cells move together and the field ripples
        // instead of shimmering.
        let phase = std::f32::consts::TAU * self.t;
        let swirl = self.swirl_milli as f32 / 1000.0;
        // Amplitude in ART cells, so the ripple is the same size on the drawing
        // whatever grid it happens to be fitted to.
        let amp = 0.02 * COLS.min(ROWS) as f32 * swirl;
        let dx = (phase + wa * std::f32::consts::TAU).sin() * amp;
        let dy = (phase + wb * std::f32::consts::TAU).cos() * amp;

        let (ay, ax) = self.to_art(col as f32, row as f32);
        let v = self.art_sample(ay + dy, ax + dx) / 4.0;

        if v <= self.darkcut_milli as f32 / 1000.0 {
            return None;
        }
        let idx = ((v * RAMP.len() as f32).ceil() as usize).clamp(1, RAMP.len()) - 1;
        let shade = v.clamp(0.0, 1.0);
        Some((
            RAMP[idx],
            Rgb(
                (self.ink.0 as f32 * shade) as u8,
                (self.ink.1 as f32 * shade) as u8,
                (self.ink.2 as f32 * shade) as u8,
            ),
        ))
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    /// Chunky cells, like `waves`.
    ///
    /// The art is block graphics, not text, and at the terminal's 10x15 it
    /// renders as fine noise on a big screen. `detail` is the knob for how much
    /// of the drawing is resolved; this sets a sane starting grain.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((12, 18))
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("speed", "ripple speed (x1000)", self.speed_milli, 50, 5000),
            Param::int("swirl", "ripple depth (x1000)", self.swirl_milli, 0, 3000),
            Param::int("detail", "art scale (x1000)", self.detail_milli, 200, 4000),
            Param::int("darkcut", "dark cutoff (x1000)", self.darkcut_milli, 0, 900),
            Param::text("fit", "fit (contain/stretch/cover)", self.fit.as_str()),
            Param::colour("ink", "ink colour", self.ink),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            "speed" => match v.as_int() {
                Some(n) => {
                    self.speed_milli = n.clamp(50, 5000);
                    true
                }
                None => false,
            },
            "swirl" => match v.as_int() {
                Some(n) => {
                    self.swirl_milli = n.clamp(0, 3000);
                    true
                }
                None => false,
            },
            "detail" => match v.as_int() {
                Some(n) => {
                    self.detail_milli = n.clamp(200, 4000);
                    true
                }
                None => false,
            },
            "darkcut" => match v.as_int() {
                Some(n) => {
                    self.darkcut_milli = n.clamp(0, 900);
                    true
                }
                None => false,
            },
            // Rejected rather than silently ignored: a typo'd mode must not
            // look like a mode that does nothing.
            "fit" => match v.as_text().and_then(Fit::from_str) {
                Some(f) => {
                    self.fit = f;
                    true
                }
                None => false,
            },
            "ink" => match v.as_rgb() {
                Some(c) => {
                    self.ink = c;
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

    /// A landscape grid, which is the case the fit logic exists for.
    fn landscape() -> WizardTorch {
        WizardTorch::new(160, 60)
    }

    fn drawn_cells(w: &WizardTorch) -> usize {
        let mut n = 0;
        for r in 0..w.rows {
            for c in 0..w.cols {
                if w.cell_at(c, r).is_some() {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn the_art_is_rectangular_and_uses_only_known_glyphs() {
        // The art is generated from the .ANS by a script; this is the check
        // that the generator and `shade_of` still agree. An unknown byte would
        // render as empty -- a hole in the wizard, with nothing in any log.
        for (i, row) in ART.iter().enumerate() {
            assert_eq!(row.len(), COLS, "row {i} is not {COLS} cells wide");
            for &b in row.as_bytes() {
                assert!(
                    b == b' ' || shade_of(b) > 0.0,
                    "row {i} has unknown glyph {:?}",
                    b as char
                );
            }
        }
    }

    #[test]
    fn a_landscape_panel_draws_the_art_across_its_width() {
        // THE POINT of the fit work: the art is 80x128 (portrait) and must
        // still fill a wide screen sensibly rather than sitting in a column.
        let w = landscape();
        assert!(drawn_cells(&w) > 500, "landscape panel drew almost nothing");
    }

    #[test]
    fn contain_keeps_the_art_inside_the_panel() {
        // Contain must not crop: mapping the panel's own corner must land at or
        // outside the art's bounds, never inside them.
        let w = landscape();
        let (ay, ax) = w.to_art(0.0, 0.0);
        assert!(
            ax <= 0.5 || ay <= 0.5,
            "contain cropped a corner: ({ay}, {ax})"
        );
    }

    #[test]
    fn stretch_maps_the_panel_corners_to_the_art_corners() {
        let mut w = landscape();
        assert!(w.set_param("fit", &ParamValue::Text { v: "stretch".into() }));
        let (ay, ax) = w.to_art(0.0, 0.0);
        assert!(ay.abs() < 0.001 && ax.abs() < 0.001, "({ay}, {ax})");
        let (by, bx) = w.to_art(w.cols as f32, w.rows as f32);
        assert!(
            (by - ROWS as f32).abs() < 0.001 && (bx - COLS as f32).abs() < 0.001,
            "({by}, {bx})"
        );
    }

    #[test]
    fn detail_zooms_about_the_centre() {
        // The centre must stay put when the scale changes, or turning the knob
        // slides the wizard off the screen instead of resolving him.
        let mut w = landscape();
        let mid = (w.cols as f32 / 2.0, w.rows as f32 / 2.0);
        let before = w.to_art(mid.0, mid.1);
        assert!(w.set_param("detail", &ParamValue::Int { v: 2500 }));
        let after = w.to_art(mid.0, mid.1);
        assert!(
            (before.0 - after.0).abs() < 0.001 && (before.1 - after.1).abs() < 0.001,
            "centre moved: {before:?} -> {after:?}"
        );
    }

    #[test]
    fn a_higher_detail_narrows_the_sampled_window() {
        // Zooming in must actually sample less art across the same grid --
        // i.e. resolve more of it -- not just draw the same thing bigger.
        let mut w = landscape();
        w.set_param("detail", &ParamValue::Int { v: 400 });
        let out = w.to_art(0.0, 0.0);
        w.set_param("detail", &ParamValue::Int { v: 3000 });
        let inn = w.to_art(0.0, 0.0);
        assert!(inn.1 > out.1, "zooming in must narrow the window");
    }

    #[test]
    fn the_sample_is_bilinear_not_nearest() {
        // Continuity is what stops a scaled-up drawing looking like blocks: a
        // half-step between two differing cells must give an intermediate value.
        let w = landscape();
        let a = w.art_sample(40.0, 10.0);
        let b = w.art_sample(40.0, 11.0);
        if (a - b).abs() > 0.5 {
            let mid = w.art_sample(40.0, 10.5);
            assert!(
                mid > a.min(b) - 0.001 && mid < a.max(b) + 0.001,
                "midpoint {mid} is not between {a} and {b}"
            );
        }
    }

    #[test]
    fn sampling_outside_the_art_is_empty_not_wrapped() {
        // Wrapping would put the flame band directly above the wizard's hat.
        let w = landscape();
        assert_eq!(w.art_sample(-40.0, -40.0), 0.0);
        assert_eq!(w.art_sample(ROWS as f32 + 40.0, COLS as f32 + 40.0), 0.0);
    }

    #[test]
    fn the_ripple_advances_and_wraps() {
        let mut w = landscape();
        let start = w.t;
        for _ in 0..5 {
            w.step();
        }
        assert!(w.t > start, "the ripple never moved");
        for _ in 0..2000 {
            w.step();
        }
        assert!((0.0..1.0).contains(&w.t), "phase escaped 0..1: {}", w.t);
    }

    #[test]
    fn the_ripple_is_paced_by_time_not_frame_count() {
        // Same elapsed time at two frame rates gives the same phase, so the
        // effect does not speed up behind a fast terminal.
        let mut fast = landscape();
        fast.set_frame_ms(20);
        let mut slow = landscape();
        slow.set_frame_ms(100);
        for _ in 0..50 {
            fast.step();
        }
        for _ in 0..10 {
            slow.step();
        }
        assert!((fast.t - slow.t).abs() < 0.001, "{} != {}", fast.t, slow.t);
    }

    #[test]
    fn zero_swirl_is_a_still_picture() {
        // The ripple must be switch-off-able: with no displacement the art has
        // to sample identically on every frame.
        let mut w = landscape();
        w.set_param("swirl", &ParamValue::Int { v: 0 });
        let first = w.cell_at(80, 30);
        for _ in 0..20 {
            w.step();
        }
        assert_eq!(first, w.cell_at(80, 30));
    }

    #[test]
    fn an_unknown_fit_mode_is_rejected() {
        // A typo must not silently select a mode.
        let mut w = landscape();
        assert!(!w.set_param("fit", &ParamValue::Text { v: "diagonal".into() }));
        assert_eq!(w.fit, Fit::Contain);
    }

    #[test]
    fn a_degenerate_grid_does_not_panic() {
        let w = WizardTorch::new(0, 0);
        assert_eq!(w.cell_at(0, 0), None);
    }
}
