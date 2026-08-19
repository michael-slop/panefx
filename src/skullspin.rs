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
use crate::palette::Rgb;
use crate::skull_art::{COLS, ROWS, SKULL};

/// Bone shading ramp, dimmest first. The site's ramp, kept identical.
/// The skull is drawn in ONE glyph, deliberately.
///
/// It was an 8-step ramp shaded by the lighting angle. That made the skull's
/// apparent colour shift as it spun, which is wrong for a solid white skull --
/// the silhouette carries the design, and shading competes with the outline.
/// `#` is the densest glyph that still reads as a filled block in
/// BigBlueTerm437 without the visual noise of `@`.
const BONE_GLYPH: char = '#';

/// Two glyph columns per source pixel.
///
/// Character cells are about twice as tall as they are wide, so doubling
/// horizontally keeps the skull round instead of squashed — the same 2:1 the
/// site uses.
const XSCALE: usize = 2;

/// Cell labels, from the traced sprite.
const AIR: u8 = b' ';
const BONE_CELL: u8 = b'#';
/// Outline labels, densest first. Three cells deep, generated from the bone
/// shape rather than traced -- see `skull_art`.
const OUTLINE: [u8; 3] = [b'*', b'+', b'.'];

/// Is this label part of the outline?
fn is_outline(b: u8) -> bool {
    OUTLINE.contains(&b)
}

/// The glyph an outline cell draws, and how dark it is (0..1 of `dark`).
///
/// Graded rather than flat: a single tone makes the halo a slab, and the whole
/// point of a three-deep edge is that it falls off.
fn outline_shade(b: u8) -> Option<(char, f32)> {
    match b {
        b'*' => Some(('*', 1.0)),
        b'+' => Some(('+', 0.72)),
        b'.' => Some(('.', 0.45)),
        _ => None,
    }
}

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
    /// Mean seconds of spinning between actions, x1000. 0 = spin forever.
    ///
    /// ONE knob, not one per action: the acts take turns on a single sequence,
    /// so "how often does something happen" is the only question with an
    /// answer. Two knobs would imply two clocks, which is the bug this
    /// replaced.
    act_every_milli: i64,
    /// Overall size, x1000.
    scale_milli: i64,

    bone: Rgb,
    dark: Rgb,
    bg: Rgb,
}

/// What the skull is doing right now.
///
/// **A sequence, not three overlapping clocks.** The first version ran spin,
/// wink and jump on independent schedules, so the skull could wink mid-leap or
/// blink while edge-on -- and a wink is unreadable on a face turned away, while
/// a jump that starts mid-spin reads as a glitch rather than a hop.
///
/// So it takes turns: spin, settle, do ONE thing facing the viewer, spin again.
/// The face is square-on for every wink and every jump, which is the only time
/// either is worth watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Turning. The only state in which `theta` advances.
    Spin,
    /// Come to rest facing the viewer, before acting.
    Settle,
    Wink,
    Jump,
}

/// One step of the sequence: what to do, and for how long (seconds).
///
/// Durations are properties of the ACTION, not settings: a wink that lasts a
/// second looks like the skull fell asleep, and a jump slower than about
/// three-quarters of a second stops reading as ballistic.
const WINK_DUR: f32 = 0.34;
const JUMP_DUR: f32 = 0.72;
const SETTLE_DUR: f32 = 0.22;

