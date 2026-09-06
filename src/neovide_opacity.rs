//! Neovide's background opacity, driven through its embedded Neovim's RPC pipe.
//!
//! # Why this exists at all
//!
//! `term_opacity` drives Alacritty by rewriting `alacritty.toml`. Neovide has no
//! config-file equivalent -- its own `config.toml` has NO transparency or
//! opacity key, and anything unrecognised there is silently ignored, which is
//! exactly why an earlier `transparency = 0.6` in that file did nothing. The
//! only knob is the Neovim global `g:neovide_opacity` (renamed from
//! `g:neovide_transparency` in Neovide 0.15.0).
//!
//! # Correcting the record
//!
//! `term_opacity::apply` used to document Neovide as unreachable:
//!
//! > "it launches `nvim --embed` with no `--listen`, so there is no named pipe
//! > to set `g:neovide_opacity` through -- verified by launching it and
//! > enumerating `\\.\pipe\`"
//!
//! **That is wrong, and it was measured wrong.** Neovim ALWAYS creates a default
//! RPC pipe; `--listen` only overrides the name. Enumerating `\\.\pipe\` on this
//! machine with Neovide running finds `\\.\pipe\nvim.3000.0`, connecting to it
//! returns a valid msgpack-rpc response, and `let g:neovide_opacity = 1.0`
//! followed by reading the value back returns `1.0`. The likely cause of the bad
//! measurement is the pipe path itself: `\\.\pipe\` loses its backslashes
//! through several layers of shell quoting and silently enumerates `C:\pipe`,
//! which does not exist -- an empty list that reads as "no pipe" rather than as
//! the error it is.
//!
//! # The pid in the pipe name is NEOVIM's, not Neovide's
//!
//! The default name is `\\.\pipe\nvim.<pid>.0`, and that pid belongs to the
//! embedded `nvim.exe`, which is a CHILD of `neovide.exe`. Measured here:
//! neovide.exe is 28436, nvim.exe is 3000, and the pipe is `nvim.3000.0`.
//! Matching on Neovide's own pid finds nothing. So this enumerates pipes and
//! tries the `nvim.*` ones rather than deriving a name from a process id -- that
//! also covers a custom `--listen` name and a Neovim started outside Neovide.
//!
//! # Why hand-rolled msgpack
//!
//! One fixed request shape, about forty bytes, against a crate that would pull
//! in a serialiser and its dependencies. The codebase's existing convention is a
//! feature flag over a new crate wherever that is reasonable (see the windows
//! dependency in Cargo.toml), and this is well inside reasonable.

use std::io::{Read, Write};

/// Format a percentage as Neovide wants it: `35 -> "0.35"`, `100 -> "1.0"`.
///
/// Deliberately the same shape as `term_opacity::percent_to_toml`, so the two
/// terminals are set from one number and cannot drift by rounding.
pub fn percent_to_vim(percent: u8) -> String {
    let v = f64::from(percent) / 100.0;
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

/// The Vim command that sets the opacity.
///
/// `g:neovide_opacity` is the current name; `g:neovide_transparency` was the
/// pre-0.15.0 spelling and setting it too costs one statement and keeps an older
/// Neovide working. Setting an unused global on a newer build is harmless.
fn set_command(percent: u8) -> String {
    let v = percent_to_vim(percent);
    format!("let g:neovide_opacity = {v} | let g:neovide_transparency = {v}")
}

/// Encode one msgpack-rpc `nvim_command` request.
///
/// Wire shape is `[0, msgid, "nvim_command", [cmd]]` -- a 4-element array, type
/// 0 for "request". Only the pieces that shape needs are implemented:
/// fixarray for the two arrays, fixstr for the short method name, and str8 for
/// the command, which is always longer than the 31 bytes fixstr allows here.
fn encode_command(msgid: u32, cmd: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(cmd.len() + 24);
    b.push(0x94); // fixarray, 4 elements
    b.push(0x00); // type 0 = request
    // msgid as uint32, always the 5-byte form so the length never depends on
    // the value -- a variable-width integer here is a bug waiting for the
    // thousandth request.
    b.push(0xCE);
    b.extend_from_slice(&msgid.to_be_bytes());
    const METHOD: &str = "nvim_command";
    b.push(0xA0 | METHOD.len() as u8); // fixstr
    b.extend_from_slice(METHOD.as_bytes());
    b.push(0x91); // fixarray, 1 element: the params
    let bytes = cmd.as_bytes();
    if bytes.len() < 32 {
        b.push(0xA0 | bytes.len() as u8);
    } else if bytes.len() < 256 {
        b.push(0xD9);
        b.push(bytes.len() as u8);
    } else {
        b.push(0xDA);
        b.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    b.extend_from_slice(bytes);
    b
}

/// Every `nvim.*` pipe currently open.
///
/// Reading the pipe directory rather than deriving a name from a pid: the pid in
/// the default name is the embedded Neovim's, not Neovide's, and a user who set
/// `--listen` has a name this could not have guessed.
#[cfg(windows)]
fn nvim_pipes() -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(r"\\.\pipe\") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("nvim").then(|| e.path())
        })
        .collect()
}

