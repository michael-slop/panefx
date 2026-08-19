//! panefx-gui — the Windows 98 control panel.
//!
//! The TUI with a better input device, not a new product. Same control channel,
//! same commands, both can run at once; `panefx-ctl` keeps working and is the
//! only way to tune panefx over SSH.
//!
//! Every constraint in the TUI — one highlighted row, arrow-key nudging, typed
//! hex codes, no previews — is a terminal limitation rather than a design
//! choice. This removes them.
//!
//! See `GUI-HANDOFF.md` for the decisions behind the layout, and `src/win98.rs`
//! for where the chrome comes from.

#![windows_subsystem = "windows"]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use eframe::egui::{self, Color32, Rect, Vec2};
use panefx::gui_prefs::GuiPrefs;
use panefx::win98::{self, Bevel, Palette};

const WINDOW_TITLE: &str = "panefx";

fn main() -> eframe::Result<()> {
    // Single instance. Two GUIs both writing config.toml is a real way to lose
    // settings -- the second to save wins and the first never knows.
    //
    // The check lives HERE rather than only in the `panefx` launcher because
    // this binary is also started directly: from the tray, from a shortcut, or
    // by someone running panefx-gui.exe. Guarding only the launcher leaves
    // every one of those paths able to open a duplicate.
    if panefx::desktop::focus_existing_gui() {
        return Ok(());
    }

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 620.0])
            .with_min_inner_size([620.0, 420.0])
            // The app draws its own Win98 title bar; the OS one would be a
            // second, differently-styled bar stacked above it.
            //
            // `with_taskbar` stays true: undecorated must not also mean
            // invisible to alt-tab.
            .with_decorations(false)
            .with_resizable(true)
            .with_icon(window_icon())
            .with_title(WINDOW_TITLE),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        opts,
        Box::new(|cc| {
            let app = App::new();
            // Apply the RESTORED theme, not a hardcoded one: loading the
            // preference and then ignoring it is the obvious way for this to
            // look like it never saved.
            win98::apply_theme(
                &cc.egui_ctx,
                if app.dark { &Palette::DARK } else { &Palette::LIGHT },
            );
            Ok(Box::new(app))
        }),
    )
}

/// The panefx icon, for the taskbar and alt-tab.
///
/// The same art the daemon's tray icon uses, so both halves of panefx look like
/// one program. Stored BGRA for Win32's `CreateIconIndirect`; egui wants RGBA,
/// so the two colour channels swap here rather than the art being kept twice.
fn window_icon() -> egui::IconData {
    let src = &panefx::icon_art::ICON_32;
    let mut rgba = Vec::with_capacity(src.len());
    for px in src.chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    egui::IconData {
        rgba,
        width: panefx::icon_art::ICON_32_SIZE as u32,
        height: panefx::icon_art::ICON_32_SIZE as u32,
    }
}

// ---------------------------------------------------------------- connection

/// One request/response round trip to the daemon.
///
/// Deliberately the same shape as the TUI's `Conn` (`panefx-ctl.rs:151`): a
/// blocking connect, a 2s read timeout, one line of JSON each way. Sharing the
/// shape rather than the code because the two clients differ in what they do
/// when it fails — see `Daemon`.
struct Conn {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

impl Conn {
    fn connect(port: u16) -> std::io::Result<Self> {
        // `connect_timeout`, NOT `TcpStream::connect`. A plain connect to a
        // refused port blocks for about a second on Windows, and this runs on
        // the UI thread -- long enough that the window stops pumping messages
        // and Windows paints "(Not Responding)" over the title bar. Measured:
        // that is exactly what the first build did while the daemon was down.
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let s = TcpStream::connect_timeout(&addr, Duration::from_millis(120))?;
        // Short read timeout for the same reason. The daemon answers a `get` in
        // microseconds; anything slower than this is a daemon that has stopped
        // servicing its socket, and waiting two seconds for it would freeze the
        // window rather than report it.
        s.set_read_timeout(Some(Duration::from_millis(400)))?;
        s.set_write_timeout(Some(Duration::from_millis(400)))?;
        Ok(Conn {
            reader: BufReader::new(s.try_clone()?),
            stream: s,
        })
    }

    fn send(&mut self, msg: &serde_json::Value) -> std::io::Result<serde_json::Value> {
        writeln!(self.stream, "{msg}")?;
        self.stream.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// What the GUI knows about the daemon right now.
///
/// The TUI EXITS if the daemon is absent (`panefx-ctl.rs:1024`). The GUI must
/// not: the previews are rendered here, so most of the window is still useful
/// with nothing listening. It offers to start it instead.
enum Daemon {
    Up(Conn),
    Down(String),
}

// ---------------------------------------------------------------- the app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// The backdrops behind Alacritty windows. Needs GlazeWM.
    Panes,
    /// The desktop wallpaper, per monitor. Needs nothing.
    Wallpaper,
    Logs,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Panes, Tab::Wallpaper, Tab::Logs];
    fn label(self) -> &'static str {
        match self {
            Tab::Panes => "TUI-fx",
            Tab::Wallpaper => "wallpaper",
            Tab::Logs => "logs",
        }
    }

