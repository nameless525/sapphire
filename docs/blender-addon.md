# The Blender add-on

Sapphire's add-on for Blender 5.2 LTS lets you author a character's anime data in Blender, export it as the glTF file the engine's importer reads, and check it against the engine's budgets without leaving Blender. It lives in [`tools/blender/sapphire/`](../tools/blender/sapphire/) (MIT). Every rule it shows comes from the pipeline binary, so the add-on, the content tests and CI judge a character the same way.

## Install

1. Build the pipeline once: `cargo build -p sapphire_pipeline --profile tools` writes `target/tools/sapphire_pipeline`.
2. Build the extension: `blender --factory-startup --command extension build --source-dir tools/blender/sapphire --output-dir <folder>` writes `sapphire-0.3.0.zip` there. In Blender: Edit > Preferences > Get Extensions > Install from Disk, and pick the zip.
3. In the add-on's preferences, set **Pipeline** to the binary (or set `$SAPPHIRE_PIPELINE`, or put `sapphire_pipeline` on the PATH).

The panels are in the 3D view's sidebar (N) under **Sapphire**, in the order you work: Export, Material IDs, Data Color and Hull, Outline Normals, Masks, Check. The material settings are in the material properties, in a **Sapphire** panel.

## What the engine reads, and where you set it

| Data | In Blender | Panel | In the glTF file |
|---|---|---|---|
| Material ID (0 to 7): which of its material's regions a face shades with, when the material has no control map | An integer attribute on faces, `_MATERIAL_ID` | Material IDs: select faces in Edit Mode and press an ID; Select picks the faces of one | `_MATERIAL_ID`, the same on all three corners of a triangle |
| The data color: R ambient occlusion, G outline width scale, B outline Z offset scale | A float color attribute, `sapphire_data` (missing: 1 for each) | Data Color and Hull: set a channel on the selected vertices; paint more in Vertex Paint | `COLOR_0` |
| Outline normals: the direction the inverted hull grows along, so hard edges don't split the line | A vector attribute on points, `_OUTLINE_NORMAL` (missing: the cook welds the shading normals) | Outline Normals: transfer from a smooth source mesh, or weld the mesh's own | `_OUTLINE_NORMAL`, turned to glTF's Y-up |
| The material's look: shade and outline colors, the light split, the rim, each region's rim | Custom properties on the material | Material properties > Sapphire | The material's extras |
| One-sided or two-sided | The material's Back-face Culling (off: two-sided) | Material properties > Sapphire | `doubleSided` |

Use a **float** color attribute for the data color: Blender converts byte colors from sRGB on export, which bends the values. A material that a material document is bound to in the mesh's sidecar takes the document's look instead of its extras.

### Material extras

| Extra | What it sets | Engine default |
|---|---|---|
| Shade color | Region 0's shadow-side color, linear RGB | 0.55, 0.5, 0.65 |
| Outline color | Region 0's line color, linear RGB | 0.08, 0.06, 0.1 |
| Character shading | Light split, its softness, rim strength, outline width (pixels at 1080 rows, at 3 m) | 0, 0.02, 0.35, 5.5 |
| Rim | The depth-offset rim's width, strength, threshold, Fresnel, light gate and back light | 4, 0.35, 0.1, 0.5, 0.75, 0 |
| Rim regions | Each region's rim strength, width and overlap scales, from region 0; the material gets one region a row, which `_MATERIAL_ID` picks | 1, 1, 1 |

Add an extra to set it; remove it to keep the engine's default.

### The hull preview

**Show hull** draws the inverted hull the engine draws around the mesh: a copy pushed out along the outline normals by the material's outline width times the data color's G, faces flipped, in a dark one-sided material. The engine's width is in pixels; the preview turns it into meters at the outline curve's middle distance, 3 m from the camera, where a 1080-row frame spans about 2.5 m (**Pixel**, 2.3 mm). Press it again after changing a material's width (**Refresh**). The preview is a modifier the export never writes.

### Outline normals

Pick a **Source** and press **Transfer Outline Normals** to give each vertex the smooth normal of the nearest point on the source's surface: a sphere or an ellipsoid around a head or a hair cap gives the rounded outline anime characters have. With no source it welds the mesh's own normals, as the cook would, so you can see them in the hull preview and edit them. **Clear** removes them and the cook welds its own again. Both repeat bit for bit, so exporting the same scene twice writes the same file.

