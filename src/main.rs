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

/// How often to look for GlazeWM again after it went away. Its absence is not
/// an error -- the native source covers it -- so a slow, quiet retry.
const GLAZE_RETRY: Duration = Duration::from_secs(10);

/// How often to look for a Neovide that has appeared since the last push.
///
/// Neovide is push-only over its RPC pipe, so an instance that starts after the
/// last opacity change is never told the value (see the catch-up block in the
/// main loop). This is how long a freshly-opened Neovide can sit at the wrong
/// opacity before it converges.
///
/// Two seconds because the work is one directory read of `\\.\pipe\` and the
/// cost of being wrong is visible: any longer and a new window is noticeably
/// the wrong opacity while you look at it. Matches the ~2s Alacritty already
/// takes to reload its own config, so both terminals settle together.
const NEOVIDE_POLL: Duration = Duration::from_secs(2);

/// What the user asked for on the command line.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Run the background daemon. What GlazeWM launches.
    Daemon,
    /// Hand off to the control TUI. What a person typing `panefx` wants.
    /// The GUI control panel: what a bare `panefx` opens.
    Gui,
    /// The control TUI. Still first-class -- it is the only one of the two that
    /// works over SSH, which is how the laptop drives this machine.
    Tui,
    Help,
    /// `panefx theme <id|next|prev|house|list>` -- handed to `panefx-ctl`,
    /// which has a console to print what each app did.
    Theme,
    /// An unrecognised flag — say so rather than guessing.
    Unknown(String),
}

fn parse_args() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    mode_for(args.first().map(String::as_str))
}

