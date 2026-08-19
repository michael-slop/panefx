# panefx-report — collect everything needed to diagnose a panefx problem.
#
# Run this, then send Michael the .txt it writes. It reads only panefx's own
# state and the display/OS facts that explain panefx failures; it changes
# nothing and starts nothing.
#
# WHY each section is here, so nobody has to guess what is safe to trim:
#
#   displays      most panefx bugs are display-shaped. The multi-monitor
#                 flicker was invisible on one screen, and a rotated or
#                 fractional-refresh output has caused real defects.
#   geometry      monitor / panel / DIB / surface are four sizes that MUST
#                 agree. `consistent=False` on any row is the bug.
#   presents      panefx presenting frames the screen never shows is a
#                 different fault from panefx not presenting at all.
#   log           the daemon has NO CONSOLE, so its log is the only place its
#                 failures are visible.
#
# Nothing here is sent anywhere. It writes one local file and stops.

$ErrorActionPreference = 'Continue'
$out = Join-Path ([Environment]::GetFolderPath('Desktop')) `
    ("panefx-report-{0}.txt" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
$lines = New-Object System.Collections.ArrayList
function W($t = '') { [void]$lines.Add($t) }

W "panefx report"
W ("generated {0}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'))
W ("=" * 70)
W

# --- what is running -------------------------------------------------------
W "-- processes --"
foreach ($n in 'panefx', 'panefx-gui', 'panefx-ctl', 'glazewm', 'alacritty') {
    $p = Get-Process -Name $n -ErrorAction SilentlyContinue
    if ($p) {
        foreach ($q in $p) {
            $age = ''
            try { $age = " up {0:N0} min" -f ((Get-Date) - $q.StartTime).TotalMinutes } catch {}
            W ("  {0,-12} pid {1,-7}{2}" -f $n, $q.Id, $age)
        }
    } else {
        W ("  {0,-12} not running" -f $n)
    }
}
W

# --- versions --------------------------------------------------------------
W "-- binaries --"
foreach ($f in 'panefx.exe', 'panefx-ctl.exe', 'panefx-gui.exe') {
    $p = Join-Path $env:USERPROFILE "bin\$f"
    if (Test-Path $p) {
        $i = Get-Item $p
        W ("  {0,-16} {1,10:N0} bytes  {2}  sha {3}" -f `
            $f, $i.Length, $i.LastWriteTime.ToString('yyyy-MM-dd HH:mm'),
            (Get-FileHash $p -Algorithm SHA256).Hash.Substring(0, 12))
    } else {
        W ("  {0,-16} not installed" -f $f)
    }
}
W

# --- the machine -----------------------------------------------------------
W "-- system --"
try {
    $os = Get-CimInstance Win32_OperatingSystem
    W ("  {0}  build {1}" -f $os.Caption, $os.BuildNumber)
} catch { W "  (could not read OS)" }
foreach ($g in (Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue)) {
    W ("  GPU: {0}  driver {1}" -f $g.Name, $g.DriverVersion)
}
W

