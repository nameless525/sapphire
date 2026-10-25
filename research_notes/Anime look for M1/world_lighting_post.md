# World, lighting, atmosphere and post: how the anime action games build the look

- **Date:** 2026-10-24
- **Scope:** everything on screen except character shading, which a separate note covers. That means environments, GI and ambient, shadows, sky and time of day (TOD), fog, weather and water, foliage, post-processing, combat screen effects, AA and upscaling, and the mobile versions of all of these.
- **Targets:** Wuthering Waves (WuWa; Kuro Games, custom UE4.26) and Zenless Zone Zero (ZZZ; HoYoverse, customized Unity). Genshin Impact and Honkai: Star Rail (HSR) are supporting references. Blue Protocol, Hi-Fi Rush and Neverness to Everness (NTE) are comparators, because their teams published more about the parts WuWa and ZZZ don't discuss.
- **Consumer:** the Sapphire renderer (wgpu 30, frame graph, clustered forward+, Metal / Vulkan / DX12, iOS and Android required). §14 maps the findings to work packages and to changes in `docs/engine-spec.md`.

**Evidence labels** (used throughout; numbers in brackets point to §16)

- **[P] Primary:** the studio's own talk, slides, official interview, patch notes or platform-holder post.
- **[S] Secondary:** press or attendee reports of a primary source.
- **[C] Community:** reverse engineering, frame captures, fan recreations, player guides and benchmarks. Treat as unverified.
- **[O] Observation:** behavior visible in public gameplay, not documented anywhere. Verify with your own captures.
- **[I] Inference or recommendation:** mine, for Sapphire.
- **[est]:** my cost estimate, not a measured or published figure.

---

## 0. What carries the look, ranked

1. **One art-directed TOD "key" drives everything.** Sky gradient, fog color, ambient tint, shadow tint, the character light rig and the grading LUT all come from the same TOD curves.
   - WuWa built its own stylized TOD instead of UE's physical atmosphere. It curve-fits character brightness across the day, and gives every scene its own atmosphere and LUT [P 6].
   - Honkai Impact 3rd keyframes cloud and sky colors over 24 h [S 21]. Blue Protocol blends morning, noon, evening and night post looks in a master post material [S 35].
   - This is what makes anime characters and a 3D world read as one picture. Cost is about zero, and it works fully on mobile.
2. **Gradients and layered atmosphere.** WuWa says its main focus is the "gradient" feel of 2D animation, expressed through lights, fake volumetric fog cards and post materials [P 6].
   - Blue Protocol reproduces anime compositing: "para" gradient overlays, sun shafts, flares, and light washing in from the screen edges [S 35].
   - The tools are distance and height fog colored by the sky, fog cards, and screen-space gradient layers. All are cheap and mobile-safe.
3. **Soft, low-frequency ambient light and tinted shadows on the world.** Genshin relights its reflection probes at runtime and takes SH ambient from them, and it stacks three AO methods [P 18].
   - WuWa clamps GI in HSV space and smooths normals so indirect light doesn't break the cel-shaded read [P 1].
   - Shadows are colored, not black, and have art-directed edges [S 35]. On mobile this means SH probes, capsule AO and analytic tint; GTAO only on high tiers.
4. **Bloom, glow and a non-filmic tone curve with LUT grading.**
   - Genshin rejected ACES because it didn't suit the art [P 18]. Blue Protocol added a separate "diffusion" glow because engine bloom wasn't enough [S 35].
   - Both WuWa and ZZZ expose a user color-filter strength slider [C 15, 24, 25]. Fully feasible on mobile as one fused pass.
5. **NPR foliage.** In WuWa, characters and foliage use the NPR path and everything else is PBR [S 8]. Trees step from leaf-card meshes to camera-facing billboard trees to impostors. Mobile starts at the billboard tier [P 6].
6. **Image stability on edges.** NPR frames have about 4–5% edge pixels against 1–2% for PBR games, so AA matters more [S 8]. WuWa wrote its own mobile TAAU with a character velocity mask [S 8].
   - Mobile: MSAA on tile memory, or a custom TAAU.
7. **Combat screen grammar.** This covers hit-stop, impact frames (inverted or monochrome), radial blur, speed lines, CRT and glitch overlays, and cut-ins.
   - ZZZ uses post blur and "old TV stripe" overlays on hits [C 26]. The cost is tiny; the work is authoring and timeline plumbing.
8. **Reflections, water and wet streets.** ZZZ added RT reflections on PS5 Pro in 1.4 and on PC in 3.0, aimed at wet streets, mirrors and puddles [P 22, 23]. Its "Mirror Reflections" setting is the most expensive one on PC [C 24].
   - Mobile: probes plus SSR on high tiers. WuWa runs SSR from the previous frame on mobile [P 6].
9. **Ray-traced or Lumen-style GI.** On WuWa this is a PC-only option added in 2.1 [P 1, 2]. Genshin declined ray tracing to preserve its art style [S 20]. It is not part of the base look. Skip it for v1.

**Mobile feasibility at a glance** [I, grounded in what WuWa, ZZZ and Genshin ship on phones]

| Feature | Mobile-High | Mobile-Low |
|---|---|---|
| TOD-keyed sky, fog, LUT, character rig | Yes | Yes |
| Gradient / para layers, fog cards | Yes (fused post) | Yes (fused post) |
| SH probe ambient, capsule AO | Yes | Yes (L1 SH) |
| GTAO | Half-res (WuWa: high preset) | No |
| SSR | Previous-frame, half-res (WuWa: medium) | No |
| CSM | 2 cascades plus cached far | 1–2 cascades, cached |
| Volumetric fog (froxel) | Optional, low-res | No (analytic fog plus cards) |
| Bloom + LUT + vignette + combat FX | One fused pass | One fused pass, fewer bloom mips |
| NPR foliage, billboard / impostor LOD | Yes | Yes, lower density |
| MSAA 4× on tile (transient attachments) | Yes | 2× or FXAA |
| Temporal upscaler | MetalFX / Arm ASR / custom TAAU | Spatial (EASU-style) |
| RT / Lumen GI | No | No |

---

## 1. Reference games: pipeline facts, version-tagged

### Wuthering Waves (Kuro Games)

**Engine**
- Customized UE4.26 with selected UE5 features re-implemented [P 1, 2, 6].
- The engine can't be upgraded because the game is live, stability comes first, and its assets were built for UE4 [P 2].
- No UE5 migration was confirmed as of October 2026. Talk of one comes from speculation by leakers [S, social].

**Pipeline**
- Deferred on every platform. Kuro gives three reasons: SSR and GTAO, fewer shader variants in an open world (weather, TOD and atmosphere changes), and one pipeline across platforms [P 6].
- **Mobile:** One Pass Deferred on PLS / framebuffer fetch. It saves about 1.5 GB/s of read/write bandwidth at 60 fps [P 6].
  - Mali PLS has limits: 128 bits of on-chip storage, no alpha blend, one resolvable RT, and depth fetch defeats Mali FPK early-Z [P 6; S 8].
  - The NPR path started as a separate forward pass. It was later folded into One Pass Deferred in a near-rewrite [S 8].
- Hybrid materials: characters and foliage are NPR, the rest is PBR [S 8].

**TOD, sky and grading**
- Self-developed stylized TOD, not UE physical atmosphere. Sky colors are tunable in real time without precomputation [P 6].
- No volumetric clouds. A 2D skybox pipeline, mostly TOD-tintable, with animated cloud drift [P 6].
- Per-scene atmosphere and LUT grading [P 6].
- A separate character lighting pipeline with presets. Character brightness is curve-fitted across TOD using UE curve tools [P 6].
- Some engine features are deliberately under-sampled, because low sample counts give a "hand-painted feel" [P 6].

**Ray tracing (PC only, since 2.1, 2025-02-13)** [P 1, 2, 3]
- Lumen-style hardware-RT GI: screen probes, radiance cache, and a custom clipmap irradiance cache instead of the surface cache.
- Hybrid reflections: screen trace first, RT for misses.
- RT shadows hybridized with CSM.
- About 2 ms for GI and 2 ms for reflections on an RTX 4060 at 2K. 60 fps target.
- Later versions:
  - 2.8 cut RT GPU load [C 13, citing patch notes].
  - 3.6 brought "Ray Tracing Megalights" to AMD GPUs [S 11]. It is probably the ReGIR many-light work from the talk [I].

**PC options** [C 12, 15]
- Anti-aliasing: Off / FXAA / SMAA / TAA.
- Upscalers: DLSS / DLAA (3.3), FSR, XeSS 3 (3.5) [S 9, 10].
- Rendering precision from 0.6 to 1.6.
- GI mode (Lumen vs off/SSR) and reflection mode (Lumen vs SSR), plus a ray-tracing toggle. How these depend on each other isn't documented.
- AO, volumetric fog, a separate "volumetric light" toggle, motion blur, and global filter intensity and sharpening.

**Mobile options**
- Shadows, volumetric fog and bloom drop out at Very Low [C 15 via Game8 summary].
- Global filter intensity and sharpening, and AA [C 16].

**Mobile timeline**
- 3.3 (2026-04-30): Kuro's own mobile frame interpolation, as a limited test on Snapdragon 8s Gen 4. It caps output at 60 fps and draws less power than plain 60 fps [S 9, 10].
- 3.5 (2026-07-10): interpolation extended to 8 Gen 3 and 8 Gen 2 [S 10].
- 3.6 (2026-08-20): Android Vulkan renderer, on by default on 8 Elite-class chips; frame generation on 8 Gen 5; DLSS 4.5 with 5× and 6× multi-frame generation on PC [S 11].

