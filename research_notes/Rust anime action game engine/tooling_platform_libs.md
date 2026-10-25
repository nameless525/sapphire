# Rust Platform, UI, Asset-Pipeline, Editor and Developer-Tooling Libraries for a Wuthering Waves / ZZZ-Style Engine (as of 2026-10-24)

*Method note: Versions, release dates, licenses and download counts were pulled live from the crates.io API on 2026-10-24. Repository activity (last push date, stars, archived flag) was pulled from the GitHub API the same day. Each crate row cites its crates.io page and, where useful, its repository. "Last push" is repository activity, which is not the same as a release. Rendering/GPU crates and runtime gameplay crates (ECS, physics, animation, audio, scripting) are out of scope. All recommendations appear under "Inferences" and are opinion.*

## 1. Windowing, platform and input (winit vs SDL3, gilrs, raw-window-handle, mobile tooling, Steamworks, consoles)

### Takeaway
winit 0.30.x is still the stable de-facto standard. Bevy 0.20 and egui 0.37 both pin `winit ^0.30`. winit 0.31 has been in beta since Nov 2025 with large breaking changes (trait-based `Window`/`ActiveEventLoop`, unified pointer events, surface-size renames, split backend crates). SDL3 is the more complete game platform layer: it covers gamepad rumble, trigger rumble, LEDs, gyro, touchpads, Steam integration, iOS/Android, and has public Xbox GDK support. Its Rust high-level binding (`sdl3` 0.20) still describes itself as "migration in progress", so the safer path is `sdl3-sys` plus your own thin wrapper. gilrs has no Android support (and no iOS listed) and no force feedback on macOS. Rust has no public PlayStation or Xbox targets. Console work requires NDA toolchains.

### Cited Findings

**Version and maintenance snapshot**

