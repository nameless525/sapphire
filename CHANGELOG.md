# Changelog

Sapphire's releases. Each release is tagged `v<version>` on `main`, and its version is the workspace's (`[workspace.package]` in [Cargo.toml](Cargo.toml)), which every package and app carries. Until 1.0, each milestone that lands is a minor release.

## [0.2.0] - 2026-11-17

Milestone M5, fidelity at scale. It asks whether the engine can hold the reference games' picture at their density on every tier: it builds a generated benchmark at the spec's content budgets, measures 0.1.0 on it, then builds what the measurement and the look ask for. It adds no game and tunes no fight; the Lantern Ward is as 0.1.0 left it. [The M5 plan](docs/m5-plan.md) has every package's "As built" note, and [the M6 handoff](docs/m6-handoff.md) what is deferred and owed.

### The benchmark
- `bench/` holds a generated district and cast as documents only, and `cargo xtask bench-content` cooks them into `target/`: never committed cooked, never shipped, and repeated byte for byte.
- The `budget` district: 8 × 8 cells of 32 m, 469 buildings of 40 variants, about 7,000 merged props, 518 lights, and textures at twice the desktop's pool. A cast at the spec's rows: heroes of 80,000 triangles and 200 joints, enemies of 8,000, and ambient figures.
- Counters for what each pass submits, the levels drawn, culling, instances, skinned vertices and the asset pools, and a baseline of 0.1.0 on the district in spec §3.8.
- A committed scenario and replay walk a party of three kinds up the district's avenue into a fight of twelve.
- `cargo xtask bench-scale` prepares, runs and judges the exit tests' walks, counted or timed, on this Mac and on the iOS Simulator and the Android emulator.

### Draws and culling
- A sorted draw list per view, and runs of equal draws as one instanced call, on every backend with one path.
- Culling in every view, exact against no culling.
- Levels of detail chosen by their cooked error in pixels, with hysteresis, for static meshes, zone cells (whose borders the cook locks) and characters a scene lets draw coarser; a level-of-detail bias as a rung of the thermal ladder.
- Shadow cascades kept while nothing in them moves, and a mesh pool that gives memory back under a trip's wipe.

### Memory and streaming
- Loads admitted against a cap on bytes decoded and waiting, and texture pools as budgets by tier.
- Textures cooked as a tail and one asset a level, raised and lowered in place, and a streamer that keeps each texture at the level the views need inside a hard pool.
- A hard texture pool, held by the asset server's own count, built so a surface newly in view reaches its level inside 30 frames; a zone's arrival holds its wipe until the first view's textures are at their plan, for 300 updates at most, and each wait is counted under what held it.
- The streamer's update makes the same decisions for less work: each texture's shape and its bytes from every top kept between passes, the lowering ranked only when the plan is over the pool, and passes that would do nothing skipped.
- Texture sizes capped per cook.
- Zones bring their own day, light rigs, probes, sky, wind, weather and fog, on opening and after a trip.

### The look
- Reflection probes baked per time of day and blended by the hour, and wet surfaces with puddles under a zone's weather.
- Effect meshes with scrolling patterns, erosion and flipbooks; distortion; ground decals for telegraphs and scorches.
- Depth of field from a timeline's focus keys, an effects pack (halftone, CRT, glitch, a flash and a desaturation that keeps characters in colour), and cut-ins.
- Dissolves, driven emission, per-region opacity and glass on characters; characters shadowing each other; cached spot-light shadows; light shafts; a denser sky.
- An edge-adaptive upscaler and sharpen for the phones, after FSR 1; merged character materials; character capsules binned by light cluster.

### Characters
- A playable kind is a document, with its own mesh, skeleton, graph, springs and weapon; a party of three kinds, and the party on the HUD.
- Animation evaluated in jobs, with levels by distance, and secondary motion's budget shared across characters.
- Chain cloth: XPBD links between a rig's neighbouring spring chains, authored per kind and solved per tier.
- Faces driven by timeline keys and conversation lines, and voiced conversations whose mouths follow curves cooked from each voice sound.
- Ambient characters who stand about a zone and walk its streets, sixty on the desktops and thirty on the phones, presentation only.

### Tools and CI
- Debug views for the level drawn, a texture's wanted and resident levels, culling, instanced runs and overdraw, and a counts panel against the spec's budgets.
- A scale lane that walks the small district's desktop tiers on lavapipe on every pull request and holds every count row, with lavapipe's few differences and their reasons beside the table, and keeps each walk's counts frame by frame, with the pipelines each mesh pass set.
- The cold boot's long frames traced, frame by frame: the stand no longer reads its zone's look on the frame's thread, and what is left is a frame waiting on its own GPU work while the warm-up compiles.
- When each frame's GPU work ended, on the GPU's clock, in the timed trace.
- The benchmark report's rules for Metal, which overlaps its passes: the prepasses and shadows judged as one row, a pass read on frames with no upload in flight, and a round with the display's 60 Hz lock void for its GPU rows; exit test 2 judged by each wait's frames and what held it, its 30 frames only on runs paced at the tier's rate, and failed by an arrival held to its bound; exit test 6 judged on the crowd's walk, with the held crowd shown.
- WARP's GPU tests in eight shards in CI.
- "How to add" recipes, with no Rust step, for a playable kind, cloth, a reflection probe, a zone's weather, ambient characters, a level-of-detail setting, a face track, a focus key, an effect mesh and a decal.

