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
//! # 3. Every monitor owns its simulation
//!
//! Monitors do **not** share simulations, even on the same effect at the same
//! grid size. Sharing would be cheaper — N monitors for one `step()` — but two
//! same-sized screens would then show the *same frame at the same instant*:
//! identical raindrops, identical waves, perfectly mirrored. That is a visible
//! defect, and resolution collisions are the common case, not the exotic one.
//!
//! So each surface gets its own state and its own **seed**, derived from the
//! monitor index. The cost is one `step()` per monitor rather than one overall —
//! and per the project's own profiling that is the cheap half: the simulation is
//! ~1% of a core while the renderer was ~72%, and the renderer was already
//! per-surface. The freeze below is what actually keeps this affordable.

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

/// Which simulation a surface owns.
///
/// Keyed by **monitor**, not by `(effect, grid)`. Two monitors on the same
/// effect at the same size must animate independently, so they get separate
/// entries even though their content would otherwise be interchangeable.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SimKey {
    /// `DISPLAY<n>` index. This is what makes the key unique per screen.
    pub monitor: usize,
    pub effect: String,
    pub cols: usize,
    pub rows: usize,
}

/// A per-monitor seed.
///
/// Two monitors running the same effect must not start from the same state, or
/// they animate in lockstep and look mirrored. Mixing the monitor index in with
/// a large odd constant separates them without needing any entropy source (the
/// daemon must stay deterministic for its tests).
pub fn seed_for(monitor: usize) -> u64 {
    0x5EED_1234u64.wrapping_add((monitor as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

struct PooledSim {
    sim: Box<dyn AsciiAnimation>,
    /// Did the last step change anything? Latched per frame, because the draw
    /// decision happens after every sim has stepped.
    dirty: bool,
}

/// One simulation per monitor.
///
/// Not a de-duplicating cache: the key includes the monitor index precisely so
/// that two identical screens get two independent animations.
#[derive(Default)]
pub struct SimPool {
    sims: HashMap<SimKey, PooledSim>,
}

impl SimPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Live simulations — one per monitor with an effect on.
    pub fn len(&self) -> usize {
        self.sims.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sims.is_empty()
    }

    /// Build the simulation for `key`, seeded so it diverges from every other
    /// monitor's.
    pub fn acquire(&mut self, key: &SimKey, cfg: &Config) {
        // Scope::Wallpaper -- THE line that makes the desktop tunable
        // independently of the terminal backdrop.
        let sim = animation::rebuild(
            cfg,
            &key.effect,
            key.cols,
            key.rows,
            seed_for(key.monitor),
            animation::Scope::Wallpaper(key.monitor),
        );
        self.sims.insert(key.clone(), PooledSim { sim, dirty: true });
    }

    /// Drop a monitor's simulation, freeing its state.
    pub fn release(&mut self, key: &SimKey) {
        self.sims.remove(key);
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

    /// Re-apply the saved wallpaper params to every live sim running `effect`.
    ///
    /// IN PLACE, not a rebuild. Rebuilding would reset the animation to frame
    /// zero on every arrow-key press, turning "nudge a colour" into a stutter.
    /// Marks them dirty so the change is drawn on the next tick rather than
    /// waiting for the simulation to happen to change by itself.
    pub fn reapply(&mut self, effect: &str, cfg: &Config) {
        for (k, p) in self.sims.iter_mut() {
            if k.effect == effect {
                // Each sim re-reads with ITS OWN monitor index, so two screens
                // on the same effect can hold different values.
                animation::apply_saved_params(
                    p.sim.as_mut(),
                    cfg,
                    animation::Scope::Wallpaper(k.monitor),
                );
                p.dirty = true;
            }
        }
    }

    /// Does a simulation exist for this monitor? Test-facing.
    pub fn has(&self, key: &SimKey) -> bool {
        self.sims.contains_key(key)
    }

    /// A monitor's current grid, as a cheap fingerprint of its visible state.
    /// Test-facing: two monitors showing identical grids are mirroring.
    pub fn fingerprint(&self, key: &SimKey) -> Option<Vec<Option<char>>> {
        let p = self.sims.get(key)?;
        let (cols, rows) = p.sim.dimensions();
        let mut out = Vec::with_capacity(cols * rows);
        for r in 0..rows {
            for c in 0..cols {
                out.push(p.sim.cell_at(c, r).map(|(ch, _)| ch));
            }
        }
        Some(out)
    }
}

/// Does `windows` fully cover `m`?
///
/// Conservative by construction: it reports occluded only when the visible
/// windows genuinely tile over the whole monitor. Animating when it was not
/// strictly necessary costs a little CPU; freezing a wallpaper the user can see
/// is a bug they notice.
pub fn is_occluded(m: &MonitorInfo, windows: &[ipc::Window]) -> bool {
    // Check the degenerate case BEFORE computing edges, so a zero-area monitor
    // never reaches the scanline (where a zero-height band would trivially
    // "cover" it).
    if m.width <= 0 || m.height <= 0 {
        return false;
    }
    // Saturating throughout: these are i32 screen coordinates straight off the
    // wire, and `x + width` on a bogus rect overflows. In release that WRAPS
    // rather than panicking, turning a far-away window into one that appears to
    // cover the screen — i.e. a wallpaper frozen for no visible reason.
    let mon_l = m.x;
    let mon_t = m.y;
    let mon_r = m.x.saturating_add(m.width);
    let mon_b = m.y.saturating_add(m.height);

    // Only windows that are actually on screen can occlude.
    //
    // `is_visible()` drops other-workspace windows; `is_minimized()` drops the
    // ones that report a stale full-size rect while sitting in the taskbar.
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    for w in windows {
        if !w.is_visible() || w.is_minimized() {
            continue;
        }
        // Skip degenerate rects outright — a window mid-creation or being
        // animated closed can report zero or negative extents.
        if w.width <= 0 || w.height <= 0 {
            continue;
        }
        let (l, t) = (w.x, w.y);
        let (r, b) = (w.x.saturating_add(w.width), w.y.saturating_add(w.height));
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
    /// The BOTTOM layer's effect. Kept as its own field because it is what
    /// `is_on`, the config and the TUI have always meant by "this monitor's
    /// effect"; the layers above it are additive.
    pub effect: String,
    /// One simulation per layer, bottom first.
    ///
    /// A single-effect monitor is a stack of one, so nothing special-cases the
    /// common case. Empty means nothing is drawn -- an `off` monitor, or one
    /// whose surface could not be built.
    pub sims: Vec<SimKey>,
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
            for k in s.sims {
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
                sims: Vec::new(),
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
        // A monitor with no area cannot show anything. Without this it would
        // still get a 1x1 surface (the `.max(1)` below), i.e. a real window and
        // a real simulation drawing one cell nobody will ever see.
        if s.monitor.width <= 0 || s.monitor.height <= 0 {
            return;
        }
        let cols = (s.monitor.width / cw).max(1) as usize;
        let rows = (s.monitor.height / ch).max(1) as usize;

        // One simulation per layer, ALL ON THE SAME GRID.
        //
        // The grid comes from the bottom layer's preferred cell size, not each
        // layer's own: the layers composite cell-for-cell into one panel, so a
        // second grid would have nothing to align to. An upper layer that wants
        // chunkier cells than the base simply draws at the base's resolution.
        let stack = cfg.wallpaper_stack(s.monitor.index);
        let stack = if stack.is_empty() {
            // `effect` is the bottom layer and `is_on` already passed, so this
            // only happens if the config and the surface disagree -- fall back
            // to the surface's own effect rather than drawing nothing.
            vec![s.effect.clone()]
        } else {
            stack
        };
        s.sims.clear();
        for effect in stack {
            let key = SimKey {
                monitor: s.monitor.index,
                effect,
                cols,
                rows,
            };
            self.pool.acquire(&key, cfg);
            s.sims.push(key);
        }

        // PARENT-RELATIVE coordinates: the surface is a CHILD of Explorer's
        // icon host, so a screen coordinate would land it off the parent's edge
        // wherever the desktop does not start at (0,0) — which is any layout
        // with a monitor left of or above the primary.
        let (x, y) = desktop::to_child(&worker, s.monitor.x, s.monitor.y);
        match Panel::create_anchored(
            Anchor::Desktop {
                // `SHELLDLL_DefView` on the raised model — the only parent in
                // which a GDI child is composited at all. `desktop::find`
                // records the measurements.
                parent: worker.parent,
                raised: worker.raised,
            },
            x,
            y,
            s.monitor.width,
            s.monitor.height,
        ) {
            Ok(p) => {
                // Assert the z-slot NOW: a freshly created child lands at the
                // TOP of its siblings, i.e. over SysListView32 and every icon.
                // It has to go to the bottom of DefView's children.
                if let Err(e) = p.pin_behind_target() {
                    crate::log_warn!("[panefx] wallpaper: z-order for {} failed: {e}", s.monitor.device);
                }
                s.panel = Some(p);
                s.force_redraw = true;
            }
            Err(e) => {
                crate::log_warn!(
                    "[panefx] wallpaper: could not create a surface for {}: {e}",
                    s.monitor.device
                );
                // Every layer, not one: a partial release leaks a simulation
                // per failed monitor, and the pool never frees it.
                for k in std::mem::take(&mut s.sims) {
                    self.pool.release(&k);
                }
            }
        }
    }

    /// Has the set of monitors changed since the surfaces were built?
    ///
    /// Compares index AND geometry, not just the count: swapping a 1080p screen
    /// for a 1440p one at the same index keeps the count identical while making
    /// every surface the wrong size.
    fn monitors_changed(&self, current: &[MonitorInfo]) -> bool {
        if current.len() != self.surfaces.len() {
            return true;
        }
        current
            .iter()
            .zip(self.surfaces.iter())
            .any(|(m, s)| *m != s.monitor)
    }

    /// Notice monitors being plugged in, unplugged, or resized.
    ///
    /// GlazeWM does emit `monitor_added` / `monitor_removed`, but they arrive as
    /// a generic "something changed" hint carrying no geometry, so this compares
    /// against the real display list. Without it a new monitor never gets a
    /// surface and a changed one keeps drawing at its old size — and because the
    /// daemon usually outlives several dock/undock cycles, that is the normal
    /// case rather than a rare one.
    ///
    /// Cheap enough for the 500ms poll: `EnumDisplayMonitors` over a handful of
    /// displays, then an equality check.
    pub fn poll_monitors(&mut self, cfg: &Config) -> bool {
        if self.worker.is_none() {
            return false;
        }
        let current = desktop::enumerate_monitors();
        if !self.monitors_changed(&current) {
            return false;
        }
        crate::log_info!(
            "[panefx] monitor layout changed ({} -> {}), rebuilding wallpaper",
            self.surfaces.len(),
            current.len()
        );
        // Re-ATTACH, not just rebuild. Every pane's position is computed
        // relative to the parent layer's top-left (`desktop::to_child`), and a
        // layout change moves that origin: unrotating a portrait monitor
        // reshapes the whole virtual desktop. Rebuilding against the origin
        // captured at first attach placed every pane offset by the delta --
        // black squares where no pane covered the screen, and a flashing strip
        // where a neighbour's misplaced edge overlapped. `try_attach` re-runs
        // `desktop::find()`, refreshing the origin, then rebuilds.
        self.try_attach(cfg);
        true
    }

    /// Recompute occlusion from the window-manager's list.
    ///
    /// Must be handed the UNFILTERED list: a terminal-only view would miss every
    /// browser and file manager, which are exactly the windows that cover a
    /// desktop.
    pub fn observe_windows(&mut self, windows: &[ipc::Window]) {
        for s in self.surfaces.iter_mut() {
            // An `off` monitor has no panel and no simulation, so whether it is
            // covered is a question with no consumer. Skip the scanline entirely
            // — this runs on every window event, and with most screens off it
            // would otherwise be the bulk of the work the wallpaper does.
            if s.panel.is_none() {
                s.occluded = false;
                continue;
            }
            let now = is_occluded(&s.monitor, windows);
            if s.occluded && !now {
                // Coming back into view: force one draw, because the sim may
                // report unchanged on this frame and leave a stale bitmap.
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
                    for k in s.sims {
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

        // Repair anything Windows asked us to repaint since the last tick.
        //
        // `WM_PAINT` on a desktop surface only RECORDS the damage (see
        // `render::note_desktop_damage`); this is where it is acted on, because
        // repairing means presenting, and every present belongs on this thread.
        // Setting `force_redraw` reuses the same path the occluded->visible
        // transition already uses.
        let damaged = crate::render::take_desktop_damage();
        if !damaged.is_empty() {
            for s in self.surfaces.iter_mut() {
                if let Some(p) = s.panel.as_ref() {
                    if damaged.contains(&(p.hwnd.0 as isize)) {
                        s.force_redraw = true;
                    }
                }
            }
        }

        // Only sims with at least one VISIBLE consumer are stepped. This is the
        // freeze: a covered monitor's simulation does no work at all.
        let live: Vec<SimKey> = self
            .surfaces
            .iter()
            .filter(|s| !s.occluded && s.panel.is_some())
            .flat_map(|s| s.sims.iter().cloned())
            .collect();
        if live.is_empty() {
            return;
        }
        self.pool.step_only(&live);

        for s in self.surfaces.iter_mut() {
            if s.occluded {
                continue;
            }
            if s.sims.is_empty() {
                continue;
            }
            let keys = s.sims.clone();
            let Some(panel) = s.panel.as_mut() else {
                continue;
            };
            // Explorer reorders and rebuilds its desktop children on theme
            // changes, wallpaper changes and desktop refreshes, which can leave
            // our surface above the icons again. Cheap to re-assert; expensive
            // to debug when the icons silently vanish an hour later.
            let _ = panel.pin_behind_target();
            // ANY layer moving means the frame changed -- a still layer above a
            // moving one still has to be redrawn, because the redraw clears the
            // panel before compositing.
            if !(keys.iter().any(|k| self.pool.dirty(k)) || s.force_redraw) {
                continue;
            }
            let layers: Vec<&dyn animation::AsciiAnimation> = keys
                .iter()
                .filter_map(|k| self.pool.get(k))
                .map(|b| b as &dyn animation::AsciiAnimation)
                .collect();
            if layers.is_empty() {
                continue;
            }
            // The BOTTOM layer owns the cell size and the background: it is the
            // only one that can, since an upper layer's background would erase
            // everything beneath it.
            let cell = Self::cell_for(layers[0], cfg);
            let surface_cfg = Self::cfg_for_surface(cfg, cell);
            crate::render::draw_layers(panel, &layers, &surface_cfg);
            s.force_redraw = false;
        }
    }

    /// Push a changed wallpaper param out to every live surface using it.
    ///
    /// `force_redraw` matters: `tick` only draws a surface when its simulation
    /// reports a change or the flag is set, so on a settled effect a colour
    /// tweak would otherwise sit invisible until the animation moved on its own.
    pub fn reapply_params(&mut self, effect: &str, cfg: &Config) {
        self.pool.reapply(effect, cfg);
        for s in self.surfaces.iter_mut() {
            if s.effect == effect {
                s.force_redraw = true;
            }
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
            // No layer to draw into (see `desktop::find`). Report success so the
            // caller still records the choice: it persists to config and takes
            // effect if the layer ever becomes available. There is nothing to
            // validate against here — with no monitors enumerated, any index is
            // as plausible as another.
            return Ok(());
        };

        let targets: Vec<usize> = match monitor {
            Some(i) => {
                // Naming a monitor that is not attached must SAY SO. Silently
                // returning ok while changing nothing is the same shape as the
                // ANSI_CHARSET bug: the call succeeds, nothing errors, and you
                // simply do not get what you asked for.
                if !self.surfaces.iter().any(|s| s.monitor.index == i) {
                    let have: Vec<String> =
                        self.monitor_indices().iter().map(|n| n.to_string()).collect();
                    return Err(if have.is_empty() {
                        format!("no monitor {i} — no monitors are attached")
                    } else {
                        format!("no monitor {i} — attached: {}", have.join(", "))
                    });
                }
                vec![i]
            }
            None => self.monitor_indices(),
        };

        for idx in targets {
            let Some(pos) = self.surfaces.iter().position(|s| s.monitor.index == idx) else {
                continue;
            };
            // Tear the old one down first: `off` must free the bitmap, not hide
            // it, and a changed effect may want a different grid entirely.
            let mut s = self.surfaces.remove(pos);
            for k in std::mem::take(&mut s.sims) {
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

    /// Live simulations — one per monitor whose effect is not `off`.
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
    fn a_zero_or_negative_size_window_is_ignored() {
        // Degenerate rects turn up in the wild (a window mid-creation, or one
        // being animated closed). They must not divide-by-zero, panic, or count
        // as coverage.
        let w = [
            win(0, 0, 0, 0, "tiling", "shown"),
            win(100, 100, -50, -50, "tiling", "shown"),
        ];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn a_degenerate_monitor_is_never_occluded() {
        // Guards the scanline against a zero-width band, which would otherwise
        // trivially "cover" a monitor with no area.
        let w = [win(0, 0, 1920, 1080, "tiling", "shown")];
        assert!(!is_occluded(&mon(0, 0, 0, 1080), &w));
        assert!(!is_occluded(&mon(0, 0, 1920, 0), &w));
    }

    #[test]
    fn extreme_coordinates_do_not_overflow() {
        // A window rect near i32::MAX must not wrap when `x + width` is computed.
        // Wrapping would make a far-away window look like it covers the screen.
        let w = [win(i32::MAX - 10, i32::MAX - 10, 100, 100, "tiling", "shown")];
        assert!(!is_occluded(&mon(0, 0, 1920, 1080), &w));
    }

    #[test]
    fn an_off_monitor_is_never_reported_as_occluded() {
        // `off` means no surface at all, so "covered" is meaningless — and the
        // TUI must not show a frozen marker for a monitor showing nothing.
        let mut set = set_with(vec![mon_at(1, 1920, 1080)]);
        set.surfaces[0].effect = OFF.to_string();
        set.surfaces[0].panel = None;
        let covering = [win(0, 0, 1920, 1080, "tiling", "shown")];
        set.observe_windows(&covering);
        assert!(
            !set.surfaces[0].occluded,
            "an off monitor has no surface to freeze"
        );
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

    fn key(mon: usize, effect: &str, c: usize, r: usize) -> SimKey {
        SimKey {
            monitor: mon,
            effect: effect.into(),
            cols: c,
            rows: r,
        }
    }

    #[test]
    fn two_identical_monitors_get_separate_simulations() {
        // Same effect, same grid, different screens. They must NOT share, or the
        // two monitors animate in lockstep.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let a = key(1, "flames", 80, 40);
        let b = key(3, "flames", 80, 40);
        pool.acquire(&a, &cfg);
        pool.acquire(&b, &cfg);
        assert_eq!(pool.len(), 2, "one simulation per monitor, never shared");
        assert!(pool.has(&a) && pool.has(&b));
    }

    #[test]
    fn two_identical_monitors_do_not_mirror_each_other() {
        // THE point of the per-monitor seed, checked on the actual output rather
        // than on the plumbing. Two same-sized screens on the same effect must
        // not show the same frame — that is what a shared sim looked like.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let a = key(1, "flames", 60, 30);
        let b = key(3, "flames", 60, 30);
        pool.acquire(&a, &cfg);
        pool.acquire(&b, &cfg);
        // Advance both together, exactly as the daemon would.
        for _ in 0..40 {
            pool.step_only(&[a.clone(), b.clone()]);
        }
        let fa = pool.fingerprint(&a).expect("monitor 1 grid");
        let fb = pool.fingerprint(&b).expect("monitor 3 grid");
        assert_eq!(fa.len(), fb.len(), "same grid size, so comparable");
        assert_ne!(
            fa, fb,
            "identical monitors are showing identical frames — they are mirroring"
        );
    }

    #[test]
    fn the_seed_differs_per_monitor() {
        // The mechanism behind the test above. Equal seeds would make two
        // monitors replay the same sequence even with separate state.
        assert_ne!(seed_for(1), seed_for(3));
        assert_ne!(seed_for(1), seed_for(2));
        assert_ne!(seed_for(0), seed_for(1));
        // Deterministic: the daemon's tests depend on it, so no entropy here.
        assert_eq!(seed_for(3), seed_for(3));
    }

    #[test]
    fn one_monitor_switching_effect_replaces_only_its_own_sim() {
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let a_flames = key(1, "flames", 80, 40);
        let b_flames = key(3, "flames", 80, 40);
        pool.acquire(&a_flames, &cfg);
        pool.acquire(&b_flames, &cfg);
        // Monitor 1 switches to rain: its old sim goes, monitor 3 is untouched.
        pool.release(&a_flames);
        pool.acquire(&key(1, "rain", 80, 40), &cfg);
        assert!(!pool.has(&a_flames), "the replaced sim must be freed");
        assert!(pool.has(&b_flames), "the other monitor must be unaffected");
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn releasing_frees_the_simulation() {
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key(1, "flames", 80, 40);
        pool.acquire(&k, &cfg);
        assert_eq!(pool.len(), 1);
        pool.release(&k);
        assert_eq!(pool.len(), 0, "off must free the state, not just hide it");
    }

    #[test]
    fn an_unconsumed_sim_is_not_stepped() {
        // THE performance requirement, as a unit test. A sim absent from `live`
        // must not advance -- that is what makes a covered monitor cost nothing.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key(1, "rain", 40, 20);
        pool.acquire(&k, &cfg);
        pool.step_only(&[]);
        assert!(!pool.dirty(&k), "an occluded sim must not report work done");
    }

    #[test]
    fn one_occluded_monitor_does_not_freeze_another() {
        // Monitor 1 is covered, monitor 3 is visible. Now that each owns its
        // simulation, the covered one must go idle WITHOUT stalling the other.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let covered = key(1, "flames", 40, 20);
        let visible = key(3, "flames", 40, 20);
        pool.acquire(&covered, &cfg);
        pool.acquire(&visible, &cfg);
        pool.step_only(std::slice::from_ref(&visible));
        assert!(pool.dirty(&visible), "the visible monitor must keep animating");
        assert!(!pool.dirty(&covered), "the covered monitor must go idle");
    }

    #[test]
    fn a_frozen_monitor_holds_its_frame_and_resumes_from_it() {
        // "Static instead of dynamic": a covered monitor stops where it was and
        // carries on from there, rather than resetting.
        let cfg = Config::default();
        let mut pool = SimPool::new();
        let k = key(1, "flames", 40, 20);
        pool.acquire(&k, &cfg);
        for _ in 0..10 {
            pool.step_only(std::slice::from_ref(&k));
        }
        let before = pool.fingerprint(&k).unwrap();
        // Covered for a while: no stepping at all.
        for _ in 0..30 {
            pool.step_only(&[]);
        }
        let during = pool.fingerprint(&k).unwrap();
        assert_eq!(before, during, "a frozen monitor must not advance");
        // Uncovered: it moves again.
        for _ in 0..5 {
            pool.step_only(std::slice::from_ref(&k));
        }
        let after = pool.fingerprint(&k).unwrap();
        assert_ne!(during, after, "it must resume when it becomes visible");
    }

    // ---- monitor hot-plug --------------------------------------------------

    fn surface_for(m: MonitorInfo) -> MonitorSurface {
        MonitorSurface {
            monitor: m,
            panel: None,
            effect: "flames".into(),
            sims: Vec::new(),
            occluded: false,
            force_redraw: true,
        }
    }

    /// A `WallpaperSet` with surfaces but no real windows, for pure logic tests.
    fn set_with(monitors: Vec<MonitorInfo>) -> WallpaperSet {
        let mut w = WallpaperSet {
            worker: None,
            error: None,
            pool: SimPool::new(),
            surfaces: Vec::new(),
            last_frame: std::time::Instant::now(),
            last_worker_try: std::time::Instant::now(),
        };
        w.surfaces = monitors.into_iter().map(surface_for).collect();
        w
    }

    fn mon_at(index: usize, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            index,
            device: format!(r"\\.\DISPLAY{index}"),
            x: 0,
            y: 0,
            width: w,
            height: h,
            primary: index == 1,
        }
    }

    #[test]
    fn an_unchanged_monitor_list_does_not_rebuild() {
        // The 500ms poll must be free when nothing has moved, or it churns every
        // surface (and every simulation) twice a second.
        let ms = vec![mon_at(1, 1920, 1080), mon_at(3, 1280, 720)];
        let set = set_with(ms.clone());
        assert!(!set.monitors_changed(&ms));
    }

    #[test]
    fn plugging_in_a_monitor_is_noticed() {
        let set = set_with(vec![mon_at(1, 1920, 1080)]);
        let now = vec![mon_at(1, 1920, 1080), mon_at(2, 2560, 1440)];
        assert!(set.monitors_changed(&now));
    }

    #[test]
    fn unplugging_a_monitor_is_noticed() {
        let set = set_with(vec![mon_at(1, 1920, 1080), mon_at(2, 2560, 1440)]);
        let now = vec![mon_at(1, 1920, 1080)];
        assert!(set.monitors_changed(&now));
    }

    #[test]
    fn a_resolution_change_is_noticed_even_though_the_count_is_the_same() {
        // The case a count check would miss entirely: swap a 1080p screen for a
        // 1440p one and every surface is now the wrong size, silently.
        let set = set_with(vec![mon_at(1, 1920, 1080)]);
        let now = vec![mon_at(1, 2560, 1440)];
        assert!(
            set.monitors_changed(&now),
            "same count, different geometry — surfaces would be stale"
        );
    }

    #[test]
    fn a_monitor_moving_is_noticed() {
        // Rearranging displays changes origins, and a wallpaper positioned from
        // a stale origin lands on the wrong screen.
        let set = set_with(vec![mon_at(1, 1920, 1080)]);
        let mut moved = mon_at(1, 1920, 1080);
        moved.x = -1920;
        assert!(set.monitors_changed(&[moved]));
    }

    #[test]
    fn a_zero_area_monitor_gets_no_surface() {
        // A display reporting no area (mid-mode-change, or a virtual device)
        // would otherwise get a real window and a real simulation drawing a
        // single cell nobody can see.
        let cfg = Config::default();
        let mut set = set_with(vec![]);
        set.worker = Some(Workerw {
            hwnd: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            origin: (0, 0),
            parent: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            raised: false,
            parent_class: "WorkerW",
        });
        let mut s = surface_for(mon_at(1, 0, 1080));
        s.effect = "flames".into();
        let worker = set.worker.unwrap();
        set.build_surface(&mut s, &cfg, worker);
        assert!(s.sims.is_empty(), "no simulation for a zero-area monitor");
        assert_eq!(set.sim_count(), 0);
    }

    #[test]
    fn naming_an_unattached_monitor_is_an_error_not_a_silent_no_op() {
        // Returning ok while changing nothing is the ANSI_CHARSET shape: the
        // call succeeds, nothing errors, and you do not get what you asked for.
        let cfg = Config::default();
        let mut set = set_with(vec![mon_at(1, 1920, 1080), mon_at(3, 1280, 720)]);
        // Pretend the layer exists so we reach the validation rather than the
        // no-layer early return.
        set.worker = Some(Workerw {
            hwnd: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            origin: (0, 0),
            parent: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            raised: false,
            parent_class: "WorkerW",
        });
        let err = set
            .set_effect(Some(9), "waves", &cfg)
            .expect_err("monitor 9 is not attached");
        assert!(err.contains('9'), "the message must name the bad index: {err}");
        assert!(
            err.contains('1') && err.contains('3'),
            "and list what IS attached: {err}"
        );
    }

    #[test]
    fn an_unknown_effect_is_rejected_before_anything_is_touched() {
        let cfg = Config::default();
        let mut set = set_with(vec![mon_at(1, 1920, 1080)]);
        let err = set.set_effect(Some(1), "banana", &cfg).expect_err("bogus");
        assert!(err.contains("banana"));
        assert_eq!(
            set.surfaces[0].effect, "flames",
            "a rejected effect must not mutate the surface"
        );
    }

    #[test]
    fn setting_an_effect_with_no_layer_succeeds_so_the_choice_persists() {
        // No WorkerW (the Windows 11 25H2 case). The daemon still records the
        // choice, so it applies if the layer ever appears — and so the TUI does
        // not report a spurious failure for something that is not the user's
        // fault.
        let cfg = Config::default();
        let mut set = set_with(vec![]);
        assert!(set.worker.is_none());
        assert!(set.set_effect(Some(3), "waves", &cfg).is_ok());
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
    fn the_terminal_cell_never_reaches_the_wallpaper_resolver() {
        // The 57% trap, tested on the RESOLVER rather than on the config fields
        // — checking that `Config` holds the right numbers proves nothing if
        // `cell_for` goes and reads the terminal's instead.
        //
        // Measured: a 1440x2560 portrait is 24,480 cells at 10x15 against
        // 10,656 at 15x23, and 12,012 cells was the panel that cost 88.7% of a
        // core before the optimisation pass.
        let mut cfg = Config::default();
        // Exactly the live config: an explicit terminal cell, no wallpaper one.
        cfg.cell_w = 10;
        cfg.cell_h = 15;
        cfg.cell_explicit = true;
        assert!(!cfg.wallpaper_cell_explicit);

        let waves = animation::build("waves", 10, 10, 1, &cfg);
        let cell = WallpaperSet::cell_for(waves.as_ref(), &cfg);
        assert_ne!(cell, (10, 15), "the terminal cell leaked into the wallpaper");
        assert_eq!(cell, (15, 23));

        // And spell out what it would have cost, so the number is not folklore.
        let (w, h) = (1440, 2560);
        let leaked = (w / 10) * (h / 15);
        let correct = (w / cell.0) * (h / cell.1);
        assert_eq!(leaked, 24_480);
        assert_eq!(correct, 10_656);
        assert!(correct * 2 < leaked, "the leak more than doubles the work");
    }

    #[test]
    fn an_effect_with_no_preference_falls_back_to_the_wallpaper_default() {
        // `flames` declares no preferred cell, so the wallpaper default must
        // apply — NOT the terminal's.
        let mut cfg = Config::default();
        cfg.cell_w = 10;
        cfg.cell_h = 15;
        cfg.cell_explicit = true;
        let flames = animation::build("flames", 10, 10, 1, &cfg);
        assert_eq!(flames.preferred_cell(), None, "precondition for this test");
        assert_eq!(
            WallpaperSet::cell_for(flames.as_ref(), &cfg),
            (cfg.wallpaper_cell_w, cfg.wallpaper_cell_h)
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

/// Does setting `key` change the wallpaper's character grid?
///
/// A `true` here must be followed by [`WallpaperSet::rebuild_surfaces`], or the
/// value is stored and read back correctly while the LIVE surfaces keep the grid
/// they were built with — the TUI then reports a change that never reaches the
/// screen. That was a real bug: `wp cell width` was editable, accepted, and
/// visibly did nothing.
///
/// A named function rather than an inline `matches!` so it is reachable from a
/// test. The equivalent check for the terminal panes lives at the `needs_query`
/// arm and is covered by the panel tests.
pub fn changes_the_grid(key: &str) -> bool {
    matches!(
        key,
        "wallpaper_detail" | "wallpaper_cell_w" | "wallpaper_cell_h"
    )
}

#[cfg(test)]
mod grid_change_tests {
    use super::*;

    #[test]
    fn every_key_that_resizes_the_grid_asks_for_a_rebuild() {
        // THE BUG: without this the value is stored, `cell_for` reads it back
        // correctly, and the surfaces keep their original grid -- so the number
        // in the TUI changes and the screen does not.
        for k in ["wallpaper_detail", "wallpaper_cell_w", "wallpaper_cell_h"] {
            assert!(changes_the_grid(k), "{k} resizes the grid but skips rebuild");
        }
    }

    #[test]
    fn keys_that_do_not_touch_the_grid_are_left_alone() {
        // Rebuilding tears down and recreates every surface. Doing that on an
        // unrelated key would flash the desktop on every keypress.
        for k in [
            "wallpaper_fps",
            "wallpaper_0_effect",
            "cell_w",
            "cell_h",
            "opacity",
            "fps",
            "font",
        ] {
            assert!(!changes_the_grid(k), "{k} must not force a rebuild");
        }
    }

    #[test]
    fn the_pane_cell_keys_are_not_mistaken_for_the_wallpapers() {
        // `cell_w` and `wallpaper_cell_w` differ only by a prefix; a sloppy
        // `contains` here would rebuild the desktop whenever the TERMINAL cell
        // was tuned.
        assert!(!changes_the_grid("cell_w"));
        assert!(changes_the_grid("wallpaper_cell_w"));
    }
}
