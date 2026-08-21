"""Trace the michael.slop skull out of the mascot sprite, and emit it three ways.

Mirrors the classification `site/static/app.js:initSkullSpin` does at runtime:
crop the skull region of the 42x42 sprite, then label every pixel

    0 = air     (alpha < 40)
    1 = bone    (opaque, bright)
    2 = dark    (opaque, dim -- outline, eye sockets, nose, tooth gaps)

Storing the LABELS rather than pixels is what lets the effect light and spin the
skull: `2` must stay dark at every angle or the face stops reading, and that is
a property of the drawing, not of any one frame.

THREE outputs from ONE trace, deliberately (the gen_warlock.py shape):

  * `panefx/src/skull_art.rs`            -- the wallpaper effect panefx renders
  * `slop/pkg/skull/grid.go`             -- the shared Go spinner
  * `slop/pkg/slopui/web/skull_grid.js`  -- the shared web module

`pkg/skull`'s own doc comment says several drawings of one skull would drift
apart the first time any of them was touched. So the grid is generated once and
written to all three places rather than hand-copied.
"""
import io
import os

from PIL import Image

SPRITE = os.path.join(os.path.expanduser("~"), "site", "static", "icons", "skeleton.png")

# The skull region, straight from app.js. Not re-derived: the site and panefx
# must show the same face, and eyeballing a second crop is how they drift.
# Height 19, not app.js's 21: the last two rows are the NECK stub below the
# teeth, and Michael wants the head alone -- a cleaner silhouette that also
# stops the jump lifting a floating neck around. The X region is unchanged, so
# the face still matches the site exactly.
# X starts one column further in and the width drops by three: one column of
# dark outline padding off the front, two off the back. Michael's ask, and
# measured safe before cutting -- the trace has 3 bone-free columns on the left
# and 2 on the right, so nothing of the drawing is lost.
#
# Column 23 of the OLD crop carries teeth, which is why the right trim is two
# and not three: one more would have cut into the jaw.
SX, SY, SW, SH = 9, 0, 23, 19

# PAD the traced grid by the outline depth on every side.
#
# The crop is tight to the art -- bone runs to the bottom and right edges. With
# no margin the outline has nowhere to go there, so the halo simply stopped:
# measured, 42 bone cells sat against air-or-edge with no outline cell beyond
# them. The skull was ringed on the top and left and bare underneath.
#
# Padding is done on the GRID rather than by widening the crop, because the
# sprite has no spare pixels there either -- widening would pull in whatever
# sits next to the skull in the source image.
PAD = 3

# The outline ramp, densest nearest the bone. Real ASCII rather than block
# shades: the skull itself is drawn in blocks, so an ASCII edge separates the
# silhouette from its surround instead of blending into it.
#
# Three cells deep -- one is a hard line, and beyond three the halo starts
# competing with the face for attention.
OUTLINE = "*+."
DEPTH = len(OUTLINE)


def main():
    im = Image.open(SPRITE).convert("RGBA")
    crop = im.crop((SX, SY, SX + SW, SY + SH))
    px = crop.load()

    # --- classify ---------------------------------------------------------
    #
    # TWO passes, and the second is the point.
    #
    # The sprite's own dark pixels are NOT used as the outline. They are baked
    # into the art asymmetrically -- heavy under the bottom-right, thin across
    # the front -- which reads as a lopsided smudge rather than a drawn edge
    # once the skull starts turning. So they are discarded, and the outline is
    # DERIVED from the bone shape instead: every empty cell touching bone
    # becomes outline, at a depth measured in cells from the edge.
    #
    # Derived means uniform by construction. There is no way for one side to
    # end up heavier than another, because the rule does not know which side
    # it is on.
    GW, GH = SW + PAD * 2, SH + PAD * 2

    bone = [[False] * GW for _ in range(GH)]
    for y in range(SH):
        for x in range(SW):
            r, g, b, a = px[x, y]
            # Opaque AND bright is bone. Everything else -- transparent, or
            # the sprite's own dark edge -- is treated as empty and
            # re-outlined below.
            bone[y + PAD][x + PAD] = a >= 40 and (r + g + b) >= 384

    def dist_to_bone(x, y, limit):
        """Chebyshev distance from (x, y) to the nearest bone cell, capped."""
        for d in range(1, limit + 1):
            for yy in range(y - d, y + d + 1):
                for xx in range(x - d, x + d + 1):
                    # Only the ring at exactly distance d.
                    if max(abs(yy - y), abs(xx - x)) != d:
                        continue
                    if 0 <= yy < GH and 0 <= xx < GW and bone[yy][xx]:
                        return d
        return None

    rows = []
    counts = {"bone": 0, "outline": 0, "air": 0}
    for y in range(GH):
        line = ""
        for x in range(GW):
            if bone[y][x]:
                line += "#"
                counts["bone"] += 1
                continue
            d = dist_to_bone(x, y, DEPTH)
            if d is None:
                line += " "
                counts["air"] += 1
            else:
                line += OUTLINE[d - 1]
                counts["outline"] += 1
        rows.append(line)

    _emit_rust(rows, GW, GH, counts)
    _emit_go(rows, GW, GH)
    _emit_js(rows, GW, GH)

    for r in rows:
        print("   |" + r + "|")
    print("{}x{}  bone={} outline={} air={}".format(
        GW, GH, counts["bone"], counts["outline"], counts["air"]))


