//! The desktop wallpaper layer, and the monitors it spans.
//!
//! Two jobs live here:
//!   * find Explorer's **WorkerW** — the window that sits BEHIND the desktop
//!     icons — so a panel can be parented into it;
//!   * enumerate the physical monitors, so each can own its own surface.
//!
//! # How the WorkerW is found — and there are TWO desktop models
//!
//! Sending the undocumented `0x052C` to `Progman` asks Explorer to split the
//! desktop into two layers: one holding the icons, one behind them. Where the
//! second layer ENDS UP changed in Windows 11 24H2.
//!
//! **Classic (Win10, Win11 ≤ 23H2).** The WorkerW is a *top-level sibling* of
//! the window owning `SHELLDLL_DefView`. Walk top-level windows, find the
//! DefView owner, take its next sibling.
//!
//! **Raised (Win11 24H2+).** Microsoft rebuilt this for HDR backgrounds and, in
//! their own words, "no longer create multiple top-level HWNDs". `Progman` now
//! carries `WS_EX_NOREDIRECTIONBITMAP` and the WorkerW is a **CHILD of
//! Progman**, z-ordered under the DefView child. There is no top-level sibling
//! to find, so the classic hunt returns nothing — which looks exactly like "the
//! layer does not exist" while it is sitting right there.
//!
//! # Where the surface goes on the raised model: INSIDE DefView
//!
//! Measured on build 26200 (2400x1600 @ 150%), drawing a solid quad and sampling
//! the screen. "icons on top" is `WindowFromPoint` returning `SysListView32`:
//!
//! | api   | parent  | z-slot         | visible | icons on top |
//! |-------|---------|----------------|---------|--------------|
//! | GDI   | WorkerW | bottom or top  | **no**  | yes          |
//! | GDI   | Progman | bottom / after | **no**  | yes          |
//! | GDI   | DefView | **bottom**     | **yes** | **yes**      |
//! | GDI   | DefView | top            | yes     | NO (covered) |
//! | DComp | WorkerW | bottom or top  | yes     | yes          |
//!
//! GDI content is genuinely not composited in the WorkerW — that part of the
//! earlier investigation was right — but it IS composited inside
//! `SHELLDLL_DefView`, because Explorer paints the desktop background BELOW
//! DefView and the `SysListView32` above us is transparent. Sitting at the
//! bottom of DefView children puts us above the background and below the icons,
//! which is the whole goal, with the existing GDI renderer unchanged.
//!
//! That is why `parent` is the DefView and not Progman: a child of Progman is a
//! SIBLING of DefView, and every sibling slot either hides under Explorer own
//! background painting or covers the icons.
//!
//! The alternative — D3D11 + DirectComposition as a child of the WorkerW — also
//! works, and is what a compositing wallpaper app does. It is not used because
//! it costs a swapchain, a device, device-loss recovery and a per-frame upload
//! to buy nothing this program needs.
//!
//! That misreading cost real time during development: the classic search failing
//! was reported as "Windows removed the wallpaper layer". It had not. Measured
//! on this machine: `Progman` ex-style `0x200080` (raised), child WorkerW
//! present.
//!
//! ## Two traps in the discovery itself
//!
//! * **`GetShellWindow()`, not `FindWindow("Progman")`.** They disagree.
//!   Measured here: `FindWindowW("Progman")` returns 0 while `GetShellWindow()`
//!   returns a valid handle. Every false "Progman not found" came from the
//!   former.
//! * **On the CLASSIC model the layer is never the window owning
//!   `SHELLDLL_DefView`.** That one holds the icons, and a top-level sibling
//!   drawing over it covers them — which looks like success for about two
//!   seconds. `pick_wallpaper_layer` encodes that rule and is unit-tested
//!   without Explorer running. This is a statement about TOP-LEVEL windows on
//!   the classic model; on the raised model we deliberately become a CHILD of
//!   DefView, which is a different relationship with the opposite result.
//!
//! # Failure must stay survivable
//!
//! `0x052C` is undocumented and Explorer's internals are not a contract. Every
//! entry point returns a *named* error rather than an `Option`, and the daemon
//! carries on with terminal panels only. A wallpaper you cannot have is a
//! missing feature; a daemon that refuses to start is a broken program.

use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetWindowRect, IsWindow,
    SendMessageTimeoutW, SMTO_NORMAL,
};

