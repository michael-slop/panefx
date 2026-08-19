# -*- coding: utf-8 -*-
"""Turn ACiD/iCE-style CP437 `.ANS` art into panefx shade-level Rust modules.

Same pipeline that produced `src/wizardtorch_art.rs`, with the upstream ANSI
parser folded back in so there is no hand-made intermediate file any more:

    .ANS bytes -> CP437 glyph grid -> ASCII shade stand-ins -> src/<name>_art.rs

The stand-ins are exactly the ones `wizardtorch::shade_of` understands, so a
generated module drops into the same sampler without a table change.
"""

import io
import os
import unicodedata
import sys
from collections import Counter

DOWNLOADS = r'C:/Users/micha/Downloads'
OUT_DIR = r'C:/Users/micha/panefx/src'
WIDTH = 80

# CP437 code point -> ASCII stand-in. Verbatim from gen_art.py's table.
SUB = {
    ' ': ' ',
    u'\u2591': '.',   # light shade
    u'\u2592': ':',   # medium shade
    u'\u2593': '*',   # dark shade
    u'\u2588': '#',   # full block
    u'\u2580': 'T',   # upper half
    u'\u2584': 'B',   # lower half
    u'\u258c': 'L',   # left half
    u'\u2590': 'R',   # right half
}

# Glyphs wizardtorch's table lacks, mapped to the NEAREST shade rather than
# dropped. Ink coverage of the glyph is what picks the stand-in.
#
# The box-drawing and line glyphs are thin strokes -- roughly a light-shade
# worth of ink across the cell -- so they read as '.'; the heavy/solid
# decoratives read darker. Nothing here is silently discarded: the script
# prints every glyph it had to fall back on, with counts.
FALLBACK = {
    # single/double box drawing and line pieces: thin strokes
    u'\u2500': '.', u'\u2502': '.', u'\u250c': '.', u'\u2510': '.',
    u'\u2514': '.', u'\u2518': '.', u'\u251c': '.', u'\u2524': '.',
    u'\u252c': '.', u'\u2534': '.', u'\u253c': '.',
    u'\u2550': ':', u'\u2551': ':', u'\u2554': ':', u'\u2557': ':',
    u'\u255a': ':', u'\u255d': ':', u'\u2560': ':', u'\u2563': ':',
    u'\u2566': ':', u'\u2569': ':', u'\u256c': ':',
    u'\u2552': '.', u'\u2553': '.', u'\u2555': '.', u'\u2556': '.',
    u'\u2558': '.', u'\u2559': '.', u'\u255b': '.', u'\u255c': '.',
    u'\u255e': '.', u'\u255f': '.', u'\u2561': '.', u'\u2562': '.',
    u'\u2564': '.', u'\u2565': '.', u'\u2567': '.', u'\u2568': '.',
    u'\u256a': '.', u'\u256b': '.',
    # solid-ish decoratives
    u'\u25a0': '*',   # small filled square
    u'\u2022': '.', u'\u25cf': ':', u'\u25d8': ':', u'\u25d9': ':',
    u'\u2666': ':', u'\u2663': ':', u'\u2665': ':', u'\u2660': ':',
    u'\u263a': ':', u'\u263b': '*',
    u'\u00b7': '.', u'\u2219': '.',
    u'\u2191': '.', u'\u2193': '.', u'\u2192': '.', u'\u2190': '.',
    u'\u25b2': ':', u'\u25bc': ':', u'\u25ba': ':', u'\u25c4': ':',
    u'\u2195': '.', u'\u2194': '.', u'\u25ac': ':',
    u'\u00a0': ' ',
}


