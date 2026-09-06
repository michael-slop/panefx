//! Runtime configuration: which effects run, how they rotate, and the font.
//!
//! Everything here is deliberately data, not code, so a new background is a
//! config entry plus one `AsciiAnimation` impl — no changes to the supervisor,
//! the panel, or the renderer.
//!
//! Sources, in increasing precedence:
//!   1. built-in defaults
//!   2. `%USERPROFILE%\.config\panefx\config.toml` (if present)
//!   3. environment variables (PANEFX_*)
//!
//! Env vars win so a value can be tried live without editing a file.

use std::time::Duration;

/// Font used to draw every effect. Must be a family name Windows can resolve.
///
/// Defaults to the author's Alacritty font so the animation shares the
/// terminal's cell grid. BigBlueTerm is a DOS/CP437 face with NO katakana —
/// which is why the rain effect uses CP437 glyphs rather than the usual
/// Matrix katakana. That is a deliberate constraint, not an oversight.
pub const DEFAULT_FONT: &str = "BigBlueTerm437 Nerd Font Mono";

/// Effects available to run, in rotation order.
pub const DEFAULT_ROTATION: &[&str] = &["flames"];

/// How long each effect runs before the next one in the rotation takes over.
/// Zero disables rotation (the first effect runs forever).
pub const DEFAULT_ROTATE_SECS: u64 = 0;

#[derive(Debug, Clone)]
pub struct Config {
    pub font: String,
    pub cell_w: i32,
    pub cell_h: i32,
    /// True when the user set the cell size explicitly (env var, config file,
    /// or the TUI). An effect's `preferred_cell()` only applies when this is
    /// false — asking for a size means it, and must not be silently overridden
    /// by whichever effect happens to be running.
    pub cell_explicit: bool,
    pub fps: u64,
    /// Effect names, in rotation order. Always non-empty.
    pub rotation: Vec<String>,
    /// `None` means "never rotate".
    pub rotate_every: Option<Duration>,
    /// Pixels chopped off the top of the animation.
    pub crop_top: i32,
    /// Must match `[window] padding` in alacritty.toml.
    pub pad_x: i32,
    pub pad_y: i32,
    /// Per-effect character set override, e.g. PANEFX_CHARS_RAIN.
    /// `None` means the effect uses its own built-in set.
    pub chars_override: Option<String>,

    /// How see-through panefx's target windows are, from
    /// [`crate::opacity::MIN_PERCENT`] to [`crate::opacity::MAX_PERCENT`].
    ///
    /// Applied by writing each app's OWN opacity setting — see
    /// [`crate::term_opacity`], which rewrites the `opacity` line in
    /// `alacritty.toml` and lets `live_config_reload` pick it up (about two
    /// seconds, no restart).
    ///
    /// This used to go through `SetLayeredWindowAttributes`, and the difference
    /// is the reason it does not any more: a layered alpha fades every pixel of
    /// the window, glyphs included, so the backdrop bled through the text and
    /// made it hard to read. Each app's own setting is PER-PIXEL — the
    /// background fades and the glyphs stay solid, which is the only version of
    /// this feature worth having. `opacity.rs` keeps the history.
    ///
    /// Because there is exactly one alpha now, the apps' own values are what
    /// panefx sets; nothing multiplies. `main.rs` calls
    /// `term_opacity::clear_legacy_layered_styles()` at startup so a window
    /// still carrying the old layered style does not stack a second alpha.
    pub opacity: u8,
    /// Per-effect params from `[rain]` / `[flames]` sections, kept as raw
    /// strings and applied through `AsciiAnimation::set_param` after the effect
    /// is built. Held generically so a new effect's knobs persist without
    /// touching `Config`.
    pub effect_params: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,

    /// The DESKTOP's own copy of the same knobs, keyed the same way.
    ///
    /// Separate from `effect_params` on purpose: `waves` behind a terminal and
    /// `waves` on the desktop are the same effect with entirely different jobs.
    /// The pane version is tuned to stay legible behind 60%-opaque text; the
    /// wallpaper has a whole screen and no text over it. Sharing one set would
    /// mean tuning either one wrecks the other.
    ///
    /// Shared across monitors — each screen picks its own EFFECT, but two
    /// screens running `waves` use the same wallpaper-waves values.
    ///
    /// An absent entry means "this effect has never been tuned for the
    /// desktop", and `apply_saved_params` then leaves the effect's own
    /// constructor defaults alone. That is deliberate: the desktop starts from
    /// the effect's defaults, NOT from a copy of the pane's values.
    pub wallpaper_effect_params:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,

    /// PER-MONITOR wallpaper params, keyed `monitor -> effect -> key -> value`.
    ///
    /// Overrides `wallpaper_effect_params` for one screen. Written as
    /// `[wallpaper.3.waves]` -- the monitor's `DISPLAY<n>` index, then the
    /// effect.
    ///
    /// Exists because the shared block above cannot express "the portrait
    /// screen wants a bigger cell and a different ink": every monitor running
    /// `waves` read the same values, so tuning one tuned all of them, and the
    /// only way to get two different looks was to run two different effects.
    ///
    /// A monitor with no entry falls back to the shared block, which is what
    /// keeps existing configs working and keeps "set it once for every screen"
    /// a single edit.
    pub wallpaper_monitor_params: std::collections::BTreeMap<
        usize,
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    >,

    // ---- desktop wallpaper -------------------------------------------------
    /// Turn the TERMINAL backdrops off entirely, keeping the wallpaper.
    ///
    /// The wallpaper has had an `off` per monitor since it shipped; the pane
    /// had no way to be switched off at all, so the only way to stop it was to
    /// kill the daemon -- which also took the wallpaper with it.
    ///
    /// A flag rather than an `"off"` entry in `EFFECTS`: that list decides
    /// which `[section]`s are read as per-effect params, and a fake member
    /// would make `[off]` a silent param sink.
    pub pane_off: bool,

    /// Frame rate for the desktop wallpaper, independent of `fps`.
    ///
    /// Lower by default: the wallpaper is glanced at rather than watched, and
    /// this path draws up to one full-monitor bitmap per screen against the
    /// terminal path's small ones.
    pub wallpaper_fps: u64,

    /// Freeze a monitor's wallpaper once windows cover this percentage of it.
    ///
    /// 100 means the old behaviour: freeze only when every pixel is covered.
    /// That sounds safe and is nearly useless -- under a tiling WM with ANY gap
    /// configured, tiles never reach the monitor edges, so it never fires and a
    /// full-monitor simulation runs all day behind a 1px border.
    ///
    /// The default leaves real headroom: a wallpaper still visibly showing
    /// through keeps animating, and one reduced to a sliver stops.
    pub wallpaper_freeze_at: u32,

    /// Where window geometry comes from: `"auto"`, `"glazewm"` or `"native"`.
    ///
    /// `auto` prefers GlazeWM and falls back to Win32, which is what makes the
    /// window manager optional instead of required. See `window_source.rs`.
    pub window_source: String,
    /// Effect per monitor, keyed by `DISPLAY<n>` index. An absent entry — or the
    /// literal `"off"` — means no surface at all for that monitor, so Windows'
    /// own wallpaper shows through.
    pub wallpaper_effects: std::collections::BTreeMap<usize, String>,

    /// Extra wallpaper layers, keyed `monitor -> layer index -> effect`.
    ///
    /// Layer 0 is `wallpaper_effects` above and is written as
    /// `wallpaper_<n>_effect`; layers 1+ live here as `wallpaper_<n>_layer<k>`.
    /// Splitting them that way is what keeps every config written before layers
    /// existed valid and unchanged -- a single-effect monitor is simply a stack
    /// of one, and nothing has to be migrated.
    ///
    /// Higher index = nearer the viewer. `cell_at` returning `None` already
    /// means "draw nothing here", so an upper layer's empty cells let the one
    /// below show through with no alpha and no second buffer -- see
    /// `render::draw_layers`.
    pub wallpaper_layers:
        std::collections::BTreeMap<usize, std::collections::BTreeMap<usize, String>>,
    /// Wallpaper cell size.
    ///
    /// SEPARATE from `cell_w`/`cell_h` on purpose. The terminal cell exists so
    /// glyphs line up with the terminal's text; a wallpaper has no text to line
    /// up with, and inheriting a small terminal cell quadruples the cell count
    /// for no visual gain. Measured: a 1440x2560 portrait at 10x15 is 24,480
    /// cells against 10,656 at 15x23 — and 12,012 cells was the panel that cost
    /// 88.7% of a core before the optimisation pass.
    pub wallpaper_cell_w: i32,
    pub wallpaper_cell_h: i32,
    /// Set when the wallpaper cell was chosen explicitly; otherwise an effect's
    /// `preferred_cell()` wins. Mirrors `cell_explicit`.
    pub wallpaper_cell_explicit: bool,
    /// How fine the wallpaper's character grid is, 1 (chunky) to 10 (fine).
    ///
    /// A friendlier front end for `wallpaper_cell_w`/`_h`: setting it WRITES
    /// those two from [`detail_to_cell`], so there is one source of truth rather
    /// than two settings that can disagree. The raw pair stays editable for
    /// anyone who wants a size off the ladder.
    pub wallpaper_detail: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: DEFAULT_FONT.to_string(),
            cell_w: 10,
            cell_h: 15,
            cell_explicit: false,
            // 10fps, not 20. This is a backdrop behind a 60%-opaque terminal —
            // the extra frames are close to invisible in use and the renderer
            // cost is per-frame per-panel, so halving this halves the whole
            // program's cost. `panefx-ctl` can raise it live if a particular
            // effect ever needs to be smoother.
            fps: 10,
            rotation: DEFAULT_ROTATION.iter().map(|s| s.to_string()).collect(),
            rotate_every: None,
            crop_top: 0,
            pad_x: 10,
            pad_y: 8,
            chars_override: None,
            // 60%: what alacritty.toml used to carry, so the look is unchanged
            // on first run after this became panefx's job.
            opacity: 60,
            effect_params: Default::default(),
            pane_off: false,
            wallpaper_effect_params: Default::default(),
            wallpaper_monitor_params: Default::default(),
            // Half the terminal rate. The desktop is scenery.
            wallpaper_fps: 5,
            // 92%: comfortably above a tiled screen's few-pixel gaps, and far
            // enough below a genuinely half-covered desktop that a visible
            // wallpaper never freezes.
            wallpaper_freeze_at: 92,
            // Auto: keep GlazeWM where it exists, work without it where it
            // does not. Nobody should have to set this to get either.
            window_source: "auto".to_string(),
            // EMPTY = wallpapers off. A new feature must not change what the
            // user already sees until they ask for it.
            wallpaper_effects: Default::default(),
            wallpaper_layers: Default::default(),
            // 15x23 matches `waves::preferred_cell()`, which was tuned live
            // against the real thing and is also 57% fewer cells than 10x15.
            wallpaper_cell_w: 15,
            wallpaper_cell_h: 23,
            wallpaper_cell_explicit: false,
            // 5 -> 16x25, within a pixel of the 15x23 this used to default to,
            // so the look does not change on first run after the knob appeared.
            wallpaper_detail: 5,
        }
    }
}

