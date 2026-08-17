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
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetShellWindow, RegisterClassExW, SetWindowPos,
    ShowWindow,
    HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_SHOWNOACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
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

/// What a panel is positioned relative to.
///
/// `Window` is the original behaviour: a sibling of the tracked terminal,
/// positioned in SCREEN coordinates, re-pinned into the z-slot behind it on
/// every reconcile because GlazeWM reasserts z-order on focus changes.
///
/// `Desktop` is the wallpaper layer: a CHILD of Explorer's WorkerW, so its
/// coordinates are parent-relative and its z-position is inherited and
/// permanent. There is nothing to re-pin and nothing to fight.
#[derive(Clone, Copy)]
pub enum Anchor {
    Window(HWND),
    Desktop {
        parent: HWND,
        /// True on the Windows 11 24H2+ "raised desktop" model.
        ///
        /// There the surface is a sibling of Progman's WorkerW and DefView
        /// children rather than a child of the WorkerW, and Microsoft's guidance
        /// is explicit that it must be `WS_EX_LAYERED` **at creation** with
        /// alpha 255. Adding the style after the fact leaves the surface
        /// mis-composited.
        raised: bool,
    },
}

/// Which z-order slot a desktop surface inserts after.
///
/// Pulled out of the `unsafe` block on purpose: this one choice is the whole
/// difference between a visible wallpaper and an invisible one, and inside the
/// block no test could reach it. The bug it encodes was real — see
/// `pin_behind_target`.
pub fn desktop_insert_after(progman: HWND) -> HWND {
    if progman.is_invalid() {
        // No shell window to anchor to. The bottom is still the right fallback:
        // wrong in the "hidden behind the background" direction rather than the
        // "covering every application on screen" direction.
        HWND_BOTTOM
    } else {
        progman
    }
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
    /// For `Anchor::Desktop`, `x`/`y` must ALREADY be WorkerW-relative — see
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

            // WS_CHILD for a desktop surface, never WS_POPUP.
            //
            // A WS_POPUP *with a parent* is an OWNED window, not a child: it
            // does not clip to the parent and does not inherit z-position, so it
            // floats above every application instead of sitting behind the
            // desktop icons. One style bit, and the ugliest possible failure.
            // BOTH kinds are top-level WS_POPUP windows with NO parent.
            //
            // The wallpaper surface used to be a WS_CHILD of Explorer's Progman,
            // following Microsoft's guidance for the "raised desktop" model.
            // That path fought us at every step: WS_EX_LAYERED was refused on a
            // child, coordinates became parent-relative in a space that did not
            // match the virtual desktop, and z-order had to be re-asserted
            // against windows Explorer recreates at will. The surface existed,
            // was correctly ordered, and drew 340 lit cells a frame -- into
            // pixels nobody could see.
            //
            // The terminal panels have been a top-level popup pinned into a
            // z-slot since day one, on two machines, without trouble. The
            // wallpaper is the same problem with a different anchor, so it uses
            // the same solution: screen coordinates, no parent, pinned to the
            // BOTTOM of the z-order instead of behind a specific window.
            let (style, parent) = (WS_POPUP, HWND::default());
            let _ = &anchor;

            // Same ex-style for both: never focusable, never in the taskbar.
            // No WS_EX_LAYERED -- that was only needed for the child-of-Progman
            // approach, and Windows refused it there anyway.
            let ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;

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

            Ok(Panel {
                hwnd,
                anchor,
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
        // On the RAISED desktop the surface is a sibling of Progman's WorkerW
        // and DefView children, so its z-slot is not inherited and must be
        // asserted: directly after SHELLDLL_DefView, i.e. behind the icons but
        // above the WorkerW. Explorer recreates these children on theme and
        // wallpaper changes, so this is re-asserted rather than done once.
        // A desktop surface sits directly ABOVE Progman: behind every
        // application window, but IN FRONT of the desktop background.
        //
        // NOT `HWND_BOTTOM`. That means the absolute bottom of the z-order,
        // which is *below* Progman -- and Progman paints the desktop background
        // over the top, so the surface renders into pixels nobody can see. It
        // looked like it worked at first only because a freshly created window
        // happened to land just above Progman; once this was re-asserted every
        // tick, each frame pushed it back under and the wallpaper vanished.
        //
        // Inserting after Progman is the whole trick: one slot in front of the
        // background, still behind everything else.
        if matches!(self.anchor, Anchor::Desktop { .. }) {
            unsafe {
                let insert_after = desktop_insert_after(GetShellWindow());
                SetWindowPos(
                    self.hwnd,
                    insert_after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )?;
            }
            return Ok(());
        }
        // A classic-model desktop surface inherits its z-position from the
        // WorkerW it is a child of, so there is nothing to re-pin.
        let Anchor::Window(target) = self.anchor else {
            return Ok(());
        };
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
    fn a_desktop_surface_inserts_after_progman_never_at_the_bottom() {
        // THE BUG. `HWND_BOTTOM` is the absolute bottom of the z-order, which is
        // *below* Progman -- and Progman paints the desktop background over the
        // top, so the surface drew every frame into pixels nobody could see.
        //
        // It looked fine at first only because a freshly created window happened
        // to land just above Progman by luck. Once z-order was re-asserted every
        // tick, each frame shoved it back under and the wallpaper vanished while
        // the daemon reported perfect health: windows present, correct geometry,
        // not occluded, no errors, 128s of CPU burned on invisible frames.
        let progman = HWND(0x1_0BDE as *mut core::ffi::c_void);
        let slot = desktop_insert_after(progman);
        assert_eq!(slot, progman, "must insert directly after Progman");
        assert_ne!(slot, HWND_BOTTOM, "HWND_BOTTOM hides the wallpaper");
    }

    #[test]
    fn without_a_shell_window_it_falls_back_to_the_bottom() {
        // Failing toward "invisible" beats failing toward "covers everything you
        // are working on".
        assert_eq!(desktop_insert_after(HWND::default()), HWND_BOTTOM);
    }
}
