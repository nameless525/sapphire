# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The mask bake's command-line entry: the add-on's bake (`sapphire/bake.py`), run headless on a
character's .blend, which it never saves:

    blender character.blend --background --python tools/blender/bake_character_masks.py -- \
        --face Face --out content/characters/heroine/face --size 1024 --vertical 0:90:15

docs/blender-mask-bake.md describes the whole recipe and every option.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from sapphire import bake  # noqa: E402  (the add-on beside this script)


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    try:
        bake.run(bake.parse_arguments(argv))
    except bake.BakeError as error:
        raise SystemExit(f"bake_character_masks.py: {error}") from None


if __name__ == "__main__":
    main()
