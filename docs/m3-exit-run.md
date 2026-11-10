# The M3 exit run

M3 ends when its eight exit tests are answered ([m3-plan.md](m3-plan.md) §1): a zone with three encounters walked to its boss, a boss with no Rust in it, one cutscene, the HUD and menus in two languages, all of it on five platforms, the editor, the pipeline inventory, and budgets and durability. This file is the procedure for all eight, for someone coming back to it cold. For each test it says what proves it, the commands, what CI proves, what only this Mac proves, and what only a device or a person can.

Agents ran everything that needs no window and no phone, and "Rehearsal" near the end says what ran. Three parts of the slice weren't on `main` when this was written, so their sections are taken from the plan's rows and marked **lands with** the package that brings them:

- the boss as documents and a script reached from the zone: the chain B3 → B3b → A1 → C2 part 2;
- the game's flow, `--game title|new-game|continue` with the Lantern Ward as the flow: G1;
- the zone bench: Z1.

"What the final run must redo", at the end, lists what has to be taken again once those land. **Nobody has played the slice,** and none of these tests asks whether it's fun ([m3-plan.md](m3-plan.md) §1, "Feel stays unknown").

| Exit test | Who answers it | Needs | Where it stands |
|---|---|---|---|
| 1. A zone | CI, then this Mac | Nothing | Replays of the ward's walk hold on all three desktop OSes. The boss at its end lands with C2 part 2, and the walk through the game's flow with G1 |
| 2. A boss with no Rust | CI | Nothing | Lands with C2 part 2 (`tests/bosses_are_content.rs`) |
| 3. One cutscene | CI and this Mac | A window, or a person, for subtitles on screen | A watch and a skip reach one trail. Subtitles resolve in either language, and nothing draws them yet |
| 4. HUD and menus | CI and this Mac | A person who reads Japanese | Walked by pad, keyboard and touch, with goldens at three aspects in two languages. The Japanese is machine-quality and owed a reader |
| 5. Five platforms | CI, this Mac, the rehearsal | A phone and a desktop GPU other than this Mac's | Done, pending devices, as M1 and M2 closed |
| 6. The editor | CI | A person with a window | Proved by tests with no window. The window is owed |
| 7. The inventory | This Mac and CI's app job | Nothing | 0 first-use builds after load on Desktop-Low and Mobile-Low |
| 8. Budgets and durability | This Mac | A phone for the budgets that matter | Saves hold: kills at every step, and migrations. The zone bench lands with Z1 |

**The shortest path:** `cargo xtask content`, then `cargo xtask replay`, then the app lanes under "Exit test 7". That's tests 1, 3 and 7 on this Mac in about ten minutes once built. Everything else is CI's, a person's, or waits for G1, C2 part 2 and Z1.

## Before anything

```bash
cargo metadata --locked --format-version 1 > /dev/null   # xtask isn't --locked: catch a stale Cargo.lock first
cargo xtask content                                       # the desktop cook, into target/content/desktop
```

A worktree's `target/content` goes stale after a rebase onto new timelines, moves or documents: the app lanes then fail with "no cooked timeline has the simulation's timeline …". Cook again. After a cooked format changes, remove `target/content/cache` first, since a renumbered kind comes back stale from it.

Commands below are run from the repository's root on a Mac with Xcode, the Android SDK (`~/Library/Android/sdk`, NDK 29.0.14206865, platform 36) and Java 17 (`/opt/homebrew/opt/openjdk@17`). Nothing here opens a window unless its section says so.

## Exit test 1: a zone

The test: one zone with three encounters, walked from its start to its boss. Replays of the walk reproduce their checksums on all three desktop OSes, with no heap allocation inside the fixed step outside script hooks (spec §9, gates 2 and 4).

**What proves it.**

- The zone is `content/zones/lantern_ward` (M3 C1): the Gate Square, then a lane each way to the Market Court, the Blossom Garden and the Temple Court with its terrace. Each room has spawners and a threshold trigger, and the encounters are documents (M3 B2) reached from the zone's document.
- `walk-the-ward` walks the ward's tour end to end and back to the court through its four checkpoints (M3 B4). `zone-trip` goes between zones through their exits (M3 W4). `walk-the-ground`, `from-the-platform` and `walk-the-square` walk the proving ground and the square, and `nav-chase` and `nav-patrol` hold navigation.
- `tools/sapphire_pipeline/tests/zone_ward.rs` holds that a path and a real body get from `player_start` to every spawner, and that the documents reach each other.
- **Lands with C2 part 2:** the boss at the walk's end, and a replay that reaches it. **Lands with G1:** the walk as the game's flow (title, new game, the ward, its checkpoints, the boss, a return to title), and the replays recorded through it.

