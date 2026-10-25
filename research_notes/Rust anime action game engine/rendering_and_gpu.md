# Anime Rendering Techniques and Rust GPU Stack for a Custom Anime Action Game Engine (as of Oct 24, 2026)

Scope note: Part A covers techniques (A1–A7). Part B covers Rust libraries and the recommendation (B1–B6). Crate versions and dates come from the crates.io API, and repo status from the GitHub API, both queried on 2026-10-24. The crates.io "updated" date is the date of the newest version unless noted. HoYoverse (Genshin, HSR, ZZZ) and Kuro (WuWa) have published little official material in English. Most per-game shader detail comes from (a) a few official talks (Unity Dojo/Unite, CEDEC, GDC, UFSH) and (b) community reverse-engineering. The findings below label which is which.

---

## A1. Character shading: how is the high-end anime character look built (ramps, lightmap/ILM, vertex colors, SDF face shadows, hair, rim, eyes, normals)?

### Takeaway
Every shipped title uses the same core recipe: a thresholded N·L ("2-tone") lighting term. Artists control it per pixel with painted masks (an ILM/lightmap texture with a threshold-offset channel) and per vertex with vertex colors. Shadow colors are picked from per-material ramp or "shadow color" textures, not computed by darkening. Faces are special-cased: either custom-edited normals (Guilty Gear Xrd, Blue Protocol) or a light-angle "threshold map"/SDF face texture (miHoYo lineage, Hi-Fi Rush). Self-shadowing from shadow maps is usually suppressed or offset on characters.

