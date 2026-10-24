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

---

## 5. Module specs

### 5.0 Module index

| Crate | Layer | Purpose | Key dependencies | Milestone |
|---|---|---|---|---|
| `sapphire_platform` | L0 | App lifecycle, windows, surfaces, events, file IO, threads, system info | `sdl3-sys`, `raw-window-handle` | M0 |
| `sapphire_crash` | L0 | Crash capture and upload | `crash-handler`, `minidumper`, `sentry` | M0 |
| `sapphire_core` | L1 | Allocators, handles, names, logging, profiling, cvars | `mimalloc`, `bumpalo`, `slotmap`, `tracing`, `profiling` | M0 |
| `sapphire_math` | L1 | Vectors, transforms, bounds, curves, springs, RNG | `glam`, `libm`, `rand_pcg` | M0 |
| `sapphire_jobs` | L1 | The single task pool, scopes, parallel loops | `bevy_tasks` | M0 |
| `sapphire_ecs` | L2 | Re-exported bevy_ecs, schedules, conventions | `bevy_ecs` (pinned) | M1 |
| `sapphire_time` | L2 | Clock domains, fixed-step driver, interpolation | — | M0 |
| `sapphire_reflect` | L2 | Type registry, stable type names, schema versions | `bevy_reflect` | M1 |
| `sapphire_serialize` | L2 | Authored (RON), cooked (rkyv) and save/network (postcard) formats | `serde`, `ron`, `rkyv`, `postcard` | M1 |
| `sapphire_assets` | L2 | Handles, packs, async loading, residency, hot reload | `zstd`, `lz4_flex`, `memmap2`, `blake3` | M1 |
| `sapphire_rhi` | L2 | wgpu device and feature tiers, uploads, pools, pipeline cache | `wgpu`, `wgpu-profiler` | M0 |
| `sapphire_render` | L3 | Frame graph, toon and environment renderers, post, upscaling | `sapphire_rhi`, `wesl`, `naga`, `encase`, `bytemuck` | M1–M3 |
| `sapphire_anim` | L3 | Skeletal runtime, anim graph, IK, spring bones, cloth | `ozz-animation-rs` (vendored) | M2 |
| `sapphire_physics` | L3 | Collision world, queries, character capsules, hit shapes | `rapier3d`, `parry3d` | M2 |
| `sapphire_audio` | L3 | FMOD Studio integration and cue playback | FMOD (own bindings); `kira` as fallback | M2 |
| `sapphire_vfx` | L3 | GPU particles, trails, decals, screen effects | `sapphire_render` | M2–M3 |
| `sapphire_ui` | L3 | Retained game UI, text, touch controls | `taffy`, `parley` | M3 |
| `sapphire_input` | L3 | Action mapping, contexts, rebinding, glyphs | `sapphire_platform` | M1 |
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

- **Desktop:** initialize first in `main`. `crash-handler` 0.8 catches signals and exceptions, and `minidumper` 0.11 writes minidumps from a separate monitor process. On the next launch, or from the monitor, upload them to Sentry (0.49.3+ supports native minidumps).
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
- **Profiling:** `profiling` crate macros everywhere. The backend is chosen in the binary: Tracy (`tracy-client` 0.19), puffin, or none.
- **Cvars:** a typed, reflected registry (`cvar!(r_shadow_cascades: u32 = 4, "…")`). It can be set from config files, the dev console, live link or the command line. Shipping builds compile cvars down to constants or read-only values.
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

### 5.9 `sapphire_serialize` (L2)

- **Authored data** (editor documents, configs) is RON via serde: text, diff-friendly and mergeable in git.
- **Cooked runtime data** is rkyv 0.8 archives, memory-mapped and accessed in place.
  - Validate with bytecheck in dev builds.
  - In shipping builds, skip validation only for packs whose blake3 hash was verified against a signed manifest.
