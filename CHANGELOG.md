# Changelog

## 0.2.1 -- switches and wallpaper layers (2026-10-08)

### Added
- **Three switches**, in the GUI's settings tab, the TUI, and ticked in the tray
  menu. Each saves itself the moment it is flipped:
  - **backdrops** -- the effects behind your terminal windows (as before).
  - **transparency** -- see-through windows everywhere, on or off. Off makes
    Alacritty and Neovide solid while the backdrops keep running; your chosen
    opacity is kept for when it goes back on.
  - **pause when covered** -- stop animating a screen while windows cover it
    (saves CPU), or keep it animating always.
- Default window opacity is 70% (was 60%). Existing configs keep their value.

### Fixed
- **A wallpaper layer could get stuck on a screen.** The UIs address a layer by
  its position in the stack, but the stored layer numbers kept gaps and kept
  layers above a screen that was switched off, so the two drifted apart: a
  plasma layer stored as "layer 2" over an `off` base showed at position 0,
  "remove" asked for the base instead, and the plasma could never be removed --
  every base chosen afterwards drew under it. Layers are now always numbered
  1, 2, 3... with no gaps, switching a screen off clears its layers, adding a
  layer to an off screen makes it the base, and a config written before this is
  repaired as it loads.
- **Changing a screen's base effect drew the old one.** The screen was rebuilt
  from the config before the new choice was saved into it, so switching
  plasma -> flames kept drawing plasma. The config is now updated first (and
  rolled back if the screen refuses), and the base is always the effect chosen.
- **Without GlazeWM, panefx no longer freezes for 4 seconds at a time.** It
  tried to reconnect to a GlazeWM that was not running every 10 s, on the thread
  that draws everything, and a refused localhost connection takes 4 s on
  Windows. It now only connects when a GlazeWM process exists -- which also
  takes 4 s off every startup on a machine without GlazeWM.
- GlazeWM quitting at the wrong moment could stop the whole daemon; it now
  falls back to following windows through Windows itself, as it already did
  otherwise.
- `revert` made the windows see-through even with transparency or the
  backdrops switched off.
- Clicking a switch in the GUI hid any unsaved changes (the save/revert buttons
  vanished), and the TUI marked self-saving switches as "*modified".
- The tray's Reload could leave no daemon running: the new one was refused by
  the old one's lock while it was still exiting.
- Saving a switch or a theme could write `PANEFX_*` environment overrides into
  config.toml; config.toml is now written atomically.
- In the TUI, cycling a layered screen's effect no longer passes through `off`
  (which would clear its layers).
- Explorer's desktop window can never count as covering a screen.
- The settings tab's hints for "freeze at %" and "bg opacity %" carried stray
  line breaks and indentation.
- The log says "standing in for GlazeWM" when GlazeWM goes away, not "by
  configuration".

### Added
- `drawing` in each monitor's snapshot: the simulations the screen is really
  running, beside `layers`, the config's view of them.

## 0.2.0 -- themes (2026-09-30)

### Added
- **The theme driver.** 27 colour themes plus *house* (the look you dialled in by
  hand, recorded the moment a theme first replaces it and restored exactly).
  One pick recolours panefx's effects -- every colour of every effect, behind the
  terminals and on every monitor, live -- and its GUI, then Alacritty, the Neovim
  editors, Windows light/dark, VS Code, Windows Terminal, Xournal++ and the
  sl0p.notepad / sl0p.ink forks. Each app reports what happened to it, can be
  switched off, and an app that was open (Xournal++) is retried until it closes.
- Themes tab in the GUI and the TUI (`t`, `n`/`N` for next/previous); a Theme
  submenu plus next/previous in the tray; `panefx theme <id|next|prev|house|list>`,
  which also works with the daemon stopped.
- `{"cmd":"theme","name":...}` on the control channel.
- Your own themes in `~\.config\panefx\themes\`.
- `extras/nvim/zz-colormesh.lua`, THEMES.md.

### Fixed
- The GUI's settings tab scrolls; on a small window its lower half was unreachable.
- The GUI's logs tab shows the log (a `logs` reply was being dropped).
- `panefx --tui` no longer trips panefx-ctl's unknown-option check.
- `build.ps1 -Install` restarts the daemon through the `panefx` logon task rather
  than as a child of the build shell.
- Only one GUI opens, however fast the tray icon is clicked.
- GlazeWM is optional everywhere: the daemon no longer exits when GlazeWM quits
  (it follows windows through Win32 and picks GlazeWM back up), and the GUI no
  longer claims the terminal backdrops need it.

### Removed
- The effects built from other people's art: wizardtorch, tgevil, wzfire,
  raalien, fishloop, warlockspin.

## 0.1.0

The follower, the desktop wallpaper with per-monitor effects and layers, the TUI,
the Win98 GUI, the tray, the installer and the report tool.
