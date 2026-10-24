//! SDL3-based platform layer: app lifecycle, windows, surfaces, input events, content
//! and user file systems (spec §5.1, layer L1).
//!
//! # Owns
//!
//! - **Entry and lifecycle** through SDL's main callbacks on every platform: [`run`]
//!   from `fn main` on desktop and iOS, [`export_android_main!`] on Android. The app
//!   implements [`App`].
//! - **The OS window** ([`Window`]) and the [`SurfaceHandle`] the renderer builds its
//!   surface from: a `CAMetalLayer` on Apple platforms, `raw-window-handle` 0.6 handles
//!   elsewhere, kept alive for as long as the surface needs them.
//! - **Raw input and OS events** as engine-owned [`Event`]s, each with SDL's
//!   nanosecond [`Timestamp`]. Mapping them to actions is `sapphire_input`'s job (M1).
//! - **Content and user storage:** [`ContentFs`] for packaged read-only content
//!   (including APK assets), [`user_dir`] for settings and saves, and [`cache_dir`] for
//!   rebuildable data such as pipeline caches, logs and crash dumps. Both directory
//!   functions work before SDL starts.
//! - **Device facts** ([`SystemInfo`]) and a native log sink ([`sdl_log`]) for mobile.
//!
//! # Does not own
//!
//! GPU devices and swapchains (`sapphire_rhi`), audio (FMOD, M2), logging setup (the
//! app wires `tracing` to [`sdl_log`]), and, until their milestones, text input / IME
//! (M1), haptics, clipboard, the async IO thread, and thermal state (M4).
//!
//! # Threading
//!
//! SDL runs every callback on the main thread (on Android, SDLActivity's SDL thread),
//! so [`Platform`], [`Window`] and [`SurfaceHandle`] are `!Send`. [`ContentFs`],
//! [`ContentFile`] and [`sdl_log`] may be used from any thread. This crate never exposes
//! SDL types or raw SDL pointers; the only pointers it hands out are the native surface
//! handles.
//!
//! # Frame pacing (known gap)
//!
//! SDL decides when [`App::iterate`] runs. On iOS that is SDL's own `CADisplayLink`, which
//! runs at the display's full rate (up to 120 Hz on ProMotion) and ignores
//! `SDL_HINT_MAIN_CALLBACK_RATE`, so a 30 / 60 fps cap (spec §6.2) can't be set here yet.
//! Whether the engine owns a display link or paces through presentation is an open M1 / M4
//! decision.
//!
//! # Android packaging
//!
//! SDL's Java glue drives the app, so an Android build needs:
//!
//! 1. The app crate builds as a `cdylib`, say `libsandbox.so`, whose `lib.rs` invokes
//!    `sapphire_platform::export_android_main!(SandboxApp, init = setup);`. That
//!    exports `SDL_main(argc, argv)`, which SDLActivity looks up with `dlsym` and calls
//!    on its SDL thread once the surface is ready, plus all of SDL's C symbols,
//!    including `JNI_OnLoad`, which registers SDL's Java native methods. SDL and the
//!    NDK's static libc++ are linked into that one library.
//! 2. Building SDL for Android needs the NDK's CMake toolchain file, for example
//!    `CMAKE_TOOLCHAIN_FILE_aarch64_linux_android=$NDK/build/cmake/android.toolchain.cmake`,
//!    alongside the usual `CC_*`, `CXX_*`, `AR_*` and linker variables for the target.
//! 3. The Gradle project compiles SDL's Java sources, `org.libsdl.app.*`, from the
//!    `sdl3-src` crate (`SDL/android-project/app/src/main/java/org/libsdl/app/` in the
//!    cargo registry, matching the `sdl3-sys` version in `Cargo.lock`).
//! 4. The activity extends `SDLActivity` and overrides `getLibraries()` to return
//!    exactly `{ "sandbox" }`: there is no separate `libSDL3.so`. `getMainFunction()`
//!    keeps its default, `"SDL_main"`.
//! 5. The manifest locks the activity to landscape, matching the orientation hint set
//!    here.
//! 6. Packaged content goes in the APK's `assets/content/` directory (see
//!    [`ContentFs`]).

// SDL's Android HIDAPI backend is C++, so the C++ runtime must be linked. The NDK's
// static libc++ keeps the app a single self-contained `.so`. The NDK allows only one
// C++ runtime per process: if a prebuilt C++ library that needs `libc++_shared.so`
// joins the APK (FMOD may, in M2), switch this to `c++_shared` and package that `.so`.
#[cfg(target_os = "android")]
#[link(name = "c++_static", kind = "static", modifiers = "-bundle")]
unsafe extern "C" {}
#[cfg(target_os = "android")]
#[link(name = "c++abi", kind = "static", modifiers = "-bundle")]
unsafe extern "C" {}

/// `tracing` target for everything this crate logs.
const LOG_TARGET: &str = "sapphire::platform";

mod app;
mod content;
mod dirs;
mod event;
mod log;
mod platform;
mod sdl;
mod system;
mod window;

#[doc(hidden)]
pub use app::__private;
pub use app::{App, AppControl, AppInitError, AppMetadata, run};
pub use content::{CONTENT_DIR_ENV, ContentError, ContentFile, ContentFs};
pub use dirs::{cache_dir, user_dir};
pub use event::{
    Event, FingerId, GamepadAxis, GamepadButton, GamepadEvent, GamepadId, KeyEvent, Keycode, LifecycleEvent,
    Modifiers, MouseButton, MouseButtonEvent, MouseMotionEvent, MouseWheelEvent, Scancode, Timestamp,
    TouchDeviceId, TouchEvent, TouchPhase, WindowEvent,
};
pub use log::sdl_log;
pub use platform::{Platform, PlatformError};
pub use raw_window_handle;
pub use system::{DeviceClass, Os, SystemInfo};
pub use window::{Rect, Size, SurfaceHandle, Window, WindowDesc, WindowId};
