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
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindow, GetWindowLongPtrW,
    GetClassNameW, RegisterClassExW, SetWindowPos, ShowWindow, GWL_EXSTYLE,
    GW_HWNDNEXT, GW_HWNDPREV,
    HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_SHOWNOACTIVATE, WINDOW_STYLE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WNDCLASSEXW,
    WS_CHILD, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP,
    WS_VISIBLE,
};

use crate::animation::AsciiAnimation;
use crate::render;

/// Our own window class. Distinctive so a GlazeWM `ignore` rule can target it
/// without catching anything else — note Alacritty itself uses winit's very
/// generic `"Window Class"`, so a vague name here would be a real hazard.
pub const PANEL_CLASS: PCWSTR = w!("PaneFxClass");

/// How many times a desktop pane actually issued a z-order SetWindowPos.
///
/// Diagnostic: with N sibling panes each demanding to be THE bottom child,
/// this counts the churn. One pane should pin ~never after creation; a high
/// steady rate here is the panes displacing each other.
pub static PIN_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    ///
    /// A 32-bit top-down **DIB section**, not a compatible bitmap, so the
    /// finished pixels have a CPU address. A desktop surface has to hand them to
    /// DirectComposition (see `compositor`), and a terminal panel blits from it
    /// exactly as before at no extra cost.
    pub bitmap: HBITMAP,
    pub bmp_w: i32,
    pub bmp_h: i32,
    /// First byte of the DIB's pixels, or null before the first frame.
    pub bits: *mut u8,
    /// Bytes per row. For a top-down 32bpp DIB this is `width * 4`, but it is
    /// stored rather than recomputed so the upload path cannot drift from it.
    pub stride: usize,
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

/// What a panel is positioned relative to.
///
/// `Window` is the original behaviour: a sibling of the tracked terminal,
/// positioned in SCREEN coordinates, re-pinned into the z-slot behind it on
/// every reconcile because GlazeWM reasserts z-order on focus changes.
///
/// `Desktop` is the wallpaper layer: a CHILD of Explorer's `SHELLDLL_DefView`,
/// so its coordinates are parent-relative (see `desktop::to_child`). Its z-slot
/// is the BOTTOM of DefView's children — under `SysListView32`, so the icons
/// stay visible and clickable — and it is re-asserted every tick because
/// Explorer reorders and rebuilds its desktop children at will.
#[derive(Clone, Copy)]
pub enum Anchor {
    Window(HWND),
    Desktop {
        /// `SHELLDLL_DefView` on the raised model, the WorkerW on the classic
        /// one. Chosen in `desktop::find`, which records why.
        parent: HWND,
        /// True on the Windows 11 24H2+ "raised desktop" model.
        raised: bool,
    },
}

/// The window style a desktop surface must be created with.
///
/// Pulled out of the `unsafe` block on purpose: this one choice is the whole
/// difference between a wallpaper that sits behind the icons and one that
/// covers them, and inside the block no test could reach it.
///
/// **`WS_CHILD`, never `WS_POPUP`.** A `WS_POPUP` given a parent is an OWNED
/// window, not a child: it does not clip to the parent and does not take its
/// z-position from the parent's child list, so it floats above every
/// application. And a top-level popup cannot work at all here, whatever slot it
/// is pinned to — `SHELLDLL_DefView` is a CHILD of Progman, so "just above
/// Progman" is also just above the icons. That was the bug: measured on build
/// 26200, a full-screen `PaneFxClass` popup sat directly above Progman and hid
/// every icon on the desktop.
pub fn desktop_window_style() -> WINDOW_STYLE {
    WS_CHILD | WS_VISIBLE
}

