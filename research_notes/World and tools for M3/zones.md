# Zones, sequencer and pipeline seams for M3

A scout's read for [the M3 plan](../../docs/m3-plan.md), 2026-11-03: read-only, nothing built or run, on `main` at 1335f14d and crate sources from crates.io. Its claims are an agent's reading, and a package proves them before leaning on them.

Paths are from the repository's root. `S/` is `apps/sapphire_sandbox/src`, `R/` is `crates/sapphire_render/src`, `G/` is `crates/sapphire_gameplay/src` and `A/` is `crates/sapphire_assets/src`. Read only, by search, so "not found" means not found by search.

**1. Static world**
- **Path to screen:** the assembly cooks to one mesh. `Content::town` (S/content.rs:953) blocks at Critical priority until loaded. `spawn_town` (S/scene.rs:1938) makes one entity, and it reaches the renderer as a `MeshInstance` in `FramePacket.instances`.
- **Packet:** it carries only `InstanceDeltas { changed, removed }` (R/frame.rs:147, :370, :701) for static geometry, with no groups or cells.
- **Culling:** CPU, per instance, by bounding sphere (R/view_cull.rs:120). Shadow cascades cull per cascade. A mesh is drawn whole, and LOD0 only (R/mesh_pool.rs:21). The town's sphere fails only if the whole town is off-screen.
- **GPU owners:** `MeshPool` owns the streams and indices. R/scene.rs owns the instance slots and draw list. `DrawList::build` (R/scene.rs:715) rebuilds when meshes or instances change.
- **Freeing:** no. "Meshes are not unloaded yet" (R/mesh_pool.rs:31). `release` (:594) is private and reached through `replace` (:388). Records have no free list, and `MeshHandle` is an index. The app does get `take_unloaded` → `gpu.unload` (S/content.rs:619).
- **Town size:** 13,855,328 bytes as `town.smesh`, from a worktree's desktop cook (.claude/worktrees/agent-ac3de61ab05ecc55e/target/content/loose/desktop/lookdev/town/). 232k triangles desktop, 153k mobile (docs/m2-handoff.md:79). Current vertex count: not found. Spec line 2636's 78k triangles is stale.
- **Seam:** cells stream as ordinary instance deltas plus a new `MeshPool::free`, so the packet needn't change. A slot-deferred free must wait on in-flight frames (my inference). The cook must split the town into per-cell meshes, because instance culling is only as fine as the mesh.

**2. Assets and packs**
- **Loading:** yes, async. Reads run on the IO thread, decodes on jobs, and GPU upload in `upload` on the server's thread (A/server/mod.rs:382, :868). `mount` itself blocks on the header and TOC (:542). Unload happens by LRU/budget and `unload_unused` (:965).
- **Mount:** runtime mount works, as audio shows (S/content.rs:1155). `remount` exists (A/server/reload.rs:90).
- **Unmount:** not found; explicitly later (A/lib.rs:88). Mounts are addressed by vector index (mod.rs:553), so removal needs care.
- **Handles on unmount:** undefined today. The nearest behaviour is remount, where assets a pack dropped "keep the version they have".
- **Pack choice:** purely by kind (tools/sapphire_pipeline/src/pack.rs:101): Sound/Cue to audio, everything else to shared. Level grouping: not found.
- **Constants, and what a replacement must discover:**
  - `LOOKDEV_SOURCES`, `COMBAT_SOURCES` (xtask/src/content.rs:118, :199): generated files that must exist. Needs a generator manifest, or each generator declaring its outputs.
  - `COOKED_MOVES` (S/tuning.rs:76): `.moves.ron` document → sidecar ID. Scan `.meta`s, or a pack TOC with names.
  - `PLAYER_DOCUMENTS`, `CLIP_DOCUMENTS`, `CLIP_SOURCES` (S/lookdev.rs:118, :109, :129): clip name → ID. Same discovery.
  - `SCENARIOS` (G/replay/scenario.rs:119): name, config, documents, setup **fn**. Needs an encounter document, and replays must still pin one.
  - `MOUNTAINS`, `PLANTINGS` (S/lookdev.rs:243, :337): zone placements.
  - `TOWN` (S/lookdev.rs:83), `WARRIOR` (S/enemy.rs:40), `PACK` (S/content.rs:129): the zone/cast manifest.
  - `COLLIDERS`, `SPAWNS` (`include_str!`, G/arena.rs:28, :31): deliberately compiled in so replays can't drift. Data-driving them needs the digest kept in the replay.

