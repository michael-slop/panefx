//! Desktop wallpaper: one animated surface per monitor, behind the icons.
//!
//! A wallpaper is a [`Panel`] whose geometry comes from a *monitor* rather than
//! a tracked window, so the whole renderer, the effects and the cached GDI
//! objects are reused unchanged.
//!
//! Three things here are worth understanding before changing any of it.
//!
//! # 1. Freeze when covered
//!
//! The primary performance requirement. When every consumer of a simulation is
//! occluded, the simulation is **not stepped at all** — not stepped-and-skipped,
//! not drawn-to-a-hidden-window. A fully covered desktop costs a pass over a
//! handful of booleans.
//!
//! # 2. Occlusion comes from the window manager's list
//!
//! The daemon already receives every window on every reconcile, so occlusion is
//! free. It is also *more correct* than enumerating rectangles from Win32:
//! GlazeWM's `display_state` already encodes "this window is on another
//! workspace", which a raw `GetWindowRect` sweep would report as a full-screen
//! occluder and freeze the wallpaper on an empty desktop.
//!
//! **A minimized window still reports its full pre-minimize rect.** Measured on
//! a live IPC session: minimized windows come back as `displayState: "shown"` at
//! 1720x939 and larger. Filtering on `state.type` is not optional.
//!
//! # 3. Simulations are shared, not duplicated
//!
//! Params are global per effect name, so two monitors running the same effect at
//! the same grid size produce identical frames. They therefore share one
//! simulation: N monitors cost N blits but one `step()`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::animation::{self, AsciiAnimation};
use crate::config::Config;
use crate::desktop::{self, MonitorInfo, Workerw};
use crate::ipc;
use crate::panel::{Anchor, Panel};

/// The sentinel effect name meaning "no wallpaper on this monitor".
///
/// Deliberately NOT a member of `animation::EFFECTS`: that list decides which
/// TOML sections hold per-effect params, and an `[off]` section is meaningless.
pub const OFF: &str = "off";

/// How often to retry finding the WorkerW after it goes away.
const WORKER_RETRY: Duration = Duration::from_secs(5);

/// Identity deciding whether two surfaces can share one simulation.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SimKey {
    pub effect: String,
    pub cols: usize,
    pub rows: usize,
}

struct PooledSim {
    sim: Box<dyn AsciiAnimation>,
    /// Live surfaces pointing at this sim. Zero means it can be dropped.
    refs: usize,
    /// Did the last step change anything? Latched per frame, because the first
    /// surface to draw must not clear it for the second.
    dirty: bool,
}

/// Simulations, de-duplicated by `(effect, cols, rows)`.
#[derive(Default)]
pub struct SimPool {
    sims: HashMap<SimKey, PooledSim>,
}

impl SimPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct simulations alive. The sharing property, observable.
    pub fn len(&self) -> usize {
        self.sims.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sims.is_empty()
    }

    /// Take a reference to the sim for `key`, building it if needed.
    pub fn acquire(&mut self, key: &SimKey, cfg: &Config, seed: u64) {
        if let Some(p) = self.sims.get_mut(key) {
            p.refs += 1;
            return;
        }
        let sim = animation::rebuild(cfg, &key.effect, key.cols, key.rows, seed);
        self.sims.insert(
            key.clone(),
            PooledSim {
                sim,
                refs: 1,
                dirty: true,
            },
        );
    }

    /// Release one reference, dropping the sim when the last one goes.
    pub fn release(&mut self, key: &SimKey) {
        let gone = match self.sims.get_mut(key) {
            Some(p) => {
                p.refs = p.refs.saturating_sub(1);
                p.refs == 0
            }
            None => false,
        };
        if gone {
            self.sims.remove(key);
        }
    }

    /// Step only the sims named in `live`.
    ///
    /// This is where the freeze pays off: a sim whose every consumer is occluded
    /// is absent from `live` and does no work at all.
    pub fn step_only(&mut self, live: &[SimKey]) {
        for (key, p) in self.sims.iter_mut() {
            if live.contains(key) {
                p.sim.step();
                p.dirty = p.sim.changed();
            } else {
                p.dirty = false;
            }
        }
    }

    pub fn get(&self, key: &SimKey) -> Option<&dyn AsciiAnimation> {
        self.sims.get(key).map(|p| p.sim.as_ref())
    }

    pub fn dirty(&self, key: &SimKey) -> bool {
        self.sims.get(key).map(|p| p.dirty).unwrap_or(false)
    }

    /// Was this sim stepped on the last `step_only`? Test-facing.
    pub fn refs(&self, key: &SimKey) -> usize {
        self.sims.get(key).map(|p| p.refs).unwrap_or(0)
    }
}

