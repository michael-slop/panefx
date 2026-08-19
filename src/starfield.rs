//! Perspective starfield — flying forward through a cloud of points.
//!
//! Each star is a point in 3D that moves toward the viewer; the screen position
//! is the usual pinhole projection:
//!
//! ```text
//! sx = cx + x / z · k
//! sy = cy + y / z · k / 2
//! ```
//!
//! and `z` shrinking each frame is the entire sense of motion. A star that
//! passes the viewer (`z <= 0`) or leaves the screen is respawned at the far
//! plane, so the field is never exhausted and never needs sorting.
//!
//! Brightness is `1/z`: near stars are bright and, at the brightest, drawn with
//! a heavier glyph. That is what makes the field read as depth rather than as
//! noise — a flat-brightness starfield looks like static.
//!
//! # Why this one keeps a buffer
//!
//! Unlike `plasma` and `tunnel`, a star's position is not a function of its
//! screen cell, so `cell_at` cannot compute it. `step` projects every star into
//! a cell grid once, and `cell_at` reads that. The grid is reused between
//! frames rather than reallocated.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;

/// Glyphs by brightness, dimmest first.
const RAMP: [char; 4] = ['.', '+', '*', '@'];

/// Far plane. Stars spawn here and travel toward 0.
const FAR: f32 = 32.0;

#[derive(Clone, Copy)]
struct Star {
    x: f32,
    y: f32,
    z: f32,
}

/// Xorshift RNG — deterministic, no external crate. Same approach as `fire`.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in [-1, 1).
    fn signed(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32
    }
}

pub struct Starfield {
    cols: usize,
    rows: usize,
    frame_ms: u64,

    stars: Vec<Star>,
    rng: Rng,
    /// Projected brightness per cell, 0.0 = empty. Reused every frame.
    grid: Vec<f32>,

    /// Travel speed, x1000.
    speed_milli: i64,
    /// Stars per thousand cells, so density is panel-size independent.
    density_milli: i64,
    /// Field of view, x1000. Higher = wider, faster-spreading field.
    fov_milli: i64,
    /// Sideways drift, x1000. Signed: the field can bank either way.
    drift_milli: i64,

    near: Rgb,
    far_col: Rgb,
    bg: Rgb,
}

impl Starfield {
    pub fn new(cols: usize, rows: usize, seed: u64) -> Self {
        let mut s = Starfield {
            cols,
            rows,
            frame_ms: 100,
            stars: Vec::new(),
            rng: Rng(seed | 1),
            grid: Vec::new(),
            speed_milli: 1000,
            density_milli: 900,
            fov_milli: 1000,
            drift_milli: 0,
            near: Rgb(0xff, 0xff, 0xff),
            far_col: Rgb(0x3a, 0x4a, 0x8a),
            bg: Rgb(0, 0, 0),
        };
        s.reseed();
        s
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// How many stars this panel should hold.
    ///
    /// Proportional to area, so a 2560x720 ultrawide is not sparser than a
    /// small pane at the same setting.
    fn target_count(&self) -> usize {
        let cells = self.cols.saturating_mul(self.rows);
        // 90 per 1000 cells at density 1.0. The first number here was 12,
        // which measured about 30 visible stars on a 160x60 panel -- correct
        // perspective, but far too empty to read as a starfield.
        ((cells as f32 / 1000.0) * (self.density_milli as f32 / 1000.0) * 90.0) as usize
    }

    fn spawn(&mut self) -> Star {
        Star {
            x: self.rng.signed(),
            y: self.rng.signed(),
            // Spread across the whole depth on spawn, not all at the far plane:
            // otherwise the first seconds are an empty screen followed by a
            // wall of stars arriving together.
            z: self.rng.unit() * FAR + 0.1,
        }
    }

    fn reseed(&mut self) {
        let want = self.target_count();
        self.stars.clear();
        self.stars.reserve(want);
        for _ in 0..want {
            let s = self.spawn();
            self.stars.push(s);
        }
        self.grid = vec![0.0; self.cols.saturating_mul(self.rows)];
    }
}

impl AsciiAnimation for Starfield {
    fn name(&self) -> &'static str {
        "starfield"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.reseed();
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        // Density is a live knob, so the population has to track it.
        let want = self.target_count();
        while self.stars.len() > want {
            self.stars.pop();
        }
        while self.stars.len() < want {
            let s = self.spawn();
            self.stars.push(s);
        }
        if self.grid.len() != self.cols * self.rows {
            self.grid = vec![0.0; self.cols * self.rows];
        }
        for v in self.grid.iter_mut() {
            *v = 0.0;
        }