/// Lowest and highest detail rungs.
pub const DETAIL_MIN: u8 = 1;
pub const DETAIL_MAX: u8 = 10;

/// A detail level as a concrete cell size in pixels.
///
/// Scales BOTH axes together, holding the 15:23 shape that `waves` was tuned at
/// (`waves::preferred_cell`). Changing only the width would stretch every effect
/// as the slider moved.
///
/// The top of the ladder is the floor on purpose. At 6x9 the author's four
/// monitors total ~179,000 cells; one rung finer would be ~403,000, roughly 28x
/// the panel once measured at 88.7% of a core. The slider must not be able to
/// walk into that by accident — typing a raw `wallpaper_cell_w` still can.
pub fn detail_to_cell(detail: u8) -> (i32, i32) {
    let d = detail.clamp(DETAIL_MIN, DETAIL_MAX) as i32;
    let w = 24 - (d - 1) * 2;
    // 23/15, rounded: keeps every rung within a pixel of the house ratio.
    let h = (w as f32 * 23.0 / 15.0).round() as i32;
    (w, h)
}

/// The rung whose cell size is closest to `(w, h)`.
///
/// Used to show a sensible detail number when the raw cell size was set
/// directly. Nothing is hidden: the slider simply reports the nearest rung.
pub fn cell_to_detail(w: i32, h: i32) -> u8 {
    let _ = h;
    (DETAIL_MIN..=DETAIL_MAX)
        .min_by_key(|d| (detail_to_cell(*d).0 - w).abs())
        .unwrap_or(5)
}

/// `wallpaper_3_layer2` -> `Some((3, 2))`. Anything else -> `None`.
///
/// Layer 0 is deliberately NOT accepted here: the base effect has its own key
/// (`wallpaper_3_effect`), and allowing both spellings would let one config
/// disagree with itself about the bottom of the stack.
pub fn parse_wallpaper_layer_key(k: &str) -> Option<(usize, usize)> {
    let rest = k.strip_prefix("wallpaper_")?;
    let (num, tail) = rest.split_once('_')?;
    let idx: usize = num.parse().ok()?;
    let layer: usize = tail.strip_prefix("layer")?.parse().ok()?;
    if idx == 0 || layer == 0 {
        return None;
    }
    Some((idx, layer))
}

/// `wallpaper_3_effect` -> `Some(3)`. Anything else -> `None`.
///
/// Strict on purpose: `wallpaper_fps` shares the prefix, and an over-eager match
/// would swallow it into the per-monitor map where nothing would ever read it.
pub fn parse_wallpaper_effect_key(k: &str) -> Option<usize> {
    let rest = k.strip_prefix("wallpaper_")?.strip_suffix("_effect")?;
    rest.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= 64)
}

/// `wallpaper.waves` -> `Some("waves")`. Anything else -> `None`.
///
/// Requiring the suffix to be a REAL effect name is the load-bearing part. The
/// TOML parser treats any unrecognised `[section]` as decorative and lets its
/// keys fall through to the top-level fields, so a loose match here would let a
/// `[wallpaper.anything]` block quietly set `fps`, `cell_w` or `chars`.
///
/// A bare `[wallpaper]` deliberately does NOT match: that spelling is already
/// decorative, and `wallpaper_keys_must_stay_flat_not_a_section` depends on it
/// staying that way.
pub fn parse_wallpaper_section(sec: &str) -> Option<String> {
    let eff = sec.strip_prefix("wallpaper.")?;
    crate::animation::EFFECTS
        .contains(&eff)
        .then(|| eff.to_string())
}