/// Does `windows` fully cover `m`?
///
/// Conservative by construction: it reports occluded only when the visible
/// windows genuinely tile over the whole monitor. Animating when it was not
/// strictly necessary costs a little CPU; freezing a wallpaper the user can see
/// is a bug they notice.
pub fn is_occluded(m: &MonitorInfo, windows: &[ipc::Window]) -> bool {
    let mon_l = m.x;
    let mon_t = m.y;
    let mon_r = m.x + m.width;
    let mon_b = m.y + m.height;
    if m.width <= 0 || m.height <= 0 {
        return false;
    }

    // Only windows that are actually on screen can occlude.
    //
    // `is_visible()` drops other-workspace windows; `is_minimized()` drops the
    // ones that report a stale full-size rect while sitting in the taskbar.
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    for w in windows {
        if !w.is_visible() || w.is_minimized() {
            continue;
        }
        let (l, t) = (w.x, w.y);
        let (r, b) = (w.x + w.width, w.y + w.height);
        // Clip to the monitor; anything outside contributes nothing.
        let (cl, ct) = (l.max(mon_l), t.max(mon_t));
        let (cr, cb) = (r.min(mon_r), b.min(mon_b));
        if cr <= cl || cb <= ct {
            continue;
        }
        // Fast path: one window covering the whole monitor. The common
        // maximised/single-tile case.
        if cl <= mon_l && ct <= mon_t && cr >= mon_r && cb >= mon_b {
            return true;
        }
        rects.push((cl, ct, cr, cb));
    }
    if rects.is_empty() {
        return false;
    }

    // Scanline union.
    //
    // NOT a bounding box: `UnionRect` would report two windows in opposite
    // corners as covering everything between them, freezing a visibly
    // half-empty desktop. Under a tiling WM, two tiles genuinely covering a
    // monitor between them is the NORMAL case, so this has to be exact.
    let mut ys: Vec<i32> = Vec::with_capacity(rects.len() * 2 + 2);
    ys.push(mon_t);
    ys.push(mon_b);
    for (_, t, _, b) in &rects {
        ys.push(*t);
        ys.push(*b);
    }
    ys.sort_unstable();
    ys.dedup();

    for pair in ys.windows(2) {
        let (band_t, band_b) = (pair[0], pair[1]);
        if band_b <= mon_t || band_t >= mon_b || band_b <= band_t {
            continue;
        }
        // Every x-interval covering this horizontal band.
        let mut spans: Vec<(i32, i32)> = rects
            .iter()
            .filter(|(_, t, _, b)| *t <= band_t && *b >= band_b)
            .map(|(l, _, r, _)| (*l, *r))
            .collect();
        if spans.is_empty() {
            return false;
        }
        spans.sort_unstable();
        // Merge and check the merged run spans the monitor's full width.
        let mut reach = mon_l;
        for (l, r) in spans {
            if l > reach {
                return false; // a gap the user can see through
            }
            reach = reach.max(r);
            if reach >= mon_r {
                break;
            }
        }
        if reach < mon_r {
            return false;
        }
    }
    true
}

/// One monitor's wallpaper.
pub struct MonitorSurface {
    pub monitor: MonitorInfo,
    /// `None` when this monitor's effect is `off`.
    ///
    /// The panel is DESTROYED rather than hidden: a hidden panel still owns a
    /// full-screen bitmap, and at 1440x2560 that is ~14MB held for something
    /// nobody asked to see.
    pub panel: Option<Panel>,
    pub effect: String,
    pub sim: Option<SimKey>,
    pub occluded: bool,
    /// Forces one draw after becoming visible again, because a sim may report
    /// `changed() == false` on the resume frame and leave a stale bitmap.
    pub force_redraw: bool,
}

impl MonitorSurface {
    pub fn is_on(&self) -> bool {
        self.effect != OFF
    }
}

