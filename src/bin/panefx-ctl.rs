//! panefx-ctl — live control TUI for the panefx daemon.
//!
//! Connects to the daemon's control channel on 127.0.0.1:6124, shows the
//! current effect and every knob it declares, and applies changes instantly so
//! you can watch the backdrop while you tune it.
//!
//! The row list is built entirely from what the daemon sends
//! (`AsciiAnimation::params()`), so a new effect's controls appear here with no
//! change to this file. Do not special-case effect names.
//!
//! Changes are live but NOT saved until `s` — experiment freely, quit with `q`
//! to abandon.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use panefx::animation::{Param, ParamValue};
use panefx::control::DEFAULT_PORT;

/// One editable row. Effect params and config fields are shown in one list so
/// the whole surface is navigable with the same keys.
#[derive(Clone)]
enum Row {
    Effect,
    Param(Param),
    Config {
        key: &'static str,
        label: &'static str,
        value: i64,
        min: i64,
        max: i64,
    },
    ConfigText {
        key: &'static str,
        label: &'static str,
        value: String,
    },
    /// One monitor's wallpaper effect. `←→` cycles, including `off`.
    WallpaperMonitor {
        index: usize,
        label: String,
        effect: String,
        occluded: bool,
    },
    /// Fires on Enter: copy the highlighted monitor's effect to every monitor.
    WallpaperApplyAll,
    /// One line from the daemon's log.
    ///
    /// `count > 1` means the line repeated — the floating-pane bug emitted the
    /// same error every frame, and showing "x412" beats 412 identical rows.
    Log {
        at: u64,
        warn: bool,
        text: String,
        count: u32,
    },
    /// Non-selectable explanation, used when the wallpaper layer is missing.
    /// An empty list there would read as "panefx is broken" when in fact the
    /// terminal backdrops are entirely fine.
    Note(String),
}

/// Which view the TUI is showing.
///
/// The row list is rebuilt wholesale on every reply, so each view keeps its own
/// rows AND its own selection — see the clamp at the end of `absorb`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Effects,
    Wallpaper,
    /// The daemon's recent log lines.
    ///
    /// Exists because the daemon has NO CONSOLE: every failure was previously
    /// invisible, and two real bugs hid behind that for an hour each.
    Logs,
}

impl View {
    fn idx(self) -> usize {
        match self {
            View::Effects => 0,
            View::Wallpaper => 1,
            View::Logs => 2,
        }
    }
    fn next(self) -> Self {
        match self {
            View::Effects => View::Wallpaper,
            View::Wallpaper => View::Logs,
            View::Logs => View::Effects,
        }
    }
}

struct App {
    conn: Conn,
    effect: String,
    effects: Vec<String>,
    /// Rows per view, rebuilt from every snapshot.
    rows: [Vec<Row>; 3],
    /// Selection per view, PRESERVED across rebuilds.
    sel: [ListState; 3],
    view: View,
    status: String,
    dirty: bool,
    /// Caret phase. Toggled by the event loop so the selected row blinks.
    blink_on: bool,
    last_blink: std::time::Instant,
    editing: Option<String>,
    /// Open RGB picker, if any: the colour being built and which channel is
    /// selected.
    ///
    /// A separate mode from `editing` rather than a parse of the text buffer:
    /// the picker steers with arrow keys, and sharing the buffer would make
    /// every keystroke ambiguous between "type a hex digit" and "nudge red".
    picker: Option<Picker>,
    /// True when the daemon has a working wallpaper layer.
    wallpaper_ok: bool,
    /// The last snapshot, kept so moving the cursor can rebuild the wallpaper
    /// rows locally. `absorb` only runs on a daemon reply, so without this the
    /// "params follow the highlighted monitor" rule would never fire on a plain
    /// arrow key.
    last_snap: serde_json::Value,
    /// Which effect the Wallpaper tab's param rows belong to.
    ///
    /// Load-bearing for dispatch: `Row::Param` is shared with the TUI-Pane tab,
    /// and without knowing the effect the wallpaper rows would send the pane's
    /// `param` command and silently retune the terminal backdrop instead.
    wallpaper_param_effect: String,
    /// Which monitor the visible param rows belong to.
    ///
    /// `None` means "no monitor is highlighted" -- the highlight is down on the
    /// fps/cell rows -- and edits then fall back to the shared block, which is
    /// the old "set it for every screen" behaviour.
    wallpaper_param_monitor: Option<usize>,
}

struct Conn {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

impl Conn {
    fn connect(port: u16) -> anyhow::Result<Self> {
        let s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(2)))?;
        let r = BufReader::new(s.try_clone()?);
        Ok(Conn {
            reader: r,
            stream: s,
        })
    }

    fn send(&mut self, v: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        writeln!(self.stream, "{v}")?;
        self.stream.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim())?)
    }
}

impl App {
    fn new(mut conn: Conn) -> anyhow::Result<Self> {
        let reply = conn.send(serde_json::json!({"cmd":"get"}))?;
        let mut app = App {
            conn,
            effect: String::new(),
            effects: Vec::new(),
            rows: [Vec::new(), Vec::new(), Vec::new()],
            sel: [
                ListState::default(),
                ListState::default(),
                ListState::default(),
            ],
            view: View::Effects,
            status: "connected".into(),
            dirty: false,
            blink_on: true,
            last_blink: std::time::Instant::now(),
            editing: None,
            picker: None,
            wallpaper_ok: false,
            last_snap: serde_json::Value::Null,
            wallpaper_param_effect: String::new(),
            wallpaper_param_monitor: None,
        };
        app.sel[0].select(Some(0));
        app.sel[1].select(Some(0));
        app.sel[2].select(Some(0));
        app.absorb(&reply);
        Ok(app)
    }

    fn rows(&self) -> &Vec<Row> {
        &self.rows[self.view.idx()]
    }

    fn sel_state(&self) -> &ListState {
        &self.sel[self.view.idx()]
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.rows().len();
        if n == 0 {
            return;
        }
        let cur = self.selected() as isize;
        let next = (cur + delta).rem_euclid(n as isize) as usize;
        self.sel[self.view.idx()].select(Some(next));
        // On the Wallpaper tab the param rows follow the highlighted monitor,
        // so moving the cursor changes which rows exist. `absorb` only runs on
        // a daemon reply, so rebuild locally from the cached snapshot rather
        // than round-tripping on every arrow key.
        if self.view == View::Wallpaper {
            self.rebuild_wallpaper_rows();
        }
    }