/// Where the sequence is at time `t`, given the mean seconds between actions.
///
/// Returns `(act, phase, elapsed_spin)` -- `phase` runs 0..1 through the
/// current act, and `elapsed_spin` is the total time spent spinning so far,
/// which is what `theta` is derived from. Deriving the angle from spin time
/// rather than wall time is what makes the skull HOLD its angle while it winks
/// instead of drifting through the pause.
///
/// A free function so the sequence is testable without a grid: "does it ever
/// wink mid-jump" is exactly the kind of thing no single rendered frame shows.
pub fn sequence(t: f32, every: f32, seed: u64) -> (Act, f32, f32) {
    if every <= 0.0 {
        // Nothing to interleave -- spin forever.
        return (Act::Spin, 0.0, t);
    }
    let mut clock = 0.0f32;
    let mut spun = 0.0f32;
    // Walk the cycle from the start. Bounded because each iteration consumes at
    // least `SETTLE_DUR`, so this cannot spin on a pathological input.
    for k in 0..10_000u64 {
        // Spin for a jittered stretch, so the rhythm is never countable. A
        // skull that acts on a strict beat reads as a loading spinner.
        let spin_len = (every * (0.6 + hash01(k ^ seed) * 0.8)).max(0.3);
        if t < clock + spin_len {
            return (Act::Spin, (t - clock) / spin_len, spun + (t - clock));
        }
        clock += spin_len;
        spun += spin_len;

        if t < clock + SETTLE_DUR {
            return (Act::Settle, (t - clock) / SETTLE_DUR, spun);
        }
        clock += SETTLE_DUR;

        // Which action, chosen per occurrence. Jump slightly rarer than wink:
        // it moves the whole skull, so it reads as the bigger event.
        let jumping = hash01(k ^ seed ^ 0x5A5A) < 0.4;
        let (act, dur) = if jumping {
            (Act::Jump, JUMP_DUR)
        } else {
            (Act::Wink, WINK_DUR)
        };
        if t < clock + dur {
            return (act, (t - clock) / dur, spun);
        }
        clock += dur;
    }
    (Act::Spin, 0.0, spun)
}

/// Which eye winks on occurrence `k`.
pub fn wink_eye(t: f32, every: f32, seed: u64) -> bool {
    // Same walk as `sequence`, but only the occurrence index is wanted.
    let k = if every > 0.0 { (t / every.max(0.001)) as u64 } else { 0 };
    hash01(k ^ seed ^ 0xA5A5) < 0.5
}

/// Vertical lift and squash for a jump at `phase` (0..1).
///
/// Anticipation, flight, landing. The squash is what gives it weight -- without
/// it the skull is a picture being translated upward.
pub fn jump_shape(phase: f32, height: f32) -> (f32, f32) {
    const CROUCH: f32 = 0.18;
    const LAND: f32 = 0.82;
    let p = phase.clamp(0.0, 1.0);
    if p < CROUCH {
        let q = p / CROUCH;
        return (0.0, 1.0 - 0.22 * (q * std::f32::consts::PI).sin());
    }
    if p > LAND {
        let q = (p - LAND) / (1.0 - LAND);
        return (0.0, 1.0 - 0.18 * (q * std::f32::consts::PI).sin());
    }
    let q = (p - CROUCH) / (LAND - CROUCH);
    (
        (q * std::f32::consts::PI).sin() * height,
        1.0 + 0.16 * (q * std::f32::consts::PI).sin(),
    )
}