/// Every wallpaper surface, plus the layer they live in.
pub struct WallpaperSet {
    worker: Option<Workerw>,
    /// Why there is no wallpaper layer, for honest reporting to the TUI.
    pub error: Option<String>,
    pool: SimPool,
    surfaces: Vec<MonitorSurface>,
    last_frame: Instant,
    last_worker_try: Instant,
}

impl WallpaperSet {
    /// Never fails: a missing WorkerW yields an inert set and a recorded reason.
    /// The terminal backdrops must keep working regardless.
    pub fn new(cfg: &Config) -> Self {
        let mut s = WallpaperSet {
            worker: None,
            error: None,
            pool: SimPool::new(),
            surfaces: Vec::new(),
            last_frame: Instant::now(),
            last_worker_try: Instant::now(),
        };
        s.try_attach(cfg);
        s
    }

    fn try_attach(&mut self, cfg: &Config) {
        match desktop::find() {
            Ok(w) => {
                self.worker = Some(w);
                self.error = None;
                self.rebuild_surfaces(cfg);
            }
            Err(e) => {
                self.worker = None;
                self.error = Some(e.to_string());
            }
        }
    }

    pub fn available(&self) -> bool {
        self.worker.is_some()
    }

    pub fn monitors(&self) -> &[MonitorSurface] {
        &self.surfaces
    }

    pub fn monitor_indices(&self) -> Vec<usize> {
        self.surfaces.iter().map(|s| s.monitor.index).collect()
    }

    /// Effective wallpaper rate.
    ///
    /// The wallpaper is ticked from the daemon loop, which is paced by `fps`, so
    /// it can never run faster than that however high `wallpaper_fps` is set.
    /// Reporting the achievable number keeps the TUI from showing 30 while the
    /// screen shows 3.
    pub fn effective_fps(cfg: &Config) -> u64 {
        cfg.wallpaper_fps.min(cfg.fps).max(1)
    }

    /// The cell size a wallpaper surface should use.
    ///
    /// Deliberately NOT the terminal rule (`render.rs`): a terminal panel wants
    /// the terminal's text cell so glyphs line up with text; a wallpaper has no
    /// text to line up with. Inheriting a 10x15 terminal cell renders a 1440x2560
    /// portrait at 24,480 cells against 10,656 at 15x23 — and 12,012 cells was
    /// the panel measured at 88.7% of a core before the optimisation pass.
    pub fn cell_for(effect: &dyn AsciiAnimation, cfg: &Config) -> (i32, i32) {
        if cfg.wallpaper_cell_explicit {
            return (cfg.wallpaper_cell_w, cfg.wallpaper_cell_h);
        }
        effect
            .preferred_cell()
            .unwrap_or((cfg.wallpaper_cell_w, cfg.wallpaper_cell_h))
    }

    /// A `Config` describing how to draw THIS surface.
    ///
    /// `draw_animation` derives the cell size from the Config it is handed, so
    /// rewriting those fields here keeps the renderer entirely untouched.
    /// Padding and crop are zeroed too — they exist to match Alacritty's text
    /// inset, and on a wallpaper they would just leave an unpainted border.
    fn cfg_for_surface(cfg: &Config, cell: (i32, i32)) -> Config {
        let mut c = cfg.clone();
        c.cell_w = cell.0;
        c.cell_h = cell.1;
        c.cell_explicit = true;
        c.pad_x = 0;
        c.pad_y = 0;
        c.crop_top = 0;
        c
    }

    /// Recreate every surface from the current monitor list and config.
    pub fn rebuild_surfaces(&mut self, cfg: &Config) {
        // Drop old surfaces first so their GDI objects and sim refs go back.
        for s in self.surfaces.drain(..) {
            if let Some(k) = s.sim {
                self.pool.release(&k);
            }
        }
        let Some(worker) = self.worker else {
            return;
        };

        for m in desktop::enumerate_monitors() {
            let effect = cfg
                .wallpaper_effects
                .get(&m.index)
                .cloned()
                .unwrap_or_else(|| OFF.to_string());
            let mut surface = MonitorSurface {
                monitor: m,
                panel: None,
                effect,
                sim: None,
                occluded: false,
                force_redraw: true,
            };
            self.build_surface(&mut surface, cfg, worker);
            self.surfaces.push(surface);
        }
    }

