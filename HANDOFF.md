# panefx — HANDOFF

> **Renamed from `asciifire-panes` on 2026-08-15.** The old name predated the
> project doing anything but fire. Env vars are now `PANEFX_*`, the window class
> is `PaneFxClass`, config lives at `.config\panefx\config.toml`, and the binary
> installed for GlazeWM is `scripts\panefx.exe`.

## What panefx is

**A borderless window that renders arbitrary content, pinned behind another
window, that follows it through a tiling WM's retiles and z-order churn.**

That follower machinery — the GlazeWM IPC client, the z-order re-pin, the
padding-aware grid, the font verification — is the hard part, and it does not
care what gets drawn. **ASCII effects are one subsystem, not the definition.**
Plausible siblings: image/GIF playback, shaders, video, live system stats,
per-workspace wallpapers.

Intended layering:

```
panefx
├── panel / follower / z-pin / config / control   ← content-agnostic core
└── fx
    └── ascii            ← the current subsystem
        ├── rain
        ├── flames
        └── fire
```

Keep the cell-grid concept (and `cell_w`/`cell_h`/`font`/`chars`) inside the
ASCII subsystem — the core should hand a renderer a rect and a surface, not a
character grid. Do not let `AsciiAnimation` become the universal interface by
default.

Windows 11 / pHub. Four ASCII effects today; the machinery is content-agnostic.

Plan: `C:\Users\micha\.claude\plans\https-github-com-mhearse-asciifire-i-wan-serene-popcorn.md`

## Status (2026-08-15) — WORKING END TO END

| Module | State |
|---|---|
| `src/animation.rs` | Trait + typed params + effect registry |
| `src/rain.rs` | Port of Michael's `createRain` |
| `src/flames.rs` | Port of the msimpson gist |
| `src/waves.rs` | Port of `blackwaves.py` |
| `src/fire.rs` | Port of `mhearse/asciifire` (legacy) |
| `src/ipc.rs` | GlazeWM websocket client, on its own thread |
| `src/control.rs` | Control channel on 127.0.0.1:6124 |
| `src/config.rs` | Layered defaults / TOML / env, with save |
| `src/panel.rs` | Panel window + cached GDI objects |
| `src/render.rs` | ExtTextOutW cell blitter |
| `src/bin/panefx-ctl.rs` | ratatui control TUI |

**66 unit tests passing. Verified visually on screen, not just by exit code.**

**It autostarts with GlazeWM** — it is a function of the WM, not a separate
service, because it depends on the WM's IPC for all window geometry.

* Binary is installed to `.glzr\glazewm\scripts\panefx.exe`
  (alongside `glazewm-move.exe`).
* `startup_commands` in `config.yaml` launches it when the WM starts.
* `shutdown_commands` kills it when the WM exits — without this the panels
  linger as orphaned windows with no WM to follow.
* Note `startup_commands` fires only at WM **startup**, not on config reload.
  After editing the config, restart GlazeWM (or start the exe by hand) to see
  it come up.

Rebuild and reinstall:
```powershell
cd C:\Users\micha\panefx
.\build.ps1 -Install     # build, install BOTH copies, verify, restart daemon
.\build.ps1              # dev build only
.\build.ps1 -Test        # tests only
```

**Always use `-Install` rather than copying by hand.** panefx installs to two
places and they are not interchangeable:

* `~\bin\` — `panefx.exe` + `panefx-ctl.exe`, on PATH, so you can type
  `panefx-ctl` from any shell.
* `~\.glzr\glazewm\scripts\` — `panefx.exe` only. GlazeWM launches it from here
  by absolute path, so **this is the copy that actually runs.**

Copy to only one and you get the worst kind of bug: the build succeeds, the code
is correct, and the screen does not change — because GlazeWM is still running
the other copy. The script copies both and hash-verifies them.

It also stops the daemon first, because a running panefx **locks its own
binary** and the build fails with `failed to remove file ... panefx.exe`, which
reads like a permissions problem.

Also changed outside this repo:
* `AppData\Roaming\alacritty\alacritty.toml` — `opacity = 1.0` → `0.6`
  (`live_config_reload = true`, so it applies on save, no restart).
* `.glzr\glazewm\config.yaml` — added an `ignore` window rule for
  `panefx` / `PaneFxClass`, next to the existing Lively rule.

## Verified live (measured, 2026-08-15)

* **Retile/resize following works.** Forced `resize --height -20%` then `+20%`
  via IPC. Terminals went 1274 → 1561/986 → 1277/1270; both panels tracked to
  the new rects *exactly*, including the restored geometry.
* **Z-order is correct and holds.** Enumerated the live z-order:
  `TERM 12911596 / PANEL 42929216 / TERM 197478 / PANEL 12388268` — each panel
  immediately behind its own terminal, and no panel above any application
  window. The re-pin-every-reconcile strategy beats GlazeWM's own z-ordering.
  **The per-pane architecture works; the WorkerW fallback was not needed.**

## Verified facts (measured on pHub, not assumed)

* **Alacritty's Win32 class name is `"Window Class"`** — winit's generic
  default, NOT `"Alacritty"`. Too generic to match on alone; other winit apps
  collide. **Match on `processName == "alacritty"`**, use class as a secondary
  check only.
* Each Alacritty process also owns a hidden 16×16 `"Winit Thread Event Target"`
  window. Must be skipped or we spawn panels for phantoms. (GlazeWM already
  ignores these — its `query windows` returns only the real ones.)
* **GlazeWM IPC verified working**: `ws://localhost:6123`, send the literal text
  `query windows`, get JSON with `handle`, `processName`, `className`,
  `x/y/width/height`, `state`, `displayState`. Handles match Win32 exactly.
* **Coordinates can be negative** (multi-monitor; a monitor sits left of
  primary — observed x = −1436). No unsigned assumptions anywhere in panel
  positioning.
* Rust: **stable-msvc has no linker on PATH** (`link.exe` not found).
  Build with `cargo +nightly-x86_64-pc-windows-gnu`. Same toolchain the patched
  GlazeWM uses.

## The algorithm

Ported from `mhearse/asciifire` (`asciifire.py`), itself a port of Thiemo
Mättig's JS. The original is a **curses** app — TTY-owning, `getmaxyx()`-sized,
five flat curses colour pairs. Only the algorithm survives; the curses shell is
discarded, which is what makes a real RGB gradient possible.

Three things were got wrong before the output looked like fire. All three
**passed the tests that existed at the time**:

1. **Uniform additive decay → a smooth heat gradient.** Cold at top, hot at
   bottom, no flames at all. Passed `bottom_is_hotter_than_top`.
   Fix: multiplicative cooling.
2. **Uniform cooling → flat horizontal bands.** Every cell in a row identical.
   Passed `has_cold_gaps_not_just_a_gradient` (the upper half really was
   empty). Fix: random per-cell jitter → but this only got dithered bands.
3. **Evenly-weighted L/C/R kernel → no vertical tongues.** A 1:1:1 average is a
   horizontal blur applied every frame; it destroys vertical structure within
   a row or two. Fix: **6:1:1 centre-weighted kernel**.

Also: the random seed row must be **off-screen** (`cells` holds `rows + 1`
rows). Mättig's description says so explicitly. Drawing it puts a solid wall of
hot glyphs along the bottom edge instead of flame roots.

Two more were only visible once it was on screen behind a real terminal:

4. **A near-black cool end is invisible through `opacity = 0.6`.** The obvious
   ramp (`#0a0f0a` → `#b4ffbe`) renders as nothing for its lower two thirds,
   because the blend pulls everything back toward the terminal's dark
   background. Only the hot `$`/`#` roots survived. Fix: brighten the whole
   ramp, cool end especially (now starts `#142818`).
