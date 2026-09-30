//! Themes: one choice, recoloured everywhere panefx can reach.
//!
//! This is color.mesh's Windows half, moved into panefx (2026-09-30). color.mesh
//! still drives the Linux laptop through Omarchy; on Windows panefx owns the
//! operation, because panefx is what is already running, already owns the
//! flames, and already writes Alacritty's config. The list of themes and their
//! palettes are shared with the laptop (see `catalog`).
//!
//! A theme change is ONE call, [`switch`], whoever asks for it -- the Themes tab
//! in the GUI or TUI, the tray menu, or `panefx-ctl theme <id|next|prev|house>`.
//! It recolours:
//!
//!   * panefx's own effects, behind the terminals and on every monitor
//!     (`effects`), and panefx's GUI (it follows the `theme` config key);
//!   * every app in `targets` -- Alacritty, the Neovim editors, Windows'
//!     light/dark mode, sl0p.notepad and sl0p.ink, Xournal++, Windows Terminal
//!     and VS Code -- each reporting honestly whether it changed now, changes on
//!     the app's next start, is waiting for the app to close, or is absent.
//!
//! The HOUSE theme is not a palette: it is the look that was dialled in by hand,
//! recorded the moment a theme first replaces it (`house`).

pub mod catalog;
pub mod derive;
pub mod effects;
pub mod fsutil;
pub mod house;
pub mod targets;

use crate::config::Config;
use catalog::{is_house, Theme};
use fsutil::Env;
use serde::{Deserialize, Serialize};
use targets::Outcome;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetReport {
    pub id: String,
    pub label: String,
    pub outcome: Outcome,
}

/// What a theme change did, target by target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub theme: String,
    pub name: String,
    pub targets: Vec<TargetReport>,
}

impl Report {
    /// Targets still waiting for their app to close, as (target id, theme id).
    pub fn deferred(&self) -> Vec<(String, String)> {
        self.targets
            .iter()
            .filter(|t| matches!(t.outcome, Outcome::Deferred(_)))
            .map(|t| (t.id.clone(), self.theme.clone()))
            .collect()
    }

    /// One line per target, for a terminal.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("theme: {} ({})", self.name, self.theme)];
        for t in &self.targets {
            out.push(format!("  {:<26} {:<10} {}", t.label, t.outcome.word(), t.outcome.note()));
        }
        out
    }
}

/// Switch `disk` -- the config as SAVED, not the live one (see
/// `effects::sync_colours`) -- and every app to `wanted`: an id, `next`,
/// `prev`, or `house`. The caller saves `disk` afterwards.
pub fn switch(disk: &mut Config, themes: &[Theme], wanted: &str, env: &Env) -> Result<Report, String> {
    let current = disk.theme.clone();
    let theme = catalog::resolve(themes, &current, wanted)
        .ok_or_else(|| format!("no theme '{wanted}' -- `panefx-ctl theme list` shows them"))?
        .clone();
    let dir = env.panefx_dir();
    let all = targets::all();
    let skip = disk.theme_skip.clone();
    let on = |id: &str| !skip.iter().any(|s| s == id);

    // Leaving house: record the hand-dialled look before anything changes it.
    if is_house(&current) && !theme.is_house() {
        let mut snap = house::Snapshot {
            taken: now_label(),
            effects: effects::capture(disk),
            ..Default::default()
        };
        for t in all.iter().filter(|t| on(t.id())) {
            if let Some(v) = t.capture(env) {
                snap.targets.insert(t.id().to_string(), v);
            }
        }
        house::save(&dir, &snap).map_err(|e| format!("could not record the house look: {e}"))?;
    }

    let mut reports = vec![TargetReport {
        id: "panefx".into(),
        label: "panefx effects".into(),
        outcome: Outcome::Live(String::new()),
    }];
    if theme.is_house() {
        let snap = house::load(&dir);
        reports[0].outcome = match &snap {
            Some(s) => {
                effects::restore(disk, &s.effects);
                Outcome::Live("the hand-dialled colours are back".into())
            }
            None => Outcome::Skipped("no house record (no theme was applied from house)".into()),
        };
        for t in &all {
            let outcome = if on(t.id()) {
                t.restore(env, snap.as_ref().and_then(|s| s.targets.get(t.id())))
            } else {
                Outcome::Skipped("switched off".into())
            };
            reports.push(TargetReport { id: t.id().into(), label: t.label().into(), outcome });
        }
    } else {
        reports[0].outcome = if effects::apply(disk, &theme) {
            Outcome::Live("every effect, terminals and wallpaper".into())
        } else {
            Outcome::Failed(format!("{} has no background or accent", theme.id))
        };
        for t in &all {
            let outcome = if on(t.id()) {
                t.apply(env, &theme)
            } else {
                Outcome::Skipped("switched off".into())
            };
            reports.push(TargetReport { id: t.id().into(), label: t.label().into(), outcome });
        }
    }

    disk.theme = if theme.is_house() { catalog::HOUSE_ID.into() } else { theme.id.clone() };
    Ok(Report { theme: disk.theme.clone(), name: theme.name.clone(), targets: reports })
}