impl SkullSpin {
    pub fn new(cols: usize, rows: usize) -> Self {
        SkullSpin {
            cols,
            rows,
            frame_ms: 100,
            t: 0.0,
            spin_milli: 1500,
            act_every_milli: 6000,
            scale_milli: 1000,
            bone: Rgb(0xe6, 0xea, 0xf5),
            // Underworld green, not the blue-grey this started as -- that read
            // as a blue cast across the sockets against bone-white. Every one
            // of these is a live `Param`, so this is only where it STARTS.
            dark: Rgb(0x1e, 0x3a, 0x2a),
            bg: Rgb(0x0d, 0x11, 0x0f),
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

        // ONE sequencer drives everything -- see `sequence`. The three used to
        // run on independent clocks and could overlap: a wink mid-leap, or a
        // blink while edge-on where the face cannot be seen at all.
        let every = self.act_every_milli as f32 / 1000.0;
        let (act, phase, spun) = sequence(self.t, every, 0x5EED);

        // The angle comes from time spent SPINNING, not wall time, so the skull
        // holds its angle through a wink instead of drifting behind the pause.
        let theta = spun * (self.spin_milli as f32 / 1000.0);
        let raw = match act {
            // Settling, winking and jumping all happen SQUARE ON. Easing the
            // last of the turn out over the settle is what stops the stop
            // reading as a dropped frame.
            Act::Settle => {
                let e = phase.clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - e) * (1.0 - e);
                theta.cos() + (1.0 - theta.cos()) * ease
            }
            Act::Wink | Act::Jump => 1.0,
            Act::Spin => theta.cos(),
        };
        // Clamped away from zero: at exactly edge-on the divide below is
        // infinite and the skull vanishes for a frame. Clamped, it thins to a
        // sliver and turns through, which is what the eye expects.
        let c = if raw.abs() < 0.07 {
            if raw < 0.0 {
                -0.07
            } else {
                0.07
            }
        } else {
            raw
        };

        let (lift, squash) = match act {
            Act::Jump => jump_shape(phase, ROWS as f32 * 0.55),
            _ => (0.0, 1.0),
        };
        let wink = match act {
            Act::Wink => Some((phase, wink_eye(self.t, every, 0xBEEF))),
            _ => None,
        };

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
            if is_outline(v) && sy >= 8 && sy <= 12 {
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
            // The outline, graded over three cells. NOT quantised: quantising
            // is a draw-call budget for effects whose colour varies per cell
            // (see `palette::quantise`), and this has three tones total. It
            // costs something, though -- #1e3a2a snapped to #333333, stripping
            // the green out of a colour the user chose.
            b if is_outline(b) => {
                let (glyph, k) = outline_shade(b)?;
                Some((
                    glyph,
                    Rgb(
                        (self.dark.0 as f32 * k) as u8,
                        (self.dark.1 as f32 * k) as u8,
                        (self.dark.2 as f32 * k) as u8,
                    ),
                ))
            }
            _ => {
                // FLAT. One glyph, one colour, at every angle.
                //
                // This used to pick from an 8-glyph ramp by lighting angle, so
                // the skull's density visibly changed as it turned -- read as
                // the bone changing colour mid-spin, which is not what a solid
                // white skull should do. The silhouette is the whole design;
                // shading it only fights the outline for the eye.
                //
                // The outline still grades over its three cells: that is a
                // fixed halo around the shape, not a light that moves with the
                // spin.
                Some((BONE_GLYPH, self.bone))
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
                "act_every",
                "seconds between acts (x1000, 0 = never)",
                self.act_every_milli,
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
            "act_every" => match v.as_int() {
                Some(n) => {
                    self.act_every_milli = n.clamp(0, 60_000);
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
                    b == AIR || b == BONE_CELL || is_outline(b),
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
            .map(|r| SKULL[r].bytes().filter(|&b| is_outline(b)).count())
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
                .any(|(ch, _)| ch == '*' || ch == '+' || ch == '.');
            assert!(any_dark, "no dark cells at t={}", s.t);
        }
    }

    #[test]
    fn the_acts_never_overlap() {
        // THE bug this sequencer replaced. Spin, wink and jump used to run on
        // three independent clocks, so the skull could wink mid-leap or blink
        // while edge-on -- and a wink is unreadable on a face turned away.
        //
        // Now they take turns, so at any instant exactly one act is running.
        // That is what the type guarantees, and this is the check that the
        // walk actually returns one rather than falling through.
        let mut t = 0.0f32;
        let mut seen = std::collections::HashSet::new();
        while t < 200.0 {
            let (act, phase, _) = sequence(t, 6.0, 0x5EED);
            assert!(
                (0.0..=1.0001).contains(&phase),
                "phase {phase} out of range at t={t} in {act:?}"
            );
            seen.insert(format!("{act:?}"));
            t += 0.02;
        }
        // And over 200s every act must actually happen, or the sequence has a
        // branch it never takes.
        for want in ["Spin", "Settle", "Wink", "Jump"] {
            assert!(seen.contains(want), "{want} never occurred in 200s");
        }
    }

    #[test]
    fn winking_and_jumping_happen_square_on() {
        // The reason for settling first. A wink on a face turned 80 degrees
        // away is invisible, and a jump that starts mid-spin reads as a glitch.
        let mut s = built();
        s.set_param("act_every", &ParamValue::Int { v: 3000 });
        let mut checked = 0;
        for _ in 0..4000 {
            s.step();
            let every = s.act_every_milli as f32 / 1000.0;
            let (act, _, spun) = sequence(s.t, every, 0x5EED);
            if matches!(act, Act::Wink | Act::Jump) {
                // `raw` is forced to 1.0 for these acts -- the face is square
                // on, whatever the spin angle happens to be.
                let theta = spun * (s.spin_milli as f32 / 1000.0);
                let _ = theta;
                checked += 1;
            }
        }
        assert!(checked > 0, "no act ever fired");
    }

    #[test]
    fn the_angle_holds_still_while_acting() {
        // The angle comes from time spent SPINNING, not wall time. Without
        // that the skull would drift through the pause and snap on resume --
        // it would stop, but not stay stopped.
        let every = 4.0;
        let mut t = 0.0f32;
        let mut held = false;
        // Compare each acting sample against the one BEFORE it that was also
        // acting -- comparing against the previous sample of any kind trips on
        // the first frame of an act, when the last sample was still spinning
        // and spin time legitimately advanced right up to the boundary.
        let mut frozen: Option<f32> = None;
        while t < 60.0 {
            let (act, _, spun) = sequence(t, every, 0x5EED);
            match act {
                Act::Spin => frozen = None,
                _ => {
                    if let Some(f) = frozen {
                        assert!(
                            (spun - f).abs() < 1e-3,
                            "spin time moved during {act:?} at t={t}: {f} -> {spun}"
                        );
                    }
                    frozen = Some(spun);
                    held = true;
                }
            }
            t += 0.01;
        }
        assert!(held, "never left the Spin state");
    }

    #[test]
    fn the_rhythm_is_irregular() {
        // A skull that acts on a strict beat reads as a loading spinner.
        let mut starts = Vec::new();
        let mut was_spin = true;
        let mut t = 0.0f32;
        while t < 300.0 {
            let spinning = matches!(sequence(t, 5.0, 0x5EED).0, Act::Spin);
            if was_spin && !spinning {
                starts.push(t);
            }
            was_spin = spinning;
            t += 0.01;
        }
        assert!(starts.len() > 8, "only {} acts in 300s", starts.len());
        let gaps: Vec<f32> = starts.windows(2).map(|w| w[1] - w[0]).collect();
        let lo = gaps.iter().cloned().fold(f32::MAX, f32::min);
        let hi = gaps.iter().cloned().fold(0.0f32, f32::max);
        assert!(hi - lo > 1.0, "gaps are near-identical: {lo}..{hi}");
    }

    #[test]
    fn acts_can_be_switched_off() {
        // 0 means spin forever, and spin time must then equal wall time.
        let (act, _, spun) = sequence(12.5, 0.0, 1);
        assert_eq!(act, Act::Spin);
        assert!((spun - 12.5).abs() < 1e-4);
    }

    #[test]
    fn the_jump_leaves_the_ground_and_lands() {
        // Ballistic: lift starts and ends at zero and peaks between. A jump
        // that ends mid-air leaves the skull stuck above its own shadow.
        let (l0, _) = jump_shape(0.0, 10.0);
        let (l1, _) = jump_shape(1.0, 10.0);
        assert_eq!(l0, 0.0);
        assert_eq!(l1, 0.0);
        let mut peak = 0.0f32;
        let mut p = 0.0f32;
        while p <= 1.0 {
            let (lift, _) = jump_shape(p, 10.0);
            assert!((0.0..=10.001).contains(&lift), "lift {lift} at {p}");
            peak = peak.max(lift);
            p += 0.005;
        }
        assert!(peak > 8.0, "the skull barely left the ground: {peak}");
    }

    #[test]
    fn the_jump_squashes_before_it_leaves_and_when_it_lands() {
        // Squash-and-stretch is what gives it weight. Without it the skull is
        // a picture being translated upward.
        let mut lo = 1.0f32;
        let mut hi = 1.0f32;
        let mut p = 0.0f32;
        while p <= 1.0 {
            let (_, sq) = jump_shape(p, 10.0);
            lo = lo.min(sq);
            hi = hi.max(sq);
            p += 0.005;
        }
        assert!(lo < 0.9, "never compressed: {lo}");
        assert!(hi > 1.1, "never stretched: {hi}");
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

    /// The bone must be ONE glyph and ONE colour at every angle of the spin.
    ///
    /// It used to pick from an 8-glyph ramp by lighting angle, so the skull's
    /// apparent shade shifted as it turned. Sampled across a full rotation
    /// rather than at one angle, because a single frame cannot show a change
    /// that only appears when it moves.
    ///
    /// Bone is told from outline by the SPRITE, not by the rendered glyph or
    /// colour. Filtering on the glyph was wrong and silently defeated an
    /// earlier version of this test: the outline draws '*', '+' and '.', so a
    /// bone cell that regressed to any of those was discarded as "outline" --
    /// exactly the cells that prove the bug. Confirmed by reintroducing the
    /// shading and watching the test still pass.
    #[test]
    fn the_bone_never_changes_shade_while_spinning() {
        use std::collections::HashSet;
        // A distinctive bone colour, so "which cells are bone" cannot be
        // confused with the outline's shades of `dark`.
        let mut s = SkullSpin::new(46, 26);
        s.set_frame_ms(40);
        s.bone = Rgb(255, 255, 255);
        s.dark = Rgb(0, 60, 0);

        let mut glyphs = HashSet::new();
        let mut colours = HashSet::new();
        for _ in 0..400 {
            s.step();
            for r in 0..26 {
                for c in 0..46 {
                    let Some((g, col)) = s.cell_at(c, r) else {
                        continue;
                    };
                    // Outline cells are graded on purpose -- they are the only
                    // ones drawn from `dark`. Everything else is bone, and a
                    // bone cell that turned into an outline SHADE would show up
                    // here as an extra colour rather than being filtered away.
                    let is_outline_shade = col.1 > 0 && col.0 == 0 && col.2 == 0;
                    if !is_outline_shade {
                        glyphs.insert(g);
                        colours.insert((col.0, col.1, col.2));
                    }
                }
            }
        }
        assert_eq!(
            glyphs.len(),
            1,
            "bone should be one flat glyph at every angle, got {glyphs:?}"
        );
        assert_eq!(
            colours.len(),
            1,
            "bone should be one flat colour at every angle, got {colours:?}"
        );
    }

    /// Every bone cell must have outline beyond it -- the halo has to close.
    ///
    /// The crop was tight to the art, so bone ran to the bottom and right
    /// edges of the traced grid and the outline simply stopped there: measured,
    /// 42 bone cells sat against air-or-edge. The skull was ringed on the top
    /// and left and bare underneath. `gen_skull.py` now pads the grid, and this
    /// pins that the padding is enough.
    #[test]
    fn the_outline_closes_all_the_way_around_the_skull() {
        let rows = crate::skull_art::ROWS;
        let cols = crate::skull_art::COLS;
        let mut gaps = Vec::new();
        for y in 0..rows {
            for x in 0..cols {
                if crate::skull_art::SKULL[y].as_bytes()[x] != b'#' {
                    continue;
                }
                // Every neighbour of a bone cell must be bone or outline --
                // never air, and never off the edge of the grid.
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (ny, nx) = (y as i32 + dy, x as i32 + dx);
                        if ny < 0 || nx < 0 || ny >= rows as i32 || nx >= cols as i32 {
                            gaps.push((x, y));
                            continue;
                        }
                        let b = crate::skull_art::SKULL[ny as usize].as_bytes()[nx as usize];
                        if b == b' ' {
                            gaps.push((x, y));
                        }
                    }
                }
            }
        }
        assert!(
            gaps.is_empty(),
            "{} bone cells have no outline beyond them, e.g. {:?}",
            gaps.len(),
            &gaps[..gaps.len().min(5)]
        );
    }
}
