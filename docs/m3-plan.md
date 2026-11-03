# M3 plan: world and tools

- **Status:** in progress (2026-11-03). The maintainer gave the go-ahead on 2026-11-03 and told the run to carry on without asking. They hadn't played the fight, so [M2's go/no-go](m2-go-no-go.md) got its answer without review 3 behind it: M3 stays in Rust, and whether the fight feels right is still unknown. §7 has what that leaves open. Every decision here was taken by the agents who wrote this plan, and each can be undone by saying so.
- **Milestone:** M3 (spec §10). It builds on M2, which is built with nothing judged by a person, and on the Sol review's fixes.
- **Inputs:** the spec (§3.2 budgets, §4 architecture, §5.17, §5.19, §5.21, §5.23–§5.25, §5.28 module specs, §7 layout, §9 gates, §10 milestones), [m3-handoff.md](m3-handoff.md), and five scouts' notes under [research_notes/World and tools for M3/](../research_notes/World%20and%20tools%20for%20M3/): `navigation.md`, `script.md`, `text.md`, `editor.md` and `zones.md`. Nothing in those notes was built or run. A package starting from one proves it first, and the spikes (§4) exist to do that.

## 1. What M3 is for

M3 answers one question: **can someone make a game with this engine, rather than this one fight?** M2's fight is built from data with one consumer each, and a new enemy or stage still needs Rust. M3 makes a place, a fight, a boss, a cutscene and a menu things a person writes in documents, and it builds the tools and the runtime they need.

The slice is a small game, `game_app`: a title screen, a walk through a zone with three encounters, a boss, a cutscene, a checkpoint to come back to, and a pause, settings and character screen, in English and Japanese, on a gamepad, a keyboard and touch.

### Exit tests

