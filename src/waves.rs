//! blackwaves — procedural ASCII wave field.
//!
//! Ported from `C:\Users\micha\blackwaves\blackwaves.py` (LAW 1: source read
//! before porting). That script's comments record measurements taken off a real
//! reference clip, and those numbers are the whole design — they are carried
//! over here verbatim rather than re-derived:
//!
//!   * Reference distribution (695x1230 crop, greyscale):
//!     `p25=4 p50=14 p75=28 p90=42 p99=71 max~89` — an almost entirely black
//!     field carrying its detail in the top decile. A stock dense-to-sparse
//!     ramp looks EMPTY on this material, which is why the luminance remap
//!     exists at all.
//!   * The field does NOT advect. Best whole-frame translation between
//!     reference frames is (0,0). But only ~30% of crest edges coincide
//!     between frames, so it is not static-under-moving-light either. It
//!     deforms in place, in large coherent patches. Hence the swirl warp.
//!   * Warp amplitude `0.012 * min(h,w)`. The Python notes that `0.045` moved
//!     crests about twice too far and left the geometry visibly sliding.
//!
//! Structure that makes this affordable in real time: `base_field` is six
//! octaves of ridged, sheared value noise, computed ONCE and cached. Each frame
//! only bilinearly resamples that cache through a cheap low-frequency warp.
//! Recomputing the octaves per frame would be far too slow for a 20fps backdrop.

