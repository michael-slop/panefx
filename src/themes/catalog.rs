//! The theme list and the palettes behind it.
//!
//! Ported from color.mesh (`themes.go`), which drives the same list on the Linux
//! laptop through Omarchy. Three layers, lowest first:
//!
//!   1. BUNDLED -- the 27 palettes compiled in from `assets/themes/`, so a
//!      machine that has never seen the laptop still has every theme.
//!   2. `~\.config\omarchy\themes\<id>\colors.toml` -- what the laptop's
//!      `sync-windows.sh` exports. Laid over the bundled copy key by key, so a
//!      palette corrected on the laptop wins here too.
//!   3. `~\.config\panefx\themes\<id>.toml` -- a user's own theme, or an edit to
//!      one of ours. A file here whose id is not in the list is appended to the
//!      menu under "mine".
//!
//! The MENU is `~\.config\color.mesh\presets.conf` when it exists -- the one list
//! color.mesh and amtui already share -- otherwise the bundled copy.
//!
//! The house theme is a menu entry with no palette of its own on Windows: it
//! means "the look you had dialled in before any theme", restored from the
//! snapshot in `house.rs`. Its colours (`slop`) are kept for the preview only.

use crate::palette::Rgb;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One entry in the menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub group: String,
    pub id: String,
    pub name: String,
    /// `key -> "#rrggbb"`, lowercased, straight from colors.toml.
    pub colors: BTreeMap<String, String>,
}

impl Theme {
    pub fn is_house(&self) -> bool {
        is_house(&self.id)
    }

    /// A colour by key, or `None` when it is missing or malformed.
    pub fn rgb(&self, key: &str) -> Option<Rgb> {
        self.colors.get(key).and_then(|v| Rgb::parse_hex(v))
    }

    /// The first of `keys` that the palette carries.
    pub fn first(&self, keys: &[&str]) -> Option<Rgb> {
        keys.iter().find_map(|k| self.rgb(k))
    }

    /// Whether the palette is a light one. Same test as the laptop's flames:
    /// the background's relative luminance above one half.
    pub fn is_light(&self) -> bool {
        self.rgb("background").map(luminance).unwrap_or(0.0) > 0.5
    }

    /// Eight blocks that say what the theme looks like -- the same eight
    /// color.mesh's picker shows.
    pub fn swatches(&self) -> Vec<Rgb> {
        let fg = self.rgb("foreground").unwrap_or(Rgb(0x88, 0x88, 0x88));
        ["background", "color1", "color2", "color3", "color4", "color5", "color6", "accent"]
            .iter()
            .map(|k| self.rgb(k).unwrap_or(fg))
            .collect()
    }
}

/// Relative luminance, 0..1.
pub fn luminance(c: Rgb) -> f64 {
    0.2126 * c.0 as f64 / 255.0 + 0.7152 * c.1 as f64 / 255.0 + 0.0722 * c.2 as f64 / 255.0
}

/// Omarchy names the house theme `aether` while it is applied through Aether's
/// own output folder; color.mesh applies the same palette as `slop`. Both mean
/// house, and so does the word itself.
pub fn is_house(id: &str) -> bool {
    matches!(id, "slop" | "aether" | "house" | "")
}

pub const HOUSE_ID: &str = "slop";

/// The menu as shipped.
pub const BUNDLED_PRESETS: &str = include_str!("../../assets/themes/presets.conf");
/// Theme -> Neovim colorscheme, read by `extras/nvim/zz-colormesh.lua`.
pub const BUNDLED_NEOVIM_CONF: &str = include_str!("../../assets/themes/neovim.conf");

