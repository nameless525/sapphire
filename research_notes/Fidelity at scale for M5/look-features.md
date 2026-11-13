# Rendering features the look still lacks: a survey for M5

A design survey's read of the code on 2026-11-13, at 27f29b5f (release 0.1.0), for [the M5 plan](../../docs/m5-plan.md). Nothing was built or run: every finding is from reading the source, the spec and the handoffs, and "unverified" marks what the source could not confirm. Line numbers are that commit's. Sizes are the surveyor's, in agent-days; the plan splits and re-sizes its packages. A package that leans on a finding checks it against the code first.

Sizes: S 1-2, M 3-5, L 6-10 agent-days. Tiers: DH (Desktop-High), DL (Desktop-Low), MH (Mobile-High), ML (Mobile-Low).

## 1. As built (hooks)

- **Reflections.** One prefiltered sky cube (`ambient.rs:104-122`; sizes 128/128/64/32). The environment reads it only through a probe grid's sky visibility. Characters' reflection add reads it on every tier (`anime_character.wgsl:997`), so metal indoors reflects open sky. No probe type, bake or SSR exists.
- **Shadows.** Cascades hold environment casters only. Characters cast into an 8-tile atlas (desktop) or as capsules (phones), received by the environment alone (`shadows.rs:19-26`, `frame.wgsl:942`, `:976`). Only foliage has masked casters. `GpuLight.cone.zw` is free (`lights.rs:129`).
- **Fog and weather.** Every world pass ends in `fogged()` (`frame.wgsl:636`), the one place a froxel lookup would go. `FrameUniforms.weather` reaches the GPU unread (`frame.wgsl:57-60`). `volumetric_fog` and `max_particles` are plain data (`quality.rs:246-251`, `:880`).
- **Zones.** `ZonePresentation` has `probes`, `day`, `rigs`, `lights` (`sapphire_zone/src/cooked.rs:182-201`), but a trip applies none of them (spec line 2719). It has no sky material, wind, weather or fog; wind is a code constant (`sapphire_stage/src/scene/zone.rs:369`).
- **Depth of field, decals.** No code. The sequencer's `Shot` has eye, look-at and fov only (`player.rs:54`). VFX quads face the camera or their velocity only (`sapphire_vfx/src/document.rs:97`).
- **VFX.** CPU sprites of six generated shapes, plus ribbons. Desktop draws them after the TAA resolve over a copy of the resolved frame (`vfx.rs:8-29`), which is a ready distortion source. Phones draw them inside the main pass with no depth to read.
- **Characters.** C7 already built matcap, highlight, reflection add, stockings (opaque sheer), breathing emission and a per-character dither fade through every pass. `Coverage::Transparent` is unsorted, one-sided, outline-less and in no prepass.
- **Screen effects.** Five effects in two fused-pass variants, with a 16-layer stack. Insets (`inset.rs`) are extra character views with their own prepass, `MAX_INSETS` 2.

## 2. Already designed

