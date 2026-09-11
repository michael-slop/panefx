#!/usr/bin/env python3
"""Trace Michael's necronomicon.png into the ASCII grids the mesh renders.

Companion to gen_skull.py, and deliberately the same shape: ONE trace of ONE
source image writes every consumer, so several drawings of one mark cannot
drift apart the first time any of them is touched.

    python gen_book.py

Writes:
    slop/pkg/book/grid.go          the 29x25 hero grid (Go)
    slop/pkg/book/logo_gen.go      the 16x6 logo mask (Go)

SOURCE: Documents/(f)art/assetset1_darkfantasy/necronomicon.png, 64x64 RGBA,
drawn by Michael. Copied to slop/pkg/book/necronomicon.png so the generator
has a source inside the repo -- the laptop is not always reachable.

WHY A TRACE AND NOT A DOWNSAMPLE
The art is 3,078 opaque pixels carrying 2,892 distinct colours: an
anti-aliased illustration, not flat pixel art. Averaging it into a 29-wide
grid produces mush, exactly as pkg/skull/logo.go records for the six-row
skull ("the eye sockets landed asymmetrically and the face read as a blob").
So this quantises through the ALPHA silhouette for shape and a
contrast-normalised luminance for interior detail, then snaps to four
levels -- the same legend gen_skull.py uses.

THE ROTATION CAVEAT, WHICH IS WHY THE BOOK ROCKS RATHER THAN SPINS
A skull is roughly symmetric, so an inverse-cosine map through its mask reads
as a head turning. A book has a FRONT COVER and no drawn back: rotating it
past edge-on would show a face that does not exist. panefx's warlockspin hit
the identical wall and rocks instead ("it faces right, so a full turn would
show a back the sprite lacks"). BookFrame therefore takes the same cosine the
skull does -- so every caller's tick loop is unchanged -- but the renderer
clamps it to a shallow rock. See pkg/book/book.go.
"""

import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SLOP = os.path.join(os.path.dirname(HERE), "slop")
SRC = os.path.join(SLOP, "pkg", "book", "necronomicon.png")

# Grid legend, identical to gen_skull.py's so a reader who knows one knows
# both: '#' the cover · '*' '+' '.' the shading, densest nearest the cover ·
# ' ' air.
LEVELS = " .+*#"

GRID_W, GRID_H = 29, 25
LOGO_W, LOGO_H = 16, 6


def read_png(path):
    """Minimal RGBA PNG reader -- no Pillow, so this runs on a bare box."""
    d = open(path, "rb").read()
    i, idat, w, h = 8, b"", 0, 0
    while i < len(d):
        ln = struct.unpack(">I", d[i:i + 4])[0]
        typ = d[i + 4:i + 8]
        if typ == b"IHDR":
            w, h = struct.unpack(">II", d[i + 8:i + 16])
            # IHDR payload: width(4) height(4) bitdepth(1) colortype(1), so
            # the two flags sit at +16 and +17 FROM THE CHUNK, not at the
            # absolute 24/25 that only happen to match for the first chunk.
            if d[i + 16] != 8 or d[i + 17] != 6:
                raise SystemExit("expected 8-bit RGBA, got depth %d type %d"
                                 % (d[i + 16], d[i + 17]))
        elif typ == b"IDAT":
            idat += d[i + 8:i + 8 + ln]
        i += 12 + ln
    raw = zlib.decompress(idat)
    stride, out, prev, pos = w * 4, [], bytearray(w * 4), 0
    for _ in range(h):
        f = raw[pos]; pos += 1
        line = bytearray(raw[pos:pos + stride]); pos += stride
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b = prev[x]
            c = prev[x - 4] if x >= 4 else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        out.append(bytes(line)); prev = line
    return w, h, out


def sample(px, w, h, gw, gh):
    """Box-average each target cell over its source rectangle.

    Returns (coverage, luminance) per cell. Coverage is the fraction of the
    box that is opaque -- that is the SHAPE, and it is what decides air vs
    ink. Luminance is averaged over the opaque pixels only, so a mostly
    transparent cell is not dragged dark by the pixels that are not there.
    """
    cov = [[0.0] * gw for _ in range(gh)]
    lum = [[0.0] * gw for _ in range(gh)]
    for gy in range(gh):
        y0, y1 = gy * h // gh, max(gy * h // gh + 1, (gy + 1) * h // gh)
        for gx in range(gw):
            x0, x1 = gx * w // gw, max(gx * w // gw + 1, (gx + 1) * w // gw)
            n = op = 0
            acc = 0.0
            for y in range(y0, y1):
                for x in range(x0, x1):
                    i = (y * w + x) * 4
                    r, g, b, a = px[y][x * 4:x * 4 + 4]
                    n += 1
                    if a > 128:
                        op += 1
                        acc += 0.2126 * r + 0.7152 * g + 0.0722 * b
            cov[gy][gx] = op / n if n else 0.0
            lum[gy][gx] = acc / op if op else 0.0
    return cov, lum


