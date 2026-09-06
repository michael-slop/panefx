//! Background opacity, driven through the terminal's own setting.
//!
//! # Why not layered windows
//!
//! An earlier version of this used
//! `SetLayeredWindowAttributes(hwnd, None, alpha, LWA_ALPHA)`, which is WRONG
//! for this job. That applies a uniform alpha to **every pixel of the window,
//! including the text** — at 60% the terminal's text is 60% opaque and harder to
//! read, and the backdrop bleeds through the letters as much as through the gaps
//! between them.
//!
//! What is wanted is **per-pixel** alpha: the background fades, the glyphs stay
//! solid, and turning the dial makes the effect more prominent *behind readable
//! text*. Alacritty already does exactly that with `window.opacity` — it renders
//! with an alpha channel through winit/DWM rather than layering the window.
//!
//! So this drives Alacritty's own setting instead of second-guessing it. There
//! is no IPC to do that with on Windows (`alacritty.exe` exposes only `migrate`
//! and `help`; `ipc_socket` is documented unix-only), so the only live route is
//! rewriting the config file and letting `live_config_reload` pick it up.
//! Measured: a write lands on screen in about two seconds with no restart.
//!
//! # Why the rewrite is line-oriented and paranoid
//!
//! This edits a file panefx does not own, whose comments are the user's notes.
//! In the real config, **three of the four lines matching "opacity" are
//! comments** — a regex over the whole file eats them. So: skip comment lines,
//! rewrite only a real assignment, never invent the key, and write atomically.

use std::path::{Path, PathBuf};

/// What a rewrite actually did, so the caller can report it honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The assignment was found and updated.
    Written,
    /// Already the requested value; the file was not touched.
    AlreadyCorrect,
    /// No `opacity = ...` assignment exists.
    ///
    /// Deliberately NOT an error, and deliberately not fixed by appending one:
    /// panefx cannot know which `[table]` the end of the file belongs to, and a
    /// key written under the wrong one is silently ignored by Alacritty. Report
    /// it and let the user add the line where they want it.
    KeyMissing,
}

/// Where Alacritty keeps its config.
///
/// `PANEFX_ALACRITTY_CONFIG` overrides it, which is also how the tests drive a
/// temp file without needing Alacritty installed.
pub fn alacritty_config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PANEFX_ALACRITTY_CONFIG") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    std::env::var("APPDATA")
        .ok()
        .map(|a| PathBuf::from(a).join("alacritty").join("alacritty.toml"))
}