use crate::animation::{clamp_int, AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;

/// Ordered by apparent ink coverage in BigBlueTerm at small sizes. Kept short
/// and low-contrast at the dark end — the reference spends most of its area
/// below luminance 16, so the first few rungs do the bulk of the work.
pub const RAMP: &str = " .:-=+*#%@";
/// The longer, finer ramp (`--sparse` in the Python).
pub const RAMP_SPARSE: &str = " .`',:;i!+*%#@";

/// Reference distribution: percentile -> luminance, from the measured crop.
const REF_P: [f32; 7] = [0.0, 25.0, 50.0, 75.0, 90.0, 99.0, 100.0];
const REF_L: [f32; 7] = [0.0, 4.0, 14.0, 28.0, 42.0, 71.0, 89.0];

/// Lookup-table resolution for the shading and remap curves. 257 entries over
/// [0,1] with linear interpolation between them is far finer than the 10-glyph
/// ramp can resolve, so the output is visually identical to exact `powf`.
const LUT_N: usize = 257;
const LUT_LAST: f32 = (LUT_N - 1) as f32;

/// Distinct ink brightnesses. Matches the Python's `--ink-steps` default of 12.
/// Quantising is what lets the renderer batch runs — see `cell_at`.
const INK_STEPS: usize = 12;

const DEFAULT_OCTAVES: usize = 5;
const DEFAULT_TILT: f32 = 0.62;
const DEFAULT_SWIRL: f32 = 0.55;
/// Gamma < 1 lifts the crowded dark end apart; without it ~80% of cells
/// collapse onto one glyph.
const DEFAULT_GAMMA: f32 = 0.62;

/// The waves are greyscale by design — the reference is a black-and-white
/// water shot. Rendered here as a very slightly cool white so it does not
/// clash with a terminal's colour scheme.
const INK: Rgb = Rgb(0xd8, 0xe4, 0xe8);
pub const BACKGROUND: Rgb = Rgb(0x00, 0x00, 0x00);

/// `np.interp(x, REF_L / REF_L[-1], REF_P / 100.0)`.
///
/// Clamping, not extrapolating, exactly as numpy does: values below the first
/// knot return the first y, values above the last return the last y.
fn interp_ref(x: f32) -> f32 {
    // Knots pre-normalised as a const. This used to build a `Vec<f32>` from
    // REF_L on EVERY CALL — and it is called once per cell, so at 143x85 that
    // was ~12,000 malloc/free pairs per frame (~243k/sec at 20fps) to
    // recompute a compile-time constant. It was the single most expensive line
    // in the program.
    const XS: [f32; 7] = [
        0.0,
        4.0 / 89.0,
        14.0 / 89.0,
        28.0 / 89.0,
        42.0 / 89.0,
        71.0 / 89.0,
        1.0,
    ];
    if x <= XS[0] {
        return REF_P[0] / 100.0;
    }
    if x >= XS[6] {
        return REF_P[6] / 100.0;
    }
    for k in 1..XS.len() {
        if x <= XS[k] {
            let d = XS[k] - XS[k - 1];
            let f = if d.abs() < 1e-9 { 0.0 } else { (x - XS[k - 1]) / d };
            return (REF_P[k - 1] + (REF_P[k] - REF_P[k - 1]) * f) / 100.0;
        }
    }
    REF_P[6] / 100.0
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Deterministic hash → [0,1). Stands in for numpy's seeded RNG grid: we need
/// the same value for the same (seed, y, x) every time, without storing the
/// grid.
#[inline]
fn hash01(seed: u32, y: i32, x: i32) -> f32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9)
        ^ (y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (x as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h >> 8) as f32 / ((1u32 << 24) as f32)
}

/// Smooth value noise on an h x w grid at the given cell frequency.
///
/// Bar-for-bar with the Python's `_value_noise`, and the details matter:
///
///   * The random grid is `(freq+2) x (freq+2)` and is INDEXED, not hashed at
///     arbitrary coordinates. Hashing per-sample gives noise with a different
///     character — measurably flatter gradients (raw lambda max 0.144 against
///     the Python's 0.241) and a longer dark tail, which then propagates all
///     the way to the glyph histogram.
///   * Sample coordinates come from `np.linspace(0, freq, n, endpoint=False)`,
///     i.e. `i * freq / n`, NOT `i / n * freq` evaluated per-axis differently.
///   * `y0 = ys.astype(int)` truncates toward zero; inputs are non-negative
///     here so that is a floor.
fn value_noise(h: usize, w: usize, freq: f32, seed: u32) -> Vec<f32> {
    let g = (freq as usize) + 2;
    // The seeded grid, generated once per (freq, seed).
    let grid: Vec<f32> = (0..g * g)
        .map(|i| hash01(seed, (i / g) as i32, (i % g) as i32))
        .collect();

    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let fy = y as f32 * freq / h as f32;
        let y0 = fy as usize;
        let ty = fade(fy - y0 as f32);
        for x in 0..w {
            let fx = x as f32 * freq / w as f32;
            let x0 = fx as usize;
            let tx = fade(fx - x0 as f32);

            let a = grid[y0 * g + x0];
            let b = grid[y0 * g + (x0 + 1)];
            let c = grid[(y0 + 1) * g + x0];
            let d = grid[(y0 + 1) * g + (x0 + 1)];

            out[y * w + x] =
                (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
        }
    }
    out
}

pub struct Waves {
    cols: usize,
    rows: usize,
    /// The fixed wave geometry, computed once per resize. This is the whole
    /// reason the effect is cheap enough to run live.
    base: Vec<f32>,
    /// Per-cell luminance for the current frame.
    lum: Vec<f32>,
    /// Two low-frequency fields driving the warp, cached alongside `base`.
    warp_a: Vec<f32>,
    warp_b: Vec<f32>,
    ramp: Vec<char>,
    /// `ambient + 0.46*l^0.85 + 0.62*l^2.6 + 0.80*l^5.0` sampled over [0,1].
    /// Constant — the lobe exponents never change.
    shade_lut: Vec<f32>,
    /// `interp_ref -> /headroom -> ^0.88 -> ^gamma` sampled over [0,1].
    /// Depends on `headroom` and `gamma`, so it is rebuilt when those change.
    remap_lut: Vec<f32>,
    lut_headroom: i64,
    lut_gamma: i64,
    /// Reusable scratch, so a frame allocates nothing.
    warped: Vec<f32>,
    lam: Vec<f32>,
    t: f32,
    frame_ms: u64,

    // ---- live-tunable ----
    octaves: usize,
    /// Phase advance per second, x1000 so it is an integer knob.
    speed_milli: i64,
    gamma_milli: i64,
    swirl_milli: i64,
    tilt_milli: i64,
    /// See the note in compute() — higher than Python's 0.62 because this
    /// port's RNG differs, so the top end needs more headroom for the same look.
    headroom_milli: i64,
    /// Luminance below which a cell is not drawn at all, x1000. See `cell_at`.
    dark_cut_milli: i64,
    ink: Rgb,
}

impl Waves {
    pub fn new(cols: usize, rows: usize, chars: Option<&str>) -> Self {
        let ramp: Vec<char> = chars
            .filter(|s| !s.is_empty())
            .unwrap_or(RAMP)
            .chars()
            .collect();
        let mut w = Waves {
            cols,
            rows,
            base: Vec::new(),
            lum: vec![0.0; cols * rows],
            warp_a: Vec::new(),
            warp_b: Vec::new(),
            ramp: if ramp.is_empty() {
                RAMP.chars().collect()
            } else {
                ramp
            },
            shade_lut: Vec::new(),
            remap_lut: Vec::new(),
            lut_headroom: -1,
            lut_gamma: -1,
            warped: Vec::new(),
            lam: Vec::new(),
            t: 0.0,
            frame_ms: 50,
            octaves: DEFAULT_OCTAVES,
            speed_milli: 1000,
            gamma_milli: (DEFAULT_GAMMA * 1000.0) as i64,
            swirl_milli: (DEFAULT_SWIRL * 1000.0) as i64,
            tilt_milli: (DEFAULT_TILT * 1000.0) as i64,
            // 1.35, not the 0.95 that matched the Python's histogram exactly.
            //
            // Two reasons, both deliberate. Aesthetically Michael wanted the
            // waves darker. Practically, raising headroom pushes more of the
            // field down onto the blank glyph, and a cell that renders blank is
            // a cell the renderer never hands to GDI — this effect's cost is
            // dominated by how many cells are lit (~24,000 of them at 0.95).
            //
            // `top_rung_stays_rare_like_the_reference` still holds: this only
            // moves the field further toward the dark end the reference already
            // sits at, never brighter.
            headroom_milli: 1350,
            // Tuned live by Michael against the real thing, not derived:
            // dark 500 -> 450 -> 405 -> 345 -> 276, each step judged on screen.
            dark_cut_milli: 276,
            ink: INK,
        };
        w.rebuild_base();
        w.rebuild_luts();
        w
    }

    /// Build the shading and remap curves. Called on construction and whenever
    /// `headroom` or `gamma` changes — never per frame.
    fn rebuild_luts(&mut self) {
        if self.shade_lut.is_empty() {
            self.shade_lut = (0..LUT_N)
                .map(|i| {
                    let l = i as f32 / LUT_LAST;
                    0.10 + 0.46 * l.powf(0.85) + 0.62 * l.powf(2.6) + 0.80 * l.powf(5.0)
                })
                .collect();
        }

        let headroom = self.headroom_milli as f32 / 1000.0;
        let gamma = (self.gamma_milli as f32 / 1000.0).max(0.05);
        let floor = 0.055f32;
        self.remap_lut = (0..LUT_N)
            .map(|i| {
                let x = i as f32 / LUT_LAST;
                let p = interp_ref(x);
                let lum = (p / headroom).clamp(0.0, 1.0);
                let v = floor + (1.0 - floor) * lum.powf(0.88);
                v.clamp(0.0, 1.0).powf(gamma)
            })
            .collect();
        self.lut_headroom = self.headroom_milli;
        self.lut_gamma = self.gamma_milli;
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// Six octaves of RIDGED value noise with a directional shear.
    ///
    /// Ridged (`1 - |2n-1|`, then squared) rather than plain value noise: the
    /// reference is creased everywhere, and plain fBm gives smooth blobs with
    /// flat faces between them.
    ///
    /// The shear is applied to the SAMPLING COORDINATES, not by rolling whole
    /// octaves. Rolling only slides crests sideways and leaves the structure
    /// axis-aligned, which reads as horizontal scan lines rather than water.
    fn rebuild_base(&mut self) {
        let (h, w) = (self.rows, self.cols);
        if h == 0 || w == 0 {
            self.base = Vec::new();
            self.warp_a = Vec::new();
            self.warp_b = Vec::new();
            self.lum = Vec::new();
            return;
        }
        let tilt = self.tilt_milli as f32 / 1000.0;
        let mut field = vec![0.0f32; w * h];
        let mut amp = 1.0f32;
        let mut freq = 3.0f32;
        let mut norm = 0.0f32;

        for o in 0..self.octaves.max(1) {
            let n = value_noise(h, w, freq, 1000 + o as u32);
            let shear_mul = 0.4 + 0.3 * o as f32;

            for y in 0..h {
                // `sh = (arange(h) * tilt) % w`, then `idx = (cols + sh*mul) % w`.
                let sh = (y as f32 * tilt) % w as f32;
                for x in 0..w {
                    let idx = (x as f32 + sh * shear_mul).rem_euclid(w as f32);
                    // LINEAR blend between the two straddling columns, not a
                    // cast to int: integer snapping makes neighbouring rows
                    // land on the same offset, which shows up as long flat
                    // horizontal stripes across the frame.
                    let i0 = (idx.floor() as usize) % w;
                    let i1 = (i0 + 1) % w;
                    let fr = idx - idx.floor();
                    let v = n[y * w + i0] * (1.0 - fr) + n[y * w + i1] * fr;

                    // Ridged, then SQUARED: `field += amp * ridged**2`.
                    let ridged = 1.0 - (2.0 * v - 1.0).abs();
                    field[y * w + x] += amp * ridged * ridged;
                }
            }
            norm += amp;
            amp *= 0.58;
            freq *= 2.03;
        }
        for v in field.iter_mut() {
            *v /= norm.max(1e-6);
        }
        self.base = field;

        // Warp drivers: two low-frequency fields sampled in quadrature so the
        // warp sweeps through the frame instead of pulsing everywhere at once.
        self.warp_a = value_noise(h, w, 2.0, 77);
        self.warp_b = value_noise(h, w, 2.7, 91);
        self.lum = vec![0.0; w * h];
        self.warped = vec![0.0; w * h];
        self.lam = vec![0.0; w * h];
    }

    #[inline]
    fn base_at(&self, y: i32, x: i32) -> f32 {
        let h = self.rows as i32;
        let w = self.cols as i32;
        let yy = y.rem_euclid(h) as usize;
        let xx = x.rem_euclid(w) as usize;
        self.base[yy * self.cols + xx]
    }

    /// Warp the fixed geometry and shade it into `self.lum`.
    fn compute(&mut self) {
        let (h, w) = (self.rows, self.cols);
        if h == 0 || w == 0 || self.base.is_empty() {
            return;
        }
        let swirl = self.swirl_milli as f32 / 1000.0;
        let phase = std::f32::consts::TAU * self.t;
        // "Small on purpose: the crests must stay put, only breathe."
        let amp_px = 0.012 * h.min(w) as f32;

        // Rebuild the remap curve only if its inputs changed.
        if self.lut_headroom != self.headroom_milli || self.lut_gamma != self.gamma_milli {
            self.rebuild_luts();
        }

        // --- warp + sample ---
        // Scratch buffers, taken out and put back so the borrow checker allows
        // &mut self methods in between. No allocation per frame.
        let mut warped = std::mem::take(&mut self.warped);
        let mut lam = std::mem::take(&mut self.lam);
        if warped.len() != w * h { warped = vec![0.0f32; w * h]; }
        if lam.len() != w * h { lam = vec![0.0f32; w * h]; }
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let wx = (phase + self.warp_a[i] * std::f32::consts::TAU).sin() * swirl;
                let wy = (phase + self.warp_b[i] * std::f32::consts::TAU).cos() * swirl;

                let sy = y as f32 + wy * amp_px;
                let sx = x as f32 + wx * amp_px;
                let y0 = sy.floor() as i32;
                let x0 = sx.floor() as i32;
                let fy = sy - y0 as f32;
                let fx = sx - x0 as f32;

                warped[i] = self.base_at(y0, x0) * (1.0 - fx) * (1.0 - fy)
                    + self.base_at(y0, x0 + 1) * fx * (1.0 - fy)
                    + self.base_at(y0 + 1, x0) * (1.0 - fx) * fy
                    + self.base_at(y0 + 1, x0 + 1) * fx * fy;
            }
        }

        // --- shade ---
        //
        // Diffuse and specular are ADDED, not multiplied. Multiplying drives
        // the result to zero wherever either factor is small and hollows the
        // frame out. Three lobes rather than two: diffuse + a single narrow
        // specular leaves a real gap in the middle of the histogram, and no
        // output remap can invent tones the shading never generated.
        let (lx, ly) = (-0.55f32, -0.83f32);
        let n = (lx * lx + ly * ly).sqrt();
        let (lx, ly) = (lx / n, ly / n);

        let mut lam_max = 1e-6f32;
        for y in 0..h {
            for x in 0..w {
                // `np.gradient`: interior uses a CENTRAL difference over a
                // spacing of 2 — (f[i+1] - f[i-1]) / 2 — while the first and
                // last rows/columns use a one-sided difference over spacing 1.
                // Dividing the edges by 2 as well (or the interior by 1)
                // scales those gradients wrongly and flattens the lit region.
                let dy = if h == 1 {
                    0.0
                } else if y == 0 {
                    warped[w + x] - warped[x]
                } else if y == h - 1 {
                    warped[(h - 1) * w + x] - warped[(h - 2) * w + x]
                } else {
                    (warped[(y + 1) * w + x] - warped[(y - 1) * w + x]) * 0.5
                };
                let dx = if w == 1 {
                    0.0
                } else if x == 0 {
                    warped[y * w + 1] - warped[y * w]
                } else if x == w - 1 {
                    warped[y * w + (w - 1)] - warped[y * w + (w - 2)]
                } else {
                    (warped[y * w + (x + 1)] - warped[y * w + (x - 1)]) * 0.5
                };
                let v = -(dx * lx + dy * ly);
                let v = v.max(0.0);
                lam[y * w + x] = v;
                if v > lam_max {
                    lam_max = v;
                }
            }
        }

        // ---- `shade()` tail, bar for bar ----
        //
        //   lam /= lam.max() + 1e-6
        //   v = ambient + 0.46*lam**0.85 + 0.62*lam**2.6 + 0.80*lam**5.0
        //   return v / (v.max() + 1e-6)
        //
        // Both normalisations are load-bearing and are NOT interchangeable
        // with one at the end.
        // The three lobes are a pure function of `l` in [0,1], so they are a
        // lookup table rather than 3 `powf` per cell. Previously this cost
        // ~1.2M `powf` calls/sec at 143x85x20fps.
        //
        // `powf(5.0)` is exactly l^2 * l^2 * l and needs no table term of its
        // own, but folding it in costs nothing and keeps the tail to one read.
        let shade_lut = &self.shade_lut;
        let lam_scale = LUT_LAST / (lam_max + 1e-6);

        let mut vmax = 0.0f32;
        for i in 0..w * h {
            // Linear interpolation between table entries keeps this visually
            // identical to the exact powf version — the curve is smooth and
            // 257 entries over [0,1] is far finer than the 10-glyph ramp can
            // resolve.
            let fi = (lam[i] * lam_scale).clamp(0.0, LUT_LAST);
            let i0 = fi as usize;
            let fr = fi - i0 as f32;
            let a = shade_lut[i0];
            let b = shade_lut[(i0 + 1).min(LUT_N - 1)];
            let v = a + (b - a) * fr;
            lam[i] = v;
            if v > vmax {
                vmax = v;
            }
        }
        for v in lam.iter_mut() {
            *v /= vmax + 1e-6;
        }

        // ---- `to_reference_levels()`, bar for bar ----
        //
        //   x = x - x.min()
        //   x /= x.max() + 1e-6
        //   lum = np.interp(x, REF_L/REF_L[-1], REF_P/100)
        //   lum = clip(lum / headroom, 0, 1)
        //   return floor + (1-floor) * lum**0.88
        //
        // Note the second step divides by the max OF THE SHIFTED ARRAY, not by
        // (max - min) of the original. Using a span here is what lifted the
        // dark half of the field off its floor: measured p50 0.27 against the
        // Python's 0.17, and ~10x too many cells on the top rung.
        let mut lo = f32::MAX;
        for v in lam.iter() {
            if *v < lo {
                lo = *v;
            }
        }
        let mut shifted_max = 0.0f32;
        for v in lam.iter_mut() {
            *v -= lo;
            if *v > shifted_max {
                shifted_max = *v;
            }
        }
        let denom = shifted_max + 1e-6;

        // Python uses headroom = 0.62. This port needs a slightly higher value
        // for the SAME visible result, and the reason is worth recording: the
        // algorithm is transcribed exactly, but `hash01` is not numpy's PCG64,
        // so the (freq+2)^2 random grids hold a different set of numbers. That
        // shifts the noise distribution a little (octave-0 mean 0.517 here vs
        // 0.456 in Python), which propagates through the ridging and shading
        // and leaves the top end hotter.
        //
        // Calibrated against the Python's measured glyph histogram at 200x60:
        // '@' 0.3%, '%' 1.1%, '#' 5.4%. `top_rung_stays_rare_like_the_reference`
        // is the tripwire.
        let headroom = self.headroom_milli as f32 / 1000.0;
        let floor = 0.055f32;
        let gamma = (self.gamma_milli as f32 / 1000.0).max(0.05);

        // The whole remap tail — interp_ref, the headroom clamp, powf(0.88) and
        // powf(gamma) — is a pure function of `x` in [0,1], so it is one table
        // read per cell instead of an interp plus 2 `powf`.
        let remap_lut = &self.remap_lut;
        let x_scale = LUT_LAST / denom;
        for i in 0..w * h {
            let fi = (lam[i] * x_scale).clamp(0.0, LUT_LAST);
            let i0 = fi as usize;
            let fr = fi - i0 as f32;
            let a = remap_lut[i0];
            let b = remap_lut[(i0 + 1).min(LUT_N - 1)];
            self.lum[i] = a + (b - a) * fr;
        }

        self.warped = warped;
        self.lam = lam;
    }
}

