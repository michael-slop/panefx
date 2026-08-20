//! The warlock, spinning.
//!
//! A hooded skeleton in sunglasses holding a wand, traced from
//! `Documents\(f)art\assets\darkfantasy\warlock.png` by `gen_warlock.py` into
//! `warlock_art.rs`. Same rotation maths as `skullspin` and as
//! michaelslop.org's own spinner: the screen column is inverse-mapped back
//! through the cosine, rounded rather than interpolated so the edges stay
//! crisp at these sizes.
//!
//! WHAT IS DELIBERATELY DIFFERENT FROM `skullspin`
//! ----------------------------------------------
//! The skull is a symmetric front-facing object, so it can wink, jump and turn
//! through edge-on without ever looking wrong. **The warlock faces RIGHT.** A
//! side-on figure rotated past edge-on shows its own back, and a wand that
//! sweeps behind the body reads as a glitch rather than as a turn.
//!
//! So this does not spin through 360 degrees. It ROCKS: the cosine is driven
//! over a limited arc, so the figure turns toward the viewer and back without
//! ever presenting a face the sprite does not have. `spin` sets the rate;
//! `sway` sets how far it turns.
//!
//! Four colours rather than the skull's two, because the sprite has four parts
//! worth telling apart while it moves -- the skull, the sunglasses (the joke of
//! the piece), the robe, and the wand.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;
use crate::warlock_art::{COLS, ROWS, WARLOCK};

/// Two glyph columns per source cell.
///
/// A character cell is roughly twice as tall as it is wide, so 1:1 would render
/// the warlock stretched into a lamppost. Same reason `skullspin` does it.
const XSCALE: usize = 2;

/// Labels in the traced grid. See `gen_warlock.py`.
const BONE: u8 = b'#';
const SHADE: u8 = b'=';
const ROBE: u8 = b'%';
const WAND: u8 = b'|';

/// The outline ramp, densest nearest the figure.
const OUTLINE: [u8; 3] = [b'*', b'+', b'.'];

fn is_outline(b: u8) -> bool {
    OUTLINE.contains(&b)
}

/// The glyph an outline cell draws, and how dark it is (0..1 of `dark`).
fn outline_shade(b: u8) -> Option<(char, f32)> {
    match b {
        b'*' => Some(('*', 1.0)),
        b'+' => Some(('+', 0.72)),
        b'.' => Some(('.', 0.45)),
        _ => None,
    }
}

/// The rock angle at time `t`, as a cosine.
///
/// Returns the cosine directly rather than an angle, because that is what the
/// inverse column map needs and it keeps the caller from having to know how the
/// motion is shaped.
///
/// `sway` is how far the figure turns, 0..1 of a quarter turn. At 0 it faces
/// straight ahead and never moves; at 1 it reaches edge-on at the extremes.
/// It never goes PAST edge-on -- see the module note.
pub fn rock(t: f32, rate: f32, sway: f32) -> f32 {
    let sway = sway.clamp(0.0, 1.0);
    // cos walks between 1.0 (facing the viewer) and 1.0 - sway. With sway = 1
    // the far end is 0.0, which is edge-on: the sprite's own profile, and as
    // far as a side-on figure can honestly turn.
    let phase = (t * rate * std::f32::consts::TAU).sin();
    1.0 - sway * (0.5 - 0.5 * phase)
}

pub struct WarlockSpin {
    cols: usize,
    rows: usize,
    frame_ms: u64,
    /// Seconds elapsed. Everything derives from this, so the animation is a
    /// pure function of time and cannot drift between monitors.
    t: f32,

    /// Rock rate, x1000 cycles per second.
    spin_milli: i64,
    /// How far it turns, x1000 of a quarter turn.
    sway_milli: i64,
    /// Overall size, x1000.
    scale_milli: i64,

    bone: Rgb,
    shade: Rgb,
    robe: Rgb,
    wand: Rgb,
    dark: Rgb,
    bg: Rgb,
}

