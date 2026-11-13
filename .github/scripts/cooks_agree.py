#!/usr/bin/env python3
"""Hold two desktop cooks of one commit to the same bytes (M4 C3, C6).

CI cooks the same commit on Linux and on Windows, each through its own C library and on whatever CPU
the runner has. Cooks are deterministic by construction, so every content file must match. When one
doesn't, each differing pack's assets are listed by ID and kind, so the step that diverged can be
found without downloading anything.

With `--across-architectures` the second cook is AArch64's (the Metal golden job's packs) and the first
x86-64's. Block-compressed textures are encoded by each architecture's own kernels, by design, so
they may differ and are only counted, a texture's tail and each of its levels alike (M5 M3); every
other asset, every mesh among them, must match."""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys
import tarfile

# `sapphire_assets::AssetKind`'s numbers, for the report only: an unknown number prints as itself.
KINDS = {
    1: "mesh", 2: "texture", 3: "material", 4: "rig", 5: "probe grid", 6: "skeleton", 7: "clip",
    8: "colliders", 9: "sound", 10: "cue", 11: "move set", 12: "anim graph", 13: "zone", 14: "strings",
    15: "font", 16: "font chains", 17: "navmesh", 18: "timeline", 19: "ui", 20: "encounter",
    21: "script", 22: "dialogue", 23: "enemy", 24: "score", 25: "voice lines", 26: "texture level",
}
TEXTURE, TEXTURE_LEVEL = 2, 26


class Cook:
    """A cook's content files: a golden-input bundle's (`golden-bundle.tar.gz`), or a directory of packs."""

    def __init__(self, path: Path):
        self.path = path
        if path.is_dir():
            self.bundle = None
            self.family, self.commit = path.name, None
            self.files = {
                f"content/{file.name}": hashlib.sha256(file.read_bytes()).hexdigest()
                for file in sorted(path.glob("*.spack"))
            }
        else:
            self.bundle = tarfile.open(path, "r:gz")
            with self.bundle.extractfile("manifest.json") as stream:
                manifest = json.load(stream)
            self.family, self.commit = manifest["family"], manifest["commit"]
            self.files = {
                name: record["sha256"] for name, record in manifest["files"].items() if name.startswith("content/")
            }

    def read(self, name: str) -> bytes:
        if self.bundle is None:
            return (self.path / Path(name).name).read_bytes()
        return self.bundle.extractfile(name).read()


def entries(pack: bytes) -> dict:
    """A pack's table of contents (`sapphire_assets::pack`'s layout): ID → (kind, size, hash)."""
    if pack[:4] != b"SPAK":
        raise ValueError("not a pack")
    (count,) = struct.unpack_from("<I", pack, 20)
    table = {}
    for index in range(count):
        offset = 256 + 80 * index
        kind = pack[offset + 16]
        (size,) = struct.unpack_from("<Q", pack, offset + 40)
        table[pack[offset:offset + 16].hex()] = (kind, size, pack[offset + 48:offset + 80].hex())
    return table


def compare(first: Cook, second: Cook, across_architectures: bool) -> int:
    """Prints what differs and returns how many differences count against the cooks."""
    a, b = first.files, second.files
    if across_architectures:
        # The packs' own bytes hold the textures, and the release manifest the packs: only assets count.
        a = {name: digest for name, digest in a.items() if name.endswith(".spack")}
    differing = sorted(name for name in a.keys() | b.keys() if a.get(name) != b.get(name))
    print(f"{len(a)} content files from {first.family}, {len(b)} from {second.family}"
          + (f", commit {first.commit}" if first.commit else ""))
    failures = 0
    encoded = {TEXTURE: 0, TEXTURE_LEVEL: 0}
    for name in differing:
        if name not in a or name not in b:
            print(f"differs: {name} ({first.family} {a.get(name, 'missing')}, {second.family} {b.get(name, 'missing')})")
            failures += 1
            continue
        if not across_architectures:
            print(f"differs: {name} ({first.family} {a[name]}, {second.family} {b[name]})")
            failures += 1
            if not name.endswith(".spack"):
                continue
        ours, theirs = entries(first.read(name)), entries(second.read(name))
        for asset in sorted(ours.keys() | theirs.keys()):
            x, y = ours.get(asset), theirs.get(asset)
            if x == y:
                continue
            kind = (x or y)[0]
            if across_architectures and x and y and kind in encoded:
                encoded[kind] += 1
                continue
            if across_architectures:
                failures += 1
            print(f"  {name}: asset {asset} ({KINDS.get(kind, kind)}): {x and x[1:]} against {y and y[1:]}")
    if across_architectures:
        print(f"{encoded[TEXTURE]} textures and {encoded[TEXTURE_LEVEL]} texture levels differ, "
              "as each architecture's block encoders do.")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("first", type=Path, help="a golden-bundle.tar.gz")
    parser.add_argument("second", type=Path, help="another from the same commit, or a directory of its packs")
    parser.add_argument("--across-architectures", action="store_true",
                        help="the second cook is another architecture's: only textures and their levels may differ")
    args = parser.parse_args()
    first, second = Cook(args.first), Cook(args.second)
    if first.commit and second.commit and first.commit != second.commit:
        raise ValueError("the bundles come from different commits")
    if compare(first, second, args.across_architectures):
        print("The cooks differ: a cook step depends on the OS, the CPU or the architecture. Spec §5.27, "
              "\"Reproducible cooks\", says what must hold.")
        return 1
    print("The cooks agree"
          + (" but for textures and their levels." if args.across_architectures else " byte for byte."))
    return 0


if __name__ == "__main__":
    sys.exit(main())