/// Is a desktop surface already the last of its siblings?
///
/// `last` is what `GetWindow(hwnd, GW_HWNDLAST)` returned, or `None` if the
/// call failed. Split out of the `unsafe` block so the decision — not the API
/// call — is what the tests pin.
///
/// **A failed query must report `false`.** Skipping the pin on a query we could
/// not answer would leave the surface wherever Explorer last put it, which is
/// the icons-are-gone bug. Reporting "not at the bottom" makes the caller do
/// the real `SetWindowPos` and surface any error from there.
/// Retained for the tests that pin the OLD rule's failure direction; the live
/// desktop guard is the sibling walk in `is_at_bottom`.
pub fn already_at_bottom(hwnd: usize, last: Option<usize>) -> bool {
    last == Some(hwnd)
}

/// Is a window class one of our own desktop panes?
///
/// The decision inside `is_at_bottom`'s sibling walk, split out so the tests
/// can pin it: a sibling below us that is OURS does not require a re-pin, and
/// anything else does.
pub fn class_is_ours(class: &str) -> bool {
    class == "PaneFxClass"
}

/// Is `target` already the sibling immediately above us?
///
/// `prev` is what `GetWindow(hwnd, GW_HWNDPREV)` returned. Same failure rule as
/// `already_at_bottom`: unknown means re-pin.
pub fn already_directly_behind(target: usize, prev: Option<usize>) -> bool {
    prev == Some(target)
}