    /// Create (or skip) one surface's panel and simulation.
    fn build_surface(&mut self, s: &mut MonitorSurface, cfg: &Config, worker: Workerw) {
        if !s.is_on() {
            return;
        }
        // Build a throwaway to ask what cell size it wants, then size the grid.
        let probe = animation::build(&s.effect, 1, 1, 0x5EED_1234, cfg);
        let (cw, ch) = Self::cell_for(probe.as_ref(), cfg);
        if cw <= 0 || ch <= 0 {
            return;
        }
        let cols = (s.monitor.width / cw).max(1) as usize;
        let rows = (s.monitor.height / ch).max(1) as usize;

        let key = SimKey {
            effect: s.effect.clone(),
            cols,
            rows,
        };
        self.pool.acquire(&key, cfg, 0x5EED_1234);

        let (x, y) = desktop::to_child(&worker, s.monitor.x, s.monitor.y);
        match Panel::create_anchored(
            Anchor::Desktop {
                parent: worker.hwnd,
            },
            x,
            y,
            s.monitor.width,
            s.monitor.height,
        ) {
            Ok(p) => {
                s.panel = Some(p);
                s.sim = Some(key);
                s.force_redraw = true;
            }
            Err(e) => {
                eprintln!(
                    "[panefx] wallpaper: could not create a surface for {}: {e}",
                    s.monitor.device
                );
                self.pool.release(&key);
            }
        }
    }

    /// Recompute occlusion from the window-manager's list.
    ///
    /// Must be handed the UNFILTERED list: a terminal-only view would miss every
    /// browser and file manager, which are exactly the windows that cover a
    /// desktop.
    pub fn observe_windows(&mut self, windows: &[ipc::Window]) {
        for s in self.surfaces.iter_mut() {
            let now = is_occluded(&s.monitor, windows);
            if s.occluded && !now {
                // Coming back into view: force one draw, because the pooled sim
                // may report unchanged on this frame and leave a stale bitmap.
                s.force_redraw = true;
            }
            s.occluded = now;
        }
    }

    /// Advance and draw. Called every daemon frame; rate-limits internally.
    pub fn tick(&mut self, cfg: &Config, now: Instant) {
        // Recover from an Explorer restart, which destroys the WorkerW and takes
        // our child windows with it.
        if let Some(w) = self.worker {
            if !desktop::is_alive(&w) {
                for s in self.surfaces.drain(..) {
                    if let Some(k) = s.sim {
                        self.pool.release(&k);
                    }
                }
                self.worker = None;
                self.error = Some("WorkerW disappeared (Explorer restarted?)".into());
            }
        }
        if self.worker.is_none() {
            if now.duration_since(self.last_worker_try) >= WORKER_RETRY {
                self.last_worker_try = now;
                self.try_attach(cfg);
            }
            return;
        }

        let budget = Duration::from_millis(1000 / Self::effective_fps(cfg));
        if now.duration_since(self.last_frame) < budget {
            return;
        }
        self.last_frame = now;

        // Only sims with at least one VISIBLE consumer are stepped. This is the
        // freeze: a covered monitor's simulation does no work at all.
        let live: Vec<SimKey> = self
            .surfaces
            .iter()
            .filter(|s| !s.occluded && s.panel.is_some())
            .filter_map(|s| s.sim.clone())
            .collect();
        if live.is_empty() {
            return;
        }
        self.pool.step_only(&live);

        for s in self.surfaces.iter_mut() {
            if s.occluded {
                continue;
            }
            let (Some(key), Some(panel)) = (s.sim.clone(), s.panel.as_mut()) else {
                continue;
            };
            if !(self.pool.dirty(&key) || s.force_redraw) {
                continue;
            }
            let Some(sim) = self.pool.get(&key) else {
                continue;
            };
            let cell = Self::cell_for(sim, cfg);
            let surface_cfg = Self::cfg_for_surface(cfg, cell);
            crate::render::draw_animation(panel, sim, &surface_cfg);
            s.force_redraw = false;
        }
    }

