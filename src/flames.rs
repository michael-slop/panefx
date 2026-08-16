//! "Flames" — a faithful port of msimpson's gist.
//!
//! Source read before porting (LAW 1):
//!   https://gist.github.com/msimpson/1096950
//!
//! This is a DIFFERENT algorithm from `fire.rs`, not a retuning of it, and the
//! differences are the whole point:
//!
//!   * Heat is an INTEGER 0..=65 and indexes the glyph table directly
//!     (`char[min(b[i], 9)]`). There is no float-to-bucket quantisation step,
//!     which is exactly where `fire.rs` produced horizontal banding.
//!   * Seeding is SPARSE — only `width/9` randomly chosen cells per frame are
//!     set hot, rather than the whole bottom row. That is what gives discrete
//!     rising sources instead of a solid sheet of flame.
//!   * The kernel is asymmetric: `(self + right + below + below_right) / 4`,
//!     with integer division doing all the cooling. There is no separate decay
//!     term.
//!   * The update is IN-PLACE over a flat array, so each cell reads
//!     already-updated neighbours. This is load-bearing: a double-buffered
//!     version of the same kernel looks different.
//!
//! The original's colours are four curses pairs keyed to value thresholds
//! (>15, >9, >4, else), not one colour per glyph.

use crate::animation::AsciiAnimation;
use crate::palette::Rgb;

/// The gist's ten-character ramp, coldest first.
pub const CHAR: [char; 10] = [' ', '.', ':', '^', '*', 'x', 's', 'S', '#', '$'];

/// Value written into a seeded cell. `65` is straight from the gist.
///
/// This is effectively the FLAME HEIGHT control. The kernel's `/4` integer
/// division cools aggressively, so with the gist's value the fire reaches only
/// ~20 rows regardless of panel height — on an 84-row terminal that is a band
/// along the bottom, which is the look Michael wants. Raise it to make the
/// flames climb higher; lower it for a thinner strip.
///
/// Override with PANEFX_SEED.
fn seed_value() -> i32 {
    std::env::var("PANEFX_SEED")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(65)
}

/// Upper bound for the range assertion in tests; the gist's default.
#[allow(dead_code)]
const SEED_VALUE: i32 = 65;

/// Colour thresholds from the original's `color=(4 if b[i]>15 else ...)`.
/// Rendered here as a green ramp rather than curses' red/yellow/blue.
const T_HOT: i32 = 15;
const T_WARM: i32 = 9;
const T_COOL: i32 = 4;

/// Green shades for the four bands. Kept bright at the cool end because the
/// panel is only ever seen through Alacritty at `opacity = 0.6`, which blends
/// everything back toward the terminal's dark background.
const C_HOT: Rgb = Rgb(0xc8, 0xff, 0xd0);
const C_WARM: Rgb = Rgb(0x62, 0xe6, 0x70);
const C_COOL: Rgb = Rgb(0x28, 0x8c, 0x36);
const C_DIM: Rgb = Rgb(0x1a, 0x4a, 0x20);

pub const BACKGROUND: Rgb = Rgb(0x1c, 0x1c, 0x1c);

/// Xorshift RNG — the gist uses Python's `random`; we carry our own so the
/// port is deterministic under test and pulls in no crate.
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
    /// Uniform in [0.0, 1.0), matching `random.random()`.
    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

pub struct Flames {
    width: usize,
    height: usize,
    /// Cached so the env var is read once, not once per frame.
    seed_value: i32,
    // ---- live-tunable parameters (defaults are the consts above) ----
    /// 1 in N bottom cells seeded per frame. The gist hardcodes `width / 9`;
    /// lifting it makes flame density tunable.
    seed_density: i32,
    t_hot: i32,
    t_warm: i32,
    t_cool: i32,
    // ---- colours ----
    // One per heat band, plus the background. The gist used curses colour
    // pairs; drawing into our own window means these can be anything.
    c_hot: Rgb,
    c_warm: Rgb,
    c_cool: Rgb,
    c_dim: Rgb,
    bg: Rgb,
    /// Flat heat array. The gist allocates `size + width + 1` so the kernel can
    /// read `b[i+width+1]` on the last row without bounds-checking; we keep
    /// that same slack for the same reason.
    b: Vec<i32>,
    rng: Rng,
}

