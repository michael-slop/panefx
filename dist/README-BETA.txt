================================================================
  panefx - beta
  animated ASCII backdrops behind windows, and on the desktop
================================================================

Thanks for testing this. It is a beta: it works on the machines it has been
run on, and the point of you having it is to find the ones where it does not.


----------------------------------------------------------------
  START HERE
----------------------------------------------------------------

  1. Extract the WHOLE zip to a folder. Do not run anything from
     inside the zip window -- the installer needs the other files
     next to it.

  2. Double-click  INSTALL.exe

     It checks what your machine has, tells you what is missing and
     what each missing thing costs you, and asks before installing
     anything. Nothing is downloaded unless you say yes.

  3. When it finishes, panefx is running. Look for the FX icon in
     your system tray (bottom-right, possibly under the "^" arrow).

  4. Click that icon to open the control panel.

Windows may show a blue "Windows protected your PC" box, because these
programs are not code-signed. Click "More info" then "Run anyway". If you
would rather not, that is a completely reasonable place to stop -- tell me
and I will find another way to get it to you.


----------------------------------------------------------------
  WHAT IT NEEDS
----------------------------------------------------------------

  Windows 10 or 11        required
  GlazeWM                 needed ONLY for the effects behind windows.
                          panefx reads window positions from GlazeWM;
                          with no GlazeWM those do nothing. The desktop
                          wallpaper effects work fine without it.
  BigBlueTerm437 font     ships in this folder; the installer sets it
                          up. Without it Windows silently substitutes
                          another typeface and every effect looks wrong
                          -- with no error message anywhere.
  Alacritty               optional, only for the terminal effect.

INSTALL.exe checks all four and offers to install what is missing.


----------------------------------------------------------------
  USING IT
----------------------------------------------------------------

  panefx                 open the control panel  (or click the tray icon)
  panefx --tui           the terminal version, for SSH
  panefx --daemon        start the background daemon by hand
  panefx --help

In the control panel:

  wallpaper tab   pick a monitor on the left, then an effect. Effects
                  STACK in layers -- set a base effect, then "add a layer
                  above" to put something on top of it (flames
                  underneath, a spinning skull over them).
  TUI-fx tab      the effect drawn behind terminal windows. Needs GlazeWM.
  logs tab        what the daemon has been doing.
  copy to...      apply one monitor's setup to the others.
  dark / light    your choice is remembered.

Buttons are Windows 98 style: raised when idle, pushed in when selected.


----------------------------------------------------------------
  WHEN SOMETHING GOES WRONG   <-- the part I actually need
----------------------------------------------------------------

  Double-click  REPORT.exe

It writes a file to your Desktop called  panefx-report-<date>.txt

Send me that file. It contains what panefx thinks is happening, its recent
log, your display layout and refresh rates, and your settings. It is plain
text -- open it and read it first if you like.

It does NOT send anything anywhere. It writes one local file and stops.

Please also say, in your own words:
  - what you were doing
  - what you expected
  - what happened instead
  - which monitor, if it is only one of them

Screenshots or a phone video of flickering are genuinely useful. Flicker is
hard to describe and obvious to see.


----------------------------------------------------------------
  KNOWN ROUGH EDGES
----------------------------------------------------------------

  - The effect behind windows needs GlazeWM running. Not installed, or
    installed but not started, means nothing appears there. Expected,
    not a bug.
  - Multi-monitor setups are where the interesting bugs live. Mixed
    refresh rates, rotated screens and mixed resolutions especially --
    please try those if you have them.
  - If the wallpaper is torn, flickering, or black, that is worth a
    report even if it fixes itself.
  - A crash leaves no dialog: the window just vanishes. Run REPORT.exe
    anyway -- the daemon's log usually survives it.
  - The control panel has no live preview yet. You see an effect by
    applying it.


----------------------------------------------------------------
  UNINSTALLING
----------------------------------------------------------------

  1. Right-click the tray icon -> Exit
  2. Delete these three files from  %USERPROFILE%\bin\
       panefx.exe   panefx-gui.exe   panefx-ctl.exe
  3. Settings live in  %USERPROFILE%\.config\panefx\  -- delete that
     folder to remove them.

Nothing is installed to Program Files, nothing runs as administrator, and
nothing is added to your startup unless GlazeWM was already launching it.


----------------------------------------------------------------
  ATTRIBUTION
----------------------------------------------------------------

BigBlueTerm437 Nerd Font Mono - CC BY-SA 4.0
  VileR's Ultimate Oldschool PC Font Pack, via Nerd Fonts.
  https://int10h.org/oldschool-pc-fonts/
  https://www.nerdfonts.com/

The Windows 98 interface is a tribute to a look, not anyone's code.
