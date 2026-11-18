# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""Sapphire's Blender add-on (M6 C3), for Blender 5.2 LTS: authors the anime data Sapphire's glTF importer
reads and exports it, and runs the pipeline's rules on the result. docs/blender-addon.md is the artist's
guide.

- `export`: Blender's glTF exporter with the engine's settings, and fixes for what it gets wrong here.
- `mesh_data`: `_MATERIAL_ID` on faces, the data color (AO, outline width, Z offset), `_OUTLINE_NORMAL`
  and the hull preview.
- `materials`: the material extras, as custom properties.
- `bake`: the mask bake (threshold masks for the face map, AO and material IDs for the ILM map), also
  run headless through `tools/blender/bake_character_masks.py`.
- `check` and `pipeline`: Check and the face sheet, through the `sapphire_pipeline` binary, which holds
  every rule.

Install it from a zip of this folder (Edit > Preferences > Get Extensions > Install from Disk), or
import it from a checkout in a script and call `register()`."""

from . import bake, check, export, materials, mesh_data, settings, ui

# In registration order: settings first, the panels last.
MODULES = (settings, export, mesh_data, materials, bake, check, ui)


def register():
    for module in MODULES:
        module.register()


def unregister():
    for module in reversed(MODULES):
        module.unregister()