/// Format a percentage the way Alacritty's TOML wants it.
///
/// `60 -> "0.6"`, `100 -> "1.0"`, `35 -> "0.35"`. Always keeps a decimal point:
/// bare `1` parses fine as a TOML integer but reads as a mistake next to the
/// other values.
pub fn percent_to_toml(percent: u8) -> String {
    let v = f64::from(percent) / 100.0;
    let s = format!("{v:.2}");
    // Trim one trailing zero so 0.60 reads as 0.6, but never strip to "0." or "1".
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

/// Is this line the real `opacity = ...` assignment?
///
/// Comment lines are skipped even when they mention opacity — the real file has
/// three of those, written by panefx itself.
fn is_opacity_assignment(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('#') {
        return false;
    }
    let Some(rest) = t.strip_prefix("opacity") else {
        return false;
    };
    // `opacity = 0.6` and `opacity=0.6`, but not `opacity_foo = 1`.
    rest.trim_start().starts_with('=')
}

/// Rewrite only the `opacity` assignment, preserving everything else byte for
/// byte — comments, ordering, indentation and line endings included.
pub fn set_alacritty_opacity(path: &Path, percent: u8) -> anyhow::Result<Outcome> {
    let text = std::fs::read_to_string(path)?;
    let wanted = percent_to_toml(percent);

    // Preserve the file's line endings rather than normalising them: rewriting
    // a CRLF file with LF would show up as every line changed in a diff.
    let crlf = text.contains("\r\n");
    let eol = if crlf { "\r\n" } else { "\n" };
    let had_trailing_eol = text.ends_with('\n');

    let mut found = false;
    let mut changed = false;
    let mut out: Vec<String> = Vec::new();

    for line in text.lines() {
        if !found && is_opacity_assignment(line) {
            found = true;
            // Keep any indentation and any trailing comment on the line.
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let trailing = line.find('#').map(|i| line[i..].to_string());
            let new_line = match trailing {
                Some(c) => format!("{indent}opacity = {wanted}   {c}"),
                None => format!("{indent}opacity = {wanted}"),
            };
            if new_line != line {
                changed = true;
            }
            out.push(new_line);
        } else {
            out.push(line.to_string());
        }
    }

    if !found {
        return Ok(Outcome::KeyMissing);
    }
    if !changed {
        return Ok(Outcome::AlreadyCorrect);
    }

    let mut rebuilt = out.join(eol);
    if had_trailing_eol {
        rebuilt.push_str(eol);
    }

    // Atomic: write a sibling temp file then rename over the original.
    //
    // `live_config_reload` is watching this file. A partial write would either
    // be parsed as a broken config or truncate the user's settings outright, so
    // the file must never exist in a half-written state.
    //
    // Written as raw bytes with NO BOM. A UTF-8 BOM in a config a parser reads
    // is the trap that broke GlazeWM's YAML earlier — the file looks perfect and
    // the parser rejects the whole thing.
    let tmp = path.with_extension("toml.panefx-tmp");
    std::fs::write(&tmp, rebuilt.as_bytes())?;
    std::fs::rename(&tmp, path)?;

    Ok(Outcome::Written)
}

/// Write the opacity to whichever terminal configs panefx can reach.
///
/// Today that is Alacritty only. **Neovide is not reachable**: it launches
/// `nvim --embed` with no `--listen`, so there is no named pipe to set
/// `g:neovide_opacity` through — verified by launching it and enumerating
/// `\\.\pipe\`. Rather than pretend, panefx leaves Neovide alone; if it is ever
/// launched with `--listen`, this is where that support would go.
pub fn apply(percent: u8) -> anyhow::Result<Outcome> {
    let Some(path) = alacritty_config_path() else {
        anyhow::bail!("no APPDATA, so alacritty.toml cannot be located");
    };
    if !path.exists() {
        anyhow::bail!("{} does not exist", path.display());
    }
    set_alacritty_opacity(&path, percent)
}

/// Strip `WS_EX_LAYERED` from any target window that still carries it.
///
/// panefx briefly applied a WHOLE-WINDOW alpha with
/// `SetLayeredWindowAttributes`, which faded the terminal's text along with its
/// background. That approach is gone, but the window style it set does NOT go
/// away when the code does — it persists on every already-open window until
/// that window closes, silently stacking with the per-pixel alpha and dimming
/// text for a reason nothing in the source explains any more.
///
/// Best-effort and silent: a window that was never layered is left alone, and a
/// failure here is not worth bothering the user about.
#[cfg(windows)]
pub fn clear_legacy_layered_styles() {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowThreadProcessId, SetWindowLongPtrW, GWL_EXSTYLE,
        WS_EX_LAYERED,
    };

    unsafe extern "system" fn cb(hwnd: HWND, _l: LPARAM) -> BOOL {
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if style & (WS_EX_LAYERED.0 as isize) == 0 {
                return BOOL(1);
            }
            // Only touch windows belonging to a process panefx targets, so we
            // never strip layering from an application that set it for its own
            // reasons.
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if !crate::term_opacity::pid_is_target(pid) {
                return BOOL(1);
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_LAYERED.0 as isize));
            BOOL(1)
        }
    }

    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(0));
    }
}

#[cfg(not(windows))]
pub fn clear_legacy_layered_styles() {}