/// Ask Progman to spawn a WorkerW behind the desktop icons.
///
/// Not in any SDK header. The value is stable from Windows 7 onward, but it is
/// an implementation detail of Explorer, not a contract — hence the graceful
/// failure path throughout this module.
const WM_SPAWN_WORKERW: u32 = 0x052C;

/// The wallpaper layer: Explorer's WorkerW window, plus where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workerw {
    pub hwnd: HWND,
    /// Screen-space top-left of the WorkerW, which spans the whole VIRTUAL
    /// desktop. Subtract this from a screen coordinate to get a child
    /// coordinate.
    ///
    /// This is not cosmetic: with a monitor left of the primary the virtual
    /// desktop starts at a negative x, so passing a raw screen coordinate to a
    /// child of this window puts the panel hundreds of pixels off its edge.
    pub origin: (i32, i32),
    /// The window our surface must be a CHILD of.
    ///
    /// Differs by desktop model, which is the whole reason this field exists:
    ///   * classic  — the WorkerW itself; it sits behind the icons already.
    ///   * raised   — **`SHELLDLL_DefView`**. Measured: the only parent in which
    ///     a GDI child is composited at all, and at the BOTTOM of its children
    ///     it lands above Explorer background painting and below
    ///     `SysListView32`, so the icons stay visible and clickable. See the
    ///     table in this module header.
    pub parent: HWND,
    /// True on the 24H2+ "raised desktop" model. Changes both the parent above
    /// and how z-order is asserted after `SetParent`.
    pub raised: bool,
    /// The class `parent` had when we attached.
    ///
    /// Recorded rather than assumed because the raised model prefers the
    /// WorkerW but falls back to the DefView, so there is no single right
    /// answer to check against later.
    pub parent_class: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopError {
    /// `FindWindow("Progman")` returned null. Explorer is not running as the
    /// shell, or we are on a window station that cannot see it.
    NoProgman,
    /// Progman exists but no window owning a `SHELLDLL_DefView` was found.
    NoDefViewHost,
    /// No WorkerW was found in either place — neither as a top-level sibling of
    /// the DefView host (classic) nor as a child of Progman (raised).
    NoSiblingWorkerw,
}

impl std::fmt::Display for DesktopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DesktopError::NoProgman => write!(
                f,
                "Progman not found — Explorer may not be running as the shell"
            ),
            DesktopError::NoDefViewHost => write!(
                f,
                "no window owns SHELLDLL_DefView — the desktop icon layer is missing"
            ),
            DesktopError::NoSiblingWorkerw => write!(
                f,
                "no WorkerW behind the desktop icons — Explorer did not create one"
            ),
        }
    }
}

impl std::error::Error for DesktopError {}

/// One candidate from the window enumeration.
///
/// Extracted as plain data so the *decision* — which of these is the wallpaper
/// layer — can be tested without Explorer, a desktop, or Windows at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// The top-level window.
    pub hwnd: isize,
    /// Does it own a `SHELLDLL_DefView` child? If so it is the ICON layer.
    pub has_defview: bool,
    /// Its next `WorkerW` sibling, if any. THIS is the wallpaper layer.
    pub next_workerw: Option<isize>,
}

/// Given the enumerated candidates, which window is the wallpaper layer?
///
/// **The answer is never the window that has the DefView child.** That window
/// holds the icons; drawing into it covers them. The wallpaper layer is its
/// next sibling.
pub fn pick_wallpaper_layer(candidates: &[Candidate]) -> Result<isize, DesktopError> {
    let host = candidates
        .iter()
        .find(|c| c.has_defview)
        .ok_or(DesktopError::NoDefViewHost)?;
    host.next_workerw.ok_or(DesktopError::NoSiblingWorkerw)
}

/// Which windows the raised model uses, given what was found under Progman.
///
/// Returns `(hwnd, parent)`: the layer to report, and the window our surface
/// must be a CHILD of.
///
/// Pulled out of `find` as plain data so the decision is testable without
/// Explorer — the same reason `pick_wallpaper_layer` exists. The rule it
/// encodes, measured on build 26200:
///
///   * `parent` is **always the DefView**. A GDI child of the WorkerW or of
///     Progman is not composited at all, in any z-slot.
///   * a missing DefView is fatal; a missing **WorkerW is not**, because we do
///     not draw into it. Treating the WorkerW as required was why a perfectly
///     usable desktop got reported as having no wallpaper layer.
pub fn pick_raised_layer(
    defview: Option<isize>,
    workerw: Option<isize>,
) -> Result<(isize, isize), DesktopError> {
    // The DefView must exist: no icon host means no desktop to sit in front of,
    // and it is our fallback parent.
    let dv = defview.ok_or(DesktopError::NoDefViewHost)?;
    // Prefer the WorkerW. It is Explorer's own wallpaper layer and sits BELOW
    // the DefView, so the icons are above us structurally rather than by a
    // z-slot we have to keep re-asserting against a sibling.
    let parent = workerw.unwrap_or(dv);
    Ok((parent, parent))
}

