# M1 research: test content, golden-image CI (Q6), crate status

Researched 2026-10-24. Every license and version claim has a source tag `[Sn]`. The sources are listed at the end. Quotes are kept under 15 words. "Verified" means I read the primary source, the file's own metadata, or crates.io / GitHub API data. Wherever I could not reach a primary source, the note says so.

---

## TL;DR

**Test content (public repo)**
- **Best characters:** the VRM Consortium samples in `vrm-c/vrm-specification`.
  - `VRM1_Constraint_Twist_Sample`: pixiv, VRM Public License 1.0. Its embedded meta allows redistribution and modified redistribution, and credit is not required.
  - `Seed-san`: VirtualCast, VPL 1.0. Same permissions, but **credit is required**.
  - `VRMC_materials_mtoon_UV_Animation_Test`: 61 KB, for UV-animation goldens.
  - Both characters ship MToon 1.0 materials with shade, normal, outline-width, and (Seed-san only) matcap and shading-shift textures. **Neither ships a ramp, ILM, or SDF face map**, so we author those ourselves.
- **Good backups:** VRoid's old CC0 samples (AvatarSample_D–G and the HairSamples; VRM 0.x), and VRoidPreset A–Z / AvatarSample_A–C. The A–C presets are not CC0, but free redistribution is allowed and credit is not required.
- **Avoid:**
  - Unity-chan: UCL 3.0. The license is inherited by recipients and can be revoked at the company's discretion. It ships falloff ramp textures, but it is a poor fit for an MIT/Apache repo.
  - Query-chan: Unity Asset Store EULA, so no standalone redistribution.
  - Alicia Solid: needs a niconico account, and sublicensing is barred.
- **Environments:** Kenney, Poly Haven, ambientCG and KayKit's free packs are CC0.
  - **Quaternius is no longer CC0.** The Quaternius Asset License v1.0, updated 2026-08-28, forbids redistributing the assets themselves, so do not commit new Quaternius downloads.

**Golden images (Q6)**
- **Yes:** GitHub's standard macOS arm64 runners expose a Metal adapter called "Apple Paravirtual device".
  - wgpu runs its whole GPU test suite on it, image comparisons included, on every PR. Bevy runs Metal screenshot jobs on it too.
  - **Limits:** Metal shader validation is broken, a few known paravirtual bugs exist, and the runner has 3 vCPU and 7 GB RAM.
  - `macos-14` retires on 2026-11-02, so target `macos-26`. wgpu itself is still on `macos-14`.
- **Recommended setup:**
  - Linux: lavapipe from a pinned Mesa tarball, as wgpu does with Mesa 26.1.3.
  - Windows: WARP from a pinned NuGet package (`Microsoft.Direct3D.WARP`, latest 1.0.21), plus `static-dxc`.
  - macOS: `macos-26` Metal, non-blocking at first.
  - Comparison: one golden set per backend, compared with nv-flip using mean plus percentile thresholds, like wgpu's `ComparisonType`.