```bash
cargo xtask replay                                  # every replay, the step's allocations, mean and longest step
cargo xtask replay --replay walk-the-ward           # one
cargo test -p sapphire_pipeline --test zone_ward    # the ward's paths and documents
cargo xtask content
cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1   # beside the renderer
```

`cargo xtask replay` ends `N replays reach their checksums on aarch64`, a line each with ticks, final checksum, allocations in the step and the step's times. A failure names the first tick that differs, and a step that allocated fails too. Script hooks run inside the step and allocate nothing either (`arena-scripted`, `dialogue-flag`), so the suite holds the stricter rule.

**CI** runs `cargo --locked xtask --locked replay` in its required `Replays` job on `ubuntu-24.04`, `windows-2025` and `macos-26`. Linux and Windows compare against one file, `tests/replays/checksums.x86_64.txt`, which is what proves they agree, and macOS against `checksums.aarch64.txt`. GitHub's macOS runners were out for much of 2026-11-09, so this Mac's run has stood in for macOS's (the stand-in in the orchestrator's notes).

**Only this Mac** proves the walk beside the renderer on Metal: the app lane above.

**Only a device or a person:** nobody has walked the ward with their own hands. `walk-the-ward` can't be played in the app, since its scenario, `lantern-walk`, isn't one a stage stands (it has no arena colliders); the app plays `zone-trip` and `ward-court-*` instead.

## Exit test 2: a boss with no Rust changes

The test: the boss is documents and a script under `content/`. A test holds it: the boss's name appears in no `.rs` file, every file it needs is reached from the zone's document, and the commit that adds it to the zone touches only `content/`, `tests/replays/` and `tests/golden/`.

**Lands with C2 part 2** (the chain B3 → B3b → A1 → C2 part 2). From the plan's rows and that chain's hand-back, not run here:

- The Knell Colossus's moves, brain and encounter are documents. Its script is a Luau file its enemy kind names (`script:` in the `.enemy.ron`, M3 B3), so no Rust names it.
- `tests/bosses_are_content.rs` holds the three clauses: the name in no `.rs`, every file reached from the zone's document, and the adding commit's paths. The third reads git history, so CI's checkout needs the full history on Linux, or that clause skips.
- The boss's figure and clips are first drafts nobody has judged (M3 C2 part 1).

```bash
cargo test --test bosses_are_content    # lands with C2 part 2: the package that has it says where
cargo xtask replay                      # the boss's replay among them
```

**CI** runs it with the workspace's tests. **Only a person** can say whether the boss reads as a boss.

## Exit test 3: one cutscene

The test: it blends in and out with the gameplay camera, shows subtitles in the chosen language, and can be skipped. A skip and a watch both leave the fight that follows on the same checksum.

**What proves it** (M3 Q3, spec §5.23):

- The cutscene is `content/zones/lantern_ward/court_gate.timeline.ron`, nine seconds before the court's fight.
- `ward-court-watched` and `ward-court-skipped` reach one trail after the gate, and `ward-court` shows the fight up to the gate is the plain fight's. `cutscene-watched` and `cutscene-skipped` are the smaller pair, and `cinematic-watched` and `cinematic-skipped` the ultimate's (M3 Q3b).
- `crates/sapphire_gameplay/tests/timelines.rs` holds the trails, and `game_combat`'s scene tests the eye's path through both blends: under 0.35 m a frame.
- `tools/sapphire_pipeline/tests/cutscene_strings.rs` holds that the subtitles resolve in each language.
- The `cutscene-gate-warden`, `cutscene-gate-guard` and `cutscene-gate-return` goldens show it at 3.5, 6.2 and 8.6 s.
- The sandbox's lane `the_wards_gate_plays_between_its_steps_and_streams_nothing_late` plays the watch on Desktop-Low and Mobile-Low: its checksums, no step allocation, no first-use pipeline and no late stream.

```bash
cargo xtask replay --replay ward-court-watched
cargo xtask replay --replay ward-court-skipped
cargo test -p sapphire_gameplay --test timelines
cargo test -p sapphire_pipeline --test cutscene_strings
cargo xtask golden --scene cutscene-gate-guard
cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1 the_wards_gate
```

**CI** runs all of it, the goldens on lavapipe and WARP too.

**Open:**

- Subtitles are resolved, not drawn. Q3 left drawing them to U7 and Q2, and no screen draws them yet. Until one does, "shows subtitles" stands on the strings test alone. **Lands with G1** if its flow draws them; otherwise it's a finding of this run.
- The cutscene plays at a fixed tick in `ward-court`, not at the court's threshold trigger. **Lands with G1**, whose flow reaches the court through the ward.
- `game_app` can't yet ask for a timeline. **Lands with G1.**

**Only a person** can say whether the blend reads well. The camera's numbers are bounded, and nobody has watched it.

## Exit test 4: a full HUD and menus in two languages

