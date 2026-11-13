# Where reflection probes live (M5 L0)

L0's spike, run on 2026-11-13. Unless a line says otherwise: an Apple M5 (24 GB), macOS 27.0, wgpu 30.0.1, the dev profile (dependencies at opt-level 3). "ours:" marks our own inference.

**Verdict: §3's cube arrays stand; its fallback changes.** Probes live in one 2D texture of six `Rgba16Float` layers a probe, roughness mips down to 8 texels, the same on every device. The shaders read it as a cube array where the device has them (`GpuCaps::cube_array_textures`), and as a 2D array of faces, the face picked in the shader, where it doesn't. That fallback keeps per-pixel probes, box parallax and the blend across a doorway, and differs from the cube array by at most 4 code values, only along the cube's edges in the rough mips. One nearest cube would cost as much to build and change the picture. Octahedral maps lose everywhere: worse at less memory, as good only at 2.7 times the memory, and the slowest lookup. The §3 row should read:

> Local cubes baked per time-of-day key, blended by the key, with the live sky added through a baked visibility. One texture of six layers a probe holds them on every device: a cube array where the device has them, a 2D array of faces where it doesn't (the iOS Simulator). No screen-space or planar reflections in M5.

**What L2 builds.** The cooker writes the same thing for every device: per probe and per key, six faces of mip 0 at the cook's size (128, 128, 64, 32). They go in `cube_texel_direction`'s face order and orientation (`probes.wgsl`), with linear RGB radiance and the sky visibility in alpha, and no mips, since the runtime prefilters. The format is L2's choice:
- `Rgba16Float` is 768 KiB a key at 128, 6 MiB a probe for the default day's eight keys.
- BC6H can carry the RGB on desktop, but has no alpha.
- ASTC HDR isn't enabled.

**What L3 builds.**
- **The texture:** one 2D texture of 6 × P layers, `Rgba16Float`, with mips from the tier's size down to 8 texels, and `STORAGE_BINDING | TEXTURE_BINDING` usage. The composing (keys and live sky) and the prefilter write it through per-mip 2D array storage views, as the sky's reflection cube is written today. The 256-layer floor caps P at 42, so L3 keeps a resident set around the camera.
- **The binding:** each family binds one view, in one texture slot, with `ambient_sampler`:
  - a `CubeArray` view where `GpuCaps::cube_array_textures` is set, read with one `textureSampleLevel`;
  - a `D2Array` view elsewhere, with the face picked by hand as in the committed test.
- **The shader source:** the cube array declaration must live in WGSL chosen when the program is assembled, because wgpu refuses it on the Simulator even unused. Everything before the lookup is shared: the probe pick, the box parallax and the roughness's mip.

## 1. Who has cube arrays

`GpuCaps::cube_array_textures` is wgpu's `DownlevelFlags::CUBE_ARRAY_TEXTURES` (committed, with tests). What sets it, in wgpu-hal 30.0.1:

| Backend | Source | Reports |
|---|---|---|
| Metal | `metal/adapter.rs:583-587`, `:760`, `:1339-1342`: `supportsFeatureSet` for iOS family 4 v1, tvOS family 1 v2 or macOS family 1 v1 | macOS: yes (run here). iOS Simulator: no (run here). ours: every phone that runs iOS 26 is A12 or later (family 5), so yes |
| Vulkan | `vulkan/adapter.rs:706`: `imageCubeArray` | Android emulator (gfxstream over MoltenVK on this Mac): yes, `imageCubeArray` 1 in `cmd gpu vkjson`. lavapipe: CI runs the test, which fails a CPU adapter without them |
| D3D12 | `dx12/adapter.rs:741`: `DownlevelCapabilities::default()`, every flag | WARP: yes |

**What wgpu does without them:** it makes a cube array view (wgpu-core never checks the flag), but refuses any shader module that declares a `texture_cube_array`, used or not: naga's `CUBE_ARRAY_TEXTURES` capability (`naga valid/type.rs:906-908`, set from the flag in `wgpu-naga-bridge lib.rs:123-124`). So the cube array lookup has to be in WGSL assembled only where the flag is set. A permutation constant can't remove a declaration. The survey's line on cube arrays is right but misses this.

