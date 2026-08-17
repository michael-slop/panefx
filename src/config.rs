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
    /// Per-effect params from `[rain]` / `[flames]` sections, kept as raw
    /// strings and applied through `AsciiAnimation::set_param` after the effect
    /// is built. Held generically so a new effect's knobs persist without
    /// touching `Config`.
    pub effect_params: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,

    // ---- desktop wallpaper -------------------------------------------------
    /// Frame rate for the desktop wallpaper, independent of `fps`.
    ///
    /// Lower by default: the wallpaper is glanced at rather than watched, and
    /// this path draws up to one full-monitor bitmap per screen against the
    /// terminal path's small ones.
    pub wallpaper_fps: u64,
    /// Effect per monitor, keyed by `DISPLAY<n>` index. An absent entry — or the
    /// literal `"off"` — means no surface at all for that monitor, so Windows'
    /// own wallpaper shows through.
    pub wallpaper_effects: std::collections::BTreeMap<usize, String>,
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
            effect_params: Default::default(),
            // Half the terminal rate. The desktop is scenery.
            wallpaper_fps: 5,
            // EMPTY = wallpapers off. A new feature must not change what the
            // user already sees until they ask for it.
            wallpaper_effects: Default::default(),
            // 15x23 matches `waves::preferred_cell()`, which was tuned live
            // against the real thing and is also 57% fewer cells than 10x15.
            wallpaper_cell_w: 15,
            wallpaper_cell_h: 23,
            wallpaper_cell_explicit: false,
        }
    }
}

/// `wallpaper_3_effect` -> `Some(3)`. Anything else -> `None`.
///
/// Strict on purpose: `wallpaper_fps` shares the prefix, and an over-eager match
/// would swallow it into the per-monitor map where nothing would ever read it.
pub fn parse_wallpaper_effect_key(k: &str) -> Option<usize> {
    let rest = k.strip_prefix("wallpaper_")?.strip_suffix("_effect")?;
    rest.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= 64)
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
        if let Some(v) = env_u64("PANEFX_WALLPAPER_FPS").filter(|v| *v > 0 && *v <= 120) {
            cfg.wallpaper_fps = v;
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

            match k.as_str() {
                "font" => self.font = v.to_string(),
                "wallpaper_fps" => {
                    if let Ok(n) = v.parse::<u64>() {
                        if n > 0 && n <= 120 {
                            self.wallpaper_fps = n;
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
        s.push_str("\n# A wallpaper has no terminal text to line up with, so it does NOT\n");
        s.push_str("# use cell_w/cell_h above. 15x23 keeps a 1440x2560 portrait at ~10k\n");
        s.push_str("# cells instead of ~24k.\n");
        s.push_str(&format!("wallpaper_cell_w = {}\n", self.wallpaper_cell_w));
        s.push_str(&format!("wallpaper_cell_h = {}\n", self.wallpaper_cell_h));
        for (idx, eff) in &self.wallpaper_effects {
            s.push_str(&format!("wallpaper_{idx}_effect = \"{eff}\"\n"));
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

    /// Set one top-level field from a JSON value, for the control channel.
    /// Returns whether the key was recognised AND the value was acceptable.
    pub fn set_field(&mut self, key: &str, v: &serde_json::Value) -> bool {
        let as_i64 = || v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let as_str = || v.as_str().map(|s| s.to_string());
        match key {
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
            "wallpaper_fps" => matches!(as_i64(), Some(n) if n > 0 && n <= 120).then(|| {
                self.wallpaper_fps = as_i64().unwrap() as u64;
            }).is_some(),
            "wallpaper_cell_w" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.wallpaper_cell_w = as_i64().unwrap() as i32;
                self.wallpaper_cell_explicit = true;
            }).is_some(),
            "wallpaper_cell_h" => matches!(as_i64(), Some(n) if n > 0 && n <= 200).then(|| {
                self.wallpaper_cell_h = as_i64().unwrap() as i32;
                self.wallpaper_cell_explicit = true;
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
}
