# Baking character masks in Blender

The anime character shader reads three authored maps besides its color and normal maps (spec §5.12): an **SDF face map**, **ramps** and an **ILM map**. The pipeline generates each from a RON document (`sapphire_pipeline::character_maps`, M1 A7):

| Document | Reads | Cooks to |
|---|---|---|
| `<name>.facesdf.ron` | Threshold masks: the face's lit region at a series of light angles | The face's threshold map (SDF class) |
| `<name>.ilm.ron` | Grayscale masks, one per ILM channel | The packed ILM map |
| `<name>.ramp.ron` | Nothing: color stops | Day and night ramp rows per material ID |

This recipe bakes the masks the first two read from a character in Blender. The bake is a **starting point**: the anime look comes from the shapes artists paint over it.

## What you need

- Blender; tested with 5.2 LTS. Baking uses Cycles on the CPU.
- The character's `.blend`, with UV-unwrapped meshes. The face's UVs must be mirror-symmetric about `u = 0.5`, because the shader mirrors the face map for light from the left.
- [`tools/blender/bake_character_masks.py`](../tools/blender/bake_character_masks.py). It runs headless and never saves the `.blend`: it bakes copies of the objects with temporary materials, removes everything it creates, and restores every setting it changes.

## 1. Prepare the face

- **Normals decide the starting masks.** Anime faces read best with smooth, rounded normals rather than the sculpted ones. Transfer normals from a sphere or ellipsoid around the head with a Data Transfer modifier (Face Corner Data → Custom Normals), as the research notes describe (character rendering §3.2). The bake uses the evaluated mesh, so modifiers and custom normals count.
- **Orientation.** Head-forward and head-up are world directions, by default `-Y` and `+Z`: Blender's front view, which is how VRM imports face. Pass `--forward` and `--up` otherwise.
- Armatures are put in their rest pose while the script bakes, and restored afterwards.

## 2. Bake

```bash
blender character.blend --background --python tools/blender/bake_character_masks.py -- \
    --face Face --out content/characters/heroine/face \
    --size 1024 --horizontal 0:180:10 --vertical 0:90:15

blender character.blend --background --python tools/blender/bake_character_masks.py -- \
    --body Body --body Hair --material-ids Skin=0,Cloth=1,Metal=2,Hair=3 \
    --out content/characters/heroine --size 2048
```