- **Saves and network:** `postcard` with a versioned header.
- **Never use `bincode`.** Version 3.0.0 is a deliberate `compile_error!` and the project is abandoned.

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
- **Uploads:** a staging ring buffer, plus `queue.write_buffer` for small per-frame data.
- **Transient pools:** textures and buffers pooled by descriptor and reused across frames. wgpu can't alias GPU memory, so pooling is the substitute.
- **Pipeline cache:** on Vulkan, create `wgpu::PipelineCache`, load it from `user://` at boot and save it at shutdown and after load screens.
- **Surfaces:**
  - Recreate on resize, on `SurfaceLost`, and on Android surface destroy/create.
  - Present mode: FIFO on mobile; FIFO by default on desktop, with Mailbox / Immediate as user options.
  - HDR surfaces come *later*.
- **Debug:**
  - Label every resource.
  - GPU pass timings via `wgpu-profiler` 0.28.
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

**Scene and draws**

- **Persistent GPU scene:** an instance buffer (transforms, material IDs) and a material parameter buffer (storage buffer indexed by material ID), updated from the extract deltas.
- **Mesh pool:** large shared vertex and index buffers with offsets, which means fewer rebinds and makes MDI possible later.
- **Culling:**
  - v1: CPU frustum, distance and small-object culling, parallel per view (main camera and each shadow cascade).
  - Later, on desktop: GPU Hi-Z occlusion culling plus MDI.
- **Sorting:** opaque draws by pipeline → material → mesh, with instancing. Transparents back to front per emitter or object.

**Skinning and morph targets**

- A compute pass skins every visible skinned instance once per frame into a per-instance vertex buffer, applying blendshape (face) weights in the same dispatch.
- The prepass, shadow, opaque and outline passes all reuse that buffer, so a toon character is skinned once rather than four times.

**Lighting**

- **Desktop:** clustered forward+. A compute pass bins lights into a 16×9×24 grid.
- **Mobile:** per-object light lists, with at most 4 local lights per object.
- **Sun:** cascaded shadows. Desktop has 4 cascades (far cascades cached and updated in turns); mobile has 2.
- **Character shadow (desktop):** a separate high-resolution per-character shadow frustum for crisp contact shadows.
- **Ambient:**
  - L2 spherical-harmonics probes baked per zone.
  - Reflection probes: BC6H on desktop; RGBA16F or RGB9E5 at small sizes on mobile.
  - Capsule AO for characters on all tiers; GTAO on desktop.

**Toon character shading**

One uber-shader family, with at most 8 static permutation bits: skinned, alpha-test, outline, morph, face, hair, eye, transparent.

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
  - Everything is driven by artist time-of-day curves, not physical atmosphere.

**Post-processing**

- **Bloom:** a downsample chain from 1/2 to 1/32 resolution, with an emissive-driven bloom mask.
- **Tonemapping:** a custom tone curve and grading baked into a 32³ LUT. No ACES look.
- **Gameplay-triggered effects:** finish-freeze, halftone, radial blur and chromatic aberration, triggered from `PostParams`.
- **Desktop only:** depth of field in cutscenes and optional camera motion blur.
- **Mobile:** bloom composite, LUT, vignette and gameplay effects are fused into one full-screen pass to save bandwidth.

**Anti-aliasing and upscaling**

- **Desktop:** TAA with a responsive mask, or DLSS (NVIDIA + Vulkan via `dlss_wgpu`) or MetalFX temporal (via `objc2-metal-fx`).
- **Mobile:** MSAA 2× / 4× on the main pass if wgpu can keep MSAA attachments on-tile (**Verify**). Otherwise use FXAA in the fused post pass. Upscale with your own spatial upscaler, an FSR1-style EASU + RCAS port in WGSL, or MetalFX spatial on iOS.

**Materials, shaders and PSOs**

- **Source:** WGSL with WESL imports (`wesl` 0.6), validated and reflected with naga. Parameter structs are laid out with `encase` / `bytemuck`; bindings are generated with `wgsl_bindgen`.
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

**Tests**

- Golden images per pass on lavapipe (Linux), WARP (Windows) and Metal (macOS runner), compared with `nv-flip` tolerances.
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

