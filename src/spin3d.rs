//! Rotating 3D solids with a z-buffer: the donut, and friends.
//!
//! The donut is Andy Sloane's `donut.c` (2006), reimplemented rather than
//! transliterated. The method is the one that made it famous:
//!
//!   * walk the torus by its two angles, `theta` around the tube and `phi`
//!     around the hole, which gives points AND surface normals for free;
//!   * rotate by two more angles that advance with time;
//!   * project with `1/z`, and keep `1/z` as the depth key — larger is nearer,
//!     so the test is a single `>` and there is no division in the inner loop;
//!   * shade by the dot product of the normal with a fixed light direction, and
//!     index a ramp with it.
//!
//! A z-buffer is what makes it a solid rather than a wireframe: the far side of
//! the torus is computed, then rejected because the near side already claimed
//! those cells.
//!
//! [`Shape`] swaps the parametric surface. The machinery — rotate, project,
//! depth-test, shade — is identical for all of them, which is the whole reason
//! they share a module: a sphere or a cube here is a different `point_at`, not
//! a different renderer.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;

/// Luminance ramp, dimmest first. Sloane's original ramp.
const RAMP: [char; 12] = ['.', ',', '-', '~', ':', ';', '=', '!', '*', '#', '$', '@'];

/// Which solid to spin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Donut,
    Sphere,
    Cube,
    /// A spiral arm cloud — the "galaxy", which is a disc rather than a solid.
    Galaxy,
}

impl Shape {
    fn from_str(s: &str) -> Option<Shape> {
        match s.trim().to_lowercase().as_str() {
            "donut" | "torus" => Some(Shape::Donut),
            "sphere" => Some(Shape::Sphere),
            "cube" => Some(Shape::Cube),
            "galaxy" => Some(Shape::Galaxy),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Shape::Donut => "donut",
            Shape::Sphere => "sphere",
            Shape::Cube => "cube",
            Shape::Galaxy => "galaxy",
        }
    }
}

pub struct Spin3d {
    cols: usize,
    rows: usize,
    frame_ms: u64,

    /// The two rotation angles.
    a: f32,
    b: f32,

    shape: Shape,
    /// Spin rate about each axis, x1000. Signed.
    spin_a_milli: i64,
    spin_b_milli: i64,
    /// Object scale, x1000.
    scale_milli: i64,
    /// Surface sampling density, x1000. Higher = more points, smoother, dearer.
    density_milli: i64,

    /// Depth key per cell (`1/z`; larger is nearer). Reused every frame.
    zbuf: Vec<f32>,
    /// Ramp index per cell, or `u8::MAX` for empty.
    lum: Vec<u8>,

    lo: Rgb,
    hi: Rgb,
    bg: Rgb,
}

impl Spin3d {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut s = Spin3d {
            cols,
            rows,
            frame_ms: 100,
            a: 0.0,
            b: 0.0,
            shape: Shape::Donut,
            spin_a_milli: 1000,
            spin_b_milli: 500,
            scale_milli: 1000,
            density_milli: 1000,
            zbuf: Vec::new(),
            lum: Vec::new(),
            lo: Rgb(0x2a, 0x1e, 0x5c),
            hi: Rgb(0xff, 0xd9, 0x8a),
            bg: Rgb(0, 0, 0),
        };
        s.alloc();
        s
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    pub fn set_shape(&mut self, shape: Shape) {
        self.shape = shape;
    }

    fn alloc(&mut self) {
        let n = self.cols.saturating_mul(self.rows);
        self.zbuf = vec![0.0; n];
        self.lum = vec![u8::MAX; n];
    }

