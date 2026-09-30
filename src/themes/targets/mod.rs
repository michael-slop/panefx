//! Everything outside panefx that a theme recolours -- one module per app.
//!
//! Each target says honestly what happened: applied live, applies on the app's
//! next start, deferred (the app is running and would overwrite the change), not
//! installed, or failed. "Not installed" is not an error: a friend without
//! Xournal++ should see it listed and skipped, not a red line.

use super::catalog::Theme;
use super::fsutil::Env;
use serde::{Deserialize, Serialize};

pub mod alacritty;
pub mod editors;
pub mod housemap;
pub mod vscode;
pub mod winmode;
pub mod wterm;
pub mod xournal;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "note", rename_all = "snake_case")]
pub enum Outcome {
    /// Changed, and the app shows it now.
    Live(String),
    /// Changed on disk; the app shows it when it next starts.
    NextStart(String),
    /// Not done YET: the app is running and would overwrite the change. The
    /// daemon retries until it can.
    Deferred(String),
    /// Nothing to do here: not installed, or switched off.
    Skipped(String),
    Failed(String),
}

impl Outcome {
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::Live(_) => "live",
            Outcome::NextStart(_) => "next start",
            Outcome::Deferred(_) => "waiting",
            Outcome::Skipped(_) => "skipped",
            Outcome::Failed(_) => "FAILED",
        }
    }
    pub fn note(&self) -> &str {
        match self {
            Outcome::Live(s)
            | Outcome::NextStart(s)
            | Outcome::Deferred(s)
            | Outcome::Skipped(s)
            | Outcome::Failed(s) => s,
        }
    }
}

pub trait Target {
    /// Stable id, as written in `theme_skip`.
    fn id(&self) -> &'static str;
    /// What a person calls it.
    fn label(&self) -> &'static str;
    /// Record what `apply` is about to change, for the house theme. `None` =
    /// nothing worth keeping.
    fn capture(&self, env: &Env) -> Option<serde_json::Value>;
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome;
    /// Put back what `capture` recorded (`None` if it recorded nothing).
    fn restore(&self, env: &Env, saved: Option<&serde_json::Value>) -> Outcome;
}

/// Every target, in the order a report lists them.
pub fn all() -> Vec<Box<dyn Target>> {
    vec![
        Box::new(alacritty::Alacritty),
        Box::new(editors::Editors),
        Box::new(winmode::WinMode),
        Box::new(xournal::Xournal),
        Box::new(housemap::HouseMap),
        Box::new(wterm::WindowsTerminal),
        Box::new(vscode::VsCode),
    ]
}

pub fn by_id(id: &str) -> Option<Box<dyn Target>> {
    all().into_iter().find(|t| t.id() == id)
}