| Option | Default | What it does |
|---|---|---|
| `--face OBJECT` | — | Bakes SDF threshold masks for this mesh |
| `--body OBJECT` | — | Bakes ILM inputs (AO and material IDs) for this mesh; repeat for more |
| `--out DIR` | required | Where masks and documents go |
| `--size N` | 1024 | Mask width and height. The map has the masks' resolution, and masks are binarized, so this sets the shadow edge's precision |
| `--uv-map NAME` | the active map | UV map to bake into |
| `--horizontal A` | `0:180:10` | Angles of the horizontal masks: `start:stop:step` or `a,b,c`, in degrees |
| `--vertical A` | none | Angles of the vertical masks (WuWa's top light) |
| `--forward`, `--up` | `0,-1,0`, `0,0,1` | Head-forward and head-up, as world directions |
| `--terminator DEG` | 90 | A texel is lit while the light is less than this many degrees past its normal |
| `--margin N` | 8 | Texels baked past UV island edges, so filtering at seams finds valid values |
| `--ao-samples`, `--ao-distance` | 64, 0.1 m | The AO bake's samples and ray length |
| `--material-ids` | slot order | `Material=ID` pairs, IDs 0 to 7 |
| `--overwrite` | off | Replace existing masks and documents, and remove masks at angles this bake no longer has (painted changes are lost). Without it, the script stops before baking if any output exists |

## 3. What it writes

For `--face`:

- `masks/h_<angle>.png`: the face lit by a light turning in the head's horizontal plane, from head-forward (0°) toward head-right (90°) and on to behind (180°).
- `masks/v_<angle>.png` (with `--vertical`): lit by a light rising in the forward–up plane, from head-forward (0°) to overhead (90°).
- `face.facesdf.ron`, listing the masks with their angles.

Masks are 8-bit PNGs (gray, stored as RGB), **white where lit**, black outside the UV islands and their margin. A texel is lit while the light is less than `--terminator` degrees past its normal, measured in the plane the light turns in. That only ever shrinks the lit region as the angle grows, which the face map needs: the side facing the light never darkens, so a light from the front lights the whole face, and the shadow creeps in from the far side.

For each `--body`:

- `<name>_ao.png`: ambient occlusion.
- `<name>_ids.png`: each material slot's ID as the gray level the ILM map stores, `ID / 7`.
- `<name>.ilm.ron`: the shadow bias from the AO, mapped onto `0.0`–`0.5` (unoccluded is the neutral 0.5; occlusion pulls toward 0, always in shadow), and the material IDs. The specular mask and specular threshold take their neutral values until you add them.

## 4. Paint

Open the masks in any editor that keeps 8-bit grayscale PNGs (Krita, Photoshop, Blender's texture paint), and draw the shadow shapes the character should have: the cheek line, a nose shadow that appears past some angle, hard shapes rather than the normals' soft sweep.

- **Keep each mask's lit region inside the one before it.** One threshold per texel can't express a texel that turns lit again, so the cook fails and names the texels:

  ```
  the horizontal masks must only lose lit texels as the light turns, but
    - masks/h_060.00.png (60°) lights 36 texel(s) that masks/h_045.00.png (45°) shadows, within x 20..=25, y 40..=45
  ```

  A few stray anti-aliased pixels can be accepted with `allowed_violations: 4` in the document; they then follow the first mask that shadows them.
- **Keep the outside of the face's UV islands black** in every mask: the generator sweeps the shadow from one mask's edge to the next, and a mask with no edge gives it nowhere to start.
- Anti-aliased brushes are fine: masks are lit where they are at least 50% white.
- **Masks can sit at any angles.** Add masks where the shape changes quickly (around the nose) and fewer where it doesn't; edit the angles in the document to match. The sweep between two masks is linear, so more masks give a smoother turn.
- With the light exactly behind the head (180°) the map lights nothing, so a mask at 180° only shapes the sweep toward it; texels it lights are dark at that one angle.
- Eyes and brows that never take shadow are painted in the face material's face mask (B, always lit; `character_maps::face_mask`), not in these masks. The face mask also holds the nose line (R) and the face highlight (G); blush and gloom go in the expression map (M1 C3).

For the ILM inputs, paint a specular mask (white where highlights may appear) and add `specular_mask: Image(path: "body_spec.png")`; adjust the AO's `range`, or paint the shadow bias directly (0.5 neutral, at least 0.95 always lit, at most 0.05 always in shadow). Paint material IDs in flat gray levels; a document whose painted levels differ from `ID / 7` lists them: `material_id: Image(path: "ids.png", levels: [0.0, 0.3, 0.5, 0.7, 1.0])`.

## 5. Check and cook

```bash
# The generated map as a 16-bit PNG, before compression:
cargo run -p sapphire_pipeline --profile tools -- bake-map content/characters/heroine/face/face.facesdf.ron /tmp/face.png

# Cook the content tree:
cargo xtask content
```

The masks are part of their document's asset: they get no `.meta` sidecar and no cooked texture of their own, and editing one recooks exactly the maps that read it. A document must name its masks with the files' exact case, and a mask must not have a sidecar of its own; if the cook already took a mask for a texture (it was cooked before any document named it), delete its `.meta` file. A face map is 16-bit on desktop and 8-bit on mobile; it has two channels when the document has vertical masks.

## Ramps

Ramps aren't baked: they're color stops in a `.ramp.ron` next to the character, one entry per material ID (the first is ID 0):

```ron
(
    type: "sapphire::pipeline::RampMap",
    version: 1,
    data: (
        materials: [
            (
                name: "skin",
                day: (stops: [(at: 0.0, color: "#b8616a"), (at: 0.55, color: "#e5927c"), (at: 1.0, color: "#f6c3a6")]),
                night: (stops: [(at: 0.0, color: "#4e3f6e"), (at: 1.0, color: "#9a86b8")]),
            ),
            (
                name: "metal",
                day: (interpolation: Step, stops: [(at: 0.0, color: "#3a3440"), (at: 0.4, color: "#8a8094"), (at: 0.8, color: "#d8d2e0")]),
            ),
        ],
    ),
)
```

`at` runs from the deepest shadow (0) to the lit edge of the terminator (1). Stops blend in OkLab unless the document, a ramp or a stop says `Linear`, `Srgb` (an image editor's gradient) or `Step` (bands). A material without a night ramp uses its day ramp for both. Warm, saturated shadow colors are the look; never a darkened grey.
