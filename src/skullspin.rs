//! The michael.slop skull, spinning ghostty-style — and it winks, and it jumps.
//!
//! A port of `site/static/app.js:initSkullSpin`, which is the reference: the
//! site and this must show the same face turning the same way, so the crop, the
//! pixel classification and the rotation all come from there rather than being
//! re-invented. See [`crate::skull_art`] for the traced sprite.
//!
//! # The spin
//!
//! Not a 3D projection — a horizontal squash. Sampling the source column at
//! `(x - centre) / cos(theta) + centre` compresses the face as it turns away,
//! which is exactly what a rotating billboard looks like and costs one divide
//! per cell instead of a matrix.
//!
//! `cos` is clamped away from zero near edge-on. At exactly zero the divide is
//! infinite and the skull vanishes for a frame; clamped, it thins to a sliver
//! and turns through, which is what the eye expects.
//!
//! # The wink and the jump
//!
//! Both are what Michael asked for on top of the site's version, and both are
//! deliberately IRREGULAR. A skull that winks on a strict four-second beat
//! reads as a loading spinner; one whose timing you cannot predict reads as
//! something alive. So each is driven by a hash of its own occurrence number —
//! deterministic (no RNG state, same on every machine) but unpredictable to a
//! viewer.
//!
//! The wink closes ONE socket, chosen per occurrence, by filling it with bone.
//! The jump is a squash-and-stretch arc: the skull compresses before it leaves,
//! stretches through the air, and compresses again on landing. Without the
//! squash it is a picture being translated upward; with it, it has weight.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::{quantise, Rgb};
use crate::skull_art::{COLS, ROWS, SKULL};

/// Bone shading ramp, dimmest first. The site's ramp, kept identical.
const BONE: [char; 8] = ['.', ':', '=', '+', '*', '#', '%', '@'];

/// Colour steps per channel. See `palette::quantise`.
const COLOUR_LEVELS: u8 = 6;

/// Two glyph columns per source pixel.
///
/// Character cells are about twice as tall as they are wide, so doubling
/// horizontally keeps the skull round instead of squashed — the same 2:1 the
/// site uses.
const XSCALE: usize = 2;

/// Cell labels, from the traced sprite.
const AIR: u8 = b' ';
const BONE_CELL: u8 = b'#';
const DARK: u8 = b'.';

/// Deterministic hash of one integer to [0,1).
///
/// Used to jitter the wink and jump timing. A hash rather than an RNG so the
/// animation is identical on every machine and across restarts — a spinner that
/// desyncs between two monitors showing the same effect looks broken.
fn hash01(n: u64) -> f32 {
    let mut h = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    ((h >> 40) as f32) / (1u64 << 24) as f32
}

pub struct SkullSpin {
    cols: usize,
    rows: usize,
    frame_ms: u64,
    /// Seconds elapsed. Everything is derived from this, so the animation is a
    /// pure function of time and cannot drift between monitors.
    t: f32,

    /// Rotation rate, x1000 turns-ish per second.
    spin_milli: i64,
    /// Mean seconds between winks, x1000. 0 disables winking.
    wink_every_milli: i64,
    /// Mean seconds between jumps, x1000. 0 disables jumping.
    jump_every_milli: i64,
    /// Overall size, x1000.
    scale_milli: i64,

    bone: Rgb,
    dark: Rgb,
    bg: Rgb,
}

/// How far through a wink we are, 0..1, and which eye. `None` when not winking.
///
/// Split out of the render so the TIMING is testable without a grid: the whole
/// point is that it is irregular, and irregular is easy to get wrong in a way
/// no rendered frame would make obvious.
pub fn wink_phase(t: f32, every: f32, seed: u64) -> Option<(f32, bool)> {
    if every <= 0.0 {
        return None;
    }
    // Which wink we are at or past, and when it was scheduled. The jitter is
    // +/-40% of the interval so the beat is never countable.
    let n = (t / every).floor().max(0.0) as u64;
    for k in [n, n.saturating_sub(1)] {
        let at = k as f32 * every + (hash01(k ^ seed) - 0.5) * every * 0.8;
        // A wink is quick: a long one looks like the skull fell asleep.
        const DUR: f32 = 0.34;
        let d = t - at;
        if d >= 0.0 && d < DUR {
            // Left or right, chosen per occurrence.
            return Some((d / DUR, hash01(k ^ seed ^ 0xA5A5) < 0.5));
        }
    }
    None
}

