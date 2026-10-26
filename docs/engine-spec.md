# Sapphire Engine Technical Spec, v0.1 draft

A custom Rust engine for a Zenless Zone Zero–style 3D anime action game, shipping on **macOS, Windows, Linux, iOS and Android**.

- **Date:** 2026-10-24
- **Companion doc:** [reports/Rust anime action game engine.md](../reports/Rust%20anime%20action%20game%20engine.md) explains *why* each choice was made. This spec says *what* to build and *how*.
- **Engine name:** Sapphire. Crates are named `sapphire_*` (ADR 0001). Never name a crate `core`, `std` or `alloc`, because those collide with Rust's built-in crates.

**Label legend**

- **Decision**: the chosen approach. Change it deliberately, and record why.
- **Proposal**: a starting number or floor to validate with measurements.
- **Verify**: a fact not yet confirmed, listed again in §11.
- **v1** is needed for the vertical slice; **later** comes after it.

---

## 1. Scope

### 1.1 Product target

- **World:** zone-based levels in the style of ZZZ's commissions and hollows, not a seamless open world. Each zone is up to about 4 km across and is reached through a loading transition.
- **Party:** three characters with swapping; one is controlled at a time, and the off-field characters keep ticking.
- **Frame rate:** 60 fps on desktop. On mobile, 30 fps is the baseline and 60 fps the high-tier target.
- **Mode:** single-player first. Online services such as accounts, gacha and patching come later and are server-authoritative.
- **Look:** the anime aesthetic of Wuthering Waves and Zenless Zone Zero, not generic toon shading.
  - Cel-shaded characters with authored light response sit in a stylized but grounded world.
  - One time-of-day key drives sky, fog, ambient, shadow tint, the character light rig and the grade.
  - The picture is built from gradients and atmosphere and finished with a glowing, non-filmic grade.
  - [docs/m1-plan.md](m1-plan.md) §2 ranks the pillars.

### 1.2 Platforms

All five platforms are first-class from month 0. Bringing up a platform is cheapest while the engine is small, so the first milestone runs on all five.

### 1.3 Non-goals for v1

- **Consoles.** They are a separate project under the platform holders' NDAs. `sapphire_rhi` keeps that door open.
- A seamless open world, online co-op, ray tracing, a web build, modding and VR.

### 1.4 Engine rules, enforced in review and CI

| # | Rule | Enforcement |
|---|---|---|
| R1 | A crate depends only on crates in lower layers (§4.1). Runtime crates never depend on tool crates. | `cargo xtask check-layers` in CI, using `cargo metadata` |
| R2 | All gameplay time reads an owner clock (§4.5). | Clippy `disallowed-methods` bans `Instant::now` / `SystemTime::now` in L3–L5 crates |
| R3 | Side effects such as hits, damage, spawns and cues go through queues that resolve at fixed points in the frame. | Code review; systems take `MessageWriter`s rather than `&mut` to other entities |
| R4 | The renderer sees only extracted plain data (§4.6). | `sapphire_render` does not depend on `sapphire_gameplay` or `sapphire_world` |
| R5 | Assets are referenced by GUID and cooked per platform. Runtime formats load in place. | Assets have no path strings at runtime |
| R6 | The simulation does no synchronous IO, decompression or script GC, and makes **zero heap allocations on the simulation thread inside the fixed step** (single-threaded by design, §4.4). Process-wide allocations per frame are a tracked budget, not a hard zero (wgpu alone does ~130 per frame in M0). | A dev-build counting allocator asserts during combat replays; the frame stats publish allocations per frame |
| R7 | Platform-specific code lives only in `sapphire_platform`, `sapphire_crash` and the backend selection in `sapphire_rhi`, plus two documented exceptions: CPU topology detection in `sapphire_jobs::topology`, and target classification and the TLS model in `sapphire_core` (ADR 0001). | `#[cfg(target_os)]` anywhere else is a review flag |
| R8 | Cargo features select platform or backend only. Dev tools enter through the leaf binary. | A layer check rejects `editor` / `dev` features on runtime crates |
| R9 | Every system has a budget (§3) and publishes a stat counter. | Visible in the `sapphire_debug` stats overlay and in Tracy |

---

## 2. Platform matrix

