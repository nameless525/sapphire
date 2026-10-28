//! Repository source paths, re-exported from the renderer-independent watcher so callers of
//! hot reload keep their existing paths. These helpers read no shader or document contents.

pub use sapphire_watch::sources::*;

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