def parse_ans(data, width=WIDTH):
    """CP437 .ANS bytes -> list of unicode rows.

    Handles what these files actually contain and nothing more:
      * CSI `n C`             cursor forward
      * CSI `... m`           SGR colour (consumed; the art is shade-only)
      * CSI `1;r;g;b t`       iCE 24-bit fg  (consumed)
      * CSI `0;r;g;b t`       iCE 24-bit bg  (consumed)
      * CR / LF               carriage return / newline
      * 0x1a                  SAUCE / DOS EOF -- everything after is metadata
      * AUTOWRAP at `width`   ESSENTIAL. Without it these files produce single
                              rows over 1000 columns and no picture at all.
    """
    eof = data.find(b'\x1a')
    if eof != -1:
        data = data[:eof]

    grid = []          # list of list-of-chars
    y = x = 0

    def put(ch):
        while len(grid) <= y:
            grid.append([])
        row = grid[y]
        while len(row) <= x:
            row.append(u' ')
        row[x] = ch

    i, n = 0, len(data)
    while i < n:
        b = data[i]
        if b == 0x1b and i + 1 < n and data[i + 1] == 0x5b:
            j = i + 2
            while j < n and (0x30 <= data[j] <= 0x3f):
                j += 1
            while j < n and (0x20 <= data[j] <= 0x2f):
                j += 1
            if j >= n:
                break
            final = data[j]
            params = data[i + 2:j].decode('latin-1')
            i = j + 1
            if final == 0x43:            # 'C' cursor forward
                try:
                    dx = int(params) if params else 1
                except ValueError:
                    dx = 1
                x += max(dx, 1)
                # A forward past the margin wraps, same as printing would.
                while x >= width:
                    x -= width
                    y += 1
            # 'm' (SGR) and 't' (iCE 24-bit colour) carry no shape: dropped.
            continue
        if b == 0x0d:                    # CR
            x = 0
            i += 1
            continue
        if b == 0x0a:                    # LF
            y += 1
            x = 0
            i += 1
            continue
        if b == 0x09:                    # TAB -> next 8-column stop
            x = min(((x // 8) + 1) * 8, width - 1)
            i += 1
            continue
        # A printable CP437 byte.
        ch = bytes([b]).decode('cp437')
        put(ch)
        x += 1
        if x >= width:                   # 80-column autowrap
            x = 0
            y += 1
        i += 1

    return [u''.join(r) for r in grid]


def to_art(rows, width=WIDTH):
    """Rows of CP437 glyphs -> rows of ASCII stand-ins, plus a fallback report."""
    # Trim trailing blank rows.
    rows = list(rows)
    while rows and rows[-1].strip() == '':
        rows.pop()
    # Trim leading blank rows too -- a leading run of empties is just the
    # file's top margin and wastes art-space in the sampler.
    lead = 0
    while lead < len(rows) and rows[lead].strip() == '':
        lead += 1
    rows = rows[lead:]

    fallbacks = Counter()
    unmapped = Counter()
    out = []
    for r in rows:
        conv = []
        for ch in r.ljust(width)[:width]:
            if ch in SUB:
                conv.append(SUB[ch])
            elif ch in FALLBACK:
                fallbacks[ch] += 1
                conv.append(FALLBACK[ch])
            else:
                # Unknown but real ink: nearest shade by eye is medium.
                unmapped[ch] += 1
                conv.append(':' if ch.strip() else ' ')
        out.append(''.join(conv))
    return out, fallbacks, unmapped


HEADER = u'''//! The `{mod}` artwork, as shade levels.
//!
//! Traced from `{src}` (ACiD-style CP437 block art): {desc}
//!
//! Near enough every glyph in the original is a shade block or a half block,
//! so the drawing reduces to a per-cell shade level. The original's own colour
//! is DISCARDED -- the effect supplies the colour instead.
//!
//! Storing shade levels rather than a finished picture is what lets the
//! animation light the scene: a cell's brightness is its shade times the light
//! reaching it, and the renderer picks the glyph from the result. The drawing
//! never changes -- only how it is lit.
'''


def emit(mod, src, desc, art, path):
    h = len(art)
    lines = [HEADER.format(mod=mod, src=src, desc=desc)]
    lines.append('pub const COLS: usize = %d;' % WIDTH)
    lines.append('pub const ROWS: usize = %d;' % h)
    lines.append('')
    lines.append('/// One row per line, one char per cell:')
    lines.append('///   space = empty, `.:*#` = quarter/half/three-quarter/full shade,')
    lines.append('///   `TBLR` = top/bottom/left/right half blocks.')
    lines.append('///')
    lines.append('/// ASCII stand-ins for the CP437 blocks so the source stays 7-bit and')
    lines.append("/// nothing depends on this file's encoding surviving a checkout.")
    lines.append('pub const ART: [&str; ROWS] = [')
    for r in art:
        lines.append('    "%s",' % r)
    lines.append('];')
    io.open(path, 'w', encoding='utf-8', newline='\n').write(u'\n'.join(lines) + u'\n')
    return h


# Descriptions written after LOOKING at the rendered previews, not guessed from
# the filenames. `ansprev-*.png` next to this script are those renders.
JOBS = [
    ('tgevil_art', 'TG-EVIL.ANS',
     'a horned demon skull inside a stone arch, a third eye burning\n'
     '//! above its brow and a bare fanged jaw below, over a lower panel of long\n'
     '//! molten drips signed `tgFiRE`.'),
    ('wzfire_art', 'WZ-FIRE.ANS',
     'a winged dragon perched on the battlement of a stone tower,\n'
     '//! ribbed body and spread wings filling the frame under blackletter `Fire`\n'
     '//! lettering, with the tower\'s shuttered windows below it.'),
    ('raalien_art', 'ra-alien.ans',
     'a full-length Giger biomechanical xenomorph drawn in NEGATIVE\n'
     '//! -- hatched half-block scanlines carve the creature out of the page rather\n'
     '//! than fill it in -- from the ribbed elongated cranium and inner jaw down\n'
     '//! through the segmented spine, clawed limbs and coiling tail.'),
]


def build(mod, ansname, desc):
    data = io.open(os.path.join(DOWNLOADS, ansname), 'rb').read()
    rows = parse_ans(data)
    art, fallbacks, unmapped = to_art(rows)
    path = os.path.join(OUT_DIR, mod + '.rs')
    h = emit(mod, ansname, desc, art, path)
    print('%-12s -> %s : %d rows x %d cols' % (mod, path, h, WIDTH))
    lit = sum(1 for r in art for c in r if c != ' ')
    print('    lit cells: %d / %d (%.1f%%)' % (lit, h * WIDTH, 100.0 * lit / max(h * WIDTH, 1)))
    if fallbacks:
        print('    FALLBACK GLYPHS (mapped to nearest shade, not dropped):')
        for ch, n in fallbacks.most_common():
            print('      U+%04X (%s) x%d -> %r' % (ord(ch), unicodedata.name(ch, '?'), n, FALLBACK[ch]))
    if unmapped:
        print('    UNMAPPED GLYPHS (no entry at all, forced to nearest shade):')
        for ch, n in unmapped.most_common():
            print('      U+%04X (%s) x%d' % (ord(ch), unicodedata.name(ch, '?'), n))
    if not fallbacks and not unmapped:
        print('    all glyphs mapped losslessly')
    return art


if __name__ == '__main__':
    if '--selftest' in sys.argv:
        # Reproduce the known-good wizardtorch intermediate, proving the parser
        # matches the upstream one gen_art.py was fed.
        data = io.open(os.path.join(DOWNLOADS, 'AXB-WIZARDTORCH.ANS'), 'rb').read()
        mine = parse_ans(data)
        want = io.open(os.path.join(DOWNLOADS, 'art_rows.txt'), encoding='utf-8').read().split('\n')
        while want and want[-1] == '':
            want.pop()
        while mine and mine[-1].strip() == '':
            mine.pop()
        ok = True
        if len(mine) != len(want):
            print('ROW COUNT %d != %d' % (len(mine), len(want)))
            ok = False
        for i, (a, b) in enumerate(zip(mine, want)):
            if a.rstrip() != b.rstrip():
                print('row %d differs:\n  mine %r\n  want %r' % (i, a, b))
                ok = False
                if i > 3:
                    break
        print('SELFTEST', 'PASS' if ok else 'FAIL')
        sys.exit(0 if ok else 1)

    for mod, ansname, desc in JOBS:
        build(mod, ansname, desc)