| Concern | Windows | macOS | Linux | iOS | Android |
|---|---|---|---|---|---|
| **Minimum OS** (Proposal) | 10 22H2 / 11 | 14 Sonoma, Apple Silicon only¹ | Ubuntu 22.04-era glibc, SteamOS 3 | iOS 17 | Android 10 (API 29), arm64 only |
| **Rust target** | `x86_64-pc-windows-msvc` (`aarch64-pc-windows-msvc` later) | `aarch64-apple-darwin` | `x86_64-unknown-linux-gnu` | `aarch64-apple-ios`, `aarch64-apple-ios-sim` | `aarch64-linux-android` (`x86_64-linux-android` for the emulator) |
| **GPU backend (wgpu)** | Vulkan 1.2 primary, DX12 fallback² | Metal | Vulkan 1.2 | Metal | Vulkan 1.1 required (no GLES) |
| **Shader output** (naga, at runtime) | SPIR-V / HLSL | MSL | SPIR-V | MSL | SPIR-V |
| **Texture cook** | `desktop` (BCn) | `desktop` (BCn)³ | `desktop` | `mobile` (ASTC) | `mobile` (ASTC) |
| **Upscaler** | DLSS (NVIDIA + Vulkan) or own spatial upscaler | MetalFX | Own spatial upscaler | MetalFX | Own spatial upscaler |
| **Window / events** | SDL3 | SDL3 | SDL3 (Wayland + X11) | SDL3 (UIKit) | SDL3 (SDLActivity) |
| **Input** | KB/M, gamepads | KB/M, gamepads | KB/M, gamepads, Steam Deck | Touch, MFi / Xbox / DualSense pads | Touch, gamepads |
| **Audio** | FMOD (WASAPI) | FMOD (CoreAudio) | FMOD (PulseAudio / PipeWire) | FMOD + AVAudioSession | FMOD (AAudio) |
| **Read-only content** | Install directory | `.app/Contents/Resources` | Install directory | App bundle | Small boot pack in APK; main packs from CDN or Play Asset Delivery |
| **Writable directories** | `SDL_GetPrefPath` | same | same | same, with downloaded packs excluded from iCloud backup | same (app-internal files directory) |
| **Crash reporting** | crash-handler + minidumper → Sentry | same | same | sentry-cocoa (native SDK) | crash-handler → Sentry (**Verify** minidumper on Android; else Sentry's Android NDK SDK) |
| **CPU profiling** | Tracy, Superluminal | Tracy, Instruments, samply | Tracy, samply / perf | Instruments | Tracy, Perfetto |
| **GPU debugging** | RenderDoc, PIX, Nsight | Xcode Metal debugger | RenderDoc | Xcode Metal debugger | RenderDoc, Android GPU Inspector, Arm / Qualcomm profilers |
| **Distribution** | Steam | Steam or direct (codesigned + notarized) | Steam (Steam Runtime) | App Store, via an Xcode project | Google Play AAB |
| **Build host** | Windows CI runner | Mac | Linux CI runner | Mac (required) | Mac or Linux (`cargo-ndk`) |
| **Store gotchas** | — | Notarization, hardened runtime | — | No JIT; App Store guideline 2.5.2 on downloaded code | 16 KB page alignment of `.so` files; yearly target-API bump |

**Notes**

1. macOS 26 Tahoe is the last release for Intel Macs, so supporting Apple Silicon only removes a whole hardware class from testing.
2. Vulkan on Windows matches Linux and Android, and it is what `dlss_wgpu` needs. Fall back to DX12 automatically if Vulkan device creation fails, or for driver versions on a blocklist. Revisit the default after driver testing on Intel iGPUs (§11).
3. Apple Silicon supports BC formats in Metal, so macOS shares the desktop cook with Windows and Linux. Its ASTC support isn't needed.

---

## 3. Hardware tiers and budgets

All numbers in this section are **Proposals**. Set them in M2 by measuring on the reference devices, and on mobile always **measure at thermally throttled clocks** (after 15 minutes of combat), not peak clocks.

### 3.1 Tiers

| Tier | Reference devices | Internal res → output | Target fps | CPU critical path | GPU frame | Memory budget |
|---|---|---|---|---|---|---|
| **Desktop-High** | RTX 3070 / RX 6700 XT, M2 Pro | 1440p native, or 4K upscaled | 60–120 | ≤ 8 ms | ≤ 8–12 ms | 10 GB RAM, 6 GB VRAM |
| **Desktop-Low** | GTX 1060 6 GB, Steam Deck, M1 8 GB | 1080p (800p on Deck) | 60 (Deck 40–60) | ≤ 12 ms | ≤ 14 ms | 6 GB RAM, 3 GB VRAM |
| **Mobile-High** | iPhone 15 Pro (A17 Pro), Snapdragon 8 Gen 2+ | ~1600×740 → native | 60 | ≤ 12 ms | ≤ 13 ms | 2.8 GB total (unified memory) |
| **Mobile-Low** | iPhone 12 (A14, 4 GB), Snapdragon 865-class with 6 GB | ~1280×590 → native | 30 | ≤ 22 ms | ≤ 26 ms | 1.6 GB total |

The device floor is the Mobile-Low column. Raise it rather than compromise the look.

- **As built (M1 R6, `sapphire_render::quality`):**
  - The four rows are data: `crates/sapphire_render/tiers.ron`, one entry per tier, compiled into the renderer (§5.12, "Quality tiers"). Each entry carries its row's internal resolution as a render scale of 1, capped in height on mobile (Mobile-High 740; Mobile-Low 590, "~1280×590 → native" on any phone). The desktop tiers render at native height until R7 brings an upscaler, because a bilinear upscale would soften the outlines and hard two-tone edges the anime look depends on; R7 restores their caps (Desktop-High 1440 rows, 4K upscaled; Desktop-Low 1080). Each entry also carries its target frame rate (60, 60, 60, 30) and GPU frame (12, 14, 13, 26 ms, the upper ends), and §3.4's light and particle budgets.
  - A device gets the highest tier of its class whose rules it meets, from its GPU's kind and API (`GpuCaps`) and its RAM (the memory column): Desktop-High needs a discrete GPU, or Apple silicon (an integrated GPU on Metal), with 16 GB; Mobile-High an 8 GB phone (at least 7,000 MiB reported). Software and paravirtual GPUs, the golden-image runners among them, get Desktop-Low. These rules are a first heuristic: they can't see VRAM, so a GTX 1060 machine with 16 GB of RAM counts as Desktop-High until a device database and measurements on the reference devices (M2) refine them.
  - §3.2 and §3.3 stay the stats overlay's budget columns; the overlay shows the active tier beside them.
- **As built (M1 R7):** temporal AA upsamples on the desktop tiers, so their caps are back: Desktop-High renders at most 1440 rows (1440p native, 4K from 2560×1440), Desktop-Low 1080 (2560×1440 and 4K outputs from 1920×1080). Below those heights both render at native height, as the golden images (640×360) and the bench (1920×1080) do. §5.12's "Anti-aliasing and upscaling" has the resolve.

### 3.2 CPU budget per frame stage

Each figure is wall time on the critical path.

| Stage | Desktop-Low @ 60 fps (ms) | Mobile-Low @ 30 fps (ms) |
|---|---|---|
| Platform events, input, orchestration | 0.3 | 0.6 |
| Fixed-step sim, per 60 Hz step (combat, AI, controller) | 1.0 | 1.5 × 2 steps |
| Physics, per step (inside the sim) | 0.7 | 1.2 × 2 steps |
| Animation + IK (parallel) | 1.5 | 3.0 |
| Secondary motion (hair, skirts) | 0.5 | 1.0 |
| VFX simulation (CPU side) | 0.3 | 0.6 |
| Script events + Luau GC step | 0.3 | 0.5 |
| Streaming activation | 0.5 | 1.0 |
| Audio update | 0.2 | 0.3 |
| Extract | 0.7 | 1.2 |
| **Game side total** | **~6.0** | **~13.6** |
| Render prepare + record (overlaps the next frame's sim) | 4.0 | 8.0 |

### 3.3 GPU budget per pass

| Pass | Desktop-Low, 1080p @ 60 fps (ms) | Mobile-Low, ~720p @ 30 fps (ms) |
|---|---|---|
| Culling + prepass | 1.5 | 1.5 (depth only) |
| Shadows | 2.0 (4 cascades) | 3.0 (2 cascades) |
| Light clustering, AO | 1.5 | — |
| Forward opaque | 3.5 | 8.0 |
| Outlines | 0.6 | 1.5 |
| Hair, eyes, transparents | 0.8 | 2.0 |
| Volumetric fog | 0.6 | — |
| Particles, distortion | 1.2 | 3.0 |
| TAA / upscaling | 0.8 | 1.0 (spatial) |
| Bloom + tonemap + LUT | 0.7 | 2.5 (one fused pass) |
| UI | 0.3 | 1.0 |
| **Total** | **~13.5** | **~23.5** |

### 3.4 Content budgets

| Item | Desktop | Mobile |
|---|---|---|
| Hero character, LOD0 triangles | 80k | 30k |
| Bones per character (incl. secondary) | ≤ 200 | ≤ 140 |
| Materials per character | ≤ 6 | ≤ 4 |
| Texture size per character part | 2K | 1K (ramps are 256 × N and uncompressed) |
| Active enemies in combat | 12 | 6 |
| Visible dynamic lights | 64 (clustered) | 8 (per-object lists) |
| Live GPU particles | 50k | 8k |
| Draw calls | 2,500 | 700 |
| Zone texture pool | 2.5 GB | 600 MB (WuWa's mobile pool is reportedly 400 MB) |

---

## 4. Architecture

### 4.1 Layer map

```
 Tools (never shipped)  tools/: sapphire_pipeline · sapphire_editor · sapphire_livelink · sapphire_testkit · xtask
 ───────────────────────────────────────────────────────────────────────────────
 Apps                   apps/: sapphire_sandbox (engine test bed) · game_app (later)
 L5  Game               game_combat · game_characters · game_ai · game_ui
 L4  Framework          sapphire_world · sapphire_gameplay · sapphire_sequencer · sapphire_script · sapphire_save · sapphire_loc
                        sapphire_patch (later) · sapphire_online (later)
 L3  Subsystems         sapphire_render · sapphire_anim · sapphire_physics · sapphire_audio · sapphire_vfx · sapphire_ui
                        sapphire_input · sapphire_nav · sapphire_debug · sapphire_hot_reload (dev builds only)
 L2  Services           sapphire_assets · sapphire_rhi · sapphire_ecs · sapphire_reflect · sapphire_serialize
 L1  Platform & system  sapphire_platform (SDL3) · sapphire_jobs · sapphire_time · sapphire_crash
 L0  Foundation         sapphire_core · sapphire_math   (pure Rust: no SDL, no GPU)
```

The layer numbers were revised in ADR 0001 so that the platform layer can use `sapphire_core`, and tools can use core, math and assets without linking SDL3 or wgpu.

**Dependency rules**

- A crate may depend on any crate in a lower layer, and on crates in its own layer only through an explicit list in `xtask/layers.toml`.
- **Tools** may depend on anything. Nothing depends on tools.
- Apps are the only shipping binaries. `sapphire_editor` is a desktop-only tool binary.

### 4.2 Frame lifecycle

```
            ┌──────────── frame N ─────────────────────────────────────────────┐
Main thread │ SDL_AppIterate: pump events → input snapshot → kick jobs → present │
Workers     │   fixed steps (sim N) → presentation (anim/cloth/VFX/camera) → extract(N)
Workers     │ ◄─ prepare + record render(N-1) in parallel ─► submit(N-1)         │
IO thread   │ positioned reads → decode jobs → GPU upload queue → activation    │
FMOD        │ (FMOD-owned mixer threads)                                         │
            └──────────────────────────────────────────────────────────────────┘
```

1. **Pump and snapshot.** Pump SDL events on the main thread and take a timestamped input snapshot.
2. **Fixed steps.** Run 0–4 fixed 60 Hz simulation steps using an accumulator. The cap of 4 prevents a spiral of death.
3. **Presentation update.** Run at the variable frame rate: pose evaluation, secondary motion, VFX and camera, each on its owner's clock, interpolated between the last two fixed states.
4. **Extract.** Build the `FramePacket` (§4.6). This is the only sync point between simulation and rendering.
5. **Render.** Prepare and record frame N on worker threads while frame N+1 simulates.
6. **Submit.** One `queue.submit` per frame, then present.

**Latency.** Pipelining adds about one frame of latency. Sample input as late as possible, measure end-to-end latency in M2, and keep `desired_maximum_frame_latency = 2`.

### 4.3 Threading

| Thread | Owner | Notes |
|---|---|---|
| Main | SDL main callbacks | Window and event APIs must run here on Apple platforms. It hosts the frame orchestrator. |
| Workers | `sapphire_jobs` (one pool) | Size: desktop = physical cores − 1, capped at 16. Mobile = performance cores − 1 (iOS: `sysctl hw.perflevel0.physicalcpu`; Android: group cores by `cpuinfo_max_freq`). |
| IO | `sapphire_platform::io` | One thread with a priority queue and positioned reads. Decompression happens on workers. |
| Audio | FMOD | The engine calls the FMOD Studio API once per frame from the audio-update job. |
| Crash monitor | `sapphire_crash` | A separate process on desktop (minidumper). Android approach is open (Q12). |

- **Decision: one task pool.** bevy_ecs's executor and all engine parallelism share `bevy_tasks`. Rayon is allowed only in tools and the cooker, because two pools oversubscribe the cores on phones.

### 4.4 Data model: a hybrid ECS

- **ECS (bevy_ecs) is the world database.** Use it for transforms, bounds, render proxies, pose instances, emitters, projectiles, hit volumes and crowds.
- **Characters are entities with a few fat components.** `CombatController` holds the move-graph state, intent buffer, abilities and cooldowns as ordinary Rust structs.
- **Resources** hold global services: the camera director, combat message queue, streaming state and encounter director.
- **Messages** carry queued side effects. Observers are used only for lifecycle hooks.
- **Hierarchy** (bevy's `ChildOf`) is only for attachments such as weapons on bones and VFX on sockets.
- **Ordering.** Combat systems run in an explicit chained order inside the fixed step. Parallelism goes to wide passes such as animation, culling and extraction.

### 4.5 Clock domains

```rust
/// Simulation time is integer ticks at 60 Hz, plus integer sub-ticks for scaled time,
/// so time scaling never accumulates float drift.
pub const TICK_HZ: u32 = 60;
pub const SUBTICKS_PER_TICK: u32 = 1000;

pub struct Clocks {
    pub real: RealClock,   // unscaled: UI, input timestamps, networking
    pub world: ScaledClock, // pause, perfect-dodge slow motion, cinematics
}

#[derive(Component)]
pub struct LocalClock {
    pub scales: SmallVec<[TimeScale; 4]>, // stack: (source, factor in per-mille)
    pub hitstop_ticks: u16,               // counts down in *real* fixed ticks
    pub now_subticks: u64,
}
// Entity time advanced per fixed step = SUBTICKS_PER_TICK × world.scale × Π(local.scales),
// or 0 while hitstop_ticks > 0.
```

- **Hitstop** sets the attacker's and victim's `hitstop_ticks`.
- **Perfect dodge** pushes a world scale of about 0.2 and a counter-scale on the player's `LocalClock`.
- **Camera** runs on its own clock.
- **Every consumer reads its owner's clock:** timers, cooldowns, animation playback, VFX, audio pitch and physics.

### 4.6 The extract boundary

```rust
/// Plain data, allocated from the frame arena. A ring of 3 packets.
pub struct FramePacket {
    pub frame: u64,
    pub views: SmallVec<[ViewData; 4]>,    // main camera, shadow cascades, UI 3D insets
    pub static_dirty: Vec<InstanceDelta>,  // only instances whose transform/material changed
    pub skinned: Vec<SkinnedInstance>,     // mesh, material, palette range in bone buffer
    pub morphs: Vec<MorphWeights>,
    pub lights: Vec<LightData>,
    pub vfx: VfxFrame,                     // spawn requests, emitter params, trails
    pub post: PostParams,                  // incl. gameplay-triggered effects (finish freeze, halftone)
    pub ui: UiDrawList,
    pub debug: DebugDrawList,              // empty in shipping builds
}
```

- Extraction is a set of systems that run after presentation and write into the packet. They use change detection to copy only what changed.
- The render world keeps persistent GPU scene buffers and applies the deltas from each packet.
- **As built (M1 F3, `sapphire_render::FramePacket`):**
  - The packet holds the frame number, the views (the main camera first), the scene constants (sun, sky gradient, time), the post-processing parameters (`PostParams`, since M1 R3) and instance deltas: `changed` (a stable `InstanceId` with mesh and transform; additions included) and `removed`. The other fields above arrive with the systems that fill them.
  - A `FramePacketRing` holds three packets; each keeps its vectors' capacity when reused, so a warm frame allocates nothing for its packet. Reused `Vec`s stand in for the frame arena and `SmallVec`.
  - The renderer must prepare every packet, in order, or deltas are lost; debug builds check the frame numbers. The sandbox therefore extracts and prepares every frame, drawn or not, and a packet with no view draws nothing. `RemovedComponents` needs this too: it keeps a removal only until the second `Cleanup` after it.
  - In the sandbox, an entity draws while it has a `RenderMesh`, which requires a `RenderTransform`. `Changed` on either (which includes additions) writes a change, `RemovedComponents` writes a removal, and IDs are entity bits. Removals apply first, so an instance removed and added again in one frame survives; unknown IDs are ignored. Poses are written with `set_if_neq`, so an entity that holds still costs nothing: the spinning turntable is one changed instance per frame, and none during hitstop. A renderer that starts empty (after device loss) is sent every instance.
  - The multi-threaded executor allocates once per run of a schedule that has systems plus once per system, and wakes a worker. Posing in the otherwise empty `Presentation` measured 2 more allocations and about 8 µs more per frame, so the sandbox poses at the head of `Extract` until animation gives `Presentation` real work.
  - Cost against M1 F2 (sandbox `--bench 3000`, Apple M5, release): CPU p50 unchanged at about 30 µs per frame, allocations unchanged at 70.1 per frame and 2 KB fewer bytes allocated. Rebuilding the draw list never allocated (its vectors were reused); the saving is the draw-record upload, and it pays for the second `Extract` system.
- **As built (M1 R5, `FramePacket::lights`):**
  - `lights` is a `Vec<LocalLight>`: point and spot lights as plain data (rule R4), each with a world position, linear color, intensity, range and `LightKind::Point` or `LightKind::Spot { direction, inner_angle, outer_angle }`. The sun stays the `DirectionalLight` among the scene constants. The spec's `LightData` is this type.
  - Lights are sent whole every frame, not as deltas: a frame has a few dozen at most (§3.4), and the renderer culls and lists them per frame anyway. It uploads its light buffer only when the culled, packed lights differ from the last frame's (a comparison of at most `max_lights` × 64 bytes).
  - Packet order is the lights' priority within a full cluster (§5.12); across the frame, the renderer keeps the nearest.
  - In the sandbox, an entity lights while it has a `RenderLight` (the `LocalLight` in the entity's space), placed by its required `RenderTransform` like a mesh; the range stays in world meters. `extract_lights` runs in `Extract` after the instances. `--lights` spawns three demo lights around the character, which the golden scene `turntable-lights` shows.
  - The second `Extract` system costs one allocation per frame (the executor's per-system cost, above), with lights or without.
- **As built (M1 R2, `FramePacket::{skinned, palettes, morph_weights}`):**
  - `skinned` is a `Vec<SkinnedInstance>`: an `InstanceId` and two `PacketRange`s, one into `palettes` (`Vec<Affine3A>`, the per-frame bone-matrix buffer) and one into `morph_weights` (`Vec<f32>`; the spec's `MorphWeights` is this flat array). `FramePacket::push_skinned` appends a pose. The ring keeps the vectors' capacity, so a warm frame allocates nothing for its poses.
  - A palette holds finished skinning matrices: `model_from_joint(bind.joint) × bind.inverse_bind_matrix` per palette entry, in the mesh's object space. Extraction multiplies, as §5.13 has it, so the renderer never needs the skeleton. There is one weight per morph target. Missing weights count as 0, and applying the targets' default weights is up to the animation.
  - Poses are deltas, like instances. An instance sends one when its pose changes; one that holds still (hitstop, pause) sends none and keeps its last. The renderer applies them after the packet's instance deltas, so an instance can be added and posed in one packet. An instance never posed draws in its bind pose. The renderer ignores, and counts, a pose for an instance without a skinned or morphed mesh, one whose palette length is wrong, and one with a non-finite value.
  - In the sandbox, an entity drawing a skinned or morphed mesh has a `RenderPose` (its palette and weights, written in place only when they change). `extract_poses` runs in `Extract` after the lights. It sends the poses that changed, plus those of entities added, re-meshed or given their transform back, since the renderer starts those over in the bind pose. `--skinned` spawns the test stalk (§5.12) and only then adds its two systems, because each `Extract` system costs an allocation per frame (above).
- **As built (M1 W1, the scene constants from time of day):**
  - The scene constants are the frame's look, written whole every frame and resolved by time of day (`sapphire_world::tod`, §5.21) from one set of keys: the key light (`sun`: the sun by day, the moon at night, blended through twilight), the sky gradient and `post`, as before, plus six new fields for the packages that will read them. `celestial` holds the sun and moon themselves (W2's discs). `ambient` is a sky and ground hemisphere with an intensity (W4, W5, C2). `fog` holds near and far distance layers, a height layer and sun inscatter (W3). `shadow_tint` is the light kept in cast shadows (R4, W4, C2). `character_light` is the character rig's key color and intensity (C2). `day_night` is 0 by day and 1 at night (C1's ramp rows). Each type's docs name its readers.
  - The renderer draws the key light, sky and post as before. `day_night` reaches the frame uniforms as `FrameUniforms::day_night`, which grow by 16 bytes; nothing reads the other new fields yet. The renderer doesn't depend on `sapphire_world` (rule R4): the world crate resolves into the packet's plain types, and the app writes them (`TodFrame::write`).
  - In the sandbox, `Scene::frame_packet` resolves time of day, with the camera's eye as the zone volumes' probe, and writes it, then the player's filter strength on top (`post.filter_strength`, the one look cvar left). It resolves there rather than in an `Extract` system, which would cost an allocation per frame. Time of day ticks in `FixedFirst`, after the clocks.
  - At the default 10:00 the packet's sun, sky and post equal the constants the sandbox wrote before, bit for bit (a unit test), and the six existing golden scenes' captures are the same PNG bytes as at ef92631.
  - Cost against ef92631 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, two sets of three alternating rounds): CPU p50 92.6–94.9 µs against 92.6–93.9 µs, and p99 within the base's own spread, so no measurable change. Allocations are unchanged at 305.3 per frame, with 16 more bytes for the frame uniforms' staging copy. The resolve itself takes 0.09–0.13 µs per frame while the hour stands still, against a 50 µs budget.
  - With the day running (`--time-of-day 17 --day-length 60`, so 3,000 frames cross seven keys), the resolve takes 0.54–0.55 µs on average and 6.3 µs at worst. CPU p50 is 94–95 µs and allocations 305.5 per frame, because each key crossed bakes one LUT (R3's 0.5 ms on the job pool, eight bakes in all, the 0.66 ms maximum frames).
- **As built (M1 C2, `FramePacket::characters`):**
  - `characters` is a `Vec<CharacterInstance>`: an `InstanceId` and the instance's `CharacterLighting`, the character light rig resolved for this frame (its key light, rim scale, local-light ceiling and stylized GI settings, §5.12). A character drawn as several instances lists each. The renderer never sees presets, cameras' constraints or time of day's curves, only these values (rule R4).
  - Characters are sent whole every frame, like lights: the resolved lighting depends on the camera and the hour, which move most frames, and a scene has a few dozen characters at most. The renderer ignores, and counts, an entry for an instance it doesn't draw or whose lighting isn't valid (non-finite, negative, a zero key direction); an instance not listed draws with the world's key light (`FramePacket::sun`) and unstyled GI.
  - In the sandbox, an entity is a character while it has a `CharacterLightRig` (`sapphire_world::light_rig`: a preset and the head's position in its own space) beside its `RenderMesh`. `Scene::frame_packet` resolves them after `Extract`, so poses are current, through a query state made once, for the main view's camera; frames without a view list none. It runs there rather than as an `Extract` system, which would cost an allocation a frame, and allocates nothing once the packet's vector has grown.
- **As built (M1 C3, `CharacterInstance::face`):**
  - Each `CharacterInstance` carries the character's face beside its lighting: a `CharacterFace` of a `HeadBasis` (the head's forward and up in world space, this frame) and `FaceExpression` weights (blush, gloom), which the face permutation of the anime character shader reads (§5.12). A character drawn as several instances lists the same face with each; instances that draw no face material carry it harmlessly.
  - **Decision: per character, sent whole.** The face shader takes the character's key light into head space, so head and key belong together in the one per-character entry C2 built, which is rewritten every frame anyway. A head that holds still costs 48 bytes a frame per listed instance, against a binding, a buffer and a delta protocol of its own.
  - For a skinned character the basis is the model's +Z and +Y (glTF and VRM models face +Z) turned by the head joint's world rotation relative to its rest (`HeadBasis::turned`): the rig's humanoid `Head` bone. Neither vector need be unit length or exactly perpendicular: the renderer orthonormalizes them, keeping forward. The shader clamps the weights to `[0, 1]`. An entry whose face makes no basis (a zero vector, up parallel to forward, a non-finite value) or has a non-finite weight is ignored and counted like one with invalid lighting (`CharacterInstance::is_valid`); an instance not listed draws its face at rest, facing +Z with +Y up, with no expression.
  - In the sandbox, `Scene::frame_packet` gives the turntable's character the head of its transform (the mannequin has no head joint yet). The heads of the face rig (`--faces`) are characters through a `FaceRig` instead of a `CharacterLightRig`, each listed with its own pose, key light and expression (§5.12).
- **As built (M1 W2, `FramePacket::{sky_layers, sky_material, time_seconds}`, `Celestial::sphere`):**
  - `sky_layers` (`SkyLayers`) is what the sky draws over its gradient, resolved by time of day with the rest of the look (§5.21): the sun's and the moon's `SkyDisc` (color and HDR intensity, glow color and angular radius), star brightness, and `CloudLight` (lit, shade and rim tones, card cloud coverage and cirrus coverage). A disc's look is its own, apart from the light its body gives, so the moon can show by day and a low sun's disc fade before its light does. The default draws nothing over the gradient, so a packet that never sets it draws the sky as W1 did.
  - `Celestial::sphere` is the celestial sphere's turn: the `Quat` that carries the stars and the Milky Way from their own frame into the world.
  - `sky_material` names the zone's sky material (the sky family, uploaded like any material): the discs' sizes, the moon's face, the star and Milky Way maps, and the cloud maps with their scale, drift and softness. `None`, or a handle that isn't an uploaded sky material (counted, `SkyStats::ignored_materials`), draws with `SkyMaterial::default`: default discs, even stars, no clouds.
  - `time_seconds` becomes an `f64` that nothing wraps (the sandbox wrapped it to an hour before). The renderer reduces cloud drift from it in `f64`, so drift neither jumps nor loses precision late in a session, and wraps it to an hour for the shaders (`FrameUniforms::time`, whose layout is unchanged), which read it only for periodic effects such as star twinkle.
  - In the sandbox, `Scene::frame_packet` writes the zone's sky material (`content::SKY`, loaded with its maps at startup and after every renderer rebuild), then `--cloud-cover` or `--plain-sky` on top.
- **As built (M1 W4, `FramePacket::{shadow_tint, weather}`):**
  - `shadow_tint` is read now: the environment shader keeps that share of the world's key light on the shadow side of its terminator and in cast shadow, times each material's tint (§5.12, "Environment"). It reaches the GPU as the frame uniforms' `shadow_tint`, appended.
  - `weather` (`Weather`: wetness and puddle coverage, 0 to 1) is new, for the wet surface model that comes with weather (M2). It reaches the GPU as the frame uniforms' `weather`, appended, and no shader reads it yet; nothing fills it, so the world is dry. `FrameUniforms` grows by 32 bytes; the sixteen earlier golden captures are unchanged.

---

## 5. Module specs

### 5.0 Module index

| Crate | Layer | Purpose | Key dependencies | Milestone |
|---|---|---|---|---|
| `sapphire_platform` | L0 | App lifecycle, windows, surfaces, events, file IO, threads, system info | `sdl3-sys`, `raw-window-handle` | M0 |
| `sapphire_crash` | L0 | Crash capture; upload deferred (M0 status) | `crash-handler`, `minidumper`; `sentry` once upload returns | M0 |
| `sapphire_core` | L1 | Allocators, handles, names, logging, profiling, cvars | `mimalloc`, `bumpalo`, `slotmap`, `tracing`, `profiling` | M0 |
| `sapphire_math` | L1 | Vectors, transforms, bounds, curves, springs, RNG | `glam`, `libm`, `rand_pcg` | M0 |
| `sapphire_jobs` | L1 | The single task pool, scopes, parallel loops | `bevy_tasks` | M0 |
| `sapphire_ecs` | L2 | Re-exported bevy_ecs, schedules, conventions | `bevy_ecs` (pinned) | M1 |
| `sapphire_time` | L2 | Clock domains, fixed-step driver, interpolation | — | M0 |
| `sapphire_reflect` | L2 | Type registry, stable type names, schema versions | `bevy_reflect` | M1 |
| `sapphire_serialize` | L2 | Authored (RON), cooked (rkyv) and save/network (postcard) formats | `serde`, `ron`, `rkyv`, `postcard` | M1 |
| `sapphire_assets` | L2 | Handles, packs, async loading, residency, hot reload | `zstd`, `lz4_flex`, `memmap2`, `blake3` | M1 |
| `sapphire_rhi` | L2 | wgpu device and feature tiers, uploads, pools, pipeline cache | `wgpu`, `wgpu-profiler` | M0 |
| `sapphire_render` | L3 | Frame graph, anime character and environment renderers, sky, post, upscaling | `sapphire_rhi`, `wesl`, `naga`, `encase`, `bytemuck` | M1–M3 |
| `sapphire_anim` | L3 | Skeletal runtime, anim graph, IK, spring bones, cloth | `ozz-animation-rs` (vendored) | M2 |
| `sapphire_physics` | L3 | Collision world, queries, character capsules, hit shapes | `rapier3d`, `parry3d` | M2 |
| `sapphire_audio` | L3 | FMOD Studio integration and cue playback | FMOD (own bindings); `kira` as fallback | M2 |
| `sapphire_vfx` | L3 | GPU particles, trails, decals, screen effects | `sapphire_render` | M2–M3 |
| `sapphire_ui` | L3 | Retained game UI, text, touch controls | `taffy`, `parley` | M3 |
| `sapphire_input` | L3 | Action mapping, contexts, rebinding, glyphs | `sapphire_platform`, `sapphire_serialize` | M1 |
| `sapphire_nav` | L3 | Navmesh baking and runtime pathfinding | `rerecast`, `landmass` | M3 |
| `sapphire_debug` | L3 | Debug draw, stats, dev console, inspector (dev only) | `egui`, `egui-wgpu` | M1 |
| `sapphire_hot_reload` | dev only | Desktop hot reload: watches packs, WGSL sources and documents (dev only) | `notify` | M1 |
| `sapphire_world` | L4 | Time of day; zones, cells, prefabs, spawning, transitions | — | M1 (time of day), M3 |
| `sapphire_gameplay` | L4 | Controller, combat core, abilities, party, camera, AI framework | `statig` | M2 |
| `sapphire_sequencer` | L4 | Timelines for cutscenes and skill cinematics; dialogue | — | M3 |
| `sapphire_script` | L4 | Luau VM, sandbox, generated bindings | `mlua` (`luau`) | M3 |
| `sapphire_save` | L4 | Settings and progress saves, migrations | `postcard`, `ron` | M3 |
| `sapphire_loc` | L4 | Strings, plurals, font fallback, pseudo-loc | `fluent`, `icu` | M3 |
| `sapphire_patch` | L4 | Chunk manifests, CDN download, verification (later) | `reqwest`, `rustls`, `ed25519-dalek` | M4 |
| `sapphire_online` | L4 | Accounts, server-authoritative economy (later) | `reqwest` | later |
| `sapphire_pipeline` | Tools | Importers, cookers, cache, packer | `gltf`, `ufbx`, `ctt`, `ktx2`, `meshopt`, `rayon` | M1 |
| `sapphire_editor` | Tools | Document-model editor, timeline editor, play-in-editor | `egui`, `egui_dock`, `transform-gizmo`, `egui-snarl` | M2–M3 |
| `sapphire_livelink` | Tools | Dev-build socket for pushing edits and pulling stats | `postcard` | M2 |
| `sapphire_testkit` | Tools | Headless world, replay runner, golden images | `insta`, `nv-flip` | M1 |

Each module below follows the same template:

- **Owns**: what it is responsible for.
- **How**: the implementation approach.
- **Platforms**: per-platform notes.
- **Tests**: how it is checked.

---

### 5.1 `sapphire_platform` (L0)

**Owns**

- The app lifecycle and main loop entry.
- Windows and surface handles.
- Raw keyboard, mouse, touch and gamepad events, plus IME and text input.
- Haptics.
- Display info: refresh rate, HDR, safe areas.
- Package and user file systems, and the async IO thread.
- Thread creation and priorities.
- System info: core counts, memory, thermal state.
- Clipboard.

**How**

- **Decision: SDL3** through `sdl3-sys` (raw FFI), wrapped in a safe Rust API. Build SDL statically from source on desktop and Android. On iOS, link an SDL3 xcframework. The high-level `sdl3` crate is still mid-migration, so it isn't used.
- **One entry style everywhere: SDL's main callbacks** (`SDL_AppInit`, `SDL_AppIterate`, `SDL_AppEvent`, `SDL_AppQuit`), implemented in Rust as `extern "C"` functions.
  - **Desktop:** Rust's `fn main()` calls `SDL_RunApp(argc, argv, entry, null)`. `entry` then calls `SDL_EnterAppMainCallbacks(...)`.
  - **iOS (M0, ADR 0002):** the app is a plain Rust binary; `fn main()` calls `SDL_RunApp`, which runs `UIApplicationMain`. xtask bundles it into a `.app` from `platform/ios/Info.plist`. Verified on the iOS 26.3 simulator. An Xcode project (for sentry-cocoa, entitlements, the privacy manifest and store archives) arrives with the M4 store work.
  - **Android (M0, ADR 0002):** the app's library builds as a cdylib. `export_android_main!` exports `SDL_main`, which calls `SDL_EnterAppMainCallbacks`, and re-exports SDL's statically linked JNI symbols. SDLActivity loads `lib<name>.so` and calls `SDL_main`.
- **Surfaces.** Build `raw-window-handle` 0.6 handles from SDL window properties:
  - Windows: `SDL_PROP_WINDOW_WIN32_HWND_POINTER`.
  - Linux: the Wayland surface or X11 window properties.
  - Android: `SDL_PROP_WINDOW_ANDROID_WINDOW_POINTER`.
  - Apple: create a Metal view with `SDL_Metal_CreateView`, get its layer with `SDL_Metal_GetLayer`, and pass it to wgpu as `SurfaceTargetUnsafe::CoreAnimationLayer`.
- **File system.** A virtual file system with two roots:
  - `pkg://` is read-only: pack files from the install directory, app bundle or downloaded-content directory.
  - `user://` is writable: `SDL_GetPrefPath`, holding settings, saves, the pipeline cache and logs.
  - Packs are opened with positioned reads (`read_at` on Unix, `seek_read` on Windows) and memory-mapped where the OS allows. On Android, ship only a small boot pack inside the APK and read it through `SDL_IOFromFile`, which handles APK assets. Main packs live as normal files in app storage, so they can be memory-mapped.
- **Async IO.** One IO thread with a priority queue (critical, high, normal, prefetch), cancellation, and completion callbacks into the job system. io_uring and DirectStorage are *later*, and only if profiling demands them.
  - **As built (M1 A4, `sapphire_platform::IoThread`):** one `sapphire-io` thread serves queued reads most urgent first, oldest first within a priority. Files open on the caller's thread as positioned readers (`ContentFs::open_reader`): plain files read with `read_exact_at` (Unix) or `seek_read` (Windows) on desktop and iOS, SDL streams that seek then read inside the APK on Android. A read's callback runs on the IO thread and always runs, with `Cancelled` for a cancelled read or one queued when the thread stops, and `Closed` for a closed file; the asset server's starts a decode job and returns. The asset server reaches the thread through its `PackSource` trait, implemented by the app, so `sapphire_assets` and the tools that use it never link SDL (ADR 0001). Memory-mapping packs and the IO thread's priority come later.
- **Lifecycle messages** go to the engine as `PlatformEvent`s:
  - `WillEnterBackground` and `DidEnterBackground`: stop GPU work, flush saves, suspend audio.
  - `WillEnterForeground`.
  - `LowMemory`: drop caches.
  - `SurfaceLost` / `SurfaceRecreated`.
  - `DisplayChanged`, `ThermalChanged`.
- **Thermal state** isn't exposed by SDL, so add small native shims:
  - iOS: `NSProcessInfo.thermalState` via `objc2-foundation`.
  - Android: `PowerManager.getThermalHeadroom` and ADPF performance-hint sessions via `jni`.

**Platforms**

- **Mobile orientation.** Lock to landscape in the Android manifest and iOS Info.plist. On iOS, set SDL hints to hide the home indicator and defer system edge gestures.
- **Linux.** Let SDL pick Wayland or X11. Test under the Steam Runtime container.
- **Windows.** Opt in to per-monitor-v2 DPI awareness, and turn off power throttling for the process while it's in the foreground.

**Tests**

- A `platform_smoke` example per platform, built in CI. It must boot, clear the screen, echo input and survive background → foreground on device.

### 5.2 `sapphire_crash` (L0)

**How**

- **Desktop:** initialize first in `main`. `crash-handler` 0.8 catches signals and exceptions, and `minidumper` 0.11 writes minidumps from a separate monitor process. On the next launch, or from the monitor, upload them to Sentry (0.49.3+ supports native minidumps). Upload is deferred: see the M0 status in §10.
- **Android:** initialize first in `SDL_main`, with `crash-handler`. A separate monitor process is awkward on Android, so either write the minidump in-process or use Sentry's Android NDK SDK from the Gradle project (Q12).
- **iOS:** use the sentry-cocoa native SDK, linked in the Xcode project and initialized from a tiny Objective-C or Swift shim before `SDL_RunApp`.
- **Panics:** a panic hook logs the panic, adds breadcrumbs (current zone, frame number, last 64 log lines) and, in shipping builds, aborts so a minidump is produced.
- **Symbols:** shipping builds use `split-debuginfo`. Upload PDBs, dSYMs and Breakpad symbols from CI.

### 5.3 `sapphire_core` (L1)

**Owns**

- Memory: allocators and budgets.
- Handles and names (string IDs).
- Logging and profiling.
- Console variables (cvars) and errors.

**How**

- **Global allocator:** `mimalloc`. Dev builds wrap it in a counting allocator that keeps per-thread counters, a per-frame allocation count (for rule R6) and per-tag byte totals.
- **Frame arenas:** a ring of 3 `bumpalo` / `bump-scope` arenas per worker, reset when a frame retires. When the std `Allocator` trait stabilizes (merged for Rust 1.100), switch the collections to `Vec::new_in(&arena)`. Until then, use bumpalo's collections or `allocator-api2`.
- **Memory tags:** every subsystem allocation category has a tag and a budget from the tier table (§3). The stats overlay shows used versus budget.
- **Handles:**
  - `slotmap` for generational handles, wrapped as typed `Handle<T>`.
  - `Guid(u128)` for asset and document identity.
  - `Name(u64)`, an xxh3 hash, for string IDs, with a debug-only reverse table.
- **Logging:** `tracing`, with sinks for stdout and a rotating file in `user://logs`. On mobile, it forwards to `SDL_Log`, which routes to logcat on Android and to NSLog on iOS.
- **Profiling:** `profiling` crate macros everywhere. The backend is chosen in the binary: Tracy (`tracy-client` 0.18 through `profiling/profile-with-tracy`, built without Tracy's own crash handler so `sapphire_crash` keeps that job), puffin, or none. The sandbox's `--bench` mode measures the frame's CPU path and allocations without a window.
- **Cvars:** a typed, reflected registry (`cvar!(r_shadow_cascades: u32 = 4, "…")`). It can be set from config files, the dev console, live link or the command line. Shipping builds compile cvars down to constants or read-only values.
  - **As built (M1 F5, `sapphire_core::cvar`):** a cvar is a typed `static Cvar<T>` (`Cvar::new("post.exposure", 0.0, "…")`, or `with_range`) in the crate that reads it: `bool`, `i32`, `u32`, `f32` or an enum of named choices, in one atomic word read with a relaxed load. Apps register each crate's cvars into a `CvarRegistry` (no link-time registration), which finds them by name, sets them from text with typed parse and range errors, and suggests near names for typos. "Reflected" is kind, range and help through `AnyCvar`, since `sapphire_core` sits below `sapphire_reflect`. Nothing writes a cvar in shipping builds, so each reads its default; compile-time constants wait for a profile that asks for them. Config files, live link and the command line come later; the dev console is `sapphire_debug`'s (§5.20).
- **Errors:** `thiserror` in libraries; `anyhow` only in tools. Soft assertions (`ensure!`) log once and continue in shipping builds.

### 5.4 `sapphire_math` (L1)

- **glam 0.34:** `Vec3A`, `Quat`, `Mat4` and `Affine3A` for hot paths.
- **Engine types:**
  - `Transform` (translation, rotation, scale).
  - `Aabb`, `Sphere`, `Frustum`, `Ray`.
  - Critically damped springs and easing curves, for the camera and UI.
  - Piecewise curves for time-of-day.
- **Coordinates. Decision:** right-handed, Y up, meters. **Every object faces +Z in its local space, cameras included** (glTF 2.0 §3.4, VRM 1.0), so right is −X; a camera's view matrix maps its +Z forward to view-space −Z. Imported assets need no axis conversion.
- **World space. Decision:** f32, with each zone at most about 4 km across around its own origin. If you ever go seamless, add `DVec3` world positions with camera-relative rendering (see the report).
- **Determinism:**
  - Simulation code uses the `libm` crate for `sin`, `cos`, `atan2`, `exp` and `pow`, because platform libm implementations differ.
  - Basic arithmetic and `sqrt` are IEEE-exact, and Rust does not fuse multiply-adds implicitly.
  - **Determinism scope. Decision:** replays must be bit-identical *within a target architecture* (aarch64 or x86_64), and checksums are recorded per architecture. glam's SIMD types round differently on NEON and SSE2 (horizontal-add order, FMA), so cross-architecture determinism stays a non-goal until lockstep netcode needs it. Simulation state that must later match across architectures should use scalar `Vec3` and `det` functions.
- **RNG:** `rand_pcg`, seeded per subsystem and per encounter. Never use thread-local or global RNG in the simulation.

### 5.5 `sapphire_jobs` (L1)

- Wrap `bevy_tasks`' `ComputeTaskPool`, sized per §4.3, plus an `AsyncComputeTaskPool` for long tasks (2 threads on mobile).
- **API:**
  - `jobs::scope(|s| s.spawn(...))`.
  - `par_for_chunks(slice, chunk, f)`.
  - `join(a, b)`.
  - Waiting threads help execute other tasks; nothing blocks idle.
- **Name every task** for Tracy, and set thread priorities: workers normal, IO above normal, the main thread high.

### 5.6 `sapphire_ecs` (L2)

- Re-export `bevy_ecs` 0.20 and pin it per milestone. Upgrade between milestones, never during one.
- **Schedules:**
  - `Startup`.
  - `FixedFirst → FixedCombat → FixedPhysics → FixedPost`, all chained in a fixed order.
  - `Presentation`, parallel.
  - `Extract`.
  - `Cleanup`.
- **Conventions:**
  - Components are plain data.
  - Fat components hold controllers.
  - Messages carry side effects.
  - Observers handle spawn and despawn hooks only.
  - No `Query<&mut Transform>` writes outside the owning system set.
- **Spawning:** streamed spawns go through a budgeted command queue: at most N entities, or M µs, per frame.
- **Debug:** use Bevy 0.20's schedule-shuffle seed in CI to flush out ordering bugs.
- **Allocation (R6):** bevy builds a schedule on its first run, which allocates. `Ecs::startup` builds them all up front, so the fixed step allocates nothing from the first frame on (tested). Input that logs is applied just before the steps, not inside them. Spawning a new archetype mid-step also allocates (the query caches grow), so gameplay spawns will need pre-sized pools.
- **Derives:** a crate that derives `Component` or `Resource` lists `bevy_ecs` directly as well, because bevy's derive macros find the crate by name. The workspace pins one version.

### 5.7 `sapphire_time` (L2)

- **Clocks** are as in §4.5. The fixed rate is **60 Hz on every platform**, because combat frame data is authored at 60.
- **Accumulator driver:** at most 4 steps per frame. Mobile at 30 fps runs 2 steps per frame.
- **Interpolation:** presentation reads `alpha = accumulator / dt` and interpolates transforms between the last two fixed states. This keeps 120 Hz displays and 30 fps phones smooth.
- **Time-scale stack:** each entry carries a source ID (pause, perfect dodge, cinematic, debug) and is pushed and popped by handle.
- **Timers** are keyed to a clock (`Timer::on(world | real | local(entity))`).

### 5.8 `sapphire_reflect` (L2)

- Use `bevy_reflect` 0.20 for the type registry. Every serializable type has a stable type name and a schema version, and migration functions upgrade old data at load in the editor and in saves.
- **Consumers:**
  - Serialization.
  - The editor inspector and undo.
  - Cvars.
  - Live-link diffs.
  - Luau binding generation.
  - Debug inspector.
- **As built (M1 F2):**
  - Stable names are `#[type_path = "sapphire::<area>"]` attributes. `stable_name` reads them, asserting in debug builds that the path was set; `stable_hash` (FNV-1a 64) is the ID binary headers store.
  - `Versioned` carries `VERSION` and an `upgrade` that decodes the previous version's frozen type and converts it, so data of any supported age walks the chain one version at a time. Migrations are typed Rust, the same for RON and postcard. RON's untyped `Value` can't represent enum variants, so migrations never rewrite trees.
  - Serde attributes absorb compatible changes in RON (`default`, `alias`, ignored unknown fields) without a version bump.
  - `bevy_reflect` runs without automatic (`inventory`) registration; types are registered explicitly.

### 5.9 `sapphire_serialize` (L2)

- **Authored data** (editor documents, configs) is RON via serde: text, diff-friendly and mergeable in git.
- **Cooked runtime data** is rkyv 0.8 archives, memory-mapped and accessed in place.
  - Validate with bytecheck in dev builds.
  - In shipping builds, skip validation only for packs whose blake3 hash was verified against a signed manifest.
- **Saves and network:** `postcard` with a versioned header.
- **Never use `bincode`.** Version 3.0.0 is a deliberate `compile_error!` and the project is abandoned.
- **As built (M1 F2):**
  - RON documents are `(type: "<stable name>", version: N, data: ...)`. `implicit_some` is on, and errors inside `data` point at the document's own line and column. `#[reflect(Document)]` registers a loader, so `from_ron_dynamic` loads a document knowing only its type name (hot reload, live link, the editor).
  - Saves and cooked data share a 32-byte little-endian header: magic (`SPSV`, `SPCK`), header format, the stable-name hash, schema version and payload length. 32 bytes keep the archive that follows 16-byte aligned.
  - postcard is positional, so any field change to a saved type needs a version bump. `skip_serializing_if` is forbidden there.
  - Cooked access always validates with bytecheck. `access_unchecked` is `unsafe` and meant for data proven intact another way, such as a pack whose blake3 hash matched its signed manifest. A version mismatch means "recook" (`Stale`); there are no upgrades for cooked data.

### 5.10 `sapphire_assets` (L2)

**Owns**

- Runtime asset identity and handles.
- Pack mounting, async load pipeline, residency and budgets.
- Hot reload in dev builds.

**How**

- **Identity:** `AssetId = Guid`. `Handle<T>` is strong (ref-counted) or weak, generational, and `Copy`-cheap. Assets never use string paths at runtime.
- **Pack format (custom):**
  - Header and signature.
  - A table of contents (`Guid → offset, size, codec, blake3`).
  - Data compressed in 64 KB blocks for random access: zstd for most data, lz4 for data that must decode fast.
  - Packs are grouped per zone, per character, per encounter, plus one shared pack.
- **Load pipeline:** request → IO thread read → decode job (decompress, fix up) → GPU upload queue (budget: 16 MB per frame on desktop, 4 MB on mobile) → ready. Dependencies load first, and a parent becomes ready only when its children are ready.
- **Residency:**
  - Each category (textures, meshes, animation, audio banks) has a tier budget.
  - Eviction is LRU, skipping pinned assets: the current party and the active encounter.
  - Texture mip streaming comes *later*.
- **Hot reload (dev):** two triggers:
  - Desktop: watch the cooked-output directory with `notify` 8.2 plus a debouncer.
  - Devices: live link pushes the asset to the device.
  - The asset is swapped in place behind the same handle.
- **As built (M1 A1, `sapphire_assets::AssetId`):**
  - `AssetId` is 16 bytes, `Copy` and ordered by its bytes. Its text form is the canonical UUID spelling (8-4-4-4-12 lowercase hex), and parsing accepts only that, so an ID has one spelling and a text search finds every use.
  - serde writes the text in human-readable formats (RON) and the 16 bytes in binary ones (postcard). rkyv archives an `AssetId` as itself (alignment 1), so cooked data hands out `&AssetId` in place.
  - IDs are random, laid out as version 4 UUIDs. Only the pipeline creates them; the runtime parses and compares them and never needs entropy.
  - **Decision:** addressing cooked data by ID waits for packs (A4), whose table of contents is the runtime's ID index. A1 adds no ID-to-path manifest: rule R5 keeps path strings out of the runtime, and A4 would replace it. Until then the sandbox loads `samples/mannequin.smesh` by path.
- **As built (M1 A2, `sapphire_assets::texture`, `sapphire_render::texture`):**
  - A cooked texture is a KTX2 file restricted to one 2D image and its mip chain: one of 20 formats (RGBA8 sRGB, R8, RG8, R16, RG16, RGBA16F, RGB9E5, BC4, BC5, BC6H, BC7 linear and sRGB, ASTC 4×4 to 8×8 LDR), no supercompression (packs compress, A4), and level 0 a whole number of blocks, which WebGPU requires. Levels are tightly packed and stored smallest first at KTX2's alignment.
  - The key/value data holds `KTXswizzle` where the stored channels aren't the logical ones (ASTC keeps two channels as luminance plus alpha, so normals and two-channel masks carry `ra01`), and `SapphireTextureVersion`. The reader rejects other versions, other data format descriptors and anything malformed without panicking: it compares the descriptor's bytes before the `ktx2` crate parses them, because ktx2 0.5.0's parser panics on some malformed descriptors. `Texture` borrows its levels from the file, so reading copies nothing.
  - **Decision:** KTX2 rather than an engine format. It already is a container of GPU-ready levels with a self-describing format, standard tools read it, and packs can hold it as one blob per texture. The writer sits next to the reader, on the `ktx2` crate's header and DFD types, because ctt's own KTX2 writer can't add key/value entries.
  - The renderer uploads every level with one `Queue::write_texture` after checking that the device can filter the format. A device without its cook's formats (BC without BC support, ASTC without ASTC) gets an error naming the cook it needs. `check_format` runs the same check without uploading; the boot check of §6.1 calls it once the runtime knows its cook (A4). `GpuCaps` also enables `TEXTURE_FORMAT_16BIT_NORM` now, for the desktop cook's 16-bit SDF maps. Binding textures into materials, and applying their swizzle in shaders, is A3's.
- **As built (M1 A5, `sapphire_assets::mesh`):**
  - A cooked mesh is a `Mesh` rkyv archive (schema 2; M0's flat 48-byte-vertex layout was 1) in `sapphire_serialize`'s cooked envelope, read in place. `Mesh::read` checks the envelope and bytecheck, then the invariants a GPU upload relies on: stream lengths, every index inside its LOD's vertex prefix, ranges, weights summing to 255, finite values. The renderer uploads the archived streams byte for byte.
  - Three vertex streams, so each pass reads only what it needs: **positions** as xyz unorm16 across the bounds (8 B; error `extent / 131 070`, 16 µm on the 2.1 m mannequin) or, by import setting, f32 (12 B, for meshes too large for 16 bits or whose edges must meet a neighbor's exactly); **shading**, the normal and outline normal octahedral snorm16 × 2 each (8 B, within 0.008°); **surface**, UV unorm16 across the UV bounds plus four bytes: the vertex color's three data channels and a flags byte since M1 C1 (§5.12), an RGBA8 color before (8 B, 0.016 texels of a 2K map). 24 bytes a vertex where M0 read 48. 8-bit normals (about 1°) were rejected because the two-tone light split is a hard threshold on N·L, and float16 UVs because their 1/2048 steps near 1.0 are half a texel of a 2K map.
  - **LODs:** LOD0 first, each LOD per-material index ranges over a prefix of the streams (vertices are ordered coarsest LOD first, so a skinning pass for a distant character skins only its prefix), with the simplifier's error estimate in object-space meters for LOD selection (it folds in the weighted normal error, so it is conservative).
  - **Skins:** the skeleton (joints parents-first, with names and local rest TRS: ozz-animation's shape), a palette of (joint, inverse bind matrix) entries, and per-vertex influences: four palette indices (u8 for palettes up to 256 entries, else u16) and unorm8 weights summing to exactly 255. **Morph targets:** names, default weights and sparse deltas per target (vertex u32, binary16 position and normal deltas: 16 B). These are already the storage layouts compute skinning (R2) reads.
  - The element encoders and decoders live together in `sapphire_assets`, so cooker and runtime can't drift apart; `Mesh::from_vertices` packs procedural single-LOD meshes (tests, the headless example). A skinned mesh embeds its skeleton until animation cooks skeletons as assets of their own (M2). Materials became material slots in M1 A3 (below).
  - The mannequin: 251,904 bytes in M0's format; 230,092 bytes desktop with four LODs (its vertex data halves, 177 → 88 KB; LOD0 alone would be 162 KB) and 117,480 bytes mobile (three LODs, from 3,130 triangles).
- **As built (M1 A3, `sapphire_assets::material`):**
  - **Documents:** one `Versioned` RON document per material family, named `sapphire::render::AnimeCharacterMaterial`, `EnvironmentMaterial`, `FoliageMaterial`, `SkyMaterial` and `FogCardMaterial` (version 1), reflectable and registered for loading by name (`material::register_types`; `AssetId` is now an opaque reflected value). Every field has a default, so families gain fields without a schema version. Colors are linear, as glTF factors are; textures are referenced by `AssetId` only (rule R5), and each family lists its texture slots with what they must hold (`TEXTURE_SLOTS`, a `TextureUsage` per slot).
  - **Anime character** (C1's asset contract): a surface (body; face with its SDF shadow map, horizontal and vertical thresholds, plus a mask map and softness; hair with a highlight mask and a painted-band or angel-ring highlight; eye with a mask and parallax depth), base color and map, alpha mode, the control map in A7's ILM layout (R specular mask, G shadow bias, B specular threshold, A material ID, which picks the region), a normal map, the light split (threshold and softness), the diffuse mode, up to 8 regions, rim, specular model, matcap and outline. Diffuse is **bands** (two tones, or three with a deep-shade threshold, colors per region, ZZZ) or a **ramp** (a ramp texture in A7's layout, a day and a night row per material ID, which a region may point elsewhere, mixed with a per-region subsurface tint as WuWa's `_RampInt`). A region carries its shade and deep-shade colors, ramp rows, subsurface tint, specular color and size, rim tint and outline color and width. Face, hair, eye, alpha test, transparency and outline are the permutation-relevant flags. The other families carry what world_lighting_post.md asks of them: stylized GGX (wrap, a terminator whose width grows with distance and an optional band ramp, shadow tint, a clamped specular in a roughness range, distance desaturation, emission), NPR foliage (alpha cutoff, wrap, canopy occlusion, translucency, a root-to-tip gradient, three-tier wind), the sky's fixed parts (sun and moon discs, stars and the Milky Way, card clouds and cirrus with scroll and dissolve; time of day supplies its colors) and fog cards (tint and opacity, a mask, scroll, depth and camera fades, sun scatter).
  - **Cooked materials** (`.smat`, schema 1): the family, its flags, the family's GPU block exactly as the shaders read it, and per texture slot the texture's ID and the swizzle it was cooked with. Blocks are `#[repr(C)]` structs mirrored in WGSL: 896 bytes for the anime character family (128 of header and 8 regions of 96), 144 environment, 160 foliage, 96 sky, 80 fog card. Each holds the bound-slot bits and one 12-bit swizzle code per slot, two to a word, so shaders read ASTC's two-channel textures (`ra01`) as BC5's. `Material::read` checks the envelope and bytecheck, then the block size, the slot count, every swizzle, that unbound slots have none, the flags, and the block itself: every float finite (one NaN pixel spreads through bloom), and its bound-slot bits and swizzle codes equal to the bindings'.
  - **Decision:** the cooked form is the packed GPU block rather than an archived document. The renderer then copies it into its slot byte for byte, as it does mesh streams; packing lives here, so runtime tools (live link, the overlay) can pack an edited document too; and a change to a block's layout or packing bumps `Material::VERSION`, which is in the materials' cache key, while a compile-time check makes `Mesh::VERSION` move with it, since meshes embed generated materials.
  - **Decision: meshes and their materials.** Mesh schema 3 replaces M0's embedded parameters with `MaterialSlot`s, named after the source's materials. A slot either embeds the material the importer generated (`SlotMaterial::Embedded`) or names a material asset its sidecar bound to it (`SlotMaterial::Asset`). Generated materials make every glTF draw as authored with no setup, and the mannequin's five materials carry their parameters over exactly; documents carry authored ramps, control maps and SDF face maps. This suits VRM (A6): MToon maps onto generated anime character materials, and the look-dev character's slots bind to documents that use A7's maps. Embedded and standalone materials are the same `Material`, so the runtime has one material path, and editing a material asset recooks only it. Generated materials reference no textures, so a mesh never depends on a texture's cook: textured materials are documents. VRM import (A6) therefore extracts a file's images as texture sources and its textured MToon materials as documents bound in the mesh's sidecar, while generated materials, from MToon's factors, cover its untextured slots. Importers never write source files and a cook never rewrites a sidecar, so that extraction is an explicit pipeline command. Until packs resolve IDs (A4), an app draws only embedded materials; a slot bound to an asset fails to load in the sandbox with a message naming A4.
- **As built (M1 C2, `sapphire_assets::material::RegionGi`):** an anime character region carries its share of the character light rig's stylized GI (§5.12): `gi: (sphere, skin)`, both 0 to 1 and 0 by default, so documents without them are unchanged. `sphere` turns the region's GI normals toward a sphere around the character's head (faces, hair); `skin` gives its GI the rig's skin tint. They ride in the region block's spare channels (the subsurface and rim vectors' fourth components), so the anime character block keeps its 896 bytes. The packing changed, so `Material::VERSION` is 2 and `Mesh::VERSION` 5 (the documented pairing), and every material and mesh recooks; the sample swatch documents gain the written-out field and cook to the same bytes.
- **As built (M1 A6, `sapphire_assets::rig`, `character_maps::ilm`):**
  - A **character rig** (`.srig`, `CharacterRig` schema 1, in the cooked envelope) holds what a VRM says about a character beyond its mesh, in VRM 1.0's model: the humanoid (VRM 1.0's 55 bones, sorted, each bone and joint once, the 15 required ones whenever there are any), expressions (presets in a fixed order, then custom ones by name; each a set of morph-target weights, the material colors it sets and the UV transforms it applies, with binary and blink/look-at/mouth override flags), look-at (eye origin in head space, bone or expression kind, four range maps), spring bones (chains of joints with hit radius, stiffness, gravity and drag, collider groups, an optional center joint, and a tip that is either a joint, as in VRM 1.0, or a virtual tail past the last joint, as VRM 0.x's 7 cm leaves are), sphere and capsule colliders in groups, and roll, aim and rotation constraints with weights. Nothing simulates or applies it until `sapphire_anim` (M2, §5.13).
  - It addresses its mesh's skeleton joints, morph targets and material slots by index, since both are cooked from one import, and records how many of each the mesh had. `CharacterRig::read` checks every index against those counts, every number's range (finite, radii and stiffness at least 0, drag and weights in `[0, 1]`, unit gravity directions), unique names and bones, that presets carry their names, and that each joint has at most one constraint and constraints form no cycle; `check_mesh` pairs a rig with a mesh by the three counts.
  - **Decision:** the rig is an asset of its own, not part of the mesh. It is animation's data: the renderer and the mesh format don't need it, so `Mesh::VERSION` doesn't move, a VRM's mesh loads and draws like any other, and when skeletons become assets (M2) the rig moves next to them. The pipeline cooks it as a sub-asset of the VRM, with a derived ID (§5.27).
  - The ILM **shadow bias** now has a scale: G holds an offset to N·L of `2·G − 1` (`ilm::encode_shadow_bias`), spanning N·L's own range with 0.5 neutral and higher values lighting a texel; the ≥ 0.95 and ≤ 0.05 locks keep their meaning. MToon's shading shift texture is that quantity, so A6 writes it there; C1 decodes G the same way.
- **As built (M1 C3, `sapphire_assets::{character_maps, material}`):**
  - `character_maps::face_sdf` adds the light in head space the face map is compared with, as a reference (`HeadLight`, `lit_margin`) the shader and its tests share: the horizontal angle (the yaw, faded by the cosine of the elevation), the vertical angle (the elevation), the two channels intersected, R mirrored by the light's side and blended toward the darker side near the head's forward–up plane (`MIRROR_BLEND`), G never mirrored (§5.12 has the reasons). A one-channel map reads G as 1 (`ONE_CHANNEL_SWIZZLE`, `r101`).
  - Two face layouts join A7's: the **face mask** (`face_mask`, the face material's surface mask slot: R nose line, G face highlight, B always lit; A unused) and the **expression map** (`face_expression`: R blush, G gloom; B and A unused), each channel scaled by a weight the frame packet animates per character (`CharacterFace`, §4.6). A3's documented face mask had blush in A; nothing read it, and expressions now share a map of their own.
  - The anime character family gains texture slot 7 (`expression`, a mask) and the face's settings (nose line threshold, highlight strength and reveal, blush color) in its block's `surface` and `surface_color`: `Material::VERSION` 3, `Mesh::VERSION` 6.
- **As built (M1 A4, `sapphire_assets::{pack, AssetServer}`, `sapphire_platform::IoThread`):**
  - **Packs** (`.spack`, version 1): a 256-byte header (magic, version, counts, the table of contents' blake3, a hash of the header's first 128 bytes, and a signature block of scheme, the signing key's ID and 64 signature bytes, zero while unsigned); a table of contents of 80-byte entries sorted by ID (kind, codec, size, the asset's blake3, its blocks and its dependencies), 48-byte block records (offset, stored length, codec, the stored bytes' blake3) and the dependency IDs; then block data, each block 16-byte aligned. Each asset is exactly its cooked bytes, at most 1 GiB (three times the largest texture the engine cooks, so a crafted pack can't make a reader allocate more), cut into 64 KiB blocks compressed independently, so any block decodes alone; a block that doesn't shrink by 1/64 is stored raw. Mounting checks the header hash, the table's hash and every structural invariant (order, counts, ranges, layout, the file's length); decoding checks each block's hash before decompressing it, the asset's hash after, then validates the asset as its kind and compares its references with the listed dependencies. Signing comes with `sapphire_patch` (M4): a signature will cover the header hash, so the reserved block keeps the layout; readers reject any scheme today. Corrupt, truncated and hostile packs are errors, never panics: tests flip every byte of a small pack, and cover bad headers, tables, blocks, truncation and malformed compressed blocks.
  - **Decision: codecs.** zstd is the reference C library (`zstd` 0.13, libzstd 1.5.7, built with cc for every target, as the tools already built it) rather than the pure-Rust `ruzstd`. Measured on the M5, one core, 64 KB blocks of the cooked mannequin and of BC7 and ASTC textures: libzstd decodes 1.0–1.3 GB/s and ruzstd 0.9 0.27–0.35 GB/s, which on a Mobile-Low phone is the difference between a zone load bound by flash reads and one bound by decoding; ruzstd's encoder also stops at its fastest level (the mannequin at 0.57 of its size, against level 19's 0.37). Blocks are hash-checked before they are decoded, so corruption never reaches the C decoder; ruling out hostile packs is the signatures' job (M4), and until then they meet libzstd's own checks on malformed frames. lz4 is `lz4_flex` 0.14: pure Rust, no unsafe code, 4–5.5 GB/s. Which kind gets which codec is the packer's (§5.27).
  - **Load pipeline:** `mount` reads a pack's header and table, which become the runtime's only index from ID to data (rule R5). `load` returns a handle at once and requests the asset's dependencies first, which must be of a lower kind (meshes on materials, materials on textures), so no pack can make chains deep or circular; loading an asset that failed tries it again, with its failed dependencies; each asset is one positioned read of its blocks through the app's `PackSource` (the platform's IO thread, §5.1, by priority); the read's completion starts a decode job on the async compute pool (`sapphire_jobs::spawn_async`), whose result comes back over a channel that `update` drains. Cooked formats use relative pointers, so the decode job's validation is the whole fix-up: assets are read in place from then on. A decoded asset waits until its dependencies are ready, then joins the upload queue, and `upload` hands queued assets to the app's uploader as plain cooked data until the frame's budget is spent (16 MiB desktop, 4 MiB mobile; one asset always goes, however large). The renderer sees cooked data and its own handles, never the server (rule R4). Meshes and textures drop their decoded bytes once uploaded; materials keep theirs for CPU readers. A6's rigs (`.srig`), animation's CPU data, are a kind of their own that never goes to the GPU: ready once decoded, bytes kept. `reupload_all` serves a new renderer (device loss, a tier change), reading again what was dropped.
  - **Handles:** `Handle<T>` (`Mesh`, `Material`, `TextureAsset`, A6's `CharacterRig`) is one `Arc` pointer: clone and drop are an atomic each and allocate nothing, so dropping handles is allowed in the fixed step (rule R6); the server sees an asset's last user go by the count on its next update. `WeakHandle<T>` is a slotmap key (index and generation, 8 bytes, `Copy`) that resolves to nothing once its asset is unloaded, even after the slot is reused. Strong handles are checked against the server they came from.
  - **Residency:** per-kind budgets: meshes 768 MiB, textures 2.5 GiB, materials 32 MiB and rigs 64 MiB on desktop; 192 MiB, 600 MiB, 8 MiB and 16 MiB on mobile (the texture numbers are §3.4's zone pools, the others proposals). Unused assets stay cached until their kind is over budget, then go least recently used first, skipping pinned ones; assets in use are never unloaded, so a kind can stay over budget, which the stats show. `unload_unused` drops the cache on `LowMemory`. Unloads reach the app with the asset's slot, so it frees only that load's GPU copy: textures are freed; the renderer keeps mesh and material slots until it gains residency of its own.
  - **Material slots resolve:** a slot bound to a material asset loads its material and the material's textures first, and `Renderer::upload_slot_materials` takes their handles. A GPU test draws a packed sphere through a packed material sampling a packed texture.
  - **The sandbox** loads the mannequin by its ID (`8659a15c-…`) from `shared.spack` on every platform: from the content directory on desktop, the app bundle on iOS, and the APK's assets on Android, where Gradle's `noCompress` stores the pack as it is so block reads seek. Captures are byte-identical to path loading in all six golden scenes, and the per-frame allocations of `--bench` are unchanged (488.3). Mounting takes 0.1–0.6 ms and the mannequin is on the GPU 0.7–0.8 ms later (M5, debug build, under load from parallel builds).
  - **Stats (rule R9):** `AssetStats` (assets by state, unused and pinned, bytes resident per kind against budget, the last frame's uploads, reads in flight, loads, unloads and failures), in the debug overlay's stats panel.
  - **Tests:** pack round trips, random block access and the corruption cases above; strong and weak handles and generation reuse; residency's budget, LRU order and pinning; dependency order, kinds and failure propagation, and retrying failed assets; the per-frame upload budget; and the soak test: 300 rounds of loading and dropping characters under tight budgets (about 2,800 loads and unloads), after which no slot, byte or weak handle survives and live heap bytes are back exactly at their baseline (a counting allocator).
- **As built (M1 A8, `AssetServer::remount`, `sapphire_hot_reload`):**
  - **Decision: the desktop trigger watches the packs**, in the content root apps already read (`target/content/<cook>/`), not the loose cooked files (`target/content/loose/<cook>/`, which stay the tools'). A pack is written whole, to a temporary file renamed into place, after every asset of its cook, so an asset and its dependencies always come from one cook, where loose files land one by one (a material could be seen naming a texture not yet written). Remounting goes through the load pipeline itself, by asset ID through the table of contents (rule R5), where loose files would need an index from paths to IDs and kinds the runtime otherwise never has. And it is live link's path too: a phone has no cook directory, and A9 pushes packs. So `cargo xtask content` after an edit is what reaches the running app; a cook that changes nothing rewrites nothing.
  - **Remount:** `AssetServer::remount(name)` reads a mounted pack's header and table of contents again and checks them as `mount` does. A pack whose header is unchanged (it holds the table's hash) changes nothing; one that fails to read or check, or holds another mounted pack's asset, keeps the mounted version. Otherwise the new table becomes the index of the pack's assets, every loaded asset of the pack moves to its new entry (all of them before any load starts, so a load that reaches a dependency finds it where the new pack holds it), and each is compared with its entry by blake3 hash: an unchanged asset only moves to its new entry (and a read in flight is requested again, since the old file is about to close); a changed one in use or pinned loads its new version; a changed one nothing uses is unloaded (the cache's copy is stale; the next load reads the new one); one still loading starts again from the new pack, dependencies included; a failed one still in use is tried again; one the pack no longer holds keeps its version until nothing uses it (`AssetChange::Removed`), but can't be read again, so a new renderer can't get it unless its kind keeps its bytes. An asset moving to another mounted pack isn't followed yet (group packs, M2). The old file is then closed (`PackSource::close`, new, a no-op by default; the sandbox's closes the IO thread's file). `RemountReport` counts changed, removed and reloading assets.
  - **Swapping behind handles:** a ready asset loads its new version (`Slot::next`) while it stays ready and readable: the read goes through the same IO thread and decode job, its dependencies (from the new table) load first, and it joins the same upload queue and per-frame budget, after any dependency that is itself reloading, so a material binds its texture's new version. The uploader gets it as the asset's next **generation** for the same `AssetSlot` (`Upload::generation`): an app holding that slot at a lower generation replaces its copy in place, one holding it at the same generation (assets are sent again after `reupload_all`) keeps it. Once uploaded (or decoded, for rigs) it replaces the old version behind every handle (`get` reads it), dependencies only the old version had are released, `AssetServer::generation` goes up and `take_changes` reports `AssetChange::Reloaded`: CPU systems that derive data from an asset (animation from a rig, M2) compare generations or read the changes. A new version that fails (a corrupt block, an invalid asset, a dependency that won't load, an upload the app refuses) is dropped and reported (`ReloadFailed`); the asset keeps its version. New versions waiting on one that is dropped (failed, superseded by an edit undone, its asset gone) go on with the version it has.
  - **Dependents follow:** kinds that bind their dependencies' GPU objects (`AssetKind::binds_dependencies`: materials, whose group 3 holds their textures) are sent again from their kept bytes, as a generation of their own, when a dependency is replaced; a compile-time check keeps such kinds keeping their bytes. A mesh names its materials by the renderer's handles, which a material updated in place keeps, so it needs nothing. A new renderer taking over while a reload is in flight (`reupload_all`) gets the new version, as the next generation, and materials re-sent to it after their texture changed come as one too, so an app that keeps its old renderer rebinds them.
  - **Live link (A9)** drives the same path from a device: `MemoryPackSource::insert` replaces a pack (it now numbers each open file, closing frees it) and `remount` swaps in what changed. Pushing only the changed assets, as a pack mounted over the shared one, is A9's (`sapphire_assets` crate docs, later milestones).
  - **Desktop trigger:** `sapphire_hot_reload`, a new `dev_only` crate. `notify` 8.2 (FSEvents, inotify, ReadDirectoryChangesW) watches directories, not recursively, behind a trailing-edge debounce on a thread of its own: a burst is reported once its files have been quiet for 100 ms, however long it lasted, each file once. `notify-debouncer-full` was tried first and dropped: it reports each event once that event is older than the timeout, found by a thread waking every quarter of it, so a burst longer than that splits into several reports, and idle it wakes 40 times a second; its rename stitching isn't needed, since a rename's event names both files. `HotReload` watches the content root's packs (a changed `<name>.spack`, written, renamed into place or removed, is a pack to remount; the temporary files packs are written to are ignored), the renderer's WGSL sources and documents, and `poll` (once a frame, one empty channel read when nothing changed, no allocation) reports what changed and reads the changed files. Where the engine's sources are comes from the crate's place in the repository at compile time (`sources::shader_dir`, `sources::default_day`, `sources::default_light_rigs`); tests check they are the files the engine compiles in.
  - **Dev builds only, desktop only:** the sandbox reaches the crate through its `dev` feature, and its `notify` feature (the backend, rule R8; on by default only for the crate's own tests) only through a dependency declared for non-mobile targets, so iOS and Android dev builds compile it without file watching (`HotReload::new` reports `Unsupported`) and link no `notify`; `cargo tree` for both targets finds none. `xtask check-layers` gains a `[dev_only_dependencies]` list (`egui`, `egui-wgpu`, `notify`): a runtime or app crate reaching one except through a dev-only crate fails the check, so a build without `dev` can't link the overlay or the watcher. `notify` 8.2.1 is CC0-1.0; it brings `notify-types`, `walkdir` and `same-file`, plus `objc2-core-services` (FSEvents) on macOS and `inotify` and `mio` on Linux, and shares the workspace's `windows-sys` 0.61 on Windows; `cargo deny` passes.
  - **Non-asset documents:** watched documents are reported with their text, for the app to parse and apply. The sandbox reloads two, both compiled into `sapphire_world` and watched in one directory: the default day (`default_day.ron`), whose timeline, if it parses and is complete, replaces time of day's base timeline (`TimeOfDay::set_timeline`), and the character light rig presets (`default_light_rigs.ron`, M1 C2), whose library, if it parses and is valid, replaces the scene's `LightRigLibrary` resource, characters keeping their preset IDs (§5.21's fallback for an ID past a shorter library's end). Either one that doesn't keeps the old. The quality-tier table and the sandbox's bindings wait (`sapphire_hot_reload` crate docs, later milestones).
  - **The sandbox:** on by default in `dev` builds (`--no-hot-reload` turns it off), in the windowed app and the offscreen modes alike, applied each frame before loading runs. Its uploader keeps each asset's slot and generation and replaces in place: a texture is uploaded anew (materials binding it are rebound when they come again), a material rewrites its slot (`Renderer::update_material`), a mesh replaces its streams under its handle (`Renderer::update_mesh`, §5.12), so the scene's handles stay valid. A renderer made later (a tier change, device loss) gets the shader sources an edit gave the last one. Errors are logged and listed in the overlay's stats panel (§5.20). A headless run (`--bench` with `--swatches`, release, dev) showed it end to end: the watcher started 80 ms after the content; a syntax error appended to `sky.wgsl` was refused as `sky.wgsl:32:12: expected identifier` and the sky kept drawing, a valid edit rebuilt the sky program's one pipeline, and restoring the file rebuilt it again; a valid edit to `default_day.ron` reloaded it and a broken one was refused (`Non-whitespace trailing characters`); editing a swatch material's base color and running `cargo xtask content` remounted `shared` (1 changed, 1 reloading) and swapped the material in as generation 1, and restoring it as generation 2. Run again on W2's sky (`--swatches`, 640×360): a syntax error in `sky.wgsl` was refused at its line, a comment rebuilt the sky's six pipelines in 4.7 ms, and `default_light_rigs.ron` reloaded, was refused when broken and reloaded when restored.
  - **Tests:** remounting a pack whose texture changed reloads it behind its handle (same slot, generation 1) and sends the material that binds it again, not the mesh; a material moved to another texture loads the new texture first and releases the old one; a texture and the material binding it, both changed, upload texture then material once; a corrupt block, an invalid material and a refused upload each keep the old version and report; an asset removed from its pack keeps its version, fails a new renderer, and loads again when it is back; changed unused assets are unloaded and assets mid-load start again with the new bytes; rigs reload without the GPU; a new renderer taking over a reload gets the new version and the rebound material as generation 1; remounting an unchanged, corrupt or conflicting pack changes nothing; a new texture two edited materials share that fails fails both their new versions; a failed character retried by a remount whose pack has fewer assets (so every entry moved) loads; a material's new version waiting on its texture's goes on when that edit is undone; an asset re-sent to a new renderer that changes meanwhile comes as a new generation. An independent review found the last four before they were fixed (two panics, a stall, a lost generation). A soak test remounts a pack between two versions (half the textures, a third of the materials' textures and a quarter of the meshes differ) over 300 rounds of loading and dropping under tight budgets, half the time undoing the edit before its reloads land: the app is never sent an older version than it holds, every reload it is told of is a generation it holds or has passed, and afterwards no slot, byte or GPU copy remains and live heap bytes are back at their baseline. Watching real directories: a burst of 30 writes over half a second is one change; packs, shaders and documents are reported as such, the pack's temporary file and unrelated files not at all; removed and renamed packs are changes, and a removed shader keeps its text and reports the error; builds without the backend watch nothing. The A4 test of the upload budget asserted a frame count that depends on the order decodes finish in; it now bounds what the order can't change.
  - **Cost:** idle, nothing measurable. Sandbox `--bench 3000` (1920×1080, 4× MSAA, Apple M5, release, `dev` builds, the watcher started on packs, shaders and the default day) against a `dev` build of c3ac568, two sets of three alternating rounds on a machine shared with four other builds: CPU p50 90.6, 93.2 and 94.1 µs against 90.6, 94.2 and 96.6 (the first set's first A8 round, 152.7 µs with a 6.5 ms p99, was a burst of outside load; its other rounds 96.6 and 94.2 against 97.7 and 99.5); the timed GPU frame 197, 256 and 260 µs against 199, 257 and 223; 305.3 allocations and 75,387 bytes per frame on both, so the per-frame poll allocates nothing. Without `dev`, CPU p50 92.3–92.8 µs against 91.3–92.0, the same 305.3 allocations and 74,738 bytes: the remount and generation bookkeeping costs nothing while no pack changes. A remount takes 0.1–0.3 ms of its frame (reading the header and table of contents, which waits on the IO thread, checking them and comparing the loaded assets); the new versions then load as any asset does.

**Tests**

- Load and unload soak tests checking for leaked handles.
- A pack-corruption test: a bad hash must be rejected.

### 5.11 `sapphire_rhi` (L2)

**How**

- **Decision:** don't write a full RHI trait now. `sapphire_rhi` re-exports wgpu 30 and adds engine helpers, and only `sapphire_rhi` and `sapphire_render` may touch wgpu. A future console backend then plugs in through wgpu's custom-backend hooks (expanded in v30) or by rewriting two crates, not the engine.
- **Device creation and feature tiers:** probe the adapter and fill a `GpuCaps` struct:
  - BC and ASTC texture support.
  - Timestamp queries, including whether they work inside passes.
  - Multi-draw indirect (MDI) and MDI-count.
  - Binding arrays (bindless).
  - `shader-f16`.
  - Pipeline-cache support.
  - Limits.
  - The renderer selects passes by `GpuCaps` plus the tier table, never by OS.
- **Uploads:** per-frame data goes through a `wgpu::util::StagingBelt`: mapped staging buffers reused across frames and copied in the frame's own encoder. `queue.write_buffer` creates a new staging buffer per call (it was most of the M0 frame's CPU time), so it's for one-off uploads such as meshes.
- **Transient pools:** textures and buffers pooled by descriptor and reused across frames. wgpu can't alias GPU memory, so pooling is the substitute.
- **Pipeline cache:** on Vulkan, create `wgpu::PipelineCache`, load it from `user://` at boot and save it at shutdown and after load screens.
- **Surfaces:**
  - Recreate on resize, on `SurfaceLost`, and on Android surface destroy/create.
  - Present mode: FIFO on mobile; FIFO by default on desktop, with Mailbox / Immediate as user options.
  - HDR surfaces come *later*.
- **Debug:**
  - Label every resource.
  - GPU pass timings via `wgpu-profiler` 0.28. **As built (M1 F5):** the frame graph times its passes with its own timestamp queries instead (§5.12, debug views; why in §5.20).
  - A capture hotkey that calls wgpu's `start_graphics_debugger_capture` (RenderDoc, Xcode or PIX).

### 5.12 `sapphire_render` (L3)

The big one. It owns everything from `FramePacket` to pixels.

**Frame graph**

- Rebuild the graph in code each frame (cheap). Each pass declares the virtual resources it reads and writes. Compile does this:
  1. Cull passes whose outputs are unused.
  2. Order the passes.
  3. Compute resource lifetimes.
  4. Assign textures from the transient pool.
- Record passes on workers into separate `CommandEncoder`s, then submit them in order in one `queue.submit`. wgpu tracks barriers, so the graph's job is ordering, culling, pooling and parallel recording.
- Choose passes per tier, from the data table in "Quality tiers" below.
- **As built (M1 R1, `sapphire_render::graph`):**
  - Passes run in the order they're added; compile checks every read follows a write.
  - A write replaces a texture's contents, so a pass that keeps them (`LoadOp::Load`, partial draws) also declares a read. Culling relies on this.
  - Transients with equal descriptors and disjoint lifetimes share one pooled texture; the pool (`sapphire_rhi::TransientPool`) matches textures across frames and evicts ones unused for 60 frames.
  - Under 4 running passes, recording stays on the calling thread in one encoder; from 4, it splits into encoders of at least 2 passes on the job pool.
  - Cost: about 10 heap allocations per frame (node lists, one boxed closure per pass) and under 1 µs of CPU for M0's two passes. Uploads are side-effect passes added first. Buffers stay outside the graph (M1 R2's note, "Skinning and morph targets", says why).
- **As built (M1 W2):** the sky adds a side-effect pass, `sky cube`, before the main pass: one face of the persistent sky cube (§5.12, "Sky"), drawn through that face's view, so a reader added after it sees this frame's face. It is timed like the main pass. The main pass draws the sky where it did, after the opaque surfaces and outlines and before transparents, through the sky's own pipeline variants.

**Scene and draws**

- **Persistent GPU scene:** an instance buffer (transforms, material IDs) and a material parameter buffer (storage buffer indexed by material ID), updated from the extract deltas.
- **Mesh pool:** large shared vertex and index buffers with offsets, which means fewer rebinds and makes MDI possible later.
- **Culling:**
  - v1: CPU frustum, distance and small-object culling, parallel per view (main camera and each shadow cascade).
  - Later, on desktop: GPU Hi-Z occlusion culling plus MDI.
- **Sorting:** opaque draws by pipeline → material → mesh, with instancing. Transparents back to front per emitter or object.
- **As built (M1 F3, `sapphire_render::scene`):**
  - Instances live in slots of the instance buffer, found by `InstanceId` (a hash map with a one-multiply hasher) and reused through a free list. A CPU mirror holds every slot's GPU data. A packet rewrites only the slots it touches, and the uploads pass writes those through the staging belt in contiguous runs.
  - Draw records name their instance by slot. The draw list is therefore rebuilt, and its records uploaded, only when instances are added or removed, change mesh, or meshes are uploaded; a move costs one 112-byte upload (176 bytes since M1 R2, plus a second upload once the instance comes to rest; see that package's note). `RenderStats` counts changed instances and draw-list builds.
  - A grown instance buffer is filled by a GPU copy of the old one, so instances uploaded earlier aren't sent again.
  - Draws stay in slot order. Sorting comes with a second opaque pipeline, and costs nothing on frames that only move instances, since the list is rebuilt only when the set changes.
- **As built (M1 W4):** a second material family draws on meshes, so draws now sort by pipeline: the draw list groups them by position format and surface variant (a coverage, and the anime character shader for a part or the environment shader), whatever order instances and submeshes come in, and families mix freely within a mesh. Within a group they keep slot order; sorting by material and mesh stays later work. "Environment" below has the details.

**Skinning and morph targets**

- A compute pass skins every visible skinned instance once per frame into a per-instance vertex buffer, applying blendshape (face) weights in the same dispatch.
- The prepass, shadow, opaque and outline passes all reuse that buffer, so a character is skinned once rather than four times.
- **As built (M1 A5, `sapphire_render::{mesh_pool, pipelines}`, `shaders/frame.wgsl`):**
  - The vertex shaders decode the cooked streams (§5.10) as uploaded: `decode_vertex` in `frame.wgsl`, called by the anime character and outline vertex shaders. Vertex slots are 0 the draw record, 1 positions, 2 shading, 3 surface, so a position-only pass (shadows, R2's prepass) binds slots 0 and 1 alone.
  - The draw record grows from 8 to 48 bytes to carry its mesh's dequantization (position offset and scale, UV transform). Decoding then costs no binding: a mesh table would have been a third vertex-stage storage buffer against the budget in the render crate docs. f32-position meshes use the same shaders with an identity transform; only slot 1's attribute differs, and their pipelines are built when the first one is uploaded.
  - The mesh pool keeps one slab of stream buffers per position format, each with a single vertex numbering, as base-vertex multi-draw-indirect will need; indices and materials are shared. Uploads copy the archive's streams as they lie. Every LOD's indices are uploaded and LOD0 is drawn until LOD selection lands with culling. The cook merges submeshes by material, so the mannequin draws 5 times per pass instead of 9.
  - **Skinned meshes draw in bind pose**, morph targets at weight 0. Influences and morph deltas stay in the cooked mesh, already in the storage layouts R2 needs. R2 uploads them, builds palettes as `model_from_joint × inverse_bind_matrix` per palette entry, decodes positions to f32 before adding deltas (morphed and skinned positions leave the bind-pose bounds), and writes per-instance f32 positions (slot 1, drawn through the f32 pipelines with an identity dequantization) and `VertexShading` (slot 2), keeping the pool's surface stream.
  - Golden images move by mean 0.0001 and p99 0.0018 FLIP on Metal (thresholds 0.01 and 0.3), so the goldens are unchanged. A GPU test renders a sphere with unorm16 and with f32 positions: 4 of 76,800 pixels differ by more than 8/255.
  - Cost against M1 R3 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, three alternating rounds): CPU p50 90.2–90.6 µs against 90.9–91.2, GPU wait 483–512 µs against 486–525, both within noise; 298.3 allocations per frame against 294.3: the renderer's own per-frame code allocates nothing new, so the four are wgpu's, presumably for binding four vertex buffers instead of two (not traced further). The mesh pool holds 229,616 bytes against 251,696, with every LOD's indices.
- **As built (M1 R2, `sapphire_render::{skinning, mesh_pool, draw}`, `shaders/skinning.wgsl`, `shaders/prepass.wgsl`):**
  - **Deformable meshes.** A mesh with a skin or morph targets is deformable. Its streams, influences and morph deltas go to one deformable buffer (storage and vertex) instead of a slab.
    - The upload regroups morph deltas by vertex with a counting sort: per vertex, its deltas in target order, each holding its target index. The skinning pass then applies morphs in the same dispatch that skins, each vertex walking only its own deltas, with no atomics and no pass per target.
    - Every instance of a deformable mesh owns outputs in one shared buffer (first-fit ranges of vertices): f32 positions twice and `VertexShading` normals, 32 bytes a vertex. Every pass draws it from those through the f32 pipelines with an identity dequantization (slots 1 and 2), plus the mesh's own surface stream (slot 3). Static meshes are untouched and cost nothing extra.
  - **One dispatch per frame, not one per instance.** The outputs are one read-write storage buffer, which wgpu fences between dispatches, so per-instance dispatches would run one after another and leave the GPU idle on small instances.
    - Each instance takes a run of 64-thread workgroups and an 80-byte job (offsets, flags, dequantization) in a per-frame buffer, along with its palette (three rows per matrix) and dense weights. A workgroup finds its job by binary search.
    - Per vertex, the pass decodes the bind pose to f32 and adds the weighted morph deltas. The binary16 deltas are decoded by hand, because naga accepts `unpack2x16float` only with the f16-in-f32 capability. It then blends four palette matrices by their unorm8 weights and transforms the position, the normal and the outline normal (normals by the blended linear part, renormalized).
    - The outline normal isn't morphed, so the inverted hull stays closed.
  - **What a frame skins.** Only instances posed since the last recorded frame, plus new ones in their bind pose. Poses from frames that were prepared but not recorded collapse into the latest. Only the drawn LOD's vertex prefix is skinned: LOD0 until LOD selection.
  - **Velocity.**
    - An instance's positions are double-buffered: a frame that skins it writes the other half, so the last drawn pose is still there for velocity. Re-skinning the previous frame from last frame's palette would read the bind pose, influences, palette and every morph delta a second time, and would need last frame's weights too. The second half costs 12 bytes a vertex of memory and one 12-byte read instead.
    - An instance not skinned in a frame binds one half twice, so it has no motion.
    - Instances keep the transform they were last drawn with: `InstanceData` grows to 176 bytes, and a moved instance is uploaded once more when it comes to rest. The view keeps its last view-projection.
  - **Prepasses.** They run after skinning and before the main pass, single-sampled, at the scene's size (the target's at the render scale, M1 R6), and `Renderer::record` returns them as `PrepassTargets`.
    - The character prepass writes the depth of characters (skinned and morphed instances) alone, position-only, on every tier.
    - The scene prepass writes every opaque instance's depth, world normals (octahedral `Rg16Float`) and velocity (`Rg16Float`, motion in texture coordinates). Pixels no geometry covers stay 0, for TAA to reproject from depth.
    - Every pass decodes positions through one function and marks its clip position invariant, so the depths match.
    - Nothing reads them yet, so the frame graph culls both: two boxed closures a frame, and no GPU time. The outline hull's velocity comes with C4.
  - **Decision: buffers stay outside the frame graph** (R1 had planned them for R2). Every buffer is persistent, compute skinning's outputs included, so declaring them would never cull a pass, and wgpu already orders accesses to them. Passes that write buffers stay side effects added before their readers. Buffer resources wait for the first transient buffers.
  - **Bindings.** The skinning pass binds three storage buffers in its own group 0 (sources, frame data, outputs), within the floor's four. The view uniforms gain the previous view-projection and the instance buffer the previous transform. See the crate docs, "Binding budget".
  - **Stats** (`RenderStats::skinning`, rule R9): instances; the instances, vertices and palette bytes the frame skins; output bytes; ignored poses; and instances whose outputs don't fit. The output buffer is a single storage binding, 128 MiB at the device floor, which is about 4 million vertices. The pass's GPU time is the graph's `skinning` timing. The overlay shows a skinning row.
  - **Sandbox:** `--skinned` puts a stalk beside the character: a 1.5 m procedural capsule of 2,145 vertices, six bones with tent weights, and two sparse morph targets (`bulb` and `pinch`) with exact normal deltas. It sways and breathes on its entity clock. The golden scene `turntable-skinned` shows it (Metal golden only; lavapipe and WARP come from CI). The four earlier goldens are unchanged (mean ≤ 0.0001). The iOS Simulator runs it at 60 fps.
  - **Tests (Metal):**
    - The GPU outputs match a CPU reference within 1.6e-7 relative position error and 0.06° of normal. The reference covers random palettes (scale 0.5–2), 1–4 influences, u8 and u16 indices, unorm16 and f32 positions and three sparse morph targets.
    - An instance never posed draws exactly like the unskinned mesh. With an identity palette, no pixel differs by more than 8 steps.
    - Posed skins and morphs draw like the mesh deformed on the CPU and drawn static: no pixel differs by more than 8 steps.
    - The scene prepass's depth equals the main pass's bit for bit. The character prepass's equals it wherever nothing stands in front.
    - Velocity of a moved static square and a lifted skinned one matches the projection within f16 precision, is exactly 0 once they hold still, and follows a camera move.
    - There are no validation errors from 1×1 to 17×1000, with MSAA off and 4×. Outputs survive buffer growth and reuse freed ranges, and bad poses are ignored.
  - **Cost** against M1 F5 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, three alternating rounds):
    - Without `--skinned`: CPU p50 91.5–91.6 µs against 90.9–92.0, unchanged; p99 142–152 µs against 125–155. 303.3 allocations per frame against 299.3: the two culled prepasses' closures and longer graph lists. GPU wait within noise.
    - With `--skinned`: CPU p50 100–103 µs, about 10 µs more, for recording the skinning pass, the stalk's draws and its two `Extract` systems. 320.4 allocations per frame, 17 more (the pass's closure, wgpu's compute pass and the systems; not traced further). The skinning pass takes 4–9 µs of GPU time for 2,145 vertices (timestamps; the main pass takes 80 µs).
    - Rebased onto M1 R6 (0cd857e), two more alternating rounds against its head agree: without `--skinned`, p50 92.1–92.8 µs against 92.1–92.5 and the same 4 more allocations; with it, p50 101–102 µs and 8–10 µs of GPU time for skinning.

**Lighting**

- **Desktop:** clustered forward+. A compute pass bins lights into a 16×9×24 grid.
- **Mobile:** per-object light lists, with at most 4 local lights per object.
- **Sun:** cascaded shadows. Desktop has 4 cascades (far cascades cached and updated in turns); mobile has 2.
- **Character shadow (desktop):** a separate high-resolution per-character shadow frustum for crisp contact shadows.
  - **M1 C2 (scope):** characters receiving environment casters only, the per-character ground shadow map on desktop and the capsule shadow on mobile belong to R4 (M1 plan), under the contract in C2's note below ("Anime character shading").
- **Ambient:**
  - A per-zone probe grid: sky visibility as SH plus bounce baked per time-of-day key (static per-zone probes go wrong as soon as the sun moves).
  - Reflection probes are relit at runtime on desktop and Mobile-High, and static on Mobile-Low (Genshin's approach).
  - Reflection probes: BC6H on desktop; RGBA16F or RGB9E5 at small sizes on mobile.
  - Capsule AO for characters on all tiers; GTAO on desktop.
- **As built (M1 R5, `sapphire_render::lights`):**
  - `RenderSettings::light_path` picks clustered forward+ or per-object lists, and `max_lights` caps the lights a frame draws (clamped to `MAX_LIGHTS`, 1024). `RenderSettings::for_device` gives desktops the clustered path with 64 lights and phones per-object lists with 8 (§3.4), until quality tiers (R6) choose per tier. The app passes the device class; the renderer has no `cfg` (rule R7). Both paths run on every backend, so both are tested on desktop.
  - Each frame the CPU drops lights that can't light anything and those whose range sphere misses the main view's frustum (five planes; the far plane is at infinity), keeps the `max_lights` nearest the camera, in packet order, and packs them at 64 bytes each.
  - **Clustered (desktop):** 16 × 9 screen tiles by 24 view-depth slices. Slice 0 runs from the camera to 1 m; slices 1–23 split 1 m to 500 m exponentially (about 31% deeper each), and the last continues to infinity, matching the infinite far plane. A pixel takes its slice from its reverse-Z depth (view depth = near / z). A compute pass, one thread per cluster and 128 per workgroup, loads the lights a batch at a time into workgroup memory in view space and tests each against its cluster's box: the range sphere against the box, then, for spots narrower than 90°, the cone against a sphere around the box (Wronski's cone test). Lists come out in index order, at most `min(max_lights, 64)` per cluster, in one buffer at a fixed stride: a count, then 16-bit indices two to a word (456 KB at 64). A full cluster keeps the packet's earliest lights; at the default budgets none can overflow. The pass is a side effect between the uploads and the main pass (buffers aren't graph resources yet), and runs only on frames with lights. On 400 random points and spots, the GPU lists match the CPU reference exactly in all 3,456 clusters, 166 of them full.
  - **Per object (mobile):** each instance lists its four most influential lights: luminance × range window × cone at the point of its bounding sphere nearest the light. Ties go to the lower index, and the list holds them in index order, as clusters do. Lists are chosen again for instances added, moved or re-meshed, or for every instance when the lights changed, and only the ones that changed are uploaded, in runs of slots. They use the cluster lists' layout (3 words each) in the same binding, indexed by the instance slot that the vertex stage passes as a flat varying. Bounding spheres come from the cooked mesh bounds.
  - **Shading (`anime_character.wgsl`):** each listed light is lit in the sun's two-tone model. Its N·L is split at the material's threshold plus the ILM bias into a lit and a shadow tone, with the same authored shadow color, in the light's color × its range window × its spot cone. The window is `(1 − d²/r²)²` (no inverse-square hot spot; intensity 1 is as bright as a unit sun), and the cone a smoothstep in cos θ from the outer to the inner angle. The one-pixel edge comes from N·L's exact screen derivative per light, built from dN and dP taken once per pixel (derivatives aren't allowed in the per-pixel loop). The summed light is held under a luminance of 2, so stacked lights tint rather than blow out (the reference games' `_FilterLight`). Local lights add no rim. A frame without lights skips all of it on a uniform branch, and the three earlier goldens stayed bit-identical. In a GPU test, a red light beside a sun-shadowed sphere splits it within two pixels of the predicted terminator, at the material's 2:1 tone ratio (0.19 to 0.40 of added red across the edge).
  - The two paths draw identical pixels for up to four lights (a GPU test, and the `turntable-lights` scene). `RenderStats::lights` counts lights drawn, culled and dropped, the clusters binned, the list capacity and per-object lists chosen.
  - Bindings: group 0 gains the light buffer and the list buffer, both fragment-only, so the fragment stage uses 3 of the 4 storage buffers Vulkan 1.1 phones guarantee and the vertex stage still 2. The cluster pass binds the same buffers in its own group 0, and the view group as group 1 (crate docs, "Binding budget").
  - Cost against M1 R3 (sandbox `--bench 3000` at 1920×1080 with 4× MSAA, Apple M5, release, two sets of three alternating rounds):
    - Without lights: CPU p50 89–96 µs against R3's 88–94 µs, so no measurable change; 295.3 allocations per frame against 294.3 (the second `Extract` system, §4.6).
    - With the three demo lights, clustered: p50 95–101 µs, about 5 µs more, which is recording the compute pass; 298.3 allocations (its boxed closure and wgpu's compute pass); 13 graph passes instead of 12.
    - With them per object: p50 89–93 µs, 295.3 allocations and 12 passes. The lists are chosen again only for the turntable, the one instance that moves, and upload nothing while they stay the same.
    - p99 was 105–272 µs in every configuration, the baseline's included, and the mean GPU wait stayed within its noise (about 465–490 µs).

**Anime character shading**

One uber-shader family, with at most 8 static permutation bits: skinned, alpha-test, outline, morph, face, hair, eye, transparent. Everything else is a uniform branch. M1's track C ([m1-plan.md](m1-plan.md)) refines what follows:

- **Two diffuse modes:** ramp rows (Genshin, Star Rail, WuWa), or analytic bands with per-ID colors (ZZZ, cheapest on mobile).
- **Lighting:** cast shadows push pixels into the shadow band. Characters receive only environment casters and never self-shadow; the ground gets a per-character shadow.
- **Stylized character GI:** spherized, camera-biased normals, plus an HSV clamp.
- **A character-only depth prepass on every tier** feeds rim light, hair shadow on the face, and eyes over the bangs.
- **Faces** get a vertical SDF channel for light from above.
- **Metals** use a matcap, plus a reflection add when metallic > 0.9.

- **Two-tone light split:** N·L is thresholded into light and shadow. Painted ILM / lightmap masks (specular mask, shadow bias, specular threshold, material ID) shift the threshold.
- **Ramps:**
  - Shadow color comes from a ramp texture array, with the row picked by material ID and a day/night ramp pair.
  - Ramps stay uncompressed, because block compression bands them.
- **Faces:**
  - An SDF face-shadow map is sampled from the light direction projected into head space.
  - Each character's head-forward and head-up vectors come through the packet; head-right is their cross product (as built, M1 C3: `CharacterInstance::face`).
  - The map is mirrored for light from the other side.
- **Hair:** anisotropic specular plus an "angel ring" highlight (a bright band across the hair), with a jitter mask.
- **Eyes:** a parallax / UV-offset refraction fake. Eyes and brows draw over hair through stencil.
- **Rim light:** a depth-difference rim on desktop (it needs the prepass); an N·V rim on mobile.
- **Character light rig:** every character gets a virtual key light whose color and intensity are curved by time of day. Shadow maps bias the ramp threshold rather than multiplying the color.
- **As built (M1 A7, `sapphire_assets::character_maps`):** the layouts of the authored maps the anime character shader reads, in one module shared with the pipeline's map tools (§5.27), encodings both ways:
  - **ILM maps** (linear RGBA): R specular mask, G shadow bias (0.5 neutral, ≥ 0.95 always lit, ≤ 0.05 always in shadow), B specular threshold, A material ID: the order M0's shader comments already named. There are 8 material IDs (`MATERIAL_IDS`); ID *k* is stored as *k*/7 and decoded by rounding, which tolerates ±1/14, more than BC7 or ASTC 4×4 move a texel. IDs blend under linear filtering and in the ILM class's mips, so the shader reads them from level 0 unfiltered (`textureLoad`), or the map cooks without mips.
  - **Ramps** (RGBA8 sRGB): 16 rows of equal height, the day ramps of IDs 0–7 then their night ramps; `u` runs from the deepest shadow (0) to the lit edge of the terminator (1). The shader samples row centers, `v = (row + 0.5) / 16`, so it needs no row height, and every ID has both rows.
  - **SDF face maps:** each texel holds the light angle at which it turns to shadow, as angle / 180°: R for a light turning from head-forward toward head-right, G (two-channel maps) for one rising toward overhead. A texel is lit while its value exceeds the light's angle in head space / 180°. The encoding is angle-linear, as the research notes recommend: equal steps of light angle are equal steps of value, whatever angles the masks sit at. Light on the head's left samples the map mirrored (`u → 1 − u`), which needs face UVs mirror-symmetric about `u = 0.5`.
- **As built (M1 C1, `shaders/anime_character.wgsl` and `outline.wgsl`, `sapphire_render::pipelines::Permutation`, `sapphire_assets::mesh::VertexFlags`):**
  - **One uber-shader, two diffuse modes,** chosen per material on a branch every pixel of a draw takes alike (the block's ramp flag). The shadow side is albedo × a tint, never a darkened albedo:
    - **Ramp mode:** the region's day row at `u = (x + 1) / (threshold + 1)`, clamped (0 where the split's input `x` is −1, 1 at the terminator), and while the frame's day/night factor (W1's `day_night`) is above 0 its night row too, blended by it; then mixed into the region's subsurface tint by the material's ramp mix (WuWa's `_RampInt`; 1 is the row alone). Rows are read at their centers through the clamped ramp sampler, level 0.
    - **Band mode:** the region's shade color, or with a deep threshold a third band below it in the region's deep shade, on the same biased input. No texture.
  - **The split's input** is N·L scaled in half-Lambert space by the occlusion, `x = N·L + (o − 1)(N·L + 1)` with `o` the vertex AO × the cast shadow, so occluded and shadowed points move into the authored shadow tones instead of darkening, and `x` is exactly N·L when nothing occludes. The cast shadow is one function, `cast_shadow`, returning 1 until R4 fills it (characters receive environment casters only). The control map's shadow bias G then adds the offset `2·G − 1` to it, the sign contract of `character_maps::ilm` (positive lights; MToon's shading shift is extracted into G as `0.5 + offset / 2`), and the sum is compared with the material's threshold; at G ≥ 0.95 or ≤ 0.05 explicit locks hold the side for any light and threshold. A neutral 0.5 adds exactly 0. The transition is the material's softness or half of `x`'s change across the pixel, whichever is wider, and never under 1e-5 (smoothstep is undefined with equal edges). Local lights split the same way, AO, bias and locks included, onto the surface's shadow color.
  - **The control map,** in A7's layout: R, G and B through the trilinear sampler with explicit gradients; A, the material ID, through a third group 3 sampler (binding 10: nearest, repeating, level 0 only), decoded as `round(a × 7)` and clamped below the region count. Without a control map the region is the vertex's material ID and the bias neutral. The specular mask and threshold are read into the surface for C7. The renderer refuses an sRGB texture in a data slot (normal, control, mask, SDF), so data maps stay linear (`RenderError::SrgbData`).
  - **Normal maps,** OpenGL-style as glTF's, turn into world space through a tangent frame built per pixel from the screen derivatives of the position and UV (Schüler 2013, with the determinant's sign restored, since WebGPU's screen y runs down): meshes carry no tangents yet.
  - **The day/night factor** is time of day's (`FramePacket::day_night`, the frame uniforms' `day_night`, W1). The shader samples the night row only while it is above 0, so a frame by day reads one row.
  - **The vertex contract** (`Mesh::VERSION` 4): the surface stream keeps its 8 bytes. Its vertex color is linear data, R ambient occlusion, G outline-width scale and B outline Z-offset scale (all 1 without vertex colors), and its fourth byte is `VertexFlags`: the material ID in bits 0–2, the region of a material without a control map; bits 3–7 are reserved for faces and eyes and must be zero (`MeshError::ReservedFlags`). The vertex shader rounds the unorm8 byte back to its bits and passes them on flat. The glTF importer (version 5) reads `COLOR_0` RGB as that data, as linear as glTF stores it, and a `_MATERIAL_ID` attribute (float holding whole numbers, or unsigned byte or short) into the flags. The sample mannequin's head moved its outline width from alpha to green; nothing else in it changed. The outline hull takes the vertex's region (width scale and color), scales its width by G, and is pushed away from the camera along the view ray by the material's Z offset × B, which hides it behind nearby surfaces without moving it on screen.
  - **Static permutations** are the plan's eight bits, in its order (skinned, alpha test, outline, morph, face, hair, eye, transparent). Two are shader variants today, as WGSL pipeline-overridable constants (`PERMUTATION_ALPHA_TEST`, `PERMUTATION_TRANSPARENT`): alpha-tested surfaces and their hulls discard below the cutoff, and transparent surfaces blend by coverage (keeping scene color's alpha) after the sky, depth-tested without writing, with no hull and in no prepass. Skinned and morphed select the f32-position pipelines that read compute skinning's outputs; outline decides whether a draw is in the outline pass; face, hair and eye wait for C3, C5 and C6. Each variant's pipelines are built for a position format when a mesh in it is uploaded with a material that needs them, or an edit moves a material to the variant (5 for a scene with all three on quantized meshes; `RenderStats::character_pipelines`); that is synchronous, so an edit into a new variant can hitch once until pipelines compile on workers at pack load. The draw list groups draws by format and variant; transparent ones keep the list's order (back-to-front sorting comes with draw sorting), and the prepasses don't cut alpha-tested ones out until something reads them. A material edit that changes the permutation rebuilds the draw list once; other edits still upload one slot and nothing else. `RenderStats` counts surface draws by variant and outline draws, and the overlay shows them.
  - **Golden images:** the seven earlier scenes (W1's `turntable-sunset` included) render pixel-identical captures on Metal (0 of 230,400 pixels differ in each; all at mean, p99 and max 0.0000 FLIP), the mannequin's math being preserved exactly where it has no maps. A new scene, `turntable-swatches` (`--swatches`), puts three spheres beside the turntable: ramp mode over a ramp and control map generated by the pipeline's `samples` module and cooked by A7's tools (four material IDs in latitude bands, each with its own ramp row; a shadow bias striped around the sphere, so the terminator jogs, with a painted shadow on the lit side and an always-lit patch on the shadow side), the same with a 35% subsurface mix, and three bands in per-ID colors from vertex material IDs. The three materials are content as well (material documents the `samples` module writes, naming the maps by ID), and the sandbox loads them by ID from the shared pack, maps included, through the asset server (A4); the spheres are procedural, as the skinned stalk is. Only its Metal golden is committed; lavapipe and WARP come from CI. On the iOS Simulator the sandbox runs it at 60 fps with the mobile cook's ASTC control map.
  - **Tests (Metal):** ramp rows follow the material ID and the night factor (day, night and halfway, per ID within 0.02 in linear color), explicit rows and the subsurface mix; IDs read unfiltered from level 0, magnified and minified, from a hand-made map whose mips hold another ID and from the pipeline's ILM cook (BC7, and ASTC where supported), never showing a blended or mipped ID; two and three bands in each region's colors from vertex IDs; the shadow bias moves the split to where N·L + 2·G − 1 meets the threshold (within 2 pixels), G = 1 lights a point that is in shadow at 0.5 and G = 0 shadows a lit one, and the locks hold past any threshold; normal maps tilt the split the right way in X and Y; alpha-tested surfaces and their hulls cut out, transparent ones blend over the sky; permutations pick pipelines and passes, and a permutation edit rebuilds the draw list once; vertex AO moves the split to where it predicts, G scales the outline (within 0.75 px of the predicted 9.5 and 4.8 px), B pushes the hull behind a wall; sRGB textures are refused in data slots. Breaking the normal frame's orientation, the unfiltered ID read, the night blend, the bias's sign, the bias locks, vertex AO or the region lookup in the shader each fails a test. Headless: the WGSL constants match `sapphire_assets` (vertex flags, bias locks, ramp rows, ID levels), and every permutation of every mesh pass specializes and validates in naga.
  - **Cost** against ef92631, before rebasing onto W1 and A6 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, three alternating rounds): CPU p50 93.3–96.8 µs against 93.0–95.2, the GPU wait 500–521 µs against 481–509, the timed main pass 80–100 µs against 75–92 and the timed frame 256–310 µs against 272–278, all within the machine's noise; 305.3 allocations per frame on both (the opaque mannequin binds the same pipelines as before). With `--swatches` (three more instances, 1,764 vertices each, three more materials) the CPU p50 is 91–99 µs, 307.3 allocations a frame and the main pass 130–142 µs. Against d64be43 (A6), on a machine loaded by other builds, the CPU p50 is 95.5–95.9 µs against 94.8–106.5 and allocations are 305.3 on both.
- **As built (M1 C2, the character light rig: `sapphire_world::light_rig`, `sapphire_render::characters`, `shaders/anime_character.wgsl`):**
  - **One rig per character, resolved on the CPU.** A preset (`LightRig`, §5.21) is data: a key light that follows the world's key (the sun, the moon, or both through twilight), or is held to the camera, or a blend; limits on where it may come from (an elevation band, and an azimuth limit around the camera's side, judged around the character's head, past which a key from behind folds back toward the camera); scales on time of day's character key color and brightness; GI settings; a rim scale; and a local-light ceiling. Each frame the world resolves every character's preset for the camera into `CharacterLighting`, which `FramePacket::characters` carries (§4.6). Shipped presets: `gameplay`, the default (the key within 5°–65° of elevation, and folded back past 120° from the camera's side; GI camera bias 0.3, saturation ≤ 0.6, value ≤ 0.5, skin tint (1.12, 0.96, 0.86)), and `portrait` (the key held 35° to the camera's left and 30° up; camera bias 0.6, saturation ≤ 0.5, rim ×0.8, local lights ≤ 1.5).
  - **On the GPU:** a per-frame storage buffer at group 0 binding 3 (fragment), one 80-byte `CharacterRig` per character inside the WGSL `CharacterData`, after entry 0: the world's key light with unstyled GI (no head, so nothing spherizes, whatever a region's weight), which every unlisted instance reads (props drawn with this shader, the swatches, the skinned stalk). An instance finds its entry through `InstanceData::character` (the instance block grows from 176 to 192 bytes), passed on flat by the vertex stage. Entries follow the packet's order, so a character listed in the same place each frame keeps its entry and its instance isn't uploaded again; the buffer is rewritten whole when an entry changes, 80 bytes a character.
  - **Decision: one per-character binding.** It takes the fragment stage's fourth and last storage buffer, because every later per-character need joins `CharacterData` as a struct of its own instead of taking a binding: faces' head basis, expressions and blush (C3), the per-character shadow's matrices (R4), outlines lit by the rig (C4). Per-character fields in the instance buffer were rejected (vertex-only, persistent and uploaded only on change, where rigs change every frame the camera moves), as were flat varyings (bandwidth per vertex on tilers) and a fixed-size uniform array (a hard cap on characters, shared with C3's data).
  - **The key light:** the fragment shader reads the rig once (`key_direction`, `key_radiance`), and the light split, the shadow side's tone and the rim (scaled by the rig) all take the key from there; C3's face shadow reads `key_direction` too. `FrameUniforms::sun_*` remain the world's key light, for the environment and shadows.
  - **Stylized GI (`character_gi`)** replaces `albedo × sky_color(N) × 0.12`. The frame's ambient (time of day's `FramePacket::ambient`, in the appended `FrameUniforms::ambient_sky` and `ambient_ground`) is read through `ambient_light(n) = mix(ground, sky, 0.5 + 0.5 n.y)` in `frame.wgsl`, the irradiance of a sky-and-ground hemisphere, which W4 can share. Per pixel: the normal turns toward a sphere around the head by the region's sphere weight (`RegionGi::sphere`: faces and hair, through A3's region parameters, so it doesn't wait for C3's face permutation) and bends toward the camera by the rig's bias; the hemisphere there, times the rig's intensity; an HSV clamp (saturation held by moving the channels toward the value, then the value capped; hue kept, and a color within both returned exactly); the rig's skin tint by the region's skin weight; and vertex AO. `sky_color` is unchanged (W2's).
  - **Local lights** keep their own directions (decision: the rig's limits are for the key light alone, and a lantern under a face lights it from below on purpose) and take the rig's luminance ceiling, which was the constant 2 (the fallback keeps 2).
  - **The contract for R4's shadows.** The rig keeps C1's `cast_shadow(in)` hook and reads it in one place, the light split's occlusion input. R4 must return how much of the world's key light (the sun or the moon, `FramePacket::sun`, whose shadow maps it renders) reaches the point past **environment casters only**: a character never shadows itself from the sun's maps, so characters stay out of the cascades the character shader samples (or are masked out of them), and the per-character shadow map on desktop and the capsule on mobile shadow only the ground and the environment (optionally other characters). The rig's key direction may differ from the sun's (limits, camera-fixed presets, shot rigs): it shades, it never casts, so a character standing in a building's shadow is in shadow whatever its rig.
  - **The default look:** at the default 10:00 the gameplay preset's key is the sun bit for bit (within its limits, so returned as it is) in time of day's character key, which W1 keyed equal to the sun at 10:00. What changes is the ambient model: the hemisphere irradiance of W1's ambient (the sky's horizon above, its ground below, × 0.12) instead of the sky gradient at the normal × 0.12, plus the camera bias (no measurable effect on the mannequin at 10:00). On `turntable`, about 24,500 of 230,400 pixels move, none by more than 7 of 255 (up-facing normals lose the gradient's zenith blue). Against the C1 goldens on Metal (FLIP): mean 0.0075–0.0081 and p99 0.10–0.11 for the five 10:00 mannequin scenes, mean 0.0101 for `turntable-skinned` and 0.0128 for `turntable-swatches`, and for `turntable-sunset`, where the character now takes time of day's warmer, brighter character key instead of the sun by design, mean 0.038 and p99 0.56. All eight existing Metal goldens are regenerated for that reason; lavapipe and WARP follow from CI.
  - **Golden image:** `turntable-night` (`--time-of-day 22`, halfway from dusk to midnight): the mannequin under the gameplay rig, its key from the moon high on the camera's left in time of day's character key (intensity 0.78, against 0.32 for the moon that lights the world), stylized GI under a deep blue ambient, and two blended LUTs. The skin reads cool pink with a lavender shade, the face lit with its split at the lower right, the outfit navy with its shading visible: not flat, dark or muddy. Only its Metal golden is committed.
  - **Tests (Metal):** in `sapphire_render` (`tests/character.rs`), each character takes its own key light and the rest the sun, a rig from behind puts a character in its authored shade, unknown instances are ignored and counted; the rim takes the key's color and the rig's scale; GI normals turn toward the head by the region's sphere weight (0, 0.5, 1), and not at all without a head, and bend toward the camera by the bias (0, 0.5, 1), each pixel as predicted; the HSV clamp holds saturation and value and keeps the hue, and returns a color within both unchanged; skin takes the tint by its weight, and AO and intensity scale the GI; a lamp adds the same light to a rigged character as to an unrigged one, and the rig's ceiling holds it. Removing the sphere weight, the bias, the clamp, the skin tint, the ceiling, the rim scale or the per-character key from the shader each fails a test. End to end in `sapphire_world` (`tests/light_rig_gpu.rs`): presets resolved under time of day and the camera, drawn by the renderer: a backlit character keeps a lit side under the gameplay preset (the neutral rig leaves it all in shade), two characters with two presets are each lit by their own, a shot's rig turns the key along one great circle as its cutscene blends in, and the key takes time of day's character curve at four hours, brighter than the moon at midnight. Headless: the WGSL `FrameUniforms`, `InstanceData`, `CharacterData` and `CharacterRig` match Rust field by field, entries pack and validate, instances point at their entries (moved, joined, dropped, duplicated and ignored), and only changed entries upload.
  - **Cost** against c3ac568 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, alternating rounds on a machine shared with four other agents' builds, load average 11–24; fourteen rounds each, of which the four with both p50s under 110 µs are quoted): CPU p50 93.5–94.6 µs against 92.5–104.1, so no measurable change (resolving the character's rig in the sandbox is about 45 ns); 305.3 allocations per frame on both, with 135 more bytes allocated (not traced: the renderer's per-frame character code allocates nothing). The timed GPU frame was 280–325 µs against 259–306 and the main pass 87–121 µs against 84–102, within the machine's noise under other agents' GPU work (one branch round reached 901 and 315 µs, as base rounds under load reached 1,842 and 600).
- **As built (M1 C3, faces: the face permutation of `shaders/anime_character.wgsl`, `CharacterFace` in the per-character entry, `sapphire_assets::character_maps::{face_sdf, face_mask, face_expression}`):**
  - **The face permutation** is a real variant. A static variant is now a coverage (opaque, alpha-tested, transparent) and a part (body, face), six in all (`pipelines::SurfaceVariant`); a face material's pipeline sets `PERMUTATION_FACE` on the shared entry points, and its outline is its coverage's (one outline pipeline per coverage). The body's code is unchanged under the constant: C2's nine golden scenes render pixel-identical captures on Metal (mean, p99 and max 0.0000 FLIP).
  - **The head per character.** The face is C3's struct in the per-character entry, beside C2's rig: WGSL `CharacterFace` in `CharacterData` (`frame.wgsl`), and `GpuFace` in `GpuCharacter`, 48 bytes (unit forward and up in world space, then blush and gloom), packed from `CharacterInstance::face` (§4.6). An entry grows from 80 to 128 bytes, and entry 0, which instances not listed read, holds a head at rest. Only the face permutation reads it, through the `character` index the rig uses: no binding, varying or vertex-stage work is added, and the storage-buffer budget is C2's.
  - **The key light is the rig's.** The fragment shader reads C2's `key_direction` and `key_radiance` once for body and face alike: the face's margin takes `key_direction`, its highlight `key_radiance`, and its diffuse goes through the body's line. Face code reads no light of its own.
  - **The light in head space** (`character_maps`, "Head space", and the reference `face_sdf::HeadLight` and `lit_margin` the GPU tests predict from). With `x = L·R`, `y = L·U`, `z = L·F` and right `R = F × U` (the character's own right, −X at rest): the horizontal angle is the yaw of the light's projection on the head's horizontal plane, `atan2(|x|, z)`, scaled by `|(x, z)| = cos(elevation)`, and the vertical angle the elevation, `asin(y)`, 0 below the horizon. The map is read twice per pixel, at `u` and at `1 − u`; R takes the light's side (`u` for light on the head's right, the mirror for its left) and G is never mirrored, since vertical masks are authored for lights in the symmetric forward–up plane. The two channels intersect: the texel's lit margin is `min(R − horizontal, G − vertical)`. In the horizontal plane the vertical angle is 0, which G lights; rising straight in front the horizontal angle is 0, which R lights; so each channel reproduces its masks exactly. The yaw's fade with elevation lets a light pass over the top without the face flipping sides; for the same reason, within `|x| < 0.25·|y|` (`face_sdf::MIRROR_BLEND`) R blends toward the darker of both sides, `min(R(u), R(1 − u))`, so a light behind and above the head darkens it evenly instead of lighting one side and flipping as it crosses the mirror. In the horizontal plane the blend is empty and R holds exactly. A one-channel map reads G as 1 (A2's SDF class gives it the swizzle `r101`), so nothing on it shadows as the light rises.
  - **The split is the body's.** The margin (threshold units, light angle / 180°) becomes the N·L of a sphere whose terminator follows the face map, `sin(π·margin + asin(threshold))`, clamped to a quarter turn either side: the map's edge falls on the material's threshold, and a quarter turn of light past it, plus the threshold's own angle, is the deepest shadow. From there occlusion (vertex AO), the control map's shadow bias and its locks, the threshold and `shade_tint` (ramp rows at that depth, or bands) apply as they do to N·L, so a face's shadow tones are the body's. The edge's half-width is the face's softness (`FaceShading::softness`, threshold units) times the curve's slope there, `π·cos(asin(threshold))`, or half a pixel's change, whichever is wider. C2's stylized GI is the body's too: a face region's sphere weight spherizes its GI normals around the rig's head center, so face code has no GI of its own.
  - **Faces never take the cast shadow: the contract R4 honors.** The face permutation doesn't call `cast_shadow()`, and R4 adds no shadow-map reads to it: shadow maps are too coarse and unstable at face scale, a face's shadow is its SDF map's alone, and characters don't self-shadow anyway. `cast_shadow()` stays as C2 defines it, for body surfaces, environment casters only. An environment shadow over a face reaches it whole, per character: a value in the character's entry (R4's struct beside the rig and the face) that the face takes as occlusion, moving the whole face into its authored shadow tones at once, never shadow-map texels on it.
  - **Masks** (`character_maps::face_mask`, the face material's surface mask slot; `face_expression`, a new eighth slot): **always lit** (B: eyes, lashes, brows) holds the split at lit whatever the light and gloom; the **face highlight** (G) brightens the lit tone by `albedo × key radiance × FaceShading::highlight`, fading in over `highlight_reveal` (threshold units) from the shadow's edge and never in shadow; the **nose line** (R) is drawn, not lit: the region's outline color where `mask × |sin(turn)|` passes `FaceShading::nose_line`, `turn` being the camera's angle from head-forward around the head, and the mask is authored for a camera on the head's right (the nose's far side from it) and mirrored for one on its left, so the line appears on the far side as the head turns. **Blush** (expression R × the character's weight) multiplies the albedo toward `FaceShading::blush_color`, lit and shadow tones alike; **gloom** (expression G × weight) multiplies the occlusion, pushing the painted region toward its deepest authored shadow tone (an anime "shadowed brow") and taking its GI with it, as AO does; always-lit texels (eyes, brows) keep both their split and their GI.
  - **The asset contract:** `FaceShading` gains `expression_map`, `nose_line`, `highlight`, `highlight_reveal` and `blush_color` (all defaulted, so documents leave them out), packed into the block's `surface` and `surface_color`; the family gains texture slot 7, `expression` (a mask). `Material::VERSION` is 3 and `Mesh::VERSION` 6 with it (C2's 2 and 5, plus one; meshes embed blocks), so every material and mesh recooks. The anime character family binds 8 of a fragment stage's 16 sampled textures.
  - **Test content** (`sapphire_pipeline::samples::face`, `content/samples/face/`): a procedural anime head (`face_head.glb`: a sphere drawn into a jaw, front-projected UVs mirror-symmetric about `u = 0.5`, and a flat hair cap) whose sidecar binds its `Face` slot to `face.material.ron`: two bands in a warm rosy shade, a thin outline, skin's GI weights (sphere 1, skin 1) and the face settings, over a painted base color (large eyes with a two-tone iris and highlight, lashes, brows, mouth), the face mask and expression map, and an SDF map that A7's generator makes from 19 horizontal masks (every 10°) and 7 vertical ones (every 15°): thresholds of a sphere's terminator with a nose shadow from 38°, a far cheek that stays lit longer, and eye sockets, the underside of the nose and the lower lip shadowed early by a top light, on a flattened face. Everything is generated and committed; no third-party art. The sandbox loads the head by ID with its material and maps through the asset server.
  - **Golden image:** `faces` (`--faces`) frames a look-dev rig of ten heads instead of the character. Each head faces the camera, eyes level in the image, and is a character of its own: the sandbox lists it with the gameplay preset resolved for its head, its key light turned to the rig's angles in the head's space, and its face (its real pose and the rig's expression; `faces::character`). Top row: a yaw sweep at 135°, 90° and 45° from the heads' right, then 45°, 90° and 135° from their left; bottom row: a top light (65°), light from 60° to the side and 40° up, a blush, and a gloom with a blush (both lit from 45° and 10° up). Only the Metal golden is committed; lavapipe and WARP come from CI. Assessed visually at 1920×1080 and 640×360: the shadow edges are clean single lines; mirrored pairs are mirror images; the 90° heads split down the nose; at 45° the far edge darkens and the nose casts its small triangle; eyes and brows stay lit on every head; the top light darkens the eye sockets and casts a separate nose shadow above a dark lower face; blush tints both cheeks and gloom darkens the forehead down to the lit eyes. No head shows the nose line, as none turns from the camera.
  - **Tests (Metal, `tests/face.rs`):** the face permutation selects its pipeline and pass (one pipeline more, its outline shared; a body-to-face edit moves the draw with one draw-list rebuild); the horizontal channel's edge lands where the light angle predicts, within 1.5 px, at 30–150° from either side (the mirror), fully lit straight ahead and dark behind; the vertical channel's edge rows at 30°, 60° and 90°, the intersection with the horizontal one against `lit_margin`, a light over the top lighting both sides alike, and one behind and above lighting neither; a one-channel map (R8, `r101`) never shadowed by a top light; the character's key light (not the world's sun) taken in its head's space, turned, tilted back and forward, and unnormalized; the head being the character's whatever the instance transform; a face with no basis ignored and counted, falling back to the sun and a head at rest, as an unlisted instance does; always-lit, the highlight in the key's light and its reveal, and the nose line on each side of the camera and not at a slight turn; blush and gloom per character at full, half and clamped weights, gloom taking the GI with it except where always lit, and none for an unlisted instance; the ramp's depth across a face shadow. Each of nineteen mutations fails a test: reading the sun instead of the key, entry 0's face or the sun's radiance for the highlight; leaving the GI under gloom, or dimming always-lit texels' GI; unclamped weights; an unnormalized basis; accepting invalid faces; a fallback head not at rest; and breaking the mirror, the vertical channel, the yaw's fade, the mirror blend, always-lit, gloom, blush, the nose line's mirror, the highlight's reveal or the sphere mapping. Headless: WGSL `CharacterData` and `CharacterFace` match `GpuCharacter` and `GpuFace` field by field, `FACE_MIRROR_BLEND` and the slot constants match `sapphire_assets`, every face permutation specializes and validates; CPU tests cover the reference math, the face's packing and validation, the packet's ring reuse and the sandbox's rig.
  - **Cost** (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, on a machine shared with other agents' builds and benches, load average 13–18): three alternating rounds of c3ac568, C2's head (b349915), this branch and this branch with `--faces` gave CPU p50 92.5–93.8, 94.3–95.3, 97.9–99.6 and 100.4–102.0 µs; the branch's 3–4 µs over C2 didn't reproduce when the two were interleaved ABBA (undisturbed: C2 93.8 and 94.2 µs, the branch 94.1 and 93.2; a second set, all runs slowed by other work, C2 117.4–129.2 and the branch 120.0–125.0), so C3 costs nothing measurable without faces. Allocations stay 305.3 per frame (49 bytes more than C2's, about the turntable's 128-byte entries now changing every frame as its head turns; not traced); the timed main pass is 89–96 µs against C2's 89–111, the GPU frame 219–298 µs against 266–292. With `--faces` (ten more characters, each head 2,450 vertices drawn as a face, a hair cap and their hulls, filling much of the frame): 312.3 allocations per frame (wgpu's recording of the extra draws; the rig adds no `Extract` system), the main pass 409–416 µs and the GPU frame 597–637 µs. A face pixel reads the face map twice, the face mask twice and the expression map once.

**Outlines**

- **Characters:** an inverted-hull pass. The cooker bakes smoothed normals into a spare vertex attribute (octahedral-encoded). Width comes from vertex color × a distance curve × a resolution scale; each material sets the outline color.
- **Environment (desktop, optional):** a screen-space edge pass driven by the prepass normals and IDs.
- **TAA interaction:** composite character outlines after TAA, or mark them in a responsive mask.
- **As built (M1 R7):** the hull still draws only in the main pass, with no velocity. The resolve finds its pixels where the main pass's depth is nearer than the scene prepass's, gives them the velocity of the nearest surface matching their depth within an outline's width, and treats them as responsive (see "Anti-aliasing and upscaling"). C4 writes the hull's own velocity; P1 settles how outlines and TAA meet.

**Environment**

- **Shading:** a stylized GGX uber-shader.
- **Foliage:** instanced with vertex-shader wind. LODs go from cards up close to billboards to impostors.
- **Grass:** instances generated on the GPU on desktop; low-density CPU-placed instances on mobile.
- **Sky:**
  - A gradient sky plus animated 2D cloud layers, and sun and moon discs.
  - Everything is driven by artist time-of-day curves, not physical atmosphere: one `TodTimeline` of `TodKey`s, evaluated in OkLab, is the single source for sky, fog, ambient, shadow tint, post and the character light rig.
- **Fog:** distance and height fog colored by the sky horizon, plus fog cards, on every tier. Volumetric fog is desktop-only and comes later.
- **As built (M1 W1):**
  - Time of day lives in `sapphire_world::tod` (§5.21), not here. The renderer receives its resolved scene constants (§4.6) and draws the key light, sky gradient and post-processing from them as before. Sky discs, fog, ambient, the shadow tint and the character key light wait for W2, W3, W4/W5, R4 and C2.
  - Neighboring keys' grades blend as post's two LUTs: `grade_a` and `grade_b` are the two keys' grades and `grade_blend` the eased weight, with B unused while one grade shows. A running day therefore bakes one LUT per key crossed, and none between keys.
  - The frame uniforms gain `day_night` for C1's ramp-pair blend; no shader reads it yet. All six existing golden captures are byte-identical.
  - The golden scene `turntable-sunset` (`--time-of-day 18.5`) sits between the default day's golden-hour and sunset keys, 80% through their LUT blend. It shows a low orange key from the right with a crisp two-tone split, violet-tinted shade, a gold horizon band under a salmon sky, and a violet ground. Only its Metal golden is committed; lavapipe and WARP come from CI.
- **As built (M1 W2, `sapphire_render::sky`, `shaders/sky.wgsl`):**
  - **Layers.** The sky stays one fullscreen triangle in the main pass at the far plane, after the opaque surfaces and outlines (depth test `Equal`), writing HDR scene color. Over `sky_color()`, unchanged (the anime character shader reads it for ambient light), it draws back to front: procedural stars and the Milky Way on the celestial sphere, the sun's and moon's glows, their discs, the cirrus and the card cloud band. Time of day gives every color and amount (`FramePacket::sky_layers`, §4.6); the sky material gives every shape, map and motion.
  - **Sun and moon.** Glows are compact (a halo with a brighter core, nothing past their radius, so a body out of view adds exactly nothing), spread along the horizon while their body is low, and fade out just under it. Discs are HDR, so the sun's blooms, with an edge as soft as the material says or a pixel, and are cut by the horizon. The moon's face map spans its disc upright; its alpha can paint a crescent, and the moon hides the stars behind it.
  - **Stars** are procedural: two grids of cells (160 and 44 per face edge) on a cube around the celestial sphere, each cell holding at most one star at a hashed spot (PCG), with a hashed brightness, warmth and twinkle; the star map sets their density and tint. A star is a normalized Gaussian 0.6 or 0.85 pixels in radius, measured from the pixel's own angular footprint, so its light summed over its pixels is the same wherever it falls between them: stars hold still under sub-pixel camera motion, where point stars would flicker, and a grid fades out before its cells shrink below 3–6 pixels. The Milky Way is a map on the same sphere. Time of day turns the sphere once a day about the sky's pole (§5.21), so stars rise and set with the hour.
  - **Card clouds:** a band of painted cumulus around the horizon. `u` runs with the azimuth, a whole number of repeats a turn, drifting; `v` with the direction's height (the sine of its elevation, for one arctangent less), its texels square at the horizon and the band's base just under it. The map is RGBA in the Genshin scheme: R the lit amount (its 0.5 line the painted terminator), G a painted rim, B a signed distance to the cloud's edge (0.5 on the silhouette, rising inward), A opacity. Coverage moves the dissolve threshold along B, so clouds shrink to their cores and vanish rather than fade. Silhouette and terminator are linear ramps a pixel wide (the texel's own screen derivatives) or the authored softness, so they stay crisp at any magnification. The lit and shade tones are authored per key, never a darkened albedo; the rim (the painted one and a band just inside the dissolving edge) is strongest toward the key light, and low clouds melt into the horizon color. Texture coordinates and their gradients come analytically from the view direction (`textureSampleGrad`), so the azimuth's wrap leaves no seam.
  - **Cirrus:** a tiling map on a plane over the camera, scrolled both ways by the wind, its directions lifted so the plane recedes smoothly to the horizon, where it fades out. It dissolves like the card clouds and has no rim: its wisps are too thin for one.
  - **Drift** is reduced on the CPU in `f64` from the packet's unwrapped time, so the clouds never jump, even weeks into a session.
  - **Data and variants.** Each frame the CPU folds the packet's layers and the sky material's block (read from the material buffer's CPU mirror: one sky draws at a time, so the family's GPU buffer goes unread) into a 320-byte `SkyUniforms` block, with every per-draw constant (the glows' inverse radii and stretch, cloud repeats) worked out there. It is the sky pipelines' own group 2, at binding 2 (bindings 0 and 1 are the scene group's, which the sky doesn't read); group 3 is the sky family's material group. `FrameUniforms` and `ViewUniforms` are unchanged. The pipelines specialize three overridable constants: `SKY_STARS` and `SKY_CLOUDS`, on while time of day gives the stars a brightness or a cloud layer a coverage, pick one of four main pipelines each frame, so a frame never pays for a layer it doesn't draw; `SKY_DISCS` is off for the cube. The shader stays in `sky.wgsl`, concatenated with `frame.wgsl` and `materials.wgsl` as before, so A8's hot reload reaches it like any other.
  - **The sky cube** (`Sky::cube_view`): the sky alone, without geometry, for the probes and reflections that will read it (W5's ambient probes, W4's and W5's reflection misses, C7's reflection add). It holds the gradient, the glows and both cloud layers; not the discs, which shading adds as the key light (a disc would add them twice and alias at probe resolution), and not the stars, far smaller than its texels. `Rg11b10Ufloat` where the device renders and filters it, else `Rgba16Float`; one level, since readers prefilter. Faces are WebGPU's, in its order and orientation; one face is drawn every `cube_interval` frames, and all six on the first. It is a persistent texture outside the frame graph, like the renderer's buffers, until textures can outlive a frame in the graph.
  - **Per tier** (`QualityTier::sky`): 256² faces, one a frame, on both desktop tiers (so goldens match on every desktop GPU); 128², one a frame, on Mobile-High; 64², one every other frame (a whole cube every 0.4 s at 30 fps), and no cirrus on Mobile-Low, where the card clouds carry the look. Card clouds, stars and discs draw on every tier.
  - **Stats** (`RenderStats::sky`, rule R9): the cube's size, faces drawn and the last, which layers the last frame drew, and ignored sky materials, in the overlay's stats panel.
  - **Content:** the sandbox's sky material and every map it names are generated by the pipeline's `samples` module (§5.27) and cooked as ordinary textures: nothing is downloaded or third-party.
  - **Golden images.** The ten scenes from before (C2's `turntable-night` and C3's `faces` included) pin the sky's gradient alone (`--plain-sky`: no discs, glows, stars or clouds), and their Metal captures are unchanged, byte for byte. Three new scenes look at the sky: `sky-day` (09:00, looking up past the sun: its disc and glow, card clouds and cirrus), `sky-sunset` (18:30: the sun low in a gap of the card clouds, its glow spread along the horizon, gold-lit clouds over violet shade with bright rims toward the sun, cirrus catching the color) and `sky-night` (21:00: the moon's disc and halo, stars and the Milky Way, dark clouds with moonlit rims). Only their Metal goldens are committed.
  - **Tests.** GPU tests render and read back: the gradient is exactly unchanged by layers that draw nothing in view; the discs sit where `Celestial` puts them, under the horizon not at all; stars scale with their brightness, vanish at 0, and keep the frame's total star light within 1% while the camera turns by quarter pixels; card clouds dissolve from their edges to where coverage puts the threshold, and vanish at 0; the cirrus drifts at its speed, as well five million seconds in; the cube draws every face first, then one a frame in order, reads back as the sky through a cube view, and keeps the glows but not the discs; a handle that isn't a sky material draws the default sky. Layout tests check `SkyUniforms` against the WGSL struct, the texture slots and bindings, and that every variant of the three constants builds.
  - **Cost.** Against c3ac568 (sandbox `--bench 3000` and its 239 GPU-timed frames, Apple M5, release, three alternating rounds; the default scene, whose sky fills most of the frame; GPU times are the passes' mean exclusive times): on Desktop-High at 1920×1080 the main pass grows by about 0.19 ms at 10:00, with card clouds, cirrus and the sun's glow and disc (286–340 µs against 95–144), and by 0.25–0.32 ms at 22:00, with the stars, the Milky Way and the moon as well (346–400 µs against 78–99); the cube's face adds 35–45 µs, mostly a render pass's own cost. With Mobile-High's settings (1316×740, 128² faces) the sky costs 0.12–0.13 ms by day and about 0.16 ms at night, cube included; with Mobile-Low's (1049×590, no cirrus, a 64² face every other frame) about 0.06 ms by day and 0.08–0.1 ms at night. The mobile tiers' settings therefore land inside the 0.1–0.3 ms budget on this GPU, and 1080p at night goes just over it on an integrated GPU well below Desktop-High's reference (§3.1); phones are measured in X1. The cost is arithmetic per sky pixel, so the shader keeps it lean (CPU-side constants, a polynomial arctangent, linear edge ramps, one hash per star cell before the star's reach is known), and each pipeline variant leaves out what time of day isn't drawing. Measured by layer at 1080p: the derivatives every layer needs with the sun's glow and disc about 75 µs, each cloud layer about 70 µs, and the stars, Milky Way and moon about 65 µs. CPU p50 moves by a few microseconds, within the rounds' spread (115–145 µs against 97–131 at 10:00, 85.8–90.5 against 82.7–89.9 with Mobile-Low's settings). Allocations go from 305.3 to 309.6 per frame (2.2 KB more) on the desktop tiers and Mobile-High: the cube's render pass, and the frame graph grouping one more pass into command buffers (five instead of six); on Mobile-Low, whose cube pass runs every other frame, from 263.3 to 263.9.
- **As built (M1 W4, the environment shader: `shaders/environment.wgsl`, `sapphire_render::environment`, `pipelines::{SurfaceVariant, Shading}`):**
  - **The model.** One uber-shader for every environment material, a stylized GGX for the world kept softer and less contrasty than the anime characters drawn over it, so they read on top (look pillar 4; world_lighting_post.md §2). The key light is the world's (`FramePacket::sun`, the frame uniforms' `sun_*`: the sun by day, the moon at night), never a character's rig. Per pixel:
    - **Terminator.** N·L, pushed toward the deepest shadow by the cast shadow in half-Lambert space (`occluded`, as vertex AO enters the characters' split), passes `smoothstep(−w, w, ·)`: `w` is the material's width up close plus its growth × view distance (Blue Protocol's distance-dependent edges), and at least half a pixel's change of N·L, so a near-zero width is a clean two-tone step for props. The terminator sits at N·L = 0, where cast shadows begin, so the two always meet.
    - **Wrap lighting.** The lit side falls off as wrapped Lambert, `(N·L + wrap) / (1 + wrap)`: 0 is Lambert's; 0.3–0.5 keeps light to the terminator, which then shows as a soft colored step rather than a fade to black.
    - **Shadow tint and band.** The key light's diffuse is `key × mix(keep, 1, lit × falloff) × band`. `keep` is time of day's shadow tint (W1's `FramePacket::shadow_tint`, now in the frame uniforms) × the material's: shadows and the shadow side keep a colored share of the key light, never black. The band is a one-row ramp read at the terminator's value (0 on the shadow side, 1 lit), blended in by its strength: a warm band along the boundary, white at the ramp's ends.
    - **Ambient** is time of day's hemisphere through C2's `ambient_light(n)`, × vertex AO (vertex color R, the characters' contract) × the mask map's AO and cavity.
    - **Specular:** GGX with the perceptual roughness (× the mask's A) remapped into the material's range, reflectance 4% or the albedo for metals, × the mask's specular mask and cavity; each light's highlight is held under the material's specular clamp × its radiance. A reflectance the mask takes under about 2% takes the grazing reflection (Schlick's F90 and the environment BRDF's bias) down with it, Filament's specular occlusion, so a masked texel is matte at every angle on both tiers that reflect. Lambert carries no 1/π in Sapphire's convention (a unit key on a surface facing it gives its albedo), so the lobes carry π. The hemisphere also reflects, seen along the reflection vector through Karis's analytic environment BRDF, so metals and glossy surfaces keep a sky-colored sheen until W5's probes and the sky cube's prefiltered mips (decision: W2's cube has one level, which would alias as a rough reflection, so W4 doesn't sample it).
    - **Distance desaturation:** the albedo moves toward its own luminance, eased from the material's start distance to its end by its amount, so far surfaces read as painted masses while the frame's light keeps its hue. Emission keeps its color.
    - **Emission** (color × map) adds HDR light whatever the lighting.
    - **Local lights** (R5) walk the same lists as the anime character shader (`light_list`, moved into `lights.wgsl`): the diffuse through the same terminator (at least a pixel's change of each light's N·L) and wrapped falloff, a highlight each, no shadow tint, and no luminance ceiling (the world takes light as it comes).
    - **Final color** is produced in one place at the end of `fs_environment` (lit color plus emission), where W3's fog joins through a shared function.
  - **Tiers** (`QualityTier::environment`, `EnvironmentSettings`, `tiers.ron`; the shader's `ENVIRONMENT_SPECULAR` override): `Full` on both desktop tiers (GGX distribution, height-correlated Smith visibility, Schlick Fresnel); `Approximate` on Mobile-High (the distribution with visibility and Fresnel folded into `1 / (4 · max(L·H², 0.1) · (α + 0.5))` at the reflectance, Zioma's mobile GGX as Unity's mobile pipeline ships it); `Off` on Mobile-Low (no specular, metals shade as diffuse in their own color). Terminator, wrap, tint, band, desaturation, emission and local lights run on every tier: a handful of ALU operations the look rests on.
  - **The cast-shadow hook** (`cast_shadow(in: EnvironmentVaryings) -> f32` in `environment.wgsl`), documented like C1's: it returns 1 (lit) until R4 fills it. R4 returns how much of the world's key light (`FramePacket::sun`, whose maps it renders) reaches the point past every caster the key light's shadow maps hold, the environment's own and the characters' (unlike `cast_shadow` in the anime character shader, which sees environment casters only), times the per-character ground shadow on desktop and capsule shadows on mobile; sampled at `in.world_position` (offset along `in.world_normal` against acne) or, with a screen-space shadow mask, at `in.clip_position`. The shader reads it in one place, the terminator's input, so cast shadows take the terminator's distance-widened remap, the tinted keep and the band, and kill the key light's highlight; local lights cast nothing.
  - **Wetness inputs:** `FramePacket::weather` (`Weather`: wetness and puddle coverage, dry by default; nothing fills it yet) reaches the GPU as the frame uniforms' `weather`, appended. No shader reads them: the shader's header says where Lagarde's wet model (darker porous albedo, lower roughness, puddles, ripples) will apply, before any light; a porosity input joins the material then.
  - **Decision: no environment outlines in M1.** The environment family has no inverted hull (`pipelines::mesh_draw` never outlines it, whatever the flags): the spec's environment outline is the screen-space edge pass from the prepass's normals (a desktop option, the tiers' `screen_space_outlines`, later), and props that should read like characters use the anime character family, hull and all.
  - **Families on meshes.** A mesh's slots bind anime character or environment materials, mixed freely within a mesh; `upload_mesh` and `update_mesh` take both, and refuse foliage, sky and fog card materials (`RenderError::NotDrawable`). A surface variant is a coverage (opaque, alpha-tested, transparent) and a shading (`Shading::Character(part)` or `Shading::Environment`), nine in all, each a pipeline per position format built when a draw first needs it; an edit that changes an environment material's coverage moves its draws with one draw-list rebuild, as anime character permutations do. Each mesh family has a scene group of its own (group 2: the instances and its family's material buffer, `pipelines::MESH_FAMILIES`), which the main pass rebinds only when the family changes; the prepasses bind the anime character family's for every draw, reading no material.
  - **Draws sort by pipeline** (§5.12 "Sorting"): the draw list groups them by position format and surface variant, so families never interleave, whatever order instances and submeshes come in. In each coverage the anime character variants draw before the environment's, so depth rejects the world behind the characters; then the characters' outlines, the sky, and both families' transparent surfaces. Sorting by material and mesh within a group stays later work.
  - **Prepasses and TAA:** environment surfaces, opaque and alpha-tested, go through the scene prepass like any mesh (depth, normals and velocity, from their instance's transform, skinned positions where they have them, and the camera's). The character prepass draws the anime character family's skinned and morphed instances alone, so a skinned prop in environment materials (a flag, a swinging sign) never counts as a character for rim light, hair shadow or the responsive mask. Converged under TAA, a test scene of the world changes by a mean of 0.016 steps from frame to frame, one pixel of 16,384 by more than 2.
  - **Shared helpers** moved out of the anime character shader unchanged, so both shaders use one copy: `LUMINANCE`, `light_step`, `occluded` and `mapped_normal` into `frame.wgsl`, the light-list lookup into `lights.wgsl`. The sixteen earlier golden scenes render exactly as before (FLIP mean, p99 and max 0.000 on Metal).
  - **Hot reload (A8):** `environment.wgsl` is a `ShaderFile` and a mesh program of its own (`frame.wgsl`, `materials.wgsl`, `lights.wgsl`, then it); an edit rebuilds the environment pipelines built so far and nothing else, and `frame.wgsl` edits now rebuild six programs.
  - **Stats** (rule R9): `RenderStats::environment_draws` (of `draws`, which now counts every family's surfaces) and `environment_pipelines`, in the overlay's stats panel.
  - **Content and sandbox:** the pipeline's `samples::environment` writes `content/samples/environment/`: `courtyard.glb` (220 m of ground, flat around the character and rolling into hills from 45 m, boulders from 3 m to 60 m with vertex AO at their feet, a colonnade of nine pillars receding from the character's left, a plaster wall with a gap, and paper lanterns on posts and on the wall), one slot per material, bound by its sidecar to five environment material documents (ground, rock, stone, wall, lantern), a terminator band ramp and a painted ground map; all generated, nothing downloaded. The sandbox's `--environment` loads it by ID through the asset server and places it around the character, whose pedestal stands on its flat middle, with a camera pulled back 1.45× and lowered. The mannequin and its pedestal keep their anime character materials (decision: the pedestal is part of the character asset, and moving it would change every golden).
  - **Golden images:** three new scenes, Metal goldens only, under the whole sky with TAA (32 still frames), as the desktop tiers draw: `environment-day` (10:00), `environment-sunset` (18:30) and `environment-night` (22:00, with the demo lights).
  - **Tests (Metal, `tests/environment.rs`):** a CPU model of the shader predicts every pixel of analytic spheres and planes (each pixel's ray meets the exact surface) within 0.025 in linear color: wrap lighting, and a terminator that reaches N·L = −0.1 at 11 m (0.33 of its step) but not at 2 m (0.04); the shadow side keeping the frame's tint × the material's, never black, and the band warming the transition's middle; the cast-shadow hook leaving a square that faces the key fully lit (one facing away keeps the tint alone); distance desaturation turning an albedo at 40 m into its luminance in the key light's hue; each tier's specular (full, approximate, and off with a metal shading as diffuse), the clamp and the hemisphere's reflection, and the mask map's AO, cavity and roughness, a zero specular mask leaving the sphere matte under grazing light; emission on either side, untouched by desaturation; two lamps on a floor through both light paths (the same frame within 64 pixels) and a lamp's terminator on a sphere; both families in one mesh and in another, each through its own shader, a coverage edit building one environment pipeline per format drawn with one draw-list rebuild, alpha test and blending, and a sky material refused on a mesh; the scene prepass holding the world's depth wherever the frame drew it, a morphed environment prop's included and the character prepass empty, no velocity while still and its projected motion when moved, and temporal AA converging (frame to frame a mean of 0.016 steps, one pixel of 16,384 over 2); an `environment.wgsl` edit rebuilding the environment's pipelines alone and swapping only the world's colors. Twenty mutations of the shader each fail a test: wrap, the width's growth, either shadow tint, the band, desaturation, the specular model, the clamp, emission, local lights and their terminator, the hook returning shadow, ambient light, its reflection, metals without specular, the mask map, a masked texel's grazing Fresnel and sheen, transparency and the alpha test; and drawing a morphed environment prop into the character prepass fails the prepass test. Headless: `EnvironmentParams`, `FrameUniforms` (with `shadow_tint` and `weather`), `InstanceData` and `LocalLight` against the environment program's WGSL; its slot, flag and specular-model constants, group 3 bindings and material binding; every environment variant under every specular model specializes and validates; the draw list groups mixed families by pipeline; each tier class's specular; the sample content's IDs, slots and winding.
  - **Cost** (sandbox `--bench 3000`, 1920×1080, Desktop-High with TAA, Apple M5, release, three alternating rounds against 6e5ad54 on a machine loaded by other agents' builds, load average 12–33):
    - Without `--environment` the frame is the base's: CPU p50 124.8–128.3 µs against 124.0–125.6, 407.7 allocations a frame on both, the GPU frame 1,257–1,403 µs against 1,252–1,348 and the main pass 266–302 against 271–288, within the rounds' noise.
    - With `--environment` (five more draws in the main pass and five in the scene prepass, about 17,000 vertices and 30,000 triangles, 1.07 MB more in the mesh pool): CPU p50 126.2–137.0 µs and 412.7 allocations (five more: wgpu recording the extra draws, the environment pipeline and the scene group's rebind; the renderer's per-frame code allocates nothing new); the main pass 304–323 µs, about 40 µs more though the courtyard takes most of the pixels the sky shaded; the scene prepass 103–122 µs against 81–83; the GPU frame 1,426–1,465 µs. Against §3.3's Desktop-Low column at 1080p: forward opaque (the whole main pass, sky included) 0.32 of 3.5 ms, culling and prepass 0.12 of 1.5 ms, the frame 1.47 of about 13.5 ms.
    - With each mobile tier's settings on the same GPU (two rounds, 4× MSAA, no TAA): the main pass grows from 156–163 to 202–258 µs on Mobile-High (approximate specular, 1316×740) and from 88–90 to 127–135 µs on Mobile-Low (no specular, 1049×590), against Mobile-Low's 8 ms for forward opaque; phones are measured in X1.

**Post-processing**

- **Bloom:** a downsample chain from 1/2 to 1/32 resolution, with an emissive-driven bloom mask.
- **Tonemapping:** a custom tone curve and grading baked into a 32³ LUT. No ACES look. Two LUTs can blend (time of day, zones), and a user filter-strength slider scales the grade, as both target games offer.
- **Anime compositing:** a wide "diffusion" glow on top of bloom, a character bloom mask, 2–4 screen-space gradient ("para") layers and sprite lens flares.
- **Gameplay-triggered effects:** finish-freeze, halftone, radial blur and chromatic aberration, triggered from `PostParams`.
- **Desktop only:** depth of field in cutscenes and optional camera motion blur.
- **Mobile:** bloom composite, LUT, vignette and gameplay effects are fused into one full-screen pass to save bandwidth.
- **As built (M1 R3, `sapphire_render::post`):**
  - The main pass renders unclamped linear HDR into an `Rgba16Float` scene-color transient, MSAA resolving into it. `Renderer::new` fails cleanly if the device can't render, filter and resolve that format. The target keeps encoding sRGB on write.
  - Bloom is a dual-filter chain (Bjørge, SIGGRAPH 2015) of frame-graph passes on pooled transients: a prefilter at ½ (exposure, soft-knee threshold, Karis average), five-tap downsamples to 1/32, and tent upsamples back to ½, each replacing `scatter` of its own level. `RenderSettings::bloom_levels` (5; at most 6) sets the length for the tiers. Levels are `Rg11b10Ufloat` where the device renders it. Bloom follows brightness alone until the emissive and character masks (C7, P2).
  - The tone curve is GT (Uchimura, CEDEC 2017) per channel: linear midtones and a short, late shoulder. A hue-preserving shoulder was tried and dropped: it couples the channels near white more than a 32³ table can follow (about two display steps of ripple along the grey axis).
  - The curve and grade are baked on the CPU when `PostParams` change: about 2.6 ms of CPU, 0.5 ms across the job pool on an M5. The LUT is 32³ `Rgb10a2Unorm` holding sRGB-encoded display color, indexed through a log shaper (ε = 1/32, maximum 16). Scene-referred grading is per channel (white balance as RGB gains, filter, contrast); saturation, lift, gain and gamma act on display values. The table then stays within 1 display step of direct evaluation for a neutral grade, 1.5 on the lit sky, and 2.5 for saturated colors at white.
  - Two LUTs blend per pixel. A key already baked in either slot is reused (least recently used is replaced), so a time-of-day transition costs one bake. Filter strength is baked as a blend toward the neutral grade, so 0 is exactly the tone curve alone.
  - One fused pass does exposure, bloom, the vignette, the shaper and both LUTs (the second fetched only while blending) on every tier.
  - Cost against M1 F3 (sandbox `--bench 3000` at 1920×1080 with 4× MSAA, Apple M5, release): the graph grows from 2 to 12 passes, which crosses the executor's parallel threshold (6 command buffers instead of 1). CPU p50 goes from 29 to 91 µs, allocations from 70 to 294 per frame, and GPU wait from about 255 to 460–515 µs. Post's own code allocates only the graph's boxed closures (bind groups are cached per pass); the rest is wgpu, about 13 allocations and 6 µs per render pass on Metal, plus the parallel encoders. Recording all 12 passes serially allocates 205 per frame but takes longer, so the per-pass cost hint the graph lists as later work is the lever. Pass timestamps put most of the post GPU time in the half-size passes (prefilter, last upsample) and the full-size composite; smaller levels cost about 20 µs each, mostly fixed per-pass overhead.

**Anti-aliasing and upscaling**

- **Desktop:** TAA with a responsive mask, or DLSS (NVIDIA + Vulkan via `dlss_wgpu`) or MetalFX temporal (via `objc2-metal-fx`).
- **Mobile:** MSAA 2× / 4× on the main pass if wgpu can keep MSAA attachments on-tile (**Verify**). Otherwise use FXAA in the fused post pass. Upscale with your own spatial upscaler, an FSR1-style EASU + RCAS port in WGSL, or MetalFX spatial on iOS.
- **As built (M1 R7, `sapphire_render::temporal`, `shaders/temporal.wgsl`):**
  - **Who gets what.** `RenderSettings::temporal` turns temporal AA on. The desktop tiers resolve with it and MSAA off; the phone tiers keep 4× MSAA without it. TAA's jittered history resolves edges inside a surface as well as silhouettes and outlines (painted shadow-bias steps, the unfiltered material-ID borders of control maps, thin highlights), where MSAA resolves only geometry. Its scene prepass already holds single-sampled depth and velocity, so 4× MSAA on top would quadruple the main pass's attachments for edges TAA already resolves. TAA with MSAA still works (MSAA resolves first), at the cost of the hull handling below.
  - **Jitter.** Every geometry pass rasterizes the same jittered samples: the view's matrices into clip space carry a Halton(2, 3) offset from its second element, and the sky follows through `world_from_clip`. The sequence is 8 phases at native resolution and 8 × the area ratio when upsampling (FSR 2's rule; 18 for 4K from 1440 rows), at most 64. It follows the renderer's frame count, so captures repeat exactly. The sky cube's six face views (W2) are never jittered. `ViewUniforms` appends `unjittered_clip_from_world` and the jitter. The scene prepass's velocity compares unjittered positions, so geometry that holds still has exactly zero velocity.
  - **Inputs.** The scene prepass (depth, velocity) and the character prepass now run under TAA; R2's graph culled both. A `responsive mask` pass writes `PrepassTargets::responsive` (`R8Unorm`, the scene's size): 1 where a character (a skinned or morphed instance) is the nearest surface, its prepass depth at or in front of the scene's. With the HDR scene color, the jitter, post's exposure and a reset flag, these are what DLSS, MetalFX temporal and FSR 2/3 take (`TemporalInputs`), so one of them can replace the resolve. The main pass's depth, below, is the resolve's own.
  - **The resolve** (`taa` pass) runs at the output's size on HDR scene color, before bloom and the tone curve, blending in a space compressed by exposed luma (`c / (1 + L)`), so a bright sample can't dominate a pixel. WuWa found that resolving after bloom and tone mapping amplified edge noise.
    - **A compute pass,** written for its cost. Each 8 × 8 workgroup gathers the scene pixels its outputs' 3 × 3 neighborhoods cover into workgroup memory once (2 × 2 `textureGather`s, color already compressed and in YCoCg), the 3 × 3 is unrolled, and textures are read through samplers. A first fragment-shader version cost 1.2 ms at 1080p on an M5: wgpu bounds every WGSL loop with a counter, which keeps Metal from unrolling the 3 × 3 loops, and each `textureLoad` carries a bounds check of three texture queries. The history is written as a storage texture; workgroup memory isn't zero-initialized, since every thread writes its share before any reads it.
    - **Reconstruction:** `exp(-4 d²)` over the 3 × 3 scene pixels around each output pixel, `d` in output pixels. The filter's sum is the frame's confidence, and the history's alpha accumulates it, so a frame counts by how close its samples landed. That makes the resolve an upsampler (TAAU), and the converged image this Gaussian. Wider filters (Unreal's `exp(-2.29 d²)`) visibly softened outlines; sharper ones (6, 9) converged less far in 48 frames.
    - **History:** fetched through a five-tap Catmull-Rom filter, clipped toward the mean of a YCoCg variance box (γ 1.5; at 1.0 one-pixel outlines lost visible darkness against 4× MSAA), and clamped back into RGB's gamut.
    - **Velocity:** a pixel follows its own surface unless one at least 10% nearer covers part of its 3 × 3, which then lends its velocity. Plain closest-depth dilation dragged edges ahead on curved, turning surfaces, whose fronts move fastest. Where no geometry is, the camera's motion at infinity reprojects the sky from depth (`previous_clip_from_clip`, computed in f64 and exactly the identity for a still camera).
    - **Outlines:** the inverted hull draws no velocity until C4, and at 1080 rows its default width of 5.5 px reaches far beyond any 3 × 3 dilation; ghost lines trailed every moving outline without handling. So the main pass keeps its depth under TAA, and where it is at least 10% nearer than the scene prepass's, the pixel is a hull pixel. It takes the velocity of the nearest surface matching its depth, searching three rings of eight taps out to 8 px at 1080 rows, and counts as responsive. So does any pixel with a hull pixel in its 3 × 3 in front of its own surface: the outline's antialiased edge, which a frame's jittered hull may miss, otherwise kept a still history of what the outline moved over (a moving outline's leading edge kept 75% of 4× MSAA's darkness, against 85% with the rule). On a body turning about its own axis the outline holds still while the surface slides, and no surface pixel carries that, so such outlines soften in motion until C4 writes the hull's velocity.
    - **Disocclusion:** what the main pass drew this frame (hulls included) against the farthest of last frame's drawn depths in the 3 × 3 around where the pixel came from, with a 10% margin. A single pixel's depth flips across an edge as the jitter moves its sample, and testing it alone rejected the history of still edges once per cycle (they flickered).
    - **Weights:** the history counts at most 32 frames' worth while still, 4 at 2 px a frame or when resampled by half a pixel. Resampling loss is `4f(1 − f)` for an offset of f; Catmull-Rom halves a one-pixel line's peak at f = 0.5, so thin lines in slow motion faded under long histories. Responsive pixels moving that fast keep half, with a γ 1.0 box. Characters that hold still accumulate like everything else, since their thin outlines need the full history.
  - **History lifetime.** The history is a pair of `Rgba16Float` textures at the output's size; the kept depth is a pair at the scene's size (the main pass's depth, or the scene prepass's under MSAA). Both are persistent and imported into each frame's graph. The history is dropped when a renderer starts (device loss or a tier change makes a new one), when the output's size changes, and on a camera cut: `ViewData::cut`, new, which also counts the camera as unmoved for velocity and carries over from a prepared frame that was never recorded.
    - **Deviation from the plan:** a render-scale change keeps the history. It is at the output's size, so it stays valid while dynamic resolution moves the scene's, as FSR 2 and DLSS keep theirs. Dropping it would flash aliased edges at every dynamic-resolution step: in a GPU test a step from scale 1 to 0.85 changes the converged frame by a mean of 0.01 steps (2 of 129,600 pixels by more than 32), a reset by 0.41 (625 pixels).
  - **Upsampling** restores §3.1's caps (Desktop-High 1440 rows, Desktop-Low 1080). Jitter and resolve follow each frame's scene size, so dynamic resolution keeps working. Against native 4× MSAA, a converged TAAU frame at two-thirds scale differs by a mean of 0.74 steps (505 pixels over 32), against 1.69 (1,266) for the old bilinear upscale; at half scale 0.95 (522) against 2.57 (1,706).
  - **Sharpening measured, not added.** RCAS (FSR 1's, 0.2 stops, in compressed HDR) in the fused pass moved TAA further from 4× MSAA in every case on the sandbox: mean 0.51 → 0.64 steps and p99 8 → 13 when still, TAAU 0.68 → 0.80, in motion 1.01 → 1.15 (measured on the fragment version of the resolve, whose output the compute one matches within a FLIP mean of 0.0003). It also drew light halos along the dark outlines. P2 can offer it as a player setting.
  - **Mobile.** The 4× MSAA color and depth attachments were already `TRANSIENT_ATTACHMENT`, cleared and discarded on tile (since M0). A GPU test now holds the mobile tiers to that: 4× samples, exactly `RENDER_ATTACHMENT | TRANSIENT_ATTACHMENT`, and no TAA or prepass running. **Verify** in M2's device testing: the tile-memory saving and the cost on Mali and Adreno, and on Apple GPUs (`transient_saves_memory` is logged at startup). The iOS and Android targets build; the Simulator wasn't run (only W2 runs it in this batch).
  - **Stats (rule R9):** `RenderStats::temporal` (`TemporalStats`) has the scene and output sizes, the jitter's index and phase count (`TemporalStats::jitter`), and history resets with the last reason. The overlay's stats panel shows them, and a "TAA / upsampling" row adds the two passes' GPU time (`temporal::PASSES`, F5's timings) against §3.3's 0.8 ms. The bench logs `gpu_taa_us` and the prepasses' and post's GPU time.
  - **Sandbox:** `--aa off|msaa|taa` (`--msaa` still sets the count), `--spin <rad/s>` for the turntable, and `--capture-frames N [--capture-step S]` to draw N frames before a capture, S ticks apart (0: still, for convergence).
  - **Tests (Metal):**
    - The jitter moves an edge as the sequence says, in x and y, for two whole cycles, and the stats report it.
    - Still geometry has exactly zero velocity under jitter; a quad moving 3 px a frame has 3 px within 0.005.
    - A still scene converges: against a 64-sample reference (4× MSAA at 4× size), one sample's mean is 1.27, 4× MSAA's 0.33 and TAA's 0.43 (p99 12 against MSAA's 12). After convergence the frame-to-frame change is at most 0.06 steps, with at most 143 of 129,600 pixels moving more than 2 where a jitter phase misses a thin feature's end.
    - Under camera motion, panning and turning over a comb with a sky, geometry reprojects without ghosts (1,148 pixels over 48 against 4× MSAA, mostly edges; 6,050 with velocity ignored).
    - Disoccluded pixels and the pixels a moving outline leaves restart their accumulation (history weight ≤ 1), while untouched ones keep theirs.
    - The responsive mask marks visible characters only. Moving, a character keeps half the history weight other geometry does; still, the same.
    - A moving outlined sphere keeps 85% (leading edge) and 102% (trailing) of 4× MSAA's outline darkness over a jitter cycle, centered within 0.13 px of it; without the hull handling the leading edge keeps 50%. A single frame varies with the jitter phase (57–89% on the leading edge), so the test averages a cycle.
    - Resets happen on start, resize and cut, but not on a render-scale change; upsampling is described above. TAA runs at any size from 1 × 1, at any scale, with and without MSAA.
    - W2's night sky (stars and the moon's disc, which write no velocity and reproject from depth at infinity): held still, the frame's light holds within 0.4% from frame to frame and 4% of 4× MSAA's, with at most 13 pixels changing by more than 4 steps (no shimmer under the jitter). Turning by 1.3 px a frame, the light is within 1% of MSAA's, and the stars line up with MSAA's: the squared difference is 156, against 618–635 for MSAA's moved a pixel either way (46 still; the history's resampling softens them a little). Without the sky's reprojection the stars keep 35% of their light.
    - Hot reload (M1 A8): `temporal.wgsl` is a `ShaderFile` and its own program; an edit rebuilds the mask's and the resolve's pipelines (and changes what the resolve writes), a resolve that validates but doesn't build leaves the old ones drawing, and the compiled-in source draws the first frames again exactly.
    - Breaking disocclusion, the responsive mask, the hull search, the kept depth or the sky's reprojection each fails a test.
  - **Golden images:** the thirteen earlier scenes (C2's `turntable-night`, C3's `faces` and W2's three sky scenes included) pin `--aa msaa` (`--aa off` for `turntable-msaa-off`), and their captures are unchanged (FLIP mean, p99 and max 0.000). Three new scenes, Metal goldens only, deterministic run to run, with W2's `--plain-sky` as the other character scenes have, so they stay what they were before the sky's layers:
    - `turntable-taa`: still, after 32 converging frames. Outlines and light splits are crisp, close to 4× MSAA.
    - `turntable-taa-motion`: the turntable at 3 rad/s for 32 frames. No ghost trails or smears; outlines are thinner than 4× MSAA's at this resolution (1.8 px), with a faint second line beside the legs' outline. At 1080 rows the spinning head's outline is visibly fainter than 4× MSAA's: the body turns about its own axis (see "Outlines" above).
    - `turntable-taa-upsampled`: 427×240 resolved to 640×360, nearly indistinguishable from native.
  - **Cost** (sandbox `--bench 3000`, release, Apple M5):
    - **1920×1080, native,** four alternating rounds against 877fb16 (A8, with W2's sky) on a quiet machine: the TAA passes (mask and resolve) take 598–599 µs of GPU time, against §3.3's 0.8 ms. The prepasses TAA runs add 73–74 µs; the main pass, now without MSAA, takes 273 µs against 289 (the sky draws in it); post 74 µs and the sky cube 31–33 µs on both. The timed GPU frame is 1,193–1,196 µs against 471–488. CPU p50 is 119–121 µs against 102–104 (p99 201–235 against 141–173), with 407.7 allocations a frame against 309.6: the four passes TAA records (two prepasses, the mask, the resolve) cost wgpu's per-pass recording and the graph one more command encoder. With `--aa msaa` the branch is the base again: 309.6 allocations, CPU p50 102 µs, GPU frame 496 µs.
    - **3840×2160 from 2560×1440,** three alternating rounds against b349915 (C2) on a machine shared with other agents' builds and GPU work, so the ranges are wide: TAA takes 2,306–2,542 µs and post 347–384 µs at the output's size; the GPU frame is 3,410–3,969 µs against 844–1,715 for the base's native 4K with 4× MSAA. CPU p50 145–172 µs against 100–133.
    - The fragment-shader resolve took about 1.2 ms at 1080p and 4.7 ms at 4K; the compute pass halved both. Bisecting it at 4K: gathering the tiles about 0.5 ms, the 3 × 3 about 0.5 ms, the history's five taps 0.46 ms and disocclusion 0.18 ms. §3.3 budgets Desktop-Low at 1080p only; at 4K output the resolve is well over 0.8 ms, and MetalFX temporal or DLSS replace it where they run.

**Materials, shaders and PSOs**

- **Source:** WGSL with WESL imports (`wesl` 0.6), validated and reflected with naga. Parameter structs are laid out with `encase` (through glam's `encase` feature) / `bytemuck`. They are written by hand and checked against naga's reflection in tests. `wgsl_bindgen` is out: it is built on the archived `naga_oil` and can't read WESL imports.
- **Permutation inventory:** the cooker writes, per pack, the list of `(family, permutation bits, vertex layout, render state)` it needs.
- **Warm-up:**
  - At boot, compile the global pipelines behind the splash screen.
  - At pack load, compile the listed pipelines on workers. An object stays hidden until its pipelines are ready.
  - On Vulkan, persist the pipeline cache.
  - On Metal, warm up behind load screens (**Verify** whether wgpu exposes Metal binary archives).
- **Hot reload:**
  - Desktop: file watch triggers an async recompile and a pipeline swap.
  - Devices: live link pushes the WGSL text. Shaders are translated at runtime anyway, so this works on phones too.
- **As built (M1 A8, `sapphire_render::shaders`, `Renderer::{set_shader_sources, update_mesh}`):**
  - **Sources:** `ShaderFile` names the renderer's WGSL files (eleven since R7's `temporal.wgsl`) and holds each one's compiled-in text; `ShaderSources` is the text of every file, compiled in by default, any of it replaceable (`set`); a program (one shader module) is its files joined by newlines, `frame.wgsl` and `materials.wgsl` first, exactly the concatenation before (every golden is pixel-identical). The concatenated constants became one table (`Program::files`), so a shader file is named in one place. The renderer only ever gets text: development builds read edited files and hand them over, live link (A9) will push them, and shipping builds use the compiled-in files alone (rule R5).
  - **Rebuilding:** `Renderer::set_shader_sources` finds the programs whose files changed, checks each with naga as wgpu compiles it, with no optional capabilities (as the headless shader tests always have, so an edit can't use what phones lack), and maps naga's location in the program back to the file and line it is in (`sky.wgsl:32:12: expected identifier`; `ShaderError::Invalid` also carries naga's full report). It then builds the new modules and every pipeline built so far from their programs' old versions, each on the pipeline layout it was built with so every bind group stays valid (the mesh programs per position format, surface variant (coverage and part, C3) and outline coverage, the character prepass, the sky's six (W2: four layer variants into the scene and two into the sky cube, which then draws all six faces again), post's four, the light-cluster and skinning passes), inside wgpu validation and internal error scopes, so a program naga accepts but wgpu refuses (a renamed entry point, a binding the layouts don't have) is `ShaderError::Pipeline` rather than an uncaptured, in debug builds fatal, error. Only if everything succeeded do the new pipelines replace the old ones, from the next recorded frame; otherwise the renderer draws on with what it had. Unchanged programs keep their modules and pipelines; pipelines built later (a new permutation, f32 positions, the first skinned mesh) use the current sources, inside the same error scopes while they are edited ones: if wgpu refuses one of those (an edit that renamed a permutation constant no pipeline built so far set), the renderer goes back to the compiled-in shaders, rebuilding every pipeline from them, and logs why, rather than draw with an invalid pipeline. `RenderStats::shader_rebuilds` counts applied edits (rule R9).
  - **Synchronous, on purpose for now:** the frame that applies an edit compiles on its own thread. A code change to the anime character shader rebuilt its two pipelines in 118–148 ms (sandbox with `--swatches --skinned --lights`, release, M5: naga's check, wgpu's own, and Metal compiling the uber-shader cold), and one to `sky_color` in `frame.wgsl` the five programs that include it and nine pipelines in 179–183 ms (both measured before W2's sky, whose six pipelines a `frame.wgsl` edit now rebuilds too); an edit whose generated code Metal has compiled before (a comment, a file restored) takes 3–6 ms. So the frame that applies an edit hitches by up to about 0.2 s, once, in development builds only. Compiling on workers and swapping when ready belongs with the warm-up above (crate docs, later milestones).
  - **New shader files** (R7's TAA) join `ShaderFile` (name and `include_str!`) and their program's `Program::files`; a new program also needs its pipelines' owner to build from `ShaderSources` and rebuild in `set_shader_sources`, as the mesh pipelines, the sky (`sky::Sky`, W2, which kept `sky.wgsl`), post, the light clusters and skinning do.
  - **Meshes replaced in place:** `Renderer::update_mesh(handle, mesh, assets)` replaces a mesh under its handle, as `upload_slot_materials` and `upload_mesh` would upload it: instances draw the new mesh from the next frame (skinned ones from their bind pose until posed again: their outputs are made anew, since the vertex count may differ), per-object light lists are chosen again for its new bounds, and the materials it embeds rewrite the slots the old mesh's embedded materials had (slot by slot where the family matches) instead of taking new ones. The mesh pool's buffers now hand out ranges first fit (the skinning outputs' `RangeAllocator`) instead of only appending, and a replaced mesh's ranges go back: replacing a mesh again and again keeps the pool's size and grows no buffer. Ranges are freed right after the new ones are written, which queue order makes safe for frames already submitted. Materials were already updated in place (A3); a texture replaced on the app's side is bound again by updating the materials that use it, which the asset server sends again for that (§5.10).
  - **Tests (Metal):** editing the sky rebuilds its six pipelines and only the sky changes; editing the anime character shader to swap red and blue rebuilds its one built pipeline and swaps the sphere's channels; appending a comment to `frame.wgsl` rebuilds the five programs that include it and their ten pipelines and draws the same pixels; with opaque, alpha-tested and transparent bodies and a face drawn, comments in the anime character shader, the outline shader and `frame.wgsl` rebuild four, two and fourteen pipelines and draw the same pixels; going back to the compiled-in sources draws the first frame again. A syntax error is reported at its file and line, a type error at its line in `sky.wgsl`, a renamed entry point as a pipeline error, and after all three the renderer still draws the same pixels and then takes a fixed edit. A new texture through `update_material` recolors a sphere uploading one slot; a mesh replaced by one of half the radius draws at its new size under the same handle; replacing it four more times leaves the pool's bytes and buffer growths unchanged; a mesh's embedded material replaced keeps the material count; an unresolvable slot and an unknown handle fail without changing the frame; an edit good for the pipelines built so far but not for an alpha-tested surface uploaded afterwards sends the renderer back to the compiled-in shaders, and the surface draws. Headless: the program table composes the compiled-in files into the same text as before, and naga's line numbers map back to files.
- **As built (M1 A3, `sapphire_render::materials`, `shaders/materials.wgsl`):**
  - **Material buffers:** one parameter buffer per family, an array of its block, bound as group 2 binding 1 by that family's pipelines. R2's character and scene prepasses share group 2 but leave group 3 out, so only the main pass binds each draw's material, and only when it changes. One shared array would pad every slot to the 896-byte anime character block, and a pipeline only reads its own family, so the per-stage storage-buffer count is unchanged. `Renderer::upload_material` gives a material a slot for the renderer's lifetime (a `MaterialHandle`); draw records name it by that slot. Slots are written through a CPU mirror and the frame's uploads pass sends the changed ones through the staging belt, in runs. `Renderer::update_material` rewrites one slot and rebinds its textures: nothing else is uploaded and the draw list isn't rebuilt, which is the hot reload (A8) and live link (A9) path for a material edit. A GPU test scribbles over every slot behind the renderer's back, updates one, and reads the buffer back: only that slot was rewritten. `RenderStats` counts materials, their bytes and the slots each frame uploads.
  - **Group 3** per family: texture slot `i` at binding `i` (filterable 2D, fragment-visible), a trilinear repeating sampler at 8 and a clamped linear one for ramps at 9; unbound slots get a white 1×1 texture. The anime character family takes 7 of the 16 sampled textures and 2 of the 16 samplers a fragment stage has on Vulkan 1.1 phones, and group 3 is the last of their 4 groups. The renderer checks each bound texture's swizzle against the one the material was cooked with. Every family uploads and binds its textures now; their shaders come with W2 (sky), W3 (fog cards), W4 (environment) and W6 (foliage), and until then `upload_mesh` accepts only anime character materials.
  - **The anime character family drives the anime character shader (`anime_character.wgsl`) and its outlines:** base color × base color map × vertex tint, region 0's shade, rim tint and outline color, the split's threshold and softness, the rim's width and strength and the outline's width (times region 0's scale) all come from the block (the rim width was a shader constant). The shaders read the block field by field from the buffer rather than copying all 896 bytes, which a per-pixel region index would otherwise force into scratch memory on mobile GPUs. Textures are sampled with explicit gradients taken once per pixel, so unbound slots are skipped on a branch, and through the slot's swizzle code. Ramp and three-band diffuse, per-region reads of the control map, faces, hair and eyes are C1–C7's, over data already in the block; alpha-tested and transparent materials draw opaque until C1's permutations.
  - **API:** `upload_material(material, textures)` takes a resolver from texture ID to `GpuTexture`; `upload_embedded_materials(mesh)` uploads a mesh's generated materials; `upload_mesh(mesh, materials)` takes a handle per material slot. Resolving asset IDs stays the app's until A4.
  - With 4× MSAA the mannequin and R2's skinned stalk render bit-identically: release captures of the golden scenes (turntable start, turntable, demo lights, render scale 0.7, skinned) are the same PNG bytes as R2's. Without MSAA, 14 of 230,400 pixels differ, 12 by 1 to 3 steps and 2 at the hair's rim by 10 and 20: the base color map's branch changes how Metal's compiler schedules the shader even when it isn't taken (without the branch the capture is identical). The goldens pass unchanged, all six scenes at mean and p99 0.0000 FLIP (`turntable-msaa-off` max 0.060). GPU tests on Metal check that a threshold edit moves the split to where N·L equals the new threshold (within 2 pixels) and color edits change the lit and shadow tones as predicted, each uploading one slot; that a texture is sampled through its swizzle (a reversed-channel RGBA8 texture everywhere, and the mobile cook's two-channel ASTC mask through `ra01`); and that a mismatched swizzle or a missing texture is an error.
  - Cost against M1 R2 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, two sets of three alternating rounds): CPU p50 93.2–96.5 µs against 90.8–93.5 (medians 94.1 and 91.9), about 2 µs more; GPU wait 481–515 µs against 486–516, and the timed GPU frame 249–310 µs against 260–360 (main pass 76–100 against 77–102), within noise; 305.3 allocations per frame against 303.3. Against M1 R6, before R2, the CPU difference was about 1 µs and the allocations 301.3 against 299.3. The renderer's per-frame material code allocates nothing, so the two are presumably wgpu's, recording the ten group 3 changes (the mannequin's five materials in two passes; not traced). Sorting draws by material, planned with a second opaque pipeline, would cut those changes. The mannequin's materials take 4,480 bytes of the anime character buffer, and the mesh pool holds 229,296 bytes against 229,616, its old 64-byte materials gone.
- **As built (M1 W4):** the environment family draws (`environment.wgsl`, "Environment" above), with pipelines per coverage and position format built when a draw first needs them, on a layout binding its own scene group (the instances and its material buffer) and its group 3 (five texture slots, the trilinear and ramp samplers). `upload_mesh` takes anime character and environment materials, mixed within a mesh; the other families stay `RenderError::NotDrawable`. Hot reload: `environment.wgsl` is a `ShaderFile` and a mesh program of its own (twelve files, nine programs), rebuilt as A8 rebuilds the others. Shared shader code moved unchanged out of `anime_character.wgsl`: `LUMINANCE`, `light_step`, `occluded` and `mapped_normal` into `frame.wgsl`, the light-list lookup into `lights.wgsl` (`light_list`).

**Quality tiers**

A data file (`tiers.ron`) lists, per tier:

- Render scale.
- Shadow cascades and resolution.
- Character shadow on or off.
- AA mode.
- Bloom levels.
- AO, volumetric fog and screen-space outlines on or off.
- Particle cap and LOD bias.
- Max local lights.

At runtime, dynamic resolution adjusts the render scale from GPU timings and thermal state.

- **As built (M1 R6, `sapphire_render::quality`, `crates/sapphire_render/tiers.ron`):**
  - **The table** is a `TierTable` document (`sapphire::render::TierTable`, schema 1) of one `QualityTier` per tier of §3.1, each knob listed above plus the tier's frame rate, GPU frame budget and selection rules. A table is checked however it is made, deserialization included: every tier exactly once, every knob in range (render scale 0.25–1, at most 6 bloom levels and 1,024 lights, 1–4 cascades of 256–8192 texels, a GPU budget that fits the frame), and each class's lowest tier without rules.
  - **Compiled in, not cooked content:** no frame can be drawn without a tier, and a table in the binary can't be missing, stale or unreadable on a phone, so the app always starts; a unit test proves the shipped one parses and validates. Hot reload and live link (A8, A9) can hand an edited table to `TierTable::from_ron`, which checks it the same way.
  - **Drawn from today:** the render scale, MSAA, bloom levels, the light path and the light budget. `QualityTier::render_settings` gives the `RenderSettings` a renderer is created with; `RenderSettings::for_device` (R5) is gone. Both desktop tiers keep exactly the settings every golden image renders with (4× MSAA, 5 bloom levels, clustered, 64 lights) and render scale 1 up to 1080p, so they differ only in what later packages read. Mobile-Low cuts bloom to 4 levels. **Plain data until their packages read them:** temporal AA (R7), shadow cascades, their resolution and the character shadow (R4, C2), capsule AO and GTAO (W5), volumetric fog (beyond W3's fog, which every tier has), screen-space environment outlines (C4), the particle cap (`sapphire_vfx`) and the LOD bias (LOD selection); the frame rate is also what frame pacing (§6.2) will hold. The crate docs name each consumer.
  - **Selection** (§3.1's as-built note has the rules) never looks at the OS (rule R7). Three overrides, in precedence order: the `r.tier` cvar unless `auto` (dev builds), the player's setting (the sandbox's `--tier`, until a settings screen), the device's tier. Any tier runs on any device, so mobile tiers preview on a desktop. A renderer keeps its settings for life: a tier change makes a new renderer on the same device, as device loss does, and a failed one keeps the old tier.
  - **Render scale:** `Renderer::set_render_scale` (0.25–1) sizes everything before post's fused pass at the scene's size, `scaled_size` of the target (each side rounded): the main pass's attachments, the views (so outline widths and light-cluster tiles follow) and the bloom chain. The fused pass writes the target, and on a uniform branch samples scene color bilinearly where it is smaller. At scale 1 it loads texel for texel as before, and the four golden scenes' captures came out byte-identical to F5's. The base scale is `r.render_scale` if set, else the player's (`--render-scale`), else the tier's for the output's height.
  - **Dynamic resolution** (`quality::dynamic`), off by default (`r.dynamic_resolution`, `--dynamic-resolution`): a controller that takes GPU time as proportional to scene pixels and smooths that load over samples (each moves it a fifth of the way). It drops the scale when the predicted frame is over the tier's GPU budget, straight to the largest 5% step predicted under 85% of it (at most three steps at once), and raises it one step at a time, after half a second, only while the step up is predicted to stay under 85%: the band is why a step up can't trigger the next step down. After a change it waits for a sample of a frame drawn at the new scale, since timings come back one to three frames late. Fixed per-pass costs make predictions pessimistic going up and optimistic going down, so it settles from above. On a doubled load it settles in two decisions (1 → 0.85 → 0.75) within 15 frames and recovers in 30-frame steps, with ±8% noise it changes once, and its floor is the tier's (0.7 on desktop, 0.75 on phones). Its signal is the frame graph's GPU pass span (F5's timings, which the app keeps on while it runs, through the overlay's `configure_graph` in dev builds); while a new executor's first timings are on their way (startup, device loss) it waits rather than judge frames that include loading. Only devices without timestamp queries follow the CPU frame time instead, against the tier's frame and only for steady frames (presented, nothing rebuilt), counting only a missed vsync (115%) as over, so on a vsync-paced display a lowered scale holds until a later probe raises it. Steps keep the transient pool from making textures every frame; each step is one new set of scene-sized targets, released after 60 unused frames.
  - **Stats** (rule R9): `Quality::stats` (the tier and where it came from, the device's tier, base and actual scale, the controller's state, estimate, band and changes) and `RenderStats::scene_size`, in the overlay's stats panel.
  - **Golden images:** `turntable-render-scale` renders the turntable at scale 0.7 (448×252 upsampled to 640×360). Only its Metal golden is committed; lavapipe and WARP goldens come from CI.
  - **The sky (M1 W2):** each tier gains `sky` (`SkySettings`): the sky cube's size (a power of two in 16–1024) and frames between face updates (1–60), and whether the cirrus layer draws. A table without it reads the desktop sky. The shipped values and why are in §5.12, "Sky".
  - **Cost** against M1 F5 (sandbox `--bench 3000` at 1920×1080 with 4× MSAA, Apple M5, release, two sets of three alternating rounds): at render scale 1, CPU p50 90.4–91.3 µs against F5's 90.6–95.9 (one cold first round at 99.2 aside), and allocations unchanged at 299.3 per frame and 71,231 bytes: following the tier and the scale is a few cvar loads and some arithmetic per frame. At 0.7 (1344×756), p50 90.5–92.4 µs with the same allocations. The mean GPU wait stays at 460–500 µs at both scales, mostly submission latency on a one-character scene; with GPU timing on (`--dynamic-resolution`, which holds the scale on this scene) the pass span goes from 263–271 µs at scale 1 to 207–234 µs at 0.7, since fixed per-pass costs dominate a scene this small. Timing itself costs about 50 µs of CPU and 101 allocations per frame on Metal (render-pass timestamps, their resolve and the readback's map), which dynamic resolution pays only while it is on.
- **As built (M1 R7):** `antialiasing` is drawn from: both desktop tiers set `(msaa: Off, temporal: true)` and the phone tiers `(msaa: X4, temporal: false)`, and `QualityTier::render_settings` passes `temporal` on (`tiers.ron` gives the reasons; "Anti-aliasing and upscaling" above has the resolve). The desktop tiers' height caps are back (§3.1's R7 note). `RenderSettings::default()` stays 4× MSAA without TAA, which the golden scenes from before R7 pin (`--aa msaa`) and most GPU tests draw with, since one frame shows them completely. The golden runners, which select Desktop-Low, would otherwise resolve with TAA. A unit test holds each tier class to its antialiasing, and a GPU test holds the phone tiers' MSAA to tile memory.
- **As built (M1 W4):** each tier gains `environment` (`EnvironmentSettings`): the environment shader's specular model, `Full` on both desktop tiers, `Approximate` on Mobile-High and `Off` on Mobile-Low (`tiers.ron` and "Environment" above give the reasons). A table without it reads `Full`. It is a pipeline-overridable constant, so a tier change builds the environment pipelines anew, as every tier change makes a new renderer.

**Debug views**

- Wireframe and overdraw.
- Light-cluster heatmap.
- Per-pass GPU timings.
- Material ID and ramp-row views.
- Outline-width view.
- **As built (M1 F5):**
  - Per-pass GPU timings are the frame graph's (`graph::timing`), off until `GraphExecutor::set_gpu_timing` turns them on. Each running pass owns a pair of timestamp queries. Where the device writes timestamps between passes (`TIMESTAMP_QUERY_INSIDE_ENCODERS`: Vulkan, DX12) the executor writes them around every pass; on Apple GPUs, whose counters sample only at stage boundaries, a pass hands `PassContext::timestamp_writes()` to its render pass, or `compute_timestamp_writes()` to its compute pass (the renderer's main, post and light-cluster passes, and the overlay's, do), and passes that begin neither go untimed. Only written pairs are resolved, each into its own 256-byte slot, and copied into one of three readbacks mapped without waiting; results arrive a frame or two later as `GpuTimings` (per pass, plus the span of the frame). On tile-based GPUs passes overlap: Apple's start a pass's vertex work while earlier passes still run, so a pass's start-to-end `duration` includes waiting on them (measured on the M5: every bloom pass about 0.5 ms). Each `PassTime` therefore also has an `exclusive` time, from the later of its start and the latest end before it to its end: what the pass added to the frame (main 0.33 ms, bloom prefilter 0.20 ms, each small bloom level under 0.06 ms). Where passes don't overlap the two are equal; the overlay shows `exclusive`. Devices without timestamp queries report timing unavailable. Dynamic resolution (quality tiers above) can read the same timings.
  - `GraphExecutor::set_report` keeps a `GraphReport` of the last graph (every pass with whether it ran and its command buffer, every texture with its pooled slot and lifetime) for the overlay's frame-graph view; it reuses its lists.
  - The shader-based views (wireframe and overdraw, the light-cluster heatmap, which will show per pixel what `LightStats` counts per frame, material ID and ramp rows, outline width) are deferred to a follow-up after A5 and R5 land, since both change the shaders the views patch. They plug in as a choice cvar the renderer declares and reads in `prepare` (`r.debug_view`, a `sapphire_core::cvar::CvarType` enum), which the overlay's cvar panel already edits as a list.

**Tests**

- Golden images per pass on lavapipe (Linux, pinned Mesa), WARP (Windows, pinned NuGet build) and Metal (the hosted macOS runner's paravirtual GPU), one golden set per backend, compared with `nv-flip` mean and percentile thresholds. Hard two-tone terminators and outlines flip whole pixels, so the gates are p95/p99 plus an error-pixel budget, alongside intermediate-buffer goldens (Q6, M1 plan §3).
- Permutation compile tests: every material compiles on every backend in CI.

### 5.13 `sapphire_anim` (L3)

**How**

- **Runtime:** `ozz-animation-rs` 0.11, vendored, with these jobs:
  - Sampling, blending (including additive and per-bone masks).
  - Local-to-model conversion.
  - Two-bone and aim IK.
  - Each character evaluates in its own job, fully parallel.
- **Cooking:** source clips come from glTF or FBX (`ufbx`). Root motion is extracted into its own track, and clips are written as ozz archives. **Verify** whether ozz-animation-rs covers the offline build. If not, run upstream ozz's `gltf2ozz` / `fbx2ozz` (C++) inside the cooker.
- **Animation graph (engine-owned data asset), per character:**
  - A layered state machine.
  - A locomotion blend space (1D/2D).
  - A combat layer driven directly by the move graph.
  - Additive hit-reaction and breathing layers.
  - Upper-body overrides.
- **Transitions use inertialization, not crossfades.** Snappy cancels need the new pose immediately, with the old pose's velocity decayed out.
- **Root motion and warping:** the move graph decides whether root motion applies. Motion warping scales and rotates root motion so an attack lands on its target within a window.
- **IK:**
  - Foot placement: two-bone IK plus a pelvis offset, from ground ray casts.
  - Head and eye look-at.
  - Hand IK for weapons.
- **Secondary motion:**
  - Spring bones for hair, ribbons and accessories, ported from the VRM spring-bone spec. Use `bevy_vrm1` as a reference.
  - An XPBD chain / cloth solver for skirts and coats, with capsule colliders on the legs and body.
  - Both run on the owner clock with fixed sub-steps, reset on teleport or camera cut, and use LOD: fewer iterations on mobile, off when off-screen.
- **Faces:** blendshape weight tracks in clips, plus a procedural blink. Lip-sync curves come *later*.
- **Output:** model-space matrices go to skinning palettes in extract, and blendshape weights go to `MorphWeights`.

**Tests**

- Pose snapshot tests (`insta`) for blend and IK edge cases.
- Determinism: the same clip time gives the same bytes.

### 5.14 `sapphire_physics` (L3)

**How**

- Wrap `rapier3d` 0.36 completely behind engine types, so a Rapier upgrade or a switch to Jolt touches only this crate.
- **Uses:**
  - Static world collision (trimesh, convex, heightfield).
  - Kinematic character capsules.
  - A few dynamic props.
  - Trigger sensors.
- **Queries:** ray casts, shape casts and overlaps for the controller, camera and AI.
- **Step:** fixed 60 Hz inside `FixedPhysics`, with the deterministic feature on.
- **Hit detection is not rigid-body physics.** `HitQuery` uses `parry3d` 0.31 shapes:
  - Hitboxes are spheres, capsules or oriented boxes attached to bones.
  - Each tick, a hitbox is swept from its previous position to its current one, so a fast swing can't tunnel through a target between 60 Hz ticks.
  - It tests against the hurtbox set, with broadphase over a dedicated collision group.
  - Multi-hit attacks dedupe hits by hit-group ID.
- Ragdolls come *later*. If console certification or large worlds ever demand it, swap in Jolt (via `joltc`).

### 5.15 `sapphire_audio` (L3)

**How**

- **Decision: FMOD Studio** through your own `bindgen` layer (`sapphire_fmod_sys` plus a safe wrapper, modeled on `fmod-oxide`). **Verify** the license tier and cost.
- **Designer workflow:** designers work in FMOD Studio, which builds a `.bank` per platform. The cooker copies the banks into packs.
- **Cue playback:** gameplay cues map to event GUIDs. 3D events attach to entities, and the listener follows the camera.
- **Clock-aware audio:**
  - A perfect-dodge snapshot (pitch, low-pass) is driven by the world time scale.
  - Hitstop can duck or freeze attached events.
- **Music:** parameterized by combat intensity and encounter phase.
- **Voice-over:** played from FMOD audio tables, with the VO language set separately from the text language.
- **Lifecycle:**
  - Suspend the mixer on background and on iOS audio interruptions (calls, Siri), and resume on foreground.
  - Set the iOS audio session category at boot.
- **Fallback:** a `kira` 0.12 backend behind the same `AudioBackend` trait, in case FMOD licensing fails.

### 5.16 `sapphire_vfx` (L3)

**How**

- **GPU particles:** compute passes for spawning, updating and compaction write indirect draw arguments. Render modes:
  - Sprites, flipbooks and mesh particles.
  - Soft particles.
  - Distortion, into a half-resolution distortion buffer.
- **CPU particles:** only for the few effects tied to gameplay.
- **Trails and ribbons:** sword trails sample bone sockets at sub-tick rate (from interpolated poses) for smooth arcs.
- **Decals:** hit marks and ground scorches.
- **Screen effects:** published as `PostParams`.
- **Effect assets:** emitters with modules and curves, as reflected data. Effects are spawned by cues and timeline-synced, and previewed in the editor.
- **Clocks:** effects run on the owner's clock, so hitstop freezes the attacker's and victim's effects while world effects keep running.
- **Budgets:** per-tier caps on particle count and an overdraw estimate. Large smoke and fog effects render at half resolution on mobile.

### 5.17 `sapphire_ui` (L3)

**How**

- **Decision:** your own retained-mode UI. No Rust framework is proven for animated gacha-style UI.
- **Layout:** `taffy` 0.15 (flexbox and grid).
- **Text:**
  - Shaping and layout: `parley` 0.12, built on `harfrust` and `skrifa`.
  - Glyphs: rasterized into a dynamic atlas per size bucket, which suits CJK text. Large display text uses an MSDF atlas.
- **Rendering:**
  - One batched quad renderer with nine-slice and SDF rounded rectangles.
  - Gradients, masks (scissor or stencil) and custom UI shaders for stylized effects.
  - Render-to-texture insets for 3D character showcases.
- **Animation:** tweens, springs and keyframed timelines on the real clock. UI state machines handle screens and transitions.
- **Authoring:**
  - UI documents are RON (layout, styles, bindings), hot-reloaded on desktop and through live link.
  - Widgets are written in Rust.
  - Data binds through reflection paths, and strings come from localization keys.
- **Input:** pointer and touch hit testing, gamepad directional focus navigation, and keyboard.
- **Mobile on-screen controls:** a floating virtual stick, an attack button with hold and drag, a dodge button, a camera drag region and skill buttons. These live in `game_ui` on top of `sapphire_ui`.
- **Layout safety:** safe-area insets, DPI scaling, and anchors from 4:3 (iPad) to 21:9 (phones, ultrawide). Test those three aspect ratios in CI with screenshots.

### 5.18 `sapphire_input` (L3)

**How**

- **Action mapping, modeled on Unreal's Enhanced Input:**
  - Devices produce raw events.
  - A priority stack of **contexts** (Combat, Explore, Menu, Cutscene) maps them to **actions** (button, axis1D, axis2D).
  - **Modifiers** adjust values: deadzones, response curves, inversion, sensitivity.
  - **Triggers** decide when an action fires: press, release, hold, tap, double-tap, chord.
- **Timestamps:** every action event carries a real-clock timestamp. Gameplay turns them into intents in the fixed step.
- **Devices:**
  - Gamepads via SDL3, with type detection for glyph sets (Xbox, PlayStation, Switch Pro, generic).
  - Rumble and trigger rumble as cues.
  - DualSense LED and gyro *later*.
- **Touch:** gestures (swipe-to-dodge, drag-camera, pinch) are recognized here, and on-screen buttons inject actions. Target assist is stronger on touch.
- **Rebinding:** saved to user settings. The prompt glyph switches to the last-used device.
- **As built (M1 F4):**
  - One pipeline for every device: platform events become `RawInput` (in each device's own conventions, with slots for gamepads and fingers), and bindings read a vocabulary of sources the crate owns, such as `Key(W)`, `PadStick(Right)`, `Touch(Swipe(Left))` and `Virtual("attack")`, because those names are a data format. Every source yields a 2D value, positive up on every device.
  - Contexts are enabled at an integer priority; among equal priorities the latest enabled wins. Consumption is static, as in Unreal: a context's consuming actions (the default) hide their sources from lower contexts. A newly enabled context ignores inputs already held until they are released (all but `Down` bindings), so the key that opened a menu doesn't also close it.
  - Chords are a binding's `chord`: an action that must be triggered. While it is, the chord also blocks the unchorded bindings of its source in its own and lower contexts, so `Ctrl+S` doesn't fire `S`. Actions are evaluated in chord-dependency order; cycles are a load error.
  - `update(now)` replays the frame's queued input in order and judges every binding at each input's own timestamp, and at every trigger deadline in between. Action events (started, triggered, completed, canceled) carry the device event's timestamp, or the exact instant of a time-driven change (a hold completes at its press plus the hold time); a press and release within one frame both count. Motion (mouse, wheel, drag, pinch) is summed per update instead, and its action starts at the first motion's timestamp.
  - The crate reads no clock (rule R2): the app passes `Platform::now`, the clock event timestamps use. Authored durations round to whole microseconds and are compared in integer nanoseconds, so trigger timing is exact under synthetic timestamps.
  - Touch gestures: press (so taps, double taps and long presses come from the ordinary triggers; dragging or a second finger cancels it without firing release triggers), one-finger drag, two-finger pinch (the natural log of the spread, so per-update sums compose) and swipes by dominant direction. On-screen controls inject values into `Virtual` sources, which bind like devices, so an on-screen attack button can tap or hold like a key.
  - Bindings are `sapphire::input::InputBindings` RON documents (schema version 1), registered for loading by type name. `rebind(name, source)` recompiles the map; `capture_next` takes the next press for a rebinding UI. Losing focus or the foreground releases everything without firing release triggers.
  - The last-used device is keyboard and mouse, touch, or a gamepad family (Xbox, PlayStation, Nintendo, generic) from SDL's gamepad type, which `GamepadEvent::Added` now carries.
  - Steady-state frames allocate nothing (tested over 1000 frames of every device, with context switches). Cost on the sandbox's 19 bindings (Apple M5, release): about 0.2 µs for an update without input and 1 µs for one with five inputs.
  - The sandbox's default bindings are a document compiled into the app rather than cooked content: the app must stay controllable when content fails to load, and bindings don't vary per cook. Its camera actions are per-update quantities (radians, and the log of the distance factor), so mouse, touch, keys, stick and triggers all add into one action, the rate-driven ones through `ScaleByDeltaTime`. Touch also reaches the debug actions: double tap for slow motion, long press for hitstop.

### 5.19 `sapphire_nav` (L3)

- **Baking:** at cook time, `rerecast` 0.4 bakes a tiled navmesh per zone.
- **Runtime:** `landmass` 0.9 handles pathfinding, steering and local avoidance.
- **Arena enemies** mostly steer directly at the player. The navmesh is for chasing across a zone, patrols and exploration NPCs.

### 5.20 `sapphire_debug` (L3, dev builds only)

- **Debug draw API** (lines, shapes, text, persistent or one-frame), callable from any thread. Each thread writes to a thread-local buffer, merged at extract.
- **Overlay** (egui via `egui-wgpu`, through an SDL3 input adapter you write; about 300 lines):
  - Stats and budgets.
  - Frame-time graph.
  - Dev console for cvars.
  - Entity inspector (reflection).
  - Time controls: pause, step a tick, scale.
  - Hitbox and hurtbox view.
  - Input-intent buffer view.
  - Replay recorder.
- **Linking:** only `game_app` with the `dev` feature links this crate. Shipping builds compile its calls to no-ops through a thin facade in `sapphire_core`.
- **As built (M1 F5, part 1):**
  - `sapphire_debug` is `dev_only` in `xtask/layers.toml`. The sandbox reaches it through an optional dependency behind its `dev` feature (`cargo xtask run --platform <p> --features dev`); without the feature none of it is compiled. It draws with egui and `egui-wgpu` 0.37, which build on wgpu 30 (default features off, so no GLES or WebGL backend). egui's embedded fonts carry OFL and Ubuntu Font licenses, excepted for this crate in `deny.toml`.
  - **Input adapter** (`input.rs`, about 300 lines with its key tables): platform events into egui's input, in points (window coordinates times pixel density over egui's pixels per point, the display scale times the `debug.ui_scale` zoom). Logical keys come from the keycode, physical ones from the scancode, so shortcuts follow the layout; `command` is Cmd on Apple platforms. The first finger the overlay owns also drives egui's pointer. Text comes from the platform's new `Event::Text`, turned on as verbatim input (with the on-screen keyboard on mobile, and no auto-capitalization or autocorrect) only while an overlay text field has focus; Enter in the console runs the command and keeps the field focused, so the keyboard stays up. A pointer release and the pointer leaving (a lifted finger) go to egui in separate frames, so a quick tap still clicks.
  - **Routing** (`route.rs`), a priority rule rather than an input context: the overlay sees each event first. A press (mouse button or finger) belongs to the side it started on until released, so camera drags keep going over a window and slider drags never turn the camera; motion and the wheel go to the side the pointer is over; keys go to the overlay only while one of its text fields has focus, and releases always reach the game. `sapphire_input` contexts weren't used because they consume per bound source, and the overlay needs "everything over a window", decided per position.
  - **Toggle:** the ` key (by position, below Esc), a one-second hold of two still fingers on touch screens (the game gets `TouchPhase::Canceled` for the fingers it had), or the bar's Hide button. A two-finger hold was chosen over a three-finger tap because the Simulator, and tools that drive it, inject at most two fingers.
  - **Drawing:** one frame-graph pass after post, reading and writing the target (`LoadOp::Load`). With an sRGB target egui-wgpu linearizes egui's colors, so opaque colors are exact; blending happens in linear space, so windows are opaque and weak text an opaque grey.
  - **Panels:** stats and budgets (frame time against the tier's frame, the windowed app's CPU stages against §3.2, game side and critical path, the GPU pass span against §3.3, allocations per frame with the overlay's own share, `RenderStats` including R5's `LightStats` (lights drawn, culled and dropped, and the clustered or per-object lists, which the light-cluster heatmap view will show per pixel), `GraphStats` with the transient pool, the last-used input device and the adapter), a frame-time graph of the last 240 frames, the frame graph (passes in order with culled ones, their command buffer, GPU time (the exclusive time, §5.12) and reads and writes; transients with their pooled slot and lifetime), cvars (a widget per kind, reset to default), a console (`<name>`, `<name> <value>`, `list`, `reset`, history, Tab completion) and time controls (pause, single step, 0.05× to 4×, through one `ScaleSource::Debug` entry on the world clock's scale stack, so they compose with pause, perfect dodge and cinematics; a step while paused runs exactly one tick at 1×).
  - **Cvars** live in `sapphire_core::cvar`, so runtime crates declare them without depending on a dev-only crate: typed statics (`bool`, `i32`, `u32`, `f32`, or an enum of named choices) read with one relaxed atomic load, registered explicitly into a `CvarRegistry` by the app. Shipping builds link no writer, so every cvar reads its default. The sandbox declares six look-dev knobs on its `PostParams` (`post.*`) whose defaults are exactly the default look, so goldens are unchanged (M1 W1 replaced all but `post.filter_strength` with time-of-day keys); the overlay adds `debug.ui_scale` and `debug.gpu_timing`.
  - **GPU pass timings** come from the frame graph (`GraphExecutor::set_gpu_timing`, M1 F5 in §5.12), not `wgpu-profiler`, which is compatible with wgpu 30 but times encoder scopes only with `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which Apple GPUs lack; on Metal every pass would need its render pass's `timestamp_writes` from the graph anyway, and its per-scope strings, channel and per-frame maps allocate where the graph's own readback allocates nothing beyond wgpu's map callback. Devices without timestamp queries (the iOS Simulator) say so in the panel.
  - **Cost** against M1 A5 (sandbox `--bench 3000`, Apple M5, release, three alternating rounds; medians of the rounds' p50): without `dev`, CPU is unchanged (90.8 µs against 90.5), allocations stay at 299.3 per frame with 97 bytes more (each pass's job in the executor grew by its order and timing flag), and the stripped executable grows 16.7 KB (0.18%: the graph report and the timestamp hooks; the timer itself is a trait object only `set_gpu_timing` creates, so it links only where it can run). With `dev` and the overlay hidden: 90.0 µs, the same allocation count and about 650 more bytes per frame (not traced), and 4.4 MB more executable (egui, its fonts and egui-wgpu). Hidden, the overlay is a frame-time store per frame and a comparison per fixed step, and the graph's report and timings are off. Shown, the default panels (stats and graph) cost about 0.23 ms of CPU (p50 315 µs), 1,500 allocations and 1.2 MB allocated per frame, every panel about 0.3 ms and 1,950 allocations; the stats panel shows the overlay's own share. The iOS Simulator holds 60 fps with it shown.
  - `--debug-overlay[=panels]` shows it from the first frame, in the window or in an offscreen `--capture` (after 8 warm-up frames, so windows are laid out and GPU timings have come back). Overlay captures are not golden images: their numbers change every run.
  - **Not yet** (later work, in the crate docs): the renderer's shader debug views (below, §5.12), the debug-draw API, the entity inspector, hitbox and intent-buffer views and the replay recorder (M2), clipboard, IME composition previews and saved panel layout.
- **As built (M1 W1, the time-of-day panel):**
  - The `day` panel (`--debug-overlay=day`) shows the day strip: the base timeline across 24 hours, its sky from zenith to horizon and its key light, with the keys and the current hour marked. Clicking or dragging on the strip scrubs the hour. Buttons pause the cycle (time of day's own clock, not the world's), step to the previous or next key, and set the day's length (off, 2 min, 24 min, 1 h).
  - It lists every resolved value, with swatches: the key light and which body gives it, the sun and moon with their elevation and azimuth, sky, ambient, fog, shadow tint, character key, day/night factor, exposure, bloom and vignette, and the grade as one LUT or two with their blend. It also shows zone volumes, the cutscene, and the resolve's CPU time against its 0.05 ms budget. The stats panel gets a time-of-day row.
  - Changes go through `TodControls`, which the app applies before each fixed step, like `TimeControls`, and which allocate nothing.
  - The character preview is a 96 × 96 sphere in a skin tone in front of the sky gradient. It is lit as the anime character shader lights characters: a crisp two-tone split under the character rig's key light (color and intensity from time of day, direction from the key light), an authored warm shade on the dark side, the ambient hemisphere and a rim, with exposure applied and no grade. So it previews the character light C2 will draw, which the scene's characters don't use yet. It is drawn on the CPU only when the look changes: nothing while the hour stands still, and about 55 µs per frame (release build, M5) while it runs with the panel open, plus the texture upload.
- **As built (M1 C2, the character light rig in the `day` panel):** the panel lists the frame's characters (up to four) with their resolved rigs: the key's color, intensity, elevation and azimuth, and the GI's intensity, camera bias, ceilings and skin tint with the rim scale, and a playing shot rig's weight. The preview draws the first character's rig (its key, rim scale and stylized GI, the sphere taking the skin tint), seen from +Z in world axes, or time of day's character key without characters. The overlay reads the characters from the frame packet (`OverlayFrame::characters`); the stats panel counts characters lit and entries ignored.
- **As built (M1 W2):** the `day` panel lists the sky's layers with swatches: the sun's and the moon's disc (radiance, glow color, intensity and glow size, or hidden), star brightness, and the clouds (lit, shade and rim tones, card cloud and cirrus coverage). The stats panel's Sky row shows the layers the frame drew and the sky cube's size, last face and faces drawn.
- **As built (M1 A8, the hot reload section):** the stats panel shows hot reload when the app runs it (`OverlayFrame::hot_reload`, a `HotReloadStatus`): what is watched or why nothing is, assets reloaded and failed (`AssetStats`), shader rebuilds (`RenderStats`) and documents reloaded, then the last change's errors in red: a shader's file, line and message, an asset whose new version didn't load, a document that didn't parse. The sandbox fills it.

### 5.21 `sapphire_world` (L4)

**How**

- A **zone** is a cooked level asset made of:
  - Streaming cells (still used inside a zone, for LOD and distance-based activation).
  - Static geometry clusters.
  - Lights and probes.
  - Navmesh tiles.
  - Spawners, triggers and splines (camera rails).
- **Prefabs** are templates referenced by GUID, with per-instance overrides, resolved at cook time where possible.
- **Zone transitions:**
  - The load screen is a stylized transition (ZZZ's TV and hollow transitions hide loads in this way).
  - Prefetch the next zone's packs while the player is in a hub.
- **Activation budget:** at most N entities, or M µs of spawning, per frame.
- **Encounter volumes:** entering one freezes streaming and pre-warms the encounter's enemy movesets, VFX, banks and pipelines *before* combat starts.
- **As built (M1 W1, `sapphire_world::tod`):** the crate starts with time of day alone (look pillar 1, M1 plan §2). Zones, prefabs and the rest come in M3.
  - **Documents** (`sapphire::world`, schema 1, RON through `sapphire_serialize`):
    - A `TodTimeline` holds `TodKey`s at hours `0 ≤ h < 24`. Each key sets groups: sun and moon (direction, color, intensity), sky (zenith, horizon, ground), ambient (sky, ground, intensity), fog (near and far layers, height fog, sun inscatter), shadow tint, character key (color, intensity), day/night, exposure, grade, bloom and vignette.
    - Every key of a timeline sets the same groups, and the base timeline sets them all.
    - A `TodCutscene` holds a duration, blend-in and blend-out times, an optional shot time (an hour, and a rate for time-lapse shots) and value keys at seconds into the shot.
    - Both are checked however they are made: times in range and increasing, values in range, the same groups in every key. Unknown fields are errors, so a misspelled group fails rather than vanishing.
  - **Evaluation:**
    - Each key's ease (`Linear`, `Smooth`, `In`, `Out` or `Hold`, from `sapphire_math::ease`) shapes the blend to the next key.
    - Colors blend in OkLab (`sapphire_math::color`, which the pipeline's ramps now share, with cube roots through `libm`). Directions move along great circles at constant angular speed (`det::slerp_direction`). Scalars blend linearly. Timelines wrap at midnight.
    - On a key the result is that key, bit for bit.
    - Grades blend as post's two LUTs (`GradeMix`). Equal grades merge. A zone that brings a third grade into a blend between keys drops the lightest of the three, and its weight is counted (`TodStats::dropped_grade_weight`).
  - **The key light** is resolved from the sun and moon: exactly whichever one shines. Through twilight their light adds up, and the direction turns from the sun's to the moon's by the moon's share of the luminance, so the terminator glides rather than jumps.
  - **Zones:** each `TodVolume` (a sphere or an axis-aligned box, a blend distance with a smooth falloff, a strength and a priority) blends a zone's timeline, complete or partial, over the base at the same hour. Volumes apply lowest priority first and are weighed at a probe point (the sandbox's camera).
  - **Cutscenes:** a playing `TodCutscene` crossfades the look to the base and zones at the shot's hour, if it sets one, then applies its value keys, both by its eased envelope. It runs on the world's clock (so pausing the cycle in the overlay doesn't hold it) and ends by itself, or a sequencer (§5.23) seeks it after each step's tick.
  - **Clock domain** (§4.5): `TodClock` is a `LocalClock` issued by the world clock, so pause, slow motion and cinematics reach it. It adds a developer pause (a `Debug` scale on its own stack) and a cycle rate.
    - The hour is computed from integer clock time on every read, so it never drifts. Without a cycle it is exactly the hour set.
    - `tick_time_of_day` runs in `FixedFirst`, after the clocks, and allocates nothing (rule R6).
  - **`TimeOfDay`**, an ECS resource, holds the base timeline, the volumes and the cutscene. `resolve(probe, alpha)` gives each frame's `TodFrame`, which the app writes into the packet (§4.6). It allocates nothing.
  - **Stats** (`TodStats`, rule R9): the resolve's CPU time, the segment, active volumes, the cutscene's weight and dropped grade weight. They show in the overlay (§5.20) and in `--bench`.
  - **The default day** (`default_day.ron`, compiled in until packs carry it, A4) has eight keys: midnight, the blue hour, sunrise, day, afternoon, golden hour, sunset and dusk. Its 10:00 key is exactly the renderer's default look. At night characters get a brighter key of their own than the world's (C2 draws it).
  - **Sandbox:** `--time-of-day <hours>` (default 10) and `--day-length <seconds>`. The look-dev cvars for exposure, bloom, saturation and vignette are gone, since the keys drive those; `post.filter_strength` stays.
  - **Tests:** OkLab blends (straight lines in OkLab, exact ends, golden bits); wraparound at midnight; every ease; the sun on a great circle; zone weights, priorities and partial overrides; a third grade dropped; cutscene blend-in, blend-out and crossfade; document round trips and validation errors; the default day's exactness; determinism (two runs through a cycle with a zone and a cutscene give identical packets); and no allocations over 3,600 frames of ticking and resolving.
  - **Cost:** 0.1 to 0.55 µs per frame on an M5 against a 0.05 ms budget (§4.6's W1 note).
- **As built (M1 C2, `sapphire_world::light_rig`):** the character light rig's presets and their resolution (look pillar 1 on characters; §5.12 has the GPU side).
  - **Documents:** a `LightRig` is a preset, reflected and serializable: `key` (`camera_follow` 0 to 1, `camera_direction` as azimuth and elevation in degrees, `min_elevation` and `max_elevation`, `max_azimuth`, a `tint` and an `intensity` on time of day's character key), `gi` (`intensity`, `camera_bias`, `max_saturation`, an optional `max_value`, `skin_tint`), `rim` and `local_light_ceiling`. Fields left out are neutral: the world's key light with no limits, the character key as it is, unstyled GI. A `LightRigLibrary` (`sapphire::world::LightRigLibrary`, schema 1) names presets, the first the default, and is checked however it is made: at least one, distinct non-empty names, every value in range; unknown fields are errors. `LightRigLibrary::default_rigs` (`default_light_rigs.ron`) is compiled in until packs carry it: `gameplay` and `portrait`. The library is an ECS resource; a character is an entity with a `CharacterLightRig` (a `LightRigId` and its head's center in its own space).
  - **Resolving** (`LightRig::resolve`, `TodFrame::character_lighting`): the key's color and brightness are time of day's `character` group times the preset's tint and intensity, so they follow its curves through the day (at night the default day's character key is brighter than the moon). Its direction starts from the world's key light; a preset with `camera_follow` turns it that far along the great circle toward its fixed place around the camera; then its azimuth and elevation are limited. The azimuth is measured around the vertical axis through the character's head from the camera's side (the horizontal direction from the head to the camera; for a camera straight above or below the head, its own axes), so the limit holds for a character at the edge of the screen as for one in its middle. Elevation is clamped to its band. An azimuth past `max_azimuth` folds back linearly, from the limit to 0° directly behind the character: a key that passes behind the character turns smoothly through the camera's side, where a clamp would jump from one side of the face to the other. A key that follows the world and stays within the limits is returned as it is, bit for bit. The result is continuous except for a key passing exactly through the zenith, whose azimuth is undefined.
  - **Per-shot overrides:** a `TodCutscene` may carry a `light_rig` (a document field; a cutscene with only a rig is valid). While the track plays, `TodFrame::shot_rig` holds it with the track's eased weight, and every character's lighting is its own preset's blended toward the shot's by that weight: the key's direction along the great circle, colors in OkLab, scalars linearly, and the value ceiling through its reciprocal so no ceiling blends smoothly into one. So a shot's rig eases in and out with its time-of-day values.
  - **Sandbox:** `--character-rig <preset>` picks the character's preset (`gameplay` by default); an unknown name fails with the presets' names. The mannequin's head is its head sphere's center, a constant until skinned characters take it from their head bone.
  - **Tests:** presets round-trip as documents and leave out what is neutral; validation names the field; the neutral rig is the world's key light exactly, and so is the gameplay preset at 10:00; the limits raise a grazing key and lower an overhead one, fold a key from behind back toward the camera on its own side (120°, 150°, −165° and 180° to 120°, 60°, −30° and 0°), turn it by under 1° per 0.4° step as it sweeps behind the character, follow an orbiting camera, and judge a character at the edge of the screen from its head; the camera's side above and below the head; a camera-fixed key ignores the sun from three cameras, and half-following turns it halfway; the key's color and intensity follow the character curve; a shot rig blends in and out along the great circle by the track's weight; blending is exact at both ends; cutscenes check and write their rigs. Ticking, resolving and lighting two characters for 3,600 frames allocates nothing.
  - **Cost:** about 45 ns a character (gameplay) and 85 ns (portrait, which turns along a great circle) on an M5, release, beside time of day's 0.1–0.55 µs resolve.
- **As built (M1 W2, the sky's layers):**
  - Keys gain three optional groups: `discs` (the sun's and the moon's disc and glow: color, HDR intensity, glow color and size in degrees), `stars` (brightness, 0 by day and 1 for a full night sky) and `clouds` (lit, shade and rim tones, card cloud coverage and cirrus coverage). They blend like the rest: colors in OkLab, amounts linearly. Unlike the other groups, the base timeline may leave them out (`TodGroups::REQUIRED`); a timeline that does draws the gradient alone, as every timeline before W2 did, so existing documents load and look the same. Every key that sets one still needs the same groups as its neighbors.
  - **The celestial sphere:** `celestial_sphere(hours)` turns the stars' sphere once a day about the sky's pole, 35° up over the far horizon (−Z, the default day's north), so stars rise on the left and set on the right as the default day's sun does. The look carries the turn (`TodLook::sphere`) and blends it like any value, so a cutscene that sets its own hour turns the stars there. A per-zone pole (its latitude) waits for zones as data.
  - **The default day** sets the three groups at every key. Discs keep apart from the bodies' light, so a setting sun's disc fades on the horizon after its light and the moon shows pale as it rises at sunset; glows are wide and warm at sunrise and sunset and spread along the horizon there, and the dawn and dusk keys keep one where the sun is below the horizon; stars come out through dusk, fullest at midnight; clouds are painted in two tones with a rim, white tops over lavender-blue shade by day, peach and gold over violet at sunrise and sunset, dark blue with moonlit rims at night, the card clouds' coverage 0.55–0.85 and the cirrus fullest at sunrise and sunset, where it catches the color. For everything W1 drew, its 10:00 key is still exactly the renderer's default look.
  - **Tests:** a timeline without the sky's layers loads, validates and draws the gradient alone; the new groups round trip, left-out glow and cloud fields default, and out-of-range values name their field; disc, glow and cloud colors blend in OkLab like the rest, glow sizes authored in degrees arrive in radians; a stormy zone overrides cover and cloud tones while keeping the base's discs and stars, and a cutscene brings the stars out by day; the stars turn 15° an hour about the pole, once a day, rising on the left and setting on the right; and the packet carries the hour's turn, with stars and a moon in the default day's night.

### 5.22 `sapphire_gameplay` (L4)

**Character controller**

- A kinematic capsule with grounded, air, slope, step-up and ledge handling.
- Root-motion integration.
- A combat movement mode that disables step-up jitter during attacks.

**Combat core** (all inside the fixed step, in this order)

1. **Intents:** a ring buffer 8–15 ticks deep, with expiry and priority per intent.
2. **Cancels:** the move graph checks cancel windows. Buffered inputs fire when a window opens; instant inputs only fire if pressed inside it.
3. **Advance moves:** each move advances on its local clock. Its frame-data timeline has tracks for startup, active and recovery, hitboxes, hurtboxes and invulnerability, perfect-dodge and parry windows, cancel windows, root motion and warp, and cues.
4. **Movement:** apply root motion and the controller.
5. **Hits:** run `HitQuery` and queue `HitEvent`s.
6. **Resolve:** apply attributes, effects and tags. This step handles damage, poise and stagger (in the spirit of ZZZ's Daze meter) and hit reactions (stagger, launch, knockdown).
7. **Hitstop and time scale.**
8. **Cues:** VFX, SFX, camera shake, rumble and post effects. Cues are presentation-only, so the simulation runs headless.

**Ability system (GAS-like)**

- **Attributes:** HP, ATK, energy, and so on.
- **Effects:** instant, duration or periodic, with stacking rules.
- **Tags:** hierarchical tags compiled to bitsets.
- **Abilities:** activation conditions, costs, and cooldowns on the owner clock.

**Party**

- Three slots. Swapping is an ability, and swap-in assists and chain attacks are moves.
- Off-field characters keep ticking for cooldowns and energy.
- The party's resources are pinned in `sapphire_assets`.

**Camera director**

- A rig stack:
  - Explore follow.
  - Combat framing (player plus target).
  - Lock-on.
  - Sequencer takeovers for ultimate-skill cinematics.
- Collision by sphere cast, smoothing by critically damped springs.
- The camera runs on its own clock. Shakes are cues, scaled by the player's shake setting.

**Targeting:** soft lock weighted by input direction, plus auto-target on touch.

**Enemy AI**

- An enemy's "brain" outputs intents into the *same* move system the player uses, so bosses are authored in the same timeline editor.
- `statig` hierarchical state machines handle behavior states; utility scores pick attacks.
- An encounter director hands out attack tokens and manages waves. Boss phases are data plus Luau hooks.

**Tests**

- The headless `sapphire_testkit` world replays recorded input files and asserts outcomes: damage dealt, final positions, a checksum of the simulation state.
- These replays run in CI on all three desktop OSes and are the combat regression suite.

### 5.23 `sapphire_sequencer` (L4)

- **Timeline asset tracks:**
  - Camera.
  - Character animation and transforms.
  - VFX and audio events.
  - Post-process parameters.
  - Subtitles and dialogue.
  - Time scale.
  - Gameplay events.
- **Used for** cutscenes and in-combat ultimate cinematics. Each timeline chooses its clock, blends in and out with the gameplay camera, and can be skipped.
- **Dialogue graphs:**
  - Each line has a localization key, a VO event and a speaker. Graphs support choices and conditions (Luau expressions) and set quest flags.
  - They feed ZZZ-style dialogue panels in `game_ui`.
- **Editor:** scrubbable, with a live preview (§5.28).

### 5.24 `sapphire_script` (L4)

- **VM:** Luau via `mlua` 0.12 with the `luau` feature and **no JIT** on any platform (iOS forbids it, and determinism benefits). One sandboxed VM per world, with read-only globals.
- **Calls are event hooks only:** encounter start, boss phase change, move start and hit, dialogue conditions and quest steps. Scripts never run every tick.
- **Bindings** are generated from the `sapphire_reflect` registry, plus a `.d.luau` type-definition file for editor autocompletion.
- **GC:** incremental, stepped once per frame with a time budget (0.2 ms desktop, 0.4 ms mobile) at a fixed point outside the simulation hot loop.
- **Determinism:** the `os`, `time` and `random` globals are removed. The engine provides seeded RNG and clock functions.
- **Hot reload:** reload the chunk and rebind its hooks. On devices, pushed through live link.
- **iOS policy:** scripts ship inside the app. Any downloadable script update needs legal and App Review sign-off first (App Store guideline 2.5.2).

### 5.25 `sapphire_save` and `sapphire_loc` (L4)

**`sapphire_save`**

- **Settings:** RON in `user://`.
- **Progress:** `postcard` with a versioned header and migrations.
  - Writes are atomic: write a temp file, fsync, then rename.
  - Two rotating slots, each with a checksum.
- **Cloud saves:** *later*, through your own backend, Steam Cloud, iCloud or Play Games.

**`sapphire_loc`**

- Fluent `.ftl` bundles per locale, cooked into binary tables.
- ICU4X (`icu` 2.4) for plurals, numbers and dates.
- **Font fallback chains** per script: Latin, CJK, Cyrillic, Thai, and so on.
- A **pseudo-localization mode** with expanded, accented strings for layout testing.

### 5.26 `sapphire_patch` / `sapphire_online` (L4, later)

**Patching**

- Content-addressed chunk manifests, signed with `ed25519-dalek`.
- **Download:** CDN downloads with resume and blake3 verification.
- **Swap:** atomically switch the manifest when a patch completes. Only changed chunks are downloaded.
- **HTTP:** `reqwest` + `rustls` + `rustls-platform-verifier`, so certificates are checked with the OS trust store on iOS and Android.
- **iOS background downloads** need a native `URLSession` shim.

**Online services**

- Accounts, gacha and inventory are **server-authoritative**. The client only displays results.
- Telemetry is opt-in.

### 5.27 `sapphire_pipeline` (Tools)

**How**

- **Pipeline:** a library plus a `cook` CLI plus an optional daemon used by the editor. It runs as a separate process, so a crash in a third-party encoder can't take down the editor.
- **Stages:** importer (source → engine-neutral intermediate) → cooker (intermediate → platform blob) → packer.
- **Cache key:** blake3 of the source bytes, the importer and cooker versions, the settings, the target cook (`desktop` / `mobile`) and the dependency keys. The local cache comes first; a shared cache can be added *later*.
- **Identity:** every source asset has a `.meta` sidecar holding its GUID and import settings, and references always go by GUID.
- **Parallelism:** `rayon` is fine here.
- **As built (M1 A1, `sapphire_pipeline::{meta, cache, cook}`):**
  - A sidecar is `<source file name>.meta` (`content/samples/mannequin.glb.meta`): a `sapphire::assets::AssetMeta` RON document holding the `id` and the `importer`, an enum whose variant names the importer and holds its typed settings (`Gltf((outline: (weld_tolerance: 0.0001)))`). Left-out settings take their defaults.
  - A cook creates missing sidecars and never rewrites an existing one. `--locked` (xtask's, so CI) fails instead, because an ID made there would be thrown away. Two sidecars with one ID are an error, and so is a source without a sidecar next to a sidecar without a source: a move that left the sidecar behind would otherwise lose the ID.
  - The cache key is blake3 (derive-key mode) over postcard-encoded inputs: the source's hash; the recipe (importer and cooker names and versions, and the cooked format's version); the import settings; the cook; and the dependencies as a set, files the importer reads (glTF external buffers) by relative path and hash and assets by ID and key. A new kind of asset adds an importer variant and a recipe without changing the key.
  - The key leaves out the source's path, its ID and the pipeline binary, so moves, copies and pipeline rebuilds are hits. The price: a stage whose output changes for the same input must bump its version. A cooked format's version is in the key, so a format bump recooks on its own.
  - The local cache is `target/content/cache/`, one file per key with a header (magic, format, length, blake3 of the output), so damaged entries are misses. Outputs are written only when their bytes change, so a file watcher sees only real changes, and stale cooked files are pruned. This replaces xtask's tree stamp, which recooked everything on any change.
  - Timings on the M5: `cargo xtask content` takes 0.26–0.27 s cold, warm, or after touching or changing the one sample mesh. Nearly all of that is cargo checking that xtask and the pipeline are built; the pipeline itself takes 11 ms cold and 8 ms warm. On 200 distinct meshes (27 MB of sources) it takes 0.46 s cold, 0.06 s warm or with one source changed (1 cooked, 199 up to date), and 0.10 s to restore every output from the cache.
  - Cooked outputs keep mirroring their source paths; addressing them by ID waits for packs (A4, see §5.10).
  - Later: a shared cache behind the same key, pruning old entries (deleting the directory is always safe), a stat cache so unchanged sources aren't rehashed, and parallel cooking.
- **As built (M1 A2, `sapphire_pipeline::{import::image, texture}`):**
  - PNG, TGA and EXR import through `image` 0.25, built with only those decoders, into linear `f32` RGBA that remembers the source's channel layout. Integer samples are decoded as the texture class says (sRGB for color, ramp and HDR classes, linear for data maps), float samples are always linear, and non-finite samples are errors.
  - The class is a typed import setting in the sidecar (`importer: Texture((class: Normal((flip_y: true))))`). New PNG and TGA sidecars default to color, EXR to HDR. The formats per class and cook are the table below: masks pick BC4, BC5 or BC7 by channel count, SDF maps are 16-bit on desktop by default and 8-bit on mobile, and mobile HDR is RGB9E5 by default (RGBA16F keeps alpha). ASTC blocks are 4×4 by default, configurable per color, normal and mask texture, and fixed at 4×4 for ILM maps.
  - The pipeline generates mip chains itself. Each level is the area average of level 0 over its footprint, a box filter of the exact scale factor, so odd sizes stay aligned and nothing rings or haloes. Averaging is in linear light: color weights by alpha, normals are averaged as vectors with Z rebuilt and renormalized, and everything else is averaged per channel.
  - Each level then goes through its own `ctt::convert` call with a named encoder (bc7enc-rdo for BC7, Intel's prebuilt ISPC kernels for BC4, BC5 and BC6H, astcenc for ASTC), so ctt's broken BC7 mip chains (its issue #114) never come into play. ctt is told the levels are premultiplied on both sides: its float path otherwise premultiplies straight alpha and divides it back out, which destroys data packed into alpha.
  - Textures plug in as a recipe (importer `image` 1, cooker `ktx2 (ktx2 0.5.0, ctt 0.6.0)` 1, format `TEXTURE_VERSION`) without changing the cache key. Two cooks of one source are byte-identical (serial encoders, no timestamps). A test ties the cooker's name to the versions of every `ctt*` crate and of `ktx2` in `Cargo.lock`, so upgrading an encoder (each is its own crate on a caret range) or the descriptor generator the reader checks against recooks every texture.
  - Timings on the M5 (release, encoders serial): a 2048² color texture cooks in 1.2 s to BC7 and 0.5 s to ASTC 4×4, and a 1024² normal map to BC5 in about 30 ms.
  - A GPU test cooks every class for both cooks, uploads the KTX2 and samples every level back on Metal, down to the 1×1 levels, within each format's tolerance.
- **As built (M1 A5, `sapphire_pipeline::{import::gltf, bake, optimize, mesh}`):**
  - The glTF importer reads **skins**: every joint of every skin in the scene plus their ancestors (so a non-joint armature node still transforms its joints), depth-first, and a palette entry per distinct joint and inverse bind matrix. Skinned primitives stay in bind space; static parts of a skinned scene (props, bone-parented weapons) bind rigidly to their nearest ancestor joint, or joint 0. It reads every `JOINTS_n`/`WEIGHTS_n` set, merges repeats of a joint and keeps the four heaviest, renormalized; further influences are dropped and their weight goes to the four in proportion. A vertex with no positive weight binds wholly to its skin's first joint.
  - **Morph targets** are named by `extras.targetNames` (or `<mesh>/<index>`), merge by name across nodes and meshes (a face, lashes and brows exported apart share one `blink`), and keep only vertices that move by over 1 µm or turn by over 1e-4. Normal deltas are the difference of transformed, normalized normals, exact at full weight.
  - An authored **`_OUTLINE_NORMAL`** attribute (VEC3 float, or normalized byte or short, in `NORMAL`'s space) wins over the welded bake vertex by vertex.
  - **`meshopt` 0.7** (meshoptimizer 1.3): submeshes merge by material; each LOD level simplifies the last kept one, per submesh, with normals weighted and material-seam vertices locked so submeshes stay sealed; a level that saves under 10%, or keeps no triangles, is dropped, and a part that simplifies away still counts toward its level's error; each LOD is reordered for the vertex cache, then overdraw (threshold 1.05); vertices are reordered coarsest LOD first, so every LOD reads a prefix. The crate's vertex-fetch remap wrapper truncates its table when vertices go unused (as when the mobile cook drops LOD0), so that remap is done in Rust.
  - Sidecar settings: `lods: (levels: [(triangle_ratio, max_error), …], mobile_skip)`, by default ½, ¼ and ⅛ of the triangles within 0.2%, 1% and 3% of the extent, with the mobile cook dropping the first level; `levels: []` turns LODs off. `position_format: Unorm16 | Float32`. All are part of the cache key; importer, cooker and format are at version 2.
  - The mannequin's chain: 6,188 / 3,130 / 1,603 / 837 triangles, with error estimates of 0 / 2.8 / 8.1 / 20.9 mm; 3,675 vertices desktop, 2,080 mobile. A cold cook of the sample content still takes about 10 ms. Cooks are deterministic: the same source writes the same bytes.
- **As built (M1 A7, `sapphire_pipeline::character_maps`):**
  - The anime character maps are generated from RON documents, each a source with a sidecar like any other, known by a compound extension: `<name>.facesdf.ron` (threshold masks per light angle), `<name>.ramp.ron` (color stops per material ID) and `<name>.ilm.ron` (a mask per ILM channel). The sidecar's importer is `CharacterMap(…)` with the cook settings (face SDF: mips and desktop precision; ramp: width 256, row height 4, no mips; ILM: mips). What the texels mean is `sapphire_assets::character_maps` (§5.12).
  - They plug in without changing the cache key: each kind's importer stage reads the document and its input images and generates the map as an image, and A2's texture cooker cooks that image as its class (SDF R16/RG16 on desktop and R8/RG8 on mobile, ramps RGBA8 sRGB, ILM BC7 or ASTC 4×4). The input images are file dependencies, by the path the document names them with, so editing a mask recooks exactly the maps that read it and moving a document with its inputs is a cache hit. Stages that decode masks carry the image importer's version in their low 16 bits.
  - Input images are part of their document's asset, as a glTF's buffers are: the cook leaves every image a document names out of its sources, so masks get no sidecar and no texture of their own. An input inside the content tree must name a scanned file exactly, case included, so every file system claims the same files, and must not have a sidecar of its own, which would make it an asset that claiming would silently drop; both are errors. This is the one change to the cook loop beyond registering the recipes.
  - **Face SDF generator:** an exact Euclidean distance transform (Felzenszwalb and Huttenlocher, linear time, separable) of each mask; distances to an edge are measured from texel centers less half a texel. A texel lit in mask *i* and shadowed in mask *i + 1* turns at `aᵢ + (aᵢ₊₁ − aᵢ) · d₀ / (d₀ + d₁)`, with *d₀* its distance to mask *i*'s edge and *d₁* to mask *i + 1*'s lit region; texels shadowed in the first mask hold 0 and texels lit in every mask hold 1. Masks may sit at any increasing angles, two or more per channel. Thresholded at a mask's angle, the map reproduces it exactly (a moving edge, nested circles and a cooked R16 tree in the tests), except that nothing reads as lit with the light at exactly 180°; halfway between masks, the edge lies halfway, within a texel. A band without an edge on one side (after a mask with no shadow, or before one with nothing lit) turns at once, at the later mask's angle. Each consecutive pair is checked for texels lit again; the cook fails naming both masks, the count and the bounding box, unless the document allows that many per pair (`allowed_violations`), and allowed ones follow the first mask that shadowed them. Masks are binarized at 50%, so their resolution sets the edge's precision. Generating a two-channel 1024² map from 26 masks takes 1.6–2.7 s on the M5 (release, with other builds running), mostly its 48 distance transforms.
  - **Ramps:** stops at positions 0 to 1 in sRGB hex, blended in OkLab by default, or `Linear`, `Srgb` or `Step`, chosen per document, ramp or stop. Each material has a day ramp and an optional night ramp (the day ramp when left out); IDs past the listed materials repeat the last, so every row the shader can read holds a ramp. Texels sample the ramp at their centers.
  - **ILM packer:** each channel is one channel of an image mapped onto a `range` (so an AO bake becomes a shadow bias with `range: (0.0, 0.5)`), or a constant; channels left out take their neutral values. Material IDs come from a constant or from painted gray levels snapped to the nearest stored level (or to listed levels, for Genshin-style 0 / 0.3 / 0.5 / 0.7 / 1), so they are flat regions compression keeps exact. One image may feed several channels.
  - `sapphire_pipeline bake-map <document> <png>` writes a document's map before compression, for checking a map without cooking the tree.
  - **Blender recipe** ([blender-mask-bake.md](blender-mask-bake.md), MIT script `tools/blender/bake_character_masks.py`): bakes object-space normals of a face (custom normals and modifiers included, armatures at rest) and turns them into threshold masks per angle that are monotonic by construction, plus AO and material-ID masks per body mesh, and writes the documents. It works on copies with temporary materials and never saves the `.blend`. Tested headless on Blender 5.2 LTS: 26 masks of 1024² in 3.5 s, Blender's start included.
  - The look-dev character's maps come with its import (A6): the tools are ready, and no character sources are in the repository yet.
- **As built (M1 A3, `sapphire_pipeline::{material, cook, import::gltf}`):**
  - Material documents are sources named `<name>.material.ron`, recognized by that suffix so other RON sources keep `.ron` free; their sidecars say `importer: Material(())`. They cook to `<name>.smat` beside the source's other outputs (recipe: importer `material` 1, cooker `smat` 1, format `Material::VERSION`).
  - A material reads the cooked textures it references, so `cook_dir` cooks every other asset first. Before the cache is consulted, on every cook, each referenced texture must be an asset of the content tree, cooked as a texture of a class its slot accepts (color for base color, ILM for the control map, ramp for ramps, SDF for face shadows, color or HDR for matcaps, emission and stars, masks or ILM for masks; A7's character maps count as the class they cook to), and each becomes a `Dependency::Asset` of the material's key by the texture's key. Editing, re-classing or deleting a texture therefore recooks or fails the materials that use it, cached or not. The cook records each texture's swizzle from its cooked KTX2 (so the mobile cook's two-channel masks record `ra01`), checks that a ramp is a whole number of A7's 16 rows tall (a day and a night row per material ID), packs the block and reads the bytes back through `Material::read`.
  - The glTF importer (version 4) turns each glTF material into a material slot with a generated anime character material: `baseColorFactor`, `alphaMode` and `alphaCutoff`, and the Blender extras `shade_color`, `outline_color` and `character_shading` (light-split threshold, softness, rim strength, outline width). Slot names are the material names, `#<index>` for unnamed materials and `#default` for glTF's default material, made unique by appending `#<index>` (again, if a literal name matches a made-up one). A cutoff above 1, which glTF allows, is clamped. Generated materials reference no textures, and packing one that does is an error. A mesh sidecar binds slots to material documents by name (`materials: {"Hair": "<id>"}`); every binding must name a slot of the source and a material document of the same tree, checked on every cook.
  - The cooked mannequin grows from 230,092 to 235,168 bytes (desktop): its five embedded materials, about 1 KB each. The cache key's encoding of mesh settings gained the bindings, so every existing key changes once.
- **As built (M1 A6, `sapphire_pipeline::{import::vrm, extract}`, sub-assets in `cook`):**
  - **VRM import.** `.vrm` sources, VRM 1.0 or 0.x, have sidecars saying `importer: Vrm((mesh: (…), version: Some(V1)))`: glTF's mesh settings (bindings included) and a note of the file's VRM version, read from the file when the sidecar is created. The note is checked, so a 0.x file re-exported as 1.0 fails until its sidecar says so: its materials and rig come out differently, and `V0` tells a reader the file is migrated. The extensions are read through pixiv's `vrm-spec` 0.1.1 (Apache-2.0; `VRMC_vrm`, `VRMC_materials_mtoon`, `VRMC_springBone` and VRM 0.x), and `VRMC_node_constraint`, which it has no types for, through the pipeline's own. Quirks: the crate doesn't build without its `gltf_index` feature, it turns on gltf's default `import` feature (base64, urlencoding, PNG and JPEG decoding; tool-only), and it types two indices as the wrong kind, which the importer reads as what they are.
  - The **mesh** goes through A5's glTF importer, with every node the VRM's data names (humanoid bones, spring joints and centers, colliders, constraint nodes) made a joint of the skeleton with its ancestors, skinned or not. MToon replaces the importer's generated materials; vertex colors are ignored, as MToon ignores them, and the outline-width texture (G; R in 0.x) is sampled bilinearly at each vertex's UV, with the texture's wrap modes, into the outline-width scale the vertex alpha carries. VRM's `meta` (licensing stays in `SOURCE.md`, M1 plan §3), first-person mesh annotations, `VRMC_vrm_animation` and `VRMC_springBone_extended_collider` are dropped.
  - **MToon → anime character materials** (the full table, field by field, is `import::vrm::mtoon`'s): band mode with two tones; the light split's threshold is `−shadingShift` and its softness `1 − toony`, exactly MToon's `linearstep`; region 0's shade color is `shadeColor / baseColor`, exact when the shade texture is the base color's image, as exporters write it; the shading shift texture becomes the control map's shadow bias (until it is extracted, its mean offset, measured on import, joins the threshold, so a 0.x grade texture's folded-in `−rate` doesn't push a generated material into shadow); the parametric rim's strength is its brightest channel and its width the N·V where MToon's falloff is at half (`1 + lift − 0.5^(1/power)`); the matcap keeps `1 − rimLightingMix` of its strength in shadow; outline widths in screen fractions are `× 1080` pixels, and world widths are converted at 2 m through 45°, since Sapphire's outlines are screen-constant. Unlit materials map as MToon with the shade side equal to the lit one. Dropped for want of fields in the family (C1–C7's to add): the rim multiply texture, emission, UV animation, GI equalization, outline lighting mix, render queue and Z-write controls, `doubleSided`, normal scale, `KHR_texture_transform` and `texCoord`; the face, hair or eye surface is the artist's to set on the extracted document.
  - **VRM 0.x migration** (as UniVRM's and three-vrm's, both MIT): the model turns half a turn about Y to face +Z, node by node, with joints' rest transforms and inverse bind matrices conjugated, and 0.x's Unity-space vectors (collider offsets, gravity, the first-person offset) get Z negated first. MToon's Unity properties are decoded (sRGB colors to linear; `toony = lerp(_ShadeToony, 1, ½ + ½·_ShadeShift)`, `shift = −_ShadeShift − (1 − toony)`; the shading grade texture becomes a shading shift texture with scale `_ShadingGradeRate`, linearized at the split; outline widths in centimeters or half-screen percent). Blend shape groups become expressions (`joy` → `happy`, `sorrow` → `sad`, `fun` → `relaxed`, `a`/`i`/`u`/`e`/`o` → `aa`/`ih`/`ou`/`ee`/`oh`, weights ÷ 100, `_MainTex_ST` to a UV transform with V flipped); the thumbs' proximal and intermediate become metacarpal and proximal; each secondary-animation root becomes one spring per branch (first children to a leaf, other children starting springs of their own) ending in a 7 cm virtual tail, which is how UniVRM 0.x swings them.
  - **Sub-assets.** A source may cook to more than one asset (`Recipe::sub_assets`): a VRM cooks to its mesh (`alicia.smesh`, the sidecar's ID) and its rig (`alicia.srig`, recipe `vrm`/`srig`, ID `meta::sub_asset_id(id, "rig")`: blake3 of both laid out as a version 8 UUID, so it needs no storage and never meets a random version 4 ID). Each is planned, keyed, cached and reported as an asset of its own, so the packer (A4) sees both. Each imports the file itself; the rig's import skips the materials and the vertex work. The VRM importer's version carries the glTF importer's in its low 16 bits.
  - **Decision: `extract-materials` is an explicit command**, never part of a cook, as A3 decided: cooks write no sources and rewrite no sidecars, so keys hold and `--locked` CI cooks change nothing, and extracted files are the artist's from then on. `sapphire_pipeline extract-materials <source> [--all]` writes, for `characters/alicia.vrm`, into `characters/alicia/`: the images its MToon materials use as `textures/<image>.png` with sidecars choosing the class by use (color for base, shade, matcap and emission; normal; masks for rim, outline-width and UV-animation textures; one file per class when an image has two), each shading shift texture as `maps/<image>.png`, without a sidecar, as the input of `maps/<image>.ilm.ron`, an ILM document putting the texture in G over `[0.5, 0.5 + scale/2]` with the other channels neutral, and each slot's document as `materials/<slot>.material.ron`, binding them; then it binds the slots in the source's sidecar. By default it extracts the slots whose documents bind a texture (the ones a generated material can't draw as authored); `--all` extracts every slot, glTF sources' too. PNG images are copied byte for byte, JPEG ones decoded and written as PNG (`image` gained its `jpeg` decoder). File names are decided over every slot and image of the source, whatever one run extracts, so a name always stands for the same slot, texture or map; two mesh sources sharing a stem in one directory can't both extract. It never overwrites a file, keeps every existing ID, and leaves slots the sidecar already binds alone; a slot whose document exists but isn't bound is bound to it again. Images that the family can't bind yet are still extracted, so nothing authored is lost.
  - Test VRMs are built in code (`import::vrm::samples`): one small character as VRM 1.0 and as UniVRM 0.x exported it (facing −Z, its data in the `VRM` extension), with textured, untextured and unlit materials, PNG and JPEG images, merged expressions, a branching hair chain, colliders and a twist constraint. The migrated 0.x import matches the 1.0 one vertex for vertex. No third-party VRM is in the repository or downloaded.
- **As built (M1 A4, `sapphire_pipeline::pack`):**
  - The packer is the stage after cooking, in the same process: `sapphire_pipeline cook --packs <dir>` packs the cook's assets, which the `CookReport` gives by ID, cooked file and cache key. `cargo xtask content` (and the mobile cook, for iOS and Android bundles) writes loose cooked files to `target/content/loose/<cook>/`, which tools read and hot reload (A8) will watch, and packs to `target/content/<cook>/`, the content root apps bundle and desktop runs read, which now holds only packs: stale packs, temporary files and the loose files shipped before packs are removed from it.
  - Each asset goes in exactly as cooked, its kind from its cooked format's extension and its dependencies the references the runtime's own check reads from its bytes; a dependency no pack holds fails the pack. Packs are deterministic: assets sorted by ID, serial encoders, no timestamps.
  - **Grouping rule** (spec §5.10's per zone, per character, per encounter, plus shared): zones, characters and encounters will be root documents (M2, M3); each root's pack holds what it reaches through dependencies, and an asset two roots reach goes in the shared pack, which apps mount first, so a group pack depends only on itself and the shared pack. Until roots exist, everything goes in `shared.spack`.
  - **Codec policy:** zstd at level 19 for textures and meshes, the bulk of a pack, loaded at zone and character transitions where install and download size matter most (the mannequin packs at 0.37 of its size; BC7 and ASTC textures at 0.66–0.69, against lz4's 0.86–0.88); lz4 for materials, small and decoded mid-play when hot reload and live link swap one, and for rigs, loaded with a character swapping in. Sub-assets (a VRM's rig, §5.27's A6 note) are packed and addressed by their derived IDs like any other asset. Level 19 encodes at 13–20 MB/s a core, which the cache absorbs; decoding speed barely depends on the level.
  - **Caching:** each asset's encoding is a content-cache entry keyed by the cooked bytes' hash, the packer's stage and the codec and level, so an encoding can't outlive the bytes it was made from and identical cooks share one; a cooked file edited after its cook is refused, and a pack directory that holds cooked files is refused rather than pruned; each pack records its file's hash under a key over its assets' IDs and encoding keys, so an unchanged pack is checked by hashing the file, without reading any encoding. A changed asset re-encodes alone, and an asset that recooks to the same bytes (a material whose texture changed but not its swizzle) reuses its encoding, and a pack deleted or edited by hand is rebuilt from cached encodings. The packer's stage name carries its encoders' versions (`spack (zstd 1.5.7, lz4_flex 0.14.0)`), which a test ties to `Cargo.lock`.
  - Timings on the M5, under load from parallel builds: `cargo xtask content` takes 0.45–0.68 s cold or warm, nearly all of it cargo checking that xtask and the pipeline are built. The pipeline itself takes 40 ms cold (19 ms without packing: zstd 19 on the 235 KB mannequin is the difference) and 8.3 ms warm (7.8 ms without packing); rebuilding a deleted pack from cached encodings takes about as long as a warm run. The desktop pack is 86,397 bytes, the mobile one 45,111.
- **As built (M1 C3, `sapphire_pipeline::{texture, samples::face}`):**
  - A one-channel SDF map cooks with the swizzle `r101` (`character_maps::face_sdf::ONE_CHANNEL_SWIZZLE`), so it reads G as 1 and the face shader treats it as a two-channel map whose vertical channel never shadows. The texture cooker's version is 2, so every texture recooks once; only one-channel SDF maps change.
  - `samples::face` writes the face test head's content into `content/samples/face/` (§5.12): the head as a glTF whose sidecar binds its face slot to a face material document, the material, a base color, the face mask, the expression map, and a face SDF document with its 26 threshold masks, all generated in code and committed. The desktop pack grows from C2's 92 KB to 305 KB.

| Asset | Source | Importer | `desktop` cook | `mobile` cook |
|---|---|---|---|---|
| Meshes (static and skinned) | Blender → glTF 2.0; FBX via `ufbx` | `gltf` 1.4, `ufbx` 0.11 | Quantized vertex streams; `meshopt` 0.7 vertex-cache / overdraw / fetch optimization; LODs; outline normals, either authored (an `_OUTLINE_NORMAL` vertex attribute, from M1) or baked by welding (M0: angle-weighted, sliver- and cancellation-safe, 0.1 mm weld) | Same, with a lower-detail LOD0 and fewer LODs |
| Characters | VRM 1.0 / 0.x (`.vrm`) | `gltf` 1.4, `vrm-spec` 0.1 | A mesh as above with MToon mapped onto anime character materials, plus an rkyv `.srig` rig (humanoid, expressions, look-at, spring bones, constraints) | Same |
| Color textures | PNG / TGA / EXR | `image`, `exr` | KTX2 (`ktx2` 0.5) BC7 via `ctt` 0.6 | KTX2 ASTC 4×4–8×8 per texture class, via `ctt` |
| Normal maps / masks / HDR | same | same | BC5 / BC4, BC5 or BC7 by channel count / BC6H | ASTC; HDR as RGBA16F or RGB9E5 |
| Ramps, ILM maps, SDF face maps | RON documents: color stops, and masks painted or baked in Blender | serde, `image` | Uncompressed RGBA8 (ramps), BC7 (ILM), R16/RG16 (SDF) | RGBA8, ASTC 4×4, R8/RG8 |
| Animation | glTF / FBX | `ufbx` / `gltf` | ozz archives plus root-motion track and curves | Same, with stronger compression |
| Materials | `.material.ron` documents, one type per family | serde | rkyv `.smat`: the family's GPU block and texture references | Same, with the mobile textures' swizzles |
| Move graphs, frame data, effects, UI, tiers | Editor documents (RON) | serde | rkyv | rkyv |
| Zones | Blender glTF + custom-property extras, plus editor documents | `gltf` | Cells, instances, probes, navmesh tiles (`rerecast`) | Same, with lower density and fewer probes |
| Shaders | WGSL / WESL | `wesl`, `naga` | Validated WGSL plus permutation inventory | Same, with mobile tier flags |
| Audio | FMOD Studio project | FMOD Studio build | `.bank` (desktop) | `.bank` (mobile codecs) |
| Fonts | TTF / OTF | `skrifa` | Subset font plus MSDF atlas for display text | Same |
| Strings | `.ftl` | `fluent` | Binary tables | Same |

- **Blender as the level and modeling tool:** export glTF with custom-property extras (the pattern Bevy's Skein uses). The in-engine editor then only needs to cover gameplay data, combat tuning, VFX timing and cinematics.

### 5.28 `sapphire_editor` (Tools, desktop only)

**How**

- **Document model first** (after The Machinery's "The Truth"):
  - All editor state is typed objects with GUIDs and reflected properties.
  - Every edit is a change record `(object, property, old, new)`.
  - Undo and redo, copy and paste, dirty tracking and live-link sync all fall out of that one model.
  - Documents save as RON files that are friendly to git.
- **UI:**
  - `egui` 0.37 with `egui_dock` or `egui_tiles` for docking.
  - `transform-gizmo` 0.11 for manipulators and `egui-snarl` 0.12 for graphs (move graphs, animation graphs).
  - `egui_plot` for curves.
- **Platform:** the editor runs on `sapphire_platform` (SDL3) too. The viewport is the real engine renderer drawing into a texture that egui displays, so the editor and the game share one input and windowing stack.
- **Play-in-editor:** creates a fresh runtime world from a snapshot of the current documents. Game code never mutates documents.
- **Combat timeline editor**, the highest-value tool:
  - Tracks for hitboxes, hurtboxes and invulnerability, cancel windows, root motion and warp, and cues.
  - Frame-accurate scrubbing with a preview character and an enemy dummy.
  - "Play from frame N."
  - A hitbox-gizmo editing mode on bones.
- **Other panels:**
  - Asset browser.
  - Inspector.
  - Material editor with live preview of ramps, ILM and outlines.
  - VFX editor.
  - Sequencer.
  - UI preview at the 4:3, 16:9 and 21:9 aspect ratios.
  - Cvar panel.
  - Profiler (Tracy connection).

### 5.29 `sapphire_livelink` (Tools plus a dev-build runtime half)

- **Transport:** TCP, with postcard-framed messages. The device dev build listens; the editor connects over Wi-Fi or USB (`adb reverse` / `iproxy`).
- **Editor → device:** change records, recompiled shaders, cooked assets, cvar sets and replay files.
- **Device → editor:** stats, logs, screenshots and Tracy (connected directly).
- **Why it matters:** combat feel and touch controls have to be tuned on the phone, not on the Mac.
- **As built (M1 A8, what A9 builds on):** the device half applies what it receives through the same calls desktop hot reload uses. A pushed pack goes into a `MemoryPackSource` (or a source that serves pushed packs over the platform's) and `AssetServer::remount` reloads what changed behind the handles, which the app's uploader replaces in place by generation (§5.10); pushed WGSL text goes to `Renderer::set_shader_sources`, which checks it and keeps the old pipelines on an error (§5.12); a pushed document goes to its owner (`TimeOfDay::set_timeline`). Pushing only the changed assets, as a pack mounted over the shared one, is A9's to add.

---

## 6. Cross-platform checklist

These concerns cut across modules. Each line names its owning module.

### 6.1 Graphics (`sapphire_rhi`, `sapphire_render`)

- **One shader source.** WGSL / WESL is translated by naga at runtime into MSL, SPIR-V or HLSL. Never fork shaders per OS. Branch on `GpuCaps` and the tier table instead.
- **Two texture cooks.** `desktop` (BCn) covers Windows, Linux and macOS; `mobile` (ASTC) covers iOS and Android. A device that is missing its expected format fails at boot with a clear message instead of silently decompressing.
- **Android:**
  - Require Vulkan 1.1 and skip GLES entirely.
  - Handle surface destroy/create on every pause.
  - **Verify** how wgpu handles swapchain pre-rotation on Android. If rotation isn't handled, the compositor rotates every frame, which costs performance.
- **Apple:** Metal only. Use Xcode's GPU frame capture and Metal System Trace in Instruments.
- **Windows:** default to Vulkan with automatic DX12 fallback (§2 note 2). Test both backends in CI.

### 6.2 Frame pacing and displays (`sapphire_platform`, `sapphire_time`, `sapphire_render`)

- **Desktop:** VRR-friendly FIFO by default, a user frame cap (30 / 60 / 120 / unlimited), and a 60 Hz sim with interpolation.
- **iOS:** ProMotion devices allow 120 Hz. Default to 60 fps on Mobile-High and 30 on Mobile-Low, and set the preferred frame-rate range so the display doesn't run at 120 Hz for a 60 fps game.
- **Android:**
  - Displays run at 60, 90 or 120 Hz. Use FIFO plus a frame limiter that divides the refresh rate evenly (for example 30 on 120 Hz → present every 4th vsync).
  - Hint the refresh rate with `ANativeWindow_setFrameRate` (API 30+).
  - Android's Swappy frame-pacing library doesn't fit wgpu's swapchain ownership, so don't plan on it.
- **Notches and safe areas:** handled by `sapphire_ui` anchors from platform insets.

### 6.3 Thermal, power and memory (`sapphire_platform`, `sapphire_render`, `sapphire_assets`)

- **Thermal input** comes from iOS `thermalState` and Android thermal headroom plus ADPF performance-hint sessions (report the target and actual work duration each frame).
- **Response ladder:**
  1. Dynamic resolution.
  2. Disable optional passes.
  3. Drop the tier.
  4. Offer a 30 fps lock.
- **iOS has no swap**, and the OS kills apps that exceed device-dependent memory limits. Check `os_proc_available_memory` at boot to pick budgets, and drop caches on `LowMemory`.
- **Android low-memory killer:** keep the background footprint small, and release GPU resources when backgrounded where practical.

### 6.4 Storage and content delivery (`sapphire_assets`, `sapphire_patch`)

- **Install model, following gacha-game practice:** ship a small app (boot pack, UI, the first zone), then download the remaining packs from a CDN in-game, with progress UI and resume.
- **Store size limits:** use Play Asset Delivery and the App Store's on-demand options only if store size limits require it. **Verify** the current limits in M4.
- **iOS:** mark downloaded packs as excluded from iCloud backup.
- **Android:** download into app-internal storage so packs can be memory-mapped.

### 6.5 Lifecycle (`sapphire_platform` → everyone)

On **background**:

- Stop submitting GPU work. This is mandatory on iOS.
- Suspend the FMOD mixer.
- Flush saves.
- Pause the simulation, using the world clock.
- Keep sockets for at most a few seconds.

On **foreground**:

- Recreate the surface.
- Resume audio.
- Show a "tap to continue" overlay if the player was in combat.

### 6.6 Input (`sapphire_input`, `game_ui`)

- **Default devices per platform:**
  - Desktop: KB/M plus gamepad, with glyphs following the last-used device.
  - Steam Deck: gamepad glyphs, and a 1280×800 layout check.
  - Mobile: touch plus optional gamepad. On-screen controls hide when a gamepad connects.
- **Text input:** SDL3 text-input events show the on-screen keyboard on mobile (for name entry and the dev console).
- **As built (M1 F4):** every device is live on every platform, through one set of bindings. `sapphire_input` reports the last-used device kind with the gamepad's glyph family (the Steam Deck counts as Xbox, whose A/B/X/Y positions it shares) and how many gamepads are connected; showing glyphs and hiding on-screen controls are `game_ui`'s (M3).
- **As built (M1 F5):** `sapphire_platform` forwards text input as `Event::Text` (UTF-8 in events of at most 32 bytes, split between characters, so events stay `Copy`), only while a window takes it: `Window::start_text_input` (which shows the on-screen keyboard on mobile; a `TextInputKind` of `Verbatim`, for names, commands and codes, turns off auto-capitalization and autocorrect, `Prose` keeps them), `stop_text_input` and `set_text_input_area` for the input method. The debug console is its first user; IME composition previews are not forwarded yet.

### 6.7 Store and platform compliance

- **iOS:**
  - No JIT.
  - App Store guideline 2.5.2 on downloaded code.
  - A privacy manifest (`PrivacyInfo.xcprivacy`) declaring required-reason APIs.
  - The ATT prompt only if you track users.
- **Android:**
  - 64-bit only.
  - 16 KB page-aligned `.so` files (use NDK r28+, plus a linker flag, §7.3).
  - Bump the target API level every year as Play requires.
- **macOS:** hardened runtime, codesigning, notarization.
- **Steam / Steam Deck:** controller support, legible text at 800p, and the Steam Runtime on Linux.

### 6.8 Accessibility (`sapphire_ui`, `sapphire_input`, `sapphire_render`, `sapphire_gameplay`)

- Subtitles with speaker names and a size setting.
- Full remapping, plus hold-to-toggle options.
- **Screen-shake and flash-intensity sliders.** Anime combat is flash-heavy, and these protect photosensitive players.
- Colorblind-safe attack telegraphs, with shape and icon as well as color.
- UI text scale.
- Adjustable auto-target strength on touch.

---

## 7. Repository layout and build

### 7.1 Layout

```
engine/
├─ Cargo.toml               # virtual manifest: [workspace.dependencies], profiles, lints
├─ rust-toolchain.toml      # pinned stable + all 7 targets
├─ .cargo/config.toml       # per-target linker flags, aliases (xtask)
├─ clippy.toml              # disallowed-methods (rule R2)
├─ deny.toml                # license policy, bans (bincode, archived crates)
├─ crates/                  # en_* engine and tool crates (flat)
├─ game/                    # game_app (bin/cdylib/staticlib), game_combat, game_characters, game_ai, game_ui
├─ platform/
│  ├─ android/              # Gradle project, GameActivity extends SDLActivity, manifest, FMOD jar
│  ├─ ios/                  # Xcode project, Info.plist, privacy manifest, entitlements, launch screen
│  ├─ macos/                # bundle Info.plist, entitlements, icons
│  ├─ windows/              # app manifest (DPI awareness), icon, installer config
│  └─ linux/                # .desktop file, Steam Runtime notes
├─ shaders/                 # WGSL / WESL sources
├─ content/                 # source assets + .meta sidecars (Git LFS)
├─ xtask/                   # build automation, layer checker (layers.toml)
├─ docs/                    # this spec, ADRs (architecture decision records)
└─ .github/workflows/
```

### 7.2 Cargo profiles

```toml
[profile.dev]
opt-level = 1
debug = "line-tables-only"

[profile.dev.package."*"]
opt-level = 3                 # deps fast, your code debuggable

[profile.release]             # internal "profile" builds with Tracy
lto = "thin"
debug = "line-tables-only"

[profile.shipping]
inherits = "release"
lto = "fat"
codegen-units = 1
panic = "abort"
debug = "full"
split-debuginfo = "packed"    # PDB / dSYM / .dwp uploaded to Sentry, then stripped
```

- **Desktop dev builds** add an `sapphire_dylib` crate (`crate-type = ["dylib"]`) that re-exports the engine. `game_app` links it behind a `dynamic` feature, as in Bevy's dynamic-linking mode. This is the single biggest compile-time win.
- **Keep gameplay code in `game_app` during iteration**, because `subsecond` hot-patching only patches the top-level crate.
- **Linkers:** keep Apple's default linker (ld-prime) on macOS. On Linux, use mold or lld. Wild is Linux-only.

### 7.3 Per-target notes

| Target | How `game_app` is built | Packaging |
|---|---|---|
| macOS / Windows / Linux | `cargo build -p <app>` (bin) | `cargo xtask bundle --platform macos` gives an `.app` with `Contents/Resources/content`. Codesigning, notarization and Steam depots come in M4 |
| iOS | `cargo build -p <app> --target aarch64-apple-ios[-sim]` (bin; SDL is statically linked) | `cargo xtask bundle --platform ios-sim` gives an ad-hoc-signed `.app`. Device signing, plus an Xcode project for sentry-cocoa, the privacy manifest and store archives, come in M4 |
| Android | `cargo rustc -p <app> --lib --target aarch64-linux-android --crate-type cdylib`, with the NDK environment set by xtask | `cargo xtask bundle --platform android`: the Gradle project in `platform/android` takes the `.so` in `jniLibs/arm64-v8a`, SDL's Java glue (synced from `sdl3-src`) and cooked content in `assets/content`. `SandboxActivity extends SDLActivity`. FMOD's Java init is added in M2 |

`.cargo/config.toml` fragment for Android 16 KB pages (**Verify** against NDK r28+ defaults):

```toml
[target.aarch64-linux-android]
rustflags = ["-C", "link-arg=-Wl,-z,max-page-size=16384"]
```

### 7.4 `xtask` commands

| Command | Does |
|---|---|
| `cargo xtask check-layers` | Enforces §4.1 and rule R8 from `cargo metadata` |
| `cargo xtask cook --cook desktop\|mobile [--only <guid/zone>]` | Runs `sapphire_pipeline` with the content-hash cache |
| `cargo xtask run --platform desktop\|ios-sim\|ios\|android` | Builds, cooks, deploys and launches, with live link enabled |
| `cargo xtask package <platform> --profile shipping` | Produces store-ready artifacts and uploads symbols |
| `cargo xtask replay <file\|dir>` | Runs headless combat replays |
| `cargo xtask golden [--update]` | Runs golden-image render tests |

---

## 8. CI

| Trigger | Job | Runners |
|---|---|---|
| Every PR | fmt, clippy (with R2 lints), `check-layers`, `cargo-deny`, `cargo-nextest` | Linux |
| Every PR | Build desktop binaries | macOS, Windows, Linux |
| Every PR | Compile-only builds for iOS (device and simulator) and Android | macOS (iOS), Linux (Android) |
| Every PR | Combat replay suite | macOS, Windows, Linux |
| Every PR | Golden images: lavapipe (Vulkan) and WARP (DX12), required; Metal (paravirtual GPU), informational until stable | Linux, Windows, macOS |
| Nightly | Full cook of both `desktop` and `mobile`; package all 5 platforms | Mixed |
| Nightly | Device smoke: install, boot, load a zone, 2 minutes of scripted combat, collect frame times and memory | Self-hosted Mac mini with an iPhone and an Android phone attached |
| Weekly | Thermal soak: 30 minutes of combat on Mobile-Low devices; budget regression report | Same |

**Build caching:** `sccache` 0.18, plus `cargo-hakari` to unify dependency features across the workspace.

---

## 9. Quality gates

A change can't merge if any of these fail:

1. **Layering and license checks:** R1, R8, and `cargo-deny`.
2. **Determinism:** all combat replays reproduce their recorded checksum for the runner's architecture, on every desktop OS.
3. **Golden images:** within `nv-flip` tolerance per backend. Rendered with the sandbox's offscreen `--capture` mode (M0), so no window is needed.
4. **Allocations:** the zero-allocation assertion (R6, simulation thread inside the fixed step) during the replay suite.
5. **Budgets:** no budget in §3 regresses by more than 10% on the nightly device run without a recorded waiver.

---

## 10. Milestones

These windows are rough estimates for a small, senior team. A solo developer should expect them to stretch.

| Milestone | Window | Build | Exit test |
|---|---|---|---|
| **M0 Bring-up** | Months 0–2 | Workspace, layer rules and CI on 3 OSes. `sapphire_platform` (SDL3 main callbacks) and `sapphire_rhi` (Metal / Vulkan / DX12) on all 5 platforms. Skeletons of `sapphire_core`, `sapphire_math`, `sapphire_jobs` and `sapphire_time`. Tracy, `sapphire_crash`, and iOS Xcode / Android Gradle packaging. A glTF character with a basic two-tone character shader and hull outline. | **The same commit** runs the outlined two-tone character at target fps on Mac, Windows, Linux (including Steam Deck), an iPhone and an Android phone. Background → foreground survives on both phones, and a forced crash reaches Sentry from every platform. |
| **M1 Data path** | Months 2–6 | ECS, reflection and serialization; the `sapphire_assets` runtime. `sapphire_pipeline`: glTF / FBX, `ctt` BC and ASTC, meshopt, packs, content-hash cache, hot reload. Renderer v1: frame graph, compute skinning, forward lighting, shadows, the anime character recipe (track C), the world look (time of day, sky, fog, ambient probes, stylized environment shader) and the post stack. VRM import and anime map tools, `sapphire_input`, `sapphire_debug`, live link, golden-image tests. The plan is [m1-plan.md](m1-plan.md). | Editing a ramp, material parameter or time-of-day key shows up on desktop *and* phone without a restart. The look-dev scene meets the tier budgets on all 5 platforms. Golden images pass on 3 backends. |
| **M2 Combat slice** | Months 4–10 | `sapphire_anim` (ozz, inertialization, IK, spring bones), `sapphire_physics`, the combat core, party swap, camera director, one enemy AI, FMOD, VFX v1, touch controls, live link, and the timeline editor v1. | One character against one enemy **feels right** with a gamepad on desktop and with touch on a phone. Replays are deterministic in CI, and Mobile-Low holds 30 fps while thermally throttled. **This is the go/no-go gate.** If tuning still feels slow, take the off-ramp in the report: move gameplay and tools to Godot with gdext, and keep the Rust systems. |
| **M3 World and tools** | Months 8–18 | Zones and transitions, navigation, sequencer and dialogue, Luau, `sapphire_ui` with the HUD and menus, saves, localization, the editor document model with undo and play-in-editor, and the pipeline (PSO) inventory. | A zone with 3 encounters, a boss authored with no Rust changes, one cutscene, and a full HUD and menus in 2 languages, all on 5 platforms. |
| **M4 Ship readiness** | Month 12 onward | PSO warm-up, signed chunk manifests and `sapphire_patch`, quality tiers with dynamic resolution and thermal handling, the symbol pipeline, accessibility, and store compliance. | Zero shader hitches in a scripted playthrough. Patches download only changed chunks. Builds pass TestFlight, Play internal testing and Steam review. |

**M0 status (2026-10-24):**

- **Done and verified on 3 of the 5 platforms.** The same code renders the outlined two-tone mannequin at 60 fps on macOS (Metal), the iOS 26.3 simulator (Metal) and the Android 16 emulator (Vulkan through MoltenVK). It survives background → foreground on Android, and a forced crash leaves a valid minidump on macOS.
- **Since then (2026-10-24):** CI runs on GitHub, and Windows and Linux build and pass their tests there. Tracy is verified: a trace from the sandbox on the headless iOS Simulator records every engine zone and wgpu's. Release builds link on every Apple target (thin LTO used to drop a symbol SDL needs).
- **Deferred by decision (2026-10-24):** crash upload. Nothing needs it yet, so the Sentry backend was removed from `sapphire_crash`, which halves its dependency tree. Minidumps are still captured and kept for the next launch, and the backend comes back from git history once there is a project to send reports to.
- **Still open for M0 exit:**
  - A run on Windows and Linux hardware.
  - A physical iPhone, which needs a signing identity.
  - A physical Android phone.


---

## 11. Open questions to resolve

| # | Question | Resolve by |
|---|---|---|
| Q1 | ~~Does the SDL3 entry flow work from Rust?~~ **Answered in M0:** yes. `SDL_RunApp` runs on the iOS simulator, and an exported `SDL_main` with SDL's JNI symbols links for Android (see Android results in ADR 0002). | M0 spike |
| Q2 | ~~Android 16 KB page alignment~~ **Answered in M0:** NDK r28+ defaults already produce 16 KB-aligned `.so` files; the rustflag in `.cargo/config.toml` is belt and braces. | M0 |
| Q3 | Steam Deck: native Linux build, or the Windows build under Proton? Prefer native. | M0 |
| Q4 | Which Windows default backend, Vulkan or DX12, based on Intel / AMD / NVIDIA driver testing? | M1 |
| Q5 | wgpu on mobile and Apple: on-tile MSAA / transient attachments, timestamp queries inside passes on Apple GPUs, Android pre-rotation, Metal binary archives. **Partly answered in M0:** MSAA and depth are transient attachments, and Apple GPUs report the memory saving; the M5 has no timestamps inside passes. The iOS simulator lacks base vertex / first instance, indirect execution, cube arrays and comparison samplers, so the CPU-driven draw path avoids them (ADR 0002). **M1 plan:** mobile AA is 4× MSAA kept in tile memory through `TRANSIENT_ATTACHMENT`, to be measured on Mali and Adreno; one-pass deferred stays impossible until wgpu gets input attachments (#10148). **M1 R7:** the phone tiers keep 4× MSAA as transient attachments, held there by a GPU test; the measurement on Mali, Adreno and Apple devices moves to M2. | M1 |
| Q6 | ~~Do hosted CI runners expose a GPU for Metal golden images?~~ **Answered (M1 plan):** yes. GitHub's macOS arm64 runners expose Metal as "Apple Paravirtual device", and wgpu runs its image tests there on every PR (shader validation is broken there; 3 vCPU). Goldens run per PR on lavapipe, WARP and `macos-26` Metal. | M1 |
| Q7 | Does ozz-animation-rs cover offline animation building, or is the C++ ozz tooling needed in the cooker? **Partly answered (M1 research):** it has no offline tools, so the cooker needs the C++ ozz tooling. It also needs **nightly Rust** (`portable_simd`), which conflicts with the pinned stable toolchain, so the runtime choice is open too. | Start of M2 |
| Q8 | FMOD license tier and cost; if blocked, the `kira` backend becomes primary. | M2 |
| Q9 | Device floors (A14 / Snapdragon 865 / 6 GB), measured while thermally throttled. | M2 |
| Q10 | iOS policy on downloadable scripts and content: legal and App Review guidance. | Before any live-service plan |
| Q11 | Current App Store and Play size limits, which shape the install-time vs CDN content split. | M4 |
| Q12 | ~~Android crash capture~~ **Answered in M0:** there's no workable monitor process on Android. `sapphire_crash` writes an async-signal-safe signal record and chains to `debuggerd`; symbolicated native crashes come from Sentry's Android NDK SDK in the Gradle project. | M0 |
| Q13 | iOS frame pacing: with SDL main callbacks, SDL's private display link drives `iterate` at up to 120 Hz on ProMotion and ignores `SDL_HINT_MAIN_CALLBACK_RATE`. Choose between an engine-owned `CADisplayLink` and present-interval pacing to hit 60 or 30 fps (spec §6.2). | M1 |
| Q14 | Android pre-rotation: wgpu doesn't pre-rotate, so SurfaceFlinger rotates our landscape buffer on portrait-native phones. Measure the cost on real devices before choosing whether to render pre-rotated. | M2 |

---

## Appendix A. Dependencies

Pin these in `[workspace.dependencies]`. Versions were checked against crates.io on 2026-10-24 during the research, except rows marked **unchecked**, which were added for this spec. Keep all `bevy_*` crates on the same 0.20.x.

| Crate | Version | Used by | Notes |
|---|---|---|---|
| `sdl3-sys` | 0.7 (SDL 3.4.x) | `sapphire_platform` | SDL is zlib-licensed |
| `raw-window-handle` | 0.6 | `sapphire_platform`, `sapphire_rhi` | **unchecked** |
| `crash-handler` / `minidumper` | 0.8 / 0.11 | `sapphire_crash` | No iOS support; use sentry-cocoa there |
| `sentry` | ≥ 0.49.3 | `sapphire_crash` | Native minidump capture. Deferred: out of the tree since 2026-10-24 (M0 status) |
| `mimalloc` | latest | `sapphire_core` | |
| `bumpalo` / `bump-scope` | 3.20 / 2.4 | `sapphire_core` | std `Allocator` trait from Rust 1.100 |
| `slotmap` | 1.1 | `sapphire_core` | |
| `tracing`, `profiling`, `tracy-client`, `puffin` | –, –, 0.18, 0.20 | `sapphire_core` | No default features for `tracing` and `profiling` (no proc macros); `tracy-client` comes through `profiling` |
| `glam` | 0.33 | `sapphire_math` and up | The line `bevy_reflect` 0.20 reflects (`^0.33.2`), so math types reflect directly; moves with Bevy (M1 F1) |
| `libm`, `rand_pcg` | latest | `sapphire_math` | `libm` **unchecked** |
| `bevy_ecs`, `bevy_tasks`, `bevy_reflect` | 0.20 | `sapphire_ecs`, `sapphire_jobs`, `sapphire_reflect` | Pin per milestone |
| `serde`, `ron`, `rkyv`, `postcard` | 1, 0.12, 0.8, 1.1 | `sapphire_serialize` | **Never** `bincode` |
| `zstd`, `lz4_flex`, `memmap2`, `blake3` | 0.13 (libzstd 1.5.7), 0.14, –, 1.8 | `sapphire_assets` | M1 A4: zstd is the C library, not `ruzstd` (§5.10 gives the measurements); `lz4_flex` with its safe block codec only. `memmap2` waits for memory-mapped packs. The packer encodes with the same crates |
| `notify` | 8.2 | `sapphire_hot_reload` (dev builds, desktop), tools | M1 A8: 8.2.1, with a debounce of its own rather than `notify-debouncer-full` (§5.10). Only dev-only crates may bring it in (`xtask check-layers`) |
| `wgpu`, `naga` | 30 | `sapphire_rhi`, `sapphire_render` | |
| `wesl` | 0.6 | `sapphire_render` | `naga_oil` is archived |
| `encase`, `bytemuck` | 0.13, – | `sapphire_render` | `encase` through glam's `encase` feature; not `wgsl_bindgen` (built on archived `naga_oil`, no WESL) |
| `vrm-spec` | 0.1 | `sapphire_pipeline` | pixiv's VRM 0.x/1.0 and MToon types, Apache-2.0 (M1 VRM import) |
| `wgpu-profiler` | 0.28 | `sapphire_rhi` | Not used (M1 F5): the frame graph times its passes itself, see §5.20 |
| `dlss_wgpu` | 6.0 | `sapphire_render` (Windows / Linux) | NVIDIA + Vulkan only |
| `objc2-metal-fx`, `objc2-foundation` | latest | `sapphire_render`, `sapphire_platform` (Apple) | |
| `ozz-animation-rs` | 0.11 | `sapphire_anim` | **Vendor it.** MPL-2.0 (file-level copyleft); one maintainer |
| `rapier3d`, `parry3d` | 0.36, 0.31 | `sapphire_physics` | Jolt via `joltc` as the escape hatch |
| FMOD Studio API | vendor SDK | `sapphire_audio` | Commercial license; own bindgen bindings |
| `kira` | 0.12 | `sapphire_audio` (fallback) | |
| `taffy`, `parley` | 0.15, 0.12 | `sapphire_ui` | `rustybuzz` is archived |
| `rerecast`, `landmass` | 0.4, 0.9 | `sapphire_nav`, `sapphire_pipeline` | Vendor both (small maintainer base) |
| `statig` | 0.4 | `sapphire_gameplay` | |
| `mlua` (feature `luau`) | 0.12 | `sapphire_script` | No `luau-jit` |
| `fluent`, `icu` | 0.17, 2.4 | `sapphire_loc` | |
| `reqwest`, `rustls`, `rustls-platform-verifier`, `ed25519-dalek` | latest | `sapphire_patch` (later) | **unchecked** |
| `jni` | latest | `sapphire_platform` (Android) | **unchecked** |
| `egui`, `egui-wgpu`, `egui_dock` / `egui_tiles`, `egui_plot` | 0.37 line | `sapphire_debug`, `sapphire_editor` | M1 F5: `egui` and `egui-wgpu` 0.37.0 (on wgpu 30); the overlay draws its own frame-time graph, without `egui_plot` |
| `transform-gizmo`, `egui-snarl` | 0.11, 0.12 | `sapphire_editor` | |
| `gltf`, `ufbx` | 1.4, 0.11 | `sapphire_pipeline` | |
| `vrm-spec` | 0.1 | `sapphire_pipeline` | M1 A6. Tools only; pixiv's VRM types, without node constraints, which the pipeline types itself |
| `ctt`, `ktx2`, `meshopt` | 0.6, 0.5, 0.7 | `sapphire_pipeline`; `ktx2` also `sapphire_assets` | `basis-universal` is stale. ctt with only the bc7enc-rdo, ISPC (prebuilt) and astcenc encoders, and no `rayon` |
| `image`, `exr`, `rayon` | 0.25, 1.74, latest | `sapphire_pipeline` | Tools only. `image` with only its PNG, TGA and EXR decoders, and JPEG for VRM images (M1 A6); `exr` comes through it |
| `insta`, `nv-flip`, `criterion` | latest | `sapphire_testkit` | `dssim` is AGPL, so avoid it |
| `cargo-ndk`, `sccache`, `cargo-hakari`, `cargo-nextest`, `cargo-deny` | 4.1, 0.18, –, –, – | Tooling | `bacon` is AGPL (fine as a local tool, not as a dependency) |

**Keep out of the dependency tree:** `kajiya`, `rend3`, `naga_oil`, `generational-arena`, `oxidized_navigation`, `laminar`, `rrise`, `oddio`, `bincode`, `optick`, `rustybuzz`, `bevy_cobweb_ui`, `distill`, `physx-rs`, `specs`, `legion`. Encode this list as bans in `deny.toml`.
