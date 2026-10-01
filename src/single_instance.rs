//! One daemon per session, and a way to SEE when a second one was refused.
//!
//! # Why this exists
//!
//! Losing the control port was deliberately survivable — see
//! `control::Server::bind`, which documents `None` as "port unavailable, carry
//! on without a control channel". That is the right call for the *control
//! channel* and the wrong call for the *daemon*: a second `panefx --daemon`
//! kept rendering with no way to reach it, so `panefx-ctl` and the GUI talked
//! to one instance while another drew on top. Every setting appeared to do
//! nothing, `pane_off` included.
//!
//! Measured 2026-09-05 on SloppyLaptopy: seven daemons in one logon, six of
//! them dead inside 58 seconds, each fully initialised to ~92MB first. The
//! visible symptom was "panefx cannot be switched off".
//!
//! # Why a mutex and not the port
//!
//! Binding the port IS a mutual exclusion, so gating on it would work. A named
//! mutex is used instead for two reasons:
//!
//!   * It still holds when the control channel is not running at all. The port
//!     can legitimately be taken by something else, and "some unrelated program
//!     owns 6124" must not be reported as "panefx is already running".
//!   * It is released by the kernel when the process dies, however it dies.
//!     There is no stale lock file to clean up after a `taskkill /F`, which is
//!     exactly how GlazeWM's `shutdown_commands` stops this program.
//!
//! `Local\` rather than `Global\`: the scope that matters is the logon session.
//! Two users on one machine each get their own desktop, their own GlazeWM and
//! their own daemon, and `Global\` would let the first of them lock out the
//! second.
//!
//! # Catching it
//!
//! Refusing to start is not enough on its own. This binary is
//! `windows_subsystem = "windows"`, so a refused instance has no console, no
//! log the user will ever read, and it exits immediately — the duplicate would
//! be prevented *and* invisible, which is how the original problem stayed
//! unexplained for so long.
//!
//! So the loser reports itself to the winner over the control channel before
//! exiting, and the winner writes it to the in-memory log. The GUI's logs tab
//! then says, in words, that a second daemon tried to start and was refused.

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Held for the daemon's whole life. Dropping it releases the mutex.
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Best effort: the kernel releases the mutex on process exit anyway,
        // so a failure here changes nothing that matters.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Try to become THE daemon for this logon session.
///
/// `Ok(lock)` means this process owns it and must keep `lock` alive.
/// `Err(())` means another daemon already has it and this one should exit.
pub fn acquire() -> Result<InstanceLock, ()> {
    acquire_named(w!("Local\\panefx-daemon"))
}

/// The same lock for the GUI: one control panel per logon session.
///
/// Exists because the GUI's old guard looked for an existing panefx-gui WINDOW,
/// and a GUI takes a moment to open its window. Clicking the tray icon five
/// times quickly started five processes before any of them had a window, so
/// every one passed the check and five control panels opened (Michael,
/// 2026-09-30). The kernel object exists from the first instruction of the
/// first instance, so there is no window to wait for.
pub fn acquire_gui() -> Result<InstanceLock, ()> {
    acquire_named(w!("Local\\panefx-gui"))
}

fn acquire_named(name: windows::core::PCWSTR) -> Result<InstanceLock, ()> {
    unsafe {
        // `binitialowner: false` — ownership of the mutex is irrelevant here.
        // Its mere existence is the signal, and not taking ownership means an
        // abandoned-mutex state can never arise.
        let handle = match CreateMutexW(None, false, name) {
            Ok(h) => h,
            // If the mutex cannot be created at all, do NOT refuse to start:
            // that would turn an unexpected OS failure into "panefx no longer
            // runs", which is far worse than the duplicate this guards against.
            Err(_) => return Err(()),
        };

        // CreateMutexW SUCCEEDS when the mutex already exists — it returns a
        // handle to the existing one. The only way to tell the two apart is
        // the last-error value, which must be read immediately.
        if windows::Win32::Foundation::GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return Err(());
        }
        Ok(InstanceLock(handle))
    }
}

/// Tell the daemon that already holds the lock that we were turned away.
///
/// Deliberately silent on failure and short-timeout: this runs on a process
/// that is about to exit, and the incumbent may be mid-frame or (if the port
/// is genuinely held by something unrelated) not a panefx at all. Nothing here
/// is allowed to hang a logon.
pub fn report_refused_to_incumbent(port: u16) {
    use std::io::Write;
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let Ok(mut addrs) = ("127.0.0.1", port).to_socket_addrs() else {
        return;
    };
    let Some(addr) = addrs.next() else { return };
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return;
    };
    let _ = sock.set_write_timeout(Some(Duration::from_millis(300)));
    let line = format!(
        "{{\"cmd\":\"duplicate\",\"pid\":{}}}\n",
        std::process::id()
    );
    let _ = sock.write_all(line.as_bytes());
    let _ = sock.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI bug: five quick tray clicks opened five panels. With the lock,
    /// a second holder of the same name is refused while the first is alive,
    /// and the name is free again the moment the first lets go.
    #[test]
    fn a_second_instance_is_refused_until_the_first_is_gone() {
        let name = w!("Local\\panefx-test-single-instance");
        let first = acquire_named(name).expect("the first instance gets the lock");
        assert!(acquire_named(name).is_err(), "a second instance must be refused");
        drop(first);
        assert!(acquire_named(name).is_ok(), "and the lock is free once the first exits");
    }
}
