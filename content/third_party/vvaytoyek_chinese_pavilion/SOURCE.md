# Chinese Four-corner Pavilion

A square Chinese garden pavilion (四角亭): four lacquered timber columns on stone bases, benches with handrails, a hanging lattice frieze, a bracketed eave, and a hipped tile roof with upturned corners and a crowning finial. One FBX of 70 meshes, about 77,000 triangles, in 22 flat-colour materials with no textures.

| | |
|---|---|
| **Author** | VVayToyek (https://vvaytoyek.itch.io) |
| **Source** | https://vvaytoyek.itch.io/chinese-four-corner-pavilion-free ("Chinese Four-corner Pavilion - Free", itch.io game id 4444097) |
| **Downloaded from** | The page's download, upload id 17036359: `Chinese Four-corner Pavilion.zip` (listed as 2.6 MB) |
| **Archive dated** | 2026-03-08 (its entries' timestamps) |
| **Downloaded** | 2026-10-27 (UTC) |
| **Archive SHA-256** | `008fb5c4508ce57c4203e5763fd01a177ded16ca94fd5de34ea26c422b483efa` (2,777,866 bytes) |
| **License** | Creative Commons Zero 1.0 (CC0), see [LICENSE.md](LICENSE.md) |
| **Credit** | Not required. Credited here anyway: Chinese Four-corner Pavilion by VVayToyek. |

## What is committed

| Here | From the archive | SHA-256 |
|---|---|---|
| `chinese_four_corner_pavilion.fbx` | `四角亭.fbx` (2,040,044 bytes) | `e5a745fceef8eb0e4b45230e4d964f618c64ab0981f098e1e329c982c159e89c` |

The archive stores its file names in GBK without the UTF-8 flag. The FBX is renamed to ASCII and is otherwise byte-identical.

Not committed: the 21 `*_UV.png` images, one per part (`瓦面_UV.png` and the like). They're plain UV-layout wireframes on grey, not textures, and the cooker would cook every PNG under `content/` as a texture.

To check, extract `四角亭.fbx` from the archive and compare its SHA-256 with the table's.

## The model

Sapphire's cooker doesn't read FBX, so the file isn't cooked as it is. Its derived glTF is listed in [MODIFIED.md](MODIFIED.md).

**Scale.** Blender's FBX import gives 38.9 × 38.9 units in plan and 35.7 units to the finial. The benches (seat at 3 units) and the bay between columns (22 units) suggest about 0.15 m per unit. That's a 3.3 m bay and 5.4 m to the finial, a garden pavilion's size. This is our estimate; the page doesn't give units.

**Parts.** The objects and materials carry Chinese timber-frame names, numbered 1–4 for the copies around the four sides or corners:

| Name | Part | Colour as shipped |
|---|---|---|
| 檐柱 / 檐柱2 | Eave columns / their stone bases | Red-orange / grey |
| 坐凳, 扶手 | Benches, handrails | Red-orange |
| 挂落 | Hanging lattice frieze between the columns | Red-orange |
| 额枋, 檐檩垫板 | Architrave, eave purlin and its pad board | Red-orange |
| 角云 | Corner cloud brackets on the columns | Red-orange |
| 老角梁, 仔角梁由戗 | Main and upper corner beams, with the hip strut | Red-orange |
| 抹角梁 | Diagonal beam across a corner | Red-orange |
| 金檩金枋 | Upper purlin and tie beam | Red-orange |
| 檐椽, 飞椽, 脑椽, 衬头木 | Eave, flying and top rafters; the rafter-raising blocks | Red-orange |
| 瓦面 | Tile roof surface | Near-black |
| 垂脊 | Hip ridges | Near-black |
| 套兽 | Beast caps on the corner beams' ends | Dark grey |
| 雷公柱宝顶 (material 雷公柱) | King post with the crowning finial | Near-black |
| 对联, 牌匾 | Couplet boards on the front columns, the name plaque | Red, blank |
| 四角亭轴点 | The pivot (an empty) | |
