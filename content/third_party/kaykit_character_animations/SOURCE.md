# KayKit Character Animations 1.1

Humanoid animation clips by Kay Lousberg for the KayKit characters' rigs, as binary glTF with each set's mannequin mesh inside. The pack holds 161 clips for two rigs; these five files are `Rig_Medium`'s 76 for locomotion, reactions, deaths, melee and the skeleton enemy.

| | |
|---|---|
| **Author** | Kay Lousberg (www.kaylousberg.com) |
| **Version** | 1.1, created 2025-12-10 by `License.txt`'s date, read as day first |
| **Source** | https://kaylousberg.itch.io/kaykit-character-animations |
| **Downloaded from** | The page's free tier, "Free 1.1", upload id 15799903 |
| **Downloaded** | 2026-11-02 (UTC) |
| **Archive SHA-256** | `65882f31f905ad2e953819648a59287cdeab8f623908d5ef701971d3758be20f` (14,858,957 bytes) |
| **License** | Creative Commons Zero 1.0 (CC0), see [License.txt](License.txt) and the page |
| **Credit** | Not required. Credited here anyway: KayKit Character Animations by Kay Lousberg, www.kaylousberg.com. |

The page also asks people not to resell unmodified copies or claim them as their own. That's a request beside the license, and this repository does neither.

## What is committed

| Here | From the archive | Bytes | SHA-256 |
|---|---|---:|---|
| `Rig_Medium/Rig_Medium_General.glb` | `Animations/gltf/Rig_Medium/Rig_Medium_General.glb` | 828,240 | `5f725c0f745f36078c7238968cd5c04843cbc6260689eb6e7f5e2bbd7ffdd7ad` |
| `Rig_Medium/Rig_Medium_MovementBasic.glb` | `Animations/gltf/Rig_Medium/Rig_Medium_MovementBasic.glb` | 689,624 | `11422afd50abf7e8c92ea107e2e0bc94abd38023d6d15f55ad580347efbfa29e` |
| `Rig_Medium/Rig_Medium_MovementAdvanced.glb` | `Animations/gltf/Rig_Medium/Rig_Medium_MovementAdvanced.glb` | 719,628 | `8b2317cf81e3f24149e4b791c27e1932bebd46cd378ed962e7d27d642fad8c56` |
| `Rig_Medium/Rig_Medium_CombatMelee.glb` | `Animations/gltf/Rig_Medium/Rig_Medium_CombatMelee.glb` | 999,416 | `c55e6ec1c79e83ae21d46fe1cd921c1b64be8b9a524a24517e51c6ba99231459` |
| `Rig_Medium/Rig_Medium_Special.glb` | `Animations/gltf/Rig_Medium/Rig_Medium_Special.glb` | 924,308 | `74047dd2178fbed388656051664158e921b7bfc613abb119860aeb7b646bd640` |

`License.txt` is the archive's, also byte for byte.

Not committed: the FBX copies, `Rig_Large`'s six sets, `Rig_Medium`'s ranged, simulation and tool sets, the two mannequin models and their texture, and the `.url` shortcuts. The clips' files hold the medium mannequin's mesh and no image.

## The rig

Read from the files' glTF on the day of download:

