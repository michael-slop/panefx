//! Plays a stored frame sequence, fitted to the panel.
//!
//! The procedural effects compute every cell; this one does not compute
//! anything. It holds an artist's frames — see [`crate::fishloop_art`] — and its
//! whole job is to put them on a grid that is not the shape they were drawn on,
//! at a rate that is not the rate they were drawn at.
//!
//! # What it has to get right
//!
//! **Fit.** Authored canvases are small and their own shape (Fish Loop is
//! 60x30). A monitor is neither, so the frame is MAPPED onto the grid the same
//! way [`crate::wizardtorch`] maps its art: contain / stretch / cover, with the
//! cell aspect (`cols : rows * 2`) in the maths.
//!
//! **Timing.** A piece authored at 12fps must play at 12fps whether the panel
//! ticks at 5 or 30. The frame index comes from elapsed WALL-CLOCK time, not
//! from a counter, so the animation neither crawls on the wallpaper nor sprints
//! behind a terminal.
//!
//! **Sparseness.** The frames store only lit cells, and cells the artist left
//! empty stay empty — a painted space is a visible smudge behind a translucent
//! terminal, and on a wallpaper it is a box around the art.
//!
//! # Why a grid is built per frame
//!
//! The stored frames are sparse LISTS, but `cell_at` is asked about one cell at
//! a time and must answer in O(1) — scanning the list per cell would be
//! quadratic. So `step` rasterises the current frame into a lookup grid once.
//! The grid is reused between frames rather than reallocated.

use crate::animation::{AsciiAnimation, Param, ParamValue};
use crate::palette::Rgb;

/// One lit cell of a stored frame: `(col, row, glyph, palette index)`.
pub type Cell = (u16, u16, char, u8);

/// A stored piece: its canvas, palette, frames, and authored rate.
///
/// Borrowed rather than owned so the art stays in `.rodata` and a `FramePlay`
/// costs nothing to construct beyond its own scratch grid.
#[derive(Clone, Copy)]
pub struct Reel {
    pub cols: usize,
    pub rows: usize,
    pub fps: u64,
    pub bg: (u8, u8, u8),
    pub palette: &'static [(u8, u8, u8)],
    pub frames: &'static [&'static [Cell]],
}

/// How a frame is mapped onto a panel of a different shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Keep the artist's proportions; letterbox the remainder.
    Contain,
    /// Fill the panel exactly, stretching the drawing.
    Stretch,
    /// Keep proportions, fill the panel, crop the overflow.
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

pub struct FramePlay {
    name: &'static str,
    reel: Reel,

    cols: usize,
    rows: usize,
    frame_ms: u64,
    /// Elapsed play time in milliseconds. The frame index is derived from this.
    elapsed_ms: u64,

    /// Playback rate, x1000 of the authored fps.
    speed_milli: i64,
    /// Zoom about the centre, x1000.
    detail_milli: i64,
    fit: Fit,
    /// Tint applied to the artist's colours, x1000. 1000 leaves them alone.
    tint_milli: i64,
    bg: Rgb,

    /// The current frame rasterised to `cols * rows`, or `None` per empty cell.
    grid: Vec<Option<(char, Rgb)>>,
    /// Which frame `grid` holds, so it is only rebuilt when the frame changes.
    grid_frame: Option<usize>,
}

impl FramePlay {
    pub fn new(name: &'static str, reel: Reel, cols: usize, rows: usize) -> Self {
        FramePlay {
            name,
            reel,
            cols,
            rows,
            frame_ms: 100,
            elapsed_ms: 0,
            speed_milli: 1000,
            detail_milli: 1000,
            fit: Fit::Contain,
            tint_milli: 1000,
            bg: Rgb(reel.bg.0, reel.bg.1, reel.bg.2),
            grid: Vec::new(),
            grid_frame: None,
        }
    }

    pub fn set_frame_ms(&mut self, ms: u64) {
        self.frame_ms = ms.max(1);
    }