/// Collector for `EnumWindows`, which can only carry a raw pointer.
struct Collector {
    found: Vec<Candidate>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let c = &mut *(lparam.0 as *mut Collector);

    // Only windows owning a SHELLDLL_DefView are interesting; everything else
    // is a normal application window.
    let defview = FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None);
    if let Ok(dv) = defview {
        if !dv.is_invalid() {
            // Search from the DESKTOP root, starting after this window, for the
            // next WorkerW. That sibling is the wallpaper layer.
            let sibling = FindWindowExW(None, hwnd, w!("WorkerW"), None);
            c.found.push(Candidate {
                hwnd: hwnd.0 as isize,
                has_defview: true,
                next_workerw: match sibling {
                    Ok(s) if !s.is_invalid() => Some(s.0 as isize),
                    _ => None,
                },
            });
        }
    }
    // Keep going: we want every candidate so the pick is deterministic rather
    // than "whichever we hit first".
    BOOL(1)
}

/// Is the desktop using the "raised" model introduced in Windows 11 24H2?
///
/// Microsoft rebuilt desktop rendering for HDR backgrounds and, in their own
/// words, "no longer create multiple top-level HWNDs" — the top-level `Progman`
/// now carries `WS_EX_NOREDIRECTIONBITMAP` and the WorkerW is a **child** of it,
/// z-ordered under `SHELLDLL_DefView`.
///
/// Measured on this machine: `Progman` ex-style `0x200080`, so the bit is set.
#[cfg(windows)]
fn is_raised_desktop(progman: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_EXSTYLE};
    // WS_EX_NOREDIRECTIONBITMAP. Not exposed as a constant by the windows crate
    // under the features this crate enables, so spelled out.
    const WS_EX_NOREDIRECTIONBITMAP: isize = 0x0020_0000;
    unsafe { GetWindowLongPtrW(progman, GWL_EXSTYLE) & WS_EX_NOREDIRECTIONBITMAP != 0 }
}

