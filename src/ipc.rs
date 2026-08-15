//! GlazeWM IPC client.
//!
//! Verified against the GlazeWM source in `RustroverProjects\glazewm`:
//!   * `wm-common/src/ipc.rs`            — port 6123, response envelope shape
//!   * `wm-common/src/dtos/window_dto.rs`— the window fields we consume
//!   * `wm-common/src/wm_event.rs`       — the event names we subscribe to
//!
//! IMPORTANT: GlazeWM emits **no move or resize event**. `wm_event.rs` has
//! `WindowManaged`, `WindowUnmanaged`, `FocusChanged`, `FocusedContainerMoved`,
//! `Workspace*`, `MonitorUpdated`, and nothing carrying new geometry. So events
//! are only a *hint that something changed*; the geometry always comes from a
//! fresh `query windows`.

use std::net::TcpStream;
use std::time::Duration;

use serde::Deserialize;
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

pub const IPC_URL: &str = "ws://localhost:6123";

/// The complete set of events GlazeWM can emit, from `SubscribableEvent` in
/// `wm-common/src/app_command.rs`. We subscribe to `all` rather than listing
/// these, but they are recorded here because of what is NOT among them:
/// there is no `window_moved` and no `window_resized`. Geometry changes are
/// never announced, so events are only ever a trigger to re-`query windows`.
pub const KNOWN_EVENTS: &[&str] = &[
    "application_exiting",
    "binding_modes_changed",
    "focus_changed",
    "focused_container_moved",
    "monitor_added",
    "monitor_updated",
    "monitor_removed",
    "tiling_direction_changed",
    "user_config_changed",
    "window_managed",
    "window_unmanaged",
    "workspace_activated",
    "workspace_deactivated",
    "workspace_updated",
    "pause_changed",
];

/// A window as we care about it. Mirrors the subset of `WindowDto` we use.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// Raw Win32 HWND. Can be compared directly with handles from `EnumWindows`.
    pub handle: isize,
    #[serde(default)]
    pub process_name: String,
    #[serde(default)]
    pub class_name: String,
    /// Coordinates may be NEGATIVE on multi-monitor setups (a monitor left of
    /// the primary). Signed throughout — never widen these to unsigned.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub display_state: String,
}

impl Window {
    /// Alacritty's Win32 class is winit's generic `"Window Class"`, which other
    /// winit apps share, so the process name is the real discriminator.
    pub fn is_alacritty(&self) -> bool {
        self.process_name.eq_ignore_ascii_case("alacritty")
    }

    /// GlazeWM reports `"shown"` / `"hiding"` / `"hidden"` / `"showing"`.
    /// A panel for a hidden window (other workspace) must be hidden too.
    pub fn is_visible(&self) -> bool {
        self.display_state.eq_ignore_ascii_case("shown")
            || self.display_state.eq_ignore_ascii_case("showing")
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    data: Option<EnvelopeData>,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeData {
    /// Present on a `query windows` response.
    #[serde(default)]
    windows: Option<Vec<Window>>,
}

pub struct Client {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Client {
    pub fn connect() -> anyhow::Result<Self> {
        let (socket, _) = tungstenite::connect(IPC_URL)?;
        Ok(Client { socket })
    }

    /// Block for at most `timeout` when reading. Applied to the underlying
    /// TCP stream so the event loop can also do periodic polling.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> anyhow::Result<()> {
        match self.socket.get_ref() {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(timeout)?,
            _ => anyhow::bail!("expected a plain (non-TLS) stream for localhost IPC"),
        }
        Ok(())
    }

    fn send(&mut self, text: &str) -> anyhow::Result<()> {
        self.socket.send(Message::Text(text.to_string()))?;
        Ok(())
    }

    /// Subscribe to every event that might imply a layout change.
    ///
    /// `SubscribableEvent` (see `wm-common/src/app_command.rs`) has an `all`
    /// variant, and the clap arg is `num_args = 1..` — i.e. SPACE-separated,
    /// not comma-separated. Subscribing to `all` avoids both the separator
    /// trap and any risk of misspelling an individual event name (a bad name
    /// makes the whole subscription fail, which looks exactly like a WM that
    /// never moves anything).
    pub fn subscribe_all(&mut self) -> anyhow::Result<()> {
        self.send("sub -e all")
    }

    /// Ask for the current window list. This is the ONLY source of geometry.
    pub fn request_windows(&mut self) -> anyhow::Result<()> {
        self.send("query windows")
    }

    /// Read one message. `Ok(None)` means "timed out / nothing structural" —
    /// the caller should carry on (and poll).
    pub fn read(&mut self) -> anyhow::Result<Option<IpcMessage>> {
        let msg = match self.socket.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e.into()),
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => return Ok(Some(IpcMessage::Closed)),
            // Ping/Pong/Binary carry nothing we need.
            _ => return Ok(None),
        };

        let env: Envelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            // An unrecognised message shape must not kill the daemon.
            Err(_) => return Ok(None),
        };