impl Flames {
    pub fn new(width: usize, height: usize, seed: u64) -> Self {
        let size = width * height;
        Flames {
            width,
            height,
            seed_value: seed_value(),
            seed_density: 9,
            t_hot: T_HOT,
            t_warm: T_WARM,
            t_cool: T_COOL,
            c_hot: C_HOT,
            c_warm: C_WARM,
            c_cool: C_COOL,
            c_dim: C_DIM,
            bg: BACKGROUND,
            b: vec![0; size + width + 1],
            rng: Rng::new(seed),
        }
    }

    #[inline]
    fn value_at(&self, col: usize, row: usize) -> i32 {
        self.b[row * self.width + col]
    }

    fn colour_for(&self, v: i32) -> Rgb {
        if v > self.t_hot {
            self.c_hot
        } else if v > self.t_warm {
            self.c_warm
        } else if v > self.t_cool {
            self.c_cool
        } else {
            self.c_dim
        }
    }
}

impl AsciiAnimation for Flames {
    fn name(&self) -> &'static str {
        "flames"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.width && rows == self.height {
            return;
        }
        self.width = cols;
        self.height = rows;
        self.b = vec![0; cols * rows + cols + 1];
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn step(&mut self) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let size = self.width * self.height;

        // `for i in range(int(width/9)): b[int((random.random()*width)
        //                                  + width*(height-1))] = 65`
        // Sparse seeding along the bottom row — only ~1 cell in 9.
        let seeds = self.width / (self.seed_density.max(1) as usize);
        let base = self.width * (self.height - 1);
        for _ in 0..seeds {
            let off = (self.rng.next_f32() * self.width as f32) as usize;
            let idx = base + off.min(self.width - 1);
            if idx < self.b.len() {
                self.b[idx] = self.seed_value;
            }
        }

