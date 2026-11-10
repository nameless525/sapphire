# Warming while frames draw (M4 H0)

H0's spike, measured on 2026-11-10. A throwaway harness (dropped, described below) built a tier's renderer and prewarmed an inventory on a worker while the thread that started it kept drawing frames. Unless a line says otherwise: an Apple M5 (4 performance and 6 efficiency cores, 24 GB), macOS 27.0.1, Metal, release profile, wgpu 30.0.1, the job system at its default size (9 compute workers, 2 async), 1280×720 offscreen, frames paced to 16.7 ms, each run alone on the machine through `slot --all --load 8` with load average 3.3–4.7. "ours:" marks our own inference.

## The harness

- **The inventory:** the 25 keys `shared` and `zone_lantern_ward` list (Mobile tiers: the same 25), plus every post variant the screen effects reach, effects, skinning, the interface, the probe grid and fades: what a game's prewarm declares for the Lantern Ward. On Desktop-High that is 29 global pipelines (`Renderer::new`) and 83 more (`Renderer::prewarm`); on Mobile-Low, 20 and 70.
- **The worker:** one `spawn_async` task builds the renderer, then prewarms. Both build their batches through `BuildOn::Jobs`, so on the compute pool, as today's code does.
- **The frame:** either a boot screen (one clear pass a frame, no job scope), the same with a `sapphire_jobs::scope` of eight small tasks every frame (the frame waits on the pool as the graph executor, the ECS and animation's parallel loops do), or a whole tier's renderer drawing its sky and post (which waits on the pool itself, in the graph executor).
- **Cold:** a fresh bundle ID. The binary ran from a throwaway `.app` with its own `CFBundleIdentifier`, exec'd directly (no LaunchServices, no window); Metal keys its shader cache to the bundle ID (`$(getconf DARWIN_USER_CACHE_DIR)/<bundle id>/com.apple.metal/`). Warm: the same ID run again straight after.
- **Not kept:** the harness was a hidden module and a binary in `sapphire_render`, with knobs for the frame, the tier, the pools and a persisted cache, and a second knob in `sapphire_jobs` (section 2). H1 builds the real thing; only the error-scope test is committed.

## 1. Frame times while a warm-up builds

| Frame | Build | Cache | Global; zone; wall | Frames during: p50, p99, worst | Over the interval |
|---|---|---|---|---|---|
| boot screen, no scope | Desktop-High | cold, 2 rounds | 918–923 ms; 3768–3944 ms; 4.7–4.9 s | 0.59, 6.8, 8.9–9.6 ms | 0 of 282 and of its pair |
| boot screen, no scope | Desktop-High | warm | 31 ms; 41 ms; 88 ms | under 1.4 ms | 0 |
| boot screen + a scope a frame | Desktop-High | cold, 2 rounds | 919–1210 ms; 3676–3725 ms; 4.7–4.9 s | 0.6, 362–628, **739–744 ms** | 4 of 147, 4 of 195 |
| Desktop-High drawing | Desktop-High | cold, 2 rounds | 24–29 ms (the frame's renderer built them); 3691–3706 ms | 3.1, 88–633, **749–793 ms** | 3 of 165, 3 of 113 |
| Desktop-High drawing + a scope | Desktop-High | cold, 2 rounds | 26–29 ms; 3630–3713 ms | 3.1, 412–639, **747–764 ms** | 3 of 148, 3 of 114 |
| Desktop-High drawing | Desktop-High | warm | 25–29 ms; 42 ms; 84 ms | worst 11.8–12.8 ms (the first) | 0 |
| boot screen + a scope, 33 ms | Mobile-Low, pool of 1 | cold, 2 rounds | 1254–1276 ms; 6014–6355 ms; 7.3–7.6 s | 0.6, 1258–1541, **1465–1541 ms** | 7 of 47, 7 of 56 |
| boot screen + a scope, 33 ms | Mobile-Low, pool of 3 | cold, 2 rounds | 913–1080 ms; 4408–4551 ms; 5.3–5.6 s | 0.6, 805–832, **832–914 ms** | 6 of 62, 6 of 45 |
| either phone pool | Mobile-Low | warm | 31–36 ms; 47–59 ms; ~100 ms | under 2.9 ms | 0 |

- **Building beside a frame costs it nothing while the frame never waits on the pool.** With the boot screen alone, a cold warm-up of 112 pipelines on all nine workers left every frame under 10 ms. Compiling happens mostly in `MTLCompilerService`, another process, so even nine workers waiting on it don't starve the frame's thread of a core on this machine.
- **A frame that waits on the pool takes a build itself.** `bevy_tasks`' `TaskPool::scope` ticks the pool's global executor while it waits, so the frame's thread runs whatever compute task is queued next, and during a warm-up that is a pipeline build: 0.4 to 0.8 s each on Desktop-High's cold cache, up to 1.5 s with a pool of one. Every configuration whose frame waits on the pool hitched 3–7 times per warm-up, and the real renderer's frame (no synthetic scope) did too. ours: the same is how `.cargo/config.toml`'s Windows stack note found "the main thread picked that build up while helping".
- **Warm, it's free:** a whole Desktop-High inventory in 70–100 ms of wall time, no frame over 13 ms.
- ours: the frame's own tasks also queue behind builds on a shared pool, so even a frame that only waited (never helped) would stall once the queue held more builds than workers.

## 2. Keeping the warm-up off the pool the frame helps

A second throwaway knob routed every `sapphire_jobs::scope` and `join` opened on an async-pool thread to the async pool, so the warm-up's batches (the worker is on that pool) never queue where the frame's thread helps. The same binary without the knob was the control. Load average 4.3–6.4, so read cold walls as upper bounds.

| Frame | Build, pools | Global; zone; wall | Frames during: p50, p99, worst | Over the interval |
|---|---|---|---|---|
| boot screen + a scope | Desktop-High, warm-up on 6 async threads | 1384–1533 ms; 5567–5678 ms; 7.1 s | 0.8, 4.8–6.2, 19–58 ms | 1 of 421, 1 of 426 |
| the same, control (on the pool) | Desktop-High, 9 compute | 1090–1107 ms; 4597–4690 ms; 5.7–5.8 s | 0.7, 404–467, **983–1052 ms** | 6 of 211, 4 of 189 |
| Desktop-High drawing | Desktop-High, 6 async | 15–19 ms; 5602–5797 ms; 5.6–5.8 s | 3.3, 7.9–8.6, 11.7–13.2 ms | 0 of 350, 0 of 338 |
| boot screen + a scope, 33 ms | Mobile-Low, 3 compute, warm-up on 2 async | 1708–1902 ms; 8667–9365 ms; 10.4–11.3 s | 0.7, 2.1–3.6, 5.4–15.6 ms | 0 of 339, 0 of 312 |
| warm, each of the above | | 15–26 ms; 40–64 ms; 66–100 ms | under 6.3 ms | 0 |

- **Off the pool the frame helps, the stolen builds are gone.** The worst frame fell from about a second to 13 ms with the real renderer drawing, and to 16 ms on a phone's pool at 30 Hz. The boot screen with a scope still had one frame of 19–58 ms per warm-up, each next to another long one: not a build on the frame's thread (a cold build here takes 400 ms or more); ours: contention for the efficiency cores or Metal's compiler service, worth watching on a device.
- **It costs wall time:** fewer threads build. Desktop-High's 112 took 5.6–7.1 s on six async threads against 4.7–5.8 s on nine compute threads and the frame's; Mobile-Low's 90 took 10.4–11.3 s on two against 5.3–5.6 s on three and the frame's. ours: the warm-up wants threads of its own, as many as the compute pool's workers on a boot or a load screen (the frame there is cheap), so it builds at the pool's rate without sharing its queue.
- **On the emulator** (section 5's setup, the host's Metal cache warm, no persisted Vulkan cache, alternating rounds, load 7–8): with the boot screen and a scope, the warm-up on 3 async threads left 0 frames over 33 ms (worst 24–30 ms), against 5 of 11 and 5 of 13 on the pool (worst 154–239 ms). With the saved Vulkan cache it built in 0.2 s, worst frame 11 ms. **But a drawing frame slows wherever the builds run:** with Mobile-Low's renderer drawing, frames went from a p50 of 3.1–3.4 ms before the warm-up to 27 ms during it off the pool (8 of 20 over 33 ms, worst 135 ms) and 38 ms on it (6 of 12, worst 174 ms); even the boot screen's p50 rose from 1 to 9–11 ms. ours: gfxstream carries every guest Vulkan call to one host renderer, so a pipeline being created there holds up the frame's own calls. A phone's driver doesn't do that, so this is the emulator's, but it means the emulator can't show H2's "at the tier's rate while it warms"; a device or the Simulator must.

## 3. The iOS Simulator

A private iPhone 17 Pro simulator on iOS 26.3, created, booted, run with `simctl spawn` and deleted; the harness's iOS-simulator build, a pool of 3 compute workers (the phone default), the frame waiting on a scope each frame. The simulator's boot drove load average to 32–55 through these runs, so its cold times are upper bounds, but not its counts of stolen builds.

- **Cold** (the first run on the new device): Mobile-Low's 20 global pipelines in 5.95 s and the zone's 70 in 15.2 s, 21.2 s of wall; 7 of 130 frames over the 33 ms interval, the worst 1.3 to 3.5 s: the same stolen builds as on the Mac. Mobile-High's own pipelines, cold after Mobile-Low's had warmed the shared ones: 25 and 83 in 0.8 and 3.6 s, one frame of 789 ms.
- **Warm:** 20 and 70 in 27–41 and 71–117 ms, no frame over 21 ms; the frame's own Mobile-Low renderer drawing beside it, worst 14 ms.
- **The simulator's cache doesn't follow the binary's name:** a copy under a fresh name was warm at once. ours: a spawned bare binary shares the device's content-keyed cache, as a bare binary does on macOS; only a fresh simulator device (or a fresh app bundle installed into one) is cold.
- Error scopes behave as on the Mac (section 7): of 16 pooled builds, the refused one's scope alone caught it, the thread that waited ran 3 and its own scope caught nothing.

## 4. Metal's cache, cold and warm, and how long it stays

- Cold against warm, the same bundle ID straight after: Desktop-High's 29 global pipelines 918–923 ms against 14–31 ms; the zone's 83, 3.7–3.9 s against 34–43 ms; Mobile-Low's 20 and 70, 0.9–1.3 s and 4.4–6.4 s against 31–36 and 47–59 ms. About 50 to 100 times faster warm.
- A bare binary, outside any bundle, shares a content-keyed cache across everything run that way (`$DARWIN_USER_CACHE_DIR/com.apple.metal/`), so a development build's runs are nearly always warm after the first. Only a fresh bundle ID gives a cold run.
- **How long it stays warm:** the first runs' bundles, run again with their own binaries, were warm 28 minutes later (three of them: Desktop-High 18 and 44 ms, Mobile-Low 26 and 72 ms) and 3 hours later (two more: Desktop-High 14 and 36 ms, Mobile-Low 41 and 92 ms). K1's throwaway bundle caches from 2026-11-03 07:13–07:33 were still on disk on 2026-11-10, so the OS kept them at least seven days without a reboot. ours: an OS update rebuilds the compiler and invalidates the cache, as an app update does for its own bundle; a player sees a cold cache after either.

## 5. The Vulkan cache across runs

- **lavapipe (CI):** readable from run 42772431295 (`App tests (vulkan-lavapipe)`, branch m3-u9-screens, 2026-11-10). The packs' prewarm logs `keys=25 built=37` in 514.6, 500.1 and 498.5 ms in three processes of the same job: no gain from a run before. ours: no offscreen run opens a `PipelineCacheDesc` (only the windowed sandbox and `game_app`'s window do), so CI never persists one, and lavapipe compiles through LLVM each time. The golden logs carry no timing lines. lavapipe isn't installed on this Mac, and downloads are out of scope, so nothing local.
- **The emulator:** the `sapphire_api36` AVD, headless (`-no-window -gpu host -read-only -no-snapshot-save`), the harness's Android build run from `adb shell`. Vulkan reaches the host through gfxstream, and the adapter reports `Vulkan "Apple M5" MoltenVK 1.4.0`, so a guest pipeline ends as a Metal compile on the Mac, behind the host's own Metal cache. Mobile-Low, a pool of 3, the frame waiting on a scope, 30 Hz; load average 7.8–13.5.

  | Run | Persisted cache | Global; zone; wall |
  |---|---|---|
  | first after boot | empty, then saved (2.77 MB, one file) | 140 ms; 13,400 ms; 13.6 s |
  | second | none | 288 ms; 798 ms; 1.12 s |
  | third | the saved one | 78 ms; 153 ms; 0.24 s |
  | fourth | none | 92 ms; 534 ms; 0.66 s |
  | fifth | the saved one | 48 ms; 140 ms; 0.23 s |

  The first run paid the host's cold Metal compiles; after it the host's cache stays warm, and what the persisted Vulkan cache saves on top is MoltenVK's own work: 0.23–0.24 s against 0.66–1.12 s, three to five times faster. ours: on a real Android GPU the driver compiles SPIR-V itself, with no host cache under it, so the persisted cache will matter more there than here; a device must measure it. The cache was saved and read back with `sapphire_rhi::pipeline_cache` as the windowed sandbox does.
- **WARP:** not reachable from this Mac (Windows CI only, and its logs print no build times).

## 6. What one shader module costs the CPU

`create_shader_module` per program, median of 15, on Metal: 0.18 ms (debug draw, 149 lines) to 3.8 ms (outline, 3837 lines); the anime character shader 3.4 ms; all 17 programs 24.8 ms. naga's parse and validate alone take the same to within 0.05 ms, so on Metal the module's cost is naga's: wgpu translates to MSL and Metal compiles only when a pipeline is built. On the simulator, 0.24 to 4.3 ms, 29.7 ms for all 17, at load average 32. On the emulator's Vulkan, 0.22 to 6.7 ms, 41.1 ms for all 17, naga's parse and validate again all but 0.7 ms of it. ours: a module is cheap enough to build on the thread that needs it; the pipeline is what must move.

## 7. How a worker learns of a build wgpu refused

- wgpu 30's error scopes are per thread (`wgpu_core` keys its stack by `ThreadId`; the guard is `!Send`; popping on another thread panics). Sixteen builds on the pool, each inside its own validation and internal scopes, one naming a fragment entry point that doesn't exist: only that build's scope caught `Validation Error`, on whichever worker ran it; the thread that waited on the scope ran 3 of the 16 itself, and its own scope caught nothing; the device's uncaptured handler saw nothing. A GPU test keeps this (`pipeline_builds::tests::a_build_refused_on_the_pool_reaches_only_its_own_scope`).
- An unscoped build reaches the uncaptured handler on the thread that built it (sapphire_rhi's handler panics in debug builds on a live device). A refused pipeline is still returned, invalid, and drawing with it is a validation error on the drawing thread.
- ours: so a warm-up build scopes itself on its worker and returns its error with its pipeline, as `shader_builds` does for an edit; `BuildOn::Jobs` can then take edited sources too.

## 8. What changes for M4

- **§3 "Warm-up" doesn't hold as written.** "Pipelines build on job workers" puts them on the compute pool, and any frame that waits on that pool (the graph executor, the ECS, animation's loops) runs a queued build itself: 3–7 frames of 0.4–1.5 s per cold warm-up on Metal, 7 of 1.3–3.5 s on the Simulator, 5 of 0.15–0.24 s on the emulator even with the host warm. Proposed: the warm-up builds on threads of its own that no frame waits on (a warm-up pool, or the async pool grown for a load), and a batch opened on them stays there; on a boot or load screen it gets as many threads as the compute pool has workers, so it loses no speed. H1's "a warm-up on the job pool" changes to match. Off the shared pool, Metal's worst frame was 13 ms and the 30 Hz phone pool's 16 ms.
- **§3 "What a hitch is" holds,** and H1's count already sees this case: a stolen build runs on the frame's thread, so `PipelineBuilds` counts it there. H1 should keep a test that a frame waiting on the compute pool during a warm-up builds nothing.
- **§3 "Driver caches" holds.** The persisted Vulkan cache builds three to five times faster on the emulator with the host warm (a device must measure a real driver). CI's lavapipe never persists one, so H4's lavapipe lane always measures a cold cache: fine for counting builds, not for timing them.
- **Error scopes:** a warm-up build pushes its scopes on the thread that runs it, as the committed test holds, so `BuildOn::Jobs` needn't be limited to compiled-in sources (spec §5.12's "Only compiled-in sources go to the pool").
- **A cold Metal cache for H4** is a fresh bundle ID (a throwaway `.app`, exec'd directly). A bare binary's cache is shared, so it's warm after any earlier run of the same shaders; the Simulator's is shared across binary names, so a cold Simulator run needs a fresh device.
- **The emulator can't prove frame rates while warming** (section 2): its drawing frames slow during any build. H2's "at the tier's rate from a cold cache" needs the Simulator, a Mac or a device.