impl WarlockSpin {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            frame_ms: 33,
            t: 0.0,
            spin_milli: 250,
            sway_milli: 850,
            scale_milli: 1000,
            // Straight off the sprite, so the effect and the source agree.
            bone: Rgb(0xff, 0xff, 0xff),
            shade: Rgb(0x99, 0xe5, 0x50),
            robe: Rgb(0x3f, 0x3f, 0x74),
            wand: Rgb(0x52, 0x4b, 0x24),
            dark: Rgb(0x22, 0x20, 0x34),
            bg: Rgb(0x00, 0x00, 0x00),
        }
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// Grid label at a source cell, or air outside it.
    fn label(&self, u: isize, v: isize) -> u8 {
        if v < 0 || u < 0 || v >= ROWS as isize || u >= COLS as isize {
            return b' ';
        }
        WARLOCK[v as usize].as_bytes()[u as usize]
    }

    /// Origin and zoom: where the figure sits and how big it is.
    fn layout(&self) -> (f32, f32, f32) {
        let scale = (self.scale_milli as f32 / 1000.0).max(0.05);
        let by_w = self.cols as f32 / (COLS * XSCALE) as f32;
        // No jump, so no headroom reservation -- unlike `skullspin`, which has
        // to reserve 1.75x for the arc. A small margin only, so the outline is
        // not flush against the panel edge.
        let by_h = self.rows as f32 / (ROWS as f32 * 1.08);
        let z = by_w.min(by_h) * scale;
        let w = (COLS * XSCALE) as f32 * z;
        let h = ROWS as f32 * z;
        // Centred: nothing needs room above it.
        (
            (self.cols as f32 - w) / 2.0,
            (self.rows as f32 - h).max(0.0) / 2.0,
            z,
        )
    }
}

