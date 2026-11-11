#!/usr/bin/env python3
"""Hold two golden-input bundles' desktop cooks to the same bytes (M4 C3).

CI cooks the same commit on Linux and on Windows, each through its own C library and on whatever CPU
the runner has. Cooks are deterministic by construction, so every content file must match. When one
doesn't, each differing pack's assets are listed by ID and kind, so the step that diverged can be
found without downloading anything."""

import argparse
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
    21: "script", 22: "dialogue", 23: "enemy", 24: "score", 25: "voice lines",
}


def read(archive: Path) -> tuple[dict, tarfile.TarFile]:
    bundle = tarfile.open(archive, "r:gz")
    with bundle.extractfile("manifest.json") as stream:
        manifest = json.load(stream)
    return manifest, bundle


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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("first", type=Path, help="a golden-bundle.tar.gz")
    parser.add_argument("second", type=Path, help="another, from the same commit")
    args = parser.parse_args()
    (first, first_bundle), (second, second_bundle) = read(args.first), read(args.second)
    if first["commit"] != second["commit"]:
        raise ValueError("the bundles come from different commits")
    content = lambda manifest: {
        name: record["sha256"] for name, record in manifest["files"].items() if name.startswith("content/")
    }
    a, b = content(first), content(second)
    differing = sorted(name for name in a.keys() | b.keys() if a.get(name) != b.get(name))
    print(f"{len(a)} content files from {first['family']}, {len(b)} from {second['family']}, "
          f"commit {first['commit']}")
    for name in differing:
        print(f"differs: {name} ({first['family']} {a.get(name, 'missing')}, "
              f"{second['family']} {b.get(name, 'missing')})")
        if name.endswith(".spack") and name in a and name in b:
            ours = entries(first_bundle.extractfile(name).read())
            theirs = entries(second_bundle.extractfile(name).read())
            for asset in sorted(ours.keys() | theirs.keys()):
                x, y = ours.get(asset), theirs.get(asset)
                if x != y:
                    kind = (x or y)[0]
                    print(f"  asset {asset} ({KINDS.get(kind, kind)}): {x and x[1:]} against {y and y[1:]}")
    if differing:
        print("The cooks differ: a cook step depends on the OS or the CPU. Spec §5.27, \"Reproducible "
              "cooks\", says what must hold.")
        return 1
    print("The cooks agree byte for byte.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
