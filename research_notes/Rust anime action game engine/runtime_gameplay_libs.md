# Rust Runtime & Gameplay Libraries for a WuWa/ZZZ-Style 3D Anime Action Engine (as of 2026-10-24)

Method note: version numbers, release dates, total and recent (last 90 days) download counts, and the "releases in the last 12 months" figures come from the crates.io API, queried on 2026-10-24. Repo activity (archived flag, last push, stars) comes from the GitHub API on the same day. Release notes, READMEs and changelogs were read from primary sources (GitHub, bevy.org, dimforge.com, joonaa.dev). "Inferences" hold opinions and recommendations. Rendering, windowing, UI, asset formats, profiling, gilrs and localization are out of scope (other researchers cover them).

---

## ECS / data model: which ECS should a custom engine use? (bevy_ecs standalone, hecs, flecs_ecs, shipyard, specs/legion, evenio, sparsey; benchmarks; relations, observers, change detection)

### Takeaway
bevy_ecs is the clear ecosystem leader. It is the only Rust ECS with relationships, observers, hooks, fine-grained change detection, a parallel scheduler and reflection all under active development, and it has been shipped standalone, with a custom renderer, in Tiny Glade. Its cost is a breaking release every 4–5 months. hecs is the best minimal, stable alternative. flecs_ecs has the richest relationship model but is still alpha. specs, legion and evenio are dead or dormant.

### Cited Findings
**Version and maintenance snapshot (crates.io / GitHub, 2026-10-24):**

