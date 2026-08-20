"""Trace the warlock sprite into an ASCII grid, and emit it as Rust and Go.

Source: `Documents\\(f)art\\assets\\darkfantasy\\warlock.png` -- a 42x42
pixel-art sprite whose figure occupies a 15x23 region. A hooded skeleton in a
robe, facing RIGHT, wearing sunglasses and holding a wand.

TWO outputs from ONE trace, deliberately:

  * `panefx/src/warlock_art.rs`  -- the effect panefx renders
  * `slop/pkg/warlock/grid.go`   -- the spinner beside `pkg/skull`

`pkg/skull`'s own doc comment says three drawings of one skull would drift
apart the first time any of them was touched. The same applies here, so the
grid is generated once and written to both places rather than hand-copied.

WHY THE LABELS ARE WHAT THEY ARE
--------------------------------
The sprite has ten colours. A spinning ASCII figure cannot carry ten, and
trying would turn the silhouette to mush at the sizes this renders at. They
collapse to four parts a viewer can actually read while it turns:

    BONE    the skull, white in the sprite -- the face, and the thing the eye
            tracks through the spin
    SHADE   the sunglasses, the one saturated colour and the joke of the piece
    ROBE    every blue of the hood and robe, plus the black shadow inside it
    WAND    the staff and its gem

The outline is NOT taken from the sprite's own dark pixels. Same reasoning as
`gen_skull.py`: those are baked in asymmetrically -- heavy on one side, thin on
another -- which reads as a lopsided smudge once the figure starts turning. It
is DERIVED from the silhouette instead, so it is uniform by construction.
"""
import io
import os

SPRITE = os.path.join(
    os.path.expanduser("~"), "Documents", "(f)art", "assets", "darkfantasy", "warlock.png"
)

# The figure, measured rather than guessed: the alpha bounding box of the
# sprite is exactly x 13..27, y 8..30.
SX, SY, SW, SH = 13, 8, 15, 23

# Outline depth, in cells. Three is what the skull uses: one is a hard line,
# and beyond three the halo starts competing with the figure for attention.
OUTLINE = "*+."
DEPTH = len(OUTLINE)

# Labels. Single bytes so the Rust side can match on them cheaply, and so the
# emitted grid is readable as text in a diff.
AIR = " "
BONE = "#"
SHADE = "="
ROBE = "%"
WAND = "|"


def classify(r, g, b, a):
    """One sprite pixel -> one part, or None for air.

    Ordered most-specific first. The tests that matter:
      - the sunglasses are the ONLY strongly green pixels;
      - the skull is the only near-white;
      - the wand is olive/tan and its gem cyan, both unlike every robe blue.
    """
    if a < 40:
        return None
    # Sunglasses: green dominates both other channels by a wide margin.
    if g > 150 and g > r + 50 and g > b + 50:
        return SHADE
    # Skull: near-white, and the grey highlight beside it.
    if r > 150 and g > 150 and b > 150:
        return BONE
    # Wand: the olive shaft, and the cyan gem.
    if r > 60 and g > 60 and b < 60:
        return WAND
    if b > 150 and g > 150 and r < 150:
        return WAND
    # Everything else -- every blue, and the black inside the hood -- is robe.
    return ROBE


