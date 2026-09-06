//! Window geometry straight from Win32, with no window manager involved.
//!
//! The fallback half of [`crate::window_source`]. It produces exactly the same
//! `Vec<ipc::Window>` GlazeWM's IPC does, so nothing downstream can tell which
//! backend is running.
//!
//! # The filter is the whole problem
//!
//! `EnumWindows` is not a list of windows a person would recognise. Filtered to
//! process `alacritty` on this machine it returns FOUR, of which one is real
//! (measured 2026-09-05):
//!
//! | class | visible | owner | `WS_EX_TOOLWINDOW` |
//! |---|---|---|---|
//! | `Window Class`              | true  | none  | false | <- the terminal
//! | `Winit Thread Event Target` | true  | none  | **true**  |
//! | `MSCTFIME UI`               | false | owned | false |
//! | `IME`                       | false | owned | false |
//!
//! The winit event-target window reports **visible**, is **owner-less**, and
//! sits at 22x22. Size alone would be a fragile way to reject it; its
//! `WS_EX_TOOLWINDOW` bit is not. The two IME windows are invisible AND owned.
//!
//! So the rule is `visible && owner-less && !WS_EX_TOOLWINDOW`, which keeps
//! exactly one — and `keeps()` is a pure function over [`RawWindow`] precisely
//! so that table can be a unit test rather than a comment.
//!
//! # Two things Win32 gets righter than the WM
//!
//! * **Frame bounds.** `GetWindowRect` includes the invisible resize border
//!   Windows 10/11 put around standard windows, so a backdrop sized to it is
//!   several pixels too big on every side. `DWMWA_EXTENDED_FRAME_BOUNDS` is the
//!   visible edge.
//! * **Move and resize events.** `ipc.rs` records that GlazeWM emits none, so
//!   that backend re-queries on every unrelated hint plus a 500ms backstop
//!   poll. `EVENT_OBJECT_LOCATIONCHANGE` is the real thing.
//!
//! # One thing it gets less right
//!
//! There is no concept of a workspace. `DWMWA_CLOAKED` catches Windows' own
//! virtual desktops and is reported as `display_state: "hidden"`, matching what
//! GlazeWM says about an off-workspace window. A tiling WM's private notion of
//! workspaces is invisible here — but a WM that hides windows with
//! `ShowWindow` (GlazeWM does) drops them from this list anyway, because they
//! stop being `IsWindowVisible`.

use crate::ipc;

/// The raw Win32 facts about one top-level window, before any judgement.
///
/// Exists so [`keeps`] can be tested against the measured capture above
/// without a live desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWindow {
    pub handle: isize,
    pub class_name: String,
    pub process_name: String,
    pub visible: bool,
    /// Has an owner window — the IME helpers do, real top-level windows do not.
    pub owned: bool,
    pub tool_window: bool,
    /// On another virtual desktop, or a suspended UWP app.
    pub cloaked: bool,
    pub iconic: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Is this a window a person would say is on their screen?
///
/// Pure, and the reason the module header's table is testable.
pub fn keeps(w: &RawWindow) -> bool {
    // Not visible: the IME helpers, and anything a WM has hidden.
    if !w.visible {
        return false;
    }
    // Owned: a helper belonging to a real window, never a window in its own
    // right. Both IME entries are caught here as well.
    if w.owned {
        return false;
    }
    // Tool windows are explicitly "not part of the normal UI" -- which is
    // exactly what winit's event-target window is declaring itself to be.
    if w.tool_window {
        return false;
    }
    // A zero-area window has nothing to sit behind. Checked last because it is
    // the weakest of the four signals and should never be the only one doing
    // the work.
    if w.width <= 0 || w.height <= 0 {
        return false;
    }
    true
}

/// Translate a kept [`RawWindow`] into the shape the rest of panefx consumes.
///
/// The two string fields mirror GlazeWM's vocabulary rather than inventing a
/// new one, so `Window::is_visible` and `Window::is_minimized` keep working
/// untouched:
///
/// * `display_state` — `"shown"`, or `"hidden"` when cloaked. Cloaked windows
///   are `IsWindowVisible`, so without this a window on another virtual desktop
///   would occlude the wallpaper and get a backdrop it cannot show.
/// * `state.kind` — `"minimized"` when iconic. A minimized window keeps its
///   pre-minimize rect here exactly as it does under GlazeWM, which is why
///   `is_on_screen()` has to consult both fields.
pub fn to_ipc(w: &RawWindow) -> ipc::Window {
    ipc::Window {
        handle: w.handle,
        process_name: w.process_name.clone(),
        class_name: w.class_name.clone(),
        x: w.x,
        y: w.y,
        width: w.width,
        height: w.height,
        display_state: if w.cloaked { "hidden" } else { "shown" }.to_string(),
        state: ipc::WindowState {
            kind: if w.iconic { "minimized" } else { "normal" }.to_string(),
        },
    }
}