**Other platforms:** Mac since 2025-03-27 [S 14]. PS5, and Xbox Series since 3.5 [S 10].

### Zenless Zone Zero (HoYoverse)

**Engine**
- Customized Unity. One third-party article claims it is Unity China's Tuanjie fork; that is unverified.
- A community capture talks about its G-buffer, which suggests deferred, but that is also unverified [C].
- No rendering talk from HoYoverse about ZZZ has been published (GDC Vault, Unite, CEDEC all searched). **Most ZZZ specifics below are community or observation.**

**PC options** [C 24]
- AA: TAA / SMAA.
- Shadows, FX detail, "Shading Quality" (ambient shadowing), character quality, environment quality.
- Mirror Reflections: about 16% of frame rate. Volumetric fog ("light shafts and haze"): about 2.6%. Bloom: about 1.5%. Measured on an RTX 4080S, which was CPU-bound.
- Distortion, Color Filter Strength, and render scale.

**Mobile (v1.1)** [C 25]
- Render precision, shadow precision, character and scene quality, effects quality.
- TAA, volumetric fog and bloom toggles, Color Filter Strength from 0 to 10.
- 60 fps cap. Minimum SoC is a Snapdragon 855.

**Ray tracing**
- 1.4 (2024-12-18) added RT on PS5 Pro. It fixes off-screen reflection failures, which implies the base path is SSR, and adds "indirect highlights" for mirrors, water, puddles and glossy metal [P 22].
- 3.0 (2026-06-17, alongside the Steam launch) added RT reflections, AO and GI on PC, plus DLSS Super Resolution and frame generation [P 23].

**City time of day:** the city runs four discrete phases (morning, afternoon, evening, midnight). Activities or resting advance them [C 28].
- This is important for Sapphire: discrete phases make per-phase baking legitimate. See §4.

### Genshin Impact (HoYoverse) [P 18, 19]

**Pipelines**
- Two custom Unity pipelines: one for PC and console, one for mobile. Mobile is the primary development platform.
- Shading is "PBR-based stylized rendering", not energy-conserving, with lighting models altered for art reasons.
- All lighting is real-time, with TOD and weather. On console, over half of the features are compute.

**From the console talk** [P 18]
- 8 CSM cascades with caching.
- Screen-space shadow mask with penumbra classification.
- AO stack: HBAO, AO volumes and capsule AO.
- Clustered deferred lighting with up to 1024 lights.
- Froxel volumetric fog under 1 ms, and a separate god-ray pass.
- Runtime-relit reflection probes with SH ambient taken from them.
- SSR at 1.5 ms on PS4 Pro.
- HDR with a LUT. ACES was rejected.

**From GDC 2021** [S 19]
- No fixed lighting. IBL and ambient change over the day, and local lights turn on at night.
- Terrain blocks are capped at mixing 8 layers.

**Ray tracing:** declined, because adding it would take "huge efforts" to keep the art style [S 20].

### Honkai: Star Rail (HoYoverse)

- Unity. Community recreations say scenes are deferred and characters forward in two passes, with separate bloom control for characters. Post includes bloom, tonemapping, color adjustments and vignette [C 31].
- MetalFX on iOS since 2.0 (early 2024) [S 32].
- No rendering talk has been published.

### Comparators (primary or near-primary)

- **Blue Protocol** (Bandai Namco, UE4, CEDEC 2020/2021) [S 35]: minimal engine changes plus added passes. Covers environment texture simplification, a distance painterly filter, para and flare compositing, TOD post blending, shadow color by distance, separate "diffusion" glow, and Responsive AA with stencil-protected outlines.
- **Hi-Fi Rush** (Tango, UE4, GDC 2024) [S 36]: a fully cel-shaded deferred environment. Covers decal "toon lights", CSM plus baked static shadow textures, capsule shadows for NPCs, and non-directional baked GI. 60 fps at native resolution.
- **Neverness to Everness** (Hotta / Perfect World, UE5, Unreal Fest Seoul 2026) [S 37]: a mobile pipeline designed from scratch, not scaled down from PC.
  - The sun is frozen within each of 12 TOD segments so far shadows can be cached.
  - On mobile, near CSM is combined with camera-anchored, tiled, incrementally updated far shadow maps.
  - Lumen and hardware RT are gated by runtime device checks.

---

## 2. Environment materials, and keeping the world consistent with anime characters

### What the games do

- **Stylized PBR environments, not toon environments.**
  - Genshin: PBR-based, not energy-conserving, lighting models bent for art [P 18].
  - WuWa: PBR for everything except characters and foliage [S 8].
  - Blue Protocol: realistic modelling, but textures deliberately simplified. Less detail, emphasized edges, hand-painted color unevenness, and Substance Painter with shared smart materials so the stylization is consistent [S 35].
  - Fully toon environments do exist (Hi-Fi Rush: two-tone light/shadow everywhere, toon "decal lights" with cutout attenuation [S 36]). That is the "cartoon" look, not the WuWa/ZZZ look.
- **Detail control at distance.**
  - Blue Protocol runs an SNN (symmetric nearest neighbor) filter on distant rocks to flatten detail into a painted read, and its grass material reduces color information with distance [S 35].
  - WuWa reduced texel and mesh density for mobile, aiming for "detail on 4K, recognizability on mobile" [P 6].
- **Mechanisms that keep characters consistent with the world:**
  1. Characters get their own light rig, with presets per scene and a TOD-fitted brightness curve (WuWa) [P 6].
  2. Environment lighting reaches characters only weakly. In Blue Protocol, sky light and indirect light ignore normals and carry little color, so characters keep their own colors instead of turning sky-blue [S 35].
  3. When characters do take GI, it is clamped. WuWa clamps in HSV, masks skin, bends normals toward the camera, and uses spherical normals on hair and face. Its slides warn that "dense GI information can damage the toon texture" [P 1].
  4. Foliage joins the NPR family, so the most visible environment element shares the characters' shading language [S 8].
  5. Shared post (LUT, bloom, gradients, fog) is the final glue [P 6].
  6. Effects near characters are opaque and anime-styled, and close-to-camera effects are culled so characters stay readable [S 35].

### Implementation for Sapphire [I]

