//! Windows' own light/dark mode, following the theme's background.
//!
//! Two DWORDs under `HKCU\...\Themes\Personalize` -- `AppsUseLightTheme` (apps,
//! Explorer, Settings) and `SystemUsesLightTheme` (taskbar, Start) -- then a
//! `WM_SETTINGCHANGE "ImmersiveColorSet"` broadcast, which is what the Settings
//! app itself sends, so open windows repaint without a sign-out.
//!
//! Only the MODE follows. Windows offers one accent colour and applies it
//! unreliably until Explorer restarts, so the accent is left alone.
//! House puts back whatever the two values were.

use super::{Outcome, Target};
use crate::themes::catalog::Theme;
use crate::themes::fsutil::Env;

pub struct WinMode;

const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const APPS: &str = "AppsUseLightTheme";
const SYSTEM: &str = "SystemUsesLightTheme";

/// (apps, system) for a theme: both light on a light theme, both dark otherwise.
pub fn desired(theme: &Theme) -> (u32, u32) {
    let v = theme.is_light() as u32;
    (v, v)
}

#[cfg(windows)]
mod reg {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Registry::{
        RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    pub fn get(name: &str) -> Option<u32> {
        let mut v: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(super::KEY),
                &HSTRING::from(name),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        rc.is_ok().then_some(v)
    }

    pub fn set(name: &str, v: u32) -> Result<(), String> {
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(super::KEY),
                &HSTRING::from(name),
                REG_DWORD.0,
                Some(&v as *const u32 as *const _),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if rc.is_ok() {
            Ok(())
        } else {
            Err(format!("{name}: {rc:?}"))
        }
    }

    /// Tell every top-level window the colour set changed.
    ///
    /// On its own thread: a broadcast waits on each window in turn (up to the
    /// timeout for a slow one), and the caller is the daemon's frame loop --
    /// the wallpaper must not stall while Windows repaints its apps.
    pub fn broadcast() {
        std::thread::spawn(|| {
            let what = HSTRING::from("ImmersiveColorSet");
            unsafe {
                let _ = SendMessageTimeoutW(
                    HWND(0xffff as *mut _), // HWND_BROADCAST
                    WM_SETTINGCHANGE,
                    WPARAM(0),
                    LPARAM(what.as_ptr() as isize),
                    SMTO_ABORTIFHUNG,
                    500,
                    None,
                );
            }
        });
    }
}

#[cfg(not(windows))]
mod reg {
    pub fn get(_: &str) -> Option<u32> {
        None
    }
    pub fn set(_: &str, _: u32) -> Result<(), String> {
        Err("not Windows".into())
    }
    pub fn broadcast() {}
}

fn set_both(apps: u32, system: u32) -> Outcome {
    let before = (reg::get(APPS), reg::get(SYSTEM));
    if before == (Some(apps), Some(system)) {
        return Outcome::Live(format!("already {}", if apps == 1 { "light" } else { "dark" }));
    }
    if let Err(e) = reg::set(APPS, apps).and_then(|_| reg::set(SYSTEM, system)) {
        return Outcome::Failed(e);
    }
    reg::broadcast();
    Outcome::Live(format!(
        "apps {} / taskbar {}",
        if apps == 1 { "light" } else { "dark" },
        if system == 1 { "light" } else { "dark" }
    ))
}

impl Target for WinMode {
    fn id(&self) -> &'static str {
        "winmode"
    }
    fn label(&self) -> &'static str {
        "Windows light/dark mode"
    }
    fn capture(&self, env: &Env) -> Option<serde_json::Value> {
        if !env.live {
            return None;
        }
        Some(serde_json::json!({ "apps": reg::get(APPS), "system": reg::get(SYSTEM) }))
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        if !env.live {
            return Outcome::Skipped("not the live system".into());
        }
        let (a, s) = desired(theme);
        set_both(a, s)
    }
    fn restore(&self, env: &Env, saved: Option<&serde_json::Value>) -> Outcome {
        if !env.live {
            return Outcome::Skipped("not the live system".into());
        }
        let get = |k: &str| saved.and_then(|v| v.get(k)).and_then(|v| v.as_u64()).map(|v| v as u32);
        match (get("apps"), get("system")) {
            (Some(a), Some(s)) => set_both(a, s),
            _ => Outcome::Skipped("nothing recorded to put back".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{load, Dirs};

    #[test]
    fn light_themes_ask_for_light_and_dark_for_dark() {
        let all = load(&Dirs::bundled_only());
        let get = |id: &str| all.iter().find(|t| t.id == id).unwrap();
        assert_eq!(desired(get("catppuccin-latte")), (1, 1));
        assert_eq!(desired(get("tokyo-night")), (0, 0));
    }

    /// A test Env must never reach the registry.
    #[test]
    fn a_test_env_never_touches_the_registry() {
        let env = Env::under(&std::env::temp_dir().join("panefx-test-winmode"));
        let t = load(&Dirs::bundled_only()).into_iter().find(|t| t.id == "white").unwrap();
        assert!(matches!(WinMode.apply(&env, &t), Outcome::Skipped(_)));
        assert!(WinMode.capture(&env).is_none());
    }
}