    /// Pull the newest log lines from the daemon.
    ///
    /// Asked for separately from the snapshot: the log is polled far more often
    /// and is much larger, so bundling it would make every keypress carry 500
    /// lines.
    fn refresh_logs(&mut self) {
        let reply = match self.conn.send(serde_json::json!({"cmd":"logs","lines":300})) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("connection lost: {e}");
                return;
            }
        };
        let entries = reply
            .get("logs")
            .and_then(|l| l.get("entries"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut rows: Vec<Row> = Vec::new();
        if entries.is_empty() {
            rows.push(Row::Note("no log entries yet".into()));
        }
        for e in entries {
            rows.push(Row::Log {
                at: e.get("at").and_then(|v| v.as_u64()).unwrap_or(0),
                warn: e.get("level").and_then(|v| v.as_str()) == Some("warn"),
                text: e
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                count: e.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            });
        }
        let n = rows.len();
        self.rows[View::Logs.idx()] = rows;
        // Stick to the newest line, which is what you want when watching a
        // problem happen.
        self.sel[View::Logs.idx()].select(Some(n.saturating_sub(1)));
    }

    /// Rebuild the Wallpaper tab from the cached snapshot.
    fn rebuild_wallpaper_rows(&mut self) {
        if self.last_snap.is_null() {
            return;
        }
        let sel = self.sel[View::Wallpaper.idx()].selected().unwrap_or(0);
        let (rows, eff, mon) = build_wallpaper_rows(&self.last_snap, sel);
        self.wallpaper_param_effect = eff;
        self.wallpaper_param_monitor = mon;
        let n = rows.len();
        self.rows[View::Wallpaper.idx()] = rows;
        // Clamp: the row count changes with the highlighted monitor's effect.
        let cur = self.sel[View::Wallpaper.idx()].selected().unwrap_or(0);
        self.sel[View::Wallpaper.idx()].select(Some(if n == 0 { 0 } else { cur.min(n - 1) }));
    }

    /// Rebuild the row list from a daemon snapshot.
    fn absorb(&mut self, reply: &serde_json::Value) {
        let Some(snap) = reply.get("snapshot") else {
            if let Some(e) = reply.get("error").and_then(|e| e.as_str()) {
                self.status = format!("error: {e}");
            }
            return;
        };
        self.effect = snap
            .get("effect")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        self.effects = snap
            .get("effects")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let params: Vec<Param> = snap
            .get("params")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let cfg = snap.get("config").cloned().unwrap_or_default();
        // Bools read as 0/1 so a flag can be an ordinary Config row and reuse
        // the whole nudge/type/dispatch path rather than needing its own.
        let ci = |k: &str| {
            cfg.get(k)
                .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64)))
                .unwrap_or(0)
        };
        let cs = |k: &str| {
            cfg.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let mut rows = vec![Row::Effect];
        if params.is_empty() {
            // An effect with no declared params would otherwise show an
            // unexplained gap; say so rather than looking broken.
            self.status = format!("'{}' exposes no tunable parameters", self.effect);
        }
        rows.extend(params.into_iter().map(Row::Param));
        // The pane's own on/off, mirroring the wallpaper's per-monitor `off`.
        //
        // FIRST of the config rows because it governs everything below it: with
        // the backdrops off, fps and opacity are settings for something that is
        // not being drawn.
        rows.push(Row::Config {
            key: "pane_off",
            label: "backdrops (1 = off)",
            value: ci("pane_off"),
            min: 0,
            max: 1,
        });
        rows.push(Row::Config {
            key: "fps",
            label: "fps",
            value: ci("fps"),
            min: 1,
            max: 120,
        });
        // Background opacity of the windows panefx draws behind. Labelled "bg"
        // deliberately: this fades the BACKGROUND and leaves the text solid,
        // which is the whole distinction from a whole-window alpha.
        rows.push(Row::Config {
            key: "opacity",
            label: "bg opacity %",
            value: ci("opacity"),
            // The daemon's own floor, not a guess. It REJECTS anything below
            // it rather than clamping, so a row that went lower would move and
            // change nothing.
            min: panefx::opacity::MIN_PERCENT as i64,
            max: panefx::opacity::MAX_PERCENT as i64,
        });
        rows.push(Row::Config {
            key: "cell_w",
            label: "cell width",
            value: ci("cell_w"),
            min: 1,
            max: 64,
        });
        rows.push(Row::Config {
            key: "cell_h",
            label: "cell height",
            value: ci("cell_h"),
            min: 1,
            max: 64,
        });
        rows.push(Row::Config {
            key: "pad_x",
            label: "pad x",
            value: ci("pad_x"),
            min: 0,
            max: 100,
        });
        rows.push(Row::Config {
            key: "pad_y",
            label: "pad y",
            value: ci("pad_y"),
            min: 0,
            max: 100,
        });
        rows.push(Row::Config {
            key: "crop_top",
            label: "crop top (px)",
            value: ci("crop_top"),
            min: 0,
            max: 2000,
        });
        rows.push(Row::Config {
            key: "rotate_secs",
            label: "rotate every (s)",
            value: ci("rotate_secs"),
            min: 0,
            max: 3600,
        });
        rows.push(Row::ConfigText {
            key: "font",
            label: "font",
            value: cs("font"),
        });
        self.rows[View::Effects.idx()] = rows;

        // ---- wallpaper view ----
        self.wallpaper_ok = snap.get("wallpaper_error").is_none();
        self.last_snap = snap.clone();
        let wsel = self.sel[View::Wallpaper.idx()].selected().unwrap_or(0);
        let (wrows, weff, wmon) = build_wallpaper_rows(snap, wsel);
        self.wallpaper_param_effect = weff;
        self.wallpaper_param_monitor = wmon;
        self.rows[View::Wallpaper.idx()] = wrows;

        // Clamp EVERY view's selection to its new row count.
        //
        // `absorb` rebuilds the lists wholesale, so a shorter list would leave
        // the selection past the end — where `nudge` silently does nothing and
        // the TUI simply looks frozen with no error anywhere.
        for v in 0..self.rows.len() {
            let n = self.rows[v].len();
            let cur = self.sel[v].selected().unwrap_or(0);
            self.sel[v].select(Some(if n == 0 { 0 } else { cur.min(n - 1) }));
        }
    }

    fn selected(&self) -> usize {
        self.sel_state().selected().unwrap_or(0)
    }

    /// The effect cycle for a wallpaper monitor: every effect plus `off`.
    ///
    /// `off` is first so it is one keypress away from the initial state, and it
    /// is NOT in the daemon's EFFECTS list (that list decides which config
    /// sections hold params).
    fn wallpaper_cycle(&self) -> Vec<String> {
        wallpaper_cycle_from(&self.effects)
    }

    fn nudge(&mut self, delta: i64) {
        let idx = self.selected();
        let Some(row) = self.rows().get(idx).cloned() else {
            return;
        };
        let msg = match row {
            Row::Effect => {
                if self.effects.is_empty() {
                    return;
                }
                let cur = self
                    .effects
                    .iter()
                    .position(|e| *e == self.effect)
                    .unwrap_or(0);
                let n = self.effects.len() as i64;
                let next = ((cur as i64 + delta).rem_euclid(n)) as usize;
                serde_json::json!({"cmd":"effect","name":self.effects[next]})
            }
            Row::Param(p) => match &p.value {
                ParamValue::Int { v } => {
                    let nv = (v + delta).clamp(p.min, p.max);
                    // Route by TAB. `Row::Param` is shared, so without this the
                    // Wallpaper tab would send the pane's `param` command and
                    // silently retune the terminal backdrop while the user
                    // watched an unchanged desktop.
                    param_command(
                        self.view == View::Wallpaper,
                        &self.wallpaper_param_effect,
                        self.wallpaper_param_monitor,
                        &p.key,
                        serde_json::json!({"kind":"int","v":nv}),
                    )
                }
                // Colours and text need typed entry, not nudging.
                _ => {
                    self.status = format!("press Enter to edit '{}'", p.label);
                    return;
                }
            },
            Row::Config {
                key,
                value,
                min,
                max,
                ..
            } => {
                let nv = (value + delta).clamp(min, max);
                serde_json::json!({"cmd":"set","key":key,"val":nv})
            }
            Row::ConfigText { label, .. } => {
                self.status = format!("press Enter to edit '{label}'");
                return;
            }
            Row::WallpaperMonitor { index, effect, .. } => {
                let cycle = self.wallpaper_cycle();
                if cycle.is_empty() {
                    return;
                }
                let cur = cycle.iter().position(|e| *e == effect).unwrap_or(0);
                let n = cycle.len() as i64;
                let next = ((cur as i64 + delta).rem_euclid(n)) as usize;
                serde_json::json!({"cmd":"wallpaper_effect",
                    "monitor":index,"name":cycle[next]})
            }
            Row::WallpaperApplyAll => {
                self.status = "press Enter to apply this effect to every monitor".into();
                return;
            }
            Row::Note(_) | Row::Log { .. } => return,
        };
        self.dispatch(msg);
    }

    fn dispatch(&mut self, msg: serde_json::Value) {
        match self.conn.send(msg) {
            Ok(reply) => {
                if reply.get("ok").and_then(|v| v.as_bool()) == Some(false) {
                    self.status = reply
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("rejected")
                        .to_string();
                } else {
                    self.dirty = true;
                    self.status = "applied".into();
                    self.absorb(&reply);
                }
            }
            Err(e) => self.status = format!("connection lost: {e}"),
        }
    }

    /// Copy the highlighted monitor's effect to every monitor.
    fn apply_to_all(&mut self) {
        let idx = self.selected();
        // Prefer the highlighted monitor; otherwise the first one listed.
        let effect = match self.rows().get(idx) {
            Some(Row::WallpaperMonitor { effect, .. }) => Some(effect.clone()),
            _ => self.rows().iter().find_map(|r| match r {
                Row::WallpaperMonitor { effect, .. } => Some(effect.clone()),
                _ => None,
            }),
        };
        let Some(effect) = effect else {
            self.status = "no monitors to apply to".into();
            return;
        };
        // `monitor` omitted == every monitor.
        self.dispatch(serde_json::json!({"cmd":"wallpaper_effect","name":effect}));
        self.status = format!("applied '{effect}' to every monitor");
    }

    fn commit_edit(&mut self, text: String) {
        let idx = self.selected();
        let Some(row) = self.rows().get(idx).cloned() else {
            return;
        };
        let msg = match row {
            Row::Param(p) => {
                // Same tab routing as `nudge` -- see the note there.
                let wall = self.view == View::Wallpaper;
                let eff = self.wallpaper_param_effect.clone();
                let val = match &p.value {
                    ParamValue::Text { .. } => serde_json::json!({"kind":"text","v":text}),
                    ParamValue::Colour { .. } => match panefx::palette::Rgb::parse_hex(&text) {
                        Some(c) => serde_json::json!({"kind":"colour","r":c.0,"g":c.1,"b":c.2}),
                        None => {
                            self.status = format!("'{text}' is not #rrggbb");
                            return;
                        }
                    },
                    ParamValue::Int { .. } => match text.trim().parse::<i64>() {
                        Ok(n) => serde_json::json!({"kind":"int","v":n.clamp(p.min, p.max)}),
                        Err(_) => {
                            self.status = format!("'{text}' is not a number");
                            return;
                        }
                    },
                };
                param_command(wall, &eff, self.wallpaper_param_monitor, &p.key, val)
            }
            Row::ConfigText { key, .. } => serde_json::json!({"cmd":"set","key":key,"val":text}),
            Row::Config { key, min, max, .. } => match text.trim().parse::<i64>() {
                Ok(n) => serde_json::json!({"cmd":"set","key":key,"val":n.clamp(min,max)}),
                Err(_) => {
                    self.status = format!("'{text}' is not a number");
                    return;
                }
            },
            Row::Effect => return,
            // These are cycled or fired, never typed into.
            Row::WallpaperMonitor { .. }
            | Row::WallpaperApplyAll
            | Row::Note(_)
            | Row::Log { .. } => return,
        };
        self.dispatch(msg);
    }
}

