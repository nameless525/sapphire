# The M3 exit run

M3 ends when its eight exit tests are answered ([m3-plan.md](m3-plan.md) §1): a zone with three encounters walked to its boss, a boss with no Rust in it, one cutscene, the HUD and menus in two languages, all of it on five platforms, the editor, the pipeline inventory, and budgets and durability. This file is the procedure for all eight, for someone coming back to it cold. For each test it says what proves it, the commands, what CI proves, what only this Mac proves, and what only a device or a person can.

Agents ran everything that needs no window and no phone. "Final run at 4fe253e6", near the end, is the run on the head with all of M3 (2026-11-10), and "Rehearsal" what ran before it. **Nobody has played the slice,** and none of these tests asks whether it's fun ([m3-plan.md](m3-plan.md) §1, "Feel stays unknown").

| Exit test | Who answers it | Needs | Where it stands |
|---|---|---|---|
| 1. A zone | CI, then this Mac | Nothing | Holds. The whole walk through the game's flow, from a new game at the title to the Knell Colossus's defeat (`ward-whole-walk`, G1c), and the ward's other replays reach their checksums on all three desktop OSes with no allocation in a step. The Simulator and the emulator reach the same |
| 2. A boss with no Rust | CI | Nothing | Holds: `bosses_are_content`'s three clauses, the third on CI's Linux leg with the whole history |
| 3. One cutscene | CI and this Mac | A person who reads Japanese, for the subtitles' wording | Holds. A watch and a skip reach one trail; the whole walk watches it from the court's threshold. The HUD draws the subtitles in the chosen language, with goldens at three aspects in two languages (M3 Q4) |
| 4. HUD and menus | CI and this Mac | A person who reads Japanese | Holds but for meaning. Walked by pad, keyboard and touch, with goldens at three aspects in two languages; title, new game, continue, the pause and its return to title in a real run. The Japanese is machine-quality and owed a reader |
| 5. Five platforms | CI, this Mac, the final run | A phone and a desktop GPU other than this Mac's | Done, pending devices, as M1 and M2 closed |
| 6. The editor | CI | A person with a window | Proved by tests with no window. The window is owed |
| 7. The inventory | This Mac and CI's app job | Nothing | Holds: 0 first-use builds after load on Desktop-Low and Mobile-Low, the whole walk through the flow among them (`wins_the_ward`) |
| 8. Budgets and durability | This Mac | A phone for the budgets that matter | Saves hold: kills at every step, and migrations. On the zone bench, §3.2's script and streaming rows are well within budget on this Mac. Secondary motion meets its rows at p50; its p99 is a solve frame's, over on Desktop-Low on every load and on Mobile-Low's fights and trips. Desktop-Low's TAA and post are over at p99 |

**The shortest path:** `cargo xtask content`, then `cargo xtask replay`, then the app lanes under "Exit test 7". That's tests 1, 3 and 7 on this Mac in about ten minutes once built. Everything else is CI's or a person's.

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
- The boss at the walk's end is the Knell Colossus on the terrace (C2 part 2), and `arena-boss` is its fight on its own. The walk as the game's flow (title, new game, the ward, its checkpoints, the boss) is G1's, below.
- **The whole walk (M3 G1c):** `ward-whole-walk` is the game's flow from a new game at the title along the ward's tour, through the market's, the garden's and the court's fights, the court's gate cutscene watched and the guide's conversation, to the terrace's Knell Colossus beaten and the flow's victory. `game/game_flow/tests/ward_walk.rs` recorded it from a player who reads the simulation before each step (a walker along the tour who fights whoever's nearest, learns each swing's wind-up once and dodges it from then on); nothing in the fight was tuned for it. `the_whole_walk_replays_to_the_win` plays the file through the same flow, headless, in every OS's tests, and `game_app`'s `wins_the_ward` plays it on the stage (exit test 7). It found the ward's run held 12 actors for 15, so the boss was refused and the terrace cleared with nobody in it: the scenario holds 15 now, and the game refuses a flow whose scenario can't hold its cast and every fighter its encounters bring.