| Crate | Latest stable (date) | Pre-release | License | Repo activity | Source |
|---|---|---|---|---|---|
| winit | 0.30.13 (2026-03-02) | 0.31.0-beta.3 (2026-09-04) | Apache-2.0 | 6.2k stars, pushed 2026-10-24 | [crates.io](https://crates.io/crates/winit), [GitHub](https://github.com/rust-windowing/winit) |
| sdl3 (high-level) | 0.20.0 (2026-09-07) | – | MIT | 392 stars, pushed 2026-10-20 | [crates.io](https://crates.io/crates/sdl3), [GitHub](https://github.com/vhspace/sdl3-rs) |
| sdl3-sys (low-level) | 0.7.2+SDL-3.4.18 (2026-10-03) | – | Zlib | Codeberg | [crates.io](https://crates.io/crates/sdl3-sys), [lib.rs](https://lib.rs/crates/sdl3-sys) |
| sdl2 | 0.38.0 (2025-07-12) | – | MIT | 2.9k stars, pushed 2026-03 | [crates.io](https://crates.io/crates/sdl2), [GitHub](https://github.com/Rust-SDL2/rust-sdl2) |
| gilrs | 0.11.2 (2026-05-30) | – | Apache-2.0/MIT | main repo on GitLab, GitHub mirror | [crates.io](https://crates.io/crates/gilrs), [GitHub mirror](https://github.com/Arvamer/gilrs) |
| raw-window-handle | 0.6.2 (2024-05-17) | – | MIT/Apache/Zlib | ~37M recent downloads, pushed 2026-04 | [crates.io](https://crates.io/crates/raw-window-handle) |
| android-activity | 0.6.1 (2026-03-24) | – | MIT/Apache | pushed 2026-06 | [crates.io](https://crates.io/crates/android-activity), [GitHub](https://github.com/rust-mobile/android-activity) |
| ndk | 0.9.0 (2024-04-26) | – | MIT/Apache | pushed 2026-10 | [crates.io](https://crates.io/crates/ndk) |
| jni | 0.22.4 (2026-03-16) | – | MIT/Apache | – | [crates.io](https://crates.io/crates/jni) |
| cargo-mobile2 | 0.22.5 (2026-08-17) | – | Apache/MIT | 1.8k stars, pushed 2026-10-20 | [crates.io](https://crates.io/crates/cargo-mobile2), [GitHub](https://github.com/tauri-apps/cargo-mobile2) |
| xbuild | 0.2.0 (2022-12-21) | – | Apache/MIT | no release since 2022 (repo pushed 2026-07) | [crates.io](https://crates.io/crates/xbuild), [GitHub](https://github.com/rust-mobile/xbuild) |
| cargo-apk | 0.10.0 (2023-11-30) | – | MIT/Apache | – | [crates.io](https://crates.io/crates/cargo-apk) |
| cargo-ndk | 4.1.2 (2025-08-09) | – | Apache/MIT | ~850k recent downloads, pushed 2026-10-17 | [crates.io](https://crates.io/crates/cargo-ndk), [GitHub](https://github.com/bbqsrc/cargo-ndk) |
| objc2 | 0.6.5 (2026-10-05) | – | MIT | pushed 2026-10 | [crates.io](https://crates.io/crates/objc2) |
| swift-bridge | 0.1.59 (2026-01-06) | – | Apache/MIT | – | [crates.io](https://crates.io/crates/swift-bridge) |
| windows (windows-rs) | 0.62.2 (2025-10-06) | – | MIT/Apache | 12.8k stars, pushed 2026-10-24 | [crates.io](https://crates.io/crates/windows) |
| steamworks | 0.13.1 (2026-05-05) | – | MIT/Apache | 489 stars, pushed 2026-10-19 | [crates.io](https://crates.io/crates/steamworks), [GitHub](https://github.com/Noxime/steamworks-rs) |

**winit**
- winit 0.31.0-beta.1 changes include:
  - `ActiveEventLoop` and `Window` become traits, and `create_window` returns `Box<dyn Window>`.
  - The generic user event is replaced by `user_wake_up` / `EventLoopProxy::wake_up`.
  - `can_create_surfaces()` is split from `resumed/suspended()`, which are now emitted only on iOS, Web and Android. Render surfaces should be created only in `can_create_surfaces` for Android portability.
  - `inner_size` is renamed to `surface_size` throughout.
  - Pointer events are unified (`PointerMoved` / `PointerButton`, with `Touch` folded in, plus a new `PointerKind`/`PointerSource`) and pen input is added.
  - Additions: `Window::safe_area`, `ImeHints` / `request_ime_update`, and per-platform `WindowAttributes` structs.
  - MSRV rises to 1.85.
  - Source: [winit v0.31.0-beta.1 release notes](https://github.com/rust-windowing/winit/releases/tag/v0.31.0-beta.1)
- The winit repo is now split into backend crates (`winit-core`, `winit-appkit`, `winit-uikit`, `winit-android`, `winit-win32`, `winit-wayland`, `winit-x11`, `winit-web`, …). The unreleased changelog adds a clipboard API (`ActiveEventLoop::clipboard`) on Wayland, X11, macOS and Windows. — [winit repo](https://github.com/rust-windowing/winit)
- Release history: 0.30.12 (2025-07-27), 0.31.0-beta.1/2 (2025-11-16), 0.30.13 (2026-03-02), 0.31.0-beta.3 (2026-09-04). — [winit releases](https://github.com/rust-windowing/winit/releases)
- Bevy 0.20's `bevy_winit` depends on `winit ^0.30`, and `egui-winit` 0.37 depends on `winit ^0.30.13`. The major consumers have not moved to 0.31. — [bevy_winit 0.20 deps](https://crates.io/crates/bevy_winit/0.20.0/dependencies), [egui-winit 0.37 deps](https://crates.io/crates/egui-winit/0.37.0/dependencies)

**SDL3 (sdl3-rs / sdl3-sys)**
- The sdl3-rs README says: "Now that the SDL3 API is mostly stabilized, we are working on a new version of the Rust bindings… migration is in progress… Expect some bugs and missing features." Remaining unchecked items: "Add new features from SDL3" and "Update documentation". — [sdl3-rs README](https://github.com/revmischa/sdl3-rs) (points to [vhspace/sdl3-rs](https://github.com/vhspace/sdl3-rs) as canonical)
- Extension support in sdl3-rs: SDL_image, SDL_mixer, SDL_ttf and SDL_net are supported. Dear ImGui, RmlUI, SDL_sound and SDL_shadercross are not. — [sdl3-rs README](https://github.com/revmischa/sdl3-rs)
- sdl3-sys links a system SDL3 dynamically by default and offers build-from-source features. — [lib.rs sdl3-sys](https://lib.rs/crates/sdl3-sys)
- "SDL officially supports Windows, macOS, Linux, iOS, and Android, and several other platforms." — [sdl3-rs README](https://github.com/revmischa/sdl3-rs)
- SDL3 gamepad API features:
  - rumble (`SDL_RumbleGamepad`) and trigger rumble (`SDL_RumbleGamepadTriggers`)
  - LEDs (`SDL_SetGamepadLED`)
  - gyro/accelerometer sensors (`SDL_GamepadHasSensor`, `SDL_GetGamepadSensorData`)
  - touchpads
  - `SDL_SendGamepadEffect`
  - mapping database management
  - Steam handle access. For Steam games, `SteamAPI_InitEx()` must be called before `SDL_Init()`.
  - Source: [SDL3 wiki: CategoryGamepad](https://wiki.libsdl.org/SDL3/CategoryGamepad)
- SDL supports Windows (GDK) and Xbox One/Series (GDKX) at the C level. — [SDL README-gdk](https://skia.googlesource.com/external/github.com/libsdl-org/SDL/+/162f8e6ee75ba1921060dbd9cf500dec29082e21/docs/README-gdk.md)

**gilrs**
- Support matrix:
  - Linux/BSD (evdev): input, hotplug and force feedback
  - Windows: input, hotplug and force feedback
  - macOS: input and hotplug, but no force feedback
  - Wasm: input and hotplug
  - Android: no support at all; iOS is not listed
  - Supports SDL2 mappings including `SDL_GAMECONTROLLERCONFIG` "which Steam uses", rumble, and power info.
  - Source: [gilrs README (GitHub mirror)](https://github.com/Arvamer/gilrs)
- Bevy 0.20's `bevy_gilrs` depends on `gilrs ^0.11.0`. — [bevy_gilrs 0.20 deps](https://crates.io/crates/bevy_gilrs/0.20.0/dependencies)

**Mobile tooling**
- cargo-mobile2's maintenance status: "created for use as a base for mobile support in Tauri… We currently cannot offer support or fix issues for uses outside of Tauri." It generates Xcode and Android Studio projects. "It's not possible to target iOS on platforms other than macOS." — [cargo-mobile2 README](https://github.com/tauri-apps/cargo-mobile2)
- Fyrox 1.0.1 (`fyrox-core`) depends on `android-activity ^0.6.0`. — [fyrox-core deps](https://crates.io/crates/fyrox-core/1.0.1/dependencies)
- Rust target tiers:
  - Tier 1 with host tools: `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`
  - Tier 2 with host tools: `x86_64-apple-darwin`
  - Tier 2: `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `aarch64-linux-android`
  - Source: [rustc platform support](https://doc.rust-lang.org/rustc/platform-support.html)
- `x86_64-apple-darwin` was demoted from Tier 1 to Tier 2 in Rust 1.90. — [Phoronix](https://www.phoronix.com/news/Rust-1.90-Released)

**Consoles**
- The only console-related rustc targets are:
  - `aarch64-nintendo-switch-freestanding` (Tier 3, no_std only)
  - `mipsel-sony-psp` / `mipsel-sony-psx` (Tier 3, no_std)
  - `armv7-sony-vita-newlibeabihf` (Tier 3, requires VITASDK)
  - There are no Xbox targets and no PS4/PS5 targets.
  - Source: [rustc platform support](https://doc.rust-lang.org/rustc/platform-support.html)
- Rust users forum comments say console toolchains are "typically behind NDA", so contributors can't upstream the work. PlayStation is described as more secretive (informal, unverified). — [users.rust-lang.org thread](https://users.rust-lang.org/t/games-how-many-consoles-supported-rust/121384), [older thread](https://users.rust-lang.org/t/is-there-a-reliable-way-to-support-mainstream-console-including-std/64451)
- `switchbrew_bevy` (0.2.0, 2025-11-29) is a highly experimental, emulator-based homebrew effort. It quotes the Bevy Cheatbook that Rust's Switch support "isn't yet useful for Bevy". — [GitHub](https://github.com/ibrahimcesar/bevy-switch), [lib.rs](https://lib.rs/crates/switchbrew_bevy)
- windows-rs provides Rust projections of Win32/COM APIs ("call any Windows APIs… using the Rust language projection"). Searches found no official Rust crate for the Xbox GDK/GDKX; SDL's GDK port is C-level. — [Rust for Windows v0.9 announcement](https://blogs.windows.com/windowsdeveloper/2021/05/06/announcing-rust-for-windows-v0-9/), [SDL README-gdk](https://skia.googlesource.com/external/github.com/libsdl-org/SDL/+/162f8e6ee75ba1921060dbd9cf500dec29082e21/docs/README-gdk.md)

**Shipped-game reference point (Tiny Glade)**
- Tiny Glade (Pounce Light, two-person studio) was built with a custom/modified version of Bevy. Bevy's ECS is "the lifeblood of everything happening within the game". The team replaced Bevy rendering with their own Vulkan renderer, partly for validation layers and Linux support. — [80.lv interview](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games), [Wikipedia](https://en.wikipedia.org/wiki/Tiny_Glade)
- It sold more than 600k copies in about a month (HN thread title; secondary). — [Hacker News](https://news.ycombinator.com/item?id=42190065)
- The interview does not name its UI library, editor tooling, hot reload or profiling stack. — [80.lv interview](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games)

**Platform services (input-adjacent)**
- steamworks-rs 0.13.1 (May 2026) is actively maintained (pushed 2026-10-19). — [GitHub](https://github.com/Noxime/steamworks-rs)

### Inferences
- **Recommendation: SDL3 as the shipping platform layer, used through `sdl3-sys` (or a vendored fork of `sdl3`) behind your own `platform` crate.** Reasons:
  - WW/ZZZ-class combat needs mature controller support: DualSense/Xbox rumble, trigger rumble, gyro and LED.
  - The game needs mobile lifecycle handling.
  - SDL's C codebase is what console vendors' middleware ecosystems already expect (public GDK support). This is the single biggest de-risking step for a PC + console + mobile target.
  - Because the high-level `sdl3` crate is still migrating, wrap `sdl3-sys` directly for the subset you need: window, events, gamepad, haptics, sensors, text input/IME and display modes.
- **Alternative (faster start, Rust-native): winit 0.30 + gilrs**, with raw-window-handle for renderer interop. This is the path Bevy/egui take. Drawbacks:
  - It needs a separate gamepad solution on Android/iOS (gilrs has none).
  - It has no trigger rumble or gyro.
  - A large API migration to winit 0.31 lies ahead.
  - If you choose winit, consider starting directly on 0.31 betas so the surface-lifecycle model (`can_create_surfaces`) matches mobile from day one.
- Whichever is chosen, abstract it behind `raw-window-handle` 0.6 so the renderer (covered elsewhere) is decoupled.
- For mobile packaging, cargo-ndk (active) plus hand-maintained Gradle/Xcode projects are more reliable for a game than cargo-mobile2 (maintained only for Tauri) or xbuild/cargo-apk (stale releases).
- For consoles, budget for NDA-only work: a custom target spec, patched std or no_std+alloc, and vendor-SDK FFI. Keep platform code isolated so it can live in a private repo.

### Gaps
- No shipped commercial game using sdl3-rs was found. Shipped-game evidence for SDL is from C/C++ titles, which were not researched here.
- No stable release date for winit 0.31 was found.
- Whether PlayStation/Switch SDL ports are available to licensed developers could not be verified from public sources.
- DualSense adaptive-trigger and HD-haptics support in Rust (outside SDL effects) was not found.
- No public evidence was found of a Rust game shipped on PS5, Xbox Series or Switch.

## 2. Game UI for a polished, heavily animated, gacha-style commercial UI (bevy_ui, taffy, Slint, iced, egui, Noesis, RmlUi, Clay, Vello, text stacks, Rive)

### Takeaway
No Rust UI framework is proven for a AAA-style animated game UI. The strongest building blocks are:
- taffy for layout
- parley (or cosmic-text) plus harfrust and skrifa/swash for text
- Vello (CPU/GPU) or lyon for vector shapes

Noesis has an actively developed but tiny third-party Rust binding (Dead Money, Oct 2026). RmlUi has no Rust binding. The Rive Rust runtime (rive-rs) is GitHub-only, uses Vello, and has not been updated since mid-2025. bevy_ui is improving fast (directional navigation, parley text, BSN, Feathers) but is Bevy-bound.

### Cited Findings

**Version and maintenance snapshot**

| Crate | Latest (date) | License | Repo activity | Notes | Source |
|---|---|---|---|---|---|
| bevy_ui | 0.20.0 (2026-10-08) | MIT/Apache | Bevy 49k stars | taffy-based | [crates.io](https://crates.io/crates/bevy_ui) |
| taffy | 0.15.0 (2026-10-12) | MIT | 3.7k stars, pushed 2026-10-23 | flexbox/grid/block layout | [crates.io](https://crates.io/crates/taffy), [GitHub](https://github.com/DioxusLabs/taffy) |
| slint | 1.18.1 (2026-09-21) | GPL-3.0 OR Royalty-free-2.0 OR Slint Software 3.0 | 24k stars | – | [crates.io](https://crates.io/crates/slint) |
| iced | 0.14.0 (2025-12-07) | MIT | 31.8k stars, pushed 2026-10-21 | – | [crates.io](https://crates.io/crates/iced) |
| egui | 0.37.0 (2026-10-15) | MIT/Apache | 31.7k stars | immediate mode | [crates.io](https://crates.io/crates/egui) |
| noesis_runtime | 0.13.0 (2026-10-01) | MIT | 3 stars (dead-money) | requires Noesis SDK 3.2.13 | [docs.rs](https://docs.rs/noesis_runtime), [GitHub](https://github.com/dead-money/noesis_runtime) |
| noesis_bevy | 0.16.0 (2026-10-09) | MIT | 4 stars | Bevy/wgpu render device | [crates.io](https://crates.io/crates/noesis_bevy) |
| noesis_wgpu | 0.2.2 (2026-10-03) | – | – | wgpu render device | [crates.io search](https://crates.io/search?q=noesis) |
| clay-layout | 0.4.0 (2025-06-03) | non-standard | 144 stars, pushed 2026-08 | Clay C bindings | [crates.io](https://crates.io/crates/clay-layout), [GitHub](https://github.com/clay-ui-rs/clay) |
| vello | 0.11.0 (2026-10-02) | Apache/MIT | 4.4k stars | – | [crates.io](https://crates.io/crates/vello) |
| vello_cpu | 0.3.0 (2026-10-02) | Apache/MIT | – | ~6.1M recent downloads | [crates.io](https://crates.io/crates/vello_cpu) |
| vello_hybrid | 0.2.0 (2026-08-07) | Apache/MIT | – | – | [crates.io](https://crates.io/crates/vello_hybrid) |
| parley | 0.12.0 (2026-10-09) | Apache/MIT | pushed 2026-10-22 | – | [crates.io](https://crates.io/crates/parley) |
| cosmic-text | 0.19.0 (2026-04-22) | MIT/Apache | 2.2k stars | – | [crates.io](https://crates.io/crates/cosmic-text) |
| glyphon | 0.12.0 (2026-07-09) | MIT/Apache/Zlib | – | wgpu text on cosmic-text | [crates.io](https://crates.io/crates/glyphon) |
| harfrust | 0.15.0 (2026-10-22) | MIT | harfbuzz org | – | [crates.io](https://crates.io/crates/harfrust), [GitHub](https://github.com/harfbuzz/harfrust) |
| rustybuzz | 0.20.1 (2024-11-12) | MIT | **archived** | – | [GitHub](https://github.com/harfbuzz/rustybuzz) |
| skrifa (fontations) | 0.49.0 (2026-10-21) | MIT/Apache | googlefonts | – | [crates.io](https://crates.io/crates/skrifa) |
| swash | 0.2.10 (2026-07-17) | Apache/MIT | – | – | [crates.io](https://crates.io/crates/swash) |
| fontdue | 0.9.5 (2026-10-19) | MIT/Apache/Zlib | – | – | [crates.io](https://crates.io/crates/fontdue) |
| lyon | 1.0.19 (2026-03-08) | MIT/Apache | – | vector tessellation | [crates.io](https://crates.io/crates/lyon) |
| femtovg | 0.27.0 (2026-08-31) | MIT/Apache | – | – | [crates.io](https://crates.io/crates/femtovg) |
| rive-rs | not on crates.io | – | 134 stars, last push 2025-07-04 | – | [GitHub](https://github.com/rive-app/rive-rs) |
| rive-bevy | not on crates.io | – | 131 stars, last push 2025-08-14 | – | [GitHub](https://github.com/rive-app/rive-bevy) |
| bevy_lunex | 0.7.0 (2026-08-31) | MIT/Apache | 970 stars | – | [crates.io](https://crates.io/crates/bevy_lunex) |
| bevy_hui | 0.8.0 (2026-10-11) | MIT/Apache | – | – | [crates.io](https://crates.io/crates/bevy_hui) |
| haalka | 0.7.1 (2026-02-11) | MIT/Apache | – | – | [crates.io](https://crates.io/crates/haalka) |
| bevy_cobweb_ui | 0.22.2 (2026-01-14) | MIT/Apache | **archived** ("No longer maintained as of 01/13/2026") | – | [GitHub](https://github.com/UkoeHB/bevy_cobweb_ui) |
| sickle_ui | 0.4.0 (2024-10-03) | MIT/Apache | repo no longer reachable via GitHub API | – | [crates.io](https://crates.io/crates/sickle_ui) |
| kayak_ui | 0.5.0 (2024-02-11) | – | last push 2024-07 (stale) | – | [crates.io](https://crates.io/crates/kayak_ui) |
| xilem / masonry | 0.4.0 (2025-10-29) | Apache-2.0 | – | – | [crates.io](https://crates.io/crates/xilem) |
| makepad-widgets | 1.0.0 (2025-05-13) | MIT/Apache | – | – | [crates.io](https://crates.io/crates/makepad-widgets) |
| freya | 0.4.3 (2026-08-30) | MIT | – | 0.5.0-beta.12 | [crates.io](https://crates.io/crates/freya) |
| floem | 0.2.0 (2024-11-14) | MIT | – | – | [crates.io](https://crates.io/crates/floem) |

**bevy_ui progress (official release notes)**
- 0.17 (2025-09-30): experimental headless widgets (Button, Slider, Scrollbar, Checkbox, RadioGroup). Feathers is a "tooling-focused" widget set for the upcoming editor. — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- 0.18 (2026-01-13): automatic directional navigation "with gamepads and keyboards", `Popover`/`MenuPopup`, `Val` interpolation, variable font weights, underline/strikethrough, OpenType features. — [Bevy 0.18](https://bevy.org/news/bevy-0-18/)
- 0.19 (2026-06-19): text entry (`EditableText`). Bevy moved text from cosmic-text to parley because parley had "meaningfully better documentation". BSN scene system landed. — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)
- 0.20 (2026-10-08): `Val::Em`/`Rem`, `FixedNode`, elliptical `CornerRadius`, headless Tab widgets, `InlineBox`/`InlineImage` for rich text, contextual theming for Feathers. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- `bevy_text` 0.20 depends on `parley ^0.11` and `swash ^0.2.6`. — [bevy_text deps](https://crates.io/crates/bevy_text/0.20.0/dependencies)

**Text shaping**
- rustybuzz is "not developed further, unmaintained, and archived". Users should switch to HarfRust. — [rustybuzz README](https://github.com/harfbuzz/rustybuzz)

**Slint licensing**
- The Royalty-free 2.0 license covers proprietary desktop, mobile and web apps at no cost. It excludes embedded systems and requires attribution (AboutSlint widget or a badge on the download page). Whether consoles count as "embedded" is not addressed. — [Slint Royalty-free 2.0 text](https://unpkg.com/slint-ui@1.12.0/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md), [Slint LICENSE](https://app.unpkg.com/slint-ui@1.17.1/files/LICENSE.md)

**egui positioning**
- egui "aims to be the best choice when you want a simple way to create a GUI, or you want to add a GUI to a game engine". It is used for the Rerun Viewer, and Rerun sponsors its development. — [egui README](https://github.com/emilk/egui)

**Noesis**
- `noesis_runtime` provides safe Rust bindings over a hand-written C ABI to the Noesis Native SDK (XAML). It is renderer-agnostic and "does no drawing itself". `noesis_bevy` supplies a Bevy/wgpu render device. The owner is "brandon-reinhart" / Dead Money, so this is not an official Noesis Technologies product. docs.rs lists only an x86_64 Linux build. — [docs.rs noesis_runtime](https://docs.rs/noesis_runtime), [docs.rs noesis_bevy](https://docs.rs/noesis_bevy)

**RmlUi**
- No Rust crate was found (`rmlui` and `rmlui-rs` return 404 on crates.io). sdl3-rs lists RmlUI as "Not currently supported". — [sdl3-rs README](https://github.com/revmischa/sdl3-rs)

**Vello**
- The README now offers three renderers:
  - "Vello CPU… is the most mature choice"
  - "Vello GPU… is intended to become the primary renderer for production GPU use cases as it matures". It does not require compute shaders and supports WebGL2 and wgpu.
  - the original compute-based `vello` "remains an experimental implementation".
  - Source: [Vello README](https://github.com/linebender/vello)

**Rive**
- rive-rs is "A Rust runtime library for Rive". It "uses Vello as a render back-end, which has certain limitations… Efforts are underway to incorporate the Rive Renderer as another back-end." — [rive-rs README](https://github.com/rive-app/rive-rs)
- The original 2023 Vello backend README says it is "a work-in-progress and might exhibit artifacts" and was "only tested on macOS". — [rive-cpp vello README](https://skia.googlesource.com/external/github.com/rive-app/rive-cpp/+show/62c29e70cd4453155234275717b8b02283ef770a/vello/README.md)
- The crates.io crate named `rive` (1.2.0, 2023, LGPL) is an unrelated project. — [crates.io](https://crates.io/crates/rive)

### Inferences
- **Recommendation: build a custom retained-mode game UI runtime, not a third-party framework.** Suggested stack:
  - taffy for layout
  - parley for text layout, with harfrust/skrifa underneath
  - a GPU-atlas (SDF/MSDF) glyph cache for CJK-heavy localization
  - your own batched sprite/9-slice/mask/gradient renderer on the engine's renderer
  - a first-class tween/timeline/state-machine animation system, data-driven from an authoring format
  - Rationale: gacha-style UIs (character showcases, banner pulls, animated menus with particles and 3D model insets) depend on tight integration with the 3D renderer, post-FX and animation curves. No Rust framework offers that today.
- If a designer-facing authoring tool is a must:
  - **Noesis** (commercial, XAML, Blend tooling) is the most "proven-in-shipped-games" option, via its C++ SDK. Its Rust binding is a one-team project with only a few stars, so plan to own or fork it.
  - **Rive** gives designer-authored vector animation for UI elements, but the Rust runtime is stale (last push 2025-07). A production integration would likely need your own FFI to the C++ rive-runtime and the Rive Renderer.
- Use bevy_ui only if the engine is Bevy-based. Its 0.18–0.20 trajectory (directional gamepad navigation, rich text, theming) is good, but it is not designed for heavy, bespoke motion design.
- Do not use Slint, iced, Xilem, Freya or Makepad for in-game UI. They are app-UI toolkits. Slint also adds licensing ambiguity for consoles.
- Use egui only for debug overlays and tools.
- Clay (via clay-layout) is an interesting immediate-mode layout model, but its Rust bindings are small and taffy is better maintained.
- Avoid archived or stale crates: rustybuzz (use harfrust), bevy_cobweb_ui, sickle_ui, kayak_ui.

### Gaps
- No Rust game with a heavily animated, gacha-style UI was found as a reference.
- Noesis's console-platform support through the Rust binding, and pricing/licensing terms, were not verified.
- The status of a Rive-Renderer-based Rust backend after July 2025 is unknown.
- Comparative performance data (draw calls, CJK glyph-atlas memory) for parley vs cosmic-text in games was not found.

## 3. Editor and tools UI (egui stack, gizmos, Dear ImGui, Fyrox/Bevy editors, Blender-as-editor, Tauri)

### Takeaway
egui is the most active, best-maintained choice for an in-house editor:
- egui 0.37 (Oct 2026)
- egui_dock or egui_tiles for docking
- transform-gizmo 0.11 for gizmos
- egui-snarl for node graphs

Dear ImGui is again viable through the new, very active `dear-imgui-rs` (Oct 2026: docking, ImGuizmo, ImPlot, node editor, wgpu/ash/sdl3/bevy backends), while `imgui-rs` has not released since May 2024. The Bevy editor is still unshipped: BSN and Feathers are its foundations as of 0.20. Fyrox 1.0 (March 2026) is the best open-source reference for a full Rust editor architecture. Blender-as-level-editor via glTF extras (Skein) is a pragmatic, actively maintained pattern.

### Cited Findings

**Version and maintenance snapshot**

| Crate/tool | Latest (date) | License | Repo activity | Source |
|---|---|---|---|---|
| egui / eframe / egui-wgpu | 0.37.0 (2026-10-15) | MIT/Apache | 31.7k stars | [crates.io](https://crates.io/crates/egui) |
| egui_dock | 0.21.1 (2026-08-06) | MIT | 645 stars, pushed 2026-09-23 | [crates.io](https://crates.io/crates/egui_dock) |
| egui_tiles | 0.18.0 (2026-10-19) | MIT/Apache | now in rerun-io/egui_crates, pushed 2026-10-23 | [crates.io](https://crates.io/crates/egui_tiles) |
| transform-gizmo (+ `-egui`, `-bevy`) | 0.11.0 (2026-08-17) | MIT/Apache | 337 stars | [crates.io](https://crates.io/crates/transform-gizmo), [GitHub](https://github.com/urholaukkarinen/transform-gizmo) |
| egui-gizmo | 0.16.2 (2024-03-14) | – | superseded by transform-gizmo | [crates.io](https://crates.io/crates/egui-gizmo) |
| egui-snarl (node graph) | 0.12.0 (2026-08-26) | MIT/Apache | 604 stars | [crates.io](https://crates.io/crates/egui-snarl) |
| egui_node_graph | no usable release | – | repo unreachable via GitHub API (treat as abandoned) | [crates.io](https://crates.io/crates/egui_node_graph) |
| egui_plot | 0.38.0 (2026-10-19) | MIT/Apache | – | [crates.io](https://crates.io/crates/egui_plot) |
| bevy_egui | 0.43.1 (2026-10-10) | – | ~500k recent downloads | [crates.io](https://crates.io/crates/bevy_egui) |
| bevy-inspector-egui | 0.38.0 (2026-10-17) | MIT/Apache | 1.6k stars | [crates.io](https://crates.io/crates/bevy-inspector-egui) |
| imgui (imgui-rs) | 0.12.0 (2024-05-05) | MIT/Apache | 3.0k stars, pushed 2026-06 | [crates.io](https://crates.io/crates/imgui) |
| dear-imgui-rs | 0.21.0 (2026-10-17) | MIT/Apache | 110 stars, pushed 2026-10-23 | [crates.io](https://crates.io/crates/dear-imgui-rs), [GitHub](https://github.com/Latias94/dear-imgui-rs) |
| fyrox / fyrox-ui | 1.0.1 (2026-03-28) | MIT | 9.6k stars, pushed 2026-10-23 | [crates.io](https://crates.io/crates/fyrox) |
| bevy_skein (Skein) | 0.6.0 (2026-06-19) | MIT/Apache | 330 stars, pushed 2026-09-20 | [crates.io](https://crates.io/crates/bevy_skein), [GitHub](https://github.com/rust-adventure/skein) |
| blenvy | 0.1.0-alpha.1 (2024-08-14) | MIT/Apache | 848 stars, pushed 2026-03-05 | [crates.io](https://crates.io/crates/blenvy), [GitHub](https://github.com/kaosat-dev/Blenvy) |
| tauri | 2.12.5 (2026-10-21) | Apache/MIT | 112k stars | [crates.io](https://crates.io/crates/tauri) |
| wry | 0.57.1 (2026-10-23) | Apache/MIT | – | [crates.io](https://crates.io/crates/wry) |
| bevy_editor_pls | 0.10.0 (2024-10-14) | – | last push 2025-04 (stale) | [crates.io](https://crates.io/crates/bevy_editor_pls) |
| space_editor | 0.3.1 (2023-12-21) | – | stale | [crates.io](https://crates.io/crates/space_editor) |

Additional pre-release: bevy_skein 0.7.0-rc.1 (2026-09-20).

**Dear ImGui**
- dear-imgui-rs includes:
  - `dear-imgui-sys` (cimgui, docking branch, Dear ImGui v1.93.1 WIP bindings)
  - backends: wgpu, glow, ash, winit, sdl3, and an experimental Bevy 0.20 backend
  - extensions: ImPlot/ImPlot3D, ImGuizmo, ImNodes, imgui-node-editor, ImGuiColorTextEdit, ImGui Test Engine, and a file browser
  - Source: [dear-imgui-rs README](https://github.com/Latias94/dear-imgui-rs)

**Fyrox**
- Fyrox 1.0.0 was released in March 2026 (blog post dated 3/24/2026), after rc.1 (2025-10-21) and rc.2 (2026-02-09). — [Fyrox blog](https://fyrox.rs/blog/), [GameFromScratch](https://gamefromscratch.com/after-7-years-of-development-fyrox-1-0-is-here/)
- FyroxEd is built entirely on fyrox-ui. It has an animation editor and an ABSM (animation state machine) editor. A new export-cli crate automates builds for PC, WebAssembly and Android. — [i-programmer](https://www.i-programmer.info/news/144-graphics-and-games/19015-fyrox-game-engine-reaches-version-100.html), [Weekly Rust](https://weeklyrust.substack.com/p/finally-a-stable-game-engine)

**Bevy editor**
- 0.17: Feathers "will be used to build the upcoming Bevy Editor". An entity inspector is listed under "What's Next". — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- 0.19: BSN landed but is "hot off the presses". The `.bsn` asset format, which the "fabled editor will actually edit", was not yet shipped. — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)
- 0.20: changes to BSN syntax; a `.bsn` hot-reloadable asset format is still "planned"; Feathers gains Color Input, List View, Dropdown and Lazy Menu; "contextual theming" matches the "design plans for the Bevy scene editor"; the roadmap includes a remote inspector and assets-as-entities. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- The `bevy_editor` crate on crates.io is a 2020 name reservation (0.0.1). — [crates.io](https://crates.io/crates/bevy_editor)
- Conflicting source: a third-party guide claims a first-party experimental `bevy_editor` landed in 0.18. Official 0.18–0.20 release notes do not describe a shipped editor. — [StraySpark guide (secondary)](https://www.strayspark.studio/blog/bevy-rust-game-engine-2026-indie-guide), contradicted by [Bevy 0.18](https://bevy.org/news/bevy-0-18/) and [Bevy 0.20](https://bevy.org/news/bevy-0-20/)

**Blender as editor**
- Skein processes glTF extras on scene spawn to insert reflected Components. The workflow is: reflect components in Bevy, apply them in Blender, export glTF, and components are instantiated at spawn. — [bevy_skein listing](https://rust-digger.code-maven.com/crates/bevy_skein)
- Blenvy describes itself as "currently in Alpha 1 state, so there are still quite a few bugs, missing functionality, missing docs, broken examples". — [Blenvy repo](https://github.com/kaosat-dev/Blenvy)

### Inferences
- **Recommendation: an egui-based editor.**
  - Shell: eframe, or egui embedded in the engine's own window.
  - Layout: egui_dock, or egui_tiles (Rerun-maintained) for docking.
  - Gizmos: transform-gizmo-egui.
  - Graphs: egui-snarl for animation/VFX/quest graphs.
  - Plots: egui_plot for profiling/curve views.
  - Rationale: highest maintenance velocity, pure Rust, trivial to embed in the game for live tweaking, and large community.
- Use Fyrox's editor (scene graph, inspector reflection, ABSM editor, play-in-editor) as the architectural reference.
- Use dear-imgui-rs instead if the team is ImGui-fluent or wants ImGuizmo, ImPlot and imgui-node-editor out of the box. Weigh the single-maintainer risk (110 stars).
- Blender as the primary DCC/level-blockout tool, exporting glTF with custom-property extras (the Skein pattern, re-implemented engine-side), is a strong pattern for a small team. The in-engine editor then handles gameplay data, combat tuning, VFX timing and cinematics.
- Tauri/web-tech editors add an IPC boundary and make embedding a live GPU viewport awkward. They are better for asset browsers or databases than for the scene/combat editor.
- Do not wait on the Bevy editor. It remains unshipped as of 0.20.

### Gaps
- No public data was found on large production teams using egui for full game editors, beyond Rerun (a data-visualization tool).
- Fyrox's commercial adoption could not be verified.
- No source was found on whether Skein works outside Bevy without re-implementation. It is Bevy-reflection-specific.

## 4. Asset pipeline and formats (glTF/FBX/USD/VRM/MMD, texture compression, mesh optimization, image decoding, asset databases, packaging, file watching)

### Takeaway
- **Import:** glTF (`gltf` 1.4.1) is stable but slow-moving; ufbx official bindings are active (0.11.5, Sept 2026) and the best FBX route; a new pure-Rust `openusd` (0.8.0, Oct 2026) covers USD read/write and composition; VRM has several active Bevy-oriented crates; MMD/PMX support is weak.
- **Textures:** the new `ctt` crate (Sept 2026) wraps ARM astcenc and Intel ISPC (via intel_tex_2), outputs KTX2/DDS, and was adopted by Bevy 0.20 for BCn/ASTC processing. `basis-universal` bindings are stale (2023).
- **Meshes:** meshopt 0.7.0 (Oct 2026) is active.
- **Asset databases:** distill is abandoned (2021).
- **Packaging and watching:** zstd, lz4_flex, notify and memmap2 are all active.

### Cited Findings

**Version and maintenance snapshot**

| Crate | Latest (date) | License | Repo activity / notes | Source |
|---|---|---|---|---|
| gltf (+gltf-json) | 1.4.1 (2024-05-10) | MIT/Apache | 644 stars, last push 2026-05-11 | [crates.io](https://crates.io/crates/gltf), [GitHub](https://github.com/gltf-rs/gltf) |
| easy-gltf | 1.1.5 (2025-03-29) | MIT | 35 stars | [crates.io](https://crates.io/crates/easy-gltf) |
| ufbx | 0.11.5 (2026-09-25) | MIT OR Unlicense | official ufbx/ufbx-rust, pushed 2026-09-25 | [crates.io](https://crates.io/crates/ufbx), [GitHub](https://github.com/ufbx/ufbx-rust) |
| bevy_ufbx | 0.19.0 (2026-07-20) | – | Bevy FBX loader via ufbx | [crates.io search](https://crates.io/search?q=ufbx) |
| fbxcel / fbxcel-dom | 0.9.0 (2022-10-29) / 0.0.10 (2023-02-11) | MIT/Apache | low-level and stale | [crates.io](https://crates.io/crates/fbxcel) |
| openusd | 0.8.0 (2026-10-11) | MIT | 145 stars, pushed 2026-10-24 | [crates.io](https://crates.io/crates/openusd), [GitHub](https://github.com/mxpv/openusd) |
| usd (usd-rs) | 0.0.9 (2020-12-21) | Apache | stale | [crates.io](https://crates.io/crates/usd) |
| bevy_vrm / serde_vrm | 0.5.0 (2026-10-13) | MIT/Apache | unavi-xyz, pushed 2026-10-20 | [crates.io](https://crates.io/crates/bevy_vrm) |
| bevy_vrm1 | 0.10.0 (2026-10-03) | – | "VRM and VRMA in Bevy" | [crates.io search](https://crates.io/search?q=vrm) |
| vrm-runtime | 0.1.0 (2026-08-18) | – | "Renderer-agnostic VRM 1.0 loader, validator, animation system, and runtime" | [crates.io search](https://crates.io/search?q=vrm) |
| gltf_kun_vrm | 0.3.0 (2026-04-07) | – | VRM extensions for gltf_kun | [crates.io search](https://crates.io/search?q=vrm) |
| PMXUtil | 0.9.0 (Feb 2022) | MIT/Apache | partial PMX 2.0/2.1 reader/writer, no soft bodies | [lib.rs](https://lib.rs/crates/pmxutil) |
| mmd-rs / mmd | 0.0.6 (2021-02-10) | BSD-2 | "work in progress and not ready for use" | [rust-digger](https://rust-digger.code-maven.com/crates/mmd-rs), [crates.io](https://crates.io/crates/mmd) |
| ctt | 0.6.0 (2026-09-27) | MIT/Apache/Zlib | cwfitzgerald/ctt, pushed 2026-10-23 | [crates.io](https://crates.io/crates/ctt), [docs.rs](https://docs.rs/ctt) |
| basis-universal | 0.3.1 (2023-11-02) | MIT/Apache | stale (last push 2024-04) | [crates.io](https://crates.io/crates/basis-universal) |
| basisu_c_sys | 0.9.1 (2026-09-21) | – | alternative active Basis bindings | [crates.io search](https://crates.io/search?q=basis%20universal) |
| ktx2 | 0.5.0 (2026-04-19) | Apache-2.0 | pushed 2026-09 | [crates.io](https://crates.io/crates/ktx2) |
| intel_tex_2 | 0.5.0 (2025-07-02) | MIT/Apache | Traverse Research, pushed 2026-09 | [crates.io](https://crates.io/crates/intel_tex_2), [GitHub](https://github.com/Traverse-Research/intel-tex-rs-2) |
| texpresso | 2.0.2 (2025-05-26) | MIT | – | [crates.io](https://crates.io/crates/texpresso) |
| astcenc-rs | 0.2.0 (2025-12-12) | Unlicense | sys crate "currently Linux-only" | [crates.io](https://crates.io/crates/astcenc-rs) |
| image_dds | 0.7.2 (2025-03-13) | MIT | pushed 2026-10 | [crates.io](https://crates.io/crates/image_dds) |
| ddsfile | 0.6.0 (2026-04-04) | MIT | – | [crates.io](https://crates.io/crates/ddsfile) |
| meshopt | 0.7.0 (2026-10-17) | MIT/Apache | – | [crates.io](https://crates.io/crates/meshopt) |
| metis | 0.2.2 (2024-10-28) | MIT/Apache | – | [crates.io](https://crates.io/crates/metis) |
| image | 0.25.10 (2026-03-10) | MIT/Apache | 5.9k stars | [crates.io](https://crates.io/crates/image) |
| zune-image / zune-jpeg | 0.5.0 (2026-01-24) / 0.5.15 | MIT/Apache/Zlib | – | [crates.io](https://crates.io/crates/zune-image) |
| png | 0.18.1 (2026-02-14) | MIT/Apache | – | [crates.io](https://crates.io/crates/png) |
| exr | 1.74.2 (2026-07-10) | BSD-3 | – | [crates.io](https://crates.io/crates/exr) |
| bevy_mikktspace | 1.0.0 (2026-02-17) | Zlib AND (MIT/Apache) | – | [crates.io](https://crates.io/crates/bevy_mikktspace) |
| mikktspace (gltf-rs) | 0.3.0 (2022) | – | stale | [crates.io](https://crates.io/crates/mikktspace) |
| bevy_asset | 0.20.0 (2026-10-08) | MIT/Apache | – | [crates.io](https://crates.io/crates/bevy_asset) |
| distill | 0.0.3 (2021-03-28) | MIT/Apache | abandoned (no release since 2021) | [crates.io](https://crates.io/crates/distill) |
| zstd | 0.14.3 (2026-10-16) | BSD-3 | ~103M recent downloads | [crates.io](https://crates.io/crates/zstd) |
| ruzstd (pure Rust) | 0.9.1 (2026-10-11) | MIT | – | [crates.io](https://crates.io/crates/ruzstd) |
| lz4_flex | 0.14.0 (2026-07-14) | MIT | – | [crates.io](https://crates.io/crates/lz4_flex) |
| vfs | 0.13.0 (2026-03-21) | Apache | – | [crates.io](https://crates.io/crates/vfs) |
| rust-embed | 8.13.0 (2026-10-07) | MIT | – | [crates.io](https://crates.io/crates/rust-embed) |
| memmap2 | 0.9.12 (2026-10-21) | MIT/Apache | – | [crates.io](https://crates.io/crates/memmap2) |
| notify | 8.2.0 (2025-08-03) | CC0 | 3.5k stars | [crates.io](https://crates.io/crates/notify) |
| notify-debouncer-full | 0.7.0 (2026-01-23) | MIT/Apache | – | [crates.io](https://crates.io/crates/notify-debouncer-full) |
| rkyv | 0.8.18 (2026-08-05) | MIT | – | [crates.io](https://crates.io/crates/rkyv) |
| postcard | 1.1.3 (2025-07-24) | MIT/Apache | – | [crates.io](https://crates.io/crates/postcard) |

Pre-releases: ufbx earlier docs.rs pages describe the bindings as "heavy in progress but functional" ([docs.rs ufbx](https://docs.rs/crate/ufbx/0.4.0)); notify 9.0.0-rc.6 (2026-10-17); notify-debouncer-full 0.8.0-rc.2.

**FBX via ufbx**
- `animsmith-fbx` loads FBX through the official ufbx bindings, normalizes scenes to glTF-style conventions, and bakes animation stacks into linear TRS tracks. — [docs.rs animsmith-fbx](https://docs.rs/crate/animsmith-fbx/0.2.0)

**USD**
- openusd is "a Rust implementation of Pixar's Universal Scene Description… with no C++ dependencies". It reads and writes `.usda`, `.usdc` and `.usdz`, supports LIVERPS composition, and has typed schemas including UsdGeom, UsdShade and UsdSkel. — [openusd README](https://github.com/mxpv/openusd)

**Texture compression with ctt**
- ctt is "a Rust library with C bindings, and CLI for GPU texture compression" with a unified interface over encoder backends. It outputs KTX2 (with optional zstd/zlib supercompression) or DDS; DDS does not support ETC/EAC/ASTC. — [docs.rs ctt](https://docs.rs/ctt)
- Its vendored backends include `ctt-astcenc` (ARM astcenc) and `ctt-intel-texture-compressor` ("based on intel_tex_2"), with prebuilt ISPC libraries. — [crates.io search: astc encoder / ispc texture](https://crates.io/search?q=ctt)
- Bevy 0.20's `CompressedImageSaver` uses a new backend built on ctt:
  - BCn on desktop, ASTC on mobile
  - automatic mipmaps
  - ASTC block size selectable via `BEVY_COMPRESSED_IMAGE_SAVER_ASTC`
  - the old Basis Universal behaviour moved to `compressed_image_saver_universal`
  - Source: [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- `bevy_image` 0.20 depends on `ctt ^0.5`, `basis-universal ^0.3` and `ktx2 ^0.5`, all optional. — [bevy_image deps](https://crates.io/crates/bevy_image/0.20.0/dependencies)

**Bevy asset processing and glTF**
- Bevy asset processing timeline:
  - 0.18: processors referenced by short type path in `.meta` files; seekable readers.
  - 0.19: `save_using_saver` for runtime asset saving; plan to port the glTF loader to BSN.
  - 0.18 also added a `GltfExtensionHandler` glTF extension system.
  - Source: [Bevy 0.18](https://bevy.org/news/bevy-0-18/), [Bevy 0.19](https://bevy.org/news/bevy-0-19/)

### Inferences
- **Recommendation: a standalone offline "cooker" (Rust CLI and library) with a content-addressed cache.**
  - Key the cache by blake3 of source bytes, import settings and cooker version.
  - Run it locally and in CI, with outputs shared via sccache-like remote storage.
  - Avoid tying the pipeline to Bevy's asset processor unless the engine is Bevy.
- **Import formats:**
  - glTF 2.0 is the primary interchange (gltf crate). It is stable, and glTF changes slowly.
  - Use FBX via ufbx for animation-heavy Maya/MotionBuilder/mocap data, which is typical for anime action (WW/ZZZ animation pipelines are FBX-centric in Unity/UE). This is an inference about those studios, not verified.
  - Use VRM types (serde_vrm or vrm-runtime) only to bootstrap test characters and MToon-like shading reference data.
  - Skip MMD/PMX: the Rust support is weak and the licensing of MMD models is problematic for commercial use.
  - Treat USD (openusd) as optional for future DCC interop.
- **Textures:** use ctt. Target BC7/BC6H/BC5 on PC and consoles (via the ISPC encoder) and ASTC on mobile (via astcenc), stored in KTX2 with zstd supercompression. Basis/UASTC is only worth it if you need one universal file for many GPU families. Its Rust bindings are stale (or use basisu_c_sys).
- **Meshes:** use meshopt for vertex-cache/overdraw/fetch optimization, simplification for LODs, and meshlet building if the renderer uses meshlets. metis is only needed for Nanite-style cluster DAGs.
- **Runtime packaging:**
  - a custom archive with a TOC
  - zstd for bulk data (pure-Rust ruzstd for decode-only builds if C dependencies are a concern on consoles)
  - lz4_flex for fast streaming chunks
  - memmap2 for zero-copy, and rkyv for zero-copy metadata
- Do not adopt distill (abandoned). Avoid bincode for new formats (see section 7).
- **Hot reload:** use notify 8.x with notify-debouncer-full, reloading cooked assets by GUID.

### Gaps
- No Rust MToon/VRM shader parity evaluation was found. That belongs to the rendering researcher.
- ufbx-rust API completeness (blend shapes, constraints) relative to the C library was not verified.
- No public benchmarks comparing ctt's encoder quality and speed with Compressonator or NVTT were found.
- No game-scale VFS crate proven in shipped titles was found. `vfs` is a general crate.

## 5. Profiling, debugging and diagnostics (Tracy, puffin, Superluminal, Optick, tracing, GPU capture, macOS tools, memory, crash reporting)

### Takeaway
Tracy (tracy-client 0.19, Aug 2026) is the primary in-engine profiler, and Bevy 0.20 integrates it via `bevy_log`. The `profiling` crate abstracts over Tracy, puffin, Superluminal and Optick. puffin (Embark) remains active for in-game egui flamegraphs. Optick bindings are abandoned (2020). Superluminal supports Rust, Windows, Xbox and PlayStation, but was not found to support macOS. For crash reporting, Embark's crash-handler and minidumper (active, Sept 2026) give an out-of-process minidump design; Sentry's Rust SDK has native minidump capture since 0.49.3. The `renderdoc` crate has had no release since March 2024.

### Cited Findings

**Version and maintenance snapshot**

| Crate/tool | Latest (date) | License | Repo activity | Source |
|---|---|---|---|---|
| tracy-client | 0.19.0 (2026-08-25) | MIT/Apache | nagisa/rust_tracy_client, pushed 2026-10-20 | [crates.io](https://crates.io/crates/tracy-client), [GitHub](https://github.com/nagisa/rust_tracy_client) |
| tracing-tracy | 0.12.0 (2026-08-25) | MIT/Apache | – | [crates.io](https://crates.io/crates/tracing-tracy) |
| tracy-client-sys | 0.30.1 (2026-10-20) | – | – | [crates.io](https://crates.io/crates/tracy-client-sys) |
| tracy_full | 1.14.1 (2026-10-03) | – | alternative full bindings | [crates.io search](https://crates.io/search?q=tracy) |
| profiling | 1.0.18 (2026-05-04) | MIT/Apache | ~29M recent downloads | [crates.io](https://crates.io/crates/profiling) |
| puffin / puffin_http / puffin_egui | 0.20.0 / 0.17.0 / 0.30.0 (2026-03-18) | MIT/Apache | EmbarkStudios/puffin, 1.8k stars, pushed 2026-10-05 | [crates.io](https://crates.io/crates/puffin), [GitHub](https://github.com/EmbarkStudios/puffin) |
| superluminal-perf | 0.4.0 (2025-05-07) | MIT/Apache | Embark | [crates.io](https://crates.io/crates/superluminal-perf) |
| optick | 1.3.4 (2020-08-05) | MIT | **abandoned** (last push 2021) | [crates.io](https://crates.io/crates/optick) |
| tracing | 0.1.44 (2025-12-18) | MIT | tokio-rs, pushed 2026-10 | [crates.io](https://crates.io/crates/tracing) |
| tracing-subscriber | 0.3.23 (2026-03-13) | MIT | – | [crates.io](https://crates.io/crates/tracing-subscriber) |
| tracing-chrome | 0.7.2 (2024-03-15) | MIT | no release since 2024 | [crates.io](https://crates.io/crates/tracing-chrome) |
| tracing-perfetto | 0.1.5 (2025-01-02) | MIT | – | [crates.io](https://crates.io/crates/tracing-perfetto) |
| renderdoc | 0.12.1 (2024-03-02) | MIT/Apache | last push 2024-04 | [crates.io](https://crates.io/crates/renderdoc) |
| wgpu-profiler | 0.28.0 (2026-07-31) | MIT/Apache | GPU-side profiling for wgpu; feature details not verified this session | [crates.io](https://crates.io/crates/wgpu-profiler) |
| samply | 0.13.1 (2025-02-01) | MIT/Apache | 4.5k stars, repo pushed 2026-10-18 | [crates.io](https://crates.io/crates/samply), [GitHub](https://github.com/mstange/samply) |
| flamegraph (cargo-flamegraph) | 0.6.14 (2026-08-12) | MIT/Apache | 6.0k stars | [crates.io](https://crates.io/crates/flamegraph) |
| dhat | 0.3.3 (2024-02-04) | MIT/Apache | last push 2025-02 | [crates.io](https://crates.io/crates/dhat) |
| sentry | 0.49.3 (2026-09-21) | MIT | pushed 2026-10-20 | [crates.io](https://crates.io/crates/sentry) |
| crash-handler | 0.8.1 (2026-09-25) | MIT/Apache | EmbarkStudios/crash-handling | [crates.io](https://crates.io/crates/crash-handler), [GitHub](https://github.com/EmbarkStudios/crash-handling) |
| minidumper | 0.11.0 (2026-07-20) | MIT/Apache | Embark | [crates.io](https://crates.io/crates/minidumper) |
| minidump-writer | 0.13.0 (2026-07-20) | MIT | rust-minidump | [crates.io](https://crates.io/crates/minidump-writer) |
| sentry-rust-minidump | 0.18.1 (2026-09-28) | MIT/Apache | timfish | [crates.io](https://crates.io/crates/sentry-rust-minidump) |
| log | 0.4.34 (2026-08-22) | MIT/Apache | – | [crates.io](https://crates.io/crates/log) |
| mimalloc | 0.1.52 (2026-05-22) | MIT | – | [crates.io](https://crates.io/crates/mimalloc) |
| tikv-jemallocator | 0.7.0 (2026-05-25) | MIT/Apache | – | [crates.io](https://crates.io/crates/tikv-jemallocator) |

**Tracy**
- `bevy_log` 0.20 optionally depends on `tracy-client ^0.19.0`, `tracing-tracy ^0.12.0` and `tracing-chrome ^0.7.0`. — [bevy_log deps](https://crates.io/crates/bevy_log/0.20.0/dependencies)
- rust_tracy_client follows semver but publishes a breaking `tracy-client-sys` bump on Tracy protocol changes. Users must pin the `tracy-client-sys` version matching their Tracy GUI version. — [rust_tracy_client README](https://github.com/nagisa/rust_tracy_client)

**puffin**
- Scopes cost "around 50-200 ns", and about 1 ns when profiling is off (M1 MacBook Pro). Remote viewing works via `puffin_http` and `puffin_viewer`. — [puffin README](https://github.com/EmbarkStudios/puffin)

**Superluminal**
- Superluminal profiles Rust with "full support for Rust symbol resolving and name demangling". It supports Xbox One/Series (no additional charge, up to 10 kHz) and PS4/PS5 (up to 40 kHz, developer status required). macOS support was not found. — [Superluminal applications](https://superluminal.eu/applications/), [Superluminal Xbox](https://superluminal.eu/xbox/), [Superluminal features](https://superluminal.eu/features/)

**Crash reporting**
- Embark crash-handling supported targets:
  - Linux: x86_64, i686, arm, aarch64 and loongarch64 (gnu/musl)
  - Android: arm and aarch64
  - Windows MSVC: x86_64 only
  - macOS (apple-darwin): x86_64 and aarch64
  - iOS: not listed
  - Source: [crash-handling README](https://github.com/EmbarkStudios/crash-handling)
- minidumper is "an IPC client and server for creating minidumps for an external process". The server writes the dump and can upload via callback. — [docs.rs minidumper](https://docs.rs/minidumper)
- Sentry Rust SDK 0.49.3+ captures native crashes (segfaults, FFI crashes) as minidumps via the `minidump` feature and `MinidumpIntegration`. Sentry warns that minidumps may contain sensitive memory. — [Sentry docs](https://docs.sentry.io/platforms/rust/native-crash-reporting/), [Sentry changelog](https://sentry.io/changelog/rust-sdk-0493-native-crash-reporting-minidumps/)
- `sentry-rust-minidump` relaunches the executable in crash-reporter mode, using minidumper out of process. — [lib.rs](https://www.lib.rs/crates/sentry-rust-minidump)

### Inferences
- **Recommendation:**
  - Instrument with the `profiling` crate macros (or `tracing` spans if you already use tracing), with Tracy as the main backend: CPU zones, GPU zones via wgpu-profiler or your own, frame marks, plots and memory.
  - Keep puffin and puffin_egui as an always-available in-game flamegraph for designers on device.
  - Add Superluminal on Windows and consoles.
  - Use samply or Instruments (Time Profiler, Allocations, Metal System Trace) on macOS/iOS. Use Xcode Metal frame capture for GPU debugging on Apple platforms.
- RenderDoc does not capture Metal (background knowledge, not verified this session). RenderDoc and PIX are for Windows/Vulkan/D3D12, and the `renderdoc` crate (2024) still works for in-app capture triggers.
- Memory: Tracy's allocation tracking plus Instruments on macOS. dhat-rs for targeted heap analysis. heaptrack is Linux-only (background knowledge).
- **Crash reporting:** an out-of-process minidumper/crash-handler monitor uploading to Sentry (sentry-rust-minidump) for PC, macOS and Android. iOS needs a separate path (e.g., Sentry's Cocoa SDK), since crash-handler doesn't list iOS. Consoles use the platform holders' crash systems.
- Avoid Optick (abandoned). tracing-chrome works but has had no release since 2024.

### Gaps
- The Tracy GUI version corresponding to tracy-client 0.19 / tracy-client-sys 0.30 was not extracted (the README table was not fully retrieved).
- No source was found confirming Superluminal macOS support (likely unsupported).
- No Rust crate for PIX event markers was found in this search.
- Tiny Glade's profiling and tooling stack was not covered in the interviews found.

## 6. Build and iteration tooling (compile-time reduction, linkers, Cranelift, hot reload, nextest, CI, cross-compilation, consoles)

### Takeaway
On macOS, Apple's default linker (ld-prime) is already faster than lld. Mold and Wild don't apply (Wild is Linux-only; Mach-O and Windows are "looking ahead"). The biggest dev-loop wins are:
- dynamic linking of the engine for dev builds
- `opt-level` tuning (dev 1, dependencies 3)
- line-tables-only debug info
- workspace splitting
- hot-patching via Dioxus `subsecond` (0.7.10 stable, 0.8 alpha), which Bevy has supported experimentally since 0.17

Cranelift is nightly-only, gives roughly 10–15% average backend speedups, and Bevy warns it can crash on macOS. The Rust project's 2026 "Fast Builds" roadmap (parallel frontend, Relink-Don't-Rebuild, Wild integration, TPDE) has not yet landed broadly.

### Cited Findings

**Version and maintenance snapshot**

| Tool | Latest (date) | License | Notes | Source |
|---|---|---|---|---|
| subsecond / dioxus-cli | 0.7.10 (2026-07-30) | MIT/Apache | 0.8.0-alpha.3 (2026-10-16) | [crates.io](https://crates.io/crates/subsecond), [docs.rs](https://docs.rs/subsecond/latest/subsecond/) |
| hot-lib-reloader | 0.8.2 (2025-08-11) | MIT | dylib-based | [crates.io](https://crates.io/crates/hot-lib-reloader) |
| bevy_simple_subsecond_system | 0.2.0 (2025-06-02) | MIT/Apache | – | [crates.io](https://crates.io/crates/bevy_simple_subsecond_system) |
| bevy_hotpatching_experiments | 0.4.0 (2026-01-29) | MIT/Apache | – | [docs.rs](https://docs.rs/crate/bevy_hotpatching_experiments/latest) |
| wild-linker | 0.11.0 (2026-10-24) | MIT/Apache | 4.0k stars; Linux only | [crates.io](https://crates.io/crates/wild-linker), [GitHub](https://github.com/wild-linker/wild) |
| sccache | 0.18.0 (2026-09-16) | Apache | Mozilla, 7.8k stars | [crates.io](https://crates.io/crates/sccache) |
| cargo-nextest | 0.9.149 (2026-10-20) | Apache/MIT | 3.4k stars | [crates.io](https://crates.io/crates/cargo-nextest) |
| cargo-hakari | 0.9.39 (2026-09-19) | MIT/Apache | workspace-hack generator | [crates.io](https://crates.io/crates/cargo-hakari) |
| cargo-zigbuild | 0.24.0 (2026-10-21) | MIT | – | [crates.io](https://crates.io/crates/cargo-zigbuild) |
| cargo-xwin | 0.23.1 (2026-08-13) | MIT | – | [crates.io](https://crates.io/crates/cargo-xwin) |
| cross | 0.2.5 (2023-02-04) | MIT/Apache | repo active 2026-10 | [crates.io](https://crates.io/crates/cross) |
| cargo-bundle | 0.12.0 (2026-09-20) | – | – | [crates.io](https://crates.io/crates/cargo-bundle) |
| cargo-packager | 0.11.8 (2025-11-27) | – | – | [crates.io](https://crates.io/crates/cargo-packager) |
| cargo-dist | 0.32.0 (2026-05-22) | – | – | [crates.io](https://crates.io/crates/cargo-dist) |
| bacon | 3.26.0 (2026-09-26) | AGPL-3.0 | dev tool only | [crates.io](https://crates.io/crates/bacon) |
| cargo-deny | 0.20.2 (2026-07-09) | MIT/Apache | Embark | [crates.io](https://crates.io/crates/cargo-deny) |
| cargo-ndk | 4.1.2 (2025-08-09) | – | – | [crates.io](https://crates.io/crates/cargo-ndk) |
| cargo-xcode | 1.11.1 (2026-06-21) | – | – | [crates.io](https://crates.io/crates/cargo-xcode) |
| cargo-swift | 0.11.1 (2026-05-20) | – | – | [crates.io](https://crates.io/crates/cargo-swift) |

**Linkers**
- Rust 1.90 made lld the default linker on `x86_64-unknown-linux-gnu`. — [Rust blog](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable/)
- Phoronix cites up to 40% lower end-to-end incremental build time and about 20% for debug builds (ripgrep benchmark). — [Phoronix](https://www.phoronix.com/news/Rust-1.90-LLD-Linking)
- Bevy's setup guide says that on macOS "the default system linker ld-prime is faster than LLD". It recommends lld for Linux/Windows and mold on Linux ("up to 5×" faster than lld). — [Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/)
- Chromium reports ld-prime is faster than lld for local macOS development. — [Chromium apple_platform_linkers.md](https://chromium.googlesource.com/chromium/src.git/+/main/docs/apple_platform_linkers.md)
- Wild supports x86-64, AArch64, RISC-V, LoongArch64 and PPC64LE on Linux, plus wasm32. "Incremental linking", "Mach-O support" and "Windows support" are listed as things they are "looking ahead to". — [Wild README](https://github.com/wild-linker/wild)
- Wild 0.8 (January 2026) added parallel data-section copying. — [Phoronix](https://phoronix.com/news/wild-linker-0.8)

**Bevy compile-time advice**
- Dynamic linking is "the most impactful compilation time decrease". Don't ship with it.
- Use `opt-level = 1` for dev and `3` for dependencies.
- `-Zshare-generics=y` is nightly-only.
- Cranelift (nightly) is "about 30% faster at compiling than LLVM" and "works best on Linux". The guide warns "MacOS builds can currently crash on Bevy applications".
- Source: [Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/)

**Cranelift status**
- In February 2026, compiling Zed on 16 cores, Cranelift was about 5% faster than LLVM with no debug info and about 12% faster with line-tables-only. On the rustc suite the speedup ranged from negligible to about 30%, averaging 10–15%.
- The 2026 goal is a 2× speedup, funded with $65,000. Candidate approaches are a persistent daemon, JIT, and IR interpretation.
- Source: [Rust goal: improve cg_clif performance](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)

**Rust 2026 "Fast Builds" roadmap**
- Items:
  - parallel front end: stabilization still ahead
  - Relink-Don't-Rebuild: claims 5–10× faster rebuilds for function-body-only changes
  - Wild linker integration: "2026 and beyond"
  - incremental redesign
  - TPDE experimental backend for debug builds
  - crate slicing: future
- Source: [Fast Builds roadmap](https://goals.rust-lang.org/2026/roadmap-fast-builds.html)

**Hot patching**
- Bevy 0.17 added the `hotpatching` feature using subsecond and `dx serve --hot-patch`. Limitations:
  - "only works on the binary crate"
  - no Wasm
  - systems whose parameters change aren't reloaded
  - "may be sensitive to Rust and linker configuration"
  - Source: [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- How subsecond works: it routes calls through a jump table and is active only in debug builds. Supported platforms: Android, iOS (simulator only, due to code signing), Linux, macOS, Windows and Wasm. Limitations:
  - only the tip crate is patched
  - no struct size/alignment changes
  - thread-locals reset
  - static initializer changes are not observed
  - Source: [docs.rs subsecond](https://docs.rs/subsecond/latest/subsecond/)
- Dioxus documents Rust hot-patching as "experimental" in 0.7. Sibling workspace crates are not picked up. — [Dioxus hot-reload docs](https://dioxuslabs.com/learn/0.7/essentials/ui/hotreload)

**Cross-compilation**
- Building for iOS requires macOS ("not possible to target iOS on platforms other than macOS"). — [cargo-mobile2 README](https://github.com/tauri-apps/cargo-mobile2)

### Inferences
- **Recommended dev-loop setup on macOS (Apple Silicon):**
  - stable toolchain plus the default ld-prime linker
  - engine built as a dylib for dev (Bevy-style `dynamic_linking` pattern)
  - `[profile.dev] opt-level=1` and `[profile.dev.package."*"] opt-level=3`
  - `debug = "line-tables-only"`
  - sccache for CI and clean builds
  - cargo-hakari to stabilize feature unification in a large workspace
  - fine-grained workspace crates so edits recompile little
  - Keep `game` code in the "tip" binary crate so subsecond hot-patching covers gameplay and combat tuning code; engine crates change less often.
- Try Cranelift on macOS only after validating stability. Bevy's warning suggests it's not yet reliable there.
- **Hot reload strategy:**
  - subsecond for gameplay functions (state stays in the ECS, so struct-layout changes force a restart)
  - data and asset hot reload (notify) for combat tuning, which matters more for an action game's frame-data iteration
  - optionally an embedded scripting language (out of scope here)
- **Testing and CI:**
  - cargo-nextest for tests
  - cargo-deny for license/advisory gating (critical given the AGPL crates in the ecosystem: bacon, dssim)
  - GitHub Actions macOS runners for Apple targets, Linux runners for Android (cargo-ndk) and Linux
  - Windows runners or cargo-xwin for Windows. cargo-xwin downloads the MSVC CRT and Windows SDK, which carries Microsoft license terms (background knowledge, verify).
- **Consoles:** no public Rust toolchain. Plan a private fork with a custom target JSON and std port per platform under NDA, and keep console CI on self-hosted, devkit-licensed machines.

### Gaps
- No Rust-specific benchmark of ld-prime vs lld for game-sized binaries was found (only Chromium's).
- Exact stabilization dates for the parallel frontend and Relink-Don't-Rebuild are unknown.
- Whether Cranelift's macOS aarch64 crash issue is fixed as of Oct 2026 is unknown.
- No public console toolchain details for Rust (PS5/Xbox/Switch 2) were found.
- No public data on shipped games using subsecond was found.

## 7. Misc: localization, config/settings, save storage, platform services, video playback, compression, hashing, testing

### Takeaway
- **Localization:** Fluent (fluent-rs) is the standard. Its core crates last released in May 2025 but the repo is active; ICU4X 2.4 (Oct 2026) covers plurals, dates and segmentation.
- **Saves:** bincode is discontinued (3.0.0 is a compile-error notice). Use postcard, rkyv or (for databases) redb/rusqlite.
- **Platform services:** Steam is well served (steamworks-rs). EOS and Discord bindings exist but are tiny. There are no public PlayStation/Xbox SDK wrappers.
- **Cutscene video:** the options are ffmpeg-based (ffmpeg-next, video-rs, bevy_cutscene, ff-preview) or AV1 via dav1d/rav1d. No turnkey, console-ready Rust video player exists.
- **Hashing:** foldhash, rustc-hash, xxhash-rust, twox-hash and blake3 are all active.
- **Testing:** insta for snapshots; nv-flip / image-compare for image tests. Beware dssim's AGPL license.

### Cited Findings

**Version and maintenance snapshot**

| Crate | Latest (date) | License | Notes | Source |
|---|---|---|---|---|
| fluent | 0.17.0 (2025-05-23) | Apache/MIT | repo pushed 2026-09-27 | [crates.io](https://crates.io/crates/fluent), [GitHub](https://github.com/projectfluent/fluent-rs) |
| fluent-bundle | 0.16.0 (2025-05-22) | Apache/MIT | – | [crates.io](https://crates.io/crates/fluent-bundle) |
| fluent-templates | 0.15.1 (2026-08-05) | MIT/Apache | – | [crates.io](https://crates.io/crates/fluent-templates) |
| rust-i18n | 4.2.4 (2026-09-29) | MIT | – | [crates.io](https://crates.io/crates/rust-i18n) |
| i18n-embed | 0.16.0 (2025-07-09) | MIT | – | [crates.io](https://crates.io/crates/i18n-embed) |
| icu (ICU4X) | 2.4.0 (2026-10-22) | Unicode-3.0 | – | [crates.io](https://crates.io/crates/icu) |
| unic-langid | 0.9.6 (2025-05-09) | MIT/Apache | – | [crates.io](https://crates.io/crates/unic-langid) |
| config | 0.15.27 (2026-09-30) | MIT/Apache | – | [crates.io](https://crates.io/crates/config) |
| figment | 0.10.19 (2024-05-17) | MIT/Apache | – | [crates.io](https://crates.io/crates/figment) |
| ron | 0.12.2 (2026-06-22) | MIT/Apache | – | [crates.io](https://crates.io/crates/ron) |
| toml | 1.1.8 | MIT/Apache | – | [crates.io](https://crates.io/crates/toml) |
| serde_json | 1.0.154 (2026-10-11) | MIT/Apache | – | [crates.io](https://crates.io/crates/serde_json) |
| dirs | 7.0.0 (2026-09-05) | MIT/Apache | – | [crates.io](https://crates.io/crates/dirs) |
| directories | 6.0.0 (2025-01-12) | MIT/Apache | repo **archived** | [GitHub](https://github.com/soc/directories-rs) |
| postcard | 1.1.3 (2025-07-24) | MIT/Apache | – | [crates.io](https://crates.io/crates/postcard) |
| rkyv | 0.8.18 (2026-08-05) | MIT | – | [crates.io](https://crates.io/crates/rkyv) |
| bincode | 3.0.0 (2025-12-16) | MIT | **development ceased** | [docs.rs](https://docs.rs/crate/bincode/3.0.0) |
| redb | 4.4.0 (2026-10-13) | MIT/Apache | – | [crates.io](https://crates.io/crates/redb) |
| rusqlite | 0.40.2 (2026-08-08) | MIT | – | [crates.io](https://crates.io/crates/rusqlite) |
| fjall | 3.1.12 (2026-10-03) | MIT/Apache | – | [crates.io](https://crates.io/crates/fjall) |
| heed (LMDB) | 0.23.0 (2026-10-22) | MIT | – | [crates.io](https://crates.io/crates/heed) |
| bevy_save | 2.0.1 (2025-08-16) | MIT/Apache | – | [crates.io](https://crates.io/crates/bevy_save) |
| steamworks | 0.13.1 (2026-05-05) | MIT/Apache | active | [GitHub](https://github.com/Noxime/steamworks-rs) |
| eos-rs / eos-sys | 0.1.1 (2026-04-24) | – | ~43–94 recent downloads | [crates.io search](https://crates.io/search?q=epic%20online%20services) |
| eos | 0.0.0 (2021) | – | placeholder | [crates.io](https://crates.io/crates/eos) |
| discord_social_sdk | 0.1.2 (2026-07-19) | – | – | [crates.io search](https://crates.io/search?q=discord%20social%20sdk) |
| ffmpeg-next | 9.0.0 (2026-08-05) | WTFPL (bindings) | 2.0k stars | [crates.io](https://crates.io/crates/ffmpeg-next) |
| ffmpeg-sidecar | 2.7.0 (2026-10-18) | MIT | drives the ffmpeg CLI | [crates.io](https://crates.io/crates/ffmpeg-sidecar) |
| video-rs | 0.12.0 (2026-09-09) | MIT/Apache | – | [crates.io](https://crates.io/crates/video-rs) |
| gstreamer | 0.25.4 (2026-09-21) | MIT/Apache | – | [crates.io](https://crates.io/crates/gstreamer) |
| openh264 | 0.9.9 (2026-10-15) | BSD-2 | – | [crates.io](https://crates.io/crates/openh264) |
| rav1d (pure-Rust AV1 decoder) | 1.1.0 (2025-05-07) | BSD-2 | repo pushed 2026-10-22 | [crates.io](https://crates.io/crates/rav1d), [GitHub](https://github.com/memorysafety/rav1d) |
| dav1d (bindings) | 0.11.1 (2025-11-25) | MIT | – | [crates.io](https://crates.io/crates/dav1d) |
| mp4 | 0.14.0 (2023-08-01) | MIT | stale | [crates.io](https://crates.io/crates/mp4) |
| re_video | 0.40.0 (2026-10-21) | MIT/Apache | Rerun | [crates.io](https://crates.io/crates/re_video) |
| ff-preview | 0.18.4 (2026-09-29) | MIT/Apache | – | [crates.io](https://crates.io/crates/ff-preview) |
| bevy_cutscene | 0.1.0 (2026-09-22) | Apache-2.0 | – | [Codeberg](https://codeberg.org/izarma/bevy_cutscene) |
| foldhash | 0.2.0 (2025-08-23) | Zlib | – | [crates.io](https://crates.io/crates/foldhash) |
| ahash | 0.8.12 (2025-05-08) | MIT/Apache | – | [crates.io](https://crates.io/crates/ahash) |
| rustc-hash | 2.1.3 (2026-07-02) | Apache/MIT | – | [crates.io](https://crates.io/crates/rustc-hash) |
| xxhash-rust | 0.8.19 (2026-09-28) | BSL-1.0 | – | [crates.io](https://crates.io/crates/xxhash-rust) |
| twox-hash | 2.1.5 (2026-10-04) | MIT | – | [crates.io](https://crates.io/crates/twox-hash) |
| blake3 | 1.8.7 (2026-08-20) | CC0/Apache | – | [crates.io](https://crates.io/crates/blake3) |
| insta | 1.49.0 (2026-10-03) | Apache-2.0 | – | [crates.io](https://crates.io/crates/insta) |
| nv-flip | 0.1.2 (2023-07-16) | MIT/Apache/Zlib | gfx-rs repo pushed 2026-09 | [crates.io](https://crates.io/crates/nv-flip), [GitHub](https://github.com/gfx-rs/nv-flip-rs) |
| image-compare | 0.5.0 (2025-08-18) | MIT | – | [crates.io](https://crates.io/crates/image-compare) |
| dssim | 3.5.1 (2026-08-29) | **AGPL-3.0** | – | [crates.io](https://crates.io/crates/dssim) |
| criterion | 0.8.2 (2026-02-04) | Apache/MIT | – | [crates.io](https://crates.io/crates/criterion) |
| divan | 0.1.21 (2025-04-10) | MIT/Apache | – | [crates.io](https://crates.io/crates/divan) |

**bincode**
- The bincode 3.0.0 README says: "Due to a doxxing and harassment incident, development on bincode has ceased… No further releases will be published on crates.io." The release contains only a README and a `lib.rs` with a compiler error. It recommends wincode, postcard and rkyv. — [docs.rs bincode 3.0.0](https://docs.rs/crate/bincode/3.0.0)
- The GitHub repo is archived and development migrated to sourcehut. — [bincode GitHub](https://github.com/bincode-org/bincode)

**Video**
- bevy_cutscene is "FFmpeg-powered video cutscene playback (video + audio) for Bevy games", built on ffmpeg-next. Limitations:
  - audio and video start together but "are not continuously re-synced"
  - no WASM
  - Windows needs dynamic FFmpeg
  - Source: [bevy_cutscene](https://codeberg.org/izarma/bevy_cutscene)
- ff-preview offers frame-accurate seek, audio-master A/V sync and a `FrameSink` trait for custom renderers. — [docs.rs ff-preview](https://docs.rs/crate/ff-preview/latest)

**Platform services**
- No public crates exist for PlayStation Network or the Xbox GDK. The crates.io names `playstation` (0.0.0) and `bevy_playstation` are reservations. — [crates.io search: playstation](https://crates.io/search?q=playstation)

### Inferences
- **Localization:** use Fluent (fluent-bundle) for message formatting, with ICU4X for plural rules, number/date formatting and line breaking/segmentation (essential for CJK and Thai text in UI). Keep string tables in the cooked asset pipeline. A WW/ZZZ-style game ships at least 4–5 voice and 10+ text languages; the Fluent + ICU4X combination covers the text side.
- **Config and settings:** serde with `ron` or `toml` for human-edited settings, and `dirs` for per-user paths on desktop. Mobile and consoles need platform-specific save locations via the platform layer.
- **Save system:**
  - a versioned serde schema serialized with postcard (compact, stable format) or rkyv (zero-copy)
  - wrapped in your own header (version, checksum via xxhash/blake3) and compressed with zstd
  - Avoid bincode for new work.
  - Server-authoritative gacha/progression data belongs on the backend anyway.
- **Platform services:**
  - steamworks-rs for Steam.
  - For EOS, Discord and console PSN/Xbox Live, plan your own bindgen FFI to the vendor C SDKs. Existing crates are too small to depend on.
- **Cutscenes:**
  - Option A: pre-rendered video. ffmpeg-next/video-rs give broad codec coverage on PC/mobile but bring LGPL/GPL and patent/licensing questions (background knowledge, verify with counsel).
  - Option B: AV1 decode via rav1d or dav1d plus your own container parsing and audio sync. This is cleaner licensing and works on all targets, but you must build the player.
  - Option C: commercial Bink via C FFI. Only a pure-Rust Bink v1 decoder for old games was found (infinitier_bik_decoder), so Bink 2 would need the official SDK.
  - Since WW/ZZZ-style games lean heavily on real-time cutscenes, the timeline/sequencer (gameplay side) may matter more than video.
- **Hashing:**
  - foldhash or rustc-hash for in-memory hash maps
  - xxhash-rust (XXH3) for fast asset and content checksums
  - blake3 for content-addressed cooking caches and integrity
- **Testing:**
  - insta for snapshot tests of cooked-asset metadata, serialized data and UI layout trees
  - nv-flip (perceptual FLIP) or image-compare for golden-image rendering tests
  - Avoid dssim in a closed-source pipeline because it is AGPL.

### Gaps
- No maintained, console-capable Rust video player crate was found.
- Licensing details for ffmpeg/openh264 distribution in a commercial game were not verified here.
- No public evidence of Rust bindings for the PlayStation, Xbox or Nintendo online SDKs was found (expected, due to NDAs).
- The fluent-rs release cadence (no release since May 2025) vs active repo commits suggests slow releases; its roadmap was not verified.
- No shipped Rust game was found documenting its localization or save stack.