| Asset | Source | Importer | `desktop` cook | `mobile` cook |
|---|---|---|---|---|
| Meshes (static and skinned) | Blender → glTF 2.0; FBX via `ufbx` | `gltf` 1.4, `ufbx` 0.11 | Quantized vertex streams; `meshopt` 0.7 vertex-cache / overdraw / fetch optimization; LODs; outline normals, either authored (an `_OUTLINE_NORMAL` vertex attribute, from M1) or baked by welding (M0: angle-weighted, sliver- and cancellation-safe, 0.1 mm weld) | Same, with a lower-detail LOD0 and fewer LODs |
| Color textures | PNG / TGA / EXR | `image`, `exr` | KTX2 (`ktx2` 0.5) BC7 via `ctt` 0.6 | KTX2 ASTC 4×4–8×8 per texture class, via `ctt` |
| Normal maps / masks / HDR | same | same | BC5 / BC4 / BC6H | ASTC; HDR as RGBA16F or RGB9E5 |
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
| Every PR | Golden images: lavapipe (Vulkan) and WARP (DX12) | Linux, Windows |
| Nightly | Golden images on Metal | Self-hosted Mac (**Verify** whether hosted macOS runners expose a usable GPU) |
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
| **M1 Data path** | Months 2–6 | ECS, reflection and serialization; the `sapphire_assets` runtime. `sapphire_pipeline`: glTF / FBX, `ctt` BC and ASTC, meshopt, packs, content-hash cache, hot reload. Renderer v1: frame graph, compute skinning, forward lighting, shadows, the full toon recipe, outlines, bloom, LUT. `sapphire_input`, `sapphire_debug`, golden-image tests. | Editing a ramp or material parameter shows up on desktop *and* phone without a restart. Golden images pass on 3 backends. |
| **M2 Combat slice** | Months 4–10 | `sapphire_anim` (ozz, inertialization, IK, spring bones), `sapphire_physics`, the combat core, party swap, camera director, one enemy AI, FMOD, VFX v1, touch controls, live link, and the timeline editor v1. | One character against one enemy **feels right** with a gamepad on desktop and with touch on a phone. Replays are deterministic in CI, and Mobile-Low holds 30 fps while thermally throttled. **This is the go/no-go gate.** If tuning still feels slow, take the off-ramp in the report: move gameplay and tools to Godot with gdext, and keep the Rust systems. |
| **M3 World and tools** | Months 8–18 | Zones and transitions, navigation, sequencer and dialogue, Luau, `sapphire_ui` with the HUD and menus, saves, localization, the editor document model with undo and play-in-editor, and the pipeline (PSO) inventory. | A zone with 3 encounters, a boss authored with no Rust changes, one cutscene, and a full HUD and menus in 2 languages, all on 5 platforms. |
| **M4 Ship readiness** | Month 12 onward | PSO warm-up, signed chunk manifests and `sapphire_patch`, quality tiers with dynamic resolution and thermal handling, the symbol pipeline, accessibility, and store compliance. | Zero shader hitches in a scripted playthrough. Patches download only changed chunks. Builds pass TestFlight, Play internal testing and Steam review. |

**M0 status (2026-10-24):**

