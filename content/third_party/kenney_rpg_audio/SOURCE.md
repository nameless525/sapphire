# RPG Audio

Foley for an RPG: cloth, belts, blades drawn, footsteps, doors and books. 51 Ogg Vorbis files.

| | |
|---|---|
| **Author** | Kenney (www.kenney.nl) |
| **Version** | 1.0, released in 2014 (per the page) |
| **Source** | https://kenney.nl/assets/rpg-audio |
| **Downloaded from** | The page's download |
| **Downloaded** | 2026-11-02 (UTC) |
| **Archive SHA-256** | `6dbeaf8544da958d8f2adcb4a4a4b76c1ade34a05f8ab9edccd327da7375f38b` (964,837 bytes) |
| **License** | Creative Commons Zero 1.0 (CC0), see [License.txt](License.txt) |
| **Credit** | Not required. Credited here anyway: RPG Audio by Kenney, www.kenney.nl. |

## What is committed

| Here | From the archive |
|---|---|
| `License.txt` | `License.txt` |
| `Audio/*.ogg` (51 files) | `Audio/*.ogg` |

Not committed: the archive's `.url` shortcuts and `Preview.ogg`.

The page lists 50 files; the archive's `Audio/` folder holds 51.

Every committed upstream file is byte-identical to the archive's. To check, extract the archive and compare:

```bash
diff -r -x '*.meta' "<extracted>/Audio" content/third_party/kenney_rpg_audio/Audio
```
