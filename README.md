# Sapphire

Sapphire is a Rust game engine built for 3D anime-aesthetic action games in the vein of Zenless Zone Zero and Wuthering Waves. It targets **macOS, Windows, Linux, iOS and Android**.

![The look-dev stage at golden hour. An anime girl with long chestnut hair, a white T-shirt and black shorts stands in a flagstone square, in three-quarter view. Behind her on the left is a pavilion with dark red columns and a sweeping charcoal tile roof, the low sun glowing over its eaves. On the right a white plastered wall opens through a round moon gate onto a street strung with paper lanterns. Long blue-violet shadows cross the paving, under a lavender sky and a pale mountain in haze.](docs/media/m1-lookdev.png)

**Status: M1 is done, pending devices (2026-10-27).** M1 built the engine's data path and its anime look:

- **Data path:** `sapphire_pipeline` imports glTF, VRM 1.0 and FBX models, textures and RON documents. It cooks them per platform (BC on desktop, ASTC on phones) through a content-hash cache into packs, which the runtime loads asynchronously, by asset ID, behind handles.
- **Characters:** anime character shading, with authored ramps and ILM maps, SDF face shadows, layered eyes, hair highlights, a depth-offset rim, inverted-hull outlines, material layers and a light rig of their own.
- **World:** one time-of-day key moves the sky, fog, ambient light, shadow tint, the character light rig and the grade. Under it are a stylized environment shader, cascaded shadows, ambient probes and AO, and NPR foliage in the wind.
- **Post:** HDR with bloom, a diffusion glow, gradient layers and a custom grade, TAA on desktop and 4× MSAA on phones, and a stack of combat screen effects.
- **Iteration:** hot reload on the desktop, and live link to any development build. Content, materials, time-of-day keys, cvars and shaders change in the running app without a restart.
- **Golden images:** 95 scenes, compared on Metal, Vulkan (lavapipe) and DX12 (WARP) on every pull request.

The look-dev stage (`--look-dev`) puts it all together: a VRM character in a Jinzhou-style town, through a day and a night.

It has been verified on macOS (Apple Silicon, Metal), on the iOS 26.3 Simulator and on the Android 16 emulator. Windows and Linux build, pass their tests and render the golden images in CI, on software rasterizers. Nothing has run on a phone or on Windows or Linux hardware yet, and those checks are still owed. The spec's [§10](docs/engine-spec.md#10-milestones) has the M1 status, and the [M2 handoff notes](docs/m2-handoff.md) what comes next.

## Documents

- [Engine spec](docs/engine-spec.md): what gets built, and how.
- [M1 plan](docs/m1-plan.md): M1's work packages, each with an "As built" note.
- [M1 exit run](docs/m1-exit-run.md): how to run M1's exit tests on the desktop, simulators and phones.
- [M2 handoff notes](docs/m2-handoff.md): what M2 inherits from M1.
- [Research report](reports/Rust%20anime%20action%20game%20engine.md): why Rust, and why these libraries.
- [Conventions](docs/conventions.md): how the code should read.
- [Privacy safeguards](docs/privacy.md): safe commit identities, local hooks and the CI guard.
- [Architecture decisions](docs/adr/).

## Requirements

| For | Install |
|---|---|
| Everything | `rustup`. The toolchain and targets are pinned in `rust-toolchain.toml` and installed automatically. CMake and Ninja are needed to build SDL3 from source. |
| macOS / iOS | Xcode 26 or newer, plus an iOS Simulator runtime (`xcodebuild -downloadPlatform iOS`). |
| Android | JDK 17 (with `JAVA_HOME` set for Gradle) and the Android SDK (`ANDROID_HOME`, or the default `~/Library/Android/sdk`), with NDK r28 or newer, `platforms;android-36`, `build-tools;36.0.0`, `platform-tools` and an emulator image. `cargo xtask run --platform android` boots an AVD named `sapphire_api36` when no device is connected, using `-gpu host` for hardware Vulkan. |
| Linux | The SDL3 build dependencies (X11, Wayland, xkbcommon, udev development headers). |

## Quick start

On any desktop host (Windows, Linux or macOS):

```bash
cargo xtask run --platform desktop
```

On a Mac, for the macOS app bundle, the iOS Simulator or Android:

```bash
cargo xtask run --platform macos
```

```bash
cargo xtask run --platform ios-sim
```

```bash
cargo xtask run --platform android
```

Each command cooks the sample content, builds the sandbox, packages it where the platform needs that, launches it and streams its log here. Arguments after `--` go to the app; for example, `-- --frames 600` exits after 600 presented frames, and `-- --look-dev hero` shows the look-dev stage (also `wide`, `sky`, `street` and `face`). `--release` builds optimized, `--features dev` adds the debug overlay, hot reload and live link, and `--dry-run` builds and packages but prints the launch command instead of running it.