const BUNDLED: &[(&str, &str)] = &[
    ("ayu-mirage", include_str!("../../assets/themes/ayu-mirage.toml")),
    ("catppuccin", include_str!("../../assets/themes/catppuccin.toml")),
    ("catppuccin-latte", include_str!("../../assets/themes/catppuccin-latte.toml")),
    ("dracula", include_str!("../../assets/themes/dracula.toml")),
    ("ethereal", include_str!("../../assets/themes/ethereal.toml")),
    ("everforest", include_str!("../../assets/themes/everforest.toml")),
    ("flexoki-light", include_str!("../../assets/themes/flexoki-light.toml")),
    ("github-dark", include_str!("../../assets/themes/github-dark.toml")),
    ("gruvbox", include_str!("../../assets/themes/gruvbox.toml")),
    ("hackerman", include_str!("../../assets/themes/hackerman.toml")),
    ("kanagawa", include_str!("../../assets/themes/kanagawa.toml")),
    ("lumon", include_str!("../../assets/themes/lumon.toml")),
    ("matte-black", include_str!("../../assets/themes/matte-black.toml")),
    ("miasma", include_str!("../../assets/themes/miasma.toml")),
    ("monokai", include_str!("../../assets/themes/monokai.toml")),
    ("night-owl", include_str!("../../assets/themes/night-owl.toml")),
    ("nord", include_str!("../../assets/themes/nord.toml")),
    ("one-dark", include_str!("../../assets/themes/one-dark.toml")),
    ("osaka-jade", include_str!("../../assets/themes/osaka-jade.toml")),
    ("retro-82", include_str!("../../assets/themes/retro-82.toml")),
    ("ristretto", include_str!("../../assets/themes/ristretto.toml")),
    ("rose-pine", include_str!("../../assets/themes/rose-pine.toml")),
    ("slop", include_str!("../../assets/themes/slop.toml")),
    ("solarized-dark", include_str!("../../assets/themes/solarized-dark.toml")),
    ("tokyo-night", include_str!("../../assets/themes/tokyo-night.toml")),
    ("vantablack", include_str!("../../assets/themes/vantablack.toml")),
    ("white", include_str!("../../assets/themes/white.toml")),
];

/// `group | id | display name` per line; comments and blanks skipped.
pub fn parse_presets(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split('|').map(str::trim).collect();
            (parts.len() == 3 && !parts[1].is_empty())
                .then(|| (parts[0].to_string(), parts[1].to_string(), parts[2].to_string()))
        })
        .collect()
}

/// The flat `key = "#rrggbb"` lines of a colors.toml. Tables and comments are
/// skipped; an inline comment after a value is dropped.
pub fn parse_colors(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let mut v = v.trim();
        if let Some(i) = v.find(" #") {
            if v[..i].matches('"').count() % 2 == 0 {
                v = &v[..i];
            }
        }
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
        out.insert(k.trim().to_string(), v.to_lowercase());
    }
    out
}

/// Where the layers live. A struct so tests can point it at a temp dir.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub omarchy: Option<PathBuf>,
    pub user: Option<PathBuf>,
    pub presets: Option<PathBuf>,
}

impl Dirs {
    pub fn from_env() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        Dirs {
            omarchy: home.as_ref().map(|h| h.join(".config").join("omarchy").join("themes")),
            user: home.as_ref().map(|h| h.join(".config").join("panefx").join("themes")),
            presets: home.map(|h| h.join(".config").join("color.mesh").join("presets.conf")),
        }
    }

    /// No overlays: only what is compiled in.
    pub fn bundled_only() -> Self {
        Dirs { omarchy: None, user: None, presets: None }
    }
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// Every theme in menu order, palettes merged across the three layers. A menu
/// entry with no palette anywhere is left out rather than shown and broken.
pub fn load(dirs: &Dirs) -> Vec<Theme> {
    let menu_text = dirs
        .presets
        .as_deref()
        .and_then(read)
        .filter(|t| !parse_presets(t).is_empty())
        .unwrap_or_else(|| BUNDLED_PRESETS.to_string());
    let mut menu = parse_presets(&menu_text);

    // A user's own theme file with an id the menu does not list joins it.
    if let Some(user) = &dirs.user {
        if let Ok(rd) = std::fs::read_dir(user) {
            let mut extra: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension()? != "toml" {
                        return None;
                    }
                    Some(p.file_stem()?.to_string_lossy().into_owned())
                })
                .filter(|id| !menu.iter().any(|(_, m, _)| m == id))
                .collect();
            extra.sort();
            for id in extra {
                menu.push(("mine".to_string(), id.clone(), id));
            }
        }
    }

    menu.into_iter()
        .filter_map(|(group, id, name)| {
            let mut colors = BTreeMap::new();
            if let Some((_, text)) = BUNDLED.iter().find(|(b, _)| *b == id) {
                colors.extend(parse_colors(text));
            }
            if let Some(o) = &dirs.omarchy {
                if let Some(t) = read(&o.join(&id).join("colors.toml")) {
                    colors.extend(parse_colors(&t));
                }
            }
            if let Some(u) = &dirs.user {
                if let Some(t) = read(&u.join(format!("{id}.toml"))) {
                    colors.extend(parse_colors(&t));
                }
            }
            (!colors.is_empty() || is_house(&id)).then_some(Theme { group, id, name, colors })
        })
        .collect()
}

