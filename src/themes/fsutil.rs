//! File plumbing every theme target shares.

use std::path::{Path, PathBuf};

/// Replace a file whole: temp file beside it, then rename. An app watching the
/// file never reads half a config. Plain bytes -- no BOM, which TOML, YAML and
/// JSON parsers reject (PowerShell 5's default encoding is how that usually
/// happens).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".panefx-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Bump a file's modified time, so a watcher that only looks at one file
/// notices a change made to a file it imports.
pub fn touch(path: &Path) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.set_modified(std::time::SystemTime::now())
}

/// The folders a theme writes into. A struct rather than `env::var` calls
/// scattered through the targets, so every test runs against a temp dir and
/// none can touch the real profile.
#[derive(Debug, Clone)]
pub struct Env {
    /// `%USERPROFILE%`
    pub home: PathBuf,
    /// `%APPDATA%` (Roaming)
    pub appdata: PathBuf,
    /// `%LOCALAPPDATA%`
    pub localappdata: PathBuf,
    /// `%XDG_CONFIG_HOME%`, which GLib (and so Xournal++) honours on Windows.
    pub xdg_config: Option<PathBuf>,
    /// Whether this is the real profile. Registry and process checks only run
    /// when it is; a test Env never reaches the live system.
    pub live: bool,
    /// Process stems a test pretends are running. Empty on the live system,
    /// which asks Windows instead.
    pub fake_running: Vec<String>,
}

impl Env {
    pub fn from_system() -> Option<Env> {
        let home = PathBuf::from(std::env::var_os("USERPROFILE")?);
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let localappdata = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Some(Env { home, appdata, localappdata, xdg_config, live: true, fake_running: Vec::new() })
    }

    /// Everything under one temp root, for tests.
    pub fn under(root: &Path) -> Env {
        Env {
            home: root.join("home"),
            appdata: root.join("roaming"),
            localappdata: root.join("local"),
            xdg_config: None,
            live: false,
            fake_running: Vec::new(),
        }
    }

    /// Whether a process with this stem (`"sl0p.ink"`) is running.
    pub fn is_running(&self, stem: &str) -> bool {
        if self.fake_running.iter().any(|s| s == stem) {
            return true;
        }
        self.live && crate::proc_name::map().values().any(|s| s == stem)
    }

    /// `~\.config\panefx`, where the house snapshot and generated theme files live.
    pub fn panefx_dir(&self) -> PathBuf {
        self.home.join(".config").join("panefx")
    }

    /// GLib's user config dir: `XDG_CONFIG_HOME` when set, else `%LOCALAPPDATA%`.
    pub fn glib_config_dir(&self) -> PathBuf {
        self.xdg_config.clone().unwrap_or_else(|| self.localappdata.clone())
    }
}