    /// Which frame is showing now.
    ///
    /// From elapsed TIME, not a frame counter: the panel's tick rate and the
    /// piece's authored rate are unrelated, and counting ticks would make the
    /// same animation crawl on a 5fps wallpaper and sprint behind a 30fps
    /// terminal.
    fn frame_index(&self) -> usize {
        let n = self.reel.frames.len().max(1);
        let fps = self.reel.fps.max(1);
        let speed = (self.speed_milli as f64 / 1000.0).max(0.01);
        let per_frame_ms = (1000.0 / (fps as f64 * speed)).max(1.0);
        ((self.elapsed_ms as f64 / per_frame_ms) as usize) % n
    }

    /// Rasterise `frame` into `grid` for the current panel size.
    ///
    /// Drawn art-cell-first rather than panel-cell-first: the art is sparse, so
    /// walking the lit cells and scattering them is proportional to what was
    /// drawn, while walking the panel and sampling would be proportional to the
    /// screen — and on a wallpaper that is far more cells than the artist drew.
    fn rasterise(&mut self, frame: usize) {
        let n = self.cols.saturating_mul(self.rows);
        if self.grid.len() != n {
            self.grid = vec![None; n];
        }
        for c in self.grid.iter_mut() {
            *c = None;
        }
        if n == 0 {
            self.grid_frame = Some(frame);
            return;
        }
        let Some(cells) = self.reel.frames.get(frame) else {
            self.grid_frame = Some(frame);
            return;
        };

        // The inverse of `to_art`, applied to each art cell's corners so a
        // scaled-up drawing FILLS the panel instead of leaving gaps between
        // scattered points.
        let (gw, gh) = (self.cols as f32, self.rows as f32);
        let tint = (self.tint_milli as f32 / 1000.0).clamp(0.0, 3.0);
        for &(acol, arow, ch, ci) in cells.iter() {
            // Where this art cell lands, and how big it is on screen.
            let (y0, x0) = self.art_to_panel(arow as f32, acol as f32);
            let (y1, x1) = self.art_to_panel(arow as f32 + 1.0, acol as f32 + 1.0);
            let (cs, ce) = (x0.min(x1), x0.max(x1));
            let (rs, re) = (y0.min(y1), y0.max(y1));
            let (r, g, b) = self
                .reel
                .palette
                .get(ci as usize)
                .copied()
                .unwrap_or((255, 255, 255));
            let colour = Rgb(
                (r as f32 * tint).clamp(0.0, 255.0) as u8,
                (g as f32 * tint).clamp(0.0, 255.0) as u8,
                (b as f32 * tint).clamp(0.0, 255.0) as u8,
            );
            // At least one cell, so a shrunk drawing does not vanish.
            let c_lo = cs.floor().max(0.0) as usize;
            let c_hi = (ce.ceil() as isize).max(c_lo as isize + 1).min(gw as isize) as usize;
            let r_lo = rs.floor().max(0.0) as usize;
            let r_hi = (re.ceil() as isize).max(r_lo as isize + 1).min(gh as isize) as usize;
            for rr in r_lo..r_hi {
                for cc in c_lo..c_hi {
                    if rr < self.rows && cc < self.cols {
                        self.grid[rr * self.cols + cc] = Some((ch, colour));
                    }
                }
            }
        }
        self.grid_frame = Some(frame);
    }

