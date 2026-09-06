//! The panefx beta installer.
//!
//! A console .exe, NOT a `.ps1`. A PowerShell script cannot be relied on to run
//! from a double-click, and every way it fails is silent -- the window appears
//! and closes with no error, which is exactly what a beta tester reported.
//! Measured on the machine this was written: `.ps1` had no file association at
//! all, the account's execution policy was Undefined (so Restricted), and
//! Explorer's zip extractor stamped Mark of the Web on every extracted file.
//!
//! What it does, in order: check what this machine has, say plainly what is
//! missing and what each missing thing costs, offer to install it, copy the
//! binaries, put them on PATH, and start the daemon.
//!
//! The DECISIONS live in `panefx::setup` so they can be unit-tested. This file
//! gathers facts and performs effects.

use panefx::setup::{can_run, verdict, Check, Need};
use std::io::Write;
use std::path::{Path, PathBuf};

const FONT_FILE: &str = "BigBlueTerm437NerdFontMono-Regular.ttf";
const BINARIES: [&str; 3] = ["panefx.exe", "panefx-gui.exe", "panefx-ctl.exe"];

fn main() {
    let here = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    banner();

    let checks = run_checks();
    for c in &checks {
        println!("  {}", c.status_line());
        if !c.found && !c.why.is_empty() {
            println!("         {}", c.why);
        }
    }
    println!();
    println!("  {}", verdict(&checks));
    println!();

    if !can_run(&checks) {
        // A hard stop is rare -- only an unsupported Windows. Say why and
        // leave, rather than installing something that cannot work.
        pause("  Nothing was installed. Press Enter to close.");
        return;
    }

    // --- offer to install what is missing ---
    let wanted: Vec<Check> = checks.iter().filter(|c| c.wants_install()).cloned().collect();
    if !wanted.is_empty() {
        println!("  These can be installed for you:");
        for c in &wanted {
            match c.winget {
                Some(id) => println!("    {:<12} winget install {id}", c.name),
                None => println!("    {:<12} from the file next to this installer", c.name),
            }
        }
        println!();
        if ask("  Install them now? [Y/n] ") {
            for c in &wanted {
                install_one(c, &here);
            }
            println!();
        } else {
            println!("  Skipped. panefx will still be installed.");
            println!();
        }
    }

    // --- install panefx itself ---
    let Some(home) = home() else {
        println!("  Could not find your user folder. Nothing installed.");
        pause("  Press Enter to close.");
        return;
    };
    let dest = home.join("bin");
    println!("  Installing panefx to {}", dest.display());
    if let Err(e) = std::fs::create_dir_all(&dest) {
        println!("  Could not create that folder: {e}");
        pause("  Press Enter to close.");
        return;
    }

    // A running daemon holds its own .exe open, so a copy over it fails with a
    // sharing violation. Stop everything first.
    stop_running();

    let mut copied = 0;
    for b in BINARIES {
        let src = here.join(b);
        if !src.exists() {
            println!("    [!] missing from this folder: {b}");
            continue;
        }
        match std::fs::copy(&src, dest.join(b)) {
            Ok(_) => {
                println!("    {b}");
                copied += 1;
            }
            Err(e) => println!("    [!] could not copy {b}: {e}"),
        }
    }
    if copied == 0 {
        println!();
        println!("  No binaries were installed -- this installer must sit in the");
        println!("  same folder as panefx.exe. Extract the WHOLE zip, then run it.");
        pause("  Press Enter to close.");
        return;
    }

    add_to_path(&dest);

    // --- start it ---
    println!();
    println!("  Starting panefx...");
    start_daemon(&dest);

    println!();
    println!("  Done.");
    println!();
    println!("  Open the control panel :  click the FX icon in your system tray,");
    println!("                            or type  panefx  in a new terminal");
    println!("  Something wrong        :  run REPORT.exe and send Michael the file");
    println!("  Read this first        :  README-BETA.txt");
    println!();
    pause("  Press Enter to close.");
}

fn banner() {
    println!();
    println!("  panefx  --  beta setup");
    println!("  animated ASCII backdrops behind windows, and on the desktop");
    println!();
    println!("  Checking what this machine has...");
    println!();
}

