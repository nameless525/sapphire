# Engine Architecture for a Rust, Performance-Focused, Anime Character-Action Engine (WuWa / ZZZ class), as of October 2026

Scope: how to organize the engine: layering and crates, data model, threading and frame loop, time management, render integration (high level only), streaming, asset pipeline, editor, gameplay and combat framework, scripting and live-service patching, and performance principles. It draws on real engines and on what is publicly known about Wuthering Waves (WuWa), Zenless Zone Zero (ZZZ), Genshin Impact and Punishing: Gray Raven (PGR). Rendering techniques and specific Rust crate choices are covered by other researchers. These notes only flag where an architecture decision constrains those choices.

Sourcing conventions: every factual bullet carries a URL. Items marked "(not re-fetched this pass)" are well-known canonical references that I cited from prior knowledge without re-opening them in this session. The report writer may want to spot-check those. Sources tagged "community/third-party" are reverse-engineering or fan analyses, not official statements.

---

## 1. Engine layering: how real engines organize modules, and what works as a Rust cargo workspace

### Takeaway
All the reference engines layer the same way: a thin platform layer, then core (memory, jobs, containers, logging), then math, then resources/assets, then subsystems (render, anim, physics, audio), then a gameplay framework, then game code. Tools and editor code sit off to the side and never ship. The productive differences are in how strictly the dependency rules are enforced: The Machinery used header rules plus an API registry, Unreal uses Runtime/Editor module types, and Bevy uses many small crates behind plugin boundaries. In Rust, the crate graph is both the layering mechanism and the main lever on compile time. Aim for a flat `crates/` workspace with a shallow dependency DAG, put heavy generic/macro code at the leaves, and keep a hard runtime vs tools split.