    /// Art coordinate -> panel coordinate.
    ///
    /// This direction, not panel->art: the art is SPARSE, so rasterising walks
    /// the lit cells and scatters them (work proportional to what was drawn),
    /// where sampling per panel cell would be proportional to the screen -- and
    /// on a wallpaper that is far more cells than the artist drew.
    fn art_to_panel(&self, arow: f32, acol: f32) -> (f32, f32) {
        let (gw, gh) = (self.cols.max(1) as f32, self.rows.max(1) as f32);
        let detail = (self.detail_milli as f32 / 1000.0).max(0.05);
        let mut sv = arow / self.reel.rows.max(1) as f32;
        let mut su = acol / self.reel.cols.max(1) as f32;
        su = (su - 0.5) * detail + 0.5;
        sv = (sv - 0.5) * detail + 0.5;

        if self.fit != Fit::Stretch {
            let panel_aspect = gw / (gh * 2.0);
            let art_aspect = self.reel.cols as f32 / (self.reel.rows as f32 * 2.0);
            let wider = panel_aspect > art_aspect;
            let expand_x = (self.fit == Fit::Contain) == wider;
            let (sx, sy) = if expand_x {
                (panel_aspect / art_aspect, 1.0)
            } else {
                (1.0, art_aspect / panel_aspect)
            };
            su = (su - 0.5) / sx + 0.5;
            sv = (sv - 0.5) / sy + 0.5;
        }
        (sv * gh, su * gw)
    }
}

