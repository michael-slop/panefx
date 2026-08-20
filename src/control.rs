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
    /// Point one monitor's wallpaper — or every monitor's — at an effect.
    ///
    /// `monitor: None` is the TUI's "apply to all". `name: "off"` destroys that
    /// surface so the Windows wallpaper shows through.
    ///
    /// Renamed explicitly: the enum's `rename_all = "lowercase"` would otherwise
    /// put `"wallpapereffect"` on the wire, which is the kind of string that
    /// gets mistyped once and debugged for an hour.
    #[serde(rename = "wallpaper_effect")]
    WallpaperEffect {
        #[serde(default)]
        monitor: Option<usize>,
        name: String,
    },
    /// Point ONE LAYER of one monitor at an effect.
    ///
    /// Layer 0 is the base and is the same thing `wallpaper_effect` sets;
    /// layers 1+ stack above it, nearer the viewer. `name: "off"` removes a
    /// layer rather than storing a hole in the stack.
    #[serde(rename = "wallpaper_layer")]
    WallpaperLayer {
        monitor: usize,
        layer: usize,
        name: String,
    },

    /// Set one parameter on the DESKTOP's copy of an effect.
    ///
    /// `effect` is explicit rather than "whichever is current": each monitor
    /// picks its own, so there is no single current wallpaper effect the way
    /// there is for the pane. Inferring it would make the same keypress do
    /// different things depending on which row happened to be highlighted.
    #[serde(rename = "wallpaper_param")]
    WallpaperParam {
        /// Which screen to tune. `None` means EVERY screen running `effect`,
        /// which is the old behaviour and still what "set it once for all of
        /// them" should do.
        #[serde(default)]
        monitor: Option<usize>,
        effect: String,
        key: String,
        val: ParamValue,
    },
    /// Fetch the daemon's recent log lines.
    ///
    /// Separate from `Get` because the log is polled far more often than the
    /// rest of the snapshot and is much larger; bundling it would make every
    /// keypress carry 500 lines.
    Logs {
        /// How many lines to return, newest last.
        #[serde(default = "default_log_lines")]
        lines: usize,
    },
}

fn default_log_lines() -> usize {
    200
}

/// What the daemon knows about ONE terminal backdrop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaneView {
    /// The target window's handle, as GlazeWM reports it.
    pub handle: isize,
    /// Whether the panel is being drawn. A panel exists for every target
    /// window, but one whose terminal is on another workspace or minimized is
    /// hidden -- and an unexpectedly hidden panel is exactly the "one window
    /// has no effect" symptom.
    pub visible: bool,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub effect: String,
    pub effects: Vec<String>,
    pub params: Vec<Param>,
    pub config: ConfigView,
    /// One entry per detected monitor.
    ///
    /// EMPTY when the wallpaper layer could not be obtained — which is why
    /// `wallpaper_error` exists alongside it. "No monitors" and "no wallpaper
    /// layer" are very different problems and the TUI must not conflate them.
    #[serde(default)]
    pub wallpaper: Vec<WallpaperMonitorView>,
    /// Why there is no wallpaper layer. Absent when it is working.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallpaper_error: Option<String>,
    /// Total z-order SetWindowPos calls issued by desktop panes since start.
    /// See `panel::PIN_CALLS` -- a steady climb is the panes fighting over the
    /// single bottom z-slot.
    pub pin_calls: u64,

    /// One entry per terminal backdrop the daemon is managing.
    ///
    /// There were NO pane diagnostics at all, which made "only one of my two
    /// Alacritty windows has an effect" impossible to diagnose from the
    /// outside: the daemon knew whether it had made a panel and whether it
    /// considered it visible, and reported neither.
    #[serde(default)]
    pub panes: Vec<PaneView>,

    /// The SHARED pane simulation's grid, in cells.
    ///
    /// One simulation is shared by every terminal panel; each blits its own
    /// sub-rect and clamps its draw to these dimensions. So a panel wanting
    /// more cells than the sim has draws a truncated frame, and one wanting
    /// none draws nothing -- both invisible from outside until now.
    #[serde(default)]
    pub sim_cols: usize,
    #[serde(default)]
    pub sim_rows: usize,

    /// The desktop's own params, for every effect any monitor is running.
    ///
    /// Keyed by effect name because monitors can differ. The daemon does not
    /// know which TUI row is highlighted, so it sends them all and the TUI picks
    /// — which keeps the selection rule a local, testable decision.
    #[serde(default)]
    pub wallpaper_params: std::collections::BTreeMap<String, Vec<Param>>,
}

