# The M5 exit run

M5 ends when its eight exit tests are answered ([m5-plan.md](m5-plan.md) §1): the district holding the frame, memory under a hard pool, the look surviving scale, the picture's missing parts, characters at budget, a crowd, no hitches at scale, and recipes with no Rust. This file is the procedure for all eight, for someone coming back to it cold. For each test it says what is run, which line or report row is read, and the rule that passes or fails it on each tier, with the budget from [engine-spec.md](engine-spec.md) §3 beside it; where each step runs (Metal on this Mac, counted or timed; the iOS Simulator; the Android emulator; CI's lavapipe and WARP jobs, and the nightly scale lane); and what stays owed in §1's terms.

The run has one machine, an Apple M5, which is none of §3.1's reference devices. It has no phone, no Windows or Linux GPU of its own, no artist and no animator, and it may not open a window or download content. Every clause below that needs one of those is owed, and says so. A time read on this Mac compares commits and tiers; it is never a phone's budget met. **Nobody has played anything,** and no test here asks whether it's fun (§1, "Feel stays unknown").

It is run three times: a rehearsal before the last packages land ("Rehearsal at …"), the final run on the release's head ("Final run at …"), and the owed rows later, by a maintainer with devices ("What stays owed").

| Exit test | Who answers it | Needs | Where it stands |
|---|---|---|---|
| 1. The district holds the frame | This Mac: counted walks for draws and lights, timed walks for §3.2 and §3.3; CI's scale lane by counts | Every reference device; exclusive GPU pass times on a Vulkan or DX12 GPU | Not yet run |
| 2. Memory holds under a hard pool | This Mac by the engine's counters; the Simulator and the emulator (no tool yet) | A phone's memory limit and its kill, a discrete GPU's VRAM, a person judging a late level | Not yet run |
| 3. The look survives scale | Metal here, lavapipe and WARP in CI | A person's eye on motion | Not yet run |
| 4. The picture's missing parts | Metal here, lavapipe and WARP in CI, the budget rows timed here | An artist's eye, Zenless Zone Zero references | Not yet run |
| 5. Characters at budget | This Mac, timed and counted; the anim and pipeline tests | Authored characters, clips and voice; a person's verdict on cloth and faces | Not yet run |
| 6. A crowd | This Mac and CI | Nothing | Not yet run |
| 7. No hitches at scale | This Mac cold and warm, CI by counts | A phone driver's cold compile | Not yet run |
| 8. Still authorable | This Mac | Nothing | Not yet run |

**The shortest path:** "Before anything", then `cargo xtask bench-scale run --counts` and `report` (the counted halves of exit tests 1, 2, 6 and 7), the GPU tests and goldens under exit test 3, and the replays under exit tests 6 and 7. That's everything but the timed sittings, in an afternoon once prepared. The timed sittings (exit tests 1, 2 and 5, and the crowd's row in 6) need the machine to themselves, about ten minutes each.

## Before anything

```bash
cargo metadata --locked --format-version 1 > /dev/null   # xtask isn't --locked: catch a stale Cargo.lock first
cargo xtask content                                       # the desktop cook, into target/content/desktop
cargo xtask content --cook mobile                         # the phones' cook
df -h /                                                   # prepare takes gigabytes
cargo xtask bench-scale prepare                           # both cooks with the kinds, the budget district, the build, the replays
cargo xtask bench-scale plan                              # the sittings, the minutes by tier, what is left
cargo xtask bench-content --preset ci --with-content      # the ci district beside the content, for the replays tests
cargo xtask bench-content --preset ci --with-content --cook mobile
```

`prepare` takes most of an hour cold: two `budget` cooks of about 1.1 GB each beside `target/bench/`'s cache, the release `dev` sandbox and the testkit copied into `bin/` so a later build can't change what a resumed batch runs. Everything lands under `target/bench-scale/<commit>/` (with `-dirty` for a tree with changes), never committed: each run's log is `logs/<run>.<tier>[.<variant>].r<n>.log`, and `report` writes `logs/report.md` beside them. A new commit needs a new `prepare`. [development-iteration.md](development-iteration.md), "The benches at scale", has the batch and how to read its report; [bench/scale.ron](../bench/scale.ron) is the batch itself, one entry a run.