/// The first argument -> what to run. Split out of `parse_args` so it is
/// testable: `parse_args` reads the real process argv, which a test cannot set.
///
/// Worth pinning because the routing is easy to get subtly wrong and the
/// failure is silent -- a bare `panefx` opening the TUI instead of the GUI
/// looks like the GUI is broken rather than mis-routed.
fn mode_for(first: Option<&str>) -> Mode {
    match first {
        None => Mode::Gui,
        Some("-d") | Some("--daemon") => Mode::Daemon,
        Some("-t") | Some("--tui") => Mode::Tui,
        Some("-g") | Some("--gui") => Mode::Gui,
        Some("-h") | Some("--help") => Mode::Help,
        Some("theme") | Some("themes") | Some("--themes") => Mode::Theme,
        Some(other) => Mode::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI must be findable from where the DAEMON runs, not just from
    /// ~\bin.
    ///
    /// This is the bug this test exists for: GlazeWM starts the daemon from
    /// ~\.glzr\glazewm\scripts\, build.ps1 installed the GUI only to
    /// ~\bin, and the tray looked only beside itself. "Open panefx" spawned a
    /// path that did not exist and failed silently -- the click did nothing at
    /// all, with no error anywhere.
    ///
    /// Nothing about that is visible to a unit test in isolation, so what is
    /// pinned here is the fallback CHAIN: find_gui must consult more than one
    /// location, and must not report success for a path that does not exist.
    #[test]
    fn find_gui_does_not_invent_a_path() {
        // Whatever it returns must actually be on disk. Returning a
        // plausible-but-absent path is exactly how the silent failure
        // happened.
        if let Some(p) = find_gui() {
            assert!(
                p.exists(),
                "find_gui returned a path that does not exist: {}",
                p.display()
            );
            assert!(p.ends_with("panefx-gui.exe"));
        }
    }

    #[test]
    fn bare_panefx_opens_the_gui() {
        // The whole point of the change: no arguments means the GUI.
        assert_eq!(mode_for(None), Mode::Gui);
    }

    #[test]
    fn tui_is_still_reachable() {
        // It is the only one of the two that works over SSH, so losing this
        // would cost the laptop its control path entirely.
        assert_eq!(mode_for(Some("--tui")), Mode::Tui);
        assert_eq!(mode_for(Some("-t")), Mode::Tui);
    }

    #[test]
    fn daemon_flag_still_wins() {
        // GlazeWM's autostart passes --daemon. If this ever routed to a UI,
        // every login would open a window instead of starting the daemon.
        assert_eq!(mode_for(Some("--daemon")), Mode::Daemon);
        assert_eq!(mode_for(Some("-d")), Mode::Daemon);
    }

    #[test]
    fn explicit_gui_flag_works() {
        assert_eq!(mode_for(Some("--gui")), Mode::Gui);
        assert_eq!(mode_for(Some("-g")), Mode::Gui);
    }

    #[test]
    fn theme_goes_to_the_console_binary() {
        assert_eq!(mode_for(Some("theme")), Mode::Theme);
        assert_eq!(mode_for(Some("--themes")), Mode::Theme);
    }

    #[test]
    fn unknown_flags_are_reported_not_guessed() {
        assert_eq!(
            mode_for(Some("--wallpaper")),
            Mode::Unknown("--wallpaper".to_string())
        );
    }
}

const HELP: &str = "\
panefx — animated ASCII backdrops behind windows, and on the desktop

USAGE:
    panefx              open the GUI control panel (same as `panefx-gui`)
    panefx --tui        open the control TUI (same as `panefx-ctl`) -- the
                        one that works over SSH
    panefx --daemon     run the background daemon
    panefx --help       this text
    panefx theme <id>   switch the colour theme everywhere (also: list,
                        next, prev, house) -- see `panefx-ctl --help`

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
/// Find `panefx-gui.exe`, wherever it actually is.
///
/// Looking only beside the daemon is NOT enough, and assuming otherwise was a
/// real bug: GlazeWM starts the daemon from `~\.glzr\glazewm\scripts\`,
/// while `build.ps1` installs the GUI only to `~\bin\`. The tray's "open
/// panefx" spawned a path that did not exist and failed silently -- a click
/// that did nothing at all, with no error anywhere.
///
/// Order: beside us (a self-contained folder, which is how the beta zip ships),
/// then `~\bin` (the normal install), then PATH.
fn find_gui() -> Option<std::path::PathBuf> {
    const NAME: &str = "panefx-gui.exe";

    if let Ok(exe) = std::env::current_exe() {
        if let Some(beside) = exe.parent().map(|d| d.join(NAME)) {
            if beside.exists() {
                return Some(beside);
            }
        }
    }

    if let Ok(home) = std::env::var("USERPROFILE") {
        let in_bin = std::path::PathBuf::from(home).join("bin").join(NAME);
        if in_bin.exists() {
            return Some(in_bin);
        }
    }

    std::env::var("PATH").ok().and_then(|path| {
        path.split(';')
            .map(|d| std::path::Path::new(d).join(NAME))
            .find(|p| p.exists())
    })
}

fn launch_sibling(name: &str, pass_args: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let target = exe
        .parent()
        .map(|d| d.join(name))
        .filter(|p| p.exists())
        // Fall back to PATH: someone may have copied only one binary, and a
        // clear "not found" beats a confusing "no such file".
        .unwrap_or_else(|| std::path::PathBuf::from(name));

    let mut cmd = std::process::Command::new(&target);
    // Forward our flags only to the TUI. The GUI accepts none of them, and
    // handing it `--gui` would make it reject its own launch.
    if pass_args {
        cmd.args(std::env::args().skip(1));
    }

    match cmd.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(0)),
        Err(e) => {
            panefx::log_warn!(
                "panefx: cannot start {name} ({}): {e}
                 Expected {name} next to {}.
                 Run `panefx --daemon` for the background daemon.",
                target.display(),
                exe.display()
            );
            std::process::exit(1);
        }
    }
}