/// `[wallpaper.3.waves]` -> `Some((3, "waves"))`. Anything else -> `None`.
///
/// Deliberately as strict as [`parse_wallpaper_section`]: both halves must be
/// valid -- a real `DISPLAY<n>` number and a REAL effect name -- or the section
/// stays decorative and its keys go on setting the top-level fields, which is
/// the old behaviour. A typo must not become a silent param sink that nothing
/// ever reads.
pub fn parse_wallpaper_monitor_section(sec: &str) -> Option<(usize, String)> {
    let rest = sec.strip_prefix("wallpaper.")?;
    let (num, eff) = rest.split_once('.')?;
    let idx: usize = num.parse().ok()?;
    // Monitor 0 does not exist -- `DISPLAY<n>` is 1-based.
    if idx == 0 {
        return None;
    }
    crate::animation::EFFECTS
        .contains(&eff)
        .then(|| (idx, eff.to_string()))
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_i32(key: &str) -> Option<i32> {
    env_str(key).and_then(|v| v.parse().ok())
}

fn env_u64(key: &str) -> Option<u64> {
    env_str(key).and_then(|v| v.parse().ok())
}

impl Config {
    /// The opacity that should actually be written to the terminal right now.
    ///
    /// With the backdrops off there is nothing behind the terminal to see, so a
    /// see-through window just shows the plain desktop through the text. That
    /// is not what "turn the backdrop off" means -- it means give me an
    /// ordinary, solid terminal back.
    ///
    /// This is deliberately a DERIVED value rather than a write to `opacity`.
    /// Clobbering the stored number would silently lose the setting: switch the
    /// backdrops off, switch them back on, and the carefully chosen 80% would
    /// have become 100% with no way to know what it used to be.
    pub fn effective_opacity(&self) -> u8 {
        if self.pane_off {
            100
        } else {
            self.opacity
        }
    }

    /// Path of the optional TOML config.
    pub fn path() -> Option<std::path::PathBuf> {
        std::env::var("USERPROFILE")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".config\\panefx\\config.toml"))
    }

    pub fn load() -> Self {
        let mut cfg = Config::default();

        // --- file layer ---
        if let Some(p) = Config::path() {
            if let Ok(text) = std::fs::read_to_string(&p) {
                cfg.apply_toml(&text);
            }
        }

        // --- env layer (wins) ---
        if let Some(v) = env_str("PANEFX_FONT") {
            cfg.font = v;
        }
        if let Some(v) = env_i32("PANEFX_CELL_W").filter(|v| *v > 0) {
            cfg.cell_w = v;
            cfg.cell_explicit = true;
        }
        if let Some(v) = env_i32("PANEFX_CELL_H").filter(|v| *v > 0) {
            cfg.cell_h = v;
            cfg.cell_explicit = true;
        }
        // Clamped for the same reason as the TOML path above: the env var is a
        // preference, and the nearest legal value honours it better than a
        // silent fallback to the default.
        if let Some(v) = env_i32("PANEFX_OPACITY") {
            let lo = crate::opacity::MIN_PERCENT as i32;
            let hi = crate::opacity::MAX_PERCENT as i32;
            let clamped = v.clamp(lo, hi);
            cfg.opacity = clamped as u8;
            if v != clamped {
                crate::log_warn!(
                    "[panefx] PANEFX_OPACITY {v}% is outside {lo}-{hi}%; using {clamped}%"
                );
            }
        }
        if let Some(v) = env_u64("PANEFX_WALLPAPER_FPS").filter(|v| *v > 0 && *v <= 120) {
            cfg.wallpaper_fps = v;
        }
        if let Some(v) = env_u64("PANEFX_WALLPAPER_FREEZE_AT").filter(|v| *v >= 1 && *v <= 100) {
            cfg.wallpaper_freeze_at = v as u32;
        }
        if let Ok(v) = std::env::var("PANEFX_WINDOW_SOURCE") {
            if crate::window_source::Preference::parse(&v).is_some() {
                cfg.window_source = v.trim().to_lowercase();
            } else {
                crate::log_warn!("[panefx] PANEFX_WINDOW_SOURCE '{v}' is not auto/glazewm/native; ignoring");
            }
        }
        if let Some(v) = env_i32("PANEFX_WALLPAPER_CELL_W").filter(|v| *v > 0) {
            cfg.wallpaper_cell_w = v;
            cfg.wallpaper_cell_explicit = true;
        }
        if let Some(v) = env_i32("PANEFX_WALLPAPER_CELL_H").filter(|v| *v > 0) {
            cfg.wallpaper_cell_h = v;
            cfg.wallpaper_cell_explicit = true;
        }
        if let Some(v) = env_u64("PANEFX_FPS").filter(|v| *v > 0 && *v <= 120) {
            cfg.fps = v;
        }
        if let Some(v) = env_i32("PANEFX_CROP_TOP").filter(|v| *v >= 0) {
            cfg.crop_top = v;
        }
        if let Some(v) = env_i32("PANEFX_PAD_X").filter(|v| *v >= 0) {
            cfg.pad_x = v;
        }
        if let Some(v) = env_i32("PANEFX_PAD_Y").filter(|v| *v >= 0) {
            cfg.pad_y = v;
        }
        if let Some(v) = env_str("PANEFX_CHARS") {
            cfg.chars_override = Some(v);
        }
        // PANEFX_EFFECT names ONE effect; PANEFX_ROTATION is a
        // comma-separated list. The single form is kept because it is what the
        // earlier builds used.
        if let Some(v) = env_str("PANEFX_ROTATION") {
            let list = Config::parse_list(&v);
            if !list.is_empty() {
                cfg.rotation = list;
            }
        } else if let Some(v) = env_str("PANEFX_EFFECT") {
            cfg.rotation = vec![v];
        }
        if let Some(v) = env_u64("PANEFX_ROTATE_SECS") {
            cfg.rotate_every = (v > 0).then(|| Duration::from_secs(v));
        }

        cfg.normalise();
        cfg
    }

    fn parse_list(v: &str) -> Vec<String> {
        v.split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Deliberately hand-rolled rather than pulling in a TOML crate: this is a
    /// dozen flat `key = value` lines, and the dependency is not worth it.
    /// Unknown keys are ignored so an old binary tolerates a newer file.
    fn apply_toml(&mut self, text: &str) {
        // Section tracking: `[rain]` / `[flames]` hold per-effect params, which
        // are stashed verbatim and handed to the effect when it is built. Only
        // the top level (no section) sets Config fields.
        let mut section: Option<String> = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                section = line
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_lowercase()
                    .into();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim().to_lowercase();
            let v = v.trim().trim_matches('"').trim_matches('\'');

            // `[wallpaper.<effect>]` -> the DESKTOP's copy of that effect's
            // params.
            //
            // MUST come before the EFFECTS check below. Without it,
            // `wallpaper.waves` matches neither branch, falls through to the
            // decorative-section path, and every key under it lands in the
            // top-level match -- where `chars` would be hijacked by waves' ramp
            // and `fps` by whatever integer happened to be there.
            if let Some(sec) = section.as_deref() {
                // `[wallpaper.<n>.<effect>]` -> ONE MONITOR's override. Checked
                // before the shared form because `wallpaper.3.waves` also
                // starts with `wallpaper.`.
                if let Some((mon, eff)) = parse_wallpaper_monitor_section(sec) {
                    self.wallpaper_monitor_params
                        .entry(mon)
                        .or_default()
                        .entry(eff)
                        .or_default()
                        .insert(k, v.to_string());
                    continue;
                }
                if let Some(eff) = parse_wallpaper_section(sec) {
                    self.wallpaper_effect_params
                        .entry(eff)
                        .or_default()
                        .insert(k, v.to_string());
                    continue;
                }
            }

            // Only sections NAMED FOR AN EFFECT hold per-effect params.
            //
            // Any other section header (`[display]`, `[window]`, …) is treated
            // as decorative grouping and its keys still set Config fields.
            // Without this, adding a purely cosmetic header to an existing
            // config would silently stop every setting under it from applying.
            if let Some(sec) = section.as_deref() {
                if crate::animation::EFFECTS.contains(&sec) {
                    self.effect_params
                        .entry(sec.to_string())
                        .or_default()
                        .insert(k, v.to_string());
                    continue;
                }
            }

            // Per-monitor wallpaper effect: `wallpaper_<n>_effect`.
            //
            // FLAT KEYS, never a `[wallpaper]` section. The check above treats
            // any section not named after an effect as decorative and lets its
            // keys fall through to the top-level match — so `[wallpaper]` with
            // `fps = 5` under it would set the TERMINAL fps. Flat keys sidestep
            // that entirely without touching the parser.
            //
            // Placed AFTER the effect-section diversion (so a stray key inside
            // `[waves]` stays a param) and BEFORE the top-level match.
            if let Some(idx) = parse_wallpaper_effect_key(&k) {
                self.wallpaper_effects.insert(idx, v.to_lowercase());
                continue;
            }
            // `wallpaper_<n>_layer<k>` -- an extra layer above the base effect.
            if let Some((idx, layer)) = parse_wallpaper_layer_key(&k) {
                self.wallpaper_layers
                    .entry(idx)
                    .or_default()
                    .insert(layer, v.to_lowercase());
                continue;
            }

            match k.as_str() {
                "font" => self.font = v.to_string(),
                // Validated on the way in. A typo must not silently become
                // `auto` -- someone who wrote `natve` wants to know.
                "window_source" => match crate::window_source::Preference::parse(v) {
                    Some(p) => self.window_source = p.as_str().to_string(),
                    None => crate::log_warn!(
                        "[panefx] window_source '{v}' is not auto/glazewm/native; keeping '{}'",
                        self.window_source
                    ),
                },
                // CLAMPED, not rejected. Every other key here drops an
                // out-of-range value and keeps the code default, which is right
                // when the value is nonsense. Opacity is different: the floor
                // has moved up before (see `opacity::MIN_PERCENT`), so a config
                // written by an older build holds a value that is now too low
                // but still expresses a real preference -- "as see-through as
                // you will let me". Dropping it would silently jump the user to
                // the default instead of the nearest legal setting, and the only
                // evidence would be a terminal that changed appearance at
                // startup for no stated reason.
                "opacity" => {
                    if let Ok(n) = v.parse::<u8>() {
                        let lo = crate::opacity::MIN_PERCENT;
                        let hi = crate::opacity::MAX_PERCENT;
                        self.opacity = n.clamp(lo, hi);
                        if n != self.opacity {
                            crate::log_warn!(
                                "[panefx] config opacity {n}% is outside {lo}-{hi}%; \
                                 using {}%",
                                self.opacity
                            );
                        }
                    }
                }
                "pane_off" => self.pane_off = v.eq_ignore_ascii_case("true"),
                "wallpaper_fps" => {
                    if let Ok(n) = v.parse::<u64>() {
                        if n > 0 && n <= 120 {
                            self.wallpaper_fps = n;
                        }
                    }
                }
                "wallpaper_freeze_at" => {
                    if let Ok(n) = v.parse::<u32>() {
                        if (1..=100).contains(&n) {
                            self.wallpaper_freeze_at = n;
                        }
                    }
                }
                "wallpaper_detail" => {
                    if let Ok(n) = v.parse::<u8>() {
                        if (DETAIL_MIN..=DETAIL_MAX).contains(&n) {
                            let (w, h) = detail_to_cell(n);
                            self.wallpaper_detail = n;
                            self.wallpaper_cell_w = w;
                            self.wallpaper_cell_h = h;
                            self.wallpaper_cell_explicit = true;
                        }
                    }
                }
                "wallpaper_cell_w" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n > 0 {
                            self.wallpaper_cell_w = n;
                            self.wallpaper_cell_explicit = true;
                        }
                    }
                }
                "wallpaper_cell_h" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n > 0 {
                            self.wallpaper_cell_h = n;
                            self.wallpaper_cell_explicit = true;
                        }
                    }
                }
                "cell_w" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n > 0 {
                            self.cell_w = n;
                            self.cell_explicit = true;
                        }
                    }
                }
                "cell_h" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n > 0 {
                            self.cell_h = n;
                            self.cell_explicit = true;
                        }
                    }
                }
                "fps" => {
                    if let Ok(n) = v.parse::<u64>() {
                        if n > 0 && n <= 120 {
                            self.fps = n;
                        }
                    }
                }
                "crop_top" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n >= 0 {
                            self.crop_top = n;
                        }
                    }
                }
                "pad_x" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n >= 0 {
                            self.pad_x = n;
                        }
                    }
                }
                "pad_y" => {
                    if let Ok(n) = v.parse::<i32>() {
                        if n >= 0 {
                            self.pad_y = n;
                        }
                    }
                }
                "chars" => self.chars_override = Some(v.to_string()),
                "rotation" => {
                    let list = Config::parse_list(v);
                    if !list.is_empty() {
                        self.rotation = list;
                    }
                }
                "rotate_secs" => {
                    if let Ok(n) = v.parse::<u64>() {
                        self.rotate_every = (n > 0).then(|| Duration::from_secs(n));
                    }
                }
                _ => {}
            }
        }
    }

    /// An empty rotation would make the supervisor draw nothing at all, which
    /// looks exactly like a crashed daemon. Never allow it.
    fn normalise(&mut self) {
        self.rotation.retain(|s| !s.trim().is_empty());
        if self.rotation.is_empty() {
            self.rotation = DEFAULT_ROTATION.iter().map(|s| s.to_string()).collect();
        }
    }

    pub fn frame_time(&self) -> Duration {
        Duration::from_millis(1000 / self.fps.max(1))
    }

    /// Render the whole config back to TOML, per-effect sections included.
    ///
    /// Comments are REGENERATED, not preserved. This file is rewritten wholesale
    /// every time the TUI saves, so any hand-written note would be lost on the
    /// next `s` keypress — which is worse than not having one, because it looks
    /// like it persisted until it silently does not. Instead the explanations
    /// live here, in code, and are re-emitted every save.
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str("# panefx config — rewritten in full whenever panefx-ctl saves.\n");
        s.push_str("#\n");
        s.push_str("# Precedence, lowest to highest:\n");
        s.push_str("#   1. code defaults   2. THIS FILE   3. PANEFX_* env vars\n");
        s.push_str("#\n");
        s.push_str("# Comments here are generated from `Config::to_toml`. Editing them by\n");
        s.push_str("# hand works until the next save, which overwrites the whole file.\n\n");

        s.push_str(&format!("font = \"{}\"\n", self.font));
        s.push_str("\n# Where window geometry comes from: auto | glazewm | native.\n");
        s.push_str("# auto prefers GlazeWM and falls back to Win32, which is what makes\n");
        s.push_str("# the window manager optional rather than required.\n");
        s.push_str(&format!("window_source = \"{}\"\n", self.window_source));

        s.push_str("\n# Backdrop behind a 60%-opaque terminal: extra frames are close to\n");
        s.push_str("# invisible, and renderer cost is per-frame per-panel.\n");
        s.push_str(&format!("fps = {}\n", self.fps));

        s.push_str("\n# The terminal's own text cell. Effects that want a different size ask\n");
        s.push_str("# for one themselves (waves uses 15x23); setting these HERE overrides\n");
        s.push_str("# that and forces every effect onto this grid.\n");
        s.push_str(&format!("cell_w = {}\n", self.cell_w));
        s.push_str(&format!("cell_h = {}\n", self.cell_h));

        s.push_str("\n# Must match `[window] padding` in alacritty.toml, or the animation\n");
        s.push_str("# does not line up with the terminal's text area.\n");
        s.push_str(&format!("pad_x = {}\n", self.pad_x));
        s.push_str(&format!("pad_y = {}\n", self.pad_y));

        s.push_str("\n# Pixels chopped off the TOP of the animation. 0 draws the full panel.\n");
        s.push_str(&format!("crop_top = {}\n", self.crop_top));

        s.push_str("\n# flames | rain | waves | fire. A comma-separated list plus a non-zero\n");
        s.push_str("# rotate_secs cycles between them.\n");
        s.push_str(&format!("rotation = \"{}\"\n", self.rotation.join(", ")));
        s.push_str(&format!(
            "rotate_secs = {}\n",
            self.rotate_every.map(|d| d.as_secs()).unwrap_or(0)
        ));
        if let Some(c) = &self.chars_override {
            s.push_str(&format!("chars = \"{c}\"\n"));
        }

        s.push_str(&format!(
            "\n# How see-through the target windows are, {}-{}%.\n",
            crate::opacity::MIN_PERCENT,
            crate::opacity::MAX_PERCENT
        ));
        s.push_str("# panefx applies this by writing the app's OWN opacity setting --\n");
        s.push_str("# for Alacritty, the 'opacity' line in alacritty.toml, which\n");
        s.push_str("# live_config_reload picks up in about two seconds. Do not set that\n");
        s.push_str("# line by hand; the value HERE is the one that wins.\n");
        s.push_str("#\n");
        s.push_str("# It is per-pixel: the background fades and the glyphs stay solid.\n");
        s.push_str("# A whole-window alpha was tried first and faded the text too.\n");
        s.push_str("#\n");
        s.push_str(&format!(
            "# The floor is {}%, not 0. Below roughly a third, a borderless window\n",
            crate::opacity::MIN_PERCENT
        ));
        s.push_str("# over an animated backdrop makes the compositor redraw the whole\n");
        s.push_str("# window on every damage rect, and heavy terminal output -- a long\n");
        s.push_str("# table, a build log -- tears and flashes the display.\n");
        s.push_str(&format!("opacity = {}\n", self.opacity));
        s.push_str(&format!("pane_off = {}
", self.pane_off));

        s.push_str("\n# --- desktop wallpaper ----------------------------------------------\n");
        s.push_str("# Drawn into Explorer's WorkerW layer, BEHIND the desktop icons.\n");
        s.push_str("# Each monitor picks its own effect by its DISPLAY<n> number;\n");
        s.push_str("# \"off\" (or no key at all) means that monitor keeps the Windows\n");
        s.push_str("# wallpaper and costs nothing.\n");
        s.push_str("#\n");
        s.push_str("# FLAT KEYS, not a [wallpaper] section: any section not named after\n");
        s.push_str("# an effect is decorative and its keys still set the fields above, so\n");
        s.push_str("# a [wallpaper] header would make `fps` in it overwrite the terminal\n");
        s.push_str("# frame rate.\n");
        s.push_str(&format!("wallpaper_fps = {}\n", self.wallpaper_fps));
        s.push_str("\n# Freeze a monitor's wallpaper once windows cover this much of it,\n");
        s.push_str("# as a percentage. 100 means only when every pixel is covered, which\n");
        s.push_str("# under a tiling WM with any gap configured means never.\n");
        s.push_str(&format!(
            "wallpaper_freeze_at = {}\n",
            self.wallpaper_freeze_at
        ));
        s.push_str("\n# A wallpaper has no terminal text to line up with, so it does NOT\n");
        s.push_str("# use cell_w/cell_h above. 15x23 keeps a 1440x2560 portrait at ~10k\n");
        s.push_str("# cells instead of ~24k.\n");
        s.push_str(&format!("wallpaper_detail = {}\n", self.wallpaper_detail));
        s.push_str(&format!("wallpaper_cell_w = {}\n", self.wallpaper_cell_w));
        s.push_str(&format!("wallpaper_cell_h = {}\n", self.wallpaper_cell_h));
        for (idx, eff) in &self.wallpaper_effects {
            s.push_str(&format!("wallpaper_{idx}_effect = \"{eff}\"\n"));
        }
        // Extra layers, after the base effects so the file reads bottom-up the
        // way the stack composites.
        for (idx, layers) in &self.wallpaper_layers {
            for (layer, eff) in layers {
                s.push_str(&format!("wallpaper_{idx}_layer{layer} = \"{eff}\"\n"));
            }
        }

        if !self.effect_params.is_empty() {
            s.push_str("\n# --- per-effect parameters ------------------------------------------\n");
            s.push_str("# Only sections named after an effect are read as parameters; any\n");
            s.push_str("# other [section] header is decorative and its keys still set the\n");
            s.push_str("# top-level fields above.\n");
        }
        for (effect, params) in &self.effect_params {
            if params.is_empty() {
                continue;
            }
            s.push_str(&format!("\n[{effect}]\n"));
            // A one-line reminder of what is non-obvious about each effect.
            // These are the things that look like bugs if you do not know them.
            match effect.as_str() {
                "waves" => {
                    s.push_str("# darkcut: cells below this luminance are NOT DRAWN. Without it\n");
                    s.push_str("# every cell is a glyph (the remap floor lands on ramp index 2)\n");
                    s.push_str("# and the grid is 100% lit. Higher headroom = darker.\n");
                }
                "rain" => {
                    s.push_str("# tick_ms 55 is the original's deliberate stepped look — \"a CRT\n");
                    s.push_str("# reads better stepped than smooth\". Do not smooth it out.\n");
                }
                "flames" => {
                    s.push_str("# Deliberately a band along the bottom, not a full-height fire.\n");
                    s.push_str("# See `stays_a_bottom_band_on_a_tall_panel` in flames.rs.\n");
                }
                _ => {}
            }
            for (k, v) in params {
                // Numbers unquoted, everything else quoted — the parser strips
                // quotes either way, but this keeps the file readable.
                if v.parse::<i64>().is_ok() {
                    s.push_str(&format!("{k} = {v}\n"));
                } else {
                    s.push_str(&format!("{k} = \"{v}\"\n"));
                }
            }
        }
        // The DESKTOP's copy of the same knobs.
        //
        // `[wallpaper.waves]` is read as params only because `waves` is a real
        // effect name -- see `parse_wallpaper_section`. A bare `[wallpaper]`
        // header is still decorative and its keys still set the fields above.
        if self.wallpaper_effect_params.values().any(|m| !m.is_empty()) {
            s.push_str("
# --- wallpaper effect parameters -----------------------------------
");
            s.push_str("# The desktop's OWN copy of each effect's knobs, independent of the
");
            s.push_str("# same effect running behind a terminal. Shared across monitors.
");
        }
        for (effect, params) in &self.wallpaper_effect_params {
            if params.is_empty() {
                continue;
            }
            s.push_str(&format!("
[wallpaper.{effect}]
"));
            for (k, v) in params {
                if v.parse::<i64>().is_ok() {
                    s.push_str(&format!("{k} = {v}
"));
                } else {
                    s.push_str(&format!("{k} = \"{v}\"
"));
                }
            }
        }

        // Per-monitor overrides, written AFTER the shared blocks so the file
        // reads the way the values are resolved: shared first, then what one
        // screen does differently.
        if self
            .wallpaper_monitor_params
            .values()
            .any(|e| e.values().any(|m| !m.is_empty()))
        {
            s.push_str("
# --- per-monitor overrides ------------------------------------------
");
            s.push_str("# Laid over the shared blocks above for ONE screen, by DISPLAY number.
");
        }
        for (mon, effects) in &self.wallpaper_monitor_params {
            for (effect, params) in effects {
                if params.is_empty() {
                    continue;
                }
                s.push_str(&format!("
[wallpaper.{mon}.{effect}]
"));
                for (k, v) in params {
                    if v.parse::<i64>().is_ok() {
                        s.push_str(&format!("{k} = {v}
"));
                    } else {
                        s.push_str(&format!("{k} = \"{v}\"
"));
                    }
                }
            }
        }

        s
    }

    /// Write to `Config::path()`, creating the directory if needed.
    pub fn save(&self) -> anyhow::Result<std::path::PathBuf> {
        let p = Config::path().ok_or_else(|| anyhow::anyhow!("no USERPROFILE"))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, self.to_toml())?;
        Ok(p)
    }

    /// Record a per-effect param so it survives a save. Kept as a string so
    /// `Config` never needs to know an effect's value types.
    pub fn set_effect_param(&mut self, effect: &str, key: &str, value: String) {
        self.effect_params
            .entry(effect.to_lowercase())
            .or_default()
            .insert(key.to_string(), value);
    }

    /// Record a wallpaper param so it survives a save.
    ///
    /// Deliberately a separate map from `set_effect_param`: tuning the desktop
    /// must not move the terminal backdrop, which is the whole point.
    /// Every layer for one monitor, bottom first.
    ///
    /// The base effect, then any extra layers in index order. `off` layers are
    /// dropped rather than kept as holes: an off layer contributes nothing and
    /// carrying it would make the stack's length lie about what is drawn.
    pub fn wallpaper_stack(&self, monitor: usize) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(base) = self.wallpaper_effects.get(&monitor) {
            if base != "off" {
                out.push(base.clone());
            }
        }
        if let Some(extra) = self.wallpaper_layers.get(&monitor) {
            out.extend(extra.values().filter(|e| *e != "off").cloned());
        }
        out
    }

    /// Point one layer of one monitor at an effect.
    ///
    /// Layer 0 writes the base effect, so the two storages cannot disagree
    /// about what the bottom of the stack is.
    pub fn set_wallpaper_layer(&mut self, monitor: usize, layer: usize, effect: &str) {
        let e = effect.trim().to_lowercase();
        if layer == 0 {
            self.wallpaper_effects.insert(monitor, e);
            return;
        }
        if e == "off" {
            // Removed, not stored as "off": a stack with holes in it is a
            // different thing to reason about, and nothing needs one.
            if let Some(m) = self.wallpaper_layers.get_mut(&monitor) {
                m.remove(&layer);
                if m.is_empty() {
                    self.wallpaper_layers.remove(&monitor);
                }
            }
            return;
        }
        self.wallpaper_layers
            .entry(monitor)
            .or_default()
            .insert(layer, e);
    }

    pub fn set_wallpaper_effect_param(&mut self, effect: &str, key: &str, value: String) {
        self.wallpaper_effect_params
            .entry(effect.to_lowercase())
            .or_default()
            .insert(key.to_string(), value);
    }

    /// Set one wallpaper param for ONE monitor, overriding the shared block.
    pub fn set_wallpaper_monitor_param(
        &mut self,
        monitor: usize,
        effect: &str,
        key: &str,
        value: String,
    ) {
        self.wallpaper_monitor_params
            .entry(monitor)
            .or_default()
            .entry(effect.to_lowercase())
            .or_default()
            .insert(key.to_string(), value);
    }

    /// The params for `effect` on `monitor`: the shared block, with any
    /// per-monitor entries laid over the top.
    ///
    /// Merged rather than either-or, so tuning one knob on one screen does not
    /// silently discard every shared value for that effect.
    pub fn wallpaper_params_for(
        &self,
        monitor: usize,
        effect: &str,
    ) -> std::collections::BTreeMap<String, String> {
        let effect = effect.to_lowercase();
        let mut out = self
            .wallpaper_effect_params
            .get(&effect)
            .cloned()
            .unwrap_or_default();
        if let Some(over) = self
            .wallpaper_monitor_params
            .get(&monitor)
            .and_then(|m| m.get(&effect))
        {
            for (k, v) in over {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    /// Set one top-level field from a JSON value, for the control channel.
    /// Returns whether the key was recognised AND the value was acceptable.
    pub fn set_field(&mut self, key: &str, v: &serde_json::Value) -> bool {
        let as_i64 = || v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let as_str = || v.as_str().map(|s| s.to_string());
        match key {
            "window_source" => match as_str() {
                Some(s2) => match crate::window_source::Preference::parse(&s2) {
                    Some(p) => {
                        self.window_source = p.as_str().to_string();
                        true
                    }
                    None => false,
                },
                _ => false,
            },
            "font" => match as_str() {
                Some(s) if !s.trim().is_empty() => {
                    self.font = s;
                    true
                }
                _ => false,
            },
            "cell_w" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.cell_w = as_i64().unwrap() as i32;
                self.cell_explicit = true;
            }).is_some(),
            "cell_h" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.cell_h = as_i64().unwrap() as i32;
                self.cell_explicit = true;
            }).is_some(),
            "fps" => matches!(as_i64(), Some(n) if n > 0 && n <= 120).then(|| {
                self.fps = as_i64().unwrap() as u64;
            }).is_some(),
            "crop_top" => matches!(as_i64(), Some(n) if n >= 0 && n <= 4000).then(|| {
                self.crop_top = as_i64().unwrap() as i32;
            }).is_some(),
            "pad_x" => matches!(as_i64(), Some(n) if n >= 0 && n <= 200).then(|| {
                self.pad_x = as_i64().unwrap() as i32;
            }).is_some(),
            "pad_y" => matches!(as_i64(), Some(n) if n >= 0 && n <= 200).then(|| {
                self.pad_y = as_i64().unwrap() as i32;
            }).is_some(),
            "rotate_secs" => match as_i64() {
                Some(n) if n >= 0 => {
                    self.rotate_every = (n > 0).then(|| Duration::from_secs(n as u64));
                    true
                }
                _ => false,
            },
            "rotation" => match as_str() {
                Some(s) => {
                    let list = Config::parse_list(&s);
                    if list.is_empty() {
                        false
                    } else {
                        self.rotation = list;
                        true
                    }
                }
                _ => false,
            },
            // Clamped, and SAID OUT LOUD when it is.
            //
            // This used to reject an out-of-range value silently: the setting
            // did not change and nothing anywhere explained why. Dragging the
            // slider below the floor moved it and did nothing, which reads as
            // "opacity is locked". The floor is real (see `opacity::MIN_PERCENT`
            // -- below it DWM re-composites continuously and the display
            // tears), so the answer is to honour the intent at the nearest
            // legal value and say so, not to ignore the request.
            "opacity" => match as_i64() {
                Some(n) => {
                    let lo = crate::opacity::MIN_PERCENT as i64;
                    let hi = crate::opacity::MAX_PERCENT as i64;
                    let clamped = n.clamp(lo, hi);
                    if clamped != n {
                        crate::log_warn!(
                            "[panefx] opacity {n}% is outside {lo}-{hi}%; using {clamped}% -- below the floor the display tears"
                        );
                    }
                    self.opacity = clamped as u8;
                    true
                }
                None => false,
            },
            // Accepts a bool, "true"/"false", or 0/1 -- the TUI sends it as an
            // int because it is rendered as an ordinary numeric row.
            "pane_off" => match v
                .as_bool()
                .or_else(|| as_i64().map(|n| n != 0))
                .or_else(|| as_str().map(|s| s.eq_ignore_ascii_case("true")))
            {
                Some(b) => {
                    self.pane_off = b;
                    true
                }
                None => false,
            },
            "wallpaper_fps" => matches!(as_i64(), Some(n) if n > 0 && n <= 120).then(|| {
                self.wallpaper_fps = as_i64().unwrap() as u64;
            }).is_some(),
            "wallpaper_freeze_at" => matches!(as_i64(), Some(n) if (1..=100).contains(&n)).then(|| {
                self.wallpaper_freeze_at = as_i64().unwrap() as u32;
            }).is_some(),
            // The friendly knob. Writes `wallpaper_cell_w/h` from the ladder
            // rather than keeping a second, competing notion of size -- the raw
            // rows below stay editable and remain the one source of truth.
            "wallpaper_detail" => matches!(
                as_i64(),
                Some(n) if n >= DETAIL_MIN as i64 && n <= DETAIL_MAX as i64
            )
            .then(|| {
                let d = as_i64().unwrap() as u8;
                let (w, h) = detail_to_cell(d);
                self.wallpaper_detail = d;
                self.wallpaper_cell_w = w;
                self.wallpaper_cell_h = h;
                self.wallpaper_cell_explicit = true;
            })
            .is_some(),
            "wallpaper_cell_w" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.wallpaper_cell_w = as_i64().unwrap() as i32;
                self.wallpaper_cell_explicit = true;
                // Keep the friendly knob on the nearest rung, so the two rows
                // never disagree about what is actually on screen.
                self.wallpaper_detail = cell_to_detail(self.wallpaper_cell_w, self.wallpaper_cell_h);
            }).is_some(),
            "wallpaper_cell_h" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.wallpaper_cell_h = as_i64().unwrap() as i32;
                self.wallpaper_cell_explicit = true;
                self.wallpaper_detail = cell_to_detail(self.wallpaper_cell_w, self.wallpaper_cell_h);
            }).is_some(),
            // `wallpaper_<n>_effect`. Guard arm, so it cannot shadow the literal
            // keys above (notably `wallpaper_fps`, which shares the prefix).
            k if parse_wallpaper_effect_key(k).is_some() => match as_str() {
                Some(s) => {
                    let s = s.trim().to_lowercase();
                    // "off" is legal and is deliberately NOT in EFFECTS — that
                    // list drives the TOML param-section rule, and a section
                    // named [off] would be meaningless.
                    if s != "off" && !crate::animation::EFFECTS.contains(&s.as_str()) {
                        return false;
                    }
                    self.wallpaper_effects
                        .insert(parse_wallpaper_effect_key(k).unwrap(), s);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    /// An out-of-range opacity must be CLAMPED and applied, never dropped.
    ///
    /// The bug: `set` rejected anything below the floor and returned false, so
    /// the value silently stayed where it was. Every UI offered 10..100 while
    /// the daemon accepted 35..100, so dragging the slider below 35 moved it
    /// and changed nothing -- which reads as "opacity is locked at 35%".
    #[test]
    fn a_too_low_opacity_is_clamped_to_the_floor_not_ignored() {
        let mut c = Config::default();
        assert!(
            c.set_field("opacity", &serde_json::json!(10)),
            "a too-low opacity should be accepted and clamped, not refused"
        );
        assert_eq!(c.opacity, crate::opacity::MIN_PERCENT);
    }

    #[test]
    fn a_too_high_opacity_is_clamped_to_the_ceiling() {
        let mut c = Config::default();
        assert!(c.set_field("opacity", &serde_json::json!(250)));
        assert_eq!(c.opacity, crate::opacity::MAX_PERCENT);
    }

    #[test]
    fn an_in_range_opacity_is_applied_exactly() {
        let mut c = Config::default();
        assert!(c.set_field("opacity", &serde_json::json!(70)));
        assert_eq!(c.opacity, 70);
    }

    use super::*;

    #[test]
    fn defaults_are_usable() {
        let c = Config::default();
        assert!(!c.rotation.is_empty());
        assert!(c.cell_w > 0 && c.cell_h > 0);
        assert_eq!(c.font, DEFAULT_FONT);
    }

    #[test]
    fn toml_layer_parses_flat_keys() {
        let mut c = Config::default();
        c.apply_toml(
            r#"
            # comment
            [display]
            font = "Consolas"
            cell_w = 12
            fps = 30
            rotation = "rain, flames"
            rotate_secs = 45
        "#,
        );
        assert_eq!(c.font, "Consolas");
        assert_eq!(c.cell_w, 12);
        assert_eq!(c.fps, 30);
        assert_eq!(c.rotation, vec!["rain", "flames"]);
        assert_eq!(c.rotate_every, Some(Duration::from_secs(45)));
    }

    #[test]
    fn effect_sections_hold_params_top_level_holds_config() {
        let mut c = Config::default();
        // r##"..."## because the colour literal contains `"#`, which would
        // close a single-hash raw string early.
        c.apply_toml(
            r##"
            fps = 25
            [rain]
            tick_ms = 40
            head = "#c8ffc8"
            [flames]
            seed = 90
        "##,
        );
        assert_eq!(c.fps, 25);
        assert_eq!(c.effect_params["rain"]["tick_ms"], "40");
        assert_eq!(c.effect_params["rain"]["head"], "#c8ffc8");
        assert_eq!(c.effect_params["flames"]["seed"], "90");
        // Effect params must NOT leak into Config fields.
        assert_eq!(c.cell_w, Config::default().cell_w);
    }

    #[test]
    fn non_effect_sections_are_decorative_only() {
        // Regression: treating EVERY `[section]` as effect params meant a
        // cosmetic header silently disabled every setting beneath it.
        let mut c = Config::default();
        c.apply_toml("[display]\nfont = \"Consolas\"\nfps = 30");
        assert_eq!(c.font, "Consolas");
        assert_eq!(c.fps, 30);
        assert!(c.effect_params.is_empty());
    }

    #[test]
    fn toml_round_trips_through_save_format() {
        let mut c = Config::default();
        c.fps = 42;
        c.cell_w = 11;
        c.cell_h = 16;
        c.rotation = vec!["rain".into()];
        c.set_effect_param("rain", "tick_ms", "40".into());
        c.set_effect_param("rain", "head", "#aabbcc".into());

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.fps, 42);
        assert_eq!(back.cell_w, 11);
        assert_eq!(back.cell_h, 16);
        assert_eq!(back.rotation, vec!["rain"]);
        assert_eq!(back.effect_params["rain"]["tick_ms"], "40");
        assert_eq!(back.effect_params["rain"]["head"], "#aabbcc");
    }

    #[test]
    fn set_field_rejects_bad_values() {
        let mut c = Config::default();
        let fps = c.fps;
        assert!(!c.set_field("fps", &serde_json::json!(0)));
        assert!(!c.set_field("fps", &serde_json::json!(999)));
        assert!(!c.set_field("nope", &serde_json::json!(1)));
        assert!(!c.set_field("font", &serde_json::json!("  ")));
        assert_eq!(c.fps, fps, "a rejected value must not mutate the config");
        assert!(c.set_field("fps", &serde_json::json!(30)));
        assert_eq!(c.fps, 30);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut c = Config::default();
        c.apply_toml("wat = 3\nfont = \"X\"");
        assert_eq!(c.font, "X");
    }

    #[test]
    fn bad_values_do_not_clobber_defaults() {
        let mut c = Config::default();
        let (w, fps) = (c.cell_w, c.fps);
        c.apply_toml("cell_w = -5\nfps = 0\nfps = 9999");
        assert_eq!(c.cell_w, w);
        assert_eq!(c.fps, fps);
    }

    #[test]
    fn empty_rotation_falls_back() {
        let mut c = Config::default();
        c.rotation = vec![" ".into(), "".into()];
        c.normalise();
        assert!(!c.rotation.is_empty(), "empty rotation would draw nothing");
    }

    #[test]
    fn rotate_zero_means_never() {
        let mut c = Config::default();
        c.apply_toml("rotate_secs = 0");
        assert!(c.rotate_every.is_none());
    }

    // ---- desktop wallpaper -------------------------------------------------

    // ---- window opacity ----------------------------------------------------

    #[test]
    fn opacity_round_trips_through_toml() {
        let mut c = Config::default();
        c.apply_toml("opacity = 45");
        assert_eq!(c.opacity, 45);
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.opacity, 45);
    }

    #[test]
    fn opacity_refuses_values_that_would_lose_the_window() {
        // 0% is not merely dark — Microsoft's docs note a fully transparent
        // window is also UNFOCUSABLE, so the user could not click it back.
        //
        // `set_field` used to REJECT an out-of-range opacity outright, on the
        // reasoning that a key silently snapping to a different number fights
        // the person holding it down. Sound in itself -- but it assumed no UI
        // would ever OFFER a value the daemon would not take, and both of them
        // did: the GUI slider and the TUI row both ran 10..100 against a floor
        // of 35. Dragging below the floor moved the control and changed
        // nothing, with no message anywhere. Reported as "opacity is locked at
        // 35%".
        //
        // Both UIs now take their range from `opacity::MIN_PERCENT`, so the
        // holding-a-key case cannot reach the floor from below at all. What is
        // left is a caller asking for something out of range, and for that a
        // clamp that LOGS beats a silent refusal.
        let mut c = Config::default();
        assert!(c.set_field("opacity", &serde_json::json!(0)));
        assert_eq!(c.opacity, crate::opacity::MIN_PERCENT);
        assert!(c.set_field("opacity", &serde_json::json!(101)));
        assert_eq!(c.opacity, crate::opacity::MAX_PERCENT);
        assert!(c.set_field("opacity", &serde_json::json!(crate::opacity::MIN_PERCENT)));
        assert!(c.set_field("opacity", &serde_json::json!(crate::opacity::MAX_PERCENT)));
    }

    #[test]
    fn an_out_of_range_opacity_in_toml_is_clamped() {
        // Clamped to the nearest legal value, NOT dropped for the default.
        //
        // This is the migration path. The floor has moved up (10% tore the
        // display under heavy output — see `opacity::MIN_PERCENT`), so configs
        // written by older builds hold values that are now illegal but still
        // mean something: "as see-through as you allow". Falling back to the
        // default would change the user's terminal at startup with no
        // connection to anything they did.
        let mut c = Config::default();
        c.apply_toml("opacity = 10");
        assert_eq!(c.opacity, crate::opacity::MIN_PERCENT);
        c.apply_toml("opacity = 0");
        assert_eq!(c.opacity, crate::opacity::MIN_PERCENT);
        c.apply_toml("opacity = 250");
        assert_eq!(c.opacity, crate::opacity::MAX_PERCENT);
    }

    #[test]
    fn opacity_is_not_swallowed_by_the_wallpaper_prefix_parser() {
        // `parse_wallpaper_effect_key` is a prefix match; make sure it does not
        // claim unrelated keys.
        assert_eq!(parse_wallpaper_effect_key("opacity"), None);
        let mut c = Config::default();
        c.apply_toml("opacity = 55\nwallpaper_fps = 7");
        assert_eq!(c.opacity, 55);
        assert_eq!(c.wallpaper_fps, 7);
        assert!(c.wallpaper_effects.is_empty());
    }

    #[test]
    fn wallpaper_keys_must_stay_flat_not_a_section() {
        // THE tripwire for the flat-key decision. Any section not named after an
        // effect is decorative, so its keys fall through to the top level — a
        // `[wallpaper]` header with `fps = 5` under it therefore sets the
        // TERMINAL frame rate, silently, which is exactly the bug flat keys
        // exist to avoid. If this ever "fails" because someone tidied the config
        // into a section, they have reintroduced it.
        let mut c = Config::default();
        c.apply_toml("[wallpaper]\nfps = 5");
        assert_eq!(c.fps, 5, "a [wallpaper] section leaks into the terminal fps");
        assert_eq!(c.wallpaper_fps, Config::default().wallpaper_fps);
    }

    #[test]
    fn per_monitor_effects_parse_and_round_trip() {
        let mut c = Config::default();
        c.apply_toml(
            r#"
            wallpaper_fps = 7
            wallpaper_1_effect = "waves"
            wallpaper_3_effect = "flames"
            wallpaper_4_effect = "off"
        "#,
        );
        assert_eq!(c.wallpaper_effects.get(&1).map(String::as_str), Some("waves"));
        assert_eq!(c.wallpaper_effects.get(&3).map(String::as_str), Some("flames"));
        assert_eq!(c.wallpaper_effects.get(&4).map(String::as_str), Some("off"));
        assert!(!c.wallpaper_effects.contains_key(&2), "absent stays absent");
        assert_eq!(c.wallpaper_fps, 7);

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_effects, c.wallpaper_effects);
        assert_eq!(back.wallpaper_fps, c.wallpaper_fps);
    }

    #[test]
    fn wallpaper_effect_key_parser_is_strict() {
        // `wallpaper_fps` shares the prefix. An over-eager match would swallow
        // it into the per-monitor map, where nothing would ever read it and the
        // frame rate would silently stay at its default.
        assert_eq!(parse_wallpaper_effect_key("wallpaper_3_effect"), Some(3));
        assert_eq!(parse_wallpaper_effect_key("wallpaper_12_effect"), Some(12));
        assert_eq!(parse_wallpaper_effect_key("wallpaper_fps"), None);
        assert_eq!(parse_wallpaper_effect_key("wallpaper_cell_w"), None);
        assert_eq!(parse_wallpaper_effect_key("wallpaper_effect"), None);
        assert_eq!(parse_wallpaper_effect_key("wallpaper_x_effect"), None);
        assert_eq!(parse_wallpaper_effect_key("wallpaper_0_effect"), None);
        assert_eq!(parse_wallpaper_effect_key("font"), None);
    }

    #[test]
    fn wallpaper_fps_is_not_captured_by_the_prefix_match() {
        let mut c = Config::default();
        c.apply_toml("wallpaper_fps = 12");
        assert_eq!(c.wallpaper_fps, 12);
        assert!(c.wallpaper_effects.is_empty());
    }

    #[test]
    fn wallpaper_cell_is_independent_of_the_terminal_cell() {
        // The 57% cell-count trap: the terminal's explicit 10x15 must not reach
        // the wallpaper, where it would more than double the glyph count on a
        // portrait monitor.
        let mut c = Config::default();
        c.apply_toml("cell_w = 10\ncell_h = 15");
        assert!(c.cell_explicit);
        assert_eq!(c.wallpaper_cell_w, 15, "terminal cell leaked into wallpaper");
        assert_eq!(c.wallpaper_cell_h, 23);
        assert!(!c.wallpaper_cell_explicit);
    }

    #[test]
    fn set_field_accepts_off_and_rejects_unknown_effects() {
        let mut c = Config::default();
        assert!(c.set_field("wallpaper_2_effect", &serde_json::json!("off")));
        assert!(c.set_field("wallpaper_2_effect", &serde_json::json!("waves")));
        assert!(!c.set_field("wallpaper_2_effect", &serde_json::json!("nope")));
        assert_eq!(
            c.wallpaper_effects.get(&2).map(String::as_str),
            Some("waves"),
            "a rejected value must not mutate"
        );
    }

    #[test]
    fn set_field_handles_wallpaper_scalars() {
        let mut c = Config::default();
        assert!(c.set_field("wallpaper_fps", &serde_json::json!(9)));
        assert_eq!(c.wallpaper_fps, 9);
        assert!(!c.set_field("wallpaper_fps", &serde_json::json!(0)));
        assert_eq!(c.wallpaper_fps, 9, "rejected value must not mutate");
        assert!(c.set_field("wallpaper_cell_w", &serde_json::json!(20)));
        assert!(c.wallpaper_cell_explicit);
    }

    #[test]
    fn an_unplugged_monitors_effect_survives_a_save_and_reload() {
        // Unplug a monitor and its chosen effect must NOT be discarded, so
        // plugging it back in restores what the user set rather than silently
        // reverting to `off`. Keying on the DISPLAY<n> number is what makes this
        // work — an index into the attached list would renumber and hand the
        // setting to a different screen.
        let mut c = Config::default();
        c.wallpaper_effects.insert(1, "waves".into());
        c.wallpaper_effects.insert(7, "rain".into()); // not currently attached
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(
            back.wallpaper_effects.get(&7).map(String::as_str),
            Some("rain"),
            "an absent monitor's choice must survive a save/load cycle"
        );
        assert_eq!(back.wallpaper_effects.len(), 2);
    }

    #[test]
    fn a_stray_wallpaper_key_inside_an_effect_section_stays_a_param() {
        // Ordering guard: the prefix match must sit AFTER the effect-section
        // diversion, or a key inside [waves] would reach global state.
        let mut c = Config::default();
        c.apply_toml("[waves]\nwallpaper_1_effect = \"rain\"");
        assert!(
            c.wallpaper_effects.is_empty(),
            "a key inside an effect section must stay a param"
        );
        assert_eq!(
            c.effect_params["waves"].get("wallpaper_1_effect").map(String::as_str),
            Some("rain")
        );
    }

    // ---- wallpaper effect params -------------------------------------------

    #[test]
    fn wallpaper_params_live_in_their_own_namespace() {
        // THE requirement: the same effect, tuned twice, independently.
        let mut c = Config::default();
        c.apply_toml("[waves]
ink = \"#ff0000\"

[wallpaper.waves]
ink = \"#0000ff\"");
        assert_eq!(c.effect_params["waves"]["ink"], "#ff0000");
        assert_eq!(c.wallpaper_effect_params["waves"]["ink"], "#0000ff");
    }

    #[test]
    fn tuning_the_pane_does_not_move_the_wallpaper() {
        let mut c = Config::default();
        c.set_effect_param("waves", "ink", "#ff0000".into());
        assert!(c.wallpaper_effect_params.is_empty(), "pane tuning leaked");
        c.set_wallpaper_effect_param("waves", "ink", "#0000ff".into());
        assert_eq!(c.effect_params["waves"]["ink"], "#ff0000", "wallpaper tuning leaked");
    }

    #[test]
    fn a_monitor_override_lies_over_the_shared_block() {
        // THE per-monitor feature. Before it, every screen running an effect
        // read one set of values, so tuning one tuned all of them and the only
        // way to get two looks was to run two different effects.
        let mut c = Config::default();
        c.set_wallpaper_effect_param("waves", "ink", "#0000ff".into());
        c.set_wallpaper_effect_param("waves", "speed", "1000".into());
        c.set_wallpaper_monitor_param(3, "waves", "ink", "#ff0000".into());

        // Monitor 3 sees its own ink but still inherits the shared speed --
        // MERGED, not either-or. Replacing wholesale would silently drop every
        // shared value the moment one knob was tuned on one screen.
        let m3 = c.wallpaper_params_for(3, "waves");
        assert_eq!(m3.get("ink").map(String::as_str), Some("#ff0000"));
        assert_eq!(m3.get("speed").map(String::as_str), Some("1000"));

        // Every other screen is untouched.
        let m1 = c.wallpaper_params_for(1, "waves");
        assert_eq!(m1.get("ink").map(String::as_str), Some("#0000ff"));
    }

    #[test]
    fn a_monitor_with_no_override_falls_back_entirely() {
        // What keeps existing configs working: a file written before
        // per-monitor params has no overrides at all, and every screen must
        // still read the shared block.
        let mut c = Config::default();
        c.set_wallpaper_effect_param("flames", "seed", "65".into());
        assert_eq!(
            c.wallpaper_params_for(2, "flames").get("seed").map(String::as_str),
            Some("65")
        );
    }

    #[test]
    fn an_effect_nobody_tuned_resolves_empty() {
        // Empty means "leave the effect's own constructor defaults alone" --
        // see `apply_saved_params`. It must not invent entries.
        let c = Config::default();
        assert!(c.wallpaper_params_for(1, "waves").is_empty());
    }

    #[test]
    fn per_monitor_sections_round_trip_through_toml() {
        // The values are useless if they do not survive a save/load.
        let mut c = Config::default();
        c.set_wallpaper_effect_param("waves", "ink", "#0000ff".into());
        c.set_wallpaper_monitor_param(3, "waves", "ink", "#ff0000".into());
        c.set_wallpaper_monitor_param(3, "waves", "speed", "2500".into());

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_monitor_params, c.wallpaper_monitor_params);
        assert_eq!(
            back.wallpaper_params_for(3, "waves").get("ink").map(String::as_str),
            Some("#ff0000")
        );
    }

    #[test]
    fn parse_wallpaper_monitor_section_is_strict() {
        // As strict as the shared form: BOTH halves must be valid or the
        // section stays decorative. A typo becoming a silent param sink that
        // nothing ever reads is the failure being avoided.
        assert_eq!(
            parse_wallpaper_monitor_section("wallpaper.3.waves"),
            Some((3, "waves".to_string()))
        );
        // Monitor 0 does not exist -- DISPLAY<n> is 1-based.
        assert_eq!(parse_wallpaper_monitor_section("wallpaper.0.waves"), None);
        // Not a real effect.
        assert_eq!(parse_wallpaper_monitor_section("wallpaper.3.nonsense"), None);
        // Not a number.
        assert_eq!(parse_wallpaper_monitor_section("wallpaper.x.waves"), None);
        // The shared form must NOT match this one.
        assert_eq!(parse_wallpaper_monitor_section("wallpaper.waves"), None);
    }

    #[test]
    fn the_shared_and_per_monitor_forms_do_not_collide() {
        // `wallpaper.3.waves` also starts with `wallpaper.`, so the two parsers
        // must not both claim it -- whichever ran second would win silently.
        let mut c = Config::default();
        c.apply_toml("[wallpaper.waves]\nink = \"#0000ff\"\n[wallpaper.3.waves]\nink = \"#ff0000\"");
        assert_eq!(
            c.wallpaper_effect_params["waves"]["ink"], "#0000ff",
            "the shared block took the per-monitor value"
        );
        assert_eq!(
            c.wallpaper_monitor_params[&3]["waves"]["ink"], "#ff0000",
            "the per-monitor block was not recorded"
        );
    }

    #[test]
    fn pane_off_round_trips_and_accepts_an_int() {
        // The TUI renders it as an ordinary numeric row, so 0/1 has to work as
        // well as a bool -- otherwise the switch silently does nothing.
        let mut c = Config::default();
        assert!(!c.pane_off, "backdrops must default to ON");
        assert!(c.set_field("pane_off", &serde_json::json!(1)));
        assert!(c.pane_off);
        assert!(c.set_field("pane_off", &serde_json::json!(0)));
        assert!(!c.pane_off);
        assert!(c.set_field("pane_off", &serde_json::json!(true)));
        assert!(c.pane_off);

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert!(back.pane_off, "pane_off did not survive a save/load");
    }

    /// Switching the backdrops off must give back an ordinary solid terminal --
    /// WITHOUT destroying the opacity that was chosen for when they are on.
    #[test]
    fn backdrops_off_makes_the_terminal_solid_but_remembers_the_setting() {
        let mut c = Config::default();
        assert!(c.set_field("opacity", &serde_json::json!(80)));
        assert_eq!(c.effective_opacity(), 80, "backdrops on: use the setting");

        assert!(c.set_field("pane_off", &serde_json::json!(true)));
        assert_eq!(
            c.effective_opacity(),
            100,
            "with nothing drawn behind it, a see-through terminal shows the bare desktop"
        );
        assert_eq!(
            c.opacity, 80,
            "the STORED value must survive -- clobbering it loses the setting for good"
        );

        assert!(c.set_field("pane_off", &serde_json::json!(false)));
        assert_eq!(c.effective_opacity(), 80, "turning them back on restores it");
    }

    /// `window_source` must survive a save/load.
    ///
    /// This test exists because it did NOT, and the failure was completely
    /// silent: `set_field` accepted `"native"`, the live daemon switched, and
    /// `save()` wrote a file with no `window_source` line at all — so the next
    /// restart came back on GlazeWM as though nothing had been asked for. A
    /// field can be fully wired through env, TOML parsing and `set_field` and
    /// still be missing from `to_toml`, which is the one direction nothing else
    /// exercises.
    #[test]
    fn window_source_round_trips_and_rejects_nonsense() {
        let mut c = Config::default();
        assert_eq!(c.window_source, "auto");

        assert!(c.set_field("window_source", &serde_json::json!("native")));
        assert_eq!(c.window_source, "native");

        assert!(
            c.to_toml().contains("window_source"),
            "to_toml must EMIT the field, not just parse it"
        );

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.window_source, "native", "did not survive save/load");

        // A typo must be refused, not silently accepted as auto.
        assert!(!c.set_field("window_source", &serde_json::json!("natve")));
        assert_eq!(c.window_source, "native", "a rejected value must not mutate");
    }

    /// The freeze threshold has to survive a save/load, or the wallpaper goes
    /// back to animating behind a covered screen on the next restart.
    #[test]
    fn wallpaper_freeze_at_round_trips_and_is_range_checked() {
        let mut c = Config::default();
        assert_eq!(c.wallpaper_freeze_at, 92);

        assert!(c.set_field("wallpaper_freeze_at", &serde_json::json!(75)));
        assert_eq!(c.wallpaper_freeze_at, 75);

        // Out of range must be refused rather than clamped silently: 0 would
        // freeze an empty desktop and 101 could never be reached.
        assert!(!c.set_field("wallpaper_freeze_at", &serde_json::json!(0)));
        assert!(!c.set_field("wallpaper_freeze_at", &serde_json::json!(101)));
        assert_eq!(c.wallpaper_freeze_at, 75, "a rejected value must not mutate");

        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_freeze_at, 75);
    }

    #[test]
    fn a_single_effect_monitor_is_a_stack_of_one() {
        // What keeps every config written before layers existed valid: the base
        // effect IS layer 0, so nothing needs migrating and a monitor that has
        // never heard of layers behaves exactly as it did.
        let mut c = Config::default();
        c.wallpaper_effects.insert(3, "waves".into());
        assert_eq!(c.wallpaper_stack(3), vec!["waves".to_string()]);
    }

    #[test]
    fn layers_stack_bottom_first() {
        // Michael's example: flames on the bottom, the skull above it. The
        // ORDER is the feature -- reversed, the skull is behind the fire.
        let mut c = Config::default();
        c.set_wallpaper_layer(2, 0, "flames");
        c.set_wallpaper_layer(2, 1, "skullspin");
        assert_eq!(
            c.wallpaper_stack(2),
            vec!["flames".to_string(), "skullspin".to_string()]
        );
    }

    #[test]
    fn layer_zero_writes_the_base_effect() {
        // The two storages must not be able to disagree about the bottom of the
        // stack, so layer 0 is routed to `wallpaper_effects` rather than stored
        // separately.
        let mut c = Config::default();
        c.set_wallpaper_layer(1, 0, "rain");
        assert_eq!(c.wallpaper_effects.get(&1).map(String::as_str), Some("rain"));
        assert!(c.wallpaper_layers.get(&1).is_none());
    }

    #[test]
    fn an_off_layer_is_removed_not_stored_as_a_hole() {
        // A stack with holes in it is a different thing to reason about, and
        // its length would lie about what is actually drawn.
        let mut c = Config::default();
        c.set_wallpaper_layer(1, 0, "waves");
        c.set_wallpaper_layer(1, 1, "skullspin");
        c.set_wallpaper_layer(1, 2, "plasma");
        assert_eq!(c.wallpaper_stack(1).len(), 3);
        c.set_wallpaper_layer(1, 1, "off");
        assert_eq!(
            c.wallpaper_stack(1),
            vec!["waves".to_string(), "plasma".to_string()],
            "removing a middle layer must close the gap, not leave a hole"
        );
        // And the map itself is cleaned up when the last extra layer goes.
        c.set_wallpaper_layer(1, 2, "off");
        assert!(c.wallpaper_layers.get(&1).is_none());
    }

    #[test]
    fn an_off_base_effect_means_nothing_is_drawn() {
        // `off` has always meant "no surface at all", and layers must not
        // resurrect a monitor the user switched off.
        let mut c = Config::default();
        c.set_wallpaper_layer(4, 0, "off");
        assert!(c.wallpaper_stack(4).is_empty());
    }

    #[test]
    fn layers_round_trip_through_toml() {
        let mut c = Config::default();
        c.set_wallpaper_layer(3, 0, "flames");
        c.set_wallpaper_layer(3, 1, "skullspin");
        c.set_wallpaper_layer(1, 1, "tunnel");
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_stack(3), c.wallpaper_stack(3));
        assert_eq!(back.wallpaper_layers, c.wallpaper_layers);
    }

    #[test]
    fn parse_wallpaper_layer_key_is_strict() {
        assert_eq!(parse_wallpaper_layer_key("wallpaper_3_layer2"), Some((3, 2)));
        // Layer 0 has its own key; accepting both spellings would let one
        // config disagree with itself about the bottom of the stack.
        assert_eq!(parse_wallpaper_layer_key("wallpaper_3_layer0"), None);
        // Monitor 0 does not exist -- DISPLAY<n> is 1-based.
        assert_eq!(parse_wallpaper_layer_key("wallpaper_0_layer1"), None);
        // And the base-effect key must not be claimed by this parser.
        assert_eq!(parse_wallpaper_layer_key("wallpaper_3_effect"), None);
        assert_eq!(parse_wallpaper_layer_key("wallpaper_x_layer1"), None);
        assert_eq!(parse_wallpaper_layer_key("wallpaper_3_layerx"), None);
    }

    #[test]
    fn the_two_wallpaper_key_parsers_do_not_collide() {
        // Both start with `wallpaper_<n>_`, so whichever ran second would win
        // silently if they overlapped.
        let mut c = Config::default();
        c.apply_toml("wallpaper_3_effect = \"waves\"\nwallpaper_3_layer1 = \"skullspin\"");
        assert_eq!(c.wallpaper_effects.get(&3).map(String::as_str), Some("waves"));
        assert_eq!(
            c.wallpaper_layers[&3].get(&1).map(String::as_str),
            Some("skullspin")
        );
    }

    #[test]
    fn parse_wallpaper_section_is_strict() {
        assert_eq!(parse_wallpaper_section("wallpaper.waves").as_deref(), Some("waves"));
        assert_eq!(parse_wallpaper_section("wallpaper.rain").as_deref(), Some("rain"));
        // A bare [wallpaper] must stay decorative -- another test depends on it.
        assert_eq!(parse_wallpaper_section("wallpaper"), None);
        assert_eq!(parse_wallpaper_section("wallpaper."), None);
        // Not a real effect: must not become a param sink.
        assert_eq!(parse_wallpaper_section("wallpaper.nonsense"), None);
        assert_eq!(parse_wallpaper_section("wallpaper.waves.extra"), None);
        assert_eq!(parse_wallpaper_section("waves"), None);
        assert_eq!(parse_wallpaper_section("display"), None);
    }

    #[test]
    fn an_unknown_dotted_section_stays_decorative() {
        // The safety story: only REAL effect names become param sinks. Anything
        // else keeps the old decorative behaviour, so its keys still set the
        // top-level fields rather than vanishing into a map nothing reads.
        let mut c = Config::default();
        c.apply_toml("[wallpaper.nonsense]
fps = 7");
        assert_eq!(c.fps, 7, "an unknown dotted section must stay decorative");
        assert!(c.wallpaper_effect_params.is_empty());
    }

    #[test]
    fn a_bare_wallpaper_section_is_still_decorative() {
        // Guards the same rule from the new feature's side: adding
        // `[wallpaper.<effect>]` must not accidentally make `[wallpaper]` a
        // section too.
        let mut c = Config::default();
        c.apply_toml("[wallpaper]
fps = 5");
        assert_eq!(c.fps, 5);
        assert_eq!(c.wallpaper_fps, Config::default().wallpaper_fps);
        assert!(c.wallpaper_effect_params.is_empty());
    }

    #[test]
    fn wallpaper_params_round_trip_through_toml() {
        let mut c = Config::default();
        c.set_effect_param("waves", "ink", "#ff0000".into());
        c.set_wallpaper_effect_param("waves", "ink", "#0000ff".into());
        c.set_wallpaper_effect_param("waves", "darkcut", "400".into());
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.effect_params, c.effect_params);
        assert_eq!(back.wallpaper_effect_params, c.wallpaper_effect_params);
    }

    #[test]
    fn wallpaper_sections_do_not_disturb_the_flat_monitor_keys() {
        // `wallpaper_1_effect` and `[wallpaper.waves]` share a prefix but are
        // parsed by different rules; neither may swallow the other.
        let mut c = Config::default();
        c.apply_toml("wallpaper_1_effect = \"waves\"
wallpaper_fps = 9

[wallpaper.waves]
ink = \"#0000ff\"");
        assert_eq!(c.wallpaper_effects.get(&1).map(String::as_str), Some("waves"));
        assert_eq!(c.wallpaper_fps, 9);
        assert_eq!(c.wallpaper_effect_params["waves"]["ink"], "#0000ff");
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_effects, c.wallpaper_effects);
        assert_eq!(back.wallpaper_fps, 9);
        assert_eq!(back.wallpaper_effect_params, c.wallpaper_effect_params);
    }

    #[test]
    fn an_empty_wallpaper_param_map_emits_no_section() {
        let c = Config::default();
        assert!(!c.to_toml().contains("[wallpaper."));
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn detail_maps_to_the_expected_cell_ladder() {
        assert_eq!(detail_to_cell(1), (24, 37), "rung 1 = chunkiest");
        assert_eq!(detail_to_cell(5), (16, 25), "rung 5 ~ today's default");
        assert_eq!(detail_to_cell(10), (6, 9), "rung 10 = the floor");
    }

    #[test]
    fn detail_preserves_the_house_aspect_ratio() {
        // Scaling only one axis would stretch every effect as the slider moved.
        // `waves` is tuned at 15:23; every rung must stay within a pixel of it.
        for d in DETAIL_MIN..=DETAIL_MAX {
            let (w, h) = detail_to_cell(d);
            let want = w as f32 * 23.0 / 15.0;
            assert!(
                (h as f32 - want).abs() <= 1.0,
                "rung {d} is {w}x{h}, off the 15:23 shape (wanted h~{want:.1})"
            );
        }
    }

    #[test]
    fn the_ladder_only_ever_gets_finer() {
        // A slider that moved right and produced BIGGER cells would be maddening.
        let mut prev = i32::MAX;
        for d in DETAIL_MIN..=DETAIL_MAX {
            let (w, _) = detail_to_cell(d);
            assert!(w < prev, "rung {d} did not get finer than the one before");
            prev = w;
        }
    }

    #[test]
    fn detail_is_clamped_never_wrapped() {
        // 0 and 99 must land on the ends, not produce absurd or negative cells.
        assert_eq!(detail_to_cell(0), detail_to_cell(DETAIL_MIN));
        assert_eq!(detail_to_cell(99), detail_to_cell(DETAIL_MAX));
        let (w, h) = detail_to_cell(99);
        assert!(w > 0 && h > 0, "clamping must never yield a degenerate cell");
    }

    #[test]
    fn setting_detail_writes_the_cell_size() {
        // ONE source of truth: the friendly knob writes the raw values rather
        // than keeping a second, competing notion of size.
        let mut c = Config::default();
        assert!(c.set_field("wallpaper_detail", &serde_json::json!(10)));
        assert_eq!(c.wallpaper_detail, 10);
        assert_eq!((c.wallpaper_cell_w, c.wallpaper_cell_h), (6, 9));
        assert!(
            c.wallpaper_cell_explicit,
            "must override the effect's preferred cell, or nothing changes"
        );
    }

    #[test]
    fn a_raw_cell_edit_keeps_the_detail_row_honest() {
        // Otherwise the two rows disagree about what is on screen.
        let mut c = Config::default();
        assert!(c.set_field("wallpaper_cell_w", &serde_json::json!(6)));
        assert_eq!(c.wallpaper_detail, 10, "detail must follow to the nearest rung");
    }

    #[test]
    fn detail_out_of_range_is_rejected_not_clamped_silently() {
        // A rejected set tells the TUI to say so. Silently accepting 99 would
        // report a level that does not exist.
        let mut c = Config::default();
        assert!(!c.set_field("wallpaper_detail", &serde_json::json!(0)));
        assert!(!c.set_field("wallpaper_detail", &serde_json::json!(11)));
    }

    #[test]
    fn detail_round_trips_through_toml() {
        let mut c = Config::default();
        assert!(c.set_field("wallpaper_detail", &serde_json::json!(8)));
        let mut back = Config::default();
        back.apply_toml(&c.to_toml());
        assert_eq!(back.wallpaper_detail, 8);
        assert_eq!(
            (back.wallpaper_cell_w, back.wallpaper_cell_h),
            (c.wallpaper_cell_w, c.wallpaper_cell_h),
            "the cell size must survive the save, not be recomputed differently"
        );
    }
}