def main():
    from PIL import Image

    im = Image.open(SPRITE).convert("RGBA")
    crop = im.crop((SX, SY, SX + SW, SY + SH))
    px = crop.load()

    # PAD by the outline depth on every side, so the halo has somewhere to go.
    # The crop is tight to the art -- without this the outline simply stops
    # where the figure meets the edge, which is the defect that left the skull
    # ringed on two sides and bare on the others.
    PAD = DEPTH
    GW, GH = SW + PAD * 2, SH + PAD * 2

    solid = [[None] * GW for _ in range(GH)]
    for y in range(SH):
        for x in range(SW):
            part = classify(*px[x, y])
            if part is not None:
                solid[y + PAD][x + PAD] = part

    def dist_to_solid(x, y, limit):
        """Chebyshev distance to the nearest non-air cell, capped at `limit`."""
        for d in range(1, limit + 1):
            for yy in range(y - d, y + d + 1):
                for xx in range(x - d, x + d + 1):
                    if max(abs(yy - y), abs(xx - x)) != d:
                        continue
                    if 0 <= yy < GH and 0 <= xx < GW and solid[yy][xx] is not None:
                        return d
        return None

    rows = []
    counts = {}
    for y in range(GH):
        line = ""
        for x in range(GW):
            v = solid[y][x]
            if v is not None:
                line += v
                counts[v] = counts.get(v, 0) + 1
                continue
            d = dist_to_solid(x, y, DEPTH)
            if d is None:
                line += AIR
                counts[AIR] = counts.get(AIR, 0) + 1
            else:
                ch = OUTLINE[d - 1]
                line += ch
                counts[ch] = counts.get(ch, 0) + 1
        rows.append(line)

    _emit_rust(rows, GW, GH, counts)
    _emit_go(rows, GW, GH)

    for r in rows:
        print("   |" + r + "|")
    print(
        "%dx%d  bone=%d shades=%d robe=%d wand=%d outline=%d"
        % (
            GW,
            GH,
            counts.get(BONE, 0),
            counts.get(SHADE, 0),
            counts.get(ROBE, 0),
            counts.get(WAND, 0),
            sum(counts.get(c, 0) for c in OUTLINE),
        )
    )


def _emit_rust(rows, gw, gh, counts):
    out = [
        "//! The warlock, traced out of the sprite.",
        "//!",
        "//! GENERATED by `gen_warlock.py` from",
        "//! `Documents\\(f)art\\assets\\darkfantasy\\warlock.png`, figure region",
        "//! ({}, {}) {}x{}, padded by {} cells so the outline can close.".format(
            SX, SY, SW, SH, DEPTH
        ),
        "//!",
        "//! A hooded skeleton facing RIGHT, in sunglasses, holding a wand. The",
        "//! sprite's ten colours collapse to four parts, because a spinning ASCII",
        "//! figure cannot carry ten and the attempt turns the silhouette to mush:",
        "//! bone (the skull), shade (the sunglasses), robe (every blue plus the",
        "//! black inside the hood), and wand.",
        "//!",
        "//! Edit `gen_warlock.py` and re-run it; do NOT hand-edit this file.",
        "",
        "/// `#` bone · `=` sunglasses · `%` robe · `|` wand · `*+.` outline · ` ` air",
        "pub const COLS: usize = {};".format(gw),
        "pub const ROWS: usize = {};".format(gh),
        "",
        "pub const WARLOCK: [&str; ROWS] = [",
    ]
    for r in rows:
        out.append('    "{}",'.format(r))
    out.append("];")
    io.open(
        os.path.join("src", "warlock_art.rs"), "w", encoding="utf-8", newline="\n"
    ).write("\n".join(out) + "\n")
    print("src/warlock_art.rs written")


def _emit_go(rows, gw, gh):
    """The same grid for slop, beside pkg/skull.

    Kept in step with the Rust copy by construction: both come from this one
    trace, so neither can drift without the other.
    """
    dest = os.path.join(
        os.path.expanduser("~"), "slop", "pkg", "warlock"
    )
    if not os.path.isdir(dest):
        os.makedirs(dest, exist_ok=True)
    out = [
        "package warlock",
        "",
        "// GENERATED by panefx/gen_warlock.py from",
        "// Documents\\(f)art\\assets\\darkfantasy\\warlock.png. Do NOT hand-edit:",
        "// panefx renders the same grid, and a hand edit here would silently",
        "// desync the two. Re-run the generator instead -- it writes both.",
        "",
        "// Grid legend: '#' bone · '=' sunglasses · '%' robe · '|' wand ·",
        "// '*', '+', '.' outline (densest nearest the figure) · ' ' air.",
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


if __name__ == "__main__":
    main()
