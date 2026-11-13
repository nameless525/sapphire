"""Cooks agree: same-architecture cooks must match whole, and across architectures all but textures and
their levels."""

from contextlib import redirect_stdout
import io
from pathlib import Path
import struct
import tempfile
import unittest

import cooks_agree

MESH, TEXTURE, TEXTURE_LEVEL = 1, 2, 26


def pack(assets: dict) -> bytes:
    """A pack with only the header fields and table of contents `cooks_agree.entries` reads."""
    header = bytearray(256)
    header[:4] = b"SPAK"
    struct.pack_into("<I", header, 20, len(assets))
    table = bytearray()
    for asset, (kind, size, digest) in sorted(assets.items()):
        entry = bytearray(80)
        entry[:16] = bytes.fromhex(asset)
        entry[16] = kind
        struct.pack_into("<Q", entry, 40, size)
        entry[48:80] = bytes.fromhex(digest)
        table += entry
    return bytes(header + table)


class CooksAgreeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.enterContext(redirect_stdout(io.StringIO()))
        self.mesh = ("11" * 16, (MESH, 10, "aa" * 32))
        self.texture = ("22" * 16, (TEXTURE, 20, "bb" * 32))
        self.level = ("33" * 16, (TEXTURE_LEVEL, 40, "ee" * 32))

    def cook(self, name: str, assets: dict) -> cooks_agree.Cook:
        directory = self.root / name
        directory.mkdir()
        (directory / "shared.spack").write_bytes(pack(assets))
        return cooks_agree.Cook(directory)

    def test_identical_cooks_agree_either_way(self):
        assets = dict([self.mesh, self.texture])
        first, second = self.cook("x86-64", assets), self.cook("aarch64", assets)
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=False), 0)
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 0)

    def test_a_texture_may_differ_only_across_architectures(self):
        texture_id, (kind, size, _) = self.texture
        first = self.cook("x86-64", dict([self.mesh, self.texture]))
        second = self.cook("aarch64", dict([self.mesh, (texture_id, (kind, size, "cc" * 32))]))
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 0)
        self.assertGreater(cooks_agree.compare(first, second, across_architectures=False), 0)

    def test_a_texture_level_may_differ_only_across_architectures(self):
        level_id, (kind, size, _) = self.level
        first = self.cook("x86-64", dict([self.mesh, self.texture, self.level]))
        second = self.cook("aarch64", dict([self.mesh, self.texture, (level_id, (kind, size, "ff" * 32))]))
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 0)
        self.assertGreater(cooks_agree.compare(first, second, across_architectures=False), 0)

    def test_a_mesh_never_differs(self):
        mesh_id, (kind, size, _) = self.mesh
        first = self.cook("x86-64", dict([self.mesh, self.texture]))
        second = self.cook("aarch64", dict([(mesh_id, (kind, size + 4, "dd" * 32)), self.texture]))
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 1)

    def test_a_missing_asset_or_pack_counts(self):
        first = self.cook("x86-64", dict([self.mesh, self.texture]))
        second = self.cook("aarch64", dict([self.mesh]))
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 1)
        (self.root / "x86-64" / "zone.spack").write_bytes(pack(dict([self.mesh])))
        self.assertEqual(cooks_agree.compare(cooks_agree.Cook(self.root / "x86-64"), second, True), 2)

    def test_a_texture_level_in_one_cook_only_counts(self):
        first = self.cook("x86-64", dict([self.mesh, self.texture, self.level]))
        second = self.cook("aarch64", dict([self.mesh, self.texture]))
        self.assertEqual(cooks_agree.compare(first, second, across_architectures=True), 1)
        self.assertEqual(cooks_agree.compare(second, first, across_architectures=True), 1)


if __name__ == "__main__":
    unittest.main()
