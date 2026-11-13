# Compressed texture copies (M5 M0)

M0's spike, run on 2026-11-13. Unless a line says otherwise: an Apple M5 (24 GB), macOS 27.0, wgpu 30.0.1, the dev profile (dependencies at opt-level 3). "ours:" marks our own inference.

**Verdict: the design stands, with two changes.** `copy_texture_to_texture` between BC7 textures and between ASTC textures, level by level, keeps every block on every backend this Mac can run: Metal, the iOS Simulator and the Android emulator, with levels smaller than one block and levels that aren't whole blocks among the copies. Raising a 2K texture one level costs about 1.1 ms on the M5, most of it the new level's 4 MiB write and the GPU's copies. The changes, both for M3 and M4:

1. **A texture's top must be whole blocks.** wgpu refuses a compressed texture whose level 0 isn't (`NotMultipleOfBlockWidth`), and so does `TextureDesc::validate`. So the cook's tail can't simply be "levels up to 128 px": its top is the larger of the first level at or under 128 px and the deepest level that can be a top, where level k of a W × H base can be a top only when the block's size times 2^k divides W and H. A power-of-two texture with 4×4 blocks streams down to 4 px, so 128 px holds; a 1000 × 600 BC7 map can drop only to 500 × 300, and an ASTC 6×6 map needs a base that's a multiple of 6 · 2^k to stream k levels. M3's split takes the rule, and every level above the tail is then a valid top for M4 and M5.
2. **Every streamed texture carries `COPY_SRC`.** Today's upload makes textures `TEXTURE_BINDING | COPY_DST`; a texture a raise or a lower copies from needs `COPY_SRC` too. In wgpu-hal 30.0.1, Metal maps no usage flag for it (`MTLTextureUsage` has none for blits), D3D12 no resource flag (any resource can be a copy source), and Vulkan adds `TRANSFER_SRC` to the image's usage. ours: for a sampled, block-compressed image that should cost nothing a driver optimizes (framebuffer compression applies to render targets); not measured on a discrete GPU. M4 adds it in `GpuTexture::upload` (or a streamed texture's own path).

Nothing else in §3's "Mip streaming" row changes: new texture, copies of the kept levels, `write_texture` of the new top, rebind.

## What ran

- **The regression test,** `crates/sapphire_render/tests/gpu/mip_copies.rs`, in the renderer's GPU suite (`cargo test -p sapphire_render --test gpu mip_copies:: -- --ignored --test-threads=1`):
  - `compressed_textures_stream_level_by_level_by_copies` cooks a smooth image with the pipeline, as the cooks do: BC7 sRGB (desktop) at 160 × 96, and ASTC sRGB (mobile) with 4×4 blocks at 160 × 96, 6×6 at 192 × 96 and 8×8 at 320 × 192. Content uses only 4×4 today; a texture's settings may choose 6×6 or 8×8 (`astc_block`), and the cook's own tests use 8×8. It uploads the tail (the deepest level that can be a top: 20 × 12, 20 × 12, 12 × 6 and 40 × 24), raises it one level at a time to the whole texture with the design's steps, lowers it back one level at a time, and after every step reads every level back with `copy_texture_to_buffer` and compares it with the cook's bytes, and samples every level (bilinear at texel centres) and compares it bit for bit with the same levels uploaded whole through `GpuTexture::upload`. Every step copies the small levels: with 4×4 blocks 10 × 6 and 5 × 3 (not whole blocks), 2 × 1 and 1 × 1 (under a block); with 8×8, 20 × 12 (over a block, not whole) down to 1 × 1. Each case also checks that wgpu refuses a texture whose top is the level under the tail. A device without a format skips it and says so; CPU adapters (lavapipe, WARP) may not skip BC7, and Metal may not skip ASTC.
  - `raising_a_2k_texture_by_one_level_keeps_every_block` raises a 2048² BC7 sRGB and a 2048² ASTC 4×4 sRGB texture from its 1024² top: create, eleven copies, a 4 MiB `write_texture`, submit, wait. It checks every level byte for byte and logs the time of each part. Its blocks are made, not cooked (BC7 mode 6 blocks, ASTC void-extent blocks, all different), so it needs no 2K encode.
- **On devices,** the same check, ported for the run and not kept: the renderer's GPU suite can't build for iOS or Android, since the cooker it cooks with links `ctt`'s Intel texture compressor, whose prebuilt kernels exist only for desktop targets. `sapphire_rhi`'s lib tests can: their only dev-dependencies are Apple's `objc2-quartz-core`. So a temporary module in `sapphire_rhi` ran the regression test's steps on made blocks (BC7 mode 6, ASTC void-extent at 4×4, 6×6 and 8×8, the same four sizes, plus both 2K raises and lowers), checking bytes after every step and decoding every level with `textureLoad` against a whole upload. Its test binary ran in a private iOS 26.3 Simulator (iPhone 17 Pro) under `simctl spawn`, and on the `sapphire_api36` emulator (`-gpu host`, arm64) under `adb shell`, the route M3 Z5b used for `process_memory`.

## Results

| Backend | Adapter | BC7 | ASTC 4×4 | ASTC 6×6 | ASTC 8×8 | 2K raise |
|---|---|---|---|---|---|---|
| Metal (macOS) | Apple M5 | every level matches | matches | matches | matches | BC7 and ASTC match |
| Metal, Apple's API validation on (`MTL_DEBUG_LAYER=1`, errors assert) | Apple M5 | matches, no validation error | matches | matches | matches | match |
| Metal (iOS 26.3 Simulator) | Apple iOS simulator GPU | no BC: skipped, as the device says | matches | matches | matches | ASTC matches |
| Vulkan (Android emulator, gfxstream on the host) | Apple M5 | matches | matches | matches | matches | BC7 and ASTC match |
| Vulkan (lavapipe, Mesa 26.1.3, CI) | llvmpipe | should run (required) | runs if lavapipe offers ASTC | as 4×4 | as 4×4 | as each format |
| D3D12 (WARP 1.0.21, CI's WARP workflow) | Microsoft Basic Render Driver | should run (required) | skipped: D3D12 has no ASTC | skipped | skipped | BC7 |

wgpu-core validates what reaches every backend: a compressed copy's origin and size must be whole blocks, measured against the level's size rounded up to whole blocks, so a 2 × 1 level of a 4×4-block format copies as 4 × 4. wgpu-hal then hands Metal that padded size ("Metal expects physical sizes here"), D3D12 a box of it, and Vulkan the size clamped to the level's own (2 × 1). Metal's own validation layer accepted the padded size for every small level. ours: Vulkan wants a region inside the level, which is why wgpu-hal clamps there; lavapipe proves that path in CI, and the emulator's Vulkan already did here.

**Cost on Metal** (the 2K test, median of 8 raises after a first, `slot --all --load 8`, load average 3.9 at the start): three runs of the binary, the range of their medians, in milliseconds:

| Format | Create | Copies recorded | `write_texture` (4 MiB) | Submit | Until the GPU is done | Total | The first raise of a run |
|---|---|---|---|---|---|---|---|
| BC7 sRGB | 0.007–0.008 | 0.023–0.027 | 0.326–0.336 | 0.016–0.020 | 0.742–0.756 | 1.10–1.15 | 2.6–3.4 |
| ASTC 4×4 sRGB | 0.006–0.009 | 0.021–0.022 | 0.323–0.355 | 0.013–0.015 | 0.732–0.757 | 1.14 | 1.1–1.3 |

Earlier runs while other agents' builds held the machine (load 11–25) gave 0.9–1.2 ms in all, so the cost doesn't move much with load; the first raise of a process pays wgpu's and Metal's first use of a blit and a staging buffer.

ours: on Apple's unified memory a raise is mostly the CPU's 4 MiB copy into the staging buffer and the GPU's upload of it; the copies of the eleven kept levels (1.3 MiB) are a small part. A raise holds the old texture beside the new one until the old one's last use retires: a 2K BC7 or ASTC 4×4 raise holds 6.7 MiB for a moment where it ends at 5.3. A streamer that raises many textures in one frame holds the sum, which M5's pool must count.

## What stays unverified

- **lavapipe and WARP:** the test is written for them and CI runs it, but a pass prints nothing there, so which formats ran is known from the table's rules, not seen: BC7 must run on both (the test fails if a CPU adapter skips it); whether lavapipe offers ASTC isn't known here (the renderer's older texture test skips silently too). WARP's run waits for the orchestrator's dispatch of `gpu-tests-warp.yml`.
- **Phones:** no Apple GPU in a phone, no Mali, Adreno or PowerVR has run this. The Simulator is Metal on this Mac's GPU, and the emulator's Vulkan is gfxstream translating to the host, so both prove the API path and the padded and clamped sizes, not a phone driver's copy of a compressed level. ours: a driver that keeps ASTC decompressed behind the API, as emulation layers do, is where a copy could differ. Owed to the phone run of §1.
- **Discrete GPUs on Windows and Linux,** whose drivers tile compressed textures their own way: owed, as all discrete-GPU checks are.
- **Timings anywhere but this Mac,** and the cost of rebinding materials, which M4 builds.

## Noticed

- On the Android emulator, a debug build's GPU context crashes in the emulator's Vulkan driver (`vulkan.ranchu.so`, `vk_common_SetDebugUtilsObjectNameEXT`) as soon as wgpu names an object: debug builds turn object labels on by default. `SAPPHIRE_GPU_DEBUG=none` avoids it; every run above used it there. `sapphire_rhi`'s own `creates_a_headless_device` crashes the same way without it.
