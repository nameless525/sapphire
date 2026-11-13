# Assets, memory, world data and the benchmark: a survey for M5

A design survey's read of the code on 2026-11-13, at 27f29b5f (release 0.1.0), for [the M5 plan](../../docs/m5-plan.md). Nothing was built or run: every finding is from reading the source, the spec and the handoffs, and "unverified" marks what the source could not confirm. Line numbers are that commit's. Sizes are the surveyor's, in agent-days; the plan splits and re-sizes its packages. A package that leans on a finding checks it against the code first.

## 1. As built

**Packs and loading**
- Format 4: 64 KiB blocks, each with its own blake3 in the table of contents (`crates/sapphire_assets/src/pack/mod.rs:1-84`). `PackReader::read_block` already decodes one block alone (`:728`).
- Codecs: zstd 19 for meshes, textures, probe grids and fonts; lz4 for CPU kinds; raw for sound only (`tools/sapphire_pipeline/src/pack.rs:162-185`).
- A mount reads the header and table only. Each asset is then one positioned read of its block range, decoded on a job, validated, and queued for upload at 16 MiB a frame (4 on mobile) (`crates/sapphire_assets/src/server/mod.rs:468-501`, `1236-1262`).
- One `sapphire-io` thread with four priorities. Android reads SDL streams inside the APK, where packs are stored uncompressed (`crates/sapphire_platform/src/io.rs:137-152`; `platform/android/app/build.gradle.kts:61`).
- `read()` caps neither reads in flight nor decoded bytes waiting for the GPU. A queued load's priority can't be raised.

**Giving back**
- `unmount` unloads what nothing holds; `take_unloaded` leads to `Renderer::free_mesh` and `free_material` once frames retire (`crates/sapphire_render/src/renderer.rs:1270-1284`).
- Mesh-pool buffers never shrink (`mesh_pool.rs:33`).
- Per-kind budgets are a cache limit: LRU over unused assets only, and in-use assets never evict (`server/mod.rs:558-565`).
- Textures get 2,560 MiB on desktop and 600 MiB on mobile, chosen by form factor, not tier (`crates/sapphire_stage/src/content.rs:917`).

**Textures**
- KTX2, full chain, smallest level first. A 2K colour map cooks in 1.2 s (BC7) or 0.5 s (ASTC), serially (`tools/sapphire_pipeline/src/texture.rs:1-70`).
- Upload creates one texture with every level, usage `TEXTURE_BINDING | COPY_DST` (`crates/sapphire_render/src/texture.rs:44-87`).
- Each material owns a group 3 holding its textures. Hot reload already swaps a texture and re-sends the materials that bind it (`stage/src/content.rs:373-415`, `AssetKind::binds_dependencies`).