/// Does this PID belong to a process panefx draws behind?
///
/// Deliberately conservative — used only to decide whether to strip a leftover
/// window style, and stripping one from an unrelated app would be rude.
#[cfg(windows)]
pub fn pid_is_target(pid: u32) -> bool {
    // One shared walk (see crate::proc_name) instead of a second copy of the
    // ToolHelp loop that disagreed with desktop.rs about trimming.
    match crate::proc_name::of(pid) {
        Some(stem) => crate::ipc::DEFAULT_TARGETS
            .iter()
            .any(|t| stem.eq_ignore_ascii_case(t)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture shaped like the real config: an `[window]` table, the real
    /// assignment, and — critically — comment lines that mention "opacity".
    const SAMPLE: &str = "\
[general]
live_config_reload = true

[window]
decorations = \"None\"
# panefx writes the opacity line below. Hand edits are overwritten.
#
# NOTE: this comment mentions opacity twice; opacity = 9.9 is not real.
opacity = 0.6
padding = { x = 10, y = 8 }

[font]
size = 11
";

    fn tmpfile(name: &str, body: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("panefx-test-{name}.toml"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn percent_maps_to_the_decimal_alacritty_wants() {
        assert_eq!(percent_to_toml(60), "0.6");
        assert_eq!(percent_to_toml(100), "1.0");
        assert_eq!(percent_to_toml(35), "0.35");
        assert_eq!(percent_to_toml(10), "0.1");
        // Never a bare integer — "1" is valid TOML but reads as a typo.
        assert!(percent_to_toml(100).contains('.'));
    }

    #[test]
    fn a_commented_line_is_never_the_assignment() {
        // THE trap: the real config has three comment lines containing the word
        // "opacity", including one with `opacity = 9.9` inside prose.
        assert!(!is_opacity_assignment("# opacity = 0.5"));
        assert!(!is_opacity_assignment("   # opacity = 0.5"));
        assert!(!is_opacity_assignment("# NOTE: opacity = 9.9 is not real."));
        assert!(is_opacity_assignment("opacity = 0.6"));
        assert!(is_opacity_assignment("  opacity=0.6"));
    }

    #[test]
    fn a_similarly_named_key_is_not_matched() {
        assert!(!is_opacity_assignment("opacity_mode = 'x'"));
        assert!(!is_opacity_assignment("background_opacity = 0.5"));
    }

    #[test]
    fn rewrites_only_the_assignment_and_keeps_every_comment() {
        let p = tmpfile("only-assignment", SAMPLE);
        assert_eq!(set_alacritty_opacity(&p, 35).unwrap(), Outcome::Written);
        let after = std::fs::read_to_string(&p).unwrap();

        // The one line that should differ.
        assert!(after.contains("opacity = 0.35"));
        assert!(!after.contains("opacity = 0.6"));

        // Everything else byte-identical, line for line.
        let before_lines: Vec<&str> = SAMPLE.lines().collect();
        let after_lines: Vec<&str> = after.lines().collect();
        assert_eq!(before_lines.len(), after_lines.len());
        let differing: Vec<usize> = before_lines
            .iter()
            .zip(&after_lines)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(differing.len(), 1, "exactly one line may change");

        // The prose comment survived intact.
        assert!(after.contains("# NOTE: this comment mentions opacity twice; opacity = 9.9 is not real."));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_missing_key_is_reported_not_invented() {
        // Appending would land the key in whatever table happens to be last —
        // here `[font]` — where Alacritty silently ignores it.
        let p = tmpfile("missing", "[window]\ndecorations = \"None\"\n\n[font]\nsize = 11\n");
        assert_eq!(set_alacritty_opacity(&p, 50).unwrap(), Outcome::KeyMissing);
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(!after.contains("opacity"), "must not invent the key");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_unchanged_value_does_not_touch_the_file() {
        // Avoids waking live_config_reload for nothing on every startup.
        let p = tmpfile("unchanged", SAMPLE);
        assert_eq!(set_alacritty_opacity(&p, 60).unwrap(), Outcome::AlreadyCorrect);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), SAMPLE);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn no_bom_is_written() {
        // A UTF-8 BOM in a config a parser reads is the trap that broke
        // GlazeWM's YAML in this project's history.
        let p = tmpfile("bom", SAMPLE);
        set_alacritty_opacity(&p, 40).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_ne!(&bytes[..3.min(bytes.len())], &[0xEF, 0xBB, 0xBF]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn crlf_line_endings_survive() {
        // Rewriting a CRLF file with LF shows up as EVERY line changed.
        let crlf = SAMPLE.replace('\n', "\r\n");
        let p = tmpfile("crlf", &crlf);
        set_alacritty_opacity(&p, 25).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("\r\n"), "CRLF must be preserved");
        assert!(!after.contains("\n\n"), "no bare LF should appear");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_trailing_comment_on_the_line_is_kept() {
        let p = tmpfile("trailing", "[window]\nopacity = 0.6   # see-through\n");
        set_alacritty_opacity(&p, 80).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("opacity = 0.8"));
        assert!(after.contains("# see-through"), "trailing comment must survive");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let p = tmpfile("tempclean", SAMPLE);
        set_alacritty_opacity(&p, 45).unwrap();
        assert!(!p.with_extension("toml.panefx-tmp").exists());
        let _ = std::fs::remove_file(&p);
    }
}