/// Gather every fact the decisions depend on.
fn run_checks() -> Vec<Check> {
    let mut out = Vec::new();

    // Windows version. DirectComposition is the floor.
    let build = windows_build();
    let win_ok = build.map(|b| b > 0).unwrap_or(true);
    out.push(Check {
        name: "Windows 10+",
        need: Need::Required,
        why: "panefx composites the desktop layer with DirectComposition.",
        winget: None,
        url: "",
        found: win_ok,
        detail: build.map(|b| format!("build {b}")),
    });

    // GlazeWM. No longer needed for anything: panefx falls back to a native
    // Win32 window source, so the pane effects work with no window manager at
    // all. Demoted from PartLost to Optional -- reporting it as a lost feature
    // would tell a tester to go and install a whole window manager they do not
    // need.
    let glaze = find_glazewm();
    out.push(Check {
        name: "GlazeWM",
        need: Need::Optional,
        why: "Optional. panefx follows windows natively without it; GlazeWM adds workspace awareness.",
        winget: Some("glzr-io.glazewm"),
        url: "https://glzr.io/",
        found: glaze.is_some(),
        detail: glaze.as_ref().map(|p| p.display().to_string()),
    });

    // The font. GDI does NOT fail on a missing font -- it substitutes another
    // face and returns success, so every effect renders in the wrong typeface
    // with no error. `verify_font` is the daemon's OWN check, reused here so
    // the installer cannot disagree with the daemon about whether it worked.
    let want = panefx::render::FALLBACK_FONT;
    let got = panefx::render::verify_font(want);
    let font_ok = panefx::setup::font_is_real(want, got.as_deref());
    out.push(Check {
        name: "the font",
        need: Need::PartLost,
        why: "Without it Windows silently substitutes another face and every effect looks wrong.",
        winget: None,
        url: "https://www.nerdfonts.com/font-downloads",
        found: font_ok,
        detail: if font_ok {
            Some(want.to_string())
        } else {
            Some(match got {
                Some(g) => format!("missing -- Windows substitutes '{g}'"),
                None => "missing".to_string(),
            })
        },
    });

    // Alacritty: genuinely optional, and never offered for install. Installing
    // a terminal for someone who did not ask for one is presumptuous.
    let alac = find_on_path("alacritty.exe");
    out.push(Check {
        name: "Alacritty",
        need: Need::Optional,
        why: "",
        winget: Some("Alacritty.Alacritty"),
        url: "https://alacritty.org/",
        found: alac.is_some(),
        detail: match alac {
            Some(p) => Some(p.display().to_string()),
            None => Some("optional -- only for the terminal effect".into()),
        },
    });

    out
}

fn install_one(c: &Check, here: &Path) {
    if c.name == "the font" {
        install_font(here);
        return;
    }
    let Some(id) = c.winget else { return };
    println!("  Installing {} ...", c.name);
    if find_on_path("winget.exe").is_none() {
        println!("    winget is not available here.");
        println!("    Install {} by hand from {}", c.name, c.url);
        return;
    }
    let status = std::process::Command::new("winget")
        .args([
            "install",
            "--id",
            id,
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--silent",
        ])
        .status();
    match status {
        Ok(s) if s.success() => println!("    {} installed.", c.name),
        Ok(s) => {
            println!("    winget exited {}.", s.code().unwrap_or(-1));
            println!("    Install by hand from {}", c.url);
        }
        Err(e) => println!("    could not run winget: {e}"),
    }
}

/// Install the bundled font for the current user. No admin rights needed.
///
/// BOTH steps are required and each fails differently alone:
///   - the registry value is what makes GDI find it by name after a reboot;
///   - `AddFontResourceW` is what makes it work in THIS session, before one.
/// Registry only means "works after you log out"; the API only means "works
/// until you log out".
fn install_font(here: &Path) {
    let src = here.join(FONT_FILE);
    if !src.exists() {
        println!("  The font file is not next to this installer -- skipping.");
        println!("  Get it from https://www.nerdfonts.com/font-downloads");
        return;
    }
    let Some(home) = home() else { return };
    let dir = home.join("AppData\\Local\\Microsoft\\Windows\\Fonts");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        println!("  Could not create the font folder: {e}");
        return;
    }
    let target = dir.join(FONT_FILE);
    if let Err(e) = std::fs::copy(&src, &target) {
        println!("  Could not copy the font: {e}");
        return;
    }
    println!("  Installing the font ...");
    match register_font(&target) {
        Ok(()) => println!("    font installed for your account."),
        Err(e) => {
            println!("    the font file was copied, but registering it failed: {e}");
            println!(
                "    You can install it by double-clicking {}",
                target.display()
            );
        }
    }
}