5. **Fixed decay = flames a fixed number of rows tall.** Tuned on an 80x25 grid
   it looked right; on a real 85-row terminal the fire was a stripe along the
   bottom edge with the top two thirds empty. Fix: `decay_for_rows()` scales
   cooling to panel height, so flames always reach ~60-70% of the way up.

6. **Row 0 had no neighbour above it**, so nothing cooled it and whatever heat
   arrived rendered as a hard flat line across the full panel width. On screen
   this read as horizontal streaks along the top edge of every terminal
   (measured: green `92,120,81` pixels against the `46,46,46` background).
   This is the mirror of the off-screen seed row at the bottom. Fix:
   `top_fade()` ramps the topmost rows to zero so the fire dies out naturally.
   Cropping the panel would have hidden the symptom while leaving the fire
   sliced off at whatever the new boundary was.

Each fix has a named regression test (`top_of_fire_goes_cold`,
`rows_are_not_flat_bands`, `flames_have_vertical_structure`,
`seed_row_is_off_screen`, `ramp_survives_the_opacity_blend`,
`flames_scale_with_panel_height`, `top_rows_are_completely_dark`).
**Green tests did not mean correct output at any point in this module's
history — always look at the rendered frame.**

## The effect actually in use: `flames` (msimpson gist)

**`src/flames.rs` is the default and the one Michael chose.** It is a faithful
port of https://gist.github.com/msimpson/1096950 — read before porting.

It is a genuinely different algorithm from `fire.rs`, and the differences are
why it works where the other did not:

* **Integer heat (0..=65) indexing the glyph table directly** —
  `char[min(b[i], 9)]`. There is no float-to-bucket quantisation, which is
  exactly where `fire.rs` generated horizontal banding.
* **Sparse seeding** — only `width/9` random cells per frame, not the whole
  bottom row. Gives discrete rising sources instead of a solid sheet.
* **Asymmetric kernel** `(self + right + below + below_right) / 4`, updated
  IN-PLACE over a flat array so cells read already-updated neighbours. The
  integer division is the only cooling; there is no separate decay term.
* **10 glyphs** `[' ', '.', ':', '^', '*', 'x', 's', 'S', '#', '$']` and four
  colour bands keyed to value thresholds (>15, >9, >4, else), not one colour
  per glyph.

### It is a BOTTOM BAND on purpose

The `/4` cooling means the fire reaches ~20 rows whatever the panel height, so
on an 84-row terminal it sits as a band along the bottom. **Michael saw this
live and chose it — "what if I want the bottom of the pane to only have the
animation? that's kind of why I like it".** Do not "fix" it into a full-height
effect. `stays_a_bottom_band_on_a_tall_panel` is the tripwire.

Raise `PANEFX_SEED` (default 65) if a taller flame is ever wanted.

`PANEFX_EFFECT=fire` switches back to the original `mhearse/asciifire` port,
kept for comparison.

## The `rain` effect

Ported from **`site/static/app.js:773`** (`createRain`) — Michael's own
renderer, the one driving the **self.net boot screen** (`initBoot`, app.js:716,
`{dim:true}`). Read before porting.

Disambiguation, because this cost real time: `app.js` calls `createRain` twice —
line 716 is the self.net boot screen (**the right one**) and line 927 is the
Konami-code easter egg. The **moecode window pane** that opens after the boot
screen uses a *different* rain implementation entirely, NOT `createRain`, and
Michael considers that one wrong. Do not port from the moecode pane.

Kept: the exact CP437 charset (ASCII-not-katakana on purpose — BigBlueTerm is a
DOS face with no katakana), one head per column with negative start rows so
columns stagger, `#c8ffc8` head over `#22cc44` trail, respawn past the bottom,
and the stepped 55ms tick ("a CRT reads better stepped than smooth").

