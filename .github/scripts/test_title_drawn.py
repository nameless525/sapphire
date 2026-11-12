"""A packaged game's title capture must be a drawn frame: blank, one colour or the menu alone fail (M4 S7)."""

from contextlib import redirect_stdout
import io
from pathlib import Path
import struct
import tempfile
import unittest
import zlib

import title_drawn

WIDTH, HEIGHT = 128, 72


def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)


def filtered(kind, line, previous, bpp):
    """Filters a row as an encoder would, so reading it back checks the reader's unfiltering."""
    out = bytearray(len(line))
    for x, value in enumerate(line):
        left = line[x - bpp] if x >= bpp else 0
        up = previous[x]
        corner = previous[x - bpp] if x >= bpp else 0
        if kind == 0:
            predicted = 0
        elif kind == 1:
            predicted = left
        elif kind == 2:
            predicted = up
        elif kind == 3:
            predicted = (left + up) >> 1
        else:
            p = left + up - corner
            pa, pb, pc = abs(p - left), abs(p - up), abs(p - corner)
            predicted = left if pa <= pb and pa <= pc else up if pb <= pc else corner
        out[x] = (value - predicted) & 0xFF
    return bytes(out)


def png(pixel, alpha=False):
    """An 8-bit PNG of WIDTH x HEIGHT, `pixel(x, y)` an RGB triple, each row with the next of the five filters."""
    channels = 4 if alpha else 3
    raw = bytearray()
    previous = bytes(WIDTH * channels)
    for y in range(HEIGHT):
        line = bytearray()
        for x in range(WIDTH):
            line += bytes(pixel(x, y)) + (b"\xff" if alpha else b"")
        raw.append(y % 5)
        raw += filtered(y % 5, line, previous, channels)
        previous = bytes(line)
    header = struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6 if alpha else 2, 0, 0, 0)
    return title_drawn.SIGNATURE + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(raw))) + chunk(b"IEND", b"")


def scene(x, y):
    """Detail everywhere, as the town behind the menu: every pixel its own colour."""
    return ((x * 37 + y * 11) % 256, (x * 5 + y * 53) % 256, (x * y + 3 * x) % 256)


class TitleDrawnTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.enterContext(redirect_stdout(io.StringIO()))

    def check(self, data):
        path = Path(self.temp.name) / "title.png"
        path.write_bytes(data)
        return title_drawn.main([str(path)])

    def test_a_drawn_frame_passes_in_rgb_and_rgba(self):
        self.assertEqual(self.check(png(scene)), 0)
        self.assertEqual(self.check(png(scene, alpha=True)), 0)
        rgb = title_drawn.read_png(png(scene))
        self.assertEqual(rgb, title_drawn.read_png(png(scene, alpha=True)))
        width, height, rows = rgb
        self.assertEqual((width, height, len(rows)), (WIDTH, HEIGHT, HEIGHT))
        self.assertEqual(rows[7][3 * 9:3 * 9 + 3], bytes(scene(9, 7)), "every filter reads back")

    def test_blank_and_one_colour_fail(self):
        for colour in ((0, 0, 0), (40, 44, 80)):
            flat, cells, colours, problems = title_drawn.judge(*title_drawn.read_png(png(lambda x, y: colour)))
            self.assertEqual((flat, cells, colours), (16, 16, 1))
            self.assertEqual(len(problems), 2, problems)
            self.assertEqual(self.check(png(lambda x, y: colour)), 1)

    def test_the_menu_alone_over_a_cleared_frame_or_a_gradient_fails(self):
        for backdrop in (lambda x, y: (0, 0, 0), lambda x, y: (20 + x // 4, 24 + x // 4, 60 + x // 4)):
            menu = lambda x, y: scene(x, y) if x < WIDTH * 0.55 and y > HEIGHT / 4 else backdrop(x, y)
            flat, _, _, problems = title_drawn.judge(*title_drawn.read_png(png(menu)))
            self.assertGreater(flat, title_drawn.MAX_FLAT_CELLS)
            self.assertIn("cells are flat", problems[0])
            self.assertEqual(self.check(png(menu)), 1)

    def test_what_isnt_a_png_it_reads_is_refused(self):
        with self.assertRaises(ValueError):
            title_drawn.read_png(b"GIF89a")
        sixteen = png(scene).replace(struct.pack(">IIBB", WIDTH, HEIGHT, 8, 2), struct.pack(">IIBB", WIDTH, HEIGHT, 16, 2))
        with self.assertRaises(ValueError):
            title_drawn.read_png(sixteen)


if __name__ == "__main__":
    unittest.main()