### Cited Findings
- **Canonical layering (Gregory).** *Game Engine Architecture* (3rd ed., 2018) describes the runtime as a stack: hardware/OS/drivers, third-party SDKs, platform independence layer, core systems, resource manager, then rendering / collision & physics / animation / HID / audio / online, then gameplay foundations, then game-specific subsystems. Tools (the asset pipeline and world editor) sit beside the runtime, not inside it. — [Game Engine Architecture site](https://www.gameenginebook.com/) (not re-fetched this pass)
- **The Machinery (Our Machinery, 2017–2021): enforce physical design with machine-checkable rules.** Rule: "Header files (.h) cannot include other header files", except `<inttypes.h>`/`<stdbool.h>`. The rule was checked in a pre-commit hook. APIs are plain C99. OS types are hidden as opaque structs. Templates are confined to `.inl` files included only from `.cpp`. Inline functions are avoided in favour of "coarser, batch-oriented APIs". The stated motivation is lower coupling, which fixes slow compiles and whole-project recompiles. The authors argue that bad physical design is costly to fix and tends to regress, so it should be prevented with enforceable rules. — [Physical Design of The Machinery](https://ruby0x1.github.io/machinery_blog_archive/post/physical-design/index.html)
- **The Machinery plugin model.** APIs are structs of function pointers kept in a global registry. Hot reload patches the registry with the new DLL's pointers. Each plugin's global state lives in one static struct that is `memcpy`'d across reloads, and heap objects are left in place because both DLLs share an allocator. — [DLL Hot Reloading in Theory and Practice](https://ruby0x1.github.io/machinery_blog_archive/post/dll-hot-reloading-in-theory-and-practice/index.html)
- **The Machinery blog archive** also covers a fiber-based job system, bulk-data structures and indices, ECS and rendering, syncing a data-oriented ECS with stateful external systems (e.g., physics), the asset pipeline, and the document model. These are directly relevant reading for a small team. — [Machinery blog archive index](https://ruby0x1.github.io/machinery_blog_archive/)
- **Unreal layering.** Unreal splits code into modules with declared types (Runtime, Editor, Developer, Program, etc.). Editor/Developer modules are excluded from shipping builds, so the editor-vs-runtime split is enforced by the build system. — [Unreal Engine Modules docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules) (not re-fetched this pass)
- **Bevy: many crates plus plugins, and still decoupling.** Bevy 0.17 (Sept 30, 2025) "decoupled most of the user-facing renderer API from `bevy_render`", so that third-party renderers can work without it. It also added a separate `bevy_shader` crate for shader-only libraries. — [Bevy 0.17 release notes](https://bevy.org/news/bevy-0-17/)
- **Bevy release cadence.** Bevy 0.20 shipped Oct 8, 2026. Earlier releases: 0.19 (June 19, 2026), 0.18 (Jan 13, 2026), 0.17 (Sept 30, 2025), 0.16 (Apr 24, 2025). That is still roughly one breaking release every 3–5 months. — [Bevy news index](https://bevy.org/news/)
- **id Tech 7 (Doom Eternal, 2020).** There is "no main or render thread. It's all jobs with one worker thread per core" (Axel Gneiting, id Software, 2020 social post, as quoted by the wikis). — [Doom Wiki: id Tech 7](https://doomwiki.org/wiki/Id_Tech_7); [Wikipedia: id Tech 7](https://en.wikipedia.org/wiki/Id_Tech_7)
- **Destiny (Bungie, GDC 2015).** Bungie turned "almost every part of our engine into a job graph", with only limited thread pre-emption. The main game loop was split into strings of jobs called "job fibers". A runtime resource-tracking system verifies that every data access (reads, writes, creations, destructions) is valid and happens at a safe time in the frame. — [GDC Vault: Multithreading the Entire Destiny Engine (Barry Genova, 2015)](https://gdcvault.com/play/1022164/Multithreading-the-Entire-Destiny)
- **Large Rust workspace layout (matklad, 2021).** Use a flat `crates/` directory, with "adding or splitting the crates is trivial". Keep a virtual manifest at the root (no root crate). Folder names should match crate names. Unpublished internal crates get `version = "0.0.0"`. Put stable published libs in a separate `libs/` folder that must not depend on `crates/`. Do automation via `cargo xtask` in Rust. — [matklad: Large Rust Workspaces](https://matklad.github.io/2021/08/22/large-rust-workspaces.html)
- **Console toolchains are NDA-bound.** Chucklefish said getting Rust working on the PS4/Switch/Xbox One was feasible, but it could not contribute that work back without violating NDAs. Breaking such NDAs risks storefront removal and revoked signing keys. — [users.rust-lang.org thread on console support](https://users.rust-lang.org/t/is-there-a-reliable-way-to-support-mainstream-console-including-std/64451) (older community thread)

### Inferences
- **Recommended workspace (Rust).** Use a flat `crates/` directory with a virtual manifest. The intended dependency direction runs strictly downward:
  - **L0 `platform`**: window, input devices, file and async IO, threads/affinity, virtual memory, timers. One backend module per OS. Console backends live in **private, separately-licensed crates**, because of the NDAs. So the public engine crates must reach the OS only through `platform` traits/`cfg`, and must not pull in deps that hard-code desktop OS assumptions.
  - **L1 `core`**: allocators (frame/linear, tagged-block, pools), generational handles/arenas, containers, string IDs/hashes, logging, profiling markers. `math` sits beside it. `jobs` is the task scheduler.
  - **L2**: `reflect` (type registry, versioned schemas), `serialize`, `assets_rt` (runtime asset handles, IO scheduler, archive/pak reader), `ecs` (or a wrapped third-party ECS), `time` (clock domains, see Section 3).
  - **L3 subsystems**, each depending only on L0–L2: `render` (RHI + render graph + extract), `anim`, `physics` (wrapper), `audio`, `vfx`, `ui`, `nav`.
  - **L4 gameplay framework**: `world` (partition/streaming, entity lifecycle, prefabs), `gameplay` (abilities/effects/tags/timelines), `script` (VM bindings).
  - **L5**: `game_*` crates (characters, combat content, AI).
  - **Tools, outside the shipping dependency closure**: `asset_pipeline` (cookers, with heavy deps like texture encoders isolated there), `editor` (binary), and `xtask`.
- **Mirror Unreal's Runtime/Editor module types in Rust.** Put tools-only code in crates that runtime crates never depend on. Use cargo features only for optional platform/back-end selection, not to toggle editor code inside runtime crates. Feature unification across a workspace makes "editor feature leaks into shipping" bugs easy, which is an inference from how cargo features unify.
- **Compile time.** The crate DAG's critical path determines parallel build time. Keep `core`/`math`/`reflect` small and stable. Push monomorphization-heavy code (ECS queries, serde derives, proc macros) into leaf crates. Offer a dev-only dynamic-linking build of the engine, as Bevy does. A Machinery-style C-ABI "API registry" boundary between engine and game crates is the Rust equivalent of the no-header-include rule. It is also the cleanest seam for DLL hot reload (see Section 10).
- **Architecture over library choice.** Choosing Bevy's ECS or renderer imports its release cadence (about every 3–5 months), its scheduling model and its asset model. A small team building a custom engine should wrap any third-party ECS, physics or audio crate behind its own L2/L3 crate API. That way a breaking upstream release touches only one crate.

### Gaps
- I found no public, detailed description of Frostbite's or Ubisoft Anvil's overall module layering. Only Frostbite's FrameGraph (Section 4) is well documented publicly. Naughty Dog's layering beyond the job/memory system is also not public.
- No current (2025–2026) public statement on rustc support for PS5/Xbox Series/Switch 2 toolchains was found. Treat console Rust as possible but private-toolchain work.

---

## 2. Data model: ECS (archetype vs sparse-set) vs scene graph vs actor/component hybrid

### Takeaway
ECS is a clear win for homogeneous, high-count, per-frame work: transforms, culling and extraction, animation pose jobs, VFX, crowds, projectiles. The archetype layout gives fast iteration, and sparse sets give cheap add/remove. ECS hurts when it is forced onto bespoke, stateful, sequential logic: combat state machines, UI, editor documents, scripted sequences. The industry trend is toward hybrids. Unity is merging GameObjects and Entities under one `EntityId`. Unreal runs Mass (ECS) beside Actors. Overwatch's ECS needed singletons, utility functions and deferment to stay sane. For Rust, generational handles plus ECS fit the borrow checker well, but gameplay code needs an escape hatch: "fat" components, or script objects that own state machines.

### Cited Findings
- **Archetype vs sparse set (Sander Mertens, flecs author).**
  - Archetype: "An archetype ECS stores entities in tables, where components are columns and entities are rows". Iteration is fast, and query overhead goes to roughly zero once tables stabilize. The cost is table moves on add/remove (fragmentation).
  - Sparse set: "allow for fast add/remove operations", but needs per-entity checks during multi-component iteration.
  - Archetype users: Flecs, Unity DOTS, Unreal Mass, Bevy, Our Machinery, Legion, Hecs. Sparse-set users: EnTT, Shipyard.
  - Hierarchies stored as components "rely on component lookups". Depth-sorted tables speed traversal but "can create a lot of small tables". Flecs-style relationships (pairs) extend the model.
  - [ECS FAQ](https://github.com/SanderMertens/ecs-faq)
- **Bevy hit archetype-move costs in its own renderer.** Bevy 0.12 switched render-world component data to an `EntityHashMap` instead of table storage, "avoiding costly archetype moves". — [Bevy 0.12 release notes](https://bevy.org/news/bevy-0-12/)
- **Bevy 0.20 (Oct 2026) keeps optimizing change detection for extraction.** Components can opt into per-column `summary_tick`, which skips whole unchanged columns. The release reports a 132× speedup in GPU mesh extraction. It also added a batched `despawn_all`. — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/)
- **Overwatch ECS (Tim Ford, GDC 2017).**
  - Rules: components have no behavior, and systems (originally) have no state.
  - About 46 systems and 103 component types on the client, of which about 40% were singleton components for shared state.
  - Shared behavior goes in utility functions with few call sites.
  - Side effects (e.g., impact effects) are deferred into a singleton and resolved later, which allows batching and budgeting.
  - The killcam is a second ECS World replaying about 8–12 s of data.
  - Legacy non-ECS code was the main source of bugs.
  - Deferring entity creation to end-of-frame caused next-frame-availability problems that were fixed post-launch.
  - [GDC Vault: Overwatch Gameplay Architecture and Netcode (2017)](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and); details via [juejin Chinese transcript/translation](https://juejin.cn/post/6844903635864780813)
- **Unity is converging GameObjects and ECS ("ECS for All").**
  - From Unity 6.4, Entities/Collections/Mathematics/Entities Graphics become core packages. An `EntityId` replaces `InstanceID` for both GameObjects and Entities, and "every GameObject will have the ability to have ECS component data attached to it".
  - EntityId becomes 8 bytes in 6.5. Fast Enter Play Mode becomes the default in 6.6. A CoreCLR player is experimental in 6.7 LTS.
  - These are roadmap statements with safe-harbor caveats.
  - [Unity: CoreCLR, Scripting, and ECS Status Update – March 2026](https://discussions.unity.com/t/coreclr-scripting-and-ecs-status-update-march-2026/1711852); [ECS Development Status – Dec 2025](https://discussions.unity.com/t/ecs-development-status-december-2025/1699284/1); [ECS Development Status for Unity 6.6](https://discussions.unity.com/t/ecs-development-status-for-unity-6-6/1724480)
- **Why ECS fits Rust (Catherine West, RustConf 2018).** OOP-style architecture fights the borrow checker: a method on an entity that takes `&mut World` conflicts with the entity's own borrow. "You can do a LOT with indexes into Vecs." Generational indices make stale handles detectable. ECS emerges from "thinking about JUST the structure of state." — [kyren: RustConf 2018 closing keynote writeup](https://kyren.github.io/2018/09/14/rustconf-talk.html)
- **Where ECS hurts gameplay in Rust (LogLog Games, Apr 26, 2024, after ~3 years and 100k+ LOC).**
  - ECS is "a very specific tool that solves very specific problems, and that does not come for free".
  - "What most people attribute as benefits of ECS are for the most part benefits of generational arenas".
  - Moving logic into events/command buffers fragments simple features.
  - Dynamic borrow checks (RefCell, overlapping queries) crash after refactors.
  - Bevy UI needs "insane" ceremony.
  - Bevy's parallel scheduler "doesn't maintain consistent ordering even across frames".
  - The author prefers "fat components" for game logic.
  - [Leaving Rust gamedev after 3 years](https://loglog.games/blog/leaving-rust-gamedev/) (opinion piece)
- **Bevy's response to ordering nondeterminism (0.20).** `chain_weak` orders system sets only where data accesses conflict. A debug `shuffle_seed` randomizes the order of conflicting systems to expose hidden ordering bugs. Panics in systems become routed errors. — [Bevy 0.20 release notes](https://bevy.org/news/bevy-0-20/)

### Inferences
- **Recommended hybrid model:**
  1. An **archetype ECS as the "world database"** for everything spatial and numerous: transforms, bounds, render proxies, skeletal pose instances, VFX emitters, projectiles, hit volumes, NPC crowds.
  2. **Character "actors"** that are entities with a few *fat* gameplay components. A `CombatController` component holds the move state machine, input buffer, ability instances, cooldowns, and so on, as ordinary Rust structs, not dozens of tiny components. This keeps a character's combat logic in one place, avoiding the LogLog complaint, while the heavy per-frame data stays ECS-friendly.
  3. **Singletons/resources** for global services (input, camera director, combat-event queue, streaming state), following Overwatch.
  4. **Deferred side effects**: hits, damage, VFX/SFX spawns and camera shakes are queued and resolved at fixed points in the frame. Overwatch's deferment maps naturally onto Rust command buffers and avoids aliasing `&mut` borrows.
- **Order deterministically.** Combat must run in a fixed order. Make the combat/gameplay schedule explicitly ordered, or single-threaded within the fixed step. Parallelize the wide data passes (animation, physics broadphase, culling, extraction) instead of parallelizing gameplay systems against each other. This avoids the frame-to-frame ordering nondeterminism LogLog criticized, which Bevy's own 0.20 debug tooling now tries to catch.
- **Keep the scene graph out of the ECS core.** Use a parent/child relationship only for attachment (weapons on bones, VFX on sockets). Resolve world transforms in a dedicated hierarchical pass with depth-sorted batches. Do not use the hierarchy as the organizing structure of the world.
- **Editor and UI should not live in the runtime ECS.** The editor needs a document model with undo (Section 7). UI is better served by a retained widget tree with its own animation and shader support, per the LogLog UI critique.

### Gaps
- No public numbers on entity counts or ECS usage in WuWa (UE4 Actor model plus scripts) or HoYoverse titles (Unity GameObject model). Their data models are inferred from their engines, not documented.

---

## 3. Threading and frame architecture, fixed timestep, hitstop and time dilation

### Takeaway
Use a job/task graph with one worker per core and no privileged "main thread logic" beyond OS-mandated calls. Pipeline simulation and rendering across frames, with an explicit per-frame data packet: Naughty Dog's FrameParams, Bevy's extract. Use frame-lifetime-tagged allocators. Gameplay and combat should run on a fixed timestep with interpolation. Time needs several clock domains (real, global game time, per-entity local time) so that hitstop, perfect-dodge slow-motion and UI all compose cleanly.

### Cited Findings
- **Naughty Dog fiber job system (Christian Gyrling, GDC 2015; The Last of Us Remastered at 60 fps on PS4).**
  - **Background.** On PS3 the engine was single-threaded game logic at 30 Hz, with SPUs as workers.
  - **Threads and fibers.** 6 worker threads, each locked to a CPU core. A job always runs inside a fiber. There are 160 fibers (128 × 64 KiB stacks, 32 × 512 KiB stacks) and 3 job queues (low/normal/high priority).
  - **Synchronization.** Done with atomic counters. `WaitForCounter` parks the fiber on a wait list, and the worker picks up the next job.
  - **I/O.** Sockets, file I/O and system calls stay on separate system threads.
  - **Locks.** Atomic spin locks almost everywhere. A special "job mutex" sleeps the job for locks held longer.
  - **Tooling.** Fibers are visible in the debugger and in core dumps. Thread-local storage must be fiber-safe.
  - [Slides PDF](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf); [GDC Vault](https://www.gdcvault.com/play/1022186/Parallelizing-the-Naughty-Dog-Engine)
- **Naughty Dog frame pipelining.**
  - There was about 100 ms of CPU work per frame (16.66 ms × 6 cores ≈ 100 ms).
  - The new design runs game logic, render logic and GPU execution in a feed-forward pipeline, each stage on a different frame. Game logic plus render logic went from 25 ms (serial) to 15.5 ms.
  - Per-stage independence means "very few locks needed".
  - A frame is defined as "A piece of data that is processed and ultimately displayed on screen … NOT a length of time."
  - `FrameParams` carries frame number, delta time, camera, skinning matrices and the mesh list, plus per-stage start/end timestamps. `HasFrameCompleted(n)` governs memory lifetime. They rotate 16 FrameParams.
  - [Slides PDF](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)
- **Naughty Dog tagged heap.**
  - Linear allocators sized for worst case wasted 100–200 MiB.
  - Replacement: a block allocator with 2 MiB blocks (a PS4 large page, i.e., one TLB entry). Each block is owned by a `uint64` tag. There is no `Free(ptr)`, only "free all blocks with tag".
  - Allocators take one block per worker thread, which means "No locks required for 99.9% of memory allocations". 99% of allocations are under 2 MiB.
  - Lifetimes they handle include single game-logic stage, double game-logic stage, game→render, game→GPU and render→GPU.
  - [Slides PDF](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)
- **Destiny job graph with runtime data-access validation (GDC 2015).** — [GDC Vault](https://gdcvault.com/play/1022164/Multithreading-the-Entire-Destiny). The companion renderer talk notes the design did not scale well to differing core layouts or heterogeneous systems. — [Tatarchuk, Destiny's Multi-threaded Renderer Architecture (GDC 2015) slides](https://advances.realtimerendering.com/destiny/gdc_2015/Tatarchuk_GDC_2015__Destiny_Renderer_web.pdf)
- **id Tech 7: no main/render thread, all jobs, one worker per core.** — [Doom Wiki](https://doomwiki.org/wiki/Id_Tech_7)
- **Bevy pipelined rendering (0.10, Mar 2023).** Simulation and rendering run in parallel on different frames. Data moves from the main app to the render sub-app in an "extract step". Gains were typically 10–30%, with a 1.8 ms mean frame-time reduction in "many foxes". It is enabled by `PipelinedRenderingPlugin` everywhere except wasm. — [Bevy 0.10 release notes](https://bevy.org/news/bevy-0-10/)
- **Overwatch fixed-step simulation.** Fixed 16 ms "command frames" (7 ms in esports mode) via `UpdateFixed`. Determinism is used for prediction and rollback. The client time-dilates slightly (e.g., 15.2 ms ticks) to adjust lead under packet loss. — [juejin translation of Ford GDC 2017](https://juejin.cn/post/6844903635864780813); [GDC Vault](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and)
- **Fixed timestep with accumulator and render interpolation** is the standard pattern. — [Glenn Fiedler, Fix Your Timestep!](https://gafferongames.com/post/fix_your_timestep/) (not re-fetched this pass)
- **Work-stealing job system design** (lock-free queues, parent/child jobs). — [Molecular Matters: Job System 2.0 series (2015)](https://blog.molecular-matters.com/2015/08/24/job-system-2-0-lock-free-work-stealing-part-1-basics/) (not re-fetched this pass)
- **Unreal time dilation model.** Each actor's tick delta is multiplied by the world's global TimeDilation (WorldSettings) and by the actor's `CustomTimeDilation`, so "each actor [can] run at a different time speed". — [UE 4.26 API: AActor::CustomTimeDilation](https://docs.unrealengine.com/4.26/en-US/API/Runtime/Engine/GameFramework/AActor/CustomTimeDilation/)
  - A community-documented pitfall: latent `Delay` nodes do not obey actor time dilation. — [Unreal Answers thread](https://answers.unrealengine.com/questions/364161/view.html)
  - A common pattern is to set global dilation and counter-scale actors that must stay at normal speed. — [Essentials C++ Time Dilation component docs](https://essentialscpp.readthedocs.io/en/latest/features/time_manipulation.html) (third-party)
- **Hitstop is a core feel tool.** Sakurai's "Creating Games" channel (from Aug 2022) devotes an early episode to hit stop. He shows the same combo with and without it, covers "boss stops" (zooming in on the final hit), and argues it should be used in every genre. — [Anime News Network coverage](https://cdn.animenewsnetwork.com/interest/2022-08-24/masahiro-sakurai-launches-youtube-channel-about-game-development/.188941)
- **WuWa's game thread as a bottleneck (community/third-party, Apr 2026).**
  - Measured on an i7-12700KF + RTX 5070: max CPU thread at 97–100% while the GPU averaged 54–58%.
  - Community mobile traces: GameThread about 75% vs TaskGraph workers ≤13%.
  - The author labels the causal explanation (serialization on UE4's GameThread plus script runtime) "unverified".
  - Kuro's own 2024 interview lists "multithreading across game, render, and RHI threads" as a future goal.
  - [pqmlmaoxd: WuWa Performance & Stutter analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html); [gcores interview compilation, 2024-05-16](https://www.gcores.com/articles/181970)

### Inferences
- **Recommended frame loop (Rust):**
  1. **Platform pump on the OS thread** (window/input events are often required there), producing an input snapshot.
  2. **Fixed-step gameplay sim**, 0..N steps per frame at e.g. 60 Hz, using the accumulator pattern. It consumes buffered inputs and runs combat, abilities, AI decisions, character movement and hit queries, emitting deferred events.
  3. **Variable-rate presentation update.** Animation pose evaluation (as parallel jobs), cloth/secondary motion, VFX simulation and camera. These are interpolated or sampled with per-entity local time.
  4. **Extract.** Copy render-relevant state into a `FrameParams`-like packet that the render pipeline owns. This is the only sync point.
  5. **Render prepare/record**, overlapping the next frame's sim, parallel across workers.
  6. **GPU submit/present.**

  This gives about one frame of added latency, as at Naughty Dog and Bevy. That is acceptable for action games if input sampling happens late and the sim runs at ≥60 Hz. Measure end-to-end latency, because perfect-dodge windows are timing-critical.
- **Job system in Rust.** Stackful fibers are not a first-class Rust feature and interact badly with thread-local storage and `!Send` borrows. Naughty Dog itself had to make TLS fiber-safe. Prefer a work-stealing task graph with explicit dependencies (counters/continuations) where "wait" means "help execute other tasks". Reserve dedicated threads for IO and audio (as Naughty Dog did for I/O). Express each system's data access declaratively so the scheduler, or Rust's type system, can validate it. This is Destiny's runtime resource-tracking, enforced at compile time.
- **Avoid a single serial "game thread" chokepoint.** WuWa's reported CPU-bound profile on UE4.26, with a script runtime on the GameThread, is the cautionary tale. Keep script/gameplay code thin, and push the wide work (anim, physics, culling, VFX, streaming decode) into jobs.
- **Time architecture for hitstop and slow-motion.** Define explicit clock domains:
  - `real` (UI, menus, input timestamps, network).
  - `world` (global game time, scaled by global dilation: pause, cinematic slow-motion).
  - `local[e]` per entity, a scale stack (hitstop: attacker and victim scale = 0 for N sim ticks; perfect dodge: world scale e.g. 0.1–0.3 while the player's local scale counter-compensates).
  - Make every timer, cooldown, animation, VFX, audio pitch/rate and physics body read the clock of its owner explicitly. Hitstop should be counted in **fixed sim ticks**, not wall-clock, so it is frame-rate independent. Delayed/latent actions must also bind to an owner clock, to avoid the Unreal `Delay` pitfall.
  - Cameras usually run on `real` or a camera-specific clock so that slow-motion still feels responsive.
- **Determinism.** These games are largely single-player with server-validated progression, so full lockstep determinism is not required. Still, make the fixed-step combat sim deterministic given inputs (fixed order, no hash-map iteration order dependence, seeded RNG per encounter). That enables input-replay bug repro, automated combat regression tests and future co-op. Overwatch's ability to replay into a second "killcam" world shows the payoff.

### Gaps
- No public detail on how WuWa or ZZZ implement hitstop or perfect-dodge time dilation internally. Whether it is per-actor dilation, global dilation with exemptions, or animation-rate scaling is not documented.
- No authoritative public source on target sim tick rates for WuWa/ZZZ.

---

## 4. Render architecture (high level): render/frame graph, extract/prepare/render, GPU-driven, bindless

### Takeaway
Treat the renderer as a separate "world" fed by an extract step. Organize each frame as a declarative frame graph (setup → compile → execute) that culls unused passes, computes transient-resource lifetimes, aliases memory and schedules async compute. This is mainly an architectural decision about ownership and lifetimes. It constrains the GPU-abstraction library: it must expose explicit barriers/aliasing, or the graph must run on top of a layer that tracks them.

### Cited Findings
- **Frostbite FrameGraph (Yuriy O'Donnell, GDC 2017).**
  - Setup phase: passes declare read/write/create of resources in near-immediate-mode code (C++ lambdas for setup and execute).
  - Compile phase: culls unreferenced passes/resources, computes lifetimes ("Acquire right before first use, release after last use"), derives bind flags and extends lifetimes for async compute.
  - Execute phase: runs pass callbacks with resolved resources.
  - Transient resources live at most one frame. Backends include physical/virtual aliasing, pools and a linear allocator for buffers.
  - Memory at 720p: 147 MB non-aliased vs about 80 MB on DX12 PC, 77 MB on PS4 and 76 MB on Xbox One. At 4K DX12: 1042 MB → 472 MB.
  - The WorldRenderer shrank from 15K to 5K SLOC.
  - Modules share data through a blackboard. Lesson: aliasing is "harder than it sounds" (metadata like DCC, barrier batching).
  - [Slides (SlideShare)](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite); [GDC Vault](https://www.gdcvault.com/play/1024612/FrameGraph-Extensible-Rendering-Architecture-in)
- **Bevy's main-world/render-world split with an extract step**, and render set ordering (PrepareAssets, Queue, Sort before per-frame resource prep), with automatic batching/instancing and storage-buffer arrays (`GpuArrayBuffer`). — [Bevy 0.12 notes](https://bevy.org/news/bevy-0-12/); [Bevy 0.10 notes](https://bevy.org/news/bevy-0-10/)
- **Bevy 0.20** integrates mesh shaders with its pipeline cache (no web support), and its asset processor now emits BCn or ASTC. — [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20/)
- **Machinery on ECS ↔ renderer coupling.** See "Entity-Component-Systems and Rendering" and "Syncing a data-oriented ECS with a stateful external system". — [Machinery blog archive](https://ruby0x1.github.io/machinery_blog_archive/)
- **WuWa mobile used one-pass deferred** to support SSR/GTAO and to cut shader-variant maintenance. The article claims about 1.5 GB/s bandwidth savings at 60 fps, and notes Mali PLS limits (128-bit on-chip storage, no alpha blend, single-RT resolve). Character lighting has a dedicated pipeline. — [gcores interview compilation (2024-05-16)](https://www.gcores.com/articles/181970)
- **Genshin's console port talk** (Unity Dojo 2021, Zhenzhong Yi) describes customizing Unity's rendering pipeline. — [Docswell slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi)

### Inferences
- **Architectural contract.**
  - Gameplay never touches GPU objects. Extract produces plain-data render proxies (or deltas: Bevy 0.20's per-column change ticks show that delta extraction matters at scale).
  - The render graph is rebuilt or re-validated every frame.
  - Persistent GPU scene buffers (instances, materials, skinning palettes) are updated incrementally. That is the precondition for GPU-driven culling and bindless material indexing.
- **Platform variance.** PC, console and mobile need different graphs. Mobile wants tile-memory-friendly passes (subpasses/PLS, as WuWa did), and PC/console want async compute and GPU-driven paths. A frame graph where passes are composed per platform tier, rather than `if`-ed inside passes, is the maintainable way to support all three.
- **Library constraint.** The chosen GPU API layer must allow explicit transient-memory aliasing (or provide it) and explicit barriers for the graph compiler to be worthwhile. Otherwise the graph only handles ordering and culling. The rendering researcher should evaluate libraries on this axis.

### Gaps
- Not researched in depth here by design. See the rendering researcher's notes for GPU-driven and bindless specifics.

---

## 5. Open-world streaming: partitioning, HLOD, async IO, memory budgets, large-world coordinates

### Takeaway
Use Unreal World Partition as the reference model:
- one logical world;
- actors auto-assigned to grid cells by a spatial hash;
- streaming sources (the player, plus extra sources such as a fast-travel target or cinematic camera);
- data layers for state variants;
- one-file-per-actor for team collaboration;
- generated per-cell HLOD.

Use doubles on the CPU, and camera-relative or tile+offset floats on the GPU and in particle systems. WuWa evidently needed a custom world-partition and geometry-streaming layer on UE4.26 and still shipped streaming stutter. Treat streaming as a first-class subsystem from day one.

### Cited Findings
- **World Partition (UE5).**
  - The world lives in "a single persistent Level separated into grid cells".
  - Streaming sources are "components that define a position in the world and trigger the loading of cells around them". Player controllers are sources by default. Extra sources have Priority, Target Grid, Shapes and Target State (Loaded vs Activated).
  - Runtime spatial-hash grids have Cell Size, Loading Range, "Block on Slow Streaming" and Priority. The docs give examples such as 256 m cells with a 768 m loading radius, and warn that multiple grids can hurt performance.
  - Data Layers gate loading.
  - One File Per Actor lets team members edit without checking out the level. Actors that reference each other load together.
  - HLOD actors are generated per cell from HLOD layers.
  - The editor starts unloaded and loads regions on demand.
  - Builder commandlets batch-process worlds without loading them fully.
  - [UE docs: World Partition](https://dev.epicgames.com/documentation/en-us/unreal-engine/world-partition-in-unreal-engine)
- **Large World Coordinates (UE5).** Double-precision variants of core types. The default `WORLD_MAX` is about 88 million km (vs UE4's 21 km limit). Niagara keeps floats for performance by splitting space into grid cells, with one value for the position within a cell and another for the cell. New HLSL types live in `LargeWorldCoordinates.ush`. — [UE docs: Large World Coordinates](https://dev.epicgames.com/documentation/en-us/unreal-engine/large-world-coordinates-in-unreal-engine-5)
- **WuWa ships custom streaming plugins on UE4.26 (community/third-party).** The `Engine.ini` plugin list reportedly includes `KuroWorldPartition`, `FastGeoStreaming` and `KuroPSOTools`. A v3.1 patch note reportedly "updated the loading pipeline and accelerat[ed] data streaming". Mobile traces show `r.Streaming.PoolSize = 400`. — [pqmlmaoxd WuWa performance analysis (Apr 2026)](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)
- **WuWa region packaging (community datamine).** `mapblockinfo.json` maps world regions to `PakName` values (e.g., `Pack_HHA_Underground`), and quest config references `PakName`s. This suggests region- and content-keyed packages. The storage analysis shows PC install around 130–150 GB across v3.1–v3.3, and a v3.3 "Resource Cleanup" feature. The author notes duplication across paks cannot be proven without the asset registry. — [pqmlmaoxd WuWa storage analysis (Mar 2026)](https://pqmlmaoxd.github.io/gamedev/analysis/2026/03/15/wuthering-waves-memory-bloat.html); [WutheringWaves_Data datamine repo](https://github.com/Arikatsu/WutheringWaves_Data)
- **WuWa vegetation LOD per platform.**
  - PC/console: mesh cards near, billboards mid, imposters far.
  - Mobile: billboards near, imposters mid-far.
  - Billboard trees cost about 30–40% of mesh-card polygons.
  - Imposter slices are streamed dynamically and batched through a dynamic TextureArray.
  - [gcores interview compilation](https://www.gcores.com/articles/181970)
- **DirectStorage 1.1 added GPU decompression (GDeflate)** on Windows. — [Microsoft DirectX blog: DirectStorage 1.1](https://devblogs.microsoft.com/directx/directstorage-1-1-now-available/) (not re-fetched this pass)
- **io_uring** gives Linux batched async submission/completion rings. — [Jens Axboe, Efficient IO with io_uring](https://kernel.dk/io_uring.pdf) (not re-fetched this pass)

### Inferences
- **Recommended streaming architecture:**
  - **Authoring.** One logical world. Every placed object is a separate small file or record with a stable GUID (team-friendly, like OFPA). Objects get a "spatially loaded" flag, data-layer membership, and an explicit "load-together" group (from references).
  - **Cook.** A world builder assigns objects to cells per grid (e.g., a coarse grid for terrain/large props, a fine grid for clutter, a separate grid for gameplay entities). It generates HLOD per cell and emits cell packages: one IO request each, containing pre-relocated, pre-serialized runtime data.
  - **Runtime.** A streaming manager keeps prioritized sources: player position plus velocity look-ahead, fast-travel destinations, cutscene cameras, and the upcoming party-swap or traversal target. It issues async reads through a platform IO backend (DirectStorage on Windows, io_uring on Linux, the console SDK's IO, and `F_NOCACHE`/mmap-style paths on Apple/Android). It decodes on worker jobs and **activates on the main sim at a budgeted rate** (N entities or µs per frame). This time-slicing is where hitches are won or lost.
  - **Memory budgets per category** (textures, meshes, anim, audio, VFX, gameplay), with feedback into streaming priority. Mobile budgets are much smaller (WuWa's mobile texture pool is 400 MB per the trace above).
- **Precision.** Use `f64` world positions in sim/streaming. Convert to camera-relative `f32` at extract. VFX and particles use cell + offset, like Niagara's LWC approach. This is simpler to retrofit early than late.
- **Character-action specifics.** Combat arenas and boss rooms should be able to lock streaming state, pre-warming all content an encounter needs (enemy movesets, VFX, PSOs) before combat begins. Mid-combat streaming hitches are far more noticeable than traversal hitches.
- **Live-service packaging.** Region/content-keyed packages (as WuWa's PakName data suggests) enable optional downloads and cleanup. Content-addressed chunking avoids rebuilding giant archives on patch (see Section 6).

### Gaps
- No public detail on HoYoverse's open-world streaming (Genshin/ZZZ). ZZZ is mostly zone/level based rather than seamless open world, but its streaming design is not documented publicly.
- No official Kuro description of `KuroWorldPartition`/`FastGeoStreaming`. Their function is inferred from names only.

---

## 6. Asset pipeline: source vs cooked, GUIDs, DDC/content-addressing, hot reload, platform cooking

### Takeaway
The pipeline has two halves: (1) source assets plus sidecar metadata (GUID, importer settings) under version control, and (2) a deterministic, incremental "cook" that turns sources into platform-specific runtime blobs. Cooked outputs are cached by a content hash of (source bytes, importer version, settings, platform, dependency hashes) in local and shared caches. Unreal's DDC/Zen and Bevy's processor with `.meta` and `processed_info` hashes are the reference designs. Runtime formats should be load-in-place, with packages laid out for streaming and patching.

### Cited Findings
- **Bevy Asset V2 (0.12, Nov 2023).**
  - Optional `.meta` files set the action (Load/Process/Ignore) and loader/processor settings.
  - The `AssetProcessor` is an "asset compiler" from `assets/` to `imported_assets/`. Processed outputs carry `processed_info` with hashes and process dependencies. Only changed assets are reprocessed, and "If a dependency has changed, the dependant asset will also be re-processed!"
  - Write-ahead logging resumes after crashes.
  - Processed assets hot-reload.
  - Named asset sources (e.g., `remote://`, `embedded://`) are supported, and handles use generational indices.
  - [Bevy 0.12 release notes](https://bevy.org/news/bevy-0-12/)
- **Bevy 0.20 (Oct 2026).** The processor's texture compressor outputs BCn or ASTC with automatic mipmaps. "Assets as entities" and a human-readable `.bsn` asset format are listed as future work. — [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20/)
- **Unreal DDC/Zen.** UE 5.4+ defaults to a local Zen store DDC. A shared DDC has cloud, shared and local layers, and buildfarm cooks write to the shared and cloud layers, which replicate across regions. The Zen server is unauthenticated, so keep it on trusted networks. — [UE docs: Using Derived Data Cache](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-derived-data-cache-in-unreal-engine); [UE docs: Set up Zen Storage Server as Shared DDC](https://dev.epicgames.com/documentation/ja-jp/unreal-engine/set-up-zen-storage-server-as-shared-ddc-for-unreal-engine)
- **The Machinery's critique of Bitsquid's data model.** JSON files as the authoritative data meant copy/paste and undo had to be written per tool. Tools could not see each other's unsaved changes. Viewports needed temporary-file hacks. The runtime could only read compiled data. — [The Story behind The Truth](https://ruby0x1.github.io/machinery_blog_archive/post/the-story-behind-the-truth-designing-a-data-model/index.html)
- **WuWa patch/storage pain (community analysis).** UE pak-based packaging plus content growth (datamined file counts up about 81% from v1.0 to v3.2) led to very large installs and large predownloads. The author argues the cause is architecture rather than compression, but labels much of this inference. — [pqmlmaoxd WuWa storage analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/03/15/wuthering-waves-memory-bloat.html)

### Inferences
- **Recommended pipeline:**
  1. **Identity.** Every source asset has a GUID in a sidecar meta file. References are by GUID, never by path, so renames do not break links. This was The Truth's primary goal: references that don't break.
  2. **Importers** turn DCC formats (glTF/FBX/PSD/WAV) into an intermediate engine-neutral representation.
  3. **Cookers** turn the intermediate into a per-platform runtime blob: BCn for PC/console, ASTC for mobile, platform audio codecs, optimized mesh/meshlet layouts, compressed animation.
  4. **Cache key** = hash(source content, importer/cooker version, settings, target platform, hashes of dependencies). Look up local cache → team/cloud cache → cook. That is a DDC.
  5. **The dependency graph** is recorded per output, giving reverse-dependency invalidation for hot reload.
  6. **Packaging** turns cooked blobs into streaming packages (per world cell, per character, per encounter), with a manifest of content-hashed chunks so patches download only changed chunks.
- **Runtime asset formats.** Design for load-in-place: relocatable offsets, `#[repr(C)]` POD headers, zero-copy byte views. That makes streaming activation cheap and enables GPU-direct paths. Rust architecture constraint: this favors explicit binary layouts (e.g., a hand-rolled or zero-copy serialization layer) over serde-style tree deserialization on the runtime hot path. Keep serde-style flexibility for editor and intermediate formats.
- **Run the cook in a separate process or worker pool** so crashes in third-party encoders don't take down the editor, and so it can scale to a build farm. Bevy's in-process processor is fine for small projects. A gacha-scale content team, with hundreds of characters, VFX and voice lines per patch, needs farmable, cacheable cooks.
- **Hot reload path.** Watch sources, re-cook incrementally, swap runtime handles. Generational handles let in-flight references stay valid or detect staleness.

### Gaps
- No public description of HoYoverse's internal asset/patch pipeline (Unity AssetBundle-based, presumably heavily customized) or of Kuro's cook farm.

---

## 7. Editor architecture: in-engine vs separate process, play-in-editor, undo, reflection, live link

### Takeaway
A shared, reflected, in-memory document model is the foundation of a good editor: typed objects, properties, GUID references, and change records of (object, property, old, new). Undo/redo, copy/paste, prefab overrides, multi-user collaboration and live-link to a running game can all be derived from it. The game runtime should not *be* the document. The editor edits documents, and documents are cooked or streamed into runtime worlds. Play-in-editor (PIE) instantiates a separate runtime world from the document state.

### Cited Findings
- **The Truth (The Machinery).**
  - Typed objects with properties (bools, ints, floats, strings, buffers, references, sub-objects, sets).
  - The in-memory representation is "considered authoritative", and disk formats are backups.
  - Changes are recorded uniformly as (object, property, old value, new value), so "copy/paste and undo can be defined in terms of the data model". Real-time collaboration serializes modifications.
  - References are GUIDs. Sub-objects are owned and cloned, while references are not.
  - Buffers (textures, meshes) are opaque, and per-property merges allow concurrent edits.
  - "Having a single in-memory representation of the editor state has made implementing UI and tools a lot easier."
  - [The Story behind The Truth](https://ruby0x1.github.io/machinery_blog_archive/post/the-story-behind-the-truth-designing-a-data-model/index.html)
- **Unreal World Partition editor workflow.** The editor starts with the world unloaded and loads regions on demand. Builder commandlets process worlds without loading them fully. — [UE docs: World Partition](https://dev.epicgames.com/documentation/en-us/unreal-engine/world-partition-in-unreal-engine)
- **Bevy editor status (Oct 2026).** Bevy 0.20 ships "Feathers" widgets (color input, list views, dropdowns, scrubbable number inputs, contextual theming) as building blocks for a future scene editor, not the editor itself. Bevy 0.17 had already described Feathers as the widget set for the upcoming editor and planned a first-party entity inspector. — [Bevy 0.20 notes](https://bevy.org/news/bevy-0-20/); [Bevy 0.17 notes](https://bevy.org/news/bevy-0-17/)
- **Unity is making Fast Enter Play Mode the default in 6.6**, i.e., skipping domain reload to shorten PIE iteration. — [Unity March 2026 status update](https://discussions.unity.com/t/coreclr-scripting-and-ecs-status-update-march-2026/1711852)
- **Unreal Sequencer** was modified for WuWa so most visual elements can be controlled in cinematics. — [gcores interview compilation](https://www.gcores.com/articles/181970)

### Inferences
- **Recommended split for a small Rust team:**
  - **`editor` binary** = document model (Truth-like) + UI + viewport(s). Viewports embed the runtime renderer and a "preview world" built from documents.
  - **PIE.** Spin up a fresh runtime world (same process, separate world/allocator tags) from the current document snapshot. Tear it down on stop. Never mutate documents from game code.
  - **Live link to device builds** (mobile/console). Run the game as a separate process and have the editor push document change records over a socket. The runtime applies them via the same hot-reload path used for assets. This is the only practical way to tune combat frame data and VFX on a phone or console.
  - **Reflection.** Required for documents, inspectors, serialization, undo and script bindings. In Rust this means a derive-macro type registry with stable type IDs and versioned schemas, owned by the engine and not tied to one ECS crate. Keep derive-heavy reflection out of hot runtime crates where possible, for compile time.
- **Editor UI is a different problem from game UI.** Immediate-mode or retained tool UI is fine for the editor. In-game HUD and menus need a designer-friendly, animatable, shader-capable UI system (LogLog's critique). Budget them as two separate systems.
- **Combat tooling is the highest-ROI editor feature for this genre.** Expect a timeline editor over animation clips with tracks for hitbox windows, cancel windows, invulnerability, root-motion overrides, VFX/SFX/camera cues and hitstop values. It should scrub with a live preview character and enemy dummy, plus an in-game frame-data overlay. WuWa's unified VFX material controller and Sequencer changes show the genre's dependence on deep cinematic/VFX tooling.

### Gaps
- No public information on HoYoverse's or Kuro's internal editors or combat-authoring tools (beyond Kuro's Sequencer modifications). GDC 2026 Genshin UGC talks (Miliastra Wonderland) suggest HoYoverse exposed a substantial authoring framework to players, but I could not access their content (Section 9).

---

## 8. Gameplay architecture for character-action combat

### Takeaway
Build combat as data-driven moves on a fixed-step sim. Each move is an animation plus a frame-data timeline (startup/active/recovery, hitbox activation, cancel windows, invulnerability/dodge windows, cues). An input buffer feeds a per-character move state machine, and the cancel rules decide transitions. Hits are detected via hitbox/hurtbox overlap in the sim step, queued, then resolved through an ability/effect/attribute/tag system modeled on Unreal GAS. Hitstop, shake and VFX are cues on per-entity clocks. Party swap is a first-class ability: all party members are resident, ticking entities, which has real memory-budget implications. Enemy AI is a BT/StateTree or utility layer selecting from the same move system the player uses.

### Cited Findings
- **Cancel windows and input buffering (God of War analysis, Game Developer).**
  - A "Partial Cancel" is allowed only in specific windows. A "Complete Cancel" is allowed at any time.
  - The most common windows are pre-hit and post-hit frames.
  - The "Buffer" method stores the command and executes it when the window opens. The "Instant" method executes only if pressed while the window is open. A buffered command can itself be overridden before the window opens.
  - Example: a 60-frame attack with a hit window at frames 15–20.
  - [Combat Canceled: God of War & Action Game Design](https://www.gamedeveloper.com/pc/combat-canceled-i-god-of-war-i-action-game-design)
- **Unreal GAS as reference ability framework.**
  - GAS is "a framework for building attributes, abilities, and interactions that an Actor can own and trigger".
  - Its parts: Ability System Component; Gameplay Abilities (active or passive, coordinating animation, VFX, sound); Attributes/AttributeSets; Gameplay Effects composed of Gameplay Effect Components that modify attributes; Ability Tasks that "perform asynchronous work during a Gameplay Ability's execution"; plus Gameplay Tags and Cues.
  - [UE docs: Gameplay Ability System](https://dev.epicgames.com/documentation/en-us/unreal-engine/gameplay-ability-system-for-unreal-engine)
- **Overwatch's deferment pattern for side effects** (contacts/impacts queued in a singleton and resolved later) reduces coupling and enables batching. Its ability/projectile prediction relies on the fixed-step sim. — [juejin translation of Ford GDC 2017](https://juejin.cn/post/6844903635864780813)
- **Hitstop design (Sakurai)**: the same combo with vs without hit stop, plus "boss stops" on the final hit. — [ANN coverage](https://cdn.animenewsnetwork.com/interest/2022-08-24/masahiro-sakurai-launches-youtube-channel-about-game-development/.188941)
- **Per-actor time dilation as an engine primitive** (global × per-actor multiplier on tick delta). — [UE API: CustomTimeDilation](https://docs.unrealengine.com/4.26/en-US/API/Runtime/Engine/GameFramework/AActor/CustomTimeDilation/)
- **WuWa combat VFX.** Echo (声骸) and skill effects share a unified material controller and Niagara system within the combat loop. The 2D side-scroller mini-game sections use custom camera rules and character control, with faster turning to offset input latency. — [gcores interview compilation](https://www.gcores.com/articles/181970)
- **WuWa combat logic lives in script (community datamine).** A 2024 cheat-forum post locates damage-related JavaScript under a `client/content/Aki` path inside paks. Binary strings include `/Game/Aki/TypeScript/`. — [FearLess forum thread](https://fearlessrevolution.com/viewtopic.php?t=29316&start=165); [pqmlmaoxd analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html) (community/third-party)
- **WuWa combat additions over time** (e.g., "Tune Break" in Dec 2025) show a combat framework that must keep absorbing new mechanics each patch. — [Wikipedia: Wuthering Waves](https://en.wikipedia.org/wiki/Wuthering_Waves)
- **AI references (canonical; not re-fetched this pass):**
  - Behavior trees: [Damian Isla, Handling Complexity in the Halo 2 AI (GDC 2005)](https://www.gamedeveloper.com/programming/gdc-2005-proceeding-handling-complexity-in-the-i-halo-2-i-ai).
  - Utility AI: [Dave Mark & Kevin Dill, Improving AI Decision Modeling Through Utility Theory (GDC 2010)](https://www.gdcvault.com/play/1012410/Improving-AI-Decision-Modeling-Through).
  - Animation: [Simon Clavet, Motion Matching and The Road to Next-Gen Animation (GDC 2016)](https://www.gdcvault.com/play/1023280/Motion-Matching-and-The-Road).
  - Cameras: [John Nesky, 50 Game Camera Mistakes (GDC 2014)](https://www.gdcvault.com/play/1020460/50-Camera).

### Inferences
- **Recommended combat framework (crate `gameplay` + `game_combat`):**
  1. **Input layer.** Sample raw input into timestamped *intents* (attack, heavy, skill, ultimate, dodge, swap-to-N, jump) in a ring buffer about 8–15 sim ticks deep. Each intent has an expiry and priority (dodge > swap > skill > attack is typical; make it data). Consumption happens only in the fixed step.
  2. **Move graph per character** (data asset). Nodes are moves (anim clip, or a blend/branch for directional variants). Each move carries a frame-data timeline:
     - phase markers (startup/active/recovery);
     - hitbox tracks (shape, bone/socket, active frames, hit group/ID for multi-hit dedupe);
     - hurtbox and invulnerability/armor/"perfect-dodge window" tracks;
     - cancel windows (which intents or tags may cancel, buffer vs instant per the God of War taxonomy);
     - root-motion and motion-warp segments (snap to target);
     - cue tracks (VFX, SFX, camera shake, hitstop amount, controller rumble).
     
     Edges are cancel rules: intent + window + condition (resource, tag, airborne, etc.).
  3. **Sim tick order (deterministic):** consume intents → evaluate cancels and transitions → advance move time on the *local* clock → update root motion/character controller → evaluate active hitboxes vs hurtboxes (broadphase from physics, narrowphase in combat code, dedupe per hit group) → emit `HitEvent`s into a queue → resolve hits (damage calc via effects/attributes/tags, poise/stagger, element/status application) → emit cues (hitstop sets attacker and victim local scale = 0 for N ticks; perfect dodge triggers world slow-motion) → AI and abilities react next tick.
  4. **Ability/effect system (GAS-like).** Use attributes (HP, energy, concerto/"swap" meters, stagger) and tags (state flags like `State.Airborne` and `Status.Frozen`, used for queries and cancel conditions). Effects are instant, duration or periodic modifiers. Abilities are scripted state machines or tasks built on moves. Cues are presentation-only, so they can be skipped in headless tests and server validation.
  5. **Party/swap system.** All 3 party members are spawned, resident entities. Off-field members keep ticking timers, buffs, cooldowns and any off-field abilities (coordinated attacks, intro/outro skills) at reduced presentation cost: hidden, no full anim eval, maybe simplified collision. Swap is an ability: outro on the leaving character, intro on the arriving one, often with i-frames, camera handoff and a time-scale effect. **Memory implication:** all three characters' full movesets, VFX, audio banks and PSOs must be resident. Budget combat memory as roughly 3× the per-character footprint plus enemy sets, and pre-warm them on party change, not on swap.
  6. **Camera director.** A priority stack of camera behaviors: exploration follow, combat framing (player + locked target or enemy-cluster centroid), lock-on, ultimate/cinematic cameras (timeline-driven, matching how WuWa extends Sequencer), and swap transitions. It runs on its own clock so slow-motion doesn't make camera input feel laggy.
  7. **Enemy AI.** Use a decision layer (behavior tree or StateTree for authored boss patterns, utility scoring for target and tactic selection) that outputs intents into the *same* move system the player uses. This reuses frame data, hitboxes and cancels, and lets designers build bosses in the same timeline editor. Add an encounter-level "attack token" coordinator so crowds don't all attack at once (common industry practice, not cited here).
- **Script boundary.** Keep the per-tick hot path (move timeline evaluation, hit detection, effect math) in Rust. Expose ability logic and boss scripts to the scripting layer at event granularity (on move start, on hit, on window open), not per-tick polling. WuWa's apparent script-heavy combat on V8, with suspected GC spikes, is the cautionary example (Sections 9–10).
- **Testing.** A headless combat sim (no renderer, cues stubbed) driven by recorded intent streams enables automated regression tests of frame data and damage. This matters for a live-service game shipping new characters every 6 weeks.

### Gaps
- No public engineering talks were found on combat implementation at Kuro (WuWa/PGR) or HoYoverse (ZZZ/Honkai Impact 3rd). Also not found in this pass: implementation-level GDC talks on Bayonetta/DMC combat systems. The God of War analysis is a design article, not a code talk.
- The exact frame-data authoring tools used by these studios are not public.

---

## 9. What is publicly known about WuWa, ZZZ, Genshin, HSR and PGR architecture

### Takeaway
WuWa is a heavily customized **UE4.26** with UE5 features backported, not a UE5 game. Wikipedia's "UE5 since May 2024" sentence conflicts with Kuro's own interview and with 2026 coverage. Community reverse-engineering shows a **TypeScript/JavaScript gameplay layer via (Kuro-forked) PuerTS on V8**, with C# assemblies appearing from v2.8–3.0, and custom streaming/PSO plugins. HoYoverse's games (Genshin, HSR, ZZZ) are on **customized Unity**. Public HoYoverse tech talks are mostly rendering and porting (Genshin console, 2021) and UGC (Genshin "Miliastra Wonderland", GDC 2026). I found no public ZZZ architecture talk. PGR is Unity.

### Cited Findings
- **WuWa engine = UE4.26, customized, with UE5 features ported.** UE4.26 was chosen as "相对成熟且稳定的版本" (a relatively mature and stable version) when the project started. Kuro listed Lumen-like GI, Niagara GPU particles and game/render/RHI multithreading as future interests. — [gcores interview compilation (2024-05-16; user-compiled from official interviews)](https://www.gcores.com/articles/181970)
- **The UE5 claim conflicts.** Wikipedia's body text says "they upgraded to Unreal Engine 5" starting May 2024, while its infobox says UE4. — [Wikipedia: Wuthering Waves](https://en.wikipedia.org/wiki/Wuthering_Waves). April 2026 coverage says Kuro deliberately stayed on a heavily modified UE4. — [ixbt.games (Apr 16, 2026)](https://ixbt.games/en/news/2026/04/16/410132-otkazalis-ot-unreal-engine-5-i-vyigrali-na-the-game-awards-2025-wuthering-waves-stanovitsia-vse-krasivee.html). A community analysis quotes Kuro on UE5 migration difficulty and cites an Unreal Fest Tokyo 2025 talk describing GI work "within UE4.26". — [pqmlmaoxd analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html). **Weight of evidence: UE4.26-based.**
- **Kuro at Unreal Fest Tokyo 2025 (Nov 15, 2025).** WuWa graphics rendering lead Wang Xin presented hybrid ray tracing for the anime-style open world. — [GameBusiness.jp report (Dec 27, 2025)](https://www.gamebusiness.jp/article/2025/12/27/25893.html)
- **Team size and platforms.** The WuWa team was over 500 people at launch, per Epic's 2024 developer interview. — [Epic developer interview (zh-CN)](https://www.unrealengine.com/zh-CN/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves). Platforms: Windows/Android/iOS (May 2024), PS5 (Jan 2025), macOS (Mar 2025), Xbox Series X|S (July 2026). — [Wikipedia](https://en.wikipedia.org/wiki/Wuthering_Waves)
- **WuWa scripting (community reverse-engineering, unverified by Kuro).**
  - `Client.log` shows "LoadModule: Puerts" and a V8 11.8.172.18 header. Binary strings include `PuertsJsEnv`, `typescript`, `KuroPuerts`, `/Game/Aki/TypeScript/`.
  - Datamine paths `level_entity/` and `prefab/` coexist with `level_entity_csharp/` (from v2.8) and `prefab_csharp/` (from v3.0). `CSharpScript.dll` is found in ScriptAssemblies.
  - V8/Puerts appears to remain the primary runtime.
  - Frame-time spikes of 175/245 ms are attributed to V8 major GC ("inferred, not confirmed").
  - [pqmlmaoxd analysis (Apr 12, 2026)](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)
- **PuerTS** (Tencent) is "a multi-language scripting solution for Unity/Unreal/DotNet". It supports UE 4.22+ and Unity 5+. Backends are V8 (default), QuickJS and Node.js, with Lua 5.4 and Python added for Unity in 3.0. "Unreal currently supports JavaScript/TypeScript only." — [Tencent/puerts GitHub](https://github.com/Tencent/puerts)
- **Kuro ownership.** Tencent became Kuro's sole external shareholder with 51.4% (controlling) in late 2024, while Kuro stays independently operated. This is relevant context for PuerTS (a Tencent project) adoption, but it is not evidence of it. — [IT之家 (Dec 1, 2024)](https://www.ithome.com/0/814/491.htm)
- **PGR (Kuro's earlier game) is Unity.** — [Wikipedia: Punishing: Gray Raven](https://en.wikipedia.org/wiki/Punishing:_Gray_Raven)
- **Genshin = Unity, customized.** In the Unity Dojo 2021 console-rendering talk (Zhenzhong Yi, head of Genshin console development), Unity's "concise coding style allowed us to more conveniently customize" the rendering pipeline. — [Docswell slides](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi); [Wikipedia: Genshin Impact](https://en.wikipedia.org/wiki/Genshin_Impact)
- **Genshin GDC 2021 (Haoyu Cai)**: "Crafting an Anime-Style Open World" covered origins, style and design pipeline. — [GDC news](https://www.gdconf.com/news/learn-about-making-genshin-impacts-open-world-gdc-2021)
- **Genshin GDC 2026 (March 2026), Xin Ning, Gameplay Engineer, miHoYo.** Two talks: one on building the "Miliastra Wonderland" UGC module, covering design and technical challenges and a roadmap including PCG/AI. The other on a built-in multiplayer UGC experience on Genshin's *existing architecture*, covering performance challenges. — [GDC session: Miliastra Wonderland](https://schedule.gdconf.com/session/genshin-impact-miliastra-wonderland-co-creating-a-virtual-world-with-users/917205); [GDC session: Built-in multiplayer UGC](https://schedule.gdconf.com/session/implementing-a-built-in-multiplayer-ugc-experience-in-genshin-impact/917102) (abstracts via search; pages returned 403 to my fetcher)
- **ZZZ.**
  - Version 3.0 (June 2025) added DLSS frame generation and ray tracing. — [IT之家](https://www.ithome.com/0/960/973.htm)
  - A single non-primary source claims ZZZ uses Unity China's "Tuanjie" engine variant. **Unverified.** — [foro3d analysis](https://foro3d.com/articulo/analisis-tecnico-zenless-zone-zero-motores-flujo-trabajo.html)
  - Chinese Unity reportedly has features not available globally, possibly driven by partners like miHoYo (headline only retrieved). — [Automaton](https://automaton-media.com/en/?p=27940)
  - The ZZZ team reportedly won a CEDEC 2025 visual award, per a personal LinkedIn post. No talk content found. — [LinkedIn](https://cn.linkedin.com/in/alain-chen)

### Inferences
- **WuWa's architecture is the "commercial engine + script layer + custom plugins" pattern** typical of Chinese live-service games: UE4 Actor/Component world, TS/JS gameplay on V8 (later partly C#), custom streaming, PSO tools and Sequencer, and region-keyed pak packaging. Its publicly discussed pain points map onto architecture decisions a custom engine can make differently from day one:
  - CPU-bound game thread → a jobified sim with thin scripts.
  - Shader/PSO stutter → precaching built into material and asset design (Section 11).
  - Streaming hitches → budgeted async activation.
  - GC spikes → a script VM with bounded or no GC pauses, and no per-tick script.
  - Install size → content-addressed patching.
- **Genshin's GDC 2026 UGC talks imply** HoYoverse's gameplay framework is data-driven enough to expose authoring to players in a live game. The lesson: a robust data/scripting layer for gameplay content pays off far beyond iteration speed.
- **Kuro moving from TS/V8 toward C# (if the datamine reading is right)** would be a move toward a statically typed, AOT-friendlier runtime with more predictable GC. This is a reason to choose the scripting layer carefully up front (Section 10).

### Gaps
- No official Kuro or HoYoverse engineering talk on gameplay architecture, ECS/actor model, job system, streaming or patch pipeline was found for WuWa, ZZZ, HSR or Genshin. The GDC 2026 Genshin UGC talk contents were not accessible.
- Genshin's/HSR's scripting/hotfix layer (commonly assumed to be Lua-based, e.g., via xLua-like tech) could not be confirmed from a reliable source in this pass. Grasscutter-style private servers reference scene "ScriptData", but I did not verify Lua specifics.
- ZZZ's exact engine variant (global Unity vs Tuanjie) is unconfirmed.

---

## 10. Scripting and data-driven design; live-service hot-patching on mobile

### Takeaway
Use three tiers:
1. **Data assets** (moves, frame data, effects, enemy stats, quests) for most content. Cookable, diffable, patchable without code.
2. **An embedded, sandboxed script layer** for ability/quest/boss logic that must change between client releases. On iOS it must run as an interpreter, because there is no JIT.
3. **Rust (native)** for engine and hot-path gameplay, with dev-time hot-patching for iteration only.

Chinese live-service games rely heavily on tier 2: PuerTS (TS/JS), xLua (Lua), HybridCLR (C# interpreter in IL2CPP). Apple permits downloaded interpreted code only within limits.

### Cited Findings
- **PuerTS**: V8/QuickJS/Node backends, TypeScript declaration generation for engine APIs, UE 4.22+/Unity. — [Tencent/puerts](https://github.com/Tencent/puerts)
- **HybridCLR** extends IL2CPP from a pure AOT runtime into an AOT + interpreter hybrid, enabling C# hot updates on Android, iOS, consoles and WebGL. It is often compared with xLua (requires tagging) and ILRuntime (limited by iOS's JIT ban). — [HybridCLR (gitee mirror)](https://gitee.com/inner_bomb/hybridclr); comparison via [CSDN blog (personal opinion)](https://blog.csdn.net/2301_80252792)
- **xLua** (Tencent) is a Lua binding/hotfix solution for Unity. — [Tencent/xLua](https://github.com/Tencent/xLua) (not re-fetched this pass)
- **Apple's rules on downloaded interpreted code.**
  - As quoted from the 2022-era Apple Developer Program License Agreement: downloaded interpreted code is allowed only if it does not change the app's primary purpose, does not create a store for other code, and does not bypass OS security. — [Expo forums quoting DPLA](https://forums.expo.dev/t/expo-updates-and-apples-app-store-review-guidelines/64495)
  - The 2017 relaxation was read as "an app can update its own code, but it can't go from being one type of app into another." — [The Register (June 2017)](https://www.theregister.com/2017/06/07/apple_relaxes_developer_rules/)
  - The earliest (2010) rules required Apple's written consent. — [MacStories](https://www.macstories.net/news/apple-updates-section-3-3-2-apps-can-use-interpreted-code/)
- **Rust hot-patching for development (Bevy 0.17, Sept 2025).** Works via `subsecond` and the Dioxus `dx` CLI: "every system can now be modified during execution". Limits: binary crate only, no Wasm, not applied if a system's parameters change, and sensitive to linker configuration. — [Bevy 0.17 notes](https://bevy.org/news/bevy-0-17/)
- **The Machinery's DLL hot reload.** Function-pointer API registry plus a state struct copied across reloads. Struct layout changes break it, and debuggers locking PDB/DLL files caused failures. It was reliable without a debugger and used daily by the whole team. — [Machinery: DLL Hot Reloading](https://ruby0x1.github.io/machinery_blog_archive/post/dll-hot-reloading-in-theory-and-practice/index.html)
- **Hot reload matters for games:** "Games are anything but stateless data processors". Rust hot-reload crates were unreliable for that author. — [LogLog Games](https://loglog.games/blog/leaving-rust-gamedev/)
- **WuWa's script runtime as a suspected hitch source** (V8 GC, inferred) and its partial C# move. — [pqmlmaoxd analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)

### Inferences
- **Recommendation for a Rust engine:**
  - **Data first.** Most per-patch content in this genre (new character kits, enemy variants, events) should be expressible as data: move graphs, frame-data timelines, effect definitions and tag rules. It can then ship as patched assets with no code change. This is the single biggest lever for live-service velocity and for platform-policy safety.
  - **Script layer for logic that must hot-update.** Use a small, embeddable, interpreter-capable VM with predictable GC and good tooling. Lua/Luau-class VMs, a JS engine in interpreter mode (QuickJS-class) or a WASM interpreter are the realistic families. The final pick belongs to the library researcher. Architectural constraints:
    1. It must run without JIT on iOS (and likely consoles).
    2. Pauses must be bounded (incremental/generational GC, or arenas reset per event).
    3. It must be event-driven, not per-tick.
    4. It must have a typed binding layer generated from the engine's reflection registry (as PuerTS generates `.d.ts`).
    5. It must be sandboxed (no raw pointers), with deterministic execution for combat replay.
  - **Native Rust iteration.** Use `subsecond`-style hot-patching or a Machinery-style C-ABI plugin boundary for dev builds only. Shipped mobile builds cannot hot-load native code, so anything that must change post-release without store review must live in data or script.
  - **Policy hygiene.** Keep downloaded scripts within the app's "primary purpose" (content and balance updates, not new app categories). Keep native security boundaries intact, per Apple's DPLA terms. Console platforms have their own patch-certification rules; script/data patches still go through their patch processes. That is an inference: I found no specific source on console rules for script patches.

### Gaps
- I could not retrieve the current (2026) text of Apple's DPLA or Guideline 2.5.2 (behind login or not fetched). The rule summary relies on 2017–2022 secondary quotes.
- Console platform policies on downloadable script content were not researched (NDA-gated).

---

## 11. Performance architecture principles: DOD, allocators, frame budgets, hitch avoidance

### Takeaway
Performance in this genre is decided by architecture. It needs:
- data-oriented layouts and batch APIs;
- frame-scoped and tag-scoped allocators instead of general heap churn;
- a fully jobified frame with one sync point;
- budgeted, time-sliced streaming activation;
- *hitch* elimination: PSO/shader precompile at load, script GC control, IO off the main thread.

The WuWa community analyses read like a checklist of what happens when these are retrofitted rather than designed in.

### Cited Findings
- **Data-oriented design**: design around the data and its transformations on real hardware. — [Mike Acton, "Data-Oriented Design and C++", CppCon 2014](https://www.youtube.com/watch?v=rX0ItVEVjHc) (not re-fetched this pass)
- **Batch-oriented APIs over inline per-item calls.** The Machinery cites a potential (estimated) 4× SIMD gain. — [Machinery: Physical Design](https://ruby0x1.github.io/machinery_blog_archive/post/physical-design/index.html)
- **Naughty Dog memory architecture**: linear/frame allocators, the 100–200 MiB worst-case waste problem, and the 2 MiB tagged heap with per-thread blocks (99.9% lock-free allocations). — [ND slides](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)
- **60 fps by filling all cores.** Naughty Dog's analysis: ~100 ms of CPU work ≈ 6 cores × 16.66 ms, so 60 fps required near-100% core utilization plus frame pipelining. — [ND slides](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)
- **PSO hitch avoidance (UE PSO precaching).**
  - Primitive components precache all PSOs they might need (base, shadow, velocity, etc.) right after loading, across all mesh passes. Global PSOs compile at startup, behind the loading screen.
  - If a PSO isn't ready, the default is to skip drawing the proxy, or substitute a default material.
  - Precached PSOs are deleted after compile to save memory (otherwise hundreds of MB). Driver-cache retrieval can take a few ms. Each compile thread can use up to 2 GB.
  - [UE docs: PSO Precaching](https://dev.epicgames.com/documentation/en-us/unreal-engine/pso-precaching-for-unreal-engine)
- **WuWa shader stutter history (community).** v1.4 and v2.2 patch notes cite shader compilation optimizations, and v1.4 moved compilation to the title screen. Residual stutter occurs at area boundaries. A custom `KuroPSOTools` plugin exists. — [pqmlmaoxd analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)
- **WuWa reduced shader variants** on mobile by choosing one-pass deferred. — [gcores](https://www.gcores.com/articles/181970)
- **Frame graph transient aliasing** halves render-target memory: 4K went 1042 → 472 MB in Frostbite. — [FrameGraph slides](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite)
- **Delta extraction matters at scale**: Bevy's 132× mesh-extraction speedup from column-level change ticks. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)

### Inferences
- **Frame budget template, 60 fps (16.6 ms).**
  - On console/PC, target ≤ ~12–13 ms of CPU critical-path per frame to leave headroom for spikes.
  - On mobile, design for 30 fps baseline / 60 fps high tier. Budget for **sustained** (thermally throttled) clocks, not peak. Measure on mid-tier Android under thermal load.
  - Per-system budgets live in a config and are checked by automated perf captures.
  - These numbers are recommendations, not sourced figures.
- **Allocator policy.**
  - Per-frame linear allocators, or Naughty Dog-style tag-scoped block allocators keyed by `FrameParams` stage and frame number, for all transient data (extract packets, command lists, hit-event queues, job closures).
  - Pools for fixed-size gameplay objects.
  - Long-lived assets in streaming-aware arenas per cell or package, freed wholesale on unload.
  - In Rust, use custom allocator handles passed explicitly (or the `allocator_api` on nightly / allocator-wrapper crates) rather than relying on the global allocator.
- **Hitch elimination as architecture.**
  - **Materials** declare their full permutation set at cook time. Cooked packages list the PSOs they need. Loading a cell or character pre-warms its PSOs asynchronously and gates visibility until ready, as in UE precaching.
  - **Ship PSO caches per platform/driver tier**, with a first-launch warm-up pass. For Vulkan/Android, persist the driver pipeline cache.
  - **No synchronous IO, decompression or script GC on the sim thread.** Script heaps get per-frame step budgets or arena resets.
  - **Entity activation** (spawning ECS entities from streamed cells) is time-sliced.
  - **Kill per-frame dynamic allocation in steady state.** Track it with a debug allocator counter.

### Gaps
- No sourced public mobile frame-budget breakdown for WuWa/ZZZ/Genshin was found, nor any thermal-throttling target data from these studios.

---

## 12. Practical recommendation: what a small team should build, and in what order

### Takeaway
Build the "spine" first: platform + core + jobs + time + frame pipeline + reflection/serialization + asset IDs/cook + a minimal renderer behind extract. Then the combat vertical slice (character controller, anim, move/frame-data system, hit detection, ability/effects, camera, one enemy). Only then streaming, editor depth, scripting and live-service packaging. Each step should be designed so later systems slot in without rewrites. Specifically: clock domains, deferred event queues, GUID asset references, extract-only render access and per-platform cook from day one.

### Cited Findings
- Naughty Dog retrofitted frame pipelining and tagged-heap memory to reach 60 fps on PS4; the frame-centric `FrameParams` design simplified the engine. — [ND slides](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)
- Frostbite's FrameGraph let legacy passes be wrapped cheaply, and cut WorldRenderer code by two-thirds. — [FrameGraph slides](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite)
- Overwatch's biggest bug source was legacy code that didn't follow the ECS rules. — [juejin translation](https://juejin.cn/post/6844903635864780813)
- The Machinery's insight: building tools on a single authoritative in-memory data model made UI and tools "a lot easier". — [The Truth](https://ruby0x1.github.io/machinery_blog_archive/post/the-story-behind-the-truth-designing-a-data-model/index.html)
- Rust-specific iteration risks (hot reload, ECS ceremony, refactor friction) are real for small teams. — [LogLog Games](https://loglog.games/blog/leaving-rust-gamedev/)
- Rust gives first-class tooling for the data model via generational indices and ECS. — [kyren RustConf 2018](https://kyren.github.io/2018/09/14/rustconf-talk.html)
- Bevy shows a modular Rust engine is viable but still evolving: editor not shipped as of 0.20, and breaking releases every few months. — [Bevy news](https://bevy.org/news/); [Bevy 0.20](https://bevy.org/news/bevy-0-20/)

### Inferences
**Phase 0: foundations (months 0–3)**
1. Workspace skeleton: flat `crates/`, virtual manifest, `xtask`, CI with perf and compile-time tracking. Write the layering rules into CI: a script that fails if an L1 crate depends on L3, or if a runtime crate depends on a tools crate.
2. `platform` (desktop first, with backend traits shaped for mobile and console), `core` (allocators: frame/tagged, pools; handles; logging; profiler markers, e.g. Tracy-style integration), `math` (f32 SIMD + f64 world positions), `jobs` (work-stealing task graph, IO threads).
3. `time` with clock domains, a fixed-step driver and interpolation.
4. `reflect` + `serialize`: type registry, stable type IDs, versioned schemas.

**Phase 1: data path (months 2–6)**
5. Asset identity (GUID sidecars), importer → intermediate → per-platform cooker, content-hash cache (local first; shared later), load-in-place runtime formats, async loader with dependency tracking, hot reload.
6. ECS world: wrap a third-party archetype ECS, or write a focused one. Add the extract → `FrameParams` packet, with a render world that is fed only by extract.
7. Minimal renderer on a frame graph from the start, even if only a few passes. The anime character pipeline and GPU-driven work proceed in parallel per the rendering researcher.

**Phase 2: combat vertical slice (months 4–10)**
8. Animation runtime: sampling, blending, root motion, sockets. Character controller.
9. Combat core: intent buffer, move graph + frame-data timelines, hitbox/hurtbox queries, deferred hit resolution, GAS-like attributes/effects/tags/cues, hitstop and perfect-dodge via clock domains, a camera director.
10. Party of 3 with swap; one enemy archetype with BT/utility AI on the same move system; a headless combat test harness with recorded inputs.
11. Minimal timeline editor for frame data, even as a standalone tool, plus live-link tuning into a running game.

**Phase 3: world and tools (months 8–18)**
12. Streaming: grid cells, streaming sources, budgeted activation, HLOD generation in the cook, data layers, f64→camera-relative conversion.
13. Editor on a Truth-like document model: undo/redo, prefabs/overrides, PIE as a separate world, one-file-per-object world storage.
14. Script VM integration at event granularity, with generated bindings from reflection.

**Phase 4: shipping concerns (months 12+)**
15. PSO/permutation inventory and precache, plus per-platform PSO cache shipping.
16. Packaging: per-cell, per-character and per-encounter packages, content-addressed chunk manifests for small patches, optional-download regions.
17. Mobile and console backends (console under NDA, in private crates), thermal-aware performance tiers, automated perf captures per build.

**Key "don't paint yourself into a corner" decisions for day one**
- All gameplay time flows through owner clocks.
- All side effects go through queues.
- The renderer only sees extracted data.
- Assets are referenced by GUID and cooked per platform.
- No editor code in runtime crates.
- Gameplay logic is data-first, with script at event granularity and Rust for the hot path.
- Every allocation in the frame has a lifetime tag.

### Gaps
- Timelines above are rough estimates for a small, senior team, not sourced data.
- I found no published retrospective of a team building a custom Rust engine at this content scale (anime character-action, live-service). Bevy-based commercial titles exist but not in this genre and scale, as far as this pass found.
