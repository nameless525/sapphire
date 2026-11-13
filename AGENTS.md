# AGENTS.md

The entry point for anyone, agent or person, who lands in this repository. It's a map: each section is short and points to the doc that holds the detail. [docs/README.md](docs/README.md) indexes every doc.

## What Sapphire is

A Rust engine for 3D **anime-aesthetic** action games, the look of Wuthering Waves and Zenless Zone Zero: zone-based levels, a three-character party, character action combat. It targets macOS, Windows, Linux, iOS and Android, all five first-class. It's about 550k lines of Rust (a third of them tests) in 61 crates, on Rust 1.99 (edition 2024, [rust-toolchain.toml](rust-toolchain.toml)), with SDL3 for the platform, wgpu for the GPU and `bevy_ecs` as the world database.

- **Where it stands:** version **0.1.0**, the first release (tag `v0.1.0`, [CHANGELOG.md](CHANGELOG.md)): milestones M0 to M4 are built: bring-up, the data path and the look, a combat slice, world and tools, ship readiness. The game is one slice, the Lantern Ward, from the title to its boss, packaged for three stores. [docs/engine-spec.md §10](docs/engine-spec.md#10-milestones) has the milestones. **M5, fidelity at scale, is planned and under way** ([docs/m5-plan.md](docs/m5-plan.md)): a row's "As built" note says what has landed. [docs/m5-handoff.md](docs/m5-handoff.md) has what it inherits.
- **Words:** say "anime-aesthetic" and "anime character shading", never "toon", in code, docs, comments and commit text. Proper names such as MToon are fine.

## Read first

| # | Doc | Why |
|---|---|---|
| 1 | This file | Orientation, the gates, the rules CI enforces |
| 2 | [docs/architecture.md](docs/architecture.md) | How the frame, the content pipeline, determinism and the tests fit together |
| 3 | [docs/conventions.md](docs/conventions.md) | How code reads: errors, unsafe, logging, GPU structs, tests |
| 4 | [docs/development-iteration.md](docs/development-iteration.md) | The smallest useful check for each kind of edit; replays; build profiles |
| 5 | [docs/getting-started.md](docs/getting-started.md) | Requirements, running on each platform, the sandbox, hot reload and live link |
| 6 | [docs/m5-handoff.md](docs/m5-handoff.md) | Where things stand, "How to add" recipes, rules the reviews made, known risks |

[docs/engine-spec.md](docs/engine-spec.md) is the design reference (4,400 lines). Don't read it top to bottom: open the §5 section of the crate you're changing (§5.0 is the module index), and §4 for the architecture.

## Repository map

| Path | What's there |
|---|---|
| `crates/` | Engine crates (runtime, plus the dev-only ones), one directory each |
| `game/` | The game: `game_app` (the binary), `game_flow`, `game_combat`, `game_ui` |
| `apps/sapphire_sandbox/` | The engine test bed: every golden image, bench and offscreen capture runs through it |
| `tools/` | Never-shipped crates (pipeline, editor, replay runner, testkit, symbols, soundgen, `sapphire_document`) and plain scripts: `blender/`, `motion/` (Python posing), `figures/`, `sounds/` (placeholder sound specs) |
| `xtask/` | `cargo xtask`: layer checks, cooking, bundling, goldens, replays, packaging. `xtask/layers.toml` is the crate layering |
| `content/` | Source assets and documents (RON), each with a `.meta` sidecar; `third_party/` holds downloaded content with its provenance |
| `bench/` | The benchmark's documents only (a district, prefabs, procedural texture specs, figure specs): `cargo xtask bench-content` generates and cooks them under `target/bench/`, never committed or shipped ([bench/README.md](bench/README.md)) |
| `tests/` | `golden/` (scenes.ron and reference PNGs per backend), `replays/` (`.sreplay` files and per-architecture checksums), `saves/` (save fixtures) |
| `platform/` | Per-platform packaging: `android/` (Gradle), `ios/`, `macos/`, `windows/`, the icon, `identity.toml`, `patch.toml`, `steam.toml` |
| `docs/` | Spec, plans, runbooks, handoffs, ADRs, store checklists ([index](docs/README.md)) |
| `research_notes/`, `reports/` | Background reading behind the packages (web research, marked unverified where it is) |
| `.github/` | CI workflows, composite actions, CI scripts (the privacy guard among them) |
| `.githooks/` | Privacy-guard hooks; `python3 .github/scripts/install_privacy_hooks.py` turns them on in a clone |

## Architecture in a minute

[docs/architecture.md](docs/architecture.md) has the diagrams and links to each claim.

- **The frame** ([spec §4.2](docs/engine-spec.md#42-frame-lifecycle)): SDL's main callbacks drive the app (`sapphire_platform::App`). Each frame pumps input, then runs 0 to 4 fixed 60 Hz steps (`sapphire_time::FixedStepDriver`): the stage's scene systems and the game's show, whose fight steps the headless `sapphire_gameplay::Simulation`. Presentation (animation, camera director, effects, cues) then runs at the frame rate, interpolated between the last two steps. Extraction writes a plain-data `FramePacket`, the only thing the renderer sees. The renderer prepares it and records passes into a frame graph, which runs with one submit.
- **The data path** ([spec §5.10](docs/engine-spec.md#510-sapphire_assets-l2), [§5.27](docs/engine-spec.md#527-sapphire_pipeline-tools)): sources under `content/` with `.meta` sidecars (an asset ID and importer settings) → `cargo xtask content` runs `sapphire_pipeline`, which imports and cooks per platform (`desktop`, `mobile`, `editor`) through a content-hash cache → packs (`.spack`) and a release manifest in `target/content/<cook>/` → the runtime's `sapphire_assets` mounts packs and loads assets by ID, read in place. Nothing at runtime names a path.
- **Platform entry** ([ADR 0002](docs/adr/0002-platform-entry-and-packaging.md)): desktop and iOS call `sapphire_platform::run` from `main`; Android exports `SDL_main` from a cdylib through `export_android_main!`. SDL is built from source and linked statically. `cargo xtask bundle|run|package` makes each platform's app.

## Crate map

From [xtask/layers.toml](xtask/layers.toml), which `cargo xtask check-layers` enforces. A crate depends on lower layers only, plus the same-layer edges `[allow]` lists. Each crate's `//!` doc says what it owns and what it doesn't.

| Layer | Crate | Owns |
|---|---|---|
| L0 | `sapphire_core` | Logging, cvars, GUIDs, names, the counting allocator, target classification |
| L0 | `sapphire_math` | glam re-exports, transforms, bounds, springs, color, `det` (deterministic scalar math), seeded RNG |
| L1 | `sapphire_platform` | SDL3: app lifecycle, windows, surfaces, input events, file systems, thermal readings |
| L1 | `sapphire_jobs` | The one task pool (`bevy_tasks`), plus the pipeline warm-up pool |
| L1 | `sapphire_time` | Clock domains and the fixed-step driver |
| L1 | `sapphire_crash` | Panic hook, minidumps on desktop, signal records on Android |
| L2 | `sapphire_assets` | Asset IDs, cooked formats, packs, the asset server, the release manifest |
| L2 | `sapphire_ecs` | Pinned `bevy_ecs` and the engine's schedules |
| L2 | `sapphire_look` | The frame's look as plain data, and CPU models of how the renderer draws it |
| L2 | `sapphire_moves` | Moves' documents and cooked per-tick data |
| L2 | `sapphire_reflect` | Reflection, stable type names, schema versions |
| L2 | `sapphire_rhi` | wgpu device, capability probing, surfaces |
| L2 | `sapphire_serialize` | Data formats: RON documents, the rkyv cooked envelope, `AssetId` |
| L2 | `sapphire_colliders` | Static collision as plain data |
| L2 | `sapphire_zone` | Zones as plain data: cells, spawners, triggers, checkpoints, exits |
| L2 | `sapphire_navmesh` | A zone's walkable polygons as plain data |
| L2 | `sapphire_timeline` | Timelines (cutscenes, cinematics) as plain data |
| L2 | `sapphire_dialogue` | Dialogue graphs as plain data |
| L2 | `sapphire_bytecode` | Cooked Luau bytecode |
| L2 | `sapphire_strings` | Cooked localized string tables |
| L2 | `sapphire_loc` | Evaluates string tables: plurals and numbers through ICU4X, the live language switch |
| L3 | `sapphire_render` | The renderer: anime character shading, environment, shadows, post, the frame graph, quality tiers |
| L3 | `sapphire_tod` | Time of day's keys, timelines and how they blend into the look |
| L3 | `sapphire_body_pose` | CPU pose evaluation (sampling, blending, IK) shared by simulation and presentation |
| L3 | `sapphire_anim` | Presentation animation: jobs and secondary motion |
| L3 | `sapphire_physics` | The collision world on `parry3d`: kinematic bodies, move-and-collide, queries |
| L3 | `sapphire_nav` | Path search over a navmesh inside the fixed step |
| L3 | `sapphire_input` | Action mapping (contexts, triggers, modifiers) |
| L3 | `sapphire_audio` | Cue playback and clock-aware mixing on `kira` |
| L3 | `sapphire_vfx` | Effects documents and CPU simulation (sparks, trails, dust) |
| L3 | `sapphire_ui` | The retained UI tree, touch zones, the HUD's quads |
| L3 | `sapphire_text` | Fonts, shaping, the glyph atlas |
| L4 | `sapphire_world` | Time of day at runtime, light rigs, screen effects, zone streaming |
| L4 | `sapphire_gameplay` | The headless simulation: moves, controller, hits, brains, party, replays, checksums |
| L4 | `sapphire_script` | Luau hooks in the fixed step |
| L4 | `sapphire_camera` | The camera director (presentation, outside the checksum) |
| L4 | `sapphire_sequencer` | The timeline player's presentation half |
| L4 | `sapphire_save` | Settings and progress on disk: rotating slots, migrations, atomic writes |
| L4 | `sapphire_patch` | Patch download and install by hash |
| L5 | `sapphire_controls` | Combat's bindings: input actions to the simulation's `TickActions`, remapping |
| L5 | `sapphire_cues` | Gameplay cues to effects, sound, shake, screen effects, rumble |
| L5 | `sapphire_stage` | A GPU, cooked content and a scene, stepped and drawn; shared by apps, the game and the editor |
| L6 | `game_combat` | A fight's app side: arena, camera, enemies, cues, live tuning |
| L6 | `game_ui` | The game's HUD, menus, settings and dialogue panel |
| L6 | `game_flow` | The game's flow: boot, title, runs, beats, saves, updates |
| app | `game_app` | The game's binary (features `dev`, `playtest`) |
| app | `sapphire_sandbox` | The engine test bed (features `dev`, `tracy`) |
| dev-only | `sapphire_debug` | The egui development overlay |
| dev-only | `sapphire_hot_reload`, `sapphire_watch` | Desktop hot reload and the file watcher under it |
| dev-only | `sapphire_livelink` | Live link: the device half and the desktop tool |
| dev-only | `sapphire_document` (in `tools/`) | What the editor edits: typed documents with change records |
| dev-only | `sapphire_synthetic_zone` | An art-free zone for streaming's tests and benches |
| dev-only | `sapphire_playtest` | A playtest build's sessions on disk |
| tool | `sapphire_pipeline` | Importers, bakers, cookers, packer, content cache |
| tool | `sapphire_editor` | The combat timeline editor (desktop) |
| tool | `sapphire_replay` | The headless replay runner |
| tool | `sapphire_testkit` | Golden scenes and FLIP comparison, the flash analyser |
| tool | `sapphire_symbols` | Checks stripped binaries against symbols; Breakpad `.sym`; crash reading |
| tool | `sapphire_soundgen` | Placeholder music and voice from specs |
| tool | `xtask` | Build automation |

Dev-only crates ship only in `dev` builds, reached from an app through an optional dependency. Nothing depends on a tool.

## Build, run, test

[docs/getting-started.md](docs/getting-started.md) covers requirements and running on each platform. What an agent needs to check a change:

```sh
cargo xtask content                    # cook content/ into target/content/desktop/ (also writes missing .meta sidecars)
cargo test -p <crate> --lib --locked   # while editing one crate
cargo xtask replay                     # the combat replays, no GPU (under a second)
cargo xtask golden --scene <name>      # one golden image, rendered offscreen on this machine's GPU
cargo test -p sapphire_render --test gpu --locked -- --ignored --test-threads=1   # renderer GPU tests
```

The sandbox runs headless with `--capture <png>`, `--bench <frames>` or `--offscreen` (a dev build). Without one of them it opens a window.

**Before review, all of these pass** (CI's Lint and Test jobs):

```sh
cargo fmt --all --check
cargo xtask check-layers
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p sapphire_sandbox --features dev --all-targets --locked   # CI builds `dev`; the gates above don't
cargo test-fast --locked               # lib, bin and integration tests; CI runs `cargo test --workspace --locked`
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --document-private-items
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --document-private-items --features sapphire_sandbox/dev
```

Then whatever the change itself must prove. [docs/development-iteration.md](docs/development-iteration.md) lists the smallest check by area; in short:

| You changed | Run |
|---|---|
| Moves or the simulation | `cargo test -p sapphire_moves -p sapphire_gameplay --locked`, then `cargo xtask replay` |
| Scripts | `cargo test -p sapphire_script`; after editing `api.toml`, `cargo xtask script-api` |
| A shader or the look | the renderer's GPU tests, then `cargo xtask golden --scene <name>` for the scenes it moves |
| Content or a cooker | `cargo xtask content`, the pipeline's tests, then the replays and goldens the content reaches |
| A screen or string | `cargo test -p game_ui --test strings --test menus`, then its `game-*` goldens |
| The stage or game | `cargo xtask content`, then `cargo test -p game_app --test reaches_the_fight -- --ignored` |
| Saves | `cargo test -p sapphire_save --locked` |

## Rules CI or tests enforce

| Rule | Enforced by |
|---|---|
| Crates depend only on lower layers and listed same-layer edges; nothing depends on a tool; every crate and feature is placed (R1, R8) | `cargo xtask check-layers` reading `xtask/layers.toml` (Lint job) |
| The simulation's crates (`[simulation]`) never link SDL, wgpu, render, world, anim, input or platform, so replays run with no window or GPU | `check-layers`; each simulation crate carries the same `clippy.toml` |
| Determinism in the simulation: no OS math, no `HashMap`/`RandomState`, `sapphire_math::det` for transcendentals, actors in ID order | the simulation crates' `clippy.toml` bans; replays on three OSes |
| The OS clock is read only in `sapphire_time` and `sapphire_platform` (R2) | `clippy.toml` `disallowed-methods` on `Instant::now`, `SystemTime::now` |
| `#[cfg(target_os)]` only in `sapphire_platform`, `sapphire_crash` and `sapphire_rhi`'s backend selection (R7) | Review ([conventions.md](docs/conventions.md)) |
| The renderer sees only resolved plain values in `FramePacket` (R4) | `check-layers`: the renderer has no edge to gameplay, world or time of day |
| The pipeline never links render, world, physics or a mixer | `check-layers` `[cooker]`; the cooker's own `clippy.toml` bans `std` transcendentals |
| Tool-only crates (gltf, image, meshopt...) and dev-only ones (egui, notify) stay out of shipped trees | `check-layers` |
| No heap allocation inside the fixed step (R6, gate 4) | `cargo xtask replay` with a counting allocator |
| Replays reach their per-architecture checksums | CI's Replays job on Linux, Windows (x86-64, one file) and macOS (aarch64) |
| Golden images within FLIP gates on Metal, lavapipe and WARP | CI's Golden images jobs; `cargo xtask golden` |
| GPU tests pass on a CPU rasterizer | CI's GPU tests (lavapipe) per PR; `gpu-tests-warp.yml` on `main` |
| No boss is named in any `.rs` file, comments included; the commit that adds one to a zone touches only `content/`, `tests/replays/`, `tests/golden/` | `crates/sapphire_gameplay/tests/bosses_are_content.rs` (Linux reads the whole history) |
| Every source under `content/` has a committed `.meta` sidecar | CI cooks with `cargo --locked xtask --locked content` |
| A literal asset ID in code must be reachable in the shared pack | The cook's `--code` scan |
| `Cargo.lock` is current | Every CI cargo call is `--locked` |
| Licenses, bans, advisories, sources | `cargo-deny` with [deny.toml](deny.toml); never add advisory ignores |
| Formatting, lints, docs with no broken intra-doc links | `cargo fmt`, clippy `-D warnings`, rustdoc `-D warnings` (Lint job) |
| No private emails, home paths or secrets in tracked files, commits or tags | `.github/scripts/privacy_guard.py` (Privacy workflow, hooks); [docs/privacy.md](docs/privacy.md) |
| Third-party content keeps its provenance and bytes | [content/third_party/README.md](content/third_party/README.md), m1-plan §3, `.gitattributes` `-text`; fonts' sidecars must name a license or the cook fails |
| The generated script API matches `api.toml` | xtask's `whats_checked_in_is_what_the_table_makes` test |
| Every golden PNG is a row in `tests/golden/scenes.ron`, named by family rules | `sapphire_testkit`'s scenes tests |
| Every game string exists in English and Japanese, within the fonts' character set | `cargo test -p game_ui --test strings` |
| No pipeline builds on the frame's thread after boot; every setting's pipelines are declared | `game_app`'s `zero_hitches` and `declares_every_setting` (App tests) |
| A file packaging reads has a filter in `packages.yml` | xtask's `packages_workflow` tests |
| No "toon" in new identifiers | Review |

## Common tasks

| Task | Start at |
|---|---|
| Add a move, kind of enemy, boss, encounter, zone, dialogue, cutscene, screen, music phase | [m5-handoff.md "How to add"](docs/m5-handoff.md#how-to-add): each is documents under `content/`, with Rust steps marked |
| Add or re-baseline a golden | [m5-handoff.md "A golden"](docs/m5-handoff.md#a-golden); `tests/golden/scenes.ron`'s header |
| Add a replay, or move checksums | [development-iteration.md "Replays"](docs/development-iteration.md#replays) |
| Change a shader or the look | `crates/sapphire_render/shaders/`, the module's `//!` docs, [spec §5.12](docs/engine-spec.md#512-sapphire_render-l3); look changes for the look-dev stage go in `content/lookdev/` so other goldens stay put |
| Add a cvar | `sapphire_core::cvar` (its module docs); set one live with `cargo xtask link cvar <name> <value>` |
| Add a setting, a tier, a rung, a GPU row | [m5-handoff.md "A setting"](docs/m5-handoff.md#a-setting), ["A rung of the ladder, or a tier"](docs/m5-handoff.md#a-rung-of-the-ladder-or-a-tier), ["A GPU"](docs/m5-handoff.md#a-gpu) |
| Ship a patch | [m5-handoff.md "A patch"](docs/m5-handoff.md#a-patch) |
| Change a platform's packaging | `platform/`, `xtask/src/platforms/`, [spec §7.4](docs/engine-spec.md#74-xtask-commands); `packages.yml` runs on PRs touching those |
| A store's checklist | [docs/store/](docs/store/) (Apple, Google Play, Steam, privacy policy) |
| Add a crate | Place it in `xtask/layers.toml`; `license.workspace = true`, `[lints] workspace = true`, a `//!` doc; dependencies only in the root `[workspace.dependencies]` |
| Run a playtest | [docs/playtest.md](docs/playtest.md) |

## Gotchas

- **macOS SDK:** a plain `cargo` build of SDL may need `SDKROOT=$(xcrun --sdk macosx --show-sdk-path)`. xtask sets it itself.
- **Sidecars:** `cargo xtask content` writes a `.meta` for each new source. Commit it with the source, or CI's locked cook fails.
- **Goldens:** a Mac renders the `metal` family; lavapipe and WARP goldens come from the PR's failed CI run's artifacts (`<scene>.<family>.actual.png`). `--update` without `--scene` rewrites scenes you didn't change; `--scene` takes one exact name.
- **Replays:** you can record aarch64 checksums locally; x86-64's come from CI's `replay-checksums-ubuntu-24.04` artifact, so a change that moves a checksum takes two CI rounds. If Windows then disagrees with Linux, it's a determinism bug, not a value to update.
- **Stale cooks:** the content cache keys on inputs and stage versions, not on cooker code. A cooker change bumps its stage's version (or delete `target/content/cache`). A pack layout change bumps both `PACK_VERSION` and `PACKER`.
- **After a rebase** onto new timelines, moves or documents, run `cargo xtask content` before the app's replay tests.
- **`Cargo.lock`:** `cargo xtask` without `--locked` quietly repairs a stale lock; run `cargo metadata --locked` first, or use `cargo --locked xtask --locked` as CI does.
- **Target directories:** never share `CARGO_TARGET_DIR` between two checkouts; artifacts from one count as fresh in the other.
- **Floats across OSes:** generated content that goes through `std` transcendentals differs by ulps between OSes. What the simulation reads, and figures, combat content and assemblies, use `sapphire_math::det` and are compared byte for byte.
- **Tests with `harness = false`** (several `game_app` and stage tests) print no totals: judge them by exit code.
- **`cargo test-fast`** stops at the first failing package; add `--no-fail-fast` to see everything.
- **Windows:** don't set `RUSTFLAGS`; it replaces `.cargo/config.toml`'s 8 MiB main-thread stack.

## Glossary

| Term | Meaning |
|---|---|
| Tier | A quality tier: Desktop-High, Desktop-Low, Mobile-High, Mobile-Low (`crates/sapphire_render/tiers.ron`, spec §3.1) |
| Ladder, rung | A tier's ordered list of things to give up when hot or over budget; each rung sheds in place, with no pipeline build |
| Floor | A tier's `dynamic_floor`, the lowest render scale dynamic resolution may take |
| Cook | Turning `content/` sources into a platform's cooked files (`desktop`, `mobile`, `editor`) |
| Pack | A `.spack` file of cooked assets, mounted by the asset server; `shared` plus per-zone packs |
| Sidecar | A source's `.meta` file: its asset ID (a GUID) and importer settings |
| Release manifest | The signed list of packs and files a release ships (`release.smanifest`) |
| Golden | A reference PNG of a sandbox capture per backend family (`metal`, `vulkan-lavapipe`, `dx12-warp`), compared by FLIP |
| Replay | A recorded run (`.sreplay`): a scenario plus per-tick inputs, played headless |
| Checksum | The hash of simulated state a replay must reach, per architecture |
| Scenario | A replay's starting world (`content/scenarios/*.scenario.ron`) |
| Tick, step | One fixed 60 Hz simulation step; time is integer ticks and sub-ticks |
| `tables_hash` | The hash of every table a run depends on (moves, navmesh, timelines...); each replay stores it, and one made on other tables is refused |
| `det` | `sapphire_math::det`: scalar math that rounds the same on every platform |
| Stage | `sapphire_stage::Stage`: a GPU, cooked content and a scene, stepped and drawn |
| Show | A `SceneShow`: what a caller plays on a stage (a fight, the game's `GameShow`) |
| Stand | Setting a fight, zone or run up on a stage, spread over frames (`FightStand`) |
| Director | The camera director (`sapphire_camera`): rigs that frame the fight |
| Cue | An event the simulation queues for presentation; `sapphire_cues` maps it to effects, sound, shake and rumble |
| Hitstop | Ticks during which an attacker's and victim's local clocks stop |
| Ward | The Lantern Ward, the game's zone (`content/zones/lantern_ward/`) |
| Zone | A cooked level: cells, spawners, triggers, checkpoints, exits |
| Encounter | Waves of enemies a zone's trigger starts |
| Kind | A kind of enemy, an `.enemy.ron` with its moves and brains; a boss is a kind flagged `boss` |
| Brain | What presses an enemy's intents |
| Beat | A step in a run's flow (`game.flow.ron`) |
| Flow | The game's flow from boot to title to runs (`game_flow`) |
| `FramePacket` | The plain-data snapshot of a frame the renderer draws (spec §4.6) |
| Frame graph | The renderer's per-frame passes, culled and ordered, with pooled transient textures and one submit |
| Prepare, record | The renderer's two halves of a frame: apply the packet, then encode passes |
| Warm-up | Building pipelines on the warm-up pool, off the frame's thread |
| Look-dev | The look-development stage (`--look-dev`): `content/lookdev/`'s town and VRM character |
| Live link | A dev build's TCP link to `cargo xtask link`: push packs, shaders and cvars, capture frames |
| Cvar | A console variable: a typed static a dev tool can set at runtime |
| lavapipe, WARP | CPU rasterizers for Vulkan (Linux) and D3D12 (Windows) that CI renders goldens and GPU tests on |
| FLIP | The perceptual image metric goldens are compared by |