- **desktop** runs the build in place against `target/content/desktop/` (through `SAPPHIRE_CONTENT_DIR`). Use it to test on Windows and Linux hardware. `cargo xtask bundle --platform desktop` makes a self-contained `target/desktop/SapphireSandbox/` directory (the executable plus `content/`) to copy to another machine.
- **macos** builds and signs `target/macos/SapphireSandbox.app`, then runs the executable inside it, which reads the content bundled with it as a player's copy does. A `--features dev` build reads `target/content/desktop/` instead (through `SAPPHIRE_CONTENT_DIR`, unless you set it), so `cargo xtask content` reaches it while it runs.
- **ios-sim** boots a simulator headless if none is booted; add `--show` to also open Simulator.app.
- **android** uses the connected device, or boots the `sapphire_api36` AVD headless (`-no-window -gpu host`), and returns when the app exits.

**Sandbox controls:**

| Input | Action |
|---|---|
| Drag (mouse or finger), arrow keys, right stick | Orbit |
| Wheel, pinch, triggers | Zoom |
| `Space`, gamepad South or a double tap | World slow motion |
| `H`, gamepad East or a long press | Hitstop |
| `Esc` (desktop) | Quit |
| `` ` `` (the key below `Esc`), or hold two fingers still for a second | Show or hide the debug overlay (`dev` builds) |

The bindings are a `sapphire_input` document, [`apps/sapphire_sandbox/default_bindings.ron`](apps/sapphire_sandbox/default_bindings.ron).

### Debug overlay

Development builds add the debug overlay (`sapphire_debug`, egui): frame time and CPU stages against the spec's budgets, renderer and frame-graph stats, the frame graph pass by pass with GPU timings, cvars and a console, and time controls (pause, step one tick, speed). Build with the sandbox's `dev` feature; shipping builds leave it out entirely:

```bash
cargo xtask run --platform macos --features dev
```

`--debug-overlay` shows it from the first frame, optionally with a list of panels (`stats`, `graph`, `time`, `day`, `effects`, `console`, `cvars` or `all`). The `day` panel scrubs time of day: the day strip, every value it resolves, and a character preview under the scrubbed key light. It works in offscreen captures too, for checking it without a window:

```bash
cargo xtask run --platform desktop --features dev -- --capture overlay.png --size 1280x720 --debug-overlay=all
```

While the overlay shows, input over its windows is its own and the game gets the rest. In the console, `list` shows every cvar, `<name> <value>` sets one and Tab completes names.

### Hot reload and live link

Development builds swap edits in while they run. On a desktop, hot reload watches the cooked packs, the renderer's WGSL sources and the time-of-day, light rig and screen effect documents (under `--look-dev`, the stage's own day and light rigs): save a document and it shows within a few tenths of a second; save a shader and its pipelines build off the frame thread, so frames keep drawing until the new ones swap in (about two seconds on a Mac with a cold shader cache); edit content and run `cargo xtask content`, and the assets that changed are swapped in behind their handles. On any platform, live link takes the same edits from the desktop over TCP (127.0.0.1:7811 by default; `iproxy` for an iPhone, `adb reverse` or `adb forward` for Android). Build the app and the tool from the same commit: live link's protocol is at version 3, and different versions refuse each other.

```bash
cargo xtask link status                                   # what the app runs, its assets and their generations
cargo xtask link push target/content/mobile/shared.spack  # sends only the assets that changed
cargo xtask link push crates/sapphire_world/default_day.ron crates/sapphire_render/shaders/post.wgsl
cargo xtask link cvar post.filter_strength 0.5
cargo xtask link capture frame.png                        # the next frame the app draws
cargo xtask link watch                                    # push whatever changes, until stopped
```

[docs/m1-exit-run.md](docs/m1-exit-run.md) walks through all of it on the desktop, the simulators and phones.

## Checking visuals and performance without a window

On a desktop, the sandbox has three offscreen modes. None opens a window, starts the display stack or takes focus, so they're safe to run while someone is using the machine:

- `--capture <png> [--capture-ticks N] [--capture-frames F]` simulates N fixed ticks, draws F frames (1 by default; TAA builds its history over several) and writes the last as a PNG. The golden-image tests run on it.
- `--bench <frames>` runs the per-frame CPU path (fixed step, `Renderer::prepare`, encoding, submit) that many times and logs CPU time and allocations per frame.
- `--offscreen` (`dev` builds) keeps drawing at 60 frames a second until stopped (Ctrl-C), with hot reload and live link on: what a phone runs, without a window. `cargo xtask link capture <png>` shows what it draws.

They render at `--size WxH` (default 1920x1080). `--time-of-day <hours>` sets the hour (default 10, the default day's key that matches the look from before time of day) and `--day-length <seconds>` runs the day cycle, in these modes and in the window alike. `xtask run --platform desktop` cooks the content and points the app at it:

```bash
cargo xtask run --platform desktop --release -- --bench 600
```

```bash
cargo xtask run --platform desktop -- --capture out.png --capture-ticks 120 --size 1600x900
```

The image at the top of this page is the look-dev hero shot at 18:00, rendered this way with the flags of its golden scene, `lookdev-town-hero`, at 1280×720:

```bash
cargo xtask run --platform desktop --release -- --capture-ticks 90 --look-dev hero --time-of-day 18 --tier desktop-high --ao off --capture-frames 32 --capture docs/media/m1-lookdev.png --size 1280x720
```

On mobile, use `xcrun simctl io booted screenshot <png>` (iOS Simulator) or `adb exec-out screencap -p > <png>` (Android) while the app runs, or `cargo xtask link capture <png>` with a `dev` build on any device.

### Golden images

`cargo xtask golden` renders the golden scenes (95 of them, in `tools/sapphire_testkit/src/scenes.rs`) with `--capture` (640×360) and compares each with its reference in `tests/golden/<scene>.<backend family>.png` using FLIP. `--scene <name>` runs one scene, and `--shard <index>/<count>` every count-th scene from the index-th.

- **Backend families:** `vulkan-lavapipe` (Linux CI), `dx12-warp` (Windows CI), `metal` (any Apple GPU, including CI's).
- **CI** runs every scene on every pull request: lavapipe in two shards and WARP in four, both required, and Metal in one job, informational until it has run clean for two weeks.
- **Gates:** mean and 99th-percentile error, not the maximum, because hard two-tone edges flip whole pixels between backends.
- **On failure:** `target/golden/` gets the capture (`.actual.png`) and a FLIP error map (`.diff.png`).
- **CI artifacts:** every golden job uploads `target/golden/` (each scene's capture, plus the failures' diffs), kept for 7 days.
- **New scene or intended change:** `cargo xtask golden --update` writes this machine's family. The other families' images come from the CI run's artifacts, which a passing run has too.

## Everyday commands

```bash
cargo test-fast
```

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo xtask check-layers
```