**Environment uber-shader: "stylized GGX"** (the spec already names it). Add these knobs, all as per-material or per-zone parameters, not permutations:
- Diffuse wrap / half-Lambert amount: 0 is physical, around 0.3–0.5 gives a soft painted look.
- **Shadow terminator remap:** `s = smoothstep(edge - w, edge + w, shadow * NdotL)`. Artists set the width `w`, and `w` grows with distance (Blue Protocol's distance-dependent shadow edges).
- **Shadow tint:** `lit = mix(albedo * shadowTint_TOD * ambient, albedo * sun, s)`. Shadowed areas take an artist color, usually a saturated sky-ward hue, never gray.
- **Terminator band:** an optional saturated, slightly warm band at the light/shadow boundary, made by sampling a small 1D ramp with `s`. This is cheap and very "anime" [I].
- **Specular and roughness:** clamp specular, and remap roughness into a narrow artist range. WuWa clamps roughness to 0–0.3999 for its RT reflection path [P 1].
- **Painted AO and cavity channel** in the mask texture, multiplied into ambient only.
- **Distance stylization:** desaturate and shift hue toward the fog/sky color as `f(viewDistance)`. Optionally drop to a lower mip, the equivalent of the painterly distance filter.
- **Optional two-tone step for props** that should read like characters (ZZZ-style hero props) [O]: the same remap with `w` close to 0. Lighting goes through the same light lists.
- **What artists author:** albedo with painted gradients and AO baked in; a mask texture (AO, cavity, spec mask, material ID); per-zone "look" parameters (wrap, shadow tint offset, distance desaturation) referenced from TOD keys.
- **Forward+ is the right fit [I].** Mixed NPR and PBR in one opaque pass needs no G-buffer bits, which was WuWa's main mobile pain with Mali PLS [S 8]. Weather and wetness become uniforms read by every uber-shader, which addresses WuWa's main reason for choosing deferred [P 6].
- **Cost:** the same as plain GGX forward. Mobile is fine.

---

## 3. Global illumination and ambient light

### What the games do

**Genshin (console)** [P 18]
- Baked lighting is impossible with TOD and weather, so reflection probes are baked offline as a **mini G-buffer** and **relit at runtime**:
  1. Relight with the current sun and sky.
  2. Convolve the mip chain.
  3. Compress to BC6H.
- One compute shader handles all six faces, and **one probe updates per frame**, round-robin.
- Ambient probes store 3-band SH taken from the relit reflection probes.
- Relighting ignores shadows, which made shadowed ground too bright. The fix adds baked shadow SH and local-light SH during relight.
- Indoor and outdoor probes are separate. An interior mesh marks indoor pixels.
- GDC 2021 adds that IBL and ambient change across day and night [S 19].

**Genshin AO** [P 18]
- **HBAO** for small-scale detail.
- **AO volumes** for static objects: precomputed per object in local space and evaluated at runtime, for example a tabletop darkening the floor.
- **Capsule AO** for skinned characters, with directional occlusion.
- AO runs at half resolution, with bilateral upsampling and blur.

**WuWa, raster path (all platforms)**
- GTAO on high presets, including most mobile devices at high, and SSR at medium [P 6].
- **How ambient and diffuse GI work without ray tracing (on mobile, and on PC with RT off) is not publicly documented.** The 2024 interview only says baking can't serve a dynamic open world and that Lumen was attractive [P 6].

**WuWa, PC RT path (since 2.1)** [P 1, 2, 3]
- **Screen probes:**
  - Octahedral radiance compressed to SH.
  - Adaptive placement: 16×16 probes at base, refined to 8×8 or 4×4 where interpolation error is high.
  - Structured importance sampling, then spatial and temporal reuse.
- **Radiance cache:** world-space probes stored in 32×32 textures, with parallax correction between screen probes and cache.
- **Custom clipmap irradiance cache** instead of Lumen's surface cache, which wouldn't port to UE4.26.
- **Lighting simplifications:** a simpler material branch for GI and reflection hits.
- **Stylization:**
  - Character normals are bent toward the camera, with spherical normals on hair and face.
  - GI color is converted to HSV and its saturation and value are clamped, then multiplied by AO.
  - A skin mask reduces GI on skin.
- **ReSTIR + SHaRC + NRD was rejected:** noise on edges, at distance and in motion, plus memory traffic. Cost was 5–10 ms plus 3–4 ms of denoising on an RTX 4060.
- **Hi-Fi Rush (comparison)** [S 36]: customized UE volumetric lightmaps, reduced from 8 SH textures to 1 because only non-directional GI was needed, and baked selectively around strong emissives.

### Implementation for Sapphire [I]

The spec currently says "L2 SH probes baked per zone". That is static, and it breaks as soon as the sun moves. Two compatible layers:

**Layer A: zone probe grid with TOD keys** (all tiers)
- Bake per zone, per **TOD key**. Use ZZZ-style discrete phases (4) or 6–8 keys for a continuous cycle.
- Each key stores:
  - **Sky visibility:** an L1 SH (4 scalars) per probe, plus optionally a bent normal. It is independent of TOD, so store it once.
  - **Sun bounce:** RGB L1 on mobile, RGB L2 on desktop.
- **Runtime:** `E(n) = skyIrradianceSH(TOD) ⊗ visibility(n) + lerp(bounce[k0], bounce[k1], t)`. The sky term follows the TOD key curves exactly, and the bounce cross-fades between keys.
- **Storage:** a sparse brick grid. Use around 2 m spacing near gameplay space and 8 m in the open [est].
- **Cost:** about 0.05 ms per frame for sampling [est]. The bake is an offline cooker job: CPU path tracer or wgpu compute.

**Layer B: relightable reflection probes, Genshin-style** (desktop; mobile-high at reduced rate)
- Offline: capture a cubemap mini G-buffer per probe (albedo, normal, depth or roughness).
- Runtime: a compute pass relights it with the current sun, sky SH and the zone's baked shadow term, then GGX-prefilters the mips and stores the result.
  - Desktop: BC6H, encoded in compute.
  - Mobile: RGB9E5 or RGBA16F at 64–128 px. The spec already says this.
- Round-robin one probe per frame. On mobile, relight only when the TOD key changes by more than ε.
- Use Genshin's leak fix: bake a per-probe shadow / sky-visibility SH so relight doesn't brighten shadowed ground.
- **Cost:** about 0.2–0.4 ms per frame on desktop for one 128² probe, including prefiltering [est]. Mobile: spread over 4–8 frames.

**AO**
- **Desktop:** port XeGTAO (MIT, compute HLSL to WGSL) at half resolution with a bilateral upsample [43]. The GTAO paper claims about 0.5 ms on console hardware of its day [42].
- **Mobile-High:** half-res GTAO with fewer slices, as WuWa enables on high [P 6]. About 1–1.5 ms [est].
- **All tiers: capsule AO for characters.** It is in the spec. Genshin uses directional capsule AO; UE documents the capsule approach [51].
- **Analytic AO volumes** (Genshin) for big static props, evaluated from the clustered light/decal lists as oriented boxes or ellipsoids. Cheap, and works on mobile.

**Character ambient:** whatever reaches characters from Layers A and B should pass through WuWa's guards: a camera-bent normal, a saturation/value clamp and a skin mask. The character note owns the shader; this note supplies the probe data.

**Skip for v1:** RT GI, DDGI, SDF GI. WuWa's RT GI is an optional PC extra that took a dedicated team; Genshin ships its look without it [S 20].

---

## 4. Shadows

### What the games do

**Genshin (console)** [P 18]
- **Up to 8 cascades.** The first 4 update every frame and the last 4 interleave, so 5 cascades render per frame and each refreshes at least every 8 frames.
- **Screen-space shadow resolve:** 11 Poisson taps per pixel with rotated patterns, 2–2.6 ms on PS4.
- **Penumbra classification:** a 1/4-resolution mask (one value per 4×4 tile) marks shadow, lit or penumbra, so soft filtering runs only on penumbra pixels. This brought it to 1.3–1.7 ms.
- **Local lights:** about 100 in view can cast shadows. Static local-light shadows are baked, then compressed in 2×2 blocks into 32 bits (20–30:1) and decompressed in compute at about 0.05 ms per 1k² on PS4.

**WuWa**
- **Billboard trees:** turned to face the light when rendering shadows, not the camera, so tree shadows don't swing as the camera turns [P 6; S 8].
- **PC RT:** RT shadows plus CSM. Tile classification skips fully shadowed tiles. Billboards, impostors and translucents stay in shadow maps.
- **Shadow-ray tweaks:** front-face culling for single-sided walls. Characters are excluded from RT shadows via the instance mask and use a dedicated "ToonShadowRay", so ray-traced softness doesn't blur the crisp two-tone self-shadow [P 1].
- **TOD shadows on all platforms:** "TOD shadows across platforms" was a UFSH2023 topic, but **the cascade and caching details didn't make it into any notes I could find** [P 7]. The video is the only source.

**NTE (2026)** [S 37]
- **TOD segments:** the 24 h cycle is split into 12 segments with the sun frozen inside each, so far shadow maps can be cached.
- **PC invalidation:** cache invalidation is limited to the camera's 2D footprint. Result: 90% fewer full updates and 20–30% less shadow GPU cost.
- **Mobile:** CSM near, plus camera-anchored layered shadow maps for distance. These are N×N tiles updated incrementally only when the player moves, with a mesh-ID list so only newly visible objects are drawn.

**Blue Protocol** [S 35]
- Distance thresholds control the main shadow and the shadow edge separately, and shadow color changes with distance.
- Shadows are deliberately exaggerated; tree leaf shadows are stretched on purpose.
- Characters don't receive CSM self-shadows. A pseudo self-shadow comes from an offset-depth compare.

**Hi-Fi Rush** [S 36]
- Toon lights don't cast shadows; CSM covers outdoors.
- Baked static shadow textures: a per-actor depth capture compared in screen space inside a decal volume, filtered with 4×4 PCF.
- NPCs get capsule shadows, whose blotchy softness suits the style. Characters at height get AO-volume decal shadows.

**ZZZ:** a shadows quality setting on PC and mobile [C 24, 25]. Nothing else is documented.

### Implementation for Sapphire [I]

**Desktop**
- 4 cascades: near two every frame, far two alternating.
- **Quantize the sun for the far cascades:** update only when the sun moves past a threshold or crosses a segment boundary (NTE). With ZZZ-style discrete phases the sun is constant within a phase, so far cascades become nearly static.
- **Shadow-mask pass** (R8 or R8G8, full res) after the depth prepass. It combines:
  - CSM PCF or Poisson taps, using Genshin's 1/4-res penumbra classification;
  - screen-space contact shadows (Bend Studio's open-source SSS, used in Days Gone and other Sony titles [44]);
  - cloud shadows from a scrolling texture.
- The forward shaders then read one value. Cost: about 0.6–1.0 ms at 1080p [est].
- **Characters:** keep the spec's per-character high-res frustum for the shadow they cast. The decision to block CSM self-shadow on characters (Blue Protocol, WuWa RT) belongs to the character note.

**Mobile**
- No full-screen mask pass, because it costs a bandwidth round-trip. Sample the cascades directly in the forward shader.
- 2 cascades (spec): near updated every frame, far every 2–4 frames.
- **Static far term:** either NTE's camera-anchored tiled cache, or for zones a **baked static sun-shadow texture per TOD phase** (Hi-Fi Rush / Genshin style), with dynamic casters only in the near cascade.

**Stylization (all tiers)**
- The shadow factor goes through the terminator remap in §2. Edge width grows with distance (Blue Protocol), and color is the TOD shadow tint.
- Billboard and impostor casters face the light in the shadow pass (WuWa).
- Shadows can be exaggerated through a per-light "shadow stretch" bias for art; this is low priority.

**Artists author** shadow tint per TOD key, edge width near and far, per-zone cascade split overrides, and baked-shadow volumes for hero areas.

---

## 5. Sky, sun and moon, and the time-of-day system

### What the games do

**WuWa** [P 1, 6]
- A stylized custom TOD, not UE physical atmosphere, with real-time sky tuning.
- A 2D skybox pipeline that links 2D art and 3D directly, mostly tintable by TOD, with animated cloud drift.
- **Layered sky:** base sky, stars, Milky Way and clouds. The RT reflection path has to sample each layer on a miss, which is why it bakes the sky into a 512² cubemap, one face per frame.
- **Weather drives sky and clouds**, including special events such as the "reverse rain" (溯回雨), where the sky becomes an inverted sea. The same system runs gameplay, cutscenes and quests.

**Genshin:** 2D cloud cards. Community analysis of the cloud textures says [C 34]:
- R channel: light/dark shading.
- G channel: rim light.
- B channel: an SDF used for dissolve, with a threshold to grow or shrink the clouds.
- A channel: alpha.

**Honkai Impact 3rd** [S 21]
- Skybox settings for sky gradients and cloud lit/shadow colors. Combined, they make sunny, rainy, foggy, cloudy and night weather.
- Cloud lighting in 4 channels: base light, two shadow layers, rim. 8 cloud shape templates.
- Colors keyframed across 24 h. On mobile, clouds were particles and overdraw was the main cost.

**Blue Protocol:** a master post material blends the morning, noon, evening and night looks by TOD. Entering shade switches the post look over several seconds [S 35].

**Physically based option:** Hillaire 2020 runs from mobile to high-end PC [45]. Kuro explicitly chose not to use UE's physical atmosphere for this style [P 6].

### Implementation for Sapphire [I]

**Sky shader** (one full-screen or dome draw, about 0.1–0.2 ms on mobile [est])
- Base: zenith, horizon and ground colors, with horizon height and sharpness.
- Sun halo: a Mie-like lobe with artist color, size and power. Moon disc and halo.
- A star field and a Milky Way texture rotating with sidereal time; intensity is a TOD curve.
- **Painted cloud layers:**
  - **Card clouds** on a dome or as world-space cards, using the Genshin RGBA scheme. Lighting is `mix(shadowColor, litColor, R)` plus `rim * G * sunFacing`, dissolve is `B > threshold(coverage)`, alpha is `A`. Cloud colors come from the TOD key.
  - One or two **scrolling high-altitude layers** (cirrus), UV-scrolled with wind.
  - **Overdraw on mobile:** keep cloud cards few and large. Honkai 3's particle clouds were bound by overdraw.
- Sun and moon discs are written in HDR, so they bloom naturally.

**Reflections of the sky:** render the sky, without the world, into a small cubemap (128² mobile, 256–512² desktop), one face per frame (WuWa's trick). All probe and SSR misses fall back to it.

**TOD data model** (`sapphire_world` owns it; the renderer sees only the resolved values in the extract)

```text
TodTimeline { keys: [TodKey; N], interpolation: Curve per field (OkLab for colors) }
TodKey {
  time: f32,
  sun: { dir_override?, color, intensity }, moon: { color, intensity },
  sky: { zenith, horizon, ground, horizon_height, sun_halo{color,size,power}, stars, milky_way },
  clouds: { lit, shadow, rim, coverage, dissolve_softness, scroll },
  ambient: { sky_sh_tint, ground_bounce, ao_strength, shadow_tint },
  fog: { near{color,density,start}, far{color,density}, height{falloff,offset}, sun_inscatter{color,power}, card_tint },
  post: { exposure, lut_a, lut_b, lut_blend, bloom{intensity,threshold,tint}, diffusion, gradients[], vignette, sharpen },
  char_rig: { key_color, key_intensity, key_dir_offset, rim_color, ambient_color, ramp_row_blend }  // contract with the character note
  local_lights_on: bool    // Genshin turns local lights on at night
}
Overrides: ZoneVolume (blend radius, priority) > Weather layer (blend weight) > Cutscene track
```

**Artists author** TOD keys with curve and gradient editors, zone override volumes (WuWa's per-scene atmosphere and LUT), weather presets as partial key overrides, and cutscene tracks. The editor needs a time scrubber with a character turntable preview under each key.

**Interpolation:** blend colors in OkLab. The local-light switch should be a scheduled event with fade-in, not interpolated.

---

## 6. Fog, volumetrics and light shafts

### What the games do

**Genshin (console)** [P 18]
- **Volumetric fog:** physically based scattering, lit by lights including local lights' projected textures. Parameters vary per area and stream from textures.
- **Froxels** are aligned with the lighting clusters, ray-marched, then temporally filtered. Under 1 ms on PS4.
- **God rays** are a separate pass at 1/2 resolution that ray-marches up to 5 shadow cascades. Fog-based god rays looked worse: voxel resolution was too low, and the density needed made the screen look dirty. The separate pass gives sharper edges and more artist control.

**WuWa**
- Volumetric fog, plus a separate "volumetric light" toggle [C 15]. Fog disappears at mobile Very Low [C via Game8].
- **Fake volumetric fog cards** and post materials to build gradients [P 6].
- **PC RT:** UE4 volumetric fog broke without full CSM, so Kuro pre-builds a 3D shadow volume from shadow rays, at lower precision on lower tiers [P 1].

**ZZZ:** a volumetric fog setting ("light shafts and haze"), also on mobile [C 24, 25].

**Honkai Impact 3rd:** fog split into near and far ranges with separate color and intensity [S 21].

**Blue Protocol:** UE4 volumetric fog plus a custom volumetric shadow system; para gradients and sunshafts in post [S 35].

### Implementation for Sapphire [I]

**Analytic stylized fog** (all tiers, evaluated in the forward shader and the sky shader)
- Two distance layers, near haze and far aerial. Each has its own color, density and start, like Honkai 3.
- Exponential height fog: falloff and offset per zone.
- **Far fog color comes from the sky shader's horizon color in the view direction.** This makes geometry melt into the painted sky, which is the classic anime aerial-perspective read. Sun inscatter is a lobe tinted by the TOD key.
- Cost is a few ALU ops. Transparents use the same function.

**Fog cards:** camera-faded, depth-soft translucent sheets (WuWa). Artists place them for valley mist, light beams in rooms and street haze. They use the particle path with depth fade and distance fade near the camera.

**Desktop froxel volumetric fog** (spec budget 0.6 ms)
- About 160×90×64 froxels, which is finer than the 16×9×24 light grid, so use a separate grid.
- Per-zone density volumes plus a TOD density curve, sun shadow lookup in the cascades, and clustered local lights.
- A temporal reprojection filter, as in Wronski 2014 [46], which Genshin mirrors.

**Light shafts**
- Desktop: a separate half-res pass marching shadow cascades, as Genshin does, for sharp, art-controllable shafts.
- Mobile: screen-space radial "god rays" from the sun's screen position at 1/4 resolution, masked by sky depth. About 0.3–0.5 ms [est]. Prefer fog cards for interior beams.

---

## 7. Weather, water and wet surfaces (brief)

**What the games do**
- **WuWa:** weather drives sky, cloud and rain effects, plus special weathers [P 6].
- **WuWa water:** Single Layer Water. RT reflections that hit water spawn extra rays to fake scattering, which is costly [P 1].
- **ZZZ:** its RT showcase is wet streets and puddles. PS5 Pro 1.4 and PC 3.0 both pitch reflections on "mirrors, water surfaces, puddles" [P 22, 23].
- **Honkai Impact 3rd:** stretches raindrops along a curve in bullet time, with 4 keyframes [S 21].
- **Genshin:** dynamic weather with all lighting real-time [P 18]. Its wet-surface treatment isn't documented.

**For Sapphire [I]**
- **Weather is a TOD override layer** (§5) plus global uniforms: `wetness`, `rain_intensity`, `wind_dir` and strength, `puddle_coverage`.
- **Wetness** in every uber-shader (Lagarde's model [47]):
  - darken porous albedo by about 0.5–0.8×;
  - lower roughness toward 0.05–0.1;
  - fill puddles by a height or mask threshold to a flat, smooth normal;
  - add rain ripples as an animated normal atlas.
  - Anime twist: wet streets are where bloom and reflections of neon emissives sell the shot (ZZZ).
- **Reflections:**
  - **Desktop:** SSR on rough ≤ 0.3 surfaces with a probe fallback. Optional planar reflection for one hero water plane per view, in the frame graph as an extra view.
  - **Mobile-high:** WuWa's trick of SSR from the previous frame's depth and color, reprojected, with no extra pass inside the main pass [P 6].
  - **Mobile-low:** probes only.
- **Rain:** GPU particles (desktop) or a camera-local scrolling rain-sheet cylinder (mobile), plus splash flipbooks on surfaces.

---

## 8. Foliage and grass

### What the games do

**WuWa** [P 6; S 8]
- **NPR foliage.** It shares the characters' stylized path.
- **LOD chain, PC and console:** near trees are mesh trees built from inserted leaf cards; mid are "billboard trees", whose leaf cards rotate to face the camera and approximate the original with 30–40% of the polygons; far are impostors.
- **LOD chain, mobile:** billboard trees near, impostors mid to far.
- Billboards turn to face the light when rendering shadows.
- **Impostors:** baked offline from a ring of views with mini G-buffer data, reconstructed at runtime. Impostor slices stream in and out and batch through dynamic texture arrays, so a large set of trees draws in about 4 calls.
- Community reverse engineering says 256 view directions per tree, and two texture arrays at 160 and 64 px [C 17].

**Normals for soft lighting**
- The standard stylized-foliage trick is to transfer normals from a sphere or hull to the leaf cards, blended around 0.8 so some noise survives [C 48]. Its production origin is DreamWorks' "aggregate normals" [49].
- This gives the soft blob shading that reads as painted. That WuWa or Genshin use it is my inference, not a published fact [I].

**Grass**
- Blue Protocol: UE4 landscape grass. The material reduces color with distance, adds patchy color variation, and scrolls a highlight band to suggest wind [S 35].
- Ghost of Tsushima: procedural GPU blades (GDC 2021) is the reference for dense desktop grass [50].

**Impostor reference:** octahedral impostors (Brucks) [52].

### Implementation for Sapphire [I]

**Tree LOD** matches the spec: cards → billboard leaf clusters → octahedral impostors, with the mobile chain starting at billboards.
- Cook-time steps: sphere and hull normal transfer onto leaf cards (stored per vertex), impostor bake (albedo, normal, depth, AO), and per-cluster pivots for camera-facing.
- **Shadow pass:** impostors and billboards face the light, using a per-view "facing target" uniform.

**Foliage shading** uses the NPR family:
- wrapped N·L on the transferred normal, then the terminator remap;
- canopy AO from cluster depth (inner leaves darker);
- translucency term `pow(saturate(dot(-L, V)), p) * thickness`;
- a vertical color gradient (root to tip);
- distance color simplification.
- Alpha-tested cards: use alpha-to-coverage under MSAA on mobile, and a dithered fade on LOD transitions.

**Wind**
- A global wind texture or direction plus gust noise.
- Three-tier vertex animation: trunk sway, branch bend from pivot data, leaf flutter by vertex color.
- Grass interaction: a character trail texture (a small top-down RT around the player, decayed each frame) bends grass. About 0.1 ms [est].

**Grass**
- Desktop: GPU-generated clump instances per terrain tile (compute), culled into indirect draws.
- Mobile: CPU-placed low-density clumps (spec), shorter fade distance, and terrain color sampled at the root so the field blends into the ground.

**Cost to watch on mobile:** overdraw from alpha-tested cards. Keep card count low near the camera and prefer opaque geometry with leaf silhouettes for close shrubs [est].

---

## 9. Post-processing

### 9.1 Bloom and glow

**What the games do**
- **Honkai Impact 3rd** [S 21]:
  - fp16 HDR target, downsampled to 1/4;
  - brightness threshold;
  - 4 successively halved targets with growing Gaussian radius, combined;
  - bloom, reflection and distortion composited in **one pass** for mobile.
- **WuWa:** bloom at 1/4 resolution with a Gaussian chain (low settings, community capture). UberPost combines color grading, tonemapping, auto exposure and some blur [C 17].
- **HSR:** separate bloom control for characters [C 31].
- **Blue Protocol:** a separate "diffusion" effect that spreads bright areas like bloom. It was built separately because engine bloom conflicted with effect bloom [S 35].
- **References:** a pyramidal, temporally stable bloom (COD:AW [53]) and dual-filter downsample/upsample for mobile bandwidth (Bjørge, SIGGRAPH 2015 [54]).

**For Sapphire [I]**
- **Scene bloom:** a soft-knee threshold on pre-exposed HDR, then a mip chain.
  - Desktop: 1/2 to 1/32 (spec), COD-style 13-tap down and tent-filter up.
  - Mobile: dual filter from 1/4, 4–5 levels.
- **Emissive-driven:** emissive materials write their full HDR value, and the threshold is set so lit albedo rarely blooms.
- **Character mask:** the character pass writes a mask into scene-color alpha. Use it to cap bloom from character skin and faces, and to add an optional character-only rim glow [C 31].
- **Diffusion layer:** a very wide, low-intensity blur of the whole image, reusing the bloom chain's lowest mips, blended with screen or lighten. Intensity and tint come from TOD and zone. This is the "anime soft focus" look and costs almost nothing extra.
- **Artists author** threshold, knee, intensity, per-mip tint, diffusion amount and tint, all per TOD key.

### 9.2 Tonemapping and grading

**What the games do**
- **Genshin** [P 18]:
  - ACES (RRT + ODT) rejected as unsuited to the art style;
  - HDR10 pipeline with a WCG grading LUT, under 0.05 ms;
  - HDR output blended with the tonemapped result to stay close to SDR;
  - artists relied on tone-curve hue shifts, so the hue shift is simulated in shader and **folded into the final LUT**.
- **Honkai Impact 3rd:** filmic tonemap with exposure and contrast controls [S 21].
- **WuWa:** per-scene LUT grading [P 6], and a Kuro-specific tonemapper cvar `r.KuroTonemapping` seen in community configs [C 17].
- **User controls:** WuWa exposes global filter intensity and sharpening [C 15, 16]; ZZZ exposes Color Filter Strength 0–10 on PC and mobile [C 24, 25].
- **HSR:** community recreations use a GT-style tone curve plus LUT [C 31]. That is a guess, not a capture.

**For Sapphire [I]**
- **Tone curve:** neutral-to-slightly-punchy, close to linear in the midtones, with a soft shoulder and a controllable toe.
  - GT (Uchimura, CEDEC 2017) is a good parametric base [55]. AgX and Tony McMapface are references for hue-stable highlight desaturation [56, 57].
  - Avoid ACES's contrast and hue skew, as Genshin did.
- **Bake** the tone curve, the Genshin-style hue-shift simulation and artist grading into one 32³ LUT (spec).
- **Blend 2 LUTs** per frame (key A/B, zone override) in the fused pass.
- **User "filter strength"** is a lerp between a neutral LUT and the graded LUT. Ship it; both target games do.
- **Exposure:** fixed per TOD key, or eye adaptation clamped to ±0.5 EV. Anime framing wants stable values.
- **HDR output:** later (spec). When added, follow Genshin's blend toward the SDR look.

### 9.3 Gradients, flares and screen-edge light (the "compositing" layer)

**What the games do**
- Blue Protocol reproduces anime camera-stage effects: "para" gradients, sunshafts, light flares, and environment light entering from screen edges like a dodge [S 35].
- WuWa expresses gradients through lights, fog cards and post materials [P 6].

**For Sapphire [I]** (all in the fused post pass)
- **Gradient layers:** 2–4 per frame, each a linear or radial ramp with a blend mode (multiply, screen, overlay, soft-light), an opacity curve and an optional depth range (apply only beyond X m). Driven by TOD key, zone or cutscene track. Cost is about 2–4 ALU ops per layer.
- **Lens flares:** sprite flares (ghosts and streaks) placed along the sun-to-center axis, faded by a few depth taps at the sun position (the occlusion test). About 0.05 ms on mobile [est]. Anime prefers big, colorful, deliberate flares; artists author flare assets per TOD key.
- **Light leaks / edge dodge:** screen-edge additive gradients tinted by sun color, keyed to sun screen position.

### 9.4 DOF, motion blur, CA, vignette, grain and sharpening

**What the games do**
- **DOF:** Honkai Impact 3rd rendered the background with a separate camera rather than depth-based CoC, used hexagonal bokeh, and lowered blur resolution as blur grew [S 21].
- **Motion blur:** WuWa offers it as an option, and guides recommend off [C 12].
- **ZZZ:** uses blur selectively: an overlay on heavy hits, and motion blur only on the fastest frames of enemy attacks to telegraph them [C 26]. It also has a "Distortion" (heat-haze) setting [C 24].

**For Sapphire [I]**
- **DOF:** cutscenes and photo mode only. Desktop: half-res gather bokeh. Mobile: a cheap two-layer blur (near and far) from the bloom mips.
- **Camera motion blur:** off by default.
- **Per-object motion blur or smear:** artist-authored on attacks, through VFX smear meshes or a per-object velocity-masked blur on desktop.
- **Chromatic aberration, vignette, grain:** fused-pass parameters driven by TOD and gameplay events. CA is mostly a gameplay pulse.
- **Sharpening:** RCAS after upscaling or TAA (WuWa exposes it) [58].

---

## 10. Combat and anime screen effects

**Public documentation on how these games build their combat screen effects is thin.** Classification:

- **[C 26] ZZZ combat analyses:**
  - Taking a hit overlays a blur with old-TV stripes to express a brief daze.
  - Heavy moves overlay a blur to sell their weight.
  - Motion blur on the fastest few frames of enemy attacks helps players read the key frames.
- **[C 30] ZZZ result-screen recreation:** glitch-art post transitions, SDF text and color separation.
- **[C 26b] Fan Unity study of a ZZZ sequence:** reproduced with radial blur and monochrome impact frames. The animation comes from the game; the camera work was rebuilt.
- **[P 29] ZZZ's story presentation** moves between dialogue, comic-panel presentation and 3D action, and the team says it chose the comic style because they think it looks cool.
- **[O] Visible in ZZZ** (verify by capture):
  - Chain Attacks freeze the action and present the next agents in stylized split panels.
  - Perfect dodges trigger slow motion with a world color shift.
  - EX Specials and Ultimates use camera cut-ins with flash frames, speed-line backgrounds and comic-style typography.
  - The Hollow and TV aesthetic adds scanlines, RGB split and glitch in transitions.
- **[O] Visible in WuWa:**
  - Resonance Liberation (ultimate) camera cinematics with flash frames.
  - Perfect-dodge slow motion with a color shift.
  - Intro/Outro swap cut-ins, and hit-stop on heavy hits.

### Effect stack for Sapphire [I]

Spec §4 `PostParams` already lists finish-freeze, halftone, radial blur and CA. Make it a small, timeline-driven layer system.

| Effect | Implementation | Mobile cost [est] |
|---|---|---|
| **Hit-stop / freeze frame** | Gameplay: a per-entity time scale for 2–6 frames (rule of thumb). Render: no change, or a 1-frame impact flash. | 0 |
| **Impact frame** (invert / duotone / mono) | Fused pass: `lum → ramp(colorA, colorB)` or `1 - c`. Character mask selects "keep characters colored" or "only characters". Lasts 1–3 frames. | ~0 (in fused pass) |
| **World desaturate, keep characters** (dodge or time-slow) | Saturation lerp gated by the character mask. Optional cool tint. | ~0 |
| **Radial blur** | 8–16 taps toward a screen-space center, with strength falloff and center from a hit point or the camera. Mobile: 6 taps at half res, or reuse a bloom mip. | 0.3–0.5 ms half-res |
| **Speed lines** | Procedural polar noise `step(noise(angle * k, t), threshold)` with an inner radius mask, in post. Or camera-space VFX meshes for authored lines. | ~0.1 ms |
| **Halftone** | Screen-space rotated dot grid. Dot radius from luminance, from shadow factor (shadow areas only) or from an effect mask. A comic look. | ~0.1 ms |
| **CA / RGB split pulse** | Radial offset of R and B, driven by an event curve. | ~0 |
| **CRT / glitch** | Scanlines, block displacement and color separation for transitions (ZZZ TV aesthetic). | ~0.1 ms |
| **Screen flash / vignette pulse** | A color plus a curve. | 0 |
| **Cut-in** | An extra frame-graph view: the character on a stage with its own light rig, rendered offscreen at reduced res. Composite in the UI layer over 2D art (speed lines, gradients, typography). | One small extra view: 1–2 ms only while active |
| **Comic panels** | 2–4 views into atlas regions, each with its own camera. Panel borders and masks in UI. Draw characters plus a simple backdrop, not the world. | N small views while active |

**Plumbing**
- Effects are timeline events on skill assets: start, duration, curves and parameters. Gameplay emits them through the existing message queues (spec rule R3). Extract resolves them into `PostParams`, a fixed-size struct, so there are no allocations.
- **Character mask source:** a bit in scene-color alpha, or the stencil bit the spec already uses for eyes and brows. wgpu can bind the stencil aspect of a depth-stencil texture as a uint texture, so the post pass can read it.

---

## 11. Anti-aliasing, upscaling, and keeping outlines intact

### What the games do

**WuWa (UFSH2023)** [S 8]
- NPR frames carry about 4–5% edge pixels against 1–2% for PBR games, so AA is a bigger problem in this style.
- Kuro built a custom TAA, then TAAU with upsampling, for mobile, and added a velocity pass.
- Animated characters needed a separate **character velocity** buffer to avoid ghosting.
- **Outlines weren't written into the velocity/character mask** (for cost), which caused black lines.
- The current-frame weight rises with motion.
- **TAA placement matters:** putting it after bloom and tonemapping amplified edge noise.

**WuWa PC options:** Off / FXAA / SMAA / TAA, plus DLSS / DLAA, FSR and XeSS 3 [C 12; S 9, 10].
- In a mobile player guide, AA was ranked the single most important visual setting [C 16].

**Blue Protocol:** chose **Responsive AA instead of TAA**, with a stencil that protects the current frame's outlines from blur. Less anti-aliasing, but it looks better in motion [S 35].

**ZZZ:** TAA or SMAA on PC; TAA on mobile [C 24, 25]. DLSS from 3.0 [P 23].

**HSR / Genshin:** MetalFX on iOS (HSR from 2.0 [S 32]; Genshin discussed at the A17 Pro launch [S 33]).
- Genshin on PC offers SMAA, TAA and FSR 2 per player guides; HSR offers TAA and FXAA [C].

### Implementation for Sapphire [I]

**Desktop TAA** (spec): run it on HDR before bloom and tonemapping, with a tonemap-weighted resolve (WuWa's ordering lesson).
- **Every pass that writes color must write velocity, outlines included.** The inverted hull uses the same skinned buffer with previous-frame positions. This avoids WuWa's black-line artifact.
- **Responsive mask** written by the outline and character passes (or the stencil bit). It reduces history weight and lowers neighborhood-clamp tolerance there, the Blue Protocol idea.
- **External upscalers:**
  - FSR: the reactive and transparency/composition masks [59].
  - DLSS: the bias-current-color mask [59].
  - MetalFX: `reactiveMaskTexture`. Apple DTS describes it as geared to particles that appear and disappear quickly; that answer was given for the newer denoised scaler [60].
- **Minimum outline width ≥ 1.0–1.5 px at internal resolution.** Scale the width with the upscale ratio so thin lines don't break up.

**Mobile**
- **MSAA 4× on tile.** wgpu 30 has `TextureUsages::TRANSIENT_ATTACHMENT`, merged June 2026 and aligned with the WebGPU transient-attachments proposal [61]. It maps to memoryless storage on Metal and lazily allocated memory on Vulkan.
  - Recipe: MSAA color and depth as transient with `StoreOp::Discard`, resolving into a single-sample HDR target. **This closes the spec's "Verify" item**, but measure on a Mali and an Adreno device.
  - MSAA handles inverted-hull outlines and geometric edges cleanly.
  - Use **alpha-to-coverage** for foliage cards.
  - Shader aliasing still needs analytic AA on two-tone step edges: `fwidth`-based smoothstep.
- **Depth for post (fog, SSR, AO) under MSAA:**
  - Keep a cheap 1× depth prepass only where screen-space effects need it, on Mobile-High.
  - Otherwise rely on TBDR hidden-surface removal (Apple HSR, Mali FPK, Adreno LRZ) and skip the prepass.
  - Don't do depth fetch in the main pass on Mali; it disables FPK [S 8].
- **Upscaling:**
  - iOS: MetalFX spatial, or temporal on capable devices.
  - Android: Arm ASR (MIT, mobile-tuned FSR2 derivative; vendor-claimed up to 53% over native) [62], Snapdragon GSR [63], or an EASU + RCAS port [58].
  - A custom TAAU (WuWa's route) is a later option.
- **Fallback:** FXAA in the fused pass (spec).

**wgpu limitation: no subpass input attachments or framebuffer fetch.**
- Issue #10148 is still open as of 2026-09-18, and naga's support (#5233) is unimplemented [61].
- So **WuWa-style One Pass Deferred is not available in wgpu.**
- This confirms the spec's forward+ decision for mobile. Revisit only if #10148 lands.

---

## 12. Mobile adaptations: what the games turn off or replace

| Area | WuWa mobile | ZZZ mobile | Genshin mobile | Notes |
|---|---|---|---|---|
| Pipeline | One Pass Deferred (PLS / framebuffer fetch); NPR path merged in [P 6; S 8] | Unknown | Separate mobile pipeline, mobile primary [P 18] | wgpu can't do one-pass deferred; use forward+ |
| GI / RT | None. RT is PC only; mobile RT "under evaluation" [P 2] | None (RT only on PS5 Pro and PC) | None | — |
| AO | GTAO on high [P 6] | "Shading quality" option [C] | Not documented | — |
| Reflections | SSR from previous frame, medium+ [P 6] | Mirror-reflection option on PC; mobile not documented | Not documented | — |
| Shadows | Quality tiers; off at Very Low [C] | Shadow precision [C 25] | Quality tiers | NTE-style cached far shadows [S 37] |
| Fog | Volumetric fog off at Very Low [C] | Volumetric fog toggle [C 25] | On only with shadow quality ≥ medium (https://genshin-impact.fandom.com/wiki/Settings, via search excerpt) [C] | Analytic fog everywhere |
| Bloom / post | Bloom off at Very Low; filter intensity and sharpening [C] | Bloom toggle; Color Filter Strength [C 25] | Bloom option | Fused pass |
| AA / upscale | Custom TAAU [S 8] | TAA [C 25] | TAA / FSR (guides) [C] | HSR: MetalFX since 2.0 [S 32] |
| Trees | Billboard near, impostor mid/far [P 6] | Urban, few trees | Not documented | — |
| Frame rate | 60 cap; own frame interpolation from 3.3 (output capped at 60, lower power than native 60); Vulkan from 3.6 [S 9–11] | 60 cap [C 25] | 60 (120 on some iOS devices, unverified) | — |
| Assets | Lower texel and mesh density for mobile [P 6] | — | Terrain blocks capped at 8 layers [S 19] | — |

**Internal render resolutions on phones aren't published for any of these games.** Spec §3.1 tiers stay a proposal to measure.

**Known mobile tricks applicable to Sapphire [I]**
- **One fused post pass:** bloom composite, two-LUT grade, gradients, vignette, combat FX, FXAA if needed. This is Honkai 3's single-pass composite [S 21] and is already in the spec.
- **Half or quarter res** for bloom, light shafts, AO and SSR; full res only for the main pass and UI.
- **Previous-frame data** for SSR instead of extra passes (WuWa).
- **Amortize:** reflection-probe relight (1 face or 1 probe per N frames), sky cubemap (1 face per frame), far shadow cascades (every 2–4 frames, or NTE-style tiles).
- **Overdraw is the enemy:** cloud cards, foliage cards, fog cards and VFX. Budget them per zone, with an overdraw view in the editor.
- **Per-device profiles:** NTE ships 390k profiles plus a 135-device whitelist, and runtime checks before enabling heavy features [S 37]. Sapphire's `GpuCaps` plus a curated device table is the small version.
- **Frame interpolation** (WuWa 3.3+) is a later option for 30→60 on mid-tier Android. It is a power play, not a latency play.

---

## 13. Things that are not publicly documented

1. WuWa's non-RT ambient/GI solution (mobile, and PC with RT off), and its exact TOD shadow scheme. The UFSH2023 video [7] is the only lead.
2. Anything official about ZZZ's or HSR's world rendering. There are no talks. ZZZ's G-buffer or deferred status, its reflection method behind "Mirror Reflections", and its TOD lighting transitions are all unverified.
3. How WuWa's "Lumen GI" option relates to its ray-tracing toggle in the menu [C 12].
4. Official mobile render resolutions for every title.
5. Exact bloom and tonemap curves for any of the four games. Community captures only [C 17, 31].
6. How the combat cut-ins and comic panels are built (render-to-texture vs. pre-rendered vs. 2D animation). Observation only.

---

## 14. Implications for Sapphire

### 14.1 Changes to `docs/engine-spec.md` §4 (Rendering) to consider

1. **Ambient:** "L2 SH probes baked per zone" becomes a **per-zone probe grid with sky-visibility SH plus per-TOD-key bounce, and relightable reflection probes** (§3). Without this, continuous TOD makes the world's ambient wrong.
2. **TOD:** add the `TodTimeline` / `TodKey` contract (§5) as the single source for sky, fog, ambient, shadow tint, post and the character rig. The character rig fields are shared with the character note.
3. **Environment shader:** add the terminator remap with distance-scaled width, shadow tint, the optional terminator band, and distance desaturation (§2).
4. **Post:** add a **diffusion layer**, a **character bloom mask**, **2–4 gradient/para layers**, **sprite lens flares**, two-LUT blending and a user filter-strength slider (§9).
5. **TAA:** TAA runs before bloom and tonemapping. Velocity is mandatory for outlines. Add the responsive mask, and the upscaler mask plumbing (§11).
6. **Mobile AA:** resolve the "Verify". wgpu 30 `TRANSIENT_ATTACHMENT` enables on-tile MSAA; measure on Mali and Adreno. Add alpha-to-coverage for foliage, and a note that one-pass deferred is impossible until wgpu #10148 lands.
7. **Shadows:** sun quantization for far cascades. Desktop gets a shadow-mask pass with penumbra classification and contact shadows. Mobile options: baked static sun shadow per TOD phase, or a camera-anchored tiled far cache. Billboards and impostors face the light in the shadow pass (§4).
8. **Foliage:** NPR shading with cooked sphere and hull normals; impostor texture arrays streamed by slice (§8).
9. **Fog:** analytic fog colored by the sky horizon on all tiers; fog cards; froxel fog and a separate shaft pass on desktop; screen-space shafts on mobile (§6).
10. **Combat FX:** a timeline-driven `PostParams` effect stack, plus extra views for cut-ins and comic panels (§10).

### 14.2 Work packages

Tier key: D = desktop, MH = Mobile-High, ML = Mobile-Low. Costs are [est] targets to validate with measurements.

| WP | Content | Tiers | Depends on | Budget |
|---|---|---|---|---|
| **L1 TOD core** | `TodTimeline` / `TodKey` assets, OkLab curve evaluation, zone override volumes, weather layer, cutscene track, editor scrubber with character preview | All | Extract, editor | CPU < 0.05 ms |
| **L2 Sky** | Gradient sky, sun and moon, stars and Milky Way, RGBA card clouds with SDF dissolve, cirrus scroll, sky-only cubemap (1 face per frame) | All | L1 | 0.1–0.3 ms |
| **L3 Analytic fog + fog cards** | Two-layer distance fog plus height fog, sky-horizon color, sun inscatter; card material with depth and camera fade | All | L1, L2 | ~0 plus card overdraw |
| **L4 Ambient probes** | Cooker bake (sky visibility L1, bounce per TOD key), sparse brick grid, runtime sampling in forward shaders | All (ML: L1 bounce) | Cooker, L1 | < 0.1 ms |
| **L5 Reflection probes** | Mini-G-buffer capture, compute relight, prefilter, encode (BC6H on desktop, RGB9E5 on mobile), round-robin, box projection | D, MH (ML static) | L4 | 0.2–0.4 ms (D) |
| **L6 Shadows v2** | Cascade caching with sun quantization; desktop shadow mask with penumbra classification and Bend-style SSS; mobile baked phase shadow or tiled far cache; light-facing billboard casters | All | Spec's CSM | Within spec shadow budget |
| **L7 AO** | XeGTAO port at half res (D, MH), capsule AO (all), analytic AO volumes (all) | All | Prepass normals | D 0.5 ms, MH 1–1.5 ms |
| **L8 Env uber-shader** | Stylized GGX with wrap, terminator remap, shadow tint and band, distance stylization, wetness uniforms | All | L1, L4 | Same as GGX |
| **L9 Foliage and grass** | Normal transfer in the cooker, NPR foliage shading, 3-tier wind, impostor bake and texture-array streaming, GPU grass (D), trail texture | All | L8, streaming | Overdraw-bound |
| **L10 Volumetrics (desktop)** | Froxel fog with temporal filter, half-res shaft pass; mobile screen-space shafts | D (MH shafts only) | L3, L6 | D 0.6 + 0.3 ms |
| **L11 Water and wet** | Wetness model, puddle masks, ripples, rain sheets and particles, SSR (D, MH previous-frame), optional planar for a hero plane | All (scaled) | L5, L8 | SSR D 0.8, MH 1.0 ms |
| **P1 Post core** | Bloom (COD-style on D, dual filter on mobile), character bloom mask, diffusion, tone curve plus hue shift baked to 32³ LUT, two-LUT blend, gradient layers, sprite flares, vignette, RCAS; mobile fused pass | All | L1 | Spec post budget |
| **P2 AA and upscaling** | TAA with velocity everywhere, responsive mask; DLSS / MetalFX / FSR masks; mobile transient MSAA with A2C and analytic step AA; Arm ASR or EASU path | All | Character outline pass | Spec AA budget |
| **P3 Combat FX stack** | Event-driven `PostParams` layers from §10, character mask plumbing, hit-stop hooks with gameplay | All | P1, gameplay events | ~0 to 0.5 ms while active |
| **P4 Cut-ins and panels** | Extra offscreen views, stage lighting presets, UI composite, panel atlas | All | P3, multi-view frame graph | 1–2 ms while active |
| **P5 Cinematic post** | Half-res bokeh DOF (D), cheap DOF (mobile), per-object smear blur (D) | D (MH simplified) | P1 | Cutscene only |

**Suggested order for an "anime look" vertical slice:** L1 → L2 → L3 → L8 → P1 → L4 → L6 → P2 → L9 → P3 → L7 → L5 → L11 → L10 → P4 → P5.
- The first five deliver most of items 1, 2 and 4 in §0's ranking.
- Ray tracing stays out of scope, matching the spec's non-goals.

---

## 15. Version-specific facts, as of 2026-10-24

- **WuWa 2.1** (2025-02-13): PC ray tracing (reflections, GI, shadows) [P 2].
- **WuWa Mac** (2025-03-27) [S 14].
- **WuWa 2.8:** lower RT GPU load [C 13].
- **WuWa 3.3** (2026-04-30): mobile frame interpolation test (Snapdragon 8s Gen 4), anisotropic filtering option, DLAA [S 9, 10].
- **WuWa 3.5** (2026-07-10): interpolation on 8 Gen 3 and 8 Gen 2, XeSS 3, Xbox Series [S 10].
- **WuWa 3.6** (2026-08-20): DLSS 4.5 with 5× and 6× multi-frame generation, RT Megalights on AMD, Android Vulkan, frame generation on 8 Gen 5 [S 11].
- **WuWa engine:** still UE4.26 per the Kuro talk of November 2025 [P 2]. No confirmed UE5 migration.
- **ZZZ 1.4** (2024-12-18): PS5 Pro RT reflections and "indirect highlights" [P 22].
- **ZZZ 3.0** (2026-06-17): PC RT (reflections, AO, GI), DLSS Super Resolution and frame generation, Steam launch [P 23]. Multi-frame generation promised later.
- **HSR 2.0** (early 2024): MetalFX on iOS [S 32].
- **Genshin:** stated no ray tracing (2021 interview) [S 20]. No later reversal found.

---

## 16. Sources

Numbers 5, 27 and 38–41 are unused; the gaps keep the inline citations stable.

**Wuthering Waves / Kuro**
1. Wang Xin (Kuro), "『鳴潮』におけるレイトレーシング——光で描くアニメ風オープンワールド", Unreal Fest Tokyo 2025, slides. https://www.docswell.com/s/EpicGamesJapan/58W89L-UE_UFT2025_KUROGAMES [P]
2. 4Gamer report of [1], 2025-11-17. https://www.4gamer.net/games/634/G063420/20251117029/ [S]
3. GameMakers report of [1], 2026-01-26. https://gamemakers.jp/article/2026_01_26_129127/ [S]
4. The same talk at UFSH2025, video. https://www.bilibili.com/video/BV1hSW4zTEgQ/ [P]. Its AI-augmented summary, with code marked [AI补充] as non-original: https://wlxklyh.site/uf2025-shanghai/rendering/wuthering-waves-raytracing/ [S/C]
6. Unreal Engine developer interview, "Exploring the Post-Apocalyptic Charm of ASG Open Worlds in Wuthering Waves" (2024). https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves [P]. Read via the Chinese mirrors: https://news.qq.com/rain/a/20240603A0AD6U00 and https://www.youxituoluo.com/532205.html
7. Wang Hongbo (Kuro), "《鸣潮》基于虚幻引擎4的多平台效果和性能优化实践", UFSH2023, video. https://www.bilibili.com/video/BV1BK411v7FY/ [P]
8. Attendee notes on [7]: https://xingyudev.site/2024/01/20/%E3%80%8A%E9%B8%A3%E6%BD%AE%E3%80%8B%E5%9F%BA%E4%BA%8E%E8%99%9A%E5%B9%BB%E5%BC%95%E6%93%8E4%E7%9A%84%E5%A4%9A%E5%B9%B3%E5%8F%B0%E6%95%88%E6%9E%9C%E5%92%8C%E6%80%A7%E8%83%BD%E4%BC%98%E5%8C%96%E5%AE%9E%E8%B7%B5/ and https://zhuanlan.zhihu.com/p/20489581396 (Zhihu read via search excerpt only) [S]
9. Wuthering Waves 3.3 coverage (patch content), 2026-04-30. https://finalweapon.net/2026/04/30/wuthering-waves-version-3-3-reverbs-from-the-end-of-galaxies-update/ [S]
10. IT之家, 3.5 announcement including the 3.3 interpolation notes, 2026-06-27. https://www.ithome.com/0/969/268.htm [S]
11. Wuthering Waves 3.6 performance guide (summarizes patch notes), 2026-08. https://wutheringpity.com/wuthering-waves/36-performance-guide/ [S]
12. Inven Global, WuWa graphics options guide, 2026-05-01. https://labs.invenglobal.com/articles/21520/just-follow-these-settings-a-guide-to-visual-frame-rate-changes-by-graphics-option [C]
13. "Wuthering Waves Performance & Stutter" analysis blog (cites patch notes, Engine.ini and talks), 2026-04-12. https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html [C]
14. AppleInsider, WuWa on Mac, 2025-03-27. https://appleinsider.com/articles/25/03/27/wuthering-waves-and-control-ultimate-edition-now-available-on-mac [S]
15. WuWa 3.2 PC settings guide. https://www.qiyou.cn/games/251749.html [C]. Game8 settings guide: https://game8.co/games/Wuthering-Waves/archives/453931 [C]
16. 17173, WuWa mobile settings guide, 2026-05-19. https://news.17173.com/content/05192026/143022128.shtml [C]
17. Community WuWa captures and config notes (read via search excerpts only; Zhihu blocks fetching):
    - Pipeline capture: https://zhuanlan.zhihu.com/p/700588376
    - Config including `r.KuroTonemapping`: https://zhuanlan.zhihu.com/p/1904643716599251561
    - Mobile impostor analysis: https://zhuanlan.zhihu.com/p/1891431033540416679
    [C]

**Genshin / HSR / Honkai 3 (HoYoverse)**

18. Zhenzhong Yi (miHoYo), 「原神」におけるコンソールプラットフォーム開発 (Unity Dojo 2021; the China Unity Tech Week / Unite Seoul 2020 material). https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi and https://www.slideshare.net/slideshow/210617-unity-dojo20211mihoyozhenzhongyi/249437133 [P]
19. Haoyu Cai, "'Genshin Impact': Crafting an Anime-Style Open World", GDC 2021. https://www.youtube.com/watch?v=-JFyAdI_rO8 [P]. Preview: https://www.gamedeveloper.com/design/learn-about-the-making-of-i-genshin-impact-s-i-open-world-at-gdc-2021. Transcript excerpts (read via search excerpt): https://mihoyo.fandom.com/wiki/Game_Developers_Conference_2021 [S]
20. TechRaptor, "No Genshin Impact ray tracing to preserve art style". https://techraptor.net/gaming/news/no-genshin-impact-ray-tracing-to-preserve-art-style [S]
21. Unite 2018, 《崩坏3》 toon rendering in Unity, summary (Part 1). https://www.cnblogs.com/alps/p/9399054.html [S]
22. ZZZ 1.4 PS5 Pro (quotes the PlayStation Blog). https://mp1st.com/news/zenless-zone-zero-update-1-4-arrives-dec-18-includes-ps5-pro-support-new-agents-features-and-more [S/P]. Famitsu dev interview, 2025-01: https://www.famitsu.com/article/202501/27956 [P]
23. NVIDIA GeForce News, ZZZ 3.0 DLSS and ray tracing. https://www.nvidia.com/en-gb/geforce/news/zenless-zone-zero-version-dlss-4-5-ray-tracing [P]. PC Guide: https://www.pcguide.com/news/zenless-zone-zero-version-3-0-arrives-soon-with-dlss-4-5-and-ray-tracing-after-nvidia-shows-off-comparison-video/ [S]
24. PCOptimizedSettings, ZZZ options benchmarked. https://pcoptimizedsettings.com/zenless-zone-zero-zzz-pc-optimized-settings-every-graphics-option-benchmarked/ [C]
25. PerfDog ZZZ mobile test, part 1 (v1.1 mobile settings). https://m.163.com/dy/article/JEVNHDNL0511A3C8.html [C]
26. ZZZ combat system analysis. https://www.gameres.com/904209.html and https://zhuanlan.zhihu.com/p/671440178 [C]
    - 26b. KZhang, Unity VFX study of a ZZZ sequence (radial blur, monochrome impact frames). https://www.artstation.com/artwork/dyk2PQ [C]
28. ZZZ time phases guide. https://www.pushsquare.com/guides/zenless-zone-zero-how-to-pass-time [C]
29. Inside Games, ZZZ producer interview (comic presentation). https://www.inside-games.jp/article/2024/01/14/151640.html [S]
30. ZZZ results-screen recreation (glitch, SDF text, color split). https://zhuanlan.zhihu.com/p/708315044 (via search excerpt) [C]
31. HSR recreations (deferred scene, forward characters, character bloom; post stack). https://zhuanlan.zhihu.com/p/693831511 and https://zhuanlan.zhihu.com/p/711037262 (via search excerpts) [C]
32. iPhoneSoft, HSR 2.0 adds MetalFX on iOS. https://iphonesoft.fr/2024/01/29/version-2-0-honkai-star-rail-mobile-pc-ps5 [S]
33. Sportskeeda, Genshin developer on A17 Pro and MetalFX. https://www.sportskeeda.com/esports/news-genshin-impact-developer-applauds-apple-a17-pro-gpu-latest-iphones-performance-boost [S]
34. Genshin card-cloud channel analysis. https://zhuanlan.zhihu.com/p/637947850 (via search excerpt) [C]

**Comparators**

35. Blue Protocol:
    - CEDEC 2020 report (CGWorld): https://cgworld.jp/feature/202011-cedec2020-bluep.html [S]
    - CEDEC 2021 report (4Gamer): https://www.4gamer.net/games/467/G046741/20210826001/ [S]
36. Hi-Fi Rush:
    - Kosuke Tanaka and Takashi Komada, "3D Toon Rendering in Hi-Fi Rush", GDC 2024: https://gdcvault.com/play/1034330/3D-Toon-Rendering-in-Hi [P]
    - 80.lv summary: https://80.lv/articles/the-making-of-hi-fi-rush-s-3d-toon-rendering-style [S]
    - GCORES notes: https://www.gcores.com/articles/182264 [S]
37. Inven Global, NTE at Unreal Fest Seoul 2026 (Feng Tianli, Liu Zhenyang), 2026-08-21. https://www.invenglobal.com/articles/25007/how-neverness-to-everness-mastered-high-density-open-world-and-pcmobile-optimization [S]

**Techniques and tools**

42. Jimenez et al., "Practical Real-Time Strategies for Accurate Indirect Occlusion" (GTAO). https://www.activision.com/cdn/research/Practical_Real_Time_Strategies_for_Accurate_Indirect_Occlusion_NEW%20VERSION_COLOR.pdf
43. XeGTAO (MIT). https://github.com/GameTechDev/XeGTAO
44. Bend Studio, "Inside Bend: Screen Space Shadows" (open-source code and SIGGRAPH 2023 slides). https://www.bendstudio.com/blog/inside-bend-screen-space-shadows/
45. Hillaire, "A Scalable and Production Ready Sky and Atmosphere Rendering Technique" (EGSR 2020). https://diglib7.eg.org/bitstream/handle/10.1111/cgf14050/v39i4pp013-022.pdf
46. Wronski, "Volumetric Fog" (SIGGRAPH 2014). https://bartwronski.com/wp-content/uploads/2014/08/bwronski_volumetric_fog_siggraph2014.pdf
47. Lagarde, "Water drop 2b – Dynamic rain and its effects". https://seblagarde.wordpress.com/2013/01/03/water-drop-2b-dynamic-rain-and-its-effects/
48. Stylized tree normal transfer tutorial. https://www.patreon.com/posts/25192376. Polycount thread: https://polycount.com/discussion/202402/tree-lighting-and-normals-problems-in-ue4 [C]
49. Peterson and Lee (DreamWorks), aggregate foliage normals. https://research.dreamworks.com/wp-content/uploads/2018/07/0454-peterson-Edited.pdf
50. Wohllaib, "Procedural Grass in Ghost of Tsushima", GDC 2021. https://gdcvault.com/play/1027214/Advanced-Graphics-Summit-Procedural-Grass
51. Unreal Engine, capsule shadows overview. https://dev.epicgames.com/documentation/en-us/unreal-engine/capsule-shadows-overview-in-unreal-engine
52. Brucks, "Octahedral Impostors". https://shaderbits.com/blog/octahedral-impostors
53. Jimenez, "Next Generation Post Processing in Call of Duty: Advanced Warfare" (SIGGRAPH 2014). https://www.iryoku.com/next-generation-post-processing-in-call-of-duty-advanced-warfare/
54. Bjørge, "Bandwidth-Efficient Rendering" (SIGGRAPH 2015 Moving Mobile Graphics; dual-filter blur). https://community.arm.com/cfs-file/__key/communityserver-blogs-components-weblogfiles/00-00-00-20-66/siggraph2015_2D00_mmg_2D00_marius_2D00_notes.pdf
55. Uchimura and Suzuki, "HDR Theory and Practice" (CEDEC 2017; GT tonemapper). https://www.slideshare.net/nikuque/hdr-theory-and-practicce-jp
56. AgX. https://github.com/sobotka/AgX
57. Tony McMapface. https://github.com/h3r2tic/tony-mc-mapface
58. AMD FidelityFX FSR 1 (EASU and RCAS). https://github.com/GPUOpen-Effects/FidelityFX-FSR
59. Upscaler masks (FSR reactive and transparency masks; DLSS bias-color mask, as documented by Unigine). https://developer.unigine.com/docs/latest/editor2/settings/render_settings/upscalers/
60. Apple MetalFX temporal scaler descriptor: https://developer.apple.com/documentation/metalfx/mtlfxtemporalscalerdescriptor. Reactive mask guidance from Apple DTS: https://developer.apple.com/forums/thread/790642
61. wgpu:
    - Issue #10148, input attachments (open as of 2026-09-18): https://github.com/gfx-rs/wgpu/issues/10148
    - PR #9568, `TRANSIENT_ATTACHMENT` (merged 2026-06-19, shipped in v30): https://github.com/gfx-rs/wgpu/pull/9568
62. Arm Accuracy Super Resolution (MIT). https://github.com/arm/accuracy-super-resolution
63. Qualcomm Snapdragon Game Super Resolution. https://github.com/SnapdragonStudios/snapdragon-gsr