    /// (see App::remember)
    /// Stable key for the preferences file.
    ///
    /// Deliberately not `label()`: those are display strings, and renaming a
    /// tab in the UI must not silently reset everyone's saved tab.
    fn key(self) -> &'static str {
        match self {
            Tab::Panes => "panes",
            Tab::Wallpaper => "wallpaper",
            Tab::Logs => "logs",
        }
    }

    fn from_key(k: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == k)
    }
}

struct App {
    daemon: Daemon,
    port: u16,
    tab: Tab,
    /// The last snapshot. Kept so the window still renders between polls and
    /// while the daemon is away.
    snap: serde_json::Value,
    /// Which monitor the Wallpaper tab's detail pane is showing.
    selected_monitor: usize,
    /// Unsaved changes. The TUI only hints at this; the GUI shows it, because
    /// "why did my effects revert on restart" is the question it answers.
    dirty: bool,
    status: String,
    dark: bool,
    /// Which colour param has its picker open, as `(scope, key)`. `scope` is
    /// the monitor index, or `usize::MAX` for the pane.
    ///
    /// One at a time: two open pickers would both be editing and only one can
    /// have the pointer, which reads as the other being stuck.
    open_picker: Option<(usize, String)>,
    /// The colour being built in the open picker. Committed on Apply, so
    /// dragging around the wheel does not send a command per pixel.
    picker_rgb: [u8; 3],
    /// Which effect and layer the open picker belongs to.
    picker_effect: String,
    picker_layer: usize,
    /// Deferred, because the window closure borrows `self` immutably and
    /// cannot send a command from inside it.
    pending_apply: Option<(Option<usize>, String, usize, String, [u8; 3])>,
    pending_close: bool,
    /// The copy-to-displays sheet is open for this monitor.
    copy_from: Option<usize>,
    about: bool,
    /// Cached: probing the filesystem every frame would be silly.
    glazewm: bool,
    last_poll: std::time::Instant,
}

impl App {
    /// Persist the GUI's own preferences.
    ///
    /// One function rather than a write at each call site: theme, tab and
    /// monitor are saved together, so adding a preference cannot leave one of
    /// three places forgetting to record it.
    fn remember(&self) {
        GuiPrefs {
            dark: self.dark,
            selected_monitor: self.selected_monitor,
            tab: self.tab.key().to_string(),
        }
        .save();
    }

