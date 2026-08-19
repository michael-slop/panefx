//! The classic demoscene tunnel.
//!
//! Every cell is converted to polar coordinates about the centre, and those two
//! numbers index a texture:
//!
//! ```text
//! depth = k / radius        (perspective: far things are small)
//! angle = atan2(dy, dx)
//! ```
//!
//! Scrolling `depth` with time pulls the texture toward the viewer; scrolling
//! `angle` spins it. The illusion is entirely in `k / radius` — that division is
//! what makes a flat checkerboard read as a receding shaft.
//!
//! The texture is a checkerboard computed on the fly rather than stored: it is
//! two floors and an xor, and a table would be a cache miss per cell for no
//! benefit.
//!
//! # Cell aspect
//!
//! Character cells are about twice as tall as they are wide, so the y delta is
//! doubled before the radius is taken. Without that the "circular" mouth of the
//! tunnel is a tall ellipse.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::{quantise, Rgb};

const DEFAULT_RAMP: &str = " .:-=+*#%@";

/// Colour steps per channel. See `palette::quantise`.
const COLOUR_LEVELS: u8 = 6;

pub struct Tunnel {
    cols: usize,
    rows: usize,
    t: f32,
    frame_ms: u64,

    /// Forward travel, x1000.
    speed_milli: i64,
    /// Rotation about the axis, x1000. Signed: negative spins the other way.
    spin_milli: i64,
    /// Texture cell count around the circumference.
    slices: i64,
    /// Texture cell count along the depth axis, x1000.
    rings_milli: i64,
    /// How fast brightness falls with depth, x1000.
    fog_milli: i64,
    /// Brightness at or below this is not drawn at all, x1000.
    ///
    /// See the note on `plasma::darkcut_milli`: every lit cell is a GDI text
    /// call, so leaving the dark half of the checkerboard unpainted is what
    /// brings a full-screen effect down to `waves`-like cost. It also deepens
    /// the tunnel, because the far end genuinely goes to black.
    darkcut_milli: i64,

    ramp: Vec<char>,
    near: Rgb,
    far: Rgb,
    bg: Rgb,

    /// Per-cell `(depth, angle)`, precomputed.
    ///
    /// Both come from `sqrt` and `atan2` of the cell's offset from the centre,
    /// and NEITHER depends on time -- only on the grid. Computing them per frame
    /// cost ~100% of a core across four monitors (measured); computing them once
    /// per resize costs nothing per frame.
    ///
    /// `None` for the centre cell, where the radius is zero.
    polar: Vec<Option<(f32, f32)>>,
}

impl Tunnel {
    pub fn new(cols: usize, rows: usize, chars: Option<&str>) -> Self {
        let mut t = Tunnel {
            cols,
            rows,
            t: 0.0,
            frame_ms: 100,
            speed_milli: 1000,
            spin_milli: 300,
            slices: 12,
            rings_milli: 1000,
            fog_milli: 1000,
            darkcut_milli: 420,
            ramp: match chars {
                Some(s) if !s.trim().is_empty() => s.chars().collect(),
                _ => DEFAULT_RAMP.chars().collect(),
            },
            near: Rgb(0x8a, 0xe6, 0xff),
            far: Rgb(0x10, 0x18, 0x4a),
            bg: Rgb(0, 0, 0),
            polar: Vec::new(),
        };
        t.rebuild_polar();
        t
    }

    /// Precompute the polar map for the current grid.
    fn rebuild_polar(&mut self) {
        let (cx, cy) = (self.cols as f32 / 2.0, self.rows as f32 / 2.0);
        let short = self.cols.min(self.rows).max(1) as f32;
        self.polar = Vec::with_capacity(self.cols * self.rows);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let dx = col as f32 - cx;
                // Doubled: cells are twice as tall as wide, so this makes the
                // tunnel mouth circular rather than a tall ellipse.
                let dy = (row as f32 - cy) * 2.0;
                let r = (dx * dx + dy * dy).sqrt();
                if r < 0.5 {
                    self.polar.push(None);
                    continue;
                }
                let depth = (short * 0.5) / r;
                let a = dy.atan2(dx) / std::f32::consts::TAU;
                self.polar.push(Some((depth, a)));
            }
        }
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// `(brightness, depth)` at a cell, both 0..1-ish.
    ///
    /// Returns `None` at the exact centre, where the radius is zero and the
    /// depth would be infinite.
    fn sample(&self, col: usize, row: usize) -> Option<(f32, f32)> {
        let (depth, a) = (*self.polar.get(row * self.cols + col)?)?;
        let rings = (self.rings_milli as f32 / 1000.0).max(0.05);
        let spin = self.spin_milli as f32 / 1000.0;

        let u = a * self.slices.max(1) as f32 + self.t * spin * self.slices.max(1) as f32;
        let v = depth * 4.0 * rings + self.t * 8.0;

        // Checkerboard: xor of the two parities.
        let cell = ((u.floor() as i64) & 1) ^ ((v.floor() as i64) & 1);

        // Fog: far parts of the shaft fade out, which is most of the depth cue.
        let fog = (self.fog_milli as f32 / 1000.0).max(0.0);
        let lit = (1.0 / (1.0 + depth * 0.35 * fog)).clamp(0.0, 1.0);
        let brightness = if cell == 0 { lit } else { lit * 0.45 };
        Some((brightness, depth))
    }
}

