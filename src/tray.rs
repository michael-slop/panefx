//! A system-tray icon for the daemon.
//!
//! The daemon is `#![windows_subsystem = "windows"]` — no console, no window,
//! nothing in the taskbar. That is right for a background service but it means
//! panefx is completely invisible: there is no way to tell whether it is running,
//! no way to reach the TUI without a terminal, and no way to restart it after a
//! bad state without Task Manager.
//!
//! The tray icon fixes all three. It appears at startup and stays for the
//! daemon's whole life.
//!
//! Uses `Shell_NotifyIconW` and a plain popup menu — both already available
//! through the `windows` crate this project depends on, so **no new crates**.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetCursorPos,
    LoadIconW, PostQuitMessage, RegisterClassExW, SetForegroundWindow, TrackPopupMenu,
    IDI_APPLICATION, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_RIGHTALIGN, WM_APP,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSEXW, WS_OVERLAPPED,
};


/// Our private message for tray callbacks. `WM_APP` upward is reserved for
/// application use, so this cannot collide with a system message.
const WM_TRAYICON: u32 = WM_APP + 1;

const ID_OPEN_TUI: usize = 1;
const ID_RELOAD: usize = 2;
const ID_EXIT: usize = 3;

/// What the user picked from the tray menu.
///
/// Returned to the daemon loop rather than acted on here: restarting the process
/// and launching the TUI are the daemon's business, and keeping this module to
/// "report what was clicked" makes it testable and keeps Win32 out of the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenTui,
    Reload,
    Exit,
}

/// Menu command id -> action.
///
/// Split out so the mapping is unit-testable without a desktop: getting it wrong
/// means "Exit" quietly reloading, which is the kind of thing nobody notices
/// until it matters.
pub fn action_for(id: usize) -> Option<TrayAction> {
    match id {
        ID_OPEN_TUI => Some(TrayAction::OpenTui),
        ID_RELOAD => Some(TrayAction::Reload),
        ID_EXIT => Some(TrayAction::Exit),
        _ => None,
    }
}

/// Pending action, set by the window proc and drained by the daemon loop.
///
/// A `static` because the window proc is an `extern "system"` callback with no
/// way to carry state, and the alternative (`SetWindowLongPtr` with a boxed
/// pointer) is more unsafe code for no gain at this size.
static PENDING: std::sync::Mutex<Option<TrayAction>> = std::sync::Mutex::new(None);

/// Take whatever the user clicked, if anything. Never blocks.
pub fn take_action() -> Option<TrayAction> {
    PENDING.lock().ok().and_then(|mut p| p.take())
}

unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let event = (lparam.0 & 0xFFFF) as u32;
            match event {
                // Left click goes straight to the TUI: the common case should
                // not need a menu.
                WM_LBUTTONUP => {
                    if let Ok(mut p) = PENDING.lock() {
                        *p = Some(TrayAction::OpenTui);
                    }
                }
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as usize;
            if let Some(a) = action_for(id) {
                if let Ok(mut p) = PENDING.lock() {
                    *p = Some(a);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let open = windows::core::w!("Open panefx TUI");
    let reload = windows::core::w!("Reload panefx");
    let exit = windows::core::w!("Exit");
    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_TUI, open);
    let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, reload);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, exit);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Required by the docs: without this the menu does not dismiss when the
    // user clicks elsewhere, and sits on screen until something else steals
    // focus.
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
}

/// The tray icon and the hidden window that receives its messages.
pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    /// Create the icon. Failure is not fatal — panefx runs fine without it.
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class = windows::core::w!("PaneFxTrayClass");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(tray_proc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // A message-only window would be tidier, but TrackPopupMenu needs a
            // real window to own the menu and take foreground.
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                windows::core::w!("panefx tray"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                hinstance,
                None,
            )?;

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAYICON,
                hIcon: LoadIconW(None, IDI_APPLICATION)?,
                ..Default::default()
            };
            let tip = "panefx — click for the control TUI";
            for (i, c) in tip.encode_utf16().enumerate().take(127) {
                nid.szTip[i] = c;
            }
            Shell_NotifyIconW(NIM_ADD, &nid).ok()?;

            Ok(Tray { hwnd })
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            // Remove the icon explicitly. Without this Windows leaves a ghost
            // in the tray until the user hovers over it.
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_maps_to_its_own_action() {
        // A wrong mapping here means "Exit" quietly reloads, or "Reload" quits
        // — both silent, and both discovered at the worst moment.
        assert_eq!(action_for(ID_OPEN_TUI), Some(TrayAction::OpenTui));
        assert_eq!(action_for(ID_RELOAD), Some(TrayAction::Reload));
        assert_eq!(action_for(ID_EXIT), Some(TrayAction::Exit));
    }

    #[test]
    fn an_unknown_command_id_does_nothing() {
        // WM_COMMAND also carries accelerator and control notifications; an
        // unrecognised id must not be treated as a menu pick.
        assert_eq!(action_for(0), None);
        assert_eq!(action_for(999), None);
    }

    #[test]
    fn the_ids_are_distinct() {
        let ids = [ID_OPEN_TUI, ID_RELOAD, ID_EXIT];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate menu ids");
    }
}
