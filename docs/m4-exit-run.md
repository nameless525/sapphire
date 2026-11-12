# The M4 exit run

M4 ends when its eight exit tests are answered ([m4-plan.md](m4-plan.md) §1): no shader hitches, patches that move only changed blocks, packages each store's local validators pass, the look at every tier and rung, readable crashes, combat more people can play, recipes with no Rust, and a playtest one command away. This file is the procedure for all eight, for someone coming back to it cold. For each test it says what proves it, the commands, where each runs (Metal on this Mac, the iOS Simulator, the Android emulator, CI's lavapipe and WARP jobs, the nightly `packages.yml`), what passing looks like, and what stays owed in §1's terms.

The run has no phone, no Windows or Linux machine, no desktop GPU but this Mac's and no account anywhere, and it may not open a window, sign in, upload or publish. Every clause below that needs one of those is owed, and says so. **Nobody has played the slice,** and no test here asks whether it's fun (§1, "Feel stays unknown").

"Final run at 83dc6273", near the end, is the run on the head with all of M4 (2026-11-12), "Rehearsal at d8b621cc" what ran before the last packages (H4, R1, Z1 and the reviews) landed, and "What stays owed" what no run here can answer.

| Exit test | Who answers it | Needs | Where it stands |
|---|---|---|---|
| 1. No shader hitches | This Mac cold and warm, CI by counts, the Simulator and the emulator by counts | A phone's cold compile, a desktop GPU's driver cache, a store install's first launch | Pass for what's built: the scripted playthrough builds nothing on the frame, and timed from a cold Metal cache no load screen's frame passes 33.3 ms; the boot on a cold Simulator and a cold emulator holds the counts at 0, though the emulator's boot screen stalls while it warms |
| 2. Patches move only changed blocks | This Mac, the Simulator and the emulator against a server on this Mac | A real CDN, the release key, TLS on each OS's trust store | Pass: exactly the changed blocks on this Mac, the Simulator and the emulator, and the launch that installs the patch stands on each |
| 3. Store packages | This Mac (Apple, Android) and the nightly `packages.yml` (all five) | The maintainer's accounts: signing, uploads, notarization, review | Pass for what's local: every validator here and in the head's `packages.yml` passes; the first scheduled run on `main` and distribution are owed |
| 4. The look at every tier, scale and rung | Metal here, lavapipe and WARP in CI, the emulator's override | A phone throttled after 15 minutes | Pass: the face, the ward and the hit at every tier, floor and rung (61 scenes), `look_tiers`, the traces and the emulator's override |
| 5. Every crash is readable | macOS here, Windows and Linux in CI, the Simulator, the emulator | A crash on a device; the stores' own crash reports | Pass: read to the line on macOS, the Simulator and the emulator; Windows and Linux by CI's crash lane |
| 6. Combat more people can play | CI and this Mac | Players who need the settings; a reader of Japanese | Pass: every test holds, the scripted fight's flashes are a lane (nightly in CI), and both fights pass at both settings, the stress fight at default on the limit |
| 7. Still authorable | This Mac | Nothing | Pass: a screen, a dialogue, a sound and a screen's goldens with no Rust; the remap glyphs are data now |
| 8. Ready to play | This Mac, the Simulator, the emulator | Someone playing | Pass: sessions on the desktop, the Simulator and the emulator replay to their checksums and report |

**The shortest path:** "Before anything", then the two `game_app` lanes under exit test 1, `cargo xtask patch rehearse` (exit test 2) and `cargo xtask package --platform macos` (exit tests 3 and 5). That's the desktop half of tests 1, 2, 3 and 5 in about an hour once built. The Simulator's and the emulator's runs and the nightly packages are the rest.

## Before anything

```bash
cargo metadata --locked --format-version 1 > /dev/null   # xtask isn't --locked: catch a stale Cargo.lock first
cargo xtask content                                       # the desktop cook, into target/content/desktop
cargo xtask content --cook mobile                         # the phones' cook, for the bundles below
df -h /                                                   # five shipping packages take tens of gigabytes
```

A worktree's `target/content` goes stale after a rebase onto new timelines, moves or documents: the app lanes then fail with "no cooked timeline has the simulation's timeline …". Cook again. After a cooked format changes, remove `target/content/cache` first.

Commands below are run from the repository's root on a Mac with Xcode 26, the Android SDK (`~/Library/Android/sdk`, NDK 29.0.14206865, platform 36, the AVD `sapphire_api36`) and Java 17 (`JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home` for anything that runs Gradle). Nothing here opens a window: the Mac runs offscreen (`--capture`), the Simulator is a private one booted with `simctl` and never Simulator.app, and the emulator runs `-no-window`. Keep one simulator or emulator up at a time.