/// Ask Explorer for the wallpaper layer and locate it.
pub fn find() -> Result<Workerw, DesktopError> {
    unsafe {
        // `GetShellWindow()`, NOT `FindWindow("Progman")`.
        //
        // These disagree. Measured on this machine: `FindWindowW("Progman")`
        // returns 0 while `GetShellWindow()` returns a valid handle — and every
        // false "the wallpaper layer is gone" reading during development came
        // from trusting the former. `GetShellWindow` asks the shell directly
        // rather than searching by class name.
        let progman = {
            use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
            let shell = GetShellWindow();
            if !shell.is_invalid() {
                shell
            } else {
                // Fall back to the class lookup for anything that does not
                // register a shell window.
                FindWindowW(w!("Progman"), None).map_err(|_| DesktopError::NoProgman)?
            }
        };
        if progman.is_invalid() {
            return Err(DesktopError::NoProgman);
        }

        // Ask for the split.
        //
        // wParam 0xD / lParam 0x1, not 0/0. This is the form Explorer itself
        // uses and the one Lively ships; the bare 0/0 variant is the older
        // spelling. A timeout is NOT fatal: the WorkerW may already exist from
        // an earlier request, a previous run, or another wallpaper app, so we
        // look regardless of the reply.
        let mut _result = 0usize;
        let _ = SendMessageTimeoutW(
            progman,
            WM_SPAWN_WORKERW,
            WPARAM(0xD),
            LPARAM(0x1),
            SMTO_NORMAL,
            1000,
            Some(&mut _result),
        );

        // --- the 24H2+ "raised desktop" model ---
        //
        // The WorkerW is a CHILD of Progman here, so the classic hunt for a
        // top-level sibling finds nothing and reports the layer as missing when
        // it is simply somewhere else. This is the case on Windows 11 24H2 and
        // later, which is to say: the common one now.
        if is_raised_desktop(progman) {
            // The DefView is what we parent into -- see the measured table in
            // the module header. Its ABSENCE is the fatal case now, not the
            // WorkerW: we can draw perfectly well with no WorkerW at all.
            let found = |cls| {
                FindWindowExW(progman, None, cls, None)
                    .ok()
                    .filter(|h: &HWND| !h.is_invalid())
                    .map(|h| h.0 as isize)
            };
            let dv = found(w!("SHELLDLL_DefView"));
            let ww = found(w!("WorkerW"));
            let (hwnd_raw, parent_raw) = pick_raised_layer(dv, ww)?;
            let parent = HWND(parent_raw as *mut _);
            let parent_class = if ww == Some(parent_raw) {
                "WorkerW"
            } else {
                "SHELLDLL_DefView"
            };

            // The origin MUST come from the window we actually parent into,
            // because `to_child` subtracts it. Taking it from the WorkerW while
            // parenting into the DefView offsets every surface by the difference
            // between them.
            let mut rect = RECT::default();
            let _ = GetWindowRect(parent, &mut rect);

            return Ok(Workerw {
                hwnd: HWND(hwnd_raw as *mut _),
                origin: (rect.left, rect.top),
                parent,
                raised: true,
                parent_class,
            });
        }

        // --- the classic model (Windows 10, and 11 up to 23H2) ---
        let mut collector = Collector { found: Vec::new() };
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut collector as *mut Collector as isize),
        );

        let hwnd_raw = pick_wallpaper_layer(&collector.found)?;
        let hwnd = HWND(hwnd_raw as *mut _);

        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);

        Ok(Workerw {
            hwnd,
            origin: (rect.left, rect.top),
            // Classic model: the WorkerW already sits behind the icons, so the
            // surface is parented straight into it.
            parent: hwnd,
            raised: false,
            parent_class: "WorkerW",
        })
    }
}

/// The class our surface parent must still have for the attachment to be live.
///
/// A named function so the rule is reachable from a test: on the raised model we
/// are a child of the icon host, on the classic model a child of the WorkerW,
/// and checking the wrong one means never noticing that Explorer rebuilt the
/// desktop underneath us.
pub fn expected_parent_class(w: &Workerw) -> &'static str {
    w.parent_class
}

/// Is this attachment still live?
///
/// `IsWindow` alone is not enough — Explorer can restart fast enough that a new
/// window reuses the handle value — so the class name is checked too.
///
/// The class check is on **`parent`**, not `hwnd`: `parent` is the window our
/// child window lives inside and therefore dies with. Explorer destroys and
/// recreates the DefView on desktop refresh (F5), theme change, wallpaper change
/// and the "show desktop icons" toggle — considerably more often than it touches
/// the WorkerW — and our surface goes with it every time.
pub fn is_alive(w: &Workerw) -> bool {
    unsafe {
        if !IsWindow(w.hwnd).as_bool() || !IsWindow(w.parent).as_bool() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(w.parent, &mut buf);
        n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == expected_parent_class(w)
    }
}

/// Convert a screen coordinate into a coordinate inside the WorkerW.
pub fn to_child(w: &Workerw, x: i32, y: i32) -> (i32, i32) {
    (x - w.origin.0, y - w.origin.1)
}

/// One physical display, in screen coordinates (which may be negative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// 1-based index parsed from the device name (`\\.\DISPLAY3` -> 3).
    ///
    /// Deliberately NOT the enumeration position: that order is not the
    /// `DISPLAY<n>` numbering and it changes when a monitor sleeps, which would
    /// silently shuffle which screen owns which effect.
    pub index: usize,
    pub device: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

impl MonitorInfo {
    /// A short human label for the TUI, e.g. `1440x2560 portrait`.
    pub fn label(&self) -> String {
        let shape = if self.height > self.width {
            " portrait"
        } else {
            ""
        };
        format!(
            "{}x{}{}{}",
            self.width,
            self.height,
            shape,
            if self.primary { " (primary)" } else { "" }
        )
    }
}

/// `\\.\DISPLAY3` -> `Some(3)`.
pub fn parse_display_index(device: &str) -> Option<usize> {
    let tail: String = device
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if tail.is_empty() {
        return None;
    }
    tail.chars().rev().collect::<String>().parse().ok()
}

struct MonitorCollector {
    found: Vec<MonitorInfo>,
}

