//! The house map: how sl0p.notepad and sl0p.ink follow the theme.
//!
//! Both are forks whose look is a fixed set of house colours (slop/pkg/theme:
//! void, crypt, stone, bone, spectral, orchid...) compiled into their code and
//! stylesheets. Their patches pass every house colour through ONE table when
//! the app starts -- `~\.config\color.mesh\house-map`, a line per colour,
//! `<house hex> <themed hex>` -- and leave a colour the table does not name
//! alone. For the house theme the table is empty, so they look exactly as built.
//!
//! The contract and the role table are color.mesh's (`housemap.go`, written on
//! the Linux laptop, 2026-09-30); this is a line-for-line port, so both systems
//! hand the apps the same colours. Change the roles THERE first.

use super::{Outcome, Target};
use crate::palette::Rgb;
use crate::themes::catalog::{is_house, Theme};
use crate::themes::derive::mix;
use crate::themes::fsutil::{write_atomic, Env};
use std::path::PathBuf;

pub struct HouseMap;

/// A theme's colours with fallbacks resolved, so a role never meets a missing key.
struct Pal {
    bg: Rgb,
    fg: Rgb,
    accent: Rgb,
    ansi: [Rgb; 16],
}

fn palette_of(t: &Theme) -> Option<Pal> {
    let bg = t.rgb("background")?;
    let fg = t.rgb("foreground")?;
    let accent = t.first(&["accent"]).or_else(|| t.rgb("color4")).unwrap_or(fg);
    let mut ansi = [fg; 16];
    for i in 0..16 {
        // A missing bright slot falls back to its normal twin.
        let base = if i >= 8 { ansi[i - 8] } else { fg };
        ansi[i] = t.rgb(&format!("color{i}")).unwrap_or(base);
    }
    Some(Pal { bg, fg, accent, ansi })
}

type Fill = fn(&Pal) -> Rgb;

/// (house hex, name, fill) -- `houseRoles` in color.mesh's housemap.go.
const ROLES: &[(&str, &str, Fill)] = &[
    ("05070a", "void", |p| p.bg),
    ("070b10", "bevel shadow", |p| mix(p.bg, Rgb(0, 0, 0), 0.3)),
    ("0a0e14", "crypt", |p| mix(p.bg, p.fg, 0.04)),
    ("111823", "crypt hi", |p| mix(p.bg, p.fg, 0.08)),
    ("1a2430", "stone", |p| mix(p.bg, p.fg, 0.12)),
    ("26333f", "stone hi", |p| mix(p.bg, p.fg, 0.20)),
    ("d8d4c4", "bone", |p| p.fg),
    ("d9d4c4", "bone (neutral)", |p| p.fg),
    ("8b8778", "bone dim", |p| mix(p.fg, p.bg, 0.4)),
    ("5c6470", "ash", |p| mix(p.fg, p.bg, 0.6)),
    ("62e670", "spectral", |p| p.accent),
    ("bb9bf7", "orchid", |p| p.ansi[5]),
    ("b8453a", "viscera", |p| p.ansi[1]),
    ("d4a843", "gold", |p| p.ansi[3]),
    ("c8ffd0", "pale", |p| p.ansi[2]),
    ("3d8fa8", "teal", |p| p.ansi[4]),
    ("4fb8d6", "corpse", |p| p.ansi[6]),
    ("4f8ad6", "cobalt", |p| p.ansi[12]),
    ("9d6bd8", "necrotic", |p| p.ansi[13]),
    ("4a3568", "necrotic dark", |p| mix(p.ansi[5], p.bg, 0.45)),
    ("0c1a22", "notice ground", |p| mix(p.bg, p.ansi[4], 0.12)),
    ("1c0e0c", "error ground", |p| mix(p.bg, p.ansi[1], 0.12)),
];

