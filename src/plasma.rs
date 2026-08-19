//! Classic demoscene plasma.
//!
//! The oldest trick in the book: sum a few sine waves of the cell's position
//! and the clock, map the total through a ramp. No state, no buffers — every
//! cell is a pure function of `(x, y, t)`, which is why it is nearly free and
//! why it resizes instantly.
//!
//! The version here is the standard four-term field:
//!
//! ```text
//! v = sin(x·f)
//!   + sin(y·f/2 + t)
//!   + sin((x + y)·f/2 + t)
//!   + sin(√(x² + y²)·f + t)
//! ```
//!
//! The last term is what stops it reading as a plaid: the first three are all
//! axis-aligned, and the radial one breaks that up with a circular front moving
//! through them.
//!
//! # Cell aspect
//!
//! A character cell is about twice as tall as it is wide, so the y coordinate
//! is scaled by half before it enters the field. Without that every circular
//! feature comes out as a vertical ellipse — the same correction `wizardtorch`
//! makes for the same reason.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;

/// Default ramp, sparsest first.
const DEFAULT_RAMP: &str = " .:-=+*#%@";

pub struct Plasma {
    cols: usize,
    rows: usize,
    t: f32,
    frame_ms: u64,

    /// Animation speed, x1000.
    speed_milli: i64,
    /// Feature size, x1000. Higher = finer, busier field.
    scale_milli: i64,
    /// How many extra octaves of detail are folded in, 1..4.
    complexity: usize,
    /// Contrast applied to the normalised field, x1000.
    contrast_milli: i64,

    ramp: Vec<char>,
    /// The two ends of the colour sweep; each cell mixes between them.
    lo: Rgb,
    hi: Rgb,
    bg: Rgb,
}