| Crate | Latest stable (date) | Total dl / recent 90d | Releases last 12 mo | License | Repo status | Source |
|---|---|---|---|---|---|---|
| bevy_ecs | 0.20.0 (2026-10-08) | 9.83M / 2.32M | 13 | MIT OR Apache-2.0 | very active | [crates.io](https://crates.io/crates/bevy_ecs) |
| hecs | 0.11.2 (2026-10-04) | 636k / 171k | 3 | MIT OR Apache-2.0 | active, last push 2026-10-08, 1,384★ | [crates.io](https://crates.io/crates/hecs), [GitHub](https://github.com/Ralith/hecs) |
| flecs_ecs | 0.2.2 (2025-11-17) | 14k / 3.9k | 3 | MIT | last push 2026-07-26, 341★ | [crates.io](https://crates.io/crates/flecs_ecs), [GitHub](https://github.com/Indra-db/Flecs-Rust) |
| shipyard | 0.11.5 (2026-07-10) | 181k / 30k | 7 | MIT OR Apache-2.0 | active, last push 2026-10-04, 893★ | [crates.io](https://crates.io/crates/shipyard), [GitHub](https://github.com/leudz/shipyard) |
| sparsey | 0.13.4 (2026-08-30) | 22k / 215 | 1 | MIT OR Apache-2.0 | low use, 195★ | [crates.io](https://crates.io/crates/sparsey) |
| edict | 0.6.1 stable (2024-06-09); 1.0.0-rc10 (2026-07-24) | 44k / 417 | 2 | MIT OR Apache-2.0 | niche, 111★ | [crates.io](https://crates.io/crates/edict) |
| specs | 0.20.0 (2023-09-25) | 1.15M / 63k | 0 | MIT OR Apache-2.0 | last push 2024-06-07: **effectively unmaintained** | [crates.io](https://crates.io/crates/specs), [GitHub](https://github.com/amethyst/specs) |
| legion | 0.4.0 (2021-02-25) | 295k / 17k | 0 | MIT | last push 2021-12-30: **abandoned** | [crates.io](https://crates.io/crates/legion), [GitHub](https://github.com/amethyst/legion) |
| evenio | 0.6.0 (2024-05-19) | 18k / 471 | 0 | MIT | last push 2024-09-09: **dormant** | [crates.io](https://crates.io/crates/evenio), [GitHub](https://github.com/rj00a/evenio) |

**bevy_ecs features and recent changes:**
- Bevy release cadence: 0.17 on 2025-09-30, 0.18 on 2026-01-13, 0.19 on 2026-06-19, 0.20 on 2026-10-08, each with breaking changes and a migration guide — [bevy-website news dir](https://github.com/bevyengine/bevy-website/tree/main/content/news), [0.19→0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- 0.17 rearchitected the Event/Observer APIs to be "clearer, easier to use, and more performant". Observers are described as "one of our most popular features" — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- 0.19 made resources components on singleton entities. Resources can now be queried, can have relationships pointing to and from them, and can carry lifecycle observers and hooks. 0.19 also added observer run conditions, self-referential relationships, contiguous query access, delayed commands and remote entity reservation, which allocates entity IDs without `&World` for parallel work — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)
- 0.20 added per-column change ticks, which let a query "cheaply skip the whole column of entities when querying for changes" (132x speedup reported in GPU mesh extraction). It also added `despawn_all` batch despawning (1.30–1.70x faster), `chain_weak` system ordering, schedule randomization for testing, and a bottom-up `Ready` event for spawned scenes — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- bevy_ecs 0.20 cargo features include `std` (it is no_std-capable when this is off), `multi_threaded`, `async_executor`, `bevy_reflect`, `serialize`, `hotpatching` and `track_location`. Its mandatory deps include bevy_tasks, bevy_platform, bevy_utils, bumpalo, slotmap, smallvec and concurrent-queue, so standalone use still pulls a small set of Bevy crates — [crates.io API: bevy_ecs 0.20.0](https://crates.io/crates/bevy_ecs/0.20.0)

**Standalone bevy_ecs in a shipped game (Tiny Glade):**
- Tomasz Stachowiak (Pounce Light) says the game's base layer is "the framework called the ECS". They use "a modified version of Bevy" because "our needs outscaled what Bevy could provide at the time", and moved rendering to their own Vulkan pipeline, which let them "control pretty much the whole pipeline". The article doesn't explicitly say whether any Bevy rendering code remains — [80.lv interview](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)
- Stachowiak adds that Bevy fits only "if you're not scared of heavy engineering" — [80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)

**flecs_ecs (Rust bindings to Flecs C/C++):**
- The README says "Status: Alpha release … still in an experimental phase", with soundness and safety issues to resolve before a stable release. The test-suite port is about 70% done. Rust reflection is WIP. wasm32-unknown-unknown support was "in the process" — [Flecs-Rust README](https://github.com/Indra-db/Flecs-Rust)
- Flecs offers first-class relationships (union, exclusive, oneof, traversal, reflexive), native hierarchies and prefabs (IsA) — [Flecs-Rust README](https://github.com/Indra-db/Flecs-Rust). The upstream C library is very active (8,765★, pushed 2026-10-23) — [GitHub](https://github.com/SanderMertens/flecs)

**Benchmarks:**
- The classic `rust-gamedev/ecs_bench_suite` is **archived** (last push 2022-11-27) — [GitHub](https://github.com/rust-gamedev/ecs_bench_suite)
- Newest public multi-ECS comparison found: the SkyECS "compare-ecs" Criterion suite. It is run on GitHub Actions shared runners and written by the author of a competing ECS, so treat it as self-reported. It compares Sky, hecs, Bevy, **Flecs C** (not flecs_ecs), FreeCS and Shipyard. Selected medians — [SkyECS BENCHMARKS.md](https://github.com/jz315/SkyECS/blob/main/benches/BENCHMARKS.md):
  - Prepared iteration, 100K: Flecs C 75.2 µs, Sky 77.3, hecs 78.3, **Bevy 95.0**, FreeCS 119.8, Shipyard 173.4
  - Spawn/despawn 1K: Sky 41.5 µs, hecs 44.8, Flecs C 68.3, Bevy 102.8, Shipyard 112.7
  - Add/remove component 1K: **Shipyard 52.1 µs** (sparse-set advantage), Sky 88.2, hecs 109.2, Flecs C 120.0, Bevy 161.6
  - Random access, warm 100K: hecs 294 µs, Sky 303, Shipyard 439, Flecs C 670, Bevy 858
  - "Gameplay scenario" full frame (65,536 entities): Sky 113.9 µs, Flecs C 134.9, hecs 138.7, FreeCS 177.9, Bevy 201.1, Shipyard 311.2
  - Heavy-compute diagnostic: all six within about 10% (3.3–3.7 ms)
- An older 2020 Bevy post noted ecs_bench was single-threaded only — [bevy-website 2020 post](https://github.com/TrialDragon/bevy-website/blob/2f107e59869c15b61646043ec7c56de5002cee46/content/news/2020-08-10-introducing-bevy/index.md)

### Inferences
- **Recommendation: bevy_ecs used standalone, with the version pinned per milestone.** Combat games need relationships (weapon/owner, hitbox/attacker, parent/child bones), observers (on-hit, on-death and on-buff-applied events), change detection (dirty transforms, UI sync), a parallel scheduler and reflection for editor and save. bevy_ecs is the only Rust option with all of these, maintained by a large team (227 contributors in 0.20). Tiny Glade shows it works with a fully custom renderer.
- The micro-benchmark gaps (Bevy roughly 1.2–2x slower than hecs/Flecs on spawn and random access) are unlikely to matter at the entity counts of a zone-based action game: hundreds of characters, thousands of projectiles. Bulk work such as skinning, culling and particles should run in SoA job systems outside the ECS anyway.
- Main risk: 4–5-month breaking cadence. Plan for one migration per Bevy release, or fork and freeze as Pounce Light did.
- hecs is the conservative fallback: tiny, fast, stable API, but no scheduler, observers or relationships, so you build those yourself. flecs_ecs is attractive for its relationship and query power but is alpha, depends on C, and has WASM incomplete. Not recommended for production yet.
- Do not use specs, legion or evenio for new work.

### Gaps
- No independent, multi-threaded, game-representative ECS benchmark from 2026 was found. The SkyECS numbers are self-reported and single-threaded.
- No primary source found confirming exactly which Bevy crates Tiny Glade still uses (ECS only, or also bevy_app, bevy_asset and others).
- Console (PS5/Xbox/Switch 2) shipping evidence for bevy_ecs was not found. The `no_std` support helps porting, but no shipped console title was verified.

---

## Math & SIMD: glam vs nalgebra vs ultraviolet; mint; wide; std::simd status; large-world (f64) coordinates

### Takeaway
glam is the de facto standard for game math. Bevy uses it, and Rapier/parry moved their public API to glam in January 2026. It has SSE2/NEON/wasm-simd128 storage types, f64 `DVec*`, and a `libm` feature for no_std builds and pinned transcendental math. `std::simd` is still nightly-only. For large worlds, use f64 world positions (glam `DVec3`, avian `f64`, `rapier3d-f64`, or `big_space` grid cells) with f32 local math.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Releases last 12 mo | License | Source |
|---|---|---|---|---|---|
| glam | 0.34.1 (2026-10-06) | 169.5M / 57.5M | 21 | MIT OR Apache-2.0 | [crates.io](https://crates.io/crates/glam) |
| nalgebra | 0.35.0 (2026-05-24) | 97.4M / 17.1M | 3 | Apache-2.0 | [crates.io](https://crates.io/crates/nalgebra) |
| ultraviolet | 0.10.0 (2025-04-26) | 1.42M / 304k | 0 (repo pushed 2026-10-04) | MIT OR Apache-2.0 OR Zlib | [crates.io](https://crates.io/crates/ultraviolet), [GitHub](https://github.com/fu5ha/ultraviolet) |
| mint | 0.5.9 (2022-02-02) | 12.4M / 1.69M | 0 | MIT | [crates.io](https://crates.io/crates/mint) |
| wide | 1.7.1 (2026-09-14) | 85.9M / 24.1M | 15 | Zlib OR Apache-2.0 OR MIT | [crates.io](https://crates.io/crates/wide) |
| big_space | 0.13.0 (2026-10-08); 0.14.0-rc.0 | 39k / 4.3k | 3 | MIT OR Apache-2.0 | [crates.io](https://crates.io/crates/big_space) |
| simba | 0.10.2 (2026-08-07) | 83.2M | 3 | Apache-2.0 | [crates.io](https://crates.io/crates/simba) |

- glam: SIMD storage on x86, x86_64, aarch64 and wasm32/64. SSE2 is on by default on x86_64 and NEON by default on aarch64; wasm `simd128` needs a target feature. There is experimental `core-simd` (portable SIMD) support, f64 (`DVec2/3/4`) and i64 types, and a `libm` feature for no_std — [glam-rs README](https://github.com/bitshifter/glam-rs)
- Rapier 0.32 (2026-01-09) migrated its public math from nalgebra to glam. Dimforge's stated reason is that the "rust-gpu compiler backend has first-class support of `glam`". There was no performance change on M4 Max, `Vec3A` cost about 10% so `Vec3` was chosen, and debug builds were about 20% faster. nalgebra remains internally for multibody and SIMD solver code — [Dimforge 2025 review](https://dimforge.com/blog/2026/01/09/the-year-2025-in-dimforge), [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- Rapier 0.35 says cross-platform determinism "rests on glam's shared scalar core plus `libm`-pinned transcendentals", and SIMD is "always on, backed by `wide`" — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- `std::simd` / portable SIMD: tracking issue #86656 is still **open** (updated 2026-08-12), so the feature is still nightly-only (`#![feature(portable_simd)]`). The rust-lang/portable-simd repo is active (pushed 2026-09-16) — [rust-lang/rust#86656](https://github.com/rust-lang/rust/issues/86656), [portable-simd](https://github.com/rust-lang/portable-simd)
- Avian 0.8 (2026-10-20) reworked its `f64` feature to use double precision "only … for body positions and other world-space coordinates", with everything else single precision. The approach is "largely inspired by" Jolt — [Avian 0.8 release](https://github.com/avianphysics/avian/releases/tag/v0.8.0), [Avian 0.8 blog](https://joonaa.dev/blog/14/avian-0-8)
- `rapier3d-f64` 0.36.1 (2026-10-10) is published alongside f32 rapier3d — [crates.io](https://crates.io/crates/rapier3d-f64)

### Inferences
- **Recommendation: glam everywhere at runtime.** Use `Vec3`/`Quat`/`Affine3A` for local and render math and `DVec3` for world-space positions in open zones. Use `wide` (already a Rapier dependency) or hand-written `core::arch` intrinsics for SoA hot loops such as skinning, spring bones and culling. Keep `mint` only at FFI and interop boundaries.
- For a WuWa-scale open world (many square km), use a hybrid: f64 or grid-cell world positions (`big_space`-style or a custom equivalent) plus camera-relative f32 for rendering and animation. Pick a physics engine with matching large-world support (Jolt double-precision, Avian f64 positions, rapier3d-f64).
- nalgebra is overkill for game runtime code. Use it only for tooling or solvers.

### Gaps
- No up-to-date 2026 mathbench comparison (glam vs nalgebra vs ultraviolet) was retrieved.
- No timeline for stabilizing `std::simd` was found.

---

## Concurrency / job systems: rayon, bevy_tasks, crossbeam, fibers, async executors, lock-free structures

### Takeaway
There is no maintained, production-grade fiber job system crate in Rust. The mature building blocks are rayon (work-stealing data parallelism), crossbeam-deque (the work-stealing primitive under rayon), bevy_tasks (async-executor based, drives the Bevy scheduler), parking_lot, and lock-free channels and maps. Custom fiber systems are possible with `corosensei`, but you would build them yourself.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Notes | Source |
|---|---|---|---|---|
| rayon | 1.12.0 (2026-04-14) | 619M / 150M | data-parallel work stealing | [crates.io](https://crates.io/crates/rayon) |
| crossbeam | 0.8.5 (2026-09-05) | 152M | channels, epoch GC, utils | [crates.io](https://crates.io/crates/crossbeam) |
| crossbeam-deque | 0.8.8 (2026-09-05) | 714M | work-stealing deque | [crates.io](https://crates.io/crates/crossbeam-deque) |
| bevy_tasks | 0.20.0 (2026-10-08) | 9.75M | Bevy's task pools (compute/async-compute/IO) | [crates.io](https://crates.io/crates/bevy_tasks) |
| async-executor | 1.14.0 (2026-02-15) | 184M | smol-rs executor used under bevy_tasks | [crates.io](https://crates.io/crates/async-executor) |
| smol | 2.0.2 (2024-09-07) | 26.7M | no release in 2 years (stable, small) | [crates.io](https://crates.io/crates/smol) |
| tokio | 1.54.1 (2026-10-21) | 1.10B | 24 releases in 12 mo | [crates.io](https://crates.io/crates/tokio) |
| corosensei | 0.3.4 (2026-05-24) | 9.1M | stackful coroutines (fiber building block) | [crates.io](https://crates.io/crates/corosensei) |
| parking_lot | 0.13.0 (2026-10-21) | 1.20B | faster Mutex/RwLock | [crates.io](https://crates.io/crates/parking_lot) |
| dashmap | 6.2.1 (2026-05-17) | 424M | concurrent HashMap | [crates.io](https://crates.io/crates/dashmap) |
| flume | 0.12.0 (2025-12-08) | 267M | MPMC channel | [crates.io](https://crates.io/crates/flume) |
| arc-swap | 1.9.3 (2026-10-18) | 374M | RCU-style atomic Arc swap | [crates.io](https://crates.io/crates/arc-swap) |
| kanal | 0.1.1 (2025-03-23) | 11.2M | fast channel, no release in 19 mo | [crates.io](https://crates.io/crates/kanal) |

- Fiber job-system crates found on crates.io are dead: `zing` ("fast, fiber-based job system", 0.1.0, 2021-07-05) and `nylon` (0.1.0, 2021-03-09) — [crates.io search "fiber job"](https://crates.io/search?q=fiber%20job)
- Bevy 0.20's `chain_weak` exists because strict chaining "often leaves worker threads idle while they wait for a handful of stragglers". Bevy's multi-threaded executor runs systems greedily — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- Rapier 0.35.0-beta added a thread-pool API, and `enhanced-determinism` can now be combined with `parallel`, "with results bitwise identical for any thread-pool size" — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)

### Inferences
- **Recommendation:** Use the ECS scheduler (bevy_tasks) for system-level parallelism. Use rayon or `bevy_tasks::ComputeTaskPool` scoped tasks for data-parallel inner loops (animation sampling per character, skinning, broadphase). Use crossbeam or flume channels for streaming and asset I/O, and parking_lot for locks. Avoid running two independent thread pools (rayon plus bevy_tasks) that oversubscribe cores on mobile; size them explicitly, for example big cores only on Android/iOS.
- tokio belongs only in online-services and back-end clients (HTTP, websockets), on its own thread, never in the frame loop. smol/async-executor is the lighter option inside the engine.
- A Naughty-Dog-style fiber job system would be custom work (corosensei plus crossbeam-deque). Given Rust's ownership model and async/await, most Rust engines use task graphs rather than fibers. Treat fibers as an optional R&D item.

### Gaps
- No public Rust game-engine benchmark comparing rayon vs bevy_tasks vs custom job systems on mobile big.LITTLE CPUs was found.
- No information was found on console threading-API constraints (thread affinity, priority) for these crates. That is likely NDA material.

---

## Memory: global allocators, frame/bump allocators, allocator_api status, slotmap/generational arenas

### Takeaway
A major change: the `Allocator` trait MVP (with `Box::new_in` and `Vec::new_in`/`with_capacity_in`) was **merged for stabilization in Rust 1.100**. The current stable is 1.99.1, so 1.100 should be the next release, around Nov 2026. That makes per-frame arena allocators first-class on stable. For now, use `allocator-api2` or `bumpalo`/`bump-scope` collections. Global allocators: mimalloc (most common), jemalloc (non-Windows), snmalloc. Handles: slotmap.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Notes | Source |
|---|---|---|---|---|
| mimalloc | 0.1.52 (2026-05-22) | 66.4M / 21.3M | MIT | [crates.io](https://crates.io/crates/mimalloc) |
| tikv-jemallocator | 0.7.0 (2026-05-25) | 107M / 24.6M | MIT/Apache-2.0 | [crates.io](https://crates.io/crates/tikv-jemallocator) |
| snmalloc-rs | 0.7.6 (2026-10-02) | 9.9M | MIT (Microsoft snmalloc) | [crates.io](https://crates.io/crates/snmalloc-rs) |
| bumpalo | 3.20.3 (2026-05-22) | 668M / 179M | bump arena, own collections | [crates.io](https://crates.io/crates/bumpalo) |
| bump-scope | 2.4.1 (2026-10-19) | 685k / 423k | scoped bump allocator, 16 releases in 12 mo | [crates.io](https://crates.io/crates/bump-scope) |
| allocator-api2 | 0.4.0 (2025-12-11) | 555M | stable polyfill of allocator_api | [crates.io](https://crates.io/crates/allocator-api2) |
| slotmap | 1.1.1 (2025-12-06) | 121M / 32.6M | Zlib; generational keys | [crates.io](https://crates.io/crates/slotmap) |
| slab | 0.4.12 (2026-01-31) | 1.05B | non-generational | [crates.io](https://crates.io/crates/slab) |
| thunderdome | 0.6.1 (2023-06-25) | 1.14M | no release in 3 years | [crates.io](https://crates.io/crates/thunderdome) |
| generational-arena | 0.2.9 (2023-05-22) | 10.5M | **repo archived** | [crates.io](https://crates.io/crates/generational-arena), [GitHub](https://github.com/fitzgen/generational-arena) |

- PR #156882, "alloc: stabilise `Allocator`", was merged 2026-09-23 with milestone **1.100.0**. It stabilizes "a subset of the feature `allocator_api`": the `Allocator` trait (allocate, deallocate, grow, shrink, zeroed variants), `Box<T, A>` (`new_in`, `from_raw_in`, `allocator`), `Vec<T, A>` (`new_in`, `with_capacity_in`, `from_raw_parts_in`, `allocator`), `Global` and `System`, and blanket impls for `&A`, `&mut A`, `Box`, `Rc` and `Arc`. Implementors must not unwind, and `Allocator` will be dyn-compatible. The rest of the feature is renamed `allocator_ext` — [rust-lang/rust#156882](https://github.com/rust-lang/rust/pull/156882), [tracking issue #32838 (closed as completed 2026-09-23)](https://github.com/rust-lang/rust/issues/32838)
- On the tracking issue, a commenter asked whether it ships in 1.100. RalfJung replied "If nothing goes wrong, then yes it will" — [rust-lang/rust#32838](https://github.com/rust-lang/rust/issues/32838)
- Latest stable Rust on 2026-10-24: 1.99.1, released 2026-10-16 — [rust-lang/rust releases](https://github.com/rust-lang/rust/releases)
- bevy_ecs itself depends on `bumpalo` and `slotmap` — [crates.io bevy_ecs 0.20.0 deps](https://crates.io/crates/bevy_ecs/0.20.0)

### Inferences
- **Recommendation:** Use mimalloc as the global allocator on PC and mobile, after benchmarking against the system allocator on iOS. Use bump-scope or bumpalo per-frame and per-job arenas, reset each frame, for transient animation, physics-query and command buffers. Move to std `Vec<T, &Bump>` once 1.100 lands and the arena crates implement the stable trait. Use slotmap for non-ECS resource handles (GPU resources, audio voices, animation instances). Avoid generational-arena (archived) and thunderdome (stale).
- On consoles, the platform SDK usually requires or strongly prefers its own allocator (inference). Design a thin `GlobalAlloc` shim that can route to the platform allocator.
- `Vec`/`Box` with custom allocators covers most frame-allocator needs, but `HashMap` and `String` aren't in the MVP. hashbrown, with its allocator-api2 support, stays useful.

### Gaps
- No public game-workload benchmarks of mimalloc vs jemalloc vs snmalloc on iOS/Android in 2026 were found.
- Which ecosystem crates (bumpalo, hashbrown, bump-scope) will implement the newly stabilized trait, and when, could not be confirmed. 1.100 had not shipped on the research date.

---

## Physics & collision: rapier vs avian vs parry vs Jolt/PhysX bindings; character controllers; combat hit queries

### Takeaway
Rapier (0.36.1, Oct 2026) is the most mature pure-Rust option for a custom engine. It is engine-agnostic, has a glam API since 0.32, optional cross-platform determinism (now parallel-compatible), a kinematic character controller, and broadphase plus solver speedups through 2025–26. Avian (0.8) is excellent but tied to Bevy's ECS. Jolt is the AAA-proven choice (Horizon Forbidden West, Death Stranding 2, console support), but its Rust bindings are young (oxijolt 1.2, jolt-rust). PhysX bindings (Embark) are **archived**.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Releases last 12 mo | License | Status | Source |
|---|---|---|---|---|---|---|
| rapier3d | 0.36.1 (2026-10-10) | 1.92M / 536k | 13 | Apache-2.0 | very active, 5,906★ | [crates.io](https://crates.io/crates/rapier3d), [GitHub](https://github.com/dimforge/rapier) |
| parry3d | 0.31.1 (2026-09-18) | 2.99M / 888k | 14 | Apache-2.0 | very active | [crates.io](https://crates.io/crates/parry3d) |
| avian3d | 0.8.0 (2026-10-20) | 438k / 148k | 7 | MIT OR Apache-2.0 | very active, 3,260★ | [crates.io](https://crates.io/crates/avian3d), [GitHub](https://github.com/avianphysics/avian) |
| bevy_rapier3d | 0.37.0 (2026-10-10) | 610k | 6 | Apache-2.0 | active | [crates.io](https://crates.io/crates/bevy_rapier3d) |
| oxijolt | 1.2.0+jolt-5.6.0 (2026-10-21) | 2.7k | 7 | MIT OR Apache-2.0 | new, 15★ | [crates.io](https://crates.io/crates/oxijolt), [GitHub](https://github.com/pockerhead/oxijolt) |
| joltc-sys / rolt | 0.3.1+Jolt-5.0.0 (2024-05-19) | 9.7k / 6.9k | 0 (repo pushed 2026-10-21) | MIT OR Apache-2.0 | "early work in progress", 104★ | [crates.io](https://crates.io/crates/rolt), [GitHub](https://github.com/SecondHalfGames/jolt-rust) |
| physx / physx-sys | 0.19.0 / 0.11.5 (2023-10-11) | 184k / 217k | 0 | MIT/Apache (+BSD-3 PhysX) | **archived**, PhysX 4.1 | [crates.io](https://crates.io/crates/physx), [GitHub](https://github.com/EmbarkStudios/physx-rs) |
| bevy-tnua | 0.33.0 (2026-10-15) | 116k | 7 | MIT OR Apache-2.0 | active "floating" character controller | [crates.io](https://crates.io/crates/bevy-tnua) |
| bevy_ahoy | 0.2.0 (2026-08-06) | 3.5k | 10 | MIT OR Apache-2.0 | new KCC for Bevy+Avian | [crates.io](https://crates.io/crates/bevy_ahoy) |

**Rapier (verified changelog and blog claims):**
- 0.27 (Jul 2025) brought a BVH broad-phase. 0.29 (Sep 2025) was a solver rework "making the whole simulation up to 25% faster". 0.32 (Jan 2026) migrated to glam. 0.33 (Jun 2026) added no_std support and C/Python bindings. 0.35 (Aug 2026) added broad-phase speedups: the issue #970 scene "steps ~90x faster while stationary and ~18x faster while moving". It also brought sleep and CCD rewrites and a thread-pool API. 0.36 (Sep 2026) added soft bodies and an experimental FEM solver — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- Determinism: `enhanced-determinism` now combines with `parallel` and gives "results bitwise identical for any thread-pool size, and costs no measurable performance anymore". It "remains incompatible with `simd8`" — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- Native 0.32 is "up to 2x faster than a year before" (vs 0.23). The WASM npm package is 2–5x faster than 2024's. Benchmarks compare only against older Rapier, not PhysX, Jolt or Box2D. Sponsors are Futurewei, Hytopia (most performance work) and Foresight Spatial Labs. 2026 goals are robotics/multibody accuracy and rust-gpu GPU rigid bodies — [Dimforge 2025 review](https://dimforge.com/blog/2026/01/09/the-year-2025-in-dimforge)
- 0.35 changed snap-to-ground for the kinematic character controller to trigger on any non-upward movement — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- Soft-body support blog post (2026-09-25) — [Dimforge blog](https://dimforge.com/blog/2026/09/25/advanced-soft-bodies-for-games-in-the-rapier-physics-engine)

**Avian 0.8 (2026-10-20):**
- Highlights include a CCD rework (Box2D-inspired, avoids ghost collisions), contact recycling across timesteps (a 100-cube stack is stable with 4 substeps), and the f64-positions-only change. Large-scene spawning is 9.4x–30.7x faster (200k static bodies: 19.48 s to 0.634 s), and spatial queries now have debug rendering — [Avian 0.8 release](https://github.com/avianphysics/avian/releases/tag/v0.8.0), [Avian 0.8 blog](https://joonaa.dev/blog/14/avian-0-8)
- SIMD for the contact solver is a stated priority but "not much of it has landed yet". The blog gives no numeric comparison vs Rapier or Jolt and doesn't state cross-platform determinism. Character-controller improvement was listed as a priority but no progress is reported — [Avian 0.8 blog](https://joonaa.dev/blog/14/avian-0-8)
- Avian 0.7 (2026-06-20) was mainly a Bevy 0.19 update. Avian tracks each Bevy release — [Avian releases](https://github.com/avianphysics/avian/releases)

**Jolt (C++) and its Rust bindings:**
- Jolt is "used by Horizon Forbidden West and Death Stranding 2". "The simulation runs deterministically". Supported platforms include Android, iOS and "Platform Blue (a popular game console)", which needs an NDA'd header — [JoltPhysics README](https://github.com/jrouwe/JoltPhysics) (11,763★, pushed 2026-10-23)
- oxijolt: "Safe Rust bindings to Jolt Physics 5.6" with rigid bodies, a character controller, vehicles, ragdolls, soft bodies, constraints and contact events, "built for deterministic simulation". It sits on the `joltc` C wrapper (amerkoleci/joltc) — [oxijolt README](https://github.com/pockerhead/oxijolt)
- jolt-rust (SecondHalfGames) README: "Rust bindings for Jolt Physics 5.3.0 using JoltC … an early work in progress. Watch for exposed nails." `rolt` safety is "best-effort". The crates.io releases (Jolt 5.0) lag the repo — [jolt-rust README](https://github.com/SecondHalfGames/jolt-rust)
- Other small Jolt binding repos exist (cohaereo/jolt-rs, SlimeYummy/jolt-physics-rs, emberlightstudios/jolt_sys), each with 0–7 stars — [GitHub search](https://github.com/search?q=jolt+physics+rust&type=repositories)

**PhysX:** physx-rs is "not officially supported by NVIDIA", targets PhysX 4.1, and the repo is archived — [physx-rs](https://github.com/EmbarkStudios/physx-rs)

**Character controllers:** bevy_ahoy is a kinematic controller for Bevy + Avian + bevy_enhanced_input. It covers stairs, ramps, ground snapping, moving platforms, coyote time, input buffering, mantling and water, with double-jump and ladders unchecked on its roadmap — [bevy_ahoy README](https://github.com/janhohenheim/bevy_ahoy)

### Inferences
- **Recommendation for this game: Rapier (rapier3d, or rapier3d-f64 for open zones) behind a thin engine-side physics abstraction, with parry used directly for combat queries.** Run hitbox/hurtbox overlaps as parry shape-casts or intersection tests against a dedicated query BVH rather than as rigid-body contacts. Rapier is standalone (no Bevy dependency), Apache-2.0, no_std-capable, deterministic if needed for replays or anti-cheat, and very actively maintained by a funded company.
- **Keep Jolt as the "AAA escape hatch".** If console certification, large-world double precision, vehicles or ragdoll quality demand it, wrap Jolt through `joltc` with your own -sys crate (oxijolt is promising but has 15★ and is weeks old). Jolt's own production record is far stronger than any Rust engine's.
- Avian only makes sense if you adopt Bevy's app and schedule wholesale. Its large-world approach (f64 positions only) is a good design to copy.
- Anime action games mostly use kinematic characters. The gameplay character controller (dash, air combos, lock-on, root-motion-driven moves, wall/ledge logic) is usually custom, built on shape-casts. Rapier's KCC or bevy_ahoy are references rather than drop-ins.
- Do not rely on Rapier soft bodies or FEM for hair or cloth. They're too new and too heavy for dozens of characters (see Animation).

### Gaps
- No independent 2025–26 benchmark of Rapier vs Jolt vs Avian vs PhysX was found. Dimforge and Avian benchmark only against their own older versions.
- No shipped commercial 3D game using Rapier or Avian was verified in this research.
- Console support status (PS5/Xbox/Switch 2) for Rapier/Avian builds isn't documented. no_std support (Rapier 0.33) helps, but there's no evidence of a shipped console title.

---

## Animation: skeletal runtime (ozz-animation-rs, bevy_animation), graphs/state machines, root motion, IK, compression (ACL), motion matching, retargeting, spring bones/cloth

### Takeaway
The Rust animation ecosystem is thin for AAA character action. The best standalone runtime is **ozz-animation-rs**: a full Rust rewrite of ozz's runtime with blending, additive, partial masks, two-bone and aim IK, root-motion blending, SIMD and determinism. It has a single maintainer (47★, MPL-2.0). bevy_animation has graphs, masks, additive blending and events, but no built-in root motion (open issues). There are **no ACL bindings**, motion matching exists only as prototypes, and anime secondary motion (VRM-style spring bones) exists in bevy_vrm1. Expect to build the animation graph, combat state machine, spring bones and cloth yourself.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | License | Status | Source |
|---|---|---|---|---|---|
| ozz-animation-rs | 0.11.0 (2025-10-14) | 16.9k / 749 | **MPL-2.0** | repo pushed 2026-10-23, 47★ | [crates.io](https://crates.io/crates/ozz-animation-rs), [GitHub](https://github.com/SlimeYummy/ozz-animation-rs) |
| bevy_animation | 0.20.0 (2026-10-08) | 5.66M / 1.24M | MIT OR Apache-2.0 | Bevy core | [crates.io](https://crates.io/crates/bevy_animation) |
| bevy_animation_graph | 0.11.0 (2026-07-11) | 11k | MIT OR Apache-2.0 | 208★, has editor | [crates.io](https://crates.io/crates/bevy_animation_graph), [GitHub](https://github.com/mbrea-c/bevy_animation_graph) |
| bevy_mod_inverse_kinematics | 0.12.0 (2026-09-01) | 15.6k | MIT OR Apache-2.0 | 175★ | [crates.io](https://crates.io/crates/bevy_mod_inverse_kinematics) |
| k | 0.32.0 (2024-09-10) | 302k | Apache-2.0 | robotics kinematics (openrr) | [crates.io](https://crates.io/crates/k) |
| bevy_vrm1 | 0.10.0 (2026-10-03) | 7.4k | MIT OR Apache-2.0 | 18 releases in 12 mo | [crates.io](https://crates.io/crates/bevy_vrm1), [GitHub](https://github.com/not-elm/bevy_vrm1) |
| bevy_vrm | 0.5.0 (2026-10-13) | 20k | MIT OR Apache-2.0 | | [crates.io](https://crates.io/crates/bevy_vrm) |
| bevy_skeleton_modifiers | 0.1.1 (2026-10-09) | 33 | MIT | brand new: "Godot-derived spring bones and IK" | [crates.io](https://crates.io/crates/bevy_skeleton_modifiers) |
| vrm-runtime | 0.1.0 (2026-08-18) | 91 | — | "Renderer-agnostic VRM 1.0 loader, validator, animation system, and runtime" | [crates.io](https://crates.io/crates/vrm-runtime) |
| bevy_silk | 0.10.0 (2025-12-08) | 12.8k | — | cloth for Bevy | [crates.io](https://crates.io/crates/bevy_silk) |
| rapier-cloth | 0.3.0 (2026-09-30) | 25 | MIT | "CPU XPBD cloth simulation with Rapier 3D integration" | [crates.io](https://crates.io/crates/rapier-cloth) |

**ozz-animation-rs:**
- It is "a rust version skeletal animation library with cross-platform deterministic". It "does not simply wrap ozz-animation's runtime, but rewrite[s] the full runtime library in rust". Features: playback, joint attachment, partial/additive blending, two-bone IK, aim (look-at) IK, user channels, root motion (motion blending), skinning, multi-threading, SIMD (SSE2 + NEON), WASM, and rkyv/serde serialization. Offline features (the importer and compression tools) are not ported, so you use the C++ ozz toolset — [ozz-animation-rs README](https://github.com/SlimeYummy/ozz-animation-rs)
- Upstream C++ ozz-animation: 2,975★, last push 2026-08-01 — [GitHub](https://github.com/guillaumeblanc/ozz-animation)
- The same author (SlimeYummy) also maintains `recastnavigation-rs` and a `jolt-physics-rs` wrapper, which suggests an action-game stack built in Rust — [recastnavigation-rs](https://github.com/SlimeYummy/recastnavigation-rs), [jolt-physics-rs](https://github.com/SlimeYummy/jolt-physics-rs)

**bevy_animation:**
- Bevy 0.15 added "generalized entity animation, animation masks, additive blending, and animation events" — [Bevy 0.15](https://bevy.org/news/bevy-0-15/)
- Bevy 0.19 made skinned-mesh culling account for animation ("animated characters … would sometimes vanish mid-animation") — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)
- Root motion is **not built in**. Issues #24201 "Root motion implementation" (open, updated 2026-08-31) and #23355 "Enable robust root motion for character animations" (open) — [bevy#24201](https://github.com/bevyengine/bevy/issues/24201), [bevy#23355](https://github.com/bevyengine/bevy/issues/23355)

**Compression, motion matching, secondary motion:**
- ACL (Animation Compression Library, C++): 1,599★, last push 2025-11-26 — [GitHub](https://github.com/nfrechette/acl). **No Rust bindings found.** The crates.io `acl-sys` crate is unrelated: it's POSIX file ACLs, LGPL-2.1, last released 2020 — [crates.io acl-sys](https://crates.io/crates/acl-sys)
- Motion matching: `voxell-tech/bevy_motion_matching` is a Bevy prototype (61★, last push 2025-12-28, not on crates.io). No other Rust motion-matching library was found — [GitHub](https://github.com/voxell-tech/bevy_motion_matching)
- bevy_vrm1 implements VRM 1.0 Spring Bone and all three Node Constraint types (rotation, roll, aim) from the VRMC specs — [bevy_vrm1 README](https://github.com/not-elm/bevy_vrm1), [VRMC_springBone-1.0 spec](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_springBone-1.0/README.md)

### Inferences
- **Recommendation:** Build the animation runtime on ozz-animation-rs, vendored or forked because of the bus factor, with ozz's C++ offline toolchain or your own glTF/FBX converter for compression. Write the anime-specific layers yourself:
  - a data-driven animation graph and state machine with combat-specific features (cancel windows, hit-stop, animation-driven hitbox events, motion warping toward the target);
  - root-motion extraction and warping;
  - a VRM-style spring-bone solver (verlet chains with sphere/capsule colliders) for hair, accessories and skirts, run per character in parallel SoA;
  - an XPBD cloth or skirt solver with capsule collisions for long garments.
- Port the VRM spring-bone algorithm (well specified and widely used for anime avatars) rather than depend on bevy_vrm1. Its spec and implementation are good references.
- **MPL-2.0 caution:** it is file-level copyleft. Modifications to ozz-animation-rs files must stay open, which is fine for most studios but needs legal sign-off.
- ACL-quality compression would require writing a `-sys` crate over ACL's C++ API, which is header-only C++ so it needs a C shim, or relying on ozz's own keyframe-reduction compression. For a gacha game with 50+ playable characters on mobile, animation memory matters. Budget for this.
- Motion matching is unnecessary for ZZZ/WuWa-style hand-authored combat (state machines plus authored transitions dominate). Treat it as optional for locomotion only.

### Gaps
- No Rust retargeting library (humanoid retargeting à la Unity Mecanim) was found, apart from VRM humanoid mapping in bevy_vrm/bevy_vrm1.
- No performance benchmarks of ozz-animation-rs vs C++ ozz were found in the README.
- No Rust equivalent of Magica Cloth / Unity Cloth with mobile performance data was found. rapier-cloth and bevy_skeleton_modifiers are essentially unproven (25 and 33 downloads).

---

## Audio: kira, Firewheel, rodio/cpal, oddio, fyrox-sound, Steam Audio; FMOD/Wwise bindings

### Takeaway
For a commercial cross-platform game with console targets, FMOD Studio is the realistic middleware. Rust bindings exist (fmod-oxide, beta, Jul 2026; libfmod; bevy_fmod) but none is officially supported or console-verified. **Wwise has no maintained Rust bindings** (rrise was last released in 2022). Among pure-Rust engines, kira is the most game-oriented and mature. Firewheel is the rising audio-graph engine planned to become Bevy's default (Bevy still uses rodio). Steam Audio is available via audionimbus.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Releases last 12 mo | License | Status | Source |
|---|---|---|---|---|---|---|
| kira | 0.12.5 (2026-09-26) | 1.06M / 228k | 7 | MIT OR Apache-2.0 | active, 1,073★ | [crates.io](https://crates.io/crates/kira), [GitHub](https://github.com/tesselode/kira) |
| firewheel | 0.14.0 (2026-09-05) | 62k / 23k | 11 | MIT OR Apache-2.0 | active, 318★ | [crates.io](https://crates.io/crates/firewheel), [GitHub](https://github.com/BillyDM/Firewheel) |
| bevy_seedling | 0.8.0 (2026-08-09) | 34k | 6 | MIT OR Apache-2.0 | Firewheel-for-Bevy | [crates.io](https://crates.io/crates/bevy_seedling) |
| rodio | 0.23.0 (2026-10-11) | 13.1M / 3.7M | 4 | MIT OR Apache-2.0 | active | [crates.io](https://crates.io/crates/rodio) |
| cpal | 0.18.2 (2026-08-16) | 25.7M / 9.1M | 7 | Apache-2.0 | active (device I/O backend) | [crates.io](https://crates.io/crates/cpal) |
| oddio | 0.7.4 (2023-10-15) | 61k | 0 | MIT OR Apache-2.0 | **dormant** (last push 2023-10) | [crates.io](https://crates.io/crates/oddio), [GitHub](https://github.com/Ralith/oddio) |
| fyrox-sound | 1.0.1 (2026-03-28) | 83k | 3 | MIT | part of Fyrox engine | [crates.io](https://crates.io/crates/fyrox-sound) |
| fundsp | 0.23.0 (2026-01-07) | 242k | 3 | MIT OR Apache-2.0 | DSP graph | [crates.io](https://crates.io/crates/fundsp) |
| audionimbus | 0.16.0 (2026-08-08); -sys 4.8.1 | 16.6k | 7 | MIT OR Apache-2.0 | active, 212★ | [crates.io](https://crates.io/crates/audionimbus), [GitHub](https://github.com/MaxenceMaire/audionimbus) |
| fmod-oxide | 0.2.2 (2026-07-14) | 7.9k | 1 | **MPL-2.0** | beta | [crates.io](https://crates.io/crates/fmod-oxide), [GitHub](https://github.com/melody-rs/fmod-oxide) |
| libfmod | 2.222.6 (2024-10-15) | 51.6k | 0 | MIT | last push 2024-10 | [crates.io](https://crates.io/crates/libfmod), [GitHub](https://github.com/lebedec/libfmod) |
| fmod-core-sys / fmod-studio-sys | 1.0.2 (2026-03-05) | ~1.7k | — | — | raw bindings | [crates.io](https://crates.io/crates/fmod-studio-sys) |
| bevy_fmod | 0.10.0 (2026-01-15) | 10.4k | 1 | MIT OR Apache-2.0 | Bevy integration | [crates.io](https://crates.io/crates/bevy_fmod) |
| rrise (Wwise) | 0.2.3 (2022-11-17) | 15k / 44 | 0 | non-standard | **abandoned** (last push 2023-01) | [crates.io](https://crates.io/crates/rrise), [GitHub](https://github.com/dtaralla/rrise) |
| waapi-rs | 0.3.0 (2026-08-15) | 233 | — | — | Wwise *Authoring* API client only (not runtime) | [crates.io](https://crates.io/crates/waapi-rs) |

- Kira provides "tweens for smoothly adjusting properties of sounds, a flexible mixer for applying effects to audio, a clock system for precisely timing audio events, and spatial audio support". It is backend-agnostic — [kira README](https://github.com/tesselode/kira)
- Firewheel "is currently planned to be upstreamed into the Bevy game engine where it will become the default audio engine". The core stays usable outside Bevy, depending only on the light `bevy_platform`. Backends cover Windows, Mac, Linux, Android, iOS and WebAssembly. It supports any DAG audio graph, an ECS-friendly parameter API, silence optimizations, fault tolerance for device changes, realtime-safe operation ("no mutexes!") and `no_std` — [Firewheel README](https://github.com/BillyDM/Firewheel)
- Bevy 0.19's settings example still uses `ResMut<AudioSink>` (the rodio-based bevy_audio). No release note through 0.20 announces the Firewheel upstreaming — [Bevy 0.19](https://bevy.org/news/bevy-0-19/), [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- fmod-oxide is "Safe rust bindings to the FMOD sound engine … Currently in BETA". It supports Windows/Linux/macOS/HTML5, and "most of the real world testing … comes from one source (me) and only on a Linux system". The crate "can't bundle FMOD" due to licensing; developers download the FMOD Engine themselves — [fmod-oxide README](https://github.com/melody-rs/fmod-oxide)

### Inferences
- **Recommendation: FMOD Studio for shipping.** Anime action games need heavy adaptive music (combat intensity layers, boss phase transitions), thousands of voice lines in multiple languages, and console and mobile certification support. FMOD's authoring tool and platform coverage beat pure-Rust options. Generate your own `-sys` bindings with bindgen against the FMOD version you license, plus a thin safe layer. fmod-oxide's API is a good model, but it is single-maintainer beta and MPL-2.0.
- Avoid Wwise unless the audio team insists. You would own the entire binding (Wwise SDK is C++ with platform-specific builds), and there's no maintained Rust community work.
- **Pure-Rust fallback** (prototype, indie or license-free builds): kira for gameplay SFX and music timing. Firewheel if you want a node graph and alignment with Bevy's future audio. audionimbus (Steam Audio) for HRTF and occlusion if spatial quality matters.
- Never put the audio callback on the engine's job threads. Both Firewheel and kira are built for realtime-safe callbacks.

### Gaps
- Console support status of any Rust FMOD binding is undocumented. FMOD's console libraries are NDA'd, so expect to do this integration in-house.
- FMOD/Wwise license terms and costs were not researched here. Check the vendors directly.
- No shipped commercial games using kira or Firewheel were verified.

---

## Scripting & hot reload: mlua/Luau, rhai, rune, WASM runtimes, piccolo, JS engines, koto; Rust code hot reload (subsecond, hot-lib-reloader, Bevy hotpatching); live-service hot-patching

### Takeaway
For live-service content and bug hotfixes on iOS/Android, the proven pattern is an **interpreted VM whose bytecode or script is downloaded with content updates**: Lua via xLua, C# IL via HybridCLR, TS via PuerTS. In Rust, the strongest equivalent is **mlua with Luau** (sandboxed, typed, used at Roblox scale). Rust-native hot reload (subsecond, which Bevy integrated in 0.17) is a **development-only** tool. It is experimental and not supported on iOS devices.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Releases last 12 mo | License | Notes | Source |
|---|---|---|---|---|---|---|
| mlua | 0.12.2 (2026-10-03) | 8.1M / 2.4M | 7 | MIT | Lua 5.1–5.5, LuaJIT, Luau; 2,918★ | [crates.io](https://crates.io/crates/mlua), [GitHub](https://github.com/mlua-rs/mlua) |
| rhai | 1.27.0 (2026-10-21) | 13.4M / 4.4M | 8 | MIT OR Apache-2.0 | Rust-native embedded script | [crates.io](https://crates.io/crates/rhai) |
| rune | 0.14.2 (2026-05-22) | 182k | 1 | MIT OR Apache-2.0 | | [crates.io](https://crates.io/crates/rune) |
| koto | 0.16.1 (2026-01-05) | 34k | 1 | MIT | | [crates.io](https://crates.io/crates/koto) |
| piccolo | 0.3.3 (2024-06-16) | 96k | 0 (repo pushed 2025-07) | MIT | stackless Lua VM, experimental | [crates.io](https://crates.io/crates/piccolo), [GitHub](https://github.com/kyren/piccolo) |
| steel-core | 0.8.3 (2026-08-20) | 52k | 4 | MIT OR Apache-2.0 | Scheme | [crates.io](https://crates.io/crates/steel-core) |
| wasmtime | 50.0.0 (2026-10-20) | 41.4M / 11.8M | 81 | Apache-2.0 WITH LLVM-exception | Cranelift JIT + Pulley interpreter | [crates.io](https://crates.io/crates/wasmtime) |
| wasmi | 2.0.0 (2026-09-01) | 25.8M / 5.6M | 31 | MIT/Apache-2.0 | pure interpreter | [crates.io](https://crates.io/crates/wasmi) |
| wasmer | 7.5.0 (2026-10-01) | 8.8M | 24 | MIT | | [crates.io](https://crates.io/crates/wasmer) |
| deno_core | 0.413.0 (2026-10-21) | 9.2M | 52 | MIT | V8, heavy | [crates.io](https://crates.io/crates/deno_core) |
| rquickjs | 0.14.0 (2026-09-18) | 5.9M | 7 | MIT | QuickJS | [crates.io](https://crates.io/crates/rquickjs) |
| boa_engine | 0.22.0 (2026-08-28) | 5.8M | 2 | Unlicense OR MIT | pure-Rust JS | [crates.io](https://crates.io/crates/boa_engine) |
| bevy_mod_scripting | 0.23.0 (2026-10-14) | 49k | 9 | MIT OR Apache-2.0 | Lua/Rhai for Bevy, 656★ | [crates.io](https://crates.io/crates/bevy_mod_scripting) |
| subsecond | 0.7.10 (2026-07-30); 0.8.0-alpha.3 (2026-10-16) | 2.6M / 1.28M | 15 | MIT OR Apache-2.0 | Dioxus hot-patching | [crates.io](https://crates.io/crates/subsecond) |
| hot-lib-reloader | 0.8.2 (2025-08-11) | 62k | 0 | MIT | dylib reloading | [crates.io](https://crates.io/crates/hot-lib-reloader) |
| bevy_simple_subsecond_system | 0.2.0 (2025-06-02) | 13k | 0 | MIT OR Apache-2.0 | superseded by upstream Bevy hotpatching | [crates.io](https://crates.io/crates/bevy_simple_subsecond_system) |

**Embedding languages:**
- mlua supports "Lua 5.5, 5.4, 5.3, 5.2, 5.1 (including LuaJIT) and Luau". Features include `luau-jit` (Luau with JIT backend), `luau-vector4`, `async` (works for all Lua versions including Luau, via coroutines), `send` and `serde`. WASM is supported via `wasm32-unknown-emscripten`, excluding JIT. CI tests Windows/macOS/Linux on x86_64 plus aarch64 cross-compile — [mlua README](https://github.com/mlua-rs/mlua)
- Luau "is used by Roblox game developers to write game code, and by Roblox engineers to implement large parts of the user-facing application code". It provides type annotations and type inference. Sandboxing (`luaL_sandbox`, `safeenv`) is recommended for performance and safety — [Luau README](https://github.com/luau-lang/luau)
- Rhai's AST walker runs "1 million iterations in 0.14 sec on a single-core 2.6 GHz Linux VM". The optional "Rhai Grain" bytecode compiler and VM is "around 1.8-2x" faster. Scripts are sandboxed, and WASM is supported — [Rhai README](https://github.com/rhaiscript/rhai)
- Wasmtime's Pulley is a portable bytecode interpreter enabled by default on architectures without Cranelift support and opt-in elsewhere via the `pulley` feature — [Wasmtime docs: Using Pulley](https://docs.wasmtime.dev/examples-pulley.html). On iOS you "can't generate executable code pages in runtime", so JIT engines are unavailable there — [Wasm3 README mirror](https://gitee.com/txiki/wasm3). A Rust forum user reports using the Wasm3 interpreter to run WASM on iOS for this reason — [Rust users forum thread](https://users.rust-lang.org/t/do-android-ios-apps-have-access-to-wasm-jit/66957)

**Rust hot reload:**
- Bevy 0.17 added "Hot Patching Systems in a Running App" via subsecond and the `dx` CLI (feature `hotpatching`). Known limitations: binary crate only, no WASM, changed system parameters don't reload, and it is sensitive to linker configuration — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- subsecond "works by detouring function calls through a jump table". Platforms are Android, iOS (simulator only, "iOS devices are not yet supported" due to code signing), Linux, macOS, Windows and WASM. It "does not support hot-reloading of structs", only the "tip" crate is patched, thread-locals may reset, it is enabled only with debug assertions, and "Subsecond is still experimental" — [subsecond docs](https://docs.rs/subsecond/latest/subsecond/)

**Live-service hot-patching practice (mainly Unity-based Chinese mobile games):**
- Tencent xLua offers "使用 Lua 逻辑替换 C# 方法逻辑的方案" (replacing C# method logic with Lua). It can swap C# methods, operators, properties and events for Lua at runtime, and "supports android, ios, windows, linux, osx" — [xLua](https://github.com/Tencent/xLua)
- HybridCLR (8,001★, pushed 2026-10-18) turns IL2CPP into an AOT+interpreter hybrid "on Android … IOS, Consoles, WebGL". Its README says "thousands of commercial game projects" use it, over a thousand are live on the App Store and Google Play, and "nearly 100 of the iOS free top-500" use it. It also offers "Differential Hybrid Execution" (unchanged functions run AOT, changed ones are interpreted). Note these are vendor claims — [HybridCLR README](https://github.com/focus-creative-games/hybridclr)
- PuerTS (Tencent) provides TypeScript/JavaScript for Unity and Unreal. A third-party guide calls its hot reload "an editor iteration aid, not a production update guarantee" — [PuerTS search result / Seeles guide](https://www.seeles.ai/resources/blogs/puerts-hot-reload-debugging-unreal)
- Apple Guideline 2.5.2: apps may not "download, install, or execute code which introduces or changes features or functionality of the app". Guideline 4.7 carves out HTML5/JavaScript mini apps and mini games, streaming games, chatbots and plug-ins, subject to 4.7.1–4.7.5 — [App Store Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)

### Inferences
- **Recommendation:**
  1. **Luau via mlua** as the gameplay scripting and hotfix layer: skill and ability scripts, quest and event logic, UI flows, and server-pushed fixes. It's interpreted, so it's legal to run on iOS. It's sandboxed, has gradual typing for large teams, offers coroutine-based async that suits cutscenes and timelines, and supports consoles because it is plain C++. Don't use `luau-jit` on iOS, which forbids JIT pages (inference from the iOS JIT constraint).
  2. **Rust for engine and core combat simulation**, shipped via app-store binary updates.
  3. **subsecond/Bevy hotpatching for developer iteration only.**
  4. Optionally **WASM (wasmtime + Pulley, or wasmi)** for sandboxed modding or PC-only features. Interpreted WASM is slower than Luau's tuned interpreter for small, chatty gameplay calls (inference; no benchmark found).
- The hot-update practice in Chinese gacha games (xLua, HybridCLR, PuerTS) is a de facto industry norm despite the strict literal reading of 2.5.2. Design the content pipeline so data (tables, timelines, animation graphs, ability definitions) and scripts are hot-updatable assets, and keep native code changes to app-version releases. Get legal and App Review guidance before relying on code hot-patching on iOS.
- Rhai is pleasant for designer tunables and config expressions, but AST-walker performance makes it a poor choice for per-frame gameplay logic over hundreds of entities. rune, koto and piccolo lack production track records. JS engines (deno_core/V8, rquickjs) only make sense if the studio is TypeScript-first, as with PuerTS-style workflows. rquickjs is the embeddable choice, and V8 is too heavy for mobile.

### Gaps
- Which scripting and hot-update stack Wuthering Waves (UE4-based, per press reports) or ZZZ (Unity) actually use could not be confirmed from primary sources. Searches found no developer talk naming PuerTS, xLua or HybridCLR for these titles.
- No 2026 benchmark comparing Luau (mlua), Rhai, wasmi and Pulley for game-style call patterns was found.
- Console certification rules on downloaded or interpreted script code weren't researched (NDA material).

---

## AI & navigation: navmesh generation/pathfinding, behavior trees, utility AI, GOAP, state machines

### Takeaway
Navigation has improved: **rerecast** (a pure-Rust port of Recast, Aug 2026) replaces the now-deprecated oxidized_navigation. **landmass** covers pathfinding, steering and local avoidance, and **polyanya/vleue_navigator** offer any-angle navmesh search. Decision-making crates are small. bonsai-bt (behavior trees) and statig (hierarchical state machines) are active and engine-agnostic. big-brain (utility AI) has been unreleased since Nov 2024 and moved to Codeberg. GOAP crates are hobby-scale.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Status | Source |
|---|---|---|---|---|
| rerecast | 0.4.0 (2026-08-05) | 56k / 27.5k | "Rust port of Recast, the industry-standard navigation mesh generator", 172★ | [crates.io](https://crates.io/crates/rerecast), [GitHub](https://github.com/janhohenheim/rerecast) |
| bevy_rerecast / avian_rerecast | 0.5.0 / 0.6.0 (2026-08-05) | 19k / 10k | Bevy and Avian integration, has editor | [crates.io](https://crates.io/crates/bevy_rerecast) |
| oxidized_navigation | 0.12.0 (2024-12-25) | 57k | **archived; README says "Depricated! See Rerecast"** | [GitHub](https://github.com/TheGrimsey/oxidized_navigation) |
| recastnavigation-sys | 1.0.3 (2024-01-11) | 37k | C++ Recast/Detour bindings, no release in 21 mo | [crates.io](https://crates.io/crates/recastnavigation-sys) |
| recastnavigation-rs | 0.1.0 (2024-03-23) | 2.6k | MPL-2.0, SlimeYummy | [crates.io](https://crates.io/crates/recastnavigation-rs) |
| landmass / bevy_landmass | 0.9.3 (2026-10-11) / 0.12.1 (2026-09-30) | 33k / 27k | active, 116★ | [crates.io](https://crates.io/crates/landmass), [GitHub](https://github.com/andriyDev/landmass) |
| polyanya | 0.17.1 (2026-08-25) | 101k | active, 522★ | [crates.io](https://crates.io/crates/polyanya), [GitHub](https://github.com/vleue/polyanya) |
| vleue_navigator | 0.16.0 (2026-08-25) | 66k | Bevy navmesh pathfinding, 499★ | [crates.io](https://crates.io/crates/vleue_navigator) |
| pathfinding | 4.16.0 (2026-09-07) | 3.1M | generic A*, Dijkstra and others | [crates.io](https://crates.io/crates/pathfinding) |
| bonsai-bt | 0.14.0 (2026-09-22) | 114k / 28k | active, 1,086★, MIT | [crates.io](https://crates.io/crates/bonsai-bt), [GitHub](https://github.com/Sollimann/bonsai) |
| bevy_behave | 0.5.0 (2026-02-06) | 12k | Bevy behavior trees, 131★ | [crates.io](https://crates.io/crates/bevy_behave) |
| behavior-tree | 0.1.0 (2021-10-30) | 12.5k | dead | [crates.io](https://crates.io/crates/behavior-tree) |
| big-brain | 0.22.0 (2024-11-30) | 65k | GitHub archived, moved to Codeberg; last Codeberg commit 2025-10-07 (Bevy 0.17 port, unpublished) | [GitHub](https://github.com/zkat/big-brain), [Codeberg](https://codeberg.org/zkat/big-brain) |
| bevy_observed_utility | 0.2.0 (2025-03-05) | 2k | stale | [crates.io](https://crates.io/crates/bevy_observed_utility) |
| dogoap | 0.5.0 (2025-05-21) | 7k | GOAP, no release in 17 mo | [crates.io](https://crates.io/crates/dogoap) |
| statig | 0.4.1 (2025-07-05) | 4.7M | hierarchical state machines, 808★ | [crates.io](https://crates.io/crates/statig), [GitHub](https://github.com/mdeloof/statig) |
| seldom_state | 0.16.0 (2026-04-02) | 40k | Bevy component state machines | [crates.io](https://crates.io/crates/seldom_state) |
| rust-fsm | 0.8.0 (2025-07-21) | 1.47M | DSL FSM | [crates.io](https://crates.io/crates/rust-fsm) |

- landmass covers "Path finding (e.g., A-star)", "Path simplification (e.g., SSFA)", "Steering (e.g., boids)" and "Local collision avoidance". It aims to work "easily with other languages", and ships `landmass_rerecast` integration — [landmass README](https://github.com/andriyDev/landmass)
- bonsai offers Sequence, WhenAll/WhenAny/Race parallel combinators and more, describing itself as "behavior trees for deterministic AI (now with Python bindings)" — [bonsai README](https://github.com/Sollimann/bonsai)
- statig: "Hierarchical state machines", "Compatible with `#![no_std]` … no heap memory allocations", and "Support for async actions and handlers" — [statig README](https://github.com/mdeloof/statig)
- big-brain: "This project is no longer developed on GitHub. All development has moved to big-brain on Codeberg" — [big-brain GitHub](https://github.com/zkat/big-brain). Latest Codeberg commits are dated 2025-10-06/07 — [Codeberg API](https://codeberg.org/zkat/big-brain)

### Inferences
- **Recommendation:** Use rerecast for offline and runtime navmesh baking (tiles per zone, streamed). Use landmass for agent movement and avoidance, or Detour-style crowd logic ported or written in-house. Treat the navmesh as an asset produced by the editor.
- For enemy AI in a ZZZ/WuWa-style brawler, the industry pattern is HFSM or behavior trees per enemy, plus an **encounter/combat director** that hands out attack tokens and handles telegraphing and spacing. Use statig (no_std, hierarchical, allocation-free) or a custom data-driven HFSM authored in the editor or Luau, with bonsai-bt as an optional BT core. Build utility scoring in-house (it's simple) rather than depend on big-brain.
- Prefer engine-agnostic crates (rerecast core, landmass, polyanya, bonsai-bt, statig) over Bevy-coupled ones if the engine doesn't adopt Bevy's app layer.

### Gaps
- No production game was found using rerecast or landmass. rerecast's parity with C++ Recast output was not independently verified.
- No Rust Detour-crowd equivalent with documented performance at hundreds of agents was found.

---

## Networking (lower priority): renet, lightyear, quinn, matchbox, laminar, steamworks-rs, backend SDKs

### Takeaway
For optional co-op and online services, quinn (QUIC) and renet/renet2 are solid engine-agnostic transports. lightyear and bevy_replicon are the most complete game-replication stacks but are Bevy-coupled. laminar is dead. steamworks-rs is maintained.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Status | Source |
|---|---|---|---|---|
| quinn | 0.11.12 (2026-09-14) | 373M / 133M | QUIC, very widely used | [crates.io](https://crates.io/crates/quinn) |
| renet | 2.0.0 (2026-01-20) | 193k | engine-agnostic client/server + netcode auth, 954★ | [crates.io](https://crates.io/crates/renet), [GitHub](https://github.com/lucaspoffo/renet) |
| renet2 | 1.0.0 (2026-10-16) | 23k | fork of renet, 14 releases in 12 mo | [crates.io](https://crates.io/crates/renet2) |
| lightyear | 0.31.0 (2026-10-21) | 146k / 40k | "networking library … for the Bevy game engine", 1,195★ | [crates.io](https://crates.io/crates/lightyear), [GitHub](https://github.com/cBournhonesque/lightyear) |
| bevy_replicon | 0.45.2 (2026-10-15) | 227k / 68k | 33 releases in 12 mo | [crates.io](https://crates.io/crates/bevy_replicon) |
| aeronet | 0.21.0 (2026-06-24); 0.22.0-rc.1 | 30k | Bevy transport layer | [crates.io](https://crates.io/crates/aeronet) |
| naia-shared | 0.25.0 (2026-05-12) | 66k | | [crates.io](https://crates.io/crates/naia-shared) |
| matchbox_socket | 0.14.0 (2026-02-13) | 93k | WebRTC P2P | [crates.io](https://crates.io/crates/matchbox_socket) |
| ggrs | 0.13.0 (2026-06-26) | 143k | GGPO-style rollback | [crates.io](https://crates.io/crates/ggrs) |
| laminar | 0.5.0 (2021-05-06) | 407k | **dead** (last push 2023-10) | [crates.io](https://crates.io/crates/laminar), [GitHub](https://github.com/TimonPost/laminar) |
| steamworks | 0.13.1 (2026-05-05) | 352k / 135k | Steamworks SDK bindings | [crates.io](https://crates.io/crates/steamworks) |
| tokio | 1.54.1 (2026-10-21) | 1.10B | async runtime for service clients | [crates.io](https://crates.io/crates/tokio) |

### Inferences
- **Recommendation:** WuWa-style co-op (small drop-in parties) and gacha back-end services (login, gacha pulls, inventory, mail, events) are server-authoritative HTTP/gRPC plus a small realtime channel. Use tokio with reqwest/tonic (not researched here) for services, and quinn or renet2 for the realtime co-op channel with your own replication. lightyear is the best reference implementation for prediction and interpolation, even if you don't adopt it.
- Avoid laminar. GGRS rollback isn't a fit for 3D action co-op with many entities (inference).

### Gaps
- Back-end SDK clients (Epic Online Services, PlayFab, console network SDKs, Chinese platform SDKs) in Rust were not researched. No crate data was gathered.
- No shipped-game evidence for lightyear or renet2 was gathered.

---

## Serialization & data / reflection: serde, rkyv, bincode/postcard, ron, toml; bevy_reflect, facet

### Takeaway
serde is universal. rkyv (0.8) is the zero-copy choice for cooked runtime assets. **bincode is dead**: its final 3.0.0 release is a `compile_error!` tombstone. Use postcard, bitcode or wincode, or stay pinned on bincode 2.0.1. For editor and save-game reflection, bevy_reflect is the most mature dynamic reflection in Rust and works standalone. facet is a fast-moving newer alternative.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Status | Source |
|---|---|---|---|---|
| serde | 1.0.229 (2026-07-18) | 1.58B | MIT OR Apache-2.0 | [crates.io](https://crates.io/crates/serde) |
| rkyv | 0.8.18 (2026-08-05) | 176M / 42M | zero-copy, MIT | [crates.io](https://crates.io/crates/rkyv) |
| bincode | 3.0.0 (2025-12-16) | 354M | **tombstone: unmaintained, repo archived** | [crates.io](https://crates.io/crates/bincode), [GitHub](https://github.com/bincode-org/bincode) |
| postcard | 1.1.3 (2025-07-24) | 80M / 33M | compact, no_std | [crates.io](https://crates.io/crates/postcard) |
| bitcode | 0.6.9 (2025-12-18) | 11.3M | compact binary | [crates.io](https://crates.io/crates/bitcode) |
| speedy | 0.8.7 (2024-01-14) | 1.2M | stale | [crates.io](https://crates.io/crates/speedy) |
| flatbuffers | 25.12.19 (2025-12-19) | 116M | schema-based zero-copy | [crates.io](https://crates.io/crates/flatbuffers) |
| ron | 0.12.2 (2026-06-22) | 123M | human-readable Rust notation | [crates.io](https://crates.io/crates/ron) |
| toml | 1.1.8+spec-1.1.0 (2026-10-09) | 1.05B | config | [crates.io](https://crates.io/crates/toml) |
| savefile | 0.20.6 (2026-10-19) | 200k | versioned save files | [crates.io](https://crates.io/crates/savefile) |
| bevy_reflect | 0.20.0 (2026-10-08) | 10.1M / 2.4M | Bevy reflection | [crates.io](https://crates.io/crates/bevy_reflect) |
| facet | 0.46.5 stable (2026-05-26); 0.50.0-rc.7 (2026-08-20) | 715k / 190k | 44 releases in 12 mo, 2,585★ | [crates.io](https://crates.io/crates/facet), [GitHub](https://github.com/facet-rs/facet) |

- bincode 3.0.0's README: "Bincode is now unmaintained. Due to a doxxing and harassment incident, development on bincode has ceased. No further releases will be published on crates.io". Its `lib.rs` contains only `compile_error!`. It recommends alternatives including `wincode` ("Bincode-compatible alternative") — [bincode 3.0.0 crate contents on crates.io](https://crates.io/crates/bincode/3.0.0). The GitHub repo is archived (last push 2025-08-15) — [GitHub](https://github.com/bincode-org/bincode)
- facet describes itself as "Rust reflection, serialization, deserialization, pretty printing, etc." — [facet GitHub](https://github.com/facet-rs/facet)
- Bevy 0.17 added "Reflect Auto Registration". bevy_ecs has optional `bevy_reflect`, `reflect_functions` and `serialize` features. Bevy 0.19 added serialization and deserialization of asset handles — [Bevy 0.17](https://bevy.org/news/bevy-0-17/), [Bevy 0.19](https://bevy.org/news/bevy-0-19/), [crates.io bevy_ecs 0.20.0](https://crates.io/crates/bevy_ecs/0.20.0)
- ozz-animation-rs supports rkyv and serde serialization of its runtime data — [ozz-animation-rs README](https://github.com/SlimeYummy/ozz-animation-rs)

### Inferences
- **Recommendation:**
  - rkyv for cooked, streamed runtime data (mmap-able zero-copy for animation clips, navmesh tiles and data tables). Validate on load for downloaded patches.
  - serde with ron or toml for authored config and editor-side text formats.
  - postcard or bitcode for network packets and compact saves.
  - bevy_reflect, if using bevy_ecs, as the property system for the editor inspector, undo/redo, scene serialization and versioned saves. Otherwise evaluate facet, which is very active but still churning: 44 releases in 12 months and a 0.50 RC.
- Remove `bincode` from the dependency tree (transitive deps may still pin 1.x or 2.x). That's a supply-chain hygiene item.
- Gacha games need robust save and patch versioning (server-authoritative inventory, client caches). Schema evolution matters more than raw speed, so keep explicit version tags in all binary formats.

### Gaps
- No 2026 serialization benchmark (rust_serialization_benchmark) results were retrieved.
- facet's stability roadmap and production users were not researched.

---

## Misc gameplay: RNG (incl. deterministic), events/messaging, input action mapping

### Takeaway
rand 0.10 plus a seedable PRNG (rand_pcg, rand_xoshiro, or the tiny oorandom) covers deterministic gameplay RNG. For gacha pulls, the RNG is server-side by design. Input action mapping is well covered conceptually by bevy_enhanced_input (Unreal Enhanced Input-style contexts, modifiers and triggers) and leafwing-input-manager. Both are Bevy-coupled, so for a custom engine they are design references.

### Cited Findings
| Crate | Latest (date) | Total dl / recent 90d | Notes | Source |
|---|---|---|---|---|
| rand | 0.10.3 (2026-09-20) | 1.99B | 16 releases in 12 mo | [crates.io](https://crates.io/crates/rand) |
| rand_pcg | 0.10.2 (2026-04-11) | 230M | PCG (seedable, portable) | [crates.io](https://crates.io/crates/rand_pcg) |
| rand_xoshiro | 0.8.1 (2026-05-16) | 146M | xoshiro family | [crates.io](https://crates.io/crates/rand_xoshiro) |
| rand_chacha | 0.10.0 (2026-02-02) | 1.56B | CSPRNG | [crates.io](https://crates.io/crates/rand_chacha) |
| fastrand | 2.5.0 (2026-07-19) | 1.03B | wyrand, minimal | [crates.io](https://crates.io/crates/fastrand) |
| oorandom | 11.1.5 (2025-03-07) | 289M | tiny deterministic PCG | [crates.io](https://crates.io/crates/oorandom) |
| turborand | 0.10.1 (2024-02-24) | 102k | stale | [crates.io](https://crates.io/crates/turborand) |
| bevy_enhanced_input | 0.27.0 (2026-10-14) | 191k / 63k | 21 releases in 12 mo, 333★ | [crates.io](https://crates.io/crates/bevy_enhanced_input), [GitHub](https://github.com/simgine/bevy_enhanced_input) |
| leafwing-input-manager | 0.22.0 (2026-10-15) | 595k / 111k | 972★ | [crates.io](https://crates.io/crates/leafwing-input-manager), [GitHub](https://github.com/Leafwing-Studios/leafwing-input-manager) |

- bevy_enhanced_input: "The design of this crate was inspired by Unreal Engine Enhanced Input" — [BEI README](https://github.com/simgine/bevy_enhanced_input)
- bevy_ahoy uses BEI for its character-controller input, which shows the community trend toward BEI for action input — [bevy_ahoy README](https://github.com/janhohenheim/bevy_ahoy)
- Events and messaging: Bevy 0.17's event/observer overhaul made event kinds static (entity events vs global vs buffered) to cut overhead and misuse — [Bevy 0.17](https://bevy.org/news/bevy-0-17/). Channels: crossbeam/flume/kanal (see Concurrency).

### Inferences
- **Recommendation:**
  - Seedable rand_pcg or rand_xoshiro, explicitly seeded per subsystem (combat crits, loot, VFX variation), so replays and debug repros are possible. Never use `thread_rng` in simulation code.
  - rand_chacha or OS RNG only where unpredictability matters. Gacha results must come from the server.
  - Input: implement an Enhanced-Input-style layer (input contexts such as combat, menu and photo mode; modifiers like dead zones and swizzles; triggers like hold, tap, chord and combo buffer windows). ZZZ/WuWa-style dodge-cancel and perfect-dodge timing depend on buffered input with frame-accurate timestamps. Use BEI's design as the reference, or BEI itself if on Bevy.
  - Events: ECS observers for gameplay reactions (on-hit, on-parry) plus double-buffered message queues for cross-system batches.

### Gaps
- Whether BEI is being upstreamed into Bevy was not verified.
- No Rust crate specifically for combo/input-buffer windows in fighting or action games was found.

---

## Overall: recommended runtime/gameplay stack for this engine (synthesis)

### Takeaway
A credible Oct-2026 Rust stack: **bevy_ecs** (standalone, version-pinned), **glam**, **bevy_tasks/rayon**, **mimalloc + bump-scope** (moving to the std `Allocator` trait once Rust 1.100 ships), **Rapier/parry** with a Jolt-via-C escape hatch, **ozz-animation-rs** plus custom spring bones, cloth and a combat animation graph, **FMOD** via in-house bindings (kira or Firewheel as a pure-Rust fallback), **Luau via mlua** for scripting and live hotfixes, **rerecast + landmass** for navigation, **statig/bonsai** for AI, **rkyv + serde + bevy_reflect** for data, and **quinn/renet2 + tokio** for online. The weakest areas, which need in-house investment, are animation (compression, root motion, secondary motion, retargeting), audio middleware bindings, and console validation.

### Cited Findings
- bevy_ecs: 13 releases in 12 months, Tiny Glade production use — [crates.io](https://crates.io/crates/bevy_ecs), [80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)
- Rapier: determinism, parallelism and broad-phase gains in 0.35, soft bodies in 0.36 — [Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)
- Jolt is proven in Horizon Forbidden West and Death Stranding 2, with console support — [JoltPhysics](https://github.com/jrouwe/JoltPhysics)
- ozz-animation-rs feature set — [README](https://github.com/SlimeYummy/ozz-animation-rs)
- Allocator trait stabilized for 1.100 — [rust-lang/rust#156882](https://github.com/rust-lang/rust/pull/156882)
- Hot-update norms in Chinese mobile games — [HybridCLR](https://github.com/focus-creative-games/hybridclr), [xLua](https://github.com/Tencent/xLua)
- Abandoned or archived items to avoid: legion, specs, evenio, physx-rs, generational-arena, oxidized_navigation (deprecated → rerecast), laminar, rrise (Wwise), oddio (dormant), bincode (tombstone), big-brain (stalled; moved to Codeberg) — sources in each section above

### Inferences
- **Risk ranking (highest first):**
  1. Animation and secondary motion: core to the anime look and almost entirely custom.
  2. Audio middleware bindings and console builds: no maintained Rust FMOD/Wwise with console coverage.
  3. Bevy API churn if you track upstream.
  4. Physics bindings if Jolt is needed.
  5. Mobile performance of scripting.
- Single-maintainer dependencies with high strategic value (ozz-animation-rs, oxijolt, fmod-oxide, rerecast, landmass) should be **vendored or forked** with an internal owner from day one.
- Prefer engine-agnostic crates: they keep the option open to drop Bevy's app layer while keeping bevy_ecs.

### Gaps
- No verified shipped console title built on any of the Rust crates above was found in this research.
- Mobile thermal and performance data for a Rust stack of this composition (many skinned characters, spring bones, Luau) doesn't exist publicly. It needs an in-house vertical-slice benchmark.