- **23 joints:** `root`, `hips`, `spine`, `chest`, `head`, then per side `upperleg`, `lowerleg`, `foot`, `toes`, `upperarm`, `lowerarm`, `wrist`, `hand`, `handslot` (`.l`, `.r`). The Skeletons pack's characters have the same 23.
- **Against VRM's humanoid map:** every required bone has a match (`wrist` is VRM's hand; `hand` is a single block for the palm and fingers), and so do `chest` and `toes`. It has no neck, upper chest, shoulders, eyes, jaw or fingers.
- **Proportions:** the mannequin is 2.2 units tall, with `hips` at 0.41 and the `head` joint at 1.24 in the rest pose. Legs are about a fifth of its height and the head over two fifths: a chibi figure, so clips' translations scale by leg length when retargeted (2.04 onto the character).
- **Joint order:** the same joints at the same rests in every file, in more than one order: `General` and `CombatMelee` agree, `MovementBasic`, `MovementAdvanced`, `Special` and the Skeletons pack's characters each have their own. A clip plays on another file's skeleton by name, not by index, and a cook remaps it.
- **Reference pose:** the files' rest pose is a T-pose, as VRM 1.0's is, and each file's one-key `T-Pose` clip matches it exactly.
- **Keys:** 30 per second and linear. Each joint has translation, rotation and scale channels in most clips, and a channel that doesn't move holds two keys.
- **Root motion:** `root` translates in 10 of the 76 clips: the four dodges over 0.4 s (`Dodge_Forward` 0.25 units, `Dodge_Left` and `Dodge_Right` 0.5, `Dodge_Backward` 0.65), `Spawn_Air` and five `Skeletons_*` clips. It rotates in 5: `Melee_2H_Attack_Spin`, `Melee_2H_Attack_Spinning`, `Spawn_Air`, `Spawn_Ground` and `Skeletons_Spawn_Ground`. Every other clip holds `root` still, and `hips` carries its movement: `Melee_1H_Attack_Jump_Chop`'s travel 0.64 units, and `Death_A`'s end 0.45 back. `Running_Strafe_Left` and `_Right` hold `root` still at 60° rather than at rest, so the body stays turned. `Melee_2H_Attack_Spin` turns it a whole turn, with an overshoot of 19°.
- **Scale channels:** `Spawn_Air` and `Spawn_Ground` start with `root` at a scale of zero, so root motion can't be read from them and a sidecar can't list them. `EXPERIMENTAL_Medium_Transform` and the `Skeletons_*` clips scale or stretch limbs (translations up to 1.67 units in `Skeletons_*`), which is theirs: retargeting carries turns, so these stay on this rig.

## The clips

| File | Clips |
|---|---|
| `Rig_Medium_General.glb` | `Death_A`, `Death_A_Pose`, `Death_B`, `Death_B_Pose`, `Hit_A`, `Hit_B`, `Idle_A`, `Idle_B`, `Interact`, `PickUp`, `Spawn_Air`, `Spawn_Ground`, `T-Pose`, `Throw`, `Use_Item` |
| `Rig_Medium_MovementBasic.glb` | `Jump_Full_Long`, `Jump_Full_Short`, `Jump_Idle`, `Jump_Land`, `Jump_Start`, `Running_A`, `Running_B`, `T-Pose`, `Walking_A`, `Walking_B`, `Walking_C` |
| `Rig_Medium_MovementAdvanced.glb` | `Crawling`, `Crouching`, `Dodge_Backward`, `Dodge_Forward`, `Dodge_Left`, `Dodge_Right`, `Running_HoldingBow`, `Running_HoldingRifle`, `Running_Strafe_Left`, `Running_Strafe_Right`, `Sneaking`, `T-Pose`, `Walking_Backwards` |
| `Rig_Medium_CombatMelee.glb` | `Melee_1H_Attack_Chop`, `Melee_1H_Attack_Jump_Chop`, `Melee_1H_Attack_Slice_Diagonal`, `Melee_1H_Attack_Slice_Horizontal`, `Melee_1H_Attack_Stab`, `Melee_2H_Attack_Chop`, `Melee_2H_Attack_Slice`, `Melee_2H_Attack_Spin`, `Melee_2H_Attack_Spinning`, `Melee_2H_Attack_Stab`, `Melee_2H_Idle`, `Melee_Block`, `Melee_Block_Attack`, `Melee_Block_Hit`, `Melee_Blocking`, `Melee_Dualwield_Attack_Chop`, `Melee_Dualwield_Attack_Slice`, `Melee_Dualwield_Attack_Stab`, `Melee_Unarmed_Attack_Kick`, `Melee_Unarmed_Attack_Punch_A`, `Melee_Unarmed_Idle`, `T-Pose` |
| `Rig_Medium_Special.glb` | `EXPERIMENTAL_Medium_Transform`, `Skeletons_Awaken_Floor`, `Skeletons_Awaken_Floor_Long`, `Skeletons_Awaken_Standing`, `Skeletons_Death`, `Skeletons_Death_Pose`, `Skeletons_Death_Resurrect`, `Skeletons_Idle`, `Skeletons_Inactive_Floor_Pose`, `Skeletons_Inactive_Standing_Pose`, `Skeletons_Spawn_Ground`, `Skeletons_Taunt`, `Skeletons_Taunt_Longer`, `Skeletons_Walking`, `T-Pose` |
