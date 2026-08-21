"""Render the ASCII skull as a still image, into the (f)art folder.

A face-on frame of the spinner -- no spin, no wink (the wink is dead
anyway, see skullspin.rs) -- as both a PNG and a single-frame GIF twin:

  * `Documents\\(f)art\\assets\\darkfantasy\\skull-ascii.png`
  * `Documents\\(f)art\\assets\\darkfantasy\\skull-ascii.gif`

The input is `src/skull_art.rs`, the artifact gen_skull.py ships, so the
render cannot drift from what panefx and slop actually draw. Colours are
Michael's wallpaper config: bone #7d7d7d on black, outline #242424 graded
x1.0 / x0.72 / x0.45 for '*' / '+' / '.' -- the same maths skullspin.rs
applies per cell.

Each glyph is drawn TWICE horizontally (XSCALE=2), because the grid is
authored for terminal cells roughly twice as tall as wide; once would
render an egg. Cells are 16x32 px for the same reason.
"""
import io
import os
import re

from PIL import Image, ImageDraw, ImageFont

ART = os.path.join(os.path.expanduser("~"), "panefx", "src", "skull_art.rs")
FONT = os.path.join(
    os.path.expanduser("~"), "site", "static", "fonts",
    "BigBlueTerm437NerdFontMono-Regular.ttf",
)
DEST = os.path.join(
    os.path.expanduser("~"), "Documents", "(f)art", "assets", "darkfantasy"
)

BG = (0x00, 0x00, 0x00)
BONE = (0x7D, 0x7D, 0x7D)
# '*' '+' '.' at 1.0 / 0.72 / 0.45 of #242424, densest nearest the bone.
DARK = 0x24
COLOURS = {
    "#": BONE,
    "*": (DARK, DARK, DARK),
    "+": tuple(round(DARK * 0.72) for _ in range(3)),
    ".": tuple(round(DARK * 0.45) for _ in range(3)),
}

CELL_W, CELL_H = 16, 32   # 2:1, the terminal cell the art was designed for
XSCALE = 2
MARGIN = CELL_H           # one row of breathing room on every side
FONT_SIZE = 28


def main():
    src = io.open(ART, encoding="utf-8").read()
    rows = re.findall(r'^    "(.*)",$', src, re.M)
    if not rows or len({len(r) for r in rows}) != 1:
        raise SystemExit("could not parse a rectangular SKULL grid out of " + ART)

    cols = len(rows[0])
    w = cols * XSCALE * CELL_W + MARGIN * 2
    h = len(rows) * CELL_H + MARGIN * 2

    img = Image.new("RGB", (w, h), BG)
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, FONT_SIZE)

    for y, row in enumerate(rows):
        for x, ch in enumerate(row):
            colour = COLOURS.get(ch)
            if colour is None:
                continue
            for rep in range(XSCALE):
                cx = MARGIN + (x * XSCALE + rep) * CELL_W + CELL_W / 2
                cy = MARGIN + y * CELL_H + CELL_H / 2
                draw.text((cx, cy), ch, font=font, fill=colour, anchor="mm")

    os.makedirs(DEST, exist_ok=True)
    png = os.path.join(DEST, "skull-ascii.png")
    gif = os.path.join(DEST, "skull-ascii.gif")
    img.save(png)
    # The .gif twin: the same single frame, no animation blocks. 8 colours is
    # already double what the render uses.
    img.convert("P", palette=Image.ADAPTIVE, colors=8).save(gif)
    print("%s  %dx%d" % (png, w, h))
    print("%s  %dx%d" % (gif, w, h))


if __name__ == "__main__":
    main()
