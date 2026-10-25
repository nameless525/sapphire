# Build Your Anime Action Engine in Rust, Selectively

Rust is a good language for this engine in October 2026. For a solo developer it is a better bet than C++, provided you use it the way the one successful Rust 3D studio did. Borrow the generic foundations (bevy_ecs, wgpu, glam, Rapier, egui). Write the parts that make the game look and feel anime: the renderer, the combat and animation runtime, and the game UI. Keep the scope far below Wuthering Waves. The best-known teams that left Rust gamedev complained about iteration speed, API churn and missing tools, not runtime performance, and those gaps sit exactly where this genre lives. As of late 2026 Rust has none of the following: a production anime-character renderer, an animation stack with built-in root motion and spring bones, a proven animated game-UI framework, a maintained Wwise binding, a public console toolchain, or a shipped 3D character-action game. The games you want to emulate came from very large teams. Wuthering Waves (WuWa) had more than 500 people on a heavily modified Unreal Engine 4.26. Genshin, whose Unity-based lineage Zenless Zone Zero (ZZZ) shares, had about 700 people and cost about $100M. A seamless WuWa-scale open world is therefore not a realistic solo target in any language, but ZZZ-style zones are. The architecture that fits is a layered cargo workspace built around five things: a deterministic fixed-step combat simulation; per-entity clocks for hitstop and slow motion; one extract boundary feeding a custom clustered-forward+ anime renderer on wgpu; data-first content, with Luau scripts for live patches; and a cook pipeline that references assets by GUID and caches outputs by content hash. Ship on macOS and Windows first and mobile second. Treat consoles as a separate project, gated by platform-holder NDAs. Before investing years, spend the first months testing the riskiest assumption: that a Rust toolchain lets you tune one character's combat feel quickly enough.

## Rust's weak spots are tools, middleware and consoles, not speed

### Shipped evidence tops out at a two-person diorama builder

