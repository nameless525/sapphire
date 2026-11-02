# Modifications

- No file is modified. `characters/gltf/` is renamed `characters/`, and `assets/gltf/` is renamed `assets/`; the Blade's relative references still resolve.
- The `.meta` files are Sapphire's import sidecars, not part of the pack. Since M2 O4 the Blade's cooks, with its material slot bound to the document below. The Warrior's and `skeleton_texture.png`'s still say `cook: false`: the derived `warrior.glb` below cooks in the Warrior's place, and nothing reads the pack's palette. The Warrior's names the clips' rig as its `skeleton` (M2 N1), for the day it cooks. `warrior.glb` can't share that rig, whose rests hold the pack's lengths: it cooks a skeleton of its own, and the clips' sidecars retarget onto it.

## Derived files

M2 O4 (the enemy) re-proportions the Warrior and re-materials it and the Blade in the anime character family. These files are CC0 like the pack, and live in `content/combat/enemy/`:

| Files | How they were made |
|---|---|
| `warrior.glb` | Derived from `characters/Skeleton_Warrior.glb` by `sapphire_pipeline gen-combat` (`combat::warrior::warrior_glb`): the bind pose re-proportioned (limbs and ribs longer along their bones, head at 0.6), normals smoothed across bone and cloak, and the rig's rest translations and inverse binds to match. The same 23 joints, names and accessor layout. |
| `warrior.material.ron`, `warrior_eyes.material.ron` | Written by hand: anime character materials bound to the models' `skeleton` and `Glow` slots, in place of the pack's palette material and yellow glow. |
| `warrior_palette.png` | Painted in code by `sapphire_pipeline gen-combat` (`combat::palette_png`) on the layout of `assets/skeleton_texture.png`: its 8 × 4 cells, which each face's UVs lie in, with V running down a gradient. No texel of the pack's image is read; the colours are the project's own, and only the cells the models use are painted. |
| `warrior_control.png`, `warrior_layers.png`, `warrior_matcap.png` | Painted in code by `gen-combat` on the same layout (the matcap is a spheremap with no layout): each part's material ID, shadow bias, highlight, matcap and glow. |
| `warrior.ilm.ron`, `warrior.ramp.ron` | Written by hand: the control map's channels, and each part's shadow ramp. |
| `warrior.moves.ron` | Written by hand: the enemy's frame data and hit volumes. Nothing in it comes from the pack. |
