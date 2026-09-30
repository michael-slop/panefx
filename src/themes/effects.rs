//! A theme applied to panefx's own effects: every colour param of every effect,
//! behind the terminals AND on every monitor's wallpaper.
//!
//! Three places hold effect params in `Config`, and a theme has to reach all of
//! them or one masks another:
//!
//!   * `effect_params`            -- `[flames]`, the terminal backdrops
//!   * `wallpaper_effect_params`  -- `[wallpaper.flames]`, shared by every monitor
//!   * `wallpaper_monitor_params` -- `[wallpaper.3.flames]`, one screen's override
//!
//! The first two are written for every effect, so switching effect later still
//! wears the theme. A per-monitor block only has its EXISTING colour keys
//! rewritten: a key it does not carry already falls through to the shared block,
//! and adding one would turn a fall-through into a pinned value.
//!
//! Every slot a theme touches is recorded first (`capture`), including the ones
//! that were absent, so the house theme can put back exactly what was there --
//! hand-dialled purples included -- and remove what the theme added.

use super::catalog::Theme;
use super::derive;
use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which params map a slot lives in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum Scope {
    Pane,
    Shared,
    Monitor { n: usize },
}

/// One `[section]` a theme touched, and what its keys were before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(flatten)]
    pub scope: Scope,
    pub effect: String,
    /// Whether the section existed at all. A section the theme created is
    /// removed on the way back, not left behind empty.
    pub existed: bool,
    /// `None` = the key was absent.
    pub keys: BTreeMap<String, Option<String>>,
}

/// The colour keys a theme sets on `effect`. Fixed per effect; the values are
/// what depends on the theme.
fn keys_of(effect: &str) -> Vec<&'static str> {
    // Any palette with a background and an accent yields the full key set.
    let probe = Theme {
        group: String::new(),
        id: "probe".into(),
        name: String::new(),
        colors: [("background", "#000000"), ("accent", "#ffffff")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let r = derive::roles(&probe).expect("probe palette has both keys");
    derive::effect_colours(&r, effect).keys().copied().collect()
}

fn map_mut<'a>(
    cfg: &'a mut Config,
    scope: &Scope,
) -> &'a mut BTreeMap<String, BTreeMap<String, String>> {
    match scope {
        Scope::Pane => &mut cfg.effect_params,
        Scope::Shared => &mut cfg.wallpaper_effect_params,
        Scope::Monitor { n } => cfg.wallpaper_monitor_params.entry(*n).or_default(),
    }
}

fn section<'a>(cfg: &'a Config, scope: &Scope, effect: &str) -> Option<&'a BTreeMap<String, String>> {
    match scope {
        Scope::Pane => cfg.effect_params.get(effect),
        Scope::Shared => cfg.wallpaper_effect_params.get(effect),
        Scope::Monitor { n } => cfg.wallpaper_monitor_params.get(n)?.get(effect),
    }
}

/// Every (scope, effect, key) a theme writes in this config.
fn slots(cfg: &Config) -> Vec<(Scope, String, Vec<&'static str>)> {
    let mut out = Vec::new();
    for eff in crate::animation::EFFECTS {
        let keys = keys_of(eff);
        out.push((Scope::Pane, eff.to_string(), keys.clone()));
        out.push((Scope::Shared, eff.to_string(), keys.clone()));
        for (n, effs) in &cfg.wallpaper_monitor_params {
            if let Some(sec) = effs.get(*eff) {
                let present: Vec<&'static str> =
                    keys.iter().copied().filter(|k| sec.contains_key(*k)).collect();
                if !present.is_empty() {
                    out.push((Scope::Monitor { n: *n }, eff.to_string(), present));
                }
            }
        }
    }
    out
}

/// What the theme is about to overwrite.
pub fn capture(cfg: &Config) -> Vec<Saved> {
    slots(cfg)
        .into_iter()
        .map(|(scope, effect, keys)| {
            let sec = section(cfg, &scope, &effect);
            Saved {
                existed: sec.is_some(),
                keys: keys
                    .iter()
                    .map(|k| (k.to_string(), sec.and_then(|s| s.get(*k)).cloned()))
                    .collect(),
                scope,
                effect,
            }
        })
        .collect()
}

/// Paint `theme` into every effect. `false` when the palette cannot be used
/// (no background or accent), in which case nothing was changed.
pub fn apply(cfg: &mut Config, theme: &Theme) -> bool {
    let Some(r) = derive::roles(theme) else { return false };
    for (scope, effect, keys) in slots(cfg) {
        let colours = derive::effect_colours(&r, &effect);
        let sec = map_mut(cfg, &scope).entry(effect.clone()).or_default();
        for k in keys {
            if let Some(c) = colours.get(k) {
                sec.insert(k.to_string(), c.to_hex());
            }
        }
    }
    true
}

/// Put back what `capture` recorded.
pub fn restore(cfg: &mut Config, saved: &[Saved]) {
    for s in saved {
        let map = map_mut(cfg, &s.scope);
        {
            let sec = map.entry(s.effect.clone()).or_default();
            for (k, v) in &s.keys {
                match v {
                    Some(v) => {
                        sec.insert(k.clone(), v.clone());
                    }
                    None => {
                        sec.remove(k);
                    }
                }
            }
        }
        if !s.existed && map.get(&s.effect).is_some_and(|m| m.is_empty()) {
            map.remove(&s.effect);
        }
        if let Scope::Monitor { n } = s.scope {
            if cfg.wallpaper_monitor_params.get(&n).is_some_and(|m| m.is_empty()) {
                cfg.wallpaper_monitor_params.remove(&n);
            }
        }
    }
}

