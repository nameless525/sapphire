# Characters and animation: a survey for M5

A design survey's read of the code on 2026-11-13, at 27f29b5f (release 0.1.0), for [the M5 plan](../../docs/m5-plan.md). Nothing was built or run: every finding is from reading the source, the spec and the handoffs, and "unverified" marks what the source could not confirm. Line numbers are that commit's. Sizes are the surveyor's, in agent-days; the plan splits and re-sizes its packages. A package that leans on a finding checks it against the code first.

## 1. As built

- **Runtime.** `crates/sapphire_body_pose` (L3, in `[simulation]` since #213) holds sampling, blends, masks, additive layers, inertialization, the polar blend space, `GraphPlayer`, constraints and IK (two-bone, aim, feet, look, hands). `crates/sapphire_anim` re-exports it and adds `evaluate_all`, `CharacterMotion` and `SpringBones`. The graph document is `crates/sapphire_assets/src/graph.rs`. The graph writes no root motion; moves bake it.
- **Clips** (`crates/sapphire_assets/src/clip.rs`): sparse tracks, key reduction to 0.25 mm, 8 bytes a key, stateless search. Move clips cook to 3.8–9.7 KB (spec line 3064). No ACL-style segments, by decision.
- **Cook-side motion** (`tools/sapphire_pipeline/src/clip/`, `import/{gltf,fbx,vrma}.rs`): glTF, FBX, VRMA and key-pose sources, humanoid-map retarget (`adjust`, `gain`, `follow`), root motion by named bone. Limits:
  - Both rests must be near a T-pose.
  - No move may bake from an FBX (`BakeError::FbxSource`), because ufbx's curves differ by architecture.
  - VRMA `expressions` and `lookAt` are dropped.
- **Secondary motion** (`crates/sapphire_anim/src/springs.rs`): VRM Verlet with limits, owner-clock sub-steps, `SpringQuality` by tier, sleep off screen (`SpringView::decide`, `crates/sapphire_stage/src/animate.rs:90`). Springs cost 20–34 µs for 57 joints. The clearance pass is `HeroineClearance`, fitted to one model by material name (`"Tops_01_CLOTH"`, `springs/clearance.rs:585`), and is nearly all of the budget row.
- **Skinning and morphs** (`crates/sapphire_render/src/skinning.rs`): one compute dispatch, morphs in the same pass. `DRAWN_LOD = 0` (line 97), and every posed instance is skinned, visible or not.
- **Faces.** The VRM carries all 18 expression presets (five visemes among them), cooked into `.srig` (`rig.rs:306`). The runtime reads only `blink` (`body_pose/src/graph.rs:664`) and the look. Expression colour and UV binds are cooked and never applied. Clips key raw morph names, not expressions.
- **Timelines.** A `CharacterTrack` is `plays` plus `places` (`sapphire_timeline/src/document.rs:266`), and the only roles are `hero` and `foe` (`game/game_combat/src/timeline.rs:17`).
- **Voice.** Two generated placeholder lines per language. A dialogue's `vo` is unwired (`game_combat/src/dialogue.rs:21`), and the mixer exposes no envelope.
- **Crowd.** `animate_fighters` (`fight.rs:1494`) runs serially with no distance or visibility gate except on springs. There is no ambient-character concept: everything animated is a simulation actor.
- **Single-character assumption.**
  - `lookdev::CHARACTER` and `GRAPH` (`lookdev.rs:94,100`), `GraphKey::CHARACTER` (`content.rs:819`), `Scene::character`.
  - Roster index 0 is the player (`fight.rs:1528`); `fixed_fighters: [_; 2]` (line 458).
  - `party_looks.rs` draws three tints on one entity.
  - Baked `player.smoves`, `blade.rs`, `tools/motion/rig.py`.
  - No HUD party slots and no touch swap; `game_flow` always passes `party: false`.
- **Measured.**
  - One hero (155 joints, 36,470 triangles on desktop, 22,188 on mobile): graph 17.5 µs, IK +8 µs, GPU skinning 33–49 µs.
  - Eight 24-joint fighters: CPU frame 982 µs against 908 for two.
- **Not measured.** Any character at 80k triangles or 200 bones; three heroes on screen; 12 enemies at 8k triangles; skinning, hulls or the character prepass by character count; any phone.
- **My extrapolation (unverified).** At budget, 3 + 12 characters come to roughly 0.3–0.45 ms of GPU skinning and 0.2 ms of animation CPU on the M5. Secondary motion goes over: one clearance p99 is already 0.35 of Desktop-Low's 0.5 ms.

## 2. Already designed for later

- Spec §5.13 "Secondary motion": an XPBD chain/cloth solver with leg and body capsules, on the owner clock with fixed sub-steps and LOD by tier.
- Spec §5.13 "Faces": lip-sync curves later. "How": one job per character.
- Spec §5.14: ragdolls later (line 2215); `sapphire_physics/src/lib.rs:72`.
- `skinning.rs` "Later milestones": skin only visible instances, select the LOD prefix. `outlines.rs`: the cutoff becomes a LOD.
- `party_looks.rs:10`: a game's party is three characters, each with its own mesh.
- m1-plan line 58 names Seed-san; m2-plan line 180 dropped it for recolors.

## 3. Hard constraints

- **The simulation owns the bound pair's poses.** "Anything that changes a fighter's pose in the step moves checksums" (the simulation's determinism rules), and x86-64 re-records cost two CI rounds. New pose features go in `sapphire_anim`, after the graph, or must leave body samples bit-identical.
- **Presentation, outside checksums:** springs, clearance, `CharacterMotion`, a timeline's presentation half, cues.
- **Cook determinism:** clips to tables stay on `det`, and each retarget option needs a pinned clip (`the_pinned_clips_use_every_retarget_option`).
- **Zero allocation** once a pose, graph or springs exist. `sapphire_jobs` allocates per call past one job.
- **A frozen clock** means identical bits and no work. Interval 1 stays bit-exact. New features default off so goldens stay byte-identical.
- **Budgets (§3.2):** Animation + IK 1.5 / 3.0 ms. Secondary motion 0.5 / 1.0 ms, of which the heroine alone takes 0.20 mean / 0.35 p99 on Desktop-Low and 0.30 / 0.43 on Mobile-Low. §3.3 has no skinning row.

## 4. Proposed work packages

Order: A0, then A1 and A3 in parallel, then A2, A5, A4, A6, A7, A9, A8, A10. Sizes are agent-days.

| # | Package and goal | Touches | Needs | Headless proof | Tiers | Size | Risk |
|---|---|---|---|---|---|---|---|
| A0 | **Scale bench.** Rows by character count, and a generated hero at budget (80k, 200 bones, 6 materials, skirt chains, expression morphs). | `apps/sapphire_sandbox/src/fight_bench.rs`, pipeline `figure/`, §3.3 skinning row | none; prerequisite for all | `--bench` rows at 1, 4, 15, 60 characters | all four | M (3) | A synthetic rig flatters; Mac only |
| A1 | **Jobs and animation LOD.** A retained job list through `evaluate_all`; an `AnimView` generalising `SpringView`: update interval by distance on tick boundaries, IK and springs off far, hold off screen. | `sapphire_anim/src/character.rs`, `sapphire_stage/src/{animate,anim_graph}.rs`, `fight.rs:1494` | A0 | Allocation tests; near poses hash-identical (H7's method); bench at 15 and 60 | Every tick near; halved past 12 m, quartered past 24 m on Desktop-Low and mobile | M (4) | Job wake-ups can lose to serial at small counts; never LOD the simulation-owned pair |
| A2 | **Character kinds.** A playable is a document, as `EnemyDocument`; party members own mesh, skeleton, graph, springs and blade. | `sapphire_moves`, `sapphire_stage/src/{content,scene,lookdev}.rs`, `game_combat/src/{fight,arena,party_looks,blade}.rs`, `sapphire_gameplay/baked` | A0's hero as placeholder | `party-swap` and `arena-party-*` unchanged; a new scenario and replay with a second kind | same | L (8) | Index assumptions (W4 and C2 each crashed there); a second baked set |
| A3 | **Party HUD and touch swap.** | `game_ui/src/{hud,touch}.rs`, `hud.ui.ron` | none (works on recolors) | `game-hud-party-*` goldens; text-size and contrast tests | same | S (2) | Phone layout overflow |
| A4 | **Chain cloth.** Skirts, coat tails and long hair as sheets of existing spring chains: XPBD lateral and bend links between neighbouring chains, rig capsules, a floor. | New module beside `SpringBones`, sharing `SpringFrame`, `Substeps`, `reset`, `continue_from`, `sleep`, `SpringView`; a `cloth:` section naming chains | A0, A5 | Solver unit tests (30/60/120 Hz tolerance, frozen bits, no allocation); a shape test like `character_springs.rs`; `spring_us_*` row; a new golden | 4 / 2 / 1 iterations; off past 40 / 25 m | L (7) | The row is mostly spent already; the look needs a person |
| A5 | **Secondary-motion budgeter and floor.** A deterministic scheduler across characters (on-field first, staggered ticks); a floor plane from the foot probes. | `springs.rs`, `sapphire_stage/src/animate.rs` | A0 | Three heroes inside 0.5 / 1.0 ms; `lookdev-death-back` with springs on | by tier | M (4) | Clearance p99 |
| A6 | **Face layer and performance track.** `FaceLayer` writes `weights_mut` after the graph, by expression preset, honouring the overrides, with its own key so it runs while a dialogue holds the simulation. Timeline v2 adds `faces` and `looks` keys and any role. | `sapphire_anim`, `sapphire_timeline/src/{document,cooked}.rs`, `sapphire_sequencer/src/player.rs`, `game_combat/src/{timeline,dialogue}.rs` | none | Pure-function tests; `TimelineSimulation::digest` unchanged; close-up goldens | same | M (5) | `SolvedBody::morphs` is the simulation's, so keep this out of `sapphire_body_pose` |
| A7 | **Voice in conversations and cooked lip curves.** Wire `vo`; the cook writes a 60 Hz mouth curve per voice sound (envelope first, five visemes second), read at the line's playhead. | pipeline `audio.rs`, `sapphire_assets/src/voice.rs`, `sapphire_audio/src/voice.rs` | A6 | Cook byte test on soundgen's lines; `OfflineAudio` capture golden | same | M (4) | Float analysis diverging by architecture; bleeps prove timing only |
| A8 | **Reactions kit.** Directional hits, launch / air-hit / land (G1's missing `Ground \| Flight`), defeat dissolve through `CharacterInstance::opacity`, a presentation-only impact lean, a blade laid down. | `sapphire_moves`, `sapphire_gameplay`, `graph.rs`, `tools/motion` | A2 helps | A new replay scenario with old ones held; graph tests; contact sheets | same | L (8) | Checksums; generated motion |
| A9 | **Motion intake.** FBX to a committed architecture-neutral sampled clip; A-pose rest normalisation; humanoid-map presets; foot-lock at retarget; VRMA face tracks; clip bytes in the cook report. | `clip/{retarget,source}.rs`, `import/{fbx,vrma}.rs` | A6 for face tracks | One pinned clip per option; `tools/blender/fbx_fixtures.py` | n/a | M–L (6) | No licensed sample to test on |
| A10 | **Ambient characters.** Presentation-only NPCs from zone spawners: shared graph, pose sharing by phase bucket, joint-prefix bone LOD, no springs far. | `sapphire_zone` (presentation half), `sapphire_stage`, `skinning.rs` LOD prefix | A1, renderer LOD | Bench at 60 / 30; a street golden; zone digest and replays unchanged | 60 desktop, 30 mobile | L (8) | A zone format bump reaching `tables_hash` |

## 5. What only people can supply

- **A second model**, licensed for redistribution and modification in a public repo, plus its look-dev verdict.
- **Clips.** The hero's kit is 39 graph clips and 8 moves, so a second kit is that again. Packs that bar redistribution can't be committed.
- **Voice audio.**
- **Verdicts** on cloth, update-interval stutter and faces.
- **A phone.**
- **Placeholders meanwhile:** A0's generated hero, `tools/motion` clips, soundgen lines, tints for the HUD.

## 6. Decisions and spikes

1. **Cloth: chains or mesh.** I recommend chains: no renderer change, VRM authoring, cheapest. The research note's inference that Wuthering Waves uses bone physics is third-party and unverified.
2. **Lip-sync: cooked curves,** not runtime amplitude. They are deterministic and goldenable, and need no audio-thread read.
3. **Ragdolls: none in M5.** Authored reactions plus dissolve. What the reference games do here is unverified; the research notes don't cover it.
4. **`HeroineClearance` stays hero-only.** Spike pose-driven corrective morphs for new characters.
5. **Simulation-owned bodies stay the pair.** A crowd in the step is a budget and checksum call.
6. **May three heroes be on screen at once?** This sets the secondary-motion budget.
7. **Is "bones ≤ 140" joints or palette entries?** The hero has 155 joints and 130 palette entries.
8. **Clip compression: none** until a mocap sample is measured.
9. **Spike: stepped ("on twos") playback** per graph row.

## 7. Corrections

- **"No get-up clip" is stale** (m4-handoff line 367, m3-handoff line 184, spec line 2187). Since 6d848f63 the hero's `reaction_knockdown.keypose.ron` rises (`character.animgraph.ron:136`), the warrior has `recover.keypose.ron`, and brains press `rise` (`warrior.enemy.ron:66`).
- **"Retargeted chibi clips" is nearly gone.** The hero's graph takes only `dodge_back` and `dodge_forward` from KayKit; the rest, and all 14 of the warrior's, are generated key-pose documents.
- **Spec line 2012 is outdated:** "the simulation never reads a pose" no longer holds since #213.
- **Spec line 2010 isn't built:** "each character evaluates in its own job". `evaluate_all` has no caller outside its own test.
- **"11 opaque materials against 4" understates it:** the sidecar binds 13 slots, and desktop's limit of 6 is exceeded too.
- **"Springs only" omits the clearance pass,** which is where the row goes.
- **No committed character wears a skirt or coat** (the hero has a shirt and shorts), so cloth can't be shown on today's content.

### Critical files
- crates/sapphire_anim/src/springs.rs
- game/game_combat/src/fight.rs
- crates/sapphire_stage/src/content.rs
- crates/sapphire_timeline/src/document.rs
- tools/sapphire_pipeline/src/clip/retarget.rs