/// The file for a theme. House, or a theme without a usable background and
/// foreground, gets an empty table: the apps then paint exactly as built.
pub fn render(t: &Theme) -> String {
    let mut s = format!(
        "# color.mesh house map for \"{}\" -- <house hex> <themed hex>. Read by sl0p.notepad and\n\
         # sl0p.ink at start; empty = the house look. Rewritten on every theme change.\n",
        t.id
    );
    if is_house(&t.id) {
        return s;
    }
    let Some(p) = palette_of(t) else { return s };
    for (house, name, fill) in ROLES {
        s.push_str(&format!("{house} {}  # {name}\n", &fill(&p).to_hex()[1..]));
    }
    s
}

fn path(env: &Env) -> PathBuf {
    env.home.join(".config").join("color.mesh").join("house-map")
}

/// Either fork present (or a map already there, from color.mesh or a copy
/// built somewhere else). A machine with neither never gets the file.
fn wanted(env: &Env) -> bool {
    path(env).exists()
        || env.home.join("sl0p.notepad").join("sl0p.notepad.exe").exists()
        || env.home.join("sl0p.ink").join("bin").join("sl0p.ink.exe").exists()
}

fn write(env: &Env, t: &Theme) -> Outcome {
    if !wanted(env) {
        return Outcome::Skipped("sl0p.notepad / sl0p.ink not installed".into());
    }
    match write_atomic(&path(env), render(t).as_bytes()) {
        Ok(()) => Outcome::NextStart("house-map written; each app shows it when it next opens".into()),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

impl Target for HouseMap {
    fn id(&self) -> &'static str {
        "housemap"
    }
    fn label(&self) -> &'static str {
        "sl0p.notepad / sl0p.ink"
    }
    fn capture(&self, _env: &Env) -> Option<serde_json::Value> {
        None
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        write(env, theme)
    }
    fn restore(&self, env: &Env, _saved: Option<&serde_json::Value>) -> Outcome {
        let house = Theme {
            group: String::new(),
            id: crate::themes::catalog::HOUSE_ID.into(),
            name: String::new(),
            colors: Default::default(),
        };
        write(env, &house)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{load, Dirs};

    /// What color.mesh (Go) wrote for monokai on the laptop, 2026-09-30 16:07.
    /// The two implementations must hand the apps the same colours.
    const GO_MONOKAI: &str = "05070a 272822  # void
070b10 1b1c18  # bevel shadow
0a0e14 2f302a  # crypt
111823 383933  # crypt hi
1a2430 40413b  # stone
26333f 51524c  # stone hi
d8d4c4 f8f8f2  # bone
d9d4c4 f8f8f2  # bone (neutral)
8b8778 a4a59f  # bone dim
5c6470 7b7b75  # ash
62e670 66d9ef  # spectral
bb9bf7 ae81ff  # orchid
b8453a f92672  # viscera
d4a843 f4bf75  # gold
c8ffd0 a6e22e  # pale
3d8fa8 66d9ef  # teal
4fb8d6 a1efe4  # corpse
4f8ad6 66d9ef  # cobalt
9d6bd8 ae81ff  # necrotic
4a3568 71599c  # necrotic dark
0c1a22 2f3d3b  # notice ground
1c0e0c 40282c  # error ground
";

    #[test]
    fn the_map_matches_color_mesh_line_for_line() {
        let t = load(&Dirs::bundled_only()).into_iter().find(|t| t.id == "monokai").unwrap();
        let out = render(&t);
        let body: String = out.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect();
        assert_eq!(body, GO_MONOKAI);
    }

    #[test]
    fn house_maps_nothing_and_every_theme_maps_every_role() {
        let count = |s: &str| s.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).count();
        for t in load(&Dirs::bundled_only()) {
            let n = count(&render(&t));
            if t.is_house() {
                assert_eq!(n, 0);
            } else {
                assert_eq!(n, ROLES.len(), "{}", t.id);
            }
        }
    }

    #[test]
    fn a_machine_without_either_fork_gets_no_file() {
        let root = std::env::temp_dir().join("panefx-test-housemap");
        let _ = std::fs::remove_dir_all(&root);
        let env = Env::under(&root);
        let t = load(&Dirs::bundled_only()).into_iter().nth(3).unwrap();
        assert!(matches!(HouseMap.apply(&env, &t), Outcome::Skipped(_)));
        assert!(!path(&env).exists());
    }
}