- **Done and verified on 3 of the 5 platforms.** The same code renders the toon mannequin at 60 fps on macOS (Metal), the iOS 26.3 simulator (Metal) and the Android 16 emulator (Vulkan through MoltenVK). It survives background → foreground on Android, and a forced crash leaves a valid minidump on macOS.
- **Still open for M0 exit:**
  - A run on Windows and Linux hardware (CI builds them, but they haven't been run).
  - A physical iPhone, which needs a signing identity.
  - A physical Android phone.
  - Sentry upload with a real DSN.
  - Tracy, which is wired through the `tracy` feature but hasn't been run against a Tracy server.


---

## 11. Open questions to resolve

| # | Question | Resolve by |
|---|---|---|
| Q1 | ~~Does the SDL3 entry flow work from Rust?~~ **Answered in M0:** yes. `SDL_RunApp` runs on the iOS simulator, and an exported `SDL_main` with SDL's JNI symbols links for Android (see Android results in ADR 0002). | M0 spike |
| Q2 | ~~Android 16 KB page alignment~~ **Answered in M0:** NDK r28+ defaults already produce 16 KB-aligned `.so` files; the rustflag in `.cargo/config.toml` is belt and braces. | M0 |
| Q3 | Steam Deck: native Linux build, or the Windows build under Proton? Prefer native. | M0 |
| Q4 | Which Windows default backend, Vulkan or DX12, based on Intel / AMD / NVIDIA driver testing? | M1 |
| Q5 | wgpu on mobile and Apple: on-tile MSAA / transient attachments, timestamp queries inside passes on Apple GPUs, Android pre-rotation, Metal binary archives. **Partly answered in M0:** MSAA and depth are transient attachments, and Apple GPUs report the memory saving; the M5 has no timestamps inside passes. The iOS simulator lacks base vertex / first instance, indirect execution, cube arrays and comparison samplers, so the CPU-driven draw path avoids them (ADR 0002). | M1 |
| Q6 | Do hosted CI runners expose a GPU for Metal golden images? | M1 |
| Q7 | Does ozz-animation-rs cover offline animation building, or is the C++ ozz tooling needed in the cooker? | Start of M2 |
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
| `sentry` | ≥ 0.49.3 | `sapphire_crash` | Native minidump capture |
| `mimalloc` | latest | `sapphire_core` | |
| `bumpalo` / `bump-scope` | 3.20 / 2.4 | `sapphire_core` | std `Allocator` trait from Rust 1.100 |
| `slotmap` | 1.1 | `sapphire_core` | |
| `tracing`, `profiling`, `tracy-client`, `puffin` | –, –, 0.19, 0.20 | `sapphire_core` | |
| `glam` | 0.34 | `sapphire_math` and up | |
| `libm`, `rand_pcg` | latest | `sapphire_math` | `libm` **unchecked** |
| `bevy_ecs`, `bevy_tasks`, `bevy_reflect` | 0.20 | `sapphire_ecs`, `sapphire_jobs`, `sapphire_reflect` | Pin per milestone |
| `serde`, `ron`, `rkyv`, `postcard` | –, –, 0.8, 1.1 | `sapphire_serialize` | **Never** `bincode` |
| `zstd`, `lz4_flex`, `memmap2`, `blake3` | latest | `sapphire_assets` | |
| `notify` | 8.2 | `sapphire_assets` (dev), tools | |
| `wgpu`, `naga` | 30 | `sapphire_rhi`, `sapphire_render` | |
| `wesl` | 0.6 | `sapphire_render` | `naga_oil` is archived |
| `encase`, `bytemuck`, `wgsl_bindgen` | 0.13, –, 0.23 | `sapphire_render` | |
| `wgpu-profiler` | 0.28 | `sapphire_rhi` | |
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
| `egui`, `egui-wgpu`, `egui_dock` / `egui_tiles`, `egui_plot` | 0.37 line | `sapphire_debug`, `sapphire_editor` | |
| `transform-gizmo`, `egui-snarl` | 0.11, 0.12 | `sapphire_editor` | |
| `gltf`, `ufbx` | 1.4, 0.11 | `sapphire_pipeline` | |
| `ctt`, `ktx2`, `meshopt` | 0.6, 0.5, 0.7 | `sapphire_pipeline` | `basis-universal` is stale |
| `image`, `exr`, `rayon` | latest | `sapphire_pipeline` | Tools only |
| `insta`, `nv-flip`, `criterion` | latest | `sapphire_testkit` | `dssim` is AGPL, so avoid it |
| `cargo-ndk`, `sccache`, `cargo-hakari`, `cargo-nextest`, `cargo-deny` | 4.1, 0.18, –, –, – | Tooling | `bacon` is AGPL (fine as a local tool, not as a dependency) |

**Keep out of the dependency tree:** `kajiya`, `rend3`, `naga_oil`, `generational-arena`, `oxidized_navigation`, `laminar`, `rrise`, `oddio`, `bincode`, `optick`, `rustybuzz`, `bevy_cobweb_ui`, `distill`, `physx-rs`, `specs`, `legion`. Encode this list as bans in `deny.toml`.