The test: HUD, title, pause, settings, character screen and dialogue panel, in English and Japanese, with goldens at 4:3, 16:9 and 21:9. A scripted walk through every menu by gamepad focus, by keyboard and by synthetic touch. Pseudo-localization finds no string outside the tables.

**What proves it** (M3 U7, C3, Q2b):

- `game/game_ui/tests/menus.rs` walks every menu by pad, keyboard and finger through `MenuControls`' raw input, in both languages and under pseudo-localization, at 4:3, 16:9, 21:9 and two phone sizes. It holds that every row is reached and no string leaves what holds it.
- `game/game_ui/tests/dialogue.rs` walks the panel on a real conversation: the guide's, opened by the court's gate trigger, through a branch and a hidden answer, by the same three devices.
- `game/game_ui/tests/strings.rs` holds every key in both tables.
- **Goldens,** on a flat backdrop:
  - `game-<screen>-{tablet,wide,ultrawide}-{en,ja}` for each screen;
  - `game-character-phone-en` on Mobile-Low at 2×;
  - `game-guide-*`: the panel on the guide's real line.
- `game_app`'s `opens_the_guide` opens the guide from the cooked pack and walks it by Return.

```bash
cargo test -p game_ui --test menus --test dialogue --test strings
cargo xtask golden --scene game-settings-wide-ja
cargo xtask content
cargo test -p game_app --features dev --locked --test opens_the_guide -- --ignored
```

A screen by hand, offscreen: `cargo xtask run --platform desktop -- --capture target/settings-ja.png --size 1280x720 --ui-plain --ui-game settings --ui-locale ja`. Add `--ui-pseudo` for pseudo-localization, or `--ui-game dialogue --ui-dialogue 1` for the guide's question.

**CI** runs the tests and the goldens on lavapipe and WARP.

**Only a person:**

- The Japanese is machine-quality and owed a Japanese reader's pass (M3 C3). Both `.ftl` files' resource comments say so.
- A real pad's glyphs and a real thumb on a phone haven't touched a menu. `simctl` can't touch, so iOS touch is unproved; the Android emulator takes real multi-touch events (running.md).
- **Lands with G1:** title, new game and continue as the game's flow, and the pause and its return to title in a real run.

## Exit test 5: all on five platforms

The test: Linux and Windows through CI on software rasterizers, macOS on Metal, and iOS Simulator and an Android emulator by the exit run. With no phone and no desktop GPU but this Mac's, **M3 can only close as "done, pending devices",** as M1 and M2 did ([m3-plan.md](m3-plan.md) §1).

**What proves it:**

| Platform | Proved by | Where |
|---|---|---|
| Linux | `Test`, `Replays`, `Golden images (vulkan-lavapipe)`, `GPU tests (vulkan-lavapipe)`, `App tests (vulkan-lavapipe)` | CI on every pull request |
| Windows | `Test`, `Replays`, `Golden images (dx12-warp)`; `GPU tests (dx12-warp)` on pushes to `main` | CI |
| macOS | `Test`, `Replays`, `Golden images (metal)`, and every `--ignored` lane here | CI's `macos-26`, and this Mac when GitHub's macOS runners are out |
| iOS | `Mobile build (iOS)` builds both apps for the device and the simulator | CI. Running is the rehearsal's, on a private Simulator |
| Android | `Mobile build (Android)` builds both APKs | CI. Running is the rehearsal's, on the emulator, replays included since M3 Z2b |

**On the iOS Simulator.** Use a private one, so `simctl install` doesn't kill another agent's app, and delete it after. Never open Simulator.app.

```bash
cargo xtask bundle --platform ios-sim --app game_app --release          # target/ios-sim/<Name>.app
UDID=$(xcrun simctl create z2-exit "iPhone 17 Pro")
xcrun simctl boot "$UDID" && xcrun simctl bootstatus "$UDID" -b
xcrun simctl install "$UDID" target/ios-sim/<Name>.app
xcrun simctl launch --console-pty "$UDID" dev.sapphire.game --capture <png> --size 1280x720 --ticks 120
xcrun simctl io "$UDID" screenshot target/sim.png
xcrun simctl shutdown "$UDID" && xcrun simctl delete "$UDID"
```

Mobile bundles take `--release`: `--incremental` is for `--platform desktop` and `macos` only. The app can write only inside its own data container, so give `--capture` a path there: `xcrun simctl get_app_container "$UDID" dev.sapphire.game data` names it, and its `tmp/` will do. A container's path changes with each install.

The sandbox plays a recorded zone replay to its checksum the same way. Bundle it with `cargo xtask bundle --platform ios-sim --release --features dev`, copy the replay and `tests/replays/checksums.aarch64.txt` into the sandbox's container, and launch `dev.sapphire.sandbox` with `--capture <png> --arena --replay <file>`, both paths in that container. The app logs `replay reached its recorded checksums` with the checksum. "Rehearsal" has what ran.

