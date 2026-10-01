# Build and install panefx.
#
#   .\build.ps1             build .\target\release\panefx.exe (dev)
#   .\build.ps1 -Install    build, then install to BOTH locations and restart
#   .\build.ps1 -Test       run the test suite only
#
# Why two install locations:
#
#   ~\bin\                        panefx.exe, panefx-ctl.exe + panefx-gui.exe,
#                                 so you can type any of them
#                                 `panefx-ctl` from any shell. Assumes this dir
#                                 is on your PATH; change it if yours differs.
#   ~\.glzr\glazewm\scripts\      panefx.exe only. GlazeWM's `startup_commands`
#                                 launches it from here by absolute path, so
#                                 this copy is what actually runs day to day.
#
# Copying to only one of them is the trap this script exists to prevent: edit
# an effect, rebuild, copy to ~\bin, then wonder why the running backdrop never
# changed. It never changed because GlazeWM is still running the OTHER copy.
#
# Toolchain, which DIFFERS PER MACHINE:
#
#   pHub           nightly-x86_64-pc-windows-gnu, because stable-msvc has no
#                  link.exe on PATH there. Same toolchain the patched GlazeWM
#                  builds with.
#   SloppyLaptopy  stable-x86_64-pc-windows-msvc links fine; nightly-gnu is not
#                  installed at all.
#
# Hardcoding the nightly-gnu toolchain made this script fail outright on the
# laptop with "toolchain not installed", so it is now selected from what rustup
# actually has. Building with the default toolchain is correct wherever the
# preferred one is missing.
#
#   --offline
#       the dependency set is already cached; going online just adds latency
#       and a failure mode when the network is down. Dropped automatically if
#       the cache turns out to be cold, since a first build on a fresh machine
#       has to fetch.

