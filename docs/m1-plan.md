# M1 plan: the data path and the anime look

- **Status:** proposal, for review before work starts.
- **Date:** 2026-10-25.
- **Milestone:** M1 (spec §10). It builds on M0 (`main` at 6260ae1).
- **Inputs:** the spec (§3 budgets, §4 architecture, §5 module specs, §11 open questions) and three research notes under [research_notes/Anime look for M1/](../research_notes/Anime%20look%20for%20M1/):
  - [character_rendering.md](../research_notes/Anime%20look%20for%20M1/character_rendering.md): how WuWa, ZZZ, Genshin and Star Rail shade characters.
  - [world_lighting_post.md](../research_notes/Anime%20look%20for%20M1/world_lighting_post.md): environments, lighting, atmosphere, post and mobile.
  - [content_ci_crates.md](../research_notes/Anime%20look%20for%20M1/content_ci_crates.md): test content, golden-image CI and crate status.

## 1. What M1 is for

M1 turns the M0 bring-up into a real engine data path and a renderer that can carry the look of Wuthering Waves and Zenless Zone Zero.

That look is **not "toon shading."** It is an anime picture in which characters, world, light, atmosphere and post-processing are art-directed together:
- anime characters with authored shadow shapes;
- a stylized but physically grounded world;
- gradients and atmosphere everywhere;
- one time-of-day key that moves all of it at once;
- a soft, glowing, non-filmic grade.

M0 drew one mannequin with a two-tone light split. M1 must make the whole frame.

### Exit tests

1. **Iteration (from the spec):** editing a ramp, a material parameter or a time-of-day key shows up on desktop *and* on a physical phone without a restart.
2. **Look (new):** the **look-dev scene** runs at the tier budgets of §3 on all five platforms. The scene is a VRM character with authored ramps, ILM and an SDF face map, in a small CC0 environment slice, through a full day/night cycle, with the post stack on.
   - The spec's earlier phrase was "the full toon recipe." It is replaced by the look pillars of §2, each checked in the scene.
3. **Golden images (from the spec):** they pass on three backends: Vulkan/lavapipe, DX12/WARP and Metal (§3, Q6).

## 2. The look, defined

These pillars are ranked by how much of the look each carries, merged from both rendering notes. Each one names the work packages that deliver it.

| # | Pillar | What it means on screen | Delivered by |
|---|---|---|---|
| 1 | **One time-of-day key drives everything** | Sky gradient, fog color, ambient tint, shadow tint, the character light rig and the grading LUT all come from the same art-directed curves. WuWa built its own stylized day cycle rather than physical atmosphere, and curve-fits character brightness across the day. | W1, C2 |
| 2 | **Anime characters with authored light response** | Thresholded light shifted by painted bias, with authored shadow colors (ramp rows or per-ID bands) and never a darkened albedo. SDF face shadows, a separate character light rig and stylized GI, depth-offset rim, inverted-hull outlines, hair highlights with eyes over the bangs, layered eyes, matcap metals. | C1–C7 |
| 3 | **Gradients and layered atmosphere** | Fog colored by the sky; fog cards; screen-space gradient ("para") layers, as anime compositing does it. | W2, W3, P2 |
| 4 | **Soft ambient light and tinted shadows** | Low-frequency, art-directed ambient; colored rather than black shadows; layered AO. | W4, W5, R4 |
| 5 | **Glow and a non-filmic grade** | Bloom plus a wide "diffusion" glow, a character bloom mask, and a custom tone curve baked into LUTs. No ACES look. | R3, P2 |
| 6 | **Clean, stable edges** | Anime frames, with their hard light splits and outlines, have two to three times the edge pixels of realistic ones, so AA matters more. Outlines write velocity under TAA; mobile uses on-tile MSAA. | R7, C4 |
| 7 | **NPR foliage** | Sphere or hull normals, camera-facing leaf clusters, then billboards and impostors. | W6 |
| 8 | **Combat screen grammar** | Hit-stop, impact frames, radial blur and cut-ins. M1 builds the `PostParams` effect-stack hooks; M2 authors the effects. | P3 |
| — | Out of scope | Ray tracing and Lumen-style GI. WuWa and ZZZ treat them as PC extras, and Genshin declined them to protect its art. | — |

## 3. Decisions this plan takes

Each decision updates the spec in the same PR as this plan.

