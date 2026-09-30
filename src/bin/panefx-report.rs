//! Collect everything needed to diagnose a panefx problem, into one text file.
//!
//! An .exe rather than the `.ps1` it replaces, for the same reason as
//! `panefx-setup`: a script cannot be relied on to run from a double-click, and
//! every way it fails is silent. A tester whose panefx is broken should not
//! also have to fight the diagnostic tool.
//!
//! WHY each section is here, so nobody has to guess what is safe to trim:
//!
//!   displays    most panefx bugs are display-shaped. The multi-monitor
//! flicker was invisible on one screen, and a rotated or
//! fractional-refresh output has caused real defects.
//!   geometry    monitor / panel / DIB / surface are four sizes that MUST
//! agree. `consistent = false` on any row is the bug.
//!   presents    panefx presenting frames the screen never shows is a
//! different fault from panefx not presenting at all, so the
//! rate is sampled rather than read once.
//!   log the daemon has NO CONSOLE. Its log is the only place its
//! failures are visible at all.
//!
//! Nothing is sent anywhere. It writes one local file and stops.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let mut out = String::new();
    let stamp = timestamp();

    let _ = writeln!(out, "panefx report");
    let _ = writeln!(out, "generated {stamp}");
    let _ = writeln!(out, "{}", "=".repeat(70));
    let _ = writeln!(out);

    section_processes(&mut out);
    section_binaries(&mut out);
    section_system(&mut out);
    section_displays(&mut out);
    section_daemon(&mut out);
    section_config(&mut out);

    // Desktop, because that is where a non-technical tester can find it.
    let path = desktop()
 .unwrap_or_else(|| PathBuf::from("."))
 .join(format!("panefx-report-{}.txt", file_stamp()));

    println!();
    match std::fs::write(&path, out) {
 Ok(()) => {
 println!("  Report written to:");
 println!("    {}", path.display());
 println!();
 println!("  Send that file to whoever gave you panefx, or attach it to an issue at
  github.com/michael-slop/panefx/issues.");
 println!("  Nothing was sent anywhere by this program.");
 }
 Err(e) => {
 println!("  Could not write the report: {e}");
 println!("  Send whoever gave you panefx what this window says.");
 }
    }
    println!();
    pause();
}

/// A JSON value as plain text -- `"waves"` reads as `waves`.
///
/// The report is read by a person, and a wall of quoted strings is harder to
/// scan for the one line that is wrong.
fn plain(v: &serde_json::Value) -> String {
    match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    }
}

// --- sections --------------------------------------------------------------

fn section_processes(out: &mut String) {
    let _ = writeln!(out, "-- processes --");
    for n in [
 "panefx.exe",
 "panefx-gui.exe",
 "panefx-ctl.exe",
 "glazewm.exe",
 "alacritty.exe",
    ] {
 let running = tasklist_has(n);
 let _ = writeln!(
 out,
 "  {:<16} {}",
 n,
 if running { "running" } else { "not running" }
 );
    }
    let _ = writeln!(out);
}

fn section_binaries(out: &mut String) {
    let _ = writeln!(out, "-- binaries --");
    let Some(home) = home() else {
 let _ = writeln!(out, "  (no USERPROFILE)");
 return;
    };
    for f in ["panefx.exe", "panefx-gui.exe", "panefx-ctl.exe"] {
 let p = home.join("bin").join(f);
 match std::fs::metadata(&p) {
 Ok(m) => {
 let _ = writeln!(out, "  {:<16} {:>10} bytes", f, m.len());
 }
 Err(_) => {
 let _ = writeln!(out, "  {f:<16} not installed");
 }
 }
    }
    let _ = writeln!(out);
}

fn section_system(out: &mut String) {
    let _ = writeln!(out, "-- system --");
    if let Some(v) = cmd_output("cmd", &["/c", "ver"]) {
 let _ = writeln!(out, "  {}", v.trim());
    }
    // GPU name and driver: a driver version is the single most useful fact
    // when a composited layer misbehaves.
    //
    // NOT `wmic` -- it is removed on current Windows 11 (verified: the section
    // came back empty on build 26200). CIM through PowerShell is the supported
    // replacement, and this is a report, so the startup cost does not matter.
    if let Some(v) = cmd_output(
 "powershell",
 &[
 "-NoProfile",
 "-Command",
 "Get-CimInstance Win32_VideoController | ForEach-Object { \"$($_.Name)  driver $($_.DriverVersion)\" }",
 ],
    ) {
 for line in v.lines().map(str::trim).filter(|l| !l.is_empty()) {
 let _ = writeln!(out, "  GPU: {line}");
 }
    }
    let _ = writeln!(out);
}