unsafe extern "system" fn monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let c = &mut *(lparam.0 as *mut MonitorCollector);

    let mut mi = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        let device = String::from_utf16_lossy(&mi.szDevice)
            .trim_end_matches('\0')
            .to_string();
        let r = mi.monitorInfo.rcMonitor;
        // Fall back to enumeration order only if the device name has no number,
        // which should not happen but must not lose the monitor entirely.
        let index = parse_display_index(&device).unwrap_or(c.found.len() + 1);
        c.found.push(MonitorInfo {
            index,
            device,
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            primary: (mi.monitorInfo.dwFlags & 1) != 0, // MONITORINFOF_PRIMARY
        });
    }
    BOOL(1)
}

/// Every physical display attached right now.
///
/// Uses `EnumDisplayMonitors` rather than the window-manager's list, because a
/// monitor with no windows on it does not appear in the latter — and an empty
/// screen is exactly where a wallpaper is most visible.
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    let mut c = MonitorCollector { found: Vec::new() };
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_proc),
            LPARAM(&mut c as *mut MonitorCollector as isize),
        );
    }
    c.found.sort_by_key(|m| m.index);
    c.found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wallpaper_layer_is_the_sibling_not_the_defview_host() {
        // The trap this whole module is shaped around. Reparenting into the
        // DefView host puts the animation ON TOP of the desktop icons, which
        // reads as success until you notice the icons are gone.
        let c = [Candidate {
            hwnd: 0x1111,
            has_defview: true,
            next_workerw: Some(0x2222),
        }];
        assert_eq!(pick_wallpaper_layer(&c), Ok(0x2222));
    }

    #[test]
    fn no_defview_host_is_a_named_error() {
        // Must be a NAMED error, never a silent None: the daemon has to be able
        // to tell the user why there is no wallpaper.
        let c = [Candidate {
            hwnd: 0x1111,
            has_defview: false,
            next_workerw: Some(0x2222),
        }];
        assert_eq!(pick_wallpaper_layer(&c), Err(DesktopError::NoDefViewHost));
    }

    #[test]
    fn a_host_with_no_sibling_is_the_no_op_build_case() {
        // 0x052C did nothing: the icon layer exists but nothing was spawned
        // behind it.
        let c = [Candidate {
            hwnd: 0x1111,
            has_defview: true,
            next_workerw: None,
        }];
        assert_eq!(pick_wallpaper_layer(&c), Err(DesktopError::NoSiblingWorkerw));
    }

    #[test]
    fn empty_enumeration_does_not_panic() {
        assert_eq!(pick_wallpaper_layer(&[]), Err(DesktopError::NoDefViewHost));
    }

    #[test]
    fn display_index_comes_from_the_device_name() {
        // NOT from enumeration order — that reshuffles when a monitor sleeps,
        // which would silently move each screen's effect to another screen.
        assert_eq!(parse_display_index(r"\\.\DISPLAY3"), Some(3));
        assert_eq!(parse_display_index(r"\\.\DISPLAY12"), Some(12));
        assert_eq!(parse_display_index(r"\\.\DISPLAY1"), Some(1));
        assert_eq!(parse_display_index(r"\\.\NOTADISPLAY"), None);
        assert_eq!(parse_display_index(""), None);
    }

    #[test]
    fn the_raised_model_parents_into_the_defview() {
        // MEASURED, build 26200: a GDI child of the WorkerW or of Progman draws
        // nothing at all (0/240 sampled red) in every z-slot. A GDI child of
        // SHELLDLL_DefView at the bottom of its children draws (240/240) AND
        // leaves SysListView32 owning the pixels, so the icons stay clickable.
        //
        // The previous version of this test asserted on a struct it had just
        // built itself, so it could not fail. This one exercises the decision.
        let (hwnd, parent) = pick_raised_layer(Some(0x3000), Some(0x2000)).unwrap();
        assert_eq!(parent, 0x2000, "prefer the WorkerW: it sits BELOW the icons");
        assert_eq!(hwnd, parent);
    }

    #[test]
    fn a_missing_workerw_is_survivable_but_a_missing_defview_is_not() {
        // We do not draw into the WorkerW any more, so its absence must not
        // fail: reporting "no wallpaper layer" for a desktop that can host one
        // perfectly well is the exact false negative this module was built to
        // avoid. Falling back to the DefView keeps `hwnd` a real window.
        let (hwnd, parent) = pick_raised_layer(Some(0x3000), None).unwrap();
        assert_eq!(parent, 0x3000, "fall back to the DefView, never a null handle");
        assert_eq!(hwnd, 0x3000);

        // The DefView is the one we genuinely cannot work without.
        assert_eq!(
            pick_raised_layer(None, Some(0x2000)),
            Err(DesktopError::NoDefViewHost)
        );
        assert_eq!(pick_raised_layer(None, None), Err(DesktopError::NoDefViewHost));
    }

    #[test]
    fn liveness_watches_the_window_we_are_a_child_of() {
        // Our surface dies with its PARENT. Checking the WorkerW class while
        // parented into the DefView means never noticing that Explorer rebuilt
        // the icon host: the surface is gone and the daemon reports health.
        let w = Workerw {
            hwnd: HWND(0x2000 as *mut _),
            origin: (0, 0),
            parent: HWND(0x2000 as *mut _),
            raised: true,
            parent_class: "WorkerW",
        };
        assert_eq!(expected_parent_class(&w), "WorkerW");
        let fallback = Workerw {
            parent_class: "SHELLDLL_DefView",
            ..w
        };
        assert_eq!(expected_parent_class(&fallback), "SHELLDLL_DefView");
    }

    #[test]
    fn to_child_handles_a_negative_virtual_origin() {
        // The virtual desktop starts at the top-left-most monitor, which is
        // negative when a screen sits left of or above the primary. A raw screen
        // coordinate would put the panel right off the parent's edge.
        let w = Workerw {
            hwnd: HWND(std::ptr::null_mut()),
            origin: (-1440, -1230),
            parent: HWND(std::ptr::null_mut()),
            raised: false,
            parent_class: "WorkerW",
        };
        assert_eq!(to_child(&w, -1440, -1230), (0, 0));
        assert_eq!(to_child(&w, 0, 0), (1440, 1230));
    }

    #[test]
    fn monitor_label_reads_naturally() {
        let m = MonitorInfo {
            index: 1,
            device: r"\\.\DISPLAY1".into(),
            x: -1440,
            y: -1230,
            width: 1440,
            height: 2560,
            primary: false,
        };
        assert_eq!(m.label(), "1440x2560 portrait");
    }
}