**World**
- Streamer radii are 256/320/384 m on desktop and 128/176/224 m on mobile, at 64 or 32 entities a frame (`crates/sapphire_world/src/streaming.rs:57-88`).
- Small placements merge into one mesh per cell at cook. Repeated buildings are instances: one entity, one draw per submesh, LOD0 only, CPU sphere culling.
- Probe grids are sparse bricks, one per zone (the ward's is 7.6 MB), loaded only by `Content::open_zone`.
- Measured cook: 3,584 placements of 24 pieces in 256 cells gave 41 MB of cell meshes in 1.6 s (spec line 2689).

**Compiled-in data**
- `crates/sapphire_gameplay/build.rs` embeds scenarios, every `.enemy.ron` with its moves and script, timelines and inputs.
- A scenario's own zone is built from its compiled-in document (`replay/document.rs:62-66`). Trip targets come from the committed copies in `crates/sapphire_gameplay/baked/zones/`.
- `documents/` beside the packs is a third channel, each file marked `simulation` or not for the patch rule (`crates/sapphire_assets/src/documents.rs:12-18`).
- `Progress` v1 holds one zone (`game/game_flow/src/progress.rs:27-39`).

**Crowd**
- Brains compare all pairs over a flat list (`crowd.rs:30-33`).
- Fighters animate serially on the frame thread; the comment at `game/game_combat/src/fight.rs:1489-1494` says a crowd wants `evaluate_all` and a retained list.
- Two fighters cost 0.01-0.03 ms of animation and under 0.05 ms of simulation (§3.5). One hero's secondary motion is about 0.5 ms.

**Measured (§3.7)**
- Cook: 38.7 MB desktop, 27.9 MB mobile.
- Process peak: 1,023 MiB on Desktop-High, 558 MiB on Mobile-Low.
- Git pack 535 MiB, goldens 191 MB, CI caches 9-10 GB of 10.

## 2. Already designed for later
- Per-cook size caps by dropping top levels, and parallel encoding (`texture.rs`, "Later milestones").
- Mip streaming and splitting an upload across frames (§5.10 line 961; `AssetBudgets::upload_per_frame` doc).
- Raw assets are contiguous and aligned "so mapping needs no format change" (`pack/mod.rs:70-72`).
- LOD selection with culling, and the GPU-driven path (`mesh_pool.rs:21`).
- Navmesh tiles (§5.27 line 3649); brick streaming (line 1316).
- A run-wide identity taken at run start (m4-handoff, "`tables_hash` changes at a zone entry").

## 3. Hard constraints
- **Determinism:** the simulation streams nothing. Zone digests, kinds, scripts and patrols are in `tables_hash`; a moved checksum costs two CI rounds.
- **Patches:** presentation only (`Manifest::unpatchable_change`). An install rewrites every changed pack whole beside the old one, so it needs disk for both and time proportional to pack size. Downloads block the boot, with no background fetch. Cross-architecture cooks differ in every texture.
- **Content:** no downloads; third-party bytes only under `content/third_party` with provenance. Generators must use `sapphire_math::det`.
- **CI:** it cooks all of `content/` with `--locked` in many jobs, with the cache at its ceiling and jobs of 45-90 minutes. Git has no LFS (`.gitattributes`).
- **`ron`** parses quadratically (40 s for 2.5 MB).

## 4. Proposed work packages (in order)

**Mip design (cheapest sound one).** The cook splits each texture into a tail asset (levels up to 128 px, a valid KTX2 as today, the only thing materials depend on) and one `TextureLevel` asset per higher level, with IDs derived by `sub_asset`. The existing load, hash, LRU, unmount and block-patch paths then apply unchanged, with no pack format change. Raising or lowering a level re-creates the GPU texture, copies kept levels with `copy_texture_to_texture`, writes the new one, and rebinds through `update_material`.

| # | Package | Goal | Touches | Needs | Headless proof | Size | Risk |
|---|---|---|---|---|---|---|---|
| 1 | Bench content root | Generate and cook the benchmark outside `content/` | pipeline `generate`; `xtask/src/content.rs` (`cook_copy` path) | none | Generator byte-identical twice; `ci` preset cooks in CI; `budget` cook time and RSS recorded | M (5d) | `ron` parse; cook RAM |
| 2 | Per-cook texture size | Close §3.4's 2K/1K gap | `tools/sapphire_pipeline/src/texture.rs`, sidecars | none | Content test holds every character map to its cap | S (2d) | Mobile goldens move |
| 3 | Load admission and tier budgets | Cap decoded-waiting bytes; raise priority; budgets by tier | `server/mod.rs`, `stage/content.rs:917` | none | Server test: 1 GiB synthetic load never holds more than the cap | S-M (3d) | Dependency-order stalls |
| 4 | Mip streaming | Hold the texture pool as a hard budget | texture cook, `kind.rs`, `render/texture.rs`, a stage `TextureStreamer` | 1-3 | GPU readback: streamed-to-full equals whole upload. District walk holds `texture_bytes` at or under the pool on every tier. Goldens unchanged at full residency | L (10d) | Bind-group churn; transient double memory; visible pops |
| 5 | District-scale zones | Cook and stream thousands of placements | pipeline `zone`, `streaming.rs`, mesh pool | 1 | Cook time and RSS test at bench size; walk test on budget; draws against 2,500 and 700 | M (6d) | Depends on the renderer's LOD and instancing package |
| 6 | Trips carry probe grids; hub keeps packs | Lit zones with exits | `fight/trip.rs`, `Content`, zone cook's `exit_target` | none | `zone_trip` with grids; repeated-trip memory test | M (4d) | Tier-change rebuild holds each grid's bytes |
| 7 | Simulation data from packs | A game's scenarios, kinds, scripts and zones without a rebuild | new scenario kind, `.senemy` v+1 (name, script), `game_flow`, `manifest` | none | Pack-loaded and compiled-in give one `tables_hash` for every shipped scenario; no replay moves | L (9d) | `[simulation]` layering; Luau bytecode across architectures unverified |
| 8 | Progress v2 | Saves across several zones; run-wide identity | `game_flow/src/progress.rs`, `kept.rs`, save fixtures (add, never regenerate) | 7 | v1 fixture still loads; save in zone B continues in B | M (4d) | Restart semantics |
| 9 | Ambient crowd | 30-60 presentation-only characters | `ZonePresentation` (posts, walks), stage system on `evaluate_all` | 1, 5 | Checksums identical with crowd on and off; Animation row within 1.5/3.0 ms; draw count | L (8d) | 60 x slots x passes of draws |
| 10 | Budget bench lane | One command, every row and ceiling | sandbox `fight_bench`, spec §3.8 | all | `--bench` over district, crowd and hero per tier; nightly on the `ci` preset | M (4d) | Mac-only numbers |

## 5. The benchmark
- **Where it lives:** small documents under a new `bench/` root (zone, prefabs, procedural-texture documents, figure specs), a few hundred KB in all. Nothing cooked is committed; cooks go to `target/content/bench-<cook>/`.
- **District:** about 256 m square in 32 m cells, built from the project's own parametric shapes (`assembly/shapes.rs`: halls, roofs, stalls, carts, lanterns). Roughly 40 building variants as instances, 6-10k merged props, lights up to 64 and 8. Kenney's kit and the pavilion (both CC0) are optional extra pieces.
- **Textures:** a new procedural image source rasterized at cook through `det`. About 180 2K sets fill 2.5 GB; a 2x preset proves the budget holds.
- **Hero:** the look-dev VRM measured against the row (it fails on 11 materials), plus a generated figure at exactly 80k/30k triangles, 200/140 joints, 6/4 materials.
- **Crowd:** 6-8 Lancer-class figures (about 3k triangles, 24 joints, one material) on KayKit's clips.
- **Proves:** frame rows, memory ceilings, streaming, load times, no growth.
- **Can't prove:** the anime aesthetic at density, mip pops as judged by eye, a phone's flash or jetsam, discrete-GPU VRAM.

## 6. Decisions and spikes
- **Memory mapping: recommend out of M5.** Only raw-stored assets benefit and textures are zstd, so it saves one copy.
  - Desktop and iOS are plain files. iOS address-space limits are unverified.
  - Android needs the asset's descriptor and offset, which the SDL stream hides (unverified).
  - Patched packs are immutable hash-named files, so they map safely.
  - A dev cook rewriting a mapped pack breaks on Windows, and an IO error becomes SIGBUS.
- **Spike (1d):** `copy_texture_to_texture` for BC7 and ASTC on Metal, lavapipe and WARP. wgpu-core validates block-aligned copies; the backends are unverified.
- **Do ambient characters block the player?** Recommend presentation-only walkers, with static posts in the zone's colliders.
- **Simulation data in packs stays unpatchable.** Recommend one always-mounted simulation pack pinned by app build.
- **Mobile-Low's pool:** 558 MiB base plus 600 plus 192 nears 1.6 GB. Propose 400 MiB.
- **Spike (2d):** patch rehearsal on a 1 GiB pack, measuring install time, disk and boot blocking.

## 7. Corrections
- **Packs don't load whole.** Assets are range reads; each asset, with all a texture's mips, loads whole.
- **`memmap2` is not a dependency anywhere,** though spec lines 654, 930 and 4414 name it or say cooked data is memory-mapped.
- **§3.4's pool already exists** as the texture residency budget, but as a soft cache limit and per form factor.
- **Thousands of placements cost cell-mesh bytes,** not entities; instances cost the draws.
- **"Memory comes back" has limits:** the mesh pool never shrinks, and released cells stay cached under budget (§3.6).
- **`PackMetadataBudget` is 12 MiB,** sized for sub-megabyte tables (`pack/metadata.rs:25`). A 2 GB pack's table is about 1.6 MB before its native copy.

### Critical files
- crates/sapphire_assets/src/server/mod.rs
- tools/sapphire_pipeline/src/texture.rs
- crates/sapphire_render/src/texture.rs
- crates/sapphire_stage/src/content.rs
- crates/sapphire_gameplay/build.rs