    fn new() -> Self {
        let port = std::env::var("PANEFX_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(panefx::control::DEFAULT_PORT);
        // Whatever the user left set last time.
        let prefs = GuiPrefs::load();
        let mut app = App {
            daemon: Daemon::Down("connecting…".into()),
            port,
            tab: Tab::from_key(&prefs.tab).unwrap_or(Tab::Wallpaper),
            snap: serde_json::Value::Null,
            selected_monitor: prefs.selected_monitor,
            dirty: false,
            status: String::new(),
            dark: prefs.dark,
            open_picker: None,
            picker_rgb: [128, 128, 128],
            picker_effect: String::new(),
            picker_layer: 0,
            pending_apply: None,
            pending_close: false,
            copy_from: None,
            about: false,
            glazewm: glazewm_path().is_some(),
            last_poll: std::time::Instant::now(),
        };
        app.reconnect();
        app
    }

    fn reconnect(&mut self) {
        match Conn::connect(self.port) {
            Ok(c) => {
                self.daemon = Daemon::Up(c);
                self.refresh();
            }
            Err(e) => self.daemon = Daemon::Down(e.to_string()),
        }
    }

    /// Send a command and absorb the snapshot it replies with.
    fn send(&mut self, msg: serde_json::Value) {
        let Daemon::Up(conn) = &mut self.daemon else {
            self.status = "not connected".into();
            return;
        };
        match conn.send(&msg) {
            Ok(reply) => {
                if reply.get("ok").and_then(|v| v.as_bool()) == Some(false) {
                    self.status = reply
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("command failed")
                        .to_string();
                    return;
                }
                if let Some(s) = reply.get("snapshot") {
                    self.snap = s.clone();
                }
                // Anything that is not a read or a write-to-disk leaves the
                // daemon's live state ahead of config.toml.
                let cmd = msg.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                match cmd {
                    "get" | "logs" => {}
                    "save" | "revert" => self.dirty = false,
                    _ => self.dirty = true,
                }
            }
            // A dead connection is not fatal: previews keep working, and the
            // Down state offers to start the daemon again.
            Err(e) => self.daemon = Daemon::Down(e.to_string()),
        }
    }

    fn refresh(&mut self) {
        self.send(serde_json::json!({"cmd": "get"}));
    }

    fn monitors(&self) -> Vec<serde_json::Value> {
        self.snap
            .get("wallpaper")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    fn effects(&self) -> Vec<String> {
        self.snap
            .get("effects")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// GlazeWM's CLI, if it is installed.
///
/// The TUI-fx panes follow Alacritty windows through GlazeWM's IPC and are
/// **useless without it** — so the GUI has to say so rather than showing dead
/// controls. Also the path the daemon is started through: a process started
/// directly by a GUI can die with it, so GlazeWM owns it instead.
fn glazewm_path() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(r"C:\Program Files\glzr.io\GlazeWM\cli\glazewm.exe");
    p.exists().then_some(p)
}

impl eframe::App for App {
    // eframe 0.36 hands the app a `Ui`, not a `Context` -- the frame is already
    // begun and the central panel allocated.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = if self.dark { Palette::DARK } else { Palette::LIGHT };
        let ctx = ui.ctx().clone();

        // Poll: the daemon does not push, and something else (the TUI, a
        // script) may have changed it under us.
        // Poll while connected; retry more slowly while not. Even a 120ms
        // connect attempt is worth spacing out when it is going to fail --
        // hammering it once a second buys nothing and costs a visible hitch
        // every time.
        let interval = match self.daemon {
            Daemon::Up(_) => Duration::from_millis(900),
            Daemon::Down(_) => Duration::from_millis(2000),
        };
        if self.last_poll.elapsed() > interval {
            self.last_poll = std::time::Instant::now();
            match self.daemon {
                Daemon::Up(_) => self.refresh(),
                Daemon::Down(_) => self.reconnect(),
            }
        }
        // Previews will animate, so keep drawing.
        ctx.request_repaint_after(Duration::from_millis(100));

        let full = ui.max_rect();
        let painter = ui.painter().clone();
        win98::bevel(&painter, full, Bevel::Raised, &p, Some(p.button_face));

        let inner = full.shrink(win98::BEVEL_THICKNESS);
        let bar = Rect::from_min_size(inner.min, Vec2::new(inner.width(), win98::TITLE_BAR_HEIGHT));
        let caption = if self.dirty {
            format!("{WINDOW_TITLE} — unsaved")
        } else {
            WINDOW_TITLE.to_string()
        };
        win98::title_bar(&painter, bar, &caption, true, &p);
        if ui
            .interact(bar, ui.id().with("titlebar"), egui::Sense::click_and_drag())
            .is_pointer_button_down_on()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let body = Rect::from_min_max(
            egui::pos2(inner.min.x + 6.0, bar.max.y + 6.0),
            egui::pos2(inner.max.x - 6.0, inner.max.y - 6.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        self.chrome(&mut ui, &p);

        // These float above everything, so they are drawn last.
        self.colour_picker_window(&ctx, &p);
        self.copy_sheet(&ctx, &p);
        self.about_window(&ctx, &p);
        // Its buttons cannot send commands from inside the window closure --
        // that borrows `self` -- so they leave an intent here and it is acted
        // on now.
        if let Some((monitor, effect, layer, key, [r, g, b])) = self.pending_apply.take() {
            self.send_param(
                monitor,
                &effect,
                layer,
                &key,
                serde_json::json!({"kind":"colour","r":r,"g":g,"b":b}),
            );
            self.open_picker = None;
        }
        if std::mem::take(&mut self.pending_close) {
            self.open_picker = None;
        }
    }
}

impl App {
    fn chrome(&mut self, ui: &mut egui::Ui, p: &Palette) {
        self.tab_strip(ui, p);
        ui.add_space(6.0);

        // The body sits in a sunken well, the way a Win98 property sheet does.
        let well = Rect::from_min_max(
            ui.cursor().min,
            egui::pos2(ui.max_rect().max.x, ui.max_rect().max.y - 30.0),
        );
        win98::bevel(ui.painter(), well, Bevel::Sunken, p, Some(p.panel_bg));
        let pad = win98::BEVEL_THICKNESS + 6.0;
        let mut inner = ui.new_child(
            egui::UiBuilder::new().max_rect(well.shrink(pad)),
        );

        match self.daemon {
            Daemon::Down(_) => self.daemon_down(&mut inner, p),
            Daemon::Up(_) => match self.tab {
                Tab::Panes => self.panes_tab(&mut inner, p),
                Tab::Wallpaper => self.wallpaper_tab(&mut inner, p),
                Tab::Logs => self.logs_tab(&mut inner, p),
            },
        }

        self.status_bar(ui, p, well.max.y);
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            for t in Tab::ALL {
                let on = self.tab == t;
                let (rect, resp) = ui.allocate_exact_size(
                    Vec2::new(96.0, 24.0),
                    egui::Sense::click(),
                );
                // A button is RAISED when idle and PUSHED IN when selected --
                // depth, not a highlight colour. This is the same convention
                // the monitor buttons on the wallpaper tab use, and buttons
                // that disagree about which way is "on" read as broken.
                win98::bevel(
                    ui.painter(),
                    rect,
                    win98::toggle_bevel(on),
                    p,
                    Some(win98::toggle_face(on, p)),
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    t.label(),
                    egui::FontId::new(win98::size::TEXT, egui::FontFamily::Monospace),
                    if on { p.text } else { p.muted },
                );
                if resp.clicked() {
                    self.tab = t;
                    self.remember();
                    if t == Tab::Logs {
                        self.send(serde_json::json!({"cmd":"logs","lines":300}));
                    }
                }
            }
            ui.add_space(12.0);
            if ui.button(if self.dark { "light" } else { "dark" }).clicked() {
                self.dark = !self.dark;
                win98::apply_theme(
                    ui.ctx(),
                    if self.dark { &Palette::DARK } else { &Palette::LIGHT },
                );
                self.remember();
            }
            if ui.button("about").clicked() {
                self.about = !self.about;
            }
        });
    }

    /// The daemon is not listening. Previews would still work, so this is a
    /// state to recover from rather than an error to exit on.
    fn daemon_down(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let why = match &self.daemon {
            Daemon::Down(e) => e.clone(),
            _ => String::new(),
        };
        ui.label(
            egui::RichText::new("the panefx daemon is not running")
                .color(p.bad)
                .size(16.0),
        );
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("127.0.0.1:{} — {why}", self.port)).color(p.muted));
        ui.add_space(12.0);

        if let Some(glaze) = glazewm_path() {
            if ui.button("  start the daemon  ").clicked() {
                // Through GlazeWM, not directly: a process started by a GUI can
                // die with it. This is the same path the deploy script uses.
                let exe = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("panefx.exe")))
                    .unwrap_or_else(|| std::path::PathBuf::from("panefx.exe"));
                let _ = std::process::Command::new(glaze)
                    .args(["command", "shell-exec", &format!("{} --daemon", exe.display())])
                    .spawn();
                self.status = "asked GlazeWM to start the daemon".into();
            }
        } else {
            ui.label(
                egui::RichText::new("GlazeWM is not installed, so panefx cannot be started here.")
                    .color(p.warn),
            );
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("retrying every two seconds.").color(p.muted));
    }

    /// The backdrops behind Alacritty. **This is the half that needs GlazeWM.**
    fn panes_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if !self.glazewm {
            self.needs_glazewm(ui, p);
            return;
        }
        let effect = self
            .snap
            .get("effect")
            .and_then(|v| v.as_str())
            .unwrap_or("—")
            .to_string();
        ui.label(egui::RichText::new("terminal backdrops").size(16.0));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("effect:");
            let effects = self.effects();
            for e in &effects {
                if ui.selectable_label(*e == effect, e).clicked() {
                    self.send(serde_json::json!({"cmd":"effect","name":e}));
                }
            }
        });
        ui.add_space(8.0);
        let params: Vec<panefx::animation::Param> = self
            .snap
            .get("params")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if params.is_empty() {
            ui.label(egui::RichText::new("no tunable knobs").color(p.muted));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("paneparams")
            .show(ui, |ui| {
                self.params_editor(ui, p, &params, None, &effect, 0);
            });
    }

    /// What to show when GlazeWM is absent.
    ///
    /// Not a disabled control: the panes genuinely cannot work, and a greyed
    /// slider says "broken" where this says why and what to do about it.
    fn needs_glazewm(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.label(
            egui::RichText::new("TUI-fx needs GlazeWM")
                .color(p.warn)
                .size(16.0),
        );
        ui.add_space(6.0);
        ui.label("The terminal backdrops follow your windows as the tiling window");
        ui.label("manager moves them. Without GlazeWM there is nothing to follow,");
        ui.label("so these effects cannot run.");
        ui.add_space(10.0);
        ui.hyperlink_to("get GlazeWM — glzr.io", "https://glzr.io");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("and Alacritty, if you do not have a terminal yet:").color(p.muted));
        ui.hyperlink_to("alacritty.org", "https://alacritty.org");
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("The wallpaper tab works without either — it draws on the desktop.")
                .color(p.muted),
        );
    }

    /// Monitor list on the left, the selected monitor's detail on the right.
    fn wallpaper_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if let Some(err) = self.snap.get("wallpaper_error").and_then(|v| v.as_str()) {
            ui.label(egui::RichText::new(format!("wallpaper layer unavailable: {err}")).color(p.bad));
            return;
        }
        let monitors = self.monitors();
        if monitors.is_empty() {
            ui.label(egui::RichText::new("no monitors detected").color(p.muted));
            return;
        }
        self.selected_monitor = self.selected_monitor.min(monitors.len() - 1);

        let avail = ui.available_rect_before_wrap();
        let list_w = 220.0_f32.min(avail.width() * 0.4);
        let list = Rect::from_min_size(avail.min, Vec2::new(list_w, avail.height()));
        let detail = Rect::from_min_max(
            egui::pos2(list.max.x + 8.0, avail.min.y),
            avail.max,
        );

        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        self.monitor_list(&mut lui, p, &monitors);

        let mut dui = ui.new_child(egui::UiBuilder::new().max_rect(detail));
        self.monitor_detail(&mut dui, p, &monitors);
    }

    fn monitor_list(&mut self, ui: &mut egui::Ui, p: &Palette, monitors: &[serde_json::Value]) {
        for (i, m) in monitors.iter().enumerate() {
            let idx = m.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let label = m.get("label").and_then(|v| v.as_str()).unwrap_or("?");
            let effect = m.get("effect").and_then(|v| v.as_str()).unwrap_or("off");
            let layers = m
                .get("layers")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let occluded = m.get("occluded").and_then(|v| v.as_bool()).unwrap_or(false);

            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 54.0), egui::Sense::click());
            let on = i == self.selected_monitor;
            win98::bevel(
                ui.painter(),
                rect,
                win98::toggle_bevel(on),
                p,
                Some(win98::toggle_face(on, p)),
            );
            let f = |s: f32| egui::FontId::new(s, egui::FontFamily::Monospace);
            ui.painter().text(
                egui::pos2(rect.min.x + 8.0, rect.min.y + 6.0),
                egui::Align2::LEFT_TOP,
                format!("DISPLAY{idx}"),
                f(win98::size::TEXT),
                p.text,
            );
            ui.painter().text(
                egui::pos2(rect.min.x + 8.0, rect.min.y + 24.0),
                egui::Align2::LEFT_TOP,
                label,
                f(10.0),
                p.muted,
            );
            // The stack depth, so a layered monitor is obvious in the list.
            let tag = if layers > 1 {
                format!("{effect} +{}", layers - 1)
            } else {
                effect.to_string()
            };
            ui.painter().text(
                egui::pos2(rect.max.x - 8.0, rect.min.y + 6.0),
                egui::Align2::RIGHT_TOP,
                tag,
                f(11.0),
                if effect == "off" { p.muted } else { p.ok },
            );
            if occluded {
                ui.painter().text(
                    egui::pos2(rect.max.x - 8.0, rect.min.y + 26.0),
                    egui::Align2::RIGHT_TOP,
                    "frozen",
                    f(10.0),
                    p.warn,
                );
            }
            if resp.clicked() {
                self.selected_monitor = i;
                self.remember();
            }
            ui.add_space(4.0);
        }
    }

    fn monitor_detail(&mut self, ui: &mut egui::Ui, p: &Palette, monitors: &[serde_json::Value]) {
        let Some(m) = monitors.get(self.selected_monitor) else {
            return;
        };
        let idx = m.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let layers: Vec<String> = m
            .get("layers")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("DISPLAY{idx}")).size(16.0));
            ui.add_space(10.0);
            // Copying the whole STACK, not one effect: "make that screen look
            // like this one" means the layers too, and applying only the base
            // would silently drop them.
            if !layers.is_empty() && ui.button("copy to…").clicked() {
                self.copy_from = Some(idx);
            }
        });
        ui.add_space(6.0);

        // The stack, TOP FIRST -- the way it is seen, not the way it is stored.
        ui.label(egui::RichText::new("layers (nearest first)").color(p.muted));
        ui.add_space(2.0);
        for (li, name) in layers.iter().enumerate().rev() {
            ui.horizontal(|ui| {
                ui.label(format!("{li}:"));
                ui.label(egui::RichText::new(name).color(p.text));
                if li > 0 && ui.button("remove").clicked() {
                    self.send(serde_json::json!({
                        "cmd":"wallpaper_layer","monitor":idx,"layer":li,"name":"off"
                    }));
                }
            });
        }
        if layers.is_empty() {
            ui.label(egui::RichText::new("off").color(p.muted));
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("set the base effect").color(p.muted));
        let effects = self.effects();
        let base = layers.first().cloned().unwrap_or_else(|| "off".into());
        ui.horizontal_wrapped(|ui| {
            for e in std::iter::once("off".to_string()).chain(effects.iter().cloned()) {
                if ui.selectable_label(e == base, &e).clicked() {
                    self.send(serde_json::json!({
                        "cmd":"wallpaper_effect","monitor":idx,"name":e
                    }));
                }
            }
        });

        ui.add_space(8.0);
        ui.label(egui::RichText::new("add a layer above").color(p.muted));
        let next = layers.len().max(1);
        ui.horizontal_wrapped(|ui| {
            for e in self.effects() {
                if ui.button(&e).clicked() {
                    self.send(serde_json::json!({
                        "cmd":"wallpaper_layer","monitor":idx,"layer":next,"name":e
                    }));
                }
            }
        });

        ui.add_space(10.0);
        if layers.is_empty() {
            ui.label(egui::RichText::new("nothing to tune -- this screen is off").color(p.muted));
            return;
        }
        // EVERY layer gets its own knobs, top first so the list matches the
        // stack above it. Scrolled, because a stack can hold far more params
        // than the pane is tall.
        egui::ScrollArea::vertical()
            .id_salt(("wallparams", idx))
            .show(ui, |ui| {
                for (li, name) in layers.iter().enumerate().rev() {
                    let params = self.layer_params(idx, li);
                    let head = if li == 0 {
                        format!("layer 0 · {name}  (base)")
                    } else {
                        format!("layer {li} · {name}")
                    };
                    egui::CollapsingHeader::new(
                        egui::RichText::new(head).color(p.text),
                    )
                    // The top layer opens by default: it is the one just added
                    // and therefore the one being tuned.
                    .default_open(li + 1 == layers.len())
                    .id_salt(("layer", idx, li))
                    .show(ui, |ui| {
                        if params.is_empty() {
                            ui.label(
                                egui::RichText::new("no tunable knobs").color(p.muted),
                            );
                        } else {
                            self.params_editor(ui, p, &params, Some(idx), name, li);
                        }
                    });
                    ui.add_space(2.0);
                }
            });
    }

    /// Render one effect's params as real controls.
    ///
    /// `monitor` is the screen these belong to, or `None` for the pane. That
    /// decides which command carries the edit -- and for the wallpaper it must
    /// name the monitor, or every screen running the effect gets the change.
    fn params_editor(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        params: &[panefx::animation::Param],
        monitor: Option<usize>,
        effect: &str,
        layer: usize,
    ) {
        use panefx::animation::ParamValue;
        let scope = monitor.unwrap_or(usize::MAX);
        let label_w = 150.0;

        for param in params {
            ui.horizontal(|ui| {
                ui.allocate_ui(Vec2::new(label_w, 22.0), |ui| {
                    ui.label(egui::RichText::new(&param.label).size(12.0));
                });
                match &param.value {
                    ParamValue::Int { v } => {
                        let width = (ui.available_width() - 60.0).max(80.0);
                        if let Some(nv) =
                            win98::fader(ui, p, *v, param.min, param.max, width)
                        {
                            self.send_param(monitor, effect, layer, &param.key,
                                serde_json::json!({"kind":"int","v":nv}));
                        }
                        ui.label(egui::RichText::new(format!("{v}")).size(11.0).color(p.muted));
                    }
                    ParamValue::Colour { r, g, b } => {
                        let open = self.open_picker.as_ref()
                            == Some(&(scope, param.key.clone()));
                        if win98::swatch(
                            ui, p,
                            Color32::from_rgb(*r, *g, *b),
                            Vec2::new(56.0, 20.0),
                        )
                        .clicked()
                        {
                            if open {
                                self.open_picker = None;
                            } else {
                                self.picker_rgb = [*r, *g, *b];
                                self.picker_effect = effect.to_string();
                                self.picker_layer = layer;
                                self.open_picker = Some((scope, param.key.clone()));
                            }
                        }
                        ui.label(
                            egui::RichText::new(format!("#{r:02x}{g:02x}{b:02x}"))
                                .size(11.0)
                                .color(p.muted),
                        );
                    }
                    ParamValue::Text { v } => {
                        ui.label(egui::RichText::new(v).size(11.0).color(p.muted));
                    }
                }
            });

        }
    }

    /// The About box.
    ///
    /// Exists for a LICENCE OBLIGATION, not as decoration: BigBlueTerm437 is
    /// CC BY-SA 4.0 and embedding it requires attribution. `win98::ATTRIBUTION`
    /// carries the text and a test pins it, because it is exactly the kind of
    /// string that gets tidied out of a dialog by someone shortening it.
    fn about_window(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.about {
            return;
        }
        let mut open = true;
        egui::Window::new("about panefx")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(p.button_face)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("panefx")
                        .size(20.0)
                        .color(p.text),
                );
                ui.label(
                    egui::RichText::new(
                        "animated backdrops pinned behind windows, and on the desktop",
                    )
                    .color(p.muted),
                );
                ui.add_space(10.0);
                ui.label(egui::RichText::new("chrome").color(p.text));
                ui.label(
                    egui::RichText::new(
                        "Windows 98 look carried from michaelslop.org through slopkit.",
                    )
                    .color(p.muted)
                    .size(11.0),
                );
                ui.add_space(10.0);
                ui.label(egui::RichText::new("font").color(p.text));
                for line in win98::ATTRIBUTION.lines() {
                    ui.label(egui::RichText::new(line).color(p.muted).size(11.0));
                }
                ui.add_space(10.0);
                if ui.button("  close  ").clicked() {
                    self.about = false;
                }
            });
        if !open {
            self.about = false;
        }
    }

    /// "Apply this display's layers to…" — a checklist of the other monitors.
    ///
    /// A sheet rather than an "apply to all" button: with four screens, "all"
    /// is rarely what is meant. The portrait Acer and the 2560x720 ultrawide
    /// want different things, and copying to both to fix one is worse than not
    /// having the feature.
    fn copy_sheet(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(from) = self.copy_from else {
            return;
        };
        let monitors = self.monitors();
        let source: Vec<String> = monitors
            .iter()
            .find(|m| m.get("index").and_then(|v| v.as_u64()) == Some(from as u64))
            .and_then(|m| m.get("layers"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let mut open = true;
        let mut targets: Vec<usize> = Vec::new();
        egui::Window::new(format!("copy DISPLAY{from} to…"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(p.button_face)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("layers: {}", source.join(" → ")))
                        .color(p.muted),
                );
                ui.add_space(8.0);
                for m in &monitors {
                    let i = m.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    if i == from {
                        continue;
                    }
                    let label = m.get("label").and_then(|v| v.as_str()).unwrap_or("?");
                    let cur = m.get("effect").and_then(|v| v.as_str()).unwrap_or("off");
                    if ui
                        .button(format!("DISPLAY{i}  ({label}, now: {cur})"))
                        .clicked()
                    {
                        targets.push(i);
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("click a display to copy onto it")
                        .color(p.muted)
                        .size(11.0),
                );
            });

        for to in targets {
            self.copy_stack(&source, to);
            self.status = format!("copied DISPLAY{from} to DISPLAY{to}");
        }
        if !open {
            self.copy_from = None;
        }
    }

    /// Replace one monitor's whole stack with `source`.
    fn copy_stack(&mut self, source: &[String], to: usize) {
        // Clear DOWN from the top first. Setting the base while old upper
        // layers survive would leave a taller stack than was copied -- the
        // sheet says "make it look like that one", so leftovers are wrong.
        let existing = self
            .monitors()
            .iter()
            .find(|m| m.get("index").and_then(|v| v.as_u64()) == Some(to as u64))
            .and_then(|m| m.get("layers"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        for l in (1..existing).rev() {
            self.send(serde_json::json!({
                "cmd":"wallpaper_layer","monitor":to,"layer":l,"name":"off"
            }));
        }
        if source.is_empty() {
            self.send(serde_json::json!({
                "cmd":"wallpaper_effect","monitor":to,"name":"off"
            }));
            return;
        }
        for (l, effect) in source.iter().enumerate() {
            self.send(serde_json::json!({
                "cmd":"wallpaper_layer","monitor":to,"layer":l,"name":effect
            }));
        }
    }

    /// The colour wheel, in its own window.
    ///
    /// A WINDOW, not a panel drawn under the swatch. The params live in a
    /// `ScrollArea`, and a fixed-size child rect inside one fights the scroll
    /// layout: egui gave the saturation/value square no room and silently drew
    /// only the RGB fields -- a hex box with extra steps. A window is outside
    /// that layout and can simply be the size it needs.
    ///
    /// egui's own picker draws the saturation/value square with a hue strip,
    /// which is the wheel in rectangular form. Rebuilding colour theory to get
    /// a literal circle would look no better and behave worse.
    fn colour_picker_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some((scope, key)) = self.open_picker.clone() else {
            return;
        };
        let monitor = (scope != usize::MAX).then_some(scope);
        let effect = self.picker_effect.clone();
        let layer = self.picker_layer;

        let mut open = true;
        let mut colour = Color32::from_rgb(
            self.picker_rgb[0],
            self.picker_rgb[1],
            self.picker_rgb[2],
        );
        let title = match monitor {
            Some(m) => format!("{key} — DISPLAY{m} layer {layer}"),
            None => format!("{key} — terminal backdrop"),
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(p.button_face)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ctx, |ui| {
                // `slider_width` is what sizes the saturation/value square:
                // `color_slider_2d` allocates `Vec2::splat(slider_width)`.
                ui.spacing_mut().slider_width = 200.0;
                egui::widgets::color_picker::color_picker_color32(
                    ui,
                    &mut colour,
                    egui::widgets::color_picker::Alpha::Opaque,
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Committed on APPLY, not on every drag -- dragging around
                    // the wheel would otherwise send a command per pixel.
                    if ui.button("  apply  ").clicked() {
                        self.pending_apply = Some((
                            monitor,
                            effect.clone(),
                            layer,
                            key.clone(),
                            [colour.r(), colour.g(), colour.b()],
                        ));
                    }
                    if ui.button("  cancel  ").clicked() {
                        self.pending_close = true;
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "#{:02x}{:02x}{:02x}",
                            colour.r(),
                            colour.g(),
                            colour.b()
                        ))
                        .color(p.muted),
                    );
                });
            });
        self.picker_rgb = [colour.r(), colour.g(), colour.b()];
        if !open {
            self.pending_close = true;
        }
    }

    /// Route a param edit to the right command.
    ///
    /// The wallpaper one MUST carry the monitor: without it the daemon writes
    /// the shared block and every screen running the effect changes together --
    /// which is the exact bug per-monitor params were added to fix.
    fn send_param(
        &mut self,
        monitor: Option<usize>,
        effect: &str,
        _layer: usize,
        key: &str,
        val: serde_json::Value,
    ) {
        // NOTE: params are stored per (monitor, EFFECT), not per layer -- so
        // two layers running the same effect on one screen share their knobs.
        // The layer is threaded through anyway because that is a storage
        // limitation to lift, not a decision, and every call site already
        // knows which layer it is editing.
        let msg = match monitor {
            Some(m) => serde_json::json!({
                "cmd":"wallpaper_param","monitor":m,"effect":effect,"key":key,"val":val
            }),
            None => serde_json::json!({"cmd":"param","key":key,"val":val}),
        };
        self.send(msg);
    }

    /// The params the daemon reports for one LAYER of one monitor.
    fn layer_params(&self, idx: usize, layer: usize) -> Vec<panefx::animation::Param> {
        self.snap
            .get("wallpaper_params")
            .and_then(|m| m.get(format!("{idx}:{layer}")))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if ui.button("refresh").clicked() {
            self.send(serde_json::json!({"cmd":"logs","lines":300}));
        }
        ui.add_space(4.0);
        let entries = self
            .snap
            .get("logs")
            .and_then(|l| l.get("entries"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if entries.is_empty() {
            ui.label(egui::RichText::new("no log lines — press refresh").color(p.muted));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in &entries {
                let warn = e.get("level").and_then(|v| v.as_str()) == Some("warn");
                let text = e.get("text").and_then(|v| v.as_str()).unwrap_or("");
                ui.label(
                    egui::RichText::new(text)
                        .color(if warn { p.warn } else { p.muted })
                        .size(11.0),
                );
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette, y: f32) {
        let rect = Rect::from_min_max(
            egui::pos2(ui.max_rect().min.x, y + 4.0),
            egui::pos2(ui.max_rect().max.x, ui.max_rect().max.y),
        );
        win98::bevel(ui.painter(), rect, Bevel::Thin, p, Some(p.button_face));
        let mut bar = ui.new_child(
            egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(6.0, 2.0))),
        );
        bar.horizontal(|ui| {
            let connected = matches!(self.daemon, Daemon::Up(_));
            ui.label(
                egui::RichText::new(if connected { "connected" } else { "no daemon" })
                    .color(if connected { p.ok } else { p.bad })
                    .size(11.0),
            );
            ui.separator();
            // The dirty state, SHOWN rather than remembered -- the TUI only
            // hints at it, and "why did my effects revert" is the question.
            if self.dirty {
                ui.label(egui::RichText::new("unsaved").color(p.warn).size(11.0));
                if ui.small_button("save").clicked() {
                    self.send(serde_json::json!({"cmd":"save"}));
                    self.status = "saved to config.toml".into();
                }
                if ui.small_button("revert").clicked() {
                    self.send(serde_json::json!({"cmd":"revert"}));
                    self.status = "reverted".into();
                }
            } else {
                ui.label(egui::RichText::new("saved").color(p.muted).size(11.0));
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new(&self.status).color(p.muted).size(11.0));
            }
            let _: Color32 = p.black;
        });
    }
}