/// Resolve what a user typed -- an id, `next`, `prev`/`previous`, or `house` --
/// against the menu, given the live theme. `None` for an unknown id.
pub fn resolve<'a>(themes: &'a [Theme], current: &str, wanted: &str) -> Option<&'a Theme> {
    let wanted = wanted.trim().to_lowercase();
    let pos = themes
        .iter()
        .position(|t| t.id == current || (is_house(current) && t.is_house()));
    let step = |d: isize| {
        if themes.is_empty() {
            return None;
        }
        let n = themes.len() as isize;
        let from = pos.map(|p| p as isize).unwrap_or(if d > 0 { -1 } else { 0 });
        themes.get(((from + d).rem_euclid(n)) as usize)
    };
    match wanted.as_str() {
        "next" => step(1),
        "prev" | "previous" => step(-1),
        w if is_house(w) => themes.iter().find(|t| t.is_house()),
        w => themes.iter().find(|t| t.id == w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("panefx-test-catalog-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_bundled_menu_entry_has_a_palette() {
        let themes = load(&Dirs::bundled_only());
        assert_eq!(themes.len(), 27, "the shipped menu is 27 themes");
        for t in &themes {
            assert!(!t.colors.is_empty(), "{} has no palette", t.id);
        }
    }

    /// The Alacritty template reads every one of these; a theme without one
    /// would render a broken file, so the shipped set must carry them all.
    #[test]
    fn every_bundled_palette_carries_the_template_keys() {
        let mut keys: Vec<String> = (0..16).map(|i| format!("color{i}")).collect();
        for k in ["background", "foreground", "cursor", "accent", "selection_background", "selection_foreground"] {
            keys.push(k.into());
        }
        for t in load(&Dirs::bundled_only()) {
            for k in &keys {
                assert!(t.rgb(k).is_some(), "{} lacks {k}", t.id);
            }
        }
    }

    #[test]
    fn a_synced_palette_overrides_the_bundled_one_key_by_key() {
        let d = tmp("overlay");
        std::fs::create_dir_all(d.join("tokyo-night")).unwrap();
        std::fs::write(d.join("tokyo-night").join("colors.toml"), "accent = \"#123456\"\n").unwrap();
        let dirs = Dirs { omarchy: Some(d.clone()), user: None, presets: None };
        let t = load(&dirs).into_iter().find(|t| t.id == "tokyo-night").unwrap();
        assert_eq!(t.colors["accent"], "#123456", "the laptop's correction wins");
        assert_eq!(t.colors["background"], "#1a1b26", "keys it did not carry stay bundled");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_users_own_theme_joins_the_menu() {
        let d = tmp("user");
        std::fs::write(d.join("mine1.toml"), "background = \"#000000\"\naccent = \"#ff0000\"\n").unwrap();
        let dirs = Dirs { omarchy: None, user: Some(d.clone()), presets: None };
        let themes = load(&dirs);
        let t = themes.last().unwrap();
        assert_eq!((t.group.as_str(), t.id.as_str()), ("mine", "mine1"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn next_and_prev_wrap_and_house_resolves_by_any_name() {
        let themes = load(&Dirs::bundled_only());
        let first = &themes[0];
        let last = themes.last().unwrap();
        assert!(first.is_house(), "house leads the menu");
        assert_eq!(resolve(&themes, &last.id, "next").unwrap().id, first.id);
        assert_eq!(resolve(&themes, "slop", "prev").unwrap().id, last.id);
        assert_eq!(resolve(&themes, "", "next").unwrap().id, themes[1].id, "no theme yet = on house");
        for w in ["house", "slop", "aether"] {
            assert!(resolve(&themes, "nord", w).unwrap().is_house());
        }
        assert!(resolve(&themes, "nord", "nope").is_none());
    }

    #[test]
    fn an_inline_comment_is_not_part_of_the_value() {
        let c = parse_colors("accent = \"#abcdef\" # the signature blue\n[section]\nx = \"#000000\"");
        assert_eq!(c["accent"], "#abcdef");
    }

    #[test]
    fn light_themes_are_the_four_light_ones() {
        let light: Vec<String> = load(&Dirs::bundled_only())
            .into_iter()
            .filter(|t| t.is_light())
            .map(|t| t.id)
            .collect();
        assert_eq!(light, ["rose-pine", "catppuccin-latte", "flexoki-light", "white"]);
    }
}