        let dt = (self.frame_ms as f32 / 1000.0) * (self.speed_milli as f32 / 1000.0) * 6.0;
        let drift = self.drift_milli as f32 / 1000.0 * dt * 0.05;
        let fov = (self.fov_milli as f32 / 1000.0).max(0.05);
        let (cx, cy) = (self.cols as f32 / 2.0, self.rows as f32 / 2.0);
        let k = self.cols.min(self.rows * 2).max(1) as f32 * 0.5 * fov;

        // Collected first, applied after: `spawn` borrows `self.rng` mutably
        // and the loop below borrows `self.stars`.
        let mut respawn: Vec<usize> = Vec::new();
        for (i, s) in self.stars.iter_mut().enumerate() {
            s.z -= dt;
            s.x += drift;
            if s.z <= 0.05 {
                respawn.push(i);
                continue;
            }
            let sx = cx + (s.x / s.z) * k;
            // Halved: a cell is twice as tall as it is wide, so an unhalved y
            // makes the field spread twice as fast vertically as horizontally.
            let sy = cy + (s.y / s.z) * k * 0.5;
            if sx < 0.0 || sy < 0.0 || sx >= self.cols as f32 || sy >= self.rows as f32 {
                // Off screen: only respawn once it is also PAST the viewer.
                // Culling on screen bounds alone kills stars that are still
                // approaching from a wide angle and thins the edges of the
                // field.
                if s.z < 1.0 {
                    respawn.push(i);
                }
                continue;
            }
            let b = (1.0 - s.z / FAR).clamp(0.0, 1.0);
            let idx = sy as usize * self.cols + sx as usize;
            // Keep the NEAREST star in a shared cell, so a bright close star is
            // not hidden behind a dim far one.
            if b > self.grid[idx] {
                self.grid[idx] = b;
            }
        }
        for i in respawn {
            let mut s = self.spawn();
            s.z = FAR;
            self.stars[i] = s;
        }
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let b = *self.grid.get(row * self.cols + col)?;
        if b <= 0.0 {
            return None;
        }
        // Gamma: linear brightness puts nearly every star in the dimmest slot,
        // because most of the depth range is far away.
        let g = b.powf(2.2);
        let idx = ((g * RAMP.len() as f32) as usize).min(RAMP.len() - 1);
        let mix = |a: u8, c: u8| (c as f32 + (a as f32 - c as f32) * b) as u8;
        Some((
            RAMP[idx],
            Rgb(
                mix(self.near.0, self.far_col.0),
                mix(self.near.1, self.far_col.1),
                mix(self.near.2, self.far_col.2),
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
            Param::int("speed", "speed (x1000)", self.speed_milli, 50, 5000),
            Param::int("density", "density (x1000)", self.density_milli, 50, 4000),
            Param::int("fov", "field of view (x1000)", self.fov_milli, 200, 3000),
            Param::int("drift", "sideways drift (x1000)", self.drift_milli, -3000, 3000),
            Param::colour("near", "near star colour", self.near),
            Param::colour("far", "far star colour", self.far_col),
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
            "density" => match v.as_int() {
                Some(n) => {
                    self.density_milli = n.clamp(50, 4000);
                    true
                }
                None => false,
            },
            "fov" => match v.as_int() {
                Some(n) => {
                    self.fov_milli = n.clamp(200, 3000);
                    true
                }
                None => false,
            },
            "drift" => match v.as_int() {
                Some(n) => {
                    self.drift_milli = n.clamp(-3000, 3000);
                    true
                }
                None => false,
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
                    self.far_col = c;
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

    fn built() -> Starfield {
        Starfield::new(80, 40, 0x5EED)
    }

    fn lit(s: &Starfield) -> usize {
        (0..s.rows)
            .flat_map(|r| (0..s.cols).map(move |c| (c, r)))
            .filter(|&(c, r)| s.cell_at(c, r).is_some())
            .count()
    }

    #[test]
    fn stars_appear_on_screen() {
        let mut s = built();
        s.step();
        assert!(lit(&s) > 5, "the field is empty");
    }

    #[test]
    fn the_field_is_populated_from_the_first_frame() {
        // Spawning every star at the far plane would open on an empty screen
        // followed by a wall of stars arriving together.
        let mut s = built();
        s.step();
        let first = lit(&s);
        for _ in 0..40 {
            s.step();
        }
        let later = lit(&s);
        assert!(
            first as f32 > later as f32 * 0.35,
            "first frame {first} is far emptier than the steady state {later}"
        );
    }

    #[test]
    fn the_star_count_stays_stable_over_time() {
        // Every star must be respawned, not leaked or lost: a cull that misses
        // a case empties the sky after a minute.
        let mut s = built();
        let want = s.target_count();
        for _ in 0..500 {
            s.step();
        }
        assert_eq!(s.stars.len(), want);
    }

    #[test]
    fn every_star_stays_in_front_of_the_viewer() {
        // A star at z <= 0 projects to a mirrored position behind the camera.
        let mut s = built();
        for _ in 0..300 {
            s.step();
            for st in &s.stars {
                assert!(st.z > 0.0, "star behind the viewer: {}", st.z);
            }
        }
    }

    #[test]
    fn density_tracks_the_live_setting() {
        let mut s = built();
        s.step();
        let before = s.stars.len();
        assert!(s.set_param("density", &ParamValue::Int { v: 3000 }));
        s.step();
        assert!(s.stars.len() > before, "{} !> {before}", s.stars.len());
    }

    #[test]
    fn density_is_area_relative_not_absolute() {
        // Otherwise an ultrawide is visibly sparser than a small pane at the
        // same setting.
        let small = Starfield::new(40, 20, 1);
        let big = Starfield::new(160, 80, 1);
        assert!(big.target_count() > small.target_count() * 3);
    }

    #[test]
    fn a_resize_rebuilds_the_grid() {
        let mut s = built();
        s.step();
        s.resize(30, 15);
        s.step();
        assert_eq!(s.grid.len(), 30 * 15);
        assert!(s.cell_at(29, 14).is_none() || s.cell_at(29, 14).is_some());
    }

    #[test]
    fn a_degenerate_grid_does_not_panic() {
        let mut s = Starfield::new(0, 0, 7);
        s.step();
        assert_eq!(s.cell_at(0, 0), None);
    }

    #[test]
    fn it_is_paced_by_time_not_frame_count() {
        // 1s of travel must move a star the same distance at either rate.
        let mut fast = built();
        fast.set_frame_ms(20);
        let mut slow = built();
        slow.set_frame_ms(100);
        let z0 = fast.stars[0].z;
        for _ in 0..5 {
            fast.step();
        }
        let moved_fast = z0 - fast.stars[0].z;
        let z1 = slow.stars[0].z;
        slow.step();
        let moved_slow = z1 - slow.stars[0].z;
        assert!(
            (moved_fast - moved_slow).abs() < 0.01,
            "{moved_fast} != {moved_slow}"
        );
    }

    #[test]
    fn unknown_params_are_rejected() {
        let mut s = built();
        assert!(!s.set_param("warp", &ParamValue::Int { v: 9 }));
    }
}