```bash
cargo xtask replay                                  # every replay, the step's allocations, mean and longest step
cargo xtask replay --replay walk-the-ward           # one
cargo test -p game_flow --test ward_walk            # the whole walk through the game's flow, to the win
cargo test -p game_flow --test ward_walk -- --ignored --nocapture   # records it again, as below
cargo test -p sapphire_pipeline --test zone_ward    # the ward's paths and documents
cargo xtask content
cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1   # beside the renderer
```

When the fight changes under the whole walk, its file stops reaching the win (the replay plays what was pressed, not what the player saw): record it again with `SAPPHIRE_WALK_OUT=<repo>/tests/replays/ward-whole-walk.sreplay` on the ignored test, which writes over it, then `cargo xtask replay --update --replay ward-whole-walk`, and take x86-64's values from CI as for any moved replay. The test prints the run's length, what hit the player, and the health left; if the reading player can't win any more, that line is what loses.

`cargo xtask replay` ends `N replays reach their checksums on aarch64`, a line each with ticks, final checksum, allocations in the step and the step's times. A failure names the first tick that differs, and a step that allocated fails too. Script hooks run inside the step and allocate nothing either (`arena-scripted`, `dialogue-flag`), so the suite holds the stricter rule.

**CI** runs `cargo --locked xtask --locked replay` in its required `Replays` job on `ubuntu-24.04`, `windows-2025` and `macos-26`. Linux and Windows compare against one file, `tests/replays/checksums.x86_64.txt`, which is what proves they agree, and macOS against `checksums.aarch64.txt`. GitHub's macOS runners were out for much of 2026-11-09, so this Mac's run has stood in for macOS's (the stand-in in the orchestrator's notes).

**Only this Mac** proves the walk beside the renderer on Metal: the app lane above.

**Only a device or a person:** nobody has walked the ward with their own hands. The sandbox plays `walk-the-ward` and `ward-whole-walk` to their checksums with `--arena --replay`, on this Mac, the Simulator and the emulator ("Final run at 4fe253e6").

## Exit test 2: a boss with no Rust changes

The test: the boss is documents and a script under `content/`. A test holds it: the boss's name appears in no `.rs` file, every file it needs is reached from the zone's document, and the commit that adds it to the zone touches only `content/`, `tests/replays/` and `tests/golden/`.

**What proves it** (the chain B3 → B3b → A1 → C2 part 2):

- The Knell Colossus's moves, brain and encounter are documents. Its script is a Luau file its enemy kind names (`script:` in the `.enemy.ron`, M3 B3), so no Rust names it.
- `crates/sapphire_gameplay/tests/bosses_are_content.rs` holds the three clauses: the name in no `.rs`, every file reached from the zone's document, and the adding commit's paths. The third reads git history. A shallow clone skips it, except on CI's Linux leg, which checks out the whole history (`fetch-depth: 0`) and fails rather than skips without it.
- The boss's figure and clips are first drafts nobody has judged (M3 C2 part 1).