| Topic | Decision | Why |
|---|---|---|
| **Q6: golden images** | One golden set per backend, compared with `nv-flip` (mean plus percentile thresholds, as wgpu does).<br>- **Linux:** lavapipe from a pinned Mesa build. **Required.**<br>- **Windows:** WARP 1.0.21 from NuGet plus `static-dxc`. **Required.** The DLL is downloaded in CI and never committed.<br>- **macOS:** Metal on `macos-26` ("Apple Paravirtual device"). **Informational** for two flake-free weeks, then required.<br>Images render at 640×360 with fixed time, no jitter and no dithering. Hard two-tone edges flip whole pixels, so gate on p95/p99 and an error-pixel budget, never on max, and add intermediate-buffer goldens (band index, SDF result, outline mask). `cargo xtask golden --update` regenerates. | Hosted macOS runners expose Metal, and wgpu runs its own image tests there on every PR. `macos-14` retires on 2026-11-02, hence `macos-26`. |
| **Time of day** | Add a `TodTimeline` / `TodKey` asset contract as the single source for sky, fog, ambient, shadow tint, post and the character rig, evaluated in OkLab. | Pillar 1. |
| **Ambient** | Replace "L2 SH probes baked per zone" with a per-zone probe grid: sky visibility (SH) plus bounce baked per TOD key. Add relightable reflection probes on desktop and Mobile-High, static on Mobile-Low. | Static probes go wrong as soon as the sun moves. Genshin relights its probes at runtime. |
| **Mobile AA** | 4× MSAA kept in tile memory through wgpu 30's `TRANSIENT_ATTACHMENT`, with alpha-to-coverage for foliage. Measure on Mali and Adreno. One-pass deferred stays impossible until wgpu gets input attachments (#10148). | Closes the spec's "Verify" item, pending device measurements. Confirms forward+. |
| **Character source format** | VRM 1.0 import through pixiv's `vrm-spec` crate on top of `gltf`. MToon maps onto Sapphire materials as a two-band ramp, an ILM threshold channel, a rim mask, a matcap slot and an outline width. VRM 0.x goes through a migration step. | It's the largest pool of legally usable anime characters, and the format carries the shading intent. |
| **Test content** | **Characters:** `VRM1_Constraint_Twist_Sample` (pixiv, VPL 1.0, credit not required) as the primary, and `Seed-san` (VirtualCast, VPL 1.0, credit required) as the second.<br>**Environment:** Kenney, Poly Haven, ambientCG and KayKit (CC0).<br>**Not used:** Unity-chan (its license can be revoked), Query-chan, Alicia Solid, and any ripped model. No new Quaternius downloads: since 2026-08-28 its license forbids redistribution.<br>Third-party content lives in `content/third_party/<name>/` with the license, a `SOURCE.md` (URL, date, SHA-256, credit) and a `MODIFIED.md` listing derived files. | Every file in a public repo needs a license that allows redistribution and modification. |
| **Authored anime maps** | Neither sample ships ramps, ILM or SDF face maps. The pipeline gets tools to make them: an SDF face-map generator (exact distance transform plus pairwise threshold blend, R16 output, a monotonicity check, horizontal and vertical channels), ramps authored as RON curves and baked to rows, and a control-map packer. Face masks are baked in Blender with an MIT script. | The look needs them, and VPL allows derived maps. |
| **Shader bindings** | Don't use `wgsl_bindgen`: it is built on the archived `naga_oil` and can't read WESL imports. Keep hand-written structs, verified against naga reflection by the layout test M0 already has. | Avoids an archived dependency. |
| **GPU struct layout** | `encase` through glam's `encase-013` feature. **F1 decides the glam version:** Bevy 0.20 is on glam 0.33 and Sapphire on 0.34, which matters for reflecting math types. | `encase` 0.12+ dropped its own glam integration. |
| **Texture compression** | `ctt` for BC and ASTC, generating our own mip chain and compressing level by level, because `ctt` has an open BC7 mip-chain bug. HDR on mobile uses RGBA16F / RGB9E5, since `ctt` has no HDR ASTC. | Avoids its known bugs. |
| **Pins** | wgpu stays on 30 through M1 even though 31 is imminent. `bevy_ecs`/`bevy_reflect` 0.20 and `egui` 0.37. | Upgrade between milestones, never during one (spec §5.6). |

**Flagged for M2:** `ozz-animation-rs` needs nightly Rust (`portable_simd`) and has no offline tools. Q7's answer is therefore "use the C++ ozz tools in the cooker," and the runtime choice has to account for our stable toolchain.

## 4. Work packages

Each package is one pull request, or a few, into `main`. Size: **S** is a day or two, **M** under a week, **L** one to two weeks. "Tiers" lists where the feature runs (D = desktop, MH = Mobile-High, ML = Mobile-Low).