The committed GPU test, `caps::gpu_tests::probe_layers_read_as_faces_everywhere_and_as_cubes_where_the_device_has_them` (sapphire_rhi), holds that storage. A compute pass writes 18 `Rgba16Float` layers (3 cubes) through a 2D array storage view. Every face is then read back two ways: through a 2D array view with the face picked by hand, on every device, and through a cube array view where the device has them. Cube *i* must be layers 6*i*..6*i*+6 in wgpu's face order. Without cube arrays, wgpu must refuse the cube array module, and a CPU adapter must have them. Ran: Metal on this Mac, both reads. A private iOS 26.3 Simulator (iPhone 17 Pro) under `simctl spawn`, which reports no cube arrays: the faces read passed, and wgpu refused the cube array module ("Capability CUBE_ARRAY_TEXTURES is required"). The `sapphire_api36` emulator (`-gpu host`, arm64) under `adb shell` with `SAPPHIRE_GPU_DEBUG=none`: both reads passed. The device runs used the routes M0 used, with the test binary built for each target.

## 2. The picture

A temporary harness (scratch, not kept) built a generated environment: a sky gradient, a 0.05 rad sun at 6, window bands at angles no face lines up with, a mullion, and floor tiles turned 20°. It baked the environment into a 512 reference cube and then filled each storage at the tier sizes. Mip 0 holds the environment averaged over 4×4 subsamples of each texel's own footprint. Mip *k* holds a GGX lobe (roughness *k*/(mips − 1), α = roughness²) of the reference, 256 filtered importance samples. The truth uses the same estimator at the exact direction, and the environment itself for the mirror. So every difference is the storage's.

It read each storage with explicit LOD at 2^20 uniform directions and at 2^18 each along the cube's edges, along the octahedral map's outer seams and along its diamond (the horizon). It also read a 512² view of a mirror floor in a 10 × 4 × 10 m box through box parallax, with the probe off centre. The error is per pixel, the largest channel, in sRGB 8-bit code values of the value clamped to 1. Octahedral maps are centred on +Y, with a texel of border on each side at every level, and are read as two bilinear samples with a manual lerp. The faces lookup clamps at each face's edge.

Mean / worst error against the truth, at tier size 128 (octahedral 256, 0.67× the memory):

| Directions | Level | Cube array | Faces (fallback) | Octahedral 256 |
|---|---|---|---|---|
| uniform | mirror | 5.00 / 161 | 5.00 / 161 | 6.17 / 178 |
| uniform | 0.5 (mirror-to-first blend) | 2.35 / 49 | 2.35 / 49 | 2.92 / 56 |
| uniform | roughness 0.5 | 0.14 / 1 | 0.15 / 2 | 0.18 / 2 |
| uniform | roughness 1 | 0.65 / 4 | 0.71 / 4 | 1.00 / 5 |
| cube edges | roughness 1 | 0.76 / 3 | 1.68 / 4 | 1.04 / 4 |
| octahedral seams | roughness 1 | 0.89 / 2 | 0.94 / 3 | 2.44 / 5 |
| parallax view | mirror | 5.05 / 159 | 5.05 / 159 | 5.89 / 164 |
| parallax view | roughness 1 | 0.79 / 4 | 0.83 / 4 | 1.09 / 5 |

At 64 and 32 the pattern holds, with errors 1.6× and 2.5× larger at the mirror and the same in the rough mips. An octahedral map twice the cube's side (256 for 128) is worse than the cube everywhere and shows its border as two lines in the rough mips, the largest of its errors there. Four times the side (512 for 128, 2.7× the memory) beats the cube: a mirror mean of 3.67 against 5.00, and 0.50 against 0.65 at roughness 1. ours: between the two, an octahedral map of about √6 times the side would match the cube's memory and its picture. So the octahedral map buys nothing per byte, and its lookup is the slowest. The mirror's worst cases, 150–190 at every size and storage, are the sun's and the windows' edges, which no storage at these sizes holds.

- **Seams.** The hardware filters a cube across its faces on every backend. Faces read as a 2D array clamp there instead: a jump of up to 4 code values along the cube's edges in the roughest mip (mean 1.5 there, 0.12 over the sphere), none in the mirror and the sharp mips. Octahedral seams with a texel of border need two samples per level, since a border shifts each level's coordinates and hardware trilinear can't cross levels.
- **Box parallax** only changes the direction looked up, the same for every storage. Every lookup is `textureSampleLevel` at the roughness's mip, so the folds' derivatives never pick a mip.

## 3. The cost