#[cfg(windows)]
mod imp {
    use super::{keeps, to_ipc, RawWindow};
    use crate::ipc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_OWNER,
        WS_EX_TOOLWINDOW,
    };

    /// Set by the WinEvent callback and by an explicit request; drained by
    /// `try_recv`. An `AtomicBool` rather than a channel because the callback
    /// must do as little as possible: `EVENT_OBJECT_LOCATIONCHANGE` fires
    /// continuously for every pixel of a window drag, system-wide.
    pub(super) static PENDING: AtomicBool = AtomicBool::new(true);

    /// Enumerate every window worth drawing behind, right now.
    pub fn enumerate() -> Vec<ipc::Window> {
        // ONE process snapshot for the whole pass. `proc_name::of` walks every
        // process on the machine, so calling it per window would make this
        // O(windows x processes) -- see the module header of `proc_name`.
        let procs = crate::proc_name::map();

        let mut raw: Vec<RawWindow> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(collect),
                LPARAM(&mut raw as *mut Vec<RawWindow> as isize),
            );
        }

        // `collect` parked the pid in `process_name` as text, because resolving
        // it there would mean one ToolHelp walk per window. Filter FIRST, so
        // the name is only resolved for windows that survive.
        raw.into_iter()
            .filter(keeps)
            .map(|mut w| {
                if let Ok(pid) = w.process_name.parse::<u32>() {
                    w.process_name = procs.get(&pid).cloned().unwrap_or_default();
                }
                to_ipc(&w)
            })
            .collect()
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<RawWindow>);

        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class_name = String::from_utf16_lossy(&class[..n]);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let owned = GetWindow(hwnd, GW_OWNER).map(|h| !h.is_invalid()).unwrap_or(false);

        // Cloaked is a DWM question, not a window-style one: a window on
        // another virtual desktop is still WS_VISIBLE.
        let mut cloaked_val = 0u32;
        let cloaked = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked_val as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked_val != 0;

        // Extended frame bounds first -- the VISIBLE edge. GetWindowRect is the
        // fallback, and includes the invisible resize border.
        let mut r = RECT::default();
        let got_frame = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok();
        if !got_frame {
            let _ = GetWindowRect(hwnd, &mut r);
        }

        out.push(RawWindow {
            handle: hwnd.0 as isize,
            class_name,
            // Carried as text and resolved by the caller against ONE process
            // snapshot, rather than a ToolHelp walk per window.
            process_name: pid.to_string(),
            visible: IsWindowVisible(hwnd).as_bool(),
            owned,
            tool_window: ex & WS_EX_TOOLWINDOW.0 != 0,
            cloaked,
            iconic: IsIconic(hwnd).as_bool(),
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        });
        BOOL(1)
    }

    /// Ask for a fresh list on the next `try_recv`.
    pub fn mark_dirty() {
        PENDING.store(true, Ordering::Relaxed);
    }

    pub fn take_dirty() -> bool {
        PENDING.swap(false, Ordering::Relaxed)
    }

    /// Live WinEvent hooks. Unhooked on drop.
    pub struct Hooks(Vec<windows::Win32::UI::Accessibility::HWINEVENTHOOK>);

    impl Drop for Hooks {
        fn drop(&mut self) {
            use windows::Win32::UI::Accessibility::UnhookWinEvent;
            for h in self.0.drain(..) {
                unsafe {
                    let _ = UnhookWinEvent(h);
                }
            }
        }
    }

    /// Subscribe to the events that mean "the window layout changed".
    ///
    /// MUST be called on a thread that pumps messages: `WINEVENT_OUTOFCONTEXT`
    /// callbacks are delivered through the message queue, and a hook installed
    /// on a thread that never pumps simply never fires. `main.rs` runs a
    /// `PeekMessageW` loop, which is why this is installed from there.
    ///
    /// Ranges rather than one hook per event, because each range is contiguous
    /// and the callback is a single atomic store either way.
    pub fn install_hooks() -> Hooks {
        use windows::Win32::UI::Accessibility::SetWinEventHook;
        use windows::Win32::UI::WindowsAndMessaging::{
            EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
            EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, WINEVENT_OUTOFCONTEXT,
            WINEVENT_SKIPOWNPROCESS,
        };

        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            // DESTROY, SHOW and HIDE are 0x8001..0x8003, contiguous.
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];

        let mut hooks = Vec::new();
        for (lo, hi) in ranges {
            let h = unsafe {
                SetWinEventHook(
                    lo,
                    hi,
                    None,
                    Some(on_event),
                    // System-wide: panefx must see windows it does not own.
                    0,
                    0,
                    // SKIPOWNPROCESS keeps our own panel windows from waking us
                    // up -- we move them ourselves, every frame, and reacting to
                    // that would be a feedback loop.
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if !h.is_invalid() {
                hooks.push(h);
            }
        }
        crate::log_info!("[panefx] native window events: {} hooks", hooks.len());
        Hooks(hooks)
    }

    /// As little work as possible.
    ///
    /// `EVENT_OBJECT_LOCATIONCHANGE` fires for every pixel of every drag,
    /// system-wide, and also for carets and scrollbars. Setting one flag and
    /// letting the frame loop decide is the only affordable shape.
    unsafe extern "system" fn on_event(
        _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
        _event: u32,
        _hwnd: HWND,
        id_object: i32,
        _id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        // OBJID_WINDOW. Caret and cursor events share these ranges and say
        // nothing about layout; a blinking cursor must not force a re-enumerate
        // several times a second.
        const OBJID_WINDOW: i32 = 0;
        if id_object != OBJID_WINDOW {
            return;
        }
        PENDING.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
pub use imp::{enumerate, install_hooks, mark_dirty, take_dirty, Hooks};

#[cfg(not(windows))]
pub fn enumerate() -> Vec<ipc::Window> {
    Vec::new()
}
#[cfg(not(windows))]
pub struct Hooks;
#[cfg(not(windows))]
pub fn install_hooks() -> Hooks {
    Hooks
}
#[cfg(not(windows))]
pub fn mark_dirty() {}
#[cfg(not(windows))]
pub fn take_dirty() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(class: &str, visible: bool, owned: bool, tool: bool) -> RawWindow {
        RawWindow {
            handle: 1,
            class_name: class.into(),
            process_name: "alacritty".into(),
            visible,
            owned,
            tool_window: tool,
            cloaked: false,
            iconic: false,
            x: 1,
            y: 1,
            width: 2398,
            height: 1556,
        }
    }

    /// The measured capture from the module header, as a test.
    ///
    /// Four windows belong to process `alacritty`; exactly ONE is the terminal.
    /// Getting this wrong does not fail loudly -- it pins a 22x22 backdrop
    /// behind an invisible helper window and looks like a rendering glitch.
    #[test]
    fn only_the_real_terminal_survives_the_filter() {
        let captured = [
            (raw("Window Class", true, false, false), true),
            // Visible AND owner-less. Only the tool-window bit rejects it.
            (raw("Winit Thread Event Target", true, false, true), false),
            (raw("MSCTFIME UI", false, true, false), false),
            (raw("IME", false, true, false), false),
        ];
        for (w, want) in captured {
            assert_eq!(
                keeps(&w),
                want,
                "{} should {} be kept",
                w.class_name,
                if want { "" } else { "NOT" }
            );
        }
        assert_eq!(
            captured_count(),
            1,
            "exactly one of the four alacritty windows is real"
        );
    }

    fn captured_count() -> usize {
        [
            raw("Window Class", true, false, false),
            raw("Winit Thread Event Target", true, false, true),
            raw("MSCTFIME UI", false, true, false),
            raw("IME", false, true, false),
        ]
        .iter()
        .filter(|w| keeps(w))
        .count()
    }

    #[test]
    fn a_zero_area_window_is_never_kept() {
        let mut w = raw("Window Class", true, false, false);
        w.width = 0;
        assert!(!keeps(&w));
        w.width = 800;
        w.height = -1;
        assert!(!keeps(&w));
    }

    /// A cloaked window is `IsWindowVisible`, so it passes the filter -- it must
    /// be reported HIDDEN or it would occlude the wallpaper and be given a
    /// backdrop nobody can see.
    #[test]
    fn a_cloaked_window_is_reported_hidden() {
        let mut w = raw("Window Class", true, false, false);
        w.cloaked = true;
        assert!(keeps(&w), "still a real window, just on another desktop");
        let m = to_ipc(&w);
        assert!(!m.is_visible());
        assert!(!m.is_on_screen());
    }

    /// Minimized must survive as a window but not as an on-screen one: it keeps
    /// its stale pre-minimize rect, exactly as GlazeWM reports it.
    #[test]
    fn a_minimized_window_keeps_its_rect_but_is_not_on_screen() {
        let mut w = raw("Window Class", true, false, false);
        w.iconic = true;
        let m = to_ipc(&w);
        assert!(m.is_visible(), "display_state is still shown");
        assert!(m.is_minimized());
        assert!(!m.is_on_screen(), "the rect it reports is not occupied");
        assert_eq!((m.width, m.height), (2398, 1556));
    }

    /// The mapping must produce something `is_target` recognises, or the native
    /// backend would enumerate perfectly and then draw nothing.
    #[test]
    fn a_mapped_window_is_recognised_as_a_target() {
        let m = to_ipc(&raw("Window Class", true, false, false));
        assert!(m.is_target(), "process_name must survive the mapping");
        assert!(m.is_on_screen());
    }
}