**3. Simulation**
- **Statics:** `Vec` plus one parry `Bvh` (Binned) built once in `PhysicsWorld::new` (crates/sapphire_physics/src/world.rs:137, :155). Queries walk the tree, log-ish by leaves. Bodies are scanned linearly in every query (query.rs:203, :285, :340, :400, :465). Statics are immutable, with no add/remove.
- **Whole zone at start:** yes, through `from_cooked` and `set_physics_world` "before any has spawned" (G/simulation.rs:320). Streaming colliders means rebuilding the world. Build cost at scale wasn't measured. A depth limit of 32 makes the build fail with an error (world.rs:17).
- **Pools:** `SimConfig.max_actors` is fixed at creation (G/actors.rs:223). Bodies = max × (2 + `MAX_HURTBOXES`) (:34). Replay limit 4096. Arena scenarios use 8 (scenario.rs:165, :178).
- **The simulation itself is not the problem:** `melee` spawns three. The breaks are above it:
  - `arena_fight_on`: one enemy, `set_warp_target(player, enemy)`, `give_brain(enemy, player)` (scenario.rs:370). `SpawnPoints` has `player`/`enemy` only (G/arena.rs:81).
  - G3: no factions, so "everyone hits everyone" (docs/m3-handoff.md). Two enemies hit each other.
  - Brain: one `target` per brain (G/brain.rs:101, :147), no director of enemies, no navigation.
  - `ArenaActors`: `player`, `enemy`, `seen: [_; 2]`, `enemy_id` (S/arena.rs:419–430); `fighters() -> [_; 2]` (:479); `follow` zips onto two roots (:591); party cast index 1 is the enemy (:611).
  - Targeting candidates are the single `enemy` (S/arena.rs:640).
  - `look_target`: `other` index and eye height constants (S/arena.rs:508).
  - `animate_fighters`: `[player, enemy]` tables and `index == 1` blade (S/scene.rs:1560); `play_fighters(player, enemy)` (:1826); `enemy: Option<(Entity, Entity)>` (:414).
  - Content: one `enemy_graph_clips` and `enemy_binding_of` (S/content.rs:1074, :1401); `GraphOwner::Enemy`; `enemy::` constants and one blade (S/enemy.rs).
  - Director: `FrameInput` has one `player`, one `target`, one each of `player_motion`/`target_motion` (crates/sapphire_camera/src/director.rs:117–121, :215). Three in frame are not represented.
  - HUD: `HudValues { player, enemy: Option }` and one `enemy` bar layout (crates/sapphire_ui/src/hud.rs:131, :238); no party slot.
  - Cues: per actor (crates/sapphire_cues/src/dispatch.rs:27), so they look fine. Budgets under a crowd unchecked.
  - Soft lock: nothing in a step reads it (handoff G5).

**4. Sequencer**
- **Camera:** `CameraDirector::set_takeover(Option<CameraPose>)` (director.rs:431), called each frame. Blends by tuning half-life; `cut()` (:456) snaps. Eye still collides and slides against colliders (no opt-out found). Shakes stay on top. Rigs need a `player` subject even in a cutscene.
- **TOD:** `play_cutscene`, then `seek_cutscene(s)` after each `tick` (crates/sapphire_world/src/tod/time_of_day.rs:200, :205, :222). `tick` adds the world step and clears the track past its duration, so a scrub must re-`play` after expiry. `seconds` isn't interpolated by alpha in `cutscene()`.
- **Animation:** each frame `arena.graph_input(index, alpha)` turns `ActorView` into a `GraphInput` (crates/sapphire_anim/src/graph.rs:160), and `arena.drawn` gives the transform.
- **Sim-free seam already exists:** `animate_graph`'s `graph_show` (`show.frame(alpha)`, `show.transform(alpha)`, S/scene.rs:~1600) and `animate_clip` (:1545) play with no fight. A timeline can be another `GraphInput` + `place` source, or a clip track.
- **Time:** `ScaledClock::push_scale(source, per_mille)`, `remove_source` (crates/sapphire_time/src/scaled.rs:64, :76), via `Simulation::world_clock_mut` (G/simulation.rs:592). App pause: `input.is_paused()` stops steps; the camera gets dt 0 (S/scene.rs:1912). The clock is in the checksum (`write_canonical`, scaled.rs:134), so an in-combat timeline scaling time must be recorded (inference). Whether `ScaleSource` has a cinematic variant: not checked.

