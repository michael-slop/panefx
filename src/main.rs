// No console window for the daemon.
//
// Without this the daemon is a console subsystem app, so Windows attaches a
// console to it — a stray terminal window sits on screen for the daemon's whole
// life, and CLOSING THAT WINDOW KILLS THE DAEMON. It is a background service;
// it should have no window at all.
//
// `panefx-ctl` deliberately does NOT do this: it is a terminal UI and needs its
// console.
//
// Trade-off: `println!` output now goes nowhere when launched normally. That is
// why startup problems are reported by other means (the font check warns via
// the control channel's absence, and `-RedirectStandardOutput` still captures
// output when the daemon is started explicitly for debugging).
#![windows_subsystem = "windows"]

//! panefx — animated ASCII backdrops behind transparent windows.
//!
//! One panel per target window (Alacritty and Neovide by default; see
//! `ipc::DEFAULT_TARGETS`), each pinned directly behind it. A SINGLE simulation
//! is shared by every panel; each panel blits its own sub-rect. N panels
//! therefore cost N blits but only one simulation step.
//!
//! Because GlazeWM emits no move/resize event, the loop is:
//!   * events (any of them) are a hint that the layout may have changed
//!   * every hint triggers `query windows`, which is the only geometry source
//!   * a periodic poll covers changes that emit nothing at all
//!   * every reconcile also RE-PINS z-order, because GlazeWM re-orders windows
//!     on focus changes and would otherwise strand our panels

use panefx::{animation, config, control, ipc, panel, render, term_opacity, tray, wallpaper};

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

use animation::AsciiAnimation;
use panel::Panel;

/// Backstop re-query. Covers layout changes that emit no event at all, such as
/// a manual drag-resize of a floating window.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long the opacity slider must settle before the value is written out.
///
/// Each write rewrites a line in the terminal's own config, which the terminal
/// then re-parses — so a drag from 10% to 100% would otherwise cause ~90 writes.
/// Short enough to feel live against Alacritty's own ~2s reload latency.
const OPACITY_DEBOUNCE: Duration = Duration::from_millis(150);

/// What the user asked for on the command line.
enum Mode {
    /// Run the background daemon. What GlazeWM launches.
    Daemon,
    /// Hand off to the control TUI. What a person typing `panefx` wants.
    Tui,
    Help,
    /// An unrecognised flag — say so rather than guessing.
    Unknown(String),
}

fn parse_args() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => Mode::Tui,
        Some("-d") | Some("--daemon") => Mode::Daemon,
        Some("-h") | Some("--help") => Mode::Help,
        Some(other) => Mode::Unknown(other.to_string()),
    }
}

const HELP: &str = "\
panefx — animated ASCII backdrops behind windows, and on the desktop

USAGE:
    panefx              open the control TUI (same as `panefx-ctl`)
    panefx --daemon     run the background daemon
    panefx --help       this text

The daemon is normally started by your window manager, not by hand — it
reads every window position from GlazeWM's IPC, so it is a function of the
WM rather than an independent service.
";

/// Launch the control TUI by handing over to `panefx-ctl` next to us.
///
/// The TUI is a SEPARATE BINARY rather than a mode of this one, because this
/// binary is `#![windows_subsystem = "windows"]` — it has no console at all, and
/// a terminal UI needs one. `AttachConsole(ATTACH_PARENT_PROCESS)` could borrow
/// the caller's, but that path is full of sharp edges (it fails with
/// ERROR_INVALID_HANDLE when the parent has no console, the std handles have to
/// be rebound by hand, and Ctrl-C routing gets murky).
///
/// Running the real console binary avoids all of it: `panefx-ctl` owns its
/// terminal the way any console program does. `build.ps1` installs both to the
/// same directory, so resolving a sibling is reliable.
fn launch_tui() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let ctl = exe
        .parent()
        .map(|d| d.join("panefx-ctl.exe"))
        .filter(|p| p.exists())
        // Fall back to PATH: someone may have copied only one binary, and a
        // clear "not found" beats a confusing "no such file".
        .unwrap_or_else(|| std::path::PathBuf::from("panefx-ctl.exe"));

    let status = std::process::Command::new(&ctl)
        .args(std::env::args().skip(1))
        .status();

    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(0)),
        Err(e) => {
            panefx::log_warn!(
                "panefx: cannot start the control TUI ({}): {e}\n\
                 Expected panefx-ctl.exe next to {}.\n\
                 Run `panefx --daemon` for the background daemon.",
                ctl.display(),
                exe.display()
            );
            std::process::exit(1);
        }
    }
}

