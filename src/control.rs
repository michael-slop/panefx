//! Control channel: a localhost TCP listener the TUI talks to.
//!
//! Newline-delimited JSON, one command per line, one JSON reply per command.
//! Deliberately plain: no framing library, no async runtime.
//!
//! **Threading:** the RENDER LOOP is single-threaded and must never block, so
//! this listener and every accepted stream are non-blocking and drained once
//! per frame (`WouldBlock` means "nothing to do", not an error). The daemon as
//! a whole is no longer single-threaded — the GlazeWM socket was moved to its
//! own thread (`ipc::IpcThread`) because reading it inline burned 5ms of every
//! frame. Only that socket moved; the panel map, GDI handles and HWNDs are
//! thread-affine and stay here.
//!
//! This channel could move to a thread too, but it is genuinely non-blocking
//! already and costs nothing measurable, so it has not earned the complexity.
//!
//! Binding is best-effort. If the port is taken, panefx logs and runs headless
//! rather than refusing to start: a backdrop you cannot tune beats no backdrop.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

use serde::{Deserialize, Serialize};

use crate::animation::{Param, ParamValue};
use crate::config::Config;

pub const DEFAULT_PORT: u16 = 6124;

/// Commands the TUI can send.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    /// Full state snapshot: config, current effect, its params, effect list.
    Get,
    /// Set a top-level `Config` field by name.
    Set { key: String, val: serde_json::Value },
    /// Switch to an effect immediately.
    Effect { name: String },
    /// Set one parameter on the *current* effect.
    Param { key: String, val: ParamValue },
    /// Write the live state to config.toml.
    Save,
    /// Reload from config.toml, discarding unsaved live changes.
    Revert,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub effect: String,
    pub effects: Vec<String>,
    pub params: Vec<Param>,
    pub config: ConfigView,
}

/// The subset of `Config` the TUI can see and edit. Flat and stringly-typed on
/// purpose so the TUI can render it generically alongside effect params.
#[derive(Debug, Serialize)]
pub struct ConfigView {
    pub font: String,
    pub cell_w: i32,
    pub cell_h: i32,
    pub fps: u64,
    pub crop_top: i32,
    pub pad_x: i32,
    pub pad_y: i32,
    pub rotation: Vec<String>,
    pub rotate_secs: u64,
}

impl ConfigView {
    pub fn of(cfg: &Config) -> Self {
        ConfigView {
            font: cfg.font.clone(),
            cell_w: cfg.cell_w,
            cell_h: cfg.cell_h,
            fps: cfg.fps,
            crop_top: cfg.crop_top,
            pad_x: cfg.pad_x,
            pad_y: cfg.pad_y,
            rotation: cfg.rotation.clone(),
            rotate_secs: cfg.rotate_every.map(|d| d.as_secs()).unwrap_or(0),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
}

impl Reply {
    pub fn ok() -> Self {
        Reply {
            ok: true,
            error: None,
            snapshot: None,
        }
    }
    pub fn with(s: Snapshot) -> Self {
        Reply {
            ok: true,
            error: None,
            snapshot: Some(s),
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Reply {
            ok: false,
            error: Some(msg.into()),
            snapshot: None,
        }
    }
}

/// A connected TUI. Held for the life of the connection so a long-lived TUI
/// sees live updates without reconnecting.
struct Peer {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

pub struct Server {
    listener: TcpListener,
    peers: Vec<Peer>,
}

impl Server {
    /// Bind to localhost. `Ok(None)` means "port unavailable, carry on without
    /// a control channel" — deliberately not an error.
    pub fn bind(port: u16) -> Option<Self> {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                if l.set_nonblocking(true).is_err() {
                    eprintln!("[panefx] control: could not set non-blocking; disabled");
                    return None;
                }
                println!("[panefx] control channel on 127.0.0.1:{port}");
                Some(Server {
                    listener: l,
                    peers: Vec::new(),
                })
            }
            Err(e) => {
                eprintln!(
                    "[panefx] control channel unavailable on port {port} ({e}); \
                     running without live control"
                );
                None
            }
        }
    }

    /// Accept any pending connections and read whatever whole lines are ready.
    ///
    /// Never blocks. Returns the commands received this frame, paired with the
    /// index of the peer that sent each one so replies go to the right place.
    pub fn poll(&mut self) -> Vec<(usize, Command)> {
        // Accept.
        loop {
            match self.listener.accept() {
                Ok((s, _)) => {
                    if s.set_nonblocking(true).is_err() {
                        continue;
                    }
                    match s.try_clone() {
                        Ok(c) => self.peers.push(Peer {
                            reader: BufReader::new(c),
                            stream: s,
                        }),
                        Err(_) => continue,
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let mut out = Vec::new();
        let mut dead = Vec::new();

        for (i, p) in self.peers.iter_mut().enumerate() {
            loop {
                let mut line = String::new();
                match p.reader.read_line(&mut line) {
                    Ok(0) => {
                        dead.push(i);
                        break;
                    }
                    Ok(_) => {
                        let t = line.trim();
                        if t.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Command>(t) {
                            Ok(c) => out.push((i, c)),
                            Err(e) => {
                                // Malformed input must not kill the daemon.
                                let _ = writeln!(
                                    p.stream,
                                    "{}",
                                    serde_json::to_string(&Reply::err(format!("bad command: {e}")))
                                        .unwrap_or_default()
                                );
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        dead.push(i);
                        break;
                    }
                }
            }
        }

        for i in dead.into_iter().rev() {
            if i < self.peers.len() {
                self.peers.remove(i);
            }
        }
        out
    }

    pub fn reply(&mut self, peer: usize, r: &Reply) {
        if let Some(p) = self.peers.get_mut(peer) {
            if let Ok(s) = serde_json::to_string(r) {
                let _ = writeln!(p.stream, "{s}");
                let _ = p.stream.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command_shape() {
        let cases = [
            r#"{"cmd":"get"}"#,
            r#"{"cmd":"set","key":"fps","val":30}"#,
            r#"{"cmd":"effect","name":"rain"}"#,
            r#"{"cmd":"param","key":"tick_ms","val":{"kind":"int","v":40}}"#,
            r#"{"cmd":"param","key":"head","val":{"kind":"colour","r":200,"g":255,"b":200}}"#,
            r#"{"cmd":"save"}"#,
            r#"{"cmd":"revert"}"#,
        ];
        for c in cases {
            assert!(
                serde_json::from_str::<Command>(c).is_ok(),
                "failed to parse: {c}"
            );
        }
    }

    #[test]
    fn rejects_nonsense_without_panicking() {
        assert!(serde_json::from_str::<Command>("{").is_err());
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"nope"}"#).is_err());
    }

    #[test]
    fn bind_failure_is_not_fatal() {
        // Hold the port, then confirm a second bind reports None rather than
        // panicking or erroring out.
        let port = 6199;
        let _held = TcpListener::bind(("127.0.0.1", port)).expect("test port");
        assert!(
            Server::bind(port).is_none(),
            "a taken port must degrade to no-control, not kill the daemon"
        );
    }

    #[test]
    fn reply_serialises_without_null_noise() {
        let s = serde_json::to_string(&Reply::ok()).unwrap();
        assert_eq!(s, r#"{"ok":true}"#);
    }
}
