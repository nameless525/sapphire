//! The shipped cue table: `default_cues.ron` beside this crate's manifest, compiled in like the presets it names.
//! The table's document is `sapphire_assets::cue_table`'s.

use crate::CueTable;

/// The shipped table (`default_cues.ron` beside this crate's manifest), compiled in like the presets it names.
const DEFAULT_TABLE: &str = include_str!("../default_cues.ron");

/// The shipped table. Which of Kenney's sounds plays for what is its cue documents' data.
#[must_use]
pub fn default_table() -> CueTable {
    CueTable::from_ron(DEFAULT_TABLE).expect("the shipped table is checked by this crate's tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_table_loads_and_round_trips() {
        let table = default_table();
        assert!(!table.rows().is_empty());
        let text = table.to_ron().expect("written");
        assert_eq!(CueTable::from_ron(&text).expect("read"), table);
    }

    /// Every sound the table plays is a cue document in `content/audio`, by its sidecar's ID, so an
    /// app that plays sound can load it from `audio.spack`, and every document's sound is cooked.
    #[test]
    fn every_sound_is_a_cue_document() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/audio");
        let sidecars: Vec<String> = std::fs::read_dir(&folder)
            .expect("reads")
            .map(|entry| entry.expect("reads").path())
            .filter(|path| path.to_string_lossy().ends_with(".cue.ron.meta"))
            .map(|path| std::fs::read_to_string(path).expect("reads"))
            .collect();
        let table = default_table();
        assert!(!table.sound_cues().is_empty());
        for cue in table.sound_cues() {
            let id = cue.to_string();
            assert!(sidecars.iter().any(|sidecar| sidecar.contains(&id)), "{id} has no cue document");
        }
    }

    /// The shipped table's score is a document in `content/audio/generated` and its voice lines are the
    /// voice over's in `content/vo` (M6 U11), by their sidecars' IDs, and every line its rows speak is in
    /// the voice script, and so in each language's recording script.
    #[test]
    fn the_music_and_voices_are_documents() {
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let read = |path: &str| std::fs::read_to_string(content.join(path)).expect("reads");
        let table = default_table();
        let score = table.music().expect("the fight has a score").to_string();
        assert!(read("audio/generated/music/lantern.score.ron.meta").contains(&score));
        let voices: Vec<String> = table.voices().iter().map(ToString::to_string).collect();
        assert_eq!(voices.len(), 2);
        assert!(read("vo/en.voice.ron.meta").contains(&voices[0]));
        assert!(read("vo/ja.voice.ron.meta").contains(&voices[1]));
        let script = read("vo/script.ron");
        let lines: Vec<&str> = table.rows().iter().filter_map(|row| row.line.as_deref()).collect();
        assert!(!lines.is_empty());
        for line in lines {
            assert!(script.contains(&format!("key: \"{line}\"")), "`{line}` isn't in content/vo/script.ron");
        }
        let mix = table.mix().expect("the fight has a mix").to_string();
        assert!(read("audio/game.mix.ron.meta").contains(&mix));
    }
}
