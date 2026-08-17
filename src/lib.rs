//! panefx — animated backdrops pinned behind windows.
//!
//! **What this is:** a borderless window that renders arbitrary content, pinned
//! behind another window, that follows it through a tiling WM's retiles and
//! z-order churn. That follower machinery is content-agnostic and is the
//! valuable part.
//!
//! **ASCII effects are one subsystem, not the definition.** `rain`, `flames`
//! and `fire` all implement [`animation::AsciiAnimation`]; plausible siblings
//! are GIF playback, shaders, video, or live system stats, which would want
//! their own trait rather than being forced through a character grid.
//!
//! Shared as a library so the daemon (`src/main.rs`) and the control TUI
//! (`src/bin/panefx-ctl.rs`) agree on exactly one definition of the wire
//! protocol and the parameter types.

pub mod animation;
pub mod config;
pub mod control;
pub mod fire;
pub mod flames;
pub mod palette;
pub mod rain;
pub mod waves;

// Windows-only: the daemon's panel/render/IPC layer. The TUI does not need it,
// and keeping it out of a non-Windows build would let the effects be tested
// anywhere.
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod ipc;
#[cfg(windows)]
pub mod panel;
#[cfg(windows)]
pub mod render;
#[cfg(windows)]
pub mod wallpaper;