The strongest Rust 3D proof point is Tiny Glade, a castle-building diorama from the two-person studio Pounce Light. It launched on Windows and Linux in September 2024 and was nominated for the BAFTA Technical Achievement award. It reached macOS only on 13 May 2026, and no console version has been announced ([Wikipedia](https://en.wikipedia.org/wiki/Tiny_Glade)). Its stack is the lesson. The team kept **Bevy's ECS as the backbone of game logic** but replaced Bevy's rendering with their own Vulkan pipeline so they could "control pretty much the whole pipeline". They still run a modified Bevy ([80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)). Bevy's other commercial releases (Toroban, Court Wizard, Simulo and others) are small indie titles ([Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)). Embark, the most Rust-invested studio near AAA scale, handed rust-gpu over to community ownership in October 2025 ([GitHub](https://github.com/EmbarkStudios/rust-gpu)) and ships ARC Raiders on a modified Unreal Engine 5 ([Unreal Engine](https://www.unrealengine.com/en-US/developer-interviews/embark-studios-build-the-award-winning-arc-raiders-with-unreal-engine)). The research found **no shipped Rust-engine game with real-time combat, skinned characters and animation-heavy content**.

### The famous defections were about iteration, and are only partly fixed

LogLog Games went back to Unity after more than three years and over 100k lines of Rust. Their complaints were refactors forced by the borrow checker during prototyping, unreliable hot reload, ECS ceremony that scattered simple features across events and command buffers, and no UI library suited to stylized, animated game UI ([LogLog Games](https://loglog.games/blog/leaving-rust-gamedev/)). Architect of Ruin ported from Bevy to Unity in about six weeks in early 2025. Its reasons were slow onboarding of a non-programmer collaborator, frequent Bevy API breaks, poor AI-assistant guidance for fast-moving Bevy, and no modding story. The developer still called Bevy's ECS "a joy" and said Rust's compile-time checks made refactoring confident ([deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)).

Some of this has improved since. Bevy 0.17 added hot-patching of ECS systems through Dioxus's `subsecond`, but only in the binary crate and only for systems whose parameters don't change ([Bevy 0.17](https://bevy.org/news/bevy-0-17/)). subsecond cannot reload struct layout changes and runs on iOS only in the simulator ([docs.rs](https://docs.rs/subsecond/latest/subsecond/)).

Other gaps remain open:

- **Editor.** Bevy still had no editor in August 2026. Its lead calls the editor "our (and my) next and overriding priority", while admitting he has promised one "next year" for years ([Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)).
- **Funding.** The Bevy Foundation pays its developers **roughly 54% below baseline salaries** ([Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)).
- **API churn.** Bevy still breaks APIs every 3 to 5 months: 0.16 shipped in April 2025, and 0.20 on 8 October 2026 ([Bevy news](https://bevy.org/news/)).
- **Build times.** The Rust project's 2026 "Fast Builds" plans are goals, not shipped defaults. They include a parallel compiler front end, Wild linker integration, and "Relink-Don't-Rebuild", which aims at 5 to 10x faster rebuilds when only function bodies change ([Rust Project Goals](https://rust-lang.github.io/goals/2026/roadmap-fast-builds.html)).

One real language improvement is close. A stabilized subset of the `Allocator` trait, including `Vec::new_in` and `Box::new_in`, has been merged for **Rust 1.100**. That makes per-frame arena allocators first-class on stable Rust ([rust-lang/rust#156882](https://github.com/rust-lang/rust/pull/156882)).

### Consoles sit behind NDA walls that public Rust cannot cross

Bevy itself says "even modern consoles such as the Nintendo Switch, PlayStation 5, or Xbox Series don't have tier 1 support" because of non-disclosure agreements ([Bevy 0.15](https://bevy.org/news/bevy-0-15/)). rustc's only console targets are Tier 3 homebrew and retro ones. There are **no PlayStation or Xbox targets** ([rustc platform support](https://doc.rust-lang.org/rustc/platform-support.html)). Chucklefish reportedly got Rust running on the previous console generation but could not share the work upstream without breaking NDAs ([Rust users forum](https://users.rust-lang.org/t/is-there-a-reliable-way-to-support-mainstream-console-including-std/64451)). wgpu's built-in backends are DX12, Metal, Vulkan, GL and WebGPU, with no console backend ([wgpu docs](https://wgpu.rs/doc/wgpu/backend/index.html)). C++ middleware is different: Jolt Physics, for example, officially supports "Platform Blue (a popular game console)" through an NDA'd header ([JoltPhysics](https://github.com/jrouwe/JoltPhysics)). So a console version of a Rust engine is a private toolchain-and-graphics-backend project. You would do it as a licensed developer or pay a porting studio. That matters because WuWa and ZZZ both ship on PS5.

### The reference games show both the scale and the scars

WuWa had **more than 500 people at launch** ([Epic](https://www.unrealengine.com/zh-CN/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves)). It is built on UE4.26, chosen as "a relatively mature and stable version", with UE5 features back-ported ([GCORES](https://www.gcores.com/articles/181970)). It launched on Windows, Android and iOS in May 2024, then added PS5 (January 2025), macOS (March 2025) and Xbox (July 2026) ([Wikipedia](https://en.wikipedia.org/wiki/Wuthering_Waves)). Genshin cost about $100M ([GamingBolt](https://gamingbolt.com/genshin-impact-developer-says-it-cost-100-million-to-create-200-million-cost-per-year-expected-for-continued-development)) with about 700 developers ([Anime Corner](https://animecorner.me/mihoyo-reveals-goal-to-create-a-massive-virtual-world/)) on customized Unity, which ZZZ also uses ([Wikipedia](https://en.wikipedia.org/wiki/Zenless_Zone_Zero)). Even studios of that size chose to modify a commercial engine rather than write their own.

WuWa's publicly documented problems are also a design brief for a new engine. Community analyses report four of them. The game is CPU-bound on one thread: its busiest thread runs at 97 to 100% while an RTX 5070 GPU averages 54 to 58%. Gameplay runs as TypeScript on V8, with suspected garbage-collection spikes of 175 to 245 ms. Shader-compilation stutter keeps recurring. The PC install is 130 to 150 GB ([pqmlmaoxd performance analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html); [pqmlmaoxd storage analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/03/15/wuthering-waves-memory-bloat.html)). These are community analyses, and their author labels some causes as inferred. Each problem comes from fitting performance architecture onto an engine after the fact, which a new engine can avoid from day one.

### Verdict: the right language, a risky scope

| Area | Rust status, Oct 2026 | Evidence |
|---|---|---|
| Memory safety and threading | Strong | Tiny Glade developer: Rust "doesn't crash in your face" ([80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)) |
| Graphics portability (incl. native Metal) | Strong | wgpu 30 covers Vulkan, Metal, DX12, GLES and WebGPU ([wgpu CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)) |
| ECS and engine foundations | Strong, but changes often | bevy_ecs 0.20, 13 releases in 12 months ([crates.io](https://crates.io/crates/bevy_ecs)) |
| Physics | Good | Rapier 0.36, optional determinism ([Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)) |
| Mobile | Workable, less proven | GameActivity is the Android default in Bevy ([Bevy 0.15](https://bevy.org/news/bevy-0-15/)); ASTC texture compression ([Bevy 0.20](https://bevy.org/news/bevy-0-20/)) |
| Anime-style rendering | Build it yourself | Only `bevy_mod_outline` is actively maintained ([crates.io](https://crates.io/crates/bevy_mod_outline)); `bevy_toon_shader` targets Bevy 0.12 ([docs.rs](https://docs.rs/bevy_toon_shader)) |
| Character animation | Thin | No built-in root motion in Bevy ([bevy#24201](https://github.com/bevyengine/bevy/issues/24201)); no Rust bindings for the ACL compression library ([ACL](https://github.com/nfrechette/acl)) |
| Animated game UI | Build it yourself | The Rive Rust runtime has not been updated since July 2025 ([rive-rs](https://github.com/rive-app/rive-rs)) |
| Audio middleware | Weak | FMOD binding in beta ([fmod-oxide](https://github.com/melody-rs/fmod-oxide)); Wwise binding last released in 2022 ([rrise](https://crates.io/crates/rrise)) |
| Editor | Build it yourself | No Bevy editor as of Aug 2026 ([Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)) |
| Code hot reload | Experimental, development only | No struct changes; iOS simulator only ([docs.rs subsecond](https://docs.rs/subsecond/latest/subsecond/)) |
| Consoles | Private and NDA-only | No PlayStation or Xbox compiler targets ([rustc platform support](https://doc.rust-lang.org/rustc/platform-support.html)) |

**The case for Rust is real.** A solo developer writing a multithreaded engine (job system, asset streaming, parallel rendering) gets compile-time freedom from data races and far fewer memory-corruption bugs. Those are the bugs that sink custom C++ engines that have no QA team. Cargo's dependency management is far better than C++'s. bevy_ecs plus wgpu gives years of foundation work for free.

**The case against is also real.** The commercial middleware that WuWa-class games rely on (Wwise, FMOD Studio, Havok, Bink, platform SDKs) is C/C++ first, and its Rust bindings range from beta to abandoned.

On balance, **if you are going to build an engine at all, Rust is the right language to build it in. The bigger risk is the decision to build an engine, not the choice of language.** Two alternatives deserve a fair hearing:

- **Fyrox 1.0** is a complete Rust engine with an editor. It renders through OpenGL and supports Android but not iOS or consoles. Its team is "quite small with little to no funding", and global illumination is pencilled in for 2.0 ([Fyrox 1.0](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/); [fyrox-graphics-gl](https://crates.io/crates/fyrox-graphics-gl); [Fyrox RC1](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0-rc-1/)).
- **Godot with godot-rust (gdext)**, now at v0.5.x and targeting Godot 4.6/4.7 ([gdext releases](https://github.com/godot-rust/gdext/releases)), is the safest fallback. It keeps Rust for systems code and hands the editor, animation tools and UI to Godot.

## Organize the engine as six layers around one extract boundary

### Crate layers enforce the architecture and the compile times

Every reference engine stacks the same way: platform, core, resources, subsystems, gameplay framework, then game, with tools kept off to the side. What differs is how strictly the dependency rules are enforced. The Machinery enforced its physical design with machine-checked rules, such as headers that may not include other headers, checked in a pre-commit hook, to keep coupling and compile times down ([The Machinery](https://ruby0x1.github.io/machinery_blog_archive/post/physical-design/index.html)). Unreal declares module types (Runtime, Editor, Developer) so that editor code never ships ([Unreal docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules)). In Rust, the crate graph is both the layering mechanism and the main lever on compile time. Use a flat `crates/` directory, a root manifest with no crate of its own (a "virtual manifest"), and `cargo xtask` for automation ([matklad](https://matklad.github.io/2021/08/22/large-rust-workspaces.html)).

| Layer | Crates | Responsibility |
|---|---|---|
| L0 | `platform` | Window, input devices, file and async IO, threads and core affinity, timers. One backend module per OS. Console backends live in private crates. |
| L1 | `core`, `math`, `jobs` | Frame and tagged allocators, pools, generational handles, string IDs, logging, profiler markers. glam-based math with f64 world positions. Work-stealing task graph. |
| L2 | `reflect`, `serialize`, `assets_rt`, `ecs`, `time` | Type registry with stable IDs and versioned schemas. Runtime asset handles, IO scheduler, archive reader. The ECS world. Clock domains. |
| L3 | `render`, `anim`, `physics`, `audio`, `vfx`, `ui`, `nav` | Subsystems that depend only on L0–L2. Third-party physics and audio are wrapped here. |
| L4 | `world`, `gameplay`, `script` | Streaming and world partition, prefabs, abilities, effects and tags, Luau bindings. |
| L5 | `game_*` | Characters, combat content, AI. |
| Tools | `asset_pipeline`, `editor`, `xtask` | Cookers (with heavy encoder dependencies), the editor binary, automation. Never in the shipping dependency tree. |

Copy The Machinery's idea in CI: add a script that fails the build when a lower layer depends on a higher one, or a runtime crate depends on a tools crate. Use cargo features only to select platform or backend, never to switch editor code on inside runtime crates. Cargo merges features across the workspace, so an editor-only feature can easily leak into shipping builds.

The longest dependency chain in the crate graph sets your parallel build time. Keep `core`, `math` and `reflect` small and stable, and push code that expands into many generic copies (ECS queries, serde derives, proc macros) into leaf crates. Wrap physics and audio fully behind your own crate APIs, so a breaking upstream release touches one crate. For the ECS, full abstraction is impractical. Instead, re-export bevy_ecs through your `ecs` crate and pin its version per milestone.

### A hybrid ECS keeps combat logic in one place

There are two main ECS storage layouts:

- **Archetype**: entities are rows and components are columns. Iteration is fast, but adding or removing a component moves the entity to another table.
- **Sparse-set**: adding and removing components is cheap, but iterating over several components at once is slower ([ECS FAQ](https://github.com/SanderMertens/ecs-faq)).

Bevy hit the archetype-move cost in its own renderer and moved render-world data to an `EntityHashMap` to avoid it ([Bevy 0.12](https://bevy.org/news/bevy-0-12/)). Overwatch's ECS had about 46 systems and 103 component types. Roughly 40% of the component types were singletons (single shared instances). Side effects such as impacts were deferred into queues and resolved later, and the main bug source was legacy code that didn't follow the rules ([GDC Vault](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and); [juejin transcript](https://juejin.cn/post/6844903635864780813)). LogLog argued that what most people credit to ECS is really the benefit of generational arenas (indices that can detect stale handles), and preferred "fat components" for game logic ([LogLog Games](https://loglog.games/blog/leaving-rust-gamedev/)). Catherine West's RustConf keynote showed why indices into vectors with generation counters suit Rust's borrow checker ([kyren](https://kyren.github.io/2018/09/14/rustconf-talk.html)). Unity is now merging GameObjects and Entities under one `EntityId` ([Unity, March 2026](https://discussions.unity.com/t/coreclr-scripting-and-ecs-status-update-march-2026/1711852)). The industry is converging on hybrids.

The resulting model has five parts:

- **ECS as the world database.** Use it for everything numerous and spatial: transforms, bounds, render proxies, pose instances, VFX emitters, projectiles, hit volumes and crowds.
- **Characters as entities with a few fat components.** A `CombatController` holds the move state machine, input buffer, abilities and cooldowns as ordinary Rust structs.
- **Resources for global services.** These are input, the camera director, the combat-event queue and the streaming state.
- **Queued side effects.** Hits, damage, VFX, SFX and camera shakes are queued and resolved at fixed points in the frame.
- **Hierarchy only for attachment.** Use parent/child links for things like weapons on bones, not as the organizing structure of the world.

Combat systems run in an explicit fixed order, single-threaded inside the fixed step. Parallelize the wide data passes (animation, physics broadphase, culling, extraction) instead. Bevy 0.20 added `chain_weak` and a schedule-shuffling debug seed because nondeterministic parallel ordering hides bugs ([Bevy 0.20](https://bevy.org/news/bevy-0-20/)).

### A pipelined task graph replaces the game-thread chokepoint

Naughty Dog's PS4 engine had about **100 ms of CPU work per frame** spread across six cores. It ran game logic, render logic and the GPU on different frames at once (a pipeline), which cut game plus render logic from 25 ms run serially to 15.5 ms. Each frame was a data packet (`FrameParams`), not a span of time. Transient memory came from a "tagged heap" of 2 MiB blocks, where each worker thread took its own block, so **99.9% of allocations needed no lock** ([Naughty Dog GDC 2015 slides](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf)). id Tech 7 has "no main or render thread. It's all jobs with one worker thread per core" ([Doom Wiki](https://doomwiki.org/wiki/Id_Tech_7)). Bevy's pipelined rendering typically saved 10 to 30% of frame time ([Bevy 0.10](https://bevy.org/news/bevy-0-10/)).

Rust has **no maintained fiber-based job-system crate**. The fiber crates `zing` and `nylon` died in 2021 ([crates.io](https://crates.io/search?q=fiber%20job)). Fibers also interact badly with thread-local storage and `!Send` borrows. So build a work-stealing task graph in which a thread that has to wait helps execute other tasks, using bevy_tasks or rayon on crossbeam-deque.

The frame then runs in six steps:

1. Pump OS window and input events on the main thread and take an input snapshot.
2. Run zero or more fixed 60 Hz simulation steps, using the accumulator pattern ([Gaffer On Games](https://gafferongames.com/post/fix_your_timestep/)).
3. Run variable-rate presentation: pose evaluation, cloth, VFX and camera, each on its owner's clock.
4. Extract into a `FrameParams`-style packet. This is the only sync point.
5. Prepare and record rendering in parallel, overlapping the next frame's simulation.
6. Submit and present.

This adds about one frame of latency. That is acceptable for action games if you sample input late, but measure end-to-end latency, because perfect-dodge windows are timing-critical. Keep file IO and audio on dedicated threads. Size thread pools explicitly so rayon and the ECS executor don't oversubscribe mobile big cores.

### Clock domains make hitstop and perfect-dodge slow motion compose

Use three clock domains:

- **`real`** for UI, menus, input timestamps and networking.
- **`world`** for global game time, scaled for pause and cinematic slow motion.
- **`local[e]`** for each entity: a stack of time scales.

Hitstop sets the attacker's and victim's local scale to zero for N simulation ticks. Count those ticks in fixed steps, not wall-clock time, so the effect doesn't depend on frame rate. A perfect dodge drops world scale to about 0.1 to 0.3 while the player's local clock counter-scales. The camera runs on its own clock so slow motion never makes it feel laggy. Every timer, cooldown, animation, VFX, audio pitch and physics body must read its owner's clock explicitly. Unreal's model, a global dilation multiplied by a per-actor `CustomTimeDilation`, is the reference ([Unreal API](https://docs.unrealengine.com/4.26/en-US/API/Runtime/Engine/GameFramework/AActor/CustomTimeDilation/)). Its known trap, latent `Delay` nodes that ignore actor dilation, is what to avoid ([Unreal Answers](https://answers.unrealengine.com/questions/364161/view.html)). Masahiro Sakurai treats hitstop as a core feel tool for every genre ([Anime News Network](https://cdn.animenewsnetwork.com/interest/2022-08-24/masahiro-sakurai-launches-youtube-channel-about-game-development/.188941)). Make the combat sim deterministic for a given input stream: fixed system order, no dependence on hash-map iteration order, and RNG seeded per encounter. That buys you bug reproduction from input replays, automated combat regression tests and a path to co-op.

### Hitches are an architecture problem, not a tuning problem

A PSO (pipeline state object) is the GPU's compiled shader-plus-render-state; building one on demand mid-game causes stutter.

- **Pre-compile PSOs.** Unreal compiles every PSO a component might need right after it loads, and skips drawing the object if its PSO isn't ready ([Unreal docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/pso-precaching-for-unreal-engine)). WuWa still needed shader-compile fixes in versions 1.4 and 2.2 plus a custom `KuroPSOTools` plugin ([pqmlmaoxd](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)).
- **Alias transient GPU memory.** Frostbite's frame graph shares memory between short-lived render targets, which cut 4K render-target memory from **1042 MB to 472 MB** ([FrameGraph slides](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite)).
- **Extract only changes.** Bevy's per-column change ticks gave a **132x speedup** in mesh extraction ([Bevy 0.20](https://bevy.org/news/bevy-0-20/)).

The rules that follow:

- Materials declare their full set of shader variants at cook time.
- Packages list the PSOs they need. Loading a cell or a character warms those PSOs in the background and keeps the object hidden until they are ready.
- On Vulkan and Android, save wgpu's `PipelineCache` to disk ([wgpu CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)).
- No synchronous IO, decompression or script garbage collection on the simulation thread.
- Spawn entities from streamed data over several frames, never all at once.
- In steady state, make zero per-frame heap allocations, and check it with a debug allocation counter.

As a rule of thumb (not a sourced figure), target 12 to 13 ms of CPU critical path at 60 fps on PC and console. On mobile, design for 30 fps as the baseline and 60 fps on high-tier devices, measured at thermally throttled clock speeds, not peak.

| Day-one rule | What it prevents | Precedent |
|---|---|---|
| All gameplay time reads an owner clock | Hitstop and slow-motion bugs; feel that changes with frame rate | [Unreal CustomTimeDilation](https://docs.unrealengine.com/4.26/en-US/API/Runtime/Engine/GameFramework/AActor/CustomTimeDilation/) |
| All side effects go through queues resolved at fixed points | Overlapping `&mut` borrows; ordering bugs | [Overwatch](https://juejin.cn/post/6844903635864780813) |
| The renderer sees only extracted plain data | Coupling between simulation and rendering; blocked pipelining | [Naughty Dog](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf), [Bevy 0.10](https://bevy.org/news/bevy-0-10/) |
| Assets are referenced by GUID and cooked per platform | Links breaking on rename; mobile and PC drifting apart | [The Truth](https://ruby0x1.github.io/machinery_blog_archive/post/the-story-behind-the-truth-designing-a-data-model/index.html) |
| No editor code in runtime crates | Editor features leaking into shipping builds | [Unreal modules](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules) |
| Gameplay is data first; scripts run per event, not per tick; hot paths stay in Rust | Garbage-collection spikes; a CPU-bound game thread | [WuWa analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html) |
| Every per-frame allocation carries a lifetime tag | Heap churn; lock contention | [Naughty Dog tagged heap](http://www.swedishcoding.com/wp-content/uploads/2015/03/parallelizing_the_naughty_dog_engine_using_fibers.pdf) |

## A custom forward+ anime renderer on wgpu is where the look lives

### wgpu is the only multi-backend Rust GPU layer worth betting on

**wgpu 30.0.1 (August 2026)** releases a major version about every quarter. It covers Vulkan, Metal, DX12, GLES and WebGPU, and has moved quickly:

- **v28:** mesh shaders.
- **v29:** a Metal backend on autogenerated `objc2` bindings, Metal ray-tracing acceleration structures, and loading of Microsoft's D3D12 Agility SDK.
- **v30:** full mesh shaders including WGSL, bindless storage buffers on Metal, HDR surfaces, and expanded hooks for custom backends.

Sources: [wgpu CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md); [v28 release](https://github.com/gfx-rs/wgpu/releases/tag/v28.0.0). Bevy 0.20 and Zed both depend on it ([bevy_render deps](https://crates.io/crates/bevy_render); [Zed Cargo.toml](https://github.com/zed-industries/zed/blob/main/Cargo.toml)).

For a macOS developer, wgpu means native Metal. The alternatives are weaker:

- **Raw Vulkan through `ash`** would run through the MoltenVK translation layer on a Mac, and ash's last crates.io release is 0.38.0 from April 2024 ([crates.io](https://crates.io/crates/ash)).
- **metal-rs** describes itself as deprecated ([GitHub](https://github.com/gfx-rs/metal-rs)).
- **blade** has no DX12 backend and does ray tracing only on Vulkan ([blade README](https://github.com/kvark/blade/blob/main/blade-graphics/README.md)).

wgpu's gaps are known:

- No async compute or multiple queues found in 30.x; only a `default_queue` descriptor has appeared on the development branch.
- No pixel local storage or framebuffer fetch (the tile-memory features that mobile deferred renderers rely on).
- A Metal limit of 29 combined buffer-argument slots.
- Validation overhead that has never been rigorously measured. A 2021 anecdote put it at 10 to 20% extra CPU cost for well-optimized, draw-heavy work ([r/rust_gamedev](https://redlib.hbubli.cc/r/rust_gamedev/comments/p2ihhz/wgpu_vs_vulkan)).

Mitigate these in three ways. Put a thin engine graphics trait (RHI) that mirrors wgpu's concepts between the engine and wgpu, so an NDA console backend can be added later. Drive submission from the GPU with multi-draw indirect, which has been available on all devices with indirect execution since v27. And write your own render graph, because no maintained crate exists ([crates.io](https://crates.io/crates/render-graph)).

### Clustered forward+ beats deferred for anime characters on wgpu

**The case for deferred.** Kuro chose deferred shading for WuWa because forward rendering "can cause shader variants to increase sharply", and because deferred eased SSR, GTAO, weather and time-of-day changes. On mobile, its "one-pass deferred" path uses on-chip Mali pixel local storage to save about **1.5 GB/s of bandwidth at 60 fps** ([GCORES](https://www.gcores.com/articles/181970)). But cel shading on top of deferred required engine surgery. Blue Protocol had to compute N·L (the basic light-facing term) in UE4's base pass because the G-buffer could not hold two normals, and added an extra G-buffer just for outlines ([GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html)).

**The case for forward+ on wgpu.** A custom engine is better served by clustered forward+ with a thin prepass that writes depth, normals, material/character IDs and velocity:

- MSAA stays available for thin lines.
- Ramp, ID and material lookups don't need to be packed into a G-buffer.
- Transparent hair and eyes fit naturally.
- wgpu doesn't expose pixel local storage, so WuWa's mobile trick is out of reach anyway.

Contain shader-variant growth with two "uber-shaders": one PBR-style shader for the environment and one anime shader for characters. A material ID picks the ramp row, each shader family gets no more than about eight static variants, and dynamic branches on uniform flags handle the rest.

The frame runs in thirteen passes:

1. GPU frustum and occlusion culling (Hi-Z), writing indirect draw arguments.
2. Depth, normal, ID and velocity prepass.
3. Compute work: light clustering, GTAO, contact shadows and SSR.
4. Cached shadow cascades.
5. Forward opaque pass.
6. Inverted-hull outlines.
7. Hair, eyes and transparents, using stencil tricks.
8. Volumetric fog.
9. Particles and distortion.
10. TAA or upscaling, with a responsive mask.
11. Bloom.
12. LUT-based tonemapping and color grading.
13. UI.

Leave visibility buffers and mesh shaders for later. Anime scenes are rarely bound by triangle count.

### The character recipe is well documented by its imitators

Every shipped anime title uses the same core. N·L is thresholded into two tones. Painted per-pixel masks (the "ILM" or lightmap texture) shift that threshold, and vertex colors carry ambient occlusion and outline data. Shadow colors come from ramp textures rather than from darkening the base color. Fan reconstructions of Genshin's lightmap use R for a specular mask, G for a painted shadow mask, B for the specular threshold and A for a material ID that selects the ramp row ([HoyoToon wiki](https://github.com/Melioli/HoyoToon/wiki/Using-the-Genshin-Shader)). This layout comes from datamined materials and should be checked against shader source.

Specific parts of the recipe:

- **Faces** use a signed-distance-field shadow threshold map evaluated in head space and mirrored for light from the other side ([URPSimpleGenshinShaders](https://github.com/NoiRC256/URPSimpleGenshinShaders)). Hi-Fi Rush credits this technique to a 2018 miHoYo talk ([GCORES](https://www.gcores.com/articles/181912)). Artists author it by painting shadow shapes at many light angles and merging them with an SDF generator ([Face Shadow Map workflow](https://erichu33.notion.site/Face-Shadow-Map-Creation-Baking-Workflow-d3b8769021e04683a2f2ae4cf16ac810)).
- **Hair and eyes.** Honkai Impact 3rd used tangent-based anisotropic hair with multi-layer specular, and faked eye refraction by offsetting UVs along the view direction ([Unite Tokyo 2018](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd)).
- **Self-shadowing.** Blue Protocol keeps cel-shaded surfaces out of shadow-map self-shadowing, has point, sky and indirect light ignore normals, and darkens legs slightly to ground the character ([GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html)).
- **Character lighting.** WuWa runs a separate character-lighting pipeline with brightness curves fitted to time of day, a custom face-shadow method that includes vertical shadow maps for top light, and blendshapes with RBF deformation correction ([GCORES](https://www.gcores.com/articles/181970)).

The practical takeaway: give each character an artist-curved virtual light and keep shadow maps from multiplying the ramp directly.

### Outlines and anti-aliasing must be designed together

Most shipped titles draw outlines as inverted hulls: back faces extruded in the vertex shader. Width comes from vertex color and is corrected for perspective. Honkai Impact 3rd also used crease-edge and image-space methods ([Unite Tokyo 2018](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd)). Gaps at hard edges "can only be fixed by smoothing normals", so bake smoothed normals into a spare vertex channel at import ([Delt06 wiki](https://github.com/Delt06/urp-toon-shader/wiki/Outline); [StarRailNPRShader](https://github.com/stalomeow/StarRailNPRShader)). Blue Protocol drew outlines in a post-process from an extra G-buffer and found that "TemporalAA softens outlines by about one pixel". It fixed this with a stencil mask and Unreal's ResponsiveAA ([GameBusiness.jp](https://www.gamebusiness.jp/article/2021/09/13/19051.html)). Plan for this up front. Use hull outlines on characters, plus an optional screen-space edge pass driven by prepass normals and IDs for the environment. Then handle TAA in one of three ways: a responsive mask from the character stencil, compositing outlines after TAA, or MSAA on the character layer.

### Environment and post-processing: bright, clean and artist-curved

Genshin's console team aimed for "fresh, bright, clean, anime-like" rather than "dirty, noisy, dark", and avoided "cool-looking technologies" that did not fit. Its environment stack included:

- Up to **8 shadow cascades**, with the first four updated every frame and the rest interleaved.
- Capsule ambient occlusion for characters.
- Spherical-harmonic (SH) ambient lighting.
- Volumetric fog under 1 ms.
- HDR10 output, and a rejection of ACES tonemapping as not fitting the style; the tone curve is folded into a color LUT instead.

Source: [Unity Dojo 2021](https://www.docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi). WuWa replaced Unreal's physical atmosphere with a custom stylized sky, swapped volumetric clouds for an animated 2D skybox, and tiered its vegetation from card meshes up close to billboards and impostors in the distance ([GCORES](https://www.gcores.com/articles/181970)). Honkai Impact 3rd's bloom combines four downsampled levels (1/4 to 1/32 size) and applies distortion after bloom ([Unite Tokyo 2018](https://docswell.com/s/UnityJapan/KWLQP5-unite-tokyo-20183rd)).

ZZZ's end-of-battle freeze frame "slathers the screen in yellow while turning characters monochrome" ([Inverse](https://inverse.com/gaming/is-zenless-zone-zero-hoyoverse-good-or-bad)). Effects like this are post-process materials triggered by gameplay events. Hi-Fi Rush's SDF halftone dots are a documented template for them ([GCORES](https://www.gcores.com/articles/181912)).

Ray tracing is a late, optional extra tier. ZZZ's PC version gained ray-traced reflections, AO and GI only in version 3.0 ([Tom's Hardware Italy](https://www.tomshw.it/videogioco/zenless-zone-zero-alza-lasticella-con-ray-tracing-e-dlss-45)). Bevy's Solari path tracer is a useful ReSTIR reference on wgpu but has no built-in denoiser on macOS ([Bevy 0.20](https://bevy.org/news/bevy-0-20/)).

### Shaders: WGSL plus WESL, with Slang as an offline option

**WESL** is WGSL (WebGPU's shading language) extended with imports and conditional compilation. **Bevy rewrote all its shaders in WESL in 0.20**, and the older `naga_oil` preprocessor is archived ([Bevy 0.20](https://bevy.org/news/bevy-0-20/); [naga_oil](https://github.com/bevyengine/naga_oil)). Use the `wesl` crate (0.6.0) with naga to validate and reflect shaders, and `notify` to recompile pipelines in the background when files change ([crates.io](https://crates.io/crates/wesl)). Slang is the stronger HLSL-like language, but its Metal and WGSL targets are marked "work in progress" and its Rust bindings are at 0.1.0 ([Slang targets](https://shader-slang.org/slang/user-guide/targets.html); [crates.io](https://crates.io/crates/shader-slang)). If you want Slang, run the `slangc` command-line compiler offline and pass its SPIR-V or MSL output to wgpu as passthrough shaders. rust-gpu 0.11 is self-described as "not yet production-ready" and outputs only SPIR-V ([Rust-GPU](https://github.com/Rust-GPU/rust-gpu)).

No Rust project implements a Genshin-class anime character pipeline. You will port techniques from Unity and Unreal community replications, with `bevy_mod_outline` as the only maintained Rust NPR crate to study.

## Make combat, content and tools data-driven from day one

### Combat is a data-driven move graph on the fixed step

Model combat as data-driven moves running on the fixed-step simulation.

**Input.** Sample input into timestamped intents (attack, heavy, skill, dodge, swap) held in a ring buffer about 8 to 15 ticks deep. Give each intent an expiry and a priority, and consume intents only inside the fixed step.

**Move graph.** Each character's move graph is a data asset. Each move pairs an animation with a frame-data timeline carrying these tracks:

- startup, active and recovery phase markers;
- hitboxes attached to bones, with hit-group IDs so multi-hit attacks don't double-count;
- hurtboxes, plus invulnerability and perfect-dodge windows;
- cancel windows;
- root-motion and motion-warp segments;
- cues for VFX, SFX, camera shake, hitstop amount and rumble.

Edges in the graph are cancel rules. God of War's design vocabulary fits well. A buffered input runs when its window opens; an instant input runs only if pressed while the window is open. Partial cancels are allowed only in specific windows; complete cancels are allowed at any time. Their example is a 60-frame attack whose hit window is frames 15 to 20 ([Game Developer](https://www.gamedeveloper.com/pc/combat-canceled-i-god-of-war-i-action-game-design)).

**Tick order.** Each fixed step runs in a deterministic order:

1. Consume intents.
2. Evaluate cancels.
3. Advance move time on the local clock.
4. Apply root motion and the character controller.
5. Test hitboxes against hurtboxes.
6. Queue `HitEvent`s.
7. Resolve hits through attributes, effects and tags.
8. Emit cues.

**Abilities and effects.** Copy the structure of Unreal's Gameplay Ability System (GAS): attributes, gameplay effects, tags, and cues that are presentation-only ([Unreal GAS docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/gameplay-ability-system-for-unreal-engine)). Because cues are presentation-only, a headless combat sim can replay recorded inputs as regression tests. A live game needs that: WuWa keeps adding mechanics such as December 2025's "Tune Break" ([Wikipedia](https://en.wikipedia.org/wiki/Wuthering_Waves)).

**Party swap.** Treat swapping as an ability. Keep all three party members spawned and ticking, which means combat memory is about three characters' worth of movesets, VFX, audio banks and PSOs. Warm those up when the party changes, not at swap time.

**Enemy AI.** Enemies feed intents into the same move system the player uses, so designers build bosses in the same timeline editor. An encounter-level director hands out "attack tokens" so crowds don't all strike at once.

### Three tiers keep content patchable without shipping native code

1. **Data assets** (move graphs, frame data, effect definitions) carry most per-patch content.
2. **Luau, embedded through mlua,** carries logic that must change between client releases.
3. **Rust** carries the engine and the hot path. Its hot-patching is a development tool only.

This split is forced by platform rules. iOS bans JIT code generation, and Apple's guideline 2.5.2 forbids apps that "download, install, or execute code which introduces or changes features or functionality of the app" ([App Store Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)). Even so, Chinese gacha games routinely hot-update interpreted code. They use Tencent's xLua (Lua), HybridCLR (a C# interpreter whose vendor claims "nearly 100" of the iOS free top-500 apps use it) and Tencent's PuerTS (TypeScript) ([xLua](https://github.com/Tencent/xLua); [HybridCLR](https://github.com/focus-creative-games/hybridclr); [PuerTS](https://github.com/Tencent/puerts)). Community reverse-engineering finds PuerTS on V8 inside WuWa, with C# assemblies appearing from version 2.8 onward ([pqmlmaoxd](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)).

Luau runs Roblox's game code, adds type annotations and is designed for sandboxing ([Luau](https://github.com/luau-lang/luau)). mlua supports Luau with coroutine-based async. Leave its `luau-jit` feature off on iOS ([mlua](https://github.com/mlua-rs/mlua)). Rhai's tree-walking interpreter runs 1 million iterations in 0.14 s, which is fine for designer tunables but not for per-frame logic over hundreds of entities ([Rhai](https://github.com/rhaiscript/rhai)).

Whatever VM you choose must meet five constraints:

- It runs without JIT.
- Its garbage-collection pauses are bounded.
- It is called on events (move start, hit, window open), not every tick.
- Its typed bindings are generated from your engine's reflection registry.
- It is sandboxed and deterministic.

Get legal and App Review advice before relying on code hot-patching on iOS.

### Copy World Partition for streaming, and budget activation

Unreal's World Partition is the reference model. It has one logical world split into grid cells and "streaming sources" that load the cells around them. It adds data layers, one file per placed actor, and simplified stand-in geometry (HLOD) generated for each cell. The docs give 256 m cells with a 768 m loading radius as an example ([Unreal docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/world-partition-in-unreal-engine)). Unreal's large-world coordinates use doubles on the CPU. Its Niagara particle system keeps floats by storing a grid cell plus an offset within it ([Unreal docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/large-world-coordinates-in-unreal-engine-5)). Avian 0.8 took the same approach as Jolt: f64 only for body positions and world-space coordinates ([Avian 0.8](https://joonaa.dev/blog/14/avian-0-8)). WuWa ships custom `KuroWorldPartition` and `FastGeoStreaming` plugins and a 400 MB mobile texture pool ([pqmlmaoxd](https://pqmlmaoxd.github.io/gamedev/analysis/2026/04/12/wuthering-waves-performance-issues.html)).

Design streaming in three stages:

- **Authoring:** store every placed object as its own record with a GUID.
- **Cook:** assign objects to cells and generate HLOD for each cell.
- **Runtime:**
  - Rank streaming sources by priority: player position plus velocity look-ahead, fast-travel target, cutscene camera.
  - Read through the platform's async IO.
  - Decode on worker jobs.
  - Activate at a budgeted number of entities or microseconds per frame. That budget is where hitches are won or lost.

Combat arenas should freeze streaming and pre-warm the enemy movesets, VFX and PSOs an encounter needs before the fight starts. Players notice a hitch mid-combat far more than one while traveling. Building ZZZ-style zones instead of a seamless world shrinks this whole subsystem.

### The cook pipeline is a content-addressed compiler

Every source asset gets a GUID in a sidecar file, and references go by GUID, never by path. Importers turn glTF and FBX into an engine-neutral intermediate form. Cookers turn that into per-platform blobs: BCn textures for PC, ASTC for mobile, compressed animation, optimized meshes. Each output is cached under a hash of the source content, cooker version, settings, target platform and dependency hashes.

Two existing designs to copy:

- **Unreal's Derived Data Cache** layers local, shared and cloud stores ([Unreal docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-derived-data-cache-in-unreal-engine)).
- **Bevy's asset processor** records hashes in `processed_info` and reprocesses an asset whenever one of its dependencies changes ([Bevy 0.12](https://bevy.org/news/bevy-0-12/)).

Runtime formats should load in place, mapped straight from disk with no parsing step: relocatable offsets, rkyv archives and memory mapping. Keep serde for editor and intermediate formats. Run the cooker as a separate process, so a crash in a third-party encoder can't take down the editor.

Package per cell, per character and per encounter, with manifests of content-hashed chunks so patches download only what changed. WuWa shows the cost of skipping this: datamined file counts grew about 81% from version 1.0 to 3.2, alongside a PC install of 130 to 150 GB ([pqmlmaoxd storage analysis](https://pqmlmaoxd.github.io/gamedev/analysis/2026/03/15/wuthering-waves-memory-bloat.html)).

### The editor is a document model first and a UI second

The Machinery's editor data model, "The Truth", treated the in-memory data as authoritative and recorded every change as (object, property, old value, new value). Undo, copy/paste and real-time collaboration all fell out of that one design. Its authors reported that "having a single in-memory representation of the editor state has made implementing UI and tools a lot easier" ([The Truth](https://ruby0x1.github.io/machinery_blog_archive/post/the-story-behind-the-truth-designing-a-data-model/index.html)). The same post explains the alternative it replaced: Bitsquid's JSON-files-as-truth design made every tool write its own undo.

Build the editor like that:

- **Play-in-editor** starts a fresh runtime world from a snapshot of the current documents. Game code never mutates documents.
- **Live link to device builds** pushes change records over a socket. That is the only practical way to tune frame data on a phone.
- **A combat timeline editor** gives the highest return for this genre. It has tracks for hitboxes, cancel windows, invulnerability, root motion and cues, and scrubs with a live preview character and an enemy dummy. Kuro's own modifications to Unreal's Sequencer, made so that most visual elements can be driven from cinematics, show how much the genre relies on this kind of tooling ([GCORES](https://www.gcores.com/articles/181970)).

Use Blender for modeling and level blockout, exporting glTF with custom-property extras. Bevy's Skein add-on implements this pattern ([bevy_skein](https://crates.io/crates/bevy_skein)). Then the in-engine editor only has to cover gameplay data, combat tuning, VFX timing and cinematics. Fyrox's editor, with its animation editor and animation state-machine editor, is the best open-source Rust reference for editor architecture ([Fyrox 1.0](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/)).

## Use these crates, vendor the fragile ones, avoid the dead ones

The tables below record the October 2026 picks. Versions and dates come from crates.io and GitHub as of 24 October 2026. Third-party crates sit behind your own layer crates.

### Rendering and shaders

| Area | Use | Alternatives and cautions |
|---|---|---|
| GPU API | `wgpu` 30.x behind a thin engine graphics trait ([crates.io](https://crates.io/crates/wgpu)) | `ash` only if Vulkan-only; `blade` 0.9 (no DX12); `metal` deprecated, use `objc2-metal` |
| Shader language | WGSL + `wesl` 0.6 + `naga` 30 ([crates.io](https://crates.io/crates/wesl)) | Slang offline via `slangc`; rust-gpu not production-ready; `naga_oil` archived |
| GPU data layout | `encase` 0.13, `bytemuck`, `wgsl_bindgen` 0.23 ([encase](https://crates.io/crates/encase); [wgsl_bindgen](https://crates.io/crates/wgsl_bindgen)) | — |
| Render graph | Write your own | `screen-13` 0.13 (Vulkan) as a reference ([crates.io](https://crates.io/crates/screen-13)) |
| Textures | `ctt` 0.6 offline (BCn for desktop, ASTC for mobile) into KTX2 via `ktx2` 0.5 ([docs.rs ctt](https://docs.rs/ctt); [ktx2](https://crates.io/crates/ktx2)) | `basis-universal` crate stale since 2023 ([crates.io](https://crates.io/crates/basis-universal)) |
| Meshes | `meshopt` 0.7 for vertex-cache optimization, LOD simplification and meshlets ([crates.io](https://crates.io/crates/meshopt)) | `metis` only for Nanite-style cluster hierarchies |
| Upscaling | `dlss_wgpu` 6.0 on NVIDIA ([crates.io](https://crates.io/crates/dlss_wgpu)); MetalFX via `objc2-metal-fx` ([crates.io](https://crates.io/crates/objc2-metal-fx)) | `fsr` stale since 2024 ([crates.io](https://crates.io/crates/fsr)); no XeSS crate |
| GPU particles | Custom compute particles; study `bevy_hanabi` 0.19 ([crates.io](https://crates.io/crates/bevy_hanabi)) | — |
| Profiling GPU | `wgpu-profiler` 0.28 ([crates.io](https://crates.io/crates/wgpu-profiler)) | Xcode Metal frame capture on Apple platforms |

### Core runtime and data

| Area | Use | Alternatives and cautions |
|---|---|---|
| ECS | `bevy_ecs` 0.20 standalone, pinned per milestone ([crates.io](https://crates.io/crates/bevy_ecs)) | `hecs` 0.11 (minimal, stable); `flecs_ecs` is alpha ([Flecs-Rust](https://github.com/Indra-db/Flecs-Rust)); `specs`, `legion` and `evenio` are dead |
| Math | `glam` 0.34; `DVec3` for world positions ([glam](https://github.com/bitshifter/glam-rs)) | `wide` 1.7 for SIMD hot loops; `nalgebra` for tools only; `std::simd` still nightly-only ([rust#86656](https://github.com/rust-lang/rust/issues/86656)) |
| Concurrency | `bevy_tasks` for system scheduling; `rayon` 1.12 for data-parallel loops; `crossbeam`/`flume` channels; `parking_lot` locks ([rayon](https://crates.io/crates/rayon)) | `tokio` only for online services; build fibers on `corosensei` only as R&D |
| Memory | `mimalloc` global allocator; `bump-scope` 2.4 or `bumpalo` 3.20 per-frame arenas; std `Allocator` trait from Rust 1.100 ([bump-scope](https://crates.io/crates/bump-scope); [PR](https://github.com/rust-lang/rust/pull/156882)) | `allocator-api2` polyfill; `generational-arena` is archived ([crates.io](https://crates.io/crates/generational-arena)) |
| Handles | `slotmap` 1.1 ([crates.io](https://crates.io/crates/slotmap)) | `slab` |
| Serialization | `rkyv` 0.8 for cooked data; `serde` + `ron`/`toml` for authored data; `postcard` 1.1 for network and saves ([rkyv](https://crates.io/crates/rkyv); [postcard](https://crates.io/crates/postcard)) | `bincode` 3.0.0 is a `compile_error!` tombstone ([crates.io](https://crates.io/crates/bincode/3.0.0)) |
| Reflection | `bevy_reflect` 0.20 ([crates.io](https://crates.io/crates/bevy_reflect)) | `facet` (44 releases in 12 months) ([GitHub](https://github.com/facet-rs/facet)) |
| Hashing | `foldhash`/`rustc-hash` for maps; `xxhash-rust` for checksums; `blake3` for cache keys ([blake3](https://crates.io/crates/blake3)) | — |
| RNG | `rand_pcg`/`rand_xoshiro`, seeded per subsystem ([rand_pcg](https://crates.io/crates/rand_pcg)) | Gacha results always server-side |

### Gameplay subsystems

| Area | Use | Alternatives and cautions |
|---|---|---|
| Physics | `rapier3d` 0.36 (or `rapier3d-f64`) behind your own crate; `parry3d` 0.31 for hitbox queries ([Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)) | Jolt via the `joltc` C wrapper (`oxijolt` 1.2) as the AAA escape hatch ([oxijolt](https://github.com/pockerhead/oxijolt)); Avian only if you adopt Bevy wholesale; `physx-rs` archived ([GitHub](https://github.com/EmbarkStudios/physx-rs)) |
| Character controller | Custom kinematic controller built on shape-casts | `bevy_ahoy` as a feature reference ([GitHub](https://github.com/janhohenheim/bevy_ahoy)) |
| Animation | `ozz-animation-rs` 0.11, vendored (MPL-2.0) ([GitHub](https://github.com/SlimeYummy/ozz-animation-rs)) | `bevy_animation` has no root motion; no ACL bindings; motion matching only as prototypes ([bevy_motion_matching](https://github.com/voxell-tech/bevy_motion_matching)) |
| Secondary motion | Port the VRM spring-bone spec; write an XPBD skirt and cloth solver ([VRM spec](https://github.com/vrm-c/vrm-specification/blob/master/specification/VRMC_springBone-1.0/README.md)) | `bevy_vrm1` as a reference implementation ([GitHub](https://github.com/not-elm/bevy_vrm1)) |
| Audio | FMOD Studio through your own bindgen layer, modeled on `fmod-oxide` ([GitHub](https://github.com/melody-rs/fmod-oxide)) | `kira` 0.12 ([GitHub](https://github.com/tesselode/kira)); Firewheel 0.14 ([GitHub](https://github.com/BillyDM/Firewheel)); Steam Audio via `audionimbus`; no maintained Wwise binding |
| Scripting | Luau via `mlua` 0.12, interpreter only on iOS ([mlua](https://github.com/mlua-rs/mlua)) | `rhai` for config expressions; `wasmtime` with its Pulley interpreter, or `wasmi`, for mods ([Wasmtime docs](https://docs.wasmtime.dev/examples-pulley.html)) |
| Code hot reload (dev) | `subsecond` 0.7 ([docs.rs](https://docs.rs/subsecond/latest/subsecond/)) | A C-ABI plugin boundary in the style of The Machinery ([Machinery](https://ruby0x1.github.io/machinery_blog_archive/post/dll-hot-reloading-in-theory-and-practice/index.html)) |
| Navigation | `rerecast` 0.4 for navmesh baking + `landmass` 0.9 for paths, steering and avoidance ([rerecast](https://github.com/janhohenheim/rerecast); [landmass](https://github.com/andriyDev/landmass)) | `polyanya` for any-angle paths; `oxidized_navigation` deprecated ([GitHub](https://github.com/TheGrimsey/oxidized_navigation)) |
| AI decisions | `statig` 0.4 hierarchical state machines; `bonsai-bt` 0.14 behavior trees; in-house utility scoring and attack-token director ([statig](https://github.com/mdeloof/statig); [bonsai](https://github.com/Sollimann/bonsai)) | `big-brain` stalled and moved to Codeberg ([GitHub](https://github.com/zkat/big-brain)) |
| Input mapping | An action layer modeled on Unreal's Enhanced Input (contexts, modifiers, triggers), with tick-stamped buffers ([bevy_enhanced_input](https://github.com/simgine/bevy_enhanced_input)) | `leafwing-input-manager` (Bevy-coupled) |
| Networking | `tokio` + `quinn` 0.11 or `renet2` 1.0 for co-op; HTTP/gRPC for gacha services ([quinn](https://crates.io/crates/quinn); [renet2](https://crates.io/crates/renet2)) | `lightyear` as a prediction reference; `laminar` dead ([GitHub](https://github.com/TimonPost/laminar)) |

### Platform, UI, tools and pipeline

| Area | Use | Alternatives and cautions |
|---|---|---|
| Window, input and platform | SDL3 via `sdl3-sys` 0.7 (SDL 3.4.18) behind your `platform` crate ([crates.io](https://crates.io/crates/sdl3-sys); [SDL3 gamepad API](https://wiki.libsdl.org/SDL3/CategoryGamepad)) | `winit` 0.30 + `gilrs` 0.11 (no Android gamepads, no macOS force feedback) ([gilrs](https://github.com/Arvamer/gilrs)) |
| Mobile packaging | `cargo-ndk` 4.1 + hand-maintained Gradle and Xcode projects ([GitHub](https://github.com/bbqsrc/cargo-ndk)) | `cargo-mobile2` is supported only for Tauri ([GitHub](https://github.com/tauri-apps/cargo-mobile2)) |
| Game UI | Your own retained UI: `taffy` 0.15 layout + `parley` 0.12 text (on `harfrust`/`skrifa`) + an MSDF glyph atlas + your own batcher + a tween/timeline system ([taffy](https://github.com/DioxusLabs/taffy); [parley](https://crates.io/crates/parley)) | `noesis_runtime` for designer tooling, but it is a small third-party binding ([docs.rs](https://docs.rs/noesis_runtime)); `rustybuzz` archived ([GitHub](https://github.com/harfbuzz/rustybuzz)) |
| Debug UI and editor | `egui` 0.37 + `egui_dock`/`egui_tiles` + `transform-gizmo` 0.11 + `egui-snarl` 0.12 + `egui_plot` ([egui](https://github.com/emilk/egui); [transform-gizmo](https://github.com/urholaukkarinen/transform-gizmo)) | `dear-imgui-rs` 0.21 with ImGuizmo, ImPlot and node editor ([GitHub](https://github.com/Latias94/dear-imgui-rs)) |
| Import | `gltf` 1.4; `ufbx` 0.11 for FBX animation ([GitHub](https://github.com/ufbx/ufbx-rust)) | `openusd` 0.8 optional ([GitHub](https://github.com/mxpv/openusd)); skip MMD/PMX |
| Packaging and IO | Custom archive with a table of contents; `zstd`/`lz4_flex`; `memmap2`; `notify` 8.2 with a debouncer ([notify](https://crates.io/crates/notify)) | `ruzstd` for pure-Rust decoding; `distill` abandoned ([crates.io](https://crates.io/crates/distill)) |
| CPU profiling | `profiling` crate macros feeding Tracy (`tracy-client` 0.19); `puffin` 0.20 for in-game flame graphs ([rust_tracy_client](https://github.com/nagisa/rust_tracy_client); [puffin](https://github.com/EmbarkStudios/puffin)) | Superluminal on Windows and consoles ([Superluminal](https://superluminal.eu/applications/)); `optick` abandoned |
| Crash reporting | `crash-handler` 0.8 + `minidumper` 0.11 + Sentry 0.49.3 minidumps ([crash-handling](https://github.com/EmbarkStudios/crash-handling); [Sentry](https://docs.sentry.io/platforms/rust/native-crash-reporting/)) | iOS needs a native crash SDK |
| Build and CI | `sccache` 0.18, `cargo-hakari`, `cargo-nextest`, `cargo-deny` ([cargo-nextest](https://crates.io/crates/cargo-nextest); [cargo-deny](https://crates.io/crates/cargo-deny)) | `bacon` and `dssim` are AGPL ([dssim](https://crates.io/crates/dssim)) |
| Localization | `fluent` 0.17 + ICU4X (`icu` 2.4) ([fluent-rs](https://github.com/projectfluent/fluent-rs); [icu](https://crates.io/crates/icu)) | `rust-i18n` |
| Platform services | `steamworks` 0.13 ([GitHub](https://github.com/Noxime/steamworks-rs)) | Your own FFI for EOS, Discord, PSN and Xbox; no public crates exist ([crates.io](https://crates.io/search?q=playstation)) |
| Video | AV1 via `rav1d`/`dav1d` with your own player ([rav1d](https://github.com/memorysafety/rav1d)), or `ffmpeg-next` after a license review | Bink 2 through its official SDK; `bevy_cutscene` doesn't continuously re-sync audio and video ([Codeberg](https://codeberg.org/izarma/bevy_cutscene)) |
| Testing | `insta` snapshots, `nv-flip` golden images, `criterion` ([insta](https://crates.io/crates/insta)) | — |

### Five picks that deserve an argument

**bevy_ecs over hecs.**

- bevy_ecs is the only Rust ECS with relationships, observers, change detection, a parallel scheduler and reflection all under active development. Tiny Glade shipped it with a custom renderer.
- A self-reported benchmark puts Bevy 1.2 to 2.9x behind hecs on micro-tests. But its full "gameplay scenario" frame over 65,536 entities still costs only 201 µs, against hecs's 139 µs ([SkyECS benchmarks](https://github.com/jz315/SkyECS/blob/main/benches/BENCHMARKS.md)). That gap is irrelevant against a 16.6 ms frame.
- The real cost is a breaking release every few months.

**Rapier over Jolt and Avian.**

- Rapier is engine-agnostic and Apache-licensed. It moved its public API to glam in January 2026 ([Dimforge](https://dimforge.com/blog/2026/01/09/the-year-2025-in-dimforge)).
- Its 0.35 release made one benchmark scene step about **90x faster while stationary**, and its deterministic mode can now run in parallel with bitwise-identical results ([Rapier CHANGELOG](https://github.com/dimforge/rapier/blob/master/CHANGELOG.md)).
- Jolt, with credits on Horizon Forbidden West and Death Stranding 2 and console support ([JoltPhysics](https://github.com/jrouwe/JoltPhysics)), is the escape hatch if console certification or large-world precision requires it.

**FMOD over pure-Rust audio.**

- Adaptive combat music, thousands of voice lines in several languages, and certification support favor FMOD's authoring tools.
- No Rust FMOD binding is console-verified, so own the binding yourself.
- Kira is the best pure-Rust fallback. Firewheel is planned to become Bevy's default audio engine ([Firewheel](https://github.com/BillyDM/Firewheel)).

**SDL3 over winit.**

- An action game needs DualSense and Xbox rumble, trigger rumble, gyro and LED control, and SDL3 exposes all of them ([SDL3 wiki](https://wiki.libsdl.org/SDL3/CategoryGamepad)). It also supports Xbox's GDK at the C level ([SDL README-gdk](https://skia.googlesource.com/external/github.com/libsdl-org/SDL/+/162f8e6ee75ba1921060dbd9cf500dec29082e21/docs/README-gdk.md)).
- The high-level `sdl3` Rust crate still says "migration is in progress" ([sdl3-rs](https://github.com/revmischa/sdl3-rs)), so wrap `sdl3-sys` directly.
- winit plus gilrs is a fine pure-Rust start for the editor binary, which uses eframe anyway. Bevy and egui still pin winit 0.30 ([bevy_winit deps](https://crates.io/crates/bevy_winit/0.20.0/dependencies)), and 0.31's betas rework the window and event-loop API ([winit 0.31 beta](https://github.com/rust-windowing/winit/releases/tag/v0.31.0-beta.1)).

**Custom game UI over any framework.**

- Gacha UIs need character showcases, banner-pull animations and 3D model insets tied into post-processing. No Rust framework offers that today.
- Vello's own README calls its CPU renderer "the most mature choice" and its original compute renderer experimental ([Vello](https://github.com/linebender/vello)).
- Slint's royalty-free license excludes embedded systems without saying whether consoles count ([Slint license](https://unpkg.com/slint-ui@1.12.0/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md)).

**Vendor the dependencies with a bus factor of one.** Several dependencies that matter strategically are one person's project:

- `ozz-animation-rs` (47 GitHub stars, MPL-2.0 file-level copyleft);
- `oxijolt` (15 stars, weeks old);
- `fmod-oxide` (beta, MPL-2.0, tested "only on a Linux system");
- `rerecast`;
- `landmass`;
- `dear-imgui-rs`;
- `noesis_runtime`.

Fork or vendor each one, with an owner inside your team from day one. The ecosystem also has dead ends to keep out of the dependency tree: `kajiya` and `rend3` (archived renderers), `naga_oil`, `generational-arena`, `oxidized_navigation`, `laminar`, `rrise`, `oddio`, `bincode`, `optick`, `rustybuzz`, `bevy_cobweb_ui` and `distill`.

### macOS is a good home base with three blind spots

On an Apple Silicon Mac, `aarch64-apple-darwin` is a Tier 1 Rust host ([rustc platform support](https://doc.rust-lang.org/rustc/platform-support.html)), wgpu talks to Metal directly, and building for iOS requires macOS anyway ([cargo-mobile2](https://github.com/tauri-apps/cargo-mobile2)).

The dev-loop settings that matter:

- Keep Apple's default linker, ld-prime, which is faster than lld on macOS ([Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/); [Chromium](https://chromium.googlesource.com/chromium/src.git/+/main/docs/apple_platform_linkers.md)).
- Build the engine as a dynamically linked library in dev builds, which Bevy calls "the most impactful compilation time decrease".
- Use `opt-level = 1` for your code and `3` for dependencies, with line-tables-only debug info.
- Keep gameplay code in the top-level binary crate, because subsecond patches only that crate.

The fast-build alternatives don't help much on a Mac yet. Cranelift is nightly-only and averages 10 to 15% faster compiles ([Rust goals](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)), and Bevy warns it can crash on macOS. The Wild linker is Linux-only ([Wild](https://github.com/wild-linker/wild)).

The three blind spots:

1. NVIDIA DLSS needs Vulkan on Windows or Linux ([Bevy 0.17](https://bevy.org/news/bevy-0-17/)).
2. Superluminal lists no macOS support, so use Instruments, samply and Xcode's Metal capture instead ([Superluminal](https://superluminal.eu/applications/)).
3. You will want a Windows machine for DX12, PIX and RenderDoc validation well before any console work.

## Spend the first ten months proving combat feel

The windows below are rough estimates for a small, senior team, not sourced figures. A solo developer should expect them to stretch.

| Phase | Rough window | Build | Exit test |
|---|---|---|---|
| 0. Foundations | Months 0–3 | Workspace with CI layering rules; desktop `platform`; allocators, handles and profiler markers; glam math; task graph; `time` with clock domains and the fixed-step driver; reflection and serialization | A deterministic empty frame loop under Tracy with zero steady-state allocations |
| 1. Data path | Months 2–6 | GUID sidecars; importer, cooker and content-hash cache; rkyv runtime formats; async loader with hot reload; bevy_ecs world feeding extract; minimal frame-graph renderer with the anime character pass | Edit a ramp texture or move timeline and see it in the running game without a restart |
| 2. Combat slice | Months 4–10 | ozz runtime with root motion; character controller; intent buffer; move graph with frame data; hit queries; GAS-style effects; hitstop and perfect dodge; camera director; three-character party with swap; one enemy; headless combat tests; timeline editor with live link | One character against one enemy feels right, and recorded inputs replay as regression tests |
| 3. World and tools | Months 8–18 | Streaming cells and HLOD; f64 to camera-relative conversion; document-model editor with undo and play-in-editor; Luau called on events | A zone streams with no activation hitch, and a boss pattern is authored with no Rust changes |
| 4. Shipping | Month 12 onward | PSO inventory and pre-compilation; chunk-manifest packaging; mobile backend and thermal tiers; consoles only under NDA | Patches download only changed chunks, and a mid-tier phone holds its target frame rate at throttled clocks |

Phase 2 is the decision point. Tiny Glade, LogLog and Architect of Ruin each confirm a different part of the same lesson. Rust is excellent for the systems an engine runs on. It is weakest at the fast trial-and-error tuning a combat designer lives in. If tuning hitstop, cancel windows and camera through data hot reload and subsecond still feels slow after the slice, take the off-ramp: move gameplay and tools to Godot with gdext, and keep your Rust systems. That costs months, not years.

## Conclusion

Rust's borrow checker was a burden for gameplay prototyping, where LogLog and Architect of Ruin felt it most. In engine code it enforces exactly what this genre needs: one extract boundary, side effects routed through queues, and every timer bound to an owner's clock. Put the borrow checker where it pays off, in the engine. Put Luau and data where it hurts, in the code a designer edits daily. The engine and the game code can then each be written in the mode that suits them, inside one language.

The competitive opening is not out-building Kuro or HoYoverse. It is avoiding what their engines had to fit on after the fact. WuWa's community-documented problems, a saturated single game thread, script garbage-collection spikes, shader-compile stutter and a 130 to 150 GB install, are all decisions a new engine makes on day one, essentially for free. Make them right, scope the game toward a few polished zones and a small cast, and the remaining question is not whether Rust can do it. It is whether one person can author enough anime-quality characters to fill it. The engine plan should spend as much effort on content tools as on runtime code.
