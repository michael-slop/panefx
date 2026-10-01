//! Where window geometry comes from.
//!
//! panefx was built against GlazeWM's IPC and grew a hard dependency on it: no
//! WM meant no pane backdrops at all, and the daemon could not be given to
//! anyone who did not already run GlazeWM. The wallpaper never needed it — it
//! uses `desktop::enumerate_monitors` and the WorkerW directly — so the
//! dependency was always narrower than it looked.
//!
//! This is the seam. Everything downstream consumes `Vec<ipc::Window>` and four
//! predicates on it; nothing knows or cares which backend produced them.
//!
//! # Which backend, and why auto is the default
//!
//! GlazeWM is tried FIRST and kept when present. It is not a legacy path: it
//! knows about workspaces, which Win32 has no concept of, and it is the
//! configuration this has been tested against for months. The native source is
//! what makes the WM optional rather than required.
//!
//! The choice is always logged, because "which backend am I on" is the first
//! question worth answering when the panes look wrong, and guessing it from
//! behaviour is exactly the kind of invisible state that has cost time here
//! before.

use crate::ipc;

/// A source of window geometry.
///
/// Mirrors the surface `main.rs` already used on `IpcThread`, so adopting it
/// changed no call sites.
pub trait WindowSource {
    /// Non-blocking. `None` means "nothing new this frame".
    fn try_recv(&self) -> Option<ipc::IpcMessage>;
    /// Ask for a fresh window list.
    fn request_windows(&self) -> anyhow::Result<()>;
    /// For the log line. `"glazewm"` or `"native"`.
    fn name(&self) -> &'static str;
}

/// What the config asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// GlazeWM if it answers, native otherwise.
    Auto,
    /// GlazeWM only. Fails loudly rather than silently degrading — if someone
    /// has said "use my WM", quietly not using it is the wrong answer.
    Glazewm,
    /// Native only, even with a WM running. Mostly for testing the fallback on
    /// a machine that does have GlazeWM.
    Native,
}

impl Preference {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "auto" | "" => Some(Preference::Auto),
            "glazewm" | "glaze" => Some(Preference::Glazewm),
            "native" | "win32" => Some(Preference::Native),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Preference::Auto => "auto",
            Preference::Glazewm => "glazewm",
            Preference::Native => "native",
        }
    }
}

/// GlazeWM's IPC, wrapped.
pub struct GlazeSource(ipc::IpcThread);

impl WindowSource for GlazeSource {
    fn try_recv(&self) -> Option<ipc::IpcMessage> {
        self.0.try_recv()
    }
    fn request_windows(&self) -> anyhow::Result<()> {
        self.0.request_windows()
    }
    fn name(&self) -> &'static str {
        "glazewm"
    }
}

/// Win32, with no WM.
pub struct NativeSource;

impl WindowSource for NativeSource {
    fn try_recv(&self) -> Option<ipc::IpcMessage> {
        // Coalesced: the WinEvent callback only sets a flag, however many
        // events arrived. A drag fires EVENT_OBJECT_LOCATIONCHANGE per pixel,
        // and enumerating per event would be far worse than the 500ms polling
        // this replaces.
        if crate::native_windows::take_dirty() {
            Some(ipc::IpcMessage::Windows(crate::native_windows::enumerate()))
        } else {
            None
        }
    }
    fn request_windows(&self) -> anyhow::Result<()> {
        crate::native_windows::mark_dirty();
        Ok(())
    }
    fn name(&self) -> &'static str {
        "native"
    }
}

/// Pick a backend according to `pref`, and say which one won.
/// Which backend is live, for the snapshot (`"glazewm"` / `"native"`).
static ACTIVE: std::sync::Mutex<&'static str> = std::sync::Mutex::new("");

pub fn active() -> &'static str {
    ACTIVE.lock().map(|a| *a).unwrap_or("")
}

pub fn set_active(name: &'static str) {
    if let Ok(mut a) = ACTIVE.lock() {
        *a = name;
    }
}

/// GlazeWM, if it answers right now. For picking it back up after it went away.
pub fn try_glazewm() -> Option<Box<dyn WindowSource>> {
    ipc::IpcThread::spawn().ok().map(|t| Box::new(GlazeSource(t)) as Box<dyn WindowSource>)
}

pub fn connect(pref: Preference) -> anyhow::Result<Box<dyn WindowSource>> {
    let s = connect_inner(pref)?;
    set_active(s.name());
    Ok(s)
}

fn connect_inner(pref: Preference) -> anyhow::Result<Box<dyn WindowSource>> {
    match pref {
        Preference::Native => {
            crate::log_info!("[panefx] window source: native (Win32), by configuration");
            Ok(Box::new(NativeSource))
        }
        Preference::Glazewm => match ipc::IpcThread::spawn() {
            Ok(t) => {
                crate::log_info!("[panefx] window source: glazewm, by configuration");
                Ok(Box::new(GlazeSource(t)))
            }
            // Deliberately fatal. `window_source = "glazewm"` is an explicit
            // instruction, and silently using the other backend would make the
            // setting a lie.
            Err(e) => Err(anyhow::anyhow!(
                "window_source is 'glazewm' but GlazeWM is not reachable at {}: {e}",
                ipc::IPC_URL
            )),
        },
        Preference::Auto => match ipc::IpcThread::spawn() {
            Ok(t) => {
                crate::log_info!("[panefx] window source: glazewm");
                Ok(Box::new(GlazeSource(t)))
            }
            Err(e) => {
                // INFO, not WARN: with no WM installed this is the normal,
                // expected path, and crying wolf about it would train the log
                // to be ignored.
                crate::log_info!(
                    "[panefx] window source: native (Win32) -- GlazeWM not reachable ({e})"
                );
                Ok(Box::new(NativeSource))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_round_trips() {
        for p in [Preference::Auto, Preference::Glazewm, Preference::Native] {
            assert_eq!(Preference::parse(p.as_str()), Some(p));
        }
    }

    /// An empty value means "unset", which must mean auto -- not a hard error
    /// that stops the daemon starting.
    #[test]
    fn an_empty_preference_is_auto() {
        assert_eq!(Preference::parse(""), Some(Preference::Auto));
        assert_eq!(Preference::parse("   "), Some(Preference::Auto));
    }

    #[test]
    fn spelling_variants_are_accepted() {
        assert_eq!(Preference::parse("GLAZE"), Some(Preference::Glazewm));
        assert_eq!(Preference::parse("Win32"), Some(Preference::Native));
    }

    /// A typo must be rejected rather than silently treated as auto: a config
    /// saying `natve` should be reported, not quietly ignored.
    #[test]
    fn a_typo_is_rejected() {
        assert_eq!(Preference::parse("natve"), None);
        assert_eq!(Preference::parse("glazewm2"), None);
    }
}
