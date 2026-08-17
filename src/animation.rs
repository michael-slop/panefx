//! The animation abstraction.
//!
//! A panel does not know it is showing fire. It owns something that implements
//! [`AsciiAnimation`], asks it to advance one frame, and asks it for a glyph +
//! colour per cell. Adding a new effect means writing one more implementor —
//! no change to the panel, the renderer, or the supervisor.
//!
//! Note for anyone coming from a class-based language: this is Rust, so there
//! is no subclassing. Effects are separate types that each `impl
//! AsciiAnimation`, held behind `Box<dyn AsciiAnimation>`. The dispatch is
//! dynamic, which is what makes the swap possible at runtime, but the
//! relationship is "implements an interface", not "inherits from a base
//! class".

use crate::palette::Rgb;
use serde::{Deserialize, Serialize};

/// A live-tunable value belonging to one effect.
///
/// Deliberately NOT ascii-flavoured: a future GIF or shader effect would expose
/// `speed`/`loop`/`path` through the same mechanism. See the layering note in
/// HANDOFF.md — ascii is one subsystem of panefx, not its definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ParamValue {
    Int { v: i64 },
    Text { v: String },
    Colour { r: u8, g: u8, b: u8 },
}

impl ParamValue {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            ParamValue::Int { v } => Some(*v),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ParamValue::Text { v } => Some(v),
            _ => None,
        }
    }
    pub fn as_rgb(&self) -> Option<Rgb> {
        match self {
            ParamValue::Colour { r, g, b } => Some(Rgb(*r, *g, *b)),
            _ => None,
        }
    }
    pub fn from_rgb(c: Rgb) -> Self {
        ParamValue::Colour {
            r: c.0,
            g: c.1,
            b: c.2,
        }
    }
    /// Render for a TUI row.
    pub fn display(&self) -> String {
        match self {
            ParamValue::Int { v } => v.to_string(),
            ParamValue::Text { v } => v.clone(),
            ParamValue::Colour { r, g, b } => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// One tunable parameter, as declared by the effect that owns it.
///
/// The TUI renders whatever it is handed — it must never hardcode which knobs
/// belong to which effect, so a new effect's controls appear for free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub key: String,
    pub label: String,
    pub value: ParamValue,
    /// Inclusive bounds for `Int` params; ignored otherwise.
    pub min: i64,
    pub max: i64,
}

impl Param {
    pub fn int(key: &str, label: &str, v: i64, min: i64, max: i64) -> Self {
        Param {
            key: key.into(),
            label: label.into(),
            value: ParamValue::Int { v },
            min,
            max,
        }
    }
    pub fn text(key: &str, label: &str, v: &str) -> Self {
        Param {
            key: key.into(),
            label: label.into(),
            value: ParamValue::Text { v: v.into() },
            min: 0,
            max: 0,
        }
    }
    pub fn colour(key: &str, label: &str, c: Rgb) -> Self {
        Param {
            key: key.into(),
            label: label.into(),
            value: ParamValue::from_rgb(c),
            min: 0,
            max: 0,
        }
    }
}

/// Clamp helper for implementors — a TUI or a bad config must not be able to
/// push a value outside the range the effect declared.
pub fn clamp_int(v: i64, min: i64, max: i64) -> i64 {
    v.max(min).min(max)
}

/// One animated ASCII effect, rendered into a grid of character cells.
///
/// Implementors own their own simulation state and palette. The renderer only
/// ever asks for a glyph and a colour per cell, so an effect is free to use
/// any internal representation — a heat field, a set of falling columns, a
/// noise function.
pub trait AsciiAnimation {
    /// Human-readable name, used for logging and for selecting the effect by
    /// configuration.
    fn name(&self) -> &'static str;

    /// Resize the simulation grid. Called whenever the panel's cell dimensions
    /// change (window resized, font/DPI changed).
    ///
    /// Implementors may discard state on resize; callers must not rely on
    /// content being preserved.
    fn resize(&mut self, cols: usize, rows: usize);

    /// Current grid size in cells, as `(cols, rows)`.
    fn dimensions(&self) -> (usize, usize);

    /// Advance the simulation by one frame.
    fn step(&mut self);

    /// The character cell size this effect wants, in pixels.
    ///
    /// `None` (the default) means "use the configured size" — that is what
    /// `rain`, `flames` and `fire` do, because they are designed to sit on the
    /// terminal's own text grid.
    ///
    /// `waves` overrides it: the blackwaves field wants chunky cells so the
    /// glyphs read as marks rather than as text, and it looks wrong at the
    /// terminal's 10x15. Making this per-effect means one effect can be chunky
    /// without dragging the others with it.
    ///
    /// An explicit `PANEFX_CELL_W`/`_H` (or a `cell_w`/`cell_h` set live from
    /// the TUI) still wins — a user asking for a size means it.
    fn preferred_cell(&self) -> Option<(i32, i32)> {
        None
    }

    /// Did the last `step()` actually change what `cell_at` will return?
    ///
    /// Defaults to `true` (always redraw), which is always correct — an effect
    /// that does not implement this simply gets redrawn every frame as before.
    ///
    /// Effects that advance on their own clock can do much better: `rain` ticks
    /// every 55ms regardless of the panel's fps, so at 20fps roughly one frame
    /// in ten produces an identical grid. Reporting that lets the supervisor
    /// skip the glyph loop AND the BitBlt for every panel — and profiling shows
    /// the renderer, not the simulation, is where the time goes.
    fn changed(&self) -> bool {
        true
    }

    /// The glyph and colour to draw at a cell, in ONE call.
    ///
    /// Returning `None` means "draw nothing here" — the renderer skips the
    /// cell entirely rather than drawing a space. This matters: these panels
    /// sit behind a semi-transparent terminal, so an unnecessary painted cell
    /// is a visible smudge, not a no-op.
    ///
    /// Glyph and colour are returned together deliberately. Splitting them
    /// into `glyph_at` + `color_at` doubles the per-cell work, because an
    /// effect usually derives both from the same computed value — for the
    /// fire that is a dithered ramp index, and computing it twice per cell
    /// measurably raised CPU (12.7% -> 22.5% of a core when it was split).
    fn cell_at(&self, col: usize, row: usize) -> Option<(char, Rgb)>;

    /// Background the panel is cleared to before drawing.
    fn background(&self) -> Rgb;

    /// The effect's live-tunable parameters, current values included.
    ///
    /// Default: none. `fire` uses the default and simply shows no knobs.
    fn params(&self) -> Vec<Param> {
        Vec::new()
    }

    /// Apply one parameter. Returns whether it was recognised and applied, so
    /// the caller can report an unknown key instead of silently doing nothing.
    ///
    /// Implementors MUST clamp to the bounds they declared in `params()`.
    fn set_param(&mut self, _key: &str, _v: &ParamValue) -> bool {
        false
    }
}

/// Every effect that can be named in the rotation.
///
/// **To add a new background:** write a module implementing `AsciiAnimation`,
/// add its name here and a line in `build`. Nothing else in the program needs
/// to change — the supervisor, panel and renderer are all trait-only.
pub const EFFECTS: &[&str] = &["flames", "rain", "waves", "fire"];

/// Construct an effect by name. Unknown names fall back to the first entry in
/// `EFFECTS` rather than failing, so a typo in a config file degrades to a
/// working backdrop instead of a blank screen.
pub fn build(
    name: &str,
    cols: usize,
    rows: usize,
    seed: u64,
    cfg: &crate::config::Config,
) -> Box<dyn AsciiAnimation> {
    match name.trim().to_lowercase().as_str() {
        "rain" => {
            let mut r = crate::rain::Rain::new(cols, rows, seed, cfg.chars_override.as_deref());
            r.set_frame_ms(cfg.frame_time().as_millis() as u64);
            Box::new(r)
        }
        "waves" => {
            let mut w = crate::waves::Waves::new(cols, rows, cfg.chars_override.as_deref());
            w.set_frame_ms(cfg.frame_time().as_millis() as u64);
            Box::new(w)
        }
        "fire" => Box::new(crate::fire::Fire::new(cols, rows, seed)),
        _ => Box::new(crate::flames::Flames::new(cols, rows, seed)),
    }
}

/// Apply any `[effect]` params persisted in config.toml to a freshly built
/// effect. Values are stored as strings, so each is tried as an int, then a
/// colour, then plain text — whichever the effect accepts.
///
/// Lives here rather than in `main.rs` because both the terminal supervisor and
/// the wallpaper's simulation pool need it, and neither should own it.
pub fn apply_saved_params(sim: &mut dyn AsciiAnimation, cfg: &crate::config::Config) {
    let name = sim.name().to_lowercase();
    let Some(saved) = cfg.effect_params.get(&name) else {
        return;
    };
    // Collect first: `params()` borrows, `set_param` needs &mut.
    let saved: Vec<(String, String)> = saved.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (k, raw) in saved {
        let applied = if let Ok(n) = raw.parse::<i64>() {
            sim.set_param(&k, &ParamValue::Int { v: n })
        } else if let Some(c) = crate::palette::Rgb::parse_hex(&raw) {
            sim.set_param(&k, &ParamValue::from_rgb(c))
        } else {
            sim.set_param(&k, &ParamValue::Text { v: raw.clone() })
        };
        if !applied {
            eprintln!("[panefx] config [{name}]: effect ignored '{k}'");
        }
    }
}

/// Build an effect and restore its persisted params.
///
/// Used wherever a change cannot be applied in place — an effect switch, or a
/// value captured at construction such as rain's frame_ms.
pub fn rebuild(
    cfg: &crate::config::Config,
    name: &str,
    cols: usize,
    rows: usize,
    seed: u64,
) -> Box<dyn AsciiAnimation> {
    let mut s = build(name, cols, rows, seed, cfg);
    apply_saved_params(s.as_mut(), cfg);
    s
}
