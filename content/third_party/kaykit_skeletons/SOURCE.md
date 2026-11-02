# KayKit Skeletons 1.1

Low-poly rigged skeleton characters and their weapons by Kay Lousberg. M2 uses one character, the Warrior, as the enemy, and the Blade as its weapon.

| | |
|---|---|
| **Author** | Kay Lousberg (www.kaylousberg.com) |
| **Version** | 1.1, created 2025-10-22 by `License.txt`'s date |
| **Source** | https://kaylousberg.itch.io/kaykit-skeletons |
| **Downloaded from** | The page's free tier, "Free 1.1", upload id 15363145 |
| **Downloaded** | 2026-11-02 (UTC) |
| **Archive SHA-256** | `21fbad59ee6cc1d7bed12d0e425acab8ebe564b8620bbc1d017aedb29dd8a3d2` (8,177,445 bytes) |
| **License** | Creative Commons Zero 1.0 (CC0), see [License.txt](License.txt) and the page |
| **Credit** | Not required. Credited here anyway: KayKit Skeletons by Kay Lousberg, www.kaylousberg.com. |

## What is committed

| Here | From the archive | Bytes | SHA-256 |
|---|---|---:|---|
| `characters/Skeleton_Warrior.glb` | `characters/gltf/Skeleton_Warrior.glb` | 367,484 | `992d5cfd6beb5565c50f742587d492f2a2cfcdfb3ffe4435aa16461c6d0e54be` |
| `assets/Skeleton_Blade.gltf` | `assets/gltf/Skeleton_Blade.gltf` | 3,047 | `682a1df2eecb93dea35de628d6b93ca40d1b83586422356f3c1956741f2abf76` |
| `assets/Skeleton_Blade.bin` | `assets/gltf/Skeleton_Blade.bin` | 20,056 | `ccf4f6642130d11650400fa649669ccdb784280eef235fac3a12a197cbc7debe` |
| `assets/skeleton_texture.png` | `assets/gltf/skeleton_texture.png` | 17,037 | `15741a25c53e04fa9bf3beac3bc0de442359404b1ff9be863b892cb551ad3657` |

`License.txt` is the archive's, also byte for byte.

Not committed: the Mage, Minion and Rogue, the other weapons and accessories, the FBX and OBJ copies, the sample and contents images, and the `.url` shortcuts. The two sets of clips in the archive are byte for byte the same files as [KayKit Character Animations](../kaykit_character_animations/SOURCE.md)' and aren't repeated here.

## The model

- **The Warrior:** 5,934 triangles in 10 meshes (arms, body, cloak, eyes, head, helmet, jaw, legs), one embedded palette image, and the same 23 joints as the clips' rig, so every `Rig_Medium` clip plays on it as it is.
- **The Blade:** an unskinned mesh for the rig's `handslot` joints. It references `skeleton_texture.png` and `Skeleton_Blade.bin` beside it.
