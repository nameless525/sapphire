# Modifications

- No file is modified. `Models/GLB format/` is renamed `glb/`; the models' relative `Textures/colormap.png` references still resolve.
- The `.meta` files are Sapphire's import sidecars (asset IDs and import settings), not part of the kit.

## Derived files

M1 X1 builds the look-dev town from these models in `content/lookdev/town/`. None of it copies the kit's files, and the kit's palette (`Textures/colormap.png`) reaches no frame:

| Files | How they were made |
|---|---|
| `town.assembly.ron` and its sidecar | Written by hand: an assembly document (`sapphire_pipeline::assembly`) that places a few of the kit's `.glb` pieces (market stalls, a cart, and two rocks scaled into the low hills beyond the town) among the town's own parametric buildings, scaled to the character, and assigns each triangle a slot of the town's own by piece, palette cell and glTF material. The cook reads the pieces as they are and writes one static mesh; texture coordinates are made anew in meters and occlusion is baked into vertex colors. |
| `materials/*.material.ron` | The town's environment materials, authored for Sapphire: no colors are taken from the kit. |
| `textures/*.png` | Painted in code by `sapphire_pipeline gen-lookdev` (`sapphire_pipeline::lookdev`): stone, timber, grass, foliage, the terminator band, and the Jinzhou maps (whitewash, granite, tiles and their mask, rafters, lantern paper, flagstones). |
