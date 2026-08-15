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
    BeginPaint, DeleteDC, DeleteObject, EndPaint, InvalidateRect, SelectObject, HBRUSH, HDC,
    HBITMAP, HFONT, HGDIOBJ, PAINTSTRUCT,
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

/// GDI objects reused across frames.
///
/// Before this existed, `draw_animation` created a memory DC, a full-window
/// compatible bitmap, a solid brush and a font — plus two heap allocations —
/// **on every frame, for every panel**. `CreateFontW` in particular runs the
/// font mapper. Measured cost of that churn: `flames` has an essentially free
/// simulation (one integer pass, no transcendentals, no allocations) yet still
/// burned ~17% of a core, so the overhead was almost entirely here and every
/// effect paid it.
///
/// Each field records what it was built for, so it can be rebuilt when that
/// input changes and reused otherwise.
pub struct GdiCache {
    /// Memory DC. Valid for the panel's lifetime.
    pub mem_dc: HDC,
    /// Off-screen bitmap; must be rebuilt when the panel resizes.
    pub bitmap: HBITMAP,
    pub bmp_w: i32,
    pub bmp_h: i32,
    /// Whatever was selected into `mem_dc` before our bitmap — must be
    /// restored before the DC is deleted or GDI leaks the original.
    pub old_bmp: HGDIOBJ,

    /// Font, plus the inputs it was created from.
    pub font: HFONT,
    pub font_face: String,
    pub font_h: i32,
    pub old_font: HGDIOBJ,

    /// Background brush, plus the colour it was created for.
    pub brush: HBRUSH,
    pub brush_colour: u32,

    /// Scratch buffer for the current glyph run. Reused, never reallocated.
    pub run: Vec<u16>,
    /// Per-character advances for `ExtTextOutW`, parallel to `run`.
    pub dx: Vec<i32>,
    /// One row of (glyph, colour), so `cell_at` runs once per cell rather than
    /// once per colour bucket.
    pub row_cells: Vec<Option<(char, u32)>>,
    /// Font name as a NUL-terminated UTF-16 buffer, kept alive for CreateFontW.
    pub face_utf16: Vec<u16>,
}

pub struct Panel {
    pub hwnd: HWND,
    /// The Alacritty window this panel shadows.
    pub target: HWND,
    pub width: i32,
    pub height: i32,
    /// Whether the shadowed terminal is currently displayed. A hidden panel is
    /// not drawn at all — nobody can see it.
    pub visible: bool,
    /// Built lazily on the first draw, then reused.
    pub gdi: Option<GdiCache>,
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
                visible: true,
                gdi: None,
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

    pub fn show(&mut self) {
        self.visible = true;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Repaint from the shared animation.
    pub fn redraw(&mut self, anim: &dyn AsciiAnimation, cfg: &crate::config::Config) {
        render::draw_animation(self, anim, cfg);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            // Free the cached GDI objects BEFORE destroying the window.
            //
            // Every panel holds a full-window bitmap; leaking one per panel is
            // worse than the per-frame cost this cache removes. The originals
            // must be selected back into the DC first — deleting a DC while our
            // objects are still selected leaks whatever GDI had there before.
            if let Some(g) = self.gdi.take() {
                if !g.old_font.is_invalid() {
                    SelectObject(g.mem_dc, g.old_font);
                }
                if !g.font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(g.font.0));
                }
                if !g.old_bmp.is_invalid() {
                    SelectObject(g.mem_dc, g.old_bmp);
                }
                if !g.bitmap.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(g.bitmap.0));
                }
                if !g.brush.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(g.brush.0));
                }
                if !g.mem_dc.is_invalid() {
                    let _ = DeleteDC(g.mem_dc);
                }
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