impl AsciiAnimation for Waves {
    fn name(&self) -> &'static str {
        "waves"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.rebuild_base();
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        // The swirl is periodic in t with period 1.0, so the animation loops
        // seamlessly on its own — no mirroring needed.
        let dt = (self.frame_ms as f32 / 1000.0) * (self.speed_milli as f32 / 1000.0) * 0.15;
        self.t = (self.t + dt) % 1.0;
        self.compute();
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if self.lum.is_empty() {
            return None;
        }
        let v = self.lum[row * self.cols + col];

        // Cells below `dark_cut` are not drawn at all.
        //
        // This is a real departure from the Python, and it is deliberate. The
        // remap's `floor = 0.055` is lifted by `powf(gamma=0.62)` to ~0.166,
        // which lands on ramp index 2 — so EVERY cell in the field is a drawn
        // glyph and the grid is 100% lit whatever `headroom` is set to. That is
        // free in the Python, which rasterises to a PNG. Here every lit cell is
        // work the renderer hands to GDI, and this effect's whole cost is how
        // many cells are lit.
        //
        // Cutting the near-floor cells removes glyphs that are, through a
        // 60%-opaque terminal, indistinguishable from the background anyway.
        if v <= self.dark_cut_milli as f32 / 1000.0 {
            return None;
        }

