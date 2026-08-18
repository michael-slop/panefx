# Building panefx

## The machine you build on is the machine it runs on

`.cargo/config.toml` sets `target-cpu=native`, which lets the `waves` grid
passes vectorise instead of compiling to scalar loops. The cost is that the
binary is **not portable**: it contains whatever instructions the build CPU
supports.

Deploying a pHub build to the laptop fails with a scheduled-task result of
`3221225501` = `0xC000001D` = **STATUS_ILLEGAL_INSTRUCTION**. The process dies
instantly, prints nothing, and leaves no log — it looks exactly like a crash on
startup.

Measured:

| machine | CPU |
|---|---|
| pHub | AMD Ryzen 7 9800X3D (Zen 5) |
| SloppyLaptopy | 13th Gen Intel Core i7-13700H (Raptor Lake) |

### Building for the other machine

Override the target CPU to a portable baseline. `x86-64-v2` (SSE4.2/POPCNT)
runs on both and still vectorises, unlike plain baseline `x86-64`:

```powershell
$env:CARGO_BUILD_RUSTFLAGS = '-C target-cpu=x86-64-v2'
cargo build --release
```

Or build on the target machine itself, which is what `native` assumes.

## Deploying to SloppyLaptopy over SSH

**An SSH session on Windows runs in session 0.** GlazeWM and Explorer run in
session 1. A process started from an SSH shell lands on a different window
station: it cannot create windows, cannot see the desktop, and panefx exits
immediately with no error.

`GetShellWindow()` returns `0` from an SSH shell — so window enumeration and
screenshots taken there prove nothing about what is on screen.

To start panefx in the interactive session, register a run-once scheduled task
with `-LogonType Interactive`, start it, then delete the task; the process it
spawned keeps running:

```powershell
$me = (whoami).Trim()   # NOT "$env:USERDOMAIN\$env:USERNAME" -- over SSH on a
                        # non-domain machine USERDOMAIN reads "WORKGROUP",
                        # which fails with "No mapping between account names
                        # and security IDs was done".
$action = New-ScheduledTaskAction -Execute 'C:\Users\micha\bin\panefx.exe' -Argument '--daemon'
$principal = New-ScheduledTaskPrincipal -UserId $me -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName 'panefx-remote-launch' -Action $action -Principal $principal -Force
Start-ScheduledTask -TaskName 'panefx-remote-launch'
```

Check `(Get-ScheduledTaskInfo -TaskName ...).LastTaskResult` when it fails —
that is where the illegal-instruction code above showed up.

## Install paths differ per machine

| machine | GlazeWM `startup_commands` runs |
|---|---|
| pHub | `~\.glzr\glazewm\scripts\panefx.exe` |
| SloppyLaptopy | `~\bin\panefx.exe` |

`build.ps1 -Install` installs to both `~\bin` and `~\.glzr\glazewm\scripts` on
the local machine and verifies the hashes. Deploying by hand to only one of them
is how a stale binary survived a day of fixes on pHub.
