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
                        sapphire_input · sapphire_nav · sapphire_debug (dev builds only)
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
| `sapphire_render` | L3 | Frame graph, anime character and environment renderers, time of day, post, upscaling | `sapphire_rhi`, `wesl`, `naga`, `encase`, `bytemuck` | M1–M3 |
| `sapphire_anim` | L3 | Skeletal runtime, anim graph, IK, spring bones, cloth | `ozz-animation-rs` (vendored) | M2 |
| `sapphire_physics` | L3 | Collision world, queries, character capsules, hit shapes | `rapier3d`, `parry3d` | M2 |
| `sapphire_audio` | L3 | FMOD Studio integration and cue playback | FMOD (own bindings); `kira` as fallback | M2 |
| `sapphire_vfx` | L3 | GPU particles, trails, decals, screen effects | `sapphire_render` | M2–M3 |
| `sapphire_ui` | L3 | Retained game UI, text, touch controls | `taffy`, `parley` | M3 |
| `sapphire_input` | L3 | Action mapping, contexts, rebinding, glyphs | `sapphire_platform`, `sapphire_serialize` | M1 |
| `sapphire_nav` | L3 | Navmesh baking and runtime pathfinding | `rerecast`, `landmass` | M3 |
| `sapphire_debug` | L3 | Debug draw, stats, dev console, inspector (dev only) | `egui`, `egui-wgpu` | M1 |
| `sapphire_world` | L4 | Zones, cells, prefabs, spawning, transitions | — | M3 |
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
  - Three vertex streams, so each pass reads only what it needs: **positions** as xyz unorm16 across the bounds (8 B; error `extent / 131 070`, 16 µm on the 2.1 m mannequin) or, by import setting, f32 (12 B, for meshes too large for 16 bits or whose edges must meet a neighbor's exactly); **shading**, the normal and outline normal octahedral snorm16 × 2 each (8 B, within 0.008°); **surface**, UV unorm16 across the UV bounds plus RGBA8 color (8 B, 0.016 texels of a 2K map). 24 bytes a vertex where M0 read 48. 8-bit normals (about 1°) were rejected because the toon split is a hard threshold on N·L, and float16 UVs because their 1/2048 steps near 1.0 are half a texel of a 2K map.
  - **LODs:** LOD0 first, each LOD per-material index ranges over a prefix of the streams (vertices are ordered coarsest LOD first, so a skinning pass for a distant character skins only its prefix), with the simplifier's error estimate in object-space meters for LOD selection (it folds in the weighted normal error, so it is conservative).
  - **Skins:** the skeleton (joints parents-first, with names and local rest TRS: ozz-animation's shape), a palette of (joint, inverse bind matrix) entries, and per-vertex influences: four palette indices (u8 for palettes up to 256 entries, else u16) and unorm8 weights summing to exactly 255. **Morph targets:** names, default weights and sparse deltas per target (vertex u32, binary16 position and normal deltas: 16 B). These are already the storage layouts compute skinning (R2) reads.
  - The element encoders and decoders live together in `sapphire_assets`, so cooker and runtime can't drift apart; `Mesh::from_vertices` packs procedural single-LOD meshes (tests, the headless example). Materials stay embedded per mesh until A3, and a skinned mesh embeds its skeleton until animation cooks skeletons as assets of their own (M2).
  - The mannequin: 251,904 bytes in M0's format; 230,092 bytes desktop with four LODs (its vertex data halves, 177 → 88 KB; LOD0 alone would be 162 KB) and 117,480 bytes mobile (three LODs, from 3,130 triangles).

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
  - Cost: about 10 heap allocations per frame (node lists, one boxed closure per pass) and under 1 µs of CPU for M0's two passes. Buffers become graph resources with compute skinning (R2); until then, uploads are side-effect passes added first.

**Scene and draws**

- **Persistent GPU scene:** an instance buffer (transforms, material IDs) and a material parameter buffer (storage buffer indexed by material ID), updated from the extract deltas.
- **Mesh pool:** large shared vertex and index buffers with offsets, which means fewer rebinds and makes MDI possible later.
- **Culling:**
  - v1: CPU frustum, distance and small-object culling, parallel per view (main camera and each shadow cascade).
  - Later, on desktop: GPU Hi-Z occlusion culling plus MDI.
- **Sorting:** opaque draws by pipeline → material → mesh, with instancing. Transparents back to front per emitter or object.
- **As built (M1 F3, `sapphire_render::scene`):**
  - Instances live in slots of the instance buffer, found by `InstanceId` (a hash map with a one-multiply hasher) and reused through a free list. A CPU mirror holds every slot's GPU data. A packet rewrites only the slots it touches, and the uploads pass writes those through the staging belt in contiguous runs.
  - Draw records name their instance by slot. The draw list is therefore rebuilt, and its records uploaded, only when instances are added or removed, change mesh, or meshes are uploaded; a move costs one 112-byte upload. `RenderStats` counts changed instances and draw-list builds.
  - A grown instance buffer is filled by a GPU copy of the old one, so instances uploaded earlier aren't sent again.
  - Draws stay in slot order. Sorting comes with a second opaque pipeline, and costs nothing on frames that only move instances, since the list is rebuilt only when the set changes.

**Skinning and morph targets**

- A compute pass skins every visible skinned instance once per frame into a per-instance vertex buffer, applying blendshape (face) weights in the same dispatch.
- The prepass, shadow, opaque and outline passes all reuse that buffer, so a toon character is skinned once rather than four times.
- **As built (M1 A5, `sapphire_render::{mesh_pool, pipelines}`, `shaders/frame.wgsl`):**
  - The vertex shaders decode the cooked streams (§5.10) as uploaded: `decode_vertex` in `frame.wgsl`, called by the toon and outline vertex shaders. Vertex slots are 0 the draw record, 1 positions, 2 shading, 3 surface, so a position-only pass (shadows, R2's prepass) binds slots 0 and 1 alone.
  - The draw record grows from 8 to 48 bytes to carry its mesh's dequantization (position offset and scale, UV transform). Decoding then costs no binding: a mesh table would have been a third vertex-stage storage buffer against the budget in the render crate docs. f32-position meshes use the same shaders with an identity transform; only slot 1's attribute differs, and their pipelines are built when the first one is uploaded.
  - The mesh pool keeps one slab of stream buffers per position format, each with a single vertex numbering, as base-vertex multi-draw-indirect will need; indices and materials are shared. Uploads copy the archive's streams as they lie. Every LOD's indices are uploaded and LOD0 is drawn until LOD selection lands with culling. The cook merges submeshes by material, so the mannequin draws 5 times per pass instead of 9.
  - **Skinned meshes draw in bind pose**, morph targets at weight 0. Influences and morph deltas stay in the cooked mesh, already in the storage layouts R2 needs. R2 uploads them, builds palettes as `model_from_joint × inverse_bind_matrix` per palette entry, decodes positions to f32 before adding deltas (morphed and skinned positions leave the bind-pose bounds), and writes per-instance f32 positions (slot 1, drawn through the f32 pipelines with an identity dequantization) and `VertexShading` (slot 2), keeping the pool's surface stream.
  - Golden images move by mean 0.0001 and p99 0.0018 FLIP on Metal (thresholds 0.01 and 0.3), so the goldens are unchanged. A GPU test renders a sphere with unorm16 and with f32 positions: 4 of 76,800 pixels differ by more than 8/255.
  - Cost against M1 R3 (sandbox `--bench 3000`, 1920×1080, 4× MSAA, Apple M5, release, three alternating rounds): CPU p50 90.2–90.6 µs against 90.9–91.2, GPU wait 483–512 µs against 486–525, both within noise; 298.3 allocations per frame against 294.3: the renderer's own per-frame code allocates nothing new, so the four are wgpu's, presumably for binding four vertex buffers instead of two (not traced further). The mesh pool holds 229,616 bytes against 251,696, with every LOD's indices.

**Lighting**

- **Desktop:** clustered forward+. A compute pass bins lights into a 16×9×24 grid.
- **Mobile:** per-object light lists, with at most 4 local lights per object.
- **Sun:** cascaded shadows. Desktop has 4 cascades (far cascades cached and updated in turns); mobile has 2.
- **Character shadow (desktop):** a separate high-resolution per-character shadow frustum for crisp contact shadows.
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
  - **Shading (`toon.wgsl`):** each listed light is lit in the sun's toon model. Its N·L is split at the material's threshold plus the ILM bias into a lit and a shadow tone, with the same authored shadow color, in the light's color × its range window × its spot cone. The window is `(1 − d²/r²)²` (no inverse-square hot spot; intensity 1 is as bright as a unit sun), and the cone a smoothstep in cos θ from the outer to the inner angle. The one-pixel edge comes from N·L's exact screen derivative per light, built from dN and dP taken once per pixel (derivatives aren't allowed in the per-pixel loop). The summed light is held under a luminance of 2, so stacked lights tint rather than blow out (the reference games' `_FilterLight`). Local lights add no rim. A frame without lights skips all of it on a uniform branch, and the three earlier goldens stayed bit-identical. In a GPU test, a red light beside a sun-shadowed sphere splits it within two pixels of the predicted terminator, at the material's 2:1 tone ratio (0.19 to 0.40 of added red across the edge).
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
  - Each character's head-forward and head-right vectors come through the packet.
  - The map is mirrored for light from the other side.
- **Hair:** anisotropic specular plus an "angel ring" highlight (a bright band across the hair), with a jitter mask.
- **Eyes:** a parallax / UV-offset refraction fake. Eyes and brows draw over hair through stencil.
- **Rim light:** a depth-difference rim on desktop (it needs the prepass); an N·V rim on mobile.
- **Character light rig:** every character gets a virtual key light whose color and intensity are curved by time of day. Shadow maps bias the ramp threshold rather than multiplying the color.

**Outlines**

- **Characters:** an inverted-hull pass. The cooker bakes smoothed normals into a spare vertex attribute (octahedral-encoded). Width comes from vertex color × a distance curve × a resolution scale; each material sets the outline color.
- **Environment (desktop, optional):** a screen-space edge pass driven by the prepass normals and IDs.
- **TAA interaction:** composite character outlines after TAA, or mark them in a responsive mask.

**Environment**

- **Shading:** a stylized GGX uber-shader.
- **Foliage:** instanced with vertex-shader wind. LODs go from cards up close to billboards to impostors.
- **Grass:** instances generated on the GPU on desktop; low-density CPU-placed instances on mobile.
- **Sky:**
  - A gradient sky plus animated 2D cloud layers, and sun and moon discs.
  - Everything is driven by artist time-of-day curves, not physical atmosphere: one `TodTimeline` of `TodKey`s, evaluated in OkLab, is the single source for sky, fog, ambient, shadow tint, post and the character light rig.
- **Fog:** distance and height fog colored by the sky horizon, plus fog cards, on every tier. Volumetric fog is desktop-only and comes later.

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

- Golden images per pass on lavapipe (Linux, pinned Mesa), WARP (Windows, pinned NuGet build) and Metal (the hosted macOS runner's paravirtual GPU), one golden set per backend, compared with `nv-flip` mean and percentile thresholds. Toon edges flip whole pixels, so the gates are p95/p99 plus an error-pixel budget, alongside intermediate-buffer goldens (Q6, M1 plan §3).
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
  - **Cvars** live in `sapphire_core::cvar`, so runtime crates declare them without depending on a dev-only crate: typed statics (`bool`, `i32`, `u32`, `f32`, or an enum of named choices) read with one relaxed atomic load, registered explicitly into a `CvarRegistry` by the app. Shipping builds link no writer, so every cvar reads its default. The sandbox declares six look-dev knobs on its `PostParams` (`post.*`) whose defaults are exactly the default look, so goldens are unchanged; the overlay adds `debug.ui_scale` and `debug.gpu_timing`.
  - **GPU pass timings** come from the frame graph (`GraphExecutor::set_gpu_timing`, M1 F5 in §5.12), not `wgpu-profiler`, which is compatible with wgpu 30 but times encoder scopes only with `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which Apple GPUs lack; on Metal every pass would need its render pass's `timestamp_writes` from the graph anyway, and its per-scope strings, channel and per-frame maps allocate where the graph's own readback allocates nothing beyond wgpu's map callback. Devices without timestamp queries (the iOS Simulator) say so in the panel.
  - **Cost** against M1 A5 (sandbox `--bench 3000`, Apple M5, release, three alternating rounds; medians of the rounds' p50): without `dev`, CPU is unchanged (90.8 µs against 90.5), allocations stay at 299.3 per frame with 97 bytes more (each pass's job in the executor grew by its order and timing flag), and the stripped executable grows 16.7 KB (0.18%: the graph report and the timestamp hooks; the timer itself is a trait object only `set_gpu_timing` creates, so it links only where it can run). With `dev` and the overlay hidden: 90.0 µs, the same allocation count and about 650 more bytes per frame (not traced), and 4.4 MB more executable (egui, its fonts and egui-wgpu). Hidden, the overlay is a frame-time store per frame and a comparison per fixed step, and the graph's report and timings are off. Shown, the default panels (stats and graph) cost about 0.23 ms of CPU (p50 315 µs), 1,500 allocations and 1.2 MB allocated per frame, every panel about 0.3 ms and 1,950 allocations; the stats panel shows the overlay's own share. The iOS Simulator holds 60 fps with it shown.
  - `--debug-overlay[=panels]` shows it from the first frame, in the window or in an offscreen `--capture` (after 8 warm-up frames, so windows are laid out and GPU timings have come back). Overlay captures are not golden images: their numbers change every run.
  - **Not yet** (later work, in the crate docs): the renderer's shader debug views (below, §5.12), the time-of-day scrubber (W1, a panel over the `TodTimeline` once it exists), the debug-draw API, the entity inspector, hitbox and intent-buffer views and the replay recorder (M2), clipboard, IME composition previews and saved panel layout.

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

| Asset | Source | Importer | `desktop` cook | `mobile` cook |
|---|---|---|---|---|
| Meshes (static and skinned) | Blender → glTF 2.0; FBX via `ufbx` | `gltf` 1.4, `ufbx` 0.11 | Quantized vertex streams; `meshopt` 0.7 vertex-cache / overdraw / fetch optimization; LODs; outline normals, either authored (an `_OUTLINE_NORMAL` vertex attribute, from M1) or baked by welding (M0: angle-weighted, sliver- and cancellation-safe, 0.1 mm weld) | Same, with a lower-detail LOD0 and fewer LODs |
| Color textures | PNG / TGA / EXR | `image`, `exr` | KTX2 (`ktx2` 0.5) BC7 via `ctt` 0.6 | KTX2 ASTC 4×4–8×8 per texture class, via `ctt` |
| Normal maps / masks / HDR | same | same | BC5 / BC4, BC5 or BC7 by channel count / BC6H | ASTC; HDR as RGBA16F or RGB9E5 |
| Ramps, ILM maps, SDF face maps | Painted | `image` | Uncompressed RGBA8 (ramps), BC7 (ILM), R8/R16 (SDF) | RGBA8, ASTC 4×4, R8 |
| Animation | glTF / FBX | `ufbx` / `gltf` | ozz archives plus root-motion track and curves | Same, with stronger compression |
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
| **M0 Bring-up** | Months 0–2 | Workspace, layer rules and CI on 3 OSes. `sapphire_platform` (SDL3 main callbacks) and `sapphire_rhi` (Metal / Vulkan / DX12) on all 5 platforms. Skeletons of `sapphire_core`, `sapphire_math`, `sapphire_jobs` and `sapphire_time`. Tracy, `sapphire_crash`, and iOS Xcode / Android Gradle packaging. A glTF character with a basic toon shader and hull outline. | **The same commit** runs a toon-shaded character at target fps on Mac, Windows, Linux (including Steam Deck), an iPhone and an Android phone. Background → foreground survives on both phones, and a forced crash reaches Sentry from every platform. |
| **M1 Data path** | Months 2–6 | ECS, reflection and serialization; the `sapphire_assets` runtime. `sapphire_pipeline`: glTF / FBX, `ctt` BC and ASTC, meshopt, packs, content-hash cache, hot reload. Renderer v1: frame graph, compute skinning, forward lighting, shadows, the anime character recipe (track C), the world look (time of day, sky, fog, ambient probes, stylized environment shader) and the post stack. VRM import and anime map tools, `sapphire_input`, `sapphire_debug`, live link, golden-image tests. The plan is [m1-plan.md](m1-plan.md). | Editing a ramp, material parameter or time-of-day key shows up on desktop *and* phone without a restart. The look-dev scene meets the tier budgets on all 5 platforms. Golden images pass on 3 backends. |
| **M2 Combat slice** | Months 4–10 | `sapphire_anim` (ozz, inertialization, IK, spring bones), `sapphire_physics`, the combat core, party swap, camera director, one enemy AI, FMOD, VFX v1, touch controls, live link, and the timeline editor v1. | One character against one enemy **feels right** with a gamepad on desktop and with touch on a phone. Replays are deterministic in CI, and Mobile-Low holds 30 fps while thermally throttled. **This is the go/no-go gate.** If tuning still feels slow, take the off-ramp in the report: move gameplay and tools to Godot with gdext, and keep the Rust systems. |
| **M3 World and tools** | Months 8–18 | Zones and transitions, navigation, sequencer and dialogue, Luau, `sapphire_ui` with the HUD and menus, saves, localization, the editor document model with undo and play-in-editor, and the pipeline (PSO) inventory. | A zone with 3 encounters, a boss authored with no Rust changes, one cutscene, and a full HUD and menus in 2 languages, all on 5 platforms. |
| **M4 Ship readiness** | Month 12 onward | PSO warm-up, signed chunk manifests and `sapphire_patch`, quality tiers with dynamic resolution and thermal handling, the symbol pipeline, accessibility, and store compliance. | Zero shader hitches in a scripted playthrough. Patches download only changed chunks. Builds pass TestFlight, Play internal testing and Steam review. |

**M0 status (2026-10-24):**

- **Done and verified on 3 of the 5 platforms.** The same code renders the toon mannequin at 60 fps on macOS (Metal), the iOS 26.3 simulator (Metal) and the Android 16 emulator (Vulkan through MoltenVK). It survives background → foreground on Android, and a forced crash leaves a valid minidump on macOS.
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
| Q5 | wgpu on mobile and Apple: on-tile MSAA / transient attachments, timestamp queries inside passes on Apple GPUs, Android pre-rotation, Metal binary archives. **Partly answered in M0:** MSAA and depth are transient attachments, and Apple GPUs report the memory saving; the M5 has no timestamps inside passes. The iOS simulator lacks base vertex / first instance, indirect execution, cube arrays and comparison samplers, so the CPU-driven draw path avoids them (ADR 0002). **M1 plan:** mobile AA is 4× MSAA kept in tile memory through `TRANSIENT_ATTACHMENT`, to be measured on Mali and Adreno; one-pass deferred stays impossible until wgpu gets input attachments (#10148). | M1 |
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
| `zstd`, `lz4_flex`, `memmap2`, `blake3` | latest | `sapphire_assets` | |
| `notify` | 8.2 | `sapphire_assets` (dev), tools | |
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
| `ctt`, `ktx2`, `meshopt` | 0.6, 0.5, 0.7 | `sapphire_pipeline`; `ktx2` also `sapphire_assets` | `basis-universal` is stale. ctt with only the bc7enc-rdo, ISPC (prebuilt) and astcenc encoders, and no `rayon` |
| `image`, `exr`, `rayon` | 0.25, 1.74, latest | `sapphire_pipeline` | Tools only. `image` with only its PNG, TGA and EXR decoders; `exr` comes through it |
| `insta`, `nv-flip`, `criterion` | latest | `sapphire_testkit` | `dssim` is AGPL, so avoid it |
| `cargo-ndk`, `sccache`, `cargo-hakari`, `cargo-nextest`, `cargo-deny` | 4.1, 0.18, –, –, – | Tooling | `bacon` is AGPL (fine as a local tool, not as a dependency) |

**Keep out of the dependency tree:** `kajiya`, `rend3`, `naga_oil`, `generational-arena`, `oxidized_navigation`, `laminar`, `rrise`, `oddio`, `bincode`, `optick`, `rustybuzz`, `bevy_cobweb_ui`, `distill`, `physx-rs`, `specs`, `legion`. Encode this list as bans in `deny.toml`.