impl AsciiAnimation for Tunnel {
    fn name(&self) -> &'static str {
        "tunnel"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.rebuild_polar();
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        let dt = (self.frame_ms as f32 / 1000.0) * (self.speed_milli as f32 / 1000.0) * 0.05;
        // NOT wrapped to 0..1: the texture coordinate is `t * 8`, so wrapping
        // the phase at 1.0 would jump the tunnel eight texture cells and show
        // as a visible hitch. Wrapped at 8.0, the jump lands exactly on a
        // texture period and is invisible.
        self.t = (self.t + dt) % 8.0;
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let (b, _depth) = self.sample(col, row)?;
        if b <= self.darkcut_milli as f32 / 1000.0 {
            return None;
        }
        let idx = ((b * self.ramp.len() as f32) as usize).min(self.ramp.len() - 1);
        let ch = self.ramp[idx];
        if ch == ' ' {
            return None;
        }
        let mix = |a: u8, c: u8| (c as f32 + (a as f32 - c as f32) * b) as u8;
        Some((
            ch,
            quantise(
                Rgb(
                    mix(self.near.0, self.far.0),
                    mix(self.near.1, self.far.1),
                    mix(self.near.2, self.far.2),
                ),
                COLOUR_LEVELS,
            ),
        ))
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    /// Chunky cells, like `waves`.
    ///
    /// Block graphics, not text: at the terminal's 10x15 these render as fine
    /// noise on a big screen.
    ///
    /// Note this was ALSO tried as a performance fix and is not one -- going
    /// from 10x15 to 15x23 cut the cell count 2.4x and moved CPU by under 1%.
    /// The cost is draw calls, not cells; see `palette::quantise`.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((15, 23))
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("speed", "travel speed (x1000)", self.speed_milli, 50, 5000),
            Param::int("spin", "rotation (x1000)", self.spin_milli, -3000, 3000),
            Param::int("slices", "slices around", self.slices, 2, 48),
            Param::int("rings", "ring density (x1000)", self.rings_milli, 100, 4000),
            Param::int("fog", "depth fade (x1000)", self.fog_milli, 0, 3000),
            Param::int("darkcut", "dark cutoff (x1000)", self.darkcut_milli, 0, 900),
            Param::text("chars", "ramp", &self.ramp.iter().collect::<String>()),
            Param::colour("near", "near colour", self.near),
            Param::colour("far", "far colour", self.far),
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
            "spin" => match v.as_int() {
                Some(n) => {
                    self.spin_milli = n.clamp(-3000, 3000);
                    true
                }
                None => false,
            },
            "slices" => match v.as_int() {
                Some(n) => {
                    self.slices = n.clamp(2, 48);
                    true
                }
                None => false,
            },
            "rings" => match v.as_int() {
                Some(n) => {
                    self.rings_milli = n.clamp(100, 4000);
                    true
                }
                None => false,
            },
            "fog" => match v.as_int() {
                Some(n) => {
                    self.fog_milli = n.clamp(0, 3000);
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
            "chars" => match v.as_text() {
                Some(s) if !s.trim().is_empty() => {
                    self.ramp = s.chars().collect();
                    true
                }
                _ => false,
            },
            "near" => match v.as_rgb() {
                Some(c) => {
                    self.near = c;
                    true
                }
                None => false,
            },
            "far" => match v.as_rgb() {
                Some(c) => {
                    self.far = c;
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

    fn built() -> Tunnel {
        Tunnel::new(80, 40, None)
    }

    #[test]
    fn the_centre_is_not_a_division_by_zero() {
        // radius 0 gives infinite depth; the sample must decline rather than
        // produce a NaN that indexes the ramp.
        let t = built();
        assert_eq!(t.sample(40, 20), None);
    }

    #[test]
    fn brightness_stays_in_range_everywhere() {
        let mut t = built();
        for _ in 0..30 {
            t.step();
            for r in 0..t.rows {
                for c in 0..t.cols {
                    if let Some((b, d)) = t.sample(c, r) {
                        assert!((0.0..=1.0).contains(&b), "brightness {b}");
                        assert!(d.is_finite() && d > 0.0, "depth {d}");
                    }
                }
            }
        }
    }

    #[test]
    fn far_cells_are_dimmer_than_near_ones() {
        // The whole depth cue. A cell near the centre is FAR (small radius =>
        // large depth) and must be dimmer than one at the edge.
        let t = built();
        let mut near_edge = 0.0f32;
        let mut far_centre = 1.0f32;
        for r in 0..t.rows {
            for c in 0..t.cols {
                if let Some((b, d)) = t.sample(c, r) {
                    if d < 1.0 {
                        near_edge = near_edge.max(b);
                    }
                    if d > 8.0 {
                        far_centre = far_centre.min(b);
                    }
                }
            }
        }
        assert!(near_edge > far_centre, "{near_edge} !> {far_centre}");
    }

    #[test]
    fn the_phase_wraps_on_a_texture_period() {
        // Wrapping at 1.0 would jump the texture 8 cells and show as a hitch;
        // the wrap has to land on a whole texture period.
        let mut t = built();
        for _ in 0..5000 {
            t.step();
            assert!((0.0..8.0).contains(&t.t), "phase escaped: {}", t.t);
        }
    }

    #[test]
    fn the_tunnel_moves() {
        let mut t = built();
        let before = t.sample(10, 10).map(|s| s.0);
        for _ in 0..20 {
            t.step();
        }
        assert_ne!(before, t.sample(10, 10).map(|s| s.0));
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
        assert!((fast.t - slow.t).abs() < 1e-4);
    }

    #[test]
    fn spin_may_be_negative() {
        // Both directions are legitimate; a clamp to 0 would silently drop half
        // the range.
        let mut t = built();
        assert!(t.set_param("spin", &ParamValue::Int { v: -2000 }));
        assert_eq!(t.spin_milli, -2000);
    }

    #[test]
    fn out_of_range_cells_draw_nothing() {
        let t = built();
        assert_eq!(t.cell_at(999, 0), None);
    }
}