/// An open RGB colour picker.
///
/// Terminals have no colour wheel, and a hex field alone means guessing what
/// `#3a7f2c` looks like before pressing Enter. Three channels with a live
/// swatch is the closest a cell grid gets to picking a colour by eye, and it is
/// entirely arrow-key driven so it works over SSH with no mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Picker {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0 = red, 1 = green, 2 = blue.
    pub channel: usize,
}

impl Picker {
    pub fn from_rgb(c: panefx::palette::Rgb) -> Self {
        Picker { r: c.0, g: c.1, b: c.2, channel: 0 }
    }

    /// Move between channels, wrapping.
    pub fn cycle_channel(&mut self, delta: isize) {
        let n = 3isize;
        self.channel = (((self.channel as isize + delta) % n + n) % n) as usize;
    }

    /// Nudge the selected channel, SATURATING at the ends.
    ///
    /// Saturating, never wrapping: nudging red past 255 back to 0 turns a
    /// near-white into a near-black in one keypress, which is never what the
    /// hand meant.
    pub fn nudge(&mut self, delta: i16) {
        let v = match self.channel {
            0 => &mut self.r,
            1 => &mut self.g,
            _ => &mut self.b,
        };
        *v = (*v as i16 + delta).clamp(0, 255) as u8;
    }

    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// A bar for one channel, `width` cells wide.
    pub fn bar(value: u8, width: usize) -> String {
        let w = width.max(1);
        let filled = (value as usize * w) / 255;
        let mut s = String::with_capacity(w);
        for i in 0..w {
            s.push(if i < filled { '\u{2588}' } else { '\u{2591}' });
        }
        s
    }
}

/// The effect cycle for a wallpaper monitor: `off` plus every effect.
///
/// `off` is FIRST so it is one keypress away from the initial state, and it is
/// deliberately not a member of the daemon's `EFFECTS` list — that list decides
/// which config sections are read as per-effect params.
fn wallpaper_cycle_from(effects: &[String]) -> Vec<String> {
    let mut v = vec!["off".to_string()];
    v.extend(effects.iter().cloned());
    v
}

/// Build the Wallpaper tab's rows from a daemon snapshot.
///
/// Free function rather than a method so it can be tested against captured
/// snapshots without a live daemon or a terminal.
/// Build the JSON for a param change, routed to the correct SURFACE.
///
/// `Row::Param` is shared between the two tabs, so the tab decides where the
/// value goes. Without this the Wallpaper tab sends the pane's `param` command
/// and silently retunes the terminal backdrop while the user watches an
/// unchanged desktop -- the single most likely bug in this feature, and one
/// with no visible error when it happens.
fn param_command(
    wallpaper: bool,
    effect: &str,
    monitor: Option<usize>,
    key: &str,
    val: serde_json::Value,
) -> serde_json::Value {
    if wallpaper {
        // `monitor: null` is meaningful, not a placeholder: it means the SHARED
        // block, i.e. every screen running this effect. Sent explicitly so the
        // daemon never has to guess which was intended.
        serde_json::json!({"cmd":"wallpaper_param","monitor":monitor,"effect":effect,"key":key,"val":val})
    } else {
        serde_json::json!({"cmd":"param","key":key,"val":val})
    }
}

/// Which effect's params the Wallpaper tab shows.
///
/// The highlighted monitor's, so the knobs sit directly under the row that
/// names them. Falls back to the first monitor that is on, because the highlight
/// spends half its life on the fps/cell rows at the bottom and the param block
/// must not vanish when it does. `None` when every monitor is off.
fn wallpaper_param_effect(
    monitors: &[serde_json::Value],
    sel: usize,
) -> Option<(String, Option<usize>)> {
    let effect_of = |m: &serde_json::Value| {
        m.get("effect")
            .and_then(|v| v.as_str())
            .filter(|e| *e != "off")
            .map(String::from)
    };
    let index_of = |m: &serde_json::Value| m.get("index").and_then(|v| v.as_u64()).map(|n| n as usize);
    // The highlight is on a monitor row when sel < monitors.len(): the monitor
    // rows are always FIRST and fixed-length, which is what keeps the selection
    // stable as the param rows below change length.
    if let Some(m) = monitors.get(sel) {
        if let Some(e) = effect_of(m) {
            // A highlighted monitor means edits target THAT SCREEN.
            return Some((e, index_of(m)));
        }
    }
    // Highlight is elsewhere (the fps/cell rows). Show the first live effect,
    // and target the SHARED block -- there is no one screen being pointed at,
    // and silently editing whichever happened to be first would be worse.
    monitors.iter().find_map(effect_of).map(|e| (e, None))
}