```bash
cargo deny check
```

```bash
cargo xtask content --cook desktop
```

```bash
cargo xtask bundle --platform android --release
```

See [Development iteration](docs/development-iteration.md) for focused test commands, build profiles and measured costs.

`cargo test-fast` is the inner loop: unit and integration tests without doctests, which Cargo recompiles on every run (CI runs everything). `cargo xtask check-layers` enforces [xtask/layers.toml](xtask/layers.toml): where each crate sits, and which features it may declare. `cargo xtask content` cooks `content/` into `target/content/` with the optimized `tools` profile through a content cache, so only assets whose source, `.meta` sidecar settings or importer and cooker versions changed are cooked again. The first cook of a new source asset creates its sidecar, which holds the asset's ID: commit it with the source. `cargo xtask bundle` builds the installable package for a platform under `target/<platform>/` without launching it. CI runs xtask as `cargo --locked xtask --locked …`, so neither xtask's build nor the builds it runs may change `Cargo.lock`, and a source without a committed sidecar fails the cook.

## Layout

```
crates/   runtime crates (they ship), layered L0–L4, and the dev-only debug overlay, hot reload and live link; see ADR 0001
apps/     shipping binaries (sapphire_sandbox)
tools/    never-shipped tools (sapphire_pipeline, sapphire_testkit, headless Blender scripts)
xtask/    build automation and the layer checker
platform/ per-platform packaging (Info.plist templates, Android Gradle project)
content/  source assets and their .meta sidecars, third-party content with its licenses
tests/    golden images, one per scene and backend family
docs/     spec, M1 plan and exit run, M2 handoff, conventions, ADRs
```

## Troubleshooting

- **SDL fails to configure on macOS with "unknown architecture arm64e.x1-macos".** The Command Line Tools SDK is newer than the active Xcode's linker. `cargo xtask` sets `SDKROOT` for macOS builds itself. For plain `cargo` builds, run the following first:

  ```bash
  export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)
  ```

  The deployment targets (macOS 14.0, iOS 17.0) come from `.cargo/config.toml`, so every build agrees on them.

- **`cargo xtask bundle` reports a minimum OS that doesn't match Info.plist.** The deployment targets in `.cargo/config.toml` and `platform/{macos,ios}/Info.plist` have drifted apart; change them together. SDL doesn't rebuild when they change, so after changing them run `cargo clean -p sdl3-sys`.

- **SDL for Android still targets the old API level or NDK.** SDL doesn't rebuild when the NDK or `minSdk` changes. Run `cargo clean -p sdl3-sys --target aarch64-linux-android`.

- **`cargo xtask run --platform android` says the AVD is locked.** An emulator for `sapphire_api36` is starting or still running without reaching adb. If none is running, delete the lock file the message names.

- **The macOS window logs `skipping frames reason=Occluded`.** The window isn't visible, for example because it opened on another Space. The loop idles at 30 Hz until the window is visible again.

## License

Sapphire is licensed under the [MIT License](LICENSE).

Third-party content under [content/third_party/](content/third_party/) keeps its own licenses (CC0 and the VRM Public License 1.0); each folder's `SOURCE.md` gives the author, source and terms.
