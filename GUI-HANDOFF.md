# panefx-gui — handoff

Written 2026-08-19. **The GUI is built and installed.** Everything but live
previews is done; this is where to pick that up.

Read `HANDOFF.md` first for panefx itself; this covers only the GUI and the
layer model built alongside it.

---

## What this is

`panefx-ctl` (the ratatui TUI) works and must keep working — it is the only way
to tune panefx over SSH from the laptop. The GUI is **the same program with a
better input device**, not a new product.

Every constraint in the TUI is a terminal limitation, not a design choice: one
highlighted row, arrow-key nudging, typed hex codes, no previews. The GUI
removes them. The protocol does not change — both are clients speaking the same
line-delimited JSON to the same daemon on `:6124`, and both can run at once.

**"More agency" means two specific things** (Michael's framing, confirmed):

1. **See before you commit.** Turning `darkcut` from 300 to 500 currently means
   looking at a wall to find out what happened.
2. **Direct manipulation.** Grab any fader on any monitor — no mode, no row to
   navigate to first.

---

## Decisions already made

Settled with Michael; do not relitigate without asking.

| Question | Decision |
|---|---|
| Toolkit | **Rust + egui**, Win98 theme ported. One language, one binary. |
| Scope | Everything the TUI does, **plus** previews and a real colour picker. |
| Input | **Mouse anywhere, keyboard follows** — grab any control directly, and arrows still walk a focus ring. |
| Wallpaper layout | **Monitor list + detail pane.** Left: four monitors with live thumbnails. Right: the selected one's full controls. You tune one screen then the next; four cramped columns would serve a workflow nobody has. |
| Tabs | **Keep Effects and Wallpaper separate.** TUI-fx follows Alacritty windows and needs GlazeWM; wallpaperfx is per-monitor and needs neither. Separate tabs let the GlazeWM warning land in exactly one place. |
| Saving | **Explicit save, but VISIBLE.** Same model as the TUI (live now, `config.toml` on Save) — that is what lets you experiment and walk away. The GUI shows the dirty state instead of expecting you to remember `s`. |
| Per-monitor overrides | **Marked on the knob**, with a reset-to-shared affordance. You always know whether you are editing DISPLAY3 or the default all four inherit. |
| Previews | **Faithful, at the monitor's aspect.** The portrait Acer previews portrait, on the real cell grid, so `fit`/`detail`/`darkcut` show truthfully. |

---

## Step 1 — DONE: the chrome

`src/win98.rs`, and `src/bin/panefx-gui.rs` as a proof window.

Every value is **lifted from `slopkit/internal/win98/`**, which lifted them from
michaelslop.org. Not re-derived: panefx, slopkit and s0nar.slop must be visibly
one family, and three guesses at "Windows 98 grey" would not be.

**What makes it read as Win98, and neither is the colour:**

1. **The double bevel ring** — four tones in two nested 1px rings, light on the
   top-left, dark on the bottom-right (inverted for a sunken well). Eight
   rectangles, because egui has no inset shadow any more than 98.css's four
   stacked ones are a border. Geometry is in `bevel_rects` and unit-tested.
2. **Zero rounding anywhere.** One rounded corner and it is gone.

**The font is embedded, never requested by name.** BigBlueTerm437 is installed
on one computer; by name the design collapses to "a grey window" anywhere else.
CC BY-SA 4.0 via Nerd Fonts from VileR's Ultimate Oldschool PC Font Pack — the
one obligation reaching the code is **attribution**, so `win98::ATTRIBUTION`
exists and a test pins it. It is exactly the kind of string that gets tidied out
of a dialog, and the obligation does not go away when it does.

### Traps this cost

* **glow, NOT wgpu.** eframe's default backend pulls `windows 0.62`; panefx pins
  `windows 0.58` for Win32 and DirectComposition. Two majors in one binary do
  not coexist — wgpu-hal's DX12 backend fails to compile with a wall of trait
  errors pointing into other people's crates rather than at the cause. Measured:
  the build failed exactly that way before the switch.
* **eframe 0.36 changed the App trait** — `update(ctx, frame)` became
  `ui(ui, frame)`, and styles are per-theme (`set_style_of`). The same style is
  written to BOTH theme slots and the preference pinned, or the window reverts
  to egui's defaults when Windows is set to the other mode.
* **A Win98 button does not invert on hover.** It stays face-coloured and its
  bevel flips on PRESS; only menus and list selections invert to navy. Painting
  hover navy made every focused button look permanently selected — caught by
  screenshotting, not by reasoning.
* The running GUI holds its own `.exe`, so a rebuild fails with "failed to
  remove file" until it is closed. Same trap `build.ps1` documents for the
  daemon.

### Verifying the chrome

GUI apps cannot be launched from an agent shell (session 0). Use GlazeWM:

```powershell
& "C:\Program Files\glzr.io\GlazeWM\cli\glazewm.exe" command shell-exec "C:\Users\micha\panefx\target\release\panefx-gui.exe"
```

Screenshot it with `scratchpad\shot.ps1` run through a scheduled task with
`LogonType Interactive` — a direct capture from this shell sees nothing.

---

## Step 2 — DONE: connection, tabs, and layers

`src/bin/panefx-gui.rs`. Run it with GlazeWM's `shell-exec` (see below).

What works: it connects and polls, shows all four monitors with their effect
and stack depth, and the detail pane sets the base effect or adds a layer above
it. **That is the first UI layers have had at all** — before this the control
command was the only way in.

The GlazeWM check is in: with GlazeWM absent the TUI-fx tab explains that the
backdrops have nothing to follow and links glzr.io and alacritty.org. Not a
greyed control — a disabled slider reads as "broken" where this says why.

The dirty flag is SHOWN: "unsaved" in the title bar, save/revert in the status
bar. The daemon being down is a state with a Start button, not an exit.

### Two traps this cost

* **The window froze.** `TcpStream::connect` to a refused port blocks about a
  second on Windows, and it runs on the UI thread — long enough that Windows
  paints "(Not Responding)" over the title bar. Use `connect_timeout` (120ms)
  with short read/write timeouts (400ms) and back off to 2s while down. Anything
  blocking on the UI thread will do this.
* **A snapshot field does nothing until the DAEMON is reinstalled.** Adding
  `layers` to `WallpaperMonitorView` and rebuilding the GUI is not enough — the
  running daemon still serialises the old shape, and the symptom (detail pane
  says "off", list says "skullspin") looks exactly like a GUI bug. Run
  `build.ps1 -Install`.

## Step 3 — DONE: per-layer editing

Every LAYER of the selected monitor gets its own collapsible section of
controls, so a stacked screen is tuned layer by layer rather than only at its
base. The daemon reports params keyed `<monitor>:<layer>`; the bare
`<monitor>` key still points at layer 0 so the TUI keeps working unchanged.

New widgets in `win98.rs`, **created not ported** — slopkit never built the
fader s0nar.slop's DESIGN.md describes:

* `fader` — sunken groove, raised thumb, click-to-jump. Returns `Some(v)` only
  when the value CHANGED, so a held drag sends one command per step.
* `swatch` — sunken, because it is a sample you look into.
* `segment_meter` — hard blocks, unlit segments still visible.

`copy to…` applies one display's whole stack to another. A sheet, not an
"apply to all" button: with four screens "all" is rarely what is meant.

### The trap

**The colour picker had to become a WINDOW.** The params live in a
`ScrollArea`, and a fixed-size child rect inside one fights the scroll layout —
egui gave the saturation/value square no room and silently drew only the RGB
fields. Anything that needs a guaranteed size does not belong inside a scroll
area.

## Steps 4-5 — DONE except previews

The icon, the About box, and `build.ps1` carrying the GUI. See the commit for
the DIB details (negative height, mask required).

**Still TODO: live previews.** Every effect module is Win32-free, so the GUI can
build and step effects natively — a real render, not a screen capture. Rebuild a
preview only when its effect NAME changes; apply params in place, mirroring
`SimPool::reapply` (`wallpaper.rs:164`).

### Not verified on screen

Scripted clicks kept landing on neighbouring controls (a 56x20 swatch is a small
target, and twice the click hit a resize edge and maximised the window instead).
**The colour picker window and the About box open on a real click but were never
caught in a screenshot.** Worth a human clicking both once.

## Old TODO list

3. **Editing** — faders (Sunken trough + Raised thumb), the colour picker,
   effect cycling, save/revert. **slopkit has NO fader or meter widget**:
   s0nar.slop's DESIGN.md describes them but `voicepanel.go` uses stock
   `widget.NewSlider`. These are being created in the idiom, not ported.
4. **Live previews.** Every effect module is Win32-free (checked module by
   module), so the GUI builds and steps effects natively — a real render, not a
   screen capture. Rebuild a preview only when its effect NAME changes; apply
   params in place, mirroring `SimPool::reapply` (`wallpaper.rs:164`).
5. **Daemon Start button, About box, `build.ps1 -Gui`.** The TUI exits if the
   daemon is absent; the GUI must not, because previews work without it. Start
   it through GlazeWM's `shell-exec` — a process started directly by a GUI can
   die with it.

### Known constraint

`panic = "abort"` is crate-wide and cannot be set per-binary. A GUI panic kills
the window with no dialog — the same behaviour the daemon has today. Not worth
weakening the daemon for.

---

## Layered wallpaper effects — DONE

Built before the detail pane on purpose: layers change what that pane IS (a
stack of effects in z-order, not one effect and its knobs), and designing it
once against the real model beats building it twice.

**The compositing rule needed no new machinery.** `cell_at` already returned
`None` for "draw nothing here", which on a stack means "let the layer below show
through" — so no alpha, no blending, no second buffer.

`render::draw_layers` walks the stack from the **top** and takes the first cell
that answers: one `ExtTextOutW` per cell however deep the stack is. Painting
bottom-up would have cost a draw call per layer per cell, and the renderer is
the hot path.

### Storage — additive, nothing migrates

```toml
wallpaper_3_effect = "flames"     # layer 0, the existing key
wallpaper_3_layer1 = "skullspin"  # layers 1+, nearer the viewer
```

Layer 0 **is** the base effect key. `set_wallpaper_layer` routes layer 0 back to
it, so the two storages cannot disagree about the bottom of the stack. Every
config written before layers existed stays valid; a single-effect monitor is a
stack of one.

### Rules that matter

* **The bottom layer owns the cell size and the background.** It is the only one
  that can — an upper layer's background would erase everything beneath it,
  which is precisely what a layer is not for.
* **All layers share the bottom's grid.** They composite cell-for-cell into one
  panel, so a second grid would have nothing to align to.
* **An `off` layer is REMOVED, not stored as a hole.** A stack with gaps is a
  different thing to reason about, and its length would lie about what is drawn.
* **Any layer being dirty redraws the frame**, because the redraw clears the
  panel before compositing — a still layer above a moving one still needs
  repainting.

### Setting a layer

```json
{"cmd":"wallpaper_layer","monitor":3,"layer":1,"name":"skullspin"}
```

`name: "off"` removes that layer. Verified live on DISPLAY3 and round-tripped
through `config.toml`.

**Proven as data, not by eye**: flames + skullspin on a 100x40 grid gives 272
cells owned by the top layer, 1144 showing the bottom through it, 2584 empty.
"I saw flames and a skull" would not have distinguished compositing from the
skull merely being drawn second.

### Not yet exposed

Neither the TUI nor the GUI has UI for layers — the control command is the only
way in today. The GUI's detail pane is where it should surface (step 3).

## Update 2026-08-19 — the GUI is now the default, and there is a beta package

### Launching

`panefx` (bare) opens the **GUI**. It used to open the TUI, which meant the
GUI was only reachable by knowing its filename.

| command | opens |
|---|---|
| `panefx` | the GUI |
| `panefx --tui` / `-t` | the TUI — **the only one that works over SSH** |
| `panefx --gui` / `-g` | the GUI, explicitly |
| `panefx --daemon` / `-d` | the daemon (unchanged; GlazeWM's autostart uses this) |

The tray icon: left click opens the GUI, and the right-click menu lists
"Open panefx" then "Open panefx TUI (terminal)".

### Single instance

`panefx-gui` guards ITSELF at startup (`desktop::focus_existing_gui`) rather
than the guard living only in the launcher — it is also started from the tray,
from shortcuts, and directly. Matching is by owning **process name**, not
window title or class: the window is undecorated so its caption is chrome we
draw, and eframe's class is generic.

Why it matters: two GUIs both writing `config.toml` loses settings silently —
the second to save wins and the first never knows.

### GUI preferences — `src/gui_prefs.rs`

Theme, tab and selected monitor persist to
`%USERPROFILE%\.config\panefx\gui.toml`.

**Deliberately NOT in `config.toml`.** That file is the daemon's state and the
daemon rewrites it in full on every save; a GUI preference stored there would
be wiped. A test pins that the two paths differ.

`App::remember()` is the single writer, so adding a preference cannot leave one
of three call sites forgetting to record it.

### The bevel convention

**Raised when idle, pushed in when selected.** The tab strip had this
backwards while the monitor buttons had it right, so two halves of one window
disagreed. Both now call `win98::toggle_bevel` / `toggle_face`, defined once
and tested.

### A test that was passing while the thing was broken

`cli_tests::mode_of` was a hand-copied mirror of `parse_args`. When the default
changed from TUI to GUI it kept asserting the old answer and kept passing,
because it was only ever testing itself. It now calls the real `mode_for`, and
a deliberate regression was confirmed to turn it red.

Worth remembering as a shape: a test that re-implements the logic it tests
cannot fail for the right reason.

### The beta package — `dist/`

`dist/` holds the tester-facing pieces; the zip is assembled from them:

* **`Setup-panefx.ps1`** — checks Windows version, GlazeWM (required for pane
  effects, not for wallpaper), Alacritty (optional) and the font, installs what
  is missing, puts the binaries on PATH and starts the daemon. `-CheckOnly`
  reports without changing anything; `-Yes` skips the prompts.
* **`panefx-report.ps1`** — what testers send back. Processes, binary hashes,
  GPU, **display modes** (resolution / refresh / rotation), per-monitor
  geometry with the `consistent` flag, a sampled present rate, the daemon log,
  and the config. Writes one file to the Desktop and sends nothing anywhere.
* **`README-BETA.txt`**

**The font check is the subtle one.** The GUI embeds BigBlueTerm437, but the
**daemon asks GDI for it by name**. GDI does not fail on a missing font — it
substitutes Arial and returns success, so every effect renders in the wrong
typeface with no error anywhere. `GetTextFaceW` on a DC with the font selected
is the only call that reports the truth. Verified both ways: a bogus font name
resolves to "Arial", the real one to itself.

### Building for distribution — NOT the normal build

```powershell
$env:RUSTFLAGS = "-C target-cpu=x86-64-v2"
$env:CARGO_TARGET_DIR = "target-portable"
cargo +nightly-x86_64-pc-windows-gnu build --release
```

The default `.cargo/config.toml` sets `target-cpu=native`, which on pHub means
**AVX-512**. Measured, not assumed: the native build contains 728 AVX-512
instructions (`vmovdqu64`, `vextracti32x4`), the portable build zero. A native
binary dies on another CPU with `STATUS_ILLEGAL_INSTRUCTION` and no message.

Also note pHub has **no MSVC at all** — see the memory note
`phub-has-no-msvc-rust-is-gnu`. Builds need `+nightly-x86_64-pc-windows-gnu`
and the WinLibs mingw64 `bin` on PATH.

### Still open

* **Live previews** (step 4 of the original plan) — the only unfinished item.
* Params are stored per (monitor, effect), not per layer: two layers running
  the same effect on one monitor share their knobs.
