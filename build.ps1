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
# Flags that are not optional:
#
#   +nightly-x86_64-pc-windows-gnu
#       stable-msvc has no link.exe on PATH on pHub. This is the same toolchain
#       the patched GlazeWM builds with.
#   --offline
#       the dependency set is already vendored; going online just adds latency
#       and a failure mode when the network is down.

param(
    [switch]$Install,
    [switch]$Test
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$toolchain = '+nightly-x86_64-pc-windows-gnu'
$binDir    = Join-Path $env:USERPROFILE 'bin'
$glzrDir   = Join-Path $env:USERPROFILE '.glzr\glazewm\scripts'

if ($Test) {
    Write-Host 'running tests ...'
    cargo $toolchain test --offline
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
cargo $toolchain build --release --offline
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
Copy-Item $exe    (Join-Path $glzrDir 'panefx.exe')    -Force
Write-Host "installed -> $binDir  (panefx, panefx-ctl)"
Write-Host "installed -> $glzrDir  (panefx)"

# Prove the copies match rather than trusting that Copy-Item did what it said.
# A stale copy is invisible until you wonder why a change did not take.
$srcHash = (Get-FileHash $exe).Hash
foreach ($t in @((Join-Path $binDir 'panefx.exe'), (Join-Path $glzrDir 'panefx.exe'))) {
    if ((Get-FileHash $t).Hash -ne $srcHash) { throw "copy mismatch: $t" }
}
Write-Host 'verified: both installed copies match the build'

Start-Process -FilePath (Join-Path $glzrDir 'panefx.exe') -WindowStyle Hidden
Start-Sleep -Seconds 3
if (Get-Process -Name panefx -ErrorAction SilentlyContinue) {
    Write-Host 'daemon restarted'
} else {
    Write-Warning 'daemon did not come up — run `panefx` by hand to see the error'
}