/// Build the Wallpaper tab's rows, and report which effect the param rows are
/// for so dispatch can route them to the wallpaper rather than the pane.
fn build_wallpaper_rows(
    snap: &serde_json::Value,
    sel: usize,
) -> (Vec<Row>, String, Option<usize>) {
    let mut rows: Vec<Row> = Vec::new();
    let cfg = snap.get("config").cloned().unwrap_or_default();
    let ci = |k: &str| {
        cfg.get(k)
            .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64)))
            .unwrap_or(0)
    };

    let monitors = snap
        .get("wallpaper")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let werr = snap.get("wallpaper_error").and_then(|v| v.as_str());

    if let Some(e) = werr {
        // Say WHY, and say what is unaffected. An empty tab here would read as
        // "panefx is broken" when the terminal backdrops are working perfectly.
        rows.push(Row::Note(format!("wallpaper layer unavailable — {e}")));
        if e.contains("0x052C") {
            rows.push(Row::Note(
                "Explorer did not hand back a wallpaper layer. Restarting".into(),
            ));
            rows.push(Row::Note(
                "Explorer, then panefx, usually clears this.".into(),
            ));
        }
        rows.push(Row::Note(
            "Terminal and Neovide backdrops are unaffected.".into(),
        ));
        return (rows, String::new(), None);
    }

    if monitors.is_empty() {
        rows.push(Row::Note("no monitors detected".into()));
        return (rows, String::new(), None);
    }

    for m in &monitors {
        rows.push(Row::WallpaperMonitor {
            index: m.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            label: m
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            effect: m
                .get("effect")
                .and_then(|v| v.as_str())
                .unwrap_or("off")
                .to_string(),
            occluded: m
                .get("occluded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }
    rows.push(Row::WallpaperApplyAll);

    // The desktop's OWN knobs for whichever effect is in focus.
    //
    // Placed between the monitor rows and the fps/cell block: they belong to
    // the monitor row above them, and the fps/cell rows are set-once settings
    // that read naturally as a trailing block. Keeping the monitor rows first
    // and fixed-length also keeps the selection stable while these change
    // length underneath it.
    let (param_effect, param_monitor) = match wallpaper_param_effect(&monitors, sel) {
        Some((e, m)) => (e, m),
        None => (String::new(), None),
    };
    if param_effect.is_empty() {
        rows.push(Row::Note(
            "every monitor is off — turn one on to tune its look".into(),
        ));
    } else {
        // Keyed by monitor now, not by effect -- see the note in `main.rs`.
        // Falls back to the first entry when the highlight is off the monitor
        // rows, so the block never vanishes mid-scroll.
        let params: Vec<Param> = snap
            .get("wallpaper_params")
            .and_then(|m| match param_monitor {
                Some(i) => m.get(&i.to_string()).cloned(),
                None => m.as_object().and_then(|o| o.values().next().cloned()),
            })
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if params.is_empty() {
            rows.push(Row::Note(format!("'{param_effect}' has no tunable knobs")));
        } else {
            rows.push(Row::Note(match param_monitor {
                Some(i) => format!("{param_effect} — DISPLAY{i} only"),
                None => format!("{param_effect} — every monitor running it"),
            }));
            rows.extend(params.into_iter().map(Row::Param));
        }
    }

    let asked = ci("wallpaper_fps");
    rows.push(Row::Config {
        key: "wallpaper_fps",
        label: "wallpaper fps",
        value: asked,
        min: 1,
        max: 120,
    });
    let eff = ci("wallpaper_fps_effective");
    if eff > 0 && eff < asked {
        // The wallpaper is ticked from the daemon loop, so `fps` caps it. Say
        // so, rather than showing a number the screen is not delivering.
        rows.push(Row::Note(format!(
            "capped to {eff} fps by the daemon's own fps"
        )));
    }
    // The detail knob, above the raw cell rows it writes into. One lever for
    // "make it finer" instead of two numbers you have to keep in proportion by
    // hand -- the raw rows stay for anyone who wants an exact size.
    rows.push(Row::Config {
        key: "wallpaper_detail",
        label: "detail",
        value: ci("wallpaper_detail"),
        min: panefx::config::DETAIL_MIN as i64,
        max: panefx::config::DETAIL_MAX as i64,
    });
    rows.push(Row::Config {
        key: "wallpaper_cell_w",
        label: "wp cell width",
        value: ci("wallpaper_cell_w"),
        min: 1,
        max: 64,
    });
    rows.push(Row::Config {
        key: "wallpaper_cell_h",
        label: "wp cell height",
        value: ci("wallpaper_cell_h"),
        min: 1,
        max: 64,
    });
    (rows, param_effect, param_monitor)
}

fn row_label(r: &Row) -> String {
    match r {
        Row::Effect => "effect".into(),
        Row::Param(p) => p.label.clone(),
        Row::Config { label, .. } => (*label).into(),
        Row::ConfigText { label, .. } => (*label).into(),
        Row::WallpaperMonitor { index, label, .. } => format!("{index}  {label}"),
        Row::WallpaperApplyAll => "apply to all".into(),
        Row::Log { at, .. } => format!("{:>4}s", at),
        Row::Note(_) => String::new(),
    }
}

/// The human reading of a raw number, appended after the stored value.
///
/// Every numeric row used to show a bare integer whose meaning was invisible:
/// `headroom 1350` is really 1.350, `cell width 15` is 15 pixels, `fps 10` is 10
/// frames per second. You had to know, or do the division in your head.
///
/// The stored number is still shown unchanged — it is what gets sent and saved.
/// This only adds the reading beside it.
///
/// Derived from the KEY, not from a hardcoded list of effects, so a new effect
/// shipping a `foo (x1000)` param formats correctly with no change here. That
/// matches the existing rule that this TUI renders whatever it is handed and
/// never hardcodes effect knowledge.
fn value_units(key: &str, label: &str, value: i64) -> String {
    // Fixed point. The label already hints "(x1000)" but still leaves the
    // arithmetic to the reader.
    if label.contains("x1000") || key.ends_with("_x1000") {
        return format!(" = {:.3}", value as f64 / 1000.0);
    }
    if key.ends_with("fps") {
        return " fps".into();
    }
    if key.contains("cell_") || key.starts_with("pad_") || key == "crop_top" {
        return " px".into();
    }
    if key == "opacity" {
        return "%".into();
    }
    // The detail number is an index into a ladder, so on its own it says
    // nothing at all. Show the cell size it selects.
    if key == "wallpaper_detail" {
        let (w, h) = panefx::config::detail_to_cell(value as u8);
        return format!("  -> {w}x{h} px cells");
    }
    String::new()
}

fn row_value(r: &Row, effect: &str) -> String {
    match r {
        Row::Effect => format!("‹ {effect} ›"),
        // Only numbers carry units. A colour swatch or a text value must be
        // rendered exactly as the effect declared it.
        Row::Param(p) => match &p.value {
            panefx::animation::ParamValue::Int { v } => {
                format!("{v}{}", value_units(&p.key, &p.label, *v))
            }
            other => other.display(),
        },
        Row::Config { key, label, value, .. } => {
            format!("{value}{}", value_units(key, label, *value))
        }
        Row::ConfigText { value, .. } => value.clone(),
        // The ‹ › convention marks "this cycles". It is also load-bearing:
        // `run()` blanks the edit buffer for values starting with ‹.
        Row::WallpaperMonitor {
            effect, occluded, ..
        } => {
            if *occluded {
                // Say why it is not moving, so a frozen wallpaper reads as
                // working-as-intended rather than as a bug.
                format!("‹ {effect} ›  ❄ frozen")
            } else {
                format!("‹ {effect} ›")
            }
        }
        Row::WallpaperApplyAll => "press Enter".into(),
        Row::Log { text, count, .. } => {
            if *count > 1 {
                format!("{text}   x{count}")
            } else {
                text.clone()
            }
        }
        Row::Note(t) => t.clone(),
    }
}

/// Rows that cannot be selected — pure explanation.
fn row_is_note(r: &Row) -> bool {
    matches!(r, Row::Note(_) | Row::Log { .. })
}

/// A little bar so int values read at a glance.
fn row_bar(r: &Row) -> String {
    let (v, min, max) = match r {
        Row::Param(p) => match &p.value {
            ParamValue::Int { v } => (*v, p.min, p.max),
            _ => return String::new(),
        },
        Row::Config {
            value, min, max, ..
        } => (*value, *min, *max),
        _ => return String::new(),
    };
    if max <= min {
        return String::new();
    }
    let frac = ((v - min) as f32 / (max - min) as f32).clamp(0.0, 1.0);
    let filled = (frac * 12.0).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(12 - filled))
}

const HELP: &str = "\
panefx — animated ASCII backdrops behind windows, and on the desktop

USAGE:
    panefx              open the GUI control panel
    panefx --tui        open THIS control TUI -- the one that works over SSH
    panefx --daemon     run the background daemon
    panefx --help       this text

    panefx-ctl          this TUI directly
    panefx-gui          the GUI directly

KEYS
    Tab            switch between the Effects and Wallpaper tabs
    w / e / g      jump straight to Wallpaper / TUI-Pane / Logs
    R              (Logs) refresh
    up/down        move between rows
    left/right     adjust a value (H / L for x10)
    Enter          type a value
    a              (Wallpaper) apply this effect to every monitor
    s              save to the config file
    r              revert to the saved config
    q              quit without saving

Changes apply instantly but are NOT saved until `s`, so experiment freely.

ENVIRONMENT
    PANEFX_PORT    control-channel port (default 6124)
";

fn main() -> anyhow::Result<()> {
    // `panefx` with no arguments hands off to this binary, so any flags the
    // user typed arrive here. Handle them rather than ignoring them silently.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("-h") | Some("--help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some(other) => {
            eprintln!("panefx: unknown option '{other}'\n\n{HELP}");
            std::process::exit(2);
        }
    }

    let port: u16 = std::env::var("PANEFX_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let conn = match Conn::connect(port) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("panefx-ctl: cannot reach the daemon on 127.0.0.1:{port} ({e})");
            eprintln!();
            eprintln!("The daemon is normally started by GlazeWM. To start one by hand:");
            eprintln!("    panefx --daemon");
            eprintln!();
            eprintln!("If GlazeWM is running and this still fails, its startup_commands may");
            eprintln!("still be launching panefx without the --daemon flag.");
            std::process::exit(1);
        }
    };
    let mut app = App::new(conn)?;

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    let res = run(&mut term, &mut app);

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;
    term.show_cursor()?;
    res
}