**Timed runs need the machine to themselves:** nothing else building, a load average under 8 as they start, and the load written beside each number. Z1 measures the shipping numbers (binary sizes, cold and warm starts to the title, warm-up per tier, memory, patch sizes for P0's edits and the ladder's trace under the emulator's override) into spec §3's "Measured as shipped"; this run cites them rather than measuring twice.

### The Simulator and the emulator

The device runs below share these steps. A private Simulator, so `simctl install` doesn't kill another app, deleted after; a fresh one has a cold Metal cache:

```bash
UDID=$(xcrun simctl create z2-exit "iPhone 17 Pro")
xcrun simctl boot "$UDID" && xcrun simctl bootstatus "$UDID" -b
xcrun simctl install "$UDID" <app>
DATA=$(xcrun simctl get_app_container "$UDID" com.example.sapphire.game data)   # captures and saves go under it
xcrun simctl launch "$UDID" com.example.sapphire.game <args>                   # prints the pid
xcrun simctl spawn "$UDID" log show --predicate 'processID == <pid>' --info    # the game's lines
xcrun simctl shutdown "$UDID" && xcrun simctl delete "$UDID"
```

The emulator, headless:

```bash
~/Library/Android/sdk/emulator/emulator -avd sapphire_api36 -no-window -gpu host -no-audio -no-boot-anim -no-snapshot-save &
adb wait-for-device && adb shell 'while [ "$(getprop sys.boot_completed)" != 1 ]; do sleep 1; done'
adb uninstall com.example.sapphire.game; adb install <apk>   # uninstalled first, the game's pipeline cache is cold
adb shell am start -n com.example.sapphire.game/.GameActivity --es sapphire.args <base64 of a JSON list of arguments>
adb logcat -d | grep -E 'booted|captured|main returned'
adb emu kill
```

The game's bundle ID and Android package are both `com.example.sapphire.game`, for every build. Its `dev` bundle, its playtest build and `xtask run`'s build all land in `target/ios-sim/` or `target/android/`, each over the last: copy a bundle aside before building the next. A relative `--capture` path goes under the app's `cache/` (read it back with `adb exec-out run-as com.example.sapphire.game cat cache/<file>`). Mobile bundles take `--release`. `cargo xtask run --platform android` streams logcat until the app ends, so wrap it in a time limit. Android keeps the process cached after the game's main returns, so wait for `main returned status=` in logcat, not for the process to go. [m3-exit-run.md](m3-exit-run.md), "Exit test 5", has the rest of both recipes.

## Exit test 1: no shader hitches

The test (§1): a scripted playthrough from empty pipeline caches (the title and a new game; the Lantern Ward's whole walk with its cutscene, conversation, encounters and boss; a language switch and every setting that changes what's drawn; a tier change at a pause and a forced walk down and back up the thermal ladder; back to the title) builds no pipeline on the frame's thread after the boot screen's first frame, and counts no first use. On Metal from a cold cache, no frame of a load screen takes more than twice the tier's interval.

**What proves it:**

- **The counts** are H1's: `PipelineStats`' `first_use_builds`, `frame_thread_builds` and `frame_waits` (spec §5.17), counted from the boot screen's first frame (`Renderer::count_hitches_from_now`, H2). A bench fails on any of them.
- **The boot** (H2): `game_app` boots to a screen drawn by the interface's one pipeline while everything else warms off the frame, and logs `booted` with `frames`, `longest_frame_ms`, `longest_interval_ms`, `intervals_over` (frames past `interval_limit_ms`, twice the tier's interval), `prewarmed`, `warmed_off_frame`, `first_use`, `frame_thread` and `frame_waits`. A capture then logs `the title and the game's frames after the boot` with `frame_thread` and `frame_waits`.
- **Every setting at every rung** (H3): `game_app`'s `declares_every_setting` stands each shipped tier, holds each depth of its ladder that sheds in place, and steps every row of `menus.ron` round all its values, failing on any first use or frame-thread build with the tier, rung, setting and pipeline named.
- **A tier change and the ladder** (T3): `game_app`'s `follows_the_ladder` forces critical heat on `zone-trip` from Desktop-High: the ladder goes down on the first frame, its 30 fps cap paces, Desktop-Low is taken only under the wipe, and nothing builds on the frame.
- **The whole playthrough** (H4): `game_app`'s `zero_hitches` boots the game as its window does and plays §1's walk in one run: the title and a new game, the ward's whole walk (`ward-whole-walk.sreplay`), at the gate cutscene's first line a pause where every setting is pressed round its values (the language among them), the tier changed and changed back, and heat forced to critical down the ladder and back; then the boss's defeat and the wipe back to the title. It fails on any first use, frame-thread build or wait after the boot screen's first frame, naming where, then shows its own check failing on a debug line the game never declares. It times each load screen's frames (the boot screen, the wipe) against twice the tier's interval, and with `SAPPHIRE_HITCH_TIMED=1` a frame over fails it. It reports memory per part of the walk (Z1).
- **Every golden** fails on a first use in its captured frame (`sapphire_testkit`'s scene runner), so the `game-boot-*` scenes and the rest hold the counts on Metal, lavapipe and WARP.

```bash
cargo test -p game_app --features dev --locked --test declares_every_setting -- --ignored
cargo test -p game_app --features dev --locked --test follows_the_ladder -- --ignored
cargo test -p game_app --features dev --locked --test reaches_the_fight -- --ignored     # title to a fight, first_use=0
cargo xtask run --platform desktop --app game_app --release -- --capture target/z2/title.png --size 1280x720 --ticks 120
cargo test -p game_app --features dev --locked --test zero_hitches -- --ignored          # the playthrough, untimed
```

**Timed, from a cold Metal cache:** build the lane for release, put its binary in a throwaway `.app` with a fresh bundle ID, and exec it twice (cold, then warm), alone on the machine (`slot --all --load 8`):

```bash
cargo test -p game_app --features dev --release --locked --test zero_hitches --no-run   # prints target/release/deps/zero_hitches-<hash>
mkdir -p <abs>/zh.app/Contents/MacOS && cp target/release/deps/zero_hitches-<hash> <abs>/zh.app/Contents/MacOS/zero_hitches
# <abs>/zh.app/Contents/Info.plist: CFBundleIdentifier a new name each run, CFBundleExecutable zero_hitches, CFBundlePackageType APPL
SAPPHIRE_HITCH_TIMED=1 SAPPHIRE_HITCH_SIZE=1280x720 <abs>/zh.app/Contents/MacOS/zero_hitches --ignored   # cold; again for warm
```

The `game_app` lanes are `harness = false`: they print no totals, and a pass is exit 0. **Passing:** each lane exits 0; the `booted` line reads `first_use=0 frame_thread=0 frame_waits=0`; `zero_hitches` prints "first use 0, on the frame's thread 0, waited for 0", "the check fails as it should", and each load screen's worst frame under its limit, and timed, exits 0.

**A cold Metal cache** needs a fresh bundle ID: a throwaway `.app` with a new identifier, exec'd directly (never `open`ed). A copied bare binary shares the content-keyed cache and isn't cold (running.md's "Cold and warm Metal caches"). H4 split the boot's stand into five parts, so no boot frame passes the limit (m4-plan's H4 row); Z1 measured cold and warm starts to the title as shipped (spec §3.7).

**CI:** `App tests (vulkan-lavapipe)` runs `reaches_the_fight`, `zone_trip`, `opens_the_guide`, `plays_the_ward`, `wins_the_ward` and `comes_back`, and `declares_every_setting` on Desktop-Low and on Mobile-Low (`SAPPHIRE_WALK_TIERS`), and since X5 `follows_the_ladder`, `draws_a_marked_tick` and `zero_hitches` on Desktop-Low (one frame in eight drawn, at 160×90), every one holding the counts at 0. WARP holds them through every golden and through the nightly Windows package's title capture (`frame_thread=0 frame_waits=0`). Only this Mac times the load screens.

**The Simulator and the emulator, by counts:** the `dev` game bundle's `--capture` boot on each, read from its `booted` line. A fresh Simulator's Metal cache is cold; the emulator's frames slow while anything builds (H0), so it proves counts, never times. Uninstalled before the install, its pipeline cache is cold too.

```bash
cargo xtask bundle --platform ios-sim --app game_app --release --features dev
cargo xtask bundle --platform android --app game_app --release --features dev
# Simulator: launch with --capture "$DATA/tmp/title.png" --size 1280x720 --ticks 120, then read its `booted` line
# Emulator: am start with ["--capture","title.png","--size","1280x720","--ticks","120"], then logcat
```

**Owed:** a phone driver's cold compile, a desktop GPU's driver cache, and a store-installed build's first launch.

## Exit test 2: patches download only changed blocks

The test (§1): for each of a fixed set of edits (a texture, a mesh, a sound, a string, a screen and a zone's look), a patch from the last release's install downloads exactly the new manifest, the changed packs' tables and the blocks whose hashes are new, counted by a localhost server. A killed download resumes; a corrupted, older or expired one is refused; the game starts on the old version or the new, never a mix.

**What proves it** (P0 to P4, spec §5.10 and `sapphire_patch`):

- **The bytes, per edit:** `sapphire_patch`'s `tests/download.rs` builds two releases of real packs with a texture's blocks edited, a mesh grown, an asset added to a zone, a new pack, a string, a screen and (since X5) a sound, and holds what crosses to the manifest, the tables and the new blocks exactly, as the host's log counts them. On real cooks, P0 measured its edits ([p0-spike.md](../research_notes/Ship%20readiness%20for%20M4/p0-spike.md)); `cargo xtask patch sizes <name>=<content dir>...` (Z1) patches release 1 with each edited copy of `content/` and reports each download by kind, and spec §3.7 has Z1's numbers as shipped.
- **Refusals:** `tests/download.rs` refuses an older, expired, wrongly signed, byte-flipped or oversized manifest before any table or block is asked for, and a host that drops, truncates, corrupts, ignores ranges or stalls. `tests/tls.rs` shows the platform verifier refusing a certificate it shouldn't trust.
- **Kills:** `tests/kill.rs` kills a downloading child after two to seven requests; `tests/install_kill.rs` kills an installing child at random, 120 rounds. `tests/install.rs` faults every write, flush, rename and removal of an install (196 faulted installs): after each the app starts on exactly one whole release.
- **In the game, end to end:** `cargo xtask patch rehearse` (P4). Release 1 of the workspace's cook and release 2 with one screen edit, both signed with the test seed; a `dev` game installed with release 1 checks, downloads and installs in its boot and draws release 2's title byte for byte; launched again it fetches the manifest alone; a release signed by an untrusted key is refused and release 1 drawn; killed at a stalled block, the next launch fetches only what wasn't staged whole. It writes `target/patch-rehearsal/report.txt`.

```bash
cargo test -p sapphire_patch --locked                   # download, install, kill, install_kill, tls
cargo xtask patch rehearse                              # about 6 minutes; target/patch-rehearsal/report.txt
```

**Passing:** `patch rehearsal passed`, and its report shows exactly four requests for the patch: the signed manifest, the `ui` pack's 256-byte header and its table, and one range of blocks (1,545 bytes, the edited screen). The table grows with the `ui` pack: 1,536 bytes at P4, 2,304 at d8b621cc, 2,432 at 83dc6273. Then the relaunch fetches the manifest alone, the untrusted release is refused at its manifest, and the resumed download fetches no header or table again.

**The Simulator and the emulator** patch the same edit from a server on this Mac. A `dev` bundle ships the unsigned mobile cook (version 0); release 1 of the mobile cook carries the edit, signed with the test seed (`TEST_SEED` in `xtask/src/patch.rs`), and `patch serve` hosts it:

```bash
# with content/ui/title.ui.ron's tagline edited from 18 to 20 points, after the bundles are built; then put it back and cook again
SAPPHIRE_RELEASE_KEY=<the test seed> cargo xtask release --cook mobile --out target/z2/mobile-host \
  --release-version 1 --expires 2099-01-01 --min-build 0
cargo xtask patch serve target/z2/mobile-host --port 8475      # logs each request's bytes
adb reverse tcp:8475 tcp:8475                                 # the emulator; the Simulator reaches 127.0.0.1 itself
# the game, twice: --capture <png> --patch-dir <dir> --update-host http://127.0.0.1:8475/
```

On the Simulator `<dir>` is under `$DATA/tmp/`; on the emulator a relative `patch` lands in the app's `cache/`, as every relative path on a phone's command line does (H4, Z3e's `Options::placed`). **Passing:** the first launch fetches the manifest, the `ui` pack's header and table and one range of blocks, its `booted` line reads `updated=true` and `update_received=` those bytes, and its capture differs from the unpatched title; the second fetches the manifest alone and reads `updated=false`.

**CI:** `sapphire_patch`'s tests in `Test` on all three OSes. The rehearsal lane and the device runs are this Mac's.

**Owed:** a real CDN, the maintainer's release key and host in `platform/patch.toml` (§7), and TLS against a real certificate on each OS's trust store.

## Exit test 3: builds pass TestFlight, Play internal testing and Steam review

The test (§1): a shipping package for each store (iOS's `.xcarchive`, Android's AAB, Steam's depots for Windows, Linux and macOS) passes every validator that runs without an account, and a checklist per store gives each rule's source and the test or command that meets it.

**What proves it:**

- `cargo xtask package --platform <p>` (S1) runs each platform's checks inside the command and fails the package on any of them, keeping their output in the package's `checks/`:
  - **iOS** (S2): `plutil -lint` on the archive's seven plists, `codesign --verify --strict --deep`, the archive's layout, the dSYM's UUID equal to the executable's, `atos` finding lines for SDL, Luau and the game, and the privacy manifest's scan of required-reason APIs.
  - **Android** (S3): release lint failing on any error, `bundletool validate`, the bundle's entries, the library's 16 KB segments, `build-apks --local-testing` and its size under Play's cap, `zipalign -c -P 16`, `aapt2 dump badging` against what xtask built, and the signer.
  - **Steam's desktops** (S4): macOS's hardened runtime and empty entitlements read back and its `.icns`; Windows' subsystem, manifest, VERSIONINFO and icon read back from the PE, and its imports holding no C runtime DLL (X4 links it statically); Linux's library and symbol versions held to Steam Linux Runtime 3.0's.
  - Every package (C1, C2): every shipped binary stripped, its ID equal to its symbols', and no file holding the builder's paths.
- **The checklists:** [docs/store/apple.md](store/apple.md), [google-play.md](store/google-play.md) and [steam.md](store/steam.md) (S5), each rule with its source and date and the test or command that meets it, or "owed".
- **The nightly packages** (I2, `.github/workflows/packages.yml`): one job per platform runs the same `package`, then `cargo xtask symbols`, and keeps the package and its symbol store as artifacts. On `main` (the schedule) a package is a release's and fails on a changed checkout; anything else is `--dev`.

```bash
cargo xtask package --platform macos --dev          # each takes a plain gate slot: q -t 3600 cargo xtask package …
cargo xtask package --platform ios --dev
cargo xtask package --platform android --dev        # JAVA_HOME as above; Gradle runs --offline
cargo xtask steampipe                               # SteamPipe's scripts from the desktop packages there
gh run list --workflow packages.yml                 # then gh run view <id> for the five jobs
```

`--dev` is for a checkout off `main` or with any change; on a clean `main` drop it. Windows and Linux package only on their hosts, so they're the nightly job's. **Passing:** each `package` exits 0 and writes `package.json`; the nightly run's five jobs are green.

**Owed:** distribution signing, the uploads, notarization, and each store's server-side checks and review, all needing the maintainer's accounts (§7, and each checklist's "Owed to the maintainer"). Apple's `xcodebuild -exportArchive` stops at signing (S2).

## Exit test 4: the look at every tier, scale and rung

The test (§1): the character's close-up, the ward and a hit's post get goldens at each tier, at each tier's dynamic floor and at each rung of the thermal ladder; a GPU test holds outline widths in output pixels and the shading's two-tone edge; the ladder walks down and back up without oscillating or hitching, on modelled traces and under the emulator's thermal override.

**What proves it:**

- **Goldens** (T0, T2, T5, T6): the 61 `tier-*` scenes, on Metal, lavapipe and WARP: the face, the ward and the hit at every tier, at its dynamic floor and at each rung that sheds (a frame-rate rung draws as the rung before it, and a tier rung lands on the lower tier's scenes), and the face at Mobile-Low's three-quarters. Below scale 1 outlines stay whole (T6, `whole_line`).
- **The GPU test** `look_tiers` (T0, T2): `lines_and_two_tone_edges_hold_at_every_tier_and_scale` measures lines and the hard edge in output pixels at each tier and scale against scale 1, and `the_ladder_walks_down_and_back_without_building_on_the_frame` walks each tier's ladder on one renderer.
- **Traces** (T2): `sapphire_render::quality::ladder`'s tests (heating, recovery, heat that comes and goes, a closed-loop dynamic-resolution trace, a lowered tier's climb back, forced depths).
- **The emulator's override:** the game in its window follows `adb shell cmd thermalservice override-status <n>`, logging each step as `thermal ladder`; `reset` ends it.
- `follows_the_ladder` and `declares_every_setting` (exit test 1) hold the rungs to building nothing.

```bash
cargo xtask golden --scene tier-face-mobile-low-rung-two          # one exact name a run; all 61 for the whole set
cargo test -p sapphire_render --test gpu --locked -- --ignored --test-threads=1 look_tiers::
cargo test -p sapphire_render --locked --lib quality::ladder
adb shell cmd thermalservice override-status 3; adb logcat -d | grep 'thermal ladder'; adb shell cmd thermalservice reset
```

**Passing:** every scene `ok` within the gates (goldens.md), both GPU tests and every trace test pass, and on the emulator the ladder goes down at once and climbs back a rung at a time after its holds (10 s, 60 s for the cap and a tier), as T2's run did.

**CI:** the goldens in `Golden images` (Metal, lavapipe in three shards, WARP in six), `look_tiers` in `GPU tests (vulkan-lavapipe)` and on pushes to `main` in `GPU tests (dx12-warp)`, the traces in `Test`.

**Owed:** a phone thermally throttled after 15 minutes (M2's exit test 4, still open). Z1 records the ladder's trace under the emulator's override.

## Exit test 5: every crash is readable

The test (§1): a forced crash in a shipping build symbolicates offline to its function, file and line, from symbols kept from the same link: on macOS here, on Windows and Linux in CI, on the Android emulator from its signal record and tombstone, and on the iOS Simulator from its panic report. Every shipped binary is stripped, and its ID matches its symbols.

**What proves it** (C1, C2, spec §5.2):

- **Stripped, one ID:** `xtask package` fails unless each shipped binary is stripped and carries its symbols' ID (`sapphire_symbols check`), and records each module in `package.json`'s `modules`.
- **A forced crash:** `--crash fault|panic` on the command line (desktop and iOS), or on Android `adb shell setprop debug.sapphire.crash fault`, which a store build reads. `forced_fault` writes to 0x10.
- **Reading it:** `cargo xtask symbolicate <report>...` reads a minidump, a panic report, Android's signal record and its tombstone with `target/package/*`'s symbols, writing each module's `.sym` into `target/symbols/` as it needs it.
- **Windows and Linux in CI:** `tools/sapphire_symbols/tests/crash.rs` crashes a copy of itself under the real monitor and reads its minidump and panic report against its own stripped copy and symbols, on all three desktops (it runs where `CI` is set, or with `SAPPHIRE_CRASH_TESTS=1`). The nightly Windows and Linux packages hold the stripping and IDs of the real game.

```bash
cargo xtask package --platform macos --dev
HOME=<a private directory> target/package/macos/<Name>.app/Contents/MacOS/<executable> --crash fault   # and panic
cargo xtask symbolicate $(find <that directory> -name '*.dmp' -o -name '*.panic-*.txt')
SAPPHIRE_CRASH_TESTS=1 cargo test -p sapphire_symbols --test crash
```

The private `HOME` keeps the crash reports out of the user's own directory; they land in its `Library/Caches/Sapphire/Sapphire/crashes/`. **Passing:** the minidump's first frame names `sapphire_crash::forced::forced_fault` at `crates/sapphire_crash/src/forced.rs` and its callers, `ForcedCrash::now` and `game_app::startup`; the panic report names `forced_panic`.

**The Simulator:** `cargo xtask package --platform ios-sim --dev`, install its app on a private Simulator, launch with `--crash panic`, copy the panic report out of `$DATA`, then `symbolicate` it. **The emulator:** `cargo xtask package --platform android --dev`, install the arm64 splits of `checks/local-testing.apks` (`adb install-multiple` of `splits/base-master.apk` and `splits/base-arm64_v8a.apk`, one variant's pair: the archive holds two variants, the second's names ending `_2`), `setprop debug.sapphire.crash fault`, start the game; clear the property and start it again, so the activity keeps the tombstone from `ApplicationExitInfo`. Then copy the crash directory out and `symbolicate` the signal record and the tombstone. A store build isn't debuggable, so `run-as` can't read it: on the emulator's Google APIs image, `adb root` can.

**Owed:** a crash on a device, the stores' own crash reports, and the real Windows and Linux packages crashing (their hosts are CI's).

## Exit test 6: combat that more people can play

The test (§1): each item of §3's accessibility list is a setting, saved and migrated, reached by pad, keyboard and touch, and held by a test of what it changes. With flashes reduced, the scripted fight passes WCAG 2.3.1's general and red flash tests. The smallest text is at least 12 px at 1280×800 at the default size, in both languages. Every cue that colour carries stays apart under simulated protanopia, deuteranopia and tritanopia.

**What proves it** (A1 to A6; spec §5's "As built (M4 A…)" notes have each):

- **Settings, saved and migrated:** `PlayerSettings` version 4, upgrading 1 to 3; `game_ui/tests/settings_formats.rs` holds the old bytes, and `game_flow/tests/saved_bindings.rs` the remaps' repair.
- **Reached by pad, keyboard and touch:** `game_ui/tests/menus.rs` walks every row of `menus.ron` by all three, in both languages and under pseudo-localization, at five sizes; `remap.rs` the remap screens.
- **What each changes:** A1's test that every presentation setting leaves a fight's checksums and tables equal; A2's scales (`EffectScale`, the mixer's `Volumes` and mono in `sapphire_audio`'s `tests/offline.rs`, text scale); `sapphire_controls/tests/toggle.rs` for hold or toggle; A6's `dodge-assisted` replay and `tests/dodges.rs` for the timing assist.
- **Text:** `game_ui/tests/text_size.rs`: 14.0 px at 1280×800 by default and 10.9 at the smallest sizes, in both languages, against 12 and 9.
- **Colour:** `sapphire_cues/tests/marks.rs` holds every pair of marks apart under the three dichromacies (Machado et al. 2009), and a GPU test holds `r.color_vision` to the CPU simulation.
- **Flashes:** `sapphire_testkit::flashes`' unit tests stand just under and over each line. `sapphire_sandbox`'s `flashes` lane (Z3c) captures every frame of the scripted fight at 60 fps and judges it at both settings; CI runs it nightly on lavapipe (X5). The stress fight is a command: the sandbox writes every frame of a replay's capture (`SAPPHIRE_CAPTURE_SEQUENCE`), and `sapphire_testkit flashes` scores them, refusing a sequence shorter than a second or than `--frames` (X5).

```bash
cargo test -p game_ui --locked --test text_size --test menus --test remap --test strings --test settings_formats
cargo test -p sapphire_cues --locked --test marks
cargo test -p sapphire_controls --locked --test toggle
cargo test -p game_flow --locked --test saved_bindings
cargo test -p sapphire_testkit --locked --lib flashes
# The flash analysis, per fight and setting (normal, reduced): every frame at 60 fps from tick 1, so
# --capture-frames is the fight's length: 1700 for arena-scripted, 2400 for the stress fight arena-skirmish.
SAPPHIRE_CAPTURE_SEQUENCE=<abs>/flash-reduced cargo xtask run --platform desktop --incremental -- \
  --capture <abs>/flash-reduced.png --size 480x270 --tier desktop-high --arena --cues --hud live \
  --replay tests/replays/arena-scripted.sreplay --effect-scale reduced:1 --capture-frames 1700 --capture-step 1
cargo run -p sapphire_testkit --profile tools --locked -- flashes <abs>/flash-reduced --frames 1700
cargo test -p sapphire_sandbox --features dev --locked --test flashes -- --ignored   # the scripted fight, both settings
```

**Passing:** every test passes; the analysis prints `frames=` the fight's length and `wcag_2_3_1=pass` for each fight at both settings, with `red_rate=0`. Without `--capture-frames` the sequence is the captured frame alone, which the analyser refuses. A3 scored the scripted fight 5 transitions a second at default and 4 reduced, the stress fight 6 and 5, against a limit of 6.

**CI:** every test above in `Test` on all three OSes, the goldens of the settings pages and the notice on all three backends. The scripted fight's flashes in the nightly `App tests (vulkan-lavapipe, flashes …)` jobs, one per setting; the stress fight's here only.

**Owed:** players who need these settings, and a reader of Japanese.

## Exit test 7: still authorable

The test (§1, pillar 1 carried from M3): the "How to add" recipes in [m4-handoff.md](m4-handoff.md) have no Rust step but one, showing a kind of value no model holds, which is code by design.

**What proves it:** walking the recipes as written. R2 made scripted inputs documents, R3 found shelved zones, R4 made placeholder sounds specs, R1 made a golden a row in `tests/golden/scenes.ron`, and X4 made the remap screens' glyphs data. The enemy-kind and boss recipes were walked end to end in M3 (2026-11-09); a recipe step that edits Rust it shouldn't is a finding against the pillar.

```bash
git switch -c scratch-recipes              # a branch you then drop
# follow a recipe (a screen, a dialogue, a sound, a golden) touching only content/, tools/sounds/ and tests/golden/, then:
cargo xtask content
cargo test -p game_ui --locked --test strings --test menus
cargo xtask golden --update --scene <name>   # a golden's row, one name a run; then again without --update
cargo test -p sapphire_testkit --locked      # the scenes' families: a screen's are game-<screen>-<aspect>-<locale>, -wide- with its -deck-
git diff --stat main -- '*.rs'              # must be empty
git switch - && git branch -D scratch-recipes
```

**Passing:** each recipe cooks and its tests pass with no `.rs` file changed, except the screen showing a value no model holds.

**Owed:** nothing; the run proves it all.

## Exit test 8: ready to play

The test (§1): one command builds, installs and starts a playtest build on each target the maintainer may hold. A played session comes back as a replay that reaches its checksum here, with a report.

**What proves it** (F1, F2, [playtest.md](playtest.md)):

- `cargo xtask playtest --platform desktop|ios-sim|ios|android [-- args]` builds the game for release with its `playtest` feature, installs and starts it.
- `cargo xtask playtest pull --platform <p>` copies every session to `target/playtest/<p>/sessions/` and replays every run to the device's checksums, failing on the first tick that differs.
- `cargo xtask playtest report <session>` reads each run per encounter, and writes `report.ron` beside it.
- `game_app`'s `draws_a_marked_tick` holds a mark's capture to the tick it names.

```bash
cargo xtask playtest --platform desktop -- --new-game --script fight-back --ticks 1695 \
  --capture <abs>/fight.png --save-dir <abs>/save                # a scripted session, offscreen
cargo xtask playtest pull --platform desktop --from <abs>/save
cargo xtask playtest report target/playtest/desktop/sessions/<session>
cargo test -p game_app --features dev --locked --test draws_a_marked_tick -- --ignored
```

On the Simulator and the emulator, with the private device booted: `cargo xtask playtest --platform ios-sim|android -- --new-game --script fight-back` starts it in its window; stop it after a minute (`simctl terminate`, `am force-stop`), then `pull` and `report`. Build first with `--dry-run`. **Passing:** `pull` says every run reached the device's checksums, and `report` reads each run's encounters.

**Owed:** someone playing, and `--platform ios` on a phone (its pull is written to `devicectl`'s documentation and has never run).

## Final run at 83dc6273 (M4 Z2)

2026-11-11 and 12, on 83dc6273, the landing line's round 7, which `main` fast-forwarded to: all of M4. An Apple M5 Mac (Mac17,2, macOS 27.0.1, Xcode 26.2), headless throughout: the Mac offscreen, a private Simulator (iPhone 17 Pro, deleted after) and the emulator (`sapphire_api36`, `-no-window`). Each run took a gate slot, one at a time; other agents built beside the untimed runs, at a load average of 2 to 7. The timed playthrough ran alone (`slot --all --load 8`) at a load average of 2.1 cold and 2.6 warm. Numbers are as each command printed them.

| Exit test | Verdict at 83dc6273 |
|---|---|
| 1 | **Pass for what's built.** The counts hold at 0 everywhere: the playthrough, every setting at every rung, the ladder, the boot on this Mac, a cold Simulator and a cold emulator. On Metal from a cold cache, timed, no load screen's frame passed twice the interval: the boot screen's worst 25.5 ms and the wipe's 5.9 against 33.3. Owed: a phone's cold compile, a desktop GPU's driver cache, a store install's first launch |
| 2 | **Pass** on this Mac, the Simulator and the emulator: exactly the manifest, one table and one range of blocks, and the launch that installs it stands on the emulator now. Owed: a real CDN, the release key, TLS on each OS's trust store |
| 3 | **Pass for what's local:** every validator here (macOS, iOS, the Simulator's, Android) and the head's `packages.yml` (`--dev`, all five green). Owed: the first scheduled run on `main`, then signing, uploads, notarization and review, all the maintainer's accounts |
| 4 | **Pass:** the 61 `tier-*` scenes on Metal (lavapipe and WARP in CI), `look_tiers`, the traces, and the emulator's override down at once and back a rung at a time. Owed: a phone throttled after 15 minutes |
| 5 | **Pass** on macOS, the Simulator and the emulator, each read to `forced.rs`; Windows and Linux are CI's crash lane. Owed: a crash on a device, the stores' crash reports |
| 6 | **Pass:** every test, the `flashes` lane, and both fights' flash analysis at both settings. The stress fight at default sits on the limit, 6 of 6. Owed: players who need the settings, a reader of Japanese |
| 7 | **Pass:** a screen, a dialogue, a sound and two goldens with no Rust. The goldens' other backends come from a pull request's CI, as the recipe says. Owed: nothing |
| 8 | **Pass** on the desktop, the Simulator and the emulator: each session replays to the device's checksums and reports. Owed: someone playing, and `--platform ios` on a phone |

**Before anything.** `cargo metadata --locked` held; with `target/content/cache` removed, `cargo xtask content` cooked everything in 171 s and `--cook mobile` in 14 s.

**Exit test 1.** `declares_every_setting` exit 0 (261 s with its build): Desktop-High's 5 rungs × 27 settings × 9 screens in 1,896 frames and 7.5 s, Desktop-Low 4.8 s, Mobile-High 5.6 s, Mobile-Low's 4 rungs in 1,533 frames and 3.5 s, nothing built past the load. `follows_the_ladder` exit 0: 332 frames, the ladder to depth 5 (`Heat(Critical)`, 30 fps) on the first frame, Desktop-Low from frame 153 (tick 120). `reaches_the_fight` exit 0: `booted frames=69 warm_ms=374.2 prewarmed=83 first_use=0 frame_thread=0 frame_waits=0`, its longest frame the stand's 29.8 ms (`intervals_over=0` against 33.3 ms; 139.7 ms in the rehearsal, before H4 split the stand), the fight's checksum `09a3088c62423c67` at tick 600.

`zero_hitches`, untimed (the test profile, 480×270, a warm Metal cache) exit 0 in 186 s: booted in 285 ms over 83 frames (the renderer at 163 ms, the packs warm at 180, stood at 282), 89 pipelines prewarmed, all off the frame, its longest frame the stand's 27.9 ms. The walk won the ward at tick 14,261 after 14,799 steps and the ladder climbed back in 5,406 frames: 21,226 frames, all drawn, in 184.2 s. "Warmed off the frame 374, first use 0, on the frame's thread 0, waited for 0." The tier changed at the pause in 16.1 and 20.2 ms. The boot screen's worst frame 31.0 ms against 33.3; the wipe back to the title 98 frames, worst 3.9 ms. Memory: 333 MiB at the boot, 613 at the title, 626 on the walk, 646 in the fight, 651 at the pause, 613 at the load and at the title again; the process's peak 652. Its own check failed as it should on the injected debug line.

**Timed, from a cold Metal cache** (the lane built for release in a throwaway `.app` with a fresh bundle ID, `SAPPHIRE_HITCH_TIMED=1`, Desktop-High at 1280×720, alone), exit 0 in 101 s: booted in 3,728 ms (the renderer made by 953 ms, the packs warm by 3,082, stood by 3,171), 89 pipelines prewarmed, all off the frame, the boot's longest frame 24.6 ms in its stand; 21,217 frames, all drawn, in 101.2 s; first use 0, on the frame's thread 0, waited for 0 (374 warmed off the frame). The boot screen's worst frame 25.5 ms and the wipe back to the title's 5.9 ms (85 frames), against 33.3. The tier changed at the pause in 10.9 and 11.3 ms; the longest frame of play 27.5 ms, at the title. Memory: 324 MiB at the boot, 691 at the title, 693 on the walk, 730 in the fight, 732 at the pause, 691 at the load and at the title again; the process's peak 736. Its check failed as it should. **Warm,** the same `.app` again, exit 0 in 96 s: booted in 201 ms over 60 frames (the renderer by 106 ms, all warm by 201), the counts 0, the boot screen's worst 23.7 ms and the wipe's 8.4, the tier changed in 11.2 and 13.2 ms, the longest frame of play 22.0 ms, the process's peak 731 MiB. (Exec'd without `--ignored`, the lane only prints that it needs a GPU and exits 0: the timed recipe above now passes it.)

The release game's title capture (1280×720, warm): `booted frames=13 warm_ms=323.6`, longest 26.7 ms, `intervals_over=0 prewarmed=89 first_use=0 frame_thread=0 frame_waits=0`, then `frame_thread=0 frame_waits=0` after the boot. The six boots in `patch rehearse` (640×360, warm): 13 to 74 frames, the longest 26.3 to 28.9 ms, `intervals_over=0`, the counts 0 on all six, the installing launch and the resumed one among them (156 to 178 ms in the rehearsal).

**The Simulator,** the `dev` bundle on a fresh one, its Metal cache cold: `booted frames=451 warm_ms=16447.6` (the renderer at 4.5 s, the packs at 14.3 s), the longest frame 62.9 ms in the stage, `intervals_over=0` against 66.7, `prewarmed=73 warmed_off_frame=73 first_use=0 frame_thread=0 frame_waits=0`. Launched again: 11 frames, 414.7 ms, the longest 41.4 ms, the counts 0. **The emulator,** the `dev` APK uninstalled first, so its pipeline cache was cold too (the rehearsal's wasn't): `booted frames=38 warm_ms=9755.0`, the packs warm at 8.7 s, `prewarmed=73 warmed_off_frame=73 first_use=0 frame_thread=0 frame_waits=0`, and `frame_thread=0 frame_waits=0` after the boot; but its longest frame was 1,297.4 ms in the packs and 18 frames passed 66.7 ms while the three warm-up threads built the packs' pipelines (see "Found"). Launched again: 18 frames, 725.3 ms, the longest the stand's 57.5 ms, `intervals_over=0`, the counts 0.

**Exit test 2.** `sapphire_patch`'s tests: 43 passed. `patch rehearse` exit 0 (234 s): 5,280 bytes in four requests (the manifest's 1,047; the `ui` pack's 256-byte header and 2,432-byte table; one range of 1,545 bytes of blocks). The relaunch fetched the manifest alone (1,047); the untrusted release was refused at its manifest and release 1 drawn; killed after 3,735 bytes with one table staged, the next launch fetched 2,592 bytes in two requests, no header or table again, and stood on release 2. **The Simulator:** release 1 of the mobile cook (the title's tagline from 18 to 20 points, signed with the test seed) from `patch serve` on this Mac: 5,279 bytes in the same four requests (1,046, 256, 2,432, 1,545), `updated=true update_received=5279`, the counts 0, a capture unlike the unpatched title; launched again, the manifest alone, `updated=false`, the same picture. **The emulator, through `adb reverse`, re-run in full:** the same four requests and 5,279 bytes, `updated=true update_received=5279`, booted in 26 frames with the counts 0 and the longest frame 58.7 ms, and `main returned status=0`: the launch that installs the patch stands (it overflowed its stack in the rehearsal; H4 boxed the boot). The relative `--patch-dir patch` landed in the app's `cache/patch/` (`installed/`, `staging/`). Launched again: the manifest alone, `updated=false`, the same picture, the counts 0.

**Exit test 3.** `package --platform macos --dev` exit 0 (403 s): build 1370, a 58 MiB `.app` signed ad hoc with the hardened runtime, 377 MiB of symbols, one debug ID. `--platform ios --dev` exit 0 (406 s): a 417 MiB `.xcarchive`, the dSYM's UUID the executable's, seven plists lint clean, the privacy manifest's reasons C617.1, 35F9.1 and E174.1, `atos` finding SDL's, Luau's and the game's lines. `--platform ios-sim --dev` exit 0 (372 s). `--platform android --dev` exit 0 (327 s): a 103 MiB AAB on Android's debug key (the package warns Play refuses it), one base module, 16 KB segments, the symbols in its metadata; `build-apks --local-testing` 6 APKs aligned for 16 KB pages, 36.6 MB to download per device. Each stripped binary matched its symbols, and none held this machine's paths. `packages.yml` on 83dc6273 (a pull request, so `--dev`): all five jobs green, iOS 88 min, macOS 48, Windows 35, Linux 22, Android 13. No scheduled run on `main` yet.

**Exit test 4.** All 375 golden scenes `ok` on Metal in 1.9 minutes, the 61 `tier-*` among them (their worst tile 0.0020 at most; 0.1236 across the suite, `lookdev-death-back`, against 0.15). CI's `Golden images` held them on lavapipe and WARP for the head's pull request. `look_tiers` 2 of 2; `quality::ladder` 11 of 11. **The emulator's override,** the game in its window: depth 1 from the start (`Heat(Light)`, headroom 0.90); `override-status 4` took it to depth 3 at once (`Heat(Critical)`, scale 0.75); after `override-status 0` it climbed to depth 2 in 10 s and depth 1 10 s later, a rung at a time, with no oscillation.

**Exit test 5.** `sapphire_symbols`' `crash` lane (`SAPPHIRE_CRASH_TESTS=1`) exit 0. The macOS package's executable with a private `HOME`: `--crash fault` exited 139 leaving a minidump, which `symbolicate` read to `EXC_BAD_ACCESS` at 0x10 in `forced_fault` (forced.rs:97), then `ForcedCrash::now` (forced.rs:78) and `game_app::startup` (lib.rs:137), by frame pointers; `--crash panic` exited 134 leaving a minidump and a panic report (`0.1.0+1370.dev.d499e6a`, forced.rs:86:5), read to `forced_panic`. `symbols --package macos` exit 0. **The Simulator:** the `ios-sim` package, `--crash panic`: its panic report read to `forced_panic` (forced.rs:86). **The emulator:** the store build (`local-testing.apks`' base and arm64 splits), `debug.sapphire.crash=fault`, then started again (17 frames, the counts 0): a signal record (SIGSEGV at 0x10) and a tombstone (`SEGV_MAPERR`, its first frame in `split_config.arm64_v8a.apk!libgame_app.so`), both read to `forced_fault` (forced.rs:97) and `ForcedCrash::now` (forced.rs:78).

**Exit test 6.** `game_ui`'s `menus` 12, `remap` 5, `settings_formats` 5, `strings` 10 and `text_size` 5: 37 passed, none ignored (Z3c's scrolling let the largest-interface test stand, which the rehearsal ignored). `marks` 4, `toggle` 3, `saved_bindings` 2 and `sapphire_testkit::flashes` 9 passed. The `flashes` lane: 2 passed (the scripted fight by default and reduced). The flash analysis, every frame at 60 fps and 480×270, each capture reaching its replay's checksums with no allocations:

| Fight | Setting | Frames | General transitions a second | Red | WCAG 2.3.1 |
|---|---|---|---|---|---|
| Scripted (`arena-scripted`) | reduced | 1,700 | 3 | 0 | pass |
| Scripted | default | 1,700 | 5 | 0 | pass |
| Stress (`arena-skirmish`) | reduced | 2,400 | 3 | 0 | pass |
| Stress | default | 2,400 | 6 | 0 | pass |

The limit is 6 (three flashes a second). The rehearsal's numbers were the same.

**Exit test 7.** On a scratch branch, dropped after, four recipes as m4-handoff.md writes them: a screen (a credits page reached from the settings under the privacy policy's row, its strings in both languages, its row in `menus.ron`), a dialogue (a porter with a choice that sets an existing flag), a placeholder voice line (a spec under `tools/sounds/`, generated, its row in `SOURCE.md`) and the screen's goldens (`game-credits-wide-ja` and `game-credits-deck-ja`, rows in `tests/golden/scenes.ron`). 17 files changed, none of them Rust. `sapphire_soundgen` wrote the line (5,469 bytes) and its tests 10 passed; `cargo xtask content` cooked it all; `game_ui`'s `strings`, `menus` and `text_size` 27 passed; the sandbox drew the new screen in Japanese and the dialogue's preview (`z2_porter:0`); `golden --update` wrote both goldens on Metal, each captured frame `first_use=0`, and a second `golden` held them; `sapphire_testkit`'s tests 54 passed. Earlier walks stopped on the scratch scripts, not the recipes: the rehearsal's script put the new row where Z3b's privacy row now is (the cook refused the second `privacy` node), then the golden's name had a digit, which the runner refuses as the recipe says, then it had one aspect without its Deck twin, which the screen recipe asks for (see "Found"). The lavapipe and WARP goldens come from a pull request's first CI run (the recipe's step 3), so the scratch branch didn't make them. The remap glyphs are data since X4, so no recipe read through has a Rust step but the screen showing a value no model holds.

**Exit test 8.** `playtest --platform desktop -- --new-game --script fight-back --ticks 1695` exit 0; `pull` replayed its run (1,695 ticks, ending `006faccf3917cf07`, no allocations) to the device's checksums; `report` read 6.6 s played, 11 dodges, outside any encounter, frames 3.70 ms mean and 310.41 ms worst (2 of 1,627 late at 16.7 ms). `draws_a_marked_tick` exit 0 (tick 600 drawn after 600 frames). **The Simulator and the emulator:** `playtest --platform ios-sim|android -- --new-game --script fight-back` on the private device, stopped two minutes after it began, then pulled. The Simulator's run, 5,402 ticks ending `d0834dbc00aef61f`, and the emulator's, 6,022 ticks ending `69770e39790e645a`, each replayed here to the device's checksums with no allocations; `report` read 90.0 s and 100.4 s played, outside any encounter, 1 of 2,699 and 1 of 3,006 frames late at 33.3 ms (the worst 71.8 and 66.7 ms).

**Found, for their owners:**

- **Exit test 1, a cold emulator's boot screen:** with the pipeline cache empty, the boot screen's frames stall while the packs' pipelines build off the frame: 18 frames over 66.7 ms, the worst 1,297.4 ms, in the 7 s between "renderer created" and "the packs' pipelines prewarmed". The counts stay 0, and a warm launch is smooth (57.5 ms). The emulator's GPU is MoltenVK behind the host's renderer on four cores, three of them warming, so a phone may not show it; the Simulator's cold boot doesn't (62.9 ms). (H track; a phone's cold boot.)
- **Exit test 1, the lane run bare:** `zero_hitches`' binary exec'd without `--ignored` prints that it needs a GPU and cooked content and exits 0 in a second, so a timed run judged by its exit code passes having played nothing. The runbook's timed recipe passes `--ignored` now; the lane could fail instead when it's asked to run timed (`SAPPHIRE_HITCH_TIMED=1`) and skips. (H4.)
- **Exit test 7, a screen's golden:** the handoff's "A golden" says a name is lowercase letters and hyphens (the runner refuses anything else, as it did a scratch `z2-…`), but not the families `sapphire_testkit`'s scenes tests hold: a screen's golden is `game-<screen>-<aspect>-<locale>`, or `only_the_hud_scenes_draw_the_interface…` fails it for not being 640×360, and a `-wide-` one needs its `-deck-` twin, or `the_screens_scenes_cover_the_aspect_ratios_in_japanese` fails. The screen recipe asks for every aspect, so both recipes followed in full pass; "A golden" could name the families. (R1, the handoff.)
- **Exit test 6, the stress fight** at default scores 6 transitions a second, the limit, as in A3 and the rehearsal: it passes with no margin. (A3.)
- **Exit test 3, the nightly:** `packages.yml` has still run only on pull requests (`--dev`); its first scheduled run on `main` is the one §1 asks for. (I2.)
- **CI on `main` at 83dc6273:** every Windows job of the push was cancelled after exactly 15 minutes with no step run: `GPU tests (dx12-warp)`, `Test (windows-2025)`, `Replays (windows-2025)` and `Prepare golden inputs (dx12-warp)`, as `Test (ubuntu-24.04)` was at d8b621cc. The WARP GPU tests run only on pushes to `main`, so `look_tiers` on WARP hasn't run on the head; the pull request's `CI` (its Windows jobs among them) and `Packages` were green. (CI.)

**Resolved since the rehearsal:** the emulator's crash in the launch that installs a patch (H4 boxed the boot and the show); a relative `--patch-dir` on a phone (H4, Z3e); a one-frame flash analysis passing (X5 refuses it); the ward and the hit at every tier, floor and rung (T5); a sound in `tests/download.rs` (X5); `zero_hitches` in CI (H4), `follows_the_ladder` and `draws_a_marked_tick` too, and the flash lane nightly (X5); the remap glyphs as data (X4).

## Rehearsal at d8b621cc (M4 Z2)

2026-11-11, on d8b621cc, an Apple M5 Mac (Mac17,2, macOS 27.0.1, Xcode 26.2), headless. Other agents were building throughout: the load average ran 4 to 12, cargo had 3 jobs, and no time below is a bench. Numbers are as each command printed them.

| Exit test | Result at d8b621cc |
|---|---|
| 1 | Pass for what's built; the playthrough lane (H4) is not yet built |
| 2 | Pass on this Mac and the Simulator. **Fails on the emulator:** the game crashes in the launch that installs the patch |
| 3 | Pass: every local validator here and in CI's `--dev` run; no nightly `main` run yet |
| 4 | Pass for the 28 scenes and the emulator's override; the ward and the hit have goldens at two tiers and one rung, not every tier, floor and rung |
| 5 | Pass on macOS, the Simulator and the emulator; Windows and Linux are CI's |
| 6 | Pass: every test, and both fights' flash analysis at both settings |
| 7 | Pass for the screen, the dialogue and the sound; the remap glyphs are a Rust step |
| 8 | Pass on the desktop, the Simulator and the emulator |

**Exit test 1.** `declares_every_setting` exit 0 (347 s with its build): Desktop-High's 5 rungs × 27 settings × 8 screens in 1,766 frames and 6.1 s, Desktop-Low 6.1 s, Mobile-High 4.8 s, Mobile-Low's 4 rungs in 1,429 frames and 3.8 s, nothing built past the load. `follows_the_ladder` exit 0: 332 frames, the ladder to depth 5 (cause Heat) on the first frame, Desktop-Low from frame 153 (tick 120), `first_use=0 frame_thread=0`. `reaches_the_fight` exit 0: `booted frames=55 warm_ms=518.8 prewarmed=83 first_use=0 frame_thread=0 frame_waits=0`, its longest frame the stand's 139.7 ms (`intervals_over=1` against 33.3 ms), fight checksum `09a3088c62423c67` at tick 600. The `dev` game's six boots in `patch rehearse` (warm Metal, 640×360): 7 to 10 frames, `prewarmed=89 warmed_off_frame=89 first_use=0 frame_thread=0 frame_waits=0` each, the stand 156 to 178 ms (`intervals_over` 1 or 2): over twice the interval, which H4 spreads. Every golden ran with its first-use check (below). A fresh private Simulator (iPhone 17 Pro, its Metal cache cold), the `dev` bundle's capture: `booted frames=459 warm_ms=17077.9 prewarmed=73 first_use=0 frame_thread=0 frame_waits=0`; launched again, 7 frames and 507.9 ms, the counts 0. The emulator (`sapphire_api36`, `-no-window`): 15 frames, 956.5 ms, the counts 0; again 17 frames, the counts 0. On both the longest frame is the stand's (166 to 238 ms against 66.7). H4's `zero_hitches`: not yet built.

**Exit test 2.** `patch rehearse` exit 0 (516 s with its build and cooks): 5,152 bytes in four requests (the manifest's 1,047; the `ui` pack's 256-byte header and 2,304-byte table; one range of 1,545 bytes of blocks). The relaunch fetched the manifest alone (1,047); the untrusted release was refused at its manifest ("no key this install trusts signed the manifest") and release 1 drawn; killed after 3,607 bytes with one table staged, the next launch fetched 2,592 bytes in two requests, no header or table again, and stood on release 2. **The Simulator:** release 1 of the mobile cook (the title's tagline from 18 to 20 points, signed with the test seed) from `patch serve` on this Mac: 5,151 bytes in the same four requests (1,046, 256, 2,304, 1,545), `updated=true update_received=5151`, a capture unlike the unpatched title; launched again, the manifest alone and `updated=false`, the same picture. **The emulator, through `adb reverse`:** the same four requests and 5,151 bytes, and the release installed, then the game died: SIGSEGV on its main thread 5 ms after "the update is installed", which Android's crash dump reads as a stack overflow. Twice, from a clean install each time. The next launch fetched the manifest alone and drew release 1, so the install held; the launch that installs it doesn't survive. See "Found".

**Exit test 3.** With 98 GiB free: `package --platform macos --dev` exit 0 (595 s): build 1298, a 58 MiB `.app` signed ad hoc with the hardened runtime, a 373 MiB dSYM, one debug ID. `--platform ios --dev` exit 0 (506 s): a 414 MiB `.xcarchive` and 47 MiB app, the dSYM's UUID the executable's, seven plists lint clean, the privacy manifest's reasons C617.1, 35F9.1 and E174.1, `atos` finding SDL's and Luau's lines. `--platform ios-sim --dev` exit 0 (497 s). `--platform android --dev` exit 0 (541 s): a 103 MiB AAB, versionCode 1298, Android's debug key, a download of 36.5 MB per device from `build-apks`. `packages.yml` on d8b621cc (a pull request, so `--dev`): all five jobs green, iOS 79 min, Windows 31, macOS 45, Linux 21, Android 17, each keeping its package and symbols. No scheduled run on `main` exists yet.

**Exit test 4.** The 28 `tier-*` scenes `ok` on Metal (worst tile 0.0020 at most); the same 28 have lavapipe and WARP references in `tests/golden/`, which CI's `Golden images` holds. `look_tiers` 2 of 2; `quality::ladder` 11 of 11. **The emulator's override,** the game in its window: it stood at depth 1 from the start (`Heat(Light)`, headroom 0.90 on the emulator); `override-status 4` took it to depth 3 at once (`Heat(Critical)`, scale 0.75); after `override-status 0` it climbed to depth 2 in 21 s and to depth 1 20 s later, a rung at a time. No oscillation; Z1 records the trace as shipped.

**Exit test 5.** The macOS package's executable with a private `HOME`: `--crash fault` exited 139 leaving a minidump; `symbolicate` read `EXC_BAD_ACCESS` at 0x10 in `sapphire_crash::forced::forced_fault` (forced.rs:97), then `ForcedCrash::now` (forced.rs:78) and `game_app::startup` (lib.rs:136, inlined), by frame pointers. `--crash panic` exited 134 leaving a minidump and a panic report (`0.1.0+1298.dev.d8b621c`, forced.rs:86), which `symbolicate` read to `forced_panic`. `symbols --package macos` exit 0. **The Simulator:** the `ios-sim` package on a private Simulator, `--crash panic`: a panic report (forced.rs:86:5), which `symbolicate` read to `forced_panic` through the panic hook's frames. **The emulator:** the Android package's store build (`local-testing.apks`' base and arm64 splits), `debug.sapphire.crash=fault`, then started again: a signal record and a tombstone, copied out with `adb root`. `symbolicate` read both to `forced_fault` (forced.rs:97) and `ForcedCrash::now` (forced.rs:78), the tombstone's first frame in `split_config.arm64_v8a.apk!libgame_app.so`.

**Exit test 6.** `game_ui`'s `text_size`, `menus`, `strings` and `settings_formats`: 29 passed, 1 ignored (`every_screen_fits_at_the_largest_interface_size_at_every_aspect`: pause, character and dialogue run out of height on a phone at the largest size, A3's known). `marks` 4, `flashes` 8, `toggle` 2, `saved_bindings` 2, all passed. The flash analysis, every frame at 60 fps and 480×270 (each capture's replay reaching its checksums): the scripted fight 5 transitions a second at default and 3 reduced, the stress fight 6 and 3, no red flashes, all `wcag_2_3_1=pass` against a limit of 6. A3 had 5 and 4, 6 and 5: reduced is lower now, and the stress fight's default still has no margin.

**Exit test 7.** On a scratch branch, dropped after, three recipes as m4-handoff.md writes them: a screen (a credits page reached from the settings, its strings in both languages, its row in `menus.ron`), a dialogue (a porter with a choice that sets an existing flag) and a placeholder voice line (a spec under `tools/sounds/`, generated, its row in `SOURCE.md`). 14 files changed, none of them Rust. `sapphire_soundgen`'s tests 10 passed; `cargo xtask content` cooked them; `game_ui`'s `strings`, `menus` and `text_size` 24 passed, 1 ignored (A3's); the sandbox drew the new screen in Japanese. The dialogue's preview was given `z2_porter:1`, a line past its first node's choice, which the sandbox refuses ("line 0 wants an answer"): `:0` previews it. Read through, the other recipes need no Rust but two: the golden scene's row (R1 makes it a document) and the remap glyphs (below). The kind of enemy and the boss were walked end to end in M3.

**Exit test 8.** `playtest --platform desktop -- --new-game --script fight-back --ticks 1695` exit 0; `pull` replayed its run (1,695 ticks, ending `006faccf3917cf07`, M3's checksum, no allocations) to the device's checksums; `report` read 8.4 s played, 11 dodges, outside any encounter (the fight-back script stands in the Gate Square), frames 4.77 ms mean and 672.86 ms worst (1 of 1,682 late). `draws_a_marked_tick` exit 0 (tick 600 drawn on frame 600). **The Simulator and the emulator:** `playtest --platform ios-sim|android -- --new-game --script fight-back` built, installed and started each on the private device (the Simulator booted first, which `playtest` picks); stopped two minutes after the command began, its build included, then pulled. The Simulator's run, 1,205 ticks ending `342cf1c4c22d1770`, and the emulator's, 6,017 ticks ending `291fc16e4b9a61b6`, each replayed here to the device's checksums with no allocations; `report` read 20.1 s and 100.4 s played, outside any encounter, 1 of 597 and 9 of 2,994 frames late at 33.3 ms.

**Found, for their owners:**

- **Exit test 2, the emulator's patch:** the `dev` game on the emulator downloads and installs release 1, then overflows its stack 5 ms later (SIGSEGV, SEGV_ACCERR on `SDLThread`, "stack pointer is not in a rw map"), twice from a clean install. Read with the same build's library (build ID `ad850f57…`): the deepest frame is `GameShow::new`, from `GameShow::from_content` (game_flow/src/show.rs:213), `Options::make_show` (game_app/src/options.rs:229), `Boot::stand_on` (game_flow/src/boot.rs:536) in `Boot::frame`'s `advance`, under `capture` and `android_loop::enter_main_callbacks`. The same edit on the Simulator stands fine, and an unpatched boot stands on the emulator: the stand after an install, on Android's thread, runs out of stack. (P4 and H2's boot; Z3a's review.)
- **Exit test 2, the dev flag on Android:** a relative `--patch-dir` isn't put under the app's `cache/` as `--capture`'s is, so the game skips its check ("Read-only file system"). (P4.)
- **Exit test 6, a one-frame analysis passes:** `sapphire_testkit flashes` scores a sequence of one frame `wcag_2_3_1=pass`; a capture without `--capture-frames` writes only that. It could refuse a sequence shorter than a second. (A3.)
- **Exit test 4, the ward and the hit** have goldens on Desktop-Low and Mobile-Low and at their third rung only. §1 asks for each at every tier, at each tier's floor and at each rung, as the face has. (T track.)
- **Exit test 2, a sound:** `sapphire_patch`'s `tests/download.rs` edits a texture, a mesh, a zone's assets, a new pack, a string and a screen, but no sound, which §1's set names. P0 measured a sound on real cooks; Z1's patch sizes are the shipped measure. (P track, or Z1.)
- **Exit test 1 and 8, lanes in no CI job:** `follows_the_ladder` and `draws_a_marked_tick` run only here; CI's `App tests` list has neither. (CI.)
- **Exit test 6, the flash analysis** is a command and nobody's lane: the fights' WCAG scores hold only when someone runs it. (A3, or Z3c.)
- **Exit test 7, the remap glyphs:** a remappable action added through `inputs.ron` still needs a field in `game_ui::menus::BindingGlyphs` (`game/game_ui/src/menus.rs`) for its row's glyph: a Rust step the handoff's recipes don't name as by design. (A4, the handoff.)
- **Exit test 3, the nightly:** `packages.yml` has run only on a pull request (`--dev`); its first scheduled run on `main` is the one §1 asks for. (I2.)
- **Running the bundles:** `xtask playtest --dry-run` writes its build to `target/<platform>/`, where the `dev` bundle is, so a device run after it installs the playtest build, which has no `--update-host`. Keep a copy of each bundle, as the rehearsal did. (F1, or a note.)
- **CI on `main` at d8b621cc:** `Test (ubuntu-24.04)` was cancelled after 15 minutes; the same commit's pull-request run was green. (CI.)

## What stays owed

No run on this Mac answers these; each needs a device, an account or a person (§1, and [m4-plan.md](m4-plan.md) §7 for what the maintainer holds):

- **Exit test 1:** a phone driver's cold compile and its boot screen while it warms (the emulator's stalls, above), a desktop GPU's driver cache on Windows and Linux, and a store-installed build's first launch.
- **Exit test 2:** a real CDN, the maintainer's release key and host in `platform/patch.toml`, and TLS against a real certificate on each OS's trust store.
- **Exit test 3:** the first scheduled `packages.yml` run on `main` (a release's packages, not `--dev`); then distribution signing, the uploads, notarization, and each store's server-side checks and review.
- **Exit test 4:** a phone thermally throttled after 15 minutes.
- **Exit test 5:** a crash on a device, the stores' own crash reports, and the real Windows and Linux packages crashing.
- **Exit test 6:** players who need the settings, and a reader of Japanese.
- **Exit test 8:** someone playing, and `--platform ios` on a phone.
- **All of it:** nobody has played the slice, and no test here asks whether it's fun.
