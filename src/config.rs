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
/// Defaults to the Alacritty font on pHub so the animation shares the
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
        }
    }
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

            match k.as_str() {
                "font" => self.font = v.to_string(),
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
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str("# panefx config — written by panefx-ctl\n");
        s.push_str("# Env vars (PANEFX_*) override anything here.\n\n");
        s.push_str(&format!("font = \"{}\"\n", self.font));
        s.push_str(&format!("cell_w = {}\n", self.cell_w));
        s.push_str(&format!("cell_h = {}\n", self.cell_h));
        s.push_str(&format!("fps = {}\n", self.fps));
        s.push_str(&format!("crop_top = {}\n", self.crop_top));
        s.push_str(&format!("pad_x = {}\n", self.pad_x));
        s.push_str(&format!("pad_y = {}\n", self.pad_y));
        s.push_str(&format!("rotation = \"{}\"\n", self.rotation.join(", ")));
        s.push_str(&format!(
            "rotate_secs = {}\n",
            self.rotate_every.map(|d| d.as_secs()).unwrap_or(0)
        ));
        if let Some(c) = &self.chars_override {
            s.push_str(&format!("chars = \"{c}\"\n"));
        }
        for (effect, params) in &self.effect_params {
            if params.is_empty() {
                continue;
            }
            s.push_str(&format!("\n[{effect}]\n"));
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
}
