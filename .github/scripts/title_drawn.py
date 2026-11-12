#!/usr/bin/env python3
"""Fails unless a packaged game's capture of its title is a drawn frame (M4 S7).

`packages.yml` launches each desktop package with `--capture` and keeps the PNG. A frame the GPU never drew
comes back blank, one colour, or with only the menu over an empty clear or a gradient: none of those proves
the package draws. The title is the town behind the menu, so a drawn frame has detail across the image. This
cuts it into a 4x4 grid of cells and fails when more than three are flat (their pixels differ from their
neighbours by under half a code on average), or when the whole has too few colours.

Standard library only: it runs on the hosted runners' Python on Linux, macOS and Windows. It reads 8-bit
RGB or RGBA PNGs without interlacing, as the game writes them.
"""

import argparse
import struct
import sys
import zlib

SIGNATURE = b"\x89PNG\r\n\x1a\n"
# Cells across and down.
GRID = 4
# A cell whose pixels differ from their right and lower neighbours' luma by less than this on average (in 8-bit
# codes, the two differences summed) is flat. Every cell of the title is 2.0 or more on Metal and WARP
# (83dc6273's packages), and of the town's and the ward's lavapipe goldens 2.1 or more; the menu's goldens over
# their gradient backdrop are 0.03 to 0.04 away from the menu.
FLAT_DETAIL = 0.5
# At most this many cells may be flat. The title has none; the menu alone leaves the right half's and the top
# row's cells flat, five or more.
MAX_FLAT_CELLS = 3
# At least this many distinct colours. The title has about 28,000 on Metal and WARP, the lavapipe town
# goldens 30,000 to 50,000; a cleared frame has one, and the menu over a gradient 1,400 to 1,800.
MIN_COLOURS = 4096


def read_png(data):
    """Returns (width, height, rows), rows a list of bytes objects of RGB triples."""
    if not data.startswith(SIGNATURE):
        raise ValueError("not a PNG")
    at = len(SIGNATURE)
    header = None
    compressed = bytearray()
    while at + 8 <= len(data):
        length, kind = struct.unpack(">I4s", data[at:at + 8])
        body = data[at + 8:at + 8 + length]
        at += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            compressed += body
        elif kind == b"IEND":
            break
    if header is None:
        raise ValueError("no IHDR")
    width, height, depth, colour, _, _, interlace = header
    if depth != 8 or colour not in (2, 6) or interlace != 0:
        raise ValueError(f"only 8-bit RGB or RGBA without interlacing is read, not depth {depth} colour {colour}")
    channels = 3 if colour == 2 else 4
    stride = width * channels
    raw = zlib.decompress(bytes(compressed))
    if len(raw) != height * (stride + 1):
        raise ValueError("the image data is the wrong length")
    rows = []
    previous = bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1:start + 1 + stride])
        unfilter(kind, line, previous, channels)
        rgb = line if channels == 3 else bytes(b for x, b in enumerate(line) if x % 4 != 3)
        rows.append(bytes(rgb))
        previous = line
    return width, height, rows


def unfilter(kind, line, previous, bpp):
    if kind == 0:
        return
    for x in range(len(line)):
        left = line[x - bpp] if x >= bpp else 0
        up = previous[x]
        if kind == 1:
            line[x] = (line[x] + left) & 0xFF
        elif kind == 2:
            line[x] = (line[x] + up) & 0xFF
        elif kind == 3:
            line[x] = (line[x] + ((left + up) >> 1)) & 0xFF
        elif kind == 4:
            corner = previous[x - bpp] if x >= bpp else 0
            p = left + up - corner
            pa, pb, pc = abs(p - left), abs(p - up), abs(p - corner)
            predicted = left if pa <= pb and pa <= pc else up if pb <= pc else corner
            line[x] = (line[x] + predicted) & 0xFF
        else:
            raise ValueError(f"unknown filter {kind}")


def judge(width, height, rows):
    """Returns (flat cells, cells, distinct colours, problems)."""
    colours = set()
    lumas = []
    for row in rows:
        lumas.append([0.2126 * row[3 * x] + 0.7152 * row[3 * x + 1] + 0.0722 * row[3 * x + 2] for x in range(width)])
        colours.update((row[3 * x] << 16) | (row[3 * x + 1] << 8) | row[3 * x + 2] for x in range(width))
    # Each cell's summed luma steps to the right and below, and how many pixels have both neighbours.
    sums = [[0.0, 0] for _ in range(GRID * GRID)]
    columns = [min(x * GRID // width, GRID - 1) for x in range(width)]
    for y in range(height - 1):
        here, below = lumas[y], lumas[y + 1]
        cells = sums[min(y * GRID // height, GRID - 1) * GRID:][:GRID]
        for x in range(width - 1):
            cell = cells[columns[x]]
            cell[0] += abs(here[x + 1] - here[x]) + abs(below[x] - here[x])
            cell[1] += 1
    flat = sum(total / max(count, 1) < FLAT_DETAIL for total, count in sums)
    problems = []
    if flat > MAX_FLAT_CELLS:
        problems.append(f"{flat} of {GRID * GRID} cells are flat (luma steps under {FLAT_DETAIL} codes)")
    if len(colours) < MIN_COLOURS:
        problems.append(f"only {len(colours)} colours, under {MIN_COLOURS}")
    return flat, GRID * GRID, len(colours), problems


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("png", nargs="+", help="captures to check")
    args = parser.parse_args(argv)
    failed = False
    for path in args.png:
        with open(path, "rb") as stream:
            width, height, rows = read_png(stream.read())
        flat, cells, colours, problems = judge(width, height, rows)
        verdict = "not drawn: " + "; ".join(problems) if problems else "drawn"
        print(f"{path}: {width}x{height}, {flat}/{cells} cells flat, {colours} colours: {verdict}")
        failed |= bool(problems)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
