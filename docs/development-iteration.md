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
- The recorded combat replays: `cargo xtask replay [--replay <name>]`. They need no GPU, and the suite itself takes about 20 ms. See [Replays](#replays).
- The same replays beside the renderer: `cargo xtask content`, then `cargo test -p sapphire_sandbox --test replays -- --ignored --test-threads=1`.
- The combat views and the overlay's recorder: `cargo test -p sapphire_sandbox --lib combat`, and with `--features dev -- --include-ignored` for what the panel is shown. The panel itself, clicks included: `cargo test -p sapphire_debug --lib combat::`.
- Live tuning: `cargo test -p sapphire_gameplay --lib replay::tuning`, and through a real sandbox `cargo xtask content`, then `cargo test -p sapphire_sandbox --features dev --test live_tuning -- --ignored --test-threads=1`. See [Tuning a fight](#tuning-a-fight).
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

`--replay <file>` and `--record <file> --scenario <name> --script <name>` go with `--capture` or `--bench`: the simulation takes a tick with each fixed step, beside the renderer. A replay is held to the recorded checksums beside its file, and a capture of one that has left them isn't written. A capture stops at the replay's end unless `--capture-ticks` picks an earlier tick. A bench goes on past the end, and says so, so give it a replay long enough for its frames (60 of warm-up, its own, and 240 timed).

`--record <file> --scenario <name>`, with no `--script`, `--capture` or `--bench`, records a fight someone plays: the windowed app's simulation takes each fixed step's input from combat's bindings (`sapphire_controls::ControlsSource`, through `combat::action_source` in the sandbox), turned by the camera's yaw, so the file carries world directions and no camera. A pause records nothing, since no steps run. The file is written when the app quits, so a killed process leaves none. On Android, give an absolute path inside the app's own directory (`/data/user/0/dev.sapphire.sandbox/files/<name>.sreplay`) and fetch it with `adb exec-out run-as dev.sapphire.sandbox cat files/<name>.sreplay`.

`--arena --fight` is a fight with no file, player against the enemy's brain, on the graphs, cues, HUD and sword a game would have: in a window it's played on combat's bindings alone, and it starts over 4 s after someone falls, or on `R` or the d-pad's down. Nothing's written. With `--script <name>` and `--capture`, `--bench` or `--offscreen`, a script plays it down the same path, which is how to run one here, where no window may open. Each fight logs a line when it ends, with its ticks, who fell, both healths, its checksum and its allocations; the checksum is the one a headless run of that script reaches at that tick.

The player's moves reach the simulation as a cooked copy, `crates/sapphire_gameplay/baked/player.smoves`. After changing `content/combat/player/` or `MoveSet`'s layout, rewrite it and the fight's replays:

```
SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy
cargo xtask replay --update
```

## Tuning a fight

Run a `dev` build with a fight in it, and save. `crates/sapphire_moves/test_content/slice.moves.ron`, a fight's other `.moves.ron`, `default_camera.ron`, `character.animgraph.ron` and, with `--cues`, `default_cues.ron` are watched already, and `--watch-document <path>` adds a copy elsewhere. On a phone, `cargo xtask link push <file>`, or `cargo xtask link watch`, which watches what the app's status lists.

- A move in progress finishes on what it started with, and the next start reads the edit. The log says `the next attempt plays the new moves`, and `cargo xtask link status` shows each fighter's revision and windows.
- A bad document changes nothing, and its issues name the move and the track. The next good one works.
- A move that names a clip takes new windows and phases with no cook. Its length is its clip's, and a change to the `clip` block needs `cargo xtask content`, then hot reload's remount or `cargo xtask link push`.
- A recording ends its file at the first edit that changes something, and the fight goes on: a replay can only answer for what it recorded. A fight started afterwards starts from the edit, and its file says so through its hash.
- A fight with no file takes edits too, and starts over from them.
- To try it with no window: `--offscreen --record <file> --scenario duel --script attempts`, which presses LIGHT every half second, or on the real moves `--offscreen --arena --fight --script attempts`.
- The simulation's copy of the player's moves stays what it was until it's rewritten (above), so after tuning `player.moves.ron`, rewrite it and commit it with the document. A `dev` build's fight already starts from a newer cook.

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