**Crate problems**
- **ozz-animation-rs** needs **nightly Rust** (`#![feature(portable_simd)]`). Its 0.11.0 release uses glam 0.30, while Bevy uses glam 0.33. It is runtime-only, and a large refactor is in progress on master. This mostly affects M2.
- **wgsl_bindgen** 0.23 is built on **naga_oil, which is archived**. It does not understand WESL imports. The WESL port PR was closed unmerged.
- **encase** 0.12+ dropped its glam integration. Use glam's `encase-013` feature, available in glam 0.33.9 and later.
- **ctt** is 7 months old. It has an open BC7 mip-chain bug (#114) and no HDR ASTC support.
- Everything else (bevy_ecs/bevy_reflect 0.20, rkyv 0.8.18, ktx2 0.5, meshopt 0.7, notify 8.2.1, wesl 0.6, egui/egui-wgpu 0.37) is current and wgpu-30 compatible.
- wgpu's own v31 milestone was due 2026-10-09, so v31 is imminent.

---

## 1. Test content for an anime look-dev scene

### 1.1 Characters: summary

| Candidate | License | Public-repo redistribution | Attribution | Anime-shading textures shipped | Verdict |
|---|---|---|---|---|---|
| **VRM1_Constraint_Twist_Sample** (pixiv) | VRM Public License 1.0 [S1][S3] | **Yes**: meta `allowRedistribution: true`, `modification: allowModificationRedistribution` [S4] | `creditNotation: unnecessary` [S4] | MToon 1.0: shade-multiply (13 materials), normal (3), outline-width (1), emissive (1). No matcap, rim, shading-shift, ILM or SDF [S4] | **Primary** |
| **Seed-san** (VirtualCast) | VPL 1.0 [S2][S3] | **Yes**: same flags [S4] | **`creditNotation: required`** [S4] | MToon 1.0 + PBR mix: shade (9), normal (5), shading-shift (1), matcap (2), outline-width (2), emissive (1). Also spring bones, rotation constraints, UV-transform expressions [S4][S5] | **Secondary** (keep the credit file) |
| **VRMC_materials_mtoon_UV_Animation_Test** (pixiv) | VPL 1.0 [S5] | Same repo and license [S5] | Not read (the file was not inspected) | Three quads with different UV-animation settings [S5] | **Use** for UV-animation goldens |
| VRoid CC0 samples: AvatarSample_D–G, HairSample_Male/Female (old alpha/beta VRoid) | **CC0** (2022 archived help page) [S6] | Yes | No | VRM 0.x / MToon 0.x. Not inspected | Backup. Exercises the VRM0 → VRM1 migration |
| VRoidPreset A–Z (incl. AvatarSample_A–C on VRoid Hub) | pixiv custom terms, "not CC0" [S7] | Free redistribution appears allowed; **for a fee** is prohibited [S7] | "no need to credit the original creator" [S7] | VRoid MToon export, the same family as the Twist sample. Not inspected | Backup. Downloading needs a VRoid Hub account |
| Unity-chan (UTJ), e.g. the official `unitychan-crs` repo | Unity-chan License Terms **3.0** (2024-05-20) [S8][S9] | Allowed, but recipients inherit UCL and license files must be bundled (Art. 3.4). **Revocable at any time** (Art. 6.2) [S8] | Logo or license notation required (Art. 3.3) [S8] | **Yes, ramps:** `FO_SKIN1`/`FO_CLOTH1` falloff ramps, `FO_RIM1` rim ramp, `ENV2` env map, `_NRM`, `_SPEC`. FBX plus Unity CG shaders. No SDF [S9] | **Avoid in repo**. A local-only download script is OK |
| Query-chan (Pocket Queries) | Unity Asset Store EULA [S10][S11] | **No.** The EULA allows distribution only "as incorporated and embedded in that Licensed Product" [S11] | Logo requested [S10] | Not checked | **Reject** |
| Alicia Solid / ニコニ立体ちゃん (Dwango) | Its own character license [S12] | **No explicit grant.** Sublicensing barred: 「第三者に再許諾することはできない」 [S12] | none stated | Not checked | **Reject** |
| Ripped HoYoverse / Kuro models (Genshin, HSR, ZZZ, WuWa) | Proprietary | No | — | ILM + SDF (that is why people rip them) | **Never** |

### 1.2 Details

**VRM Consortium samples** (`github.com/vrm-c/vrm-specification/samples`) [S1][S2][S5]
- The repo itself has no LICENSE file. Each sample README says "VRM Public License 1.0".
  - Twist sample: "(c) 2022 pixiv Inc.". Seed-san: "Seed-san model by VirtualCast, Inc.".
- I read each file's embedded `VRMC_vrm.meta` with a range request into the GLB JSON chunk [S4]:
  - Twist: `allowRedistribution: true`, `modification: allowModificationRedistribution`, `commercialUsage: corporation`, `avatarPermission: everyone`, `creditNotation: unnecessary`, `allowAntisocialOrHateUsage: false`.
  - Seed-san: the same, but `creditNotation: required` and all "allow*Usage" flags true.
- **Why VPL matters** [S3]:
  - The license scopes only "Model Use", "Avatar Use" and acts the License Settings enable.
  - "Model Use" means "showing of a series of images obtained by Manipulating the 3D avatar". Golden renders are a reasonable fit.
  - Cooked engine formats are "Adapted Work Data", so both samples need `allowModificationRedistribution`, and they have it.
  - Keep a `NOTICE` with credit, the license link, and "modified" notes (VPL §3(a)). This is mandatory for Seed-san and harmless for Twist.
- Sizes: about 10.8 MB and 10.9 MB [S2]. That is fine under GitHub's limits: a warning at 50 MiB, a hard block at 100 MiB, and repos ideally under 1 GB [S40].

**VRoid samples** [S6][S7]
- The 2022 archived pixiv help page splits the samples in two:
  - AvatarSample_A/B/C have "particular conditions of use".
  - "The VRoid Studio models listed below are CC0 licensed": HairSample_Male/Female and AvatarSample_D/E/F/G [S6].
- An OpenGameArt mirror re-hosts the CC0 set and notes that D–G "are unrelated to the samples with the same names in the current version" [S13]. **Record exact provenance** (file plus SHA-256) for anything taken from it.
- The current "VRoidPreset_A - Z" page (updated 2024-12-26, archived 2025-12) says:
  - "The license to these sample models is not CC0."
  - Allowed examples include "Edit its texture and parameters, upload it to VRoid Hub, and redistribute it."
  - Prohibited: setting the conditions to CC0 on redistribution; using the data for a "character creation service"; and redistributing the files "in exchange for a fee". Terms "may be subject to change" [S7].
- An engine test-asset folder is not a character-creation service, but this is a custom, changeable license. **Prefer the VPL samples above.**
- The live pixiv help site is behind a Cloudflare challenge, so I used Wayback snapshots [S6][S7]. **Verify the live text** before committing.

**Unity-chan** [S8][S9]
- The UCL 3.0 text I read contains no clause limiting use to the Unity engine. Data repurposed for non-character uses falls under the Unity Companion License instead (Art. 1(3)).
- What makes it unsuitable for our repo:
  - Redistribution must be "distributed together with a set of separately specified license-related files" (Art. 3.4).
  - Recipients inherit UCL (Art. 3.2(1)).
  - Use "can be discontinued at any time, at The Company's discretion" (Art. 6.2), with deletion duties (Art. 6.3). Git history makes deletion painful.
  - AI-training use is banned (Art. 3.2(7)).
- Technically it is attractive. UTJ's own public repo `unity3d-jp/unitychan-crs` ships the model as FBX, with ramp textures `FO_SKIN1`, `FO_CLOTH1`, `FO_RIM1` and `ENV2`, and a shader sampling `_FalloffSampler`, `_RimLightSampler` and `_SpecularReflectionSampler` [S9].
- I found no official FAQ on non-Unity engines. **Verify with UTJ** if we ever want it.

**Other anime-style CC0/CC-BY characters**
- I found nothing better than the VRM samples with a clear license.
- Sketchfab and BOOTH have many anime models, each under per-item terms. Each would need its own check, and none is verified here.

### 1.3 Environment kits

| Source | License | Redistribution | Attribution | Notes |
|---|---|---|---|---|
| **Kenney** | CC0: "all game assets on the asset pages are public domain licensed (CC0)" [S14] | Yes (CC0) | Not required [S14] | Flat-color modular kits (city, nature, etc.). Good for outline, ramp and shadow tests |
| **Poly Haven** | CC0: "Our assets are all licensed as CC0" [S15] | "You can redistribute them" [S15] | Not required [S15] | HDRIs for sky and IBL, PBR textures, models. Their ToS bans scraping, so download by hand [S15] |
| **ambientCG** | CC0 1.0 [S16] | Yes [S16] | Not required [S16] | PBR textures |
| **KayKit** (Kay Lousberg) | Free packs are listed as CC0 on OpenGameArt (Medieval Builder, Mini-Game Variety, Dungeon, …) [S17] | Yes for CC0 packs | No | Paid itch.io "Bits"/extra packs: license **not verified**. Check each pack |
| **Quaternius** | **Quaternius Asset License v1.0**, updated 2026-08-28, *not* CC0 [S18] | **No:** "You just can't resell or redistribute the assets themselves as assets." Applies "whether for free or for payment" [S18] | Not required | QAL §7: earlier versions govern assets obtained earlier [S18]. Pre-change CC0 copies stay CC0, but provenance must be provable. **Do not commit new downloads.** The "Stylized Nature MegaKit" is tagged "Ghibli" [S19] and is fine to use locally |
| Khronos glTF-Sample-Assets | Per-model (many CC-BY 4.0 / CC0) [S20] | Per model | Per model | glTF conformance, not anime |

A real anime town or interior kit with a clean license does not exist in these sources. Plan to author one, or commission it. Ramp, ILM and SDF maps must be authored for every character anyway.

### 1.4 Suggested repo hygiene for third-party content

These are recommendations.
- Put content under `assets/third_party/<name>/`, alongside:
  - the upstream license text, or a link plus the VPL meta dump;
  - `SOURCE.md` with the URL, retrieval date, SHA-256, license name, and a credit line;
  - a `MODIFIED.md` listing derived files, such as cooked KTX2 files and authored SDF/ILM maps (VPL §3(a)(1)(B)).
- Keep Unity-chan and other revocable or inheritable licenses out of git. Fetch them with a script into an ignored directory.

### 1.5 VRM 1.0 and MToon → Sapphire's ramp/ILM/SDF pipeline

**Format.** VRM 1.0 is glTF 2.0 plus these extensions:
- `VRMC_vrm` (meta, humanoid, expressions, lookAt, firstPerson);
- `VRMC_springBone`;
- `VRMC_node_constraint`;
- `VRMC_materials_mtoon`;
- `VRMC_materials_hdr_emissiveMultiplier`;
- `VRMC_vrm_animation` (VRMA) [S21].

The sample files also use `KHR_materials_unlit`, `KHR_texture_transform` and `KHR_materials_emissive_strength` [S4]. VRM 0.x models face the opposite way: three-vrm rotates VRM0 scenes by π around Y [S22]. VRM 0.x also stores MToon as Unity material properties, so it needs a migration step, like UniVRM's `MigrationMToonMaterial.cs` (MIT) or three-vrm's `three-vrm-materials-v0compat` (MIT) [S22][S23].

**MToon 1.0 model** (from the spec [S24]):
- Lighting:
  - `shading = linearstep(-1+toony, 1-toony, N·L + shadingShiftFactor + shadingShiftTex*scale)`
  - `color = lerp(shadeColorFactor*shadeMultiplyTex, baseColor, shading) * lightColor`
- GI: `gi = lerp(rawGi(n), (rawGi(up)+rawGi(down))/2, giEqualizationFactor)`.
- Rim: `matcap*matcapFactor + pow(saturate(1-N·V+lift), max(power, eps))*rimColor`. This is multiplied by `rimMultiplyTexture`, then by `lerp(1, lighting, rimLightingMixFactor)`, and added.
- Outline:
  - `outlineWidthMode` is none, worldCoordinates (meters) or screenCoordinates (ratio of screen height).
  - Width is multiplied by `outlineWidthMultiplyTexture`.
  - The color is mixed with the surface result by `outlineLightingMixFactor`.
  - It is drawn as a separate pass after skinning.
- UV animation: scroll X/Y in UV units per second and rotation in rad/s, both multiplied by `uvAnimationMaskTexture`. Rotation is applied around (0.5, 0.5) before `KHR_texture_transform`. It affects the base, shade, rim, outline-width, emissive and normal textures, not matcap.
  - Spec oddity: the affected-texture list names `shadingToonyMultiplyTexture`, which is not defined anywhere else in the spec [S24].

**Mapping onto our ramp/ILM/SDF recipe.** This is my proposal, derived from the formulas above.

| MToon parameter | Sapphire equivalent | Notes |
|---|---|---|
| `baseColorTexture/Factor` | Lit albedo | sRGB texture, linear factor |
| `shadeColorFactor` × `shadeMultiplyTexture` | "Shadow color" map (1st-shade map in UTS terms) | Keep it per-texel. Do not bake it into a ramp row |
| `shadingShiftFactor`, `shadingToonyFactor` | **Ramp curve**: a 1D ramp texel row per material with center `-shift`, width `2(1-toony)` (`linearstep`) | Lossless if the ramp only stores the lit/shade weight, and colors stay in the textures. Our ramp can do multi-band, which MToon can't. Importing gives a 2-band ramp |
| `shadingShiftTexture` (+`scale`) | **ILM "shadow threshold / AO bias" channel** | Same role: a per-texel offset of the terminator |
| `giEqualizationFactor` | Ambient/SH flattening term | We need this anyway for anime. Use the same formula |
| `parametricRim*`, `rimMultiplyTexture`, `rimLightingMixFactor` | Fresnel rim fallback, rim-mask channel (ILM-like), light-dependence lerp | Our primary rim may be a screen-space depth rim. Keep MToon's formula as the "parametric" mode |
| `matcapTexture`, `matcapFactor` | Additive matcap slot (hair or metal highlights) | Use the spec's view-basis UV (`worldViewX/Y`, `*0.495+0.5`) for parity |
| `outlineWidthMode/Factor` | Inverted-hull outline: world width (m) or screen width | Our M0 outline normals (baked or `_OUTLINE_NORMAL`) still apply |
| `outlineWidthMultiplyTexture` | Per-vertex outline-width attribute (bake at cook time) or a vertex-shader texture fetch | Baking to vertex color avoids a VS texture fetch on mobile |
| `outlineColorFactor`, `outlineLightingMixFactor` | Outline color, lit/unlit mix | — |
| `uvAnimation*` + mask | Material UV-animation block (scroll/rotate × mask) | Driven by time. In goldens, freeze time |
| `transparentWithZWrite`, `renderQueueOffsetNumber`, alphaMode | Blend mode + sort key bias | — |
| *(none)* | **SDF face shadow, ILM specular masks** | MToon has no face SDF. Treat face materials as an override: the SDF threshold replaces `N·L`, and toony maps to the SDF smoothing width |

**Importer feasibility.** Reasonable on top of the `gltf` crate.
- `gltf` 1.4.1 has the `extensions` feature plus `KHR_texture_transform`, `KHR_materials_unlit` and `KHR_materials_emissive_strength` features [S25].
- **`vrm-spec`** 0.1.1 is pixiv's official Rust crate, Apache-2.0. It provides serde types for VRM 0.0, VRMC_vrm 1.0, MToon 1.0, springBone 1.0 and VRMA 1.0, and its README shows `gltf` + `doc.extension_value(VRMC_VRM)` [S26]. It has no `VRMC_node_constraint` module; its `src/` lacks one [S26].
- Other crates:
  - `serde_vrm` 0.5 / `bevy_vrm` 0.5 (unavi, Apache-2.0; Bevy 0.20) [S27].
  - `bevy_vrm1` 0.10 (Apache-2.0; VRM 1.0 only; spring bones and all three node-constraint types; Bevy 0.19) [S27].
  - `vrm-runtime` 0.1 (new, about 90 downloads) [S27].
  - `bevy_shader_mtoon` (WGSL MToon) [S27].
- Recommendation: use `vrm-spec` for the data model, write our own cooker-side mapping (about 1–2 kLOC including VRM0 migration), and use three-vrm's MToon (MIT) as the shading reference.

### 1.6 SDF face-shadow maps from threshold masks

**Method.** This is the widely shared one, as written up by nagakagachi [S28] and implemented by akasaki1211 [S29].
1. Author or bake N binary masks, one per light yaw from 0° (front) to 180°, each a superset of the previous one.
2. Compute a **signed** distance field per mask, because interior distance is needed.
3. For each consecutive pair, `t = SDF_i / (SDF_i + SDF_{i+1})` inside the region that changes between the two masks.
4. Write `lerp(threshold_i, threshold_{i+1}, t)` into the map.

The result is C0-continuous at pair boundaries. C1 would need neighboring frames [S28]. Distance transforms: 8SSEDT [S30], or jump flooding as in nagakagachi's UE tool [S28].

**Tools**

| Tool | License | What it does |
|---|---|---|
| `akasaki1211/sdf_shadow_threshold_map` [S29] | MIT, Python + OpenCV, 128★ | Mask folder → 8/16-bit threshold map; optional blur |
| `ReefSnax/blender-sdf-face-shadow-baker` [S31] | MIT | Cycles bakes a sun sweep into UV-space binary masks, then feeds the tool above |
| `yeczrtu/QuickSDFTool` (UE 5.7) [S32] | MIT | Paint masks on a mesh, live preview, CPU threshold bake. "Development has ended" |
| `yeczrtu/QuickSDFBlender` [S33] | **GPL-3.0** | Blender successor. Fine as an external tool, but don't vendor it |
| `nagakagachi/NagaSdfTextureToolForUE` [S28] | Not checked | UE editor utility for the same method |

**Proposal.** Port the method into `sapphire_pipeline`: an exact EDT plus the pairwise blend, with R16 output, a monotonicity check (as in QuickSDFTool's `QuickSDFMonotonicGuard` [S32]) and a mask-sequence importer. Bake the masks in Blender with ReefSnax's script for the Twist sample's face. This makes our first test asset with a real SDF map, and VPL allows the derived maps.

---

## 2. Golden-image tests on GitHub-hosted runners (Q6)

### 2.1 How wgpu does it

wgpu's `gpu-test` job runs on `windows-2022`, `macos-14` and `ubuntu-24.04` [S34].
- **Linux:**
  - Installs the Vulkan SDK 1.4.357 (loader and layers) and **Mesa 26.1.3 from `gfx-rs/ci-build` release `build29`**.
  - Writes its own ICD JSON pointing at `libvulkan_lvp.so`.
  - Exports `VK_DRIVER_FILES`, `LD_LIBRARY_PATH` and `LIBGL_DRIVERS_PATH`.
  - Sets `LVP_POISON_MEMORY=true` [S34][S35][S37].
- **Windows:**
  - Installs DXC v1.9.2602.24 (`WGPU_DX12_COMPILER`).
  - Installs **WARP 1.0.20 from NuGet**, copied next to the test binaries.
  - Installs the Agility SDK, plus Mesa's lavapipe (`mesa-dist-win`) for Vulkan [S34][S35][S36].
- **macOS:** Metal on the runner GPU. It also installs the Vulkan SDK (MoltenVK) [S34][S35].
- **Comparison:**
  - One reference PNG per test, compared with nv-flip.
  - Writes `<name>-<backend>-<adapter>-<driver>-actual.png` and `-difference.png`, uploaded as artifacts [S38].
  - Thresholds are per test: `ComparisonType::Mean(x)` or `Percentile{p, threshold}` [S38].

### 2.2 Does Metal work on GitHub's macOS arm64 runners?

**Yes, with caveats.**
- wgpu's GPU tests, Metal included, run on `macos-14`. The adapter reports as `"Apple Paravirtual device"`, an `IntegratedGpu`, and wgpu special-cases it [S39].
- Bevy's `run-examples-macos-metal` job also takes Metal screenshots on `macos-14` [S41].

**Known limits** (all sourced):
- Metal Shader Validation "is entirely broken in the paravirtualized CI environment". wgpu leaves `MTL_SHADER_VALIDATION` off when `GITHUB_ACTIONS=true` but keeps `MTL_DEBUG_LAYER` on [S39].
- Open bugs: depth / depth32-stencil8 clear tests segfault (wgpu #5016); `buffer_copy_offset` mis-copies on M1 CI (#6827, expected-fail on Paravirtual). Two benchmarks hang and are skipped [S39].
- `runner-images` #14840 (open, 2026-10-05): a heavy WebGL2 app stalls on macOS 15 and macOS 26 arm64 "paravirtual Metal" [S42].
- GitHub documents none of this. The runner docs only say "Nested-virtualization is not supported due to the limitation of Apple's Virtualization Framework." Standard arm64 macOS runners have **3 vCPU (M1), 7 GB RAM, 14 GB SSD** [S43].
- **`macos-14` retires 2026-11-02**, with brownouts during October 2026. GitHub points to `macos-latest` (= `macos-26`) or `macos-15` [S44].
- wgpu's `ci.yml` still says `macos-14` today [S34], so on `macos-26` we would be ahead of upstream.

### 2.3 Install steps

**Linux (ubuntu-24.04): lavapipe**
- **Option A (pinned; recommended for goldens).** Mirrors wgpu [S37]:
  ```bash
  sudo apt-get install -y --no-install-recommends libvulkan1   # loader (or install the Vulkan SDK for validation layers)
  curl -L --retry 5 -o mesa.tar.xz \
    https://github.com/gfx-rs/ci-build/releases/download/build29/mesa-26.1.3-linux-x86_64.tar.xz
  mkdir mesa && tar xpf mesa.tar.xz -C mesa
  cat > icd.json <<EOF
  {"ICD":{"api_version":"1.4.348","library_arch":"64",
   "library_path":"$PWD/mesa/lib/x86_64-linux-gnu/libvulkan_lvp.so"},"file_format_version":"1.0.1"}
  EOF
  echo "VK_DRIVER_FILES=$PWD/icd.json" >> "$GITHUB_ENV"
  echo "LD_LIBRARY_PATH=$PWD/mesa/lib/x86_64-linux-gnu/:$LD_LIBRARY_PATH" >> "$GITHUB_ENV"
  ```
  - `gfx-rs/ci-build` releases (build27–29, 2026-07) each carry one Mesa tarball [S45]. The repo has no license file. To avoid depending on another project's release lifetime, fork it, or build Mesa in our own workflow and attach the result to a release.
- **Option B (apt; drifts).**
  ```bash
  sudo apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1
  ```
  - noble-updates currently ships **Mesa 25.2.8** (published 2026-10-02) [S46]. The package installs `usr/share/vulkan/icd.d/*.json` and `usr/lib/*/libvulkan_*.so` [S47]; the ICD is `lvp_icd.x86_64.json`.
  - Ubuntu SRUs can change lavapipe output, and therefore our goldens.
  - Bevy uses this route via the kisak/turtle PPA [S41].
- `VK_DRIVER_FILES` is the Mesa- and loader-documented override [S48]. On a GPU-less runner, lavapipe is the only ICD anyway.

**Windows (windows-2025): WARP**
- WARP ships with Windows. Bevy's DX12 job runs on a GPU-less `windows-latest` runner with `statically-linked-dxc` [S41], so its adapter must be the system WARP. That last step is my inference.
- For reproducible goldens, pin the NuGet package as wgpu does [S36]:
  ```bash
  curl.exe -L --retry 5 -o warp.zip https://www.nuget.org/api/v2/package/Microsoft.Direct3D.WARP/1.0.21
  tar -xf warp.zip build/native/bin/x64/d3d10warp.dll
  cp build/native/bin/x64/d3d10warp.dll target/debug/        # next to the exe that runs
  cp build/native/bin/x64/d3d10warp.dll target/debug/deps/   # for cargo test / nextest binaries
  ```
  - NuGet versions: 1.0.20 (wgpu's pin, 2026-05-28), **1.0.21 (2026-09-22)**, and 1.65535.20-preview [S49].
  - License: "Purpose – for testing purposes only". It does not allow sharing or publishing the software [S50]. **Download it in CI. Never commit the DLL.**
- Select the adapter with `force_fallback_adapter: true` or `WGPU_ADAPTER_NAME`, and `WGPU_BACKEND=dx12` [S51].
- Shader compiler: enable wgpu's `static-dxc` feature [S52], or set `WGPU_DX12_COMPILER`. Without DXC, wgpu falls back to FXC [S51].

**macOS (macos-26)**
- Nothing to install. Set `WGPU_BACKEND=metal` and expect the adapter name "Apple Paravirtual device" [S39].

### 2.4 Image-comparison crates

| Crate | Version / date | License | Build | Notes |
|---|---|---|---|---|
| **`nv-flip`** (gfx-rs) | 0.1.2, 2023-07-16 [S53] | (MIT OR Apache-2.0 OR Zlib) AND BSD-3-Clause [S54] | `nv-flip-sys` compiles C++17 via `cc` [S54] | Repo maintained (Renovate). CI builds and tests on **windows-2025, macos-15, ubuntu-24.04** [S54]. Bundles NVlabs/flip at a 2023-05-19 commit [S55]. LDR-FLIP only (per flip-rs README [S56]). Used by wgpu [S38] |
| `flip-rs` | 0.1.2, 2026-10-02 [S53] | BSD-3-Clause [S56] | Pure Rust, no `unsafe`, wasm OK [S56] | FLIP v1.7, LDR + HDR-FLIP. Claims C++ parity on a 3,000-case sweep [S56]. **Created 2026-10-01, single author, 0★**. Promising, immature |
| `image-compare` | 0.5.0, 2025-08 [S53] | MIT | Pure Rust | SSIM / RMS / hybrid |
| `ssimulacra2` | 0.5.1, 2024-12 [S53] | BSD-2-Clause | Pure Rust | Perceptual, compression-oriented |
| `butteraugli` | 0.9.3, 2026-05 [S53] | BSD-3-Clause | Pure Rust | Perceptual |
| `dify` | 0.8.0, 2026-01 [S53] | MIT | Pure Rust | pixelmatch-style pixel diff |
| `dssim-core` | 3.5.1 [S53] | **AGPL-3.0** | — | Avoid (matches the spec's note) |

**Recommendation.** Keep `nv-flip` for M1: it is battle-tested in wgpu and builds on all three runners. Wrap it behind a `sapphire_testkit::compare` trait so we can switch to `flip-rs` once it matures, which would also give HDR-FLIP for pre-tonemap buffers.

### 2.5 Tolerance strategies

**What wgpu does** [S38]:
- It keeps a single reference image per test across all backends and devices.
- It uses FLIP **mean** thresholds of about 0.016–0.065 (e.g. cube 0.041 "Bounded by Apple A9", shadow 0.026, water 0.018, skybox 0.02).
- It adds **percentile** gates for thin features: cube-lines p95 ≤ 0.36; msaa-line p50 ≤ 0.29 "to differentiate real failures from variance".
- It uses an expectations system (`expect_fail` / `skip` per backend and adapter substring) for known driver bugs [S39].

**Bevy** uploads per-OS screenshots to an external service, Pixel Eagle, instead of in-repo thresholds [S41].

**Proposal for Sapphire.** These starting values are mine, extrapolated from the above.
- **Per-backend goldens:** `golden/<scene>/<pass>.{lavapipe,warp,metal-pv}.png`, each with tight thresholds (mean ≤ ~0.01, p99 gate). Add one loose cross-backend check against the lavapipe image (mean ≤ ~0.05) to catch backend-only bugs.
- **Hard-edge failure mode:** hard ramp, SDF and outline edges amplify tiny precision differences into whole-pixel band flips along terminators.
  - Gate on high percentiles (p95/p99) and an error-pixel-count budget, not on max.
  - Add **intermediate-buffer goldens**, such as the shade-band index, the SDF threshold result and outline masks, with exact or near-exact compare where possible.
- **Determinism:** fixed time (which freezes MToon UV animation and wind), no TAA jitter or a seeded one, fixed exposure, no dithering, small fixed resolution (e.g. 640×360) for CPU rasterizers, and the M0 offscreen `--capture` mode.
- **Workflow:**
  - Upload `-actual` and `-difference` PNGs as artifacts on failure, as wgpu does.
  - `cargo xtask golden --update` regenerates the goldens for one backend.
  - Pin the Mesa and WARP versions in one place (an xtask constant plus action input, as wgpu does [S36][S37]). Bumping either means regenerating that backend's goldens.

### 2.6 Recommended CI matrix (answer to Q6)

| Runner | Backend / adapter | Install | Gate |
|---|---|---|---|
| `ubuntu-24.04` | Vulkan / lavapipe (Mesa pinned) | Option A above | **Required** |
| `windows-2025` | DX12 / WARP 1.0.21 (pinned) + `static-dxc` | NuGet step above | **Required** |
| `macos-26` | Metal / Apple Paravirtual device | none | **Informational** at first. Promote it after about two weeks without flakes |
| Self-hosted Mac (nightly, optional) | Metal / real Apple GPU, iOS sim | — | Real-GPU coverage. Hosted runners cannot do iOS |

This changes the spec's "Nightly: Golden images on Metal, self-hosted Mac" row: Metal goldens can run per PR on hosted runners.

---

## 3. Crate status (late October 2026)

Project toolchain: Rust **1.99.0** stable (`rust-toolchain.toml`). The latest stable is 1.99.1 (2026-10-16) [S57]. wgpu: **30.0.1** (2026-08-22) [S53]. The wgpu **v31 milestone was due 2026-10-09** [S58], and the README says a breaking release comes every three months [S51]. Expect v31 during M1, and stay on 30 per the pin policy.

| Crate | Latest (date) | MSRV | Status for M1 | Flags |
|---|---|---|---|---|
| `bevy_ecs` / `bevy_reflect` | **0.20.0** (2026-10-08); rc.1 2026-09-15; 0.19.1 2026-08-13 [S53] | 1.95 [S53] | **Current.** 0.20 is the latest. bevy_reflect has an optional `wgpu-types ^30` dep, and Bevy 0.20 itself uses wgpu 30, wesl 0.6, glam 0.33 and encase 0.12 [S53][S59] | See notable changes below |
| `rkyv` | 0.8.18 (2026-08-05) [S53] | 1.81 | Active (main still 0.8.18; recent hardening commits) [S60] | — |
| `ctt` | 0.6.0 (2026-09-27). First release 0.1.0 2026-03-09; 6 minor releases in 7 months [S53] | 1.90 | Usable but **young**. Encoders: bc7enc-rdo, BC7F, Intel ISPC, etcpak, Compressonator, astcenc. KTX2/DDS out; zstd/zlib supercompression [S61] | All backends build C/C++ via `cc`. ISPC kernels come **prebuilt** by default (`ispc-prebuilt`) for x86_64/aarch64 Linux, macOS and Windows; `ispc-build-from-source` exists [S61][S62]. Open issues: **#114 BC7 mip chain to 1×1 with random colour** (filed 2026-10-23), **#107 alpha-tested coverage in mips** (matters for hair and lashes), **#74 HDR ASTC unsupported**, #72/#33 3D textures [S63]. Author: cwfitzgerald (Connor Fitzgerald, a wgpu maintainer) |
| `ktx2` | 0.5.0 (2026-04-19) [S53] | 1.56 | Fine | **Reader only** (parse, validate, DFD; 0.5 added DFD generation) [S64]. Its only dependency is `bitflags`, so zstd supercompression decode needs a separate crate (`zstd`/`ruzstd`) at runtime [S53] |
| `meshopt` | 0.7.0 (2026-10-17) [S53] | 1.87 | Current. meshoptimizer **1.3**, edition 2024, clusterlod, tangent generation, meshlet codec [S65] | Vertex codec now defaults to format v1. The decoder needs meshoptimizer 0.23+ [S65]. C++ via `cc` |
| `notify` | **8.2.1** (2026-10-25 UTC, i.e. hours old) [S53] | **1.85** | Current stable line. `notify-debouncer-full` 0.7.0 requires `notify ^8.2.0` [S53] | 8.2.1 raised MSRV in a patch release ("breaking"), and macOS moved to `objc2-core-services` [S66]. 9.0.0-rc.6 is in RC [S66]. Stay on 8.2.x |
| `wesl` | 0.6.0 (2026-10-04) [S53] | **1.97.1** | Current. **Bevy 0.20 migrated to WESL** and dropped naga_oil [S59][S67]. WESL 0.2: imports, `@if/@elif/@else`, cargo shader packages [S68] | No naga dependency: it emits WGSL, and we validate with naga 30. Feature `naga-ext` adds naga-extension syntax [S53] |
| `encase` | 0.13.0 (2026-09-19) [S53] | 1.77 | Fine. 0.13 only bumps `syn` to v3 [S69] | **0.12 removed impls on foreign types** [S69]. Use glam's `encase-013` feature (glam ≥ 0.33.9; glam 0.34.x also has it) [S53]. Pin **glam 0.33.x** to match Bevy 0.20 (glam 0.34.1 is out) [S53] |
| `wgsl_bindgen` | 0.23.3 (2026-08-11) [S53] | — | **wgpu 30 compatible** (0.23.x ↔ wgpu 30) [S70] | **Problem:** it depends on `naga_oil ^0.23` [S53], which is **archived** ("archived and is now unmaintained … migrated to WESL") [S67]. It only understands naga_oil `#import` syntax [S70]. WESL migration issue #87 is open; PR #101 was **closed unmerged** [S71]. Options: run it on wesl-flattened WGSL (no imports), or replace it with in-house naga-reflection codegen, which fits the spec's "reflected with naga" |
| `egui` / `egui-wgpu` | 0.37.0 (2026-10-15) [S53] | 1.95 | **Compatible:** egui-wgpu 0.37 requires `wgpu ^30` [S53] | For M2: `egui_tiles` 0.18 and `egui_plot` 0.38 are on egui 0.37. **`egui_dock` 0.21.1, `transform-gizmo-egui` 0.11 and `egui-snarl` 0.12 are still on egui 0.36** [S53] |
| `ozz-animation-rs` | 0.11.0 (2025-10-14) [S53] | 1.88 (but nightly) | **Problem** | **Requires nightly**: `#![feature(portable_simd)]` at the v0.11.0 tag, glam `core-simd`, and `rust-toolchain` = `nightly-2026-09-15` [S72][S73]. 0.11.0 depends on **glam 0.30** (Bevy: 0.33) [S53]. Master is an unreleased 0.11.1 on glam 0.33, with a large refactor merged 2026-10-22..24 [S72]. **No offline tooling**: "All offline features (no plan, use C++ library instead)" [S72], which answers Q7: the cooker needs C++ `gltf2ozz`/`fbx2ozz`. A stable-Rust port to `wide` (PR #101) was closed in 2025 [S74]. MPL-2.0, one maintainer [S72] |
| `nv-flip` | 0.1.2 (2023-07-16) [S53] | 1.64 | Works (see §2.4) | Stale release, old FLIP, LDR only |
| `gltf` | 1.4.1 (2024-05-10) [S53] | 1.61 | Stable, slow-moving (last push 2026-05) [S53] | — |
| `naga_oil` (transitive only) | 0.23.0 | — | **Archived** [S67] | Only pulled in by wgsl_bindgen |

None of the planned direct dependencies is archived or abandoned. The archived crate (naga_oil) comes in transitively through wgsl_bindgen.

**Bevy 0.20 notable ECS/reflect changes** [S59][S75]:
- ECS:
  - Weak ordering (`chain_weak` / `before_weak` / `after_weak`), and several built-in schedules now order sets weakly.
  - `ScheduleBuildSettings::shuffle_seed` for order-randomized property tests.
  - Opt-in per-column change ticks (`#[component(summary_tick)]`).
  - Bulk `despawn_all`.
  - Panics in systems, commands and observers are converted to errors, sent to `FallbackErrorHandler`, which re-panics by default.
  - `BevyError::with_context`.
  - Exclusive function systems unified with regular ones.
  - `WorldQuery` has no default impls.
  - `FilteredResources` deprecated; `DeferredWorld::query` deprecated.
  - "many"-entity query iterators yield `Result`.
  - `Entity::PLACEHOLDER` → `Option<Entity>` in places.
  - `Access::reads_and_writes` → `reads`.
  - `ComponentInfo` no longer stores its ID.
  - Observer event generics moved.
- Reflect:
  - `FromType` → `CreateTypeData`.
  - `PartialReflect::to_dynamic` returns `Result`.
  - `ReflectFromPtr::as_reflect` → `ptr_as_reflect`, and `from_ptr(_mut)` → `raw_pointer_cast`.
  - Explicit TypeId map aliases.

---

## Open items to verify

1. Re-read the live pixiv help pages, which are behind Cloudflare; I used Wayback snapshots from 2022-10 and 2025-12 [S6][S7].
2. Check the Unity-chan non-Unity-engine stance with UTJ, if Unity-chan is ever wanted.
3. Run a 1-day spike on `macos-26`: wgpu 30 + our capture path on "Apple Paravirtual device". Nobody upstream has published results for macos-26 yet.
4. Decide on wgsl_bindgen: flatten first, or in-house codegen.
5. Decide on ozz-animation-rs (before M2): fork and port the SIMD layer to stable, or accept nightly in `sapphire_anim` only. Nightly conflicts with the pinned stable toolchain.

---

## Sources

- [S1] VRM1_Constraint_Twist_Sample README: https://github.com/vrm-c/vrm-specification/tree/master/samples/VRM1_Constraint_Twist_Sample
- [S2] Seed-san README and file list: https://github.com/vrm-c/vrm-specification/tree/master/samples/Seed-san
- [S3] VRM Public License 1.0: https://vrm.dev/en/licenses/1.0/
- [S4] Embedded `VRMC_vrm.meta` and materials, read from https://raw.githubusercontent.com/vrm-c/vrm-specification/master/samples/Seed-san/vrm/Seed-san.vrm and https://raw.githubusercontent.com/vrm-c/vrm-specification/master/samples/VRM1_Constraint_Twist_Sample/vrm/VRM1_Constraint_Twist_Sample.vrm (GLB JSON chunk only)
- [S5] VRM samples index and UV Animation Test README: https://github.com/vrm-c/vrm-specification/tree/master/samples
- [S6] pixiv help, "Do VRoid Studio's sample models come with conditions of use?" (Wayback 2022-10-03): https://web.archive.org/web/20221003010347/https://vroid.pixiv.help/hc/en-us/articles/4402614652569
- [S7] pixiv help, "VRoidPreset_A - Z" (updated 2024-12-26; Wayback 2025-12-11): https://web.archive.org/web/20251211214754/https://vroid.pixiv.help/hc/en-us/articles/4402394424089-VRoidPreset-A-Z
- [S8] Unity-chan License Terms Version 3.0 (Wayback 2024-11-30): https://web.archive.org/web/20241130185904/https://unity-chan.com/contents/license_en/ (live URL now redirects to https://unity3d.jp/unity-chan/license?lang=en)
- [S9] unity3d-jp/unitychan-crs (README_EN.txt, Textures, CharaMain.cg): https://github.com/unity3d-jp/unitychan-crs
- [S10] Query-chan download/license page (Wayback 2026-07-11): https://web.archive.org/web/20260711135749/http://query-chan.com/download/
- [S11] Unity Asset Store Terms of Service and EULA: https://unity.com/legal/as-terms
- [S12] ニコニ立体ちゃん license: https://3d.nicovideo.jp/alicia/rule.html
- [S13] OpenGameArt "VRoid Studio CC0 models": https://opengameart.org/content/vroid-studio-cc0-models
- [S14] Kenney support / FAQ: https://kenney.nl/support
- [S15] Poly Haven license: https://polyhaven.com/license
- [S16] ambientCG license: https://docs.ambientcg.com/license/
- [S17] KayKit on OpenGameArt (e.g.): https://opengameart.org/content/kaykit-medieval-builder-pack-10 , https://opengameart.org/content/kaykit-mini-game-variety-pack-12
- [S18] Quaternius Asset License v1.0 (updated 8/28/2026): https://quaternius.com/license.html
- [S19] Quaternius asset index: https://quaternius.com/
- [S20] Khronos glTF-Sample-Assets README: https://github.com/KhronosGroup/glTF-Sample-Assets
- [S21] VRM 1.0 overview: https://vrm.dev/vrm1/ ; spec tree: https://github.com/vrm-c/vrm-specification/tree/master/specification
- [S22] three-vrm `rotateVRM0` and packages (MIT): https://github.com/pixiv/three-vrm/blob/dev/packages/three-vrm/src/VRMUtils/rotateVRM0.ts , https://github.com/pixiv/three-vrm
- [S23] UniVRM VRM0→1 MToon migration (MIT): https://github.com/vrm-c/UniVRM/blob/master/Packages/VRM10/Runtime/Migration/Materials/MigrationMToonMaterial.cs
- [S24] VRMC_materials_mtoon-1.0 spec: https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_materials_mtoon-1.0/README.md
- [S25] gltf 1.4.1 on crates.io (features): https://crates.io/crates/gltf/1.4.1
- [S26] pixiv/vrm-utils-rs, `vrm-spec` crate: https://github.com/pixiv/vrm-utils-rs , https://crates.io/crates/vrm-spec
- [S27] bevy_vrm / serde_vrm: https://github.com/unavi-xyz/bevy_vrm ; bevy_vrm1: https://github.com/not-elm/bevy_vrm1 ; vrm-runtime: https://crates.io/crates/vrm-runtime ; bevy_shader_mtoon: https://crates.io/crates/bevy_shader_mtoon
- [S28] nagakagachi, "SDF Based Transition Blending for Shadow Threshold Map": https://nagakagachi.hatenablog.com/entry/2024/03/02/140704 ; tool: https://github.com/nagakagachi/NagaSdfTextureToolForUE
- [S29] akasaki1211/sdf_shadow_threshold_map: https://github.com/akasaki1211/sdf_shadow_threshold_map
- [S30] 8SSEDT tutorial: https://github.com/Lisapple/8SSEDT
- [S31] ReefSnax/blender-sdf-face-shadow-baker: https://github.com/ReefSnax/blender-sdf-face-shadow-baker
- [S32] yeczrtu/QuickSDFTool: https://github.com/yeczrtu/QuickSDFTool
- [S33] yeczrtu/QuickSDFBlender: https://github.com/yeczrtu/QuickSDFBlender
- [S34] wgpu CI workflow: https://github.com/gfx-rs/wgpu/blob/trunk/.github/workflows/ci.yml
- [S35] wgpu composite actions (install-mesa, install-vulkan-sdk, install-dxc, install-agility-sdk): https://github.com/gfx-rs/wgpu/tree/trunk/.github/actions
- [S36] wgpu `xtask install-warp` (WARP 1.0.20 via NuGet): https://github.com/gfx-rs/wgpu/blob/trunk/xtask/src/install_warp.rs ; https://github.com/gfx-rs/wgpu/blob/trunk/.github/actions/install-warp/action.yml
- [S37] wgpu install-mesa action: https://github.com/gfx-rs/wgpu/blob/trunk/.github/actions/install-mesa/action.yml ; testing docs (LVP_POISON_MEMORY): https://github.com/gfx-rs/wgpu/blob/trunk/docs/testing.md
- [S38] wgpu image comparison: https://github.com/gfx-rs/wgpu/blob/trunk/tests/src/image.rs ; example thresholds: https://github.com/gfx-rs/wgpu/blob/trunk/examples/features/src/cube/mod.rs , .../msaa_line/mod.rs , .../shadow/mod.rs , .../skybox/mod.rs , .../water/mod.rs
- [S39] wgpu paravirtual handling: https://github.com/gfx-rs/wgpu/blob/trunk/tests/src/native.rs ; https://github.com/gfx-rs/wgpu/issues/5016 ; https://github.com/gfx-rs/wgpu/issues/6827 ; benches/benches/wgpu-benchmark/renderpass.rs
- [S40] GitHub, About large files: https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-large-files-on-github
- [S41] Bevy example-run workflow: https://github.com/bevyengine/bevy/blob/main/.github/workflows/example-run.yml
- [S42] actions/runner-images #14840: https://github.com/actions/runner-images/issues/14840
- [S43] GitHub-hosted runners reference: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- [S44] GitHub changelog, macOS 14 retirement: https://github.blog/changelog/2026-10-01-github-actions-macos-14-runner-image-retirement ; macOS 26 GA: https://github.blog/changelog/2026-02-26-macos-26-is-now-generally-available-for-github-hosted-runners/
- [S45] gfx-rs/ci-build releases: https://github.com/gfx-rs/ci-build/releases
- [S46] Launchpad, mesa in noble (25.2.8-0ubuntu0.24.04.4): https://launchpad.net/ubuntu/+source/mesa
- [S47] Ubuntu noble-updates `mesa-vulkan-drivers.install`: https://git.launchpad.net/ubuntu/+source/mesa/plain/debian/mesa-vulkan-drivers.install?h=ubuntu/noble-updates
- [S48] Mesa environment variables (VK_DRIVER_FILES): https://docs.mesa3d.org/envvars.html
- [S49] NuGet Microsoft.Direct3D.WARP versions: https://www.nuget.org/packages/Microsoft.Direct3D.WARP
- [S50] WARP NuGet license: https://www.nuget.org/packages/Microsoft.Direct3D.WARP/1.0.21/license
- [S51] wgpu README (env vars, release cadence): https://github.com/gfx-rs/wgpu/blob/trunk/README.md
- [S52] wgpu 30.0.1 features (`static-dxc`): https://crates.io/crates/wgpu/30.0.1
- [S53] crates.io API (versions, dates, MSRV, licenses, dependencies), queried 2026-10-24: https://crates.io/api/v1/crates/<name> and /<name>/<version>/dependencies, for bevy_ecs, bevy_reflect, bevy_render, bevy_shader, rkyv, ctt, ktx2, meshopt, notify, notify-debouncer-full, wesl, encase, glam, wgsl_bindgen, naga_oil, egui, egui-wgpu, egui_dock, egui_tiles, egui_plot, transform-gizmo-egui, egui-snarl, ozz-animation-rs, nv-flip, flip-rs, image-compare, ssimulacra2, butteraugli, dify, dssim-core, wgpu, naga, gltf, syn. GitHub repo metadata via the GitHub API
- [S54] gfx-rs/nv-flip-rs (README, CI, nv-flip-sys build.rs): https://github.com/gfx-rs/nv-flip-rs
- [S55] NVlabs/flip commit pinned by nv-flip-sys: https://github.com/NVlabs/flip/commit/330a18fa7a487c3de9b86abb960309fdf9e646b6
- [S56] flip-rs README: https://github.com/Tavrin/flip-rs
- [S57] Rust releases: https://github.com/rust-lang/rust/releases
- [S58] wgpu milestones (v31): https://github.com/gfx-rs/wgpu/milestones
- [S59] Bevy 0.20 release notes: https://bevy.org/news/bevy-0-20/
- [S60] rkyv repo: https://github.com/rkyv/rkyv
- [S61] cwfitzgerald/ctt README: https://github.com/cwfitzgerald/ctt
- [S62] ctt CI and ISPC build workflows: https://github.com/cwfitzgerald/ctt/tree/main/.github/workflows
- [S63] ctt issues #114, #107, #74, #72, #33: https://github.com/cwfitzgerald/ctt/issues
- [S64] BVE-Reborn/ktx2 README and CHANGELOG: https://github.com/BVE-Reborn/ktx2
- [S65] meshopt-rs CHANGES.md (0.7.0): https://github.com/gwihlidal/meshopt-rs/blob/master/CHANGES.md
- [S66] notify releases: https://github.com/notify-rs/notify/releases
- [S67] bevyengine/naga_oil (archived): https://github.com/bevyengine/naga_oil
- [S68] wesl-rs README: https://github.com/webgpu-tools/wesl-rs
- [S69] encase CHANGELOG and issue #94: https://github.com/teoxoy/encase/blob/main/CHANGELOG.md , https://github.com/teoxoy/encase/issues/94
- [S70] wgsl_bindgen README (version matrix, naga-oil): https://github.com/Swoorup/wgsl-bindgen
- [S71] wgsl_bindgen issue #87 and PR #101: https://github.com/Swoorup/wgsl-bindgen/issues/87 , https://github.com/Swoorup/wgsl-bindgen/pull/101
- [S72] SlimeYummy/ozz-animation-rs (README, Cargo.toml, rust-toolchain, commits): https://github.com/SlimeYummy/ozz-animation-rs
- [S73] glam README (`core-simd` requires nightly): https://github.com/bitshifter/glam-rs
- [S74] ozz-animation-rs PR #101 (stable port with `wide`, closed): https://github.com/SlimeYummy/ozz-animation-rs/pull/101
- [S75] Bevy 0.19 → 0.20 migration guide: https://bevy.org/learn/migration-guides/0-19-to-0-20/