    /// Surface point and normal for the current shape, at parameters `(u, v)`
    /// each in 0..TAU.
    ///
    /// Returning the normal alongside the point is what makes shading cheap: for
    /// every one of these surfaces the normal is available analytically, so
    /// nothing has to be differenced or cross-producted per point.
    fn point_at(&self, u: f32, v: f32) -> ([f32; 3], [f32; 3]) {
        let (su, cu) = u.sin_cos();
        let (sv, cv) = v.sin_cos();
        match self.shape {
            Shape::Donut => {
                // r1 = tube radius, r2 = hole radius.
                const R1: f32 = 1.0;
                const R2: f32 = 2.0;
                let cx = R2 + R1 * cu;
                (
                    [cx * cv, cx * sv, R1 * su],
                    // The normal of a torus is the tube's own outward direction.
                    [cu * cv, cu * sv, su],
                )
            }
            Shape::Sphere => {
                // u is latitude here, so it only needs half a turn.
                let lat = u * 0.5;
                let (sl, cl) = lat.sin_cos();
                let n = [sl * cv, sl * sv, cl];
                ([n[0] * 2.0, n[1] * 2.0, n[2] * 2.0], n)
            }
            Shape::Cube => {
                // Six faces, chosen by where `v` falls; `u` walks one axis and
                // `v`'s fraction the other. A cube has no smooth
                // parameterisation, so this is a deliberate patchwork.
                let face = ((v / std::f32::consts::TAU) * 6.0) as usize % 6;
                let s = ((u / std::f32::consts::TAU) * 2.0 - 1.0) * 1.6;
                let t = (((v / std::f32::consts::TAU) * 6.0).fract() * 2.0 - 1.0) * 1.6;
                match face {
                    0 => ([1.6, s, t], [1.0, 0.0, 0.0]),
                    1 => ([-1.6, s, t], [-1.0, 0.0, 0.0]),
                    2 => ([s, 1.6, t], [0.0, 1.0, 0.0]),
                    3 => ([s, -1.6, t], [0.0, -1.0, 0.0]),
                    4 => ([s, t, 1.6], [0.0, 0.0, 1.0]),
                    _ => ([s, t, -1.6], [0.0, 0.0, -1.0]),
                }
            }
            Shape::Galaxy => {
                // A logarithmic spiral disc: radius grows with the arm angle,
                // and `u` picks which arm plus a little thickness.
                let arms = 2.0;
                let r = 0.35 + (v / std::f32::consts::TAU) * 2.6;
                let ang = v * 1.6 + (u * arms).floor() * (std::f32::consts::TAU / arms);
                let (sa, ca) = ang.sin_cos();
                // Thickness: a thin disc, not a plane, so it catches light.
                let z = (u * 7.0).sin() * 0.18;
                // The normal LEANS OUTWARD along the arm rather than pointing
                // flatly at +z. A flat disc normal is nearly perpendicular to
                // the light for every point on it, so the whole galaxy renders
                // at the bottom of the ramp -- measured: a barely-visible
                // outline. Leaning it gives the arms a bright and a dark side,
                // which is also what makes the spin readable.
                let n = {
                    let (nx, ny, nz) = (ca * 0.55, sa * 0.55, 0.62);
                    let m = (nx * nx + ny * ny + nz * nz).sqrt();
                    [nx / m, ny / m, nz / m]
                };
                ([r * ca, r * sa, z], n)
            }
        }
    }

    /// How many samples to take along each parameter.
    fn steps(&self) -> (usize, usize) {
        let d = (self.density_milli as f32 / 1000.0).clamp(0.1, 4.0);
        // Scaled to the panel: a big wallpaper needs more points to look solid,
        // a small terminal pane does not and should not pay for them.
        let base = (self.cols.max(self.rows) as f32 * 0.9 * d) as usize;
        (base.clamp(24, 720), base.clamp(24, 720))
    }
}

impl AsciiAnimation for Spin3d {
    fn name(&self) -> &'static str {
        "spin3d"
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.alloc();
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let dt = self.frame_ms as f32 / 1000.0;
        self.a = (self.a + dt * (self.spin_a_milli as f32 / 1000.0)) % std::f32::consts::TAU;
        self.b = (self.b + dt * (self.spin_b_milli as f32 / 1000.0)) % std::f32::consts::TAU;