/// Try one deferred target again. `None` once the theme has moved on (the
/// retry is moot) or the target no longer exists.
pub fn retry(target: &str, theme_id: &str, current: &str, themes: &[Theme], env: &Env) -> Option<Outcome> {
    if theme_id != current {
        return None;
    }
    let t = targets::by_id(target)?;
    if is_house(theme_id) {
        let snap = house::load(&env.panefx_dir());
        Some(t.restore(env, snap.as_ref().and_then(|s| s.targets.get(target))))
    } else {
        let theme = themes.iter().find(|x| x.id == theme_id)?;
        Some(t.apply(env, theme))
    }
}

fn now_label() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix {secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::Dirs;

    fn env(name: &str) -> (std::path::PathBuf, Env) {
        let root = std::env::temp_dir().join(format!("panefx-test-switch-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        (root.clone(), Env::under(&root))
    }

    fn hand_dialled() -> Config {
        let mut c = Config::default();
        c.set_wallpaper_effect_param("flames", "c_hot", "#fb00ff".into());
        c.set_effect_param("flames", "c_warm", "#62e670".into());
        c
    }

    #[test]
    fn a_tour_of_themes_ends_back_on_the_hand_dialled_look() {
        let (root, env) = env("tour");
        let themes = catalog::load(&Dirs::bundled_only());
        let before = hand_dialled();
        let mut disk = before.clone();
        for w in ["tokyo-night", "next", "dracula", "prev"] {
            switch(&mut disk, &themes, w, &env).unwrap();
            assert!(!is_house(&disk.theme), "{w}");
        }
        let r = switch(&mut disk, &themes, "house", &env).unwrap();
        assert_eq!(r.targets[0].outcome.word(), "live");
        assert_eq!(disk.to_toml(), before.to_toml(), "the snapshot is the look BEFORE the first theme");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_theme_key_is_written_only_off_house() {
        let (root, env) = env("key");
        let themes = catalog::load(&Dirs::bundled_only());
        let mut disk = Config::default();
        assert!(!disk.to_toml().contains("theme ="));
        switch(&mut disk, &themes, "nord", &env).unwrap();
        assert!(disk.to_toml().contains("theme = \"nord\""));
        let mut back = Config::default();
        back.apply_toml(&disk.to_toml());
        assert_eq!(back.theme, "nord", "and it reads back");
        let _ = std::fs::remove_dir_all(root);
    }

    /// `theme` is a flat key. A `[theme]` section would be decorative, its keys
    /// falling through to real settings -- the trap `apply_toml` documents.
    #[test]
    fn a_theme_section_never_leaks_into_config() {
        let mut c = Config::default();
        c.apply_toml("[flames]\nseed = 70\n");
        switch(&mut c, &catalog::load(&Dirs::bundled_only()), "gruvbox", &env("leak").1).unwrap();
        let text = c.to_toml();
        assert!(!text.contains("[theme"), "{text}");
    }

    #[test]
    fn a_switched_off_target_is_left_alone() {
        let (root, env) = env("skip");
        let conf = env.appdata.join("alacritty").join("alacritty.toml");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "import = []\n").unwrap();
        let mut disk = Config::default();
        disk.theme_skip = vec!["alacritty".into()];
        let r = switch(&mut disk, &catalog::load(&Dirs::bundled_only()), "nord", &env).unwrap();
        let a = r.targets.iter().find(|t| t.id == "alacritty").unwrap();
        assert_eq!(a.outcome, Outcome::Skipped("switched off".into()));
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "import = []\n");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_theme_changes_nothing() {
        let mut disk = hand_dialled();
        let before = disk.to_toml();
        assert!(switch(&mut disk, &catalog::load(&Dirs::bundled_only()), "nope", &env("unknown").1).is_err());
        assert_eq!(disk.to_toml(), before);
    }
}