fn run<B: Backend>(term: &mut Terminal<B>, app: &mut App) -> anyhow::Result<()> {
    loop {
        term.draw(|f| draw(f, app))?;

        // Blink the caret on a fixed cadence, independent of keypresses.
        if app.last_blink.elapsed() >= Duration::from_millis(500) {
            app.blink_on = !app.blink_on;
            app.last_blink = std::time::Instant::now();
        }

        if !event::poll(Duration::from_millis(120))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        // A keypress means the user is looking at the cursor — show it now
        // rather than leaving them staring at a blank half-cycle.
        app.blink_on = true;
        app.last_blink = std::time::Instant::now();
        if k.kind != KeyEventKind::Press {
            continue;
        }

        // Picker mode captures everything except Esc/Enter, like text entry.
        if let Some(mut pk) = app.picker {
            match k.code {
                KeyCode::Esc => app.picker = None,
                KeyCode::Enter => {
                    app.picker = None;
                    app.commit_edit(pk.hex());
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    pk.cycle_channel(-1);
                    app.picker = Some(pk);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    pk.cycle_channel(1);
                    app.picker = Some(pk);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    pk.nudge(1);
                    app.picker = Some(pk);
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    pk.nudge(-1);
                    app.picker = Some(pk);
                }
                // Capitals move in tens, matching the row nudge keys.
                KeyCode::Char('L') => {
                    pk.nudge(16);
                    app.picker = Some(pk);
                }
                KeyCode::Char('H') => {
                    pk.nudge(-16);
                    app.picker = Some(pk);
                }
                // Drop to typing a hex code, for a colour taken from elsewhere.
                KeyCode::Char('#') => {
                    app.picker = None;
                    app.editing = Some(String::new());
                }
                _ => {}
            }
            continue;
        }

        // Text-entry mode captures everything except Esc/Enter.
        if let Some(buf) = app.editing.clone() {
            match k.code {
                KeyCode::Esc => app.editing = None,
                KeyCode::Enter => {
                    app.editing = None;
                    app.commit_edit(buf);
                }
                KeyCode::Backspace => {
                    let mut b = buf;
                    b.pop();
                    app.editing = Some(b);
                }
                KeyCode::Char(c) => app.editing = Some(format!("{buf}{c}")),
                _ => {}
            }
            continue;
        }

        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Tab => {
                app.view = app.view.next();
                if app.view == View::Logs {
                    app.refresh_logs();
                }
                app.status = match app.view {
                    View::Effects => "TUI-Pane — the backdrop behind your windows".into(),
                    View::Wallpaper => "desktop wallpaper".into(),
                    View::Logs => "daemon log — newest last".into(),
                };
            }
            KeyCode::Char('w') => app.view = View::Wallpaper,
            KeyCode::Char('e') => app.view = View::Effects,
            // `g` for logs: `l` is already "adjust right".
            KeyCode::Char('g') => {
                app.view = View::Logs;
                app.refresh_logs();
            }
            // Manual refresh, and the only way to see NEW lines without
            // leaving and re-entering the tab.
            KeyCode::Char('R') if app.view == View::Logs => app.refresh_logs(),
            KeyCode::Char('a') if app.view == View::Wallpaper => app.apply_to_all(),
            KeyCode::Down | KeyCode::Char('j') => app.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_sel(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('+') => app.nudge(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('-') => app.nudge(-1),
            KeyCode::Char('L') => app.nudge(10),
            KeyCode::Char('H') => app.nudge(-10),
            KeyCode::Enter => {
                let row = app.rows().get(app.selected()).cloned();
                match row {
                    Some(Row::WallpaperApplyAll) => app.apply_to_all(),
                    Some(Row::WallpaperMonitor { .. }) | Some(Row::Note(_)) | None => {}
                    // A colour opens the PICKER, not a text field: a hex code
                    // alone means guessing what it looks like before
                    // committing. `#` inside the picker still drops to typing.
                    Some(Row::Param(ref p))
                        if matches!(p.value, ParamValue::Colour { .. }) =>
                    {
                        let cur = panefx::palette::Rgb::parse_hex(&row_value(
                            &Row::Param(p.clone()),
                            &app.effect,
                        ))
                        .unwrap_or(panefx::palette::Rgb(128, 128, 128));
                        app.picker = Some(Picker::from_rgb(cur));
                    }
                    Some(r) => {
                        let cur = row_value(&r, &app.effect);
                        // Start from the current value so a small tweak is easy.
                        app.editing =
                            Some(if cur.starts_with('‹') { String::new() } else { cur });
                    }
                }
            }
            KeyCode::Char('s') => {
                app.dispatch(serde_json::json!({"cmd":"save"}));
                app.dirty = false;
                app.status = "saved to config.toml".into();
            }
            KeyCode::Char('r') => {
                app.dispatch(serde_json::json!({"cmd":"revert"}));
                app.dirty = false;
                app.status = "reverted to config.toml".into();
            }
            _ => {}
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(f.area());

    let title = format!(
        " panefx — {} {}",
        app.effect,
        if app.dirty { "*modified" } else { "" }
    );

    // Tab strip, rendered INSIDE the existing header block rather than adding a
    // fourth chunk — the header already has a spare line.
    let tab_style = |v: View| {
        if app.view == v {
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };
    let mut header: Vec<Span> = vec![
        Span::styled(" TUI-Pane ", tab_style(View::Effects)),
        Span::raw(" "),
        Span::styled(" Wallpaper ", tab_style(View::Wallpaper)),
        Span::raw(" "),
        Span::styled(" Logs ", tab_style(View::Logs)),
        Span::raw("   "),
    ];
    match app.view {
        View::Effects => {
            header.push(Span::raw("effect: "));
            header.push(Span::styled(
                app.effect.clone(),
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        View::Logs => {
            let warns = app
                .rows()
                .iter()
                .filter(|r| matches!(r, Row::Log { warn: true, .. }))
                .count();
            header.push(Span::styled(
                if warns > 0 {
                    format!("{warns} warning(s)")
                } else {
                    "no warnings".into()
                },
                Style::default().fg(if warns > 0 {
                    Color::Yellow
                } else {
                    Color::LightGreen
                }),
            ));
        }
        View::Wallpaper => {
            let n = app
                .rows()
                .iter()
                .filter(|r| matches!(r, Row::WallpaperMonitor { .. }))
                .count();
            header.push(Span::styled(
                if app.wallpaper_ok {
                    format!("{n} monitor(s)")
                } else {
                    "layer unavailable".into()
                },
                Style::default().fg(if app.wallpaper_ok {
                    Color::LightGreen
                } else {
                    Color::Yellow
                }),
            ));
        }
    }
    if app.dirty {
        header.push(Span::raw("   *modified"));
    }
    f.render_widget(
        Paragraph::new(Line::from(header))
            .block(Block::default().borders(Borders::ALL).title(title)),
        chunks[0],
    );

    let sel = app.selected();
    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // Notes are prose, not controls: render them dim, full width, with
            // no caret, label column or bar.
            if let Row::Log { warn, .. } = r {
                // Warnings yellow, info dim: a per-frame failure has to be
                // distinguishable from startup chatter at a glance.
                let style = if *warn {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                return ListItem::new(Line::from(vec![
                    Span::styled(format!("  {:>6} ", row_label(r)), Style::default().fg(Color::DarkGray)),
                    Span::styled(row_value(r, ""), style),
                ]));
            }
            if row_is_note(r) {
                return ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(row_value(r, &app.effect), Style::default().fg(Color::Yellow)),
                ]));
            }
            let editing = i == sel && app.editing.is_some();
            // The open picker replaces the row's value with three live bars.
            // Rendered INLINE rather than as a popup: a popup would cover the
            // wallpaper being tuned, and the whole point of picking a colour
            // here is watching the screen behind it change.
            if i == sel {
                if let Some(pk) = app.picker {
                    let mark = |c: usize| if pk.channel == c { '>' } else { ' ' };
                    let mut spans = vec![Span::styled(
                        "  colour ",
                        Style::default().fg(Color::DarkGray),
                    )];
                    for (ci, (name, v, col)) in [
                        ("R", pk.r, Color::LightRed),
                        ("G", pk.g, Color::LightGreen),
                        ("B", pk.b, Color::LightBlue),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        spans.push(Span::styled(
                            format!("{}{name} ", mark(ci)),
                            if pk.channel == ci {
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::DarkGray)
                            },
                        ));
                        spans.push(Span::styled(
                            Picker::bar(v, 12),
                            Style::default().fg(col),
                        ));
                        spans.push(Span::raw(format!(" {v:>3} ")));
                    }
                    // The swatch itself, in the colour being built -- the one
                    // thing a hex field cannot show.
                    spans.push(Span::styled(
                        "  \u{2588}\u{2588}\u{2588} ",
                        Style::default().fg(Color::Rgb(pk.r, pk.g, pk.b)),
                    ));
                    spans.push(Span::styled(
                        format!("{}  (# to type)", pk.hex()),
                        Style::default().fg(Color::DarkGray),
                    ));
                    return ListItem::new(Line::from(spans));
                }
            }
            let value = if editing {
                format!("{}_", app.editing.clone().unwrap_or_default())
            } else {
                row_value(r, &app.effect)
            };
            let style = if i == sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            // Truncate long values (the glyph set is 45 chars) so the bar
            // column stays aligned instead of being shoved off-screen.
            let shown = if value.chars().count() > 30 {
                let mut t: String = value.chars().take(29).collect();
                t.push('…');
                t
            } else {
                value
            };
            // A blinking caret on the selected row. The highlight alone is easy
            // to lose track of against a busy animated backdrop — which is the
            // whole point of this program, so the cursor has to survive it.
            let caret = if i == sel {
                if app.blink_on {
                    "▌ "
                } else {
                    "  "
                }
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    caret,
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<18}", row_label(r)), style),
                Span::styled(format!("{:<32}", shown), style),
                Span::styled(
                    row_bar(r),
                    if i == sel {
                        Style::default().fg(Color::Black).bg(Color::LightGreen)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
            ]))
        })
        .collect();

    let mut state = app.sel_state().clone();
    let list_title = match app.view {
        View::Effects => " ↑↓ move   ←→ adjust (H/L ×10)   Enter type ",
        View::Wallpaper => " ↑↓ move   ←→ effect (incl. off)   [a] all monitors ",
        View::Logs => " ↑↓ scroll   [R] refresh ",
    };
    f.render_stateful_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(list_title)),
        chunks[1],
        &mut state,
    );

    // Tell the user which keys do anything on THIS row — colours and text
    // cannot be nudged, and silently ignoring ←→ on them reads as broken.
    let hint = match app.rows().get(sel) {
        Some(Row::Param(p)) if !matches!(p.value, ParamValue::Int { .. }) => {
            "  ·  Enter to type a value"
        }
        Some(Row::ConfigText { .. }) => "  ·  Enter to type a value",
        Some(Row::Effect) => "  ·  ←→ cycles effects",
        Some(Row::WallpaperMonitor { occluded, .. }) => {
            if *occluded {
                "  ·  frozen: fully covered, so it costs nothing"
            } else {
                "  ·  ←→ cycles this monitor's effect"
            }
        }
        Some(Row::WallpaperApplyAll) => "  ·  Enter copies the effect to every monitor",
        _ => "",
    };
    f.render_widget(
        Paragraph::new(format!("{}{}", app.status, hint))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" [Tab] view  [g] logs  [s]ave  [r]evert  [q]uit "),
            ),
        chunks[2],
    );
}

