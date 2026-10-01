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

// In-memory log the TUI can read. The daemon has no console, so without
// this every failure is invisible -- which cost real debugging time.
pub mod log;

pub mod animation;
pub mod compositor;
pub mod config;
// pid -> executable stem, extracted from the two copies that used to live in
// desktop.rs and term_opacity.rs.
pub mod proc_name;
pub mod control;
// One daemon per logon session. Lives in the LIB, not main.rs, so the guard and
// its "a duplicate was refused" reporting can be unit-tested and reused.
pub mod single_instance;
// Where window geometry comes from: GlazeWM, or Win32 directly.
pub mod window_source;
#[cfg(windows)]
pub mod native_windows;
// Not `#[cfg(windows)]`: `config` needs the MIN/MAX percent constants to
// validate its `opacity` field, and that runs everywhere.
pub mod opacity;
// Background opacity, driven through the terminals' own per-pixel alpha.
pub mod term_opacity;
// The Neovide half of that: it has no config-file key, so its opacity is set
// over the embedded Neovim's RPC pipe.
pub mod neovide_opacity;
pub mod fire;
pub mod flames;
pub mod palette;
pub mod rain;
pub mod plasma;
pub mod spin3d;
pub mod skull_art;
pub mod skullspin;
pub mod starfield;
pub mod tunnel;
pub mod waves;
pub mod gui_prefs;
pub mod setup;
pub mod theme_gen;
// Colour themes: one choice, recoloured everywhere panefx can reach -- its own
// effects and GUI, and every app in `themes::targets`. color.mesh's Windows half.
pub mod themes;
pub mod win98;

// Windows-only: the daemon's panel/render/IPC layer. The TUI does not need it,
// and keeping it out of a non-Windows build would let the effects be tested
// anywhere.
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod icon_art;
pub mod ipc;
#[cfg(windows)]
pub mod panel;
#[cfg(windows)]
pub mod render;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod wallpaper;