    /// Point one monitor (or every monitor) at an effect.
    pub fn set_effect(
        &mut self,
        monitor: Option<usize>,
        name: &str,
        cfg: &Config,
    ) -> Result<(), String> {
        let wanted = name.trim().to_lowercase();
        if wanted != OFF && !animation::EFFECTS.contains(&wanted.as_str()) {
            return Err(format!("unknown effect '{name}' (or '{OFF}')"));
        }
        let Some(worker) = self.worker else {
            // Config was already updated by the caller, so the choice persists
            // and takes effect if the layer ever becomes available.
            return Ok(());
        };

        let targets: Vec<usize> = match monitor {
            Some(i) => vec![i],
            None => self.monitor_indices(),
        };

        for idx in targets {
            let Some(pos) = self.surfaces.iter().position(|s| s.monitor.index == idx) else {
                continue;
            };
            // Tear the old one down first: `off` must free the bitmap, not hide
            // it, and a changed effect may want a different grid entirely.
            let mut s = self.surfaces.remove(pos);
            if let Some(k) = s.sim.take() {
                self.pool.release(&k);
            }
            s.panel = None;
            s.effect = wanted.clone();
            s.occluded = false;
            s.force_redraw = true;
            self.build_surface(&mut s, cfg, worker);
            self.surfaces.insert(pos, s);
        }
        Ok(())
    }

