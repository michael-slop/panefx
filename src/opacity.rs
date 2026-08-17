//! Window opacity, applied from outside the window's own process.
//!
//! # Why this exists rather than using the apps' own settings
//!
//! Every terminal has its own opacity setting, and none of them can be driven
//! live on Windows:
//!
//! * **Alacritty** (`window.opacity`) renders its own alpha through winit ->
//!   `DwmEnableBlurBehindWindow`. `live_config_reload` picks up a changed value,
//!   but that means rewriting a file panefx does not own. Its `ipc_socket` is
//!   documented "(unix only)" — on Windows `alacritty.exe` exposes only
//!   `migrate` and `help`, so there is no message channel at all.
//! * **Neovide** has NO `transparency` or `opacity` config-file key. The real
//!   knob is the nvim global `g:neovide_opacity` (renamed from
//!   `g:neovide_transparency` in 0.15.0), which would mean driving nvim's RPC
//!   over a named pipe.
//!
//! Layered-window alpha is applied by the compositor from *outside*, works on
//! any `HWND` regardless of whether the app knows about opacity, takes effect
//! immediately, and needs no cooperation from the target. It is the only
//! mechanism that is uniform across every window panefx draws behind.
//!
//! This is the same pair of calls GlazeWM's `set-transparency` uses — see
//! `native_window.rs::set_transparency` in the GlazeWM source.
//!
//! # The multiply trap
//!
//! Layered alpha does not REPLACE an app's own alpha, it **stacks** with it.
//! `SetLayeredWindowAttributes` applies a uniform `SourceConstantAlpha` on top
//! of whatever per-pixel alpha the app already drew, so an Alacritty at
//! `window.opacity = 0.6` layered at 60% renders at roughly **36%**.
//!
//! For this slider to be linear, the apps' own opacity must be 1.0 and panefx
//! must own the whole range. `alacritty.toml` carries a comment saying so.

// Only the Win32 calls are Windows-only. The percent/alpha maths and the
// MIN/MAX constants are used by `config`, which builds everywhere.
#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetLayeredWindowAttributes, GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW,
    GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED,
};

/// Lowest opacity the UI will allow.
///
/// NOT zero. Microsoft's docs note a fully transparent window is also
/// unfocusable — an easy way to lose a terminal with no way to click it back.
pub const MIN_PERCENT: u8 = 10;
pub const MAX_PERCENT: u8 = 100;

/// Percent (10..=100) to the 0..=255 alpha `SetLayeredWindowAttributes` wants.
///
/// Rounds rather than truncates, so 60% is 153 and not 152.
pub fn percent_to_alpha(percent: u8) -> u8 {
    let p = percent.clamp(MIN_PERCENT, MAX_PERCENT) as f32;
    (p / 100.0 * 255.0).round() as u8
}

/// Inverse of [`percent_to_alpha`], for reading a window's current state back.
pub fn alpha_to_percent(alpha: u8) -> u8 {
    ((alpha as f32 / 255.0) * 100.0).round() as u8
}

/// Apply an opacity to one window.
///
/// Idempotent: setting the same alpha twice is a no-op as far as the user can
/// see, so callers may re-apply freely (the caller still caches to avoid the
/// syscall).
#[cfg(windows)]
pub fn set_opacity(hwnd: HWND, percent: u8) -> anyhow::Result<()> {
    let alpha = percent_to_alpha(percent);
    unsafe {
        // WS_EX_LAYERED can be added after creation — explicitly sanctioned by
        // the docs, and what GlazeWM does. Read-modify-write so no other
        // ex-style bit is clobbered.
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = style | (WS_EX_LAYERED.0 as isize);
        if style != wanted {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        }
        SetLayeredWindowAttributes(hwnd, None, alpha, LWA_ALPHA)?;
    }
    Ok(())
}

/// Read a window's current opacity as a percentage.
///
/// Returns `None` when the window has never been layered — which is the normal
/// state for a freshly opened terminal, NOT an error. `GetLayeredWindowAttributes`
/// fails outright in that case, so a failure must be read as "fully opaque"
/// rather than propagated.
#[cfg(windows)]
pub fn get_opacity(hwnd: HWND) -> Option<u8> {
    unsafe {
        let mut alpha = 0u8;
        let mut flags = Default::default();
        GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags))
            .ok()
            .map(|()| alpha_to_percent(alpha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_to_alpha_is_linear() {
        assert_eq!(percent_to_alpha(100), 255);
        assert_eq!(percent_to_alpha(60), 153);
        assert_eq!(percent_to_alpha(50), 128);
        assert_eq!(percent_to_alpha(10), 26);
    }

    #[test]
    fn percent_is_clamped_to_a_focusable_range() {
        // 0% would make the window unfocusable — you could not click it back.
        assert_eq!(percent_to_alpha(0), percent_to_alpha(MIN_PERCENT));
        assert_eq!(percent_to_alpha(200), 255);
    }

    #[test]
    fn alpha_round_trips_close_enough_to_show_in_a_ui() {
        // Not exact — 255 steps do not divide into 100 evenly — but a value set
        // from the TUI must read back as the same number the TUI shows, or the
        // slider appears to drift every time it is redrawn.
        for p in MIN_PERCENT..=MAX_PERCENT {
            let back = alpha_to_percent(percent_to_alpha(p));
            assert_eq!(back, p, "{p}% round-tripped to {back}%");
        }
    }
}