Deliberately NOT ported: skeleton silhouette mask, the "michaelslop.org" text
flourish, wizard lightning. Michael asked for just the rain; lightning is a
possible later addition.

**`CELL = 16` in the original.** That is not the Alacritty text cell (10x15 for
BigBlueTerm437 @ 11pt/96dpi). Run the rain at `PANEFX_CELL_W=11
PANEFX_CELL_H=16` or it renders denser and finer than the site version.

One thing had to change rather than be copied: the original is a canvas that
paints a translucent black rect each frame, so the trail is a side effect of
pixel persistence. This is a character grid cleared every frame, so the trail is
modelled explicitly as per-cell brightness that decays.

## THE FONT TRAP: ANSI_CHARSET silently substitutes Arial

**`CreateFontW` with `ANSI_CHARSET` silently returns Arial when asked for
BigBlueTerm437 Nerd Font Mono.** No error. Drawing succeeds. You just get the
wrong typeface. Verified with `GetTextFaceW`:

```
BigBlueTerm437 Nerd Font Mono  charset=ANSI     -> GOT 'Arial'   *** SUBSTITUTED ***
BigBlueTerm437 Nerd Font Mono  charset=DEFAULT  -> GOT 'BigBlueTerm437 Nerd Font Mono'  OK
Consolas                       charset=ANSI     -> GOT 'Consolas'  OK
```

Consolas survives `ANSI_CHARSET`, which is why this hid for so long — the font
looked correct until the face name was changed, and **everything rendered in
Arial from that point on**.

Two rules:
* Always `DEFAULT_CHARSET` for Nerd Fonts / anything with wide glyph coverage.
* **Checking that the family resolves in .NET/GDI+ does NOT catch this.** Only
  `GetTextFaceW` on a DC with the font selected reports the truth.
  `render::verify_font` does exactly that, and `main` prints `font OK: ...` or a
  warning naming the substitute at startup.

## The `waves` effect (blackwaves)

Ported bar-for-bar from `C:\Users\micha\blackwaves\blackwaves.py`. That script's
comments record measurements taken off a real reference clip and ARE the design —
read it before changing anything here.

Structure that makes it affordable: the six-octave ridged/sheared noise field is
built **once per resize** and cached; each frame only bilinearly resamples it
through a cheap low-frequency swirl warp. The field deliberately does not
advect — measured best whole-frame translation between reference frames is
(0,0) — it deforms in place.

Three transcription traps, all of which cost real debugging time:

1. **`np.interp` clamps, it does not extrapolate.** Falling through to `p=1.0`
   past the last knot sent a cluster of bright cells to the top rung.
2. **`np.gradient` uses a central difference over spacing 2 in the interior**
   and one-sided over spacing 1 at the edges. Using one rule everywhere
   flattens the lit region.
3. **`to_reference_levels` divides by the max of the SHIFTED array**
   (`x -= x.min(); x /= x.max()`), not by `(max - min)` of the original.

**`headroom` defaults to 0.95 here, not the Python's 0.62.** The algorithm is
transcribed exactly, but `hash01` is not numpy's PCG64, so the `(freq+2)²`
random grids hold different numbers — octave-0 mean 0.517 vs 0.456 — which
leaves the top end hotter. 0.95 reproduces the Python's measured glyph
histogram at 200x60 ('@' 0.30%, '%' 1.1%, '#' 5.4%).
`top_rung_stays_rare_like_the_reference` is the tripwire.

**Cost: ~87% of one core** — far more than `rain` (14%) or `flames` (10%),
because every cell gets warp + gradient + three shading lobes + a remap every
frame. This is the main target for the optimisation pass.

## Live control: `panefx-ctl`

A ratatui TUI that talks to the running daemon. The daemon stays headless under
GlazeWM; run the TUI whenever you want to fiddle, quit it, daemon unaffected.

```powershell
C:\Users\micha\.glzr\glazewm\scripts\panefx-ctl.exe
```

