//! Neovim, Neovide and nvs-ide.
//!
//! Every editor config that carries `extras/nvim/zz-colormesh.lua` watches
//! `~\.config\color.mesh\current` and switches to that theme's real Neovim
//! colorscheme the moment it changes -- running editors included. The file is
//! the contract color.mesh set up on the Linux laptop (an Omarchy hook writes it
//! there), so the same plugin works on both systems. This target writes it, and
//! puts the theme -> colorscheme map (`neovim.conf`) beside it if there is none.

use super::{Outcome, Target};
use crate::themes::catalog::{Theme, BUNDLED_NEOVIM_CONF, HOUSE_ID};
use crate::themes::fsutil::{write_atomic, Env};
use std::path::PathBuf;

pub struct Editors;

fn dir(env: &Env) -> PathBuf {
    env.home.join(".config").join("color.mesh")
}

fn write(env: &Env, id: &str) -> Outcome {
    let d = dir(env);
    let map = d.join("neovim.conf");
    if !map.exists() {
        if let Err(e) = write_atomic(&map, BUNDLED_NEOVIM_CONF.as_bytes()) {
            return Outcome::Failed(format!("neovim.conf: {e}"));
        }
    }
    match write_atomic(&d.join("current"), format!("{id}\n").as_bytes()) {
        Ok(()) => Outcome::Live(format!(
            "`current` = {id}; editors with zz-colormesh.lua switch now"
        )),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

impl Target for Editors {
    fn id(&self) -> &'static str {
        "editors"
    }
    fn label(&self) -> &'static str {
        "Neovim / Neovide / nvs-ide"
    }
    fn capture(&self, _env: &Env) -> Option<serde_json::Value> {
        None
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        write(env, &theme.id)
    }
    fn restore(&self, env: &Env, _saved: Option<&serde_json::Value>) -> Outcome {
        // `slop` maps to `house` in neovim.conf: each editor back on its own scheme.
        write(env, HOUSE_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{load, Dirs};

    #[test]
    fn current_names_the_theme_and_the_map_is_not_overwritten() {
        let root = std::env::temp_dir().join("panefx-test-editors");
        let _ = std::fs::remove_dir_all(&root);
        let env = Env::under(&root);
        std::fs::create_dir_all(dir(&env)).unwrap();
        std::fs::write(dir(&env).join("neovim.conf"), "mine\n").unwrap();
        let t = load(&Dirs::bundled_only()).into_iter().find(|t| t.id == "gruvbox").unwrap();
        Editors.apply(&env, &t);
        assert_eq!(std::fs::read_to_string(dir(&env).join("current")).unwrap(), "gruvbox\n");
        assert_eq!(std::fs::read_to_string(dir(&env).join("neovim.conf")).unwrap(), "mine\n");
        Editors.restore(&env, None);
        assert_eq!(std::fs::read_to_string(dir(&env).join("current")).unwrap(), "slop\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