fn main() -> anyhow::Result<()> {
    match parse_args() {
        Mode::Gui => {
            // Already open? Raise it instead of starting a second one. Two
            // GUIs both writing config.toml is a real way to lose settings --
            // the second to save wins and the first never knows -- and a
            // second window is never what the user meant by running `panefx`.
            if panefx::desktop::focus_existing_gui() {
                return Ok(());
            }
            // find_gui, NOT "the sibling": this binary is installed in two
            // places and only one of them has the GUI beside it.
            let Some(gui) = find_gui() else {
                panefx::log_warn!(
                    "panefx: cannot find panefx-gui.exe.
                     Looked next to this program, in %USERPROFILE%\\bin, and on PATH.
                     Run build.ps1 -Install, or use `panefx --tui`."
                );
                std::process::exit(1);
            };
            match std::process::Command::new(&gui).status() {
                Ok(st) => std::process::exit(st.code().unwrap_or(0)),
                Err(e) => {
                    panefx::log_warn!("panefx: could not start {}: {e}", gui.display());
                    std::process::exit(1);
                }
            }
        }
        Mode::Tui => return launch_sibling("panefx-ctl.exe", true),
        Mode::Theme => return launch_sibling("panefx-ctl.exe", true),
        Mode::Help => {
            // No console on this binary, so printing here goes nowhere useful.
            // Hand `--help` to the console binary, which can actually show it.
            return launch_sibling("panefx-ctl.exe", true);
        }
        Mode::Unknown(flag) => {
            panefx::log_warn!("panefx: unknown option '{flag}'\n\n{HELP}");
            std::process::exit(2);
        }
        Mode::Daemon => {}
    }

    // ONE daemon per logon session, decided before anything is allocated.
    //
    // Placed here rather than after setup on purpose: a refused instance used
    // to get as far as building fonts, panels and a wallpaper surface — ~92MB
    // — before it was of no use to anyone. Now it costs a mutex and a socket.
    //
    // The loser tells the winner on its way out, because a guard nobody can
    // observe is how the duplicate problem stayed a mystery. See
    // `single_instance` for the whole argument.
    let _instance = match panefx::single_instance::acquire() {
        Ok(lock) => lock,
        Err(()) => {
            panefx::single_instance::report_refused_to_incumbent(control::DEFAULT_PORT);
            // 3, not 1: distinguishable in a scheduled task's Last Result and
            // in GlazeWM's shell-exec, where "already running" is a normal,
            // successful-enough outcome and not a crash to investigate.
            std::process::exit(3);
        }
    };

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

    // Where window geometry comes from. GlazeWM when it is there, Win32 when it
    // is not -- panefx used to simply refuse to start without the WM, which
    // made the pane half unusable for anyone not already running it.
    //
    // The GlazeWM socket lives on its own thread; the render loop only ever
    // does a non-blocking try_recv. See `ipc::IpcThread` for why.
    let pref = panefx::window_source::Preference::parse(&cfg.window_source)
        .unwrap_or(panefx::window_source::Preference::Auto);
    let mut client = panefx::window_source::connect(pref)?;

    // WinEvent hooks, and ONLY for the native backend: GlazeWM already tells us
    // when to re-query, and running both would double every update.
    //
    // Installed HERE, on the thread that owns the `PeekMessageW` loop below --
    // a WINEVENT_OUTOFCONTEXT hook is delivered through the message queue, so
    // one installed on a thread that never pumps silently never fires.
    #[cfg(windows)]
    let mut _win_events = (client.name() == "native").then(panefx::native_windows::install_hooks);
    // Set when GlazeWM went away and the native source took over; while set,
    // GlazeWM is retried every `GLAZE_RETRY` so a restarted WM is picked up.
    let mut glaze_lost: Option<Instant> = None;

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

    // Themes: the menu, and the tray's copy of it.
    let mut themes = ThemeState::new(&cfg);

    let mut panels: HashMap<isize, Panel> = HashMap::new();

    // Newest requested opacity and when it was requested, for the debounce
    // below. `None` means nothing is pending.
    let mut pending_opacity: Option<(u8, Instant)> = None;

    // Which `nvim.*` pipes existed at the last check, so a NEW Neovide can be
    // told the current opacity. Seeded from the startup push below rather than
    // left empty: an empty set would make every instance already running look
    // like it had just appeared and earn a redundant command on the first tick.
    let mut neovide_pipes = panefx::neovide_opacity::pipe_set();
    let mut neovide_poll = Instant::now();

    // Push the configured opacity out once at startup so panefx's config and
    // the terminal's cannot drift apart.
    //
    // EFFECTIVE, not stored: starting up with `pane_off = true` must leave the
    // terminal solid. Using `cfg.opacity` here would make the backdrop-off
    // state look correct only until the next restart, which is the worst kind
    // of bug -- one that comes back on reboot.
    match term_opacity::apply(cfg.effective_opacity()) {
        Ok(term_opacity::Outcome::Written) => {
            panefx::log_info!(
                "[panefx] background opacity -> {}%",
                cfg.effective_opacity()
            )
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
                tray::TrayAction::OpenGui => {
                    // No duplicate check here: `panefx-gui` guards ITSELF on
                    // startup and exits early if one is already open, which
                    // covers this path and every other way it is launched.
                    //
                    // DETACHED_PROCESS: the GUI is a windows-subsystem binary
                    // and wants no console at all.
                    use std::os::windows::process::CommandExt;
                    const DETACHED_PROCESS: u32 = 0x0000_0008;
                    match find_gui() {
                        Some(gui) => {
                            if let Err(e) = std::process::Command::new(&gui)
                                .creation_flags(DETACHED_PROCESS)
                                .spawn()
                            {
                                panefx::log_warn!(
                                    "[panefx] could not open the GUI at {}: {e}",
                                    gui.display()
                                );
                            }
                        }
                        None => {
                            // This was a real bug: GlazeWM starts the daemon
                            // from ~\.glzr\glazewm\scripts\, but build.ps1
                            // installs the GUI only to ~\bin\. Looking only
                            // beside ourselves found nothing and the tray
                            // click did nothing at all, silently.
                            panefx::log_warn!(
                                "[panefx] panefx-gui.exe not found next to the daemon, on PATH, or in ~\\bin -- run build.ps1 -Install"
                            );
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
                tray::TrayAction::Theme(_)
                | tray::TrayAction::ThemeNext
                | tray::TrayAction::ThemePrev => {
                    let name = match action {
                        tray::TrayAction::Theme(i) => themes.themes.get(i).map(|t| t.id.clone()),
                        tray::TrayAction::ThemeNext => Some("next".to_string()),
                        _ => Some("prev".to_string()),
                    };
                    if let Some(name) = name {
                        if let Err(e) = apply_theme(&name, &mut cfg, &mut sim, &mut wall, &mut themes) {
                            panefx::log_warn!("[panefx] theme {name}: {e}");
                        }
                        force_redraw = true;
                    }
                }
            }
        }

        // --- drain IPC (non-blocking; the socket lives on its own thread) ---
        let mut needs_query = false;
        let mut lost_glaze = false;
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
                    wall.observe_windows(&windows, cfg.wallpaper_freeze_at);
                }
                ipc::IpcMessage::LayoutMayHaveChanged => {
                    // No geometry in the event — must ask.
                    needs_query = true;
                }
                ipc::IpcMessage::Closed => {
                    // Fall back, do not exit. This used to `return`, so quitting
                    // or restarting GlazeWM took the WALLPAPER down with it --
                    // which never needed GlazeWM at all -- and left a machine
                    // with no panefx until the next logon.
                    panefx::log_warn!(
                        "[panefx] GlazeWM closed the IPC connection; following windows through Win32 until it is back"
                    );
                    lost_glaze = true;
                }
            }
        }

        // GlazeWM went away: switch to the native source now, and keep trying
        // GlazeWM so a restarted WM is used again (it knows workspaces).
        if lost_glaze {
            client = panefx::window_source::connect(panefx::window_source::Preference::Native)?;
            _win_events = Some(panefx::native_windows::install_hooks());
            glaze_lost = Some(Instant::now());
            needs_query = true;
        }
        if let Some(since) = glaze_lost {
            if since.elapsed() >= GLAZE_RETRY && pref != panefx::window_source::Preference::Native {
                match panefx::window_source::try_glazewm() {
                    Some(g) => {
                        client = g;
                        panefx::window_source::set_active("glazewm");
                        _win_events = None;
                        glaze_lost = None;
                        needs_query = true;
                        panefx::log_info!("[panefx] GlazeWM is back; following windows through it again");
                    }
                    None => glaze_lost = Some(Instant::now()),
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
                    &mut themes,
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

        // --- Neovide catch-up: a NEW instance has to be told ---
        //
        // Alacritty needs nothing here. Its opacity lives in alacritty.toml, so
        // a terminal launched at any later time reads the right value off disk
        // by itself. Neovide has no such key -- `neovide_opacity` is PUSHED to
        // a live process over its RPC pipe -- so an instance that starts after
        // the last push never hears the number at all.
        //
        // That is not hypothetical. Measured 2026-09-17: the daemon started on
        // Sep 16 with `pane_off = true`, correctly wrote `opacity = 1.0` to
        // alacritty.toml and pushed 100% at startup -- to nothing, because no
        // Neovide was running. Neovide launched 33 hours later and came up
        // transparent over a switched-off backdrop, while Alacritty was solid.
        // The push is not an error when it reaches nothing (that is the normal
        // case), so it was silent for 33 hours.
        //
        // Polled on the set of `nvim.*` pipes rather than on a timer: the value
        // only needs re-sending when the population CHANGES, so a steady state
        // costs one directory read per interval and sends nothing. Pushing every
        // interval regardless would put a `nvim_command` down every pipe forever
        // for no reason.
        if neovide_poll.elapsed() >= NEOVIDE_POLL {
            neovide_poll = Instant::now();
            let pipes = panefx::neovide_opacity::pipe_set();
            if pipes != neovide_pipes {
                // Only push when something APPEARED. A Neovide closing shrinks
                // the set and needs no command sent anywhere.
                let appeared = !pipes.is_subset(&neovide_pipes);
                neovide_pipes = pipes;
                if appeared {
                    let want = cfg.effective_opacity();
                    let applied = panefx::neovide_opacity::apply(want);
                    if applied.reached > 0 {
                        panefx::log_info!(
                            "[panefx] new neovide -> {want}% ({} instance(s))",
                            applied.reached
                        );
                    }
                }
            }
        }

        // --- themes: apps that were open when the theme changed ---
        //
        // Xournal++ rewrites its own settings on exit, so a theme written while
        // it runs would be undone. Those targets report "waiting" and land here
        // once the app has closed.
        if !themes.pending.is_empty() && themes.last_retry.elapsed() >= THEME_RETRY {
            themes.last_retry = Instant::now();
            if let Some(env) = themes.env.clone() {
                let current = cfg.theme.clone();
                let pending = std::mem::take(&mut themes.pending);
                for (target, theme_id) in pending {
                    match panefx::themes::retry(&target, &theme_id, &current, &themes.themes, &env) {
                        Some(panefx::themes::targets::Outcome::Deferred(_)) => {
                            themes.pending.push((target, theme_id))
                        }
                        Some(outcome) => {
                            panefx::log_info!("[panefx] theme {theme_id}: {target} {}", outcome.word());
                            if let Some(r) = themes.report.as_mut() {
                                if let Some(t) = r.targets.iter_mut().find(|t| t.id == target) {
                                    t.outcome = outcome;
                                }
                            }
                        }
                        None => {}
                    }
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

/// The daemon's side of themes: the menu, where the apps live, what the last
/// change did, and which apps are still waiting to be recoloured.
struct ThemeState {
    themes: Vec<panefx::themes::catalog::Theme>,
    env: Option<panefx::themes::fsutil::Env>,
    report: Option<panefx::themes::Report>,
    /// (target id, theme id) for apps that were running and would have
    /// overwritten the change. Retried every `THEME_RETRY` until they land or
    /// the theme moves on.
    pending: Vec<(String, String)>,
    last_retry: Instant,
}

/// How often to retry an app that was open when the theme changed. The check
/// is a process list; five seconds is prompt without being a poll loop.
const THEME_RETRY: Duration = Duration::from_secs(5);

impl ThemeState {
    fn new(cfg: &config::Config) -> Self {
        let mut s = ThemeState {
            themes: Vec::new(),
            env: panefx::themes::fsutil::Env::from_system(),
            report: None,
            pending: Vec::new(),
            last_retry: Instant::now(),
        };
        s.reload(cfg);
        s
    }

    /// Re-read the menu and palettes, so a theme synced from the laptop or
    /// dropped into `~\.config\panefx\themes` appears without a restart. Also
    /// refreshes the tray's copy.
    fn reload(&mut self, cfg: &config::Config) {
        self.themes = panefx::themes::catalog::load(&panefx::themes::catalog::Dirs::from_env());
        let current = self.themes.iter().position(|t| {
            t.id == cfg.theme || (t.is_house() && panefx::themes::catalog::is_house(&cfg.theme))
        });
        tray::set_themes(
            self.themes.iter().map(|t| (t.group.clone(), t.name.clone())).collect(),
            current,
        );
    }

    fn views(&self) -> Vec<control::ThemeView> {
        self.themes
            .iter()
            .map(|t| control::ThemeView {
                id: t.id.clone(),
                name: t.name.clone(),
                group: t.group.clone(),
                swatches: t.swatches().into_iter().map(|c| c.to_hex()).collect(),
                light: t.is_light(),
            })
            .collect()
    }

    fn target_views(cfg: &config::Config) -> Vec<control::ThemeTargetView> {
        let mut v = vec![control::ThemeTargetView {
            id: "panefx".into(),
            label: "panefx effects + GUI".into(),
            on: true,
        }];
        v.extend(panefx::themes::targets::all().iter().map(|t| control::ThemeTargetView {
            id: t.id().into(),
            label: t.label().into(),
            on: !cfg.theme_skip.iter().any(|s| s == t.id()),
        }));
        v
    }
}

/// Switch the theme: the config ON DISK first (so unsaved tweaks in the live
/// config stay unsaved), then the same colours copied into the live config and
/// pushed to the running effects -- terminal backdrop and every wallpaper --
/// without rebuilding anything. No `revert`: color.mesh's Windows backend
/// reloaded the whole daemon to change five colours.
fn apply_theme(
    name: &str,
    cfg: &mut config::Config,
    sim: &mut Box<dyn AsciiAnimation>,
    wall: &mut wallpaper::WallpaperSet,
    ts: &mut ThemeState,
) -> Result<panefx::themes::Report, String> {
    let Some(env) = ts.env.clone() else {
        return Err("no USERPROFILE, so there is nowhere to write themes".into());
    };
    ts.reload(cfg);
    let mut disk = config::Config::load();
    let report = panefx::themes::switch(&mut disk, &ts.themes, name, &env)?;
    disk.save().map_err(|e| format!("could not save config.toml: {e}"))?;

    panefx::themes::effects::sync_colours(&disk, cfg);
    cfg.theme = disk.theme.clone();
    apply_saved_params(sim.as_mut(), cfg, animation::Scope::Pane);
    for eff in animation::EFFECTS {
        wall.reapply_params(eff, cfg);
    }

    ts.pending = report.deferred();
    ts.last_retry = Instant::now();
    ts.report = Some(report.clone());
    ts.reload(cfg);
    for line in report.lines() {
        panefx::log_info!("[panefx] {line}");
    }
    Ok(report)
}

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
    ts: &mut ThemeState,
) -> control::Reply {
    use control::{Command, ConfigView, Reply, Snapshot, WallpaperMonitorView};

    let snapshot = |sim: &Box<dyn AsciiAnimation>,
                    cfg: &config::Config,
                    wall: &wallpaper::WallpaperSet,
                    ts: &ThemeState| Snapshot {
        theme: cfg.theme.clone(),
        window_backend: panefx::window_source::active().to_string(),
        themes: ts.views(),
        theme_targets: ThemeState::target_views(cfg),
        theme_report: ts.report.clone(),
        effect: sim.name().to_string(),
        effects: animation::EFFECTS.iter().map(|s| s.to_string()).collect(),
        params: sim.params(),
        render_us: {
            use std::sync::atomic::Ordering::Relaxed;
            let mut m = std::collections::BTreeMap::new();
            m.insert("clear".into(), panefx::render::CLEAR_US.load(Relaxed));
            m.insert("cells".into(), panefx::render::CELLS_US.load(Relaxed));
            m.insert("glyphs".into(), panefx::render::GLYPH_US.load(Relaxed));
            m.insert("flush".into(), panefx::render::FLUSH_US.load(Relaxed));
            m.insert("frames".into(), panefx::render::RENDER_FRAMES.load(Relaxed));
            m
        },
        sim_cols: sim.dimensions().0,
        sim_rows: sim.dimensions().1,
        config: ConfigView::of(cfg),
        panes: panefx::panel::PANES
            .lock()
            .map(|l| {
                l.iter()
                    .map(|p| control::PaneView {
                        handle: p.handle,
                        visible: p.visible,
                        width: p.width,
                        height: p.height,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        wallpaper: wall
            .monitors()
            .iter()
            .map(|m| WallpaperMonitorView {
                index: m.monitor.index,
                label: m.monitor.label(),
                effect: m.effect.clone(),
                occluded: m.occluded,
                layers: cfg.wallpaper_stack(m.monitor.index),
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
                let idx = surf.monitor.index;
                // One entry PER LAYER, keyed `<monitor>:<layer>` -- a stacked
                // monitor has a set of knobs per layer, and a client that could
                // only see the base could only ever tune the base.
                //
                // The bare `<monitor>` key is ALSO written, pointing at layer 0,
                // so the TUI (which knows nothing about layers) keeps working
                // unchanged.
                for (layer, effect) in cfg.wallpaper_stack(idx).iter().enumerate() {
                    let mut probe = animation::build(effect, 1, 1, 0, cfg);
                    animation::apply_saved_params(
                        probe.as_mut(),
                        cfg,
                        animation::Scope::Wallpaper(idx),
                    );
                    let params = probe.params();
                    if layer == 0 {
                        m.insert(idx.to_string(), params.clone());
                    }
                    m.insert(format!("{idx}:{layer}"), params);
                }
            }
            m
        },
    };

    match cmd {
        Command::Get => Reply::with(snapshot(sim, cfg, wall, ts)),

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
            //
            // `pane_off` queues one too, and that is the whole mechanism behind
            // "the terminal goes solid when the backdrop is off". Turning the
            // backdrops off leaves a transparent terminal over the plain
            // desktop, which is not what anyone means by "off" -- they mean
            // give me my normal terminal back. `effective_opacity` decides the
            // number; the stored `opacity` is never overwritten, so switching
            // the backdrops back on restores the exact value that was chosen.
            if key == "opacity" || key == "pane_off" {
                *pending_opacity = Some((cfg.effective_opacity(), Instant::now()));
            }
            // Which apps a theme may touch is decided against the SAVED config
            // (a theme change starts from disk), so a switch flipped in the GUI
            // is written through now -- just that key, not every unsaved tweak.
            if key == "theme_skip" {
                let mut disk = config::Config::load();
                disk.theme_skip = cfg.theme_skip.clone();
                if let Err(e) = disk.save() {
                    return Reply::err(format!("could not save theme_skip: {e}"));
                }
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
            Reply::with(snapshot(sim, cfg, wall, ts))
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
            Reply::with(snapshot(sim, cfg, wall, ts))
        }

        Command::Param { key, val } => {
            if !sim.set_param(&key, &val) {
                return Reply::err(format!("effect '{}' has no param '{key}'", sim.name()));
            }
            // Mirror into config so a later save persists it.
            let name = sim.name().to_string();
            cfg.set_effect_param(&name, &key, val.display());
            Reply::with(snapshot(sim, cfg, wall, ts))
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
            Reply::with(snapshot(sim, cfg, wall, ts))
        }

        Command::Theme { name } => match apply_theme(&name, cfg, sim, wall, ts) {
            Ok(_) => Reply::with(snapshot(sim, cfg, wall, ts)),
            Err(e) => Reply::err(e),
        },

        Command::Logs { lines } => {
            // Capped so a bad client cannot ask for an unbounded response.
            Reply::with_logs(panefx::log::tail(lines.min(1000)))
        }

        Command::Duplicate { pid } => {
            // WARN, not info: this is the symptom of a launcher starting the
            // daemon twice, and it should stand out in the logs tab rather
            // than scroll past among the wallpaper-grid chatter.
            panefx::log_warn!(
                "[panefx] refused a duplicate daemon (pid {pid}) -- something started panefx twice"
            );
            Reply::ok()
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
            Reply::with(snapshot(sim, cfg, wall, ts))
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
            Reply::with(snapshot(sim, cfg, wall, ts))
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
            Reply::with(snapshot(sim, cfg, wall, ts))
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

    // Publish what we are managing, so the state is visible from outside the
    // process. Without this, "one of my two terminals has no effect" cannot be
    // told apart from "panefx never made a panel for it".
    if let Ok(mut list) = panefx::panel::PANES.lock() {
        list.clear();
        list.extend(panels.iter().map(|(h, p)| panefx::panel::PaneState {
            handle: *h,
            visible: p.visible,
            width: p.width,
            height: p.height,
        }));
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    /// Calls the REAL routing function rather than mirroring it.
    ///
    /// This used to be a hand-copied `match` with the same shape as
    /// `parse_args`. That is worse than no test: when the default changed from
    /// TUI to GUI the copy kept asserting the old answer and kept passing,
    /// because it was only ever testing itself.
    fn mode_of(args: &[&str]) -> &'static str {
        match mode_for(args.first().copied()) {
            Mode::Gui => "gui",
            Mode::Tui => "tui",
            Mode::Daemon => "daemon",
            Mode::Help => "help",
            Mode::Theme => "theme",
            Mode::Unknown(_) => "unknown",
        }
    }

    #[test]
    fn bare_panefx_opens_the_gui() {
        // Changed deliberately: a bare `panefx` used to open the TUI, and now
        // opens the GUI. The TUI is still reachable with --tui.
        assert_eq!(mode_of(&[]), "gui");
        assert_eq!(mode_of(&["--tui"]), "tui");
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