### Track F: foundations

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| F1 | **ECS:** `sapphire_ecs` re-exports `bevy_ecs` 0.20 with the spec's schedules (fixed steps, Presentation, Extract, Cleanup) on the `sapphire_jobs` pool. The sandbox scene moves into ECS. Settle the glam version. | M | — | R6 is asserted inside the fixed schedules. |
| F2 | **Reflection and serialization:** `sapphire_reflect` (type registry, schema versions, migrations) and `sapphire_serialize` (RON for authored data, rkyv 0.8 for cooked data, postcard for saves). | M | F1 | bytecheck validation in dev builds. |
| F3 | **Extract boundary:** a ring of `FramePacket`s built by extract systems with change detection. The renderer keeps a persistent GPU scene updated from deltas. | M | F1 | Replaces M0's `FrameView`. |
| F4 | **Input:** `sapphire_input` with contexts, actions, modifiers, triggers and touch gestures. The sandbox camera and its debug keys move onto it. | M | F1 | |
| F5 | **Debug overlay:** `sapphire_debug` with egui (`egui-wgpu` 0.37) behind an SDL input adapter. Stats and budgets, frame graph, cvars, a time-of-day scrubber, and the renderer's debug views (§5.12). | M | F1, R1 | Dev builds only (R8). Look-dev tuning needs it. |
| F6 | **Golden-image harness:** `sapphire_testkit` with `compare` (`nv-flip`), golden scenes driven by `--capture`, `cargo xtask golden [--update]`, and the CI matrix in §3. | M | — | Can start immediately; it grows with every visual package. |
| F7 | **iOS frame pacing (Q13):** engine-owned `CADisplayLink` versus present-interval pacing, to hold 60 or 30 fps. | S | — | Needs a device for the final check. |

### Track A: asset data path

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| A1 | **Asset identity and cache:** `.meta` sidecars with GUIDs and import settings. A blake3 content cache keyed by source, importer and cooker versions, settings, cook and dependencies, replacing xtask's stamp. | M | F2 | |
| A2 | **Textures:** import PNG, TGA and EXR. Cook BC (desktop) or ASTC (mobile) through `ctt` with our own mip chains, into KTX2. Texture classes: color, normal, mask, ramp (uncompressed), ILM, SDF (R8/R16) and HDR. | L | A1 | `ctt` workarounds (§3). |
| A3 | **Materials as assets:** RON documents cooked to rkyv. Families: anime character, environment (stylized GGX), foliage, sky, fog card. Parameters reach the GPU through the material buffer. | M | F2, A1 | Feeds hot reload and the exit test. |
| A4 | **Packs and runtime loading:** the spec's pack format (TOC, zstd/lz4 64 KB blocks, blake3), the IO thread, decode jobs, a per-frame GPU upload budget, handles and basic residency. | L | A1, F2 | The corruption and leak tests from the spec. |
| A5 | **Skinned meshes:** glTF skins and skeletons, morph targets, `meshopt` optimization, quantized streams, LODs, and authored `_OUTLINE_NORMAL` attributes. | M | A1 | |
| A6 | **VRM 1.0 import:** `vrm-spec` types, MToon → Sapphire material mapping, VRM 0.x migration, and spring-bone and constraint data kept for M2. | M | A3, A5 | |
| A7 | **Anime map tools:** the SDF face-map generator, RON ramp curves baked to rows, the ILM channel packer, and a Blender mask-bake recipe. | M | A2 | Produces the look-dev character's maps. |
| A8 | **Desktop hot reload:** watch the cooked output with `notify`, swap assets in place behind handles, and hot-reload shaders. | M | A4 | As built: watches the packs, not the loose files; `AssetServer::remount`, `Renderer::set_shader_sources` and `update_mesh`, the dev-only `sapphire_hot_reload` (spec §5.10, §5.12). |
| A9 | **Live link, minimal:** `sapphire_livelink`. The dev build listens on TCP (postcard frames); the desktop pushes cooked assets, material changes, TOD keys and cvars over Wi-Fi, `adb reverse` or `iproxy`. | M | A8 | Required by exit test 1 on a phone.<br>**As built:** a `dev_only` crate whose device half every dev build runs (listening on loopback by default, or dialing for `adb reverse`) and whose binary is the desktop tool, `cargo xtask link status\|cvar\|push\|watch`. A versioned preamble, then postcard frames. A pushed pack sends only the assets the device lacks and is remounted through A8's path, the reply naming each new generation; documents and WGSL sources go through A8's parse-and-swap and rebuild, cvars through the registry. Run end to end on the iOS Simulator and, through `adb reverse`, the Android emulator. Spec §4.1, §5.3, §5.10, §5.20, §5.29. |