# --- displays: the shape of most panefx bugs -------------------------------
W "-- displays --"
try {
    Add-Type @"
using System; using System.Runtime.InteropServices;
public class RptDisp {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string dmDeviceName;
    public ushort a,b,c,d; public uint e;
    public int px, py, orient, fixedOut;
    public short f,g,h,i,j;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string dmFormName;
    public ushort k; public uint bpp, w, hgt, flags, freq; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct DISPLAY_DEVICE {
    public int cb;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)]  public string name;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string str;
    public int flags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string id;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string key; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplayDevicesW(string d, uint n, ref DISPLAY_DEVICE p, uint f);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumDisplaySettingsW(string d, int m, ref DEVMODE dm);
}
"@ -ErrorAction Stop
    $i = 0
    while ($true) {
        $dd = New-Object RptDisp+DISPLAY_DEVICE
        $dd.cb = [Runtime.InteropServices.Marshal]::SizeOf($dd)
        # [NullString]::Value, not $null: PowerShell marshals $null to an empty
        # string and the call then looks for a device literally named "".
        if (-not [RptDisp]::EnumDisplayDevicesW([NullString]::Value, $i, [ref]$dd, 0)) { break }
        if ($dd.flags -band 1) {
            $dm = New-Object RptDisp+DEVMODE
            $dm.c = [uint16][Runtime.InteropServices.Marshal]::SizeOf($dm)
            [void][RptDisp]::EnumDisplaySettingsW($dd.name, -1, [ref]$dm)
            $rot = switch ($dm.orient) { 0 {'none'} 1 {'90'} 2 {'180'} 3 {'270'} default {'?'} }
            W ("  {0,-14} {1,5}x{2,-5} @{3,4}Hz  {4}bpp  rot={5,-5} at ({6},{7})  {8}" -f `
                $dd.name.Replace('\\.\',''), $dm.w, $dm.hgt, $dm.freq, $dm.bpp,
                $rot, $dm.px, $dm.py, $dd.str)
        }
        $i++
    }
} catch { W ("  (display query failed: {0})" -f $_.Exception.Message) }
W

# --- panefx's own view -----------------------------------------------------
$port = if ($env:PANEFX_PORT) { [int]$env:PANEFX_PORT } else { 6124 }
function Ask($json) {
    try {
        $c = New-Object Net.Sockets.TcpClient
        $c.Connect('127.0.0.1', $port)
        $s = $c.GetStream()
        $s.ReadTimeout = 2000
        $w = New-Object IO.StreamWriter($s); $r = New-Object IO.StreamReader($s)
        $w.WriteLine($json); $w.Flush()
        $line = $r.ReadLine()
        $c.Close()
        return $line | ConvertFrom-Json
    } catch { return $null }
}

W "-- panefx state --"
$snapReply = Ask '{"cmd":"get"}'
if (-not $snapReply) {
    W ("  could not reach the daemon on 127.0.0.1:{0}" -f $port)
    W "  (if panefx is not running, that is the answer -- start it and re-run)"
} else {
    $snap = $snapReply.snapshot
    W ("  effects available : {0}" -f ($snap.effects -join ', '))
    W ("  pane effect       : {0}" -f $snap.effect)
    W ("  wallpaper error   : '{0}'" -f $snap.wallpaper_error)
    W ("  z-order re-pins   : {0}   (a climbing number means panes fighting)" -f $snap.pin_calls)
    W ("  fps {0} / wallpaper {1} (effective {2})" -f `
        $snap.config.fps, $snap.config.wallpaper_fps, $snap.config.wallpaper_fps_effective)
    W ("  backdrops off     : {0}" -f $snap.config.pane_off)
    W
    W "  -- per monitor --"
    foreach ($m in @($snap.wallpaper)) {
        W ("    DISPLAY{0}  {1}" -f $m.index, $m.label)
        W ("      layers   : {0}" -f (($m.layers -join ' -> ') -replace '^$', '(off)'))
        W ("      occluded : {0}" -f $m.occluded)
        $g = $m.geometry
        if ($g) {
            # These four MUST agree. A False here is the bug.
            W ("      sizes    : monitor {0}x{1}  panel {2}x{3}  dib {4}x{5}  surface {6}x{7}" -f `
                $g.monitor_w, $g.monitor_h, $g.panel_w, $g.panel_h, $g.dib_w, $g.dib_h, $g.surface_w, $g.surface_h)
            W ("      CONSISTENT: {0}{1}" -f $g.consistent, $(if ($g.consistent) { '' } else { '   <<< THIS IS A BUG' }))
            # Present COUNT and cost. The DXGI counter is deliberately not
            # shown: it resets when a surface is rebuilt, so a raw 0 reads as
            # "nothing accepted" when it means "rebuilt since last count".
            W ("      presents : {0} frames, {1:N0} us each" -f `
                $g.presents, $(if ($g.presents) { $g.present_us / $g.presents } else { 0 }))
        } else {
            W "      (no surface -- this monitor is off)"
        }
    }
    W
    # Present RATE, sampled: a still wallpaper and a stalled one look the same
    # in a single reading.
    W "  -- present rate over 3s --"
    $before = @{}
    foreach ($m in @($snap.wallpaper)) { if ($m.geometry) { $before[$m.index] = $m.geometry.presents } }
    Start-Sleep -Seconds 3
    $after = Ask '{"cmd":"get"}'
    if ($after) {
        foreach ($m in @($after.snapshot.wallpaper)) {
            if ($m.geometry -and $before.ContainsKey($m.index)) {
                W ("    DISPLAY{0}: {1,6:N2} presents/sec" -f $m.index, (($m.geometry.presents - $before[$m.index]) / 3.0))
            }
        }
    }
}
W

# --- the log: the daemon has no console ------------------------------------
W "-- daemon log (newest last) --"
$logReply = Ask '{"cmd":"logs","lines":300}'
if ($logReply -and $logReply.logs) {
    $entries = @($logReply.logs.entries)
    if ($entries.Count -eq 0) { W "  (empty)" }
    $warns = 0
    foreach ($e in $entries) {
        if ($e.level -eq 'warn') { $warns++ }
        W ("  [{0}] {1}" -f $e.level, $e.text)
    }
    W
    W ("  {0} line(s), {1} warning(s)" -f $entries.Count, $warns)
} else {
    W "  (could not read the log)"
}
W

# --- config ----------------------------------------------------------------
W "-- config.toml --"
$cfg = Join-Path $env:USERPROFILE '.config\panefx\config.toml'
if (Test-Path $cfg) {
    W ("  {0}" -f $cfg)
    # -Encoding UTF8: the generated comments contain em dashes, and the
    # default encoding turns them into mojibake in the report.
    Get-Content $cfg -Encoding UTF8 | ForEach-Object { W ("  | " + $_) }
} else {
    W "  (no config file -- panefx is running on its defaults)"
}

$lines -join "`r`n" | Set-Content -Path $out -Encoding UTF8
Write-Host ""
Write-Host "  Report written to:" -ForegroundColor Green
Write-Host "    $out"
Write-Host ""
Write-Host "  Send that file to Michael. Nothing was sent anywhere by this script."
Write-Host ""
if ($Host.Name -eq 'ConsoleHost') {
    Write-Host "  press any key to close"
    [void]$Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown')
}
