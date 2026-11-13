# Getting started

How to build Sapphire, run its apps on every platform, and use the tools it gives you while they run. For the code's layout and rules, start at [AGENTS.md](../AGENTS.md); for how it fits together, [architecture.md](architecture.md).

- [Requirements](#requirements)
- [Run it](#run-it)
- [Sandbox controls](#sandbox-controls)
- [Debug overlay](#debug-overlay)
- [Hot reload and live link](#hot-reload-and-live-link)
- [Without a window](#without-a-window)
- [Golden images](#golden-images)
- [Replays](#replays)
- [Packages, playtests and patches](#packages-playtests-and-patches)
- [Everyday commands](#everyday-commands)
- [Troubleshooting](#troubleshooting)

## Requirements

| For | Install |
|---|---|
| Everything | `rustup`. The toolchain (Rust 1.99) and every target are pinned in `rust-toolchain.toml` and install themselves. CMake and Ninja build SDL3 from source. |
| macOS and iOS | Xcode 26 or newer, and an iOS Simulator runtime (`xcodebuild -downloadPlatform iOS`). |
| Android | JDK 17 (`JAVA_HOME` set for Gradle) and the Android SDK (`ANDROID_HOME`, or `~/Library/Android/sdk`) with NDK r28 or newer, `platforms;android-36`, `build-tools;36.0.0`, `platform-tools` and an emulator image. `cargo xtask run --platform android` boots an AVD named `sapphire_api36` when no device is connected. |
| Linux | SDL3's build dependencies: the X11, Wayland, xkbcommon and udev development headers. |

## Run it

`cargo xtask run` cooks the content, builds the app, packages it where the platform needs that, launches it and streams its log to your terminal:

```bash
cargo xtask run --platform desktop                  # Windows, Linux or macOS, in place
cargo xtask run --platform macos                    # the signed .app bundle
cargo xtask run --platform ios-sim                  # the iOS Simulator, booted headless if none is
cargo xtask run --platform android                  # the connected phone, or the sapphire_api36 emulator
```

The default app is the engine's **sandbox**, `sapphire_sandbox`. `--app game_app` runs **the game**, the Lantern Ward: a title screen, a hub, three fights, cutscenes and a boss, in English and Japanese.

```bash
cargo xtask run --platform desktop --app game_app --release
```

- **Flags:**
  - `--release` builds optimized.
  - `--incremental` (desktop and macOS) builds optimized without LTO, so a one-line edit rebuilds in seconds.
  - `--features dev` adds the debug overlay, hot reload and live link.
  - `--dry-run` builds and packages but prints the launch command instead.
- **Arguments after `--`** go to the app. For example:
  - `-- --look-dev hero` shows the look-dev stage, the town at golden hour; its other shots are `wide`, `sky`, `street` and `face`.
  - `-- --frames 600` exits after 600 frames.
  - `-- --version` prints the build's version (`Sapphire 0.1.0`, with the build number in a packaged build) and exits; the title screen shows the same in its corner.

  `cargo run -p sapphire_sandbox -- --help` lists the rest. The crate's `Arguments` docs (`cargo doc -p sapphire_sandbox --open`) list every scene, capture, benchmark, animation, HUD and development option.
- **On each platform:**
  - **desktop** runs the build against `target/content/desktop/`. `cargo xtask bundle --platform desktop` makes a self-contained directory to copy to another machine.
  - **macos** builds and signs `target/macos/<Name>.app`, which reads its bundled content as a player's copy does. A `dev` build reads `target/content/desktop/`, so a recook reaches it while it runs.
  - **ios-sim** boots a simulator headless; `--show` also opens Simulator.app.
  - **android** boots the AVD headless with `-gpu host` for hardware Vulkan, and returns when the app exits.

## Sandbox controls

| Input | Action |
|---|---|
| Drag (mouse or finger), arrow keys, right stick | Orbit |
| Wheel, pinch, triggers | Zoom |
| `Space`, gamepad South or a double tap | World slow motion |
| `H`, gamepad East or a long press | Hitstop |
| `P`, gamepad Start or Android's Back | Pause: the simulation stops; the overlay, hot reload and live link don't |
| `P`, `Esc`, gamepad Start or East, or a tap, while paused | Resume |
| Android's Back, gamepad Back or `Backspace`, while paused | Leave |
| `Esc` (desktop) | Quit |
| `` ` `` (below `Esc`), or two fingers held still for a second | Show or hide the debug overlay (`dev` builds) |

The bindings are documents: [`apps/sapphire_sandbox/default_bindings.ron`](../apps/sapphire_sandbox/default_bindings.ron), over combat's own in [`crates/sapphire_controls/combat_bindings.ron`](../crates/sapphire_controls/combat_bindings.ron). `--combat-input` leaves combat's alone: every verb on the pad and the keyboard. The game's controls are remappable in its settings.

## Debug overlay

`dev` builds add the overlay (`sapphire_debug`, egui). It has these panels:

- **stats:** frame time and CPU stages against the spec's budgets.
- **graph:** the frame graph pass by pass, with GPU timings.
- **time:** pause, step one tick, speed.
- **day:** scrubs the time of day.
- **effects:** the screen effects.
- **combat:** the fight's state.
- **console:** `list` shows every cvar, `<name> <value>` sets one, Tab completes names.
- **cvars:** every cvar.

Shipping builds leave it out entirely.

```bash
cargo xtask run --platform macos --features dev -- --debug-overlay=all
```

It works in offscreen captures too:

```bash
cargo xtask run --platform desktop --features dev -- --capture overlay.png --size 1280x720 --debug-overlay=all
```

## Hot reload and live link

`dev` builds take edits while they run, with no restart.

**Hot reload (desktop)** watches the cooked packs, the renderer's WGSL, and the time-of-day, light rig and screen effect documents:

- a saved document shows within a few tenths of a second;
- a saved shader builds its pipelines off the frame thread and swaps them in;
- after `cargo xtask content`, the assets that changed swap in behind their handles.

**Live link (any platform)** takes the same edits from the desktop over TCP, on 127.0.0.1:7811 by default. Use `iproxy` for an iPhone, and `adb reverse` or `adb forward` for Android. Build the app and the tool from the same commit: the protocol is at version 6, and different versions refuse each other.

```bash
cargo xtask link status                                   # what the app runs, its assets and their generations
cargo xtask link push target/content/mobile/shared.spack  # sends only the assets that changed
cargo xtask link push crates/sapphire_world/default_day.ron crates/sapphire_render/shaders/post.wgsl
cargo xtask link cvar post.filter_strength 0.5
cargo xtask link capture frame.png                        # the next frame the app draws
cargo xtask link watch                                    # push whatever changes, until stopped
```

[m1-exit-run.md](m1-exit-run.md) walks through both on the desktop, the simulators and phones.

## Without a window

The sandbox has offscreen modes that never open a window or take focus, so they're safe on a machine someone is using:

| Mode | What it does |
|---|---|
| `--capture <png> [--capture-ticks N] [--capture-frames F]` | Simulates N fixed ticks, draws F frames (TAA builds its history over several) and writes the last. The golden images run on it. |
| `--bench <frames>` | Runs the per-frame CPU path that many times, and logs CPU time and allocations per frame. |
| `--offscreen` (`dev` builds) | Draws at 60 frames a second until stopped, with hot reload and live link: what a phone runs, without a window. |

They render at `--size WxH` (1920x1080 by default). `--tier` picks a quality tier (`desktop-high`, `desktop-low`, `mobile-high`, `mobile-low`), `--time-of-day <hours>` sets the hour and `--day-length <seconds>` runs the day.

```bash
cargo xtask run --platform desktop --release -- --look-dev hero --time-of-day 18 --tier desktop-high --capture-ticks 90 --capture-frames 32 --capture hero.png --size 2560x1440
```

```bash
cargo xtask run --platform desktop --release -- --bench 600
```

On a phone, use `xcrun simctl io booted screenshot <png>` on the iOS Simulator, or `adb exec-out screencap -p > <png>` on Android. `cargo xtask link capture <png>` works with a `dev` build on any device.

## Golden images

`cargo xtask golden` renders the 376 golden scenes in [`tests/golden/scenes.ron`](../tests/golden/scenes.ron) with `--capture`, and compares each with its reference `tests/golden/<scene>.<family>.png` using FLIP.

- **Families:**
  - `metal`: any Apple GPU;
  - `vulkan-lavapipe`: Linux CI, in 3 shards;
  - `dx12-warp`: Windows CI, in 6 shards.

  Every pull request runs all three.
- **Gates:** the mean error, the 99th-percentile error and the worst 16×16 tile's mean error, not the maximum, because hard two-tone edges flip whole pixels between backends. Spec §9 has the numbers.
- **One scene:** `--scene <name>`. **A shard:** `--shard <index>/<count>`.
- **On failure:** `target/golden/` gets the capture (`.actual.png`) and a FLIP error map (`.diff.png`). CI uploads the folder for 7 days.
- **A new scene or an intended change:**
  1. A scene is a row in `scenes.ron`.
  2. `cargo xtask golden --update --scene <name>` writes this machine's family.
  3. The other families' images come from the CI run's artifacts. A passing run has them too, with each scene's time in `timings.json`, and that time becomes the row's `ci_seconds`.

## Replays

The simulation is deterministic. A replay (`tests/replays/*.sreplay`, 39 of them) records a fight's or a zone's input, and holds the state's checksums tick by tick.

```bash
cargo xtask replay                      # every replay; no GPU needed
cargo xtask replay --replay arena-boss  # one
cargo xtask replay --list               # scenarios, scripts and replays
```

A failure names the first tick that differs. When a change moves the simulation on purpose:

1. `cargo xtask replay --update` rewrites this machine's checksums.
2. Take x86-64's from CI's `replay-checksums-*` artifacts.

[development-iteration.md](development-iteration.md) has how to record a new replay.

## Packages, playtests and patches

| Command | Makes |
|---|---|
| `cargo xtask package --platform <platform>` | The game's store package under `target/package/`: the shipping profile, the platform's cook, the identity in `platform/identity.toml` and the icon in [`platform/icon/`](../platform/icon/), signed ad hoc and checked offline, with its symbols beside it. |
| `cargo xtask playtest --platform <platform>` | A playtest build that keeps a session's runs, marks and crash reports for [the playtest procedure](playtest.md). |
| `cargo xtask patch …` | Signed content releases and patches; a game patches itself by downloading only the blocks that changed (`cargo xtask help` lists the subcommands). |

The store checklists are in [`docs/store/`](store/), and what a first upload still needs is in [m5-handoff.md](m5-handoff.md), "Before a first upload".

## Everyday commands

```bash
cargo test-fast                                           # unit and integration tests, no doctests: the inner loop
cargo test --workspace --locked                           # what CI runs
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check-layers                                  # crate layering, xtask/layers.toml
cargo deny check
cargo xtask content --cook desktop                        # cook content/ into target/content/
cargo xtask bundle --platform android --release
```

- **`cargo xtask content`** cooks through a content cache. Only assets whose source, `.meta` sidecar, importer or cooker version changed are cooked again.
- **Sidecars:** the first cook of a new source asset creates its sidecar, which holds the asset's ID. Commit it with the source.
- **CI** runs xtask as `cargo --locked xtask --locked …`, so a build that would change `Cargo.lock`, or a source without a committed sidecar, fails.
- **The smallest useful check** for each kind of edit is in [development-iteration.md](development-iteration.md).

## Troubleshooting

- **SDL fails to configure on macOS with "unknown architecture arm64e.x1-macos".** The Command Line Tools SDK is newer than the active Xcode's linker. `cargo xtask` sets `SDKROOT` itself; for a plain `cargo` build, run this first:

  ```bash
  export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)
  ```

- **`cargo xtask bundle` reports a minimum OS that doesn't match Info.plist.** The deployment targets in `.cargo/config.toml` (macOS 14.0, iOS 17.0) and `platform/{macos,ios}/Info.plist` have drifted apart. Change them together, then run `cargo clean -p sdl3-sys`, since SDL doesn't rebuild on its own.
- **SDL for Android still targets the old API level or NDK.** Run `cargo clean -p sdl3-sys --target aarch64-linux-android`.
- **`cargo xtask run --platform android` says the AVD is locked.** An emulator for `sapphire_api36` is starting, or still running without reaching adb. If none is running, delete the lock file the message names.
- **The macOS window logs `skipping frames reason=Occluded`.** The window isn't visible, perhaps on another Space. The loop idles at 30 Hz until it is.