The tiers, as the manifest draws them: Desktop-High at 2560×1440, Desktop-Low at 1920×1080, both from the desktop cook; Mobile-High and Mobile-Low on a phone's panel, 2532×1170, their scenes capped at 740 and 590 rows and upscaled as they ship, from the mobile cook (the phones read the district's own phone tree, B6). Mobile-Low's lap is half the others' frames, at its 30 fps.

**Counted and timed.** A counting walk reads no time and runs in an ordinary gate slot: `cargo xtask bench-scale run --counts`. **A timed sitting needs the machine to itself:** nothing else building, the load under 8 as it starts. On the run's machine that is the orchestrator's alone, in a quiet window: `slot --all --load 8 cargo xtask bench-scale run`, once a sitting, until `plan` says nothing is left (`--sitting <n>` runs one again). Each reading records the load at its start and end, and the report marks one above the manifest's bound (8) rather than dropping it. The steps below say which they are.

**The report judges nothing per exit test.** `cargo xtask bench-scale report` fails only on a count missing, a failed unit, an exact count that differs between rounds, or a pipeline built after a load. A row over its budget is bold and says by how much; it does not fail the report. The pass rules below are this runbook's, applied by reading the rows.

### The Simulator and the emulator

M5's batch runs nothing on either. M4's recipes for a private Simulator and the headless emulator stand ([m4-exit-run.md](m4-exit-run.md), "The Simulator and the emulator"): a Simulator made with `simctl create`, booted with `simctl boot`, never Simulator.app; the emulator with `-no-window -gpu host`; one up at a time. The sandbox's Android activity is `dev.sapphire.sandbox/.SandboxActivity`. Exit test 2's §1 row names both as places its counters prove it, but no tool today installs the district beside a phone bundle and walks it there: see exit test 2.

## Exit test 1: the district holds the frame

The test (§1): "A scripted walk of the benchmark district: dusk into night, dry into wet, through its crowd, ending in an encounter of twelve enemies against a party of three. With the view at §3.4's counts (2,500 draws and 64 lights on a desktop, 700 and 8 on a phone), every row of §3.2 and §3.3 is at or under its budget at each tier's settings."

No single run walks all of that. Three timed runs stand for it: `exit-walk` (the tour under a running day, `--day-length 935`, dusk into night, the crowd on), `night-weather` (the night held dry against `--weather 0.8,0.4`), and `district-fight` (the party of three against the twelve, at night). `tour-dusk` and `tour-night` are B4's walks, read beside.

**What proves it:**

- **Draws and lights, counted (this Mac, any slot).** The ruling: §3.4's draw budget is on the main view's surface draws after culling (`main draws`), read from the exact counted walks, never the timed ones. `cargo xtask bench-scale run --counts` walks `counted` (the tour at dusk, a frame in seven drawn, the crowd on, whole textures) on every tier. Read in each `logs/counted.<tier>.r<n>.log`, or the report's counts table for `counted`:

  ```
  count row phase="drawn" row="main draws" frames= p50= p95= worst= … budget=Some(2500) over_p50= over_worst=
  count row phase="drawn" row="lights" frames= p50= p95= worst= … budget=Some(64) over_p50= over_worst=
  ```

  The phones' rows read `budget=Some(700)` and `budget=Some(8)`. The sorted colour views with instanced runs (D2b) change the phones' figure; the rule below holds whatever it is on the day.
- **Draws on CI (nightly and on dispatch only).** `Scale lane (vulkan-lavapipe, <walk>)` walks the `ci` district by counts (`cargo xtask counts`: the eight walks desktop-high-dusk … mobile-low-night, at 320×180, a frame in seven) and holds every row exactly against `tests/counts/district-ci.txt`. It proves the counts don't move between commits; the `ci` district is small, so its draws are not the budget's.
- **The frame's rows, timed (this Mac, the orchestrator's quiet window).** `slot --all --load 8 cargo xtask bench-scale run`, sitting by sitting, then `report`. Read each tier's section for `exit-walk`, `night-weather` (both variants), `district-fight`, `tour-dusk` and `tour-night`: the CPU table against §3.2, the GPU table against §3.3. A time is the least of the rounds at p50 and at p95, with the worst beside it. Metal overlaps its passes, so of its GPU rows only `gpu frame` is exact; the pass rows are read and owed.