**On the Android emulator** (the AVD `sapphire_api36`, headless):

```bash
~/Library/Android/sdk/emulator/emulator -avd sapphire_api36 -no-window -gpu host -no-audio -no-boot-anim &
adb wait-for-device
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask bundle --platform android --app game_app --release
adb install -r <apk>
adb shell am start -n dev.sapphire.game/.GameActivity --es sapphire.args <base64 JSON of the arguments>
adb exec-out screencap -p > target/emulator.png
adb logcat -d | grep -i sapphire
adb emu kill
```

The windowed apps have no `--capture`: read their logcat and take the screen with `screencap`.

**A replay on the emulator** (M3 Z2b). Android's `SDL_main` runs the sandbox's offscreen app when the intent's arguments ask for `--capture`, `--bench`, `--contact-sheet` or `--offscreen`, as `main` does on the desktop. A relative path goes under the app's cache, so push the replay and the checksums there:

```bash
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask bundle --platform android --release --features dev
adb install -r target/android/SapphireSandbox.apk
adb push tests/replays/zone-trip.sreplay tests/replays/checksums.aarch64.txt /data/local/tmp/
adb shell run-as dev.sapphire.sandbox cp /data/local/tmp/zone-trip.sreplay /data/local/tmp/checksums.aarch64.txt cache/
adb shell am start -n dev.sapphire.sandbox/.SandboxActivity --es sapphire.args <base64 of ["--capture","end.png","--size","640x360","--arena","--replay","zone-trip.sreplay"]>
adb logcat -d | grep -E 'replay reached|captured frame|main returned'
adb exec-out run-as dev.sapphire.sandbox cat cache/end.png > target/emulator-end.png
```

`cargo xtask run --platform android --release --features dev -- --capture end.png --arena --replay zone-trip.sreplay` installs, launches and exits with the app's status once the files are in the cache: 0 when the replay reached its checksums, 1 with the reason in the log when it didn't. The activity's window stays black while it runs. The game takes `--capture` the same way: `--open-fight --script fight-back --ticks 1695 --capture fight.png` writes `cache/fight.png` and logs its fight's checksum.

**CI** builds both mobile apps on every pull request and runs nothing on them.

**Only this Mac:** Metal, the Simulator and the emulator. All three draw on this Mac's GPU.

**Only a device:**

- An iPhone 12 and a Snapdragon 865-class phone, the reference phones ([m3-handoff.md](m3-handoff.md)): a real thumb, thermal throttling, a phone's memory and flash, iOS touch.
- A desktop GPU other than Apple's: an AMD, NVIDIA or Intel card on Windows and Linux. CI's lavapipe and WARP are CPU rasterizers.

## Exit test 6: the editor

The test: every edit is a change record, undo returns a document to the same bytes, and play-in-editor starts from a snapshot and leaves the documents alone. All of it is proved by tests with no window, and the window itself is owed to a person (m3-plan §7).

**What proves it** (M3 E1 to E5):

- `tools/sapphire_document`: `fuzz.rs` holds that any sequence of edits undoes to the same bytes, and `cases.rs`, `set.rs` and `saves.rs` hold the records, a set of documents and saving.
- `tools/sapphire_editor/tests/viewport_stage.rs`: `a_run_plays_its_snapshot_and_the_next_plays_the_edit` holds a run to a headless fight's checksums, a document to its bytes, history and file through 600 ticks, and an edit made during a run to the next one. `the_keys_play_a_run` covers the keys. The viewport draws with 0 pixels differing from a capture of the same scene, and 0 first-use builds.
- `docking.rs`, `panes.rs`, `ui.rs` and `viewport.rs` drive the shell under `egui_kittest` with no window. `preview.rs` and `sandbox_push.rs` cover the preview and pushing to a running sandbox.
- E5's zone and sequencer panels: a gizmo drag changes that one number's bytes, and undo gives the file's back.

```bash
cargo test -p sapphire_document
cargo test -p sapphire_editor
cargo xtask content --cook editor
cargo test -p sapphire_editor --locked --test preview --test sandbox_push --test viewport_stage -- --ignored
```

**CI** runs the unit tests in `Test`, and the `--ignored` lanes in `App tests (vulkan-lavapipe)`.

**Only a person:** `cargo xtask editor` opens a window, which no agent may do. Docking by hand, a real drag on a gizmo, the keyboard's focus and a lost device in a window are owed. Only moves reach a run in play-in-editor; spec §5.28 lists the edits it doesn't show.

## Exit test 7: the inventory

The test: a cooked pack lists its pipelines, a load compiles them, and a replay of the walk reads 0 first-use builds after load on Desktop-Low's and Mobile-Low's settings.

**What proves it** (M3 K1):