def quantise(cov, lum, gw, gh, floor):
    """Coverage decides ink; normalised luminance picks the glyph.

    The art is very dark (p2..p98 spans about 4..166 of 255), so a raw
    luminance ramp collapses to one glyph. Percentile-normalising against the
    INKED cells only is what makes the cover's face readable at 29 columns.
    """
    vals = sorted(lum[y][x] for y in range(gh) for x in range(gw)
                  if cov[y][x] >= floor and lum[y][x] > 0)
    if not vals:
        raise SystemExit("nothing inked -- check the coverage floor")
    lo = vals[int(len(vals) * 0.05)]
    hi = vals[int(len(vals) * 0.95)]
    rows = []
    for y in range(gh):
        row = ""
        for x in range(gw):
            if cov[y][x] < floor:
                row += " "
                continue
            t = (lum[y][x] - lo) / (hi - lo) if hi > lo else 0.5
            t = 0.0 if t < 0 else (1.0 if t > 1 else t)
            # 1..4 -- an inked cell is never ' ', or the silhouette gains
            # holes the drawing does not have.
            row += LEVELS[1 + int(t * 3.999)]
        rows.append(row)
    return rows


def go_lines(rows):
    return "\n".join('\t"%s",' % r for r in rows)


def main():
    w, h, px = read_png(SRC)
    print("source %dx%d" % (w, h))

    cov, lum = sample(px, w, h, GRID_W, GRID_H)
    grid = quantise(cov, lum, GRID_W, GRID_H, floor=0.45)

    lcov, llum = sample(px, w, h, LOGO_W, LOGO_H)
    # A lower floor at six rows: each cell spans ~10 source rows, so the
    # book's sloped top edge never fills a cell and a 0.45 floor erases the
    # whole first row.
    logo = quantise(lcov, llum, LOGO_W, LOGO_H, floor=0.30)

    print("\nhero %dx%d:" % (GRID_W, GRID_H))
    for r in grid:
        print("  |%s|" % r)
    print("\nlogo %dx%d:" % (LOGO_W, LOGO_H))
    for r in logo:
        print("  |%s|" % r)

    os.makedirs(os.path.join(SLOP, "pkg", "book"), exist_ok=True)

    header = ('// GENERATED by panefx/gen_book.py from\n'
              '// pkg/book/necronomicon.png (64x64 RGBA, drawn by Michael).\n'
              '// Do NOT hand-edit -- re-run the generator, which writes every\n'
              '// grid from one trace so the drawings cannot drift apart.\n'
              '//\n'
              '// Legend: \'#\' cover \xc2\xb7 \'*\', \'+\', \'.\' shading (densest nearest\n'
              '// the cover) \xc2\xb7 \' \' air.\n')

    with open(os.path.join(SLOP, "pkg", "book", "grid.go"), "w",
              newline="\n", encoding="utf-8") as f:
        f.write("package book\n\n")
        f.write(header.encode("latin-1").decode("utf-8"))
        f.write("\nconst (\n\tgridW = %d\n\tgridH = %d\n)\n\n" % (GRID_W, GRID_H))
        f.write("var grid = [gridH]string{\n%s\n}\n" % go_lines(grid))

    with open(os.path.join(SLOP, "pkg", "book", "logo_gen.go"), "w",
              newline="\n", encoding="utf-8") as f:
        f.write("package book\n\n")
        f.write(header.encode("latin-1").decode("utf-8"))
        f.write("\nconst (\n\tlogoW = %d\n\tlogoH = %d\n)\n\n" % (LOGO_W, LOGO_H))
        f.write("var logoGrid = [logoH]string{\n%s\n}\n" % go_lines(logo))

    print("\nwrote pkg/book/grid.go and pkg/book/logo_gen.go")


if __name__ == "__main__":
    main()