#[cfg(test)]
mod view_tests {
    use super::*;

    /// The snapshot a daemon with no wallpaper layer sends.
    fn snapshot_without_layer() -> serde_json::Value {
        serde_json::json!({"snapshot":{
            "effect":"waves","effects":["flames","rain","waves","fire"],
            "params":[],
            "config":{"font":"F","cell_w":10,"cell_h":15,"fps":10,"crop_top":0,
                      "pad_x":10,"pad_y":8,"rotation":["waves"],"rotate_secs":0,
                      "wallpaper_fps":5,"wallpaper_fps_effective":5,
                      "wallpaper_cell_w":15,"wallpaper_cell_h":23},
            "wallpaper":[],
            "wallpaper_error":"Explorer did not create a WorkerW behind the icons (0x052C had no effect on this build)"
        }})
    }

    fn snapshot_with_monitors() -> serde_json::Value {
        serde_json::json!({"snapshot":{
            "effect":"waves","effects":["flames","rain","waves","fire"],
            "params":[],
            "config":{"font":"F","cell_w":10,"cell_h":15,"fps":10,"crop_top":0,
                      "pad_x":10,"pad_y":8,"rotation":["waves"],"rotate_secs":0,
                      "wallpaper_fps":5,"wallpaper_fps_effective":5,
                      "wallpaper_cell_w":15,"wallpaper_cell_h":23},
            "wallpaper":[
                {"index":1,"label":"1440x2560 portrait","effect":"waves","occluded":false},
                {"index":3,"label":"1920x1080 (primary)","effect":"off","occluded":true}
            ]
        }})
    }