/// Vertical offset in CELLS and the squash factor, for the jump.
///
/// Returns `(lift, squash)`: `lift` is how far up the skull sits, `squash`
/// multiplies its height (below 1 = compressed, above 1 = stretched).
pub fn jump_phase(t: f32, every: f32, seed: u64, height: f32) -> (f32, f32) {
    if every <= 0.0 {
        return (0.0, 1.0);
    }
    let n = (t / every).floor().max(0.0) as u64;
    for k in [n, n.saturating_sub(1)] {
        let at = k as f32 * every + (hash01(k ^ seed) - 0.5) * every * 0.8;
        const DUR: f32 = 0.72;
        let d = t - at;
        if d < 0.0 || d >= DUR {
            continue;
        }
        let p = d / DUR;
        // Anticipation, flight, landing. The squash is what gives it weight --
        // without it the skull is a picture being translated upward.
        const CROUCH: f32 = 0.18;
        const LAND: f32 = 0.82;
        if p < CROUCH {
            let q = p / CROUCH;
            return (0.0, 1.0 - 0.22 * (q * std::f32::consts::PI).sin());
        }
        if p > LAND {
            let q = (p - LAND) / (1.0 - LAND);
            return (0.0, 1.0 - 0.18 * (q * std::f32::consts::PI).sin());
        }
        // Ballistic arc: up fast, hang, down fast.
        let q = (p - CROUCH) / (LAND - CROUCH);
        let lift = (q * std::f32::consts::PI).sin() * height;
        // Stretched at the top of the leap.
        let stretch = 1.0 + 0.16 * (q * std::f32::consts::PI).sin();
        return (lift, stretch);
    }
    (0.0, 1.0)
}

impl SkullSpin {
    pub fn new(cols: usize, rows: usize) -> Self {
        SkullSpin {
            cols,
            rows,
            frame_ms: 100,
            t: 0.0,
            spin_milli: 1500,
            wink_every_milli: 5000,
            jump_every_milli: 9000,
            scale_milli: 1000,
            bone: Rgb(0xe6, 0xea, 0xf5),
            dark: Rgb(0x2a, 0x2e, 0x3d),
            bg: Rgb(0x14, 0x14, 0x1c),
        }
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// The source label at a sprite cell, or `AIR` outside it.
    fn label(&self, col: isize, row: isize) -> u8 {
        if row < 0 || col < 0 || row >= ROWS as isize || col >= COLS as isize {
            return AIR;
        }
        SKULL[row as usize].as_bytes()[col as usize]
    }

    /// Size of one sprite pixel in panel cells, and where the skull sits.
    ///
    /// Fitted to whichever axis runs out first, so the skull is always whole
    /// and centred whatever the panel shape — a wallpaper is nothing like the
    /// sprite's 26x21.
    fn layout(&self) -> (f32, f32, f32) {
        let scale = (self.scale_milli as f32 / 1000.0).max(0.05);
        let by_w = self.cols as f32 / (COLS * XSCALE) as f32;
        // Leave room overhead for the jump, or the skull clips through the top.
        //
        // 1.75, not 1.45: the arc lifts by 0.55 of the sprite height, so the
        // reservation has to cover 1.0 + 0.55 plus a margin. At 1.45 the top of
        // the skull hit row 0 at the peak -- measured off a rendered strip, not
        // reasoned about.
        let by_h = self.rows as f32 / (ROWS as f32 * 1.75);
        let z = by_w.min(by_h) * scale;
        let w = (COLS * XSCALE) as f32 * z;
        let h = ROWS as f32 * z;
        // Rest LOW in the reserved space, so the headroom sits above the skull
        // where the jump needs it rather than being split evenly.
        let free = (self.rows as f32 - h).max(0.0);
        ((self.cols as f32 - w) / 2.0, free * 0.78, z)
    }
}

impl AsciiAnimation for SkullSpin {
    fn name(&self) -> &'static str {
        "skullspin"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        // Wall-clock, so the skull turns at the same rate behind a 30fps
        // terminal and on a 5fps wallpaper.
        self.t += self.frame_ms as f32 / 1000.0;
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let (ox, oy, z) = self.layout();
        if z <= 0.0 {
            return None;
        }