impl AsciiAnimation for FramePlay {
    fn name(&self) -> &'static str {
        self.name
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        // Force a rebuild: the grid is sized to the panel.
        self.grid_frame = None;
    }

    fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn step(&mut self) {
        self.elapsed_ms = self.elapsed_ms.wrapping_add(self.frame_ms);
        let f = self.frame_index();
        if self.grid_frame != Some(f) {
            self.rasterise(f);
        }
    }

    fn changed(&self) -> bool {
        // A piece authored at 12fps shown on a 30fps panel repeats each frame
        // more than twice; there is no reason to redraw an identical grid.
        self.grid_frame != Some(self.frame_index())
    }

    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        *self.grid.get(row * self.cols + col)?
    }

    fn background(&self) -> Rgb {
        self.bg
    }

    /// Chunky cells, like the other full-screen effects.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        Some((15, 23))
    }

    fn params(&self) -> Vec<Param> {
        vec![
            Param::int("speed", "playback speed (x1000)", self.speed_milli, 100, 4000),
            Param::int("detail", "art scale (x1000)", self.detail_milli, 200, 4000),
            Param::int("tint", "colour tint (x1000)", self.tint_milli, 200, 3000),
            Param::text("fit", "fit (contain/stretch/cover)", self.fit.as_str()),
            Param::colour("bg", "background", self.bg),
        ]
    }

    fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
        match key {
            "speed" => match v.as_int() {
                Some(n) => {
                    self.speed_milli = n.clamp(100, 4000);
                    true
                }
                None => false,
            },
            // Both of these change the RASTERISATION, so the cached grid has to
            // be dropped -- otherwise the knob does nothing until the frame
            // happens to change, which reads as a dead control.
            "detail" => match v.as_int() {
                Some(n) => {
                    self.detail_milli = n.clamp(200, 4000);
                    self.grid_frame = None;
                    true
                }
                None => false,
            },
            "tint" => match v.as_int() {
                Some(n) => {
                    self.tint_milli = n.clamp(200, 3000);
                    self.grid_frame = None;
                    true
                }
                None => false,
            },
            "fit" => match v.as_text().and_then(Fit::from_str) {
                Some(f) => {
                    self.fit = f;
                    self.grid_frame = None;
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
    use crate::fishloop_art as art;

    fn reel() -> Reel {
        Reel {
            cols: art::COLS,
            rows: art::ROWS,
            fps: art::FPS,
            bg: art::BG,
            palette: &art::PALETTE,
            frames: &art::FRAMES,
        }
    }

    fn built(cols: usize, rows: usize) -> FramePlay {
        let mut f = FramePlay::new("fishloop", reel(), cols, rows);
        f.set_frame_ms(100);
        f
    }

    fn lit(f: &FramePlay) -> usize {
        (0..f.rows)
            .flat_map(|r| (0..f.cols).map(move |c| (c, r)))
            .filter(|&(c, r)| f.cell_at(c, r).is_some())
            .count()
    }

    #[test]
    fn the_art_has_frames_and_a_palette() {
        assert!(!art::FRAMES.is_empty(), "no frames were generated");
        assert!(!art::PALETTE.is_empty(), "no palette was generated");
        // Every index must be in range, or a cell silently renders white.
        for (i, frame) in art::FRAMES.iter().enumerate() {
            for &(c, r, _, ci) in frame.iter() {
                assert!(
                    (ci as usize) < art::PALETTE.len(),
                    "frame {i}: palette index {ci} out of range"
                );
                assert!(
                    (c as usize) < art::COLS && (r as usize) < art::ROWS,
                    "frame {i}: cell {c},{r} outside the {}x{} canvas",
                    art::COLS,
                    art::ROWS
                );
            }
        }
    }

    #[test]
    fn it_draws_something_on_a_landscape_panel() {
        // The fit case: a 60x30 canvas on a wide monitor.
        let mut f = built(160, 50);
        f.step();
        assert!(lit(&f) > 20, "only {} cells drawn", lit(&f));
    }

    #[test]
    fn the_frames_advance_and_loop() {
        let mut f = built(80, 30);
        f.step();
        let first = f.frame_index();
        // Long enough to pass the last frame and wrap.
        for _ in 0..(art::FRAMES.len() * 4) {
            f.step();
        }
        assert!(
            f.frame_index() < art::FRAMES.len(),
            "frame index escaped the reel"
        );
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            f.step();
            seen.insert(f.frame_index());
        }
        assert!(seen.len() > 1, "the reel never advanced past frame {first}");
    }

    #[test]
    fn playback_is_paced_by_time_not_tick_count() {
        // The whole point of deriving the index from elapsed ms: a piece
        // authored at 12fps must play at 12fps on a 5fps wallpaper and a 30fps
        // pane alike.
        let mut slow = built(80, 30);
        slow.set_frame_ms(200); // 5fps panel
        let mut fast = built(80, 30);
        fast.set_frame_ms(20); // 50fps panel
        for _ in 0..5 {
            slow.step();
        }
        for _ in 0..50 {
            fast.step();
        }
        // Both have played 1 second.
        assert_eq!(
            slow.frame_index(),
            fast.frame_index(),
            "1s of playback disagreed between a 5fps and a 50fps panel"
        );
    }

    #[test]
    fn empty_cells_stay_empty() {
        // The art is sparse and must stay sparse: a painted space is a visible
        // smudge behind a translucent terminal.
        let mut f = built(120, 40);
        f.step();
        assert!(lit(&f) < f.cols * f.rows, "every cell was painted");
    }

    #[test]
    fn stretch_fills_more_than_contain() {
        // Contain letterboxes a 60x30 canvas on a wide panel; stretch must not.
        let mut c = built(160, 40);
        c.step();
        let contained = lit(&c);
        let mut s = built(160, 40);
        assert!(s.set_param("fit", &ParamValue::Text { v: "stretch".into() }));
        s.step();
        assert!(
            lit(&s) > contained,
            "stretch {} !> contain {contained}",
            lit(&s)
        );
    }

    #[test]
    fn an_unknown_fit_is_rejected() {
        let mut f = built(80, 30);
        assert!(!f.set_param("fit", &ParamValue::Text { v: "sideways".into() }));
        assert_eq!(f.fit, Fit::Contain);
    }

    #[test]
    fn changing_the_scale_rebuilds_the_frame() {
        // A knob that edits a field but not the cached grid does nothing until
        // the next frame boundary, which reads as a dead control.
        let mut f = built(120, 40);
        f.step();
        assert!(f.grid_frame.is_some());
        assert!(f.set_param("detail", &ParamValue::Int { v: 2000 }));
        assert!(f.grid_frame.is_none(), "the cached grid was not dropped");
    }

    #[test]
    fn a_degenerate_grid_does_not_panic() {
        let mut f = built(0, 0);
        f.step();
        assert_eq!(f.cell_at(0, 0), None);
    }

    #[test]
    fn a_resize_rebuilds_the_grid() {
        let mut f = built(80, 30);
        f.step();
        f.resize(40, 20);
        f.step();
        assert_eq!(f.grid.len(), 40 * 20);
    }
}