**Passes when**, on each tier:

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `main draws` on `counted`, every round | `over_worst=false` (≤ 2,500) | `over_worst=false` (≤ 2,500) | `over_worst=false` (≤ 700) | `over_worst=false` (≤ 700) |
| `lights` on `counted` | worst ≤ 64 | worst ≤ 64 | worst ≤ 8 | worst ≤ 8 |
| `cpu frame`, each timed run | p95 ≤ 8 ms | p95 ≤ 12 ms | p95 ≤ 12 ms | p95 ≤ 22 ms |
| `gpu frame`, each timed run | p95 ≤ 12 ms | p95 ≤ 14 ms | p95 ≤ 13 ms | p95 ≤ 26 ms |
| §3.2's stage rows and §3.3's pass rows, each timed run | no adopted column: read beside B4's proposal, not judged | each row's p95 at or under its Desktop-Low figure | no adopted column: read beside B4's proposal, not judged | each row's p95 at or under its Mobile-Low figure |

The statistic, p95 of the least round, is this runbook's proposal: §3 states budgets without a percentile, and the orchestrator rules it at the rehearsal. The sandbox logs a budget only on `cpu frame` and `gpu frame` for all four tiers; every other timed row carries Desktop-Low's and Mobile-Low's figures alone (`fight_bench.rs`'s rows), so on Desktop-High and Mobile-High only the frame is judged. `gpu main` holds §3.3's forward opaque, outlines, and hair, eyes and transparents together (4.9 ms and 11.5 ms).

**Can't prove here:** every reference device's times; exclusive GPU pass times, which need a Vulkan or DX12 GPU. Owed to a maintainer with the devices and a desktop GPU.

## Exit test 2: memory holds under a hard pool

The test (§1): "The district's textures come to twice the tier's pool. Through the walk the pool never exceeds its budget and the process stays under §3.1's memory. A surface newly in view reaches the level it asks for within 30 frames at the tier's rate (99th percentile; a proposal M5 sets). Nothing loads on the frame's thread, bytes decoded and waiting for the GPU stay under a cap, and after a trip the mesh pool returns to its floor."

`streaming` (timed) and `counted-streaming` (counted) give each tier a pool of half what its cook's walk holds whole, so the district's textures come to twice it: `--texture-pool 1280` on the desktop tiers, `331` on the phones (MiB). `trip` plays the committed zone-trip replay, a stand-in for a trip from the district.

**What proves it:**