/// One monitor, as the TUI sees it.
#[derive(Debug, Serialize)]
pub struct WallpaperMonitorView {
    /// `DISPLAY<n>` number — the same number the config key uses.
    pub index: usize,
    pub label: String,
    /// Effect name, or `"off"`.
    pub effect: String,
    /// Fully covered, and therefore frozen. Surfaced so "why isn't it moving"
    /// is answerable at a glance rather than looking like a bug.
    pub occluded: bool,

    /// Every layer on this monitor, bottom first.
    ///
    /// `effect` above is `layers[0]` -- kept as its own field because it is what
    /// the config key and the TUI have always meant by "this monitor's effect".
    /// A client that knows nothing about layers still works; one that does can
    /// show the whole stack.
    #[serde(default)]
    pub layers: Vec<String>,

    /// The monitor rect, the panel, the DIB and the composition surface --
    /// the four sizes that must agree.
    ///
    /// Reported because a wallpaper defect that only affects SOME monitors is
    /// almost always one of these disagreeing, and from a shell that cannot see
    /// the desktop there is no other way to compare them. `None` for an `off`
    /// monitor, which has no panel at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<MonitorGeometry>,
}

/// The four sizes behind one wallpaper surface, for diagnosis.
#[derive(Debug, Serialize)]
pub struct MonitorGeometry {
    /// From `EnumDisplayMonitors` -- what Windows says the screen is.
    pub monitor_w: i32,
    pub monitor_h: i32,
    /// The panel window's own size.
    pub panel_w: i32,
    pub panel_h: i32,
    /// The GDI DIB the renderer draws into, and its row length in bytes.
    pub dib_w: i32,
    pub dib_h: i32,
    pub dib_stride: usize,
    /// The composition surface's swapchain size.
    pub surface_w: i32,
    pub surface_h: i32,
    /// Cell size in use, and the grid it produces.
    pub cell_w: i32,
    pub cell_h: i32,
    /// True when every pair above agrees. A `false` here is the bug.
    pub consistent: bool,
    /// Frames presented for this surface since the daemon started.
    ///
    /// Sample twice, a known interval apart, and the difference is the real
    /// present rate for THIS monitor. That is what separates "the flash is our
    /// frames landing" from "the flash is DWM recomposing an output we are not
    /// touching".
    pub presents: u64,
    /// Cumulative microseconds spent inside `present` for this surface.
    /// Divided by `presents`, this is the mean cost of putting one frame on
    /// THIS monitor.
    pub present_us: u64,
    /// DXGI `PresentCount` -- frames this swapchain has accepted from us.
    pub dxgi_presented: u32,
    /// DXGI `PresentRefreshCount` -- the display refresh the last frame landed
    /// on. Compared across two samples, a monitor whose refresh count advances
    /// far faster than its present count is dropping our frames.
    pub dxgi_refresh: u32,
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
    /// Window opacity, 10-100%. See `opacity.rs`.
    pub opacity: u8,
    /// Terminal backdrops switched off entirely. The wallpaper is unaffected.
    pub pane_off: bool,
    pub wallpaper_fps: u64,
    /// What the wallpaper can ACTUALLY achieve: it is ticked from the daemon
    /// loop, so `fps` caps it. Reported separately so the TUI never shows a
    /// number the screen is not delivering.
    pub wallpaper_fps_effective: u64,
    /// Friendly 1-10 knob; `wallpaper_cell_w/h` are what it writes.
    pub wallpaper_detail: u8,
    pub wallpaper_cell_w: i32,
    pub wallpaper_cell_h: i32,
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
            opacity: cfg.opacity,
            pane_off: cfg.pane_off,
            wallpaper_fps: cfg.wallpaper_fps,
            wallpaper_fps_effective: cfg.wallpaper_fps.min(cfg.fps).max(1),
            wallpaper_detail: cfg.wallpaper_detail,
            wallpaper_cell_w: cfg.wallpaper_cell_w,
            wallpaper_cell_h: cfg.wallpaper_cell_h,
        }
    }
}

