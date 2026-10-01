//! A theme's palette turned into the colours each panefx effect takes.
//!
//! The flames mapping is the laptop's (`sl0p-desktop _theme_colours`), which
//! color.mesh ported to Go (`winapply.go flameColours`) and this ports again,
//! line for line, with the same test values:
//!
//!   void = background · warm = accent · hot = accent 60% toward white (45%
//!   toward BLACK on a light theme, or it vanishes into the pale background) ·
//!   cool = the blue, or the cyan when the blue IS the accent · dim = the
//!   magenta 45% toward the background.
//!
//! Every other effect gets the same five roles, plus the theme's foreground and
//! comment grey, by key name -- see `effect_colours`. One difference from the Go:
//! a palette without a background or accent yields NOTHING rather than the house
//! fire, so a broken theme leaves the colours alone instead of overwriting them.

use super::catalog::Theme;
use crate::palette::Rgb;
use std::collections::BTreeMap;

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let l = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round().clamp(0.0, 255.0) as u8;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

fn near(a: Rgb, b: Rgb) -> bool {
    let d = |x: u8, y: u8| (x as f64 - y as f64).abs() / 255.0;
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2) < 0.12
}

/// The five roles a theme lends every effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roles {
    pub bg: Rgb,
    pub fg: Rgb,
    pub muted: Rgb,
    pub hot: Rgb,
    pub warm: Rgb,
    pub cool: Rgb,
    pub dim: Rgb,
    pub magenta: Rgb,
}

/// `None` when the palette lacks a background or an accent: there is nothing
/// sound to derive from, and guessing would repaint the user's colours.
pub fn roles(t: &Theme) -> Option<Roles> {
    let bg = t.rgb("background")?;
    let acc = t.rgb("accent")?;
    let mut cool = t.first(&["color4", "blue"]).unwrap_or(acc);
    if near(cool, acc) {
        cool = t.first(&["color6", "cyan"]).unwrap_or_else(|| mix(acc, bg, 0.4));
    }
    let magenta = t.first(&["color5", "magenta"]).unwrap_or(cool);
    let dim = mix(magenta, bg, 0.45);
    let hot = if t.is_light() {
        mix(acc, Rgb(0, 0, 0), 0.45)
    } else {
        mix(acc, Rgb(255, 255, 255), 0.6)
    };
    let fg = t.rgb("foreground").unwrap_or(hot);
    let muted = t.first(&["color8"]).unwrap_or_else(|| mix(fg, bg, 0.5));
    Some(Roles { bg, fg, muted, hot, warm: acc, cool, dim, magenta })
}

/// The colour keys of `effect` that a theme sets, and their values.
///
/// Keyed by the names each effect declares in its own `params()` -- the test
/// `every_themed_key_is_a_real_colour_param` builds every effect and checks, so
/// a renamed param fails here instead of becoming a silent no-op. An effect not
/// listed keeps its colours; one with only a `bg` gets only that.
pub fn effect_colours(r: &Roles, effect: &str) -> BTreeMap<&'static str, Rgb> {
    let shadow = mix(r.fg, r.bg, 0.75);
    let pairs: Vec<(&'static str, Rgb)> = match effect {
        "flames" => vec![("c_hot", r.hot), ("c_warm", r.warm), ("c_cool", r.cool), ("c_dim", r.dim)],
        "rain" => vec![("head", r.hot), ("trail", r.warm)],
        // Waves is a dark field; its ink is the colour of the water, not a
        // highlight, so it stays well down toward the background.
        "waves" => vec![("ink", mix(r.warm, r.bg, 0.6))],
        "fire" => vec![("ramp_lo", r.dim), ("ramp_hi", r.hot)],
        "plasma" | "donut" | "sphere" | "cube" | "galaxy" => vec![("lo", r.dim), ("hi", r.warm)],
        "tunnel" | "starfield" => vec![("near", r.hot), ("far", r.cool)],
        "skullspin" => vec![("bone", r.fg), ("dark", shadow)],
        _ => vec![],
    };
    let mut out: BTreeMap<&'static str, Rgb> = pairs.into_iter().collect();
    out.insert("bg", r.bg);
    out
}

/// The color.mesh flames, as hex. For the parity tests and for anyone asking
/// "what would the laptop's fire be".
pub fn flame_colours(t: &Theme) -> Option<BTreeMap<&'static str, String>> {
    let r = roles(t)?;
    Some(effect_colours(&r, "flames").into_iter().map(|(k, v)| (k, v.to_hex())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(pairs: &[(&str, &str)]) -> Theme {
        Theme {
            group: String::new(),
            id: "x".into(),
            name: "x".into(),
            colors: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    /// The exact values color.mesh's `TestFlameColours` asserts -- the three
    /// implementations (Python, Go, Rust) must agree.
    #[test]
    fn the_flames_match_color_mesh_value_for_value() {
        let dark = theme(&[("background", "#000000"), ("accent", "#ff0000"), ("color4", "#0000ff"), ("color5", "#ff00ff")]);
        let got = flame_colours(&dark).unwrap();
        for (k, v) in [("bg", "#000000"), ("c_warm", "#ff0000"), ("c_hot", "#ff9999"), ("c_cool", "#0000ff"), ("c_dim", "#8c008c")] {
            assert_eq!(got[k], v, "dark {k}");
        }
        let same = theme(&[("background", "#000000"), ("accent", "#0000ff"), ("color4", "#0000ff"), ("color6", "#00ffff")]);
        assert_eq!(flame_colours(&same).unwrap()["c_cool"], "#00ffff", "the cyan when blue is the accent");
        let light = theme(&[("background", "#ffffff"), ("accent", "#ff0000"), ("color4", "#0000ff")]);
        assert_eq!(flame_colours(&light).unwrap()["c_hot"], "#8c0000", "hot sinks on a light theme");
    }

    #[test]
    fn a_palette_without_an_accent_changes_nothing() {
        assert!(flame_colours(&theme(&[("background", "#000000")])).is_none());
    }

    /// Every key this module sets must be a colour param the effect really
    /// declares; otherwise `set_param` refuses it and the theme silently skips
    /// that effect.
    #[test]
    fn every_themed_key_is_a_real_colour_param() {
        let t = theme(&[("background", "#101010"), ("foreground", "#e0e0e0"), ("accent", "#7aa2f7")]);
        let r = roles(&t).unwrap();
        let cfg = crate::config::Config::default();
        for eff in crate::animation::EFFECTS {
            let probe = crate::animation::build(eff, 1, 1, 0, &cfg);
            let declared: Vec<String> = probe
                .params()
                .into_iter()
                .filter(|p| matches!(p.value, crate::animation::ParamValue::Colour { .. }))
                .map(|p| p.key)
                .collect();
            for k in effect_colours(&r, eff).keys() {
                assert!(declared.iter().any(|d| d == k), "{eff} has no colour param '{k}' (it has {declared:?})");
            }
        }
    }
}
