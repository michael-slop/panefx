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
}

struct App {
    conn: Conn,
    effect: String,
    effects: Vec<String>,
    rows: Vec<Row>,
    sel: ListState,
    status: String,
    dirty: bool,
    editing: Option<String>,
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
            rows: Vec::new(),
            sel: ListState::default(),
            status: "connected".into(),
            dirty: false,
            editing: None,
        };
        app.absorb(&reply);
        app.sel.select(Some(0));
        Ok(app)
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
        self.rows = rows;
    }

    fn selected(&self) -> usize {
        self.sel.selected().unwrap_or(0)
    }

    fn nudge(&mut self, delta: i64) {
        let idx = self.selected();
        let Some(row) = self.rows.get(idx).cloned() else {
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

    fn commit_edit(&mut self, text: String) {
        let idx = self.selected();
        let Some(row) = self.rows.get(idx).cloned() else {
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
        };
        self.dispatch(msg);
    }
}

fn row_label(r: &Row) -> String {
    match r {
        Row::Effect => "effect".into(),
        Row::Param(p) => p.label.clone(),
        Row::Config { label, .. } => (*label).into(),
        Row::ConfigText { label, .. } => (*label).into(),
    }
}

fn row_value(r: &Row, effect: &str) -> String {
    match r {
        Row::Effect => format!("‹ {effect} ›"),
        Row::Param(p) => p.value.display(),
        Row::Config { value, .. } => value.to_string(),
        Row::ConfigText { value, .. } => value.clone(),
    }
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

        if !event::poll(Duration::from_millis(120))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
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
            KeyCode::Down | KeyCode::Char('j') => {
                let n = app.rows.len();
                if n > 0 {
                    app.sel.select(Some((app.selected() + 1) % n));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let n = app.rows.len();
                if n > 0 {
                    app.sel.select(Some((app.selected() + n - 1) % n));
                }
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('+') => app.nudge(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('-') => app.nudge(-1),
            KeyCode::Char('L') => app.nudge(10),
            KeyCode::Char('H') => app.nudge(-10),
            KeyCode::Enter => {
                let cur = app
                    .rows
                    .get(app.selected())
                    .map(|r| row_value(r, &app.effect))
                    .unwrap_or_default();
                // Start from the current value so a small tweak is easy.
                app.editing = Some(if cur.starts_with('‹') { String::new() } else { cur });
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
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "panefx",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  effect: "),
            Span::styled(
                app.effect.clone(),
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(if app.dirty { "   *modified" } else { "" }),
        ]))
        .block(Block::default().borders(Borders::ALL).title(title)),
        chunks[0],
    );

    let sel = app.selected();
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
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
            ListItem::new(Line::from(vec![
                Span::styled(format!("  {:<18}", row_label(r)), style),
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

    let mut state = app.sel.clone();
    f.render_stateful_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ↑↓ move   ←→ adjust (H/L ×10)   Enter type "),
        ),
        chunks[1],
        &mut state,
    );

    // Tell the user which keys do anything on THIS row — colours and text
    // cannot be nudged, and silently ignoring ←→ on them reads as broken.
    let hint = match app.rows.get(sel) {
        Some(Row::Param(p)) if !matches!(p.value, ParamValue::Int { .. }) => {
            "  ·  Enter to type a value"
        }
        Some(Row::ConfigText { .. }) => "  ·  Enter to type a value",
        Some(Row::Effect) => "  ·  ←→ cycles effects",
        _ => "",
    };
    f.render_widget(
        Paragraph::new(format!("{}{}", app.status, hint))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" [s]ave  [r]evert  [q]uit "),
            ),
        chunks[2],
    );
}