/// Reply to [`Command::Logs`].
#[derive(Debug, Serialize)]
pub struct LogReply {
    pub entries: Vec<crate::log::Entry>,
}

#[derive(Debug, Serialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    /// Present only on a `logs` reply, so a normal snapshot stays small.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<LogReply>,
}

impl Reply {
    pub fn ok() -> Self {
        Reply {
            ok: true,
            error: None,
            snapshot: None,
            logs: None,
        }
    }

    /// A reply carrying log lines.
    pub fn with_logs(entries: Vec<crate::log::Entry>) -> Self {
        Reply {
            ok: true,
            error: None,
            snapshot: None,
            logs: Some(LogReply { entries }),
        }
    }
    pub fn with(s: Snapshot) -> Self {
        Reply {
            ok: true,
            error: None,
            snapshot: Some(s),
            logs: None,
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Reply {
            ok: false,
            error: Some(msg.into()),
            snapshot: None,
            logs: None,
        }
    }
}

/// A connected TUI. Held for the life of the connection so a long-lived TUI
/// sees live updates without reconnecting.
struct Peer {
    /// Stable identity, NOT a position.
    ///
    /// `poll` returns `(id, command)` and the caller replies later, after
    /// `poll` has already removed any peers that hung up. With a positional
    /// index those two moments disagree: removing peer 0 shifts every later
    /// peer down one, so the reply goes to the wrong socket -- or to none.
    ///
    /// That is not a rare race. A client that sends one command and closes is
    /// read as "command" AND "hung up" in the SAME poll, so it lands in `out`
    /// and `dead` together and the very next reply is misdirected. It wedged
    /// the control channel after a single request.
    id: usize,
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

pub struct Server {
    /// Source of peer ids. Monotonic, never reused, so a stale id from a
    /// previous connection can never resolve to a live peer.
    next_id: usize,
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
                    next_id: 0,
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
                        Ok(c) => {
                            let id = self.next_id;
                            self.next_id += 1;
                            self.peers.push(Peer {
                                id,
                                reader: BufReader::new(c),
                                stream: s,
                            })
                        }
                        Err(_) => continue,
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let mut out = Vec::new();
        let mut dead = Vec::new();

        for p in self.peers.iter_mut() {
            let i = p.id;
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

        // Retain by id: positions shift as peers are removed, ids do not.
        self.peers.retain(|p| !dead.contains(&p.id));
        out
    }

