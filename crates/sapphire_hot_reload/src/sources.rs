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

#[cfg(test)]
mod tests {
    use sapphire_render::{ShaderFile, ShaderSources};
    use sapphire_world::light_rig::LightRigLibrary;
    use sapphire_world::screen_effects::EffectLibrary;
    use sapphire_world::tod::TodTimeline;

    use super::*;

    #[test]
    fn the_shader_dir_holds_the_files_the_renderer_compiles_in() {
        let dir = shader_dir();
        for file in ShaderFile::ALL {
            let text = std::fs::read_to_string(dir.join(file.file_name()))
                .unwrap_or_else(|error| panic!("{file} in {}: {error}", dir.display()));
            assert_eq!(text, file.compiled_in(), "{file} is the file the renderer compiles in");
        }
        assert!(ShaderSources::compiled_in().is_compiled_in());
    }

    #[test]
    fn the_default_day_is_the_document_time_of_day_compiles_in() {
        let text = std::fs::read_to_string(default_day()).expect("the default day's document");
        let compiled_in = sapphire_world::tod::default_day();
        assert_eq!(TodTimeline::from_ron(&text).expect("a valid timeline"), compiled_in);
    }

    #[test]
    fn the_light_rigs_are_the_presets_the_library_compiles_in() {
        let text = std::fs::read_to_string(default_light_rigs()).expect("the light rig presets' document");
        let rigs = LightRigLibrary::from_ron(&text).expect("a valid library");
        assert_eq!(rigs, LightRigLibrary::default_rigs());
    }

    #[test]
    fn the_screen_effects_are_the_presets_the_library_compiles_in() {
        let text =
            std::fs::read_to_string(default_screen_effects()).expect("the screen effect presets' document");
        let effects = EffectLibrary::from_ron(&text).expect("a valid library");
        assert_eq!(effects, EffectLibrary::default_effects());
    }
}
