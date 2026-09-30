# Changelog

## 0.2.0 -- themes (unreleased)

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

## 0.1.0

The follower, the desktop wallpaper with per-monitor effects and layers, the TUI,
the Win98 GUI, the tray, the installer and the report tool.