- `sapphire_assets::PipelineKey` per material slot in each pack's table, and `Renderer::prewarm` over the app's declared families. `PipelineBuilds` counts every build as prewarmed, rebuilt or first use.
- The sandbox's lane `walking_a_zone_builds_no_pipeline_and_a_grid_left_out_is_named` walks the ward's tour on every tier's settings and reads `first_use=0 rebuilt=0`. It then leaves a family out of the inventory and sees it named.
- The same file's zone-trip lane reads no misses across the zone changes.
- `game_app`'s `reaches_the_fight` goes from the title through a fight's first seconds with no first-use build. `zone_trip` and `opens_the_guide` hold the same through a trip and a conversation.

```bash
cargo xtask content
cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1
cargo test -p game_app --features dev --locked --test reaches_the_fight -- --ignored
cargo test -p game_app --features dev --locked --test zone_trip -- --ignored
cargo test -p game_app --features dev --locked --test opens_the_guide -- --ignored
```

A walk by hand: `SAPPHIRE_CONTENT_DIR=target/content/desktop target/release/sapphire_sandbox --bench 240 --zone lantern_ward --zone-walk tour --tier mobile-low --size 640x360`. Read the `zone walk` row's `first_use=` and `rebuilt=`.

**CI's** `App tests (vulkan-lavapipe)` runs all of these, and it's CI's slowest job (about 75 minutes, with a 90-minute limit).

**Only a device:** a phone's driver compiles at its own pace. Cold, the load blocks 6–7 s on an M5, and the simulator built for 5.7 s and the emulator for 4.0 s on first launch (M3 S2). **Lands with G1:** the replay of the whole walk through the game's flow.

## Exit test 8: budgets and durability

The test: §3.2's script and streaming rows read within budget on a zone bench. A save survives a kill at every step of a write, and an old save migrates.

**Durability, proved** (M3 V1):

- `crates/sapphire_save/src/slot.rs`'s tests inject a fault at every step of a write: `each_fault_names_itself`, and `test_support::faulty_replace`.
- `crates/sapphire_save/tests/kill.rs` kills a helper process at a random moment a few hundred times while it writes, and each load after must be whole and hold every write the helper reported done.
- `crates/sapphire_save/tests/migrations.rs` loads committed old saves (`tests/fixtures`) through `Versioned` migrations and refuses a newer one.
- Q2 added a save made mid-quest that replays as never having stopped, through the real `ProgressStore`.

```bash
cargo test -p sapphire_save
cargo test -p sapphire_save --test kill
cargo test -p sapphire_save --test migrations
```

**CI** runs them on all three OSes. **Only a device:** a power cut, a phone's flash and `SDL_GetPrefPath`'s real paths. The kill is the process's, so the page cache survives it.

**Budgets: lands with Z1.** §3.2's rows are 0.3 ms (Desktop) and 0.5 ms (Mobile) for script events and Luau's collector step, and 0.5 and 1.0 ms for streaming activation. Until Z1:

- The script row has `hook_cost` (`cargo run --release -p sapphire_script --example hook_cost`, M3 L2) with no zone behind it.
- The streaming row was read over a synthetic walk (spec §5.21, M3 W2): p99 about 24 µs on both Low tiers at a load average of 33 to 39.
- A conversation's conditions bind in 16 µs and a condition is asked in 0.6 µs (`dialogue_bench`, Q2b).

Z1 brings `--bench` on the ward's walk with §3.2's rows, memory across the walk and a transition's time, and grows spec §3.5's table. Its command is about:

```bash
SAPPHIRE_CONTENT_DIR=target/content/desktop target/release/sapphire_sandbox \
  --bench 4000 --zone lantern_ward --zone-walk tour --stream 64:48 --tier mobile-low
```

**Only a device:** the rows that matter are a phone's, on Mobile-Low, thermally throttled. This Mac's numbers say the work is there and nothing grows, as M2's did.

## Rehearsal

Run on 2026-11-09 on Q2b's head, before the chain, G1 and Z1, on an M5 at a load average of 14 to 75 with other agents building. Headless throughout: a private Simulator, the emulator with `-no-window`, and the Mac offscreen.

**Builds.** All four mobile bundles built with `--release`: the iOS game in 643 s, the iOS sandbox in 318 s, the Android game in 430 s and the Android sandbox in 189 s.

**iOS Simulator** (a private iPhone 17 Pro on iOS 26.3, booted in 37 s, deleted after):

| Run | Result |
|---|---|
| The game's title, `--capture … --size 1280x720 --ticks 120` | Exit 0 in 14 s on Mobile-Low. Cold, the packs' pipelines built in 5.0 s, then `prewarmed=68 first_use=0`. The title draws |
| The game's fight, `--open-fight --script fight-back --ticks 1500 --capture …` | `checksum=4ed8d3c92d6b7a08 allocations=0`, this Mac's at tick 1500. The HUD draws |
| The game in its window, `simctl launch` with no arguments | The title draws in landscape. Warm, the prewarm took 34 ms |
| The sandbox, `--capture … --arena --replay zone-trip.sreplay` | 720 of 720 ticks, five trips, `checksum=ba5b63eb905b3d70 allocations=0 first_use=0`: the committed aarch64 checksums. It needed the fix below |
| The sandbox, `ward-court-watched` and `ward-court-skipped` the same way | Both 1200 ticks to `ff6c6da6027a0857`, the committed checksum, `first_use=0` |

