# The M5 exit run

M5 ends when its eight exit tests are answered ([m5-plan.md](m5-plan.md) §1): the district holding the frame, memory under a hard pool, the look surviving scale, the picture's missing parts, characters at budget, a crowd, no hitches at scale, and recipes with no Rust. This file is the procedure for all eight, for someone coming back to it cold. For each test it says what is run, which line or report row is read, and the rule that passes or fails it on each tier, with the budget from [engine-spec.md](engine-spec.md) §3 beside it; where each step runs (Metal on this Mac, counted or timed; the iOS Simulator; the Android emulator; CI's lavapipe and WARP jobs, and the nightly scale lane); and what stays owed in §1's terms.

The run has one machine, an Apple M5, which is none of §3.1's reference devices. It has no phone, no Windows or Linux GPU of its own, no artist and no animator, and it may not open a window or download content. Every clause below that needs one of those is owed, and says so. A time read on this Mac compares commits and tiers; it is never a phone's budget met. **Nobody has played anything,** and no test here asks whether it's fun (§1, "Feel stays unknown").

It is run three times: a rehearsal before the last packages land ("Rehearsal at …"), the final run on the release's head ("Final run at …"), and the owed rows later, by a maintainer with devices ("What stays owed").

| Exit test | Who answers it | Needs | Where it stands |
|---|---|---|---|
| 1. The district holds the frame | This Mac: counted walks for draws and lights, timed walks for §3.2 and §3.3; CI's scale lane by counts | Every reference device; exclusive GPU pass times on a Vulkan or DX12 GPU | Rehearsed at ce7707fd9: **fails** on the desktops' counted draws; the timed rows not run |
| 2. Memory holds under a hard pool | This Mac by the engine's counters; the Simulator and the emulator by the same counters on the `ci` district (`bench-scale device`) | A phone's memory limit and its kill, a discrete GPU's VRAM, a person judging a late level | Rehearsed: **fails** on the waits for a texture's level, every tier, and the desktops' bytes over budget; the timed rows not run |
| 3. The look survives scale | Metal here, lavapipe and WARP in CI | A person's eye on motion | Rehearsed: passes, two moved references waiting on a ruling |
| 4. The picture's missing parts | Metal here, lavapipe and WARP in CI, the budget rows timed here | An artist's eye, Zenless Zone Zero references | Rehearsed: passes; the timed pairs not run |
| 5. Characters at budget | This Mac, timed and counted; the anim and pipeline tests | Authored characters, clips and voice; a person's verdict on cloth and faces | Rehearsed: the tests pass; the timed rows not run |
| 6. A crowd | This Mac and CI | Nothing | Rehearsed: the count and the replays pass; the timed row not run |
| 7. No hitches at scale | This Mac cold and warm, CI by counts | A phone driver's cold compile | Rehearsed: passes, by counts and tests |
| 8. Still authorable | This Mac | Nothing | Rehearsed: all ten recipes walked with no Rust |

**The shortest path:** "Before anything", then `cargo xtask bench-scale run --counts`, `bench-scale device` on each device, and `report` (the counted halves of exit tests 1, 2, 6 and 7), the GPU tests and goldens under exit test 3, and the replays under exit tests 6 and 7. That's everything but the timed sittings. The timed sittings (exit tests 1, 2 and 5, and the crowd's row in 6) need the machine to themselves, and there are many: at ce7707fd9 `plan` counted 54 sittings of 5.5 to 9.8 minutes, 431 minutes in all, about seven hours of a quiet machine. Plan a night's window for them, never an afternoon.

## Before anything

```bash
cargo metadata --locked --format-version 1 > /dev/null   # xtask isn't --locked: catch a stale Cargo.lock first
cargo xtask content                                       # the desktop cook, into target/content/desktop
cargo xtask content --cook mobile                         # the phones' cook
df -h /                                                   # prepare takes gigabytes
slot --all --load 8 cargo xtask bench-scale prepare       # alone on the machine: both cooks with the kinds, four budget cooks, the build, the replays
cargo xtask bench-scale plan                              # the sittings, the minutes by tier, what is left
cargo xtask bench-content --preset ci --with-content      # the ci district beside the content, for the replays tests
cargo xtask bench-content --preset ci --with-content --cook mobile
```

`prepare` takes most of an hour cold: four `budget` cooks of about 1.1 GB each beside `target/bench/`'s cache (the desktop's and the phones', each with its cells' levels and again without them for `cell-levels`), the release `dev` sandbox and the testkit copied into `bin/` so a later build can't change what a resumed batch runs. The cooks take every core for minutes at a time, so run it with nothing else running, as the timed sittings are run: in the rehearsal it ran in a plain gate slot beside other agents' builds, and loaded the machine for everything else on it. Warm, with `target/bench/cache` kept, it took 832 s. Everything lands under `target/bench-scale/<commit>/` (with `-dirty` for a tree with changes), never committed: each run's log is `logs/<run>.<tier>[.<variant>].r<n>.log`, and `report` writes `logs/report.md` beside them.

**Run the batch from a worktree left at the prepared commit.** Every `bench-scale` command finds its folder by the commit `HEAD` names when it runs, and reads the manifest, [bench/scale.ron](../bench/scale.ron), from the working tree. So a commit made in that worktree, even one that touches only docs, leaves the batch looking unprepared under the new commit's key, and an edit to the manifest changes what a resumed batch runs. Prepare at the commit being measured, run every sitting and `report` there, with the tree clean, and commit the run's record (this file's table) on another branch until `report` has read the batch. A new commit needs a new `prepare`. [development-iteration.md](development-iteration.md), "The benches at scale", has the batch and how to read its report; [bench/scale.ron](../bench/scale.ron) is the batch itself, one entry a run.

The tiers, as the manifest draws them: Desktop-High at 2560×1440, Desktop-Low at 1920×1080, both from the desktop cook; Mobile-High and Mobile-Low on a phone's panel, 2532×1170, their scenes capped at 740 and 590 rows and upscaled as they ship, from the mobile cook (the phones read the district's own phone tree, B6). Mobile-Low's lap is half the others' frames, at its 30 fps.

**Counted and timed.** A counting walk reads no time and runs in an ordinary gate slot: `cargo xtask bench-scale run --counts`. The sandbox refuses `--bench-trace` on one (`--zone-walk` with `--draw-every`), since it times nothing and writes no trace. **A timed sitting needs the machine to itself:** nothing else building, the load under 8 as it starts. On the run's machine that is the orchestrator's alone, in a quiet window: `slot --all --load 8 cargo xtask bench-scale run`, once a sitting, until `plan` says nothing is left (`--sitting <n>` runs one again). Each reading records the load at its start and end, and the report marks one above the manifest's bound (8) rather than dropping it. The steps below say which they are.

**Which counts are exact.** A counting walk's draws, lights, triangles, instances, characters, levels and pipelines are the same in every round. Its streaming rows are not: what streams follows when each read lands, so they move between rounds, on `counted-streaming` most of all. At ce7707fd9, of `counted-streaming`'s 93 rows, 13 differed between its two rounds on Desktop-High, 12 on Desktop-Low and 7 on each phone: `bytes admitted`, `bytes given back`, `bytes over budget in use` (worst 3,428,652 bytes against 2,179,076 on Desktop-High), `bytes decoded waiting`, `streamed texture bytes` (its total; its worst held), `texture pool bytes`, `texture bytes raised`, `texture levels raised` and `lowered`, `texture lowers waiting`, `textures in debt` and `worst texture debt frames`, the `texture waits` line, and the `zone walk` line's upload fields and `texture_bytes_peak`. On the walks that draw whole textures, every count row matched on the desktops; on the phones the `zone walk` line's `upload_bytes`, `upload_bytes_most` and `upload_frames` moved (and once `over_budget_frames`, 1 against 0), and so did Mobile-Low `counted`'s totals of `mesh pool capacity` and `mesh pool floor` (their p50, p95 and worst held). The report shows a row that moved as "differs between rounds", each round's figures beside the other's, and judges each round's worst; whether it should fail on a streaming row that moves is decided with exit test 2's fix.