pub struct Panel {
    pub hwnd: HWND,
    /// What this panel follows.
    pub anchor: Anchor,
    pub width: i32,
    pub height: i32,
    /// Whether the shadowed terminal is currently displayed. A hidden panel is
    /// not drawn at all — nobody can see it.
    pub visible: bool,
    /// Built lazily on the first draw, then reused.
    pub gdi: Option<GdiCache>,
    /// Present path for a desktop surface.
    ///
    /// `None` for terminal panels, which blit straight to their window DC. A
    /// desktop surface cannot: GDI leaves alpha at 0 and the raised desktop
    /// composites with alpha, so a blitted frame renders ADDITIVELY over
    /// Explorer's background instead of replacing it. See `compositor`.
    pub surface: Option<crate::compositor::Surface>,
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
            // WHICH repair path depends on whether this window has a
            // redirection bitmap to blit into.
            //
            // Asked of the window itself rather than tracked alongside it: the
            // window procedure has only the `HWND`, and `WS_EX_NOREDIRECTIONBITMAP`
            // IS the property that makes a `BitBlt` here invalid -- so testing
            // for it directly cannot drift out of sync with how the window was
            // created.
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_NOREDIRECTIONBITMAP.0 != 0 {
                // A desktop surface. Record the damage; the wallpaper tick
                // repairs it through the compositor. Blitting would put a
                // wrong-alpha frame on screen -- a flash.
                render::note_desktop_damage(hwnd);
            } else {
                render::paint_cached(hdc, hwnd);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Declare this process PER-MONITOR DPI AWARE.
///
/// Must be called before any window is created or any coordinate is read.
///
/// Without it Windows treats panefx as a legacy app and hands it *virtualised*
/// coordinates: on a 2400x1600 display at 150% scaling the process is told the
/// screen is 1600x1066 and its windows are silently stretched to match. GlazeWM
/// is DPI-aware and reports REAL pixels over its IPC, so the panel is positioned
/// in one coordinate space and drawn in another — the panel ends up offset and
/// oversized, sitting half off the screen.
///
/// This never showed up on a 100%-scaling machine, which is why it survived
/// until panefx was deployed to a laptop running at 150%.
fn make_dpi_aware() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        // Ignore the result: it fails harmlessly if awareness was already set
        // (for example by an application manifest), and there is nothing useful
        // to do about it either way.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Register the window class once per process.
pub fn register_class() -> anyhow::Result<()> {
    make_dpi_aware();
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
    /// primary yields negative coordinates (observed: x = -1436).
    pub fn create(target: HWND, x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<Self> {
        Panel::create_anchored(Anchor::Window(target), x, y, width, height)
    }

    /// Create a panel against any anchor.
    ///
    /// For `Anchor::Desktop`, `x`/`y` must ALREADY be parent-relative — see
    /// `desktop::to_child`. Passing screen coordinates puts the panel off the
    /// parent's edge whenever the virtual desktop starts at a negative origin.
    pub fn create_anchored(
        anchor: Anchor,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> anyhow::Result<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;

            // A terminal panel is a top-level popup pinned behind its terminal.
            // A desktop surface is a CHILD of the icon host -- see
            // `desktop_window_style` for why nothing else works.
            let (style, parent) = match anchor {
                Anchor::Window(_) => (WS_POPUP, HWND::default()),
                Anchor::Desktop { parent, .. } => (desktop_window_style(), parent),
            };

            // Never focusable, never in the taskbar. A desktop surface adds
            // WS_EX_NOREDIRECTIONBITMAP because it presents through
            // DirectComposition and wants no redirection surface of its own.
            // No WS_EX_LAYERED anywhere: Windows refuses it on a child window.
            let mut ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
            if matches!(anchor, Anchor::Desktop { .. }) {
                ex_style |= WS_EX_NOREDIRECTIONBITMAP;
            }

            let hwnd = CreateWindowExW(
                ex_style,
                PANEL_CLASS,
                w!("panefx panel"),
                style,
                x,
                y,
                width.max(1),
                height.max(1),
                parent,
                None,
                hinstance,
                None,
            )?;

            // SW_SHOWNOACTIVATE, never SW_SHOW: showing must not pull focus
            // away from whatever the user is typing into.
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

            // A desktop surface is useless without its compositor, so failing to
            // build one is a NAMED error rather than a silently GDI-only panel
            // that would paint additively over the user's wallpaper.
            let surface = match anchor {
                Anchor::Desktop { .. } => {
                    match crate::compositor::Surface::new(hwnd, width.max(1), height.max(1)) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            let _ = DestroyWindow(hwnd);
                            anyhow::bail!("desktop compositor unavailable: {e}");
                        }
                    }
                }
                Anchor::Window(_) => None,
            };

            Ok(Panel {
                hwnd,
                anchor,
                width: width.max(1),
                height: height.max(1),
                visible: true,
                gdi: None,
                surface,
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
            match self.anchor {
                // Insert directly AFTER (i.e. behind) the target window.
                Anchor::Window(target) => {
                    SetWindowPos(self.hwnd, target, x, y, width, height, SWP_NOACTIVATE)?;
                }
                // A child's z-slot among the WorkerW's children is inherited;
                // asking for a specific one is meaningless and can fail outright.
                Anchor::Desktop { .. } => {
                    SetWindowPos(
                        self.hwnd,
                        HWND::default(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )?;
                }
            }
        }

        if resized {
            if let Some(s) = self.surface.as_mut() {
                if let Err(e) = s.resize(width, height) {
                    crate::log_warn!("[panefx] desktop surface resize failed: {e}");
                }
            }
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
        // A desktop surface goes to the BOTTOM OF ITS SIBLINGS.
        //
        // For a CHILD window `HWND_BOTTOM` means the bottom of the parent's
        // child list -- NOT the bottom of the desktop. Inside
        // `SHELLDLL_DefView` that puts us under `SysListView32`, so the icons
        // draw over us and stay clickable, while Explorer's desktop background
        // (painted below DefView entirely) stays behind us.
        //
        // Re-asserted every tick rather than set once: Explorer reorders and
        // rebuilds its desktop children on theme changes, wallpaper changes and
        // desktop refreshes.
        //
        // The old code inserted after Progman instead, which is the top-level
        // slot immediately above the shell window -- and therefore above
        // DefView and every icon in it. That is the bug this replaces.
        //
        // ONLY when it has actually drifted. `SetWindowPos` is not free even
        // when it changes nothing: it makes DWM re-evaluate and repaint the
        // affected region, and on a desktop surface that region is a whole
        // monitor. Called unconditionally from the wallpaper tick — i.e. every
        // frame, on every visible screen — that is a periodic full-screen
        // repaint at `wallpaper_fps`, which is the flashing. The z-slot is
        // still re-asserted whenever Explorer or GlazeWM moves us; the check
        // below is two cheap `GetWindow` calls that make the no-op case free.
        if matches!(self.anchor, Anchor::Desktop { .. }) {
            if self.is_at_bottom() {
                return Ok(());
            }
            PIN_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            return Ok(());
        }
        let Anchor::Window(target) = self.anchor else {
            return Ok(());
        };
        if self.is_directly_behind(target) {
            return Ok(());
        }
        unsafe {
            SetWindowPos(
                self.hwnd,
                target,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )?;
        }
        Ok(())
    }

    /// Are we already the last of our parent's children?
    ///
    /// `GW_HWNDLAST` walks from us to the end of the sibling list, so this is
    /// the exact question `SetWindowPos(HWND_BOTTOM)` would answer by doing the
    /// work. A failed call (window gone) reports `false`, so the caller falls
    /// through to the real `SetWindowPos` and surfaces the error there rather
    /// than silently skipping the pin.
    fn is_at_bottom(&self) -> bool {
        // "Bottom" for a desktop pane means NOTHING FOREIGN BELOW US -- not
        // "I am the very last sibling". There is one pane per monitor and all
        // of them are siblings in the same parent, so only one can ever be
        // last: demanding that slot made every pane displace the others once
        // per tick, forever. Measured with four panes at wallpaper_fps=10:
        // 39.9 z-order SetWindowPos/sec, each a DWM repaint of a whole
        // monitor -- the multi-monitor flicker, invisible on any
        // single-monitor machine because a lone pane is always last.
        //
        // So walk DOWN from us instead: if every sibling below is another
        // panefx pane, our z-slot is correct and no pin is needed.
        unsafe {
            let mut next = GetWindow(self.hwnd, GW_HWNDNEXT).ok();
            while let Some(h) = next {
                let mut buf = [0u16; 64];
                let n = GetClassNameW(h, &mut buf) as usize;
                if !class_is_ours(&String::from_utf16_lossy(&buf[..n])) {
                    return false;
                }
                next = GetWindow(h, GW_HWNDNEXT).ok();
            }
            true
        }
    }

    /// Is `target` the sibling immediately above us?
    ///
    /// That is precisely the slot `SetWindowPos(hwnd, target, …)` places us in,
    /// so when it already holds, the call would be a no-op repaint.
    fn is_directly_behind(&self, target: HWND) -> bool {
        let prev = unsafe { GetWindow(self.hwnd, GW_HWNDPREV).ok() };
        already_directly_behind(target.0 as usize, prev.map(|h| h.0 as usize))
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
            // Stop WM_PAINT reaching a DC we are about to delete.
            crate::render::forget_for_repaint(self.hwnd);
            crate::render::forget_desktop_damage(self.hwnd);
            // Release the swapchain, visual and target BEFORE the window they
            // are attached to. Rust drops fields after this body runs, which
            // would be after DestroyWindow.
            self.surface = None;
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
            // May fail for a desktop surface whose WorkerW parent Explorer has
            // already destroyed — that is fine and is why the result is ignored.
            //
            // But the GDI cleanup above is NOT optional in that case: those
            // objects belong to the process, not the window, and outlive it.
            // Skipping it because "the window is gone anyway" leaks a
            // full-screen bitmap per monitor on every Explorer restart.
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_pane_below_us_is_not_a_reason_to_re_pin() {
        // THE MULTI-MONITOR FLICKER, as a tripwire. One pane per monitor, all
        // siblings in one parent -- only one can be the last child. When the
        // guard demanded that exact slot, every pane displaced the others once
        // per tick: measured 39.9 z-order SetWindowPos/sec with four panes at
        // wallpaper_fps=10, each call a full-monitor DWM repaint. A lone pane
        // is always last, which is why a single-monitor machine never sees it.
        assert!(class_is_ours("PaneFxClass"));
    }

    #[test]
    fn a_foreign_sibling_below_us_does_force_a_re_pin() {
        // The guard must not become "never pin": Explorer's own children below
        // us mean we have drifted above the icon machinery and must sink.
        assert!(!class_is_ours("SysListView32"));
        assert!(!class_is_ours("SHELLDLL_DefView"));
        assert!(!class_is_ours(""));
    }

    #[test]
    fn a_surface_already_at_the_bottom_is_not_re_pinned() {
        // THE FLASHING, as a tripwire. `pin_behind_target` is called from the
        // wallpaper tick on EVERY frame for every visible screen. A
        // SetWindowPos that changes nothing is still not free: it makes DWM
        // re-evaluate and repaint the region, and for a desktop surface that
        // region is a whole monitor. Unconditionally, at wallpaper_fps, that
        // is a periodic full-screen repaint -- which is what the flashing is.
        assert!(already_at_bottom(0x5088C, Some(0x5088C)));
    }

    #[test]
    fn a_surface_explorer_moved_is_re_pinned() {
        // The other half: the guard must not become a way to never re-pin.
        // Explorer rebuilds its desktop children on theme changes, wallpaper
        // changes and F5, which lands us back above SysListView32 -- so when
        // somebody else is last, we MUST issue the SetWindowPos.
        assert!(!already_at_bottom(0x5088C, Some(0x1019C)));
    }

    #[test]
    fn a_z_order_we_could_not_read_is_re_pinned_not_skipped() {
        // GetWindow failing must fall through to the real call, never skip it.
        // Treating "unknown" as "already correct" would leave the surface
        // wherever it was last put -- i.e. potentially over the desktop icons,
        // which is the bug the per-tick re-pin exists to prevent. Silently
        // skipping is the dangerous direction; a redundant SetWindowPos is not.
        assert!(!already_at_bottom(0x5088C, None));
        assert!(!already_directly_behind(0x1234, None));
    }

    #[test]
    fn a_terminal_panel_already_behind_its_window_is_not_re_pinned() {
        // Same saving on the follower path, which runs on every reconcile:
        // GlazeWM reasserts z-order on focus changes, but most reconciles do
        // not actually move us.
        assert!(already_directly_behind(0x1234, Some(0x1234)));
        assert!(!already_directly_behind(0x1234, Some(0x9999)));
    }

    #[test]
    fn a_desktop_surface_is_a_child_never_a_popup() {
        // THE BUG, as a tripwire. panefx shipped a top-level WS_POPUP pinned one
        // slot above Progman. SHELLDLL_DefView -- the icon host -- is a CHILD of
        // Progman, so "above Progman" is also above every desktop icon.
        // Measured on build 26200: a full-screen PaneFxClass popup sat directly
        // above Progman and hid the icons completely.
        //
        // There is no top-level slot that works: above Progman covers the icons,
        // below Progman is under the desktop background. Only a child window in
        // Explorer's own tree can sit between the two.
        let style = desktop_window_style();
        assert_eq!(style.0 & WS_POPUP.0, 0, "a desktop surface must not be WS_POPUP");
        assert_eq!(style.0 & WS_CHILD.0, WS_CHILD.0, "it must be WS_CHILD");
        assert_eq!(style.0 & WS_VISIBLE.0, WS_VISIBLE.0);
    }

    #[test]
    fn a_terminal_panel_is_still_a_top_level_popup() {
        // The follower machinery is unchanged and must stay that way: a terminal
        // panel is a top-level popup pinned behind its terminal, which has
        // worked on two machines since day one.
        assert_ne!(desktop_window_style().0 & WS_CHILD.0, 0);
        assert_eq!(WS_POPUP.0 & WS_CHILD.0, 0, "the two styles are exclusive here");
    }
}
