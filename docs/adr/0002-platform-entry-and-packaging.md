# ADR 0002: Platform entry and packaging (M0)

- **Status:** accepted (M0)
- **Date:** 2026-10-24

## Context

M0 has to run the same app crate on macOS, Windows, Linux, iOS and Android. The spec (§5.1, §7.3) sketched an iOS static library inside an Xcode project and cargo-ndk for Android. Building M0 turned up simpler routes, plus a few platform constraints the renderer has to respect.

## Decision

1. **SDL3 main callbacks on every platform.**
   - The app implements `sapphire_platform::App`.
   - Desktop and iOS call `sapphire_platform::run::<A>()` from `fn main`. `SDL_RunApp` hands the thread to `UIApplicationMain` on iOS.
   - Android uses `export_android_main!(A, init = setup)` in the app's `lib.rs`.
   - SDL is built from source by `sdl3-sys` and linked statically everywhere, so no platform ships an SDL library or framework.
2. **iOS is a plain Rust executable.** `cargo xtask bundle --platform ios-sim` builds the binary and wraps it in an ad-hoc-signed `.app` from `platform/ios/Info.plist`, with cooked content in `content/`. It runs on the iOS 26.3 simulator. An Xcode project arrives with the M4 store work; it is what device signing, sentry-cocoa, the privacy manifest and store archives need.
3. **Android builds the app library as a cdylib.**
   - xtask sets the NDK environment itself; there is no cargo-ndk.
   - `export_android_main!` exports `SDL_main`, plus SDL's statically linked JNI symbols, from `libsapphire_sandbox.so`.
   - `platform/android` is a minimal Gradle project (AGP 9.4, minSdk 29, targetSdk 36, arm64-v8a, Vulkan 1.1 required).
   - SDL's Java glue is synced from the `sdl3-src` crate that matches `Cargo.lock`, and is never committed.
   - NDK r28+ produces 16 KB-aligned libraries by default.
4. **One portable CPU-driven draw path.**
   - The iOS simulator lacks base-vertex / first-instance drawing, indirect execution, cube arrays and comparison samplers.
   - So the CPU-driven path keeps indices mesh-relative and emulates base vertex by binding the shared vertex buffer at each mesh's offset. Per-draw records (instance and material) arrive as an instance-step vertex attribute.
   - This keeps per-instance skinned vertex buffers and pool compaction possible (M1).
   - The GPU-driven multi-draw-indirect path (later, desktop) uses real `base_vertex` and `first_instance`, and is gated on `GpuCaps::base_vertex_first_instance`.
5. **Crash capture per platform.**
   - Desktop: an out-of-process minidump monitor, which is the app re-executed with a guard.
   - Android: an in-process signal record that chains to `debuggerd`; Sentry's Android NDK SDK supplies symbolicated native crashes.
   - iOS: panic reports now; sentry-cocoa arrives with the Xcode project.
6. **Apple build environment.**
   - `.cargo/config.toml` forces `MACOSX_DEPLOYMENT_TARGET=14.0` and `IPHONEOS_DEPLOYMENT_TARGET=17.0` (spec §2) for every cargo invocation, and `xtask bundle` verifies the result against the Info.plist minimums.
   - For macOS builds only, xtask sets `SDKROOT` from `xcrun` when the user hasn't. That works around Command Line Tools SDKs that are newer than the active Xcode's linker.
7. **Verification never takes focus.**
   - `sapphire_platform::run_with` with `RunOptions { background: true }` keeps a process from becoming the foreground app.
   - The sandbox's `--capture <png>` renders offscreen through it, without a window.
   - `xtask run --platform ios-sim` is headless unless given `--show`, and the Android emulator boots with `-no-window`.
   - Desktop visuals are checked with captures, mobile visuals with `simctl` / `adb` screenshots.

## Consequences

- **One command per platform.** `cargo xtask run --platform macos|ios-sim|android` builds, cooks, packages and launches.
- **Store packaging is still to come (M4):** the iOS Xcode project, the Android AAB, signing and notarization.
- **The Android emulator needs adjustments:**
  - Its Vulkan driver crashes on debug object labels, so the app turns labels off when `SystemInfo::is_emulator` is true.
  - Only `-gpu host` gives hardware Vulkan; it goes through MoltenVK.
- **iOS frame pacing is SDL's.** With main callbacks, SDL's private display link drives `iterate` at up to 120 Hz on ProMotion. Open question Q13 tracks this.
