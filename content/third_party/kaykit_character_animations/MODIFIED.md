# Modifications

- No file is modified. `Animations/gltf/Rig_Medium/` is renamed `Rig_Medium/`.
- The `.meta` files are Sapphire's import sidecars, not part of the pack. Since M2 N1 they choose the clips that cook, by name: the character's and the enemy's derived warrior's, by the humanoid maps in `Rig_Medium_General.glb.meta` and the target's own sidecar. The other four files name that one as their `rig`, so the five cook one skeleton. Since M2 O2 they also say how each clip's motion lands on its target (`adjust`, `gain`, `follow`, in `clip::retarget`), and nothing cooks on this rig as authored, since no mesh plays it there.
- Since M3 C2 they also retarget onto the generated figures in `content/combat/lancer/` and `content/combat/knell_colossus/` (made by `sapphire_pipeline gen-figures` from `tools/figures/`, with KayKit's joint names): the Lancer takes `Running_B`, `Walking_B`, `Walking_Backwards`, `Running_Strafe_Left`, `Running_Strafe_Right` and `Melee_2H_Idle`, and the boss `Walking_A`. Those figures are the project's own geometry: nothing of the pack's is in them.

## Derived files

None. Retargeting happens in each cook and nothing it makes is committed. The key-pose documents that stand in for clips M2 O2 turned down (`content/lookdev/character/idle.keypose.ron`, `idle_combat`, `death_back`, `death_forward`) are the project's own, written by hand against contact sheets: they read nothing from the pack.