**Android emulator** (`sapphire_api36`, arm64-v8a, 1080x2400, headless):

| Run | Result |
|---|---|
| The game's fight, windowed, `--open-fight --script fight-back` | Ended at tick 1695 with `checksum=7be80d23e926a16a allocations=0`, this Mac's at tick 1695, and the same `fell_at` and health as the Simulator's. The HUD and touch controls draw. Cold, the packs' pipelines built in 16.4 s under load |
| The game's title, windowed | Draws on Mobile-Low, Vulkan. Warm, the prewarm took 291 ms, then `prewarmed=68 first_use=0` |
| The sandbox, `--arena --replay zone-trip.sreplay` | Refused: `--replay needs --capture or --bench`. No replay played on Android then |

So a scripted fight reached one checksum on three targets: this Mac on Metal, the Simulator and the emulator. The zone and cutscene replays reached their committed checksums on the Mac and the Simulator, beside the renderer.

**Replays on the emulator, after M3 Z2b** (on the chain's head, f39fad2b, before G1 and Z1). Android's entry now runs the offscreen app when asked, so the sandbox's `--capture … --size 640x360 --arena --replay <file>`, with the file and the checksums in the app's cache:

| Replay | Result |
|---|---|
| `zone-trip` | 720 of 720 ticks, five trips, `checksum=ba5b63eb905b3d70 allocations=0`, `prewarmed=58 first_use=0`, `main returned status=0` |
| `ward-court-watched` | 1200 of 1200 ticks, `checksum=ff6c6da6027a0857 allocations=0`, `prewarmed=61 first_use=0` |
| `arena-fight-won` | 1700 of 1700 ticks, `checksum=3357fdf8b6651dc5 allocations=0`, `prewarmed=57 first_use=0`. The fight draws |

| The game, `--open-fight --script fight-back --ticks 1695 --capture fight.png` | `checksum=7be80d23e926a16a allocations=0`, as windowed before, `prewarmed=68 first_use=0`, and `cache/fight.png` with the HUD and touch controls |

All three replays reached the committed aarch64 checksums, so the Mac, the Simulator and the emulator agree on the replays too. `cargo xtask run --platform android` of `zone-trip` exited 0, and of a missing file exited 1, naming it. The windowed sandbox still opened as before.

**Found and fixed: a capture to a replay's end hung on a zone trip.** `--capture` with `--replay zone-trip.sreplay` spun at full CPU forever, on this Mac and on the Simulator alike. A capture of a replay's end steps on through the steps a trip holds, but the trip waits on what only a drawn frame moves: the next zone's load, its first cells, and the wipe covering and clearing. The capture now draws a frame for each step a trip holds, and the sandbox's zone-trip lane captures the end as well as benching it. Nothing else draws differently, since only a trip's held steps take the new frames. The picture at `zone-trip`'s end is the wipe still clearing from the fifth trip, which lands just before the end.

**Noticed, not fixed:**

- No replay played on Android. Z2b gave Android the offscreen entry, above.
- `walk-the-ward` plays in no app, since its scenario isn't one a stage stands (exit test 1). The walk through the game's flow lands with G1.
- On the emulator, audio warns that real-time scheduling was refused. Sound wasn't checked.
- `--capture-ticks` without a replay's end counts the steps a trip holds: a capture at 600 on `zone-trip` stops at the replay's tick 120. That's by design (the capture's steps are frames), and worth knowing before choosing a tick.

### Rehearsal at 00d05300 (M3 Z2c)

The runbook again on 2026-11-10, on the line after the chain, G1, zone-scripts and Z3e (00d05300), before Z1's fixes, Z3f and S3b. Not a final measurement: an M5 with other agents building, at load averages of 30 to 720 (each figure has its load beside it).

**Builds:** the desktop cook in 193 s, the mobile in 8 s, the release sandbox and game in 768 s; on iOS the game in 1,577 s and the sandbox in 1,026 s; on Android the game in 537 s and the sandbox in 706 s. Gradle needs `JAVA_HOME` set, as `running.md` says.

**Exit test 1:** `cargo xtask replay`: 37 of 37 replays reach their aarch64 checksums (432 s). The sandbox's replay lane: 21 of 22, the one failure below. `zone_ward` 6 of 6, `sapphire_save` 70 of 70.

**Exit test 2:** `bosses_are_content` 2 of 2, with this checkout's whole history.

