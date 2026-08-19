//! Preferences belonging to the GUI itself.
//!
//! Deliberately SEPARATE from `Config`. `config.toml` is the daemon's state --
//! effects, layers, fps, per-monitor params -- and the daemon rewrites that
//! whole file whenever anything saves it. A GUI-only preference like the colour
//! theme has no meaning to the daemon, and putting it there would mean the GUI
//! writing to a file the daemon owns, purely to record which way a button was
//! flipped.
//!
//! So: its own small file next to the config. It is written when a preference
//! changes and read once at startup, and a missing or corrupt file is not an
//! error -- it just means the defaults.

/// What the GUI remembers between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiPrefs {
    /// Dark palette rather than the michaelslop light one.
    pub dark: bool,
    /// Which monitor's detail pane was open. Restored because on a four-monitor
    /// desk, landing on DISPLAY1 every time when you are working on DISPLAY3 is
    /// a small papercut that repeats forever.
    pub selected_monitor: usize,
    /// Which tab was open.
    pub tab: String,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        Self {
            // Light is michaelslop.org's own scheme, so it is the default the
            // design was drawn for.
            dark: false,
            selected_monitor: 0,
            tab: "wallpaper".to_string(),
        }
    }
}

impl GuiPrefs {
    /// `%USERPROFILE%\.config\panefx\gui.toml`, beside the daemon's config.
    pub fn path() -> Option<std::path::PathBuf> {
        std::env::var("USERPROFILE")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".config\\panefx\\gui.toml"))
    }

    /// Read the file, falling back to defaults for anything missing.
    ///
    /// Never fails: a preferences file that cannot be read is not worth
    /// refusing to start over.
    pub fn load() -> Self {
        let Some(p) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(p) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    /// Parse the tiny `key = value` subset this file uses.
    ///
    /// Hand-rolled rather than pulling in a TOML parser: three keys, and the
    /// project's "no new crates" rule holds. Split out from `load` so it is
    /// testable without touching the filesystem.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            match k {
                "dark" => out.dark = v == "true",
                "selected_monitor" => {
                    if let Ok(n) = v.parse() {
                        out.selected_monitor = n;
                    }
                }
                "tab" => out.tab = v.to_string(),
                _ => {}
            }
        }
        out
    }

    /// Render to the file's text form.
    pub fn to_toml(&self) -> String {
        format!(
            "# panefx GUI preferences. Written by panefx-gui when you change one.\n\
             #\n\
             # Separate from config.toml on purpose: that file is the DAEMON's\n\
             # state and the daemon rewrites all of it on every save. These are\n\
             # the GUI's own, and the daemon neither reads nor cares about them.\n\
             \n\
             # The colour theme: michaelslop light, or the dark scheme.\n\
             dark = {}\n\
             \n\
             # Which monitor's detail pane was open, and which tab.\n\
             selected_monitor = {}\n\
             tab = \"{}\"\n",
            self.dark, self.selected_monitor, self.tab
        )
    }

    /// Write the file, creating the directory if needed. Errors are ignored:
    /// failing to save a preference must never interrupt what the user was
    /// doing.
    pub fn save(&self) {
        let Some(p) = Self::path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.to_toml());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_theme_survives_a_round_trip() {
        // The actual ask: the GUI should come back up in the theme you left it
        // in.
        let mut p = GuiPrefs::default();
        p.dark = true;
        p.selected_monitor = 3;
        p.tab = "logs".to_string();
        assert_eq!(GuiPrefs::parse(&p.to_toml()), p);
    }

    #[test]
    fn light_is_the_default() {
        // michaelslop.org's own scheme, which the design was drawn for.
        assert!(!GuiPrefs::default().dark);
    }

    #[test]
    fn a_missing_or_junk_file_gives_defaults_rather_than_failing() {
        assert_eq!(GuiPrefs::parse(""), GuiPrefs::default());
        assert_eq!(GuiPrefs::parse("!!! not toml at all"), GuiPrefs::default());
        // A partial file keeps the defaults for whatever it does not mention.
        let p = GuiPrefs::parse("dark = true");
        assert!(p.dark);
        assert_eq!(p.selected_monitor, GuiPrefs::default().selected_monitor);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let p = GuiPrefs::parse("# a comment\n\n  dark = true  \n");
        assert!(p.dark);
    }

    #[test]
    fn it_does_not_live_in_the_daemons_config_file() {
        // If these ever collide, the daemon's next save wipes the GUI's
        // preferences -- it rewrites config.toml in full.
        let (a, b) = (GuiPrefs::path(), crate::config::Config::path());
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b);
    }
}