param(
    [switch]$Install,
    [switch]$Test,
    # Build the beta tester zip onto the Desktop. Uses a SEPARATE target dir
    # and a portable CPU target -- see the -Beta block at the end for why.
    [switch]$Beta
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# Prefer the pinned nightly-gnu toolchain, fall back to the default. Splatted as
# an ARRAY: an empty string would reach cargo as an empty argument and be
# rejected, which reads like a cargo bug rather than a missing toolchain.
$installed = (rustup toolchain list) -join "`n"
if ($installed -match 'nightly-x86_64-pc-windows-gnu') {
    $toolchain = @('+nightly-x86_64-pc-windows-gnu')
} else {
    $toolchain = @()
    Write-Host 'nightly-x86_64-pc-windows-gnu not installed; using the default toolchain'
}

# WHICH cargo, not just which toolchain.
#
# `+toolchain` is a RUSTUP PROXY feature -- the real cargo.exe has no idea what
# it means. pHub has two cargos on PATH:
#
#   scoop\apps\rustup\current\.cargo\bin\cargo.exe   the rustup proxy, understands +toolchain
#   scoop\shims\cargo.exe                            a scoop shim, does NOT
#
# and which one wins depends on PATH order, which DIFFERS BETWEEN AN ELEVATED
# AND A NORMAL SHELL. Run from an Administrator prompt the scoop shim came
# first and the build died with
#
#     error: no such command: `+nightly-x86_64-pc-windows-gnu`
#
# after the script had already stopped the daemon -- so the wallpaper stayed
# down. The same script succeeded in a non-elevated shell minutes earlier,
# which makes this look like a broken toolchain rather than a PATH-order
# problem.
#
# `rustup run <toolchain> cargo ...` asks rustup by name and cannot be captured
# by a shim. When no preferred toolchain is installed, fall back to plain cargo.
if ($toolchain.Count -gt 0) {
    $cargo = @('rustup', 'run', 'nightly-x86_64-pc-windows-gnu', 'cargo')
} else {
    $cargo = @('cargo')
}
$cargoExe = $cargo[0]
# Select-Object -Skip, NOT $cargo[1..($cargo.Count - 1)].
#
# With a single-element $cargo -- the fallback branch, which is every machine
# WITHOUT nightly-gnu, i.e. the laptop -- that slice is $cargo[1..0], and in
# PowerShell `1..0` is a DESCENDING range @(1, 0). So it yields $null followed
# by 'cargo', and the script runs `cargo cargo build`, which fails with
#
#     error: no such command: `cargo`
#
# after the daemon has already been stopped -- the identical "and the wallpaper
# stayed down" trap the comment above describes, hiding in the other branch.
# Measured on SloppyLaptopy 2026-09-05.
$cargoArgs = @($cargo | Select-Object -Skip 1)
$binDir    = Join-Path $env:USERPROFILE 'bin'
$glzrDir   = Join-Path $env:USERPROFILE '.glzr\glazewm\scripts'

if ($Test) {
    Write-Host 'running tests ...'
    & $cargoExe @cargoArgs test
    if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
    return
}

# The running daemon holds an open handle to its own .exe, so cargo cannot
# replace it. The failure is "failed to remove file ... panefx.exe", which is
# easy to misread as a permissions problem — and if you ignore it you silently
# keep testing the OLD binary.
$wasRunning = [bool](Get-Process -Name panefx -ErrorAction SilentlyContinue)
if ($wasRunning) {
    Write-Host 'stopping running daemon (it locks the output binary) ...'
    Stop-Process -Name panefx -Force
    Start-Sleep -Milliseconds 900
}
# The GUI locks its own exe for exactly the same reason, and gives exactly the
# same misleading error. It is NOT restarted afterwards -- it is a window the
# user opened, and a build script reopening windows is surprising.
if (Get-Process -Name panefx-gui -ErrorAction SilentlyContinue) {
    Write-Host 'stopping running GUI (it locks its own binary) ...'
    Stop-Process -Name panefx-gui -Force
    Start-Sleep -Milliseconds 600
}

Write-Host 'building release ...'
& $cargoExe @cargoArgs build --release
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$exe    = Join-Path $PSScriptRoot 'target\release\panefx.exe'
$ctlExe = Join-Path $PSScriptRoot 'target\release\panefx-ctl.exe'
$guiExe = Join-Path $PSScriptRoot 'target\release\panefx-gui.exe'
Write-Host ('built: panefx {0:N0} KB, panefx-ctl {1:N0} KB, panefx-gui {2:N0} KB' -f `
    ((Get-Item $exe).Length / 1KB), ((Get-Item $ctlExe).Length / 1KB),
    ((Get-Item $guiExe).Length / 1KB))

# --- the beta tester package ----------------------------------------------
if ($Beta) {
    Write-Host ""
    Write-Host "building the beta package ..." -ForegroundColor Cyan

    # x86-64-v2, NOT the repo default of target-cpu=native.
    #
    # Measured, not assumed: a native build on this machine contains 728
    # AVX-512 instructions (vmovdqu64, vextracti32x4). Those crash any CPU
    # without AVX-512 with STATUS_ILLEGAL_INSTRUCTION and NO error message --
    # the program simply vanishes. A separate CARGO_TARGET_DIR keeps these
    # out of the normal build's cache, so a later `build.ps1 -Install` cannot
    # accidentally install the slower portable binaries here.
    $old = $env:RUSTFLAGS, $env:CARGO_TARGET_DIR
    $env:RUSTFLAGS = "-C target-cpu=x86-64-v2"
    $env:CARGO_TARGET_DIR = "target-portable"
    try {
        & $cargoExe @cargoArgs build --release
        if ($LASTEXITCODE -ne 0) { throw "portable build failed" }
    } finally {
        $env:RUSTFLAGS, $env:CARGO_TARGET_DIR = $old
    }

    # Prove it: any AVX-512 here would crash a tester's machine.
    $bad = 0
    foreach ($b in 'panefx.exe','panefx-gui.exe','panefx-ctl.exe','panefx-setup.exe','panefx-report.exe') {
        $n = (objdump -d --no-show-raw-insn "target-portable\release\$b" 2>$null |
              Select-String -Pattern 'vmovdqu64|vextracti32x4|vpternlog|%zmm' |
              Measure-Object).Count
        if ($n -gt 0) { Write-Host "  $b contains $n AVX-512 instructions" -ForegroundColor Red; $bad++ }
    }
    if ($bad) { throw "portable build is not portable -- refusing to package" }
    Write-Host "  verified: no AVX-512 in any shipped binary" -ForegroundColor Green

    $stage = Join-Path $env:TEMP 'panefx-beta-stage'
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    # These three keep their real names: they land on PATH and `panefx` is what
    # the docs tell people to type.
    foreach ($b in 'panefx.exe','panefx-gui.exe','panefx-ctl.exe') {
        Copy-Item "target-portable\release\$b" $stage -Force
    }
    # The two tester-facing ones get obvious names -- a folder of six .exe
    # files should make it clear which two you are meant to double-click.
    Copy-Item "target-portable\release\panefx-setup.exe"  "$stage\INSTALL.exe" -Force
    Copy-Item "target-portable\release\panefx-report.exe" "$stage\REPORT.exe"  -Force
    Copy-Item "dist\README-BETA.txt" $stage -Force
    Copy-Item "assets\BigBlueTerm437NerdFontMono-Regular.ttf" $stage -Force
    Copy-Item "LICENSE" $stage -Force -ErrorAction SilentlyContinue
    # The theme credits and the editor plugin travel with the zip: THEMES.md is
    # how the palettes are credited, and extras\nvim is what makes Neovim follow.
    Copy-Item "THEMES.md" $stage -Force
    Copy-Item "extras" $stage -Recurse -Force

    $zip = Join-Path ([Environment]::GetFolderPath('Desktop')) 'panefx-beta.zip'
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Compress-Archive -Path "$stage\*" -DestinationPath $zip -CompressionLevel Optimal
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue

    $i = Get-Item $zip
    Write-Host ("  {0}  ({1:N2} MB)" -f $i.FullName, ($i.Length / 1MB)) -ForegroundColor Green
}

if (-not $Install) {
    if ($wasRunning) {
        Write-Host 'NOTE: the daemon was stopped for the build and NOT restarted.'
        Write-Host '      run with -Install, or start it yourself:  panefx'
    }
    return
}

foreach ($d in @($binDir, $glzrDir)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
}

Copy-Item $exe    (Join-Path $binDir 'panefx.exe')     -Force
Copy-Item $ctlExe (Join-Path $binDir 'panefx-ctl.exe') -Force
Copy-Item $exe    (Join-Path $glzrDir 'panefx.exe')     -Force
# panefx-ctl too, NOT just the daemon. The tray's "Open panefx TUI" resolves a
# SIBLING of the running exe (see `launch_tui`), and GlazeWM starts the daemon
# from $glzrDir -- so without this the tray opens whatever stale panefx-ctl.exe
# happens to sit there. That is exactly how a morning build survived a day of
# fixes: hand-deploying to ~\panefx only, while GlazeWM relaunched from here.
Copy-Item $ctlExe (Join-Path $glzrDir 'panefx-ctl.exe') -Force
# The GUI goes to BOTH, for the same reason panefx-ctl does. "Launched by hand,
# so ~\bin is enough" was wrong and shipped a real bug: the TRAY launches it,
# the tray belongs to the daemon, and GlazeWM starts the daemon from $glzrDir.
# Clicking "Open panefx" spawned a path that did not exist and failed SILENTLY
# -- nothing happened, with no error anywhere.
#
# The daemon now falls back to ~\bin and PATH (see `find_gui`), so this copy is
# belt and braces rather than the only thing holding it up.
Copy-Item $guiExe (Join-Path $binDir 'panefx-gui.exe') -Force
Copy-Item $guiExe (Join-Path $glzrDir 'panefx-gui.exe') -Force
Write-Host "installed -> $binDir  (panefx, panefx-ctl, panefx-gui)"
Write-Host "installed -> $glzrDir  (panefx, panefx-ctl, panefx-gui)"

# Prove the copies match rather than trusting that Copy-Item did what it said.
# A stale copy is invisible until you wonder why a change did not take.
#
# Guarded, because this block sits BETWEEN stopping the old daemon and starting
# the new one: anything that throws here leaves panefx installed but NOT
# RUNNING, which looks like a failed build rather than a skipped check. Seen
# 2026-09-06 -- Get-FileHash was not on PATH in a non-interactive shell and the
# install died here with the daemon down and the binaries already correct.
# A verification step must never be the reason the thing it verifies is stopped.
if (-not (Get-Command Get-FileHash -ErrorAction SilentlyContinue)) {
    Write-Warning 'Get-FileHash unavailable; skipping the copy check (install continues)'
    $skipHashCheck = $true
}
if (-not $skipHashCheck) {
$srcHash = (Get-FileHash $exe).Hash
foreach ($t in @((Join-Path $binDir 'panefx.exe'), (Join-Path $glzrDir 'panefx.exe'))) {
    if ((Get-FileHash $t).Hash -ne $srcHash) { throw "copy mismatch: $t" }
}
# The TUI is checked too: a stale panefx-ctl is just as invisible as a stale
# daemon, and shows up as a TUI missing controls the daemon already supports.
$ctlHash = (Get-FileHash $ctlExe).Hash
foreach ($t in @((Join-Path $binDir 'panefx-ctl.exe'), (Join-Path $glzrDir 'panefx-ctl.exe'))) {
    if ((Get-FileHash $t).Hash -ne $ctlHash) { throw "copy mismatch: $t" }
}
# And the GUI, for the same reason: a stale one shows up as a window missing
# controls the daemon already supports.
$guiHash = (Get-FileHash $guiExe).Hash
foreach ($t in @((Join-Path $binDir 'panefx-gui.exe'), (Join-Path $glzrDir 'panefx-gui.exe'))) {
    if ((Get-FileHash $t).Hash -ne $guiHash) { throw "copy mismatch: $t" }
}
Write-Host 'verified: all five installed copies match the build'
}

# --daemon is REQUIRED: without it panefx.exe hands off to the control TUI,
# because typing `panefx` in a terminal should open the TUI. Omit the flag here
# and the daemon never starts, which looks exactly like a build that failed.
# GlazeWM's startup_commands needs the same flag.
#
# Through the `panefx` logon task when there is one (INSTALL.exe registers it),
# NOT Start-Process: a daemon started from this shell is this shell's child and
# inherits its environment, and one started from an agent's or an SSH session's
# shell dies with that session. The task starts it the way a logon does.
if (Get-ScheduledTask -TaskName 'panefx' -ErrorAction SilentlyContinue) {
    Start-ScheduledTask -TaskName 'panefx'
} else {
    Start-Process -FilePath (Join-Path $glzrDir 'panefx.exe') -ArgumentList '--daemon' -WindowStyle Hidden
}
Start-Sleep -Seconds 3
if (Get-Process -Name panefx -ErrorAction SilentlyContinue) {
    Write-Host 'daemon restarted'
} else {
    Write-Warning 'daemon did not come up — run `panefx --daemon` by hand to see the error'
}
