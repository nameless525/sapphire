# Sapphire

Sapphire is a Rust game engine built for a stylized 3D anime action game in the vein of Zenless Zone Zero and Wuthering Waves. It targets **macOS, Windows, Linux, iOS and Android**.

![The M0 sandbox on macOS: a toon-shaded mannequin with inverted-hull outlines](docs/media/m0-macos.png)

**Status: milestone M0 (bring-up).** It has been verified on macOS (Apple Silicon, Metal), on the iOS 26.3 simulator (Metal) and on the Android 16 emulator (Vulkan). Windows and Linux build in CI but haven't run on hardware yet. The sandbox app runs the whole engine path:

1. SDL3 platform layer.
2. Fixed 60 Hz simulation on the engine clocks.
3. Plain-data frame view.
4. wgpu toon renderer (two-tone shading, rim light, inverted-hull outlines, 4× MSAA, reverse-Z).
5. A cooked sample character.

Crash capture, logging, the job system and the asset pipeline are all in place.

## Documents

- [Engine spec](docs/engine-spec.md): what gets built, and how.
- [Research report](reports/Rust%20anime%20action%20game%20engine.md): why Rust, and why these libraries.
- [Conventions](docs/conventions.md): how the code should read.
- [Architecture decisions](docs/adr/).

## Requirements

| For | Install |
|---|---|
| Everything | `rustup`. The toolchain and targets are pinned in `rust-toolchain.toml` and installed automatically. CMake and Ninja are needed to build SDL3 from source. |
| macOS / iOS | Xcode 26 or newer, plus an iOS Simulator runtime (`xcodebuild -downloadPlatform iOS`). |
| Android | JDK 17 (with `JAVA_HOME` set for Gradle) and the Android SDK (`ANDROID_HOME`, or the default `~/Library/Android/sdk`), with NDK r28 or newer, `platforms;android-36`, `build-tools;36.0.0`, `platform-tools` and an emulator image. `cargo xtask run --platform android` boots an AVD named `sapphire_api36` when no device is connected, using `-gpu host` for hardware Vulkan. |
| Linux | The SDL3 build dependencies (X11, Wayland, xkbcommon, udev development headers). |

## Quick start

```bash
cargo xtask run --platform macos
```

```bash
cargo xtask run --platform ios-sim
```

```bash
cargo xtask run --platform android
```

Each command cooks the sample content, builds the sandbox, packages it for the platform and launches it. Arguments after `--` go to the app; for example, `-- --frames 600` exits after 600 frames.

**Sandbox controls:**

| Input | Action |
|---|---|
| Drag (mouse or finger), arrow keys, right stick | Orbit |
| Wheel, pinch, triggers | Zoom |
| `Space` or gamepad South | World slow motion |
| `H` or gamepad East | Hitstop |

## Everyday commands

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
cargo xtask content --cook desktop
```

`cargo xtask check-layers` enforces [xtask/layers.toml](xtask/layers.toml), and `cargo xtask content` cooks `content/` into `target/content/`.

## Layout

```
crates/   runtime crates (they ship), layered L0–L3, see ADR 0001
apps/     shipping binaries (sapphire_sandbox)
tools/    never-shipped tools (sapphire_pipeline)
xtask/    build automation and the layer checker
platform/ per-platform packaging (Info.plist templates, Android Gradle project)
content/  source assets
docs/     spec, conventions, ADRs
```

## Troubleshooting

- **SDL fails to configure on macOS with "unknown architecture arm64e.x1-macos".** The Command Line Tools SDK is newer than the active Xcode's linker. `cargo xtask` handles this. For plain `cargo` builds, run the following first:

  ```bash
  export SDKROOT=$(xcrun --sdk macosx --show-sdk-path) MACOSX_DEPLOYMENT_TARGET=14.0
  ```

- **The macOS window logs `skipping frames reason=Occluded`.** The window isn't visible, for example because it opened on another Space. The loop idles at 30 Hz until the window is visible again.