Keys: `↑↓` move · `←→` adjust (`H`/`L` for ×10) · `Enter` type a value ·
`s` save · `r` revert · `q` quit without saving.

**Changes are live but not persisted until `s`.** Experiment freely.

### Protocol

Newline-delimited JSON on `127.0.0.1:6124`, one reply per command. Bind failure
is non-fatal — the daemon logs and runs without live control, because a backdrop
you cannot tune beats no backdrop.

```
{"cmd":"get"}                                       -> snapshot
{"cmd":"set","key":"fps","val":30}                  -> Config field
{"cmd":"effect","name":"rain"}                      -> switch now
{"cmd":"param","key":"tick_ms","val":{"kind":"int","v":40}}
{"cmd":"param","key":"head","val":{"kind":"colour","r":200,"g":255,"b":200}}
{"cmd":"save"}   {"cmd":"revert"}
```

Test it by hand with a TcpClient — but **wait ~250ms before reading the reply**.
The daemon answers once per frame, so an immediate blocking read looks like a
crashed daemon when it is just impatience. (That cost real debugging time.)

### Adding a knob to an effect

Effects declare their own parameters; the TUI renders whatever it is handed and
**must never hardcode which knobs belong to which effect**.

```rust
fn params(&self) -> Vec<Param> {
    vec![Param::int("tick_ms", "step (ms)", self.tick_ms as i64, 10, 300)]
}
fn set_param(&mut self, key: &str, v: &ParamValue) -> bool {
    // MUST clamp to the bounds declared above.
}
```

`params()`/`set_param()` have default impls, so an effect with no knobs (like
`fire`) needs no changes. A param round-trips to `config.toml` under a section
named for the effect (`[rain]`, `[flames]`) — see `Config::set_effect_param` and
`apply_saved_params` in `main.rs`.

**Only sections named after an effect are treated as params.** Any other
`[section]` header is decorative and its keys still set top-level Config fields,
so adding a cosmetic header cannot silently disable everything under it
(`non_effect_sections_are_decorative_only` is the tripwire).

### What needs a rebuild vs applies in place

* **In place, same frame:** everything the renderer re-reads per frame — `font`,
  `cell_w/h`, `pad_x/y`, `crop_top` — and any effect param via `set_param`.
* **Needs `sim` rebuilt:** effect switch, and `fps` (rain captures `frame_ms` at
  construction).
* **Needs a re-query:** `cell_*`, `pad_*`, `crop_top` set `needs_query` so the
  grid resizes immediately instead of waiting up to 500ms for the poll.

## Architecture: adding a new animation

Effects implement the `AsciiAnimation` trait (`src/animation.rs`). The panel,
renderer and supervisor know nothing about fire — they hold a
`Box<dyn AsciiAnimation>`. To add an effect (matrix rain, plasma, starfield):

1. Write a new module with your own state.
2. `impl AsciiAnimation for YourEffect` — `name`, `resize`, `dimensions`,
   `step`, `cell_at`, `background`.
3. Construct it instead of `Fire` in `main.rs`.

Nothing else changes. Note this is a trait, not class inheritance — Rust has no
subclassing, so effects are independent types implementing a shared interface,
dispatched dynamically through the box.

`cell_at` returns glyph AND colour in one call on purpose: effects usually
derive both from the same computed value (for fire, a dithered ramp index), and
splitting it into separate `glyph_at`/`color_at` doubles the per-cell work.

## Tuning knobs

* `PANEFX_CELL_W` / `PANEFX_CELL_H` env vars override the character cell
  size. Defaults 10x15, measured from the Alacritty font on pHub
  (BigBlueTerm437 Nerd Font Mono @ 11pt, 96 DPI = 9.78 x 14.67 px). Change
  these if the font, its size, or the display DPI changes.
