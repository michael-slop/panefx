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
}

impl View {
    fn idx(self) -> usize {
        match self {
            View::Effects => 0,
            View::Wallpaper => 1,
        }
    }
    fn next(self) -> Self {
        match self {
            View::Effects => View::Wallpaper,
            View::Wallpaper => View::Effects,
        }
    }
}

struct App {
    conn: Conn,
    effect: String,
    effects: Vec<String>,
    /// Rows per view, rebuilt from every snapshot.
    rows: [Vec<Row>; 2],
    /// Selection per view, PRESERVED across rebuilds.
    sel: [ListState; 2],
    view: View,
    status: String,
    dirty: bool,
    /// Caret phase. Toggled by the event loop so the selected row blinks.
    blink_on: bool,
    last_blink: std::time::Instant,
    editing: Option<String>,
    /// True when the daemon has a working wallpaper layer.
    wallpaper_ok: bool,
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
            rows: [Vec::new(), Vec::new()],
            sel: [ListState::default(), ListState::default()],
            view: View::Effects,
            status: "connected".into(),
            dirty: false,
            blink_on: true,
            last_blink: std::time::Instant::now(),
            editing: None,
            wallpaper_ok: false,
        };
        app.sel[0].select(Some(0));
        app.sel[1].select(Some(0));
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
        let ci = |k: &str| cfg.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
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
        rows.push(Row::Config {
            key: "fps",
            label: "fps",
            value: ci("fps"),
            min: 1,
            max: 120,
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
        self.rows[View::Wallpaper.idx()] = build_wallpaper_rows(snap);

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
                    serde_json::json!({"cmd":"param","key":p.key,
                        "val":{"kind":"int","v":nv}})
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
            Row::Note(_) => return,
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
            Row::Param(p) => match &p.value {
                ParamValue::Text { .. } => serde_json::json!({"cmd":"param","key":p.key,
                    "val":{"kind":"text","v":text}}),
                ParamValue::Colour { .. } => {
                    match panefx::palette::Rgb::parse_hex(&text) {
                        Some(c) => serde_json::json!({"cmd":"param","key":p.key,
                            "val":{"kind":"colour","r":c.0,"g":c.1,"b":c.2}}),
                        None => {
                            self.status = format!("'{text}' is not #rrggbb");
                            return;
                        }
                    }
                }
                ParamValue::Int { .. } => match text.trim().parse::<i64>() {
                    Ok(n) => serde_json::json!({"cmd":"param","key":p.key,
                        "val":{"kind":"int","v":n.clamp(p.min,p.max)}}),
                    Err(_) => {
                        self.status = format!("'{text}' is not a number");
                        return;
                    }
                },
            },
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
            Row::WallpaperMonitor { .. } | Row::WallpaperApplyAll | Row::Note(_) => return,
        };
        self.dispatch(msg);
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
fn build_wallpaper_rows(snap: &serde_json::Value) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let cfg = snap.get("config").cloned().unwrap_or_default();
    let ci = |k: &str| cfg.get(k).and_then(|v| v.as_i64()).unwrap_or(0);

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
                "Windows 11 25H2 removed the layer third-party wallpapers used.".into(),
            ));
            rows.push(Row::Note(
                "Other wallpaper apps hit the same wall on this build.".into(),
            ));
        }
        rows.push(Row::Note(
            "Terminal and Neovide backdrops are unaffected.".into(),
        ));
        return rows;
    }

    if monitors.is_empty() {
        rows.push(Row::Note("no monitors detected".into()));
        return rows;
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
    rows
}

fn row_label(r: &Row) -> String {
    match r {
        Row::Effect => "effect".into(),
        Row::Param(p) => p.label.clone(),
        Row::Config { label, .. } => (*label).into(),
        Row::ConfigText { label, .. } => (*label).into(),
        Row::WallpaperMonitor { index, label, .. } => format!("{index}  {label}"),
        Row::WallpaperApplyAll => "apply to all".into(),
        Row::Note(_) => String::new(),
    }
}

fn row_value(r: &Row, effect: &str) -> String {
    match r {
        Row::Effect => format!("‹ {effect} ›"),
        Row::Param(p) => p.value.display(),
        Row::Config { value, .. } => value.to_string(),
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
        Row::Note(t) => t.clone(),
    }
}

/// Rows that cannot be selected — pure explanation.
fn row_is_note(r: &Row) -> bool {
    matches!(r, Row::Note(_))
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

fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::var("PANEFX_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let conn = match Conn::connect(port) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("panefx-ctl: cannot reach the daemon on 127.0.0.1:{port} ({e})");
            eprintln!("Is panefx running? It logs '[panefx] control channel on ...' at startup.");
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
                app.status = match app.view {
                    View::Effects => "effects".into(),
                    View::Wallpaper => "desktop wallpaper".into(),
                };
            }
            KeyCode::Char('w') => app.view = View::Wallpaper,
            KeyCode::Char('e') => app.view = View::Effects,
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
        Span::styled(" Effects ", tab_style(View::Effects)),
        Span::raw(" "),
        Span::styled(" Wallpaper ", tab_style(View::Wallpaper)),
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
            if row_is_note(r) {
                return ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(row_value(r, &app.effect), Style::default().fg(Color::Yellow)),
                ]));
            }
            let editing = i == sel && app.editing.is_some();
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
                    .title(" [Tab] view  [s]ave  [r]evert  [q]uit "),
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
        let rows = build_wallpaper_rows(&snapshot_without_layer()["snapshot"]);
        assert!(!rows.is_empty(), "never show an empty wallpaper tab");
        let text: String = rows.iter().map(|r| row_value(r, "waves")).collect::<Vec<_>>().join(" ");
        assert!(text.contains("unavailable"));
        assert!(text.contains("25H2"), "name the actual cause");
        assert!(text.contains("unaffected"), "say what still works");
        assert!(rows.iter().all(row_is_note), "nothing here is selectable");
    }

    #[test]
    fn monitors_render_with_effect_and_frozen_state() {
        let rows = build_wallpaper_rows(&snapshot_with_monitors()["snapshot"]);
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
}
