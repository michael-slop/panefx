//! Windows Terminal: a colour scheme named `panefx`, delivered as a FRAGMENT.
//!
//! Terminal's own `settings.json` is JSONC it rewrites itself, and a profile's
//! scheme lives three levels deep in it. Fragments are Terminal's supported way
//! for another program to add schemes: a JSON file under
//! `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\<app>\`, merged in when
//! Terminal loads its settings. So panefx never touches the user's file: it
//! keeps one scheme called `panefx` up to date, and choosing that scheme for a
//! profile (once) makes the profile follow every theme change.
//!
//! House sets the scheme to the house palette rather than deleting it -- a
//! profile pointing at a scheme that vanished falls back with a warning.

use super::{Outcome, Target};
use crate::themes::catalog::{self, Dirs, Theme};
use crate::themes::fsutil::{write_atomic, Env};
use std::path::PathBuf;

pub struct WindowsTerminal;

const PACKAGES: &[&str] = &[
    "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
    "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
];

fn installed(env: &Env) -> bool {
    PACKAGES.iter().any(|p| env.localappdata.join("Packages").join(p).is_dir())
        || env.localappdata.join("Microsoft").join("Windows Terminal").join("settings.json").exists()
}

fn fragment(env: &Env) -> PathBuf {
    env.localappdata
        .join("Microsoft")
        .join("Windows Terminal")
        .join("Fragments")
        .join("panefx")
        .join("panefx.json")
}

pub fn scheme(t: &Theme) -> Option<String> {
    let names = [
        "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
    ];
    let mut m = serde_json::Map::new();
    m.insert("name".into(), "panefx".into());
    for (k, src) in [
        ("background", "background"),
        ("foreground", "foreground"),
        ("cursorColor", "cursor"),
        ("selectionBackground", "selection_background"),
    ] {
        m.insert(k.into(), t.rgb(src)?.to_hex().into());
    }
    for (i, n) in names.iter().enumerate() {
        m.insert((*n).into(), t.rgb(&format!("color{i}"))?.to_hex().into());
        let bright = format!("bright{}{}", n[..1].to_uppercase(), &n[1..]);
        m.insert(bright, t.rgb(&format!("color{}", i + 8))?.to_hex().into());
    }
    let doc = serde_json::json!({ "schemes": [serde_json::Value::Object(m)] });
    serde_json::to_string_pretty(&doc).ok()
}

fn write(env: &Env, t: &Theme) -> Outcome {
    if !installed(env) {
        return Outcome::Skipped("not installed".into());
    }
    let Some(json) = scheme(t) else {
        return Outcome::Failed(format!("{} lacks a terminal colour", t.id));
    };
    match write_atomic(&fragment(env), json.as_bytes()) {
        Ok(()) => Outcome::NextStart(
            "scheme `panefx` updated; a profile using it follows on Terminal's next start".into(),
        ),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

impl Target for WindowsTerminal {
    fn id(&self) -> &'static str {
        "wterm"
    }
    fn label(&self) -> &'static str {
        "Windows Terminal"
    }
    fn capture(&self, _env: &Env) -> Option<serde_json::Value> {
        None
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        write(env, theme)
    }
    fn restore(&self, env: &Env, _saved: Option<&serde_json::Value>) -> Outcome {
        match catalog::load(&Dirs::bundled_only()).into_iter().find(|t| t.is_house()) {
            Some(house) => write(env, &house),
            None => Outcome::Skipped("no house palette".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_makes_a_complete_scheme() {
        for t in catalog::load(&Dirs::bundled_only()) {
            let s = scheme(&t).unwrap_or_else(|| panic!("{}", t.id));
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["schemes"][0].as_object().unwrap().len(), 21, "{}", t.id);
            assert!(v["schemes"][0]["brightPurple"].is_string());
        }
    }

    #[test]
    fn nothing_is_written_without_terminal() {
        let root = std::env::temp_dir().join("panefx-test-wterm");
        let _ = std::fs::remove_dir_all(&root);
        let env = Env::under(&root);
        let t = catalog::load(&Dirs::bundled_only()).into_iter().nth(1).unwrap();
        assert!(matches!(WindowsTerminal.apply(&env, &t), Outcome::Skipped(_)));
        assert!(!fragment(&env).exists());
    }
}