        // `b[i] = int((b[i] + b[i+1] + b[i+width] + b[i+width+1]) / 4)`
        // In-place and forward-walking, so later cells see updated earlier
        // ones. Integer division is the only cooling in the algorithm.
        for i in 0..size {
            let s = self.b[i] + self.b[i + 1] + self.b[i + self.width] + self.b[i + self.width + 1];
            self.b[i] = s / 4;
        }
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        let v = self.value_at(col, row);
        // `char[(9 if b[i]>9 else b[i])]`
        let idx = if v > 9 { 9 } else { v.max(0) as usize };
        if idx == 0 {
            // Skip blanks entirely — a painted space is a visible smudge
            // behind a semi-transparent terminal.
            return None;
        }
        Some((CHAR[idx], self.colour_for(v)))
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    fn params(&self) -> Vec<crate::animation::Param> {
        use crate::animation::Param;
        vec![
            // Flame height. See `stays_a_bottom_band_on_a_tall_panel` — the
            // short default is the look Michael chose, not a shortfall.
            Param::int("seed", "flame height", self.seed_value as i64, 1, 255),
            Param::int("density", "1-in-N sources", self.seed_density as i64, 2, 40),
            Param::int("t_hot", "hot threshold", self.t_hot as i64, 1, 64),
            Param::int("t_warm", "warm threshold", self.t_warm as i64, 1, 64),
            Param::int("t_cool", "cool threshold", self.t_cool as i64, 1, 64),
            Param::colour("c_hot", "hot colour", self.c_hot),
            Param::colour("c_warm", "warm colour", self.c_warm),
            Param::colour("c_cool", "cool colour", self.c_cool),
            Param::colour("c_dim", "dim colour", self.c_dim),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &crate::animation::ParamValue) -> bool {
        use crate::animation::clamp_int;

        // Colours first: they are not Int, so the `as_int` guard below would
        // reject them.
        if let Some(c) = v.as_rgb() {
            match key {
                "c_hot" => { self.c_hot = c; return true; }
                "c_warm" => { self.c_warm = c; return true; }
                "c_cool" => { self.c_cool = c; return true; }
                "c_dim" => { self.c_dim = c; return true; }
                "bg" => { self.bg = c; return true; }
                _ => return false,
            }
        }

        let Some(n) = v.as_int() else { return false };
        match key {
            "seed" => {
                self.seed_value = clamp_int(n, 1, 255) as i32;
                true
            }
            "density" => {
                self.seed_density = clamp_int(n, 2, 40) as i32;
                true
            }
            "t_hot" => {
                self.t_hot = clamp_int(n, 1, 64) as i32;
                true
            }
            "t_warm" => {
                self.t_warm = clamp_int(n, 1, 64) as i32;
                true
            }
            "t_cool" => {
                self.t_cool = clamp_int(n, 1, 64) as i32;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_matches_the_gist() {
        assert_eq!(CHAR.len(), 10);
        assert_eq!(CHAR[0], ' ');
        assert_eq!(CHAR[9], '$');
    }

    #[test]
    fn does_not_panic_at_any_size() {
        for (w, h) in [(0, 0), (1, 1), (9, 3), (143, 84)] {
            let mut f = Flames::new(w, h, 7);
            for _ in 0..50 {
                f.step();
            }
            for r in 0..h {
                for c in 0..w {
                    let _ = f.cell_at(c, r);
                }
            }
        }
    }

    #[test]
    fn values_stay_bounded() {
        let mut f = Flames::new(143, 84, 3);
        for _ in 0..600 {
            f.step();
            for v in &f.b {
                assert!(
                    *v >= 0 && *v <= SEED_VALUE.max(f.seed_value),
                    "value {v} out of range"
                );
            }
        }
    }

    #[test]
    fn resize_reallocates_with_slack() {
        // The kernel reads b[i+width+1]; without the slack that is an OOB
        // index on the final row.
        let mut f = Flames::new(10, 5, 1);
        f.resize(20, 8);
        assert!(f.b.len() >= 20 * 8 + 20 + 1);
        for _ in 0..40 {
            f.step();
        }
    }

    #[test]
    fn stays_a_bottom_band_on_a_tall_panel() {
        // DELIBERATE: with the gist's seed value the fire reaches ~20 rows
        // regardless of panel height, so on a tall terminal it is a band along
        // the bottom rather than a full-height effect. Michael chose this look
        // explicitly after seeing it live — it is the feature, not a shortfall.
        // Raise PANEFX_SEED to make it climb higher.
        let mut f = Flames::new(143, 84, 1234);
        for _ in 0..600 {
            f.step();
        }
        let top_lit = (0..84).find(|&r| (0..143).any(|c| f.cell_at(c, r).is_some()));
        let reach = match top_lit {
            Some(t) => 84 - t,
            None => 0,
        };
        assert!(reach > 5, "fire barely renders at all ({reach} rows)");
        assert!(
            reach < 50,
            "fire now fills {reach}/84 rows — the bottom-band look is gone"
        );
    }

    #[test]
    fn params_round_trip_and_clamp() {
        use crate::animation::ParamValue;
        let mut f = Flames::new(60, 30, 1);
        for p in f.params() {
            assert!(
                f.set_param(&p.key, &p.value),
                "declared param '{}' is not settable",
                p.key
            );
        }
        assert!(f.set_param("seed", &ParamValue::Int { v: 100_000 }));
        assert!(f.seed_value <= 255);
        assert!(f.set_param("density", &ParamValue::Int { v: 0 }));
        assert!(f.seed_density >= 2, "density must stay >=2 or width/N divides by zero");
        assert!(!f.set_param("nope", &ParamValue::Int { v: 1 }));

        // Still runs after the extremes.
        for _ in 0..50 {
            f.step();
        }
    }

    #[test]
    fn produces_visible_output() {
        let mut f = Flames::new(143, 84, 11);
        for _ in 0..400 {
            f.step();
        }
        let lit = (0..84)
            .flat_map(|r| (0..143).map(move |c| (c, r)))
            .filter(|(c, r)| f.cell_at(*c, *r).is_some())
            .count();
        assert!(lit > 100, "almost nothing is drawn: {lit} lit cells");
    }
}