def _emit_rust(rows, gw, gh, counts):
    out = []
    out.append("//! The michael.slop skull, traced out of the mascot sprite.")
    out.append("//!")
    out.append("//! GENERATED by `gen_skull.py` from")
    out.append("//! `site/static/icons/skeleton.png`, region ({}, {}) {}x{}.".format(SX, SY, SW, SH))
    out.append("//!")
    out.append("//! The X region is app.js's, less one column of dark padding at the front")
    out.append("//! and two at the back -- a tighter silhouette, with no bone lost (measured:")
    out.append("//! the old trace had 3 bone-free columns left and 2 right). The")
    out.append("//! HEIGHT is two rows shorter on purpose: app.js takes 21 rows, of which")
    out.append("//! the last two are the neck stub below the teeth. Head alone is the")
    out.append("//! cleaner silhouette, and it stops the jump lifting a floating neck.")
    out.append("//!")
    out.append("//! Cells are LABELS, not pixels:")
    out.append("//!")
    out.append("//!   ` ` air   `#` bone   `*` `+` `.` outline, densest nearest the bone")
    out.append("//!")
    out.append("//! The outline is DERIVED, not traced. The sprite\'s own dark pixels are")
    out.append("//! baked in asymmetrically -- heavy under the bottom-right, thin across the")
    out.append("//! front -- which reads as a lopsided smudge once the skull turns. They are")
    out.append("//! discarded and the edge is regenerated from the bone shape, so it cannot")
    out.append("//! be heavier on one side: the rule does not know which side it is on.")
    out.append("//!")
    out.append("//! Outline cells must stay DARK at every angle and light level, or the")
    out.append("//! sockets fill in and the face stops reading as a face.")
    out.append("")
    out.append("pub const COLS: usize = {};".format(gw))
    out.append("pub const ROWS: usize = {};".format(gh))
    out.append("")
    out.append("/// One row per line. See the module header for the alphabet.")
    out.append("pub const SKULL: [&str; ROWS] = [")
    for r in rows:
        out.append('    "{}",'.format(r))
    out.append("];")
    dest = os.path.join(os.path.expanduser("~"), "panefx", "src", "skull_art.rs")
    io.open(dest, "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
    print("%s written" % dest)


def _emit_go(rows, gw, gh):
    """The same grid for slop's shared spinner.

    Kept in step with the Rust copy by construction: both come from this one
    trace, so neither can drift without the other.
    """
    dest = os.path.join(os.path.expanduser("~"), "slop", "pkg", "skull")
    out = [
        "package skull",
        "",
        "// GENERATED by panefx/gen_skull.py from",
        "// site/static/icons/skeleton.png, region ({}, {}) {}x{}, padded by".format(SX, SY, SW, SH),
        "// {} cells so the outline can close. Do NOT hand-edit: panefx and the".format(PAD),
        "// web module render the same grid, and a hand edit here would silently",
        "// desync them. Re-run the generator instead -- it writes all three.",
        "",
        "// Grid legend: '#' bone · '*', '+', '.' outline (densest nearest the",
        "// bone) · ' ' air. The outline is DERIVED from the bone shape, not",
        "// traced from the sprite's own dark pixels -- see the generator.",
        "const (",
        "\tgridW = {}".format(gw),
        "\tgridH = {}".format(gh),
        ")",
        "",
        "var grid = [gridH]string{",
    ]
    for r in rows:
        out.append('\t"{}",'.format(r))
    out.append("}")
    io.open(
        os.path.join(dest, "grid.go"), "w", encoding="utf-8", newline="\n"
    ).write("\n".join(out) + "\n")
    print("%s written" % os.path.join(dest, "grid.go"))


def _emit_js(rows, gw, gh):
    """The same grid for the shared web module beside slop.js."""
    dest = os.path.join(
        os.path.expanduser("~"), "slop", "pkg", "slopui", "web"
    )
    out = [
        "// GENERATED by panefx/gen_skull.py from",
        "// site/static/icons/skeleton.png, region ({}, {}) {}x{}, padded by".format(SX, SY, SW, SH),
        "// {} cells so the outline can close. Do NOT hand-edit: panefx and".format(PAD),
        "// pkg/skull render the same grid, and a hand edit here would silently",
        "// desync them. Re-run the generator instead -- it writes all three.",
        "//",
        "// Legend: '#' bone · '*' '+' '.' outline (densest nearest the bone) ·",
        "// ' ' air.",
        "export const COLS = {}, ROWS = {};".format(gw, gh),
        "",
        "export const SKULL = [",
    ]
    for r in rows:
        out.append('  "{}",'.format(r))
    out.append("];")
    io.open(
        os.path.join(dest, "skull_grid.js"), "w", encoding="utf-8", newline="\n"
    ).write("\n".join(out) + "\n")
    print("%s written" % os.path.join(dest, "skull_grid.js"))


if __name__ == "__main__":
    main()