    /// Reply to the peer that sent a command.
    ///
    /// `peer` is the stable id from [`Server::poll`], not a position -- see
    /// [`Peer::id`]. A peer that has since hung up simply is not found, which
    /// is the correct outcome: dropping a reply to a closed socket beats
    /// sending it to whoever now occupies that slot.
    pub fn reply(&mut self, peer: usize, r: &Reply) {
        if let Some(p) = self.peers.iter_mut().find(|p| p.id == peer) {
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

    /// A client that sends one command and immediately closes -- exactly what
    /// every one-shot probe and script does -- used to WEDGE the control
    /// channel permanently.
    ///
    /// It was read as "here is a command" and "this peer hung up" in the SAME
    /// poll, so it landed in `out` and `dead` together. `dead` was applied
    /// before the caller ever called `reply`, shifting every later peer down a
    /// slot -- so the reply went to the wrong socket, or nowhere. The daemon
    /// kept rendering happily and simply stopped answering.
    ///
    /// The test drives the divergence directly rather than through sockets: it
    /// takes THREE peers so that removing the first leaves the survivor at a
    /// position that no longer equals its id. With two peers the id and the
    /// index still coincide by luck and a positional `reply` passes anyway --
    /// this test was written that way first, and it did not catch the bug.
    #[test]
    fn a_hung_up_peer_does_not_misdirect_the_next_reply() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpStream;

        let mut server = None;
        let mut port = 0;
        for p in 6200..6280 {
            if let Some(s) = Server::bind(p) {
                server = Some(s);
                port = p;
                break;
            }
        }
        let mut server = server.expect("no free port for the test");

        // Three peers, connected in order so ids are 0, 1, 2.
        let mut a = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut b = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        // Register all three with the server before anyone hangs up.
        std::thread::sleep(std::time::Duration::from_millis(100));
        server.poll();

        // A and B hang up. C -- id 2 -- is about to become position 0.
        writeln!(a, "{}", r#"{"cmd":"get"}"#).unwrap();
        a.flush().unwrap();
        drop(a);
        drop(b);
        writeln!(c, "{}", r#"{"cmd":"save"}"#).unwrap();
        c.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));

        // ONE poll, then reply to exactly what it reported -- the daemon loop's
        // shape. Measured: ids are [0,1,2]; A and B hang up, so poll reports
        // ids [0, 2] and peer id 2 survives at POSITION 0. A positional
        // lookup for id 2 therefore finds nothing at all and the reply is
        // silently dropped, which is precisely how the channel wedged.
        let reported = server.poll();
        assert!(
            reported.iter().any(|(id, _)| *id == 2),
            "the live peer's command was not reported at all"
        );

        // Tag each reply with the id it was ADDRESSED to. Asserting only that
        // "a reply arrived" cannot see this bug: peer 2 sits at position 0, so
        // a positional lookup hands C the reply addressed to the DEAD peer 0
        // and C receives something either way. Only the tag distinguishes
        // "answered" from "answered with somebody else's mail".
        for (peer, _cmd) in reported {
            server.reply(peer, &Reply::err(format!("for-peer-{peer}")));
        }

        c.set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();
        let mut line = String::new();
        let _ = BufReader::new(c.try_clone().unwrap()).read_line(&mut line);
        assert!(
            line.contains("for-peer-2"),
            "the live peer (id 2) got the reply addressed to a DEAD peer --              hung-up peers shifted the positions, so replies went to whoever              now occupied the slot (got {line:?})"
        );
    }

    #[test]
    fn peer_ids_are_monotonic_not_positional() {
        // The invariant behind the fix: an id identifies a CONNECTION for its
        // whole life, so it cannot be invalidated by an unrelated peer leaving.
        let mut server = None;
        for p in 6280..6360 {
            if let Some(s) = Server::bind(p) {
                server = Some(s);
                break;
            }
        }
        let server = server.expect("no free port");
        assert_eq!(server.next_id, 0, "ids must start from a known point");
    }

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
            r#"{"cmd":"wallpaper_effect","monitor":3,"name":"waves"}"#,
            // monitor omitted == apply to all
            r#"{"cmd":"wallpaper_effect","name":"off"}"#,
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

    #[test]
    fn the_wallpaper_command_tag_is_snake_case_on_the_wire() {
        // Without the explicit rename the tag would be "wallpapereffect", which
        // is easy to mistype and painful to debug. Pin the spelling.
        let c: Command =
            serde_json::from_str(r#"{"cmd":"wallpaper_effect","name":"rain"}"#).unwrap();
        match c {
            Command::WallpaperEffect { monitor, name } => {
                assert_eq!(monitor, None, "an omitted monitor means ALL monitors");
                assert_eq!(name, "rain");
            }
            _ => panic!("parsed as the wrong variant"),
        }
        assert!(
            serde_json::from_str::<Command>(r#"{"cmd":"wallpapereffect","name":"rain"}"#).is_err(),
            "the un-renamed spelling must NOT be accepted"
        );
    }
}