### Track R: renderer v1 core

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| R1 | **Frame graph:** passes declare reads and writes; compile culls, orders, sets lifetimes and pools transients; record in parallel, one submit. | L | — | M0's single pass becomes the first graph. |
| R2 | **Compute skinning, morphs and prepass:** one skinning dispatch per visible character into per-instance buffers. A prepass writes depth, normals and velocity. | M | R1, A5 | Velocity is mandatory for outlines under TAA. |
| R3 | **HDR and post core:** an RGBA16F scene target; a custom tone curve and grading baked to a 32³ LUT with blending between two LUTs; bloom (dual filter on mobile); a user filter-strength slider. Mobile fuses it all into one pass. | M | R1 | Early, so every later look package is judged in HDR. |
| R4 | **Shadows:** CSM (4 cascades on D, 2 on mobile) with sun quantization and caching, plus a per-character shadow frustum on D. Billboards and impostors face the light in the shadow pass. **Characters** (moved from C2): they receive only environment casters and never self-shadow; a per-character ground shadow map on D, a capsule on mobile. | L | R1, R2, C2 | Shadow *tint* comes from W1. Characters' cast shadow enters through C1's `cast_shadow`, under the contract C2 wrote in spec §5.12 ("Anime character shading", C2's note): the world key light's visibility past environment casters only, whatever direction a character's rig lights it from.<br>**As built:** `sapphire_render::shadows`: the frame's character list classifies (environment instances cast into the cascades and receive everything; characters cast only their own shadow and receive the cascades, so they never self-shadow); stable cascades (sphere fits snapped to texels, the sun's slope quantized), the far ones cached in turns and empty ones left undrawn; a tent PCF (5 × 5 on D, 3 × 3 on mobile) under normal-offset bias; per-character maps in a desktop atlas and capsules on mobile; a face pass bringing the environment's shadow to faces whole through the character's entry; W4's and C1's `cast_shadow` hooks filled. Tiers, `--shadows on\|off`, stats and the overlay's budget row; the sandbox's `--stage floor\|pillar`; six goldens (`shadows-contact`, `-pillar`, `-faces`, `-courtyard`, `-sunset`, `-capsule`), the twenty-six earlier ones pinned off (`--shadows off`), the new ones on plain post; C4's lit lines go into cast shadows with their surfaces. On an M5 at 1080p the courtyard's shadows cost about 0.26 ms of passes and 0.65 ms of receiving in the main pass, against 2.0 ms. Spec §4.6, §5.12. |
| R5 | **Lights:** clustered forward+ on D (a 16×9×24 grid), per-object lists with up to 4 lights on mobile. | M | R1 | |
| R6 | **Quality tiers:** `tiers.ron` (render scale, cascades, AA, bloom levels, AO, fog, particles, lights) and the dynamic-resolution hook. | S | R1 | |
| R7 | **AA:** TAA with velocity and a responsive mask on D; transient 4× MSAA on mobile, measured on devices. Upscaler mask plumbing for DLSS, MetalFX and FSR comes later. TAA upsampling restores the desktop tiers' internal-height caps (R6 renders desktop at native height until then). | M | R2 | |

- **As built (M1 R7):** the desktop tiers resolve with TAA and upsampling (MSAA off), phones keep 4× MSAA on tile. Desktop caps are back: 1440 rows on Desktop-High, 1080 on Desktop-Low. The resolve runs on HDR before bloom, tonemap-weighted, with Halton jitter and unjittered velocity, YCoCg clipping, velocity dilation, sky reprojection, disocclusion rejection and a responsive mask from the character prepass, as one compute pass (0.6 ms at 1080p on an M5, against the 0.8 ms budget; 2.3 ms at 4K from 1440 rows). Its inputs are the ones an external upscaler takes. Two deviations: a render-scale change keeps the history; and since outlines write no velocity yet, the resolve finds hull pixels from the main pass's depth and treats them as responsive, which C4 replaces with real hull velocity. Sharpening was measured and left out. Device measurements of the mobile MSAA move to M2. Spec §5.12 has the details.

### Track C: anime characters

The reference games agree on the core: **light response is authored, not physical**.
- N·L is thresholded and shifted by painted and per-vertex bias.
- The shadow side takes an authored color, never a darkened albedo.
- Cast shadows push pixels into the shadow band instead of multiplying them.
- Characters get their own light rig, decoupled from the world's.

The spec's §5.12 recipe holds. These packages add what the research found on top of it.

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| C1 | **Character shader core and asset contract.** One uber-shader with two diffuse modes:<br>- **ramp mode** (Genshin, Star Rail, WuWa): ramp-array row per material ID, day/night pair, WuWa-style subsurface-tint lerp;<br>- **band mode** (ZZZ): 2–3 analytic bands with per-ID colors, no ramp texture, cheapest on mobile.<br>Per-ID parameter arrays. Cast shadows feed the threshold.<br>**Asset contract:** base; a control map (shadow bias, specular mask, matcap/metal mask, material ID); normal; face map; ramp array; vertex color (AO, outline width, outline Z-offset); octahedral smoothed outline normal; a `u8` flags attribute. | L | R2, R3, A3 | Static permutation bits stay ≤ 8 (skinned, alpha-test, outline, morph, face, hair, eye, transparent). Ramp vs band, matcap channels and stockings are uniform branches. Data maps are linear. |
| C2 | **Character light rig and stylized GI.** Per-character key light with presets and per-shot overrides; brightness and color follow TOD curves (from W1). A GI stylization function: spherized, camera-biased normals on face and hair, HSV clamp, AO, skin tint. | M | C1, W1 | Pillar 1 on characters. WuWa runs its character lighting as a separate pipeline. Characters' shadows moved to R4 (batch 5): R4 lands after C2, which keeps C1's `cast_shadow` hook and writes down the contract R4 honors.<br>**As built:** presets in `sapphire_world::light_rig` resolved per character into `FramePacket::characters`; a per-character GPU buffer that C3's face data joins; stylized GI over W1's ambient. Spec §4.6, §5.12, §5.21. |
| C3 | **Faces.** SDF face shadow from the light angle in head space, with a horizontal and a **vertical** channel (WuWa's top light), mirrored for the other side. Always-lit mask (eyes, brows); nose line and face highlight masks; expression and blush channels animated per instance. Faces never take shadow-map self-shadowing. | M | C1, A7 (C2 as built) | A7's generator writes both SDF channels. |
| C4 | **Outlines v2.** Cooked tangent-space octahedral smoothed normals; width = vertex color × a two-segment depth curve × FOV normalized to 45° × model scale; view-space Z flatten and push-back; color = per-ID outline color blended with albedo × (light + flattened GI); writes velocity. | M | C1, R2 | GPU budget 0.6 ms (D) / 1.5 ms (ML), with an LOD cutoff.<br>**As built:** outline normals in a tangent frame derived from the normal (no cooked tangents); the width curve, model scale and LOD cutoff per material; hulls drawn into the scene prepass for depth, velocity and an outline mask. Spec §5.10, §5.12, §5.27. |
| C5 | **Character prepass, rim, hair and eyes over hair.**<br>- A character-only depth and ID prepass on **all tiers**.<br>- Depth-offset rim × Fresnel × N·L gate × region mask, with ZZZ's back-light weighting as an option.<br>- Hair specular: painted band, angel ring, optional Kajiya-Kay.<br>- Hair shadow on the face: hair-depth lookup on D, offset bangs plus stencil on mobile.<br>- Eyes and brows through bangs: 2 stencil bits and a view-angle alpha. | L | C1, R2 | The prepass costs about one extra draw per character and serves rim, hair shadow and eyes. |
| C6 | **Eyes.** Layered meshes (eye, highlight, eye shadow), a parallax/refraction fake, animated highlights and an eye depth mask. | S | C1, C5 | |
| C7 | **Materials on top of the anime character base.**<br>- Matcap (1–4 channels) and stylized specular (Blinn or minimal GGX), gated by masks.<br>- A "reflection add" for metallic > 0.9: probe on all tiers, plus SSR on D later.<br>- Stockings: N·V denier, anisotropic sheen, weave.<br>- Breathing emission and the character bloom mask (feeds P2).<br>- Dithered proximity fade that also fades the outline and kills rim. | M | C1, W5 | Screen-pattern and SDF-reveal effects are M2. |

- **As built (M1 C3):** faces are the anime character shader's face permutation, lit by the character's key light (C2's rig) in head space, from a head basis and expression weights each character carries beside its lighting (`CharacterInstance::face`, packed next to the rig in the per-character entry), with the SDF map's horizontal and vertical channels intersected and the result fed through the body's split, diffuse mode and stylized GI. Faces take no cast shadow; an environment shadow is to reach a face whole, per character (R4). Masks: always lit, face highlight and nose line (the face mask), blush and gloom weighted per character (an expression map, the family's eighth texture slot). The face test head, its maps and material are generated content; the golden scene `faces` shows a look-dev rig of ten heads, each a character lit from its own angle. Spec §5.12 has the details.
- **As built (M1 C4):** outlines v2 are the inverted hull with the research's controls, per material: a width in pixels at 1080 rows through a two-segment curve over the effective distance (the depth normalized to a 45° lens) that keeps the line's width in meters outside it, a model scale, view-space flattening and a push-back along the view ray, and an LOD cutoff that fades the line and then drops the hull (`r.outline_distance` scales it). Lines are lit as their surface: the per-ID outline color, blended toward the albedo, through the character's key light, split and shadow tone plus flattened GI, so they darken at night, and a character's lines are in post's character mask (P2). Outline normals stay the cooker's welded bake, stored in a tangent frame derived from each vertex's normal, which skinning carries across, so hard edges stay closed, bent or not; meshes still carry no tangents. Hulls draw into the scene prepass with their own depth and velocity and an outline mask the responsive mask takes in; R7's search for hull pixels is now redundant and left for P1 to remove. The four new golden scenes show the width over distance and lens, a hard-edged sample and a turning close-up under TAA. Cost at 1080p on an M5: under 15 µs for six characters on Desktop-High and under 35 µs with Mobile-Low's settings, against 0.6 and 1.5 ms. Spec §5.10, §5.12 and §5.27 have the details.

Community reconstructions of these shaders (HoyoToon) are **GPL-3.0**. They are read to understand the techniques, and no code is taken from them.

### Track W: the world

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| W1 | **Time-of-day core:** `TodTimeline` / `TodKey` assets, curve evaluation in OkLab, zone override volumes, a cutscene track, and the overlay scrubber with a character preview. Feeds sky, fog, ambient, shadow tint, post and the character rig. | M | F3, A3 | Pillar 1; costs under 0.05 ms of CPU. Starts `sapphire_world` (L4) with just this module; the renderer only sees the resolved values in the `FramePacket` (rule R4). |
| W2 | **Sky:** gradient sky, sun and moon, stars, RGBA card clouds with SDF dissolve and scrolling cirrus, and a sky-only cubemap updated one face per frame. | M | W1 | 0.1–0.3 ms; all tiers.<br>**As built:** `sapphire_render::sky`, colored by time of day's new optional `discs`, `stars` and `clouds` groups: procedural stars on a celestial sphere that turns with the hour, sun and moon discs with glows, Genshin-scheme card clouds and cirrus dissolving with coverage, and a sky cube for W4, W5 and C7 (256² a face a frame on D, 128² on MH, 64² every other frame on ML, which also drops the cirrus). The maps are generated by the pipeline's samples. On an M5: 0.12–0.16 ms with MH's settings, 0.06–0.1 ms with ML's, 0.23–0.36 ms at 1080p on the D tiers. Spec §4.6, §5.12, §5.20, §5.21, §5.27. |
| W3 | **Fog:** two-layer distance fog plus height fog, colored by the sky horizon, with sun inscatter; fog cards with depth and camera fade. | S | W1, W2 | All tiers; nearly free.<br>**As built:** `sapphire_render::fog`: one shared function, `fogged` in `frame.wgsl`, at the final color of the environment, anime characters and their outlines, fog cards and the sky (as at infinite distance, so distant geometry meets it without a seam): near and far distance layers up to a 30 m ceiling, analytic exponential height fog, the far color a tint on the sky's horizon, colors mixed over the stretch of the ray the camera sees, a cos⁸ inscatter lobe. Characters take time of day's share of it (decision; 40–50% in the default day, retuned for drawn fog). Fog cards draw on meshes (`CardFacing` world, camera or upright; `Material::VERSION` 5, `Mesh::VERSION` 8), lit by the fog, back to front, camera-faded on every tier and depth-faded against the scene prepass on the desktop tiers. Older goldens pin `--fog off`; seven `fog-*` goldens. On an M5 at 1080p the fog costs about 90 µs of main pass on Desktop-High and 20 µs on Mobile-Low, nothing with it off; the courtyard's seven large cards about 100 and 40 µs more. Spec §4.6, §5.10, §5.12, §5.21. |
| W4 | **Environment uber-shader:** stylized GGX with wrap lighting, a terminator remap whose width grows with distance, shadow tint and band, distance desaturation, and wetness inputs for later. | M | W1, R5 | Keeps the world on the characters' side of the picture.<br>**As built:** `shaders/environment.wgsl` under the world's key light: a terminator remap whose half-width grows with view distance, wrapped falloff, time of day's shadow tint × the material's, a terminator band ramp, C2's ambient hemisphere, GGX under a clamp per tier (full on D, approximate on MH, off on ML), distance desaturation, emission, R5's light lists; a cast-shadow hook for R4, lit until then; the frame's wetness plumbed, unread. Meshes mix both families and draws group by pipeline; the world goes through the scene prepass under TAA; no environment hulls in M1 (decision). Test content generated by the pipeline (`samples::environment`, `--environment`) and the goldens `environment-day`, `-sunset`, `-night`. On an M5 at 1080p the courtyard costs about 40 µs of main pass and 30 µs of prepass. Spec §4.6, §5.12. |
| W5 | **Ambient probes and AO:** a cooker bake of sky visibility (L1/L2 SH) plus bounce per TOD key into a sparse brick grid, sampled in forward shaders. Capsule AO on all tiers; GTAO at half resolution on D and MH. | L | W1, A1, R2 | Relightable reflection probes follow if time allows, otherwise early M2. |
| W6 | **NPR foliage:** sphere and hull normals transferred at cook time, NPR foliage shading, three-level wind, and alpha-to-coverage. Impostors in M2. | M | W4, A5 | |

### Track P: post and screen effects

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| P1 | **Outline-aware TAA integration:** composite or mask character outlines so TAA doesn't smear them. | S | R7, C4 | |
| P2 | **Post v2:** the diffusion glow layer, the character bloom mask, 2–4 gradient/para layers, sprite lens flares, vignette and RCAS sharpening, all in the fused mobile pass. | M | R3, W1 | Pillars 3 and 5. |
| P3 | **Effect-stack hooks:** a timeline-driven `PostParams` effect stack (radial blur, color inversion and duotone, chromatic aberration, speed lines), with character-mask plumbing. M2 authors the effects. | S | P2 | Pillar 8, groundwork only. |

- **As built (M1 P1):** outlines stay in the resolve, a hybrid chosen by measurement against 4× MSAA. A hull pixel's velocity is now its line's: the silhouette's motion, extrapolated in the outline prepass along the facing's screen gradient, rather than that of the hull points it shows, which slide through the silhouette whenever a body turns against the view. Lines are fully reactive in the responsive mask (now fractional: characters 0.5, lines 1), so a moving line is drawn from the current frame and a still one keeps the full history; pixels with a line in their 3 × 3 also try the camera's motion alone and no motion, and keep the history that matches best. R7's hull search, `OUTLINE_REACH` and the kept main-pass depth are gone; the resolve binds one texture fewer. On the sandbox character turning at 3 rad/s its lines keep 96% / 93% (head / figure) of MSAA's strength, from 81% / 83%, and at 4K upsampled from 1440 rows 101% / 100% at 104% / 101% of its width, from 86% / 90% at 111% / 109%; still frames are unchanged, moving characters leave no trail and a render-scale step is a non-event. Compositing the lines after the resolve was prototyped and rejected: aliased one-sample lines, worse than the resolve even when still, and 190–215 µs at 1080p for one character. An external upscaler gets the same inputs, the line's velocity and the lines' reactivity included. TAA's cost is unchanged within a shared machine's noise (589 µs at 1080p at its least disturbed). Golden scene `outlines-turning-taa-upsampled`; spec §5.12 ("Outlines" and "Anti-aliasing and upscaling") has the measurements.
- **As built (M1 P2):** every layer is in R3's fused pass on every tier, picked per frame by pipeline variants (the plain ones are R3's code, which the 19 earlier goldens pin with `--plain-post`): the diffusion glow (the bloom chain's widest smooth level through a soft threshold, screened over the graded frame), characters' own bloom threshold and strength (the character mask is scene color's alpha on every tier, written by the anime character shader; under TAA read before the resolve, so R7's responsive mask isn't needed), up to four gradient ("para") layers in six blend modes, a lens flare of eight sprites whose occlusion is measured on the frame itself (so clouds put it out too, and phones need no depth), a tinted vignette, and RCAS as a player option, off by default since it measurably helps nowhere. Time of day's new optional groups (`diffusion`, `character_bloom`, `gradients`, `lens_flare`, the vignette's `color`) blend in OkLab, gradient layers slot by slot, and cutscenes override them per shot; the default day sets them all. Golden scenes `post-day` (TAA), `post-sunset` (Mobile-Low) and `post-flare`. Costs and the sharpening measurements are in spec §5.12 ("Post-processing"), the keys in §5.21.
- **As built (M1 P3):** `sapphire_world::screen_effects` holds a fixed stack of 16 effect layers that gameplay pushes as presets (an `EffectLibrary` document, hot reloaded like the default day, with a sample per effect and `impact_frame`): each layer one effect with a pulse, sustain or keyed envelope on the world's clock or on real fixed steps (so impact frames authored in frames last those frames through slow motion and play on through hit-stop, which freezes only the combatants' clocks). Each frame the stack blends its layers bottom to top into `PostParams::effects` (rule R4), allocating nothing. Post draws them in the fused pass on every tier: a radial blur with the tier's taps (16, 12, 8, 8) and chromatic aberration in place of the scene's texel, then inversion, duotone and speed lines over the display values, each color effect weighing characters and the world apart through P2's character mask (read before TAA's resolve, within a pixel of the silhouette, so the resolve carries nothing new). Two variants, lens and color effects, chosen by their cost on Mobile-Low's settings; an empty stack draws exactly P2's frame, so no earlier golden needed pinning. Sandbox `--post-effect <preset>[@<seconds>]`, the overlay's effects panel, and golden scenes `effects-blur-lines`, `effects-impact-frame` and `effects-chromatic-inversion`. While they play the blur and the impact frame take post past §3.3's 0.7 ms on the desktop tiers on an M5; costs are in spec §5.12 ("Post-processing", P3's note), the stack in §5.21.

### Track X: exit

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| X1 | **Look-dev scene:** the VRM character with authored maps in a CC0 environment slice, a TOD cycle and full post. Goldens per backend; budgets measured on desktop and, with a phone, on Mobile-Low. | M | most of C, W, P | The visual exit test.<br>**Content (2026-10-26):** `content/third_party/`: pixiv's `VRM1_Constraint_Twist_Sample` (VPL 1.0) and Kenney's Fantasy Town Kit 2.0 (CC0), with their provenance. The character is anime as shipped; the town kit is flat-palette, low-poly base geometry, which X1 re-materials, scales and composes into an anime game's town before its frames count as the look. |
| X2 | **Exit run:** desktop and phone hot reload of ramps, materials and TOD keys; goldens on three backends; spec and README updated; M2 handoff notes. | S | X1, A9 | |

## 5. Order

The packages fall into five phases. Within a phase, packages that don't depend on each other can run as parallel pull requests.

1. **Foundations and harness:** F1, F6, R1, then F2, F3, F5.
   - F6 comes first so every visual change after it is guarded by goldens.
   - R1 next, because every rendering package builds on the frame graph.
2. **Data path:** A1, A2, A3, A5, then A4.
   - At the end of this phase, the mannequin's material is a real asset.
   - A8 lands as soon as A4 does, so hot reload is available to all look work.
3. **Renderer core:** R2, R3, R5, then R4, R6, R7.
   - R3 (HDR and LUT) is early on purpose: judging the look in LDR would mislead every later decision.
4. **The look, in pillar order:**
   - W1 (TOD) first.
   - C1, C2 and C3 next, plus A6 and A7, so a real VRM character with authored maps is on screen and lit by the TOD rig.
   - Then C4, C5, W2, W3, W4, P2, C6, C7, W5, W6, P1 and P3.
5. **Exit:** A9 (live link), X1, X2, and F7 when a device is available.

The **critical path** is F1 → F2 → A1 → A3/A4 → A8 → A9 → X2 for the data path, and R1 → R2 → R3 → W1 → C1 → X1 for the look.

## 6. Risks

| Risk | Mitigation |
|---|---|
| **No physical phone yet.** Exit test 1 and the Mobile-Low budgets need one. | Develop on the emulator and the Simulator. A phone is needed before X2 at the latest (§7). |
| **Authoring the anime maps takes art time**, and there is no artist on the project. | A7's generators do the heavy lifting. Masks for one face are a Blender bake, and ramps are data. The look-dev character can start from MToon-derived maps and improve. |
| **Lavapipe and WARP differ from real GPUs** on hard two-tone edges. | Per-backend goldens, percentile gates, intermediate-buffer goldens. Real-GPU checks happen on this Mac and on devices. |
| **wgpu 31 lands mid-milestone** with fixes we want. | Stay pinned to 30 (§3). Upgrade at the M1→M2 boundary. |
| **Scope:** M1 is the largest milestone. | Pillars are ranked. If time runs short, W5's reflection probes, W6, C6's extras and P3 slip to M2; W1, C1–C5, R3 and P2 don't. |
| **Permutation growth** in the character uber-shader. | Static bits are capped at 8, and everything else is a uniform branch (C1). The permutation compile test runs in CI. |

## 7. Needed from you

1. **A physical Android phone** (any arm64 device with Vulkan 1.1, ideally Snapdragon 865-class or newer) before X2. It's needed for the live-link exit test and Mobile-Low budgets.
2. **An Apple signing identity** to run on an iPhone: a free personal team works for development builds.
3. **Art-direction feedback** at the end of phase 4. A handful of reference screenshots of what you want, from WuWa, ZZZ or anything else, would help me tune the TOD curves, ramps and grade.
4. **Optional:** a Windows or Linux machine with a real GPU, for Q4 (the default Windows backend) and for hardware runs.