**5. Pipelines**
- **Prewarm:** `prewarm_post(PostVariants)`, `prewarm_vfx` (R/renderer.rs:1234, :1508), both synchronous. Mesh pipelines build at upload in `build_mesh_pipelines` (:1203), so a streamed mesh with a new variant compiles there.
- **Counting:**
  - `PostStats::first_use_builds` (R/post.rs:435), surfaced as `post_first_use` (S/fight_bench.rs:178, :513).
  - `at_start`/`after_first_frame` are bench locals from wgpu's global report (S/fight_bench.rs:304, :373), logged only and whole-process. They aren't per-pipeline or keyed.
  - `shader_builds` is **hot-reload** build stats, not prewarm (R/shader_builds.rs:153).
- **Inventory needs:** public keys. `SurfaceVariant` is `pub(crate)` (R/pipelines.rs:~532), and `of_material` (:851) maps family and flags to a variant inside render. Render state lives in render.
- **Cooker vs recording:** the cooker is forbidden `sapphire_render` (xtask/layers.toml:165), but it has `MaterialFamily`/`MaterialFlags` from `sapphire_assets`. So it can write `(family, flags, position format)` per submesh, and the renderer derives the rest at load. Moving the mapping down also works. Watch out: skinned meshes draw from `draw_format()` (R/mesh_pool.rs:178), not the cooked format. Post, VFX, shadow and skinning pipelines are not mesh-derived, so they need app declaration or a recorded run. A recording also verifies the cooked list.

**6. App targets**
- **Mostly parameterised:** `App::load(package)` reads `[package.metadata.sapphire]`. The hard-coding is `SANDBOX` (xtask/src/platforms/mod.rs:81) used at xtask/src/main.rs:158, :165 and xtask/src/golden.rs:27. No `--app` flag found.
- **Android:** namespace `dev.sapphire.sandbox` (build.gradle.kts:14), `SandboxActivity.java` with `getLibraries` returning `"sapphire_sandbox"` (:29), manifest (:63), `ACTIVITY` (xtask/src/platforms/android.rs:48).
- **iOS and macOS:** no Xcode project. Only `platform/*/Info.plist`, generated from `App`.
- **CI:** ci.yml names the package 13 times; also gpu-tests-warp.yml, .github/actions/golden-capture/action.yml, and .github/scripts. The golden `--sandbox` argument assumes that binary's flags.
- **`game_app` needs:** a workspace member and a layers.toml entry (`kind = "app"`, features). `game/` doesn't exist. Metadata, an xtask `--app`, and a renamed or templated Activity. The shared code currently in S/ (`app.rs`, `content.rs`, `combat.rs`, `arena.rs`, etc.) must move to a library first, or `game_app` would have to depend on an app crate.

**Three surprises**
1. Neither half of the runtime can give memory back: no `unmount` and no mesh free. The address space and handle model assume everything stays once loaded, and `Content` holds handles for the whole run ("They stay loaded from then on").
2. Physics statics and `include_str!` arena data are compiled into determinism: world digest, replay checksums, immutable BVH. Streaming colliders within a fight changes the checksum and costs two CI rounds per move; rebuild-at-zone-load is the only cheap path.
3. "Two fighters" lives in the app, camera and HUD types rather than in the simulation. Multi-enemy looks like a data change but touches about a dozen array and field shapes. With no factions, a second enemy immediately hits the first.
