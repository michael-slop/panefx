# Themes -- where the palettes come from

panefx ships 27 colour themes, compiled in from `assets/themes/`. Each is a flat
`colors.toml` in Omarchy's format: `background`, `foreground`, `accent`, `cursor`,
`selection_foreground`, `selection_background`, and `color0`-`color15`.

| themes | source | licence |
|---|---|---|
| Tokyo Night, Catppuccin Mocha, Catppuccin Latte, Gruvbox, Nord, Kanagawa, Everforest, Rosé Pine Dawn, Flexoki Light, Ristretto, Osaka Jade, Matte Black, Hackerman, Lumon, Miasma, Retro 82, Vantablack, Ethereal, White | [Omarchy](https://github.com/basecamp/omarchy)'s own themes (`themes/<id>/colors.toml`), themselves ports of each scheme's original | MIT, © David Heinemeier Hansson |
| Dracula, One Dark, Solarized Dark, Monokai, GitHub Dark, Ayu Mirage | the [Tinted Theming](https://github.com/tinted-theming/schemes) Base16 schemes, imported through [Aether](https://github.com/bjarneo/aether) and mapped to the 16 ANSI slots with the standard Base16→ANSI map. Two accents were corrected by hand to each theme's signature colour (GitHub Dark `#58a6ff`, Ayu Mirage `#ffcc66`). | MIT, © Tinted Theming |
| Night Owl | written from [Sarah Drasner's Night Owl](https://github.com/sdras/night-owl-vscode-theme) VS Code palette | MIT |
| slop (house) | the author's own palette, from michaelslop.org | MIT (this repository) |

Each theme is still its designers' work; the palettes are credited here, not
claimed. The original schemes are worth visiting.

## The editors

[`extras/nvim/zz-colormesh.lua`](extras/nvim/zz-colormesh.lua) makes Neovim,
Neovide and any LazyVim-based config follow panefx's theme with that theme's
**real** colorscheme plugin, switching live in every running editor. The map from
theme to plugin is `~\.config\color.mesh\neovim.conf` (panefx writes the bundled
copy there if you have none). Copy the Lua file into your config's
`lua/plugins/`; the plugins install on the editor's next start.

## Your own

Drop `~\.config\panefx\themes\<name>.toml` in the same format and it joins the
menu under "mine". A file there named after a shipped theme overrides that
theme's colours key by key.