        let n = self.ramp.len();
        let idx = ((v * (n - 1) as f32) + 0.5) as usize;
        let idx = idx.min(n - 1);
        let g = self.ramp[idx];
        if g == ' ' {
            return None;
        }
        // Glyph choice carries the coarse steps; ink brightness carries
        // everything between them. Together they give a far wider visible
        // gradient than a 10-rung ramp at one brightness ever could.
        //
        // The brightness is QUANTISED to `INK_STEPS` levels, and that is a
        // performance decision as much as an aesthetic one. The renderer
        // batches runs of identical glyph+colour into one `TextOutW`; a
        // continuously-varying colour makes every cell its own run, so a lit
        // field of ~24,000 cells becomes ~24,000 GDI text calls per frame.
        // That — not the wave maths — was the bulk of this effect's cost
        // (measured: sim 1.0% of a core, renderer ~72%).
        //
        // The Python does exactly the same thing for the same reason: its
        // `--ink-steps` defaults to 12, quantising "the per-cell grey into a
        // handful of buckets ... per-character d.text() calls would be cw*ch
        // times more work per frame for grey steps no eye can separate".
        let q = ((v * INK_STEPS as f32) as usize).min(INK_STEPS - 1);
        let f = (0.25 + 0.75 * (q as f32 / (INK_STEPS - 1) as f32)).clamp(0.0, 1.0);
        Some((
            g,
            Rgb(
                (self.ink.0 as f32 * f) as u8,
                (self.ink.1 as f32 * f) as u8,
                (self.ink.2 as f32 * f) as u8,
            ),
        ))
    }

    /// Chunky on purpose. The blackwaves field wants each glyph to read as a
    /// mark, not as a character — the Python's own note says 12px "is the
    /// chosen look: big enough that each glyph reads as a mark, small enough
    /// that the field still reads as water", and that below ~6 "the glyphs
    /// collapse into flat dither".
    ///
    /// Tuned live by Michael at 15x23 against the real thing. Also cheaper:
    /// at 15x23 a 1432x1274 panel is 94x54 cells instead of 143x84, which is
    /// 58% fewer cells to simulate and to draw.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((15, 23))
    }

    fn background(&self) -> Rgb {
        BACKGROUND
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("speed", "speed (x1000)", self.speed_milli, 50, 5000),
            Param::int("octaves", "octaves", self.octaves as i64, 1, 8),
            Param::int("gamma", "gamma (x1000)", self.gamma_milli, 100, 2000),
            Param::int("headroom", "headroom (x1000)", self.headroom_milli, 200, 2000),
            Param::int("darkcut", "dark cutoff (x1000)", self.dark_cut_milli, 0, 900),
            Param::int("swirl", "swirl (x1000)", self.swirl_milli, 0, 3000),
            Param::int("tilt", "shear (x1000)", self.tilt_milli, 0, 2000),
            Param::text("chars", "ramp", &self.ramp.iter().collect::<String>()),
            Param::colour("ink", "ink colour", self.ink),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            "speed" => match v.as_int() {
                Some(n) => {
                    self.speed_milli = clamp_int(n, 50, 5000);
                    true
                }
                None => false,
            },
            // octaves/tilt change the GEOMETRY, so the cached base must go.
            "octaves" => match v.as_int() {
                Some(n) => {
                    self.octaves = clamp_int(n, 1, 8) as usize;
                    self.rebuild_base();
                    true
                }
                None => false,
            },
            "tilt" => match v.as_int() {
                Some(n) => {
                    self.tilt_milli = clamp_int(n, 0, 2000);
                    self.rebuild_base();
                    true
                }
                None => false,
            },
            "darkcut" => match v.as_int() {
                Some(n) => { self.dark_cut_milli = clamp_int(n, 0, 900); true }
                None => false,
            },
            "headroom" => match v.as_int() {
                Some(n) => { self.headroom_milli = clamp_int(n, 200, 2000); true }
                None => false,
            },
            "gamma" => match v.as_int() {
                Some(n) => {
                    self.gamma_milli = clamp_int(n, 100, 2000);
                    true
                }
                None => false,
            },
            "swirl" => match v.as_int() {
                Some(n) => {
                    self.swirl_milli = clamp_int(n, 0, 3000);
                    true
                }
                None => false,
            },
            "chars" => match v.as_text() {
                Some(s) => {
                    let set: Vec<char> = s.chars().collect();
                    self.ramp = if set.is_empty() {
                        RAMP.chars().collect()
                    } else {
                        set
                    };
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
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_matches_the_python() {
        assert_eq!(RAMP, " .:-=+*#%@");
        assert_eq!(RAMP_SPARSE, " .`',:;i!+*%#@");
    }

    #[test]
    fn does_not_panic_at_any_size() {
        for (c, r) in [(0, 0), (1, 1), (7, 3), (120, 40)] {
            let mut w = Waves::new(c, r, None);
            for _ in 0..10 {
                w.step();
            }
            for y in 0..r {
                for x in 0..c {
                    let _ = w.cell_at(x, y);
                }
            }
        }
    }

    #[test]
    fn luminance_stays_in_range() {
        let mut w = Waves::new(80, 25, None);
        for _ in 0..40 {
            w.step();
            for v in &w.lum {
                assert!(v.is_finite(), "non-finite luminance");
                assert!((0.0..=1.0).contains(v), "luminance {v} out of range");
            }
        }
    }

    #[test]
    fn field_is_mostly_dark_like_the_reference() {
        // The measured reference is 33% below luminance 8/255 and 52% below
        // 16/255 — an almost entirely black field with detail in the top
        // decile. A port that lights up most cells has lost the look.
        let mut w = Waves::new(120, 40, None);
        for _ in 0..30 {
            w.step();
        }
        let total = w.lum.len() as f32;
        let dark = w.lum.iter().filter(|v| **v < 0.35).count() as f32;
        assert!(
            dark / total > 0.25,
            "field is too bright: only {:.0}% dark",
            dark / total * 100.0
        );
    }

    #[test]
    fn interp_clamps_like_numpy() {
        // np.interp does NOT extrapolate. Getting this wrong sent every bright
        // cell to the top rung.
        assert!((interp_ref(-1.0) - 0.0).abs() < 1e-6);
        assert!((interp_ref(0.0) - 0.0).abs() < 1e-6);
        assert!((interp_ref(99.0) - 1.0).abs() < 1e-6);
        assert!((interp_ref(1.0) - 1.0).abs() < 1e-6);
        // Monotonic in between.
        let mut prev = -1.0;
        for i in 0..=20 {
            let v = interp_ref(i as f32 / 20.0);
            assert!(v >= prev - 1e-6, "interp not monotonic at {i}");
            prev = v;
        }
    }

    #[test]
    fn top_rung_stays_rare_like_the_reference() {
        // Measured on the Python original at 200x60: '@' 0.3%, '%' 1.1%.
        // The port previously emitted 3.5% '@' — ~10x too much — because the
        // luminance interp extrapolated past its last knot instead of
        // clamping. The field read visibly busier than the reference.
        let mut w = Waves::new(200, 60, None);
        for _ in 0..20 {
            w.step();
        }
        let total = (200 * 60) as f32;
        let mut top = 0.0;
        for y in 0..60 {
            for x in 0..200 {
                if let Some((g, _)) = w.cell_at(x, y) {
                    if g == '@' {
                        top += 1.0;
                    }
                }
            }
        }
        let pct = top / total * 100.0;
        assert!(pct < 2.0, "top rung '@' at {pct:.1}% — reference is 0.3%");
    }

    #[test]
    fn uses_more_than_one_glyph() {
        // A flat field would render as a single repeated character.
        let mut w = Waves::new(120, 40, None);
        for _ in 0..30 {
            w.step();
        }
        let mut seen = std::collections::HashSet::new();
        for y in 0..40 {
            for x in 0..120 {
                if let Some((g, _)) = w.cell_at(x, y) {
                    seen.insert(g);
                }
            }
        }
        assert!(seen.len() >= 4, "only {} distinct glyphs", seen.len());
    }

    #[test]
    fn it_actually_animates() {
        // The swirl warp must change the field between frames — a static
        // render would look like a still image.
        let mut w = Waves::new(80, 25, None);
        for _ in 0..5 {
            w.step();
        }
        let first = w.lum.clone();
        for _ in 0..25 {
            w.step();
        }
        let changed = first
            .iter()
            .zip(w.lum.iter())
            .filter(|(a, b)| (*a - *b).abs() > 0.01)
            .count();
        assert!(changed > 50, "field barely moves: {changed} cells changed");
    }

    #[test]
    fn params_round_trip_and_clamp() {
        let mut w = Waves::new(60, 20, None);
        for p in w.params() {
            assert!(w.set_param(&p.key, &p.value), "param '{}' not settable", p.key);
        }
        assert!(w.set_param("octaves", &ParamValue::Int { v: 9999 }));
        assert!(w.octaves <= 8);
        assert!(w.set_param("speed", &ParamValue::Int { v: -5 }));
        assert!(w.speed_milli >= 50);
        assert!(!w.set_param("nope", &ParamValue::Int { v: 1 }));
        // Still renders after the extremes.
        for _ in 0..10 {
            w.step();
        }
    }

    #[test]
    fn empty_ramp_falls_back() {
        let w = Waves::new(10, 10, Some(""));
        assert!(!w.ramp.is_empty(), "empty ramp would divide by zero");
    }
}