impl AsciiAnimation for WarlockSpin {
    fn name(&self) -> &'static str {
        "warlockspin"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        // Wall-clock, so it turns at the same rate behind a 30fps terminal and
        // on a 5fps wallpaper.
        self.t += self.frame_ms as f32 / 1000.0;
    }

    fn changed(&self) -> bool {
        true
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((15, 23))
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let (ox, oy, z) = self.layout();
        if z <= 0.0 {
            return None;
        }

        let rate = self.spin_milli as f32 / 1000.0;
        let sway = self.sway_milli as f32 / 1000.0;
        let raw = rock(self.t, rate, sway);

        // Clamped away from zero: at exactly edge-on the divide below is
        // infinite and the figure vanishes for a frame. Clamped, it thins to a
        // sliver, which is what the eye expects.
        let c = if raw.abs() < 0.07 {
            if raw < 0.0 {
                -0.07
            } else {
                0.07
            }
        } else {
            raw
        };

        // Panel cell -> sprite cell, undoing the layout and the rotation.
        let fx = (col as f32 - ox) / z;
        let fy = (row as f32 - oy) / z;
        if fy < 0.0 || fy >= ROWS as f32 {
            return None;
        }
        let sy = fy as isize;
        let cx = (COLS - 1) as f32 / 2.0;
        let u = ((fx / XSCALE as f32 - cx) / c + cx).round() as isize;

        match self.label(u, sy) {
            b' ' => None,
            // The outline, graded over three cells. A fixed halo around the
            // shape, NOT a light that moves with the turn -- so it does not
            // read as the figure changing colour as it rocks.
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
            // Every part is FLAT: one glyph, one colour, at every angle.
            //
            // `skullspin` learned this the hard way -- it shaded bone by the
            // lighting angle, which made the skull's apparent colour shift as
            // it turned. The silhouette carries the design; shading only fights
            // the outline for the eye.
            BONE => Some(('#', self.bone)),
            SHADE => Some(('=', self.shade)),
            WAND => Some(('|', self.wand)),
            ROBE => Some(('%', self.robe)),
            _ => None,
        }
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("spin", "rock rate (x1000)", self.spin_milli, -3000, 3000),
            Param::int(
                "sway",
                "how far it turns (x1000)",
                self.sway_milli,
                0,
                1000,
            ),
            Param::int("scale", "size (x1000)", self.scale_milli, 200, 3000),
            Param::colour("bone", "skull colour", self.bone),
            Param::colour("shade", "sunglasses", self.shade),
            Param::colour("robe", "robe colour", self.robe),
            Param::colour("wand", "wand colour", self.wand),
            Param::colour("dark", "outline colour", self.dark),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            "spin" => match v.as_int() {
                Some(n) => {
                    self.spin_milli = n.clamp(-3000, 3000);
                    true
                }
                None => false,
            },
            "sway" => match v.as_int() {
                Some(n) => {
                    self.sway_milli = n.clamp(0, 1000);
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
            "bone" => match v.as_rgb() {
                Some(c) => {
                    self.bone = c;
                    true
                }
                None => false,
            },
            "shade" => match v.as_rgb() {
                Some(c) => {
                    self.shade = c;
                    true
                }
                None => false,
            },
            "robe" => match v.as_rgb() {
                Some(c) => {
                    self.robe = c;
                    true
                }
                None => false,
            },
            "wand" => match v.as_rgb() {
                Some(c) => {
                    self.wand = c;
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

    #[test]
    fn the_traced_sprite_is_rectangular_and_uses_known_labels() {
        assert_eq!(WARLOCK.len(), ROWS);
        for (i, row) in WARLOCK.iter().enumerate() {
            assert_eq!(row.len(), COLS, "row {i} is the wrong width");
            for b in row.bytes() {
                assert!(
                    b == b' ' || b == BONE || b == SHADE || b == ROBE || b == WAND || is_outline(b),
                    "row {i} has an unknown label {:?}",
                    b as char
                );
            }
        }
    }

    #[test]
    fn the_outline_closes_all_the_way_around() {
        // Same check the skull needed: the crop is tight to the art, so without
        // padding the halo stops where the figure meets the edge.
        let solid = |b: u8| b == BONE || b == SHADE || b == ROBE || b == WAND;
        let mut gaps = Vec::new();
        for y in 0..ROWS {
            for x in 0..COLS {
                if !solid(WARLOCK[y].as_bytes()[x]) {
                    continue;
                }
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (ny, nx) = (y as i32 + dy, x as i32 + dx);
                        if ny < 0 || nx < 0 || ny >= ROWS as i32 || nx >= COLS as i32 {
                            gaps.push((x, y));
                            continue;
                        }
                        if WARLOCK[ny as usize].as_bytes()[nx as usize] == b' ' {
                            gaps.push((x, y));
                        }
                    }
                }
            }
        }
        assert!(
            gaps.is_empty(),
            "{} figure cells have no outline beyond them",
            gaps.len()
        );
    }

    #[test]
    fn it_rocks_rather_than_spinning_through_its_own_back() {
        // THE point of this effect. The warlock faces right; turning past
        // edge-on would show a back the sprite does not have, and sweep the
        // wand behind the body.
        for i in 0..2000 {
            let t = i as f32 * 0.01;
            let c = rock(t, 0.25, 1.0);
            assert!(
                (-0.001..=1.001).contains(&c),
                "cos {c} at t={t} turned past edge-on"
            );
        }
    }

    #[test]
    fn sway_zero_holds_it_still_facing_the_viewer() {
        for i in 0..200 {
            let c = rock(i as f32 * 0.05, 0.25, 0.0);
            assert!((c - 1.0).abs() < 1e-5, "sway=0 should not move, got {c}");
        }
    }

    #[test]
    fn every_part_stays_one_flat_colour_while_it_turns() {
        // `skullspin` shaded bone by the lighting angle, which made it appear
        // to change colour mid-spin. Each part here must be exactly one colour
        // at every angle.
        use std::collections::HashMap;
        let mut w = WarlockSpin::new(60, 34);
        w.set_frame_ms(40);
        let mut seen: HashMap<char, std::collections::HashSet<(u8, u8, u8)>> = HashMap::new();
        for _ in 0..400 {
            w.step();
            for r in 0..34 {
                for c in 0..60 {
                    if let Some((g, col)) = w.cell_at(c, r) {
                        seen.entry(g).or_default().insert((col.0, col.1, col.2));
                    }
                }
            }
        }
        for (glyph, colours) in &seen {
            if is_outline(*glyph as u8) {
                continue; // the halo is graded on purpose
            }
            assert_eq!(
                colours.len(),
                1,
                "glyph {glyph:?} drew {} different colours",
                colours.len()
            );
        }
        // And the parts that make this the warlock rather than a blob.
        assert!(seen.contains_key(&'#'), "the skull never drew");
        assert!(seen.contains_key(&'='), "the sunglasses never drew");
        assert!(seen.contains_key(&'|'), "the wand never drew");
    }

    #[test]
    fn it_stays_inside_the_panel_at_any_shape() {
        for (cols, rows) in [(20usize, 10usize), (200, 60), (40, 120), (8, 8)] {
            let w = WarlockSpin::new(cols, rows);
            for r in 0..rows {
                for c in 0..cols {
                    let _ = w.cell_at(c, r); // must not panic or index out of range
                }
            }
        }
    }

    #[test]
    fn unknown_params_are_rejected() {
        let mut w = WarlockSpin::new(40, 20);
        assert!(!w.set_param("nope", &ParamValue::Int { v: 1 }));
        assert!(w.set_param("sway", &ParamValue::Int { v: 500 }));
    }
}
