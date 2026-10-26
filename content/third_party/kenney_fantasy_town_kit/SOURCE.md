# Fantasy Town Kit 2.0

A modular fantasy town: stone and timber walls, roofs, roads and stairs, a fountain, trees, hedges, rocks, market stalls, lanterns, a windmill and a watermill. 167 models in the kit's GLB format, sharing one palette texture.

| | |
|---|---|
| **Author** | Kenney (www.kenney.nl) |
| **Version** | 2.0, created 2025-08-03 (per `License.txt`) |
| **Source** | https://kenney.nl/assets/fantasy-town-kit |
| **Downloaded from** | https://kenney.nl/media/pages/assets/fantasy-town-kit/efe948d309-1754222374/kenney_fantasy-town-kit_2.0.zip |
| **Downloaded** | 2026-10-26 |
| **Archive SHA-256** | `1a7530c09f4d2fa2cdee259876f089334f8b1f27fa86a0c4f54ef86cdd8676ef` (3,854,691 bytes) |
| **License** | Creative Commons Zero 1.0 (CC0), see [License.txt](License.txt) |
| **Credit** | Not required. Credited here anyway: Fantasy Town Kit by Kenney, www.kenney.nl. |

## What is committed

| Here | From the archive |
|---|---|
| `License.txt` | `License.txt` |
| `glb/*.glb` (167 files) | `Models/GLB format/*.glb` |
| `glb/Textures/colormap.png` | `Models/GLB format/Textures/colormap.png`, which every model references as `Textures/colormap.png` |

Not committed: the FBX and OBJ formats, the previews, `Overview.html` and the alternative palette `Models/Textures/variation-a.png`.

Every committed file is byte-identical to the archive's. To check, extract the archive and compare:

```bash
diff -r "<extracted>/Models/GLB format" content/third_party/kenney_fantasy_town_kit/glb -x '*.meta'
```