- **Reflection probes, three different sketches:** spec 1243-1244 (relit at runtime on desktop and MH, static on ML; BC6H or RGB9E5); `ambient.rs:195` (baked cubes blended by volume); research note §3 (a mini G-buffer per probe, one relit per frame).
- **Fog:** `fog.rs:90-98` (froxels lit by cascades and local lights, shafts, density volumes). The research note wants about 160×90×64 froxels, and suggests radial shafts at quarter resolution on phones.
- **Wetness:** `environment.rs:33-36` (Lagarde's model, plus a porosity input).
- **VFX:** spec 2322-2332 (compute spawn/update/compaction with indirect draws, flipbooks, mesh particles, a half-resolution distortion buffer, decals, half-resolution smoke).
- **Screen effects:** `post.rs:212-219`. Cut-ins are "views, not post" (`screen_effects.rs:95-98`).
- **Shadows:** `shadows.rs:142-163`; `lights.rs:41-56` ("a few hero lights").
- **Depth of field:** spec 1702 says desktop only, in cutscenes.

## 3. Ranking by the look

1. **Reflection probes with wet surfaces.** Interiors and night streets are where a zone-and-hub game lives, and today they reflect sky or nothing. This is the biggest gap.
2. **VFX meshes, flipbooks, distortion.** Combat reads through effects, and six procedural shapes cannot carry it.
3. **Ground decals for telegraphs.** They are gameplay readability.
4. **Depth of field** for close-ups, the showcase and ultimates. The look target's notes name it twice.
5. **Halftone, CRT and glitch effects, and cut-ins.** Cheap, because the hooks exist.
6. **Dissolve and driven emission** for deaths, swaps and charge glows. Cheap, on C7's fade.

- **Cheap:** sky content, masked casters, characters shadowing each other, wetness, radial light shafts.
- **Traps:** froxel fog, point-light shadow cubes, planar reflections, full-screen SSR in a forward renderer, 50k GPU particles (budgeted, but the look doesn't need them), sorted translucent cloth, any deferred decal pass, re-keying the permutations.

## 4. Hard constraints

- **Bindings.** The device asks for `Limits::downlevel_defaults()` (`sapphire_rhi/src/caps.rs:267-275`): 16 sampled textures and 4 storage buffers per stage, 4 groups. wgpu counts by layout visibility across the pipeline layout (`wgpu-core device/resource.rs:3882-3904`), not by what the shader reads.
- **No input attachments** in wgpu 30; none found in its source.
- **Cube arrays are optional.** Metal needs iOS family 4 (`wgpu-hal metal/adapter.rs:583`); the iOS Simulator lacks them and indirect execution (ADR 0002:25); Vulkan's `imageCubeArray` is optional. `GpuCaps` has no field for it.
- **Phones.** 4× MSAA in tile memory, no TAA, no resolved colour, no readable depth. MH has a scene prepass only while GTAO is unshed. So SSR, depth-reconstructed decals, soft particles and depth-based blur have no input on ML.
- **Phone lights.** 8 per frame, 4 per instance (`lights/objects.rs:30`), and a zone cell is one 32 m instance.
- **Goldens.** Old scenes stay byte-identical and pin new features off; a new scene commits Metal only. A feature that is off builds no pipelines and dispatches nothing. A new pipeline owner must join `PipelineBuilds`, `PipelineInventory`, `Held`, `shader_job`, `warm()`/`adopt()`, the hot-reload counts and `declares_every_setting`. Editing shared WGSL moves lavapipe by one code value everywhere, and can move Metal through fast math.
- **WARP.** No workgroup barriers in loops bounded at run time; no early `return` under a per-pixel condition; ground in cells of 2 m or less; FXC recurses deep on `anime_character.wgsl`, so grow that file sparingly.
- **Budgets.** §3.3 has only DL and ML columns, and no row for reflections, depth of field, decals or water.

## 5. Proposed work packages (in order)

| # | Package | Goal and files | Deps | Headless proof | Tiers | §3.3 row | Size, risk |
|---|---|---|---|---|---|---|---|
| G0 | Frame group per family | Split group 0's layout (`pipelines.rs:993-1024`) so characters drop the probe atlas, bricks and GTAO (3 slots free) and the world drops character depth, IDs and the character buffer (2 textures, 1 storage buffer free) | none | All goldens byte-identical; a layout test of free slots; hot-reload counts | all | none | M; lavapipe re-baseline |
| Z0 | Zone look | Apply day, rigs and probes on open and on trips; add sky material, wind, weather, fog to `ZonePresentation`; lift the exit refusal | none | A `zone-trip` golden pair; trip test | all | none | M; overlaps the world survey |
| R1 | Probe bake | Cooker bakes local cubes per time-of-day key with sky visibility in alpha, on the probe grid's tracer | Z0 | CPU tests; "Cooks agree" | cook 128/128/64/32 | cook | L; cross-arch determinism |
| R2 | Probe runtime | `ambient.rs`: blend keys by `ProbeKeys`, add the live sky by alpha, prefilter one probe a frame; environment picks by grid cell with box parallax; characters by an index in `CharacterData` | G0, R1 | GPU test (a red room reflects red); `reflect-room-*`; nothing built without probes | DH/DL full; MH no parallax; ML characters only, one cube composed at load | Clustering/AO, Forward opaque | L; array fallback |
| W1 | Wet surfaces | `environment.wgsl` reads `frame.weather`; porosity; a weather override in `sapphire_tod` | Z0, R2 | `wet-street-night`; old goldens exact | all; ML darkening only | Forward opaque | M; fast-math drift |
| V1 | Effect meshes | A fifth mesh family: unlit, scrolling UVs, erosion, flipbook frames, drawn where sprites draw | none | `vfxmesh-*`; nothing-built test; `gpu effects` row | all; phones no soft fade | Particles | L; authoring without an artist |
| V2 | Distortion | Offset buffer applied in the fused pass | V1 | GPU test; golden | DH/DL half-res; MH quarter; ML off | Particles | M |
| D1 | Ground decals | `Facing::Ground` quads plus SDF fill shapes for telegraphs and scorches, depth-biased in the main pass | none | `decal-telegraph`; z-fight test on three backends | all | Particles | S-M; uneven ground |
| P1 | Depth of field | Half-res gather after the resolve from prepass depth; backdrop blur in insets; focus keys on `Shot` | none | `dof-closeup`; `gpu post` row | DH/DL; MH insets only; ML off | new row | M |
| P2 | Effects pack | Halftone, CRT/glitch, flash, desaturate-keeping-characters in the existing colour variant | none | Goldens; no new pipelines | all | Bloom/LUT | S-M |
| P3 | Cut-ins | Full-frame inset views composed by UI | P2 | `cutin-*`; inventory declared | all; phones at lower `max_side` | unbudgeted, while active | M; desktop inset supersampling cost |
| C1 | Character materials | Noise dissolve with an emissive edge; driven emission and flash in `CharacterData`; per-region opacity through the fade dither; a glass branch under `LAYERS` | none | `tests/gpu/layers.rs`; `dissolve-*`; old goldens exact | all | Forward opaque, transparents | M; FXC stack |
| S1 | Shadows pack | Masked casters for hair cards; characters receive others' tiles (desktop) or capsules (phones) | G0 | GPU tests; `shadows-party` | all | Shadows | M |
| S2 | Spot shadows | At most 4 cached spot tiles in the character atlas | S1 | GPU test; bench | DH 4, DL 2, MH/ML 0 | Shadows | M |
| A1 | Light shafts | Radial shafts in the fused pass | none | `shafts-golden-hour` | DH/DL/MH; ML off | Bloom/LUT | S-M |
| K1 | Sky content | Denser stars in the nebula, finer cumulus (`lookdev::sky_maps`) | none | Five look-dev goldens move | all | none | S |
| C2 | Material merge | Cook-time atlas merging same-part materials | none | Draw-count test; golden within gates | all | Forward opaque | M; mip bleed |

Core: G0 through C1. Stretch: S1 through C2. Spike only: froxels, SSR, GPU particles, projected decals, a water plane.

## 6. Decisions and spikes

- **Probe design.** I recommend per-key baked cubes plus the live sky, not a G-buffer relight: the cooker already traces per key, `ProbeKeys` already blends, and the research note records ZZZ's city as four discrete phases (community-sourced).
- **Probe storage.** Cube array where the device has it; otherwise one nearest cube. Spike octahedral 2D arrays (S).
- **§3.3 needs rows** for reflections and depth of field, and columns for DH and MH. The maintainer's call.
- **Phone depth.** I recommend no prepass on ML; every feature above has a no-depth fallback.
- **Decals.** Quads first. Clustered projected decals need G0's freed storage buffer and break on phones' 4-per-cell lists; spike later.
- **GPU particles.** Defer. Without indirect draws they need a fixed-capacity draw; spike only if rain or motes demand it.
- **SSR.** Spike after R2, as an in-shader march for flagged materials only (DH/DL).
- **Froxels.** Spike after A1; WARP cost is unknown.
- **VFX reference study.** No research note covers how the reference games build effects; my ranking of meshes over particles is unverified.
- **No Zenless Zone Zero reference frames exist** among the frames kept outside the repository, only Wuthering Waves'.

## 7. Corrections

- **"The character shader binds all 16 textures"** is true of the shared layout only. The shader reads 13; of W5's four it samples only `reflection_cube`.
- **"Eight permutation bits" is the spec's own cap** (`Permutation(u8)`, `pipelines.rs:291`), not a GPU limit, and `OUTLINE` selects a pass, not a shader. Spec line 1294 still says "one free"; C8 took it.
- **"Metal and matcap" are built** (C7), as is opaque sheer cloth. What is missing is translucent cloth, glass, dissolve, driven pulses, screen pattern and SDF reveal.
- **"11 opaque materials against 4"** is the look-dev VRM against §3.4's row (6 desktop, 4 mobile; spec 156, 245; `m3-handoff.md:200`). Four is one per shader part. The generated enemies already use one material of six regions.
- **Cut-ins "may need the mask through TAA"** conflicts with `screen_effects.rs:95-98` and with M3's insets, which make them views.
- **`environment.rs:31` promises SSR on Mobile-High;** it has no history or resolved colour to trace.
- **`fog.rs:98` waits for a Mobile-High prepass that already runs** for GTAO, though the ladder sheds it.

### Critical files
- crates/sapphire_render/src/pipelines.rs
- crates/sapphire_render/src/ambient.rs
- crates/sapphire_render/shaders/frame.wgsl
- crates/sapphire_render/src/vfx.rs
- crates/sapphire_zone/src/cooked.rs