1. **A zone (from the spec):** one zone with three encounters, walked from its start to its boss. Replays of the walk reproduce their checksums on all three desktop OSes with no heap allocation inside the fixed step outside script hooks (gates 2 and 4, §3).
2. **A boss with no Rust changes (from the spec):** the boss is documents and a script under `content/`. A test holds it: the boss's name appears in no `.rs` file, every file it needs is reached from the zone's document, and the commit that adds it to the zone touches only `content/`, `tests/replays/` and `tests/golden/`.
3. **One cutscene (from the spec):** it blends in and out with the gameplay camera, shows subtitles in the chosen language, and can be skipped. A skip and a watch both leave the fight that follows on the same checksum.
4. **A full HUD and menus in two languages (from the spec):** HUD, title, pause, settings, character screen and dialogue panel, in English and Japanese, with goldens at 4:3, 16:9 and 21:9. A scripted walk through every menu by gamepad focus, by keyboard and by synthetic touch. Pseudo-localization finds no string outside the tables.
5. **All on five platforms (from the spec):** Linux and Windows through CI on software rasterizers, macOS on Metal, and iOS Simulator and an Android emulator by the exit run. With no phone and no desktop GPU but this Mac's, M3 can only close as "done, pending devices", as M1 and M2 did.
6. **The editor (from the spec's row):** every edit is a change record, undo returns a document to the same bytes, and play-in-editor starts from a snapshot and leaves the documents alone. All of it is proved by tests with no window, and the window itself is owed to a person (§7).
7. **The inventory (from the spec's row):** a cooked pack lists its pipelines, a load compiles them, and a replay of the walk reads 0 first-use builds after load on Desktop-Low's and Mobile-Low's settings. M4's warm-up builds on this.
8. **Budgets and durability (new, so they can fail):** §3.2's script and streaming rows read within budget on a zone bench. A save survives a kill at every step of a write, and an old save migrates.

**Feel stays unknown.** None of these tests ask whether the fights are fun, and agents can't say. Three enemies in a room may expose what one enemy never did.

## 2. What "done well" means

Ranked. Each has a measure a test or a bench can read.

| # | Pillar | Measure |
|---|---|---|
| 1 | **Authorable.** A new enemy, encounter, stage, boss, cutscene or screen is documents. | `m4-handoff.md`'s "How to add" sections have no step that edits Rust. M3's handoff lists any that remain as bugs. |
| 2 | **Seamless.** Nothing inside a zone hitches, and a transition hides what it must load. | Streaming activation within §3.2's 0.5 ms and 1.0 ms. No visible pop in a walk's goldens. A zone change reads in the bench. |
| 3 | **Deterministic.** The simulation's gates extend to everything new. | Gates 2 and 4 cover a zone's walk, a boss's hooks and navigation. Negative tests prove the gates catch a script that tries. |
| 4 | **Readable in both languages.** Text stays sharp and fits at every aspect ratio. | Goldens at three aspects in two languages. Pseudo-localization's longer strings fit. |
| 5 | **Small enough for a phone.** New runtime dependencies pay for themselves. | Binary size and memory deltas reported per package, on the Android and iOS builds. §3's budgets on Mobile-Low's settings. |
| 6 | **Undoable.** An editor never loses work. | Undo and redo round trip to the same bytes. A killed editor recovers. |

## 3. Decisions this plan takes

Each decision updates the spec in the same PR as this plan.

| Topic | Decision | Why |
|---|---|---|
| **The go-ahead** | M3 starts in Rust, with no one having played M2. | The maintainer's instruction. The go/no-go's question is still open, and if they answer it differently later, [m3-handoff.md](m3-handoff.md)'s "What M3 builds on" is the list of Rust systems that stay. |
| **Where the game goes** | **`sapphire_stage`,** a new engine crate above L5: `Scene`, `Content` and the offscreen `Stage`, moved out of the sandbox. It's what turns a simulation and cooked content into a `FramePacket`, so the sandbox, the game and the editor's viewport share one. **`game/`:** `game_combat` (what a fight needs from the app: `combat.rs`, `arena.rs`, `enemy.rs`, `blade.rs`, `cues.rs`, `party_looks.rs`, `trails.rs`), `game_ui` (HUD, touch, menus, panels) and `game_app` (the game, on five platforms). The sandbox stays as the bench and the goldens' host. | M2's handoff names this as M3's first job. The editor's viewport needs it too. Spec §7 also lists `game_characters` and `game_ai`; they aren't made until there's something to put in them, since brains stay in `sapphire_gameplay` as documents. |
| **The simulation streams nothing** | A zone's colliders, triggers, spawners and navigation tiles load before its first step and stay. Only presentation streams: meshes, textures, sounds. Cells gate what's drawn and activated, never what the simulation can read. | Gates 2 and 4: a streamed collider would make a replay depend on load timing, and `parry3d`'s statics are fixed once built. **The cost:** all of a zone's collision is always in memory, which Z1 measures. A zone that outgrows it is split into zones. |
| **Zones as a format** | **`sapphire_zone`** (L2, plain cooked data, as `sapphire_moves` and `sapphire_colliders` are), so the cooker, the simulation and the world all read it. `.zone.ron` is the source and `.szone` the cooked form. Prefabs are referenced by GUID, with overrides resolved at cook. The cook splits static geometry into a mesh per cell. `sapphire_world` owns streaming and activation, and `sapphire_gameplay` owns triggers, spawners and encounters. **A replay pins a digest of everything it read from a zone,** as `ARENA_WORLD_DIGEST` and `tables_hash` do today, so stages stop being compiled in with `include_str!` and a replay still can't drift. | R3 and #74's lesson: a cooker or a replay must not link a renderer. Culling is per instance by bounding sphere, and an instance is only as fine as its mesh, so the town has to be cut up at cook. |
| **Giving memory back** | Two things the runtime can't do today: `AssetServer` gets an `unmount` with defined handle behaviour, since a mount is addressed by index in a vector, and `MeshPool` gets a free that waits for frames in flight, with a free list. Cells then stream as ordinary `InstanceDeltas`, so `FramePacket` doesn't change. `Content` stops holding handles for the whole run. | Neither half gives memory back today ("Meshes are not unloaded yet"), and a walk through a zone needs both. |
| **Navigation** | **`rerecast` 0.5 bakes tiles in the cooker. The runtime is our own,** about 1,200 lines in `[simulation]`: a plain format in `sapphire_navmesh` (L2), `sapphire_nav` (grid point location, A* on a preallocated pool, funnel, corridor, repath budget, separation). **`landmass` isn't used.** Arena enemies keep steering directly in a room. | The scout read `landmass` 0.9.3: it allocates on every update and search, iterates hash maps, calls `rand::random()` through `dodgy_2d`, uses `std`'s transcendentals, and is on glam 0.30. Any one of those fails a gate. P0 proves the bake. |
| **Several enemies** | **Factions,** data on a fighter, so enemies stop hitting each other. **A soft lock's choice enters the step,** from weights in the recording, never from the device. **An encounter director** with attack tokens, in `sapphire_gameplay`, from `.encounter.ron`. A moved checksum costs two CI rounds, so these land together. | G3, G5 and G6's deferrals. Tokens are the reference games' way of keeping a crowd readable. |
| **Luau's job** | `mlua` 0.12 with Luau and **no JIT.** One VM, event hooks only, never a per-tick script. **Scripts are pure decisions:** they read through queries and write typed commands into a fixed queue that one point in the step applies. Each command goes into the checksum. Anything that must last lives in a simulation-owned, fixed-size, numeric blackboard, which is hashed. VM state is never relied on between hooks, so a reload can't change a replay. | It's the only shape where a checksum means something. Hashing a VM's heap isn't possible, since Luau's table order follows addresses. |
| **Luau's determinism** | Remove `os.*`, `math.random`, `gcinfo`, `collectgarbage`, `newproxy`, `getfenv`, `setfenv`, `debug.*`, `print`, `require`, transcendental `math.*`, `math.noise`, `vector.angle`, weak tables, float conversions in `string.format`, and `tonumber` on non-integer text. Forbid `pairs` over non-primitive keys, `tostring` of reference types, `^` except 2, 3 and 0.5, and coroutines across ticks. Replace the math through `sapphire_math`'s pinned functions, with libm builtins disabled at compile (`set_disabled_builtins`). Cook bytecode once with a pinned compiler, keep the bytes in `tables_hash`, and scan them for `POW`. | The scout read Luau's source and found each of these ([script.md](../research_notes/World%20and%20tools%20for%20M3/script.md)). The spec's "`os`, `time` and `random` globals" isn't quite what Luau has. L0 attacks this on Linux and Windows CI with negative tests. |
| **Gate 4 with a VM** | Zero allocations on every step with no hook. Around a hook, a thread-local moves the VM's allocations to their own count and budget (`script_allocations`), and generated bindings flip back, so a Rust allocation in a binding still fails gate 4. GC stops inside a step and is stepped outside it. | `mlua` hardcodes `std::alloc`, with no custom allocator, so a true zero isn't available. This is the honest version, and it's a narrower gate than M2's. The spec says so. |
| **`panic = "abort"` in shipping** | **L0 decides.** `mlua` raises a binding's `Err` and an interrupt's error with `lua_error` from a Rust frame, which probably aborts under the shipping profile (inferred, untested). The default answer is bindings that never return `Err` and a budget that ends a hook by a yield, or else `panic = "unwind"`, whichever L0 can prove. | A script error mustn't kill a shipping game, and it can't be left to find out on a phone. |
| **Script bindings** | A declared table of commands and queries, and `xtask` generates Rust wrappers and a `.d.luau` from it, with a test that the checked-in output matches. **Not** generated from the `sapphire_reflect` registry as the spec said. | The surface has to be narrow and reviewed, and `bevy_reflect`'s function reflection isn't on. |
| **Sequencer** | `sapphire_sequencer` holds timelines, and they're presentation: camera through G5's takeover slot, character poses through the animation graph's slot, VFX, audio, post and subtitles. Time scale and gameplay events go through commands the replay records, and a skip is a recorded action. A cutscene holds the simulation at a step's edge, and each timeline names its clock. Characters in a timeline are drawn with no fight running, through the road the look-dev stage already has (`animate_graph`'s `graph_show`, `animate_clip`). | The fight after a cutscene has to reach the same checksum whether someone watched or skipped. `ScaledClock` is in the checksum, so a timeline's time scale in a fight must be recorded. |
| **Dialogue** | A graph document: each line has a localization key, a VO event and a speaker, with choices. Conditions are Luau expressions compiled to hooks, and flags live in the blackboard and the save. | Spec §5.23. |
| **Text** | `parley` 0.12 with no system fonts, `swash` to rasterize, `etagere` for a dynamic glyph atlas of pages, so the draw list gains a page and the packet carries uploads (R4). HUD digits keep their signed-distance atlas and bypass `parley`. MSDF for display text is last and can be dropped. `taffy` 0.15 for layout. | The scout's read, and U1 measures it: binary size, shape cost, atlas churn, Japanese line breaking. `UiAtlas` is one immutable texture today. |
| **The second language** | **Japanese.** | The reference games' language, and the hardest test of the text stack that's still useful: CJK atlases, line breaking, fallback, display faces. |
| **Fonts** | **M PLUS 1** for body text in both languages. **Oxanium** and **Dela Gothic One** for display. All under the SIL Open Font License, fetched by the orchestrator with provenance. Whole fonts stay byte-identical under `content/third_party/`, and a cook subsets them with `skera`. | The look wants squared, slightly mechanical faces, and M PLUS 1 covers Latin and Japanese in 4.2 MB. |
| **Localization** | Fluent `.ftl` sources. The cooker parses with `fluent-syntax` into `postcard` tables, and a small evaluator of our own runs them (about 300 lines), with `icu_plurals` and `icu_decimal`. Dates wait for a menu that shows one, since `icu_datetime`'s data is 18.75 MB of source. Pseudo-localization is our own. | `fluent`'s bundles can't be cooked (`FluentResource::try_new` takes a `String`), which spec §5.25 assumed they could. |
| **Saves** | As spec §5.25: settings as RON, progress as `postcard` with a versioned header and migrations, temp file, fsync, rename, two rotating slots with checksums. A save holds what a zone start reads (checkpoint, cleared encounters, flags, party), never a fight in progress. A replay can start from a named save. | A mid-fight save would have to carry the whole simulation and hold it across versions, and nothing here asks for that. |
| **The editor** | **`sapphire_document`:** typed objects with GUIDs, and one edit, a change record `(object, property path, old, new)`. Its backing is a `RonPatch`: find the path's byte range, splice, re-parse, check equal. Untouched bytes never move, so comments and layout survive, which is what T3's text model was protecting. **The shell:** `egui_tiles` for docking and `egui_plot` for curves. A small translate gizmo of our own. **Not** `egui_dock`, `transform-gizmo` or `egui-snarl`, which are on egui 0.36 with upgrade PRs still open. T3's timeline becomes a panel. The viewport is the real renderer through `sapphire_stage`, so the editor may link it; the cooker still may not. | The spec's model plus T3's reason for not using it. E0 proves `egui_kittest` can test a viewport pane headless. |
| **The inventory** | **The cooker writes `(family, flags, position format)` for each submesh in a pack,** from `sapphire_assets`' `MaterialFamily` and `MaterialFlags`, so it needn't link the renderer. The renderer derives each pipeline from that at load, off the first frame's path. **Post, effects, shadow and skinning pipelines** aren't mesh-derived: the app declares them, as `post_variants` and `prewarm_vfx` already do. **A recorded walk checks the list,** through a new first-use count across every pipeline family, and fails on a miss. Full warm-up with a blocking screen and a driver cache stays M4's. | A streamed mesh with a new variant compiles at upload today (`build_mesh_pipelines`), which is a hitch mid-walk. `first_use_builds` covers post alone, and `at_start` is a whole-process count from wgpu. A cooked list needs a recording to prove it, and a recording alone can't say what a new pack needs. |
| **Music and voice** | Music as stems crossfaded by encounter phase, with a CC0 loop if one fits. The voice route through cues, with its own language, with no recordings. | Spec §5.15's "M3 on". Nobody has heard any of the sound yet, so this stays small. |
| **Test content** | **The zone** from the town kit and KayKit and Kenney's CC0 sets. **A second enemy and the boss:** generated meshes committed as source files, with clips retargeted and key-posed as first drafts, in `tools/motion/`. **A cutscene** in key poses. Each new source goes in `content/third_party/` under m1-plan §3's rules. | M2's sources, plus the fonts. A generated boss still has to reach the cooker with no Rust edit, so generators write files and the cook finds them through documents. |
| **Pins** | wgpu stays on 30 (31 isn't out), egui 0.37, bevy 0.20.1, glam 0.33, and `ron` takes 0.12.3's patch. Reassessed 2026-11-03. | Spec §5.6: upgrades happen between milestones. |

**Not in M3:** XPBD cloth, ragdolls, dynamic props, decals, GPU particles, lip-sync, a real second character, PSO warm-up in full, upscalers, local-light shadows, mip streaming, hot-patched Rust, patching and accounts. Spec §10 places most of these later. Every device check [m3-handoff.md](m3-handoff.md) lists stays owed.

## 4. Work packages

Each package is one pull request, or a few, into `main`. Size: **S** is a day or two, **M** under a week, **L** one to two weeks, at a person's pace. Each finished package gets an "As built" note in its row and its spec §5 section.

The usual rules carry over: existing goldens stay byte-identical unless a row says otherwise, a feature builds its pipelines only when its work can run, and every system publishes a stat counter (R9) against its §3.2 budget. Each new simulation crate joins `[simulation]` with the shared `clippy.toml`. Each new document struct denies unknown fields. A rule from [m3-handoff.md](m3-handoff.md)'s "Rules the reviews made" binds whatever it touches.

### Track S: structure

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| S1 | **`sapphire_stage`:** move `Scene`, `Content`, the offscreen `Stage` and what they lean on out of the sandbox, with a public surface a game can call. Nothing else changes: every golden, replay and bench reads the same. | L | Plan | It conflicts with everything, so it lands before the parallel work starts. |
| S2 | **`game/`:** `game_combat`, `game_ui` and `game_app` with a title that opens the arena's fight. `xtask` learns a second app: an `--app` where `platforms/mod.rs`'s `SANDBOX` is hard-coded, for `run`, `bundle` and `link`. Android's Activity, namespace and manifest are templated, and CI's mobile builds gain the game. The touch controls move from `sapphire_ui` to `game_ui`, as the spec planned. | M | S1 | First run of `game_app` on a simulator and an emulator goes in the row. Goldens stay on the sandbox's flags. |
| S3 | **Numbers that are code, into documents:** `SpringTuning::ACTION`, `IkControls`, `TargetingWeights`, the HUD's layout, with a reload route for `combat_bindings.ron` and `default_effects.ron`. The content lists that are constants in code become discovered: `COOKED_MOVES`, `PLAYER_DOCUMENTS` and `CLIP_DOCUMENTS` from sidecars or a pack's table with names, `LOOKDEV_SOURCES` and `COMBAT_SOURCES` from each generator declaring its outputs. `SCENARIOS`, `MOUNTAINS`, `PLANTINGS`, `TOWN`, `WARRIOR` and the arena's `include_str!` documents go with B2 and W1, once documents exist to replace them. | M | S1 | Goldens and replays don't move. Pillar 1 fails without this. |

### Track W: zones

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| W1 | **The format:** `sapphire_zone`, `.zone.ron` to `.szone`: cells on a grid, a mesh per cell cut from placed kits, lights and probes, navigation tiles, spawners, triggers, splines. Prefabs by GUID with overrides resolved at cook. `pack.rs` chooses a pack by more than kind: per zone and per encounter, by §5.10's grouping rule. A replay's digest covers the zone. | L | S1 | Document checks name the field. The cook is repeatable across architectures for everything the simulation reads. The simulation takes the zone's colliders through `from_cooked` at start; a large zone's build time and the BVH's depth limit of 32 get read here. |
| W2 | **Streaming and activation:** `sapphire_world` activates cells by distance within the budget of N entities or M µs per frame. `AssetServer` gains `unmount`, with what a handle does after it defined and tested, and `mount` stops blocking the main thread on a pack's header. `Content` stops holding every handle for the run. The town stops being one mesh. | L | W1 | Scout's seams in [zones.md](../research_notes/World%20and%20tools%20for%20M3/zones.md). A synthetic zone comes first, so streaming's bugs show before content's. |
| W3 | **The renderer's side:** `MeshPool::free` with a free list, waiting on frames in flight; textures likewise; cells as ordinary `InstanceDeltas` with no change to the packet; `DrawList::build`'s cost when cells come and go; memory within §3.4 across a walk. | M | W2 | Goldens over the town don't move, or move once on purpose with both families rebootstrapped. LOD selection stays out. |
| W4 | **Zone transitions:** a stylized wipe in `game_ui` that hides a load, prefetching the next zone's packs from a hub, unloading the last. | M | W2, U4 | The wipe's look is a first draft. |
| W5 | **Encounter volumes:** entering one freezes streaming and pre-warms its enemies' moves, effects, sound banks and pipelines before the fight starts. | M | W2, B2, K1 | Reads in the zone bench as no first-use build and no stream during a fight. |

### Track B: fights with several enemies

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| B1 | **Factions and a choice of target:** factions on fighters; a soft lock's choice read in the step, from recorded weights; a defeated actor's capsule leaves; brains pick a foe rather than hold a fixed one. **Two fighters stop being a shape:** `ArenaActors`' fields and `fighters() -> [_; 2]`, `animate_fighters`' tables and `index == 1` blade, `Content`'s one enemy graph, the camera director's `FrameInput` with its one target, `HudValues` with its one enemy bar. Moves checksums, so B1 and B2 land together. | L | S1 | The simulation already holds several (`melee` spawns three). The breaks are above it, about a dozen shapes, listed in [zones.md](../research_notes/World%20and%20tools%20for%20M3/zones.md). |
| B2 | **Encounters in data:** `.encounter.ron`: waves, spawners, rules, and a director with attack tokens. Triggers in the simulation. Scenarios become documents, so a replay can name a zone and an encounter. | L | B1, W1 | |
| B3 | **A second enemy type and a boss's machinery:** enemy brains as documents (the warrior's first), boss phases as hook events, a boss bar in the HUD. Capsule pushing if a crowd needs it. | M | B2, L2 | Machinery only. The boss itself is C2, in documents. |
| B4 | **Walking a zone:** the controller over real ground, a teleport API, checkpoints, replays that walk the whole zone. The curb's throw fixed in the controller if a zone's walls need it. | M | B2, P2 | Exit test 1's replays. |

### Track P: navigation

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| P0 | **Spike:** a tile bake's repeatability across architectures (commit bytes, or re-cook), two stitched tiles, a replay with N chasers at zero allocations, worst tick's cost, and whether polygons alone suffice. Checks that `rerecast`'s `libm` feature leaves glam's alone. | S | Plan | Report changes §3 if it must. |
| P1 | **Format and bake:** `sapphire_navmesh` (L2), `rerecast` 0.5 in `sapphire_pipeline`, with tile stitching. | M | P0, W1 | Vendoring decided here, as Appendix A asks. |
| P2 | **Runtime:** `sapphire_nav`: point location, A* on a preallocated pool, funnel, corridor, repath budget. Golden-path tests and a replay gate. | L | P1 | Gates 2 and 4 from the start. |
| P3 | **Chasing and patrols:** brains use paths across a zone and steer directly in a room, with separation. | M | P2, B1 | |

### Track L: Luau

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| L0 | **Spike, first of everything here:** checks and links on five targets; clean compile time and binary delta; the shipping profile's behaviour on a binding's `Err`, an interrupt's kill, a script's error and a yield; a boss-phase hook under `CountingAllocator`; a determinism attack on Linux and Windows CI with negative tests and ASLR runs; a reload mid-replay; a hook's cost against §3.2's row. Android's `c++_static` decided here. | M | Plan | §3's Luau rows change with it. If Luau can't pass, this is where it's found, and data-only hooks are the fallback, recorded as a decision. |
| L1 | **`sapphire_script`:** the VM, the library allowlist, the interrupt budget, the memory limit, the GC step outside the simulation, the allocation accounting. | M | L0 | Joins `[simulation]`, and registers through `Simulation::add_combat_system`. |
| L2 | **The script API:** hook events and the command queue in `sapphire_gameplay`; the blackboard, in the checksum; the generated bindings and `.d.luau`, held by a test. A `.luau` cook kind with bytecode in `tables_hash` and the `POW` scan. | M | L1, B1 | Moves checksums; lands with B1 or B2 if it can. |
| L3 | **Hot reload and the first real scripts:** a name-to-function table swapped between steps, live link's push, a reload that keeps the blackboard; a test boss phase by script. | M | L2, B3 | |

### Track Q: sequencer and dialogue

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| Q1 | **`sapphire_sequencer`:** a timeline document and cook; tracks for camera, character animation and transform, effects and sound events, post parameters, subtitles, time scale and gameplay events; a clock per timeline; blends with the gameplay camera; skip. | L | S1 | Seams in [zones.md](../research_notes/World%20and%20tools%20for%20M3/zones.md): `set_takeover` still collides the eye and its rigs want a player, so a cutscene's camera needs an opt-out, and `tick` clears a `TodCutscene` past its end, so a scrub plays it again. |
| Q2 | **Dialogue:** graph document and evaluator, a panel in `game_ui`, conditions as hooks, flags. | M | Q1, L2, U5, V2 | |
| Q3 | **A cutscene and an ultimate's cinematic:** content and whatever they find. | M | Q1, C1 | The ultimate's cinematic can be cut. |

### Track U: UI and text

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| U1 | **Spike:** `parley` and `swash` on five targets: binary size, font registration, shape cost, atlas churn on a full Japanese screen, line breaking with and without `complex-scripts`, a `skera` round trip. | S | Plan | |
| U2 | **The glyph atlas:** `etagere` pages per size bucket; `UiDrawList` gains a page, and `FramePacket` carries uploads (R4). Goldens of existing HUD scenes don't move. | M | U1 | |
| U3 | **The font cook:** a font kind, subsets per pack, fallback chains per script, the fonts under `content/third_party/`. | M | U1 | Fonts fetched by the orchestrator first. |
| U4 | **The retained tree:** nodes, `taffy` layout, RON documents, hot reload on the desktop and through live link, data bound through reflection paths, nine-slice, gradients and masks. Output stays plain quads. | L | U2 | |
| U5 | **Motion and input:** tweens and keyframed timelines on the real clock, screen state machines, gamepad directional focus, keyboard, pointer and touch hit testing. Scripted focus walks. | M | U4 | |
| U6 | **A 3D inset:** a character showcase rendered to a texture a UI node shows. | M | U4 | Can be cut, with the character screen drawn flat. |
| U7 | **`game_ui`:** the HUD on documents, title, pause, settings, character screen, dialogue panel. Goldens at 4:3, 16:9 and 21:9 in both languages, on a plain background so they stay cheap. | L | U5, V2, S2 | Exit test 4. |
| U8 | **MSDF display text:** `fdsm` in the cooker only. | S | U3 | First to cut. |

### Track V: saves and localization

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| V1 | **`sapphire_save`:** settings, progress, migrations, atomic writes, rotating slots. A test kills a write at each step, by fault injection. | M | Plan | `user://` on all five, Simulator and emulator included. |
| V2 | **`sapphire_loc`:** `.ftl` to tables, the evaluator, `icu_plurals`, `icu_decimal`, pseudo-localization, a live language switch, a voice language apart from text's. | M | U1 | |

### Track E: editor

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| E0 | **Spike:** `egui_tiles` and `egui_kittest` with a real renderer's texture in a pane, headless. | S | Plan | |
| E1 | **`sapphire_document`:** typed objects, GUIDs, change records, `RonPatch`, undo and redo, dirty tracking, a reflection walker. `Reflect` derives on `sapphire_moves` and `sapphire_vfx` where they lack them. | L | E0 | Fuzz: any sequence of edits undoes to the same bytes. |
| E2 | **The shell:** docking, asset browser, inspector, cvar panel; T3's timeline as a panel on the new document model, keeping what its tests hold. | M | E1 | |
| E3 | **The viewport:** the real renderer through `sapphire_stage`, in a pane. | M | E2, S1 | |
| E4 | **Play-in-editor:** a fresh runtime from a snapshot of the documents. Game code never mutates them. | M | E3 | Exit test 6. |
| E5 | **Zone, sequencer and UI panels:** placing prefabs with a translate gizmo, scrubbing a timeline, previewing a screen at three aspects. | L | E3, W1, Q1, U4 | Can shrink: the sequencer panel goes first. |

### The rest

| ID | Package | Size | Depends on | Notes |
|---|---|---|---|---|
| K1 | **The pipeline inventory:** `(family, flags, position format)` per submesh in a pack's table; a public key the renderer derives its variants from (`SurfaceVariant` is private today, and skinned meshes draw from `draw_format()` rather than their cooked format); the app's declared families; a first-use count over all of them; a recorded walk fails on a miss. | M | W1 | Exit test 7. |
| A1 | **Music and voice:** stems by encounter phase, the voice route with its own language. | S | B2, V2 | Nothing has been heard. |
| G1 | **The game's flow:** title, new and continue, a zone, checkpoints, a boss, a cutscene, a return to title; quest flags in a save. | M | S2, B4, V1, U7 | Exit tests 1, 3 and 4 run through this. |
| C1 | **The zone:** a hub and three rooms from the kits, with colliders, spawners, a navigation bake and light rigs. | L | W2 | A first draft nobody has judged. |
| C2 | **A second enemy and the boss:** meshes as source files, retargeted and key-posed clips, moves and brain documents, a boss script, encounter documents. Adding the boss to the zone is one commit in `content/`, held by a test. | L | B3, L3, C1 | Exit test 2. |
| C3 | **Strings:** every string of the game in English and Japanese. The Japanese is machine-quality and marked so. | S | U7 | The maintainer can't review it through an agent, so it's owed. |
| Z1 | **The zone bench:** `--bench` on the walk, §3.2's rows, memory across a walk, a transition's time. | M | C1, G1 | Spec §3.5's table grows. |
| Z2 | **The exit run:** `docs/m3-exit-run.md`, and a rehearsal on Simulator and emulator. | M | All above | |
| Z3 | **Reviews:** fresh agents read each area, as M2's five did, and fix what they find. | M | All above | |
| Z4 | **Handoff:** `docs/m4-handoff.md`, spec §10's M3 status, the "How to add" sections with no Rust in them. | S | Z2, Z3 | |

## 5. Order

The packages fall into five phases. Within a phase, packages that don't depend on each other run as parallel PRs, about five at a time on this machine.

1. **Prove it:** this plan lands first. Spikes L0, P0, U1 and E0 run alongside S1, since they touch nothing it moves. Their reports change §3 if they must, in a docs commit before the packages that lean on them.
2. **Structure:** S1, then S2 and S3. These land before anything else starts in the sandbox.
3. **Foundations, in parallel:** V1, V2, U2, U3, U4, L1, L2, P1, P2, W1, B1, K1, E1.
4. **The place:** W2, W3, W4, W5, B2, B3, B4, P3, L3, Q1, Q2, U5, U6, U7, A1, E2, E3, E4.
5. **The slice and the exit:** C1, C2, C3, Q3, G1, E5, U8, Z1, Z2, Z3, Z4.

**Critical path:** S1 → W1 → W2 → B2 → B3 → L3 → C2 → Z2.

**If time runs short, cut in this order:** U8; E5's sequencer panel; U6; Q3's ultimate cinematic; A1's music; P3's separation. **Never cut** S1, W1, W2, B2, L1, L2, Q1, U4, U7, V1, V2, E1, E4 or K1: the spec's row names them.

**Landing:** checksum-moving packages go in one line where they can, because each move costs two CI rounds with no x86-64 here. Goldens that move go in the package that moves them, with the other backends bootstrapped from CI.

## 6. Risks

| Risk | Mitigation |
|---|---|
| **Feel is still unknown,** and this builds a game on an unjudged fight. | [m2-exit-run.md](m2-exit-run.md)'s reviews are still there to run. Everything authorable makes a bad answer cheaper to fix: a move, a brain or an encounter is a document. Should the answer be the off-ramp, what M3 adds to the Rust systems stays useful, and `game/` and the editor are what go. |
| **Luau fails a gate.** Shipping's `panic = "abort"`, or an address-dependent behaviour nothing scanned for. | L0 runs first, with negative tests. The fallback is hooks as data, a smaller language of conditions and commands in RON. Dialogue conditions and boss phases need little more. |
| **No phone, still.** Parley, ICU, Luau and zones all land on phones untested. | Size is measured on each package, budgets on Mobile-Low's settings, and the exit run rehearses on a Simulator and emulator. Nothing here makes [m3-handoff.md](m3-handoff.md)'s device list shorter. |
| **Our own navigation runtime.** 1,200 lines of pathfinding is real code to own. | It's small because it's polygons only, with no dynamic obstacles or crowds. P0 checks that's enough. The alternative fails gates. |
| **The sandbox's move conflicts with everything.** | S1 lands first, alone, with no other package open against the files. |
| **Nothing gives memory back today,** so streaming is new at every level: no pack unmount, no mesh free, and handles held for the run. | W2 and W3 stay apart from zone content, with a synthetic zone first. The zone bench reads memory over time. |
| **Motion without an animator,** now for a boss too. | C2's clips are first drafts, and a person is owed a look. Nothing in the exit tests pretends otherwise. |
| **Machine translation** in a language nobody here reviews. | C3 marks it. Layout and glyph coverage are what the test can hold, and meaning is owed to a person. |
| **CI time:** more goldens, a C++ dependency, more replays. | UI goldens on a plain background. Measure CI after L1 and U7. **GitHub's macOS runners** went out for 11 hours during M2, and the local stand-in in the merge recipe covers that. |
| **A large checksum moves often** while B1, B2, L2 and B4 land. | Land them in one line, and fill x86-64's numbers once. |
| **Scope.** Fifty packages, on the spec's 10-month window. | §5 names the cuts. M2's speed says nothing about a person's pace in this code. |

## 7. Needed from you

Nothing here blocks the work, and it will carry on without an answer. All of it is owed from you whenever you're back:

1. **Review 3 from [m2-exit-run.md](m2-exit-run.md),** thirty minutes with a gamepad. It's still the most useful thing anyone could do for this project.
2. **A phone,** for everything in [m3-handoff.md](m3-handoff.md)'s device list.
3. **The decisions above.** Each one stands until you say otherwise. The ones most worth a look: Japanese as the second language, the fonts, our own navigation runtime, and starting M3 before anyone has played M2.
4. **A look at the editor's window,** and the game's first run on a desktop, neither of which an agent may open on your Mac.
5. **A reader of Japanese,** for C3's strings.
