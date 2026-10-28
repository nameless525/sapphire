//! Where the engine's own sources are, in the repository this build was compiled from: the
//! files besides content that development builds watch.
//!
//! They are found from this crate's place in the repository at compile time (it sits beside the
//! crates it names), so they hold on the machine that built it, the one a developer edits on.
//! Shipping code never learns them (rule R5): the runtime crates only ever get the text. Tests
//! check that they still name the files the engine compiles in.

use std::path::{Path, PathBuf};

/// The workspace's `crates/` directory.
fn crates_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

/// The renderer's WGSL sources: every `sapphire_render::ShaderFile`, by its file name.
pub fn shader_dir() -> PathBuf {
    crates_dir().join("sapphire_render").join("shaders")
}

/// The default day's timeline document, which `sapphire_world::tod::default_day` compiles in.
pub fn default_day() -> PathBuf {
    crates_dir().join("sapphire_world").join("default_day.ron")
}

/// The character light rig presets' document, which
/// `sapphire_world::light_rig::LightRigLibrary::default_rigs` compiles in.
pub fn default_light_rigs() -> PathBuf {
    crates_dir().join("sapphire_world").join("default_light_rigs.ron")
}

/// The screen effect presets' document, which
/// `sapphire_world::screen_effects::EffectLibrary::default_effects` compiles in.
pub fn default_screen_effects() -> PathBuf {
    crates_dir().join("sapphire_world").join("default_screen_effects.ron")
}