**The report judges what the batch measures (M5 Z2).** `cargo xtask bench-scale report` opens with a Verdicts section: one line for each exit test and tier, **pass**, **fail**, or *not judged here* with the reason (a run not made yet, a stand-in, a test whose proof is outside the batch), by this runbook's rules below, and it exits non-zero on any fail. It still fails, as before, on a count missing, a failed unit, an exact count that differs between rounds, or a pipeline built after a load. What it judges: exit test 1's counted draws and lights and every timed row that logs a budget; exit test 2's pools, admission cap, frame-thread loads, waits and the process against §3.1; exit test 5's and 6's rows and the crowd's count; exit test 7's pipelines. Exit tests 3, 4 and 8, and each test's halves outside the batch (the GPU tests, goldens, replays, the cast's, conversation's and cloth's tests, `zero_hitches`, the recipes), stay this runbook's steps: the section names them on each line. The manifest's `deviations` print above the table: where a run stands in for what §1 asks, as a line of its own.

### The Simulator and the emulator

M4's recipes for a private Simulator and the headless emulator stand ([m4-exit-run.md](m4-exit-run.md), "The Simulator and the emulator"): a Simulator made with `simctl create`, booted with `simctl boot`, never Simulator.app; the emulator with `-no-window -gpu host`; one up at a time. The sandbox's Android activity is `dev.sapphire.sandbox/.SandboxActivity`. M5's batch walks one run on both, exit test 2's counters (M5 Z2), with one command a device that keeps the device's whole life inside it:

```bash
cargo xtask bench-scale device --platform ios-sim --dry-run       # cook and bundle only: build before taking the device
slot --device cargo xtask bench-scale device --platform ios-sim   # a private iPhone 17 Pro simulator, made, walked, deleted
cargo xtask bench-scale device --platform android --dry-run       # JAVA_HOME as running.md gives it; Gradle offline
slot --device cargo xtask bench-scale device --platform android   # sapphire_api36, -no-window -gpu host -no-snapshot-save
```

It puts the shipped mobile cook and the `ci` district's phone tree together in `content/ci/mobile/` under the commit's folder, bundles the release `dev` sandbox with them in place of the mobile cook (only a `dev` bundle takes other content), boots its own device headless (a simulator from `simctl create` on the newest runtime; the emulator, refusing to start beside a device already connected), installs the bundle fresh (the app uninstalled first on the emulator), walks the manifest's `device-streaming` run on Mobile-High and Mobile-Low (`--tier` picks one; `--smoke` walks 200 frames at 320x180 into `smoke/`), and shuts the device down however the walks went. The walks go through `cargo xtask bench-scale launch`, the testkit's launcher, which starts the sandbox on the booted device and prints its log as it runs: the Simulator's through `simctl launch --console-pty`, the emulator's from logcat until the app's `main returned status=` line. Each log comes back as `logs/device-streaming.<tier>.<ios-sim|android>.r1.log`, beside the Mac's; the command prints the device's verdict lines and fails on a fail, and `report` shows them as lines of their own. See exit test 2 for what they judge.

## Exit test 1: the district holds the frame

The test (§1): "A scripted walk of the benchmark district: dusk into night, dry into wet, through its crowd, ending in an encounter of twelve enemies against a party of three. With the view at §3.4's counts (2,500 draws and 64 lights on a desktop, 700 and 8 on a phone), every row of §3.2 and §3.3 is at or under its budget at each tier's settings."

No single run walks all of that. Three timed runs stand for it (a ruling, and a deviation the report prints): `exit-walk` (the tour under a running day, `--day-length 935`, dusk into night, the crowd on), `night-weather` (the night held dry against `--weather 0.8,0.4`), and `district-fight` (the party of three against the twelve, at night). `tour-dusk` and `tour-night` are B4's walks, judged beside them. By counts, each has its own (M5 Z2): `counted` at dusk, `counted-night` held dry and held wet, and `counted-fight`, the district's encounter with a frame in seven drawn.

**What proves it:**

- **Draws and lights, counted (this Mac, any slot).** The ruling: §3.4's draw budget is on the main view's surface draws after culling (`main draws`), read from the exact counted walks, never the timed ones. `cargo xtask bench-scale run --counts` walks `counted` (the tour at dusk, a frame in seven drawn, the crowd on, whole textures), `counted-night` (the same at night, its `dry` and `wet` variants as `night-weather`'s) and `counted-fight` (the district's encounter at night, a frame in seven drawn) on every tier. Read in each `logs/counted.<tier>.r<n>.log`, `logs/counted-night.<tier>.<dry|wet>.r<n>.log` and `logs/counted-fight.<tier>.r<n>.log`, or the report's counts tables; the verdict reads all three:

  ```
  count row phase="drawn" row="main draws" frames= p50= p95= worst= … budget=Some(2500) over_p50= over_worst=
  count row phase="drawn" row="lights" frames= p50= p95= worst= … budget=Some(64) over_p50= over_worst=
  ```

  The phones' rows read `budget=Some(700)` and `budget=Some(8)`. The sorted colour views with instanced runs (D2b) change the phones' figure; the rule below holds whatever it is on the day.
- **Draws on CI (nightly and on dispatch only).** `Scale lane (vulkan-lavapipe, <walk>)` walks the `ci` district by counts (`cargo xtask counts`: the eight walks desktop-high-dusk … mobile-low-night, at 320×180, a frame in seven) and holds every row exactly against `tests/counts/district-ci.txt`, whose header names both cooks' sources (`sources:` the desktop's, `sources-mobile:` the phones' own tree) so a stale table fails on either. It proves the counts don't move between commits; the `ci` district is small, so its draws are not the budget's.
- **The frame's rows, timed (this Mac, the orchestrator's quiet window).** `slot --all --load 8 cargo xtask bench-scale run`, sitting by sitting, then `report`. Read each tier's section for `exit-walk`, `night-weather` (both variants), `district-fight`, `tour-dusk` and `tour-night`: the CPU table against §3.2, the GPU table against §3.3. A time is the least of the rounds at p50 and at p95, with the worst beside it. Metal overlaps its passes, so of its GPU rows only `gpu frame` is exact; the report judges the pass rows by Metal's rules below, and exclusive pass times stay owed. A timed sitting wants the display asleep and nothing playing: a video or a compositor drawing at 60 Hz beside the run slows the passes it lands on at its own beat, and voids the round.

**Passes when**, on each tier:

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `main draws` on `counted`, `counted-night` (both) and `counted-fight`, every round | `over_worst=false` (≤ 2,500) | `over_worst=false` (≤ 2,500) | `over_worst=false` (≤ 700) | `over_worst=false` (≤ 700) |
| `lights` on the same | worst ≤ 64 | worst ≤ 64 | worst ≤ 8 | worst ≤ 8 |
| `cpu frame`, each timed run | p95 ≤ 8 ms | p95 ≤ 12 ms | p95 ≤ 12 ms | p95 ≤ 22 ms |
| `gpu frame`, each timed run | p95 ≤ 12 ms | p95 ≤ 14 ms | p95 ≤ 13 ms | p95 ≤ 26 ms |
| §3.2's stage rows and §3.3's pass rows, each timed run | no adopted column: shown beside B4's proposed figure, fails nothing | each row's p95 at or under its Desktop-Low figure | no adopted column: shown beside B4's proposed figure, fails nothing | each row's p95 at or under its Mobile-Low figure |

