//! One borderless flame panel, pinned directly behind one Alacritty window.
//!
//! Style choices and why they matter:
//!   * `WS_POPUP`            — no frame, no title bar, no system menu.
//!   * `WS_EX_NOACTIVATE`    — clicking it never steals focus from the
//!                             terminal. Without this the panel would fight
//!                             the user for keyboard focus on every click.
//!   * `WS_EX_TOOLWINDOW`    — keeps it out of the taskbar and alt-tab.
//!
//! The panel is positioned to its terminal's exact rect and re-pinned into the
//! z-order slot *directly behind* that terminal. GlazeWM reasserts z-order on
//! every focus change (see `platform_sync.rs` in the GlazeWM source), so
//! re-pinning is not a one-off — it happens on every reconcile.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowPos, ShowWindow,
    HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

use crate::animation::AsciiAnimation;
use crate::render;

/// Our own window class. Distinctive so a GlazeWM `ignore` rule can target it
/// without catching anything else — note Alacritty itself uses winit's very
/// generic `"Window Class"`, so a vague name here would be a real hazard.
pub const PANEL_CLASS: PCWSTR = w!("PaneFxClass");

pub struct Panel {
    pub hwnd: HWND,
    /// The Alacritty window this panel shadows.
    pub target: HWND,
    pub width: i32,
    pub height: i32,
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // We repaint the entire client area every frame, so letting Windows
        // erase the background first would only add flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            render::paint_cached(hdc, hwnd);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Register the window class once per process.
pub fn register_class() -> anyhow::Result<()> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PANEL_CLASS,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            anyhow::bail!("RegisterClassExW failed: {:?}", windows::core::Error::from_win32());
        }
        Ok(())
    }
}

impl Panel {
    /// Create a panel shadowing `target` at the given rect.
    ///
    /// `x`/`y` are signed and may be negative — a monitor to the left of the
    /// primary yields negative coordinates (observed on pHub: x = -1436).
    pub fn create(target: HWND, x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PANEL_CLASS,
                w!("panefx panel"),
                WS_POPUP,
                x,
                y,
                width.max(1),
                height.max(1),
                None,
                None,
                hinstance,
                None,
            )?;

            // SW_SHOWNOACTIVATE, never SW_SHOW: showing must not pull focus
            // away from whatever the user is typing into.
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

            Ok(Panel {
                hwnd,
                target,
                width: width.max(1),
                height: height.max(1),
            })
        }
    }

    /// Move/resize to match the terminal, and re-pin z-order behind it.
    pub fn reposition(&mut self, x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        let resized = width != self.width || height != self.height;
        self.width = width;
        self.height = height;

        unsafe {
            SetWindowPos(
                self.hwnd,
                // Insert directly AFTER (i.e. behind) the target window.
                self.target,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE,
            )?;
        }

        if resized {
            unsafe {
                let _ = InvalidateRect(self.hwnd, None, false);
            }
        }
        Ok(())
    }

    /// Re-assert the z-order slot without touching position or size.
    ///
    /// Called after every event, because GlazeWM re-orders windows on focus
    /// changes and can otherwise leave our panel in front of, or detached
    /// from, its terminal.
    pub fn pin_behind_target(&self) -> anyhow::Result<()> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                self.target,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )?;
        }
        Ok(())
    }

    /// Drop to the very bottom of the z-order. Used when the target is hidden
    /// (e.g. on another workspace) as a belt-and-braces companion to `hide`.
    pub fn sink(&self) -> anyhow::Result<()> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )?;
        }
        Ok(())
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Repaint from the shared animation.
    pub fn redraw(&self, anim: &dyn AsciiAnimation, cfg: &crate::config::Config) {
        render::draw_animation(self.hwnd, anim, cfg, self.width, self.height);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