        let n = self.cols * self.rows;
        if self.zbuf.len() != n {
            self.alloc();
        }
        for z in self.zbuf.iter_mut() {
            *z = 0.0;
        }
        for l in self.lum.iter_mut() {
            *l = u8::MAX;
        }

        let (sa, ca) = self.a.sin_cos();
        let (sb, cb) = self.b.sin_cos();
        // Light from up and behind the viewer's shoulder, normalised.
        let light = {
            let l = [0.0f32, 0.7071, -0.7071];
            l
        };

        let scale = (self.scale_milli as f32 / 1000.0).max(0.05);
        // Fit to the SHORT axis, counting a cell as half as wide as it is tall,
        // so the solid stays round and on-screen at any panel shape.
        let k = self.cols.min(self.rows * 2).max(1) as f32 * 0.32 * scale;
        let (cx, cy) = (self.cols as f32 / 2.0, self.rows as f32 / 2.0);
        // Camera distance. Big enough that the near face never crosses the
        // viewer, which would invert the projection.
        const KD: f32 = 7.0;

        let (nu, nv) = self.steps();
        let du = std::f32::consts::TAU / nu as f32;
        let dv = std::f32::consts::TAU / nv as f32;

        let mut v = 0.0f32;
        for _ in 0..nv {
            let mut u = 0.0f32;
            for _ in 0..nu {
                let (p, nrm) = self.point_at(u, v);
                u += du;

                // Rotate about x by `a`, then about z by `b`.
                let rot = |q: [f32; 3]| -> [f32; 3] {
                    let y1 = q[1] * ca - q[2] * sa;
                    let z1 = q[1] * sa + q[2] * ca;
                    let x2 = q[0] * cb - y1 * sb;
                    let y2 = q[0] * sb + y1 * cb;
                    [x2, y2, z1]
                };
                let pr = rot(p);
                let nr = rot(nrm);

                let z = pr[2] + KD;
                if z <= 0.1 {
                    continue;
                }
                let ooz = 1.0 / z;
                let sx = cx + k * ooz * pr[0] * 2.0;
                // Halved: cells are twice as tall as wide.
                let sy = cy + k * ooz * pr[1];
                if sx < 0.0 || sy < 0.0 || sx >= self.cols as f32 || sy >= self.rows as f32 {
                    continue;
                }
                let idx = sy as usize * self.cols + sx as usize;
                // Depth test on 1/z: larger is nearer, so this is one compare
                // and no division.
                if ooz <= self.zbuf[idx] {
                    continue;
                }

                let l = nr[0] * light[0] + nr[1] * light[1] + nr[2] * light[2];
                if l <= 0.0 {
                    // Facing away from the light. Still claim the depth slot --
                    // it is genuinely the nearest surface, and leaving it free
                    // lets the FAR side of the solid show through the near one.
                    self.zbuf[idx] = ooz;
                    self.lum[idx] = 0;
                    continue;
                }
                self.zbuf[idx] = ooz;
                let i = (l * (RAMP.len() - 1) as f32) as usize;
                self.lum[idx] = i.min(RAMP.len() - 1) as u8;
            }
            v += dv;
        }
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let i = row * self.cols + col;
        let l = *self.lum.get(i)?;
        if l == u8::MAX {
            return None;
        }
        let l = l as usize;
        let f = l as f32 / (RAMP.len() - 1) as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f) as u8;
        Some((
            RAMP[l],
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
            Param::text("shape", "shape (donut/sphere/cube/galaxy)", self.shape.as_str()),
            Param::int("spin_a", "tumble (x1000)", self.spin_a_milli, -4000, 4000),
            Param::int("spin_b", "roll (x1000)", self.spin_b_milli, -4000, 4000),
            Param::int("scale", "size (x1000)", self.scale_milli, 200, 3000),
            Param::int("density", "surface detail (x1000)", self.density_milli, 100, 4000),
            Param::colour("lo", "shadow colour", self.lo),
            Param::colour("hi", "lit colour", self.hi),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            // Rejected rather than ignored: a typo'd shape must not silently
            // leave the previous one spinning.
            "shape" => match v.as_text().and_then(Shape::from_str) {
                Some(s) => {
                    self.shape = s;
                    true
                }
                None => false,
            },
            "spin_a" => match v.as_int() {
                Some(n) => {
                    self.spin_a_milli = n.clamp(-4000, 4000);
                    true
                }
                None => false,
            },
            "spin_b" => match v.as_int() {
                Some(n) => {
                    self.spin_b_milli = n.clamp(-4000, 4000);
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
            "density" => match v.as_int() {
                Some(n) => {
                    self.density_milli = n.clamp(100, 4000);
                    true
                }
                None => false,
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

    fn built() -> Spin3d {
        Spin3d::new(80, 40)
    }

    fn lit(s: &Spin3d) -> usize {
        (0..s.rows)
            .flat_map(|r| (0..s.cols).map(move |c| (c, r)))
            .filter(|&(c, r)| s.cell_at(c, r).is_some())
            .count()
    }

    #[test]
    fn the_donut_draws_something_solid() {
        let mut s = built();
        s.step();
        assert!(lit(&s) > 100, "only {} cells drawn", lit(&s));
    }

    #[test]
    fn every_shape_draws_something() {
        // A shape whose parameterisation is wrong renders as an empty screen,
        // which looks exactly like a broken effect.
        for name in ["donut", "sphere", "cube", "galaxy"] {
            let mut s = built();
            assert!(s.set_param("shape", &ParamValue::Text { v: name.into() }));
            s.step();
            assert!(lit(&s) > 50, "{name} drew only {} cells", lit(&s));
        }
    }

    #[test]
    fn the_z_buffer_hides_the_far_surface() {
        // THE point of the depth test. Without it the back of the torus draws
        // over the front and the solid reads as a tangle. The donut's silhouette
        // covers well under half the screen; a tangle covers far more.
        let mut s = built();
        s.step();
        let total = s.cols * s.rows;
        assert!(
            lit(&s) < total / 2,
            "{}/{total} cells lit -- the far side is showing through",
            lit(&s)
        );
    }

    #[test]
    fn the_depth_key_is_nearest_wins() {
        // 1/z: larger is nearer. If the comparison were inverted the far
        // surface would win every contested cell.
        let mut s = built();
        s.step();
        assert!(
            s.zbuf.iter().any(|&z| z > 0.0),
            "nothing ever claimed a depth slot"
        );
    }

    #[test]
    fn it_rotates() {
        let mut s = built();
        s.step();
        let first: Vec<u8> = s.lum.clone();
        for _ in 0..12 {
            s.step();
        }
        assert_ne!(first, s.lum, "the solid never turned");
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
        assert!((fast.a - slow.a).abs() < 1e-3, "{} != {}", fast.a, slow.a);
    }

    #[test]
    fn the_solid_stays_on_screen_at_any_panel_shape() {
        // Fitting to the wrong axis puts a wide panel's donut off the top, or a
        // tall one's off the sides.
        for (c, r) in [(200usize, 20usize), (20, 200), (80, 40)] {
            let mut s = Spin3d::new(c, r);
            s.step();
            assert!(lit(&s) > 20, "{c}x{r} drew only {} cells", lit(&s));
        }
    }

    #[test]
    fn an_unknown_shape_is_rejected() {
        let mut s = built();
        assert!(!s.set_param("shape", &ParamValue::Text { v: "dodecahedron".into() }));
        assert_eq!(s.shape, Shape::Donut);
    }

    #[test]
    fn a_degenerate_grid_does_not_panic() {
        let mut s = Spin3d::new(0, 0);
        s.step();
        assert_eq!(s.cell_at(0, 0), None);
    }

    #[test]
    fn density_changes_the_sample_count() {
        let mut s = built();
        let (u0, _) = s.steps();
        s.set_param("density", &ParamValue::Int { v: 3000 });
        let (u1, _) = s.steps();
        assert!(u1 > u0, "{u1} !> {u0}");
    }
}