        let theta = self.t * (self.spin_milli as f32 / 1000.0);
        // Clamped away from zero: at exactly edge-on the divide below is
        // infinite and the skull vanishes for a frame. Clamped, it thins to a
        // sliver and turns through, which is what the eye expects.
        let c = {
            let raw = theta.cos();
            if raw.abs() < 0.07 {
                if raw < 0.0 {
                    -0.07
                } else {
                    0.07
                }
            } else {
                raw
            }
        };

        let (lift, squash) = jump_phase(
            self.t,
            self.jump_every_milli as f32 / 1000.0,
            0x5EED,
            ROWS as f32 * 0.55,
        );
        let wink = wink_phase(self.t, self.wink_every_milli as f32 / 1000.0, 0xBEEF);

        // Panel cell -> sprite cell, undoing the layout, the jump and the spin.
        let fx = (col as f32 - ox) / z;
        let fy = (row as f32 - oy + lift * z) / (z * squash);
        if fy < 0.0 || fy >= ROWS as f32 {
            return None;
        }
        let sy = fy as isize;
        // The spin: sample the column the rotation brings to this position.
        let cx = (COLS - 1) as f32 / 2.0;
        let u = ((fx / XSCALE as f32 - cx) / c + cx).round() as isize;

        let mut v = self.label(u, sy);

        // The wink fills one socket with bone. Both sockets sit in the sprite's
        // upper half; the eye is chosen per occurrence.
        if let Some((p, left)) = wink {
            if v == DARK && sy >= 8 && sy <= 12 {
                let on_left = (u as f32) < cx;
                if on_left == left {
                    // Closes and opens rather than snapping shut, so it reads
                    // as a blink and not a dropped frame.
                    let closed = (p * std::f32::consts::PI).sin();
                    if closed > 0.35 {
                        v = BONE_CELL;
                    }
                }
            }
        }