/// Make `to`'s effect colours exactly `from`'s, touching nothing else.
///
/// How the daemon mirrors a theme change: the theme is applied to the config ON
/// DISK and saved, then copied into the live one this way. Applying it to the
/// live config and saving that instead would also save every unsaved tweak made
/// in the GUI since the last save -- a colour change is not permission to
/// persist those.
pub fn sync_colours(from: &Config, to: &mut Config) {
    let mut every_key: Vec<&'static str> = Vec::new();
    for eff in crate::animation::EFFECTS {
        every_key.extend(keys_of(eff));
    }
    every_key.sort();
    every_key.dedup();

    let mut scopes = vec![Scope::Pane, Scope::Shared];
    for n in from.wallpaper_monitor_params.keys().chain(to.wallpaper_monitor_params.keys()) {
        scopes.push(Scope::Monitor { n: *n });
    }
    scopes.sort();
    scopes.dedup();

    for scope in scopes {
        for eff in crate::animation::EFFECTS {
            let src = section(from, &scope, eff).cloned();
            let map = map_mut(to, &scope);
            let dst = map.entry(eff.to_string()).or_default();
            for k in &every_key {
                match src.as_ref().and_then(|s| s.get(*k)) {
                    Some(v) => {
                        dst.insert(k.to_string(), v.clone());
                    }
                    None => {
                        dst.remove(*k);
                    }
                }
            }
            if dst.is_empty() && src.is_none() {
                map.remove(*eff);
            }
        }
        if let Scope::Monitor { n } = scope {
            if to.wallpaper_monitor_params.get(&n).is_some_and(|m| m.is_empty()) {
                to.wallpaper_monitor_params.remove(&n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{self, Dirs};

    /// pHub's real shape, cut down: green terminal fire, a hand-dialled PURPLE
    /// shared wallpaper block, and a monitor override carrying its own colours.
    const HAND_DIALLED: &str = r##"
fps = 10
wallpaper_1_effect = "flames"
wallpaper_3_effect = "flames"

[flames]
bg = "#000000"
c_cool = "#3d8fa8"
c_dim = "#4a3568"
c_hot = "#c8ffd0"
c_warm = "#62e670"
seed = 65

[plasma]
bg = "#000000"
hi = "#9a9a9a"

[wallpaper.flames]
c_cool = "#792396"
c_dim = "#55476a"
c_hot = "#fb00ff"
c_warm = "#8800ff"
seed = 93

[wallpaper.1.flames]
c_warm = "#62e670"
seed = 65

[wallpaper.1.skullspin]
bone = "#7d7d7d"
scale = 303
"##;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.apply_toml(HAND_DIALLED);
        c
    }

    fn tokyo() -> Theme {
        catalog::load(&Dirs::bundled_only()).into_iter().find(|t| t.id == "tokyo-night").unwrap()
    }

    #[test]
    fn house_restores_the_hand_dialled_colours_byte_for_byte() {
        let before = cfg();
        let mut c = before.clone();
        let saved = capture(&c);
        assert!(apply(&mut c, &tokyo()));
        assert_ne!(c.to_toml(), before.to_toml(), "the theme changed something");
        assert_eq!(c.wallpaper_effect_params["flames"]["c_warm"], "#7aa2f7");
        restore(&mut c, &saved);
        assert_eq!(c.to_toml(), before.to_toml(), "house must be exactly what was dialled in");
    }

    #[test]
    fn a_monitor_override_is_themed_not_masked() {
        let mut c = cfg();
        apply(&mut c, &tokyo());
        assert_eq!(
            c.wallpaper_params_for(1, "flames")["c_warm"], "#7aa2f7",
            "monitor 1's own c_warm would otherwise keep the old green over the theme"
        );
        // A key the override did NOT carry is still inherited, not pinned.
        assert!(!c.wallpaper_monitor_params[&1]["flames"].contains_key("c_hot"));
        assert_eq!(c.wallpaper_params_for(1, "flames")["seed"], "65", "non-colour keys untouched");
    }

    #[test]
    fn every_effect_wears_the_theme_in_both_scopes() {
        let mut c = cfg();
        apply(&mut c, &tokyo());
        for eff in crate::animation::EFFECTS {
            assert_eq!(c.effect_params[*eff]["bg"], "#1a1b26", "pane {eff}");
            assert_eq!(c.wallpaper_effect_params[*eff]["bg"], "#1a1b26", "wallpaper {eff}");
        }
    }

    #[test]
    fn the_snapshot_survives_a_round_trip_through_json() {
        let saved = capture(&cfg());
        let back: Vec<Saved> = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back, saved);
    }

    #[test]
    fn unsaved_tweaks_survive_a_theme_change() {
        // The live config has a tweak the disk does not: fps 30, and a pane seed.
        let mut live = cfg();
        live.fps = 30;
        live.set_effect_param("flames", "seed", "80".into());
        let mut disk = cfg();
        apply(&mut disk, &tokyo());
        sync_colours(&disk, &mut live);
        assert_eq!(live.fps, 30);
        assert_eq!(live.effect_params["flames"]["seed"], "80", "a non-colour tweak stays");
        assert_eq!(live.effect_params["flames"]["c_warm"], "#7aa2f7", "the colours follow");
        // And back: house on disk, synced into live, removes what the theme added.
        let saved = capture(&cfg());
        restore(&mut disk, &saved);
        sync_colours(&disk, &mut live);
        assert!(!live.effect_params.contains_key("rain"), "a section the theme created is gone");
        assert_eq!(live.wallpaper_effect_params["flames"]["c_hot"], "#fb00ff");
    }
}
