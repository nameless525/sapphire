# Character rendering for the anime look (Wuthering Waves / ZZZ target): research notes for M1

- **Date:** 2026-10-24
- **Scope:** characters only (body, face, hair, eyes, outlines, rim, character lighting, special materials). Environment, post and GPU-stack material is in `research_notes/Rust anime action game engine/rendering_and_gpu.md` (cited below as [PRIOR]); this file goes deeper on characters and does not repeat that file's environment findings.
- **Targets:** Wuthering Waves (WuWa, Kuro Games, heavily customized UE4.26) and Zenless Zone Zero (ZZZ, HoYoverse, Unity). Supporting: Genshin Impact, Honkai: Star Rail (HSR), Punishing: Gray Raven (PGR, Kuro's previous game), Blue Protocol, Guilty Gear Xrd, Hi-Fi Rush, Idolmaster.

## How to read the evidence

Every claim carries a source key (list at the end). Each source is one of four kinds:

| Tag | Meaning | Trust |
|---|---|---|
| **P** | Primary: the developer's own talk, slides, interview or docs | High, but usually vague about shader math |
| **J** | Journalist or attendee write-up of a primary talk (GameBusiness, 4Gamer, CGWORLD, GCORES summaries, talk notes) | Medium-high |
| **C** | Community reverse engineering: shader reconstructions built on datamined materials (HoyoToon, StarRailNPRShader), or blogs | Medium. Parameter names and texture packing come from real game data; the math is reconstructed and may differ from the shipped shader |
| **I** | Inference: Sapphire engineering judgment, not sourced | Verify by prototyping |

**The most important caveat.** HoYoverse has not given a public character-shader talk since 2018. The 2018 Unite Tokyo talk covered Honkai Impact 3rd [U18]; the 2021 Unity Dojo console talk is about environment, apart from capsule AO for characters [UD21]. I found no GDC, Unite or CEDEC talk on Genshin, HSR or ZZZ character shading (searches in English, Chinese and Japanese; the HSR GDC 2025 talk is about game design [GDC-HSR25]). Kuro's public talks and interviews cover lighting, mobile deferred and ray tracing. Their character content is limited to lighting policy, face-shadow policy and GI stylization [UE-WW][GC-WW][UF25-WW]. **So the per-channel texture layouts and shader math for ZZZ and WuWa below are community reconstructions (C).** The best of these is HoyoToon, a Unity reimplementation driven by datamined material files (GPL-3.0) [HT]. Its WuWa, Genshin and HSR shader folders were last changed 2025-09-01 and the ZZZ folder on 2025-07-14 (GitHub API, checked 2026-10-24). That places its data around WuWa 2.5–2.6 and ZZZ 2.0–2.1.

**Licensing.** HoyoToon and StarRailNPRShader are GPL-3.0 [HT][SRN]. Sapphire must re-derive the techniques and must not copy their code. These notes describe the math in prose and formulas only.

---

## 0. Summary: what produces the look, ranked

1. **Authored light response instead of physical light.** All the reference games threshold N·L. A painted per-pixel bias (lightmap/ILM channel) and a per-vertex bias (vertex color) move the threshold. The shadow side takes an authored color (ramp row, or per-material shadow colors), never a darkened albedo. Cast shadows feed the threshold input rather than multiplying the result. [GGX-4G][HT-*][BP21]
2. **A character light rig decoupled from the environment.** WuWa runs an independent character-lighting pipeline with presets. Character brightness follows time of day through artist-fitted curves. Character GI goes through a stylization function: spherical normals for face and hair, normals biased toward the camera, HSV clamping and AO [UE-WW][GC-WW][UF25-WW]. Hi-Fi Rush and Guilty Gear do the same in spirit: separate light systems, and a light vector set per character or even per motion [HFR2][GGX-4G].
3. **The face is its own lighting problem.** An SDF/threshold face-shadow map is driven by the light angle in head space. WuWa adds special vertical-direction maps for top light [GC-WW]. Faces never take shadow-map self-shadowing. Hair shadow on the face is done in screen space or with stencil, nose lines and nose/cheek highlights come from masks, and blush comes from expression maps.
4. **Inverted-hull outlines with baked smoothed normals.** Width comes from vertex color × a distance curve × FOV normalization, with a Z-offset. Outline color comes per material ID and is lit by the scene light and GI.
5. **Screen-space rim light from depth differences.** Reconstructions of Genshin, HSR and WuWa use it; the ZZZ reconstruction uses it as a mask on an N·V/back-light "rim glow" [HT-WW-C][HT-ZZZ-C][SRN-Help].
6. **Material richness on top of the toon base.** Matcap metals, toon Blinn or "minimal GGX" specular gated by masks, stockings and emissive breathing. WuWa adds PBR touches: a normal map that also packs metallic and roughness, and ray-traced reflections on metals added on top of the matcap [TOS][UF25-WW].
7. **Hair:** a painted or anisotropic highlight band, plus eyes and brows readable through bangs (stencil).
8. **Eyes:** layered meshes and materials (eye, highlight, eye shadow), a parallax fake, and animated highlights.
9. **No self-shadow on characters, but grounding shadows.** WuWa uses per-object shadow maps for the ground and excludes characters from their own ray-traced shadows [UF25-WW]. Hi-Fi Rush gives the player a dedicated shadow-only light [HFR2]. Genshin uses capsule AO [UD21].

**How the targets differ (based on the C sources):**

| | WuWa | ZZZ | Genshin | HSR |
|---|---|---|---|---|
| Diffuse | Ramp row per material, blended with a "subsurface" tint (`_RampInt` default 0.3, so mostly tint) | **No ramp texture**: analytic lit / "shallow" / "shadow" bands with per-ID colors plus terminator "SSS/front" tints | 2D ramp: 5 IDs × day/night rows | Warm and cool ramps, 8 IDs |
| Spec | Thresholded Blinn plus a 4-channel matcap; optional PBR reflection/roughness (`UsePBR`) | URP-style minimal GGX with a toon step, or "HighlightShape" mode on spherized head normals; 5 matcap slots with blend modes | Blinn per ID; metals via spheremap | Per-region Blinn plus matcap |
| Normal map | XY normal, B = metallic, A = roughness | Lightmap RG = normal, B = diffuse bias | Separate | Separate |
| Face | SDF in mask alpha; special vertical maps (P) | SDF in LightTex.R, face highlight in .G, AO in .A; nose line | SDF in FaceMap.A | SDF in FaceMap.A; expression map |
| Rim | Depth-offset, gated by N·L | N·V "RimGlow" × depth mask | Depth-offset | Depth-offset × Fresnel; plus "rim shadow" |
| Outline normal | Smoothed normal in **tangent** | Tangent-space smoothed normal in **UV3** | Smoothed normal in **tangent** | Tangent or UV (replica options) |
| Outline width | Vertex G | Vertex R × lightmap B | Vertex A × 2-segment depth curve | Vertex A |

Sources: [HT-WW-C][HT-WW-P][HT-WW-S][TOS][HT-ZZZ-C][HT-ZZZ-P][HT-GI-C][HT-GI-P][HT-SR-P][HT-SR-C][SRN-OL].

---

## 1. Base shading model

### 1.1 The common core

**What it is.** Diffuse is a step or smoothstep of a *biased* N·L into two or three tones. Three inputs move the threshold:

- a painted "shadow tendency" channel (Guilty Gear ILM.G; Genshin and HSR lightmap G; ZZZ LightTex.B; WuWa mask G);
- a per-vertex bias (Guilty Gear vertex R as hand-tuned AO; HSR vertex R);
- the cast shadow (shadow-map attenuation multiplied into N·L before thresholding).

The shadow color is authored, not computed. [GGX-4G][HT-SR-C][HT-ZZZ-C][HT-WW-C]

- Guilty Gear Xrd (P/J):
  - Vertex color R is a "weight that makes areas likely to be shadowed", painted high under the jaw and around the neck.
  - ILM.G holds the same weight; at its maximum it acts as a baked shadow.
  - The toon split is a plain two-tone threshold. The example cutoff is 128.
  - Values near the threshold flicker; artists fix this by editing normals and the weights.
  - Source: [GGX-4G].
- Blue Protocol (J):
  - BaseColor is used on the lit side and ShadowColor on the shaded side.
  - ShadowColor is stored separately so shadows can shift hue. It was later derived from BaseColor through separate skin and cloth tables to save artist time.
  - Hair sets both colors directly.
  - Source: [BP21].
- Hi-Fi Rush (J): body uses `step(threshold, N·L)` (step rather than a branch) with flat, sharp shadow colors [HFR1].

**Implementation sketch (I, consistent with all the reconstructions):**

```
bias     = lightmap.shadowBias (0..1, 0.5 = neutral) and vertex AO
ndl      = dot(N, L) * castShadow          // cast shadow pushes toward shadow; it never multiplies the final color
x        = remap(ndl, bias)                // e.g. 0.5*ndl + 0.5, then scaled by 2*bias*vertexAO (HSR style),
                                           // or ndl + 2*(2*bias - 1)                     (ZZZ style)
hard-lock: bias ≥ ~0.95 → always lit; bias ≤ ~0.05 → always shadow
lit      = smoothstep(t - w, t + w, x)     // w is small: WuWa recon uses smoothstep(0.55, 0.81, N·L·cast) × AO,
                                           // then saturate(5·s + 0.5), a very short terminator
                                           // (note this also floors the term at 0.5; verify against captures)
diffuse  = albedo * lerp(shadowColor(x, materialID), 1, lit)
```

**Artist data:** base color; a shadow-bias channel; vertex-color AO; a material-ID channel; ramp rows or per-ID colors.

**Cost:** 1–2 texture taps and a few ALU ops. This is the cheapest part of the look.

**Mobile:** unchanged.

### 1.2 Genshin (C)

- **Lightmap RGBA** [JJ-GI][HT-GI-C][PRIOR]:
  - R: specular intensity / metal mask. Non-zero gets Blinn-Phong; 255 gets the metal treatment.
  - G: shadow weight (multiplied by vertex R).
  - B: specular shape/threshold.
  - A: material ID, five values ≈ 0, 0.3, 0.5, 0.7, 1.0. These select the ramp row and per-ID parameters. [JJ-GI] gives approximate meanings for 0/77/128/179/255: hard/emission/silk, soft/common, metal, tights, skin.
- **Shadow ramp:** one packed texture with 10 rows. The day row for ID *i* is at v = 1 − (0.1·i + 0.05) and the night row at v = 1 − (0.1·i + 0.55). Ramp x is the thresholded shadow term × AO. A day/night factor lerps the two rows. HoyoToon can derive that factor from light brightness and temperature, but this is an emulation. [HT-GI-C][JJ-GI]
- **Newer path:** a "shadow transition" path with no ramp exists alongside the ramp path. It uses per-ID warm and cool shadow colors with per-ID transition range and softness. Datamined parameter names: `_FirstShadowMultColor{1..5}`, `_CoolShadowMultColor{1..5}`, `_ShadowTransitionRange{1..5}`, `_ShadowTransitionSoftness{1..5}` [HT-GI-C].
- **Metals:** a spheremap/matcap (`_MTMap`) sampled with view-space normal.xy·0.5 + 0.5, with brightness, tile scale and an optional specular ramp [HT-GI-C].

### 1.3 Honkai: Star Rail (C)

- **Material ID:** `id = floor(8·lightmap.A)` gives **8 IDs**. The ramp v is `(2·id + 1)/16`, so a 16-pixel-tall ramp holds 8 rows. There are **separate warm and cool ramp textures** [HT-SR-P]. One walkthrough gives the ramp size as 256×16 with clamp wrap [LIKIRA].
- **Diffuse:**
  - `shadow = min(1, halfLambert · castShadow · 2·lightmap.G · vertex.R)`, remapped to 0.15..1.
  - Anything above a ramp-cutoff parameter is forced lit; otherwise it is the ramp u. [HT-SR-C]
- **Other lightmap channels** [HT-SR-P][SRN-Help]:
  - R masks the rim width.
  - B is the specular threshold.
  - [LIKIRA] gives a different layout (R = AO, G = highlight range, B = spec mask, A = ramp row). I trust the code-based reconstructions more.
- **Per-region parameters:** specular color, shininess and roughness; rim width, softness and darkening; and matcap. These are 8-element arrays indexed by region. [HT-SR-P]
- **"Rim shadow":** an N·V edge *darkening* with an offset view vector, a power, a width and a feather. It adds silhouette definition without lines. [SRN-Help]
- **Pipeline (C, via search excerpt):** the replicators say the HSR scene is deferred and characters are forward, with two passes per part [ZH-HSR].

### 1.4 ZZZ (C): analytic bands, no ramp texture

HoyoToon's ZZZ shader shows no ramp texture for body diffuse [HT-ZZZ-C][HT-ZZZ-P]. Instead:

- **Material ID:** OtherData.R holds five bands (thresholds 0.2/0.4/0.6/0.8). Every lighting parameter exists ×5:
  - `_ShallowColor{,2..5}` (first shade) and `_ShadowColor{,2..5}` (second shade);
  - `_SpecularColor*`, `_SpecularRange*`, `_ToonSpecular*`, `_ModelSize*`, `_HighlightShape*`, `_ShapeSoftness*`;
  - `_RimGlowLightColor*`, `_UISunColor*`, `_OutlineColor*`;
  - five matcap slots with per-slot blend modes (alpha-blend, add, overlay).
- **Diffuse input:** `x = N·L·castShadow + 2·(2·AO·castShadow − 1)`, with AO = LightTex.B. One smoothness parameter (`_AlbedoSmoothness`) builds three bands with thresholds near −⅓ and +⅓. The shallow and shadow colors are normalized and multiplied by "post" tints (`_PostShallowTint`, `_PostShadowTint`, `_PostFrontTint`, `_PostSssTint`). The front and SSS tints add a colored band at the terminator: a cheap "subsurface edge". [HT-ZZZ-C]
- **Specular:**
  - Structurally this is Unity URP's minimal Cook-Torrance (`r² / ((N·H²(r²−1)+1)² · max(0.1, L·H²) · (4r+2))`).
  - F0 = lerp(0.04, albedo, metallic). Roughness = (1 − smoothness·gloss)².
  - The inputs are remapped ×0.75 + 0.25. The result is scaled by `_ToonSpecular × _ModelSize` and then hard-clamped (`saturate(10x)·100`), which makes a crisp-edged highlight.
  - An alternative "HighlightShape" mode thresholds a painted highlight mask (OtherData.B) against a lit term. That term is computed on normals **spherized around `_HeadSphereNormalCenter`** inside a radius.
  - [HT-ZZZ-C]
- **Textures:**
  - `_LightTex`: RG = normal, B = diffuse bias / AO, A = face AO.
  - `_OtherDataTex`: R = material ID, G = metallic, B = specular mask.
  - `_OtherDataTex2`: R = transparency, G = smoothness, B = emission mask. [HT-ZZZ-P]
- **Color grading inside the character shader:** an optional `LUT_2D(_Lut2DTexParam)` runs in the character shader [HT-ZZZ-P]. This may be HoyoToon emulating the game's post LUT, so don't read it as proof that ZZZ grades characters separately.
- **Engine:** ZZZ is Unity [ZH-ZZZ-ENG]. A 2026 article says it runs on Tuanjie, Unity China's fork [FORO3D]; that is unverified.

### 1.5 Wuthering Waves: a PBR-influenced toon hybrid

- **Lineage (C, via search excerpt):** a Chinese replicator writes that WuWa's character rendering builds on PGR's "PBR-style NPR". Shadows come from a LUT with N·L range on x and body-part ID on y [ZH-WW-ASE].
- **What PGR does (C):**
  - Skin and cloth use two-tone toon lighting; metal and leather use PBR. A mask selects between them [ZH-PGR].
  - A reproduction lists LightMap.R = ramp offset, G = AO, B = "SpecularMask decides PBR use", and a separate PBRMask (R metallic, G smoothness, B specular AO, A PBR-blend factor).
  - It uses a GGX D·F·G specular with Blinn-Phong blended by the mask. [PGR]
- **WuWa textures** (C; [TOS] marks itself work in progress):
  - `_D`: RGB base. A is opacity or mask (uncertain).
  - `_N`: R = X, G = Y, **B = metallic, A = roughness**; Z is reconstructed.
  - `_ID`: cloth and skin type mask.
  - `_HM` (hair mask): R = highlight intensity, G = highlight shift, B = self-shadow/AO.
  - Material switches: `UsePBR` (PBR reflection and roughness), `UseTypeMask`, `UseRampColor`, `UseTranslucent`.
  - Asset paths and prefixes: `Client/Content/Aki/Character/Role/{FemaleM,…}`, `R2T1_` meshes, `MI_` material instances. [TOS]
- **HoyoToon's WuWa reconstruction** [HT-WW-C][HT-WW-P][HT-WW-S]:
  - Material types: Base, Face, Eye, Bangs, Hair, Glass, Tacet Mark.
  - Body shadow: a hard smoothstep on N·L × castShadow, with `_ShadowProcess` (default 0.55) and `_ShadowWidth` (0.01).
  - Shadow color: `lerp(subsurfaceColor, rampRow, _RampInt)` with `_RampInt` default **0.3**. Skin uses `_SkinSubsurfaceColor`, default (0.94, 0.60, 0.41), a warm peach. So WuWa skin shadows are mostly a warm tint and only partly the ramp.
  - The ramp row comes from a per-material `_RampPosition` parameter; skin is pinned to row 0.1.
  - Skin and stocking types come from thresholds on the type mask (0.05/0.3/0.5/0.9).
  - Specular: thresholded Blinn (`pow(N·H, power)`, kept only where ≥ 0.1, and only where the shadow mask ≥ 0.1). There is a separate metal power and a second toon lobe above `_ToonMaxSpecular`. It is tinted by albedo^lerp(0.5, 2, spec), so highlights stay saturated.
  - Metals and gloss: a 4-channel matcap. The spec value picks which of the four channels to use, and lit and shadow intensities are separate (`_MatCapInt`, `_MetalMatCapInt`, `_MetalMatCapBack`).
- **Primary confirmation of matcap metals and how ray-traced reflections are added (attendee notes of Kuro's UFSH2025 talk)** [UF25-WW]:
  - Character reflections apply to metal surfaces (weapons, accessories).
  - Roughness is clamped. **Metallic is used as reflection strength, not as physical metallic.**
  - Only pixels with metallic > 0.9 reflect.
  - The reflection result is **added on top of the existing MatCap**, which stays the base metal look.
  - The material graph has separate raster and RT branches, and the GI branch "can skip full PBR".
- **Gradient painting (P):** gradients are painted into base color and mask textures; system-UI characters get especially rich gradients. The team restrains engine physical features: low sample counts give a "hand-drawn" feel. [GC-WW][GR-WW]
- **Takeaway (I).** WuWa reads as "more PBR" than Genshin for several reasons:
  - real normal maps with metallic and roughness;
  - matcap plus screen-space or RT reflections on metals;
  - stylized but present GI on characters (§9);
  - softer, color-graded scene lighting with per-scene LUTs [GR-WW].

  Diffuse is still a hard toon split with authored shadow color.

### 1.6 Other references

- **Idolmaster (Bandai Namco, Unity, CEDEC 2024)** (J) [IM24]:
  - Lighting: a dedicated "toon shadow main light" plus additive point and spot lights with distance falloff. Sometimes the main light is set dark and additive lights make the look.
  - Shadow color: skin shadow uses a multiply. Non-skin lerps between color and shade-color textures by brightness.
  - Specular: cube map × a per-character specular ramp.
  - Textures: Def texture RGBA = shade offset, smoothness, metallic, specular mask. Ramp RGB = skin ramp, A = non-skin ramp. The material ramp stacks 6 columns, selected by vertex-color material ID.
  - Vertex color: **7 channels packed at 4 bits each into one RGBA**.
- **Blue Protocol (UE4 deferred)** (J): N·L is computed in the BasePass because the G-buffer can't hold two normals. Specular is brightened BaseColor. [BP21]

### 1.7 Sapphire recommendation (I)

One character uber-shader with two diffuse modes, both supported:

- **Ramp mode** (Genshin/HSR/WuWa): ramp texture array, row by material ID, day/night or warm/cool pair, plus a subsurface-tint lerp (WuWa).
- **Band mode** (ZZZ): 2–3 bands, with per-ID shallow/shadow colors and terminator tints from the material parameter buffer.

Band mode needs no texture, is cheaper on mobile, and is easier to tune live. Ramp mode gives painterly multi-hue shadows. Shared by both:

- per-ID parameter arrays (5–8 IDs);
- toon Blinn or minimal-GGX specular gated by a mask;
- a matcap (1–4 channels);
- an optional "reflection add" for metallic > 0.9 (SSR or probe on desktop, probe only on mobile).

This matches the spec's "two-tone split + ramp rows by material ID" and adds the band mode and the matcap and reflection terms.

---

## 2. Texture and vertex-data contract

### 2.1 What each game packs (C unless marked)

| Data | Genshin | HSR | ZZZ | WuWa |
|---|---|---|---|---|
| Base | RGB albedo, A = emission/alpha (claimed) [JJ-GI] | RGB, A alpha/emission | RGB, A alpha (legacy: other data) | RGB, A = shadow mask, emission or opacity [HT-WW-P][TOS] |
| Lightmap / mask | R spec/metal, G shadow, B spec shape, A ID (5) | R rim mask, G shadow, B spec threshold, A ID (8) | LightTex RG normal, B bias/AO, A face AO | `_MaskTex`: G = shadow/AO; face A = SDF |
| Second data map | none | none | OtherData: R ID, G metal, B spec mask; OtherData2: R alpha, G smooth, B emission | `_ID`/TypeMask: R skin/stocking type, Z ramp mask |
| Normal | separate | separate | in LightTex RG | XY + metallic (B) + roughness (A) [TOS] |
| Ramp | 1 packed, 10 rows | warm + cool, 8 rows | none (per-ID colors) | per-material row (`_RampPosition`) |
| Face | FaceMap A = SDF; newer FaceMap B = outline width | FaceMap A = SDF, G = eye/brow mask, B = nose-line mask; ExpressionMap R cheek, G shy, B shadow | LightTex.R SDF, G face highlight | MaskTex.A SDF |
| Hair | lightmap; spec ramp | lightmap | lightmap | `_HM`: R highlight, G shift, B AO |
| Matcap | `_MTMap` | `_MatCap` per region | 5 slots | 4-channel `_MatCapTex` |
| Vertex color | R = AO, G = shadow-ramp width, B = outline Z-offset (signed, B − 0.5, along the view ray), A = outline width | R = AO, A = outline width | R = outline width (on eye meshes, the eye-shadow LUT index), G = outline extra, B = **bit flags** (face direction, IDs, eye flags), A = flag | R = skin/type fallback ID, G = outline width, B = inner-outline range on face |
| UVs | uv0 + mirrored face uv | uv1/uv2 (one-sided eye stencil uses uv2) [SRN-Face] | uv3 = tangent-space smoothed outline normal | uv3 = gradient coordinate (`UVGradient`) |

Sources: [HT-GI-C][HT-GI-P][HT-SR-P][SRN-Face][HT-ZZZ-P][HT-WW-P][HT-WW-S][TOS][JJ-GI].

### 2.2 Gotchas visible in the reconstructions

- **Data maps must be linear.** The face SDF needs sRGB disabled [NOIRC]. HoyoToon ships an sRGB warning UI for packed maps [HT].
- **Vertex colors are sRGB-decoded in the WuWa path.** HoyoToon converts WuWa vertex RGB from sRGB to linear before use [HT-WW-P]. Unreal stores vertex color as 8-bit sRGB `FColor` (I). Decide the convention in the cooker and keep it consistent.
- **ZZZ packs bit fields into vertex color B** (`int(B·255) & mask`) [HT-ZZZ-P]. Bit fields don't survive interpolation, so they only work as per-vertex constants on whole regions and must be decoded per vertex. Sapphire should cook these into a dedicated `u8` or `u16` attribute instead (I).
- **Ramps stay uncompressed.** BC formats band them; the spec already says so.

### 2.3 Proposed Sapphire character asset contract (I)

- **Base** (BC7 sRGB): RGB albedo, A = alpha or emission (per material flag).
- **Control map** (BC7 linear, or uncompressed on mobile if banding shows):
  - R: shadow bias (0.5 = neutral; ≥0.95 locked lit; ≤0.05 locked shadow);
  - G: specular mask/threshold;
  - B: matcap/metal mask (> 0.9 enables reflection add);
  - A: material ID (8 levels, nearest-filtered or read by `textureLoad`).
- **Normal** (BC5): XY. Optional second BC5 or BC7 holds metallic/roughness, or follows WuWa (B/A) on BC7.
- **Face map** (R16 or BC4, linear):
  - R: horizontal SDF threshold;
  - G (optional): vertical SDF (WuWa top light);
  - B: nose-line and highlight mask;
  - A: always-lit mask (eyes, brows).
  - Plus an expression mask (blush, shy, shadow).
- **Ramp array:** RGBA8, uncompressed, 256×N rows, day/night or warm/cool pair.
- **Vertex attributes:**
  - color RGBA8 (R = AO, G = outline width, B = outline Z-offset, A = spare);
  - smoothed outline normal (octahedral 2×snorm16 or 2×unorm8, object or tangent space);
  - a `u8` flag attribute (face side, eye/brow, region).
- **Per-material parameter block:** per-ID arrays (shadow colors, specular, rim, outline color), indexed by ID.

---

## 3. Faces

### 3.1 SDF / threshold face-shadow maps

**What.** A grayscale "threshold map" in face UV space. For a light at horizontal angle θ from head-forward, a pixel is lit when map(uv) > f(θ). The map is authored for light from one side and mirrored (u → 1 − u) for the other side.

- Hi-Fi Rush credits the method to a 2018 miHoYo talk at Unity Seoul [HFR1].
- Genshin, HSR, ZZZ and WuWa all use variants [HT-GI-C][HT-SR-C][HT-ZZZ-C][HT-WW-C].
- A community dataminer found WuWa's face "gradient texture" in the game files: each gray level sets a shadow amount, and the texture is mirrored [TSUKI].
- Kuro's interview confirms WuWa's special facial shadow approach. It supports custom performance shadows for **"vertical-direction or special-mood light sources"**, for example top light that needs a special vertical-direction shadow map [GC-WW].

**Runtime math (two variants seen):**

| Variant | Formula | Used by |
|---|---|---|
| Dot-linear | `t = 1 − (F·L_h · 0.5 + 0.5)`; lit = `smoothstep(t − s, t + s, map)`; plus a back-light kill `step(−0.5, F·L_h)` | HoyoToon WuWa/Genshin/ZZZ reconstructions [HT-WW-C][HT-GI-C][HT-ZZZ-C] |
| Angle-linear | `t = acos(F·L_h)/π` (or `acos(R·L_h)` remapped) | Hi-Fi Rush [HFR1]; HSR URP walkthrough [LIKIRA] |

- **Side:** choose u or 1 − u from `sign(R·L_h)`, where R is head-right.
- **Softness:** Genshin's datamined parameters are `_FaceMapSoftness` and `_FaceMapRotateOffset` [HT-GI-C]. WuWa uses `_SDFSmoothness`. ZZZ softens by ±0.5 around t on a remapped map (`map·0.9 + 0.1`) and multiplies by face AO [HT-ZZZ-C].
- **Eyes and brows always lit:** HSR forces lit where FaceMap.G marks eyes [LIKIRA].
- **Input vectors:** head-forward and head-right per character. HoyoToon takes them as material vectors (`_headForwardVector`, `_headRightVector`, ZZZ adds `_headUpVector`) transformed by the head object [HT-ZZZ-P]. The engine should take them from the head bone each frame (spec §5.12 already does).

**Implementation notes (I):**

- Project L onto the head's horizontal plane using head-up (`L_h = normalize(L − (L·U)U)`), not world XZ as the reconstructions do. World-XZ projection breaks when the head tilts or during cinematic rolls.
- Pick **one** threshold convention and bake with it. Equal-angle masks need angle-linear t. The akasaki tool assigns *equal-interval thresholds* to its input masks [NAGA], so equal-angle painting plus angle-linear runtime is the consistent pair.
- **Vertical/top light (WuWa P):** add a second channel authored for pitch. Blend by `L·U`, or switch when |L·U| exceeds a cutoff. Expose a per-shot override for cutscenes.
- **Shadow-map interaction:** ignore the shadow map on the face, or only accept *scene* casters (§9.4). Reconstructions disable self-shadow on face and eyes (WuWa) or skip cast shadow under the newer Genshin face path [HT-WW-P][HT-GI-C]. A Genshin replica notes that cast shadows on the face become unstable up close [NOIRC].

**Authoring the map (the main artist cost):**

1. Paint N binary masks of the shadow shape at increasing light angles (from front to 90° side or beyond). The ASP manual uses 9 [ASP]; one Unity tool claims 180 SDFs in under a minute [FENG].
2. Compute a signed distance field per mask (8SSEDT or jump flooding [8SSEDT]).
3. For each consecutive pair, blend in the region where the shadow moved: `t = SDF₁/(SDF₁ + SDF₂)` gives a 0→1 gradient from the source boundary to the destination boundary [NAGA]. The map value there is `lerp(T_i, T_{i+1}, t)`. That last step is my reading; [NAGA] says thresholds are equal-interval.
4. Pairwise blending is only C0-continuous. With few masks the shadow "stutters" between keys, which is why the input should have many masks [NAGA].
5. Output 8- or 16-bit grayscale. 16-bit gives smoother boundaries [AKA].

- **Tools:** akasaki1211's script (any mask count, Gaussian or bilateral filter, 8/16-bit) [AKA]; yespure's UE tool, which can also bake light masks from face normals [SDFUE]; fenglvv's compute tool [FENG].
- **For Sapphire (I):** build a cooker step (`sapphire_cook face-sdf`) that takes a mask folder and writes the R16 map. That keeps the threshold convention in one place.

**Cost:** 1–2 taps per face pixel, negligible.

**Mobile:** unchanged. Use BC4 or R8 if 16-bit is too much memory, and test for banding.

### 3.2 Face normals: editing and transfer

- **Older approach (still used for body and some faces):** hand-edit normals so the terminator falls in anime shapes.
  - Guilty Gear pulled cheek normals toward temple normals, and transferred normals from proxy shapes such as cylinders for trousers using Softimage GATOR. Programmers patched UE3 so edited normals survived import [GGX-4G].
  - Blue Protocol painted normals for the triangular cheek shadow and imported avatar parts together to avoid seams [BP21].
  - Idolmaster hand-edited face normals and converted normals to tangents on export to fix outlines and shadows [IM24].
  - Hi-Fi Rush started with normal tweaks and moved to threshold maps because edited normals break under free rotation and deformation [HFR1].
- **Modern split (I, consistent with the reconstructions):** the face SDF drives *diffuse*; normals drive spec, rim, outline and GI only.
  - Normals for those terms are often spherized around a head center. ZZZ's highlight mode uses `_HeadSphereNormalCenter` [HT-ZZZ-C].
  - WuWa uses spherical normals for face and hair *in GI* [UF25-WW].
  - Tooling: Blender's Data Transfer from a sphere, or "sphereize normals" add-ons [BNPR].
- **Artist data:** a head-center point (bone or socket) and a radius for spherized normals; optionally transferred custom normals.

### 3.3 Nose line, nose and cheek highlights

- **Nose line (ZZZ, HSR; C):** a line that appears on the nose ridge as the face turns. Formula: `pow(N·V, k)`, thresholded and masked by a nose texture, colored by the per-ID outline color.
  - ZZZ: smoothstep with `_NoseSmoothX/Y` minus a nose texture [HT-ZZZ-C].
  - HSR: `step(0.1, FaceMap.B · pow(N·V, NoseLinePower))` [HT-SR-P].
  - StarRailNPRShader exposes `_NoseLineColor` and `_NoseLinePower` [SRN-Face].
- **Face highlight (ZZZ; C):** LightTex.G holds painted highlight shapes (nose tip, cheek). They are revealed by the same face-angle term with a ±0.75 softness, gated to the central region of the face map, × `pow(N·H, 10)·10` [HT-ZZZ-C].
- **Artist data:** a nose/highlight mask channel in the face map.

### 3.4 Blush and expression overlays

- **HSR ExpressionMap (C):** R = cheek blush (thresholded by `_ExMapThreshold`, colored `_ExCheekColor`), G = "shy" (`_ExShyColor`), B = shadow (`_ExShadowColor`). Intensities are animated material parameters. [HT-SR-P][SRN-Face]
- **Implementation (I):** expose these as animatable per-instance floats in the frame packet (driven by facial animation curves), not as material swaps.

### 3.5 Hair shadow cast on the face

Shadow maps are too coarse and unstable at face scale [NOIRC], and characters exclude self-shadow anyway. Two screen-space methods appear:

| Method | How | Where seen | Cost |
|---|---|---|---|
| **A. Hair-depth lookup** | Render hair (bangs) depth into a separate target. In the face shader, offset the screen UV by the view-space light direction × width. Width is scaled by FOV (`cot(fov/2)/2.414`), by 1/(depth/modelScale), and by log10(screen height)/3.33 on x. If hair depth there is in front of the face depth, the pixel is in shadow. | StarRailNPRShader (`HairDepthOnlyPass`, `GetHairShadow`, `_HairShadowDistance`) [SRN-HD][SRN-Face] | One extra hair depth draw plus 1 tap per face pixel |
| **B. Offset bangs redraw + stencil** | Redraw the bangs mesh with vertices pushed a small amount along the view-space light direction, stencil-tested to face pixels. Output a shadow color (blend or multiply). | HoyoToon WuWa "Stencil Shadow" pass on Bangs, hair-shadow color default (0.94, 0.60, 0.41) [HT-WW-P][HT-WW-S]; HoyoToon ZZZ "Shadow" material type pushed toward light, outputting a (1, 0.9, 0.9) tint [HT-ZZZ-P] | One extra bangs draw; no extra render target |

- ZZZ's datamined materials include a dedicated **"Shadow" material type** next to Base, Face, Eye, Hair, EyeHighlight and EyeShadow [HT-ZZZ-S]. This suggests ZZZ ships hair-shadow geometry or materials per character. Inference from C data.
- **Sapphire (I):**
  - Desktop: method A. The character-only prepass already exists; add a hair-depth attachment, R16F, or reuse the prepass with a hair ID.
  - Mobile: method B. No extra render target; works with stencil on tile.
  - Tint the shadow warm (skin shadow color), never gray.

---

## 4. Hair

### 4.1 Highlights

- **miHoYo 2018 (P, Honkai Impact 3rd)** [U18]:
  - tangent-based anisotropic lighting with multi-layer specular, roughness, flow and AO maps;
  - low- and high-frequency spec combined;
  - a stylized anisotropic strip that follows strand direction and changes shape toward the tips;
  - per-strand UV gradients with jitter noise on the strip;
  - silk lit along the bitangent with 1–3 layers.
- **WuWa (C):** `_HM` R = highlight intensity, G = shift, B = AO [TOS]. HoyoToon's hair specular is a painted band revealed by N·H: `max(mask, 1 − N·H)` normalized against `1 − N·H` and × `_SpecStrength`. The highlight *shape* is painted and light only gates it [HT-WW-C].
- **ZZZ (C):** the same per-ID specular, with the "HighlightShape" mode on spherized head normals [HT-ZZZ-C]. The highlight stays a clean shape regardless of strand geometry.
- **"Angel ring" (Unity Toon Shader, P/official docs):**
  - Docs: the ring "always appears in a fixed position as seen from the camera"; it needs a 2nd UV; it is added to lighting; Offset U/V parameters control it [UTS-AR].
  - Source math: u = view-space normal.x·0.5 + 0.5 (with camera roll compensation); v = lerp(UV2.y, view-space normal.y·0.5 + 0.5, OffsetV) [UTS-SRC].
  - Authoring: lay out UV2 so v runs root to tip and paint the ring texture.
- **Sapphire (I):** offer two hair-spec modes.
  1. **Painted band:** mask × thresholded N·H (or spherized-normal N·L). Cheapest, and closest to WuWa and ZZZ.
  2. **Angel ring:** UV2 + view-space normal, for a Genshin-like ring.

  Kajiya-Kay with a shifted tangent is optional on desktop. Data: hair mask (intensity, shift, AO), a UV2 for the ring, and strand-aligned tangents if using anisotropy.

### 4.2 Hair diffuse

- Hair gets its own shadow path and colors in every game. Blue Protocol sets hair Base and Shadow colors directly [BP21]. WuWa hair uses mask G for solid shadow and its own subsurface tint offset [HT-WW-C]. HSR has separate body and hair ramps [ZH-HSR].
- Hair should generally be *excluded* from fine self-shadow (I): its strands create noisy shadow-map acne.

### 4.3 Eyes and brows readable through bangs (stencil)

**StarRailNPRShader's scheme** (C, most complete public reference) [SRN-Face][SRN-HairS][SRN-Hair]:

- **Stencil bits:** bit 0 = character, bit 1 = eye.
- **Face pass:** writes the eye bit where the FaceMap.G eye/eyelash/brow mask is set. It also requires |sceneDepth − eyeDepth| < `_MaxEyeHairDistance × modelScale`, so you can't see a *different* character's eyes through someone's hair. On game models the mask is sampled with uv2, because some characters need it on one eye only.
- **Hair pass 1 (opaque):** stencil `NotEqual` eye bit.
- **Hair pass 2 (over eyes):**
  - stencil `Equal` eye bit, `Cull Back`, alpha-blended;
  - a **manual depth test** against scene depth so only the top hair layer blends (with a HALF_EPS bias against MSAA and Android precision fighting);
  - it must not clear the eye stencil, or overlapping hair breaks.
- **Alpha:** `max(viewFadeH, viewFadeV, _HairBlendAlpha)`. viewFadeH = `saturate((1 − cos∠(V_xz, F))/0.658)` (70°); viewFadeV uses 45°. Seen from the side, bangs go opaque again.
- **Outline pass:** writes the character bit.

**Other games:**

- HoyoToon's HSR variant: the same idea, plus a side mask (`_HairSideChoose`) for asymmetric bangs [HT-SR-P].
- WuWa (C): eyes and face clip against a `_Mask` stencil texture. Bangs (type 3) use either alpha-stencil output or clip at 0.8 [HT-WW-P].
- Blue Protocol (J): hair goes in a separate ToonHairPass before lighting. The BasePass masks the eyebrows, and the hair pass skips updating the inner-outline ID in that mask, so the eyebrow line shows through [BP21].

**Sapphire (I):**

- Adopt the two-bit stencil scheme. wgpu supports stencil ops and masks per pipeline.
- The eye mask comes from a face-map channel. The depth mask needs the character prepass depth.

**Cost:** one extra raster of the bangs region with blending, small.

**Mobile:** stencil is fine on tile GPUs. Reading scene depth for the manual depth test needs a depth texture, so either do it in a second subpass, or use plain depth test LEqual against the already-written hair depth (I).

### 4.4 Hair motion (out of scope, but affects the look)

- WuWa hair physics probably uses the KawaiiPhysics plugin with carefully weighted inner bones. This is a third-party TA's inference [AUT2].
- ZZZ embeds a Maya dynamics solver in the rig for cinematics and hand-adjusts hair per frame in action shots [ZZZ-AM].

---

## 5. Eyes

**Structure (layered materials/meshes):**

- ZZZ's datamined material types include **Eye, EyeHighlight and EyeShadow** as separate materials [HT-ZZZ-S]. EyeShadow is tinted by a small LUT looked up by vertex color R (4-bit x, 4-bit y) [HT-ZZZ-P].
- HSR has a separate eye-shadow shader [SRN] and eye-shadow color parameters [SRN-Face].
- A third-party TA calls WuWa's highlight and iris/sclera layering "not especially difficult" and suspects expressions and eye tremble are animated by hand [AUT2].

**Refraction/parallax fake:**

- miHoYo 2018: "Texture UV offset based on view dir" [U18].
- HoyoToon's WuWa reconstruction uses full parallax occlusion mapping with a height map in `_EM.G`, masked to the iris [HT-WW-C]. It is unclear whether WuWa itself uses POM or a single offset.
- **Recommendation (I):** a single-tap bump offset is enough at anime scales. `uv' = uv − (V_ts.xy / V_ts.z) · h · irisMask`, with h from a painted iris depth.

**Highlights (C, WuWa):**

- A highlight texture is sampled through a 2D affine transform: position, rotation, a scale of 1/(0.24, 0.135).
- An animated **"shake"** term `sin(2π·speed·t)·scale` makes the highlight tremble; defaults scale 0.01, speed 10.
- A second highlight is taken from `_EM.R` with its own width.
- [HT-WW-C][HT-WW-S]

**Always lit and through hair:** eyes ignore face shadow (HSR forces lit by FaceMap.G [LIKIRA]) and render over or through bangs (§4.3).

**Artist data:** iris/sclera texture, iris depth/mask, highlight texture(s), eye-shadow mesh or material, eye bones for gaze. Idolmaster uses eye bones and calls eye shaders among its most-used [IM24].

**Cost:** tiny screen area.

**Mobile:** same. Skip POM.

---

## 6. Outlines

### 6.1 Method: inverted hull, everywhere for characters

- A second pass with front-face culling extrudes vertices along a *smoothed* normal in view space. HSR, Genshin, ZZZ and WuWa all use it [SRN-OL][HT-GI-P][HT-ZZZ-P][HT-WW-P].
- miHoYo 2018 lists three outline methods [U18]:
  1. geometry, needing continuous normals, with width from vertex color and perspective correction;
  2. "enhanced geometry" with crease edges detected in preprocessing;
  3. image-space from normals and depth, with "constant performance", suited to scenes.
- Guilty Gear runs a Z-prepass plus an outline geometry pass, so characters get three geometry passes [GGX-4G].

### 6.2 Smoothed normals: computation and storage

- **Computation (C):** per vertex *position*, sum the face normals of all incident triangles **weighted by corner angle**, then normalize [SRN-Norm].
  - Do it per submesh: some ripped or MMD meshes have back-face duplicates whose normals would cancel.
  - Use double precision for tiny models, since small triangles produce zero vectors in float.
- **Storage options seen:**
  - **Tangent attribute** (object space, replacing the real tangent): Genshin and WuWa reconstructions; WuWa's outline type enum is Off/Tangent/Normal with default Tangent [HT-GI-P][HT-WW-S]. The cost is that the real tangent is lost: WuWa's stocking anisotropy has to rebuild tangents from screen derivatives [HT-WW-C].
  - **UV channel, tangent-space XY** (Z reconstructed as `sqrt(1 − x² − y²)`, rotated by TBN): ZZZ, default `_NormalUV = 3` [HT-ZZZ-P][HT-ZZZ-S]. StarRailNPR offers object-space tangent, object-space UV7 and tangent-space UV7 [SRN-Norm]. Tangent space survives skinning without extra bone math.
  - **Vertex color RGB:** reported for HSR by Chinese replicators (search excerpt) [ZH-HSR]. A commercial UE tool also bakes into vertex color in tangent, object or world space [PRIOR].
  - **Idolmaster:** normals converted to tangents and two extra UV sets added on Maya→Unity export [IM24].
- **Sapphire (spec already says "smoothed normals into a spare attribute, octahedral"):** bake **tangent-space, octahedral, 2×snorm16** into its own attribute. That keeps real tangents for normal maps and anisotropy. Compute skinning then transforms it through the skinned TBN for free.

### 6.3 Width control

- **Per-vertex width channel:**
  - Genshin: vertex A, optionally × an outline texture channel (newer faces use FaceMap.B) [HT-GI-P].
  - ZZZ: vertex R × `_OutlineWidth` × 0.0015, optionally × LightTex.B [HT-ZZZ-P].
  - WuWa: vertex G (optional), with vertex B reducing width as an "inner range" on faces [HT-WW-P].
  - HSR and PGR: vertex A [SRN-OL][PGR].
- **Distance curve:** Genshin's datamined parameters are `_OutlineWidthAdjustZs` (three depth breakpoints) and `_OutlineWidthAdjustScales` (three scales). The scale is a **two-segment piecewise-linear function of view depth × FOV factor** [HT-GI-P].
- **FOV normalization:**
  - Reconstructions multiply depth or width by `2.414/cot(fov/2)`, where 2.414 = cot(22.5°), i.e. normalized to a 45° FOV [HT-GI-P][HT-ZZZ-P].
  - StarRailNPR scales by `clamp(fixScale·0.025, 0.04, 0.1)`, with `fixScale = (2.414/cot(fov/2)) · viewDepth/modelScale`. Lines get relatively thinner with zoom-in and stay bounded [SRN-OL].
  - Unity Toon Shader uses `smoothstep(farthest, nearest, distance)` [UTS-SRC].
- **Model scale:** StarRailNPR multiplies every screen-space width (outline, rim, hair shadow) by `_ModelScale` so chibi and adult models match (I: worth copying).

### 6.4 Z-offset and flattening

- Push outline vertices away from the camera so they don't poke through the front surface:
  - Genshin and ZZZ move the view-space position along itself by `lerp(0.1·max, max, saturate(1 − depth))` [HT-GI-P][HT-ZZZ-P].
  - StarRailNPR adds a view-space Z offset × modelScale "to hide outlines on cards/patches" [SRN-OL].
  - Unity Toon Shader offsets clip-space z [UTS-SRC].
- **Flatten the extrusion normal in view space:** set z to a small constant before normalizing. Genshin uses 0.01, ZZZ 0.0001, StarRailNPR −0.1. Lines then grow sideways rather than toward the camera [HT-GI-P][HT-ZZZ-P][SRN-OL]. PGR scales z by `bias·(1 − vertex.B)` [PGR]. A Chinese practitioner note says the same [PRIOR].
- **Per-vertex Z-offset mask:**
  - Genshin reconstruction: vertex B − 0.5 scales a signed push along the normalized view-space position, × `_MaxOutlineZOffset` [HT-GI-P].
  - Guilty Gear: vertex B, as reported by secondary sources [PRIOR].
  - HSR walkthrough: an outline Z-offset mask texture [LIKIRA].

### 6.5 Color

- **Per material ID:**
  - Genshin: `_OutlineColor{,2..5}` with `_OutLineIntensity*`. The color lerps toward ~0.2 × albedo by intensity [HT-GI-P].
  - ZZZ: per-ID color, `pow(c·0.5, 1.5) × albedo`, darkened on the unlit side by `N·L·0.5 + 0.5` [HT-ZZZ-C].
- **Lit by the scene:** WuWa and ZZZ reconstructions multiply outline color by the light color and add flattened GI, so lines darken at night [HT-WW-P][HT-ZZZ-P]. PGR darkens and saturates albedo for the line color [PGR].
- **Emissive and special outlines:** Genshin has outline emission and the Natlan-era "Nyx state" animated noise-ramp outlines [HT-GI-C].
- **Sapphire (I):** outline color = `lerp(idColor, albedo·k, intensity) × (lightColor·mainTerm + flattenedGI)`. Store it per ID. Do not use pure black.

### 6.6 Inner lines

- **Guilty Gear "Motomura lines" (secondary):** lines painted in texture (ILM alpha) on UV layouts aligned to keep lines crisp at any scale [PRIOR]. Game Watch's GDC 2015 report says black was set along polygon "edges" so lines stay clean when scaled [GGX-GW].
- **Dynamic:** nose lines (§3.3); Blue Protocol's inner-outline ID in its outline G-buffer [BP21].
- **For Sapphire:** texture-space lines need no engine work. Optional prepass-ID edge detection could add inner lines on desktop.

### 6.7 Screen-space outlines on characters

- Blue Protocol draws outlines in a post-process-like pass from an extra G-buffer [BP21]:
  - R: thickness from TexCoord1.x, adjusted for distance and aspect;
  - G: inner-outline ID;
  - B: painted outline;
  - A: depth shift.
- Hi-Fi Rush has a post-process outline [PRIOR].
- I found **no evidence of screen-space outlines on WuWa or ZZZ characters**. The reconstructions use hull outlines only.
- **Recommendation (I):** hull for characters. Prepass-driven screen-space edges for environment (desktop, optional), as the spec already plans.

### 6.8 AA interaction

- TAA softens outlines by about 1 px. Blue Protocol writes outlines to stencil and uses responsive AA [BP21].
- Kuro's UFSH2023 talk covered adapting mobile TAAU for "anime elements"; details weren't available in text [UF23-WW].
- **Sapphire:** keep the spec's plan (composite or mask outlines for TAA; MSAA on mobile if on-tile).

**Cost (I):** one extra rasterization of each outlined mesh. Vertex work is negligible with compute pre-skinning; fill is a thin shell. Budget 0.3–1 ms on desktop for a party plus a crowd. Skip outlines on far LODs and on eyes, glass and hair-shadow materials (reconstructions clip them [HT-WW-P][HT-ZZZ-P]).

**Mobile:** same technique. Fade width to 0 and disable the pass beyond an LOD distance. Use MSAA instead of TAA where possible.

---

## 7. Rim light

### 7.1 Depth-offset screen-space rim (Genshin, HSR, WuWa reconstructions)

**Math:**

```
offsetUV  = screenUV + N_view.xy * width
rimDepth  = linearDepth(offsetUV) − linearDepth(screenUV)
rim       = smoothstep(a, b, rimDepth)        // or step(threshold, rimDepth)
```

Width scales with 1/distance and FOV:

- WuWa reconstruction: `_RimWidth·0.0025 × lerp(0.5, 0.45, f) × saturate(1/distance) / f`, with f = fovDeg/180 [HT-WW-C].
- StarRailNPR: pixels × `(cot(fov/2)/2.414) × 10/sqrt(depth/modelScale)` × screen height, offsetting **horizontally only** using `sign(cross(V, N).y)` for an even band [SRN-Help].
- A PGR reproduction takes 4 taps (left, right, up, down) and uses the max [PGR].

Community attributes the Genshin version to "offset in screen space, sample depth, subtract, step()" [BILI-RIM] (search excerpt).

**Masks and modulation:**

- **WuWa (C):** multiplied by `smoothstep(0, 0.5, N·L)`, so the rim only appears on the lit side. Color `_RimColor`. Added to albedo before the shadow lerp [HT-WW-C][HT-WW-P].
- **HSR (C):**
  - 8 per-region width, softness and darkening values; masked by lightmap R ("some places shouldn't get rim");
  - × Fresnel `pow(1 − N·V, edgeSoftness)`;
  - lit by `lerp(darken, 1, N·L·shadow·attenuation)` with separate front- and back-face intensities;
  - faded out under dither alpha so dithered holes don't produce rims. [HT-SR-P][SRN-Help]

### 7.2 N·V "rim glow" with a depth mask (ZZZ reconstruction)

ZZZ's `RimGlow` [HT-ZZZ-C][HT-ZZZ-P]:

- an N·V edge term with different curves for skin and non-skin;
- × a back-light factor from `(−V·L)` (stronger when the light is behind the character);
- × a factor from `normal.y`;
- colored per ID (`_RimGlowLightColor*`), tinted by a per-ID sun color (`_UISunColor*`);
- then multiplied by the **depth-difference mask × N·L** (×2.5, saturated, ×1.5).

So ZZZ combines Fresnel shaping with a depth-based silhouette mask.

### 7.3 Other variants

- **Blue Protocol:** Sobel edge extraction composited as rim for uniform thickness [BP21].
- **Idolmaster:** rim uses the original (unedited) normals; dense meshes keep it clean [IM24].
- **"Rim shadow" (HSR):** edge darkening (§1.3).

### 7.4 Sapphire (I)

- **Desktop and mobile:** depth-offset rim sampling the *character prepass depth* (scene depth works too). A character-only prepass is cheap and also feeds hair shadow and the eye depth mask. That makes the spec's "N·V rim on mobile" fallback unnecessary except on the lowest tier.
- Rim = `depthMask × fresnel × lightGate × regionMask × rimColor`, with an option to add it to albedo or to emission (bloom).

**Cost:** 1–2 depth taps per character pixel.

**Mobile:** reading depth needs the prepass resolved to a texture. A character-only prepass at full res costs one more draw per character (I).

---

## 8. Character lighting and consistency across environments

### 8.1 A separate character light rig

- **WuWa (P):**
  - "We built a separate character lighting pipeline." Presets and automation set up lighting quickly.
  - Character brightness through the day is fitted with UE curve tools so it tracks the 3D environment.
  - In-game lighting with custom light components was used even for PVs.
  - Sources: [GC-WW][GR-WW]; UE interview quotes via search excerpt [UE-WW].
- **Hi-Fi Rush (J):**
  - Character and environment lighting are **separate systems**: forward lights and decal lights don't affect characters.
  - The main directional light is computed in the forward base pass; two main lights are supported.
  - Source: [HFR2].
- **Guilty Gear Xrd (J):**
  - Only one directional light casts dynamic shadows, used for ground shadows.
  - Each character has its **own internal light vector, tuned per motion**, and in cutscenes per frame.
  - The team chose "artistically correct" over physically correct.
  - Source: [GGX-4G].
- **GBF Relink (J):** cutscenes start from in-game lighting and add per-character lights for eye texture, armor spec, cape bounce and hair color [GBF].

**Sapphire (I):** the spec's "virtual key light curved by time of day" matches WuWa. Add:

- per-zone or per-volume character-light presets (direction offset, color, intensity, rim color, shadow-tint multiplier);
- per-shot and per-character overrides for cutscenes;
- an optional "light follows camera" yaw clamp, so the face SDF never goes fully dark during gameplay. This is a common practice inferred from GGXrd's per-motion vectors, not documented for WuWa.

### 8.2 GI and ambient on characters: stylize it

- **WuWa (attendee notes of the UFSH2025 talk)** [UF25-WW]:
  - spherical normals for hair and face, and the whole character's normals biased toward the camera, for low-frequency, smooth indirect light;
  - indirect converted to **HSV, with S and V limited** to prevent blow-out;
  - AO multiplied on for layering;
  - extra color adjustment for skin. The stated reason is that toon shadow areas are not black;
  - characters need indirect light from scene reflections for an airy look.
- **Blue Protocol (J):** point, sky and indirect light **ignore normals** (distance or uniform only) to avoid a volumetric look [BP21].
- **Community emulation (C):**
  - light color = `max(SH(0) and SH(down), lightColor)`;
  - GI = `SH(N)` clamped to ≤ 1 per channel, weighted by its luminance and by `smoothstep(1, 0, lum/2)`;
  - result: `color·lightColor + GI·…` [HT-WW-P][HT-ZZZ-C].

**Sapphire (I):** a `character_gi(N, V, headCenter, partMask)` function:

```
N' = normalize(lerp(N, sphereNormal, s_part) + b·V)
c  = SH(N')
c  = hsvClamp(c, maxS, maxV)
c  = c * AO * skinTint
```

Feed it SH probes (spec: L2 per zone) and optionally the nearest reflection probe at high roughness. Expose s/b/maxS/maxV per zone preset. This is the main lever for the WuWa-style "character belongs in the scene" look.

### 8.3 Day/night and color temperature

- Genshin: day and night ramp rows per ID [HT-GI-C][JJ-GI].
- HSR: warm and cool ramp textures [HT-SR-P].
- Genshin shadow-transition path: warm and cool shadow colors per ID [HT-GI-C].
- WuWa: TOD-fitted brightness curves (P) [GC-WW] and per-scene LUT grading [GR-WW].
- **Sapphire:** a ramp-pair blend factor (or band-color pair) driven by the TOD curve, alongside the light color curve.

### 8.4 Shadows: no self-shadow, but grounding

- **WuWa (attendee notes):**
  - "Toon rendering doesn't want character self-shadow."
  - In raster, the scene uses CSM and characters use **per-object shadow maps**; character shading samples only the CSM (scene casters), which avoids self-shadow.
  - For RT shadows, an **instance mask** excludes characters when the shading point is a character.
  - Source: [UF25-WW].
- HoyoToon exposes `_UseSelfShadow` (default on for WuWa body) because Unity shadow maps include the character [HT-WW-S]. Prefer the talk notes.
- **Blue Protocol (J):** cel-shaded surfaces don't receive self-shadows. Clean results would need high shadow-map resolution, and shadow color already keeps shade areas from going dark. Toon and non-toon objects use separate shadowing with an "offset shadow" method [BP21].
- **Hi-Fi Rush (J):**
  - the player gets a **dedicated shadow-only light** with a stylized edge, replacing cascades outdoors;
  - the giant boss gets its own as well;
  - NPCs use capsule shadows, with an AO-volume decal fallback for small flying characters.
  - Source: [HFR2]. The talk's agenda (toon light, dynamic shadow-map strategy, static shadow maps, probe GI, face shadow) is in [HFR-GDC].
- **Genshin (P):**
  - capsule AO for characters, driven by the skeleton and directional, because skinned characters can't use baked AO volumes;
  - added because characters standing in shadow "appear to be floating".
  - Source: [UD21].
- **HSR replica (C):** self-shadow plus up to 16 per-object shadows on screen [SRN]. Its author found *low-quality* soft-shadow filtering better on characters, because wider filters cause more perspective aliasing [SRN-Help].
- **miHoYo 2018 (P):** a view-dependent character shadow map with PCSS soft shadows that works for transparents [U18].
- **Common shading rule (C):** cast shadow multiplies N·L (or the AO bias) *before* the threshold, so the result is the authored shadow color, not black [HT-WW-C][HT-ZZZ-C][HT-SR-C].

**Sapphire (I):**

1. A **shadow receiver mask**: characters receive CSM from environment casters only, never from themselves.
2. A per-character shadow map (spec: desktop) used only for the character's shadow on the environment and on *other* characters (optional).
3. Capsule AO for grounding on all tiers (spec has it).
4. Optional hair→face screen-space shadow (§3.5).
5. If RT is added later, use instance masks the same way.

### 8.5 Local lights

- Reconstructions add point and spot lights as a soft additive term, clamped by luminance (`_FilterLight`) so lights can't blow characters out. Extra directional lights are ignored [HT-WW-P][HT-ZZZ-P].
- Blue Protocol makes point lights ignore normals [BP21]. Idolmaster uses additive lights with distance falloff [IM24].
- **Sapphire:** local lights on characters go through the same threshold (or a half-Lambert soft term) plus a luminance clamp. Allow light-layer masks so VFX lights don't hit faces (StarRailNPR checks light layers [SRN-Help]).

---

## 9. Transparency and special materials

### 9.1 Stockings and tights

- **WuWa (C):** stocking regions come from the type mask [HT-WW-C]. Shading:
  - an anisotropic term from `1 − (T·H)²` raised to `_AnistropyInt`, perturbed by the normal map;
  - a *denier* color shift from `smoothstep(_StockingRangeMin, _StockingRangeMax, N·V)` lerping `_StockingEdgeColor` → `_StockingColor`, applied to albedo by `_StockingIntensity`;
  - a sheen `smoothstep(min, max, N·H) × _StockingLightColor`.
- **HSR (C):** `_StockRangeTex` [HT-SR-P]:
  - R = stocking area/density;
  - G = specular mask;
  - B = a tiled weave texture for a fabric feel;
  - shading: Fresnel-like `smoothstep(power, darkWidth·power, N·V)`, a dark-edge color, a stocking color, and a spec term `pow(N·V, power1)`.
- **Non-anime reference:** inZOI's documented stocking layer uses one texture (normal, Fresnel, alpha) blended onto skin, with a darker center and bright edge [INZOI].
- **Sapphire (I):** a "sheer fabric" sub-mode: `skin → stocking color by N·V curve × density mask`, plus optional anisotropic sheen. Opaque, so no sorting.
- **Mornye's "crystal legs" (WuWa 3.x):** a TA flags translucency sorting as the hard part and doesn't know how Kuro did it [AUT2]. Not documented.

### 9.2 Glass, visors, lenses

- WuWa's reconstruction has a Glass type with a highlight texture; HoyoToon calls its logic not yet reverse-engineered [HT-WW-P].
- An HSR walkthrough does sunglasses with Schlick Fresnel (F0 0.04) × intensity [LIKIRA].
- **Sapphire:** transparent pass, Fresnel + matcap/probe reflection + painted highlight. No outline.

### 9.3 Emission and bloom masks

| Game | Emission source (C) | Notes |
|---|---|---|
| WuWa | diffuse alpha ≥ `_EmissionBreathThreshold` | "Breath light" pulse (`_UseBreathLight`); emissive pixels skip scene lighting [HT-WW-C][HT-WW-P] |
| ZZZ | OtherData2.B | Plus a second emission layer by material ID [HT-ZZZ-P] |
| Genshin | base alpha (claimed) | Per-ID emission color and scaler; outline emission [JJ-GI][HT-GI-C] |
| HSR | replica | Per-material bloom color and intensity: `color × (1 + intensity·bloomColor)` [SRN-Help] |

- **Sapphire (I):** write an emissive/bloom-weight channel the bloom prefilter reads (the spec has an emissive-driven bloom mask). Let character materials push bloom without raising base luminance.

### 9.4 Screen-space pattern materials (a common "gacha" trick)

- Genshin "star cloak" materials (Mona-style starfields), screen- or world-mapped [HT-GI-C].
- ZZZ `ScreenImage` (screen-UV texture with rotation, scroll and blink, masked) [HT-ZZZ-C].
- WuWa "starry sky / aurora" and **Tacet marks**. The marks are SDF decals revealed by two scrolling noises, on uv3 sides [HT-WW-C].
- **Sapphire:** a generic `screen_pattern` and `sdf_reveal` layer in the uber-shader (one permutation bit or a branch on a material flag).

### 9.5 Dithered fade near the camera

- WuWa applies a camera-proximity "Transparency Filter" to playable characters. A ReShade add-on patches it out by detecting the pattern in compiled pixel shaders, which shows it is a shader-level fade [WWTFR]. The mechanism (dither or alpha) isn't documented.
- StarRailNPR uses a 4×4 Bayer `clip(alpha − threshold)` and suppresses rim under dithering [SRN-Help].
- **Sapphire (I):**
  - Ordered or blue-noise dither, with TAA or MSAA resolve (spec: TAA on desktop).
  - Fade outlines with the same dither.
  - Kill rim when the dither alpha is below 0.9.
  - Disable the hair-shadow and eye stencil tricks while fading.
  - Also use it for character swap-in and swap-out.

---

## 10. Other essentials for the look (not in the question list)

- **Vertical gradients on the body:**
  - WuWa interview: gradients painted into base and mask textures [GC-WW].
  - WuWa reconstruction: a dynamic UV gradient (`_UseUVGradient_DYN`, `_UvGradientScale/Process`, `_UVGradientColor`, saturation) on uv3.y, tinting toward feet or head [HT-WW-S][HT-WW-C].
  - Blue Protocol: legs slightly darkened "to ground the character" [BP21].
  - Cheap and very visible. Add a per-material vertical-gradient term in model space (I).
- **Warm, saturated shadow hues:**
  - WuWa skin shadow tint default (0.94, 0.60, 0.41) and `_RampInt` 0.3 [HT-WW-S].
  - ZZZ terminator SSS tint [HT-ZZZ-C].
  - Blue Protocol "shadows can shift hue" [BP21].
  - GBF Relink: sky-blue "Celeste blue" in shadows [GBF].
  - Never darken toward gray.
- **Specular tinted by albedo:** WuWa raises albedo to a power by spec value [HT-WW-C]; Blue Protocol uses brightened base color [BP21]. White highlights look plastic.
- **Per-material-ID parameter arrays:** 5 (Genshin, ZZZ) or 8 (HSR) [HT-*]. They let one draw carry skin, cloth, metal and leather looks without material splits. This fits the spec's storage-buffer-by-material-ID plan.
- **Color pipeline:**
  - Genshin rejects ACES and uses a custom curve plus hue shift [PRIOR].
  - The HSR replica uses custom bloom and tonemap [SRN].
  - WuWa grades with a LUT per scene [GR-WW].
  - The spec's custom curve in a 32³ LUT is right. Keep characters' saturated mid-tones out of the shoulder.
- **Facial animation:**
  - WuWa moved from bone-only to blendshape expressions, plus RBF correctives for joints [GC-WW].
  - ZZZ rebuilt cutscene rigs with dozens of extra face controllers [ZZZ-AM].
  - Guilty Gear used ~170 head bones (~70 for expressions) and separate battle and cutscene heads, with battle eyes enlarged for readability [GGX-4G].
  - The look depends on supporting morph targets *and* bones (the spec does).
- **Distance-aware identity details:** Blue Protocol builds moles from polygons and enlarges them with a dedicated shader as the camera moves away, so they stay visible. Its face parts are bone-driven rather than blendshapes for crowd cost [BP21b]. The general idea (I): keep identity marks such as moles, eye highlights and lines at a minimum screen size.
- **Limited animation:** Guilty Gear's keyframe-only, sprite-like timing [GGX-GW]. ZZZ hand-keys almost all combat animation [ZZZ-AM]. This is an animation-system feature (stepped evaluation per clip), not rendering.
- **Debug views to add (I):** face-SDF threshold view, eye/hair stencil view, rim depth mask, GI-stylization before and after, shadow receiver mask. The spec already lists material ID, ramp row and outline width.

---

## 11. Version-specific notes (as of 2026-10-24)

**Wuthering Waves:**

- UE4.26 with UE5 features back-ported; deferred on mobile ("one-pass deferred") [UE-WW][GR-WW].
- RT arrived in v2.1 [WW-21RT]. A TA notes an Ultra Quality setting from v2.4 [AUT2].
- The UFSH2025 RT talk by graphics lead Wang Xin targets an RTX 4060 at 2K/60. It covers the Lumen-framework choice, character instance masks, character GI stylization and the metal reflection rules [UF25-WW].
- v3.0 "We Who See the Stars" (2025-12-25): shadow and effects tuning, a Carlotta hair-clipping fix; no character-shader change listed [WW-30].
- v3.5 (2026-07-10): XeSS 3, DLAA, anisotropic filtering options, mobile frame interpolation; no character-rendering change [WW-35].
- I found no public evidence of a character-shader overhaul through 3.x. Character packages keep adding special materials (crystal legs) [AUT2].
- AUTOMATON's May 2026 columns (TA Masahiko Nakamura, who was not on the project) speculate that a Lumen-HW-RT-based system includes a custom toon implementation [AUT1][AUT2].

**ZZZ:**

- Unity. v3.0 (2026-06-17) added NVIDIA DLSS frame generation and ray tracing on PC [ZZZ-30]. Earlier reporting says RT reflections, AO and GI [ZZZ-RT-IT].
- No official character-shader information. HoyoToon's ZZZ data dates from mid-2025 (~v2.0–2.1).

**Genshin:** the reconstruction includes a newer face path (`_UseFaceMapNew`, FaceMap.B as outline width) and Natlan-era "Nyx" outlines [HT-GI-C][HT-GI-P]. That shows the character shader still evolves per region update.

**HSR:** StarRailNPRShader was archived on 2025-03-21 [SRN].

---

## 12. Implications for Sapphire work packages

The spec §5.12 already plans: two-tone split with ILM masks, ramp array by ID with a day/night pair, face SDF from head vectors with mirroring, hair aniso plus angel ring, eye parallax plus stencil, depth rim (desktop) / N·V (mobile), a TOD-curved character key light, shadow bias of the ramp, and hull outlines with octahedral smoothed normals × vertex color × distance curve × resolution scale with per-material color.

This research **confirms** all of that and suggests these additions or changes (all I):

| # | Work package | Why (source) | Rough cost | Mobile |
|---|---|---|---|---|
| WP1 | Band diffuse mode (2–3 bands with per-ID shallow/shadow colors and terminator tints) next to ramp mode | ZZZ reconstruction [HT-ZZZ-C] | ALU only | Preferred (no ramp fetch) |
| WP2 | Subsurface-tint lerp with the ramp (`_RampInt`-style) and warm skin defaults | WuWa reconstruction [HT-WW-C] | ~0 | Same |
| WP3 | Matcap (1–4 channels by spec value) plus a "reflection add" for metallic > 0.9 with clamped roughness | WuWa talk [UF25-WW], reconstructions | 1–2 taps | Matcap only |
| WP4 | Character GI stylization function (spherized and camera-biased normals, HSV clamp, AO, skin tint), with per-zone presets | WuWa talk [UF25-WW]; Blue Protocol [BP21] | Few ALU + SH | Same |
| WP5 | Shadow receiver mask (characters receive only environment casters) and a per-character shadow map for ground only | WuWa talk [UF25-WW]; Blue Protocol; Hi-Fi Rush | 1 shadow draw per hero | Player-only shadow map or capsule |
| WP6 | Character-only depth prepass (with hair ID/depth) shared by rim, hair shadow and eye depth mask, on **all tiers** | [SRN-Face][SRN-Help] | ~1 extra draw per character | Yes (cheap at character coverage) |
| WP7 | Hair→face shadow: A (hair-depth lookup) on desktop, B (offset bangs + stencil) on mobile | [SRN-HD][HT-WW-P][HT-ZZZ-P] | 1 tap or 1 bangs draw | B |
| WP8 | Eye/brow-through-bangs stencil (2 bits), view-angle alpha, eye depth mask | [SRN-HairS][SRN-Face] | 1 bangs redraw | Yes |
| WP9 | Face SDF in head-space angles with a **vertical** channel and a cooker tool for mask → SDF → threshold map | WuWa P [GC-WW]; [NAGA][AKA] | 1–2 taps | Same |
| WP10 | Face extras: nose line, face highlight mask, expression/blush channels animated per instance | ZZZ/HSR reconstructions | ~0 | Same |
| WP11 | Outline: tangent-space octahedral smooth normal; Genshin-style 2-segment depth curve; FOV normalization to 45°; view-space z flatten plus push-back; color = per-ID lerp with albedo × (light + flattened GI); model-scale factor | [HT-GI-P][HT-ZZZ-P][SRN-OL] | Spec budget 0.6/1.5 ms | LOD cutoff |
| WP12 | Rim: depth-offset (horizontal, model-scale normalized) × Fresnel × N·L gate × region mask; ZZZ-style back-light weighting option | [SRN-Help][HT-ZZZ-C][HT-WW-C] | 1–2 taps | Yes with WP6 |
| WP13 | Hair spec modes (painted band; UV2/view-space angel ring; optional Kajiya-Kay) | [TOS][UTS-SRC][U18] | 1–2 taps | Band/ring |
| WP14 | Special layers: stockings (N·V denier, aniso sheen, weave), screen pattern, SDF reveal (Tacet-like), breathing emission, vertical gradient | Reconstructions [HT-*] | Per feature | Same |
| WP15 | Dithered proximity/swap fade that also fades outline and kills rim | [WWTFR][SRN-Help] | ~0 | Same |
| WP16 | Character light presets and per-shot overrides (direction, color, rim, shadow tint), driven by TOD curves | WuWa P [GC-WW]; Guilty Gear; Hi-Fi Rush | CPU | Same |

**Permutation pressure.** The spec caps the family at 8 static bits. Keep face, hair, eye and transparent as static bits, and make band-vs-ramp, matcap channels, stocking and screen-pattern *dynamic branches* on uniform flags. Kuro chose deferred partly to curb variant growth [PRIOR], so watch this.

---

## 13. Not publicly documented (state plainly)

- **ZZZ:**
  - every character-shader detail beyond community reconstruction;
  - whether characters are forward or deferred;
  - the outline width curve;
  - how ZZZ characters receive RT lighting in 3.0.
- **WuWa:**
  - exact diffuse math and ramp layout;
  - the face SDF channel layout and the vertical-map scheme beyond the interview sentence;
  - outline normal storage beyond reconstruction;
  - the rim method;
  - the proximity-fade mechanism;
  - the crystal-legs translucency;
  - whether "per-object shadow maps" ever self-shadow the body.
- **Genshin and HSR:** no official layout documents. All channel meanings are from datamined materials plus reconstructions.
- **Guilty Gear Xrd:** the full ILM.R/B/A and vertex G/B/A meanings rest on secondary sources [PRIOR]. The Motomura handout PDF [GGX-PDF] was unreachable (connection refused) and the 4Gamer part 2 article (outlines) wasn't found.
- **Fetch gaps:** GDC Vault was in maintenance during this session. Zhihu, CSDN and Bilibili articles were blocked (403, 521 or CAPTCHA), so claims from them are marked "search excerpt" and should be re-verified by a human with a browser.

---

## Sources

Tags: P = primary, J = journalist/attendee report, C = community reverse engineering, I = inference.

**Wuthering Waves / Kuro**
- [UE-WW] (P) Epic, "Exploring the post-apocalyptic charm of ASG open worlds in Wuthering Waves" (May 2024): https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves. Returned 403 to fetch; quotes taken via search excerpts.
- [GC-WW] (J, interview compilation) GCORES, "探究二次元开放世界的后启示录美学｜《鸣潮》专访汇总" (2024-05-16): https://www.gcores.com/articles/181970
- [GR-WW] (J, interview) GameRes, "技术专访：探究《鸣潮》二次元开放世界的后启示录美学": https://www.gameres.com/905377.html
- [UF25-WW] (J, attendee notes of the P talk) UFSH2025, "开放世界二次元游戏的光追实战" (Wang Xin, Kuro):
  - notes: https://wlxklyh.site/uf2025-shanghai/rendering/wuthering-waves-raytracing/
  - video: https://www.bilibili.com/video/BV1hSW4zTEgQ/
  - more notes (403; search excerpts only): https://zhuanlan.zhihu.com/p/1967117197915693977, https://zhuanlan.zhihu.com/p/1994601108010713503
- [UF23-WW] (P, video) UFSH2023, 王宏波 (Kuro), "《鸣潮》基于虚幻引擎4的多平台效果和性能优化实践": https://www.bilibili.com/video/BV1BK411v7FY/
- [WW-21RT] (J) Sohu on v2.1 ray tracing: https://www.sohu.com/a/856619895_122004016
- [WW-30] (P) Kuro, v3.0 patch notes: https://wutheringwaves.kurogames.com/en/main/news/detail/3920
- [WW-35] (J) IT之家, WuWa 3.5 graphics features (2026-06-27): https://www.ithome.com/0/969/268.htm
- [AUT1] (J, third-party TA opinion) AUTOMATON, "Even from a developer's perspective, Wuthering Waves' use of Unreal Engine is borderline perverse" (2026-05-07):
  - article: https://automaton-media.com/en/column/even-from-a-developers-perspective-wuthering-waves-use-of-unreal-engine-is-borderline-perverse-a-ue4-game-decked-out-in-custom-technology/
  - JSON used for reading: https://automaton-media.com/en/wp-json/wp/v2/posts/66290
- [AUT2] (J, third-party TA opinion) AUTOMATON, "Wuthering Waves is full of beautiful technical headaches…" (2026-05-27):
  - article: https://automaton-media.com/en/column/wuthering-waves-is-full-of-beautiful-technical-headaches-that-would-make-the-average-ue-developer-say-please-dont-make-me-do-this/
  - JSON used for reading: https://automaton-media.com/en/wp-json/wp/v2/posts/67296
- [TOS] (C) Tales of Star, "Wuthering Waves: Shader and Material" (work in progress): https://blog.talesofstar.com/posts/wuwa-shader-material/
- [TSUKI] (C) "A surface-level dive into Wuthering Waves' character models" (2024-05-28): https://forum.tsuki.games/t/a-surface-level-dive-into-wuthering-waves-character-models/149
- [ZH-WW-ASE] (C, search excerpt only) Zhihu, "Unity BRP ASE还原鸣潮人物渲染尝试": https://zhuanlan.zhihu.com/p/787922179
- [WWTFR] (C) wuwatfr, WuWa transparency-filter remover: https://github.com/wuwatfr/wuthering-waves-transparency-filter-remover
- [ZH-PGR] (C, search excerpt only) Zhihu PGR analysis: https://zhuanlan.zhihu.com/p/471945089
- [PGR] (C) "战双卡通渲染复现": https://xiatianlaila.github.io/2023/10/14/%E6%88%98%E5%8F%8C%E5%8D%A1%E9%80%9A%E6%B8%B2%E6%9F%93%E5%A4%8D%E7%8E%B0/

**HoYoverse / miHoYo**
- [U18] (P) Unite Tokyo 2018, miHoYo, Honkai Impact 3rd "from mobile to high-end PC" slides: https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd
- [UD21] (P) Unity Dojo 2021, Zhenzhong Yi, Genshin console slides: https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi
- [GDC-HSR25] (P, abstract) GDC 2025, "Honkai: Star Rail Reimagining" (design talk): https://gdcvault.com/play/1035480/-Honkai-Star-Rail-Reimagining
- [ZZZ-AM] (J) GCORES translation of the Animedia Dec 2024 ZZZ interview (2024-12-23): https://www.gcores.com/articles/192638
- [ZZZ-30] (J) IT之家, ZZZ 3.0 DLSS FG and RT (2026-06-06): https://www.ithome.com/0/960/973.htm
- [ZZZ-RT-IT] (J, via PRIOR) Tom's Hardware Italy on ZZZ RT: https://www.tomshw.it/videogioco/zenless-zone-zero-alza-lasticella-con-ray-tracing-e-dlss-45
- [ZH-ZZZ-ENG] (C, search excerpt) Zhihu answer stating ZZZ uses Unity: https://www.zhihu.com/question/532683323/answer/2485717586
- [FORO3D] (J, unverified) Foro3D, ZZZ engines and workflow (Jan 2026): https://foro3d.com/en/2026/january/technical-analysis-of-zenless-zone-zero-engines-and-3d-workflow.html
- [ZH-HSR] (C, search excerpts only) Chinese HSR replication notes: https://zhuanlan.zhihu.com/p/1891924287054673274, https://zhuanlan.zhihu.com/p/711037262
- [LIKIRA] (C) "卡通渲染全流程，以崩坏星穹铁道角色为例": https://li-kira.github.io/2023/11/13/ToonShader/
- [JJ-GI] (C) Juejin, "原神角色渲染 Shader 分析还原": https://juejin.cn/post/7207631397094948920
- [BILI-RIM] (C, search excerpt only) Bilibili opus on Genshin depth rim: https://www.bilibili.com/opus/453497651103088679
- [NOIRC] (C) NoiRC256/URPSimpleGenshinShaders: https://github.com/NoiRC256/URPSimpleGenshinShaders

**Community reconstructions (code, GPL-3.0; describe, don't copy)**
- [HT] Hoyotoon/HoyoToon: https://github.com/Hoyotoon/HoyoToon
- WuWa:
  - [HT-WW-C] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Wuthering%20Waves/Include/HoyoToonWutheringWaves-common.hlsl
  - [HT-WW-P] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Wuthering%20Waves/Include/HoyoToonWutheringWaves-program.hlsl
  - [HT-WW-S] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Wuthering%20Waves/HoyoToonWutheringWaves.shader
- ZZZ:
  - [HT-ZZZ-C] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/ZenlessZoneZero/Include/zzz-common.hlsl
  - [HT-ZZZ-P] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/ZenlessZoneZero/Include/zzz-program.hlsl
  - [HT-ZZZ-S] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/ZenlessZoneZero/HoyoToonZenlessZoneZero.shader
- Genshin:
  - [HT-GI-C] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Genshin%20Impact/Includes/HoyoToonGenshin-common.hlsl
  - [HT-GI-P] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Genshin%20Impact/Includes/HoyoToonGenshin-program.hlsl
- HSR:
  - [HT-SR-P] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Honkai%20Star%20Rail/Includes/HoyoToonStarRail-program.hlsl
  - [HT-SR-C] https://github.com/Hoyotoon/HoyoToon/blob/main/Shaders/Honkai%20Star%20Rail/Includes/HoyoToonStarRail-common.hlsl
- [SRN] stalomeow/StarRailNPRShader (archived 2025-03-21): https://github.com/stalomeow/StarRailNPRShader
  - [SRN-Face] https://github.com/stalomeow/StarRailNPRShader/blob/main/Shaders/Character/CharFaceCore.hlsl
  - [SRN-Hair] https://github.com/stalomeow/StarRailNPRShader/blob/main/Shaders/Character/CharHairCore.hlsl
  - [SRN-HairS] https://github.com/stalomeow/StarRailNPRShader/blob/main/Shaders/Character/CharHair.shader
  - [SRN-OL] https://github.com/stalomeow/StarRailNPRShader/blob/main/Shaders/Character/Shared/CharOutline.hlsl
  - [SRN-Help] https://github.com/stalomeow/StarRailNPRShader/blob/main/Shaders/Character/Shared/CharRenderingHelpers.hlsl
  - [SRN-Norm] https://github.com/stalomeow/StarRailNPRShader/blob/main/Runtime/Utils/NormalUtility.cs
  - [SRN-HD] https://github.com/stalomeow/StarRailNPRShader/blob/main/Runtime/Passes/HairDepthOnlyPass.cs

**Other games and talks**
- [GGX-4G] (J) 4Gamer, Zenji Nishikawa, GGXrd graphics, part 1 (2014): https://www.4gamer.net/games/216/G021678/20140703095/
- [GGX-GW] (J) Game Watch, GDC 2015 GGXrd talk report: https://game.watch.impress.co.jp/docs/news/20150307_691725.html
- [GGX-PDF] (P, unreachable this session) Motomura handout: https://www.ggxrd.com/Motomura_Junya_GuiltyGearXrd.pdf; GDC Vault: https://gdcvault.com/play/1022031/GuiltyGearXrd-s-Art-Style-The
- [BP21] (J) GameBusiness.jp, Blue Protocol CEDEC 2021 implementation: https://www.gamebusiness.jp/article/2021/09/13/19051.html
- [BP21b] (J) GameBusiness.jp, Blue Protocol character creation: https://www.gamebusiness.jp/article/2021/08/27/18980.html
- [HFR1] (J) GCORES, Hi-Fi Rush GDC 2024 commentary, part 1: https://www.gcores.com/articles/181912
- [HFR2] (J) GCORES, Hi-Fi Rush GDC 2024 commentary, part 2: https://www.gcores.com/articles/182264
- [HFR-GDC] (P, abstract) https://schedule.gdconf.com/session/3d-toon-rendering-in-hi-fi-rush/899802
- [IM24] (J) CGWORLD, Idolmaster CEDEC 2024: https://cgworld.jp/article/202410-cedec-imas.html
- [GBF] (J) CGWORLD, GBF Relink cutscenes: https://cgworld.jp/article/202405-cgw309-relink04.html
- Unity Toon Shader:
  - [UTS-AR] (P, docs) Angel Ring: https://docs.unity3d.com/Packages/com.unity.toonshader@0.13/manual/AngelRing.html
  - [UTS-SRC] (P, source) https://github.com/Unity-Technologies/com.unity.toonshader/blob/master/com.unity.toonshader/Runtime/Shaders/BuiltIn/UCTS_ShadingGradeMap.cginc and https://github.com/Unity-Technologies/com.unity.toonshader/blob/master/com.unity.toonshader/Runtime/Shaders/BuiltIn/UCTS_Outline.cginc

**Face SDF tooling**
- [NAGA] nagakagachi, "SDF Based Transition Blending for Shadow Threshold Map" (2024-03-02): https://nagakagachi.hatenablog.com/entry/2024/03/02/140704
- [AKA] akasaki1211/sdf_shadow_threshold_map: https://github.com/akasaki1211/sdf_shadow_threshold_map
- [ASP] EricHu33, Anime Shading Plus face shadow map workflow: https://github.com/EricHu33/AnimeShadingPlus-Anime-Toon-Shader/blob/main/Anime%20Shading%20Plus(+)%20User%20Manual%20e9875988ae1e41caa5198370d9cc963d/Face%20Shadow%20Map-%20Creation%20&%20Baking%20Workflow%20d3b8769021e04683a2f2ae4cf16ac810.md
- [SDFUE] yespure/SDFGeneratorUE: https://github.com/yespure/SDFGeneratorUE
- [8SSEDT] zqsd/8ssedt: https://github.com/zqsd/8ssedt
- [FENG] fenglvv SDF tool: https://fenglvv.github.io/pages/SDF.html
- [BNPR] BNPR Abnormal, sphereize normals: https://bnpr.gitbook.io/bnpr/shaders/toon-shading-nbpr-abnormal-normal-sphering

**Misc**
- [INZOI] inZOI modkit stocking docs: https://mod-docs.playinzoi.com/docs/modkit-docs/modkit/project/caz/stocking/01-overview
- [PRIOR] Earlier Sapphire research notes: `research_notes/Rust anime action game engine/rendering_and_gpu.md`. Its cited sources include the Polycount GGXrd posts (https://polycount.com/discussion/comment/2186979), the Delt06 outline wiki (https://github.com/Delt06/urp-toon-shader/wiki/Outline) and the Tencent GWB notes (https://gwb.tencent.com/community/detail/121023).