        match v {
            AIR => None,
            // Dark cells stay dark at every angle and light level. Without this
            // the sockets fill in as the skull turns and the face stops reading.
            DARK => Some(('.', quantise(self.dark, COLOUR_LEVELS))),
            _ => {
                // Brightest facing the viewer, dimmer toward the rim.
                let mut light = 0.3 + 0.7 * c.abs();
                light *= 0.8 + 0.2 * (((u as f32 - cx) / COLS as f32) * std::f32::consts::PI).cos();
                let idx = ((light * BONE.len() as f32) as usize).min(BONE.len() - 1);
                // The GLYPH carries the shading; the colour does not.
                //
                // Scaling the bone colour by `light` as well looked right in
                // the maths and wrong on screen: quantising a smooth gradient
                // to six levels per channel banded the skull into vertical
                // stripes -- rendered and looked at. The site varies only the
                // glyph for the same reason, and bone is one colour anyway.
                Some((BONE[idx], quantise(self.bone, COLOUR_LEVELS)))
            }
        }
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((15, 23))
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("spin", "spin rate (x1000)", self.spin_milli, -6000, 6000),
            Param::int("scale", "size (x1000)", self.scale_milli, 200, 3000),
            Param::int(
                "wink_every",
                "seconds between winks (x1000, 0 = off)",
                self.wink_every_milli,
                0,
                60_000,
            ),
            Param::int(
                "jump_every",
                "seconds between jumps (x1000, 0 = off)",
                self.jump_every_milli,
                0,
                60_000,
            ),
            Param::colour("bone", "bone colour", self.bone),
            Param::colour("dark", "socket colour", self.dark),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            "spin" => match v.as_int() {
                Some(n) => {
                    self.spin_milli = n.clamp(-6000, 6000);
                    true
                }
                None => false,
            },
            "scale" => match v.as_int() {
                Some(n) => {
                    self.scale_milli = n.clamp(200, 3000);
                    true
                }
                None => false,
            },
            "wink_every" => match v.as_int() {
                Some(n) => {
                    self.wink_every_milli = n.clamp(0, 60_000);
                    true
                }
                None => false,
            },
            "jump_every" => match v.as_int() {
                Some(n) => {
                    self.jump_every_milli = n.clamp(0, 60_000);
                    true
                }
                None => false,
            },
            "bone" => match v.as_rgb() {
                Some(c) => {
                    self.bone = c;
                    true
                }
                None => false,
            },
            "dark" => match v.as_rgb() {
                Some(c) => {
                    self.dark = c;
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

    fn built() -> SkullSpin {
        let mut s = SkullSpin::new(90, 34);
        s.set_frame_ms(50);
        s
    }

    fn lit(s: &SkullSpin) -> usize {
        (0..s.rows)
            .flat_map(|r| (0..s.cols).map(move |c| (c, r)))
            .filter(|&(c, r)| s.cell_at(c, r).is_some())
            .count()
    }

    #[test]
    fn the_traced_sprite_is_rectangular_and_uses_known_labels() {
        // Generated by `gen_skull.py`; this is the check that the generator and
        // this module still agree. An unknown byte renders as air -- a hole in
        // the face, with nothing in any log to say why.
        assert_eq!(SKULL.len(), ROWS);
        for (i, row) in SKULL.iter().enumerate() {
            assert_eq!(row.len(), COLS, "row {i} is not {COLS} wide");
            for &b in row.as_bytes() {
                assert!(
                    b == AIR || b == BONE_CELL || b == DARK,
                    "row {i}: unknown label {:?}",
                    b as char
                );
            }
        }
    }

    #[test]
    fn the_skull_has_two_sockets_to_wink_with() {
        // The wink fills a socket; if the trace lost them there is nothing to
        // close and the feature silently does nothing.
        let dark_in_eye_band: usize = (8..=12)
            .map(|r| SKULL[r].bytes().filter(|&b| b == DARK).count())
            .sum();
        assert!(dark_in_eye_band > 20, "only {dark_in_eye_band} dark cells in the eye band");
    }

    #[test]
    fn it_draws_a_skull() {
        let mut s = built();
        s.step();
        assert!(lit(&s) > 80, "only {} cells drawn", lit(&s));
    }

    #[test]
    fn it_never_vanishes_while_turning() {
        // The edge-on clamp. Unclamped, `1/cos` is infinite at exactly 90
        // degrees and the skull disappears for a frame -- a flicker that looks
        // like a bug in the renderer rather than a feature of the spin.
        let mut s = built();
        for _ in 0..400 {
            s.step();
            assert!(lit(&s) > 0, "the skull vanished at t={}", s.t);
        }
    }

    #[test]
    fn the_sockets_stay_dark_at_every_angle() {
        // Without this the sockets fill in as the skull turns and the face
        // stops reading as a face -- the one thing the label scheme exists for.
        let mut s = built();
        s.set_param("wink_every", &ParamValue::Int { v: 0 });
        for _ in 0..120 {
            s.step();
            let any_dark = (0..s.rows)
                .flat_map(|r| (0..s.cols).map(move |c| (c, r)))
                .filter_map(|(c, r)| s.cell_at(c, r))
                .any(|(ch, _)| ch == '.');
            assert!(any_dark, "no dark cells at t={}", s.t);
        }
    }

    #[test]
    fn the_wink_happens_and_ends() {
        // It must actually fire within a few intervals, and it must not stick.
        let mut saw = false;
        let mut open_after = false;
        let mut t = 0.0f32;
        while t < 20.0 {
            if wink_phase(t, 4.0, 0xBEEF).is_some() {
                saw = true;
            } else if saw {
                open_after = true;
            }
            t += 0.02;
        }
        assert!(saw, "the skull never winked in 20s");
        assert!(open_after, "the wink never ended");
    }

    #[test]
    fn the_wink_is_irregular() {
        // A strict beat reads as a loading spinner. Collect the start of each
        // wink and check the gaps are not all the same.
        let mut starts = Vec::new();
        let mut was = false;
        let mut t = 0.0f32;
        while t < 120.0 {
            let now = wink_phase(t, 5.0, 0xBEEF).is_some();
            if now && !was {
                starts.push(t);
            }
            was = now;
            t += 0.01;
        }
        assert!(starts.len() > 8, "only {} winks in 120s", starts.len());
        let gaps: Vec<f32> = starts.windows(2).map(|w| w[1] - w[0]).collect();
        let lo = gaps.iter().cloned().fold(f32::MAX, f32::min);
        let hi = gaps.iter().cloned().fold(0.0f32, f32::max);
        assert!(hi - lo > 1.0, "wink gaps are near-identical: {lo}..{hi}");
    }

    #[test]
    fn winking_can_be_switched_off() {
        assert_eq!(wink_phase(3.0, 0.0, 1), None);
        assert_eq!(jump_phase(3.0, 0.0, 1, 5.0), (0.0, 1.0));
    }

    #[test]
    fn the_jump_leaves_the_ground_and_lands() {
        // Ballistic: lift starts and ends at zero, and peaks in between. A jump
        // that ends mid-air leaves the skull stuck above its own shadow.
        let mut peak = 0.0f32;
        let mut t = 0.0f32;
        while t < 40.0 {
            let (lift, _) = jump_phase(t, 6.0, 0x5EED, 10.0);
            assert!(lift >= 0.0, "lift went negative at t={t}");
            assert!(lift <= 10.001, "lift {lift} exceeded the height at t={t}");
            peak = peak.max(lift);
            t += 0.01;
        }
        assert!(peak > 8.0, "the skull barely left the ground: {peak}");
    }

    #[test]
    fn the_jump_squashes_before_it_leaves_and_when_it_lands() {
        // Squash-and-stretch is what gives it weight. Without it the skull is a
        // picture being translated upward.
        let mut min_squash = 1.0f32;
        let mut max_squash = 1.0f32;
        let mut t = 0.0f32;
        while t < 40.0 {
            let (_, sq) = jump_phase(t, 6.0, 0x5EED, 10.0);
            min_squash = min_squash.min(sq);
            max_squash = max_squash.max(sq);
            t += 0.01;
        }
        assert!(min_squash < 0.9, "never compressed: {min_squash}");
        assert!(max_squash > 1.1, "never stretched: {max_squash}");
    }

    #[test]
    fn the_jump_never_clips_through_the_top() {
        // Measured off a rendered strip: at the original 1.45 headroom the top
        // of the skull hit row 0 at the peak of the arc. The layout has to
        // reserve the jump height (0.55 of the sprite) plus a margin, and the
        // skull has to REST low so that headroom sits above it.
        let mut s = SkullSpin::new(46, 26);
        s.set_frame_ms(40);
        let mut clipped = false;
        for _ in 0..1200 {
            s.step();
            // Anything drawn on row 0 means the skull reached the edge.
            if (0..s.cols).any(|c| s.cell_at(c, 0).is_some()) {
                clipped = true;
                break;
            }
        }
        assert!(!clipped, "the skull clipped through the top of the panel");
    }

    #[test]
    fn it_is_paced_by_time_not_frame_count() {
        let mut fast = SkullSpin::new(90, 34);
        fast.set_frame_ms(20);
        let mut slow = SkullSpin::new(90, 34);
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
    fn the_skull_stays_on_screen_at_any_panel_shape() {
        for (c, r) in [(200usize, 20usize), (30, 90), (90, 34)] {
            let mut s = SkullSpin::new(c, r);
            s.set_frame_ms(50);
            s.step();
            assert!(lit(&s) > 10, "{c}x{r} drew only {} cells", lit(&s));
        }
    }

    #[test]
    fn a_degenerate_grid_does_not_panic() {
        let mut s = SkullSpin::new(0, 0);
        s.step();
        assert_eq!(s.cell_at(0, 0), None);
    }

    #[test]
    fn unknown_params_are_rejected() {
        let mut s = built();
        assert!(!s.set_param("nope", &ParamValue::Int { v: 1 }));
    }
}