### Known at release
- Exit test 1 is not met on Desktop-High: the district fight's GPU frame read p95 12.03 ms against its 12.00 ms row on the final run's one valid round. Every other verdict the run judged passes. Some units on Desktop-High, Mobile-High and Mobile-Low have no valid round, because the display's 60 Hz lock voided each, so they are not judged. The maintainer finished M5 there; [the M6 handoff](docs/m6-handoff.md) carries both, and [the exit run](docs/m5-exit-run.md) has the verdicts.

### Known limitations
- Nobody has played anything yet, and nothing has run on a physical phone, a Steam Deck, or Windows or Linux hardware. Every figure at scale is this Mac's.
- The benchmark is generated, so it proves cost, not beauty: an artist's district will cost more.
- D7 (cooked visibility) and D11 (a GPU-driven spike) were cut, and a MetalFX spike waits on its crate. [The M6 handoff](docs/m6-handoff.md) lists the rest of what is deferred, with what would reopen each.

## [0.1.0] - 2026-11-13

The first release: milestones M0 to M4, from bring-up to ship readiness. The renderer, the content pipeline, combat, the world and the tools are all built. The game is the **Lantern Ward**, a slice from the title to its boss, in English and Japanese. It runs on macOS, the iOS Simulator and the Android emulator. Windows and Linux build, pass their tests and render the golden images in CI. [The M5 handoff](docs/m5-handoff.md) says what each milestone left owed.

### M0: Bring-up
- A workspace whose crate layers `cargo xtask check-layers` enforces. CI on three operating systems.
- `sapphire_platform` on SDL3 and `sapphire_rhi` on `wgpu` (Metal, Vulkan, DX12), on all five platforms.
- Tracy profiling, and crash reports with minidumps on desktop.

### M1: Data path
- Content: `bevy_ecs`, reflection and versioned serialization. An asset runtime that loads by ID from packs.
- The pipeline cooks glTF, VRM 1.0 and FBX per platform (BC7 or ASTC) through a content-hash cache.
- The renderer: a frame graph, compute skinning, forward lighting, shadows, ambient probes and AO, HDR, bloom, a diffusion glow and a custom grade.
- Anime character shading: authored ramps and ILM maps, SDF face shadows, layered eyes, hair highlights, depth-offset rims and stable inverted-hull outlines, under the character's own light rig.
- One time-of-day key that drives the sky, fog, ambient light, shadow tint, the character's light and the grade together.
- Iteration: hot reload, live link to any device, and the debug overlay.
- Golden images on Metal, Vulkan (lavapipe) and DX12 (WARP).

### M2: Combat slice
- Animation: our own runtime with inertialization, IK and spring bones.
- Physics, and a deterministic fixed 60 Hz simulation whose replays are checked tick by tick.
- The combat core: moves as data, hit windows, hitstop, perfect-dodge slow motion, cancels and party swaps.
- The camera director, enemy AI, and audio on `kira`.
- Visual effects and combat screen effects, with live tuning.
- Touch controls and the timeline editor.

### M3: World and tools
- Zones and their transitions, navigation, the sequencer, dialogue and subtitles.
- Luau scripting for encounters.
- The UI with its HUD and menus, saves with migrations, and localization in English and Japanese.
- The editor's document model with undo and play-in-editor.
- Content as documents: a new enemy, encounter, boss or cutscene needs no Rust.
- The Lantern Ward, from the title to the boss.

### M4: Ship readiness
- Pipelines warm off the frame, behind a boot screen, so a scripted playthrough draws with zero shader hitches.
- Signed releases that patch themselves by downloading only the blocks that changed.
- Four quality tiers, with dynamic resolution and a ladder that steps down under heat.
- Crash symbols, so a crash in a shipping build names its function, file and line.
- Accessibility settings: remapping, subtitles, reduced flashes and larger text.
- `cargo xtask package` builds store packages for all five platforms, nightly in CI.
- Playtest builds that bring sessions back as replays.

### Known limitations
- Nobody has played the game yet, and nothing has run on a physical phone, a Steam Deck, or Windows or Linux hardware.
- The game's identity is a placeholder (`com.example.sapphire.game` in [platform/identity.toml](platform/identity.toml)). The release keys and patch host in `platform/patch.toml` are the maintainer's to set. No store has seen a build.
- [The M5 handoff](docs/m5-handoff.md) and the spec's [milestones](docs/engine-spec.md#10-milestones) list what each exit test still needs from a device, an account or a person.

[0.2.0]: https://github.com/nameless525/sapphire/releases/tag/v0.2.0
[0.1.0]: https://github.com/nameless525/sapphire/releases/tag/v0.1.0