```bash
cargo test -p sapphire_gameplay --test bosses_are_content
cargo xtask replay                      # the boss's replay, arena-boss, and the whole walk among them
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

**In the game:**

- Subtitles are drawn (M3 Q4): the HUD document's `subtitle` node shows the line at the playhead and its speaker, and the fight's HUD steps aside while the cutscene plays. `game-ward-gate-*` capture the warden's line through the game's flow in both languages at 4:3, 16:9 and 21:9.
- In the game's flow the court's threshold trigger starts the cutscene: `ward-whole-walk` watches it on its way to the boss, and `game_app`'s `plays_the_ward` skips it. `ward-court` still plays it at a fixed tick, as the sandbox's smaller pair.

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
- Title, new game and continue as the game's flow, and the pause and its return to title in a real run, are G1's: `game_app`'s `reaches_the_fight` goes from the title through a new game, and `plays_the_ward` continues a save, pauses and goes back to the title. The title's goldens are `game-title-*`.

## Exit test 5: all on five platforms

The test: Linux and Windows through CI on software rasterizers, macOS on Metal, and iOS Simulator and an Android emulator by the exit run. With no phone and no desktop GPU but this Mac's, **M3 can only close as "done, pending devices",** as M1 and M2 did ([m3-plan.md](m3-plan.md) §1).

**What proves it:**

| Platform | Proved by | Where |
|---|---|---|
| Linux | `Test`, `Replays`, `Golden images (vulkan-lavapipe)`, `GPU tests (vulkan-lavapipe)`, `App tests (vulkan-lavapipe)` | CI on every pull request |
| Windows | `Test`, `Replays`, `Golden images (dx12-warp)`; `GPU tests (dx12-warp)` on pushes to `main` | CI |
| macOS | `Test`, `Replays`, `Golden images (metal)`, and every `--ignored` lane here | CI's `macos-26`, and this Mac when GitHub's macOS runners are out |
| iOS | `Mobile build (iOS)` builds both apps for the device and the simulator | CI. Running is the exit run's, on a private Simulator |
| Android | `Mobile build (Android)` builds both APKs | CI. Running is the exit run's, on the emulator, replays included since M3 Z2b |

**On the iOS Simulator.** Use a private one, so `simctl install` doesn't kill another agent's app, and delete it after. Never open Simulator.app.

```bash
cargo xtask bundle --platform ios-sim --app game_app --release          # target/ios-sim/<Name>.app
UDID=$(xcrun simctl create z2-exit "iPhone 17 Pro")
xcrun simctl boot "$UDID" && xcrun simctl bootstatus "$UDID" -b
xcrun simctl install "$UDID" target/ios-sim/<Name>.app
xcrun simctl launch --console-pty "$UDID" com.example.sapphire.game --capture <png> --size 1280x720 --ticks 120
xcrun simctl io "$UDID" screenshot target/sim.png
xcrun simctl shutdown "$UDID" && xcrun simctl delete "$UDID"
```

Mobile bundles take `--release`: `--incremental` is for `--platform desktop` and `macos` only. The app can write only inside its own data container, so give `--capture` a path there: `xcrun simctl get_app_container "$UDID" com.example.sapphire.game data` names it, and its `tmp/` will do. A container's path changes with each install.

The sandbox plays a recorded zone replay to its checksum the same way. Bundle it with `cargo xtask bundle --platform ios-sim --release --features dev`, copy the replay and `tests/replays/checksums.aarch64.txt` into the sandbox's container, and launch `dev.sapphire.sandbox` with `--capture <png> --arena --replay <file>`, both paths in that container. The app logs `replay reached its recorded checksums` with the checksum. "Final run at 4fe253e6" has what ran.

**On the Android emulator** (the AVD `sapphire_api36`, headless):

```bash
~/Library/Android/sdk/emulator/emulator -avd sapphire_api36 -no-window -gpu host -no-audio -no-boot-anim &
adb wait-for-device
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home \
  cargo xtask bundle --platform android --app game_app --release
adb install -r <apk>
adb shell am start -n com.example.sapphire.game/.GameActivity --es sapphire.args <base64 JSON of the arguments>
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
- `game_app`'s `wins_the_ward` plays the whole walk (`ward-whole-walk`, exit test 1) through the game's flow on the stage, from the title's new game to the boss's defeat, once on Desktop-Low's settings and once on Mobile-Low's. Every step draws a frame unless `SAPPHIRE_WALK_STEPS_PER_FRAME` says otherwise (CI's does); it reads `first_use=0`, no step allocates, and the stage's run reaches the file's checksums.

```bash
cargo xtask content
cargo test -p sapphire_sandbox --features dev --locked --test replays -- --ignored --test-threads=1
cargo test -p game_app --features dev --locked --test reaches_the_fight -- --ignored
cargo test -p game_app --features dev --locked --test zone_trip -- --ignored
cargo test -p game_app --features dev --locked --test opens_the_guide -- --ignored
cargo test -p game_app --features dev --locked --test wins_the_ward -- --ignored
```

A walk by hand: `SAPPHIRE_CONTENT_DIR=target/content/desktop target/release/sapphire_sandbox --bench 240 --zone lantern_ward --zone-walk tour --tier mobile-low --size 640x360`. Read the `zone walk` row's `first_use=` and `rebuilt=`.

**CI's** `App tests (vulkan-lavapipe)` runs all of these, and it's CI's slowest job (about 75 minutes before `wins_the_ward`, which raised its limit from 90 minutes to 110, and to 150 once the final run's took 1 h 48 min). There `wins_the_ward` steps every step but draws one frame in eight (`SAPPHIRE_WALK_STEPS_PER_FRAME=8`), as lavapipe can't draw the walk's frames twice in the job's time; this Mac draws every one.