    #[test]
    fn a_missing_layer_explains_itself_instead_of_showing_nothing() {
        // An empty list would read as "panefx is broken" when the terminal
        // backdrops are working perfectly. It must say why, and say what is
        // unaffected.
        let (rows, _, _) = build_wallpaper_rows(&snapshot_without_layer()["snapshot"], 0);
        assert!(!rows.is_empty(), "never show an empty wallpaper tab");
        let text: String = rows.iter().map(|r| row_value(r, "waves")).collect::<Vec<_>>().join(" ");
        assert!(text.contains("unavailable"));
        // Not "25H2 removed the wallpaper layer" -- that was a wrong reading of
        // a failed probe, and the wallpaper demonstrably works on this build.
        // Point at the recoverable cause instead, and say how to recover.
        assert!(text.contains("Explorer"), "name the actual cause");
        assert!(text.contains("Restarting"), "say how to recover");
        assert!(text.contains("unaffected"), "say what still works");
        assert!(rows.iter().all(row_is_note), "nothing here is selectable");
    }

    #[test]
    fn monitors_render_with_effect_and_frozen_state() {
        let (rows, _, _) = build_wallpaper_rows(&snapshot_with_monitors()["snapshot"], 0);
        let mons: Vec<&Row> = rows.iter()
            .filter(|r| matches!(r, Row::WallpaperMonitor { .. })).collect();
        assert_eq!(mons.len(), 2);
        assert!(row_label(mons[0]).contains("1440x2560"));
        assert!(row_value(mons[0], "").contains("waves"));
        // The frozen marker is why a still wallpaper does not read as a bug.
        assert!(row_value(mons[1], "").contains("frozen"));
        assert!(!row_value(mons[0], "").contains("frozen"));
        assert!(rows.iter().any(|r| matches!(r, Row::WallpaperApplyAll)));
    }

    #[test]
    fn off_is_reachable_in_one_keypress_from_the_cycle() {
        // `off` must be first so it is adjacent to the initial state, and it is
        // deliberately NOT in the daemon's EFFECTS list.
        let effects: Vec<String> =
            ["flames","rain","waves","fire"].iter().map(|s| s.to_string()).collect();
        let cycle = wallpaper_cycle_from(&effects);
        assert_eq!(cycle[0], "off");
        assert_eq!(cycle.len(), 5);
    }

    /// A snapshot with two monitors on DIFFERENT effects, plus their params.
    fn snapshot_two_effects() -> serde_json::Value {
        serde_json::json!({"snapshot":{
            "effect":"waves","effects":["flames","rain","waves","fire"],
            "params":[],
            "config":{"font":"F","cell_w":10,"cell_h":15,"fps":10,"crop_top":0,
                      "pad_x":10,"pad_y":8,"rotation":["waves"],"rotate_secs":0,
                      "opacity":60,"wallpaper_fps":5,"wallpaper_fps_effective":5,
                      "wallpaper_cell_w":15,"wallpaper_cell_h":23},
            "wallpaper":[
                {"index":1,"label":"1440x2560 portrait","effect":"waves","occluded":false},
                {"index":3,"label":"1920x1080 (primary)","effect":"flames","occluded":false}
            ],
            // Keyed by MONITOR index, not effect -- two screens on one effect
            // each get their own entry, which is what makes them separately
            // editable. See `main.rs`.
            "wallpaper_params":{
                "1":[{"key":"ink","label":"ink colour","value":{"kind":"colour","r":0,"g":0,"b":255},"min":0,"max":0}],
                "3":[{"key":"seed","label":"flame height","value":{"kind":"int","v":65},"min":1,"max":200}]
            }
        }})
    }

    #[test]
    fn the_wallpaper_tab_shows_the_highlighted_monitors_params() {
        let snap = snapshot_two_effects();
        let s = &snap["snapshot"];
        // Row 0 is DISPLAY1 (waves), row 1 is DISPLAY3 (flames).
        let (_, eff0, mon0) = build_wallpaper_rows(s, 0);
        assert_eq!(eff0, "waves");
        let (_, eff1, mon1) = build_wallpaper_rows(s, 1);
        assert_eq!(eff1, "flames", "params must follow the highlighted monitor");
        // AND the monitor must follow too, or every screen edits the same
        // stored block -- the bug this whole per-monitor pass exists to fix.
        assert_eq!(mon0, Some(1), "row 0 must target DISPLAY1");
        assert_eq!(mon1, Some(3), "row 1 must target DISPLAY3");
        assert_ne!(mon0, mon1, "two monitor rows must not edit the same target");
    }

    #[test]
    fn the_highlight_below_the_monitor_rows_falls_back_to_the_first_one_on() {
        // The cursor spends half its life on the fps/cell rows; the param block
        // must not vanish when it does.
        let snap = snapshot_two_effects();
        let (_, eff, mon) = build_wallpaper_rows(&snap["snapshot"], 99);
        assert_eq!(eff, "waves");
        // No monitor row is highlighted, so edits target the SHARED block --
        // silently editing whichever screen happened to be first would be
        // worse than editing all of them on purpose.
        assert_eq!(mon, None, "an unhighlighted list must not target one screen");
    }

    #[test]
    fn an_off_monitor_falls_back_rather_than_showing_nothing() {
        let mut snap = snapshot_two_effects();
        snap["snapshot"]["wallpaper"][0]["effect"] = serde_json::json!("off");
        let (_, eff, mon) = build_wallpaper_rows(&snap["snapshot"], 0);
        assert_eq!(eff, "flames", "an off monitor falls through to one that is on");
        // Fallen back to another screen's effect, so the edit must NOT be
        // aimed at the highlighted (off) monitor.
        assert_eq!(mon, None);
    }

    #[test]
    fn all_monitors_off_says_so_instead_of_showing_stale_knobs() {
        let mut snap = snapshot_two_effects();
        snap["snapshot"]["wallpaper"][0]["effect"] = serde_json::json!("off");
        snap["snapshot"]["wallpaper"][1]["effect"] = serde_json::json!("off");
        let (rows, eff, mon) = build_wallpaper_rows(&snap["snapshot"], 0);
        assert!(eff.is_empty());
        assert_eq!(mon, None);
        assert!(!rows.iter().any(|r| matches!(r, Row::Param(_))), "no knobs to show");
        let text: String = rows.iter().map(|r| row_value(r, "")).collect::<Vec<_>>().join(" ");
        assert!(text.contains("turn one on"), "must explain, not just go blank");
    }

    #[test]
    fn wallpaper_params_sit_between_apply_all_and_the_fps_rows() {
        // Load-bearing for selection stability: the monitor rows stay first and
        // fixed-length, so the cursor does not jump when the param block below
        // changes size.
        let snap = snapshot_two_effects();
        let (rows, _, _) = build_wallpaper_rows(&snap["snapshot"], 0);
        let apply = rows.iter().position(|r| matches!(r, Row::WallpaperApplyAll)).unwrap();
        let param = rows.iter().position(|r| matches!(r, Row::Param(_))).unwrap();
        let fps = rows
            .iter()
            .position(|r| matches!(r, Row::Config { key: "wallpaper_fps", .. }))
            .unwrap();
        assert!(apply < param, "params come after the monitor rows");
        assert!(param < fps, "params come before the fps/cell block");
        // And the monitor rows are the first thing in the list.
        assert!(matches!(rows[0], Row::WallpaperMonitor { .. }));
    }