impl Plasma {
    pub fn new(cols: usize, rows: usize, chars: Option<&str>) -> Self {
        Plasma {
            cols,
            rows,
            t: 0.0,
            frame_ms: 100,
            speed_milli: 1000,
            scale_milli: 1000,
            complexity: 3,
            contrast_milli: 1000,
            ramp: ramp_from(chars),
            lo: Rgb(0x1b, 0x2a, 0x6b),
            hi: Rgb(0xff, 0x9a, 0x3c),
            bg: Rgb(0, 0, 0),
        }
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// The plasma field at a cell, normalised to 0..1.
    fn field(&self, col: usize, row: usize) -> f32 {
        let scale = (self.scale_milli as f32 / 1000.0).max(0.01);
        // Normalise to the SHORT axis so the feature size is the same on a
        // 2560x720 ultrawide as on a square panel; dividing each axis by its own
        // length stretches the pattern to the screen instead.
        let short = self.cols.min(self.rows).max(1) as f32;
        // Measured FROM THE CENTRE, in units of the short axis.
        //
        // Two things fall out of that, and both matter. The radial term needs a
        // centre, and the panel's middle is the only one that makes sense --
        // anchoring at a corner puts the rings off-screen on a wide panel.
        // And measuring from the centre is what keeps the feature SIZE fixed:
        // an offset that grows with `cols` (as `col / short` does) stretches
        // every wave on a wide panel, which measured as 19 cells per period
        // against 9 on a square one.
        let x = (col as f32 - self.cols as f32 / 2.0) / short * 8.0 * scale;
        // Doubled: a character cell is about twice as tall as it is wide, so an
        // unscaled row delta makes every circle a vertical ellipse.
        let y = (row as f32 - self.rows as f32 / 2.0) / short * 8.0 * scale * 2.0;
        let t = self.t * std::f32::consts::TAU;

        let mut v = (x).sin() + (y * 0.5 + t).sin() + ((x + y) * 0.5 + t).sin();
        // Radial term: breaks up the axis-aligned plaid the three above would
        // make on their own. Centred, so it reads as rings from the middle.
        v += ((x * x + y * y).sqrt() - t * 2.0).sin();
        let mut terms = 4.0;

        // Extra octaves: same field at higher frequency and lower weight.
        for o in 1..self.complexity.clamp(1, 4) {
            let f = 1.0 + o as f32;
            let w = 1.0 / f;
            v += ((x * f).sin() + (y * f * 0.5 + t * f).sin()) * w;
            terms += 2.0 * w;
        }

        // Mean of the terms, mapped from [-1,1] to [0,1].
        let mean = v / terms;
        let contrast = self.contrast_milli as f32 / 1000.0;
        ((mean * contrast) * 0.5 + 0.5).clamp(0.0, 1.0)
    }
}

/// Build a ramp from an override, falling back to the default.
///
/// An empty or whitespace-only override would leave nothing to draw with, so it
/// falls back rather than producing an invisible effect.
fn ramp_from(chars: Option<&str>) -> Vec<char> {
    match chars {
        Some(s) if !s.trim().is_empty() => s.chars().collect(),
        _ => DEFAULT_RAMP.chars().collect(),
    }
}

impl AsciiAnimation for Plasma {
    fn name(&self) -> &'static str {
        "plasma"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        // Wall-clock paced, so the field drifts at the same rate behind a 30fps
        // terminal and on a 5fps wallpaper.
        let dt = (self.frame_ms as f32 / 1000.0) * (self.speed_milli as f32 / 1000.0) * 0.08;
        self.t = (self.t + dt) % 1.0;
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let v = self.field(col, row);
        let idx = ((v * self.ramp.len() as f32) as usize).min(self.ramp.len() - 1);
        let ch = self.ramp[idx];
        if ch == ' ' {
            // The dimmest ramp slot draws nothing at all rather than a space:
            // these panels sit behind a semi-transparent terminal, where a
            // painted space is a visible smudge.
            return None;
        }
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * v) as u8;
        Some((
            ch,
            Rgb(
                mix(self.lo.0, self.hi.0),
                mix(self.lo.1, self.hi.1),
                mix(self.lo.2, self.hi.2),
            ),
        ))
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("speed", "speed (x1000)", self.speed_milli, 50, 5000),
            Param::int("scale", "feature scale (x1000)", self.scale_milli, 100, 5000),
            Param::int("complexity", "octaves", self.complexity as i64, 1, 4),
            Param::int("contrast", "contrast (x1000)", self.contrast_milli, 200, 3000),
            Param::text("chars", "ramp", &self.ramp.iter().collect::<String>()),
            Param::colour("lo", "low colour", self.lo),
            Param::colour("hi", "high colour", self.hi),
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
            "scale" => match v.as_int() {
                Some(n) => {
                    self.scale_milli = n.clamp(100, 5000);
                    true
                }
                None => false,
            },
            "complexity" => match v.as_int() {
                Some(n) => {
                    self.complexity = n.clamp(1, 4) as usize;
                    true
                }
                None => false,
            },
            "contrast" => match v.as_int() {
                Some(n) => {
                    self.contrast_milli = n.clamp(200, 3000);
                    true
                }
                None => false,
            },
            "chars" => match v.as_text() {
                Some(s) if !s.trim().is_empty() => {
                    self.ramp = s.chars().collect();
                    true
                }
                _ => false,
            },
            "lo" => match v.as_rgb() {
                Some(c) => {
                    self.lo = c;
                    true
                }
                None => false,
            },
            "hi" => match v.as_rgb() {
                Some(c) => {
                    self.hi = c;
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

    fn built() -> Plasma {
        Plasma::new(80, 40, None)
    }

    #[test]
    fn the_field_stays_normalised() {
        // The ramp index is derived from this, so a value outside 0..1 either
        // panics on the index or silently clamps to one end and flattens the
        // whole effect.
        let mut p = built();
        for _ in 0..40 {
            p.step();
            for r in 0..p.rows {
                for c in 0..p.cols {
                    let v = p.field(c, r);
                    assert!((0.0..=1.0).contains(&v), "field out of range: {v}");
                }
            }
        }
    }

    #[test]
    fn the_field_moves() {
        let mut p = built();
        let before = p.field(20, 10);
        for _ in 0..10 {
            p.step();
        }
        assert!((p.field(20, 10) - before).abs() > 1e-4, "plasma is static");
    }

    #[test]
    fn it_is_paced_by_time_not_frame_count() {
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
        assert!((fast.t - slow.t).abs() < 1e-4, "{} != {}", fast.t, slow.t);
    }

    #[test]
    fn feature_size_does_not_stretch_with_the_panel() {
        // Normalising each axis by its OWN length would stretch the pattern to
        // the screen: an ultrawide would show a square panel's picture, just
        // wider. Both axes are normalised by the SHORT one instead, so a
        // feature is the same number of cells across whatever the panel shape.
        //
        // Measured as the distance between zero crossings along a row, which is
        // the feature size directly. The absolute VALUES may differ between
        // panels -- the radial term is centred on each panel's own middle, and
        // should be -- so comparing samples point-for-point would assert
        // something the effect does not (and should not) promise.
        fn period(p: &Plasma, row: usize) -> usize {
            let mut crossings = Vec::new();
            let mut prev = p.field(0, row) - 0.5;
            for c in 1..p.cols {
                let cur = p.field(c, row) - 0.5;
                if (prev < 0.0) != (cur < 0.0) {
                    crossings.push(c);
                }
                prev = cur;
            }
            if crossings.len() < 2 {
                return 0;
            }
            (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1)
        }
        let wide = Plasma::new(200, 60, None);
        let square = Plasma::new(60, 60, None);
        let (a, b) = (period(&wide, 30), period(&square, 30));
        assert!(a > 0 && b > 0, "no crossings found: {a}, {b}");
        // Within 2x, not exact. The radial term is centred, so a row of a wide
        // panel genuinely crosses more ring fronts than the same row of a
        // square one -- that is the effect working, not stretching. What this
        // catches is the real failure: normalising each axis by its own length,
        // which measured 19 vs 9 cells here (>2x) before the field was centred.
        let (lo, hi) = (a.min(b) as f32, a.max(b) as f32);
        assert!(
            hi / lo < 2.0,
            "feature size stretched with the panel: {a} vs {b} cells"
        );
    }

    #[test]
    fn an_empty_ramp_override_falls_back() {
        // An empty ramp has no glyph to draw, so the effect would be invisible
        // with nothing to say why.
        let p = Plasma::new(10, 10, Some("   "));
        assert_eq!(p.ramp, DEFAULT_RAMP.chars().collect::<Vec<_>>());
        let mut q = built();
        assert!(!q.set_param("chars", &ParamValue::Text { v: "".into() }));
    }

    #[test]
    fn the_dimmest_slot_draws_nothing() {
        // A painted space is a visible smudge behind a translucent terminal.
        let mut p = Plasma::new(10, 10, Some(" #"));
        p.set_param("contrast", &ParamValue::Int { v: 3000 });
        let any_none = (0..10)
            .flat_map(|r| (0..10).map(move |c| (c, r)))
            .any(|(c, r)| p.cell_at(c, r).is_none());
        assert!(any_none, "no cell was left unpainted");
    }

    #[test]
    fn out_of_range_cells_draw_nothing() {
        let p = built();
        assert_eq!(p.cell_at(999, 0), None);
        assert_eq!(p.cell_at(0, 999), None);
    }

    #[test]
    fn unknown_params_are_rejected() {
        let mut p = built();
        assert!(!p.set_param("nope", &ParamValue::Int { v: 1 }));
    }
}