**Only a device:** a phone's driver compiles at its own pace. Cold, the load blocks 6–7 s on an M5, and the simulator built for 5.7 s and the emulator for 4.0 s on first launch (M3 S2).

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

**Budgets (M3 Z1).** §3.2's rows are 0.3 ms (Desktop) and 0.5 ms (Mobile) for script events and Luau's collector step, and 0.5 and 1.0 ms for streaming activation. Z1's fight bench plays a replay three times beside the renderer and logs §3.2's and §3.3's rows, memory across the walk and each trip's steps (spec §3.6, `apps/sapphire_sandbox/src/fight_bench.rs`). Each run, on a tier's own size and cook:

```bash
SAPPHIRE_CONTENT_DIR=target/content/<desktop|mobile> target/release/sapphire_sandbox --bench 1 \
  --tier <tier> --size <size> --arena --replay tests/replays/walk-the-ward.sreplay \
  --cues --hud live --blade on --time-of-day 10      # and --stream 64:48 or 32:48; zone-trip, arena-scripted
```

A measured run needs the machine to itself: nothing else building, a load average under 8 as it starts, and the load written beside each number. `hook_cost` (`sapphire_script`'s example, M3 L2) and `dialogue_bench` (Q2b) read the script's and a conversation's costs alone.

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
- `walk-the-ward` plays in no app, since its scenario isn't one a stage stands (exit test 1). The whole walk through the game's flow, `ward-whole-walk`, does: `game_app`'s `wins_the_ward`.
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

### Final run at 4fe253e6 (M3 Z2)

The runbook's "What the final run must redo" on 2026-11-10, on 4fe253e6, the head with all of M3: the chain, G1, Z1's fixes, Q4 and Z5e. An Apple M5 (4 performance and 6 efficiency cores, 10-core GPU, 24 GB), macOS 27.0.1, on AC power, release builds. Headless throughout: a private Simulator, the emulator with `-no-window`, the Mac offscreen. Every timed run had the machine to itself (the gate slots all held, and a load average under 8 before it started), and each figure has the load (1 minute) beside it. Nothing below moved a checksum or a golden.

**Before anything.** Wall times here and under tests 1 to 4 are at loads of 2.5 to 5.9. `cargo metadata --locked` passed. With `target/content/cache` removed, the desktop cook took 76 s and the mobile 15 s; the release sandbox and game 225 s. On iOS the game bundled in 139 s and the sandbox in 204 s; on Android in 141 s and 163 s.

**Exit test 1.** `cargo xtask replay`: 38 of 38 replays reach their committed aarch64 checksums with no allocation in a step (34 s). Among them `ward-whole-walk`, the whole walk from a new game to the Knell Colossus's defeat, 14,320 ticks to `8ea31d36ee8e5d22`, its step 9.6 µs on average and 34.0 at the longest; `walk-the-ward` (`e30e2e2b09cd0e58`), `arena-boss` (`52604d73e73c10e5`) and `zone-trip` (`05a575699a3c001a`). `zone_ward` 6 of 6. The sandbox's replay lane: 22 of 22 in 159 s, so `a_bench_of_a_fight_plays_it_whole_beside_the_renderer`, the rehearsal's failure, passes since Z1's fixes. CI's `Replays` passed on ubuntu-24.04, windows-2025 and macos-26 (run 42790087819); GitHub's macOS runners were back, so no stand-in.

**Exit test 2.** `bosses_are_content` 2 of 2 on this Mac, whose history is whole. On CI's `Test (ubuntu-24.04)`, with `fetch-depth: 0`, `the_commit_that_put_a_boss_in_its_zone_touched_only_content` passed; on CI's Linux leg a shallow clone fails that clause rather than skipping it, so the pass means the third clause ran.

**Exit test 3.** `ward-whole-walk` watches the cutscene from the court's threshold, and `ward-court-watched` and `ward-court-skipped` reach one trail (`b2cd3b39382446ce`), on the Mac, CI's three OSes, the Simulator and the emulator. `timelines` 14 of 14 and `cutscene_strings` 1 of 1. The subtitles in both languages: the `game-ward-gate-*` goldens at three aspects in two languages pass on Metal, lavapipe and WARP (run 42790087819). On lavapipe the English three match their references exactly and the Japanese three read a mean of 0.0024 against 0.01 allowed; WARP matches exactly.

**Exit test 4.** `game_ui`'s menus, dialogue and strings 24 of 24; `reaches_the_fight`, `zone_trip`, `opens_the_guide` and `plays_the_ward` pass, the last walking title, new game, continue, the pause and its return to title in a real run. The `game-title-*` goldens at three aspects in two languages pass on Metal, lavapipe and WARP.

**Exit tests 5 and 7, one checksum on three targets.** The Mac (Metal), a private iPhone 17 Pro Simulator (iOS 26.3, booted in 28 s, deleted after) and the emulator (`sapphire_api36`, Vulkan, headless, killed after), each with 0 first-use builds. Every Simulator and emulator run had the machine to itself, at a load of 2.7 to 4.0:

| Run | Mac | Simulator | Emulator |
|---|---|---|---|
| Sandbox `--capture … --arena --replay zone-trip` | `05a575699a3c001a` | the same, `prewarmed=58` | the same |
| … `ward-court-watched`, `ward-court-skipped` | `b2cd3b39382446ce` | the same, 61 | the same |
| … `arena-fight-won` | `d82aa8979fe8bc56` | the same, 57 | the same |
| … `arena-boss` | `52604d73e73c10e5` | the same, 57 | the same |
| … `walk-the-ward` | `e30e2e2b09cd0e58` | the same, 61 | the same |
| … `ward-whole-walk`, 14,320 ticks | `8ea31d36ee8e5d22`; its last frame has the Knell Colossus | the same, 61 | the same; `allocations=1`, below |
| Game `--new-game --ticks 600 --capture` | `9c8d3f44f62bdf27` | the same | the same |
| Game `--open-fight --script fight-back --ticks 1695 --capture` | `006faccf3917cf07`, `prewarmed=93` | the same, 77 | the same, 77 |
| Game `--new-game --script walk-the-ward --ticks 4298 --capture` | two runs end, at 2126 (`e910a17177b479a4`) and 2107 (`3ed79d8d86e62164`) | the same | the same |
| Game's title, `--capture … --ticks 120` | | draws; cold, the packs' prewarm 3,994 ms, warm 17.5 to 18.7 | draws; the prewarm 149 to 161 ms |

- The game's walk script only walks, so the player falls to the market's encounter, as in the rehearsal; the whole walk to the boss is `ward-whole-walk` and `wins_the_ward`.
- On the emulator the game in its window logs `the device refused the low-latency path; sound plays on its normal one` once, as Z2c left it, and every run ends `main returned status=0`.
- `wins_the_ward` (188 s): on Desktop-Low the whole walk through the flow wins at tick 14,261 after 14,824 frames drawn in 110.4 s, offscreen and unpaced, with `first_use=0`, no allocation in a step and the checksum `334e854f44c47650`; on Mobile-Low the same in 75.0 s. `reaches_the_fight` reads `prewarmed=83 first_use=0`.

**Exit test 8: durability.** `sapphire_save` 70 of 70, the kill and migration tests among them.

**Exit test 8: the bench,** as §3.6's: `--bench 1` on each tier's own size and cook, three rounds of the same loads, each run alone at a load of 2.2 to 5.7 (the walks 2.2 to 3.3). Each figure is the least across rounds; **worst** is the least of the rounds' worst frames. ms, p50 / p99 / worst, on the walk unless named, the CPU rows from the bench's `cpu` phase and the GPU rows from its `gpu` phase, with §3.2's and §3.3's budget in brackets. **Bold is over.**

| Row | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| CPU frame | 1.835 / 2.317 / 2.505 [8] | 0.631 / 2.344 / 3.235 [12] | 0.480 / 1.538 / 2.968 [12] | 0.837 / 1.549 / 1.718 [22] |
| Game side | 1.377 / 1.669 / 1.835 | 0.086 / 1.777 / 2.239 [6] | 0.060 / 1.071 / 1.616 | 0.581 / 1.022 / 1.324 [13.6] |
| Simulation, a frame's steps | 0.019 / 0.044 / 0.091 | 0.020 / 0.044 / 0.083 [1] | 0.013 / 0.028 / 0.080 | 0.012 / 0.029 / 0.059 [3] |
| Physics, inside it | 0.002 / 0.005 / 0.020 | 0.002 / 0.005 / 0.018 [0.7] | 0.001 / 0.003 / 0.009 | 0.002 / 0.004 / 0.009 [2.4] |
| Animation + IK | 0.010 / 0.022 / 0.065 | 0.011 / 0.021 / 0.062 [1.5] | 0.008 / 0.017 / 0.060 | 0.007 / 0.014 / 0.062 [3] |
| Secondary motion | 1.313 / 1.596 / 1.769 | 0.021 / **1.717 / 2.046** [0.5] | 0.013 / 1.022 / 1.385 | 0.547 / 0.970 / **1.266** [1] |
| VFX simulation | 0.001 / 0.002 / 0.004 | 0.001 / 0.002 / 0.007 [0.3] | 0.001 / 0.001 / 0.006 | 0.001 / 0.001 / 0.003 [0.6] |
| **Script events + Luau GC** | 0.003 / 0.011 / 0.028 | 0.003 / 0.011 / 0.028 [0.3] | 0.003 / 0.009 / 0.025 | 0.003 / 0.008 / 0.014 [0.5] |
| … on L3's scripted fight | | 0.003 / 0.013 / 0.022 [0.3] | | 0.004 / 0.011 / 0.014 [0.5] |
| **Streaming activation**, the tier's radius | 0.000 / 0.001 / 0.007 | 0.000 / 0.001 / 0.006 [0.5] | 0.000 / 0.001 / 0.001 | 0.000 / 0.001 / 0.001 [1] |
| … with a 48 m radius | | 0.000 / 0.002 / 0.015 [0.5] | | 0.000 / 0.003 / 0.011 [1] |
| Audio update | 0.000 / 0.002 / 0.004 | 0.001 / 0.002 / 0.004 [0.2] | 0.000 / 0.001 / 0.003 | 0.000 / 0.001 / 0.001 [0.3] |
| Extract | 0.007 / 0.018 / 0.055 | 0.007 / 0.017 / 0.046 [0.7] | 0.004 / 0.011 / 0.051 | 0.004 / 0.009 / 0.024 [1.2] |
| Render prepare + record | 0.432 / 0.827 / 0.944 | 0.430 / 0.819 / 1.323 [4] | 0.348 / 0.722 / 1.333 | 0.283 / 0.623 / 0.757 [8] |
| GPU frame | 8.243 / 9.142 / 9.595 [12] | 4.760 / 5.724 / 6.023 [14] | 2.486 / 3.134 / 3.433 [13] | 1.278 / 2.110 / 2.342 [26] |
| Main pass | 3.886 / 4.560 / 4.748 | 2.245 / 2.855 / 2.979 [4.9] | 1.464 / 2.028 / 2.174 | 0.801 / 1.550 / 1.743 [11.5] |
| Shadows | | 0.365 / 0.712 / 1.101 [2] | | |
| Clustering and AO | | 0.616 / 1.024 / 1.126 [1.5] | | |
| Prepasses | | 0.222 / 0.540 / 0.956 [1.5] | | |
| TAA | 1.032 / 1.425 / 1.493 | 0.594 / **0.975 / 1.057** [0.8] | none | none |
| Post | 0.848 / 1.233 / 1.310 | 0.488 / **0.862 / 0.912** [0.7] | 0.294 / 0.641 / 0.737 | 0.190 / 0.540 / 0.600 [2.5] |
| UI | 0.015 / 0.040 / 0.323 | 0.017 / 0.021 / **0.312** [0.3] | 0.015 / 0.017 / 0.283 | 0.013 / 0.125 / 0.282 [1] |

- **§3.2's rows hold:** the script row reads on the walk now that the ward's scripts are in it, at 0.011 ms p99 against 0.3 and 0.5; streaming at 0.001 p99, and at 0.002 to 0.003 with a 48 m radius where cells come and go, against 0.5 and 1. The worst frame of either is 0.028 ms.
- **The GPU frame reads on Metal** on every timed frame of every walk (4,297 of 4,298 on the full-rate tiers, 2,148 of 2,149 on Mobile-Low), and within §3.3's frame on every tier.
- **What's over:**
  - **Secondary motion's p99, a solve frame's.** Desktop-Low solves clearance every third tick and the phones every fourth (§5's springs, "Clearance by tier and distance"), so p50 is a held frame and p99 a solving one. On every load Desktop-Low's p50 is at most 0.024 ms and its p99 1.42 to 1.82 against 0.5. Mobile-Low's p99 is 0.91 to 0.99 against 1 on the walk, streaming and the scripted fight, and 1.10 to 1.18 on `arena-fight-won`, `arena-boss` and `zone-trip`; its worst passes 1 on every load. Its p50 on the walk is 0.55 to 0.59, since a 30 Hz frame holds two steps and every other frame holds a solve; on the fights 0.024.
  - **Desktop-Low's TAA and post at p99,** by 0.18 and 0.16 ms, and UI's worst frame by 0.01.
  - Nothing else: the rehearsal's one-off CPU frames of 26 to 58 ms aren't there on a quiet machine, where the worst CPU frame is 3.2 ms.
- **Allocations:** a frame on the walk 764, 760, 637 and 506 by tier. In fixed steps: none after the first play on any load; in the first play's tick 2, 6 on the walk, 167 on `arena-scripted` and 83 on `arena-boss` (each fight's start, once); on `zone-trip` none at all, since Z1's fixes took the trip's copy out of the step. `post_first_use=0` on every run.

**Memory across the walk,** start / most / end in MB, the range across rounds; the engine's own accounting after every frame, the process's footprint as `sapphire_platform::process_memory` reads it, and the OS's most for the process:

| | Desktop-High | Desktop-Low | Mobile-High | Mobile-Low |
|---|---|---|---|---|
| Engine resident | 60.9, flat | 60.9, flat | 48.9, flat | 48.9, flat |
| Heap | 63.3 | 63.2 | 59.3 | 48.0 |
| Process footprint | 729–739 / 785–789 / 776–780 | 550–557 / 608–615 / 596–604 | 408–428 / 431–449 / 416–432 | 368–376 / 400–415 / 393–410 |
| The OS's most | 805–814 | 625–632 | 443–460 | 427–430 |

- **Nothing grows across the walk.** With a 48 m radius the ward comes in as the walk reaches it: resident 59.2 to 60.9 MB by tick 3,419 on Desktop-Low, 48.0 to 48.9 by 3,428 on Mobile-Low, and held to the end; the OS's most 604 to 617 and 404 to 410.

**A trip's time,** on `zone-trip`, five trips a play, from the frame its exit was taken (load 2.9 to 5.7). Frames are what a player sees; the bench's ms are unpaced:

| Step | Desktop-Low | Mobile-Low |
|---|---|---|
| The wipe covers | 34 frames | 18 |
| The swap | 35 | 19 |
| Its first cells placed | 36 | 20 |
| The wipe gone | 69, at 241–289 ms | 37, at 43.6–45.8 ms |

**CI's half,** run 42790087819 on 4fe253e6: `Test` and `Replays` on all three OSes, `Lint`, both mobile builds, `GPU tests (vulkan-lavapipe)` and every golden shard (Metal, lavapipe 1–2 of 2, WARP 1–6 of 6) passed. `App tests (vulkan-lavapipe)` passed too, `wins_the_ward` among its lanes, in 1 h 48 min against its 110-minute limit. `GPU tests (dx12-warp)` passed on this head as a dispatched run (42795020801).

**Noticed, not fixed:**

- **`ward-whole-walk` on the emulator reports `allocations=1`,** where the Mac and the Simulator report 0, at the same checksum. Android's Rust target has emulated TLS, so `sapphire_core::alloc` counts process-wide there and a scope's count is an upper bound that other threads' allocations can raise (its module doc says so). Over 14,320 ticks one allocation by another thread is plausible, so it isn't evidence of an allocation in the step. Desktop CI and this Mac hold the zero.
- **Mobile-Low's secondary motion p99 is 0.97 to 0.99 ms on the walk against 1,** on an M5. A reference phone will be over.
- The new game's capture at tick 600, at the Gate Square, shows the HUD's "Skeleton Warrior" and "Objective: Defeat the enemy" with no enemy in view.

**Still owed:** the reference phones and a desktop GPU other than this Mac's (exit tests 5 and 8); a person in the editor's window (6); a reader of Japanese (3 and 4); and anyone playing the slice.

## What the final run must redo

**Done at 4fe253e6 on 2026-11-10:** "Final run at 4fe253e6", above, has each item's result. Exit test 8's figures stay there, with a pointer from spec §3.6, the zone walk's section.

The rehearsal ran on Q2b's head, before the chain, G1 and Z1. The final run is on the head after all three land, and it takes again:

- **Before anything,** on that head: the lock check and a fresh cook. Remove `target/content/cache` first, since the chain renumbers cooked kinds.
- **Exit test 1:** `cargo xtask replay` and the sandbox's replay lane, with the boss's replay and the replays G1 records through its flow among them. Then the walk from the title to the boss and back, through the game's flow. CI's `Replays` on all three OSes on that head: a macOS job if GitHub's runners are back, this Mac's stand-in if not.
- **Exit test 2:** `tests/bosses_are_content.rs` on CI's Linux job with the full history, so its third clause runs rather than skips.
- **Exit test 3:** the cutscene from the court's threshold in the ward's walk, not at a fixed tick. Its subtitles on screen in both languages, by the `game-ward-gate-*` goldens (M3 Q4), with CI's lavapipe and WARP.
- **Exit test 4:** title, new game, continue, the pause and its return to title in a real run, and the title's goldens (`game-title-*`, with CI's lavapipe and WARP).
- **Exit test 5:** the whole rehearsal again on the final head, on a fresh private Simulator and the emulator. The replays this time on both, the walk G1 records and the boss's among them. Add G1's flow to it: `--game new-game` to a capture on both.
- **Exit test 7:** the app lanes, `wins_the_ward` among them: the whole walk through the flow reading 0 first-use builds on Desktop-Low and Mobile-Low.
- **Exit test 8:** Z1's bench on the ward's walk on Desktop-Low and Mobile-Low, §3.2's script and streaming rows, memory across the walk and a transition's time, into spec §3.5's table. Note the load average beside each number. Run it after the chain and G1, since both change what a step runs.
- **The record:** the table at the top, each test's "Where it stands", and an "As built" line for Z2 in [m3-plan.md](m3-plan.md).

Exit test 6 has nothing to redo unless the editor changes.
