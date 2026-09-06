//! pid -> executable stem, in one place.
//!
//! # Why this exists
//!
//! This walk was written twice — `desktop::process_name` and
//! `term_opacity::pid_is_target` — with the same ToolHelp snapshot, the same
//! UTF-16 trimming and two subtly different ways of stripping `.exe`. Both are
//! now thin wrappers over this module.
//!
//! # Why [`map`] and not just [`of`]
//!
//! Every existing caller looks up ONE pid occasionally, so a fresh snapshot per
//! call was free. The native window source is different: it resolves a name for
//! every window on every enumeration, and a snapshot walks *every process on
//! the machine*. Doing that per window turns an O(windows) pass into
//! O(windows x processes).
//!
//! So the expensive part is exposed directly: take one snapshot, look up as
//! many pids as you like against it.

use std::collections::HashMap;

/// Every running process, as `pid -> lower-case stem` (`"alacritty"`).
///
/// The stem has no extension and no case: both are noise for every question
/// panefx asks, and normalising once here means no caller has to remember to.
#[cfg(windows)]
pub fn map() -> HashMap<u32, String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            // An empty map, never a panic. Failing to name a process must
            // degrade to "no backdrop for it", not take the daemon down.
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            out.insert(e.th32ProcessID, stem_of(&e.szExeFile));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn map() -> HashMap<u32, String> {
    HashMap::new()
}

/// One pid's executable stem. Convenience over [`map`] for occasional lookups.
pub fn of(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    map().remove(&pid)
}

/// `PROCESSENTRY32W::szExeFile` -> `"alacritty"`.
///
/// Split out and tested because the trimming is where the two original copies
/// disagreed: the field is a FIXED 260-wide buffer padded with NULs, so a naive
/// `from_utf16_lossy` yields a string with a long NUL tail, and
/// `trim_end_matches(".exe")` on that tail does nothing at all.
pub fn stem_of(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]).to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str, pad_to: usize) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.resize(pad_to, 0);
        v
    }

    /// The bug both originals were working around, pinned.
    ///
    /// `szExeFile` is a fixed 260-wide buffer. Trimming must stop at the first
    /// NUL, or the `.exe` suffix is buried mid-string and never stripped.
    #[test]
    fn a_nul_padded_buffer_still_yields_a_clean_stem() {
        assert_eq!(stem_of(&wide("alacritty.exe", 260)), "alacritty");
        assert_eq!(stem_of(&wide("Neovide.EXE", 260)), "neovide");
    }

    #[test]
    fn a_name_without_an_extension_survives() {
        assert_eq!(stem_of(&wide("System", 260)), "system");
    }

    /// `.exe` must only be stripped from the END -- a process genuinely called
    /// something like `exe-runner` must not lose characters from the middle.
    #[test]
    fn only_a_trailing_extension_is_stripped() {
        assert_eq!(stem_of(&wide("exeplorer.exe", 260)), "exeplorer");
    }

    #[test]
    fn an_unpadded_buffer_works_too() {
        assert_eq!(stem_of(&wide("panefx.exe", 10)), "panefx");
    }

    #[test]
    fn pid_zero_is_never_a_process() {
        assert_eq!(of(0), None);
    }
}
