# M2 handoff notes

What M2 inherits from M1, for whoever starts M2. It is a map, not a copy. Each item gets a line or two and a pointer to where the detail lives:
- the spec, [engine-spec.md](engine-spec.md), by section;
- the M1 plan, [m1-plan.md](m1-plan.md), by package ID (each package's row carries its "As built" notes);
- or the code, where its crate docs keep a "Later milestones" section.

Written at M1's close (X2, 2026-10-27), on `main` at dd8ffa7 plus the A8 follow-up 2 (PR #77).

## Where M1 stands

- **Done, pending devices** (spec §10, "M1 status"). Every M1 package has landed. Exit test 1 (iteration) passes on the desktop, the iOS Simulator and the Android emulator. Exit test 2 (the look) has its scene, with budgets measured on this Mac only. Exit test 3 (golden images) passes on all three backends.
- **Why now:** there is no test hardware for now, so M1 closes with its device checks open, and work pauses before M2.
- **M2's own exit test** (spec §10) is the go/no-go gate: one character against one enemy feels right with a gamepad and with touch, replays are deterministic in CI, and Mobile-Low holds 30 fps while thermally throttled. The last part needs a phone.

## Device checks still owed

None of these blocks M2's code, but M2's exit test needs a phone.

- **Exit test 1 on phones:** the exit run's edits pushed to an iPhone and an Android phone ([m1-exit-run.md](m1-exit-run.md), "iPhone" and "Android emulator and phones"). The iPhone path (`cargo xtask run --platform ios`, `iproxy`) has never run on a phone and needs the user's signing identity.
- **Exit test 2 on phones:** the look-dev scene's frame times against the Mobile tiers, measured throttled after 15 minutes (spec §3), and the tier a phone's memory selects ([m1-exit-run.md](m1-exit-run.md), "What to measure and record").
- **iOS frame pacing (F7):** the ProMotion check, 60 Hz for 60 fps and no frames shown early (spec §6.2, "Device check").
- **Android frame pacing (F8):** a 90 or 120 Hz panel taking the refresh-rate hint, and the limiter's cadence on real vsyncs (spec §6.2, "Phone check").
- **Mobile MSAA (R7, Q5):** on-tile 4× MSAA's memory saving and cost on Mali, Adreno and Apple GPUs (spec §5.12, "Anti-aliasing and upscaling").
- **Post on phones (P2, P3):** with all its layers, the fused pass may be near or over §3.3's 2.5 ms on an A14-class phone. Measurements decide which layers, lens flare and blur taps each phone tier keeps (spec §5.12, "Post-processing").
- **A phone driver's shader compiles:** how long Adreno and Mali take to build a pushed shader. The emulator compiles through MoltenVK on the Mac, so it can't tell ([m1-exit-run.md](m1-exit-run.md), "Only a device can prove").
- **Q14:** whether Android's compositor rotates every frame, before choosing to render pre-rotated (spec §11).
- **Q9:** the device floors (A14, Snapdragon 865, 6 GB), throttled (spec §11).
- **Tier selection:** R6's rules can't see VRAM. A device database and measurements on the reference devices refine them (spec §3.1, R6's note).
- **Windows and Linux hardware** (open since M0): a run on real GPUs, Q4 (Windows' default backend) and Q3 (the Steam Deck) (spec §10, "M0 status"; §11). CI builds, tests and renders the goldens there on software rasterizers only.
- **Crash capture on Android:** minidumper, or Sentry's NDK SDK (spec §2, **Verify**). Crash upload itself is deferred by decision (spec §10, "M0 status").

## What M2 builds on

### Crates and layering

- **19 crates** in `crates/`, plus the sandbox app and three tools (`sapphire_pipeline`, `sapphire_testkit`, xtask). Each is placed in [xtask/layers.toml](../xtask/layers.toml), and `cargo xtask check-layers` enforces it (spec §4.1, [ADR 0001](adr/0001-crate-layering.md)).
  - L0: core, math.
  - L1: platform, jobs, time, crash.
  - L2: assets, ecs, look, reflect, rhi, serialize.
  - L3: input, render, tod.
  - L4: world.
  - Dev only: debug, hot reload, live link.
- **M2's crates are new placements:** `sapphire_anim`, `sapphire_physics`, `sapphire_audio`, `sapphire_gameplay` and the editor's start. The spec's module index (§5.0) gives their layers.
- **The renderer sees only `FramePacket`'s plain data** (rule R4, spec §4.6). Time of day's model is in `sapphire_tod`, and the look's plain types in `sapphire_look` (spec §4.1, the A8 follow-up's note).
- **The pipeline must not link the renderer or the world again.** When it did, the first cook after a shader edit took up to 72 s (spec §5.27, the A8 follow-up's note).

### Data path

- **Sources:** glTF and VRM 1.0 (A5, A6), FBX meshes (A10), PNG, TGA and EXR (A2), and RON documents: materials, ramps, ILM and face maps, layer masks, assemblies and time of day.
- **The cook:** per platform (BC on desktop, ASTC on mobile), through a blake3 content cache, into packs (A1 to A4; spec §5.10, §5.27).
- **Cooked formats** are Material 10, Mesh 13, pack 1 and probe grid 1. A changed layout bumps its version and the asset recooks; cooked data has no upgrades (spec §5.9).
- **The runtime** loads through an IO thread, decode jobs and a per-frame upload budget, behind handles. Hot reload remounts a pack and swaps what changed behind the same handles (A4, A8; spec §5.10).
- **The VRM's rig** (spring bones, colliders, constraints) is kept as data for `sapphire_anim` (A6; spec §5.10).

### Renderer

- **Core:** a wgpu 30 frame graph (R1) and compute skinning and morphs (R2). HDR post with grading LUTs, bloom, the diffusion glow, gradient layers and a lens flare (R3, P2). Cascaded shadows (R4), clustered lights (R5) and quality tiers in `tiers.ron` (R6). TAA with upsampling on desktop and on-tile 4× MSAA on phones (R7, P1). All in spec §5.12.
- **Anime characters** (C1 to C8; spec §5.12, "Anime character shading"):
  - ramp or band diffuse, ILM maps and SDF faces;
  - the light rig and stylized GI;
  - inverted-hull outlines;
  - the character prepass, with the depth-offset rim, hair and eyes through hair;
  - layered eyes and material layers;
  - the proximity fade and two-sided materials;
  - a keyed shadow hue, a camera rim, a soft bangs' shadow and light through hair.
- **The world:** one time-of-day key moves the sky, fog, ambient, shadow tint, post and the character rig (W1). The sky has clouds, lit cumulus and a nebula (W2, X1 part 2e). Fog and fog cards (W3), the environment shader (W4), probe grids and AO (W5), NPR foliage with wind (W6) and keyed emission. Spec §5.12 and §5.21.
- **Screen effects:** P3's stack of 16 layers and its presets, drawn in post's fused pass. These are the hooks; M2 authors the effects (spec §5.21, P3's note).
- **Tools around it:** the debug views (F5), the debug overlay (F5) and the input system (F4) (spec §5.12, §5.18, §5.20).
- **Rules M1 learned:**
  - A fragment stage that cuts pixels discards once, at its exit ([conventions.md](conventions.md)).
  - A feature builds its pipelines only when its work can run (spec §5.12, W5's note and post's lazy pipelines).
  - No workgroup barriers inside loops bounded at run time (spec §5.12, the WARP cost fixes).

### The look-dev stage

- **The scene:** `--look-dev wide|hero|sky|street|face` (X1) stands the VRM character in a Jinzhou-style town. The stage has its own day, sky, light rigs and plants (`content/lookdev/`, X1 parts 1 to 2f, C8's adoption).
- **The town** is one assembly mesh of 232k triangles in the desktop cook and 153k in the mobile cook, built from parametric shapes (spec §5.27; X1 part 2f). Around it stand 48 far-mountain sectors (part 2d) and 40 plants, each its own mesh (part 2b).
- **Its uses:** it has seven golden scenes (`lookdev-*`). It is the scene exit test 2 measures and the scene of the exit run ([m1-exit-run.md](m1-exit-run.md)).
- **Its gaps:**
  - Its goldens and benches keep W5's AO off, and the town has no probe grid (X1 part 1 left both to part 2, which kept them off).
  - The town is drawn whole, with nothing culled by view.
  - The character's eyes keep the model's rounder shape and lash line, and the blue hour's lit skin reads gray-mauve (X1 part 2c).

### Iteration: hot reload and live link

- **Desktop hot reload** watches the packs, the WGSL sources and the documents (A8; spec §5.10, §5.12).
- **Live link** (A9, protocol 3) pushes packs (only the changed assets), documents, WGSL and cvars to any `dev` build, and captures frames back: `cargo xtask link status|cvar|push|watch|capture` (spec §5.29). The app and the tool must come from the same commit, since other protocol versions refuse each other.
- **Shaders build off the frame thread** and swap in whole (the A8 follow-up 2, `sapphire_render::shader_builds`; spec §5.12).
- **`--offscreen`** serves a running scene headless, for the desktop leg (X2; spec §5.29).

### Golden images

- **95 scenes** in [scenes.rs](../tools/sapphire_testkit/src/scenes.rs), each with a golden per backend family in `tests/golden/`: Metal, lavapipe (Mesa 26.1.3) and WARP 1.0.21 (F6; spec §9, Q6). The README's "Golden images" section says how to run and update them.
- **Gates:** a FLIP mean of 0.01 and a 99th percentile of 0.3, by default. Existing goldens stay byte-identical when a feature lands: older scenes pin new features off (`--plain-post`, `--fog off`, `--ao off`, …).
- **CI** runs every scene on every pull request ([ci.yml](../.github/workflows/ci.yml)). Lavapipe runs in two shards and WARP in four, both required. Metal runs in one job, informational until it has run clean for two weeks (Q6).
  - At 87 scenes each shard took 11 to 14 minutes with its build (run 40493256145).
  - Each scene is a fresh process, and lavapipe and WARP compile and draw on the CPU, so work done with a feature off shows up as job time. Add shards as scenes grow.

### Budgets measured so far

All on this Mac's Apple M5 with release builds, which is none of §3's reference devices. No phone or Windows or Linux hardware has been measured.

| What | Measured | Where |
|---|---|---|
| Look-dev wide and hero shots, Desktop-High, 1080p | about 4.2 ms GPU, 0.22–0.33 ms CPU (p50) | X1 parts 2c, 2f |
| Look-dev street, Desktop-High | 6.6 ms GPU | X1 part 2b |
| Look-dev with the Mobile tiers' settings, at 1280×590 or 1080p | Mobile-High 1.1–4.3 ms, Mobile-Low 1.0–3.3 ms GPU | X1 parts 2a–2c |
| TAA at 1080p | 0.6 ms, against 0.8 | R7 |
| Shadows in the courtyard | 0.26 ms of passes and 0.65 ms of receiving, against 2.0 | R4 |
| GTAO; the ambient pass with a grid | 0.56 ms; 0.9–1.0 ms on Desktop-High, 0.1 ms on Mobile-Low | W5 |
| Foliage, 14 plants | 1.05 ms on Desktop-High, 0.27 ms on Mobile-Low | W6 |
| Sky | 0.04–0.32 ms on Desktop-High, against 0.1–0.3 | W2 follow-up, X1 part 2e |
| Screen effects while they play | past §3.3's 0.7 ms for post on the desktop tiers (the blur, the impact frame) | P3 |
| A cold shader push | visible after 1.8–2.1 s, no frame over 15 ms | A8 follow-up 2 |

## Deferred to M2 or later

### Toolchain and dependencies

- **wgpu 31:** pinned to 30 through M1, to be upgraded at the M1→M2 boundary (plan §3, "Pins"; §6). `bevy_ecs` and `bevy_reflect` 0.20 and egui 0.37 also move only between milestones (spec §5.6).
- **ctt's git patch:** `ctt` comes from an upstream commit with zstd 0.14 (for RustSec advisories). Drop the patch and `deny.toml`'s `allow-git` once a release newer than 0.6.0 carries it ([Cargo.toml](../Cargo.toml), `[patch.crates-io]`).
- **SDL 3.4.18, pinned exactly:** any upgrade must re-read SDL's main-callback internals against `ios_loop.rs` and `android_loop.rs` ([ADR 0003](adr/0003-ios-frame-pacing.md), [ADR 0004](adr/0004-android-frame-pacing.md)).
- **WESL and `encase`:** the spec's shader design names WESL imports (`wesl`) and `encase`, but the code uses neither. Shaders are WGSL files joined in order, and GPU structs are hand-written `bytemuck` types (spec §5.12, "Materials, shaders and PSOs"; `sapphire_render::shaders`). WESL imports are listed as later work in the renderer's crate docs.

### Animation, M2's core

- **`sapphire_anim` and Q7:** `ozz-animation-rs` has no offline tools and needs nightly Rust (`portable_simd`), which the stable toolchain rules out. Decide the runtime at the start of M2, and use C++ ozz tools in the cooker (spec §5.13, §11 Q7; plan §3).
- **Clips:** FBX animation isn't imported yet (A10; spec §5.27, FBX's "Not imported").
- **Skeletons as assets:** skinned meshes embed their skeleton until animation cooks skeletons of their own, and the VRM's rig moves next to them (spec §5.10).
- **Spring bones and constraints** are cooked, but nothing simulates them (A6; spec §5.10). The look-dev character holds one standing pose (X1 part 1).
- **Lip-sync curves** come later (spec §5.13).

### Gameplay, world and tools

- **The combat slice:** the combat core, party, camera director, one enemy AI, FMOD (Q8) and touch controls (spec §10; §5.15, §5.22).
- **Hit-stop:** it freezes the combatants' clocks, and M2 pushes a hit's screen effect preset in the step that applies it. A finish freeze decides whether effects then need a clock of their own (spec §5.21, P3's note).
- **Spawning in the fixed step** allocates, so gameplay spawns need pre-sized pools (spec §5.6, rule R6).
- **Zones** (M3) are to place the look-dev town as culled instances instead of one mesh (spec §5.21; §5.27, X1 part 1's "Later milestones").
- **Wind and weather per zone and hour:** `FramePacket::wind` and `weather` are plumbed, but nothing resolves them per zone and no shader reads the weather (spec §4.6).
- **Debug tools:** the debug-draw API, an entity inspector, hitbox and intent-buffer views and the replay recorder (spec §5.20).
- **CI:** combat replays, nightly cooks and device runs aren't in it yet (spec §8; §9 gates 2 and 4).
- **Latency:** end-to-end input latency is to be measured in M2 (spec §4.2).

### Renderer

- **PSO warm-up (M4) can grow from #77:** `shader_builds`' owned snapshots of what each owner built, its worker with error scopes of its own, cancellation, the swap at the top of `prepare` and the record of what was built.
  - A cache of pipelines by descriptor would let a build in flight keep pipelines built lazily meanwhile, instead of starting again.
  - Prewarming could move off the loading thread through the same path.
  - See spec §5.12 ("Materials, shaders and PSOs", the A8 follow-up 2) and the renderer's crate docs.
- **Still on the frame thread:** the debug views' replay copies, rebuilt after a swap while a view is shown (`debug_view.rs`), and the fallback to the compiled-in shaders when a pipeline built later fails under edited ones (spec §5.12).
- **Culling and LODs:** nothing culls instances by view, so every mountain sector draws every frame, and draws use LOD0. LOD selection, GPU culling and multi-draw-indirect come with culling (X1 part 2d; the renderer's crate docs).
- **Reflection probes:** relightable reflection probes slipped from W5 to early M2. Reflections read the prefiltered sky cube today (plan W5; spec §5.12, W5's note). SSR for the characters' reflection add on desktop (C7).
- **Upscalers:** a spatial upscaler on phones, and DLSS or MetalFX on desktop. TAA's inputs are already what they take (R7; spec §5.12, "Anti-aliasing and upscaling").
- **Effects on P3's hooks:**
  - cut-ins, which may need the character mask carried through TAA;
  - finish freeze and halftone;
  - C7's screen-pattern and SDF-reveal effects.
  - See spec §4.6 and §5.12 ("Post-processing"), and plan C7.
- **Look gaps X1 found:** depth of field, light shafts and water; a denser starfield inside the nebula and finer billows in the cumulus (X1 parts 2a, 2c, 2e).
- **Shadows:** characters shadowing each other, alpha-tested casters of the other families, cheaper receivers, and foliage casters without flutter or with a caster LOD (R4, W6; the `shadows` and `foliage` module docs).
- **Foliage:** impostors and camera-facing leaf clusters (W6), and skinned foliage (spec §5.12).
- **Fog and environment:** volumetric fog on desktop, the wet surface model, and screen-space edges for the environment (spec §5.12, "Environment").
- **Local lights:** cluster occupancy on the GPU, light layers and local-light shadows (the `lights` module docs).
- **Characters' remaining items** are listed in spec §5.12, in C5's, C6's and C7's "What remains" (alpha-tested eye parts in the prepasses, the eye mask under MSAA, and more).
- **Items the crate docs still file under "M1":** mesh residency with generational handles, a staging ring for mesh uploads, and draw sorting by material and mesh within a pipeline's group ([lib.rs](../crates/sapphire_render/src/lib.rs), "Later milestones").
- **Everything else:** each renderer module's own "Later milestones" section lists the rest.

### Assets and pipeline

- **Group packs:** packs per zone, character and encounter, from root documents (M2, M3). Leaving assets nothing reaches out of packs is the same work (spec §5.10; §5.27, A4's grouping rule).
- **Loading:** memory-mapped packs, the IO thread's priorities and texture mip streaming (spec §5.10).
- **The cook:** a shared cache and pruning (spec §5.27, A1's note), and a watch mode that cooks on save (`sapphire_hot_reload`'s crate docs).
- **FBX pieces in assemblies** (A10's note).
- **Pack signatures** come with `sapphire_patch` (M4) (spec §5.10).

### Content

- **A second character:** the plan picked VirtualCast's Seed-san (VPL 1.0, credit required) as the second test character, but it was never added. pixiv's sample is the only character (plan §3, "Test content").
- **Rules for new sources:** a license that allows redistribution and modification, and each folder's license, `SOURCE.md` and `MODIFIED.md` ([content/third_party/README.md](../content/third_party/README.md); plan §3). HoyoToon (GPL-3.0) is read for technique only, and no code is taken from it (plan, track C).

### Live link, platforms and devices

- **Live link:** a pairing token (spec §5.29, "Security"). The quality tier table and the bindings as hot-reloaded documents (`sapphire_hot_reload`'s crate docs).
- **Store and signing work (M4):** device signing, an Xcode project, the local-network prompt's text and store compliance (spec §5.29, §6.7, §7.3).
- **Frame pacing:** a desktop frame cap on F7's pacer, and timed presentation if a device check finds frames shown early (spec §6.2).
- **Crash upload:** deferred by decision (spec §10, "M0 status").

## Known limits and risks

- **No phone, no Windows or Linux GPU.** Every mobile number comes from the M5 with the Mobile tiers' settings, the iOS Simulator or the Android emulator, and both of those draw on the Mac's GPU (plan §6). M2's exit test needs a throttled Mobile-Low phone.
- **§3's budgets are proposals** until M2 measures them on the reference devices (spec §3).
- **The look rests on generated content.** There is no artist: ramps are data, and masks and maps are generated in code or baked headless (plan §6; A7, X1 part 2c).
- **Software rasterizers aren't GPUs.** Lavapipe and WARP differ from real GPUs on hard edges (plan §6). WARP also has traps of its own:
  - It drops or shifts huge triangles the near plane clips, so keep ground near cameras in cells of 2 m or less (spec §5.27, X1 part 1).
  - Workgroup barriers in loops bounded at run time cost minutes per scene (spec §5.12, R5's and W5's WARP fixes).
- **Metal's compiler has miscompiled early discards**, so every fragment stage that cuts pixels takes one exit ([conventions.md](conventions.md)).
- **Golden job time is a cost signal.** A feature that is off must build no pipelines and dispatch nothing (spec §5.12, W5's note).
- **The anime character shader is full** (spec §5.12):
  - all eight permutation bits are taken (C8);
  - its fragment stage binds all 16 sampled textures and its 4 storage buffers (W5, C2).
  - So a new feature is a uniform branch, shares a texture slot, or joins `CharacterData`.
- **The look-dev town doesn't scale as built:** one mesh drawn whole, nothing culled by view (X1 parts 2b and 2d).
- **Pinned dependencies** (wgpu 30, SDL 3.4.18, ctt from git) each need a deliberate upgrade (above).
- **M2 is the go/no-go gate.** If tuning still feels slow, the off-ramp moves gameplay and tools to Godot with gdext and keeps the Rust systems (spec §10).
