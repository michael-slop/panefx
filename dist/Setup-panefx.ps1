# panefx setup — check this machine has what panefx needs, install what it
# does not, then put panefx in place.
#
# Everything it installs is named on screen with its source before anything is
# fetched, and nothing is downloaded without you saying yes. Run it again any
# time; every step is safe to repeat.
#
# WHAT PANEFX ACTUALLY NEEDS, and why each check is the one that catches it:
#
#   Windows 10+       DirectComposition. Below that the desktop wallpaper
#                     layer cannot be built at all.
#   GlazeWM           REQUIRED for the pane effects. panefx reads every window
#                     position from GlazeWM's IPC -- with no GlazeWM there is
#                     nothing behind which to draw. Desktop wallpaper effects
#                     still work without it, so this is a warning, not a stop.
#   BigBlueTerm437    The daemon asks GDI for this font BY NAME. If it is
#                     missing GDI does not fail -- it silently substitutes
#                     Arial, and every effect renders in the wrong typeface
#                     with no error anywhere. Checked with GetTextFaceW, the
#                     only call that tells the truth about substitution.
#   Alacritty         Optional. Only if you want the terminal pane effect.

param(
    # Skip the prompts and install whatever is missing.
    [switch]$Yes,
    # Check and report, change nothing.
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$dest = Join-Path $env:USERPROFILE 'bin'
$missing = @()
$warn = @()

function Head($t) {
    Write-Host ""
    Write-Host "  $t" -ForegroundColor Cyan
    Write-Host "  $('-' * $t.Length)" -ForegroundColor DarkGray
}
function Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Bad($t)  { Write-Host "  [need] $t" -ForegroundColor Yellow }
function Note($t) { Write-Host "         $t" -ForegroundColor DarkGray }

Write-Host ""
Write-Host "  panefx setup" -ForegroundColor White
Write-Host "  animated ASCII backdrops behind windows, and on the desktop" -ForegroundColor DarkGray

# --- 1. Windows version ----------------------------------------------------
Head "Windows"
$build = [Environment]::OSVersion.Version.Build
if ([Environment]::OSVersion.Version.Major -ge 10) {
    Ok "Windows build $build (DirectComposition available)"
} else {
    Bad "Windows 10 or newer is required (found build $build)"
    Note "panefx composites the desktop layer with DirectComposition."
    Write-Host ""
    Write-Host "  Cannot continue on this version of Windows." -ForegroundColor Red
    exit 1
}

# --- 2. GlazeWM ------------------------------------------------------------
# REQUIRED for pane effects: panefx has no window-tracking of its own, it reads
# GlazeWM's IPC. Desktop wallpaper effects do not need it, so a miss is a
# warning rather than a stop.
Head "GlazeWM  (required for the effects behind windows)"
$glaze = @(
    "$env:ProgramFiles\glzr.io\GlazeWM\glazewm.exe",
    "$env:ProgramFiles\glzr.io\GlazeWM\cli\glazewm.exe",
    "${env:ProgramFiles(x86)}\glzr.io\GlazeWM\glazewm.exe",
    "$env:LOCALAPPDATA\Programs\glzr.io\GlazeWM\glazewm.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $glaze) { $glaze = (Get-Command glazewm.exe -ErrorAction SilentlyContinue).Source }

if ($glaze) {
    Ok "GlazeWM found: $glaze"
    if (Get-Process glazewm -ErrorAction SilentlyContinue) {
        Ok "GlazeWM is running"
    } else {
        $warn += "GlazeWM is installed but not running -- start it, or the pane effects will do nothing."
        Note "installed but NOT RUNNING -- start it before using pane effects"
    }
} else {
    Bad "GlazeWM not found"
    Note "Without it, effects BEHIND WINDOWS do nothing at all."
    Note "Desktop wallpaper effects will still work."
    $missing += [pscustomobject]@{
        Name = 'GlazeWM'
        Id   = 'glzr-io.glazewm'
        Url  = 'https://glzr.io/'
        Why  = 'window positions; required for pane effects'
    }
}

# --- 3. Alacritty ----------------------------------------------------------
Head "Alacritty  (optional -- only for the terminal pane effect)"
$alac = (Get-Command alacritty.exe -ErrorAction SilentlyContinue).Source
if (-not $alac) {
    $alac = @(
        "$env:ProgramFiles\Alacritty\alacritty.exe",
        "$env:LOCALAPPDATA\Programs\Alacritty\alacritty.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if ($alac) {
    Ok "Alacritty found: $alac"
} else {
    Bad "Alacritty not found  (optional)"
    Note "Only needed if you want the effect behind a terminal."
    $missing += [pscustomobject]@{
        Name = 'Alacritty'
        Id   = 'Alacritty.Alacritty'
        Url  = 'https://alacritty.org/'
        Why  = 'optional: the terminal pane effect'
    }
}

# --- 4. The font -----------------------------------------------------------
# The one check nobody thinks to make. GDI does NOT fail on a missing font --
# it substitutes Arial and returns success, so every effect renders in the
# wrong typeface with no error anywhere. GetTextFaceW on a DC with the font
# selected is the only call that reports what you actually got.
Head "BigBlueTerm437 Nerd Font  (the DOS/CP437 typeface the effects are drawn in)"
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public class FontChk {
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr CreateFontW(
    int h,int w,int esc,int ori,int wt,uint it,uint un,uint so,uint cs,uint op,uint cp,uint q,uint pf,string face);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleDC(IntPtr h);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
  [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr o);
  [DllImport("gdi32.dll")] public static extern bool DeleteDC(IntPtr dc);
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern int GetTextFaceW(IntPtr dc, int n, StringBuilder s);
  public static string Resolve(string face) {
    IntPtr dc = CreateCompatibleDC(IntPtr.Zero);
    if (dc == IntPtr.Zero) return null;
    // DEFAULT_CHARSET (1), not ANSI_CHARSET: with ANSI, GDI silently hands
    // back Arial for this font and the substitution is invisible.
    IntPtr f = CreateFontW(16,0,0,0,400,0,0,0,1,4,0,5,0,face);
    IntPtr old = SelectObject(dc, f);
    StringBuilder sb = new StringBuilder(128);
    int n = GetTextFaceW(dc, 128, sb);
    SelectObject(dc, old); DeleteObject(f); DeleteDC(dc);
    return n > 0 ? sb.ToString() : null;
  }
}
"@
$want = 'BigBlueTerm437 Nerd Font Mono'
$got = [FontChk]::Resolve($want)
$fontOk = ($got -eq $want)
if ($fontOk) {
    Ok "'$want' is installed"
} else {
    Bad "'$want' is NOT installed -- Windows substitutes '$got'"
    Note "Effects would render in the wrong typeface, with no visible error."
    $ttf = Join-Path $root 'BigBlueTerm437NerdFontMono-Regular.ttf'
    if (Test-Path $ttf) {
        Note "The font ships in this folder -- setup can install it for you."
    } else {
        Note "Get it from https://www.nerdfonts.com/font-downloads (search BigBlueTerminal)."
    }
}

# --- 5. Install what is missing -------------------------------------------
if ($CheckOnly) {
    Head "check only -- nothing installed"
    exit 0
}

$winget = (Get-Command winget.exe -ErrorAction SilentlyContinue).Source

if ($missing.Count -gt 0 -or -not $fontOk) {
    Head "Missing pieces"
    foreach ($m in $missing) { Write-Host ("    {0,-12} {1}" -f $m.Name, $m.Why) }
    if (-not $fontOk) { Write-Host ("    {0,-12} {1}" -f 'the font', 'effects render in the wrong typeface without it') }
    Write-Host ""

    $doIt = $Yes
    if (-not $doIt) {
        Write-Host "  Install these now? Sources:" -ForegroundColor White
        foreach ($m in $missing) { Write-Host ("    {0}  <-  winget {1}   ({2})" -f $m.Name, $m.Id, $m.Url) -ForegroundColor DarkGray }
        if (-not $fontOk) { Write-Host "    the font  <-  the .ttf in this folder (no download)" -ForegroundColor DarkGray }
        Write-Host ""
        $a = Read-Host "  [Y] install  /  [S] skip and continue  /  [N] quit"
        if ($a -match '^[Nn]') { Write-Host "  stopped."; exit 0 }
        $doIt = ($a -notmatch '^[Ss]')
    }

    if ($doIt) {
        # The font first: it needs no network and it is the one whose absence
        # is invisible later.
        if (-not $fontOk) {
            $ttf = Join-Path $root 'BigBlueTerm437NerdFontMono-Regular.ttf'
            if (Test-Path $ttf) {
                Head "Installing the font"
                # Per-user install: no admin rights needed. The registry entry
                # is what makes GDI find it by name -- dropping the file in
                # alone is not enough.
                $fdir = Join-Path $env:LOCALAPPDATA 'Microsoft\Windows\Fonts'
                New-Item -ItemType Directory -Force -Path $fdir | Out-Null
                $target = Join-Path $fdir 'BigBlueTerm437NerdFontMono-Regular.ttf'
                Copy-Item $ttf $target -Force
                $key = 'HKCU:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts'
                New-Item -Path $key -Force | Out-Null
                Set-ItemProperty -Path $key -Name "$want (TrueType)" -Value $target
                Ok "font installed for your user account"
                Note "If effects still show the wrong typeface, sign out and back in."
            } else {
                Write-Host "  [skip] font .ttf not found beside this script" -ForegroundColor Yellow
            }
        }

        foreach ($m in $missing) {
            Head "Installing $($m.Name)"
            if (-not $winget) {
                Write-Host "  winget is not available on this machine." -ForegroundColor Yellow
                Note "Install $($m.Name) by hand from $($m.Url)"
                continue
            }
            Write-Host "  winget install --id $($m.Id)" -ForegroundColor DarkGray
            & $winget install --id $m.Id --accept-package-agreements --accept-source-agreements --silent
            if ($LASTEXITCODE -eq 0) { Ok "$($m.Name) installed" }
            else {
                Write-Host "  winget exited $LASTEXITCODE -- install by hand from $($m.Url)" -ForegroundColor Yellow
            }
        }
    }
}

# --- 6. Put panefx in place ------------------------------------------------
Head "Installing panefx"
New-Item -ItemType Directory -Force -Path $dest | Out-Null

# A running daemon holds its own .exe open, so a copy over it fails with a
# sharing violation. Stop it first; step 7 starts it again.
$wasRunning = [bool](Get-Process panefx -ErrorAction SilentlyContinue)
foreach ($n in 'panefx', 'panefx-gui', 'panefx-ctl') {
    Get-Process $n -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Milliseconds 400

$bins = 'panefx.exe', 'panefx-gui.exe', 'panefx-ctl.exe'
foreach ($b in $bins) {
    $src = Join-Path $root $b
    if (-not (Test-Path $src)) { Write-Host "  [!] missing from this folder: $b" -ForegroundColor Red; continue }
    Copy-Item $src (Join-Path $dest $b) -Force
    Ok "$b  ->  $dest"
}

# On PATH, so `panefx` works from any terminal.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$dest*") {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dest", 'User')
    Ok "added $dest to your PATH"
    Note "Open a NEW terminal before `panefx` resolves there."
} else {
    Ok "$dest is already on your PATH"
}

# --- 7. Start it -----------------------------------------------------------
Head "Starting panefx"
$daemon = Join-Path $dest 'panefx.exe'
if ($glaze) {
    # Through GlazeWM's shell-exec, not directly: a process started as a child
    # of this script can die when the script's console closes. GlazeWM owns it
    # instead.
    $cli = @(
        "$env:ProgramFiles\glzr.io\GlazeWM\cli\glazewm.exe",
        $glaze
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    & $cli command shell-exec "$daemon --daemon" 2>&1 | Out-Null
} else {
    Start-Process -FilePath $daemon -ArgumentList '--daemon' -WindowStyle Hidden
}
Start-Sleep -Seconds 2
if (Get-Process panefx -ErrorAction SilentlyContinue) {
    Ok "the panefx daemon is running (look for the FX icon in your system tray)"
} else {
    Write-Host "  [!] the daemon did not stay running." -ForegroundColor Yellow
    Note "Run panefx-report.ps1 in this folder and send Michael the .txt."
}

# --- done ------------------------------------------------------------------
Write-Host ""
Write-Host "  Done." -ForegroundColor Green
Write-Host ""
foreach ($w in $warn) { Write-Host "  ! $w" -ForegroundColor Yellow }
if ($warn) { Write-Host "" }
Write-Host "  Open the control panel :  click the FX tray icon, or run  panefx"
Write-Host "  Something wrong        :  run  panefx-report.ps1  and send Michael the .txt"
Write-Host "  Read this first        :  README-BETA.txt"
Write-Host ""
if ($Host.Name -eq 'ConsoleHost' -and -not $Yes) {
    Write-Host "  press any key to close"
    [void]$Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown')
}
