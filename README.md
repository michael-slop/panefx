# panefx

Animated ASCII backdrops that sit **behind** your terminal windows and follow
them around a tiling window manager.

Alacritty at 60% opacity, a panel pinned directly behind it, and a live TUI to
switch effects and tune them while you watch.

> ### ⚠️ This is a personal tool, published as-is
>
> It is written for **one machine**: Windows 11, [GlazeWM](https://github.com/glzr-io/glazewm),
> and [Alacritty](https://alacritty.org/). It depends on GlazeWM's IPC for every
> window position, so **without GlazeWM running it does nothing at all.**
>
> There is no installer, no configuration wizard, and the build sets
> `target-cpu=native` — the binary is not portable to another CPU. If you want to
> use it, expect to read the source and adjust paths.

---

## What it actually is

The interesting part is not the ASCII. It is the **follower**: a borderless,
never-focusable window that pins itself directly behind another window in the
z-order and tracks it through every retile, workspace switch and focus change a
tiling WM throws at it.

That machinery does not care what gets drawn into it. ASCII effects are the
current subsystem, not the definition — image playback, shaders or live system
stats would slot into the same trait.

Two problems make this harder than it sounds, both solved here:

* **GlazeWM emits no move or resize event.** Nothing in its IPC announces new
  geometry. Events are only a hint that *something* changed; the actual rect
  always comes from a fresh `query windows`.
* **The WM actively fights you for z-order.** GlazeWM reasserts window ordering
  on every focus change. A panel has to re-pin itself behind its terminal
  continuously, forever.

---

## The effects

| effect | what it is | source |
|---|---|---|
| `waves` | Procedural black-water field. Six octaves of ridged, sheared value noise, deformed in place by a travelling swirl. | ported from my own `blackwaves.py` |
| `rain` | CP437 matrix rain — deliberately **not** katakana, because BigBlueTerm is a DOS font and "ASCII rain" ought to be ASCII. | ported from michaelslop.org's boot screen |
| `flames` | Sparse-seeded integer fire that sits as a band along the bottom. | port of [msimpson's gist](https://gist.github.com/msimpson/1096950) |
| `fire` | The classic heat-dissipation fire, full height. | port of [mhearse/asciifire](https://github.com/mhearse/asciifire) |

---

## Usage

```powershell
panefx            # open the control TUI
panefx --daemon   # run the daemon by hand (normally GlazeWM's job)
panefx --help     # keys and usage

panefx-ctl        # the TUI directly — what `panefx` hands off to
```

Typing `panefx` gives you the TUI, because that is what you actually want from a
terminal. The daemon needs `--daemon`, and that is what GlazeWM's
`startup_commands` passes — **drop the flag there and the backdrops silently
stop appearing at startup.**

It autostarts with GlazeWM, so normally you never launch the daemon yourself.

### The TUI

Two tabs: **Effects** (the running backdrop) and **Wallpaper** (the desktop).

| key | does |
|---|---|
| `Tab` | switch tab (`w` / `e` jump straight to one) |
| `↑` `↓` | move between rows |
| `←` `→` | adjust (`H` / `L` for ×10) |
| `Enter` | type a value directly |
| `a` | *(Wallpaper tab)* apply this effect to every monitor |
| `s` | save to `~\.config\panefx\config.toml` |
| `r` | revert to the saved config |
| `q` | quit — **without saving** |

Changes apply to the running panels **instantly**. Nothing persists until `s`,
so experiment freely and quit to throw it away.

The row list is built from whatever the effect declares, so a new effect's knobs
appear automatically.

### Desktop wallpaper

The same effects can run on the desktop itself, behind the icons — one
independent effect per monitor, or `off` to leave that screen alone. A monitor
that is fully covered stops being simulated entirely rather than animating where
nobody can see it, and shows as `❄ frozen` in the TUI.

Each monitor animates **independently**, seeded from its display index. Two
screens of the same size running the same effect would otherwise share one
simulation and show the identical frame at the identical instant, which looks
like a mirror rather than two wallpapers.

> **This works on Windows 11 25H2 (build 26200)** — measured on a four-monitor
> desk. An earlier note here said it did not, on the basis that the undocumented
> `0x052C` message is a no-op on that build. That much is true, but it is not
> the only way in: the desktop surface is created as a `WS_CHILD` of Explorer's
> own icon host and presented through DirectComposition, which needs no spawned
> WorkerW. See `compositor.rs` for why GDI alone cannot present there.

### Control protocol

The daemon listens on `127.0.0.1:6124`, newline-delimited JSON:

```
{"cmd":"get"}
{"cmd":"effect","name":"waves"}
{"cmd":"set","key":"fps","val":20}
{"cmd":"param","key":"darkcut","val":{"kind":"int","v":300}}
{"cmd":"wallpaper_effect","monitor":3,"name":"rain"}   // one monitor
{"cmd":"wallpaper_effect","name":"off"}                // every monitor
{"cmd":"save"}   {"cmd":"revert"}
```

Binding is best-effort — if the port is taken, panefx runs without live control
rather than refusing to start.

### Environment

`PANEFX_EFFECT`, `PANEFX_ROTATION` (comma list) + `PANEFX_ROTATE_SECS`,
`PANEFX_FPS`, `PANEFX_CELL_W` / `_H`, `PANEFX_FONT`, `PANEFX_CHARS`,
`PANEFX_PAD_X` / `_Y`, `PANEFX_CROP_TOP`, `PANEFX_WALLPAPER_FPS`,
`PANEFX_WALLPAPER_CELL_W` / `_H`. Env wins over the config file.

---

## Building

```powershell
.\build.ps1 -Install   # build, install both copies, verify, restart the daemon
.\build.ps1            # dev build
.\build.ps1 -Test      # tests
```

Use `-Install`. panefx lands in **two** places — `~\bin\` (on PATH) and
`~\.glzr\glazewm\scripts\` (what GlazeWM actually launches) — and copying to
only one produces a genuinely confusing bug: the build succeeds, the code is
correct, and the screen does not change, because the WM is still running the
other copy.

Requires the `nightly-x86_64-pc-windows-gnu` toolchain (stable-msvc has no
linker on PATH on my machine).

---

## Performance

Steady state at 10fps, one panel:

| effect | before optimisation | after |
|---|---|---|
| `flames` | 16.9% of a core | ~2.8% |
| `rain` | 14.8% | ~4.0% |
| `waves` | 88.7% | ~14% |
| `fire` | 30.1% | ~5.5% |

~11 MB working set, flat. On the machine this was built for that is 0.38% of
total CPU.

**The renderer was the bottleneck, not the effect maths** — the wave simulation
costs 1% of a core; the other ~72% was GDI. The fix was drawing one
`ExtTextOutW` per row per colour instead of one per run of identical cells,
which took `waves` from 12,242 draw calls per frame to a few hundred.

---

## Notes for anyone reading the source

Some things in here look wrong and are not:

* **`ANSI_CHARSET` silently substitutes Arial** for BigBlueTerm437. No error —
  `CreateFontW` succeeds and you get the wrong typeface. Only `GetTextFaceW` on
  the DC reveals it; checking that the family resolves in .NET does not.
  `DEFAULT_CHARSET` is mandatory, and the daemon verifies its font at startup.
* **Alacritty's Win32 class name is `"Window Class"`** — winit's generic
  default, not `"Alacritty"`. Matching on it would catch other winit apps, so
  panefx matches on process name.
* **`flames` is a band along the bottom on purpose.** There is a test asserting
  it, so nobody "fixes" it into a full-height fire.
* **`rain`'s 55ms tick is deliberate**, quoting the original: *"a CRT reads
  better stepped than smooth."* Do not smooth it out.
* **`waves` uses `darkcut`**, which has no equivalent in the Python original.
  Without it every cell is a drawn glyph — free when rasterising to a PNG, but
  here every lit cell is a GDI call.
* **GlazeWM reports minimized windows as `displayState: "shown"`**, carrying the
  full rect they had before being minimized. `displayState` answers "is this on
  the active workspace", not "is this on screen". The wallpaper's occlusion check
  filters on `state.type`; without that it would freeze behind a window sitting
  in the taskbar, forever, with nothing visibly covering it.
* **Occlusion is a scanline union, not a bounding box.** Two windows in opposite
  corners have a bounding box covering the whole screen, so `UnionRect` would
  freeze a visibly half-empty desktop.
* **The wallpaper has its own cell size and ignores `cell_w`/`cell_h`.** The
  terminal cell exists so glyphs line up with terminal text; a wallpaper has no
  text to line up with. Inheriting a 10×15 terminal cell renders a 1440×2560
  portrait at 24,480 cells instead of 10,656.
* **A `WS_POPUP` with a parent is an *owned* window, not a child** — it would
  float above every application instead of sitting behind the desktop icons.
  Desktop surfaces are `WS_CHILD`.

---

## AI disclosure

**This project was written almost entirely by Claude (Anthropic), working from
my direction, in an interactive session.** I specified what it should do, chose
the algorithms and the look, tested it on my own machine, and rejected the
versions that were wrong. The code, comments and commit messages are Claude's.

Worth being specific about what that means in practice, because "AI-written"
covers a lot of ground:

* The **architecture decisions were mine** — per-pane panels rather than a
  desktop wallpaper layer, a separate control binary rather than a flag, typed
  per-effect parameters rather than a hardcoded list.
* The **visual tuning was entirely mine and could not have been automated.**
  Every effect went through rounds of "too dark", "chunkier", "that's not my
  rain" against the real thing on screen. Several times the code passed all its
  tests and still looked wrong.
* **The ports are faithful to sources I chose**, and where they deviate the code
  says so and why.
* It is **not vibe-coded and not unreviewed**: 207 tests, measured before/after
  numbers on every optimisation, and the commit history shows the failures as
  well as the fixes — including changes that were reverted because they measured
  no better.

If you object to AI-authored code, that is a reasonable position and this
repository is a fair thing to skip.

---

## License

MIT. See [LICENSE](LICENSE).

The ported effects credit their sources above; each port's module header records
what was kept, what was changed, and why.