**Exit tests 5 and 7, one checksum on three targets.** The Mac (Metal, release), a private iPhone 17 Pro Simulator (iOS 26.3, Mobile-Low) and the emulator (`sapphire_api36`, Vulkan, Mobile-Low, headless), each with 0 first-use builds:

| Run | Mac | Simulator | Emulator |
|---|---|---|---|
| Sandbox `--capture … --arena --replay zone-trip` | `83b541032f77330b` | the same, `prewarmed=58` | the same |
| … `ward-court-watched` | `e40bf9319bda39cc` | the same, 61 | the same |
| … `arena-fight-won` | `3357fdf8b6651dc5` | the same, 57 | the same |
| … `arena-boss` | `21f263cd61ff61a9` | the same, 57 | the same |
| … `walk-the-ward`, now standable | `512b7b7cf30583c8` | the same, 61 | the same |
| Game `--open-fight --script fight-back --ticks 1695 --capture` | `53054fedc47e8922`, `prewarmed=93` | the same, 77 | the same, 77 |
| Game `--new-game --script walk-the-ward --ticks 4298 --capture` | two runs end, at 2126 (`32e4246e8074bcb1`) and 2107 (`76dea41f2df08eef`) | the same | the same |
| Game's title, `--capture … --ticks 120` | | draws, cold prewarm 5.1 s | |

The game's lanes (`reaches_the_fight`, `zone_trip`, `opens_the_guide`, `plays_the_ward`) pass on the Mac. In the game, `walk-the-ward`'s script only walks: the market's encounter starts at tick 545, the player's health reaches 0 and the run ends at 2126, then again at 2107 after the restart. A walk to the boss through the flow needs a script that fights (G1c).

**Exit test 8, the bench** (`--bench 1`, §3.6's loads), ms p50 / p99 / worst, its "cpu" phase; load average (1 minute) in the last row:

| Row | DH walk | DL walk | MH walk | ML walk | DL walk again | ML walk again |
|---|---|---|---|---|---|---|
| CPU frame | 5.56 / 16.1 / 85.5 | 5.43 / 22.4 / 86.3 | 4.50 / 15.5 / 103 | 4.59 / 29.2 / 128 | 5.48 / 25.8 / 126 | 4.63 / 33.7 / 104 |
| Simulation | 0.070 / 0.178 / 4.09 | 0.069 / 0.306 / 14.2 | 0.071 / 0.306 / 11.4 | 0.091 / 0.481 / 21.6 | 0.069 / 0.258 / 10.6 | 0.090 / 0.253 / 5.84 |
| Animation + IK | 0.036 / 0.081 / 1.23 | 0.036 / 0.138 / 2.61 | 0.036 / 0.141 / 4.87 | 0.037 / 0.146 / 1.20 | 0.036 / 0.130 / 6.27 | 0.036 / 0.104 / 7.77 |
| Secondary motion | 4.12 / 7.82 / 84.0 | 4.02 / 12.7 / 84.9 | 3.11 / 8.62 / 100 | 3.21 / 14.5 / 100 | 4.03 / 12.3 / 111 | 3.23 / 18.4 / 66.6 |
| Script events | 0.008 / 0.033 / 0.888 | 0.008 / 0.036 / 0.398 | 0.008 / 0.040 / 11.3 | 0.015 / 0.060 / 1.01 | 0.008 / 0.033 / 1.26 | 0.015 / 0.040 / 2.13 |
| Streaming activation | 0.001 / 0.003 / 0.116 | 0.001 / 0.002 / 0.143 | 0.001 / 0.003 / 1.81 | 0.001 / 0.003 / 0.024 | 0.001 / 0.003 / 0.056 | 0.001 / 0.002 / 0.010 |
| Audio update | 0.002 / 0.004 / 0.027 | 0.002 / 0.004 / 2.57 | 0.002 / 0.004 / 0.269 | 0.002 / 0.004 / 0.202 | 0.002 / 0.005 / 11.6 | 0.002 / 0.003 / 1.18 |
| Extract | 0.033 / 0.072 / 2.08 | 0.033 / 0.091 / 0.991 | 0.034 / 0.104 / 1.15 | 0.033 / 0.107 / 14.3 | 0.033 / 0.094 / 8.42 | 0.032 / 0.065 / 0.681 |
| Render prepare + record | 1.19 / 11.0 / 67.4 | 1.20 / 11.4 / 57.2 | 1.13 / 8.43 / 53.4 | 1.07 / 16.1 / 70.7 | 1.22 / 17.2 / 42.1 | 1.07 / 20.8 / 59.3 |
| Main pass (GPU) | 4.44 / 8.45 / 10.3 | 6.77 / 11.6 / 16.0 | 5.02 / 10.0 / 13.1 | 2.38 / 7.83 / 8.98 | 6.69 / 10.3 / 14.1 | 2.63 / 6.58 / 8.84 |
| Load | 33 to 41 | 41 to 447 | 447 to 721 | 721 to 693 | 32 to 34 | 34 to 35 |

- **Every CPU row reads about 2 to 2.8 times §3.6's** (secondary motion 4.03 against 1.947 on Desktop-Low, the simulation 0.069 against 0.025), and the second round, at a load of 32 to 35, reads the same as the first at 41 to 720. No row grows apart from the others, so this looks like a saturated machine (§3.6 read at a median load of 9) rather than a change in the code; only a quiet machine can say. Secondary motion is over its budget on both low tiers, as §3.6 found; Z1's fixes are on it.
- **The script row is read on the walk now:** the ward's scripts are in this base. Its p50 and p99 are within §3.2's 0.3 and 0.5 on both low tiers; single worsts reach 1.3 to 11 ms under this load.
- **Other loads, once each:** streaming at 48 m on Mobile-Low 0.001 / 0.009 / 3.9 at load 35. `zone-trip`: five trips each play, covered in 34 frames and clear in 69 on Desktop-Low (0.67 to 1.16 s), 18 and 37 on Mobile-Low (0.25 to 0.37 s), at load 300. `arena-scripted`'s script row 0.009 / 0.038 / 0.162 (DL) and 0.021 / 0.038 / 1.34 (ML). `arena-boss` and `arena-fight-won` read no script row.
- **Allocations:** a frame on the walk 764 / 760 / 637 / 506 by tier, as §3.6. In fixed steps: none on the walks, fights and boss after the first play; on the first play's tick 2, 6 on the walk, 167 on `arena-scripted` and 83 on `arena-boss` (each fight's start, once); on `zone-trip`, one a trip in every play (§3.6's `holds_step` copy, which Z1's fixes take).
- **Memory across the walk:** the engine's resident 60.9 MB on the desktop tiers and 48.9 on the mobile ones, flat; the process's most 818, 631, 473 and 439 MB by tier.
- **The GPU frame row** is "not exercised" on Metal in every run, as §3.6 says.

