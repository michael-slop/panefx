# Build and install panefx.
#
#   .\build.ps1             build .\target\release\panefx.exe (dev)
#   .\build.ps1 -Install    build, then install to BOTH locations and restart
#   .\build.ps1 -Test       run the test suite only
#
# Why two install locations:
#
#   ~\bin\                        panefx.exe + panefx-ctl.exe, so you can type
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
    [switch]$Test
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
$cargoExe  = $cargo[0]
$cargoArgs = @($cargo[1..($cargo.Count - 1)])
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

Write-Host 'building release ...'
& $cargoExe @cargoArgs build --release
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$exe    = Join-Path $PSScriptRoot 'target\release\panefx.exe'
$ctlExe = Join-Path $PSScriptRoot 'target\release\panefx-ctl.exe'
Write-Host ('built: panefx {0:N0} KB, panefx-ctl {1:N0} KB' -f `
    ((Get-Item $exe).Length / 1KB), ((Get-Item $ctlExe).Length / 1KB))

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
Write-Host "installed -> $binDir  (panefx, panefx-ctl)"
Write-Host "installed -> $glzrDir  (panefx, panefx-ctl)"

# Prove the copies match rather than trusting that Copy-Item did what it said.
# A stale copy is invisible until you wonder why a change did not take.
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
Write-Host 'verified: all four installed copies match the build'

# --daemon is REQUIRED: without it panefx.exe hands off to the control TUI,
# because typing `panefx` in a terminal should open the TUI. Omit the flag here
# and the daemon never starts, which looks exactly like a build that failed.
# GlazeWM's startup_commands needs the same flag.
Start-Process -FilePath (Join-Path $glzrDir 'panefx.exe') -ArgumentList '--daemon' -WindowStyle Hidden
Start-Sleep -Seconds 3
if (Get-Process -Name panefx -ErrorAction SilentlyContinue) {
    Write-Host 'daemon restarted'
} else {
    Write-Warning 'daemon did not come up — run `panefx --daemon` by hand to see the error'
}