        if !env.success {
            if let Some(err) = env.error {
                eprintln!("[ipc] glazewm reported error: {err}");
            }
            return Ok(None);
        }

        // A `query windows` reply carries the list; an event subscription
        // message does not. Either way the correct reaction is "re-read
        // geometry", so we only distinguish those two cases.
        if let Some(windows) = env.data.and_then(|d| d.windows) {
            return Ok(Some(IpcMessage::Windows(windows)));
        }

        Ok(Some(IpcMessage::LayoutMayHaveChanged))
    }
}

#[derive(Debug)]
pub enum IpcMessage {
    /// Fresh window list — authoritative geometry.
    Windows(Vec<Window>),
    /// Some event arrived; geometry unknown, so re-query.
    LayoutMayHaveChanged,
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_query_windows_reply() {
        // Captured from the live IPC on pHub — note the negative coordinates
        // from the left-hand monitor, and the generic winit class name.
        let json = r#"{
          "messageType":"client_response",
          "clientMessage":"query windows",
          "success":true,
          "data":{"windows":[
            {"handle":197478,"processName":"alacritty","className":"Window Class",
             "x":-1436,"y":-1226,"width":1432,"height":1274,"displayState":"shown"},
            {"handle":12911596,"processName":"alacritty","className":"Window Class",
             "x":-1436,"y":52,"width":1432,"height":1274,"displayState":"shown"}
          ]}
        }"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        let windows = env.data.unwrap().windows.unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].handle, 197478);
        assert!(windows[0].is_alacritty());
        assert!(windows[0].is_visible());
        assert_eq!(windows[0].x, -1436, "negative coords must survive parsing");
        assert_eq!(windows[1].y, 52);
    }

    #[test]
    fn non_alacritty_windows_are_rejected() {
        let w = Window {
            handle: 1,
            process_name: "chrome".into(),
            class_name: "Window Class".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            display_state: "shown".into(),
        };
        // Same generic winit class, different process: must not match.
        assert!(!w.is_alacritty());
    }

    #[test]
    fn hidden_windows_are_not_visible() {
        let mut w = Window {
            handle: 1,
            process_name: "alacritty".into(),
            class_name: "Window Class".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            display_state: "hidden".into(),
        };
        assert!(!w.is_visible());
        w.display_state = "shown".into();
        assert!(w.is_visible());
    }

    #[test]
    fn there_is_no_move_or_resize_event() {
        // The whole follower design exists because of this absence. If a future
        // GlazeWM adds these, the architecture can be simplified — this test
        // is the tripwire that says so.
        assert!(!KNOWN_EVENTS.contains(&"window_moved"));
        assert!(!KNOWN_EVENTS.contains(&"window_resized"));
    }

    #[test]
    fn malformed_json_is_tolerated() {
        assert!(serde_json::from_str::<Envelope>("{not json").is_err());
    }
}