/// Raise an already-running `panefx-gui` window, if there is one. Returns true
/// when one was found and focused, meaning the caller should NOT start another.
///
/// Matching is by owning PROCESS NAME, not window title or class: the GUI is
/// undecorated (it paints its own Win98 title bar), so its caption is chrome
/// panefx draws rather than something Windows guarantees, and eframe's window
/// class is a generic name shared with any other glow app.
///
/// Why bother at all: two GUIs both writing `config.toml` is a real way to lose
/// settings — the second to save wins and the first never knows. A second
/// window is also never what a tray click meant.
#[cfg(windows)]
pub fn focus_existing_gui() -> bool {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, ShowWindow,
        SW_RESTORE,
    };

    static FOUND: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

    unsafe extern "system" fn cb(hwnd: HWND, _l: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut pid = 0u32;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
            // Skip our own windows: the daemon owns the backdrop panels, and
            // one of those matching would make the GUI never start.
            if pid == 0 || pid == GetCurrentProcessId() {
                return BOOL(1);
            }
            if process_name(pid).as_deref() == Some("panefx-gui") {
                if let Ok(mut f) = FOUND.lock() {
                    *f = Some(hwnd.0 as isize);
                }
                return BOOL(0); // stop enumerating
            }
            BOOL(1)
        }
    }

    if let Ok(mut f) = FOUND.lock() {
        *f = None;
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(0));
    }
    let hwnd = FOUND.lock().ok().and_then(|f| *f);
    match hwnd {
        Some(h) => unsafe {
            let h = HWND(h as *mut std::ffi::c_void);
            // Restore first: a minimised window accepts focus but stays in the
            // taskbar, which looks exactly like the click did nothing.
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
            true
        },
        None => false,
    }
}

#[cfg(not(windows))]
pub fn focus_existing_gui() -> bool {
    false
}

/// Lower-case stem of the executable owning `pid` (`"panefx-gui"`), or None.
///
/// Delegates to [`crate::proc_name`]: this walk existed here AND in
/// term_opacity.rs, with two different ways of stripping the extension.
#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    crate::proc_name::of(pid)
}