/// Display modes, via the daemon's own monitor enumeration.
///
/// Reusing `desktop::enumerate_monitors` rather than calling EnumDisplayDevices
/// again here: if the two ever disagreed, the report would be describing a
/// machine the daemon is not running on.
/// Displays, from the daemon's own enumeration plus the mode facts it does not
/// carry.
///
/// `enumerate_monitors` is reused rather than re-implemented: if the two ever
/// disagreed, the report would be describing a machine the daemon is not
/// running on.
fn section_displays(out: &mut String) {
    let _ = writeln!(out, "-- displays --");
    let monitors = panefx::desktop::enumerate_monitors();
    if monitors.is_empty() {
        let _ = writeln!(out, "  (none enumerated -- this is itself a bug)");
    }
    for m in &monitors {
        let _ = writeln!(
            out,
            "  DISPLAY{:<3} {:>5}x{:<5} at ({},{}){}",
            m.index,
            m.width,
            m.height,
            m.x,
            m.y,
            if m.primary { "  (primary)" } else { "" }
        );
    }
    let _ = writeln!(out);

    // Refresh rate and ROTATION per display. Neither is on `MonitorInfo`, and
    // both have caused real panefx defects that were invisible on every other
    // screen. Deliberately NOT Win32_VideoController -- that reports per GPU,
    // so on a four-monitor desk it answers a different question than the one
    // asked.
    let _ = writeln!(out, "  -- mode per display (refresh / rotation) --");
    let script = "$s=@'
using System;using System.Runtime.InteropServices;
public class D{
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]public struct DM{
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=32)]public string dn;
  public ushort a,b,c,d;public uint e;public int px,py,o,f;
  public short g,h,i,j,k;
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=32)]public string fn;
  public ushort l;public uint bpp,w,ht,fl,fr;}
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]public struct DD{
  public int cb;
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=32)]public string name;
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=128)]public string str;
  public int flags;
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=128)]public string id;
  [MarshalAs(UnmanagedType.ByValTStr,SizeConst=128)]public string key;}
 [DllImport(\"user32.dll\",CharSet=CharSet.Unicode)]public static extern bool EnumDisplayDevicesW(string a,uint b,ref DD c,uint d);
 [DllImport(\"user32.dll\",CharSet=CharSet.Unicode)]public static extern bool EnumDisplaySettingsW(string a,int b,ref DM c);}
'@
Add-Type $s
$i=0
while($true){
 $dd=New-Object D+DD
 $dd.cb=[Runtime.InteropServices.Marshal]::SizeOf($dd)
 if(-not [D]::EnumDisplayDevicesW([NullString]::Value,$i,[ref]$dd,0)){break}
 if($dd.flags -band 1){
  $dm=New-Object D+DM
  $dm.c=[uint16][Runtime.InteropServices.Marshal]::SizeOf($dm)
  [void][D]::EnumDisplaySettingsW($dd.name,-1,[ref]$dm)
  $r=switch($dm.o){0{'none'}1{'90'}2{'180'}3{'270'}default{'?'}}
  '{0} {1}x{2} @{3}Hz {4}bpp rot={5} {6}' -f $dd.name.Substring(11),$dm.w,$dm.ht,$dm.fr,$dm.bpp,$r,$dd.str}
 $i++}";
    match cmd_output("powershell", &["-NoProfile", "-Command", script]) {
        Some(v) if !v.trim().is_empty() => {
            for line in v.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let _ = writeln!(out, "    {line}");
            }
        }
        _ => {
            let _ = writeln!(out, "    (could not query display modes)");
        }
    }
    let _ = writeln!(out);
}

fn section_daemon(out: &mut String) {
    let port = std::env::var("PANEFX_PORT")
 .ok()
 .and_then(|p| p.parse().ok())
 .unwrap_or(panefx::control::DEFAULT_PORT);

    let _ = writeln!(out, "-- panefx state --");
    let Ok(mut conn) = Conn::connect(port) else {
 let _ = writeln!(out, "  could not reach the daemon on 127.0.0.1:{port}");
 let _ = writeln!(
 out,
 "  (if panefx is not running, that IS the answer -- start it and re-run)"
 );
 let _ = writeln!(out);
 return;
    };

    let Some(snap) = conn.ask("get") else {
 let _ = writeln!(out, "  the daemon did not answer a 'get'.");
 let _ = writeln!(out);
 return;
    };

    let s = &snap["snapshot"];
    let _ = writeln!(out, "  pane effect     : {}", plain(&s["effect"]));
    let _ = writeln!(out, "  wallpaper error : {}", plain(&s["wallpaper_error"]));
    let _ = writeln!(
 out,
 "  z-order re-pins : {}   (a climbing number means panes fighting)",
 s["pin_calls"]
    );
    let _ = writeln!(
 out,
 "  fps {} / wallpaper {}",
 s["config"]["fps"], s["config"]["wallpaper_fps"]
    );
    let _ = writeln!(out, "  backdrops off   : {}", s["config"]["pane_off"]);
    let _ = writeln!(out);

    // Terminal backdrops. Added because "only one of my two Alacritty windows
    // has an effect" could not be investigated from outside the process: the
    // daemon knew whether it had built a panel and whether it thought the panel
    // was visible, and reported neither.
    let _ = writeln!(out, "  -- terminal backdrops (panes) --");
    match s["panes"].as_array() {
        Some(list) if !list.is_empty() => {
            for p in list {
                let _ = writeln!(
                    out,
                    "    hwnd {:<10} visible {:<6} {}x{}",
                    plain(&p["handle"]),
                    plain(&p["visible"]),
                    plain(&p["width"]),
                    plain(&p["height"])
                );
            }
            let _ = writeln!(out, "    ({} pane(s))", list.len());
        }
        _ => {
            let _ = writeln!(
                out,
                "    none -- panefx is drawing behind no terminals at all."
            );
            let _ = writeln!(
                out,
                "    (needs GlazeWM running, and a target window: alacritty or neovide)"
            );
        }
    }
    let _ = writeln!(out);

    // Per-monitor geometry. `consistent` is the one to read first.
    let _ = writeln!(out, "  -- per monitor --");
    let before = s["wallpaper"].clone();
    if let Some(list) = before.as_array() {
 for m in list {
 let _ = writeln!(out, "    DISPLAY{}  {}", plain(&m["index"]), plain(&m["label"]));
 let layers: Vec<String> = m["layers"]
 .as_array()
 .map(|a| a.iter().map(plain).collect())
 .unwrap_or_default();
 let _ = writeln!(
 out,
 "      layers    : {}",
 if layers.is_empty() {
 "(off)".to_string()
 } else {
 layers.join(" -> ")
 }
 );
 let _ = writeln!(out, "      occluded  : {}", plain(&m["occluded"]));
 let g = &m["geometry"];
 if g.is_null() {
 let _ = writeln!(out, "      (no surface -- this monitor is off)");
 continue;
 }
 let _ = writeln!(
 out,
 "      sizes     : monitor {}x{}  panel {}x{}  dib {}x{}  surface {}x{}",
 g["monitor_w"],
 g["monitor_h"],
 g["panel_w"],
 g["panel_h"],
 g["dib_w"],
 g["dib_h"],
 g["surface_w"],
 g["surface_h"]
 );
 // These four MUST agree; a false here is the bug itself.
 let ok = g["consistent"].as_bool().unwrap_or(false);
 let _ = writeln!(
 out,
 "      CONSISTENT: {}{}",
 ok,
 if ok { "" } else { "   <<< THIS IS A BUG" }
 );
 let presents = g["presents"].as_u64().unwrap_or(0);
 let us = g["present_us"].as_u64().unwrap_or(0);
 let each = if presents > 0 { us / presents } else { 0 };
 let _ = writeln!(
 out,
 "      presents  : {presents} frames, {each} us each"
 );
 }
    }

    // Present RATE, sampled: a still wallpaper and a stalled one look
    // identical in a single reading.
    let _ = writeln!(out);
    let _ = writeln!(out, "  -- present rate over 3s --");
    println!("  sampling the present rate (3 seconds)...");
    std::thread::sleep(Duration::from_secs(3));
    if let Some(after) = conn.ask("get") {
 let a = &after["snapshot"]["wallpaper"];
 if let (Some(b), Some(a)) = (before.as_array(), a.as_array()) {
 for (bm, am) in b.iter().zip(a.iter()) {
 let p0 = bm["geometry"]["presents"].as_u64();
 let p1 = am["geometry"]["presents"].as_u64();
 if let (Some(p0), Some(p1)) = (p0, p1) {
 let _ = writeln!(
 out,
 "    DISPLAY{}: {:.2} presents/sec",
                        plain(&am["index"]),
 (p1.saturating_sub(p0)) as f64 / 3.0
 );
 }
 }
 }
    }
    let _ = writeln!(out);

    // The log. The daemon has no console, so this is the only place its
    // failures show up at all.
    let _ = writeln!(out, "-- daemon log (newest last) --");
    match conn.ask_logs(300) {
 Some(entries) if !entries.is_empty() => {
 let mut warns = 0;
 for e in &entries {
 let level = e["level"].as_str().unwrap_or("?");
 if level == "warn" {
 warns += 1;
 }
 let _ = writeln!(out, "  [{level}] {}", e["text"].as_str().unwrap_or(""));
 }
 let _ = writeln!(out);
 let _ = writeln!(out, "  {} line(s), {warns} warning(s)", entries.len());
 }
 Some(_) => {
 let _ = writeln!(out, "  (empty)");
 }
 None => {
 let _ = writeln!(out, "  (could not read the log)");
 }
    }
    let _ = writeln!(out);
}

fn section_config(out: &mut String) {
    let _ = writeln!(out, "-- config.toml --");
    match panefx::config::Config::path() {
 Some(p) if p.exists() => {
 let _ = writeln!(out, "  {}", p.display());
 match std::fs::read_to_string(&p) {
 Ok(text) => {
 for line in text.lines() {
 let _ = writeln!(out, "  | {line}");
 }
 }
 Err(e) => {
 let _ = writeln!(out, "  (could not read it: {e})");
 }
 }
 }
 _ => {
 let _ = writeln!(out, "  (no config file -- panefx is on its defaults)");
 }
    }
}

// --- daemon client ---------------------------------------------------------

struct Conn {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
}

impl Conn {
    fn connect(port: u16) -> std::io::Result<Self> {
 let s = TcpStream::connect(("127.0.0.1", port))?;
 s.set_read_timeout(Some(Duration::from_secs(2)))?;
 let r = BufReader::new(s.try_clone()?);
 Ok(Conn {
 reader: r,
 stream: s,
 })
    }

    fn ask(&mut self, cmd: &str) -> Option<serde_json::Value> {
 self.raw(&serde_json::json!({ "cmd": cmd }))
    }

    fn ask_logs(&mut self, lines: usize) -> Option<Vec<serde_json::Value>> {
 let v = self.raw(&serde_json::json!({"cmd": "logs", "lines": lines}))?;
 v["logs"]["entries"].as_array().cloned()
    }

    fn raw(&mut self, v: &serde_json::Value) -> Option<serde_json::Value> {
 writeln!(self.stream, "{v}").ok()?;
 self.stream.flush().ok()?;
 let mut line = String::new();
 self.reader.read_line(&mut line).ok()?;
 serde_json::from_str(line.trim()).ok()
    }
}

// --- helpers ---------------------------------------------------------------

fn home() -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
}

fn desktop() -> Option<PathBuf> {
    // OneDrive redirection is common enough that the plain path is not safe to
    // assume; prefer whichever exists.
    let h = home()?;
    let one = h.join("OneDrive").join("Desktop");
    if one.is_dir() {
 return Some(one);
    }
    let plain = h.join("Desktop");
    if plain.is_dir() {
 return Some(plain);
    }
    Some(h)
}

fn cmd_output(exe: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(exe).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn tasklist_has(name: &str) -> bool {
    cmd_output("tasklist", &["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
 .map(|s| s.contains(name))
 .unwrap_or(false)
}

/// Local time, via cmd -- avoids a chrono dependency for one line.
fn timestamp() -> String {
    cmd_output("cmd", &["/c", "echo %DATE% %TIME%"])
 .map(|s| s.trim().to_string())
 .unwrap_or_else(|| "unknown".into())
}

/// A filename-safe stamp: `20260819-1216`.
///
/// Built from PowerShell rather than `%DATE%`, whose format follows the
/// machine's locale -- on some it contains slashes, which cannot appear in a
/// filename at all.
fn file_stamp() -> String {
    cmd_output(
        "powershell",
        &["-NoProfile", "-Command", "Get-Date -Format yyyyMMdd-HHmm"],
    )
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    .unwrap_or_else(|| "report".into())
}

fn pause() {
    println!("  Press Enter to close.");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}
