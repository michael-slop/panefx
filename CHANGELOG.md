# Changelog

## 0.2.1 -- wallpaper layers (unreleased)

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