* `render::FONT_FACE` must match `[font] normal.family` in `alacritty.toml`.
* `TARGET_FPS` in `main.rs` (20). Raise for a smoother backdrop at linear cost.
* Flame height: the `2.6` constant in `fire::decay_for_rows`. Higher = shorter.
* `fire::TOP_FADE_ROWS` (6) — how many rows at the top fade to black.
* `fire::DITHER_STRENGTH` (1.0) — spread of the glyph-threshold dither, in ramp
  buckets. Setting it to 0 brings back the horizontal-streak bug.
* `PANEFX_PAD_X` / `PANEFX_PAD_Y` (10 / 8) — must match `[window]
  padding` in `alacritty.toml`, or the fire is not rooted at the bottom of the
  visible text area.

* `PANEFX_EFFECT` — one of `flames`, `rain`, `waves`, `fire`. `PANEFX_ROTATION` takes a comma-separated list, with `PANEFX_ROTATE_SECS` to cycle.
* `PANEFX_SEED` (65) — flame height for `flames`. Higher climbs further.
* `PANEFX_CROP_TOP` (0) — pixels chopped off the top of the animation.
  Not needed by `flames`; was added for `fire`'s banding tail.

## Deploying to another machine

**`target-cpu=native` makes the binary NON-PORTABLE.** `.cargo/config.toml` sets
it, which is right for the machine it is built on and wrong for every other one.
pHub is a Ryzen 9800X3D (Zen 5); SloppyLaptopy is an Intel i7-13700H. Copying
the native binary risks an illegal-instruction crash.

Build a portable one instead:

```powershell
$env:RUSTFLAGS = "-C target-cpu=x86-64-v2"
cargo +nightly-x86_64-pc-windows-gnu build --release --offline --target-dir target-portable
Remove-Item Env:\RUSTFLAGS
```

`x86-64-v2` is SSE4.2-era — safe on anything from the last decade. Verified
running on the laptop's Intel CPU with no crash.

### SloppyLaptopy, deployed 2026-08-16

* `~\bin\panefx.exe` + `panefx-ctl.exe` (portable build; `~\bin` already on PATH)
* `~\.config\panefx\config.toml` — copied from pHub
* GlazeWM `config.yaml` — panefx **appended** to the existing
  `startup_commands` / `shutdown_commands`, which already launch Zebar. Append,
  never replace, or Zebar stops starting. Backup at `config.yaml.bak-panefx`.
* GlazeWM ignore rule for `panefx` / `PaneFxClass`
* `alacritty.toml` opacity 1.0 → 0.6
* `%APPDATA%\neovide\config.toml` — `transparency = 0.6`, `frame = "none"`

No Rust toolchain on the laptop, and none needed — binaries are built on pHub.

**SSH quoting:** nested quotes through PowerShell→SSH mangle reliably (this cost
several attempts). `scp` a `.ps1` and run it with
`powershell -NoProfile -ExecutionPolicy Bypass -File`. Also note `scp target:bin/x`
silently did nothing; the explicit `scp target:C:/Users/micha/bin/x` worked.

## Performance — read this before "optimising" anything

Measured steady state at 20fps, before vs after the optimisation pass:

| effect | before (1 panel) | after, 1 panel | after, 2 panels |
|---|---|---|---|
| flames | 16.9% | ~2.8% | 4.5% |
| rain | 14.8% | ~4.0% | 4.7% |
| waves | 88.7% | ~14% | 19.1% |
| fire | 30.1% | ~5.5% | 8.6% |

**Always record the panel count with a CPU figure.** The renderer is per-panel
while the simulation is shared, so two panels cost noticeably more — and a
number quoted without its panel count is not comparable to anything. During
this pass a 14% reading (1 panel) and a 19% reading (2 panels) were briefly
mistaken for a regression caused by a code change; they were just different
window layouts.

