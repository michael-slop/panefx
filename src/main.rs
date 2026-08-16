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

use panefx::{animation, config, control, ipc, panel, render};

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

fn main() -> anyhow::Result<()> {
    let mut cfg = config::Config::load();

    // Font substitution is SILENT — CreateFontW succeeds and you simply get a
    // different typeface. Say so loudly rather than rendering the wrong font
    // for weeks.
    match render::verify_font(&cfg.font) {
        Some(got) if got.eq_ignore_ascii_case(&cfg.font) => {
            println!("[panefx] font OK: {got:?}");
        }
        Some(got) => {
            eprintln!(
                "[panefx] WARNING: asked for {:?} but GDI selected {:?}. \
                 Is the font installed, and is the family name exact?",
                cfg.font, got
            );
        }
        None => eprintln!("[panefx] WARNING: could not verify font {:?}", cfg.font),
    }

    panel::register_class()?;

    // The GlazeWM socket lives on its own thread; the render loop only ever
    // does a non-blocking try_recv. See `ipc::IpcThread` for why.
    let client = match ipc::IpcThread::spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[panefx] cannot reach GlazeWM at {}: {e}\n\
                 Is GlazeWM running? Panels need its IPC for window geometry.",
                ipc::IPC_URL
            );
            return Err(e);
        }
    };

    let mut panels: HashMap<isize, Panel> = HashMap::new();

    // One shared simulation, sized to the largest panel seen so far.
    let mut effect_idx = 0usize;
    let mut sim: Box<dyn AsciiAnimation> =
        animation::build(&cfg.rotation[0], 0, 0, 0x5EED_1234, &cfg);
    apply_saved_params(sim.as_mut(), &cfg);
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
    let mut last_poll = Instant::now();
    let mut frame_start;

    println!(
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
                        reconcile(&mut panels, &windows, &mut sim_cols, &mut sim_rows, &cfg, cell);
                    }
                }
                ipc::IpcMessage::LayoutMayHaveChanged => {
                    // No geometry in the event — must ask.
                    needs_query = true;
                }
                ipc::IpcMessage::Closed => {
                    eprintln!("[panefx] GlazeWM closed the IPC connection; exiting.");
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
                    &mut effect_idx,
                    &mut last_rotate,
                    sim_cols,
                    sim_rows,
                    &mut needs_query,
                );
                server.reply(peer, &reply);
            }
        }

        if last_poll.elapsed() >= POLL_INTERVAL {
            needs_query = true;
            last_poll = Instant::now();
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
                println!("[panefx] effect -> {name}");
            }
        }

        // --- advance and draw ---
        if sim_cols > 0 && sim_rows > 0 {
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

        frames_this_sec += 1;
        if fps_window.elapsed() >= Duration::from_secs(5) {
            if std::env::var("PANEFX_FPS_LOG").is_ok() {
                println!(
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

/// Apply any `[effect]` params persisted in config.toml to a freshly built
/// effect. Values are strings, so each is tried as an int, then a colour, then
/// plain text — whichever the effect accepts.
fn apply_saved_params(sim: &mut dyn AsciiAnimation, cfg: &config::Config) {
    use animation::ParamValue;
    let name = sim.name().to_lowercase();
    let Some(saved) = cfg.effect_params.get(&name) else {
        return;
    };
    // Collect first: `params()` borrows, `set_param` needs &mut.
    let saved: Vec<(String, String)> = saved.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (k, raw) in saved {
        let applied = if let Ok(n) = raw.parse::<i64>() {
            sim.set_param(&k, &ParamValue::Int { v: n })
        } else if let Some(c) = panefx::palette::Rgb::parse_hex(&raw) {
            sim.set_param(&k, &ParamValue::from_rgb(c))
        } else {
            sim.set_param(&k, &ParamValue::Text { v: raw.clone() })
        };
        if !applied {
            eprintln!("[panefx] config [{name}]: effect ignored '{k}'");
        }
    }
}

/// Rebuild the effect, preserving persisted params. Used whenever a change
/// cannot be applied in place (effect switch, or a value captured at
/// construction such as rain's frame_ms).
fn rebuild(
    cfg: &config::Config,
    name: &str,
    cols: usize,
    rows: usize,
    seed: u64,
) -> Box<dyn AsciiAnimation> {
    let mut s = animation::build(name, cols, rows, seed, cfg);
    apply_saved_params(s.as_mut(), cfg);
    s
}

/// Handle one control command. Returns the reply to send back.
#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: control::Command,
    cfg: &mut config::Config,
    sim: &mut Box<dyn AsciiAnimation>,
    effect_idx: &mut usize,
    last_rotate: &mut Instant,
    sim_cols: usize,
    sim_rows: usize,
    needs_query: &mut bool,
) -> control::Reply {
    use control::{Command, ConfigView, Reply, Snapshot};

    let snapshot = |sim: &Box<dyn AsciiAnimation>, cfg: &config::Config| Snapshot {
        effect: sim.name().to_string(),
        effects: animation::EFFECTS.iter().map(|s| s.to_string()).collect(),
        params: sim.params(),
        config: ConfigView::of(cfg),
    };

    match cmd {
        Command::Get => Reply::with(snapshot(sim, cfg)),

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
            // Rain captures frame_ms at construction, so fps needs a rebuild.
            if key == "fps" {
                let name = sim.name().to_string();
                *sim = rebuild(cfg, &name, sim_cols, sim_rows, 0x5EED_1234);
            }
            Reply::with(snapshot(sim, cfg))
        }

        Command::Effect { name } => {
            let wanted = name.trim().to_lowercase();
            if !animation::EFFECTS.contains(&wanted.as_str()) {
                return Reply::err(format!("unknown effect '{name}'"));
            }
            *sim = rebuild(cfg, &wanted, sim_cols, sim_rows, 0x5EED_1234);
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
            println!("[panefx] effect -> {wanted}");
            Reply::with(snapshot(sim, cfg))
        }

        Command::Param { key, val } => {
            if !sim.set_param(&key, &val) {
                return Reply::err(format!("effect '{}' has no param '{key}'", sim.name()));
            }
            // Mirror into config so a later save persists it.
            let name = sim.name().to_string();
            cfg.set_effect_param(&name, &key, val.display());
            Reply::with(snapshot(sim, cfg))
        }

        Command::Save => match cfg.save() {
            Ok(p) => {
                println!("[panefx] saved {}", p.display());
                Reply::ok()
            }
            Err(e) => Reply::err(format!("save failed: {e}")),
        },

        Command::Revert => {
            *cfg = config::Config::load();
            let name = cfg.rotation[0].clone();
            *sim = rebuild(cfg, &name, sim_cols, sim_rows, 0x5EED_1234);
            *effect_idx = 0;
            *needs_query = true;
            Reply::with(snapshot(sim, cfg))
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
                        eprintln!("[panefx] failed to create panel for {}: {e}", w.handle);
                        continue;
                    }
                }
            }
        };

        if w.is_visible() {
            panel.show();
            if let Err(e) = panel.reposition(w.x, w.y, w.width, w.height) {
                eprintln!("[panefx] reposition failed: {e}");
            }
            // Re-pin every time: GlazeWM reasserts z-order on focus changes.
            if let Err(e) = panel.pin_behind_target() {
                eprintln!("[panefx] z-pin failed: {e}");
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
            // Terminal is on another workspace — hide rather than draw.
            panel.hide();
            let _ = panel.sink();
        }
    }

    *sim_cols = max_cols;
    *sim_rows = max_rows;
}