**Fragment stage**, Metal, M5. A 1920 × 1080 `Rgba16Float` target, 8 additive full-screen draws a pass, 16 lookups a pixel at mip 1.3. Sixteen 128 probes, a probe per 64-pixel tile. Three processes of 40 rounds under `slot --all --load 8`, a load average of 5.0 at the start. Medians, each process within 0.5%, less a pass that computes the same directions without a texture:

| Lookup | One lookup over a 1080p frame |
|---|---|
| Cube array | 16.4 µs |
| Single cube | 16.3 µs |
| Faces, 2D array | 20.9 µs (+27%) |
| Octahedral, two samples | 24.9 µs (+52%) |

Times are wall clock from submit to the GPU's done: Metal's pass timestamps came back with the end before the beginning, so they were set aside.

**Memory** per probe resident, `Rgba16Float`, mips to 8 texels:

| Tier size | Cube or faces | Octahedral 2× side | Octahedral 4× side | A baked key's mip 0, which L2 writes |
|---|---|---|---|---|
| 128 (DH, DL) | 1023 KiB | 682 KiB | 2728 KiB | 768 KiB |
| 64 (MH) | 255 KiB | 170 KiB | 682 KiB | 192 KiB |
| 32 (ML) | 63 KiB | 42 KiB | 170 KiB | 48 KiB |

**Formats.** `ambient.rs`'s reflection cube is `Rgba16Float` (`STORAGE_FORMAT`), because compute writes it. The runtime array is written the same way, by the composing and the prefilter, so it is `Rgba16Float` on every backend. That format is core WebGPU, both as storage and as a sampled 2D or cube array. BC6H can't be a storage target and has no alpha for the visibility. ASTC HDR (`TEXTURE_COMPRESSION_ASTC_HDR`) isn't among the features `GpuCaps` enables. Neither is open to the runtime array.

The device's floor (`downlevel_defaults`) is 256 array layers, so one array holds at most 42 probes. Holding both keys as layers halves that.

**The cook.** The cooker has no cube path today: the probe grid's bake traces rays to L1/L2 SH (`tools/sapphire_pipeline/src/probes.rs`), and the sky cube is drawn on the GPU at run time. L2 writes the same thing for both lookups, which an octahedral choice would have forked. That is six faces a key, mip 0 only, in `cube_texel_direction`'s convention (`probes.wgsl`).

## 4. Binding slots

After L1 the characters' fragment stage has 3 sampled textures free and the environment's has 5. Each option takes **one** texture slot in each family and no sampler, since `ambient_sampler` is linear and clamped. That leaves the characters 2 and the environment 4.

- **Keys composed in compute** (§3's "one probe is prefiltered a frame") take no fragment slot. Reading two keys in the fragment stage would take one slot holding both keys' layers (twice the lookups, half the probes), or two slots holding a texture each.
- The sky's reflection cube (binding 14) stays as it is for places without probes.
- The nearest cube would also take one slot, unless it replaced binding 14's contents.

## 5. Blending and the live sky

Both happen in the composing compute pass, on the faces, before any lookup, so they are the same in every storage. Composing doesn't care how the fragment stage reads the result.

A person would see these losses with one nearest cube:
- **A pop.** Every reflection on screen changes in one frame when the nearest probe changes. A crossfade in the composing pass would trade the pop for a slide.
- **No blend across a doorway.** Every surface reflects the camera's room. The street seen from a doorway reflects the interior, through the interior's box, so its reflections slide wrongly as the camera moves.

The faces fallback loses neither. It is also less to build. One nearest cube needs its own declaration in the WGSL too (a `texture_cube` where the device path has a cube array), plus its own composing: the pick and any crossfade. The faces fallback needs only the declaration and a dozen lines that pick the face.

## What stays unverified

- **lavapipe and WARP:** the test runs there in CI (lavapipe in the GPU job, WARP in `gpu-tests-warp.yml` on dispatch). WARP's answer is read from source here.
- **Phones:** no phone GPU has run anything. The Simulator and the emulator run on this Mac's GPU. ours: the faces lookup's cost on a tile GPU and whether any shipping Android GPU lacks `imageCubeArray` are owed to §1's phone run.
- **The look:** judged by numbers on a generated environment, not by an artist on content.
- **Timings** anywhere but this Mac, and the composing and prefilter's cost per probe, which L3 measures.