**The renderer is the hot path, not the effects.** This is the single most
useful thing to know here, and it was counter-intuitive: `waves.step()` costs
1.0% of a core and `cell_at` 0.2%, against ~72% that was in `draw_animation`.
Do not go hunting in the simulation maths first.

What actually bought the wins, in order of value:

1. **One `ExtTextOutW` per row per colour** (`render.rs`). The old run-batcher
   broke a run whenever glyph OR colour changed, so a varied field produced
   **12,242 draw calls per frame** at 143x170. Now each row is read once,
   bucketed by colour, and drawn with the per-character advance array placing
   glyphs at their cells.
2. **Quantised ink** (`waves::INK_STEPS = 12`). Continuously varying colour
   defeats any batching. The Python does the same thing (`--ink-steps`) for the
   same reason.
3. **Cached GDI objects** (`panel::GdiCache`). The DC, bitmap, font and brush
   used to be recreated every frame per panel; `CreateFontW` runs the font
   mapper. Rebuilt only when their inputs change.
4. **IPC on its own thread** (`ipc::IpcThread`). The inline socket read burned
   5ms of every 50ms frame.
5. LUTs and allocation removal in `waves` — real, but smaller than expected
   precisely because the maths was never the bottleneck.

### Measuring it correctly

**Wait ≥8s after switching effects before measuring.** `waves` rebuilds its
base field on switch and resize; a reading taken 3s in catches that rebuild and
reports ~60% when steady state is 14%. This produced a phantom "regression"
during the optimisation pass.

`PANEFX_FPS_LOG=1` prints the achieved frame rate. Use it to tell "slower" apart
from "no longer throttled, so doing more work per second" — the frame sleep only
pads out to the budget, so an over-budget frame silently lowers the real rate
with no catch-up.

### Do not undo these

* `flames` must NOT be parallelised — its kernel is deliberately in-place and
  forward-walking, and row-splitting changes the output.
* `Panel::drop` must keep freeing every cached GDI object and restoring the
  DC's originals. Verified: handles flat at 5 across repeated panel
  create/destroy cycles.
* `AsciiAnimation::changed()` defaults to `true`, which is always correct. Only
  override it where the effect genuinely knows nothing moved.


## Running the fire standalone

`fire.rs` and `palette.rs` have no Windows or network dependencies, so they can
be tested without the full crate (whose deps need network to vendor):

```powershell
# scratch copy used during development
cargo +nightly-x86_64-pc-windows-gnu test --offline
```

To eyeball a frame, a `main.rs` that steps 300 frames and prints
`f.glyph_at(c, r)` per row is enough — that is how all three bugs above were
found.

## Next steps

1. `cargo fetch` (needs network) for `tungstenite`, `serde`, `windows` crates.
2. `ipc.rs` — websocket client to 6123: subscribe to `window_managed`,
   `window_unmanaged`, `focus_changed`, `workspace_activated`, plus a 500ms
   backstop poll. **GlazeWM emits NO move/resize event** — events are only a
   trigger to re-`query windows` for truth.
3. `panel.rs` — `WS_POPUP` + `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`.
4. `render.rs` — GDI cell blitter.
5. Add a GlazeWM `ignore` window rule for our panel class.
6. Set `window.opacity = 0.6` in `alacritty.toml`.

## Known risk

GlazeWM actively reasserts z-order (`ZOrder::Normal | TopMost |
AfterWindow(handle)`) on every focus change — see
`RustroverProjects\glazewm\packages\wm\src\commands\general\platform_sync.rs`.
Panels must re-pin behind their terminal after Glaze finishes, every focus
change, forever. If this flickers badly, the fallback is the **WorkerW
wallpaper layer** (send `0x052C` to `Progman`, reparent into the spawned
`WorkerW`): one fullscreen surface, no following, no z-order war, invisible to
Glaze — at the cost of painting the whole desktop rather than per-pane.
`fire.rs` and `render.rs` are shared either way, so the pivot is cheap.