**Found and fixed: the emulator's audio warning.** `cpal`'s `realtime` feature asks AAudio for its low-latency path and reports a refusal through the stream's error callback, which logged a warning on every stream opened. The emulator's audio has no low-latency path, so it always refused. The refusal is now a state (`DeviceStatus::low_latency_refused`, on `audio stats` and the overlay) and one info line. On the emulator now: `the device refused the low-latency path; sound plays on its normal one` once, `low_latency_refused=true` on each stats line, no warning, 0 xruns.

**Failed:** the sandbox's replay lane's `a_bench_of_a_fight_plays_it_whole_beside_the_renderer` (`apps/sapphire_sandbox/tests/replays.rs:670`). It finds the `gpu frame` row by `row="gpu frame"`, which the `fight row not exercised` line also carries, so on Metal, where no frame's span decodes, it expects a row with no samples. Z1's fixes are on it.

## What the final run must redo

The rehearsal ran on Q2b's head, before the chain, G1 and Z1. The final run is on the head after all three land, and it takes again:

- **Before anything,** on that head: the lock check and a fresh cook. Remove `target/content/cache` first, since the chain renumbers cooked kinds.
- **Exit test 1:** `cargo xtask replay` and the sandbox's replay lane, with the boss's replay and the replays G1 records through its flow among them. Then the walk from the title to the boss and back, through the game's flow. CI's `Replays` on all three OSes on that head: a macOS job if GitHub's runners are back, this Mac's stand-in if not.
- **Exit test 2:** `tests/bosses_are_content.rs` on CI's Linux job with the full history, so its third clause runs rather than skips.
- **Exit test 3:** the cutscene from the court's threshold in the ward's walk, not at a fixed tick. Its subtitles on screen in both languages, by a golden, if G1 draws them; if nothing does, say so as a finding.
- **Exit test 4:** title, new game, continue, the pause and its return to title in a real run, and the title's goldens (`game-title-*`, with CI's lavapipe and WARP).
- **Exit test 5:** the whole rehearsal again on the final head, on a fresh private Simulator and the emulator. The replays this time on both, the walk G1 records and the boss's among them. Add G1's flow to it: `--game new-game` to a capture on both.
- **Exit test 7:** the app lanes, with G1's replay of the whole walk through the flow reading 0 first-use builds on Desktop-Low and Mobile-Low.
- **Exit test 8:** Z1's bench on the ward's walk on Desktop-Low and Mobile-Low, §3.2's script and streaming rows, memory across the walk and a transition's time, into spec §3.5's table. Note the load average beside each number. Run it after the chain and G1, since both change what a step runs.
- **The record:** the table at the top, each test's "Where it stands", and an "As built" line for Z2 in [m3-plan.md](m3-plan.md).

Exit test 6 has nothing to redo unless the editor changes.