The statistic is ruled (M5 Z2): a timed row passes on the p95 of the least round, with p50 and the worst shown beside it; a worst frame alone fails nothing. The sandbox logs a budget on `cpu frame` and `gpu frame` for all four tiers; every other timed row carries Desktop-Low's and Mobile-Low's figures as its budget (`fight_bench.rs`'s rows), and each §3.3 row logs B4's Desktop-High and Mobile-High figure as `proposed_us`. The report shows a row over a proposed figure under the verdicts, marked *proposed*, and fails nothing on it; adopting a column is moving its figures into the rows' budgets, a data edit. A grouped row is judged against the sum of its members' budgets: `gpu main` holds §3.3's forward opaque, outlines, and hair, eyes and transparents together (4.9 ms and 11.5 ms).

**On Metal, a stated deviation (M5 Z2, the orchestrator's ruling).** Metal overlaps its passes, so a pass's time is what it added beside the others, not its own. The report reads the GPU phase's line, `fight gpu reading phase="gpu" backend= overlaps= timed_frames= no_upload_frames= in_flight_pct= taa_slow_in_flight_pct= taa_slow_no_upload_pct= post_slow_in_flight_pct= post_slow_no_upload_pct= slow_frames= lock= lock_period_us=`, and where `overlaps=true`:

- **The prepasses and shadows are one row.** Metal runs the scene prepass beside a shadow cascade, so how their depth work splits between the two isn't read. `gpu depth only` is each frame's prepasses and shadows together, judged against the sum of their budgets (Desktop-Low 3.5 ms, Mobile-Low 4.5 ms; B4's 3.2 and 2.6 ms proposed on the others); `gpu prepasses` and `gpu shadows` are shown apart beside it and judged on nothing. On any other backend the two are judged apart, as logged, and `gpu depth only` on nothing.
- **A pass row is read on the frames with no upload in flight:** no bytes uploaded, new assets or raised texture levels, from two frames before to one after (`fight row no upload phase="gpu" row= samples= p50_us= p95_us= p99_us=`). An upload's copies land in whichever passes are drawing, so a frame with a texture raise has about one chance in four of every pass reading slow at once, which is the raise's cost, not the pass's. `gpu frame` and every CPU row stay on all frames. A round where fewer than a tenth of the timed frames have no upload in flight judges no pass row: they read *not judged*, never a pass. The uploads' cost is shown beside the verdicts, judged on nothing: the share of frames with an upload in flight, and the share of frames slow (over 1.15 times the round's median) in TAA and in post among those frames against the rest.
- **A round with the display's 60 Hz lock is void.** `lock` is how closely the frames slow in TAA or post (over their p95) end at one phase of a 60 Hz beat on the GPU's clock (`gpu_end` in the trace), 0 spread evenly to 1 all at one phase, at the period within 0.03% of 1/60 s that lines them up most; `-` with under 20 slow frames, which voids nothing. At 0.10 or over the round is void: its GPU rows are left out of the least of the rounds, and the verdicts show it as void with its lock and its frame's p95. A row with every round void is *not judged*. The two quiet rounds measured read 0.03 and 0.07; six rounds with another app drawing beside the run read 0.14 to 0.43.

**Can't prove here:** every reference device's times; exclusive GPU pass times, which need a Vulkan or DX12 GPU. Owed to a maintainer with the devices and a desktop GPU.

## Exit test 2: memory holds under a hard pool

The test (§1): "The district's textures come to twice the tier's pool. Through the walk the pool never exceeds its budget and the process stays under §3.1's memory. A surface newly in view reaches the level it asks for within 30 frames at the tier's rate (99th percentile; a proposal M5 sets). Nothing loads on the frame's thread, bytes decoded and waiting for the GPU stay under a cap, and after a trip the mesh pool returns to its floor."

`streaming` (timed) and `counted-streaming` (counted) give each tier a pool of exactly half what its cook's walk holds whole, in whole MiB rounded down, so the district's textures come to twice it: `--texture-pool 1303` on the desktop tiers, `333` on the phones (MiB). The whole is the `texture pool bytes` the `counted` walk holds at the tour's end: 2,733,906,860 bytes on the desktop cook, the same on B7's district of thirteen materials a cell as on sixteen (its palette draws from the same texture sets), and 699,285,048 on the mobile cook. The desktop pool holds the views' worst ask on the walk with 8% to spare (`texture bytes wanted`, 1,263,504,874 bytes on Desktop-High and 1,201,966,570 on Desktop-Low, against 1,366,294,528; on the district of sixteen materials a cell it was 1,362,595,306, 0.3% under, and 57 long waits were the pool's). `counted-streaming` is paced at the tier's frame rate (`--bench-pace 60` on three tiers, `30` on Mobile-Low), so a wait's frames are each a frame's time, as a game's are: paced, it is timed work, planned into the sittings, though it reads only counts and a quiet machine moves none of them (two walks a sitting apart read the same). `trip` plays the committed zone-trip replay, a stand-in for a trip from the district.

**What proves it:**

- **The pool, counted** (`counted-streaming`, paced, so run in the sittings by `cargo xtask bench-scale run`, not `run --counts`; `logs/counted-streaming.<tier>.r<n>.log`, and the report's streaming table):
  - `count row … row="bytes over budget in use"`: worst 0, or `count row not exercised … row="bytes over budget in use"`.
  - `row="streamed texture bytes"` worst at or under the run's pool: its budget is the pool the run's asset server holds, `--texture-pool` where given (1303 MiB = 1,366,294,528; 333 MiB = 349,175,808), else the tier's. `texture pool bytes` carries the tier's §3.4 pool as its budget, not the run's, so it is read and not judged here.
  - `row="bytes decoded waiting"`: worst at or under its budget, the admission cap the run's asset server holds, 256 MiB on the desktop tiers and 64 MiB on the phones (`AssetBudgets::admission`, M2), or not exercised.
  - `row="frame thread loads"`: worst 0 (its budget).
- **The process, timed** (`streaming`'s Memory table in the report: the cold lap (`first`) and the warm one (`cpu`), start / most / end in MiB, with §3.1's figure beside them from the manifest's tiers). The verdict takes the Process row's "most" over both laps.
- **The 30-frame rule: every completed wait.** The ruling: read every completed wait, pool-held and loads-only together; each phase, the cold one too, is held on its own; a wait still open at a run's end for 30 frames or more fails. The line is **from the waits counter** (a package beside this one; check at the rehearsal that it is on the head). At each phase's end the sandbox logs:

  ```
  texture waits phase="…" completed= p50_frames= p99_frames= worst_frames= pool_held= pool_held_p99_frames= pool_held_worst_frames= open= longest_open_frames= pool_open= pool_longest_open_frames=
  ```

  The histogram holds 0 to 1,023 frames and 1,024 or more, so a p99 is read as a number; an empty histogram prints `-`, and the last bin prints `1024+`. A counting walk logs one line too, phase `drawn`, over the frames it drew. Beside each it logs what held the waits:

  ```
  texture wait reasons start_budget= start_budget_long= loading= loading_long= raise_budget= raise_budget_long= pool_views_fit= pool_views_fit_long= pool_views_over= pool_views_over_long= phase="…" long_over_frames=30 raised_bytes= reraised_bytes= reraised_within_updates=300 tails_over_pool= arrival_waits= arrival_long= arrival_open= arrived_at_update= arrival_held_to_bound=
  ```

  Each completed wait counts once, under what held it longest: the start budget (16 starts a frame within the tier's raise budget), its levels loading, the raise budget, or the pool, split by whether the views' asks fit the pool that frame; `_long` counts those over 30 frames. `tails_over_pool` counts the updates where the tails and what sits beside them didn't fit the pool, so no plan could. **The opening is an arrival** (the orchestrator's ruling, M5 Z2): a counting walk starts from nothing, with no wipe, where a game's zone arrives under one (`Scene::zone_arrived` holds the wipe until the first view's textures are at their plan). So until the walk's first view's textures are first at their plan, the streamer raises at the wipe's budget, and a wait that opens before then is counted apart (`arrival_waits`, `_long`, `_open`), in neither histogram nor the reasons, and the update it arrived at is `arrived_at_update`. The walk itself is the same: no frame is held, and the count rows keep their ticks. A texture is at its plan once the views have been estimated with it on the renderer, read or not: one no view reads plans its tail. An arrival, a trip's or the opening, waits for its textures **300 updates at most** (`ARRIVAL_HOLD`, twice what the slowest tier's asset server takes to upload its whole pool), and one let go there is counted, `arrival_held_to_bound` on the reasons line, and logged as a warning: a streamer fault, which nothing judges yet but which a run must read as one.

  The report reads every phase's `texture waits` line and fails a `p99_frames` over 30 (a `1024+` too), a `longest_open_frames` or `pool_longest_open_frames` of 30 or more, or a run whose log has no line. On the reasons line it fails `tails_over_pool` above 0 or an `arrived_at_update` of 0 (the walk never arrived); the reasons, the bytes raised and re-raised and the arrival's counts are shown and fail nothing. The `texture streaming cpu` line (the counting walk's streamer, p50 and p99 of its total, coverage, choosing and raising in µs) is shown only. To read them by hand:

  ```bash
  grep -h 'texture waits phase=\|texture wait reasons' target/bench-scale/<commit>/logs/counted-streaming.*.log target/bench-scale/<commit>/logs/streaming.*.log
  ```

  The report's line "**A surface newly in view at its level within 30 frames (exit test 2):**", a bound from the trace's `worst_texture_debt` column, stays beside it as a note.
- **Nothing on the frame's thread.** Counted: the `frame thread loads` row (budget 0) and the `zone walk` line's `frame_thread_loads=`, `_reads=`, `_decodes=` and `_builds=`.
- **The pool's floor, counted** (M5 Z2). The district has no exit, so a trip from it can't be played; the counting walk stands for it (a deviation the report prints): at its end, after every count row is logged, it closes the zone and unmounts its pack, as a trip leaves a zone, asks the mesh pool to give back what the zone held, as a trip's arrival does, and draws until the pool is at its floor or 120 frames have passed. It logs:

  ```
  zone closed unmounted="unmounted" frames= at_floor= capacity_before= floor_before= capacity= floor= in_use= meshes_freed=
  ```

  The verdict reads it on `counted-streaming`, every round: it fails on no line, a pack that didn't unmount, or `at_floor=false` (a capacity over its floor). On the `ci` district the pool came back in two frames, from 84 MiB to 6.75 MiB on Desktop-Low and from 46 MiB to 3.75 MiB on Mobile-High.
- **The trip, timed** (`trip`'s section): "at the lap's last frame the mesh pool is back at its floor", the ward's zone-trip, read beside the district's close and judged on nothing.

**Passes when**, on each tier (desktop pools 1303 MiB on both desktop tiers, phones 333 MiB):

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| `bytes over budget in use`, `counted-streaming` | 0 or not exercised | 0 or not exercised | 0 or not exercised | 0 or not exercised |
| `streamed texture bytes` worst ≤ the run's pool | ≤ 1303 MiB | ≤ 1303 MiB | ≤ 333 MiB | ≤ 333 MiB |
| Process "most", `streaming` | ≤ 10 GB | ≤ 6 GB | ≤ 2.8 GB | ≤ 1.6 GB |
| `texture waits`, each phase, each run (from the waits counter) | `p99_frames` ≤ 30, `longest_open_frames` and `pool_longest_open_frames` < 30 | the same | the same | the same |
| `texture wait reasons`, each run | `tails_over_pool=0`, `arrived_at_update` > 0 | the same | the same | the same |
| `bytes decoded waiting` worst | ≤ 256 MiB | ≤ 256 MiB | ≤ 64 MiB | ≤ 64 MiB |
| `frame thread loads` worst | 0 | 0 | 0 | 0 |
| `zone closed`, `counted-streaming`, every round | `unmounted`, `at_floor=true` | the same | the same | the same |
| `trip` | back at its floor (read beside) | the same | the same | the same |

A `p99_frames` of `1024+` fails; `-` (no wait completed) passes. §3.1's figures are taken at 1,024 MiB a GB (10,240 / 6,144 / 2,867 / 1,638 MiB). On unified memory the process's footprint holds what the GPU allocates too, so it stands for RAM and VRAM together against §3.1's RAM figure; a discrete GPU's VRAM (6 GB and 3 GB on the desktops) is owed to one.

**On the Simulator and the emulator, by counts (M5 Z2):** `slot --device cargo xtask bench-scale device --platform ios-sim`, then `--platform android` ("The Simulator and the emulator", above). Each walks `device-streaming`, which is `counted-streaming`'s walk (a frame in seven drawn, the crowd on, `--texture-streaming on`) on the **`ci` district**: the budget's is gigabytes, too much to bundle and install on a device each run. Its phone tree with the shipped mobile cook holds 28,932,504 bytes (27.6 MiB) of textures whole on both phones (`texture pool bytes` in [tests/counts/district-ci.txt](../tests/counts/district-ci.txt)), and its views ask for 28,903,381 bytes at worst. Its pool is **28 MiB** (`--texture-pool 28`, 29,360,128 bytes), the orchestrator's ruling (M5 Z2): the `ci` district puts everything it places in view, so no pool under the views' worst ask can pass, and half its whole (13 MiB, by the batch's rule) only proves that. At 28 MiB the pool doesn't bind: the device rows prove the streamer's waits, uploads and counters on each device's backend, and the pool's pressure is proved on this Mac's `budget` walks; `budget` on a device is owed to the maintainer. The walk is paced as `counted-streaming` is. The command reads each walk's log by this test's counted rules above, and exit test 7's pipelines:

- `bytes over budget in use` worst 0 or not exercised; `streamed texture bytes` worst at or under 28 MiB; `bytes decoded waiting` at or under 64 MiB, the phones' admission cap; `frame thread loads` 0;
- every phase's `texture waits` line: `p99_frames` at or under 30, `longest_open_frames` and `pool_longest_open_frames` under 30; the reasons line's `tails_over_pool` 0 and `arrived_at_update` above 0; the `zone closed` line unmounted and at its floor;
- exit test 7's line, the walk's `zone walk` pipelines: `first_use=0 rebuilt=0 frame_thread=0 frame_waits=0`.

It prints a line for each exit test and tier, **pass**, **fail** or **not run**, and `report` shows them as lines of their own beside this Mac's ("Mobile-High, the iOS Simulator"). **Counts only:** no time read on either device is judged, and none is a phone's; the process's memory is the Mac's row alone. The Simulator draws on the Mac's GPU through Apple's simulated Metal, without indirect draws, which the engine never uses; the emulator draws through gfxstream on the host's Vulkan. On Android a scoped allocation count is process-wide, which no row here reads.

**What the `ci` district's walk shows at half its textures (first run, 2026-11-15):** the district is small enough that the tour keeps every texture it places in view (`zone walk` `placed=244 removed=0`), so a 13 MiB pool is full from the first drawn frame and stays full, and about 27 waits are still open when the walk ends (`texture waits` `pool_open=27 longest_open_frames=7660` on Mobile-High). Both devices read the same: every counted row and exit test 7 pass, and exit test 2 fails on its waits alone, on both tiers. With the pool at 28 MiB, which holds the district whole, the Simulator's Mobile-High walk passes (`p99_frames=12`, nothing open). So on this district the fail is the pool's, not the device's: twice the pool proves the streamer's eviction only where a walk leaves textures behind, which the `budget` district does and `ci` does not. Which pool the device rows are judged at is the maintainer's call.

**Can't prove here:** a phone's own memory limit and the OS killing the app at it, a discrete GPU's VRAM, and whether a level arriving late reads as a pop to a person. Owed to a maintainer with a phone and a desktop GPU, and to a person watching.

## Exit test 3: the look survives scale

The test (§1): "Sorting, instancing, culling, levels of detail, streaming and the phone's upscaler leave the picture alone: every existing golden stays byte-identical; each optimization has a test that draws the same image with it on and off, or within a stated gate where it can't be exact; at each switch of a level of detail, outline widths in output pixels and the shading's two-tone edge stay inside the testkit's gates."

**What proves it:**

- **Every golden, on three backends.** `cargo xtask golden` on Metal here (463 scenes; a warm run takes minutes), every scene `ok`. CI holds lavapipe and WARP: `Golden images (vulkan-lavapipe n/5)` and `Golden images (dx12-warp n/8)` green on the head, and `Golden images (metal 1/1)`. Each scene also fails on a first use in its captured frame. **Which references moved:** `git diff --name-only --diff-filter=M v0.1.0..HEAD -- 'tests/golden/*.png'` lists every existing reference M5 changed (140 scenes and 390 files at ce7707fd9, most of them the look packages' re-baselines). Each must be named by one of three, and none by any other scale package (D1 to D10, M5, L15, L18, or L14 with the upscaler off):
  - a look package's row in [m5-plan.md](m5-plan.md), whose re-baseline it is (L17's cumulus moved 137 Metal scenes and 124 on lavapipe and WARP, L9's effects pack `arena-boss`);
  - D4's row, for the eight `game-ward-*` Metal scenes it captured again: the game draws with levels of detail, so the town's far roofs and mountains moved, inside the scenes' gates (worst tile 0.047). This is the test's own "within a stated gate where it can't be exact" and a stated deviation from "every existing golden stays byte-identical": the exact test stays `render::levels::levels_off_draw_what_a_mesh_without_levels_draws`, and a person's eye on the eight is owed;
  - the material-level fix, "Re-capture the two Metal goldens the material level moves" (`foliage-ground`, and `game-hud-party-three-en`, which is new in M5 and so not in the list): a renderer fix, not a scale package.
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

  The `zone-district-*`, `lod-*`, `lod-crowd-*`, `debug-levels*`, `debug-runs-district`, `debug-culling-district`, `debug-textures-district` and `upscale-*` goldens hold the same work in a picture.
- **What each optimization costs and saves, timed** (the orchestrator's sittings): the batch's pairs, each package's on against its off, read beside each other in each tier's section and judged on nothing: `lods`, `character-lods`, `character-lods-crowd`, `draw-runs`, `shadow-cache`, `capsule-bins`, `capsule-bins-hub`, `upscaler`, and D6's `cell-levels` (M5 Z2): the district's cells cooked with their levels against cooked with one. Its `off` variant mounts the content `prepare` puts beside each cook's, `content/<preset>/<cook>-no-cell-levels/`, cooked from `gen-bench --cell-levels off`, whose tree differs from the other only in the zone's sidecar; read `main triangles`, `shadow triangles`, `mesh pool capacity` and `mesh pool floor` beside `gpu main` and `gpu shadows`.
- **At each switch.** `look_tiers::lines_and_two_tone_edges_hold_at_every_tier_and_scale` holds the lines and the two-tone edge at every tier and render scale; `cargo test -q -p sapphire_render --lib view_cull::tests::a_camera_held_at_a_switch_never_flaps` holds that a level doesn't flap. **Across a level switch:** `cargo test -p sapphire_render --test gpu render::level_switches:: -- --ignored --test-threads=1` walks every step of the chains the shipped characters (D5), static meshes (D4) and cells (D6) cook with, on every tier, through each switch's hysteresis, and holds the lines and the two-tone edge on both sides of the switch to level 0 and to each other with the look test's gates (`a_characters_line_and_two_tone_edge_hold_at_each_switch`, `a_static_meshs_two_tone_response_holds_at_each_switch`, `a_cells_two_tone_response_holds_at_each_switch`). D5's one-off measure (`--lods 0` against `--lods 1`: line width within 1.9%, strength within 1.2%, the two-tone edge within 0.1%) stands beside it.

**Passes when**, on every tier: every golden `ok` on Metal and green in CI's lavapipe and WARP jobs; every moved reference named as the rule above says; every test above passes; the switch clause as above. The tiers are covered within each test (`on_every_tier`) and by the `tier-*` scenes.

**Can't prove here:** a person's eye on motion, which no golden shows: a level popping, a culled character coming back. Owed to a person watching the district move.

## Exit test 4: the picture's missing parts

The test (§1): "Reflection probes with wet streets, effect meshes with distortion, ground decals, depth of field, the effects pack and cut-ins, dissolves and driven emission, characters shadowing each other, light shafts. Each has goldens on three backends, a stated behaviour on each tier, a budget row it's measured against, a document that authors it, and nothing built or dispatched while it's off."

**What proves it,** feature by feature. Goldens by `cargo xtask golden --scene <name>` here and CI's three golden jobs; each scene's references must exist for `metal`, `vulkan-lavapipe` and `dx12-warp` (`ls tests/golden/<scene>.*.png`). The tier behaviour is the package's row in [m5-plan.md](m5-plan.md) and its spec section's As built; the budget row is spec §3.3's; the document is [m5-handoff.md](m5-handoff.md)'s "How to add" (until the next handoff holds them).

| Feature (package) | Goldens | Budget row (§3.3) | Recipe in "How to add" |
|---|---|---|---|
| Reflection probes, wet streets (L2, L3, L4) | `reflect-*`, `wet-*` | Reflection probes (inside forward opaque and AO) | A reflection probe; A zone's weather and wet surfaces |
| Effect meshes, distortion (L5, L6) | `vfxmesh-*`, `distort-*` | Particles, distortion | An effect mesh; A bend |
| Ground decals (L7) | `decal-*` | Ground decals (proposal) | A decal |
| Depth of field (L8) | `dof-closeup`, `dof-closeup-upsampled` | Depth of field (not in the total) | A focus key |
| The effects pack, cut-ins (L9, L10) | `effects-*`, `cutin-*` | Bloom + tonemap + LUT, screen effects | A cut-in |
| Dissolves, driven emission (L11) | `dissolve-*` | Forward opaque | A dissolve / flash / glass |
| Characters shadowing each other (L12, L16) | `shadows-party`, `shadows-spot*`, `hair-shadow` | Shadows | none by name (Light shafts' and the party's recipes touch it) |
| Light shafts (L13) | `shafts-golden-hour` | Bloom + tonemap + LUT (the fused pass) | Light shafts |

"Nothing built or dispatched while it's off" is held by `ambient::nothing_is_built_or_run_for_work_nothing_reads` and `scene_prepass::mobile_low_builds_none_of_a_pass_it_never_runs` (`cargo test -p sapphire_render --test gpu --locked -q -- --ignored --test-threads=1 <filter>`), and by each package's own off test named in its row.

**Each part's figure** (M5 Z2). The sandbox times §3.3's passes, not features, so a part's cost is the difference its switch makes in the pass that holds it, where a switch exists, and its package's own measurement where not. The batch carries two such pairs today (the wet streets, and the capsules of characters shadowing each other); the others are the commands below, run as a pair on the stage named (a run each way, `--bench 3800 --tier <tier>`, timed, the orchestrator's) or cited from the package's row:

| Part | §3.3 row | The timed row that holds it | The switch | Where the figure comes from |
|---|---|---|---|---|
| Reflection probes, wet streets | Reflection probes (inside forward opaque and AO, not in the total); Forward opaque | `gpu main`, `gpu clustering and ao` | `--no-reflection-probes`; `--weather dry` against `--weather 0.8,0.4` | The wet streets: `night-weather`'s `dry` and `wet`, in the batch. The probes: L3's measurement (0.06 ms in forward opaque, 0.12 in the ambient pass on a frame that composes one), since the batch has no probes-off pair; `--no-reflection-probes` on the zone stage makes one |
| Effect meshes, distortion | Particles, distortion | `gpu effects` (the effects' pass and their bends) | `--distortion on\|off` with `--cues` or with `--vfx distort-full`; `--vfx mesh-eight` against none | L5's and L6's measurement (bends 0.10 to 0.21 ms at 1080p): the batch's fights run without `--cues`, so nothing in it fires effects; `district-fight` with `--cues`, distortion on and off, would make the pair |
| Ground decals | Ground decals (proposal, inside forward opaque) | `gpu main` | `--decals off` with `--cues`; `--vfx decal-full` against none | L7's measurement: no run in the batch lays decals, and the row is a proposal, so it would be shown and fail nothing |
| Depth of field | Depth of field (desktop, close-ups and cutscenes only, not in the total) | none of its own: the post chain's passes | `--focus <distance>,<sharp>,<near>,<far>,<blur>` against none, on a desktop tier | L8's measurement (1.2 ms): no walk or fight in the batch holds a focus, since only a close-up or a cutscene draws it |
| The effects pack, cut-ins | Bloom + tonemap + LUT, screen effects | `gpu post` | `--post-effect <preset>` against none; `--cutins off` with `--cues` | L9's and L10's measurements: the batch's runs push no screen effect and fire no cue, so `gpu post` holds the chain without them |
| Dissolves, driven emission | Forward opaque | `gpu main` | `--dissolve 0.5` against none | L11's measurement: a material's drive changes its shading, not a pass, and no run in the batch drives one |
| Characters shadowing each other | Shadows; Forward opaque | `gpu shadows`, `gpu main` | `--character-shadows own\|others`; `--capsule-bins on\|off` | `capsule-bins` and `capsule-bins-hub`, in the batch (the capsules' share); the shadow maps' share is L12's and L16's measurement |
| Light shafts | Bloom + tonemap + LUT (the fused pass) | `gpu post` | `--shafts on\|off` | L13's measurement: the district's day draws no shafts key, so a pair there would measure nothing |

Where the table cites a package's measurement, that measurement is the part's figure against its row (the spec's §3.3 notes and the package's As built row hold it); a pair in the batch makes it the batch's.

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
| `gpu skinning`, p95 | *proposed* 0.4 ms, fails nothing | *proposed* 0.5 ms, fails nothing | *proposed* 0.6 ms, fails nothing | *proposed* 1.0 ms, fails nothing |
| `gpu main` (outlines within it), p95 | not judged | ≤ 4.9 ms | not judged | ≤ 11.5 ms |
| `gpu prepasses` (the character prepass within it), p95 | not judged | ≤ 1.5 ms | not judged | ≤ 1.5 ms |
| the cast, the conversation and cloth tests | pass | pass | pass | pass |

`gpu skinning` logs §3.3's skinning row as proposed on all four tiers, since the spec proposes the whole row (a ruling, M5 Z2): the report shows it beside the row and fails nothing on it, until a figure is adopted into the row's budget. Outlines (0.6 / 1.5 ms) and the character prepass have no timed row of their own: they are read inside `gpu main` and `gpu prepasses`, which Metal's overlap makes inexact anyway.

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

- **The district's walk, counted** (`cargo xtask bench-scale run --counts`): `counted`, `counted-streaming` and `counted-night` (both variants) on every tier, and `counted-fight`, whose `fight pipelines` line reads the same counts. Read the `zone walk` line of each log, `… prewarmed= first_use= rebuilt= warmed_off_frame= frame_thread= frame_waits=`, and the report's "Pipelines built after the load (exit test 7, by counts):". The report fails on a pipeline built after the load, and its verdict fails exit test 7 on any of `first_use`, `rebuilt`, `frame_thread` or `frame_waits` above 0. That `zero_hitches` doesn't play the district is a ruling, printed as a deviation: these walks and B5's fight test stand in.
- **The district's fight, cold and warm** (Metal here): `cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1 the_district_encounter_plays_with_no_hitch_cold_and_warm`, Desktop-High and Mobile-Low, cold then warm, crowd on and off; each `fight pipelines` row must read `first_use=0 frame_thread=0 frame_waits=0 rebuilt=0`. It needs the `ci` district ("Before anything").
- **The timed runs' pipelines** (the orchestrator's sittings): `exit-walk`, `district-fight` and `party-kinds` read Pipelines too.
- **The ward, as in M4.** `zero_hitches` plays the Lantern Ward, not the district; the counting walks and the fight's rows stand in on the district. CI's `App tests (vulkan-lavapipe, walk-desktop-low | walk-mobile-low | walk-mobile-high)` run it by counts. Here, on each of the four tiers (`SAPPHIRE_HITCH_TIER=desktop-high`, the default, then `desktop-low`, `mobile-high`, `mobile-low`):
  - **Counted, any slot:** `cargo test -p game_app --features dev --locked --test zero_hitches -- --ignored`, with `SAPPHIRE_HITCH_TIER` set. Whether Metal's cache is warm doesn't matter to the counts.
  - **Timed, cold then warm, in the quiet window** (`slot --all --load 8`): build the lane for release (`cargo test -p game_app --features dev --release --locked --test zero_hitches --no-run`, which prints `target/release/deps/zero_hitches-<hash>`), put the binary in a throwaway `.app` whose `Info.plist` names a bundle ID no run has used (`CFBundleIdentifier`, with `CFBundleExecutable zero_hitches`, `CFBundlePackageType APPL`, `LSUIElement` and `LSBackgroundOnly` true), and exec `<app>/Contents/MacOS/zero_hitches --ignored` directly, never `open`ed, with `SAPPHIRE_HITCH_TIMED=1 SAPPHIRE_HITCH_SIZE=1280x720`. **Cold** is its first run: Metal's cache is per bundle ID, so `$(getconf DARWIN_USER_CACHE_DIR)<bundle ID>` must be absent before it and hold `com.apple.metal` after. A copied bare binary shares the content-keyed cache and isn't cold. **Warm** is the same `.app` run again. Delete the `.app` and that cache folder after. `SAPPHIRE_HITCH_BOOT_ONLY=1` stops after the boot, for many cold boots in a few minutes.
  - **What each prints:** exit 0. The lane prints `zero_hitches: warm-up threads=9 (default) class=<class> (inherited); the frame's thread: class=<class> core=<kind>` on this Mac, the boot's `booted in … ms` line (cold about 5 s, warm a few hundred ms: a cold run that boots in under 1.5 s wasn't cold), first use 0, on the frame's thread 0 and waited for 0, and each load screen's worst frame under its limit. Timed, it also traces the boot screen's frames (M5 X3): `zero_hitches: boot trace: … over_limit=0`, and a `zero_hitches: boot frame` line for each frame over half the interval and the worst. A frame over the limit fails the run, and its line says where its time went: its `advance`, layout and `submit` as wall/CPU, `own_wait` for its own GPU work, `building` for the warm-up's compiles in flight, and the cores' busy share. Engine-spec §3.7's "The cold boot's long frames, traced" has what 72 cold boots showed.
- **A texture level.** Under the hard pool, `counted-streaming`'s `frame_waits` counts frames that waited on a pipeline; a frame that drew a lower texture level while a higher one loaded is a wait in exit test 2's line, not a stall.

**Passes when,** on every tier: every `zone walk` line on `counted`, `counted-streaming` and `counted-night`, and `counted-fight`'s `fight pipelines` line, reads `first_use=0 rebuilt=0 frame_thread=0 frame_waits=0`; the report lists no pipeline built after a load; `the_district_encounter_plays_with_no_hitch_cold_and_warm` passes (it runs Desktop-High and Mobile-Low; the counted walks cover all four); `zero_hitches` passes here and in CI's three lanes.

**Can't prove here:** a phone driver's cold compile, as in M4; a desktop GPU's driver cache on Windows and Linux. Owed to a maintainer with the devices.

## Exit test 8: still authorable

The test (§1): "Each new thing has a "How to add" recipe with no Rust step: a level-of-detail setting, a reflection probe, a zone's weather, a decal, an effect mesh, a focus key, a cloth, a face track, an ambient character and a playable kind."

**What proves it:** on a scratch branch, dropped after, walk each recipe in [m5-handoff.md](m5-handoff.md)'s "How to add" (or the next handoff's, once it holds M5's) as written: A playable kind; A cloth; A reflection probe; A zone's weather and wet surfaces; Ambient characters; Which characters draw coarser and a close-up (a level-of-detail setting; the world's cells take theirs in the zone recipe's sidecar, `importer: Zone((lods: …))`); A face track; A decal; An effect mesh; A focus key. After each, cook (`cargo xtask content`) and run the check the recipe names, then diff the Rust against the commit the scratch branch was cut from, never `main`: during a milestone `main` can be many commits behind the line being run (110 at the rehearsal, where `git diff --stat main` listed 192 Rust files on a tree no recipe had touched).

```bash
git diff --stat <the commit the scratch branch was cut from> -- '*.rs'    # empty: no Rust
```

**Passes when:** all ten walked, each cooks and its check passes, and the diff above is empty for each. A recipe whose steps write Rust fails until a document carries that step.

**Can't prove here:** nothing §1 names.

## Rehearsal at ce7707fd9

2026-11-16, at ce7707fd9 (the line being landed, ahead of `main`), in agent worktrees; the machine an Apple M5 with 24 GB, macOS 27.0.1, Xcode 26.2. **Every reading below is a count or a test's pass, none a time,** and none was taken on a quiet machine. `prepare` took 832 s on a warm `target/bench/cache` (four `budget` cooks and the release build), in a plain gate slot at a load of 20 to 30; `run --counts` took 1,141 s for its 40 units (two rounds each), and the named tests ran beside it, at loads from 9 to 168 (the 1-minute average) while seven other agents built; the recipes ran later the same night, one plain gate slot at a time, at loads of 3 to 12. **No timed sitting was run:** every timed row reads "timed sitting: not run in this pass", and its sittings wait for the orchestrator's quiet window, from a worktree left at ce7707fd9. `plan` leaves 54 sittings, 431 minutes by the units' stated lengths. The goldens, the GPU suites and the replays are the line's gate at ce7707fd9, which passed whole: 463 Metal goldens, 452 renderer GPU tests, the GPU library suites (112, 9 and 102), 21 viewport tests, the replays, `cargo xtask counts` and 6,442 tests. `report` exits 1 on the fails below. On disk: `target/bench-scale/ce7707fd9/` 3.0 GB (its content hard-linked from `target/bench/`), `target/bench/` 18 GB (the `budget` cooks 12 GB, the cache 5.5 GB, `ci` 0.3 GB).

| Exit test | Tier | What was read | Measured | Budget | Verdict | Evidence |
|---|---|---|---|---|---|---|
| 1, `main draws` | Desktop-High | `counted`, `counted-night`, `counted-fight`, worst | 2,622 (`counted` and both `counted-night`, at the tour's 1.5 m); 1,665 (`counted-fight`); the same each round | 2,500 | **fail** | `logs/counted.desktop-high.r1.log` and beside it; the report's Verdicts |
| 1, `main draws` | Desktop-Low | the same | 2,626; 1,665 | 2,500 | **fail** | `logs/counted.desktop-low.r1.log` and beside it |
| 1, `main draws` | Mobile-High | the same | 588; 391 | 700 | pass (counts) | `logs/counted.mobile-high.r1.log` and beside it |
| 1, `main draws` | Mobile-Low | the same | 583; 482 | 700 | pass (counts) | `logs/counted.mobile-low.r1.log` and beside it |
| 1, `lights` | each tier | the same, worst | 64 / 64 / 8 / 8 on every walk, exactly the cap (`lights dropped` up to 320 on Desktop-High: the cap holds the row) | 64 / 64 / 8 / 8 | pass | the counted logs |
| 1, `cpu frame`, `gpu frame` | each tier, each timed run | the timed runs | timed sitting: not run in this pass | §3.1 | not judged | `report.md` |
| 1, §3.2 / §3.3 rows | Desktop-Low, Mobile-Low | the timed runs | timed sitting: not run in this pass | §3.2, §3.3 | not judged | `report.md` |
| 2, pool and process | each tier | `counted-streaming`'s `bytes over budget in use` and `streamed texture bytes`; `streaming`'s Process | over budget in use, worst: 3,428,652 (r1) and 2,179,076 (r2) bytes on Desktop-High, 23,612,804 and 287,460 on Desktop-Low, not exercised on the phones; streamed bytes, worst: 1,342,148,074 / 1,342,148,074 / 315,963,541 / 255,146,133, under each run's pool; the process: timed sitting: not run in this pass | 0; 1280 / 1280 / 331 / 331 MiB; §3.1 | **fail** on the desktops (bytes over budget in use); streamed bytes pass | `logs/counted-streaming.<tier>.r<n>.log` |
| 2, `texture waits` | each tier, each phase | `counted-streaming`'s `drawn` line; `streaming`'s phases | `p99_frames=32+` on every tier and round (p50 24, 9, 2 and 2 frames; 10,710, 10,327, 8,081 and 4,187 waits completed in r1); `longest_open_frames` 0, 0, 0 and 2; `streaming`: timed sitting: not run in this pass | 30 frames | **fail**, every tier | the same |
| 2, decoded waiting; the pool's floor | each tier | `counted-streaming`'s `bytes decoded waiting`, `frame thread loads` and `zone closed` | decoded waiting not exercised on the desktops, 1,048,864 and 2,097,296 bytes worst on the phones; `frame thread loads` not exercised, `frame_thread_loads=0`; `zone closed` `unmounted`, `at_floor=true` every round, 163.6 MB to 7.1 MB on the desktops and 98.6 MB to 3.9 MB on the phones in 2 frames | 256 / 64 MiB; 0; the floor | pass | the same |
| 2, the iOS Simulator, counts only | Mobile-High, Mobile-Low | `device-streaming` on the `ci` district | `bench-scale device`: not run in this pass (its first walks, 2026-11-15, are under exit test 2) | 13 MiB pool; 64 MiB; 30 frames; pipelines 0 | not run | `logs/device-streaming.mobile-high.ios-sim.r1.log` and beside it |
| 2, the Android emulator, counts only | Mobile-High, Mobile-Low | `device-streaming` on the `ci` district | `bench-scale device`: not run in this pass (its first walks, 2026-11-15, are under exit test 2) | 13 MiB pool; 64 MiB; 30 frames; pipelines 0 | not run | `logs/device-streaming.mobile-high.android.r1.log` and beside it |
| 3 | all | goldens, on/off tests, switch; the moved references | 140 existing scenes' references moved since v0.1.0, 390 files (140 Metal, 125 lavapipe, 125 WARP): L17's cumulus 137 Metal scenes (64ed04e3a) and 124 on each of lavapipe and WARP (71cfab7f1); L9's effects pack `arena-boss` on all three; the eight `game-ward-*` Metal scenes D4 captured again (bf4067481); `foliage-ground` by the material-level fix (28736718a); and `game-character-tablet-en` and `-ja` on Metal, one pixel one code value apart (e49c7fddd), which no row names. Goldens, the GPU suite and `level_switches`: the line's gate at ce7707fd9: pass | byte-identical, or named as the rule says | pass but for the two character screens, which need a ruling; D4's eight are a stated deviation inside their gates (worst tile 0.047), owed to a person's eye | `git diff --name-only --diff-filter=M v0.1.0..ce7707fd9 -- 'tests/golden/*.png'`; the line's gate |
| 4 | all | eight features' checklist | every feature's goldens have references on all three backends (reflect and wet 11 scenes, vfxmesh and distort 8, decal 2, dof 2, effects and cutin 14, dissolve 2, shadows 4, shafts 1); goldens and off tests: the line's gate at ce7707fd9: pass; each part's figure as its row cites it; recipes: exit test 8 | | pass (goldens, off tests, recipes); the timed pairs not run | `ls tests/golden/<scene>.*.png`; the line's gate |
| 5 | each tier | the cast's rows; tests | `every_budget_figure_cooks_at_its_row`, the conversation's two tests, the cloth's soak and `the_cloth_switching_at_its_distance_never_pops` pass (one test each); the rows: timed sitting: not run in this pass | §3.2, §3.3 | the tests pass; the rows not judged | the named tests' `q` logs |
| 6 | each tier | spawned; `animation and ik`; replays | spawned 60 / 60 / 30 / 30 on every counted walk; the four replays tests pass; `cargo xtask replay`: the line's gate at ce7707fd9: pass; `animation and ik`: timed sitting: not run in this pass | 60 / 60 / 30 / 30 | spawned and the replays pass; the row not judged | `logs/counted.<tier>.r1.log`; the replays test's log |
| 7 | each tier | `zone walk` counts; district fight; `zero_hitches` | every `zone walk` and `fight pipelines` line reads `first_use=0 rebuilt=0 frame_thread=0 frame_waits=0`, both rounds, all four tiers; the report lists no pipeline built after a load; `the_district_encounter_plays_with_no_hitch_cold_and_warm` passes; `zero_hitches` passes on Desktop-High after a fresh build (booted in 4,305 ms, 111 pipelines prewarmed off the frame) and again warm (320 ms) | 0 | pass (counts and tests); CI's three `zero_hitches` lanes are CI's | the counted logs; the `zero_hitches` logs |
| 8 | all | ten recipes, each cooked (`cargo xtask content`), its check run, and the Rust diffed against the commit its scratch branch was cut from | all ten walked with no Rust step, each diff empty: a playable kind (the cook refused the rig and named the right one, as step 5 says; `party-kestrel` recorded; the figure and strings tests; its look-dev capture, which the first pass's script missed for want of the content folder, taken again on the second scratch branch); a cloth (Wren's bangs as a second, open sheet, in a motion of its own named from the kind; the look-dev capture running); a reflection probe (a lamp's open place in the proving ground: baked, `zone_baked`, the capture differing with `--no-reflection-probes`); the zone's weather (`zone_baked`; wet against `--weather dry`); ambient characters (a post and a walk of two: `zone_ambient`, `zone_baked`, on against `--ambient off`); the cells' levels (the zone's sidecar asks for two: `zone_baked` holds the simulation's copies; `--debug-view levels`); a face track and a focus key (the court gate's timeline at versions 2 and 3: both cook, and the focus key blurs the court 330 frames into `--arena --timeline`); an effect mesh and a decal (layers on `heavy_spark` and `heavy_land`: `sapphire_vfx`'s 91 tests). `cargo xtask replay` after the ten: pass, nothing moved | no Rust | pass; two look steps need words (found, 13 and 14) | the scratch branches `m5-z2-recipes-scratch` and `m5-z2-recipes-scratch-2`, never pushed |

### Found in rehearsal

One line a finding, most serious first, with what closes it.

1. **Exit test 1 fails on the desktops' draws:** `main draws` worst 2,622 on Desktop-High and 2,626 on Desktop-Low, at the tour's first metres, against 2,500. The district is the project's own and is meant to sit at the row; its desktop density is being trimmed. Closed by that change and a counted run at the new top.
2. **Exit test 2 fails on every tier:** `texture waits` p99 32+ frames on all four, and bytes over budget in use on the desktops; the devices' walks read the same. Under diagnosis: the walks run unpaced, so a wait's frames may not be the tier's. Closed by that fix and a run at the new top.
3. **`counted-streaming`'s streaming rows aren't exact between rounds** (nor, on the phones, a few upload and mesh-pool totals of the other counted walks): "Which counts are exact", above, says which rows hold. Whether the report should fail on one that moves is decided with exit test 2's fix.
4. **Exit test 3's moved references:** two sets come from outside the look packages, D4's eight `game-ward-*` scenes and the material-level fix's; ruled, and the rule under exit test 3 names both, with D4's row naming the eight. Closed by those words. The count is 140 scenes and 390 files at ce7707fd9, not 139.
5. **Two more moved references no row names:** `game-character-tablet-en` and `-ja` on Metal, one pixel a code value apart, re-captured by "Re-baseline the character screen's 4:3 Metal goldens by one pixel" (e49c7fddd), which landed with D1's stack. Needs a ruling: named by D1's row, or held as the one-pixel noise every golden run allows.
6. **`sapphire_replay record` panics on a bad playable document** (`crates/sapphire_gameplay/src/replay/document.rs`, `read_all`) where it must return an error. Closed by `m5-fix-replay-record-bad-document`: the kinds are read as a scenario sets up, and a bad one is that set-up's error, naming the member, the document and what is wrong.
7. **The batch is keyed by `HEAD` and reads its manifest from the working tree,** so a docs commit in the batch's worktree left it unprepared. Closed by "Run the batch from a worktree left at the prepared commit", above.
8. **`prepare` cooks `budget` four times in a plain slot,** where a `budget` cook needs the machine. Closed by running it with nothing else running, above.
9. **The runbook's sizes were wrong:** `plan` counts 54 sittings, about seven hours of a quiet machine, not an afternoon. Corrected in "The shortest path".
10. **Exit test 8's `git diff --stat main` was the wrong base:** `main` was 110 commits behind, so it listed 192 Rust files on an untouched tree. Corrected: the diff is against the commit the scratch branch was cut from.
11. **Scene names that don't exist:** `shafts-golden`, `dof`, `debug-runs`, `debug-culling` and `debug-textures`. Corrected to `shafts-golden-hour`, `dof-closeup-upsampled` and the `*-district` views.
12. **`zero_hitches` "cold and warm" names no tier and no way to make a cold run** (exit test 7). Another branch rewrites that step.
13. **A recipe's look step run by `cargo run -p sapphire_sandbox` reads the content beside the binary,** `target/debug/content/`, which nothing writes: "A reflection probe", step 3, fails as written ("failed to read pack shared") until `SAPPHIRE_CONTENT_DIR=<checkout>/target/content/desktop/` is set, as the walk set it. Closed by those words in the recipes (the next handoff's "How to add").
14. **A timeline's look step, `--arena --timeline <id>`, plays it in the arena's space and by drawn frames:** the court gate's shots, framed on the ward's places, show no fighter in the arena, so a face track's faces can't be seen there, and a capture by `--capture-ticks 330` alone drew the street with none of the timeline; `--capture-frames 330` drew the focus key's blur. Closed by a line in "A face track" and "A focus key" saying how to look at a zone's timeline, and to capture it by frames.

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
- **Exit test 2:** a phone's own memory limit and its kill, a discrete GPU's VRAM, and whether a level arriving late reads as a pop to a person; and the budget district's walk on a device, since the Simulator and the emulator walk the `ci` district's (`bench-scale device`).
- **Exit test 3:** a person's eye on motion, and on the eight `game-ward-*` scenes D4 captured again with levels of detail on.
- **Exit test 4:** an artist's eye, and Zenless Zone Zero references.
- **Exit test 5:** a second authored character, authored clips, recorded voice, and a person's verdict on the cloth and the faces.
- **Exit test 7:** a phone driver's cold compile, and a desktop GPU's driver cache on Windows and Linux.
- **All of it:** nobody has played anything, and no test here asks whether it's fun.