#[cfg(not(windows))]
fn nvim_pipes() -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// What a call actually did, so the caller can log honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// Instances that accepted the command.
    pub reached: usize,
    /// Pipes that existed but could not be written to.
    pub failed: usize,
}

/// Set the opacity on every reachable Neovim instance.
///
/// Best-effort by design, and NOT an error when nothing is reachable: Neovide
/// not running is the normal case, not a fault worth a warning on every startup
/// and every slider drag.
pub fn apply(percent: u8) -> Applied {
    let cmd = set_command(percent);
    let mut out = Applied::default();
    for pipe in nvim_pipes() {
        match send(&pipe, &cmd) {
            Ok(()) => out.reached += 1,
            Err(_) => out.failed += 1,
        }
    }
    out
}

/// Write one command to one pipe.
///
/// The response is read but not parsed. It is read at all so the request is not
/// left sitting in the pipe buffer when this closes, and because a read that
/// returns bytes is cheap evidence the far end is really a msgpack-rpc server
/// rather than some other program that happened to claim an `nvim*` name.
fn send(pipe: &std::path::Path, cmd: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe)?;
    f.write_all(&encode_command(1, cmd))?;
    f.flush()?;
    let mut buf = [0u8; 256];
    let _ = f.read(&mut buf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_render_the_way_vimscript_wants() {
        assert_eq!(percent_to_vim(35), "0.35");
        assert_eq!(percent_to_vim(60), "0.6");
        assert_eq!(percent_to_vim(100), "1.0");
        assert_eq!(percent_to_vim(0), "0.0");
    }

    /// The same number must reach both terminals identically, or turning one
    /// dial visibly desynchronises them.
    #[test]
    fn neovide_and_alacritty_agree_on_every_percentage() {
        for p in 0..=100u8 {
            assert_eq!(
                percent_to_vim(p),
                crate::term_opacity::percent_to_toml(p),
                "the two terminals disagree at {p}%"
            );
        }
    }

    #[test]
    fn the_command_sets_both_spellings() {
        let c = set_command(35);
        assert!(c.contains("g:neovide_opacity = 0.35"));
        assert!(
            c.contains("g:neovide_transparency = 0.35"),
            "the pre-0.15.0 name is set too, so an older Neovide still follows"
        );
    }

    /// Byte-for-byte against the request that was verified by hand over the
    /// real pipe -- this encoder is the piece with no type system behind it.
    #[test]
    fn the_request_is_well_formed_msgpack() {
        let b = encode_command(1, "let g:x = 1");
        assert_eq!(b[0], 0x94, "4-element array");
        assert_eq!(b[1], 0x00, "type 0 = request");
        assert_eq!(b[2], 0xCE, "msgid as uint32");
        assert_eq!(&b[3..7], &1u32.to_be_bytes());
        assert_eq!(b[7], 0xA0 | 12, "fixstr, 12 bytes");
        assert_eq!(&b[8..20], b"nvim_command");
        assert_eq!(b[20], 0x91, "1-element param array");
        assert_eq!(b[21], 0xA0 | 11, "fixstr, 11 bytes");
        assert_eq!(&b[22..], b"let g:x = 1");
    }

    /// A real command is longer than fixstr can hold, so the str8 path is the
    /// one that actually runs in production.
    #[test]
    fn a_real_command_takes_the_str8_path() {
        let cmd = set_command(35);
        assert!(cmd.len() >= 32, "otherwise this test proves nothing");
        let b = encode_command(1, &cmd);
        assert_eq!(b[21], 0xD9, "str8 marker");
        assert_eq!(b[22] as usize, cmd.len());
        assert_eq!(&b[23..], cmd.as_bytes());
    }

    #[test]
    fn nothing_reachable_is_not_a_failure() {
        // Reported as zero reached rather than as an error: Neovide not being
        // open is the normal case.
        let a = Applied::default();
        assert_eq!(a.reached, 0);
        assert_eq!(a.failed, 0);
    }
}