## Export

Choose the **File** (a `.glb` inside your content tree, beside its `.meta` sidecar) and press **Export for Sapphire**. It runs Blender's glTF exporter with the engine's settings: a binary file, Y-up, normals and UVs, the data color as `COLOR_0`, the `_` attributes, material extras, skins and shape keys, no cameras or lights, no images (the engine binds maps through material documents; **Images** embeds them anyway). **Animations** writes the scene's actions as glTF animations, the mesh's clips; **Apply modifiers** applies everything but the armature, and Blender's exporter then drops shape keys.

It also works around what Blender 5.2's exporter gets wrong for this data: it writes custom vectors without turning them to Y-up, and it writes a mesh's vertex colors into its first material only, every other material's white.

The file is written under a hidden name beside it and renamed into place, so whatever watches the folder sees it whole.

## Send: from Blender to the running game

**Send to Sapphire** is the export into your content tree: set **Content** to the tree (the repository's `content/`), choose a **File** inside it, and press Send. It refuses a file outside the tree, since nothing would cook it there. With the loop running, the saved character is on screen in the running game a few seconds later, with no restart:

```bash
cargo xtask content --watch                  # cooks content/, then again on every save
cargo xtask run --platform desktop --features dev -- --look-dev enemy --model <the mesh's ID>
                                             # or any dev app reading the cook
```

The watch cooks only what the save changed (the mesh, and anything that reads it) and rewrites only the packs it lands in; the running app's hot reload remounts those and swaps the new mesh in. For a phone or a simulator, watch the mobile cook and let live link carry the packs:

```bash
cargo xtask content --watch --cook mobile
cargo xtask link watch                       # pushes each rewritten pack to the device's app
```

A cook that fails (a file the importer refuses, say) prints why, and the watch waits for the next save. A new file gets its `.meta` sidecar from the first cook, to commit with it. An app loads a character by its mesh's ID, which is in that sidecar, so start the app on a new character after its first cook: from then on, each Send shows in it.

## Masks

**Bake Face Masks** bakes the active face mesh's threshold masks and its face SDF document; **Bake Body Masks** bakes each selected mesh's AO and material IDs and its ILM document; **Face Sheet** draws a face SDF document's map lit at the angles you give beside its masks, through the pipeline, and opens it as an image. [blender-mask-bake.md](blender-mask-bake.md) is the whole recipe: preparing the face, every option, painting the masks.

## Check

**Check** runs `sapphire_pipeline check` on the **Content** folder (by default, the exported file's folder) and lists, for each mesh whose sidecar names a role, its triangles, joints, material slots and largest map against that role's row on both cooks, then every finding: errors fail, warnings don't, and a waived finding shows its reason. A source needs its sidecar before it can be checked (a cook of the tree writes one); give a character its role in the sidecar's `budget`, beside `id` and `importer`:

```ron
budget: (role: Hero),
```

The roles are `Hero`, `Enemy`, `Boss`, `Ambient` and `Tree`; spec §3.4 has their rows, and [`check.rs`](../tools/sapphire_pipeline/src/check.rs) every rule.

## The test character

[`tools/blender/test_character.py`](../tools/blender/test_character.py) builds the project's own test character with the add-on, headless, and exports it through **Export for Sapphire**: a 1.6 m figure on 17 bones, in four materials, with every piece of data above. The pipeline's tests read the committed file back (`tools/sapphire_pipeline/tests/blender_character.rs`) and check it at the hero role.

```bash
blender -b --factory-startup --python tools/blender/test_character.py -- \
    --out tools/sapphire_pipeline/fixtures/character \
    --check --pipeline target/tools/sapphire_pipeline
```

`--hull` also confirms the hull preview, and `--bake <folder>` bakes the face's masks there and draws their sheet. `--send <content tree>` exports with Send into a tree that holds `--out`, and `--skin-shade r,g,b` changes the skin's shade color first, so a re-export into a running loop has something to show. It uses the add-on installed in Blender if there is one (`--installed` requires it), else the checkout's. Each run writes the same bytes on Blender 5.2.0 LTS.
