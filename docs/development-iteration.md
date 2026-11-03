# Development iteration

See [Optimization pass two](optimization-pass-two.md) for the subsequent watcher, packing, test,
capture, runtime and CI fanout changes and their separate measurements.

Keep each checkout's Cargo target directory separate. Copying a source tree while reusing another
checkout's target directory can retain path-dependent artifacts; never point two worktrees at the
same `CARGO_TARGET_DIR`.

## Choose the smallest useful check

- While editing one crate: `cargo test -p <crate> --lib --locked`.
- Before handing off a change: `cargo test-fast --locked` runs workspace unit and integration tests.
- Full CPU coverage: `cargo test --workspace --locked --no-fail-fast`, including doctests. CI keeps this gate.
- Moves, their documents and the simulation's determinism and allocation tests: `cargo test -p sapphire_moves -p sapphire_gameplay --locked`. They need no GPU. Its cost: `cargo run --release -p sapphire_gameplay --example step_bench`.
- The recorded combat replays: `cargo xtask replay [--replay <name>]`. They need no GPU, and the suite itself takes under 0.2 s. See [Replays](#replays).
- The same replays beside the renderer: `cargo xtask content`, then `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1`.
- The combat views and the overlay's recorder: `cargo test -p sapphire_sandbox --lib combat`, and with `--features dev -- --include-ignored` for what the panel is shown. The panel itself, clicks included: `cargo test -p sapphire_debug --lib combat::`.
- Live tuning: `cargo test -p sapphire_gameplay --lib replay::tuning`, and through a real sandbox `cargo xtask content`, then `cargo test -p sapphire_sandbox --features dev --test live_tuning -- --ignored --test-threads=1`. See [Tuning a fight](#tuning-a-fight).
- The timeline editor, with no window: `cargo test -p sapphire_editor --locked`, against a cook `cargo test -p sapphire_editor --test preview -- --ignored`, and through a real fight `cargo build -p sapphire_sandbox --features dev`, then `cargo test -p sapphire_editor --test sandbox_push -- --ignored`. See [The timeline editor](#the-timeline-editor).
- Combat's controls, driven by synthetic input: `cargo test -p sapphire_input -p sapphire_controls --locked`. Their cost: `cargo run --release -p sapphire_controls --example update_bench`.
- Renderer GPU fixtures: `cargo test -p sapphire_render --test gpu --locked -- --ignored --test-threads=1`.
- One renderer fixture family: `cargo test -p sapphire_render --test gpu ambient:: --locked -- --ignored --test-threads=1`.
- All runtime GPU tests: `cargo test -p sapphire_render -p sapphire_rhi -p sapphire_world -p sapphire_debug --tests --locked -- --ignored --test-threads=1`.

The renderer's integration tests live in `crates/sapphire_render/tests/gpu/`, with one module per
fixture family. One integration executable links the renderer once instead of once per family.
The GPU tests stay ignored by ordinary CPU runs and execute serially in the GPU gate.
For example, the former `--test ambient` selection is now `--test gpu ambient::`.

A deterministic channel handshake replaces one 200 ms delay in the live-link shutdown regression.
Timeout and rate-limit tests retain the timing behavior they are intended to check.

## Contact sheets

To read a clip's motion, render it as one image:

```
cargo xtask content
SAPPHIRE_CONTENT_DIR=<abs>/target/content/desktop/ target/debug/sapphire_sandbox --contact-sheet <clip> sheet.png
```

`<clip>` is a name `lookdev::ClipName` lists or any cooked clip's asset ID, so a new key-pose document's first sheet takes only its sidecar's ID and a cook, with no sandbox build. A sheet takes about half a second and opens no window. The top row is the front and the bottom row the character's left side, each cell labelled with the clip's tick, over a red ground line with a mark every half meter. The camera holds still, so root motion shows as travel: `--zoom 1.6` keeps a dodge in frame and `--clip-in-place` leaves it out. For a close look at a few ticks, `--sheet-cells 3 --size 400x500`. Eight 192-pixel cells across fit what an image reader shows whole.

## Replays

`tests/replays/` holds the combat regression suite (spec §9, gates 2 and 4): `<name>.sreplay` files of per-tick actions, and one `checksums.<architecture>.txt` per architecture (`aarch64`, `x86_64`) of what each must reach. CI's `Replays` job plays them on Linux, Windows and macOS with no GPU.

- **Run them:** `cargo xtask replay`, or one with `--replay <name>`. A failure names the first tick that differs, counted as steps run, and a step that allocated fails its replay too.
- **A change moved one on purpose:** `cargo xtask replay --update`, then read the diff, as with a golden. It rewrites this architecture's file alone and prints what moved from which tick. If move data changed, it also stamps the new hash into each replay; the actions stay.
- **See what there is:** `cargo xtask replay --list`.

### Adding a replay

1. **A starting world**, if none fits: write a setup function and add a row to `SCENARIOS` in `crates/sapphire_gameplay/src/replay/scenario.rs`. It loads move sets and then spawns, in a fixed order, and returns the actors input drives, the player first.
2. **Its actions:** write a function of the tick and add a row to `SCRIPTS` in `crates/sapphire_gameplay/src/replay/script.rs`. A session someone plays comes from the app's `--record` instead (below).
3. **Record it:** `cargo xtask replay --record <name> --scenario <scenario> --script <script> [--ticks <n>] [--seed <n>]`. This writes `tests/replays/<name>.sreplay`, and adds it to this architecture's checksums. It refuses a name that's taken: delete the file to replace it.
4. **Commit both files.** The PR's first CI run fails `Replays` on Linux and Windows, because nothing is recorded for x86-64 yet. Fill that in as below.

### Filling in x86-64's checksums

No machine here runs x86-64, so its values come from CI, whenever a replay is added or a move changes:

1. Find the failed run: `gh run list --branch <branch> --workflow CI`.
2. Download what the Linux job uploaded: `gh run download <run> -n replay-checksums-ubuntu-24.04 -D <dir>`. It holds the whole file as it should be.
3. Copy `<dir>/checksums.x86_64.txt` over `tests/replays/checksums.x86_64.txt`, and commit it.

The next run's Linux and Windows jobs compare against that one file, which is what proves they agree. If Windows then fails, it names the first tick where it differs from Linux: that's a determinism bug to fix, not a value to update.

### In the app

`--replay <file>` and `--record <file> --scenario <name> --script <name>` go with `--capture` or `--bench`: the simulation takes a tick with each fixed step, beside the renderer. A replay is held to the recorded checksums beside its file, and a capture of one that has left them isn't written. A capture stops at the replay's end unless `--capture-ticks` picks an earlier tick.

`--bench <frames>` with `--replay`, or `--fight --script`, measures a whole fight (`fight_bench.rs`, M2 Z1): three phases, as it first plays, warm for CPU times and allocations, and with GPU timing on, each playing whole fights until it has `<frames>` frames, so `--bench 1` plays one fight three times. Frames take the tier's own steps, two on Mobile-Low. It logs a `fight row` for each row of spec §3, with its 50th and 99th percentile, its worst, the tick that fell on and the tier's budget, and `--bench-trace <csv>` writes every frame. For the real load: `--arena --replay tests/replays/arena-fight-won.sreplay --cues --hud live --blade on --tier <tier> --size <its resolution>`, with `SAPPHIRE_CONTENT_DIR` on `target/content/mobile/` for the phone tiers. A bench of a `--record` stays a plain one: warm-up, its frames, then 240 timed.

`--record <file> --scenario <name>`, with no `--script`, `--capture` or `--bench`, records a fight someone plays: the windowed app's simulation takes each fixed step's input from combat's bindings (`sapphire_controls::ControlsSource`, through `combat::action_source` in the sandbox), turned by the camera's yaw, so the file carries world directions and no camera. A pause records nothing, since no steps run. The file is written when the app quits, and as far as it's got each time the app goes to the background, because a phone kills what's there with no word, so a killed process loses only what came after the last of those. Each write goes to `<name>.partial` and is renamed, so a kill in the middle of one leaves the last whole file standing, and it replaces only what the session itself put at that name. If its file can't be written, or something else has appeared at the name, the session goes to one `<name>-unsaved-<n>.sreplay` in the temp directory, and the log or the error says so; reaching its own name again removes that file. If that fails too, the error says which file still holds the last of it, if any does. On Android the temp directory is `/data/local/tmp` unless `TMPDIR` says otherwise, which an app can't write, so nothing there rescues a session. On a phone a path with no directory of its own goes under the app's cache: on Android, fetch it with `adb exec-out run-as dev.sapphire.sandbox cat cache/<name>.sreplay`.

`--arena --fight` is a fight with no file, player against the enemy's brain, on the graphs, cues, HUD and sword a game would have: in a window it's played on combat's bindings alone, by pad, keyboard or touch ([m2-exit-run.md](m2-exit-run.md) has a phone's run), and it starts over 4 s after someone falls, or on `R` or the d-pad's down. Nothing's written. With `--script <name>` and `--capture`, `--bench` or `--offscreen`, a script plays it down the same path, which is how to run one here, where no window may open. A script plays it in a window too, which is how a phone fights unattended for exit test 4: `cargo xtask run --platform ios|android --release -- --arena --fight --script fight-back --tier mobile-low`. Each fight logs a line when it ends, with its ticks, who fell, both healths, its checksum and its allocations; the checksum is the one a headless run of that script reaches at that tick.

The player's moves reach the simulation as a cooked copy, `crates/sapphire_gameplay/baked/player.smoves`. After changing `content/combat/player/` or `MoveSet`'s layout, rewrite it and the fight's replays:

```
SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy
cargo xtask replay --update
```

## Tuning a fight

Run a `dev` build with a fight in it, and save. `crates/sapphire_moves/test_content/slice.moves.ron`, a fight's other `.moves.ron`, `default_camera.ron`, its fighters' `.animgraph.ron` and, with `--cues`, `default_cues.ron` are watched already, and `--watch-document <path>` adds a copy elsewhere. [m2-exit-run.md](m2-exit-run.md), "What changes what", has every route. On a phone, `cargo xtask link push <file>`, or `cargo xtask link watch`, which watches what the app's status lists.

- A move in progress finishes on what it started with, and the next start reads the edit. The log says `the next attempt plays the new moves`, and `cargo xtask link status` shows each fighter's revision and windows.
- A fighter's `movement` needs no press: the next step moves by it, and the log says `the next step moves by the new numbers`. An edit moves nobody by itself, and a character in the air finishes that arc. `sapphire_moves::MOVEMENT_LIMITS` says what each number may be, and `--script laps` runs up and down while you try.
- A bad document changes nothing, and its issues name the move and the track. The next good one works.
- A move that names a clip takes new windows and phases with no cook. Its length is its clip's, and a change to the `clip` block needs `cargo xtask content`, then hot reload's remount or `cargo xtask link push`. A cook reads a document when it starts, so where the app has the file, one that started before your last save leaves that save playing, over the cook's tables.
- A graph takes edits where something plays it: `character.animgraph.ron` on the graph show's character or a fight's player, and `warrior.animgraph.ron` on a fight's enemy. A push of any other is refused by name, and a clip it names must already be cooked for that character. While a new version of that fighter is still landing, a push is refused for that reason: push again once it has.
- A recording ends its file at the first edit that changes something, and the fight goes on: a replay can only answer for what it recorded. A fight started afterwards starts from the edit, and its file says so through its hash.
- A fight with no file takes edits too, and starts over from them.
- To try it with no window: `--offscreen --record <file> --scenario duel --script attempts`, which presses LIGHT every half second, or on the real moves `--offscreen --arena --fight --script attempts`.
- The simulation's copy of the player's moves stays what it was until it's rewritten (above), so after tuning `player.moves.ron`, rewrite it and commit it with the document. A `dev` build's fight already starts from a newer cook.

## The timeline editor

`tools/sapphire_editor` opens a `.moves.ron`: a move's windows on a timeline of 60 Hz ticks, the real simulation at the playhead, and save and push to a running fight. It's for a person at the machine, because it opens a window. Agents never launch it and use `--capture` or the tests.

To use it, with a fight running in another terminal:

```
cargo xtask run --platform desktop --features dev -- --offscreen --record target/fight.sreplay --scenario duel --script attempts
cargo xtask editor crates/sapphire_moves/test_content/slice.moves.ron
```

That fight presses LIGHT every half second with no window, and a dev build listens on 127.0.0.1:7811 unless told otherwise. To play the fight yourself, drop `--offscreen` and `--script attempts`. `cargo xtask editor` cooks, builds in the `tools` profile and passes `--content`. Its address field and `--link <address:port>` name the device.

- **Editing:** drag a window's ends or body, a cue, or a phase's edge, tick by tick. `[` and `]` put a window's ends at the playhead, Alt+arrows nudge what's selected, Delete removes it, `+` on a row adds to it at the playhead. Cmd+Z and Cmd+Shift+Z (Ctrl on other systems) undo and redo a whole drag at a time. Rows a move doesn't use appear with **All tracks**. A name typed in the inspector goes in when its field lets go or with a save, as one step of undo, and shows as unsaved until then. Text the file couldn't read stays in its field and stops a save, and the top bar has **Show it** and **Drop it**.
- **Scrubbing:** click or drag the ruler, or use the arrows, `,`, `.`, Home and End. Space plays from the frame you're on and comes back to it at the end; a stop stays where it stopped. The preview simulates the move, with a dummy at the distance you set: its hit marks and hitstop are the fight's own, so a hit shows as ticks that repeat.
- **What's wrong:** the document's own check, as you drag, naming the move and the track, in the row and under the timeline. Nothing stops you going there. Saving is allowed, because the file is yours and a device refuses what it can't take.
- **Save and push:** Cmd+S writes the file and pushes it. The side panel shows what it replaced, or what the device refused, with your edit still there, and the attempts the fight is playing now. A save won't write over a file that changed on disk, stopped being text or can't be read: the top bar asks, or the notice says why. An answer that comes back after the text, the file or the device changed shows as an earlier save's, and marks nothing on the timeline.

A push goes to the file names a running build knows: a fight listing the file plays the edit on its next attempt (`--arena --fight` lists `player.moves.ron` and `warrior.moves.ron`, the older scenarios `slice.moves.ron` and `show.moves.ron`), a name the build cooks but no running fight plays is kept for the next fight and the reply says so, and any other name is refused. `player.moves.ron`'s numbers are tuned in the file: `tools/motion`'s `moves.py --write` leaves it alone, and `--write-numbers` won't replace an edited file unless forced, which the editor's banner says. An edit that reaches a cook owes the simulation's copy a rewrite (`tools/motion/README.md`), and `the_simulations_copy_of_the_set_is_what_a_cook_makes` fails saying how.

To see it with no window: `cargo run -p sapphire_editor -- <file> --capture <png> [--move <name>] [--tick <n>] [--size <w>x<h>] [--content target/content/desktop]`, with `SDKROOT` set on a Mac. It draws on the CPU, with no SDL and no GPU.

No test can show:

- that the window opens at all, on any OS: `window.rs` is the one part nothing runs, with its surface, device loss, resizing, HiDPI scale and text input;
- how a drag feels at speed, whether handles are easy to grab at a real pointer's precision, and whether tick snapping reads as steady;
- whether playback looks right at 60 Hz (its timing is tested on a synthetic clock, not seen);
- the real keyboard and mouse: each key's translation from SDL is tested, but on a real layout (a non-US one's `[`, say) it hasn't been pressed;
- the system's own Close and Quit, which should ask first with edits unsaved, and save instead while a lost GPU leaves the window black;
- the figure against the game's own look: it's flat clay for reading a pose, with no materials, outlines, springs or IK.

## Build profiles and content

`cargo xtask --locked content --cook desktop` builds the cooker with the `tools` profile. That profile
inherits release optimization, disables cross-crate LTO and enables incremental compilation for local
importer edits. CI explicitly retains `CARGO_INCREMENTAL=0`. It has its own `target/tools` output,
so cooker iteration does not invalidate release app builds. The existing `--release` app switch and
shipping profile retain their previous optimization and symbol policies.

For a direct cooker invocation, use `cargo run -p sapphire_pipeline --profile tools -- <arguments>`.
The first use compiles this profile's dependencies. A fresh profile is not a warm-cache speedup.
xtask reads the executable from Cargo's artifact messages, so custom target directories work too. In the measured five-edit experiment, incremental compilation
added about 558 MiB of unique logical file storage (hardlinks counted once); this is a local disk
tradeoff, not an increase in CI caches where incremental compilation is disabled.

**An app you're tuning:** `cargo xtask run --platform desktop --incremental --features dev` (or `--platform macos`) builds the app in this same profile. A one-line edit takes 1.4–5.5 s where `--release`, with thin LTO and no incremental compilation, takes 76–152 s, and a fight's CPU frame reads 0.82 ms against 0.54, with the same GPU frame and the same checksums (spec §7.2). It isn't for numbers: bench on `--release`. A plain debug build rebuilds as quickly at 1.44 ms and 1.6× the GPU frame, and adds debug checks such as R6's per-thread count.

Build the development sandbox and live-link CLI in one graph when both are needed:

```sh
cargo build -p sapphire_sandbox -p sapphire_livelink --features sapphire_sandbox/dev --locked
```

This unifies their requested dependency features before compilation. Separate commands can build
and link different renderer variants in succession.

## Golden comparisons

Exact RGB matches skip FLIP's perceptual calculation. The result still uses the same metric pooling
and colorized error-map path, and alpha remains ignored as before. Any changed RGB channel takes
the original FLIP path. Captures, scene isolation, thresholds, golden references and sharding are
unchanged. This optimization helps the comparison stage; it does not accelerate scene rendering.

## CI setup and cache identity

The local setup action reads the exact compiler pin from `rust-toolchain.toml`. Each job installs
only its host and required cross-compilation targets: host for lint/test/goldens, iOS device and
simulator for iOS, and Android arm64 for Android. Lint alone needs the manifest's lint components.
`RUSTUP_TOOLCHAIN` keeps subsequent Cargo calls from reinstalling every target in the manifest.

The pinned rust-cache action hashes every installed Rust toolchain. Different preinstalled compiler
versions on otherwise identical hosted images therefore produce different cache keys. The setup
helper removes unused toolchains only after installing and verifying the pinned compiler, and only
on disposable GitHub-hosted runners. It refuses local and self-hosted environments before mutation.
Do not invoke it as a local setup script. Unit tests mock every rustup command.

All existing platform, golden, GPU, lint and security gates remain. Windows golden images expand
from four to six shards, taking the workflow from 14 to 16 jobs; Linux keeps two shards. Requested
target combinations fall from the original 98 to 19, including the extra Windows jobs. This count is not an installation-time
benchmark. The new profile/environment initially creates cold caches; evaluate subsequent complete
runs as well as the first run. Compare cache restore logs, compilation steps, total critical-path
wall time and failed/retried jobs separately.

Evaluating the existing cost table against all 95 scene times from main run `40628572200` projects
that WARP 4→6 lowers the maximum scene load from 523.1 to 367.8 seconds. Including observed
cache-restoring job overhead and the other jobs, the model projects roughly 2m35s less critical-path
time for 13.6 additional runner-minutes. Additional Linux shards do not reduce that modeled critical
path. Each family still covers every scene exactly once. This is a scheduling projection; runner
speed, cache state and queuing vary, so validate it against complete hosted runs.

## Stack use

A release build's inlining decides what a frame holds, so read a release binary's prologues on arm64: `otool -tv target/release/sapphire_sandbox` shows `sub sp, sp, #N` for a small frame and, for a large one, `sub x9, sp, #N, lsl #12` with a probing loop after it. Add the frames down a call chain. Then `RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR=target/typesizes cargo rustc -p sapphire_render --lib --locked -- -Zprint-type-sizes` lists every type's size and fields; it rebuilds everything, so it gets a target directory of its own, to delete afterwards.

From `main` to the first pipeline, on an M5, a `--capture` run holds about 190 KiB (`OffscreenApp::init`, `Stage::new`, `Renderer::new`, the `join` of its closures), and the windowed app about 250 KiB (`app_init`, `SandboxApp::init`, `create_renderer`, then the same two). iOS's main thread has 1 MiB, and the driver's pipeline creation runs below that.

## Measurements

Controlled local measurements use Rust 1.99.0 on an Apple M5, four Cargo build jobs, isolated target
directories and alternating baseline/candidate order. Baseline commit:
`c2fe88353d618ef74039f9052378c17800a663b7`.

| Workload | Baseline median | Candidate median | Scope |
| --- | ---: | ---: | --- |
| Cooker-library semantic edit, original release → final tools, 5 pairs | 24.971 s | 1.702 s | 93.18% less time; every edited error message verified in the executable |
| Warm renderer source edit, workspace test build, 5 pairs | 3.804 s | 2.524 s | 33.65% less time; source comment forces rebuild/link; no tests executed |
| One gated shutdown regression, 10 pairs | 215.46 ms | 10.92 ms | 204.54 ms saved in this test only |
| 95 identical golden comparisons, 7 pairs | 2.987 s | 0.330 s | Includes two PNG decodes; identical metric digests |
| 95 alpha-only differences, 7 pairs | 3.001 s | 0.346 s | Same RGB semantics and output digests |
| Five changed-pixel controls, 7 pairs | about 0.162 s | about 0.164 s | No meaningful performance difference established |

The renderer rebuild's first pair included another candidate xtask compilation. Excluding that
pair still gives about a 33% reduction. The measurement does not establish a cold-build gain.
Golden comparison savings are about 2.66 seconds per identical 95-image pass, not a ninefold
improvement to the whole golden job. These savings cannot simply be added into an overall CI claim.

The cooker headline uses a reachable error-message edit, with a distinct marker for each sample,
and verifies the marker by invoking the built binary. Both variants rebuild the library and CLI.
It measures a localized edit with warm dependencies; broad refactors and first builds have different
costs. Initial profile preparation is excluded and does not establish a cold-build improvement.

Intermediate controls separated the two changes: disabling LTO reduced comment-only library
rebuilds from 24.926 to 15.298 seconds and CLI-only rebuilds from 17.539 to 0.609 seconds. Adding
incremental compilation reduced semantic edits from 15.307 to 2.074 seconds. These experiments
are separate from the direct original/final comparison and their speedups must not be multiplied.

All 379 loose assets and one pack matched byte for byte on both desktop and mobile in the direct
original/final comparison. Desktop full-cook samples were 15.078 and 15.177 seconds; mobile samples
were 10.352 and 10.284 seconds (one pair each, exploratory). Five paired full desktop cooks with
incremental off/on were 15.127 and 15.006 seconds at the median, with overlapping ranges. The
earlier release/no-LTO-only experiment was 14.757 versus 14.992 seconds, a 1.6% ratio-of-medians
overhead in that sample. The change targets rebuild latency; it does not claim faster cooking.

Golden validation compares complete metric and error-map byte streams as well as digests for
exact, alpha-only and changed-pixel cases. No images were rebaselined.

The final manifest was also exercised through unmodified `cargo --locked xtask --locked content
--cook desktop`, with no profile environment overrides. Five warm invocations took 0.545 seconds
at the median, and every output matched the original release cook. This is a candidate-only
measurement, not a before/after claim.