#[cfg(windows)]
fn register_font(path: &Path) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Graphics::Gdi::AddFontResourceW;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let wide = HSTRING::from(path.as_os_str());
    unsafe {
        // Session: makes the font usable immediately, before any logout.
        if AddFontResourceW(PCWSTR(wide.as_ptr())) == 0 {
            return Err("AddFontResourceW found no fonts in the file".into());
        }

        // Persistence: this value is what GDI reads at logon.
        let sub = HSTRING::from("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts");
        let mut key = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if rc.is_err() {
            return Err(format!("could not open the Fonts key ({rc:?})"));
        }
        // The value NAME is the font's face name plus " (TrueType)"; the DATA
        // is the file path. Getting the name wrong means the file is present
        // and GDI still cannot find it by name.
        let name = HSTRING::from(format!("{} (TrueType)", panefx::render::FALLBACK_FONT));
        let data = HSTRING::from(path.as_os_str());
        // Bytes of the wide string INCLUDING its terminating NUL -- a REG_SZ
        // written without it is read back with trailing garbage.
        let bytes = std::slice::from_raw_parts(
            data.as_ptr() as *const u8,
            (data.len() + 1) * std::mem::size_of::<u16>(),
        );
        let rc = RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if rc.is_err() {
            return Err(format!("could not write the font value ({rc:?})"));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn register_font(_: &Path) -> Result<(), String> {
    Err("not Windows".into())
}

/// Put `dir` on the user's PATH, if it is not already there.
fn add_to_path(dir: &Path) {
    let want = dir.display().to_string();
    let current = std::env::var("PATH").unwrap_or_default();
    if current.split(';').any(|p| {
        p.trim_end_matches('\\')
            .eq_ignore_ascii_case(want.trim_end_matches('\\'))
    }) {
        println!("    already on your PATH");
        return;
    }
    // setx writes the USER environment, which persists. Deliberately not the
    // machine one: that needs admin and this installer does not.
    //
    // Read the STORED user PATH rather than the process one -- the process PATH
    // is the user's and the machine's joined together, and writing that back
    // would copy every machine entry into the user's own.
    let stored = stored_user_path().unwrap_or_default();
    let joined = if stored.is_empty() {
        want.clone()
    } else {
        format!("{};{}", stored.trim_end_matches(';'), want)
    };
    match std::process::Command::new("setx")
        .args(["PATH", &joined])
        .output()
    {
        Ok(o) if o.status.success() => {
            println!("    added to your PATH (open a NEW terminal to use it)")
        }
        _ => println!("    could not update PATH -- run panefx from this folder instead"),
    }
}

/// The user's OWN stored PATH, not the process one.
#[cfg(windows)]
fn stored_user_path() -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "PATH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "    PATH    REG_EXPAND_SZ    C:\...;C:\..."
    // splitn(3) so a path containing spaces survives intact.
    text.lines()
        .find(|l| l.trim_start().starts_with("PATH"))
        .and_then(|l| {
            l.trim()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

#[cfg(not(windows))]
fn stored_user_path() -> Option<String> {
    None
}

fn stop_running() {
    for n in ["panefx-gui.exe", "panefx-ctl.exe", "panefx.exe"] {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", n])
            .output();
    }
    std::thread::sleep(std::time::Duration::from_millis(400));
}

/// Register the logon task, then start the daemon.
///
/// # Why a scheduled task and not GlazeWM
///
/// This used to hand the daemon to GlazeWM's `shell-exec`, because a process
/// started as a child of this installer dies with it when the console closes,
/// and making the WM its parent avoided that. It worked, and it cost more than
/// it was worth:
///
///   * It required a window manager to start a program that no longer needs one.
///   * On this machine it produced two launchers, and GlazeWM's matching
///     shutdown command killed panefx BY IMAGE NAME -- every copy, including
///     ones it had not started. Seven daemons in one logon, six of them dead.
///
/// A logon task solves the original console-death problem outright: the task
/// scheduler is the parent, so nothing this installer does can take the daemon
/// with it. It is the same shape mesh.ether uses on this machine.
fn start_daemon(dest: &Path) {
    let exe = dest.join("panefx.exe");
    register_logon_task(&exe);
    spawn_detached(&exe);
    std::thread::sleep(std::time::Duration::from_secs(2));
    report_daemon();
}

/// Make panefx start at logon, with no window manager involved.
///
/// `schtasks.exe` rather than the Task Scheduler COM API: this is one command,
/// it needs no extra crate features, and it works unelevated for the current
/// user. `/F` makes re-running the installer an update rather than an error.
#[cfg(windows)]
fn register_logon_task(exe: &Path) {
    // Quoted INSIDE the /TR value: the path contains spaces on plenty of
    // machines, and schtasks parses this string itself.
    let tr = format!("\"{}\" --daemon", exe.display());
    let out = std::process::Command::new("schtasks.exe")
        .args([
            "/Create", "/TN", "panefx", "/TR", &tr, "/SC", "ONLOGON", "/RL", "LIMITED", "/F",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => println!("  logon task 'panefx' registered"),
        Ok(o) => {
            // Not fatal. The daemon still starts below; it just will not come
            // back by itself after a reboot, and saying so is better than
            // implying the install failed.
            let msg = String::from_utf8_lossy(&o.stderr);
            println!("  could not register the logon task ({}): {}", o.status, msg.trim());
            println!("  panefx will run now but will not start automatically at logon.");
        }
        Err(e) => println!("  could not run schtasks.exe: {e}"),
    }
}

#[cfg(not(windows))]
fn register_logon_task(_exe: &Path) {}

#[cfg(windows)]
fn spawn_detached(exe: &Path) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let _ = std::process::Command::new(exe)
        .arg("--daemon")
        .creation_flags(DETACHED_PROCESS)
        .spawn();
}

#[cfg(not(windows))]
fn spawn_detached(exe: &Path) {
    let _ = std::process::Command::new(exe).arg("--daemon").spawn();
}

fn report_daemon() {
    if process_running("panefx.exe") {
        println!("    running -- look for the FX icon in your system tray");
    } else {
        println!("    the daemon did not stay running.");
        println!("    Run REPORT.exe and send Michael the file it writes.");
    }
}

// --- small helpers ---------------------------------------------------------

fn home() -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
}

/// Windows build number, or `Some(0)` when this is older than Windows 10.
fn windows_build() -> Option<u32> {
    let out = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "Microsoft Windows [Version 10.0.26200.1234]"
    let start = text.find("Version ")? + "Version ".len();
    let rest = &text[start..];
    let end = rest.find(']')?;
    let mut parts = rest[..end].split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let _minor = parts.next();
    let build: u32 = parts.next()?.parse().ok()?;
    if major >= 10 {
        Some(build)
    } else {
        Some(0)
    }
}

fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    path.split(';')
        .map(|d| Path::new(d).join(exe))
        .find(|p| p.exists())
}

fn find_glazewm() -> Option<PathBuf> {
    let pf = std::env::var("ProgramFiles").unwrap_or_default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let candidates = [
        format!("{pf}\\glzr.io\\GlazeWM\\glazewm.exe"),
        format!("{pf}\\glzr.io\\GlazeWM\\cli\\glazewm.exe"),
        format!("{local}\\Programs\\glzr.io\\GlazeWM\\glazewm.exe"),
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .or_else(|| find_on_path("glazewm.exe"))
}

// `glazewm_cli` lived here. It existed only to hand the daemon to the WM's
// `shell-exec` so the WM would be its parent; the logon task does that job now
// and does not need a window manager to exist. `find_glazewm` stays, because
// the dependency report still tells you whether GlazeWM is installed.

fn process_running(name: &str) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(name))
        .unwrap_or(false)
}

fn ask(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return false;
    }
    let s = s.trim().to_lowercase();
    // Default YES: they ran an installer.
    s.is_empty() || s.starts_with('y')
}

fn pause(msg: &str) {
    println!("{msg}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}