    #[test]
    fn the_picker_moves_between_channels_and_wraps() {
        let mut pk = Picker::from_rgb(panefx::palette::Rgb(10, 20, 30));
        assert_eq!(pk.channel, 0);
        pk.cycle_channel(1);
        assert_eq!(pk.channel, 1);
        // Wrapping both ways: three channels is short enough that stepping off
        // either end and stopping would be a papercut on every use.
        pk.cycle_channel(2);
        assert_eq!(pk.channel, 0);
        pk.cycle_channel(-1);
        assert_eq!(pk.channel, 2);
    }

    #[test]
    fn the_picker_saturates_rather_than_wrapping() {
        // THE trap this avoids: wrapping red from 255 back to 0 turns a
        // near-white into a near-black in ONE keypress, which is never what the
        // hand meant.
        let mut pk = Picker::from_rgb(panefx::palette::Rgb(254, 1, 128));
        pk.channel = 0;
        pk.nudge(16);
        assert_eq!(pk.r, 255, "red must stop at the top");
        pk.channel = 1;
        pk.nudge(-16);
        assert_eq!(pk.g, 0, "green must stop at the bottom");
    }

    #[test]
    fn the_picker_only_moves_the_selected_channel() {
        let mut pk = Picker::from_rgb(panefx::palette::Rgb(100, 100, 100));
        pk.channel = 1;
        pk.nudge(20);
        assert_eq!((pk.r, pk.g, pk.b), (100, 120, 100));
    }

    #[test]
    fn the_picker_round_trips_through_hex() {
        // The picker's output is committed as text, so it has to survive the
        // same parse the typed path uses -- otherwise picking a colour and
        // typing one disagree.
        let pk = Picker::from_rgb(panefx::palette::Rgb(0x3a, 0x7f, 0x2c));
        assert_eq!(pk.hex(), "#3a7f2c");
        let back = panefx::palette::Rgb::parse_hex(&pk.hex()).expect("must reparse");
        assert_eq!(back, panefx::palette::Rgb(0x3a, 0x7f, 0x2c));
    }

    #[test]
    fn the_channel_bars_track_their_value() {
        // The bar IS the feedback -- if it does not track the number, the
        // picker is just a hex field with extra steps.
        assert_eq!(Picker::bar(0, 8).chars().filter(|c| *c == '\u{2588}').count(), 0);
        assert_eq!(Picker::bar(255, 8).chars().filter(|c| *c == '\u{2588}').count(), 8);
        let half = Picker::bar(128, 8).chars().filter(|c| *c == '\u{2588}').count();
        assert!((3..=5).contains(&half), "half-value bar was {half}/8");
        // Always full width, so the columns beside it stay aligned.
        assert_eq!(Picker::bar(70, 12).chars().count(), 12);
    }

    #[test]
    fn a_zero_width_bar_does_not_panic() {
        // Width comes from a layout calculation, so 0 has to be survivable.
        assert_eq!(Picker::bar(200, 0).chars().count(), 1);
    }

    #[test]
    fn a_wallpaper_param_carries_the_monitor_it_targets() {
        // THE per-monitor fix, as a tripwire. Params are stored per effect with
        // per-monitor overrides, so the command MUST say which screen it means.
        // Without it every screen running an effect shared one set of knobs and
        // tuning one tuned all of them.
        let v = serde_json::json!({"kind":"int","v":400});
        let one = param_command(true, "waves", Some(3), "darkcut", v.clone());
        assert_eq!(one["monitor"], 3);
        // `null` is MEANINGFUL: the shared block, i.e. every screen. It must be
        // sent explicitly rather than omitted, so the daemon never guesses.
        let all = param_command(true, "waves", None, "darkcut", v);
        assert!(all["monitor"].is_null(), "the shared case must be explicit");
    }

    #[test]
    fn a_wallpaper_param_never_retunes_the_pane() {
        // THE trap. `Row::Param` is shared between tabs, so a missing tab check
        // sends the pane's `param` command from the Wallpaper tab -- retuning
        // the terminal backdrop while the desktop sits unchanged, with no error
        // anywhere. Sabotage-checked: reverting the fork fails this test.
        let v = serde_json::json!({"kind":"int","v":400});
        let wall = param_command(true, "waves", Some(2), "darkcut", v.clone());
        assert_eq!(wall["cmd"], "wallpaper_param");
        assert_eq!(wall["effect"], "waves", "the effect must be explicit");
        assert_eq!(wall["key"], "darkcut");

        let pane = param_command(false, "waves", None, "darkcut", v);
        assert_eq!(pane["cmd"], "param");
        assert!(pane.get("effect").is_none(), "the pane has one current effect");
    }

    #[test]
    fn the_two_param_commands_are_never_the_same_shape() {
        // Guards against a refactor that "unifies" them and reintroduces the bug.
        let v = serde_json::json!({"kind":"int","v":1});
        assert_ne!(
            param_command(true, "waves", None, "k", v.clone())["cmd"],
            param_command(false, "waves", None, "k", v)["cmd"]
        );
    }
}

#[cfg(test)]
mod units_tests {
    use super::*;

    #[test]
    fn a_x1000_param_shows_its_decimal_value() {
        // `headroom 1350` really means 1.350. The label hinted at it, but the
        // division was left to the reader.
        assert_eq!(value_units("headroom", "headroom (x1000)", 1350), " = 1.350");
        assert_eq!(value_units("dark", "dark cutoff (x1000)", 276), " = 0.276");
        assert_eq!(value_units("speed", "speed (x1000)", 1000), " = 1.000");
    }

    #[test]
    fn the_suffix_comes_from_the_key_not_a_hardcoded_list() {
        // A NEW effect shipping a `foo (x1000)` knob must format correctly with
        // no change to this file -- the TUI renders what it is handed and never
        // hardcodes effect knowledge.
        assert_eq!(
            value_units("brand_new_knob", "brand new knob (x1000)", 42),
            " = 0.042"
        );
    }

    #[test]
    fn pixel_rows_are_labelled_px() {
        // These were bare integers with no unit anywhere on screen.
        assert_eq!(value_units("wallpaper_cell_w", "wp cell width", 15), " px");
        assert_eq!(value_units("cell_h", "cell height", 15), " px");
        assert_eq!(value_units("pad_x", "pad x", 10), " px");
        assert_eq!(value_units("crop_top", "crop top", 0), " px");
    }

    #[test]
    fn rates_and_percentages_say_so() {
        assert_eq!(value_units("fps", "fps", 10), " fps");
        assert_eq!(value_units("wallpaper_fps", "wallpaper fps", 5), " fps");
        assert_eq!(value_units("opacity", "opacity", 60), "%");
    }

    #[test]
    fn the_detail_row_shows_the_cell_size_it_selects() {
        // On its own the detail number is an index into a ladder and says
        // nothing about what will appear on screen.
        assert_eq!(
            value_units("wallpaper_detail", "detail", 10),
            "  -> 6x9 px cells"
        );
        assert_eq!(
            value_units("wallpaper_detail", "detail", 1),
            "  -> 24x37 px cells"
        );
    }

    #[test]
    fn a_plain_number_gets_no_invented_unit() {
        // Better silent than wrong: guessing a unit for an unknown knob would
        // mislabel it confidently.
        assert_eq!(value_units("rotate_secs", "rotate secs", 0), "");
        assert_eq!(value_units("mystery", "mystery", 7), "");
    }
}
