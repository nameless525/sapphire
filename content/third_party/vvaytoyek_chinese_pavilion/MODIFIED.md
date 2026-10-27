# Modifications

- `chinese_four_corner_pavilion.fbx` is `四角亭.fbx` renamed to ASCII, byte for byte the archived file (see the SHA-256 in [SOURCE.md](SOURCE.md)).
- The archive's UV-layout images are not committed.
- `chinese_four_corner_pavilion.fbx.meta` is Sapphire's import sidecar (asset ID and import settings), not part of the model. It says `cook: false`: X1 converts the FBX to a glTF piece of its own, so the FBX itself is never cooked or packed (M1 A10, spec §5.27).

## Derived files

| File | How it was made |
|---|---|
| `content/lookdev/town/pieces/pavilion.glb` (and its sidecar) | Converted from the FBX by [`tools/blender/pavilion_to_gltf.py`](../../../tools/blender/pavilion_to_gltf.py), run headless in Blender 5.2.0 LTS with its defaults: `blender -b --factory-startup --python tools/blender/pavilion_to_gltf.py -- --fbx content/third_party/vvaytoyek_chinese_pavilion/chinese_four_corner_pavilion.fbx --out content/lookdev/town/pieces/pavilion.glb`. The output is byte for byte the same on every run (SHA-256 `1d1adb97a062ede237d63625efed57c4da2408a42884b539018aa081df5e1a25`). |

What the conversion changes (the script's docstring has the details):

- **Scale and axes.** Scaled by 0.15 m a unit, the estimate in [SOURCE.md](SOURCE.md): 5.83 m square over the eaves, 5.35 m to the finial, the benches' seats 45 cm high. glTF's +Y up, the front (couplet boards and plaque) toward +Z, standing on its origin.
- **Merged.** The 70 meshes are joined into one, transforms applied. The pivot empty is dropped.
- **Materials renamed.** The 22 Chinese-named materials become six with ASCII names, and their flat colours are dropped: the look-dev town binds each to an environment material of its own in the Jinzhou palette.

  | Piece material | Source materials (see [SOURCE.md](SOURCE.md)'s parts table) |
  |---|---|
  | `lacquer` | 檐柱, 坐凳, 扶手 (columns, benches, handrails) |
  | `timber` | 挂落, 额枋, 檐檩垫板, 角云, 老角梁, 仔角梁由戗, 抹角梁, 金檩金枋, 檐椽, 飞椽, 脑椽, 衬头木 (frieze, architraves, pad boards, cloud brackets, beams, purlins, rafters, raising blocks) |
  | `tiles` | 瓦面 (tile surface) |
  | `ridge` | 垂脊, 套兽, 雷公柱 (hip ridges, corner beasts, king post and finial) |
  | `stone` | 檐柱2 (the columns' stone bases) |
  | `boards` | 对联, 牌匾 (couplet boards, plaque) |

- **Decimated** by Blender's edge collapse, from 76,926 triangles to 30,758: the tile surface to 25%, the hip ridges and corner beasts to 20%, the king post and finial and the round eave rafters to 35%, the top rafters to 40% and the cloud brackets to 60%. Everything else keeps its triangles.
- **Two kinds of faces moved 4 mm back**, where they lay in one plane with another part's and would z-fight: the columns' tops, flush with the architraves' tops (under the cloud brackets), and the upper corner beams' outer ends, flush with the hip ridges' ends at the eave corners. The script fails if any other part's faces meet in one plane.
- **No texture coordinates**, colours, cameras, lights or animation. The town makes its own texture coordinates, in meters.