### Cited Findings
**Guilty Gear Xrd (Arc System Works, UE3; GDC 2015, Junya C. Motomura)**
- Talk: "GuiltyGearXrd's Art Style: The X Factor Between 2D and 3D", GDC 2015, by technical artist/lead character modeler Junya C. Motomura, who also wrote the shaders. Handout PDF hosted by ArcSys — [GDC Vault](https://gdcvault.com/play/1022031/GuiltyGearXrd-s-Art-Style-The); [ArcSys post](https://www.arcsystemworks.com/guilty-gear-xrd-art-talk-at-game-developers-conference-2015/); [handout PDF](https://www.ggxrd.com/Motomura_Junya_GuiltyGearXrd.pdf). The PDF was unreachable from this session (connection refused), so its content is known only through search excerpts.
- Goal: rebuild a 2D sprite fighter in 3D "while maintaining the charm the sprites had". The game still plays on a 2D plane — [handout PDF via search excerpt](https://www.ggxrd.com/Motomura_Junya_GuiltyGearXrd.pdf).
- Team "shunned mathematical accuracy to create perfectly consistent cel shading". Commenters note that editing normals smooths the shading — [BlenderNation](https://www.blendernation.com/2015/07/26/junya-c-motomura-behind-the-scenes-of-guilty-gear-xrd/).
- Community-reported texture/vertex channel layout (secondary source, widely reproduced):
  - ILM RGBA: R = specular intensity, G = shadow bias (threshold offset), B = specular size, A = inner lines.
  - "SSS" texture: RGB shadow-color multiplier, not real subsurface scattering.
  - Vertex color: R = ambient occlusion, G = outline scaling, B = outline Z-offset, A = outline width.
  - Source: [Polycount discussion](https://polycount.com/discussion/comment/2186979) / [Polycount](https://polycount.com/discussion/comment/2344470) (the search tool did not say which of these posts holds the layout).
- The "Motomura line" (inner lines drawn as texture-space lines in the ILM alpha) addresses line anti-aliasing — [GCORES translation/commentary](https://www.gcores.com/articles/181422).

**miHoYo lineage (Honkai Impact 3rd → Genshin → HSR → ZZZ)**
- Unite Tokyo 2018, Jack He (miHoYo TD), "anime-style rendering from mobile to high-end PC" — [slides (docswell)](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd):
  - "Multi-channel 2D ramps" for diffuse and specular, stacked 1–3 layers. "Brush-style" ramp shading. Diffuse ramp with soft-ramp mask.
  - Face: "Vertex color Mask control diffuse intensity".
  - Hair: tangent-based anisotropic lighting, multi-layer specular, roughness/flow/AO maps. Stylized hair strip pattern driven by a jitter map. Silk lit along the bitangent, with 1–3 highlight layers.
  - Eyes: refraction faked by "Texture UV offset based on View dir".
  - Also listed: IBL, rim light, PCSS soft shadows.
- Genshin lightmap layout (community/modding wiki, from datamined materials): RGBA; R = specular/metallic mask, G = hand-painted shadow mask, B = specular threshold, A = material ID. The material ID selects the ramp row — [HoyoToon wiki via search excerpt](https://github.com/Melioli/HoyoToon/wiki/Using-the-Genshin-Shader). My direct fetch of that page did not show this text, so verify against the shader source.
- Genshin face shadow: a light-direction-dependent "facial shadow gradient / SDF texture". It needs sRGB disabled. Known issue: shadows cast onto the face by other objects become unstable at close range — [NoiRC256/URPSimpleGenshinShaders](https://github.com/NoiRC256/URPSimpleGenshinShaders). The texture "stores shadow coverage information for left-side lighting" and is mirrored for the other side — [same repo, via search excerpt](https://github.com/NoiRC256/URPSimpleGenshinShaders).
- Genshin uses "shadow ramps for each character" and does not self-shadow faces or characters. Its subsurface look is "very artificial" — [B. Ayers, Blender NPR: Recreating the Genshin Impact Shader](https://bjayers.artstation.com/blog/category/5).
- Face shadow texture authoring: hand-paint shadow shapes at many light angles, then merge them with an SDF generator into one gradient map. More angles give finer control — [Face Shadow Map Creation workflow (Notion)](https://erichu33.notion.site/Face-Shadow-Map-Creation-Baking-Workflow-d3b8769021e04683a2f2ae4cf16ac810).
- HSR fan replication (Unity URP, Forward/Forward+): ripped-model support, transparent front hair, self-shadow plus per-object shadows (up to 16 on screen), custom bloom and tonemapping, automated smooth-normal baking. Archived 2025-03-21 — [stalomeow/StarRailNPRShader](https://github.com/stalomeow/StarRailNPRShader). Notes: character rendering uses MRT in a pass after UniversalForward, and custom ACES tonemapping — [ecosyste.ms summary](https://awesome.ecosyste.ms/projects/1135012).

**Blue Protocol (Bandai Namco Online, UE4; CEDEC 2020/2021)**
- Aim: theatrical-anime-quality cel look. The team found that "cel style" meant different things to different staff — [CGWORLD CEDEC 2020](https://cgworld.jp/feature/202011-cedec2020-bluep.html). Many small anime-like touches add up. Cel-look shading conflicts with a free camera — [Famitsu CEDEC 2020](https://www.famitsu.com/news/202009/08205405.html).
- Implementation (CEDEC 2021, Takayoshi Oi) — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html):
  - Small modifications to UE4's deferred renderer, rather than unlit or post-process-only approaches.
  - Artists paint normals so anime shadow shapes appear, such as the triangular shadow on the face.
  - Avatar parts are imported together to avoid hard seams.
  - ShadowColor is stored separately from BaseColor so shadows can shift hue. It is now derived from BaseColor, with separate tables for skin and cloth. Hair uses material-specified Base/Shadow colors.
  - N·L is computed in BasePass because the G-buffer cannot hold two normals.
  - Specular is a brightened base color. Hair highlights are custom node graphs.
  - Point, sky, and indirect light ignore normals (distance or uniform only) to avoid unwanted 3D shading.
  - Cel-shaded surfaces do not receive self-shadows from shadow maps. Cel-shaded and non-cel-shaded objects use separate shadowing with an "offset shadow" method.
  - Rim light uses Sobel-based edge extraction for uniform thickness.
  - Legs are slightly darkened to ground the character.

**Hi-Fi Rush (Tango, UE4 deferred; GDC 2024, Kosuke Tanaka & Takashi Komada)**
- Deferred cel-shading renderer at 60 fps native, with customized UE4 passes plus the team's own passes, combined in deferred post-process volumes — [80.lv](https://80.lv/articles/the-making-of-hi-fi-rush-s-3d-toon-rendering-style).
- Body: thresholded N·L, using step rather than branching. Face: a threshold (height-like) map sampled by comparing the face's horizontal forward against the light's horizontal direction in bone space (normalized by π), flipped for light from the left. Credited to a 2018 Unity talk by miHoYo — [GCORES commentary on the GDC talk](https://www.gcores.com/articles/181912).

**Wuthering Waves (Kuro, heavily customized UE4.26)**
- UE4.26 with UE5 features back-ported. Includes "a custom implementation of character toon/cel shading". Proprietary processing keeps anime characters "soft and natural" under high-fidelity lighting. These are a third-party TA's observations, not Kuro's — [AUTOMATON](https://automaton-media.com/en/column/even-from-a-developers-perspective-wuthering-waves-use-of-unreal-engine-is-borderline-perverse-a-ue4-game-decked-out-in-custom-technology/).
- Kuro team interview (Chinese):
  - A separate character-lighting pipeline with presets and automation.
  - Character brightness is fitted to time of day with curves so it tracks the environment.
  - A custom face-shadow method supports directional and special-mood lights, including vertical shadow maps for top light.
  - Moved from bone-only facial animation to blendshapes. RBF used for deformation correction.
  - Gradients are painted into base color and mask textures.
  - Source: [GCORES interview compilation](https://www.gcores.com/articles/181970).

**Other**
- NetEase GDC 2022 (Chen Zhou), "Finding Harmony in Anime Style and Physically Based Rendering": a practical deferred pipeline with lighting, shadowing, and post for an anime look in a PBR world — [GDC Vault](https://gdcvault.com/play/1027597/Technical-Artist-Summit-Finding-Harmony).
- Neople (Dungeon & Fighter) converted Unreal material logic to a shared HLSL shader so artists preview light, AO, base color, rim, and outline in 3ds Max — [Inven Global](https://labs.invenglobal.com/articles/22927/natural-toon-shading-techniques-from-a-dungeon-fighter-technical-artist-at-neople).
- Unity Toon Shader docs describe the generic base / 1st shade / 2nd shade color-map plus shading-control-map structure — [Unity Toon Shader manual](https://docs.unity3d.com/Packages/com.unity.toonshader@0.15/manual/Basic.html).

### Inferences
- Minimum character material for a custom engine:
  - Base color, ramp or shadow-color texture (rows indexed by material ID), and lightmap/ILM (spec mask, threshold offset, spec size/threshold, material ID or inner lines).
  - Vertex colors for AO/threshold bias and outline width/Z-offset.
  - Optional face SDF threshold map plus head forward/right vectors passed as uniforms. Head bones are needed, because the face map is evaluated in head space.
  - Optional hair anisotropic or "angel ring" term, and matcap for metals.
  - All of this fits a single forward uber-shader with a few static permutations (face, hair, body, eye).
- Do not let the shadow-map term multiply the ramp lookup directly on characters. Shipped titles suppress self-shadow (Blue Protocol, Genshin) or offset it. Plan a "character shadow" path: per-character shadow maps or screen-space shadows with bias, as in StarRailNPRShader's per-object shadows.
- Lighting the character from a "virtual" light that tracks time of day but is artist-curved (WuWa, Blue Protocol) matters more for the look than physically accurate light. Expose per-character light-direction and color overrides.
- Not sourced this session (general practitioner knowledge, verify before relying on it):
  - Stocking/fabric tricks: fresnel-based opacity/denier blend.
  - Eye see-through-hair: stencil or depth-offset second pass.
  - Matcap for metallic accessories.
  - Screen-space depth-offset rim light: sample depth offset along the view-space normal, and light the pixel where the depth difference exceeds a threshold. Commonly attributed to Genshin/HSR.

### Gaps
- No official HoYoverse documentation of Genshin, HSR, or ZZZ character shader channel layouts was found. Everything is datamine-based.
- The GGXrd handout PDF could not be fetched. The ILM and vertex-color layout above is second-hand (Polycount).
- WuWa's UFSH2023 talk ([Bilibili, Wang Hongbo/Kuro](https://www.bilibili.com/video/BV1BK411v7FY/)) and Hi-Fi Rush GDC 2024 slides were not read in full. GDC Vault was in maintenance or login-gated.
- No primary source found for the screen-space depth rim light or the hair "angel ring" implementation in specific HoYo titles.

---

## A2. Outlines: inverted hull vs screen-space, line width control, how the reference games do it

### Takeaway
Shipped anime games mostly use geometry outlines (inverted hull / backface extrusion). Width comes from vertex color, with perspective/distance correction and Z-offset, and smoothed normals avoid gaps at hard edges. Interior lines are added via textures ("Motomura lines") or image-space edge detection. Blue Protocol is the notable exception: it does post-process-style outlines from an extra G-buffer and works around TAA blurring them.

### Cited Findings
- **Guilty Gear Xrd:** hull-based outline. Vertex color carries outline scale, Z-offset, and width. The hull method "only requires information about the current vertex", so width and distance scaling happen in the vertex shader. Inner lines live in a texture — [Polycount](https://polycount.com/discussion/comment/2186979); [Unity forum discussion](https://discussions.unity.com/t/toon-outline-for-selected-edges-in-vr/858814).
- **Honkai Impact 3rd (Unite Tokyo 2018)** used three outline methods — [slides](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd):
  - Geometry outline: needs "Continuous vertex normal required"; "Vertex color controls outline width"; "perspective corrected line width".
  - Enhanced geometry: crease edges detected in preprocessing and combined with the backface method.
  - Image-space: normal plus depth detection, "Constant performance", with camera-based width tweaks.
- **Blue Protocol (CEDEC 2021):** outlines drawn in a post-process-like pass using an extra G-buffer — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html):
  - Channel layout: R = thickness (adjusted for camera distance and aspect), G = inner-outline ID, B = hand-painted outline from texture, A = depth shift.
  - Edges are detected from depth, ID, normals, and G-buffer coverage.
  - A second vertex-color set is moved into TexCoord1 because UE4 holds only one.
  - Eyebrows get their own outline. Hair is split into a ToonHairPass before lighting.
  - "TemporalAA softens outlines by about one pixel", so outlines are written to stencil and ResponsiveAA is used.
- **Smoothed normals for hull outlines:** hard-edge artifacts "can only be fixed by smoothing normals". Options are recalculating normals (which changes lighting) or an asset post-processor that bakes smoothed normals into a separate channel — [Delt06 URP toon shader wiki](https://github.com/Delt06/urp-toon-shader/wiki/Outline). A commercial UE tool bakes smoothed normals into vertex color RGB in tangent, object, or world space — [Fab listing](https://fab.com/s/09ce193c9fac). StarRailNPRShader automates "smoothing normals" in import — [repo](https://github.com/stalomeow/StarRailNPRShader).
- Chinese practitioner notes: flatten back-face normals (push Z toward the camera) during extrusion to avoid outline faces poking through — [Tencent GWB notes](https://gwb.tencent.com/community/detail/121023).
- Thinner lines on the lit side and thicker in shadow is a known stylization, discussed for Blender — [BlenderNation rim tutorial thread](https://www.blendernation.com/2020/07/15/how-to-get-perfect-rim-lights-toon-shader-tutorial/).
- Hi-Fi Rush lists a "post-process outline" — [GCORES](https://www.gcores.com/articles/181912).

### Inferences
- Recommended for a custom engine: inverted-hull pass per character.
  - Separate pipeline with front-face culling and depth test on.
  - Extrude in clip space so width stays constant in pixels, then scale by a distance/FOV curve and vertex-color width.
  - Read smoothed normals from a baked attribute: tangent-space smoothed normal in a spare UV or tangent channel, computed offline by averaging normals of coincident positions.
  - Per-material outline color taken from the ramp or base color.
- Add an optional screen-space edge pass (depth plus normal plus character/material ID buffer) for environment props and interior creases. A forward+ prepass that writes normals and IDs makes this cheap.
- TAA plus thin lines: either (a) exclude outline pixels from history via a stencil or responsive mask (Blue Protocol), (b) draw outlines after TAA or upscaling, or (c) use MSAA or SMAA on the character layer. Forward rendering keeps MSAA viable.

### Gaps
- No primary source describes Genshin's, HSR's, ZZZ's, or WuWa's outline implementations (the vertex channel used for smoothed normals, or width curves). Community replications suggest inverted hull with smoothed normals in tangents or UVs, but this was not confirmed here.

---

## A3. Environment: blending stylized characters with PBR-ish worlds; GI, sky, fog, shadows, AO, water, foliage at open-world scale

### Takeaway
The modern pattern is a PBR-based (often deferred) environment pipeline with "stylized" parameter choices: clean, bright, low-noise lighting, artist-driven time of day, probe/SH ambient, and AO volumes plus capsule AO for characters. Cel-shaded characters get their own lighting rules on top. Genshin's console talk is the most detailed public reference. WuWa shows the mobile-scalable deferred variant.

### Cited Findings
**Genshin Impact (Unity Dojo 2021, Zhenzhong Yi, "From Mobile to Console")** — [slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi)
- Pipeline:
  - Separate customized console and mobile pipelines, both "based on PBR with a stylized look".
  - Over half of console features are implemented in compute.
- Lights:
  - Clustered deferred local lighting: 1024 lights in view, 64×64 px tiles × 16 depth slices.
  - Nearly 100 local lights can cast real-time shadows, with baked static shadows.
- Shadows:
  - Up to 8 shadow cascades with Poisson-disc soft shadows.
  - Cached cascades: the first 4 update every frame, the last 4 are interleaved, and each updates at least once every 8 frames.
  - Screen-space shadow with a ¼×¼-res penumbra mask cut soft-shadow cost from 2–2.6 ms to 1.3–1.7 ms.
- AO: HBAO for small-scale detail, AO Volumes for static objects, Capsule AO for characters (driven by skeleton, directional). Computed at ½ res with bilateral upsample.
- Indirect lighting:
  - Reflection probes relit, convolved, and BC6H-compressed, updated across frames.
  - Ambient stored as 3-band SH derived from the probes. Shadow SH and local-light SH added to reduce leaks. Interior masks separate indoor and outdoor ambient.
- Volumetric fog: physically based scattering with a temporal filter, under 1 ms. Voxels aligned to the light clusters. Separate ½-res god-ray pass marching up to 5 cascades.
- Reflections: Hi-Z SSR with temporal filter, about 1.5 ms on PS4 Pro. Water uses a different reflection technique.
- Art direction: "Fresh, bright, clean, anime-like" rather than "Dirty, noisy, dark". The team avoided "cool-looking technologies" that did not match the style.

**Wuthering Waves** — [GCORES interview](https://www.gcores.com/articles/181970); [UE developer interview (403 to fetch; quoted via search excerpt)](https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves)
- Deferred shading choice:
  - Chosen to support SSR and GTAO and to curb shader-variant growth.
  - UE interview: "traditional forward rendering can cause shader variants to increase sharply", while deferred "is advantageous for managing materials, weather, the shift between day and night, and atmospheric transitions".
- Mobile:
  - "One-pass deferred" saves about 1.5 GB/s of bandwidth at 60 fps. Constraints: Mali PLS has 128-bit on-chip storage, no alpha blend, and a single RT resolve.
  - SSR on Medium and GTAO on High for most mobile devices.
- Sky and weather:
  - Custom stylized time-of-day/sky (not UE's physical atmosphere), adjustable in real time without precompute.
  - Volumetric clouds replaced by a 2D skybox pipeline with drift animation.
  - Fake volumetric fog cards plus post materials. Per-scene mood/LUT grading.
- Vegetation tiers:
  - PC/console: card trees near, billboards mid, impostors far. Mobile: billboards near, impostors mid and far.
  - Billboards use about 30–40% of the card model's polygons. Billboard shadows face the light, not the camera.
  - Impostors use slice streaming plus a dynamic TextureArray.
- The UFSH2023 talk covered TOD shadows and vegetation multi-platform, mobile TAAU for anime elements, and One Pass Deferred — [Bilibili UFSH2023](https://www.bilibili.com/video/BV1BK411v7FY/).
- PS5 version said to match PC Ultra (secondhand translation of the Unreal Fest Shanghai talk) — [Push Square](https://www.pushsquare.com/news/2024/01/highly-anticipated-genshin-impact-rival-wuthering-waves-reportedly-bound-for-ps5-ps4).
- A TA speculates the PC build uses Lumen-derived GI or HW-RT back-ported into UE4 — [AUTOMATON](https://automaton-media.com/en/column/even-from-a-developers-perspective-wuthering-waves-use-of-unreal-engine-is-borderline-perverse-a-ue4-game-decked-out-in-custom-technology/).

**Honkai Impact 3rd (2018)** — [slides](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd)
- Anime-style AO based on HBAO with "HVS color shift".
- Enlighten real-time GI.
- Volumetric light using blue-noise dither plus TAA.
- Caustics faked via "inverse diffuse".
- Planar reflections at low res on mobile.

**Hi-Fi Rush** — [GCORES](https://www.gcores.com/articles/181912)
- Environment stylization: procedural SDF halftone dots for light and hatching for shadow, anti-aliased with smoothstep. Dot size scales with light intensity.
- Projection: screen-space UVs for bloom and fog; dominant-axis world projection elsewhere (triplanar rejected).
- GI: probe GI (SH), AO volume, SSAO, SSR. Shadows: screen-space shadows, baked static shadow maps, capsule shadows, decal lights.

**Blue Protocol**
- City optimization: DX12 adopted for render-thread CPU savings. Custom ISM/HISM instancing tool with reverse conversion. Halving the actor count reached 60 fps — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html).

**ZZZ**
- PC v3.0 reportedly added RT reflections, AO, and GI plus DLSS — [Tom's Hardware Italy](https://www.tomshw.it/videogioco/zenless-zone-zero-alza-lasticella-con-ray-tracing-e-dlss-45).
- Xbox version marketed with native 4K60, DXR, and HDR — [OneMoreGame](https://onemoregame.ph/2025/04/zenless-zone-zero-coming-to-xbox-june-2025/).

### Inferences
- For a Rust engine targeting 60 fps on PC/console/mobile, the minimum "anime-PBR" environment set (all achievable without RT):
  - Cascaded shadows (4 cascades on mobile, 6–8 with caching on PC) plus screen-space contact shadows.
  - Probe-based SH ambient plus relightable reflection probes. AO volumes (baked) plus capsule AO for characters plus GTAO/HBAO at half res.
  - Froxel volumetric fog aligned to light clusters.
  - Stylized artist-curve sky, or a cheap analytic sky with LUT tint, rather than a full physical atmosphere.
- Foliage at scale: GPU-instanced cards near, billboards and impostors far, with GPU-driven culling. Shadows from billboards should face the light (WuWa).
- Characters should sample the same SH probes but through a flattening function (Blue Protocol ignores normals for indirect light) so they stay graphic.

### Gaps
- No public details found on Genshin's mobile pipeline, water rendering in any of these titles, or grass rendering specifics (density, GPU instancing approach) for Genshin, ZZZ, or WuWa.

---

## A4. Post-processing: bloom, tonemapping, grading, DoF, AA/upscalers, ZZZ's comic look

### Takeaway
Bloom is multi-level (mip-chain) and strong. Tonemapping is usually custom: Genshin explicitly rejected ACES and folds a custom curve plus hue-shift into a LUT. Per-scene or per-volume LUT grading is standard. TAA must be tamed for thin outlines (responsive masks or stencil). Upscalers (TAAU on mobile, DLSS on PC) are now used in these games.

### Cited Findings
- **Honkai 3rd bloom** — [slides](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd):
  - Bloom at 1/4, 1/8, 1/16, and 1/32 sizes, composed with the original image, then distortion applied after bloom.
  - Layered DoF with hexagonal bokeh. Image-space lens glare and star streaks.
  - Full-screen distortion with depth-clip mask.
- **HSR fan replication:** custom bloom credited to Jack He's Unite 2018 technique, plus custom ACES tonemapping — [ecosyste.ms](https://awesome.ecosyste.ms/projects/1135012); [StarRailNPRShader](https://github.com/stalomeow/StarRailNPRShader).
- **Genshin tonemapping and HDR** — [Unity Dojo slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi):
  - HDR10 on PS4 from v1.2. ACES (RRT+ODT) not used because it "did not fit the game's style".
  - Hue shift simulated in shader. Tone curve folded into the LUT. WCG LUT pass under 0.05 ms.
- **Blue Protocol:** final-pass "diffusion-style glow" common in anime. TAA blur on outlines handled with stencil plus ResponsiveAA — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html).
- **WuWa:** per-scene mood/LUT grading. Mobile TAAU adapted to anime rendering needs — [GCORES](https://www.gcores.com/articles/181970); [UFSH2023](https://www.bilibili.com/video/BV1BK411v7FY/).
- **Hi-Fi Rush:** post-processing per volume rather than per camera. The shader tests G-buffer/depth against volume bounds so screen regions get different grading — [GCORES](https://www.gcores.com/articles/181912).
- **ZZZ:** at the end of each battle "a freeze frame slathers the screen in yellow while turning characters monochrome" — [Inverse review](https://inverse.com/gaming/is-zenless-zone-zero-hoyoverse-good-or-bad). DLSS added on PC — [Tom's Hardware Italy](https://www.tomshw.it/videogioco/zenless-zone-zero-alza-lasticella-con-ray-tracing-e-dlss-45).
- **Reference tonemapper menu** (Bevy 0.20 docs): None, Linear, Reinhard, ReinhardLuminance, AcesFitted, AgX, SomewhatBoringDisplayTransform, TonyMcMapface (default), BlenderFilmic, KhronosPbrNeutral. Each has a hue-shift and saturation characterization — [docs.rs Tonemapping](https://docs.rs/bevy/latest/bevy/core_pipeline/tonemapping/enum.Tonemapping.html).
- **Rust upscaler bindings:**
  - DLSS: `dlss_wgpu` 6.0.0 (2026-09-16), maintained by bevyengine. Supports DLSS-RR 4.5 per Bevy 0.20 — [crates.io](https://crates.io/crates/dlss_wgpu); [Bevy 0.20](https://bevy.org/news/bevy-0-20/).
  - FSR: `fsr` 0.1.11 (FSR2 bindings, EmbarkStudios) last released 2024-04-07, so stale — [crates.io](https://crates.io/crates/fsr).
  - MetalFX: raw bindings `objc2-metal-fx` 0.3.2 (2025-10-04) — [crates.io](https://crates.io/crates/objc2-metal-fx).
  - XeSS: no crate found.

### Inferences
- Pipeline suggestion:
  - HDR scene → (optional) TAA/upscale with a responsive mask from character/outline stencil.
  - Multi-mip bloom (compute downsample/upsample, threshold per material via an emissive/bloom-mask channel; anime looks often use colored, high-intensity bloom on characters' emissive and sky).
  - Custom tonemap curve baked into a 3D LUT along with grading (Genshin approach). Per-region or per-volume LUT blending (Hi-Fi Rush, WuWa).
  - Screen-space "impact" effects: radial blur, chromatic aberration, monochrome/tint flashes à la ZZZ.
  - Outlines composited after AA if TAA is used.
- Treat ZZZ-style comic effects (halftone, color-key freeze frames) as post-process materials driven by gameplay events. Hi-Fi Rush's SDF halftone technique is a documented template.

### Gaps
- No primary source on ZZZ's post stack (halftone, comic panels, urban grading) or Genshin's bloom specifics.
- No source this session for the GT (Uchimura) tonemapper or AgX adoption in any of the named games.
- FSR3/FSR4 and XeSS Rust bindings: none found.

---

## A5. VFX for action combat: particles, trails, distortion, flipbooks, impact frames

### Takeaway
Public material on combat VFX in these games is thin. The confirmed pieces are distortion composited after bloom (Honkai 3rd), a unified material controller plus Niagara for combat effects (WuWa), ZZZ's full-screen freeze-frame recolor, and GPU particles as the default modern approach.

### Cited Findings
- Honkai 3rd: full-screen distortion with depth clipping mask and distance-based intensity. Bloom combined before distortion. Mobile has "dynamic particles, distortion" — [slides](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd).
- WuWa: a unified material controller plus Niagara manage combat-loop effects. Echo effects are mostly gold-toned with spiral absorption visuals. Kuro wanted better GPU and large-scale particles (Niagara upgrades) — [GCORES](https://www.gcores.com/articles/181970).
- ZZZ: end-of-battle yellow freeze frame with monochrome characters — [Inverse](https://inverse.com/gaming/is-zenless-zone-zero-hoyoverse-good-or-bad).
- Rust GPU particle reference: `bevy_hanabi` 0.19.0 (2026-06-27, repo active 2026-10-15) — [crates.io](https://crates.io/crates/bevy_hanabi).

### Inferences
- Needed systems:
  - Compute-shader GPU particles: spawn, update, sort for alpha, indirect draw.
  - Ribbon/trail meshes generated on CPU or GPU from weapon sockets.
  - Flipbook sampling with motion-vector blending.
  - A distortion buffer (screen-space UV offsets) composited after bloom.
  - Hit-stop (time-scale freeze) plus a global "impact frame" post material (inverted or limited palette, radial lines).
  - Anime VFX is mostly unlit, additive or alpha with hand-painted textures. Throughput (overdraw, sort) matters more than lighting. Use half-res particle buffers on mobile.

### Gaps
- No talks found on HoYoverse or Kuro combat VFX pipelines, or on Punishing: Gray Raven rendering. No Rust trail/ribbon crate was researched.

---

## A6. Index of public talks and reverse-engineering breakdowns

### Takeaway
The best primary sources:
- GGXrd GDC 2015
- Honkai 3rd Unite Tokyo 2018
- Genshin Unity Dojo 2021 (console)
- Blue Protocol CEDEC 2020/2021
- Hi-Fi Rush GDC 2024
- WuWa UFSH2023 plus team interviews
- NetEase GDC 2022

Community replications are useful for Genshin and HSR channel layouts.

### Cited Findings
- GGXrd GDC 2015 — [GDC Vault](https://gdcvault.com/play/1022031/GuiltyGearXrd-s-Art-Style-The); [ArcSys page with video/handout](https://www.arcsystemworks.com/guilty-gear-xrd-art-talk-at-game-developers-conference-2015/).
- Honkai Impact 3rd, Unite Tokyo 2018 (Jack He) — [docswell slides](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd).
- Genshin "From Mobile to Console" (Unity Dojo 2021, Zhenzhong Yi) — [docswell slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi).
- Blue Protocol CEDEC 2020 — [CGWORLD](https://cgworld.jp/feature/202011-cedec2020-bluep.html). CEDEC 2021 — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html), [Famitsu](https://www.famitsu.com/news/202108/25231530.html).
- Hi-Fi Rush GDC 2024 — [80.lv](https://80.lv/articles/the-making-of-hi-fi-rush-s-3d-toon-rendering-style); [GCORES part 1](https://www.gcores.com/articles/181912).
- WuWa UFSH2023 (Wang Hongbo, Kuro) — [Bilibili](https://www.bilibili.com/video/BV1BK411v7FY/); interviews — [GCORES](https://www.gcores.com/articles/181970), [UE developer interview](https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves).
- NetEase GDC 2022 anime + PBR (deferred) — [GDC Vault](https://gdcvault.com/play/1027597/Technical-Artist-Summit-Finding-Harmony).
- Community:
  - [HoyoToon](https://github.com/Melioli/HoyoToon/wiki/Using-the-Genshin-Shader): Unity BiRP, covers Genshin, HI3, HSR, ZZZ, and WuWa.
  - [StarRailNPRShader](https://github.com/stalomeow/StarRailNPRShader) (archived 2025-03).
  - [URPSimpleGenshinShaders](https://github.com/NoiRC256/URPSimpleGenshinShaders).
  - [Blender Genshin recreation](https://bjayers.artstation.com/blog/category/5).
- Related open-world rendering talks (non-anime) useful for architecture:
  - Assassin's Creed Shadows, GDC 2025 (GPU-driven, lighting) — [GDC Vault](https://gdcvault.com/play/1035526/Rendering-Assassin-s-Creed-Shadows).
  - Hogwarts Legacy, GDC 2024 (UE open world, hundreds of shadow-casting lights) — [GDC schedule](https://schedule.gdconf.com/session/open-world-rendering-techniques-in-hogwarts-legacy/902654).

### Inferences
- When building, read the Genshin console deck (environment) and Blue Protocol CEDEC 2021 (cel shading in deferred plus outline G-buffer) end to end. Together they cover most decisions.

### Gaps
- No ZZZ, HSR, or Punishing: Gray Raven official rendering talk was found (searched GDC 2025/2026, Unite, Chinese-language sources). GDC Vault was in maintenance during this session.

---

## A7. Renderer architecture fit: forward+/clustered vs deferred, visibility buffer, GPU-driven, render graph, bindless, permutations/PSO caching

### Takeaway
Shipped anime titles are split. UE4-based ones (WuWa, Blue Protocol, Hi-Fi Rush, NetEase) are deferred with cel-shading logic hacked into BasePass and extra G-buffers. Genshin console uses clustered deferred lighting for local lights. HSR-style Unity replications use forward/forward+. For a custom engine, a clustered forward+ renderer with a thin prepass (depth, normal, IDs, velocity) is the best fit for anime characters (MSAA, material variety, transparency, many ramp/ID lookups that don't fit a G-buffer). It needs disciplined permutation management to avoid the variant explosion that pushed Kuro to deferred. Add GPU-driven culling and multi-draw indirect for open-world environment density, and a render graph for transient resources and async-compute scheduling.

### Cited Findings
- Kuro picked deferred because forward "can cause shader variants to increase sharply" and deferred eases material, weather, day-night, and atmosphere transitions. Deferred was also chosen for SSR and GTAO — [UE interview (search excerpt)](https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves); [GCORES](https://www.gcores.com/articles/181970).
- Blue Protocol: in UE4 deferred, cel shading needed engine edits. N·L was moved into BasePass due to G-buffer capacity, and an extra G-buffer was added for outlines — [GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html).
- Genshin console: clustered deferred lighting for local lights (64×64 tiles × 16 slices, 1024 lights) — [Unity Dojo slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi).
- HSR replication uses Forward and Forward+ paths, with MRT for characters — [StarRailNPRShader](https://github.com/stalomeow/StarRailNPRShader); [ecosyste.ms](https://awesome.ecosyste.ms/projects/1135012).
- GPU-driven rendering (Haar & Aaltonen, SIGGRAPH 2015): the GPU decides what to draw via mesh-cluster culling and occlusion depth, with a single "draw scene" call across viewports — [slides PDF](https://advances.realtimerendering.com/s2015/aaltonenhaar_siggraph2015_combined_final_footer_220dpi.pdf). AC Shadows (GDC 2025) focuses on GPU-driven pipelines for open world — [GDC Vault](https://gdcvault.com/play/1035526/Rendering-Assassin-s-Creed-Shadows).
- Frame graph (Frostbite, GDC 2017, Yuriy O'Donnell): a graph of passes and resources plus a transient resource system. Benefits: extensibility, simpler async compute, automatic memory aliasing, and lower memory use — [slides](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite).
- wgpu status relevant to these choices — [wgpu CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md):
  - `multi_draw_indirect` is available on all devices with `DownlevelFlags::INDIRECT_EXECUTION`; the feature flag was removed in v27. `MULTI_DRAW_INDIRECT_COUNT` is a separate feature that cannot be emulated.
  - `PipelineCache` (Vulkan pipeline caches) has existed since v22.
- Bevy 0.19 replaced the `RenderGraph` node API with render passes as ECS systems in schedules (`Core3d`, `Core2d`) — [Bevy 0.19](https://bevy.org/news/bevy-0-19/).

### Inferences
- **Proposed frame (60 fps, PC/console/mobile):**
  1. GPU culling: compute frustum and Hi-Z occlusion, writing indirect args.
  2. Depth + normal + material/character ID + velocity prepass.
  3. Async-able compute: light clustering, GTAO, contact shadows, SSR.
  4. Shadow cascades (cached).
  5. Clustered forward opaque. One environment PBR-ish uber-shader and one anime character uber-shader. Material ID selects ramp rows; static permutations limited to roughly ≤8 per shader family.
  6. Inverted-hull outlines.
  7. Hair, eyes, and transparents (stencil tricks).
  8. Volumetric fog apply.
  9. Particles and distortion.
  10. TAA/upscale with responsive mask.
  11. Bloom.
  12. LUT tonemap and grade.
  13. UI.
- **Visibility buffer / mesh shaders:** defer these. Mesh shaders in wgpu are full on Vulkan and complete on DX12 only as of v30 (see B1). On Metal they were passthrough-only in v28, and mobile support is limited. Anime scenes are rarely triangle-bound the way Nanite-style scenes are.
- **Mobile:** forward+ is bandwidth-friendly on tilers. Kuro instead built "one-pass deferred" with PLS for mobile. With wgpu you cannot access PLS or framebuffer fetch directly, which favors forward on mobile.
- **Stutter avoidance:**
  - Precompute the full pipeline set at load: materials × vertex layouts × pass types. Keep character-shader permutations few by using dynamic branching on uniform material flags.
  - Use wgpu `PipelineCache` on Vulkan/Android and persist it to disk.
  - Never create pipelines lazily mid-combat.
- **Bindless:** use texture binding arrays for material textures where supported (Vulkan, DX12, Metal) with a fallback bind-group path. Buffer binding arrays on Metal arrived only in wgpu v30 (see B1).

### Gaps
- No public numbers comparing forward+ vs deferred for anime character shading specifically. No source on Metal binary-archive PSO caching through wgpu (the `PipelineCache` docs reference Vulkan). No source verifying wgpu mesh-shader status on Metal after v30.

---

## B1. GPU abstraction layer: wgpu vs ash vs vulkano vs Metal bindings vs D3D12 vs blade et al.; platforms, consoles, shipped users

### Takeaway
wgpu (v30.0.1, 2026-08-22) is the only actively developed, multi-backend Rust GPU API that covers Vulkan, Metal, DX12, GLES, and WebGPU. Since 2025–26 it has had mesh shaders, ray queries plus experimental RT pipelines, Metal acceleration structures, broader bindless, the D3D12 Agility SDK, HDR surfaces, and a custom-backend hook. Bevy 0.20 and Zed use it. ash is still the raw-Vulkan option: the repo is active with Vulkan 1.4.352 headers, but the last crates.io release was 0.38.0 in April 2024. Mac Vulkan goes through MoltenVK 1.4.2. metal-rs is deprecated in favor of objc2-metal. blade is a smaller, active alternative (Vulkan, Metal, GLES; RT on Vulkan only). There is no public console backend for any Rust GPU crate.

### Cited Findings
**wgpu** — [releases](https://github.com/gfx-rs/wgpu/releases); [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md); [crates.io](https://crates.io/crates/wgpu)
- Latest 30.0.1 (2026-08-21/22). Previous releases: 30.0.0 (2026-07-01), 29.0.0 (2026-03-18), 28.0.0 (2025-12-17), 27.0.0 (2025-10-01). Major versions are roughly quarterly. naga shares the version number (30.0.1). About 14.7M recent downloads.
- v28 "Mesh Shaders, Immediates, and More!":
  - Mesh shaders "fully supported on Vulkan, and supported on Metal and DX12 with passthrough shaders".
  - Push constants replaced by WebGPU-style "immediates".
  - Vulkan backend moved from gpu-alloc to gpu-allocator.
  - Source: [v28.0.0 release](https://github.com/gfx-rs/wgpu/releases/tag/v28.0.0).
- v29 (2026-03-18):
  - Metal backend switched to autogenerated `objc2` bindings ("should resolve a lot of leaks and unsoundness").
  - Ray-tracing acceleration structures on Metal. TLAS binding arrays. Initial WGSL ray-tracing pipelines.
  - D3D12 Agility SDK loading. `PASSTHROUGH_SHADERS` no longer experimental, with metallib passthrough added.
  - MSRV 1.87.
  - Source: [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md); [releases](https://github.com/gfx-rs/wgpu/releases).
- v30 (2026-07-01):
  - "Full support for mesh shaders, including in WGSL"; the naga HLSL writer completes DX12 mesh-shader support.
  - Bindless storage buffers (buffer binding arrays) on Metal. `enable wgpu_binding_array;`.
  - SPIR-V-out ray-tracing pipelines. BLAS with procedural AABBs.
  - i16/u16 in WGSL. HDR/wide-gamut surface color spaces.
  - `as_custom` expanded "increasing the capabilities of custom backends".
  - New `naga-types` crate. A `STRICT_WEBGPU_COMPLIANCE` instance flag.
  - Combined Metal buffer-argument-table limit of 29.
  - Source: [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- Earlier bindless work:
  - v25 (2025-04-10): Metal bindless limits went from 32–128 to 500,000 sampled textures. Binding arrays in a bind group forbid dynamic-offset or uniform buffers in that group (Vulkan UpdateAfterBind rule).
  - v24: DX12 `PARTIALLY_BOUND_BINDING_ARRAY` on Tier 3.
  - Source: [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- Unreleased trunk adds a `default_queue` descriptor and texture component swizzle — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- Experimental ray tracing is gated behind `EXPERIMENTAL_RAY_TRACING_PIPELINES`. The ray-query feature was merged with acceleration structures in v27 — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- Built-in backends: DX12, Metal, Vulkan, GL/GLES, and WebGPU. A `custom` module provides "wrappers [for] custom backend implementations" — [wgpu backend docs](https://wgpu.rs/doc/wgpu/backend/index.html); [docs.rs](https://docs.rs/wgpu).
- **Consoles:** no public console backend found. Console SDKs are NDA-bound. Engines typically keep their own abstraction above platform APIs — [GameDev.net comment](https://gamedev.net/forums/topic/707692-question-about-gaming-console-operating-systems/). The Aurora project (Dawn/WebGPU-based) lists a planned Switch deko3d backend, as a plan only — [Aurora](https://git.axiodl.com/encounter/aurora/src/commit/b91c2739c9bff672355daf3bf6daa4d8813a52ba).
- **Overhead vs raw Vulkan:**
  - No current rigorous benchmark found. A 2021 anecdote estimates CPU overhead of about 100% if naive, 10–20% if optimized for draw-heavy work, and near zero for GPU-bound work. The extra cost is attributed to wgpu's validation and tracking — [r/rust_gamedev (2021)](https://redlib.hbubli.cc/r/rust_gamedev/comments/p2ihhz/wgpu_vs_vulkan).
  - A 2026 arXiv paper measured WebGPU per-dispatch cost of 24–36 µs (Vulkan) and 31–71 µs (Metal) for batch-1 LLM workloads. This is not a direct game-rendering comparison — [arXiv 2604.02344](https://arxiv.org/pdf/2604.02344).
- **Users:**
  - Bevy 0.20's `bevy_render` depends on `wgpu ^30` — [crates.io deps](https://crates.io/crates/bevy_render).
  - Zed's workspace now has a `gpui_wgpu` crate and pins wgpu to a git rev, with the note "30.0.1 lacks the Vulkan surface-drop panic fix". No blade dependency remains in its root Cargo.toml — [Zed Cargo.toml](https://github.com/zed-industries/zed/blob/main/Cargo.toml).

**ash (raw Vulkan)**
- crates.io latest 0.38.0+1.3.281 (2024-04-01) — [crates.io](https://crates.io/crates/ash).
- The repo is active (commit 2026-10-23). Vulkan-Headers updated to 1.4.352 on 2026-05-20, and new extensions were added (e.g., `VK_NV_partitioned_acceleration_structure`, opacity micromap). Using these means a git dependency — [ash commits](https://github.com/ash-rs/ash/commits/master).
- `ash-window` 0.13.0 (2024-04-01). `ash-molten` 0.20.0+1.2.11 (2024-09-12, stale; bundles an old MoltenVK) — [crates.io](https://crates.io/crates/ash-molten).
- `vulkanalia` 0.35.0 (2026-02-15) is an alternative Vulkan binding with a tutorial — [crates.io](https://crates.io/crates/vulkanalia).

**MoltenVK (Vulkan on Apple)**
- v1.4.2 (2026-07-24). Minimum macOS 12 / iOS 15. Added gl_DrawID/DrawIndex on macOS and min/max sampler filtering. Fixed an argument-buffer device-loss issue — [MoltenVK releases](https://github.com/KhronosGroup/MoltenVK/releases).

**vulkano (safe Vulkan wrapper)**
- 0.35.2 (2025-08-16). Repo active (push 2026-09-28) — [crates.io](https://crates.io/crates/vulkano); [GitHub](https://github.com/vulkano-rs/vulkano).

**Apple Metal**
- `metal` (metal-rs) 0.33.0 (2025-12-17). The repo describes itself as "Deprecated Rust bindings for Metal" — [GitHub](https://github.com/gfx-rs/metal-rs).
- `objc2-metal` 0.3.2 (2025-10-04) is the successor, from the objc2 project (active 2026-10-17). wgpu itself moved to objc2 in v29 — [crates.io](https://crates.io/crates/objc2-metal); [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).

**D3D12**
- `windows` (windows-rs) 0.62.2 (2025-10-06). Repo very active — [crates.io](https://crates.io/crates/windows).

**blade (kvark)**
- `blade-graphics` 0.9.0 (2026-09-06). Repo active 2026-10-23 — [crates.io](https://crates.io/crates/blade-graphics).
- Backends: Vulkan (Linux, Windows, Android), Metal (macOS, iOS), GLES3 (web). Ray tracing on Vulkan only. Requires timeline semaphores and dynamic rendering — [blade-graphics README](https://github.com/kvark/blade/blob/main/blade-graphics/README.md).

**Dead or inactive**
- `gfx-hal` 0.9.0 (2021-06-19) is in maintenance mode, last push 2023 — [GitHub](https://github.com/gfx-rs/gfx).
- `sierra` 0.6.0 (2023-01-09), last push 2024-06 — [crates.io](https://crates.io/crates/sierra).
- `rafx` 0.0.16 (2024-03-02), last push 2024-08 — [crates.io](https://crates.io/crates/rafx).

**Tiny Glade's stack: unresolved, sources conflict**
- Reddit commenters disagree: one says "straight up Vulkan, through ash", and another recalls the devs said wgpu "was missing a few important features" — [r/rust_gamedev 2025](https://redlib.hbubli.cc/r/rust_gamedev/comments/1izyj0l/rust_graphics_libraries).
- A 2026 secondary article claims Bevy plus wgpu, without a primary citation — [foro3d](https://foro3d.com/en/2026/mayo/rust-y-bevy-en-tiny-glade-construccion-procedural-sin-cuadricula.html).

**Traverse Research (Breda framework)**
- All code is in Rust. Builds the "Breda framework". Maintains gpu-allocator (Vulkan, DX12, Metal) and hassle-rs — [Rust Foundation spotlight](https://rustfoundation.org/media/member-spotlight-traverse-research/).

### Inferences
- For a macOS developer shipping PC, mobile, and later console, wgpu gives native Metal (no MoltenVK translation), iOS/Android, and DX12/Vulkan from one codebase. Its gaps are no exposed multi-queue/async compute (only a `default_queue` descriptor is appearing in trunk), no PLS or framebuffer fetch, validation overhead, and quarterly breaking releases.
- Mitigations:
  - Write a thin engine-side RHI trait mirroring wgpu's concepts, so a future NDA console backend can be added.
  - Use wgpu's `custom` backend or your own trait for consoles.
  - Use `as_hal` / passthrough to reach native features when needed.
- Raw ash makes sense only if you accept Vulkan-only (with MoltenVK on Apple) plus your own DX12/Metal later, a git-pinned dependency, and far more code. For a small team this is likely a net loss versus wgpu.
- blade is attractive for minimalism, but it has a smaller user base, no DX12, and RT on Vulkan only. Zed, a flagship user, appears to have moved to wgpu.

### Gaps
- No primary source on Tiny Glade's renderer API.
- No current wgpu vs ash CPU-overhead benchmark for game workloads.
- No public statement of any Rust game shipping on PlayStation, Xbox, or Switch via wgpu.
- Unverified whether wgpu exposes async compute or multiple queues in 30.x (only the trunk `default_queue` descriptor was seen).

---

## B2. Shader languages and toolchains: WGSL/naga, naga_oil/WESL, Slang, HLSL/DXC, GLSL/shaderc, rust-gpu, cross-compilation, hot reload

### Takeaway
For wgpu, the native path is WGSL plus WESL (imports and conditional compilation). Bevy switched all internal shaders to WESL in 0.20, and naga_oil has been archived. Slang (v2026.20, 2026-10-17) is the strongest "modern HLSL-like" option, with SPIR-V, DXIL, MSL, and WGSL targets. However, its Metal and WGSL targets are "work in progress" and its Rust bindings are immature (`shader-slang` 0.1.0, July 2025). Feed wgpu Slang's SPIR-V or MSL through passthrough or naga. rust-gpu is alive under community stewardship (0.11.0, 2026-10-13) but self-described "not yet production-ready" and SPIR-V-only.

### Cited Findings
- **naga** 30.0.1 (2026-08-22). Front ends: WGSL, SPIR-V, GLSL. Back ends include SPIR-V, MSL, HLSL, WGSL. Mesh shader and RT pipeline support in WGSL-in, HLSL-out, and SPIR-V-out per v28–v30. It now reads signed ViewIndex as glslang and Slang emit it — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md); [crates.io](https://crates.io/crates/naga).
- **WESL:**
  - A "strict superset of WGSL" adding imports, conditional compilation, and packages. Current edition `2026_pre` — [WESL spec](https://wesl-lang.dev/spec/README).
  - `wesl` crate 0.6.0 (2026-10-04), repo active 2026-10-19 — [crates.io](https://crates.io/crates/wesl); [GitHub](https://github.com/webgpu-tools/wesl-rs).
  - Roadmap: 2026 focus on testing, docs, and IDE tools. Planned generics and visibility control — [Khronos Shader Languages Symposium talk](https://www.khronos.org/developers/linkto/wesl-a-pioneer-language-for-webgpu).
  - Bevy 0.20: "Bevy's shaders are now written in WESL"; the old custom WGSL dialect was removed. wgsl-analyzer gives editor support — [Bevy 0.20](https://bevy.org/news/bevy-0-20/).
- **naga_oil** 0.23.0 (2026-07-14). Repo archived (status checked 2026-10-24) — [GitHub](https://github.com/bevyengine/naga_oil).
- **Slang:**
  - Latest v2026.20 (2026-10-17), monthly-ish cadence — [releases](https://github.com/shader-slang/slang/releases).
  - Targets: DXBC (via fxc), DXIL, SPIR-V, GLSL (limited), Metal MSL ("work in progress"; RT on Metal WIP; "Arrays of buffers are not supported"; shared binding slots across stages), WGSL ("work in progress"), CUDA/PTX, CPU — [Slang targets guide](https://shader-slang.org/slang/user-guide/targets.html).
  - Rust bindings: `shader-slang` 0.1.0 plus `shader-slang-sys` 0.1.0 (2025-07-23). Repo FloatyMonkey/slang-rs last push 2026-05-20 — [crates.io](https://crates.io/crates/shader-slang); [GitHub](https://github.com/FloatyMonkey/slang-rs).
  - Gotcha: the crates.io crate named `slang-rs` (0.24.0) is a Verilog parser, unrelated — [crates.io](https://crates.io/crates/slang-rs).
  - `slang-hal` 0.4.0 (dimforge, scientific compute) — [crates.io](https://crates.io/crates/slang-hal).
- **HLSL/DXC:** `hassle-rs` 0.12.0 (2025-06-13), repo active 2026-10-14 (Traverse Research) — [crates.io](https://crates.io/crates/hassle-rs). wgpu DX12 can force a shader model for passthrough "without bundling DXC" — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- **GLSL:** `shaderc` 0.10.1 (2025-09-06), repo last push 2025-09 — [crates.io](https://crates.io/crates/shaderc). `inline-spirv` 0.2.1 (2024-03) — [crates.io](https://crates.io/crates/inline-spirv).
- **rust-gpu:**
  - Embark's repo was archived. The community repo Rust-GPU/rust-gpu is active (push 2026-10-16) — [EmbarkStudios/rust-gpu](https://github.com/EmbarkStudios/rust-gpu); [Rust-GPU/rust-gpu](https://github.com/Rust-GPU/rust-gpu).
  - `spirv-builder`, `spirv-std`, and `cargo-gpu` 0.11.0 were all released 2026-10-13 — [crates.io spirv-builder](https://crates.io/crates/spirv-builder); [cargo-gpu](https://crates.io/crates/cargo-gpu).
  - README: "still heavily in development … not yet production-ready". SPIR-V is the only target, with DXIL or WGSL possible in future — [GitHub](https://github.com/Rust-GPU/rust-gpu).
- **Cross-compilation and reflection:**
  - `spirv-cross2` 0.7.2 (2026-10-11) — [crates.io](https://crates.io/crates/spirv-cross2).
  - `rspirv` 0.13.0 (2026-04-10) — [crates.io](https://crates.io/crates/rspirv).
  - `spirq` 1.3.0 (2026-08-04, reflection) — [crates.io](https://crates.io/crates/spirq).
  - `spirv-reflect` 0.2.3 (2019, stale) — [crates.io](https://crates.io/crates/spirv-reflect).
- **Typed bindings for WGSL:** `wgsl_bindgen` 0.23.3 (2026-08-11) — [crates.io](https://crates.io/crates/wgsl_bindgen). `encase` 0.13.0 (2026-09-19) for WGSL layout — [crates.io](https://crates.io/crates/encase).
- **Hot reload:** `notify` 8.2.0 (2026-10-17) — [crates.io](https://crates.io/crates/notify). `hotwatch` 0.5.0 (2023) — [crates.io](https://crates.io/crates/hotwatch).

### Inferences
- **Recommended:** author in WGSL plus WESL (`wesl` crate) for everything wgpu runs natively, including mobile and web. Use naga for validation and reflection. Use `notify` to watch files, recompile modules, and rebuild affected pipelines asynchronously.
- If you want generics, interfaces, and HLSL familiarity, use Slang offline. Compile to SPIR-V for Vulkan, MSL or metallib for Metal, and DXIL for DX12, then load through wgpu passthrough (`PASSTHROUGH_SHADERS`). Accept Slang's Metal WIP caveats, and consider calling `slangc` as a CLI rather than relying on immature bindings.
- rust-gpu is not recommended as the primary shader language for a production anime game in 2026. It has no direct Metal or DXIL target and is self-described as pre-production.

### Gaps
- No source on whether wgpu has a first-party Slang integration or plans (none was found in the changelog except SPIR-V input compatibility).
- No benchmark of WESL compile times.

---

## B3. Supporting crates: allocators, render graph, bindless helpers, textures, UI, profiling, misc (versions and maintenance)

### Takeaway
The supporting ecosystem is healthy for wgpu users (egui-wgpu, wgpu-profiler, ktx2, ddsfile, ctt, meshopt, glam, encase). For raw Vulkan, gpu-allocator and vk-mem are maintained. No mature standalone render-graph crate exists, so write your own. Basis Universal bindings are stale.

### Cited Findings
(All from crates.io / GitHub API, 2026-10-24.)
- **Memory:**
  - `gpu-allocator` 0.28.0 (2025-09-26), Vulkan, DX12, Metal; repo active 2026-10-19; now used by wgpu's Vulkan backend — [crates.io](https://crates.io/crates/gpu-allocator); [wgpu v28](https://github.com/gfx-rs/wgpu/releases/tag/v28.0.0).
  - `vk-mem` (VMA) 0.5.0 (2025-08-12), repo push 2026-07 — [crates.io](https://crates.io/crates/vk-mem).
  - `gpu-alloc` 0.6.2 (2026-06-29) — [crates.io](https://crates.io/crates/gpu-alloc).
  - `gpu-descriptor` 0.3.2 (2025-05-23) — [crates.io](https://crates.io/crates/gpu-descriptor).
  - `presser` 0.3.1 (2022, Embark) — [crates.io](https://crates.io/crates/presser).
- **Render graph:**
  - No maintained general crate found. `render-graph` on crates.io is a 2018 name reservation — [crates.io](https://crates.io/crates/render-graph).
  - `screen-13` 0.13.0 (2025-12-14) is a Vulkan engine built around a render graph — [crates.io](https://crates.io/crates/screen-13).
  - Bevy moved render passes to ECS schedules in 0.19 — [Bevy 0.19](https://bevy.org/news/bevy-0-19/).
- **Textures:**
  - `ktx2` 0.5.0 (2026-04-19) — [crates.io](https://crates.io/crates/ktx2).
  - `basis-universal` 0.3.1 (2023-11-02), repo last push 2024-04, so stale — [crates.io](https://crates.io/crates/basis-universal).
  - `image` 0.25.10 (2026-03-10) — [crates.io](https://crates.io/crates/image).
  - `ddsfile` 0.6.0 (2026-04-04) — [crates.io](https://crates.io/crates/ddsfile).
  - `ctt` 0.6.0 (2026-09-27): "Compress images to GPU texture formats". Bevy 0.20 uses it for BCn (desktop) and ASTC (mobile) with mip generation — [crates.io](https://crates.io/crates/ctt); [Bevy 0.20](https://bevy.org/news/bevy-0-20/).
- **Geometry:**
  - `meshopt` 0.7.0 (2026-10-17) — [crates.io](https://crates.io/crates/meshopt).
  - `gltf` 1.4.1 (2024-05-10) — [crates.io](https://crates.io/crates/gltf).
  - `glam` 0.34.1 (2026-10-06) — [crates.io](https://crates.io/crates/glam).
  - `bytemuck` 1.25.2 — [crates.io](https://crates.io/crates/bytemuck).
- **UI and debug:**
  - `egui`/`egui-wgpu` 0.37.0 (2026-10-15) — [crates.io](https://crates.io/crates/egui-wgpu).
  - `egui-ash-renderer` 0.13.0 (2026-08-12) — [crates.io](https://crates.io/crates/egui-ash-renderer).
  - `glyphon` 0.12.0 (wgpu text, 2026-07-09) — [crates.io](https://crates.io/crates/glyphon).
- **Profiling:**
  - `wgpu-profiler` 0.28.0 (2026-07-31) — [crates.io](https://crates.io/crates/wgpu-profiler).
  - `tracy-client` 0.19.0 (2026-08-25) — [crates.io](https://crates.io/crates/tracy-client).
  - `puffin` 0.20.0 (2026-03-18) — [crates.io](https://crates.io/crates/puffin).
  - `renderdoc` 0.12.1 (2024-03-02, stale but stable API) — [crates.io](https://crates.io/crates/renderdoc).
- **Upscalers:** `dlss_wgpu` 6.0.0. `fsr` 0.1.11 (2024, stale). `objc2-metal-fx` 0.3.2 — see A4.

### Inferences
- Build a small in-house render graph (passes declare reads and writes, plus transient textures with aliasing and barrier generation). wgpu handles barriers itself, so on wgpu the graph is mostly about pass ordering, culling, and transient pooling.
- Prefer KTX2 with BC7/BC5/BC6H on desktop and ASTC on mobile, produced offline by `ctt` (or the Basis Universal CLI rather than the stale crate).

### Gaps
- No dedicated, maintained debug-draw crate for wgpu was identified in this session. A bindless helper crate was not researched.

---

## B4. Existing Rust renderers and NPR crates to study

### Takeaway
Bevy 0.20 is the most relevant living reference: wgpu 30, WESL, Solari, initial mesh shaders, partial bindless, contact shadows, SSR, and render passes as systems. kajiya and rend3 are archived. Fyrox 1.0 and three-d are OpenGL/glow-based, so they are less relevant to a modern GPU-driven design. Rust NPR crates are thin: `bevy_mod_outline` is the only actively maintained one.

### Cited Findings
- **Bevy 0.20** (2026-10-08) — [Bevy 0.20](https://bevy.org/news/bevy-0-20/); [crates.io](https://crates.io/crates/bevy):
  - Solari ReSTIR improvements (ReSTIR now optional and off by default). Retained scene management. Supports Atmosphere, EnvironmentMapLight, DirectionalLight, and emissive meshes. Runs on macOS/Metal "but has no built-in denoiser there".
  - DLSS-RR 4.5 via dlss_wgpu.
  - Initial mesh shaders via `draw_mesh_tasks` (not on web).
  - Shaders in WESL. 2D sprite materials. `ctt` texture compression.
- **Bevy 0.19** (2026-06-19) — [Bevy 0.19](https://bevy.org/news/bevy-0-19/):
  - GPU light clustering, batched depth prepasses, and GPU visibility-range checks.
  - Contact shadows, physically based SSR, LTC rect area lights, vignette and lens distortion, parallax-corrected cubemaps.
  - Partial bindless: "Metal supports texture binding arrays but not buffer binding arrays" (before wgpu v30).
  - `RenderErrorHandler` for device loss. Render passes as ECS systems.
- **Bevy 0.18** (2026-01-13): Solari got many new features — [Bevy 0.18](https://bevy.org/news/bevy-0-18/).
- **kajiya** (real-time GI): Embark repo archived. The h3r2tic fork's last push was 2024-02 — [EmbarkStudios/kajiya](https://github.com/EmbarkStudios/kajiya); [h3r2tic/kajiya](https://github.com/h3r2tic/kajiya).
- **rend3:** archived, "MAINTENCE MODE". Last crate 0.3.0 (2022-02-12) — [GitHub](https://github.com/BVE-Reborn/rend3).
- **Fyrox** 1.0.1 (2026-03-28). Graphics through `fyrox-graphics-gl` ("OpenGL-based graphics server", glow 0.17) — [crates.io fyrox](https://crates.io/crates/fyrox); [fyrox-graphics-gl](https://crates.io/crates/fyrox-graphics-gl).
- **three-d** 0.19.0 (2026-04-17), built on glow/OpenGL/WebGL — [crates.io](https://crates.io/crates/three-d).
- **NPR crates:**
  - `bevy_mod_outline` 0.13.0 (2026-07-09, active; mesh outlining) — [crates.io](https://crates.io/crates/bevy_mod_outline).
  - `bevy_toon_shader` 0.3.0 (2023-12, stale) — [crates.io](https://crates.io/crates/bevy_toon_shader).
  - `bevy_toon_material` 0.1.1 (2025-01, Wind Waker / BotW style, tiny) — [crates.io](https://crates.io/crates/bevy_toon_material).
  - `bevy_edge_detection` 0.15.4 (2025-01) — [crates.io](https://crates.io/crates/bevy_edge_detection).
- **Bevy platforms:** the 2023 announcement listed Windows, macOS, Linux, iOS, and Web — [Bevy + WebGPU](https://bevy.org/news/bevy-webgpu/). Consoles are not listed.

### Inferences
- Study Bevy's `bevy_pbr` clustered forward, prepass, bindless material, GPU preprocessing/culling, and its tonemapping/bloom/SMAA/TAA implementations as wgpu-specific prior art. Study `bevy_mod_outline` for hull and jump-flood outlines on wgpu.
- No Rust project implements a Genshin-class anime character pipeline. You will be porting techniques from Unity/UE community replications (HoyoToon, StarRailNPRShader, URPSimpleGenshinShaders).

### Gaps
- Current status of Bevy's experimental meshlet/virtual geometry feature in 0.20 was not confirmed (the 0.20 notes don't describe it). Whether Bevy 0.20 still ships SMAA, TAA, and bloom unchanged was not re-verified (very likely, but not checked).

---

## B5. Ray tracing and GI in Rust: options and relevance to the anime look

### Takeaway
RT is now feasible in Rust: wgpu ray queries, experimental RT pipelines, Metal acceleration structures, and Bevy Solari (ReSTIR). But it is not central to the anime look. Genshin's art direction explicitly favors "clean" lighting and avoids flashy tech. HoYo and Kuro titles ship probe, SH, and AO-volume GI, adding RT only as a PC/console extra (ZZZ 3.0). Treat RT as a late, optional high tier.

### Cited Findings
- **wgpu:** ray queries plus acceleration structures (merged feature, v27). Metal acceleration structures (v29). WGSL RT pipelines (v29) and SPIR-V-out RT pipelines (v30), gated behind `EXPERIMENTAL_RAY_TRACING_PIPELINES`. Raw `VkAccelerationStructureKHR` access added in trunk for unsupported extensions — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- **Bevy Solari:** realtime path tracing with ReSTIR and a world cache. On macOS it runs via Metal "but has no built-in denoiser there". DLSS-RR 4.5 is used for denoising on NVIDIA — [Bevy 0.20](https://bevy.org/news/bevy-0-20/). `bevy_solari` 0.20.0 — [crates.io](https://crates.io/crates/bevy_solari).
- **kajiya:** archived — [GitHub](https://github.com/EmbarkStudios/kajiya).
- **blade:** RT on Vulkan only — [README](https://github.com/kvark/blade/blob/main/blade-graphics/README.md).
- **Art direction:** Genshin aimed for "Fresh, bright, clean, anime-like" and avoided "cool-looking technologies" — [Unity Dojo slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi).
- **ZZZ:** PC 3.0 added RT reflections, AO, GI, and DLSS — [Tom's Hardware Italy](https://www.tomshw.it/videogioco/zenless-zone-zero-alza-lasticella-con-ray-tracing-e-dlss-45).
- **WuWa:** Kuro wants Lumen-style dynamic GI because baking is impractical for a dynamic open world — [GCORES](https://www.gcores.com/articles/181970).

### Inferences
- Start with SH probes, relightable reflection probes, AO volumes, capsule AO, and GTAO. This covers the anime look on mobile and console at 60 fps. An optional PC tier could use wgpu ray queries for RT shadows on characters or RT reflections later.
- Solari is a reference for ReSTIR on wgpu, not something to adopt for anime characters.

### Gaps
- No performance data for wgpu RT on Apple Silicon or mobile.

---

## B6. Recommendation: Rust stack and renderer architecture for a 60 fps anime action game (PC, console, mobile; dev on macOS)

### Takeaway
Use wgpu 30.x as the GPU layer behind a thin engine RHI trait, with WGSL plus WESL shaders (Slang offline only if needed). Build a custom clustered-forward+ renderer with a depth, normal, ID, and velocity prepass, a hand-written render graph, GPU-driven culling with multi-draw indirect, and texture bindless where available. Implement the anime character pipeline yourself (ramps, ILM/lightmap, SDF face, hull outlines with baked smoothed normals). Use Bevy's renderer and the Genshin, Blue Protocol, and WuWa talks as references. Keep RT optional. Plan console support as a separate NDA-backed backend, because no public Rust path exists.

### Cited Findings
- wgpu is the most actively maintained multi-backend option: v30.0.1 (2026-08-22), about quarterly majors, used by Bevy 0.20 (wgpu ^30) and Zed — [crates.io](https://crates.io/crates/wgpu); [bevy_render deps](https://crates.io/crates/bevy_render); [Zed Cargo.toml](https://github.com/zed-industries/zed/blob/main/Cargo.toml).
- Native Metal backend on objc2 (v29). Mesh shaders, RT, bindless, and Agility SDK as listed in B1 — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- Custom-backend hooks were expanded in v30 (`as_custom`) — [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md).
- WESL is now Bevy's shader language, and naga_oil is archived — [Bevy 0.20](https://bevy.org/news/bevy-0-20/); [naga_oil](https://github.com/bevyengine/naga_oil).
- Shipped anime games' choices (deferred in UE4-based titles; clustered lighting in Genshin; forward+ in HSR replication) — see A7 sources.

### Inferences
- **Concrete stack (Oct 2026):**

  | Concern | Choice |
  |---|---|
  | GPU API | `wgpu` 30.x |
  | Window | `winit` (not version-checked) |
  | Math | `glam` 0.34 |
  | Data layout | `bytemuck`/`encase` |
  | Shaders | `wesl` 0.6 plus naga 30 |
  | Hot reload | `notify` 8 |
  | Textures | `ktx2` 0.5 and `ddsfile` 0.6, offline `ctt` 0.6 |
  | Geometry | `meshopt` 0.7, `gltf` 1.4 |
  | Debug UI | `egui-wgpu` 0.37 |
  | Profiling | `wgpu-profiler` 0.28 plus `tracy-client` 0.19 |
  | Upscaling | `dlss_wgpu` 6 (PC NVIDIA); MetalFX via `objc2-metal-fx` plus `as_hal` interop (Apple) |
  | Particles | custom GPU particles (study `bevy_hanabi`) |

- **Why not raw ash or vulkano first:** a macOS developer would run through MoltenVK, ash's crates.io release is 2.5 years old, and you would still need DX12/Metal paths for Xbox and Apple-native quality. Revisit only if profiling shows wgpu CPU overhead is the bottleneck. Mitigate first with GPU-driven submission (few draws), render bundles, and multi-draw indirect.
- **Why not Bevy wholesale:** Bevy's renderer is PBR-first and ECS-coupled. An anime action game needs bespoke character passes, stencil and outline passes, custom tonemapping, and tight frame control. A custom renderer on wgpu, borrowing Bevy's techniques, is the more controllable path. Using Bevy (ECS plus custom render plugins) is a valid alternative if you want the tooling.
- **Risks to track:**
  - wgpu quarterly breaking changes: pin a version and upgrade deliberately.
  - Lack of async compute in wgpu.
  - Metal per-stage buffer limit of 29.
  - Mobile thermal budgets: forward+ at reduced internal resolution plus TAAU, like WuWa's mobile TAAU.
  - Console backend effort: expect to write against vendor APIs under NDA, possibly as a wgpu custom backend or a parallel RHI implementation.

### Gaps
- Console feasibility for Rust/wgpu could not be verified from public sources.
- No independent 2026 measurement of wgpu overhead in a draw-heavy game scene.
- Exact maturity of Slang's Metal output for production anime character shaders is unknown.