- **The pool, counted** (`cargo xtask bench-scale run --counts`, `logs/counted-streaming.<tier>.r<n>.log`, and the report's streaming table):
  - `count row … row="bytes over budget in use"`: worst 0, or `count row not exercised … row="bytes over budget in use"`.
  - `row="streamed texture bytes"` worst at or under `row="streamed texture pool"` worst, which is the run's `--texture-pool` in bytes (1280 MiB = 1,342,177,280; 331 MiB = 347,078,656). `texture pool bytes` carries the tier's §3.4 pool as its budget, not the run's, so it is read and not judged here.
  - `row="bytes decoded waiting"`: worst at or under the admission cap, 256 MiB on the desktop tiers and 64 MiB on the phones (`AssetBudgets::admission`, M2), or not exercised. The row logs no budget of its own.
- **The process, timed** (`streaming`'s Memory table in the report: "Memory through the cold walk (`first` phase), MiB, start / most / end"). Read the Process row's "most".
- **The 30-frame rule: every completed wait.** The ruling: read every completed wait, pool-held and loads-only together; each phase, the cold one too, is held on its own; a wait still open at a run's end for 30 frames or more fails. The line is **from the waits counter** (a package beside this one; check at the rehearsal that it is on the head). At each phase's end the sandbox logs:

  ```
  texture waits phase="…" completed= p50_frames= p99_frames= worst_frames= pool_held= pool_held_p99_frames= pool_held_worst_frames= open= longest_open_frames= pool_open= pool_longest_open_frames=
  ```

  An empty histogram prints `-`, and the last bin prints `32+`. The report doesn't read it yet, so read the logs:

  ```bash
  grep -h 'texture waits phase=' target/bench-scale/<commit>/logs/counted-streaming.*.log target/bench-scale/<commit>/logs/streaming.*.log
  ```

  Until the counter is on the head, the report's line "**A surface newly in view at its level within 30 frames (exit test 2):**", a bound from the trace's `worst_texture_debt` column, is all there is: read and noted, not a pass.
- **Nothing on the frame's thread.** The report's line "**Nothing loaded on the frame's thread:**" says it holds by construction (reads and decodes run on the asset server's threads); it is not counted.
- **The trip, timed** (`trip`'s section): "at the lap's last frame the mesh pool is back at its floor". The run is a stand-in, which its section says, so it is read and never a pass of a trip from the district.

**Passes when**, on each tier (desktop pools 1280 MiB on both desktop tiers, phones 331 MiB):

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `bytes over budget in use`, `counted-streaming` | 0 or not exercised | 0 or not exercised | 0 or not exercised | 0 or not exercised |
| `streamed texture bytes` worst ≤ the run's pool | ≤ 1280 MiB | ≤ 1280 MiB | ≤ 331 MiB | ≤ 331 MiB |
| Process "most", `streaming` | ≤ 10 GB | ≤ 6 GB | ≤ 2.8 GB | ≤ 1.6 GB |
| `texture waits`, each phase, each run (from the waits counter) | `p99_frames` ≤ 30 and `longest_open_frames` < 30 | the same | the same | the same |
| `bytes decoded waiting` worst | ≤ 256 MiB | ≤ 256 MiB | ≤ 64 MiB | ≤ 64 MiB |
| `trip` | back at its floor (stand-in: read) | the same | the same | the same |

A `p99_frames` of `32+` fails; `-` (no wait completed) passes. §3.1's desktop VRAM figures (6 GB and 3 GB) can't be read on unified memory: the Process row stands for RAM alone.

**On the Simulator and the emulator:** nothing yet. §1 names both as places this test's counters run, but no command today installs the `budget` or `ci` district beside a phone bundle and walks it with `--texture-streaming on --texture-pool 331`. Until one does, this half is not run, and the rehearsal says so.

**Can't prove here:** a phone's own memory limit and the OS killing the app at it, a discrete GPU's VRAM, and whether a level arriving late reads as a pop to a person. Owed to a maintainer with a phone and a desktop GPU, and to a person watching.

## Exit test 3: the look survives scale

The test (§1): "Sorting, instancing, culling, levels of detail, streaming and the phone's upscaler leave the picture alone: every existing golden stays byte-identical; each optimization has a test that draws the same image with it on and off, or within a stated gate where it can't be exact; at each switch of a level of detail, outline widths in output pixels and the shading's two-tone edge stay inside the testkit's gates."

**What proves it:**

- **Every golden, on three backends.** `cargo xtask golden` on Metal here (463 scenes; a warm run takes minutes), every scene `ok`. CI holds lavapipe and WARP: `Golden images (vulkan-lavapipe n/5)` and `Golden images (dx12-warp n/8)` green on the head, and `Golden images (metal 1/1)`. Each scene also fails on a first use in its captured frame. **Which references moved:** `git diff --name-only --diff-filter=M v0.1.0..HEAD -- 'tests/golden/*.png'` lists every existing reference M5 changed (139 scenes at 800c4d9a3, most of them the look packages' re-baselines); each must be a re-baseline its package's row in [m5-plan.md](m5-plan.md) names, and none a scale package's (D1 to D10, M5, L15, L18, or L14 with the upscaler off).
- **On and off, per optimization** (`cargo test -p sapphire_render --test gpu --locked -q -- --ignored --test-threads=1 <filter>`, Metal here; CI's `GPU tests (<family>)` and `GPU tests (WARP)` hold them on lavapipe and WARP):

  | Optimization | Test | Rule |
  |---|---|---|
  | Sorted lists and instanced runs (D1, D2) | `render::runs::runs_draw_what_their_draws_drew_one_by_one_on_every_tier`, `render::runs::a_tie_between_a_runs_instances_shows_one_of_them` | exact |
  | Culling (D3) | `render::culling::culling_by_the_frustum_alone_draws_what_no_culling_draws_on_every_tier`, `view_culling::offscreen_instances_are_culled_and_change_no_pixel`, `render::culling::a_character_panned_out_of_view_and_back_moves_as_if_never_culled` | exact |
  | The world's levels (D4) | `render::levels::levels_off_draw_what_a_mesh_without_levels_draws`, `render::levels::cells_at_any_two_levels_meet_with_no_crack`, `render::levels::cells_at_any_two_levels_cast_with_no_light_leak` | exact, and the seams' gates |
  | Characters' levels (D5) | `render::character_levels::levels_off_draw_a_character_as_before_on_every_tier`, `render::character_levels::depth_and_motion_across_a_switch_are_those_of_no_switch` | exact |
  | Kept cascades (D8) | `shadows::kept::kept_cascades_hold_what_drawing_in_turns_draws_under_the_same_fits` | exact |
  | Depth-equal main pass (D10) | `render::depth_equal::the_main_pass_draws_every_byte_it_drew_testing_equal_on_every_tier` | exact |
  | Texture streaming (M5) | `texture_coverage::textures_at_the_top_their_estimate_needs_draw_the_whole_textures_picture`, `mip_copies::compressed_textures_stream_level_by_level_by_copies` | exact |
  | Merged character materials (L15) | `merge::a_merged_character_draws_fewer_ranges_and_the_same_picture` | exact |
  | Capsule bins (L18) | `capsule_bins::sixteen_capsules_or_fewer_draw_the_same_bytes_binned_on_every_tier` | exact |
  | The phones' upscaler (L14) | `look_tiers::the_edge_upscaler_beats_bilinear_at_each_phone_floor`, `upscale::the_edge_upscaler_beats_bilinear_with_no_halo_or_ring` | within its gate |
  | Debug views (D5, Z3) | `render::scale_views::with_no_view_shown_every_tier_draws_what_it_draws_without_them` | exact |

  The `zone-district-*`, `lod-*`, `lod-crowd-*`, `debug-levels*`, `debug-runs`, `debug-culling`, `debug-textures` and `upscale-*` goldens hold the same work in a picture.
- **At each switch.** `look_tiers::lines_and_two_tone_edges_hold_at_every_tier_and_scale` holds the lines and the two-tone edge at every tier and render scale; `cargo test -q -p sapphire_render --lib view_cull::tests::a_camera_held_at_a_switch_never_flaps` holds that a level doesn't flap. **No standing test measures outline widths and the two-tone edge across a level switch:** D5 measured it once (`--lods 0` against `--lods 1` on the look-dev enemy stage: line width within 1.9%, strength within 1.2%, the two-tone edge within 0.1%, against ±10%, ±10% and ±5%), and the world's levels (D4) have none. Until a test does, this clause is read from D5's row and not passed.

**Passes when**, on every tier: every golden `ok` on Metal and green in CI's lavapipe and WARP jobs; every moved reference named by a look package's row; every test above passes; the switch clause as above. The tiers are covered within each test (`on_every_tier`) and by the `tier-*` scenes.

**Can't prove here:** a person's eye on motion, which no golden shows: a level popping, a culled character coming back. Owed to a person watching the district move.

## Exit test 4: the picture's missing parts

The test (§1): "Reflection probes with wet streets, effect meshes with distortion, ground decals, depth of field, the effects pack and cut-ins, dissolves and driven emission, characters shadowing each other, light shafts. Each has goldens on three backends, a stated behaviour on each tier, a budget row it's measured against, a document that authors it, and nothing built or dispatched while it's off."

**What proves it,** feature by feature. Goldens by `cargo xtask golden --scene <name>` here and CI's three golden jobs; each scene's references must exist for `metal`, `vulkan-lavapipe` and `dx12-warp` (`ls tests/golden/<scene>.*.png`). The tier behaviour is the package's row in [m5-plan.md](m5-plan.md) and its spec section's As built; the budget row is spec §3.3's; the document is [m5-handoff.md](m5-handoff.md)'s "How to add" (until the next handoff holds them).

| Feature (package) | Goldens | Budget row (§3.3) | Recipe in "How to add" |
|---|---|---|---|
| Reflection probes, wet streets (L2, L3, L4) | `reflect-*`, `wet-*` | Reflection probes (inside forward opaque and AO) | A reflection probe; A zone's weather and wet surfaces |
| Effect meshes, distortion (L5, L6) | `vfxmesh-*`, `distort-*` | Particles, distortion | An effect mesh; A bend |
| Ground decals (L7) | `decal-*` | Ground decals (proposal) | A decal |
| Depth of field (L8) | `dof`, `dof-closeup` | Depth of field (not in the total) | A focus key |
| The effects pack, cut-ins (L9, L10) | `effects-*`, `cutin-*` | Bloom + tonemap + LUT, screen effects | A cut-in |
| Dissolves, driven emission (L11) | `dissolve-*` | Forward opaque | A dissolve / flash / glass |
| Characters shadowing each other (L12, L16) | `shadows-party`, `shadows-spot*`, `hair-shadow` | Shadows | none by name (Light shafts' and the party's recipes touch it) |
| Light shafts (L13) | `shafts-golden` | Bloom + tonemap + LUT (the fused pass) | Light shafts |

"Nothing built or dispatched while it's off" is held by `ambient::nothing_is_built_or_run_for_work_nothing_reads` and `scene_prepass::mobile_low_builds_none_of_a_pass_it_never_runs` (`cargo test -p sapphire_render --test gpu --locked -q -- --ignored --test-threads=1 <filter>`), and by each package's own off test named in its row.

The budget rows are timed by exit test 1's sittings (`gpu effects`, `gpu main`, `gpu post`, `gpu shadows`); the sandbox has no row of its own for reflections, decals, depth of field or shafts, so a feature's cost is the package's own measurement, cited from its row.

**Passes when,** for each of the eight: its goldens exist on all three backends and pass; its row states a behaviour on each of the four tiers; its cost, as its row measured it, is at or under its §3.3 row on Desktop-Low and Mobile-Low; a recipe authors it; its off test passes. A checklist, one line a feature, in the run's table.

**Can't prove here:** an artist's eye on any of it, and Zenless Zone Zero references, of which there are none. The look is judged by description against the Wuthering Waves frames kept outside the repository. Owed to an artist.

## Exit test 5: characters at budget

The test (§1): "A party of three kinds, each with its own mesh, skeleton, graph and springs at §3.4's hero row, and twelve enemies at 8,000 triangles. Animation and IK, secondary motion with cloth, skinning, outlines and the character prepass each hold their row on each tier. A conversation plays its voice lines with expressions and lip curves. Cloth stays bounded through a ten-minute soak at 30, 60 and 120 Hz."

**What proves it:**

- **The cast at its rows.** `cargo test -q -p sapphire_pipeline --lib bench::cast::tests::every_budget_figure_cooks_at_its_row`: the hero ×3 at 80,000 / 30,000 triangles, 200 joints, 6 materials (4 on mobile); the enemies at 8,000 (6,610 on mobile).
- **The rows, timed** (the orchestrator's sittings): `party-kinds` and `district-fight`, and `crowd-tour`'s two variants. Read each tier's "Characters (exit test 5):" table (`animation and ik`, `secondary motion`, `gpu skinning`, `gpu main`) and its counted rows (`characters animated`, `poses evaluated`, `skinned instances`, `skinned vertices`, `joints`, `character prepass draws`, `outline draws`). `gpu prepasses` in the GPU table holds the character prepass with the others.
- **A conversation.** The C5 and C6 tests: `cargo test -q -p sapphire_stage --lib audio::tests::a_conversations_voice_plays_where_its_line_says_and_its_mouth_keeps_time`, `cargo test -q -p sapphire_audio --lib system::tests::a_conversations_line_speaks_once_holds_on_a_pause_and_is_cut_by_the_next`, and the `face-*` goldens.
- **Cloth.** `cargo test -q -p sapphire_anim --lib springs::cloth::tests::a_ten_minute_soak_stays_bounded` (30, 60 and 120 Hz), and `cargo test -q -p sapphire_pipeline --test party_springs the_cloth_switching_at_its_distance_never_pops`.

**Passes when:**

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `animation and ik`, p95 | not judged (no column) | ≤ 1.5 ms | not judged | ≤ 3.0 ms |
| `secondary motion`, p95 | not judged | ≤ 0.5 ms | not judged | ≤ 1.0 ms |
| `gpu skinning`, p95 (proposed row) | ≤ 0.4 ms | ≤ 0.5 ms | ≤ 0.6 ms | ≤ 1.0 ms |
| `gpu main` (outlines within it), p95 | not judged | ≤ 4.9 ms | not judged | ≤ 11.5 ms |
| `gpu prepasses` (the character prepass within it), p95 | not judged | ≤ 1.5 ms | not judged | ≤ 1.5 ms |
| the cast, the conversation and cloth tests | pass | pass | pass | pass |

The skinning row is B4's proposal and the sandbox logs no budget beside it, so it is read against the figures above by hand. Outlines (0.6 / 1.5 ms) and the character prepass have no timed row of their own: they are read inside `gpu main` and `gpu prepasses`, which Metal's overlap makes inexact anyway.

**Can't prove here:** a second authored character, authored clips, recorded voice, and a person's verdict on the cloth and the faces. The figures and voice lines are generated. Owed to an artist, an animator and a person watching.

## Exit test 6: a crowd

The test (§1): "Sixty ambient characters on the desktop tiers and thirty on the phones' walk the district inside the animation row. They are presentation only: every committed replay reaches its checksum with the crowd on and off."

**What proves it:**

- **How many, counted.** `counted`'s log (`cargo xtask bench-scale run --counts`): `count row phase="drawn" row="ambient characters spawned"`.
- **Inside the animation row, timed** (the orchestrator's sittings): `crowd-tour`'s `on` variant, its "Characters (exit test 5):" table's `animation and ik`, with `off` beside it; `crowd-hold` holds the crowd at its densest.
- **Presentation only.** The ruling: the replay's checksum with the crowd on equals the checksum with it off. `cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1 <name>` for `a_zones_ambient_characters_change_nothing_the_checksums_see` (zone-trip, crowd on and off), `a_zones_people_stand_the_same_at_a_tick_however_the_frames_came`, `a_zones_people_hold_no_springs_and_draw_at_their_levels`, and `the_district_encounter_plays_with_no_hitch_cold_and_warm` (which plays the district's fight with the crowd on and off and reads the same end checksum). `cargo xtask replay` holds every committed replay's checksums headless, where no crowd exists. CI's `App tests (vulkan-lavapipe, the-rest)` runs the sandbox's replays tests.

**Passes when:**

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `ambient characters spawned` worst, `counted` | 60 | 60 | 30 | 30 |
| `animation and ik` p95, `crowd-tour` on | not judged | ≤ 1.5 ms | not judged | ≤ 3.0 ms |
| replays tests above and `cargo xtask replay` | pass | pass | pass | pass |

Only zone-trip and the district's encounter are played with the crowd on and off; the other committed replays are held headless, where the crowd isn't, which proves the simulation never reads it but not "every committed replay … with the crowd on".

**Can't prove here:** nothing §1 names; the crowd's look on a phone rides with exit tests 1 and 3.

## Exit test 7: no hitches at scale

The test (§1): "`zero_hitches` plays the district's walk too: no first use, no pipeline built on the frame's thread, and no frame waiting on a load, a level of detail or a texture level."

**What proves it:**

- **The district's walk, counted** (`cargo xtask bench-scale run --counts`): `counted` and `counted-streaming` on every tier. Read the `zone walk` line of each log, `… prewarmed= first_use= rebuilt= warmed_off_frame= frame_thread= frame_waits=`, and the report's "Pipelines built after the load (exit test 7, by counts):". The report fails on a pipeline built after the load.
- **The district's fight, cold and warm** (Metal here): `cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1 the_district_encounter_plays_with_no_hitch_cold_and_warm`, Desktop-High and Mobile-Low, cold then warm, crowd on and off; each `fight pipelines` row must read `first_use=0 frame_thread=0 frame_waits=0 rebuilt=0`. It needs the `ci` district ("Before anything").
- **The timed runs' pipelines** (the orchestrator's sittings): `exit-walk`, `district-fight` and `party-kinds` read Pipelines too.
- **The ward, as in M4.** `cargo test -p game_app --features dev --locked --test zero_hitches -- --ignored` here, cold and warm; CI's `App tests (vulkan-lavapipe, walk-desktop-low | walk-mobile-low | walk-mobile-high)` by counts. `zero_hitches` plays the Lantern Ward, not the district; the counting walks and the fight's rows stand in on the district.
- **A texture level.** Under the hard pool, `counted-streaming`'s `frame_waits` counts frames that waited on a pipeline; a frame that drew a lower texture level while a higher one loaded is a wait in exit test 2's line, not a stall.

**Passes when,** on every tier: every `zone walk` line on `counted` and `counted-streaming` reads `first_use=0 rebuilt=0 frame_thread=0 frame_waits=0`; the report lists no pipeline built after a load; `the_district_encounter_plays_with_no_hitch_cold_and_warm` passes (it runs Desktop-High and Mobile-Low; the counted walks cover all four); `zero_hitches` passes here and in CI's three lanes.

**Can't prove here:** a phone driver's cold compile, as in M4; a desktop GPU's driver cache on Windows and Linux. Owed to a maintainer with the devices.

## Exit test 8: still authorable

The test (§1): "Each new thing has a "How to add" recipe with no Rust step: a level-of-detail setting, a reflection probe, a zone's weather, a decal, an effect mesh, a focus key, a cloth, a face track, an ambient character and a playable kind."

**What proves it:** on a scratch branch, dropped after, walk each recipe in [m5-handoff.md](m5-handoff.md)'s "How to add" (or the next handoff's, once it holds M5's) as written: A playable kind; A cloth; A reflection probe; A zone's weather and wet surfaces; Ambient characters; Which characters draw coarser and a close-up (a level-of-detail setting; the world's cells take theirs in the zone recipe's sidecar, `importer: Zone((lods: …))`); A face track; A decal; An effect mesh; A focus key. After each, cook (`cargo xtask content`) and run the check the recipe names, then:

```bash
git diff --stat main -- '*.rs'    # empty: no Rust
```

**Passes when:** all ten walked, each cooks and its check passes, and the diff above is empty for each. A recipe whose steps write Rust fails until a document carries that step.

**Can't prove here:** nothing §1 names.

## Rehearsal at <commit>

The date, the commit, the machine (model, macOS, Xcode), the load, and how the timed sittings were run. Then a row a test and tier:

| Exit test | Tier | What was read | Measured | Budget | Verdict | Evidence |
|---|---|---|---|---|---|---|
| 1, `main draws` | Desktop-High | `counted`, worst | | 2,500 | | `logs/counted.desktop-high.r1.log` |
| 1, `main draws` | Desktop-Low | | | 2,500 | | |
| 1, `main draws` | Mobile-High | | | 700 | | |
| 1, `main draws` | Mobile-Low | | | 700 | | |
| 1, `lights` | each tier | | | 64 / 64 / 8 / 8 | | |
| 1, `cpu frame`, `gpu frame` | each tier, each timed run | | | §3.1 | | `report.md` |
| 1, §3.2 / §3.3 rows | Desktop-Low, Mobile-Low | | | §3.2, §3.3 | | |
| 2, pool and process | each tier | | | the run's pool; §3.1 | | |
| 2, `texture waits` | each tier, each phase | | | 30 frames | | |
| 2, decoded waiting; trip | each tier | | | 256 / 64 MiB; the floor | | |
| 3 | all | goldens, on/off tests, switch | | | | |
| 4 | all | eight features' checklist | | | | |
| 5 | each tier | Cast rows; tests | | §3.2, §3.3 | | |
| 6 | each tier | spawned; `animation and ik`; replays | | 60 / 60 / 30 / 30 | | |
| 7 | each tier | `zone walk` counts; district fight; `zero_hitches` | | 0 | | |
| 8 | all | ten recipes | | no Rust | | |

## Final run at <commit>

The same table, on the release's head, with what changed since the rehearsal.

| Exit test | Tier | What was read | Measured | Budget | Verdict | Evidence |
|---|---|---|---|---|---|---|
| 1 | | | | | | |
| 2 | | | | | | |
| 3 | | | | | | |
| 4 | | | | | | |
| 5 | | | | | | |
| 6 | | | | | | |
| 7 | | | | | | |
| 8 | | | | | | |

## What stays owed

No run on this Mac answers these; each needs a device or a person (§1, and [m5-plan.md](m5-plan.md) §7 for what the maintainer holds):

- **Exit test 1:** every reference device's frame times, and exclusive GPU pass times on a Vulkan or DX12 GPU (Metal overlaps its passes).
- **Exit test 2:** a phone's own memory limit and its kill, a discrete GPU's VRAM, and whether a level arriving late reads as a pop to a person; and, until a tool walks the district there, the Simulator's and the emulator's counters.
- **Exit test 3:** a person's eye on motion.
- **Exit test 4:** an artist's eye, and Zenless Zone Zero references.
- **Exit test 5:** a second authored character, authored clips, recorded voice, and a person's verdict on the cloth and the faces.
- **Exit test 7:** a phone driver's cold compile, and a desktop GPU's driver cache on Windows and Linux.
- **All of it:** nobody has played anything, and no test here asks whether it's fun.
