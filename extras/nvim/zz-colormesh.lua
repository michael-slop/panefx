-- color.mesh -> this editor. Installed as lua/plugins/zz-colormesh.lua in every Neovim config
-- the mesh runs (nvim + Neovide share one; nvs-ide has its own), on the laptop and on Windows.
-- Source: ~/src/color.mesh/nvim/zz-colormesh.lua. Do not edit the installed copies.
--
-- It does three things:
--   1. declares every theme plugin in ~/.config/color.mesh/neovim.conf, lazily, so each scheme
--      is installed once and loads only when used;
--   2. at startup, hands LazyVim the scheme for the live color.mesh theme, so there is no flash
--      of the old one;
--   3. watches ~/.config/color.mesh/current and switches every running editor the moment the
--      theme changes (color.mesh on Windows writes that file; on the laptop Omarchy's theme-set
--      hook does, so Omarchy's own menu counts too).
-- For the HOUSE theme it hands back to the editor's own deliberate scheme (tokyobones here,
-- necronomicon in nvs-ide), whatever LazyVim was configured with before this file.
-- "zz-" so this spec merges LAST: its LazyVim opts see, and override, every other file's.

local home = (vim.uv or vim.loop).os_homedir()
local dir = home .. "/.config/color.mesh"

local function read(path)
  local f = io.open(path, "r")
  if not f then
    return nil
  end
  local s = f:read("*a")
  f:close()
  return s
end

local function trim(s)
  return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

-- neovim.conf: id | colorscheme | repo | lazy name | background | needs (comma-separated repos)
local function load_map()
  local map = {}
  for line in (read(dir .. "/neovim.conf") or ""):gmatch("[^\r\n]+") do
    if not line:match("^%s*#") and line:match("|") then
      local f = {}
      for part in (line .. "|"):gmatch("([^|]*)|") do
        f[#f + 1] = trim(part)
      end
      if f[1] ~= "" then
        map[f[1]] = { scheme = f[2], repo = f[3] or "", name = f[4] or "", bg = (f[5] ~= "" and f[5]) or "dark",
          needs = f[6] or "" }
      end
    end
  end
  return map
end

local map = load_map()

-- The live theme's entry, or nil for the house theme / an unmapped one.
local function live()
  local id = trim(read(dir .. "/current") or "")
  local e = map[id]
  if not e or e.scheme == "house" or e.scheme == "" then
    return nil, id
  end
  return e, id
end

-- The editor's own scheme, recorded before this file overrides it (see the LazyVim opts below).
local house = { scheme = nil, bg = vim.o.background }

local function after_scheme()
  -- nvim's transparency layer (plugin/after/transparency.lua) runs once at startup, not on
  -- ColorScheme, so a live switch would bring every background back. Re-run it; a config
  -- without one (nvs-ide) finds nothing and this is a no-op.
  pcall(vim.cmd, "runtime plugin/after/transparency.lua")
end

local function apply(e)
  if not e then
    if house.scheme then
      vim.o.background = house.bg or "dark"
      pcall(vim.cmd.colorscheme, house.scheme)
      after_scheme()
    end
    return true
  end
  if vim.g.colors_name == e.scheme then
    return true
  end
  vim.o.background = e.bg
  local ok, err = pcall(vim.cmd.colorscheme, e.scheme)
  if not ok then
    vim.notify(("color.mesh: %s is not installed yet -- it installs on the next start (%s)"):format(
      e.scheme, tostring(err):gsub("\n.*", "")), vim.log.levels.WARN)
    return false
  end
  after_scheme()
  return true
end

-- Live: watch the DIRECTORY (color.mesh and the hook replace `current` by rename, which a watch
-- on the file itself would lose), debounced, since one write raises several events.
local function watch()
  local uv = vim.uv or vim.loop
  local w, t = uv.new_fs_event(), uv.new_timer()
  if not w or not t then
    return
  end
  w:start(dir, {}, function(err, fname)
    if err or (fname and not fname:match("^current")) then
      return
    end
    t:stop()
    t:start(150, 0, vim.schedule_wrap(function()
      apply((live()))
    end))
  end)
end

-- One lazy spec per theme plugin. A repo listed twice (catppuccin, monokai-pro) is one plugin.
local specs, seen = {}, {}
for _, e in pairs(map) do
  if e.repo ~= "" and not seen[e.repo] then
    seen[e.repo] = true
    local deps = {}
    for r in e.needs:gmatch("[^,%s]+") do -- hackerman's colors/ requires aether.nvim's module
      deps[#deps + 1] = r
    end
    specs[#specs + 1] = { e.repo, name = e.name ~= "" and e.name or nil, lazy = true, priority = 1000,
      dependencies = #deps > 0 and deps or nil }
  end
end

specs[#specs + 1] = {
  "LazyVim/LazyVim",
  opts = function(_, opts)
    house.scheme = house.scheme or opts.colorscheme
    local e = live()
    if e then
      -- LazyVim accepts a function: set the background first, then the scheme.
      opts.colorscheme = function()
        if not apply(e) and type(house.scheme) == "string" then
          pcall(vim.cmd.colorscheme, house.scheme)
        end
      end
    end
  end,
}

-- After startup: start watching, and win over anything that set a scheme later (nvs-ide's own
-- "Colour scheme" setting runs from its settings file). On the house theme, record what the
-- editor actually ended up on, so switching back restores THAT, including an nvs-ide choice.
vim.api.nvim_create_autocmd("User", {
  pattern = "VeryLazy",
  once = true,
  callback = function()
    local e = live()
    if e then
      apply(e)
    elseif vim.g.colors_name then
      house.scheme, house.bg = vim.g.colors_name, vim.o.background
    end
    watch()
  end,
})

return specs