    /// How many distinct simulations are alive. Observable sharing.
    pub fn sim_count(&self) -> usize {
        self.pool.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            index: 1,
            device: r"\\.\DISPLAY1".into(),
            x,
            y,
            width: w,
            height: h,
            primary: true,
        }
    }

    fn win(x: i32, y: i32, w: i32, h: i32, state: &str, display: &str) -> ipc::Window {
        ipc::Window {
            handle: 1,
            process_name: "test".into(),
            class_name: "Test".into(),
            x,
            y,
            width: w,
            height: h,
            display_state: display.into(),
            state: ipc::WindowState {
                kind: state.into(),
            },
        }
    }

    #[test]
    fn an_empty_desktop_is_not_occluded() {
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &[]));
    }

    #[test]
    fn exact_cover_occludes() {
        let w = [win(0, 0, 1920, 1080, "tiling", "shown")];
        assert!(is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn a_minimized_window_does_not_occlude() {
        // MEASURED TRAP: GlazeWM reports minimized windows as displayState
        // "shown" with their full pre-minimize rect. Without the state filter
        // the wallpaper freezes behind a window that is not on screen at all,
        // forever, with nothing visibly covering it.
        let w = [win(0, 0, 1920, 1080, "minimized", "shown")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn a_window_on_another_workspace_does_not_occlude() {
        // hide_method: cloak means these keep reporting geometry. This is the
        // case a raw GetWindowRect sweep gets wrong, and the reason occlusion
        // reads the window manager's list instead.
        let w = [win(0, 0, 1920, 1080, "tiling", "hidden")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn two_tiles_covering_the_monitor_occlude() {
        // The normal GlazeWM split.
        let w = [
            win(0, 0, 960, 1080, "tiling", "shown"),
            win(960, 0, 960, 1080, "tiling", "shown"),
        ];
        assert!(is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn two_windows_in_opposite_corners_do_not_occlude() {
        // THE bounding-box bug. `UnionRect` of these two spans the whole
        // monitor, so a bounding-box implementation freezes a desktop that is
        // half visible. This test is why the scanline exists.
        let w = [
            win(0, 0, 960, 540, "tiling", "shown"),
            win(960, 540, 960, 540, "tiling", "shown"),
        ];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn a_window_one_pixel_short_does_not_occlude() {
        // ipc::Window is x/y/width/height, so its exclusive right edge is
        // x + width. Mixing that with Win32's exclusive-right RECT is the
        // classic off-by-one here.
        let w = [win(0, 0, 1919, 1080, "tiling", "shown")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
        let w2 = [win(0, 0, 1920, 1079, "tiling", "shown")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w2));
    }

    #[test]
    fn a_window_on_another_monitor_does_not_occlude_this_one() {
        // The portrait monitor sits at a negative origin; a window at 0,0 is on
        // a different screen entirely.
        let portrait = mon(-1440, -1230, 1440, 2560);
        let w = [win(0, 0, 1920, 1080, "tiling", "shown")];
        assert!(!is_occluded(&portrait, &w));
    }

    #[test]
    fn a_window_covering_a_negative_origin_monitor_occludes() {
        let portrait = mon(-1440, -1230, 1440, 2560);
        let w = [win(-1440, -1230, 1440, 2560, "tiling", "shown")];
        assert!(is_occluded(&portrait, &w));
    }

    #[test]
    fn a_band_with_a_gap_does_not_occlude() {
        // Full width at the top, nothing below: the lower band is visible.
        let w = [win(0, 0, 1920, 500, "tiling", "shown")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn three_stacked_bands_occlude() {
        let w = [
            win(0, 0, 1920, 400, "tiling", "shown"),
            win(0, 400, 1920, 400, "tiling", "shown"),
            win(0, 800, 1920, 280, "tiling", "shown"),
        ];
        assert!(is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn overlapping_windows_still_occlude() {
        let w = [
            win(0, 0, 1000, 1080, "tiling", "shown"),
            win(900, 0, 1020, 1080, "tiling", "shown"),
        ];
        assert!(is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    // ---- sim pool ----------------------------------------------------------

    fn key(effect: &str, c: usize, r: usize) -> SimKey {
        SimKey {
            effect: effect.into(),
            cols: c,
            rows: r,
        }
    }

    #[test]
    fn same_effect_and_grid_share_one_sim() {
        // The property that keeps four monitors affordable: one step(), N blits.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key("flames", 80, 40);
        pool.acquire(&k, &cfg, 1);
        pool.acquire(&k, &cfg, 1);
        assert_eq!(pool.len(), 1, "identical keys must share");
        assert_eq!(pool.refs(&k), 2);
    }

    #[test]
    fn different_grids_do_not_share() {
        // Portrait and landscape produce different grids. Sharing would force a
        // resize twice per frame, and `waves::resize` rebuilds its base field.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        pool.acquire(&key("flames", 80, 40), &cfg, 1);
        pool.acquire(&key("flames", 40, 80), &cfg, 1);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn releasing_the_last_reference_evicts() {
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key("flames", 80, 40);
        pool.acquire(&k, &cfg, 1);
        pool.acquire(&k, &cfg, 1);
        pool.release(&k);
        assert_eq!(pool.len(), 1, "still one consumer left");
        pool.release(&k);
        assert_eq!(pool.len(), 0, "last consumer gone, sim dropped");
    }

    #[test]
    fn an_unconsumed_sim_is_not_stepped() {
        // THE performance requirement, as a unit test. A sim absent from `live`
        // must not advance -- that is what makes a covered monitor cost nothing.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key("rain", 40, 20);
        pool.acquire(&k, &cfg, 1);
        pool.step_only(&[]);
        assert!(!pool.dirty(&k), "an occluded sim must not report work done");
    }

    #[test]
    fn a_sim_with_one_visible_consumer_is_stepped() {
        // Composability: sharing must not let an occluded surface freeze a
        // visible one.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key("flames", 40, 20);
        pool.acquire(&k, &cfg, 1);
        pool.acquire(&k, &cfg, 1);
        pool.step_only(std::slice::from_ref(&k));
        assert!(pool.dirty(&k));
    }

    #[test]
    fn effective_fps_cannot_exceed_the_daemon_loop() {
        let mut cfg = Config::default();
        cfg.fps = 3;
        cfg.wallpaper_fps = 30;
        assert_eq!(
            WallpaperSet::effective_fps(&cfg),
            3,
            "the wallpaper is ticked from the daemon loop and cannot outrun it"
        );
    }

    #[test]
    fn wallpaper_cell_prefers_the_effects_own_choice() {
        let cfg = Config::default();
        let waves = animation::build("waves", 10, 10, 1, &cfg);
        assert_eq!(
            WallpaperSet::cell_for(waves.as_ref(), &cfg),
            (15, 23),
            "an effect's preferred cell must win on the desktop"
        );
    }

    #[test]
    fn an_explicit_wallpaper_cell_overrides_the_effect() {
        let mut cfg = Config::default();
        cfg.wallpaper_cell_w = 8;
        cfg.wallpaper_cell_h = 12;
        cfg.wallpaper_cell_explicit = true;
        let waves = animation::build("waves", 10, 10, 1, &cfg);
        assert_eq!(WallpaperSet::cell_for(waves.as_ref(), &cfg), (8, 12));
    }

    #[test]
    fn surface_config_zeroes_terminal_padding() {
        // Alacritty's padding would leave an unpainted border around a monitor.
        let cfg = Config::default();
        let c = WallpaperSet::cfg_for_surface(&cfg, (15, 23));
        assert_eq!((c.pad_x, c.pad_y, c.crop_top), (0, 0, 0));
        assert_eq!((c.cell_w, c.cell_h), (15, 23));
        assert!(c.cell_explicit, "the renderer must honour the size we chose");
    }
}
