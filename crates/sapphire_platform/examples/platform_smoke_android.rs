//! The Android build of `platform_smoke`: the same app, packaged as the `cdylib`
//! (`libplatform_smoke_android.so`) that SDLActivity loads and calls `SDL_main` in.
//!
//! ```text
//! cargo build -p sapphire_platform --example platform_smoke_android --target aarch64-linux-android
//! ```
//!
//! Empty on every other target.
#![cfg(target_os = "android")]

#[path = "platform_smoke.rs"]
mod smoke;

sapphire_platform::export_android_main!(smoke::Smoke, init = smoke::init_logging);
