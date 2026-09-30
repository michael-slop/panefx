//! The house theme: the look that was dialled in by hand before any theme.
//!
//! color.mesh's Windows backend restored a hard-coded five-colour fire for
//! house, into every flames section it found. On pHub the shared wallpaper
//! block was a hand-dialled purple, so the first "back to house" would have
//! painted it green. Michael, 2026-09-30: *"that house theme is the hand dialed
//! theme i want."*
//!
//! So house has no palette. The first theme applied FROM house records every
//! value it is about to change -- panefx's effect colours and each other app's
//! -- into `~\.config\panefx\house.json`, and switching back to house puts
//! those values back. Theme to theme never re-records: the snapshot is always
//! the hand-dialled look, however many themes were tried in between.

use super::effects::Saved;
use super::fsutil::write_atomic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// When it was taken, for the curious reading the file.
    #[serde(default)]
    pub taken: String,
    /// panefx's own effect colours.
    #[serde(default)]
    pub effects: Vec<Saved>,
    /// Each other target's own record, by target id. Absent = that target had
    /// nothing to keep.
    #[serde(default)]
    pub targets: BTreeMap<String, serde_json::Value>,
}

pub fn path(panefx_dir: &Path) -> PathBuf {
    panefx_dir.join("house.json")
}

pub fn load(panefx_dir: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path(panefx_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(panefx_dir: &Path, snap: &Snapshot) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(snap).map_err(std::io::Error::other)?;
    write_atomic(&path(panefx_dir), text.as_bytes())
}