fn main() -> anyhow::Result<()> {
    match parse_args() {
        Mode::Tui => return launch_tui(),
        Mode::Help => {
            // No console on this binary, so printing here goes nowhere useful.
            // Hand `--help` to the console binary, which can actually show it.
            return launch_tui();
        }
        Mode::Unknown(flag) => {
            panefx::log_warn!("panefx: unknown option '{flag}'\n\n{HELP}");
            std::process::exit(2);
        }
        Mode::Daemon => {}
    }

    let mut cfg = config::Config::load();

    // Font substitution is SILENT — CreateFontW succeeds and you simply get a
    // different typeface. Say so loudly rather than rendering the wrong font
    // for weeks.
    match render::verify_font(&cfg.font) {
        Some(got) if got.eq_ignore_ascii_case(&cfg.font) => {
            panefx::log_info!("[panefx] font OK: {got:?}");
        }
        Some(got) => {
            panefx::log_warn!(
                "[panefx] WARNING: asked for {:?} but GDI selected {:?}. \
                 Is the font installed, and is the family name exact?",
                cfg.font, got
            );
        }
        None => panefx::log_warn!("[panefx] WARNING: could not verify font {:?}", cfg.font),
    }

    panel::register_class()?;

    // The GlazeWM socket lives on its own thread; the render loop only ever
    // does a non-blocking try_recv. See `ipc::IpcThread` for why.
    let client = match ipc::IpcThread::spawn() {
        Ok(c) => c,
        Err(e) => {
            panefx::log_warn!(
                "[panefx] cannot reach GlazeWM at {}: {e}\n\
                 Is GlazeWM running? Panels need its IPC for window geometry.",
                ipc::IPC_URL
            );
            return Err(e);
        }
    };

    // Tray icon: the daemon has no window and no console, so without this it is
    // completely invisible -- no way to tell it is running, reach the TUI, or
    // restart it after a bad state. Failure here is not fatal.
    let _tray = match tray::Tray::new() {
        Ok(t) => Some(t),
        Err(e) => {
            panefx::log_warn!("[panefx] no tray icon: {e}");
            None
        }
    };

    let mut panels: HashMap<isize, Panel> = HashMap::new();

    // Newest requested opacity and when it was requested, for the debounce
    // below. `None` means nothing is pending.
    let mut pending_opacity: Option<(u8, Instant)> = None;

    // Push the configured opacity out once at startup so panefx's config and
    // the terminal's cannot drift apart.
    match term_opacity::apply(cfg.opacity) {
        Ok(term_opacity::Outcome::Written) => {
            panefx::log_info!("[panefx] background opacity -> {}%", cfg.opacity)
        }
        Ok(term_opacity::Outcome::AlreadyCorrect) => {}
        Ok(term_opacity::Outcome::KeyMissing) => panefx::log_warn!(
            "[panefx] alacritty.toml has no '[window] opacity =' line; opacity not applied"
        ),
        Err(e) => panefx::log_warn!("[panefx] could not set opacity at startup: {e}"),
    }

    // Undo the previous mechanism on any window that still carries it.
    //
    // panefx used to set WS_EX_LAYERED for a WHOLE-WINDOW alpha. Removing that
    // code does not undo it on windows that are already open — the style
    // persists until they close, and it would stack with the per-pixel alpha
    // below, dimming text for reasons invisible in the source.
    term_opacity::clear_legacy_layered_styles();

    // The desktop wallpaper. Never fatal: if Explorer will not give us the
    // WorkerW layer, this is inert and the terminal backdrops carry on exactly
    // as before.
    let mut wall = wallpaper::WallpaperSet::new(&cfg);
    match &wall.error {
        Some(e) => panefx::log_warn!("[panefx] wallpaper unavailable: {e}"),
        None => panefx::log_info!(
            "[panefx] wallpaper layer attached — {} monitor(s)",
            wall.monitors().len()
        ),
    }

    // One shared simulation, sized to the largest panel seen so far.
    let mut effect_idx = 0usize;
    let mut sim: Box<dyn AsciiAnimation> =
        animation::build(&cfg.rotation[0], 0, 0, 0x5EED_1234, &cfg);
    apply_saved_params(sim.as_mut(), &cfg, animation::Scope::Pane);
    let mut last_rotate = Instant::now();

    // Live control. Absent if the port is taken — panefx still runs.
    let mut ctl = control::Server::bind(control::DEFAULT_PORT);
    let mut sim_cols = 0usize;
    let mut sim_rows = 0usize;

    // Frame-rate probe (PANEFX_FPS_LOG=1). The sleep only PADS a frame out to
    // the budget — an over-budget frame gets no sleep and no catch-up, so the
    // real rate can drop silently. Needed to tell "this change made things
    // slower" apart from "the loop is no longer throttled so it does more work
    // per second".
    let mut fps_window = Instant::now();
    let mut frames_this_sec = 0u32;

    // Set whenever something other than the sim changes what should be drawn:
    // an effect switch, a param change, a config change.
    let mut force_redraw = true;
    // Was any panel on screen last frame? Drives the freeze below, and the
    // false -> true edge has to force a redraw: a frozen sim reports no change,
    // so the first frame back would otherwise be skipped and leave a stale
    // backdrop until the effect happened to touch a cell.
    let mut was_visible = false;
    let mut last_poll = Instant::now();
    let mut frame_start;

    panefx::log_info!(
        "[panefx] running — {} fps, cell {}x{}, font {:?}, effects {:?}",
        cfg.fps, cfg.cell_w, cfg.cell_h, cfg.font, cfg.rotation
    );

    loop {
        frame_start = Instant::now();

        // --- pump our own window messages ---
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // --- tray clicks ---
        if let Some(action) = tray::take_action() {
            match action {
                tray::TrayAction::OpenTui => {
                    // Spawn the console binary; the daemon has no console of its
                    // own to host a TUI in.
                    let exe = std::env::current_exe().ok();
                    let ctl = exe
                        .as_ref()
                        .and_then(|e| e.parent())
                        .map(|d| d.join("panefx-ctl.exe"));
                    if let Some(ctl) = ctl {
                        // CREATE_NEW_CONSOLE: without it the TUI inherits the
                        // daemon's (nonexistent) console and dies immediately.
                        use std::os::windows::process::CommandExt;
                        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
                        if let Err(e) = std::process::Command::new(&ctl)
                            .creation_flags(CREATE_NEW_CONSOLE)
                            .spawn()
                        {
                            panefx::log_warn!("[panefx] could not open the TUI: {e}");
                        }
                    }
                }
                tray::TrayAction::Reload => {
                    // Re-exec ourselves and exit, which clears any accumulated
                    // bad state -- stranded panels, a dead wallpaper layer, a
                    // WorkerW that Explorer recreated.
                    panefx::log_info!("[panefx] reloading on request from the tray");
                    if let Ok(exe) = std::env::current_exe() {
                        use std::os::windows::process::CommandExt;
                        const DETACHED_PROCESS: u32 = 0x0000_0008;
                        let _ = std::process::Command::new(exe)
                            .arg("--daemon")
                            .creation_flags(DETACHED_PROCESS)
                            .spawn();
                    }
                    return Ok(());
                }
                tray::TrayAction::Exit => {
                    panefx::log_info!("[panefx] exiting on request from the tray");
                    return Ok(());
                }
            }
        }

        // --- drain IPC (non-blocking; the socket lives on its own thread) ---
        let mut needs_query = false;
        while let Some(msg) = client.try_recv() {
            match msg {
                ipc::IpcMessage::Windows(windows) => {
                    {
                        let cell = match sim.preferred_cell() {
                            Some(c) if !cfg.cell_explicit => c,
                            _ => (cfg.cell_w, cfg.cell_h),
                        };
                        reconcile(
                            &mut panels,
                            &windows,
                            &mut sim_cols,
                            &mut sim_rows,
                            &cfg,
                            cell,
                        );
                    }
                    // Occlusion needs the UNFILTERED list: `reconcile` keeps only
                    // terminals, but a browser or file manager covering the
                    // screen is exactly what should freeze the wallpaper.
                    wall.observe_windows(&windows);
                }
                ipc::IpcMessage::LayoutMayHaveChanged => {
                    // No geometry in the event — must ask.
                    needs_query = true;
                }
                ipc::IpcMessage::Closed => {
                    panefx::log_warn!("[panefx] GlazeWM closed the IPC connection; exiting.");
                    return Ok(());
                }
            }
        }

        // --- live control ---
        //
        // Drained BEFORE the re-query decision so a cell/pad/crop change can
        // set `needs_query` and resize the grid on this same frame instead of
        // waiting up to POLL_INTERVAL. The new fps also governs this frame's
        // sleep, and new render fields reach draw_animation below.
        if let Some(server) = ctl.as_mut() {
            for (peer, cmd) in server.poll() {
                force_redraw = true;
                let reply = handle_command(
                    cmd,
                    &mut cfg,
                    &mut sim,
                    &mut wall,
                    &mut effect_idx,
                    &mut last_rotate,
                    sim_cols,
                    sim_rows,
                    &mut needs_query,
                    &mut pending_opacity,
                );
                server.reply(peer, &reply);
            }
        }

        if last_poll.elapsed() >= POLL_INTERVAL {
            needs_query = true;
            last_poll = Instant::now();
            // Monitors can be plugged in, unplugged or resized while the daemon
            // runs. GlazeWM's monitor events carry no geometry, so the display
            // list is the only source of truth — same reasoning as window rects.
            wall.poll_monitors(&cfg);
        }
        if needs_query {
            client.request_windows()?;
        }

        // --- rotate effects, if configured ---
        if let Some(every) = cfg.rotate_every {
            if cfg.rotation.len() > 1 && last_rotate.elapsed() >= every {
                effect_idx = (effect_idx + 1) % cfg.rotation.len();
                let name = &cfg.rotation[effect_idx];
                // Seed varies per switch so a repeated effect does not replay
                // the identical sequence each time round the rotation.
                let seed = 0x5EED_1234u64.wrapping_add(effect_idx as u64 * 0x9E37_79B9);
                sim = animation::build(name, sim_cols, sim_rows, seed, &cfg);
                last_rotate = Instant::now();
                panefx::log_info!("[panefx] effect -> {name}");
            }
        }

        // --- advance and draw ---
        //
        // A single sim is shared by every panel (see the module header), so the
        // freeze is all-or-nothing: step it only while SOMETHING can see it.
        // With every terminal on another workspace there is no consumer at all,
        // and advancing the simulation is pure waste -- the same reasoning, and
        // the same guarantee, as the wallpaper's occlusion freeze in
        // `wallpaper.rs` (the sim is not stepped at all, rather than stepped and
        // skipped at draw time).
        //
        // This deliberately does NOT freeze while a terminal is on screen and
        // busy. The backdrop animating behind a live terminal is the feature;
        // gating on activity would mean it almost never ran.
        let any_visible = panels.values().any(|p| p.visible);
        if any_visible && !was_visible {
            force_redraw = true;
        }
        was_visible = any_visible;

        if any_visible && sim_cols > 0 && sim_rows > 0 {
            let resized = sim.dimensions() != (sim_cols, sim_rows);
            sim.resize(sim_cols, sim_rows);
            sim.step();

            // Skip the whole draw when the grid is provably unchanged. The
            // renderer — not the simulation — is the hot path, so a skipped
            // frame saves the glyph loop AND the BitBlt for every panel.
            // `rain` ticks on its own 55ms clock, so ~1 frame in 10 at 20fps is
            // identical to the last. A resize always forces a redraw, and
            // `force_redraw` covers effect switches and param changes.
            if resized || force_redraw || sim.changed() {
                force_redraw = false;
                for p in panels.values_mut() {
                    // A panel whose terminal is on another workspace is hidden;
                    // drawing into it is wasted work nobody can see.
                    if p.visible {
                        p.redraw(sim.as_ref(), &cfg);
                    }
                }
            }
        }

        // --- background opacity, debounced ---
        //
        // Written to the terminal's own config, so a settle delay keeps a fast
        // drag from rewriting the file on every keypress. Alacritty's
        // live_config_reload picks the change up in about two seconds.
        if let Some((want, since)) = pending_opacity {
            if since.elapsed() >= OPACITY_DEBOUNCE {
                pending_opacity = None;
                match term_opacity::apply(want) {
                    Ok(term_opacity::Outcome::Written) => {
                        panefx::log_info!("[panefx] background opacity -> {want}%");
                    }
                    Ok(term_opacity::Outcome::AlreadyCorrect) => {}
                    Ok(term_opacity::Outcome::KeyMissing) => {
                        panefx::log_warn!(
                            "[panefx] alacritty.toml has no '[window] opacity =' line;                              not changed"
                        );
                    }
                    Err(e) => panefx::log_warn!("[panefx] could not set opacity: {e}"),
                }
            }
        }

        // --- desktop wallpaper, on its OWN clock ---
        //
        // Called every daemon frame but rate-limited internally to
        // `wallpaper_fps`. Deliberately AFTER the terminal draw: the terminals
        // are what the user is looking at, and a wallpaper frame is never worth
        // delaying them for.
        wall.tick(&cfg, frame_start);

        frames_this_sec += 1;
        if fps_window.elapsed() >= Duration::from_secs(5) {
            if std::env::var("PANEFX_FPS_LOG").is_ok() {
                panefx::log_info!(
                    "[panefx] actual {:.1} fps (target {})",
                    frames_this_sec as f64 / fps_window.elapsed().as_secs_f64(),
                    cfg.fps
                );
            }
            frames_this_sec = 0;
            fps_window = Instant::now();
        }

        if let Some(rest) = cfg.frame_time().checked_sub(frame_start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

// `apply_saved_params` and `rebuild` now live in `animation`, because the
// wallpaper's simulation pool needs them too and neither module should own the
// other. Re-exported here under their old names so the call sites below read
// unchanged.
use animation::{apply_saved_params, rebuild};

/// Handle one control command. Returns the reply to send back.
#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: control::Command,
    cfg: &mut config::Config,
    sim: &mut Box<dyn AsciiAnimation>,
    wall: &mut wallpaper::WallpaperSet,
    effect_idx: &mut usize,
    last_rotate: &mut Instant,
    sim_cols: usize,
    sim_rows: usize,
    needs_query: &mut bool,
    pending_opacity: &mut Option<(u8, Instant)>,
) -> control::Reply {
    use control::{Command, ConfigView, Reply, Snapshot, WallpaperMonitorView};

    let snapshot = |sim: &Box<dyn AsciiAnimation>,
                    cfg: &config::Config,
                    wall: &wallpaper::WallpaperSet| Snapshot {
        effect: sim.name().to_string(),
        effects: animation::EFFECTS.iter().map(|s| s.to_string()).collect(),
        params: sim.params(),
        config: ConfigView::of(cfg),
        wallpaper: wall
            .monitors()
            .iter()
            .map(|m| WallpaperMonitorView {
                index: m.monitor.index,
                label: m.monitor.label(),
                effect: m.effect.clone(),
                occluded: m.occluded,
                geometry: m.panel.as_ref().map(|p| {
                    let (sw, sh) = p
                        .surface
                        .as_ref()
                        .map(|s| s.size())
                        .unwrap_or((-1, -1));
                    let (dw, dh, ds) = p
                        .gdi
                        .as_ref()
                        .map(|g| (g.bmp_w, g.bmp_h, g.stride))
                        .unwrap_or((-1, -1, 0));
                    let cell = wallpaper::WallpaperSet::cell_for(
                        &*animation::build(&m.effect, 1, 1, 0x5EED_1234, cfg),
                        cfg,
                    );
                    // Every pair that must agree. The DIB is allowed to be
                    // absent (-1) before the first frame is drawn; anything
                    // else disagreeing is the defect.
                    let drawn = dw > 0 && dh > 0;
                    let consistent = p.width == m.monitor.width
                        && p.height == m.monitor.height
                        && sw == m.monitor.width
                        && sh == m.monitor.height
                        && (!drawn
                            || (dw == m.monitor.width
                                && dh == m.monitor.height
                                && ds >= (m.monitor.width as usize) * 4));
                    control::MonitorGeometry {
                        monitor_w: m.monitor.width,
                        monitor_h: m.monitor.height,
                        panel_w: p.width,
                        panel_h: p.height,
                        dib_w: dw,
                        dib_h: dh,
                        dib_stride: ds,
                        surface_w: sw,
                        surface_h: sh,
                        cell_w: cell.0,
                        cell_h: cell.1,
                        consistent,
                        presents: p.surface.as_ref().map(|s| s.presents()).unwrap_or(0),
                        present_us: p.surface.as_ref().map(|s| s.present_us()).unwrap_or(0),
                        dxgi_presented: p
                            .surface
                            .as_ref()
                            .and_then(|s| s.frame_stats())
                            .map(|(c, _)| c)
                            .unwrap_or(0),
                        dxgi_refresh: p
                            .surface
                            .as_ref()
                            .and_then(|s| s.frame_stats())
                            .map(|(_, r)| r)
                            .unwrap_or(0),
                    }
                }),
            })
            .collect(),
        wallpaper_error: wall.error.clone(),
        pin_calls: panel::PIN_CALLS.load(std::sync::atomic::Ordering::Relaxed),
        // Params for every effect in use on any monitor.
        //
        // Built from a throwaway 1x1 instance rather than read off a pooled
        // sim: a monitor that was just switched on has no sim yet, and showing
        // an empty param block for an effect named in the row above would look
        // broken. The probe path is uniform.
        wallpaper_params: {
            // Keyed by MONITOR, not by effect.
            //
            // Keyed by effect, two screens running `waves` shared one entry, so
            // the TUI could only ever show -- and therefore only ever edit --
            // one set of knobs for both. Params resolve per-monitor now (the
            // shared block plus that screen's overrides), so the snapshot has
            // to carry one entry per screen or the TUI cannot show the
            // difference it is editing.
            let mut m = std::collections::BTreeMap::new();
            for surf in wall.monitors().iter().filter(|s| s.is_on()) {
                let mut probe = animation::build(&surf.effect, 1, 1, 0, cfg);
                animation::apply_saved_params(
                    probe.as_mut(),
                    cfg,
                    animation::Scope::Wallpaper(surf.monitor.index),
                );
                m.insert(surf.monitor.index.to_string(), probe.params());
            }
            m
        },
    };

    match cmd {
        Command::Get => Reply::with(snapshot(sim, cfg, wall)),

        Command::Set { key, val } => {
            if !cfg.set_field(&key, &val) {
                return Reply::err(format!("unknown or invalid config key '{key}'"));
            }
            // Grid geometry changed → re-query so panels resize now.
            if matches!(
                key.as_str(),
                "cell_w" | "cell_h" | "pad_x" | "pad_y" | "crop_top"
            ) {
                *needs_query = true;
            }
            // Opacity goes out to the terminal's OWN config, which means a
            // file write. Queue it rather than writing per keypress: holding
            // L to drag would otherwise rewrite alacritty.toml ~90 times and
            // make it re-parse each one. `needs_query` is deliberately NOT set
            // — window geometry has nothing to do with opacity.
            if key == "opacity" {
                *pending_opacity = Some((cfg.opacity, Instant::now()));
            }
            // The wallpaper grid changed. Without this the new value is stored
            // and read back correctly by `cell_for` -- but the live surfaces
            // keep the grid they were BUILT with, so the TUI reports a change
            // that never reaches the screen.
            //
            // Rebuild rather than resize in place: `SimKey` includes cols/rows,
            // so a changed grid is a different sim by construction, and
            // `rebuild_surfaces` already tears down and recreates cleanly.
            if wallpaper::changes_the_grid(&key) {
                wall.rebuild_surfaces(cfg);
                panefx::log_info!(
                    "wallpaper grid -> {}x{} px (detail {})",
                    cfg.wallpaper_cell_w,
                    cfg.wallpaper_cell_h,
                    cfg.wallpaper_detail
                );
            }
            // Rain captures frame_ms at construction, so fps needs a rebuild.
            if key == "fps" {
                let name = sim.name().to_string();
                *sim = rebuild(cfg, &name, sim_cols, sim_rows, 0x5EED_1234, animation::Scope::Pane);
            }
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::Effect { name } => {
            let wanted = name.trim().to_lowercase();
            if !animation::EFFECTS.contains(&wanted.as_str()) {
                return Reply::err(format!("unknown effect '{name}'"));
            }
            *sim = rebuild(cfg, &wanted, sim_cols, sim_rows, 0x5EED_1234, animation::Scope::Pane);
            // Effects can prefer different cell sizes, so the grid dimensions
            // may be wrong for the new one until it is recomputed.
            *needs_query = true;
            // Point the rotation at the chosen effect so it does not get
            // rotated away a moment later.
            *effect_idx = cfg.rotation.iter().position(|r| *r == wanted).unwrap_or(0);
            if !cfg.rotation.contains(&wanted) {
                cfg.rotation = vec![wanted.clone()];
                *effect_idx = 0;
            }
            *last_rotate = Instant::now();
            panefx::log_info!("[panefx] effect -> {wanted}");
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::Param { key, val } => {
            if !sim.set_param(&key, &val) {
                return Reply::err(format!("effect '{}' has no param '{key}'", sim.name()));
            }
            // Mirror into config so a later save persists it.
            let name = sim.name().to_string();
            cfg.set_effect_param(&name, &key, val.display());
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::Save => match cfg.save() {
            Ok(p) => {
                panefx::log_info!("[panefx] saved {}", p.display());
                Reply::ok()
            }
            Err(e) => Reply::err(format!("save failed: {e}")),
        },

        Command::Revert => {
            *cfg = config::Config::load();
            let name = cfg.rotation[0].clone();
            *sim = rebuild(cfg, &name, sim_cols, sim_rows, 0x5EED_1234, animation::Scope::Pane);
            *effect_idx = 0;
            *needs_query = true;
            // The wallpaper must follow the reloaded config too, or revert
            // silently half-works — which is worse than not working.
            wall.rebuild_surfaces(cfg);
            // Same for opacity: the reloaded config may carry a different one.
            *pending_opacity = Some((cfg.opacity, Instant::now()));
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::Logs { lines } => {
            // Capped so a bad client cannot ask for an unbounded response.
            Reply::with_logs(panefx::log::tail(lines.min(1000)))
        }

        Command::WallpaperParam {
            monitor,
            effect,
            key,
            val,
        } => {
            let eff = effect.trim().to_lowercase();
            if !animation::EFFECTS.contains(&eff.as_str()) {
                return Reply::err(format!("unknown effect '{effect}'"));
            }
            // Validate against a throwaway BEFORE recording, so a bad key is
            // rejected rather than persisted -- the same rule `set_field`
            // follows for every other setting.
            let mut probe = animation::build(&eff, 1, 1, 0, cfg);
            if !probe.set_param(&key, &val) {
                return Reply::err(format!("effect '{eff}' has no param '{key}'"));
            }
            match monitor {
                // One screen: an override laid over the shared block.
                Some(m) => cfg.set_wallpaper_monitor_param(m, &eff, &key, val.display()),
                // Every screen: the shared block itself. A monitor that already
                // has its own override for this key keeps it -- "set the
                // default" must not silently wipe a deliberate exception.
                None => cfg.set_wallpaper_effect_param(&eff, &key, val.display()),
            }
            // Push it to every live surface running this effect, in place, so
            // the change is visible immediately without restarting the
            // animation.
            wall.reapply_params(&eff, cfg);
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::WallpaperLayer {
            monitor,
            layer,
            name,
        } => {
            let wanted = name.trim().to_lowercase();
            // `off` is legal here and means "remove this layer", so it is
            // checked before the effect-name validation.
            if wanted != "off" && !animation::EFFECTS.contains(&wanted.as_str()) {
                return Reply::err(format!("unknown effect '{name}'"));
            }
            if monitor == 0 {
                return Reply::err("monitor 0 does not exist; DISPLAY<n> is 1-based".to_string());
            }
            cfg.set_wallpaper_layer(monitor, layer, &wanted);
            // Rebuilt rather than patched in place: the stack's LENGTH changed,
            // so the surface's sim list has to be rebuilt from the config.
            wall.rebuild_surfaces(cfg);
            Reply::with(snapshot(sim, cfg, wall))
        }

        Command::WallpaperEffect { monitor, name } => {
            let wanted = name.trim().to_lowercase();
            if wanted != wallpaper::OFF && !animation::EFFECTS.contains(&wanted.as_str()) {
                return Reply::err(format!("unknown effect '{name}' (or 'off')"));
            }
            // Apply FIRST, mirror into config only on success.
            //
            // The other order leaves a rejected value in the config — where it
            // saves to disk and comes back on the next load — which is the
            // behaviour `set_field_rejects_bad_values` already forbids for every
            // other setting.
            if let Err(e) = wall.set_effect(monitor, &wanted, cfg) {
                return Reply::err(e);
            }
            match monitor {
                Some(i) => {
                    cfg.wallpaper_effects.insert(i, wanted.clone());
                }
                None => {
                    for m in wall.monitor_indices() {
                        cfg.wallpaper_effects.insert(m, wanted.clone());
                    }
                }
            }
            Reply::with(snapshot(sim, cfg, wall))
        }
    }
}

/// Bring the panel set in line with the current window list.
fn reconcile(
    panels: &mut HashMap<isize, Panel>,
    windows: &[ipc::Window],
    sim_cols: &mut usize,
    sim_rows: &mut usize,
    cfg: &config::Config,
    cell: (i32, i32),
) {
    let (cell_w, cell_h) = cell;
    // Switched off: hide every panel and keep them hidden. Reusing the existing
    // off-screen path rather than destroying the panels, so flipping back on is
    // instant and does not rebuild a window per terminal.
    if cfg.pane_off {
        for panel in panels.values_mut() {
            panel.hide();
            let _ = panel.sink();
        }
        return;
    }
    let terminals: Vec<&ipc::Window> = windows.iter().filter(|w| w.is_target()).collect();

    // Drop panels whose terminal is gone.
    let live: std::collections::HashSet<isize> = terminals.iter().map(|w| w.handle).collect();
    panels.retain(|handle, _| live.contains(handle));
    let mut max_cols = 0usize;
    let mut max_rows = 0usize;

    for w in terminals {
        let target = HWND(w.handle as *mut _);

        let entry = panels.entry(w.handle);
        let panel = match entry {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                match Panel::create(target, w.x, w.y, w.width, w.height) {
                    Ok(p) => v.insert(p),
                    Err(e) => {
                        panefx::log_warn!("[panefx] failed to create panel for {}: {e}", w.handle);
                        continue;
                    }
                }
            }
        };

        // `is_visible()` alone is NOT enough: GlazeWM reports a MINIMIZED
        // window as displayState "shown", with the full geometry it had before
        // it was minimized. Trusting that leaves a panel sitting at a rect
        // nothing occupies -- a blank backdrop floating in the middle of the
        // screen with no window in front of it.
        //
        // The wallpaper's occlusion check already filtered on `is_minimized`
        // for exactly this reason; the panel path was missed.
        if w.is_on_screen() {
            panel.show();
            if let Err(e) = panel.reposition(w.x, w.y, w.width, w.height) {
                panefx::log_warn!("[panefx] reposition failed: {e}");
            }
            // Re-pin every time: GlazeWM reasserts z-order on focus changes.
            if let Err(e) = panel.pin_behind_target() {
                panefx::log_warn!("[panefx] z-pin failed: {e}");
            }
            // Size the grid to the DRAWABLE area, not the full window rect —
            // the renderer insets by Alacritty's padding, so a grid sized to
            // the whole window would be a couple of rows taller than what can
            // actually be shown.
            let (pad_x, pad_y) = (cfg.pad_x, cfg.pad_y);
            let usable_w = (w.width - pad_x * 2).max(cell_w);
            let usable_h = (w.height - pad_y * 2 - cfg.crop_top).max(cell_h);
            max_cols = max_cols.max((usable_w / cell_w).max(1) as usize);
            max_rows = max_rows.max((usable_h / cell_h).max(1) as usize);
        } else {
            // On another workspace, or minimized. Either way it is not on
            // screen, so hide rather than draw.
            panel.hide();
            let _ = panel.sink();
        }
    }

    *sim_cols = max_cols;
    *sim_rows = max_rows;
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn mode_of(args: &[&str]) -> &'static str {
        // Mirrors `parse_args` against an explicit list, since that reads the
        // real argv. The mapping is the contract worth pinning: which spellings
        // start a DAEMON, and which open the TUI.
        match args.first().copied() {
            None => "tui",
            Some("-d") | Some("--daemon") => "daemon",
            Some("-h") | Some("--help") => "help",
            Some(_) => "unknown",
        }
    }

    #[test]
    fn bare_panefx_opens_the_tui() {
        assert_eq!(mode_of(&[]), "tui");
    }

    #[test]
    fn the_daemon_needs_an_explicit_flag() {
        // GlazeWM launches the daemon by absolute path, and this flag is what
        // tells it apart from a person typing `panefx`. If this ever changes,
        // startup_commands on every machine has to change with it.
        assert_eq!(mode_of(&["--daemon"]), "daemon");
        assert_eq!(mode_of(&["-d"]), "daemon");
    }

    #[test]
    fn help_is_recognised_both_ways() {
        assert_eq!(mode_of(&["--help"]), "help");
        assert_eq!(mode_of(&["-h"]), "help");
    }

    #[test]
    fn an_unknown_flag_is_not_silently_treated_as_the_daemon() {
        // The dangerous failure: a typo like `--deamon` starting a SECOND
        // daemon, which then fights the first over the control port and the
        // panels. It must be rejected outright.
        assert_eq!(mode_of(&["--deamon"]), "unknown");
        assert_eq!(mode_of(&["-D"]), "unknown");
        assert_eq!(mode_of(&["daemon"]), "unknown");
    }
}
