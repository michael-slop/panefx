panefx — animated ASCII backdrops behind your windows, and on your desktop
==========================================================================

Thanks for testing this. It is a beta: it works, but you are looking at it
because things may still be wrong.


WHAT IT DOES
------------

Two separate things, and you can run either without the other:

  * WALLPAPER   an animated effect drawn on the desktop, behind your icons.
  * BACKDROPS   the same kind of effect drawn behind each terminal window,
                showing through it. This one needs a see-through terminal --
                panefx draws BEHIND the window, so an opaque window hides it
                completely. Alacritty and Neovide are supported out of the box.

Windows only. It uses DirectComposition and the desktop's WorkerW layer, so
there is no Linux or macOS build and there will not be one.


INSTALLING
----------

Double-click INSTALL.exe.

It checks what is present, tells you what is missing and why it matters, copies
the programs into your user folder, registers the bundled font, and sets panefx
to start when you log in.

Nothing is written outside your own user profile. No service, no driver, no
admin prompt.


USING IT
--------

    panefx           the control panel
    panefx --tui     the same thing in a terminal, if you prefer
    panefx --daemon  the background process (the installer starts this for you)

The control panel is also on the tray icon. Right-click it for Reload and Exit.

Everything is live: change an effect or a colour and it happens immediately.
Settings are saved to

    %USERPROFILE%\.config\panefx\config.toml

which is a plain text file you can read, edit and delete. Deleting it resets
everything to defaults.


THEMES
------

The "themes" tab (or the tray's Theme menu, or `panefx theme list`) holds 27
colour themes. Pick one and panefx recolours its own effects and window, then
every app it finds: Alacritty, Neovim/Neovide (with extras\nvim installed),
Windows light/dark mode, VS Code, Windows Terminal (choose the `panefx` scheme
once) and Xournal++. The tab lists each app, what the last change did to it,
and a box to leave it alone.

"house" puts everything back exactly as it was before your first theme --
that moment is recorded in %USERPROFILE%\.config\panefx\house.json.

    panefx theme tokyo-night      switch from a terminal
    panefx theme next             cycle (also: prev, house)


TURNING PARTS OFF
-----------------

  * Backdrops only:  settings tab -> "backdrops" -> off.
                     Your terminal goes solid again automatically.
  * Wallpaper only:  wallpaper tab -> pick the monitor -> effect "off".
                     That monitor goes back to your normal Windows wallpaper
                     and costs nothing.

They are independent. Turning one off never affects the other.


ABOUT YOUR WINDOW MANAGER
-------------------------

panefx does not need one. It reads window positions from Windows directly.

If you happen to run GlazeWM it will use that instead, which additionally
understands workspaces. You do not need to install it, and INSTALL.exe lists it
as optional for exactly that reason.


IF SOMETHING IS WRONG
---------------------

Run REPORT.exe and send the output. It prints what panefx thinks is happening:
which windows it found, what each monitor is doing, and the recent log.

Known things worth checking first:

  * Nothing appears behind the terminal
        Your terminal is opaque. panefx draws behind it, so it needs
        transparency. In Alacritty that is `opacity` in alacritty.toml --
        panefx sets it for you, so give it a couple of seconds to reload.

  * The wrong font, or blocks instead of characters
        The bundled font did not register. Right-click
        BigBlueTerm437NerdFontMono-Regular.ttf and Install, then restart panefx.

  * Your antivirus removed it
        This is an unsigned binary built by one person, and heuristic scanners
        do flag those. If panefx vanishes after install, check your antivirus
        quarantine before reinstalling.

  * It stops animating when a window covers the screen
        That is deliberate, and it is saving your battery. The threshold is
        "freeze at %" in the settings tab.


LICENCE
-------

See LICENSE.
