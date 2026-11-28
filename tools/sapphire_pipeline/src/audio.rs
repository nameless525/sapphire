//! Cooking sound (M2 E1, spec §5.15, §5.27): `.ogg` files to `.ssound`, `.cue.ron` documents to
//! `.scue`, and music and voice (M3 A1): `.score.ron` to `.sscore` and `.voice.ron` to `.svoice`; and a game's
//! mix (M6 U2), `.mix.ron` to `.smix`. The formats are `sapphire_assets`' plain data, so no mixer is linked here.
//!
//! A sound's cook copies the Ogg Vorbis file, the target format on every platform, after decoding
//! all of it with the runtime's decoder, so a file the runtime couldn't play fails the cook. Desktop
//! and mobile cooks are the same bytes.
//!
//! A cue's cook checks the document's values ([`Cue::from_document`]), and its references are
//! checked on every cook, cached or not, by [`dependencies`](crate::cook): each must be a sound of
//! this content tree that cooks. A score's stems must be sounds as long as its loop, to the frame, and
//! a voice line's cue must play on the voice bus; both are read from their sources on every cook too.
//!
//! Sounds, cues, scores and voice lines go in a pack of their own ([`crate::pack::AUDIO`]), but for a
//! language's voice lines and what they alone play, which go in its own (`vo_<locale>`, M6 U11). The voice
//! over's masters, encode and documents are [`crate::vo`]'s.
//!
//! A voice sound with a phoneme sidecar beside it (`<key>.phonemes.ron`, M6 U14, [`phonemes_path`]) has its mouth
//! curve's visemes said by the sidecar through its language's viseme table ([`viseme_table`], the one place a cook
//! chooses one), the envelope kept: [`sapphire_assets::phonemes`] has the format and the maths. The sidecar and
//! the table are both in the sound's cache key ([`phonemes_dependencies`]), so a sound without one keys and cooks
//! as before.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sapphire_assets::cue::{Bus, Cue, CueError};
use sapphire_assets::mix::{Mix, MixError};
use sapphire_assets::phonemes::{PHONEMES_SUFFIX, PhonemeError, PhonemeTimings, VisemeTable};
use sapphire_assets::score::{Score, ScoreError};
use sapphire_assets::sound::{Sound, SoundError};
use sapphire_assets::voice::{VoiceLines, VoiceLinesError};
use sapphire_serialize::cooked::CookedError;
use serde::{Deserialize, Serialize};

use crate::cache::{Dependency, Stage};

/// The sound importer as cache keys know it. Its name carries the decoder's version, which a test
/// keeps in step with `Cargo.lock`: a new decoder could find a different length.
pub const SOUND_IMPORTER: Stage = Stage { name: "ogg (lewton 0.10.2)", version: 1 };
/// The sound cooker. Bump it whenever it changes output for the same file.
// Version 2 preserves complete same-format Ogg chains; version 1 could cache truncated PCM
// frame counts from the decoder's chained-stream granule handling. The archive layout is unchanged.
// Version 3 writes a voice's mouth curve (M5 C6): a change to its analysis bumps it again. A change to how
// a phoneme sidecar says the visemes bumps `VISEME_MAPPER` instead.
pub const SOUND_COOKER: Stage = Stage { name: "ssound", version: 3 };
/// How a sound's phoneme sidecar becomes its curve's visemes (M6 U14), keyed into every sound with a sidecar
/// and no other ([`phonemes_dependencies`]). Bump it whenever `sapphire_assets::phonemes`' maths changes
/// output for the same sidecar and table: `frame_of`'s rounding, `shared`, the stress digit read past,
/// `SILENCES`, the frame walk in `Sound::with_phonemes`, or smoothing added there.
pub const VISEME_MAPPER: Stage = Stage { name: "visemes", version: 1 };
/// The cue importer as cache keys know it.
pub const CUE_IMPORTER: Stage = Stage { name: "cue document", version: 1 };
/// The cue cooker. Bump it whenever it changes output for the same document.
// Version 2 cooks a cue's tree (M6 U4a).
pub const CUE_COOKER: Stage = Stage { name: "scue", version: 2 };

/// The score importer as cache keys know it.
pub const SCORE_IMPORTER: Stage = Stage { name: "score document", version: 1 };
/// The score cooker. Bump it whenever it changes output for the same document.
pub const SCORE_COOKER: Stage = Stage { name: "sscore", version: 1 };
/// The voice-line importer as cache keys know it.
pub const VOICE_IMPORTER: Stage = Stage { name: "voice lines document", version: 1 };
/// The voice-line cooker. Bump it whenever it changes output for the same document.
pub const VOICE_COOKER: Stage = Stage { name: "svoice", version: 1 };
/// The mix importer as cache keys know it (M6 U2).
pub const MIX_IMPORTER: Stage = Stage { name: "mix document", version: 1 };
/// The mix cooker. Bump it whenever it changes output for the same document.
pub const MIX_COOKER: Stage = Stage { name: "smix", version: 1 };

/// What a cue document's file name ends with.
const CUE_SOURCE_SUFFIX: &str = ".cue.ron";
/// What a score document's file name ends with.
const SCORE_SOURCE_SUFFIX: &str = ".score.ron";
/// What a voice-line document's file name ends with.
const VOICE_SOURCE_SUFFIX: &str = ".voice.ron";
/// What a mix document's file name ends with.
const MIX_SOURCE_SUFFIX: &str = ".mix.ron";

/// Import settings for sounds: `importer: Sound(())` for most, and `Sound((mouth: true))` for a voice's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoundSettings {
    /// Cook a mouth curve into it (M5 C6): what a face says it with. Every sound a voice line's cue plays
    /// needs one, which the voice lines' cook holds.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mouth: bool,
}

/// Import settings for cues, as for sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueSettings {}

/// Import settings for scores, as for sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreSettings {}

/// Import settings for voice lines, as for sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceLinesSettings {}

/// Import settings for a mix, as for sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixSettings {}

/// A sound or cue that won't cook.
#[derive(Debug, thiserror::Error)]
pub enum AudioCookError {
    #[error(transparent)]
    Sound(#[from] SoundError),
    #[error(transparent)]
    Cue(#[from] CueError),
    #[error(transparent)]
    Score(#[from] ScoreError),
    #[error(transparent)]
    VoiceLines(#[from] VoiceLinesError),
    #[error(transparent)]
    Mix(#[from] MixError),
    /// A stem that wouldn't loop in step with its score's bars.
    #[error(
        "stem `{stem}` is {frames} frames at {sample_rate} Hz, and the loop is {loop_seconds} s: {expected}"
    )]
    StemLength { stem: String, frames: u32, sample_rate: u32, loop_seconds: f64, expected: String },
    /// A voice line whose cue plays somewhere music wouldn't duck for it.
    #[error("line `{key}`'s cue plays on the {bus:?} bus, not Voice")]
    LineBus { key: String, bus: Bus },
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("the document isn't UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    /// A phoneme sidecar that can't be read, or can't be said by the sound beside it (M6 U14).
    #[error("its phonemes, `{file}`: {source}")]
    Phonemes { file: String, source: PhonemeError },
    /// A sidecar in a language no viseme table maps.
    #[error("its phonemes, `{file}`, are in {locale}, and no viseme table maps that language")]
    NoVisemeTable { file: String, locale: String },
}

/// Whether `path` names a cue document.
pub fn is_cue_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(CUE_SOURCE_SUFFIX))
}

/// Whether `path` names a score document.
pub fn is_score_source(path: &Path) -> bool {
    ends_with(path, SCORE_SOURCE_SUFFIX)
}

/// Whether `path` names a voice-line document.
pub fn is_voice_source(path: &Path) -> bool {
    ends_with(path, VOICE_SOURCE_SUFFIX)
}

/// Whether `path` names a mix document.
pub fn is_mix_source(path: &Path) -> bool {
    ends_with(path, MIX_SOURCE_SUFFIX)
}

fn ends_with(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(suffix))
}

/// The cooked file a cue document at `relative` writes: `hit.cue.ron` → `hit.scue`.
pub fn cue_output_path(relative: &Path, extension: &str) -> PathBuf {
    output_path(relative, CUE_SOURCE_SUFFIX, extension)
}

/// The cooked file a score at `relative` writes: `ward.score.ron` → `ward.sscore`.
pub fn score_output_path(relative: &Path, extension: &str) -> PathBuf {
    output_path(relative, SCORE_SOURCE_SUFFIX, extension)
}

/// The cooked file voice lines at `relative` write: `ja.voice.ron` → `ja.svoice`.
pub fn voice_output_path(relative: &Path, extension: &str) -> PathBuf {
    output_path(relative, VOICE_SOURCE_SUFFIX, extension)
}

/// The cooked file a mix at `relative` writes: `game.mix.ron` → `game.smix`.
pub fn mix_output_path(relative: &Path, extension: &str) -> PathBuf {
    output_path(relative, MIX_SOURCE_SUFFIX, extension)
}

fn output_path(relative: &Path, suffix: &str, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(suffix.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a cue document.
pub fn read_cue(bytes: &[u8]) -> Result<Cue, AudioCookError> {
    Ok(Cue::from_document(std::str::from_utf8(bytes)?)?)
}

/// The bytes of a cue's cooked file, read back through the runtime's check.
pub fn cook_cue(bytes: &[u8]) -> Result<Vec<u8>, AudioCookError> {
    let cooked = read_cue(bytes)?.to_cooked()?;
    Cue::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Reads and checks a score document.
pub fn read_score(bytes: &[u8]) -> Result<Score, AudioCookError> {
    Ok(Score::from_document(std::str::from_utf8(bytes)?)?)
}

/// The bytes of a score's cooked file, read back through the runtime's check.
pub fn cook_score(bytes: &[u8]) -> Result<Vec<u8>, AudioCookError> {
    let cooked = read_score(bytes)?.to_cooked()?;
    Score::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Holds the stem `stem` of `score`, whose `.ogg` is `vorbis`, to the loop's length: a stem a frame off
/// drifts from the bars a little more each time round.
pub fn check_stem(score: &Score, stem: &str, vorbis: &[u8]) -> Result<(), AudioCookError> {
    let sound = Sound::from_vorbis(vorbis.to_vec())?;
    let expected = score.stem_frames(sound.sample_rate);
    if expected == Some(u64::from(sound.frames)) {
        return Ok(());
    }
    Err(AudioCookError::StemLength {
        stem: stem.to_owned(),
        frames: sound.frames,
        sample_rate: sound.sample_rate,
        loop_seconds: score.loop_seconds(),
        expected: match expected {
            Some(frames) => format!("it must be {frames}"),
            None => "no stem at that rate can be a whole number of frames that long".to_owned(),
        },
    })
}

/// Reads and checks a voice-line document.
pub fn read_voice_lines(bytes: &[u8]) -> Result<VoiceLines, AudioCookError> {
    Ok(VoiceLines::from_document(std::str::from_utf8(bytes)?)?)
}

/// The bytes of a voice-line table's cooked file, read back through the runtime's check.
pub fn cook_voice_lines(bytes: &[u8]) -> Result<Vec<u8>, AudioCookError> {
    let cooked = read_voice_lines(bytes)?.to_cooked()?;
    VoiceLines::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Reads and checks a mix document.
pub fn read_mix(bytes: &[u8]) -> Result<Mix, AudioCookError> {
    Ok(Mix::from_document(std::str::from_utf8(bytes)?)?)
}

/// The bytes of a mix's cooked file, read back through the runtime's check.
pub fn cook_mix(bytes: &[u8]) -> Result<Vec<u8>, AudioCookError> {
    let cooked = read_mix(bytes)?.to_cooked()?;
    Mix::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Holds the line `key` to its cue's document, `cue`: it plays on the voice bus, which music ducks for.
pub fn check_line(key: &str, cue: &[u8]) -> Result<(), AudioCookError> {
    let bus = read_cue(cue)?.bus;
    if bus == Bus::Voice { Ok(()) } else { Err(AudioCookError::LineBus { key: key.to_owned(), bus }) }
}

/// Where the phoneme sidecar of the sound at `sound` is (M6 U14): beside it, by its stem.
pub fn phonemes_path(sound: &Path) -> PathBuf {
    let stem = sound.file_stem().map(|stem| stem.to_string_lossy()).unwrap_or_default();
    sound.with_file_name(format!("{stem}{PHONEMES_SUFFIX}"))
}

/// The viseme table timings in `locale` are mapped through: the engine's own ([`VisemeTable::builtin`]) until
/// M6 A24's per-language documents replace it here, the one place a cook chooses a table. Reading a document
/// takes more than this body: this signature (it sees only a locale, not the planned assets), the Sound arms of
/// `cook::cook_source` and of `cook::source_dependencies` (the table keyed as a `Dependency::Asset` rather than
/// [`phonemes_dependencies`]' `#visemes` text), and `Sound::with_phonemes`, for smoothing.
pub fn viseme_table(locale: &str) -> Option<VisemeTable> {
    VisemeTable::builtin(locale)
}

/// A phoneme sidecar, `text`, read with the table its language maps through. `file` names it in errors.
fn read_phonemes(file: &str, text: &[u8]) -> Result<(PhonemeTimings, VisemeTable), AudioCookError> {
    let phonemes = |source| AudioCookError::Phonemes { file: file.to_owned(), source };
    let text = std::str::from_utf8(text)?;
    let timings = PhonemeTimings::from_document(text).map_err(phonemes)?;
    let table = viseme_table(&timings.locale).ok_or_else(|| AudioCookError::NoVisemeTable {
        file: file.to_owned(),
        locale: timings.locale.clone(),
    })?;
    Ok((timings, table))
}

/// What a sound's phoneme sidecar, `text` in the file `file` beside it, adds to its cache key: the sidecar's
/// bytes, and the table it maps through, by its contents, under [`VISEME_MAPPER`]'s name and version
/// (`#visemes v1 en`), so a change to either, or a bump, cooks the sound again.
pub fn phonemes_dependencies(file: &str, text: &[u8]) -> Result<Vec<Dependency>, AudioCookError> {
    let (_, table) = read_phonemes(file, text)?;
    let mut shapes = format!("{};", table.language);
    for (phoneme, shape) in &table.shapes {
        let [aa, ih, ou, ee, oh] = shape;
        // A table's own text, one spelling of it: `AA=255,0,0,0,0;`.
        let _ = write!(shapes, "{phoneme}={aa},{ih},{ou},{ee},{oh};");
    }
    Ok(vec![
        Dependency::File { path: file.to_owned(), hash: blake3::hash(text) },
        Dependency::File {
            path: format!("#{} v{} {}", VISEME_MAPPER.name, VISEME_MAPPER.version, table.language),
            hash: blake3::hash(shapes.as_bytes()),
        },
    ])
}

/// The bytes of a sound's cooked file, decoded whole on the way and read back through the runtime's
/// check, with its mouth curve if `settings` ask, its visemes said by `phonemes` if the sound has a sidecar:
/// its file's name, and its text.
pub fn cook_sound(
    bytes: &[u8],
    settings: SoundSettings,
    phonemes: Option<(&str, &[u8])>,
) -> Result<Vec<u8>, AudioCookError> {
    let sound = Sound::from_vorbis(bytes.to_vec())?;
    let sound = if settings.mouth { sound.with_mouth()? } else { sound };
    let sound = match phonemes {
        Some((file, text)) => {
            let (timings, table) = read_phonemes(file, text)?;
            sound
                .with_phonemes(&timings, &table)
                .map_err(|source| AudioCookError::Phonemes { file: file.to_owned(), source })?
        }
        None => sound,
    };
    let cooked = sound.to_cooked()?;
    Sound::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::fs;
    use std::sync::Arc;

    use sapphire_assets::cue::Bus;
    use sapphire_assets::sound::SOUND_EXTENSION;
    use sapphire_assets::{AssetBudgets, AssetId, AssetKind, AssetServer, Handle, MemoryPackSource};

    use super::*;
    use crate::cook::{self, Cook, CookError, CookOptions, Recipe, cook_dir};
    use crate::meta::{self, ImportSettings};
    use crate::pack::{self, pack_cooked};
    use crate::test_util::{TempDir, read_aligned};

    /// A small sound from the third-party folder.
    fn sample_ogg() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kenney_interface_sounds/Audio/click_001.ogg");
        fs::read(path).expect("the sample sound")
    }

    fn cue_document(sounds: &[AssetId]) -> String {
        let names: Vec<String> = sounds.iter().map(|id| format!("\"{id}\"")).collect();
        format!(
            "(\n    type: \"sapphire::assets::Cue\",\n    version: 1,\n    data: (\n        sounds: [{}],\n        bus: Ui,\n    ),\n)\n",
            names.join(", ")
        )
    }

    /// A content tree holding `source.ogg` and whatever the caller writes beside it, each with the
    /// sidecars a first cook gives it.
    struct Tree {
        dir: TempDir,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let dir = TempDir::new(name);
            fs::create_dir_all(dir.path().join("content")).unwrap();
            fs::write(dir.path().join("content/source.ogg"), sample_ogg()).unwrap();
            Self { dir }
        }

        fn content(&self) -> PathBuf {
            self.dir.path().join("content")
        }

        fn cook(&self) -> Result<cook::CookReport, CookError> {
            let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
            cook_dir(&self.content(), &self.dir.path().join("loose"), &options)
        }

        fn id_of(&self, source: &str) -> AssetId {
            let sidecar = meta::sidecar_path(&self.content().join(source));
            meta::read(&sidecar).expect("a sidecar").id
        }
    }

    #[test]
    fn extensions_pick_the_importers() {
        assert_eq!(
            ImportSettings::for_source(Path::new("a/b.ogg")),
            Some(ImportSettings::Sound(SoundSettings::default()))
        );
        assert_eq!(
            ImportSettings::for_source(Path::new("a/HIT.OGG")),
            Some(ImportSettings::Sound(SoundSettings::default()))
        );
        assert_eq!(
            ImportSettings::for_source(Path::new("a/hit.cue.ron")),
            Some(ImportSettings::Cue(CueSettings::default()))
        );
        let recipe = Recipe::for_settings(&ImportSettings::Sound(SoundSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/hit.ogg")), Path::new("a/hit.ssound"));
        let recipe = Recipe::for_settings(&ImportSettings::Cue(CueSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/hit.cue.ron")), Path::new("a/hit.scue"));
    }

    #[test]
    fn a_sound_and_its_cue_cook_pack_and_load_as_assets() {
        let tree = Tree::new("audio_round_trip");
        // The first cook gives the sound its sidecar, and so its ID.
        tree.cook().unwrap();
        let sound_id = tree.id_of("source.ogg");
        fs::write(tree.content().join("hit.cue.ron"), cue_document(&[sound_id])).unwrap();
        let report = tree.cook().unwrap();
        let cue_id = tree.id_of("hit.cue.ron");
        assert_eq!(report.assets.len(), 2);
        assert!(report.created_sidecars.iter().any(|path| path.ends_with("hit.cue.ron.meta")));

        // Round trip: the cooked sound holds the source's bytes.
        let cooked_sound = report.assets.iter().find(|asset| asset.id == sound_id).unwrap();
        assert_eq!(cooked_sound.output.extension().unwrap(), SOUND_EXTENSION);
        let bytes = read_aligned(&cooked_sound.output);
        let sound = Sound::read(&bytes).unwrap();
        assert_eq!(sound.vorbis.as_slice(), sample_ogg().as_slice());
        let decoded = sound.decode().unwrap();
        assert_eq!(decoded.frames() as u32, sound.frames.to_native());

        // The pack: sounds and cues on their own, in a pack the cue's dependency reaches.
        let packs = tree.dir.path().join("packs");
        let packed = pack_cooked(&report, &packs, None).unwrap();
        let names: BTreeSet<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
        assert_eq!(names, BTreeSet::from([pack::AUDIO]));
        assert!(!packs.join("shared.spack").exists());
        let bytes = fs::read(packs.join("audio.spack")).unwrap();
        let source = Arc::new(MemoryPackSource::new());
        source.insert(pack::AUDIO, bytes);
        let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
        server.mount_now(pack::AUDIO).unwrap();
        // Asking for the cue loads its sound: neither ever reaches the uploader.
        let cue: Handle<Cue> = server.load(cue_id).unwrap();
        server.finish(&cue, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
        let cue = server.get(&cue).expect("the cue loaded").to_owned_cue();
        assert_eq!((cue.sounds(), cue.bus), (vec![sound_id], Bus::Ui));
        let sound: Handle<Sound> = server.load(sound_id).unwrap();
        assert_eq!(
            server.get(&sound).expect("loaded with its cue").vorbis.as_slice(),
            sample_ogg().as_slice()
        );
    }

    #[test]
    fn a_kinds_group_decides_its_pack() {
        assert_eq!(pack::group(Some(AssetKind::Sound)), pack::AUDIO);
        assert_eq!(pack::group(Some(AssetKind::Cue)), pack::AUDIO);
        assert_eq!(pack::group(Some(AssetKind::Score)), pack::AUDIO);
        assert_eq!(pack::group(Some(AssetKind::VoiceLines)), pack::AUDIO);
        assert_eq!(pack::group(Some(AssetKind::Mix)), pack::AUDIO);
        for kind in
            [AssetKind::Mesh, AssetKind::Texture, AssetKind::Material, AssetKind::Rig, AssetKind::ProbeGrid]
        {
            assert_eq!(pack::group(Some(kind)), pack::SHARED, "{kind}");
        }
        // What the packer can't name is left for its error.
        assert_eq!(pack::group(None), pack::SHARED);
    }

    #[test]
    fn the_shipped_mix_cooks_to_the_same_bytes_twice_and_reads_back() {
        let document = include_bytes!("../../../content/audio/game.mix.ron");
        let first = cook_mix(document).unwrap();
        assert_eq!(first, cook_mix(document).unwrap(), "two cooks of the mix agree byte for byte");
        let mut aligned: sapphire_serialize::cooked::AlignedVec = Default::default();
        aligned.extend_from_slice(&first);
        let cooked = Mix::read(&aligned).unwrap().to_mix();
        assert_eq!(cooked, read_mix(document).unwrap(), "and read back as the document");
    }

    #[test]
    fn a_cue_naming_what_is_not_a_sound_fails_the_cook_by_name() {
        let tree = Tree::new("audio_not_a_sound");
        tree.cook().unwrap();
        // Nothing has this ID.
        let missing: AssetId = "9f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c".parse().unwrap();
        fs::write(tree.content().join("hit.cue.ron"), cue_document(&[missing])).unwrap();
        let error = tree.cook().unwrap_err().to_string();
        assert!(error.contains("hit.cue.ron") && error.contains(&missing.to_string()), "{error}");
        // A cue that names a cue.
        let id = tree.id_of("hit.cue.ron");
        fs::write(tree.content().join("hit.cue.ron"), cue_document(&[id])).unwrap();
        assert!(matches!(tree.cook(), Err(CookError::NotASound { .. })));
    }

    #[test]
    fn a_cue_with_a_bad_value_fails_the_cook_naming_it() {
        let tree = Tree::new("audio_bad_value");
        tree.cook().unwrap();
        let id = tree.id_of("source.ogg");
        let text = cue_document(&[id]).replace("bus: Ui", "pitch: (2.0, 1.0)");
        fs::write(tree.content().join("hit.cue.ron"), text).unwrap();
        let error = anyhow_chain(&tree.cook().unwrap_err());
        assert!(error.contains("hit.cue.ron") && error.contains("pitch"), "{error}");
    }

    /// A sound half a second long from the third-party folder: long enough to be a bar.
    fn stem_ogg() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kenney_interface_sounds/Audio/confirmation_002.ogg");
        fs::read(path).expect("the stem")
    }

    /// A score of the one stem `sound`, [`stem_ogg`], one bar of one beat at the tempo that makes it
    /// that bar, looping `bars` bars.
    fn score_document(sound: AssetId, bars: u32) -> String {
        let stem = Sound::from_vorbis(stem_ogg()).unwrap();
        let tempo = (60.0 * f64::from(stem.sample_rate) / f64::from(stem.frames)) as f32;
        let beats = 1;
        format!(
            "(\n    type: \"sapphire::assets::Score\",\n    version: 2,\n    data: (\n        tempo_bpm: {tempo:?},\n        \
             beats_per_bar: {beats},\n        bars: {bars},\n        stems: [(name: \"click\", sound: \"{sound}\")],\n        \
             phases: [(name: \"running\", play: [\"click\"])],\n    ),\n)\n"
        )
    }

    fn voice_document(key: &str, cue: AssetId) -> String {
        format!(
            "(\n    type: \"sapphire::assets::VoiceLines\",\n    version: 1,\n    data: (\n        locale: \"ja\",\n        \
             lines: [(key: \"{key}\", cue: \"{cue}\")],\n    ),\n)\n"
        )
    }

    /// A tree with a score of `stem.ogg` and Japanese lines speaking `line.cue.ron`, a cue of it on
    /// the voice bus, all with their sidecars, beside the tree's own `source.ogg`.
    fn music_tree(name: &str) -> (Tree, AssetId, AssetId) {
        let tree = Tree::new(name);
        fs::write(tree.content().join("stem.ogg"), stem_ogg()).unwrap();
        tree.cook().unwrap();
        let sound = tree.id_of("stem.ogg");
        set_mouth(&tree, "stem.ogg", true);
        fs::write(
            tree.content().join("line.cue.ron"),
            cue_document(&[sound]).replace("bus: Ui", "bus: Voice"),
        )
        .unwrap();
        fs::write(tree.content().join("fight.score.ron"), score_document(sound, 1)).unwrap();
        tree.cook().unwrap();
        let cue = tree.id_of("line.cue.ron");
        fs::write(tree.content().join("ja.voice.ron"), voice_document("perfect-dodge", cue)).unwrap();
        (tree, sound, cue)
    }

    /// Asks the cook for `source`'s mouth curve, or not, in its sidecar.
    fn set_mouth(tree: &Tree, source: &str, mouth: bool) {
        let sidecar = meta::sidecar_path(&tree.content().join(source));
        let (plain, voiced) = ("importer: Sound(())", "importer: Sound((mouth: true))");
        let text = fs::read_to_string(&sidecar).unwrap();
        let (from, to) = if mouth { (plain, voiced) } else { (voiced, plain) };
        assert!(text.contains(from), "{text}");
        fs::write(&sidecar, text.replace(from, to)).unwrap();
    }

    /// M5 C6: every sound a line's cue plays carries a mouth curve, so a face can say it.
    #[test]
    fn a_line_whose_sound_has_no_mouth_fails_the_cook_naming_it() {
        let (tree, sound, _) = music_tree("audio_line_mouth");
        tree.cook().unwrap();
        set_mouth(&tree, "stem.ogg", false);
        let error = tree.cook().unwrap_err();
        assert!(matches!(error, CookError::LineWithoutMouth { .. }), "{error}");
        let error = error.to_string();
        assert!(
            error.contains("ja.voice.ron")
                && error.contains("`perfect-dodge`")
                && error.contains(&sound.to_string()),
            "{error}"
        );
    }

    /// M5 C6's cook byte test: the committed voices' sounds cook to the same bytes twice, and their mouth
    /// curves to the bytes pinned here, on every machine CI cooks on. A change to the analysis moves them, and
    /// bumps [`SOUND_COOKER`]. These are the analysis' curves, with no sidecar: M6 U14 moved none of them, and
    /// the next test holds what the lines with a sidecar cook to.
    #[test]
    fn the_generated_lines_mouths_cook_to_pinned_bytes() {
        let voices = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/vo");
        let pinned: [(&str, usize, u64); 6] = [
            ("en/perfect-dodge", 52, 4565562129575115727),
            ("en/swap-in", 49, 3905336652525319422),
            ("en/guide_greet", 193, 1843938029179445591),
            ("ja/perfect-dodge", 65, 5414503529634744099),
            ("ja/swap-in", 56, 8243626753512071208),
            ("ja/guide_greet", 142, 12031829780209364577),
        ];
        let mut found = Vec::new();
        for (line, _, _) in pinned {
            let ogg = fs::read(voices.join(format!("{line}.ogg"))).unwrap();
            let cooked = cook_sound(&ogg, SoundSettings { mouth: true }, None).unwrap();
            assert_eq!(cook_sound(&ogg, SoundSettings { mouth: true }, None).unwrap(), cooked, "{line}");
            let plain = cook_sound(&ogg, SoundSettings::default(), None).unwrap();
            assert!(Sound::read(&read_aligned_bytes(&plain)).unwrap().mouth.is_none(), "{line}");
            let curve = Sound::read(&read_aligned_bytes(&cooked)).unwrap().to_sound().mouth.unwrap();
            // FNV-1a over each frame's six bytes.
            let hash = curve
                .frames
                .iter()
                .flat_map(|frame| [&[frame.open][..], &frame.visemes].concat())
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            found.push((line, curve.frames.len(), hash));
        }
        assert_eq!(found, pinned);
    }

    /// M6 U14's: each committed line with a phoneme sidecar cooks, on every machine, to the analysis' envelope
    /// byte for byte and visemes the sidecar says, frame by frame, pinned here as the last test pins the analysis'.
    /// A change to the analysis moves them and bumps [`SOUND_COOKER`]; a change to how a sidecar says the
    /// visemes (`sapphire_assets::phonemes`' maths) moves them and bumps [`VISEME_MAPPER`]; a change to a table
    /// or a sidecar moves them and re-keys the sound by its own text, with no bump.
    #[test]
    fn the_lines_with_phonemes_cook_their_envelope_and_their_phonemes() {
        use sapphire_assets::mouth::MouthCurve;
        use sapphire_assets::phonemes::frame_of;

        let voices = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/vo");
        let pinned: [(&str, u64); 2] =
            [("en/perfect-dodge", 16494492188682201362), ("ja/perfect-dodge", 12618320998961130273)];
        let mut found = Vec::new();
        for (line, _) in pinned {
            let ogg = fs::read(voices.join(format!("{line}.ogg"))).unwrap();
            let sidecar = phonemes_path(&voices.join(format!("{line}.ogg")));
            let text = fs::read(&sidecar).unwrap();
            let file = sidecar.file_name().unwrap().to_str().unwrap();
            let curve = |phonemes| {
                let cooked = cook_sound(&ogg, SoundSettings { mouth: true }, phonemes).unwrap();
                Sound::read(&read_aligned_bytes(&cooked)).unwrap().to_sound().mouth.unwrap()
            };
            let (heard, said) = (curve(None), curve(Some((file, &text))));
            assert_eq!(said, curve(Some((file, &text))), "{line}: two cooks agree");
            let opens = |curve: &MouthCurve| curve.frames.iter().map(|frame| frame.open).collect::<Vec<_>>();
            assert_eq!(opens(&said), opens(&heard), "{line}: the envelope is the analysis'");
            assert_ne!(said, heard, "{line}: the visemes are the sidecar's");
            let (timings, table) = read_phonemes(file, &text).unwrap();
            for phoneme in &timings.phonemes {
                let at = frame_of(phoneme.from_ms);
                if at < frame_of(phoneme.to_ms) {
                    let shape = table.shape(&phoneme.phoneme).unwrap();
                    let frame = said.frames[at];
                    let share = |share: u8| ((u32::from(frame.open) * u32::from(share) + 127) / 255) as u8;
                    assert_eq!(frame.visemes, shape.map(share), "{line}: {phoneme:?} at frame {at}");
                }
            }
            let hash = said
                .frames
                .iter()
                .flat_map(|frame| [&[frame.open][..], &frame.visemes].concat())
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            found.push((line, hash));
        }
        assert_eq!(found, pinned);
    }

    /// A phoneme sidecar's document with one phoneme, from 0 to 50 ms.
    fn timings(locale: &str, phoneme: &str) -> String {
        format!(
            "(type: \"sapphire::assets::PhonemeTimings\", version: 1, data: (locale: \"{locale}\", phonemes: [\
             (phoneme: \"{phoneme}\", from_ms: 0, to_ms: 50)]))"
        )
    }

    /// A sidecar beside a voice's sound cooks into its curve, and one that can't be said fails the cook naming
    /// the sidecar and what's wrong in it; one beside a sound with no curve, or in a language no table maps, too.
    #[test]
    fn a_sounds_phonemes_cook_into_its_curve_or_fail_naming_the_sidecar() {
        let tree = Tree::new("audio_phonemes");
        fs::write(tree.content().join("stem.ogg"), stem_ogg()).unwrap();
        tree.cook().unwrap();
        set_mouth(&tree, "stem.ogg", true);
        let cooked = || {
            let bytes = fs::read(tree.dir.path().join("loose").join("stem.ssound")).unwrap();
            Sound::read(&read_aligned_bytes(&bytes)).unwrap().to_sound().mouth.unwrap()
        };
        tree.cook().unwrap();
        let heard = cooked();
        let sidecar = tree.content().join("stem.phonemes.ron");
        fs::write(&sidecar, timings("ja", "i")).unwrap();
        tree.cook().unwrap();
        let said = cooked();
        assert_eq!(said.frames.len(), heard.frames.len());
        for (at, (said, heard)) in said.frames.iter().zip(&heard.frames).enumerate() {
            assert_eq!(said.open, heard.open, "frame {at}");
            // 50 ms lands on frame 3: `i` is `ih`, the rest shut.
            let expected = if at < 3 { [0, said.open, 0, 0, 0] } else { [0; 5] };
            assert_eq!(said.visemes, expected, "frame {at}");
        }

        for (text, why) in [
            (timings("ja", "qq"), "`phonemes[0]` is `qq`, which the ja viseme table hasn't"),
            (timings("fr", "a"), "are in fr, and no viseme table maps that language"),
            (timings("ja", "a").replace("to_ms: 50", "to_ms: 0"), "it must end after it starts"),
        ] {
            fs::write(&sidecar, text).unwrap();
            let error = crate::refusal::chain(&tree.cook().unwrap_err());
            let named = error.contains("stem.ogg") && error.contains("stem.phonemes.ron");
            assert!(named && error.contains(why), "{error}");
        }
        fs::write(&sidecar, timings("ja", "a")).unwrap();
        set_mouth(&tree, "stem.ogg", false);
        let error = crate::refusal::chain(&tree.cook().unwrap_err());
        assert!(error.contains("stem.phonemes.ron") && error.contains("has no mouth curve"), "{error}");
    }

    /// The sidecar and the table are in a sound's key, by their contents, and a sound without one keys as before.
    #[test]
    fn a_sidecar_and_its_table_key_the_sound() {
        let keyed = |phoneme: &str| {
            phonemes_dependencies("line.phonemes.ron", timings("en-GB", phoneme).as_bytes()).unwrap()
        };
        let (first, second) = (keyed("AA"), keyed("IY"));
        assert_eq!(first.len(), 2);
        let hash = |dependency: &Dependency| match dependency {
            Dependency::File { path, hash } => (path.clone(), *hash),
            Dependency::Asset { .. } => unreachable!("a sidecar's dependencies are files"),
        };
        assert_ne!(hash(&first[0]), hash(&second[0]), "the sidecar's bytes");
        assert_eq!(hash(&first[1]), hash(&second[1]), "the same table");
        assert_eq!(hash(&first[1]).0, "#visemes v1 en", "the mapper's version keys it too");
        assert_eq!(
            phonemes_path(Path::new("vo/en/perfect-dodge.ogg")),
            Path::new("vo/en/perfect-dodge.phonemes.ron")
        );
    }

    fn read_aligned_bytes(bytes: &[u8]) -> sapphire_serialize::cooked::AlignedVec {
        let mut aligned = sapphire_serialize::cooked::AlignedVec::with_capacity(bytes.len());
        aligned.extend_from_slice(bytes);
        aligned
    }

    #[test]
    fn scores_and_voice_lines_are_found_by_their_suffixes() {
        assert_eq!(
            ImportSettings::for_source(Path::new("a/fight.score.ron")),
            Some(ImportSettings::Score(ScoreSettings::default()))
        );
        assert_eq!(
            ImportSettings::for_source(Path::new("a/ja.voice.ron")),
            Some(ImportSettings::VoiceLines(VoiceLinesSettings::default()))
        );
        let recipe = Recipe::for_settings(&ImportSettings::Score(ScoreSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/fight.score.ron")), Path::new("a/fight.sscore"));
        let recipe = Recipe::for_settings(&ImportSettings::VoiceLines(VoiceLinesSettings::default()));
        assert_eq!(recipe.output_path(Path::new("a/ja.voice.ron")), Path::new("a/ja.svoice"));
    }

    #[test]
    fn a_score_and_voice_lines_cook_pack_and_load_with_what_they_name() {
        let (tree, sound, cue) = music_tree("audio_music_round_trip");
        let report = tree.cook().unwrap();
        let (score_id, lines_id) = (tree.id_of("fight.score.ron"), tree.id_of("ja.voice.ron"));
        assert_eq!(report.assets.len(), 5);

        // The lines and their cue go in their language's pack (M6 U11); the stem stays with the score that
        // plays it too, since the audio pack can't lean on a language's.
        let packs = tree.dir.path().join("packs");
        let packed = pack_cooked(&report, &packs, None).unwrap();
        let names: BTreeSet<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
        assert_eq!(
            names,
            BTreeSet::from([pack::AUDIO, "vo_ja"]),
            "music goes with the sounds, voice by language"
        );
        let held = |name: &str| packed.packs.iter().find(|pack| pack.name == name).unwrap().assets;
        assert_eq!((held(pack::AUDIO), held("vo_ja")), (3, 2));
        let source = Arc::new(MemoryPackSource::new());
        source.insert(pack::AUDIO, fs::read(packs.join("audio.spack")).unwrap());
        source.insert("vo_ja", fs::read(packs.join("vo_ja.spack")).unwrap());
        let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
        server.mount_now(pack::AUDIO).unwrap();
        server.mount_now("vo_ja").unwrap();
        let score: Handle<Score> = server.load(score_id).unwrap();
        server.finish(&score, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
        assert_eq!(server.get(&score).expect("loaded").to_owned_score().stems[0].sound, sound);
        let stem: Handle<Sound> = server.load(sound).unwrap();
        assert!(server.get(&stem).is_some(), "the stem came with it");
        let lines: Handle<VoiceLines> = server.load(lines_id).unwrap();
        server.finish(&lines, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
        let lines = server.get(&lines).expect("loaded").to_owned_lines();
        assert_eq!((lines.locale.as_str(), lines.lines[0].cue), ("ja", cue));
    }

    /// What two languages' lines share, and a sound a voice cue and a plain cue share, stay in `audio.spack`
    /// (fixed after M6 U11's review, which found the split untested); what one language's lines alone reach goes
    /// in its pack.
    #[test]
    fn shared_sounds_and_cues_stay_in_the_audio_pack() {
        let tree = Tree::new("audio_voice_split");
        let sounds = ["shared.ogg", "en_only.ogg", "ja_only.ogg"];
        for sound in sounds {
            fs::write(tree.content().join(sound), stem_ogg()).unwrap();
        }
        tree.cook().unwrap();
        for sound in sounds {
            set_mouth(&tree, sound, true);
        }
        let [shared, en_only, ja_only] = sounds.map(|sound| tree.id_of(sound));
        let voiced = |sound: AssetId| cue_document(&[sound]).replace("bus: Ui", "bus: Voice");
        fs::write(tree.content().join("both.cue.ron"), voiced(shared)).unwrap();
        fs::write(tree.content().join("en.cue.ron"), voiced(en_only)).unwrap();
        fs::write(tree.content().join("ja.cue.ron"), voiced(ja_only)).unwrap();
        // A plain cue playing the Japanese line's sound: the sound can't leave for `vo_ja`.
        fs::write(tree.content().join("bell.cue.ron"), cue_document(&[ja_only])).unwrap();
        tree.cook().unwrap();
        let [both, en, ja, bell] =
            ["both", "en", "ja", "bell"].map(|cue| tree.id_of(&format!("{cue}.cue.ron")));
        let lines = |locale: &str, own: AssetId| {
            let text = voice_document("own", own).replace("locale: \"ja\"", &format!("locale: \"{locale}\""));
            text.replace("lines: [", &format!("lines: [(key: \"both\", cue: \"{both}\"), "))
        };
        fs::write(tree.content().join("en.voice.ron"), lines("en", en)).unwrap();
        fs::write(tree.content().join("ja.voice.ron"), lines("ja", ja)).unwrap();
        let report = tree.cook().unwrap();

        let packs = tree.dir.path().join("packs");
        pack_cooked(&report, &packs, None).unwrap();
        let ids = |name: &str| -> BTreeSet<AssetId> {
            let bytes = read_aligned(&packs.join(format!("{name}.spack")));
            let reader = sapphire_assets::pack::PackReader::new(&bytes).unwrap();
            reader.toc().entries().iter().map(|entry| entry.id).collect()
        };
        let (en_lines, ja_lines) = (tree.id_of("en.voice.ron"), tree.id_of("ja.voice.ron"));
        let source = tree.id_of("source.ogg");
        assert_eq!(ids(pack::AUDIO), BTreeSet::from([source, shared, both, ja_only, bell]));
        assert_eq!(ids("vo_en"), BTreeSet::from([en_lines, en, en_only]));
        assert_eq!(ids("vo_ja"), BTreeSet::from([ja_lines, ja]));
    }

    #[test]
    fn a_stem_that_isnt_whole_bars_fails_the_cook_naming_it() {
        let (tree, sound, _) = music_tree("audio_stem_length");
        fs::write(tree.content().join("fight.score.ron"), score_document(sound, 2)).unwrap();
        let error = anyhow_chain(&tree.cook().unwrap_err());
        assert!(error.contains("fight.score.ron") && error.contains("stem `click`"), "{error}");
    }

    #[test]
    fn a_line_whose_cue_isnt_on_the_voice_bus_fails_the_cook_naming_it() {
        let (tree, sound, _) = music_tree("audio_line_bus");
        fs::write(tree.content().join("line.cue.ron"), cue_document(&[sound])).unwrap();
        let error = anyhow_chain(&tree.cook().unwrap_err());
        assert!(
            error.contains("ja.voice.ron") && error.contains("`perfect-dodge`") && error.contains("Ui"),
            "{error}"
        );
    }

    #[test]
    fn a_score_or_lines_naming_the_wrong_kind_fail_the_cook_by_name() {
        let (tree, sound, cue) = music_tree("audio_wrong_kind");
        fs::write(tree.content().join("fight.score.ron"), score_document(cue, 1)).unwrap();
        assert!(matches!(tree.cook(), Err(CookError::NotAStem { .. })));
        fs::write(tree.content().join("fight.score.ron"), score_document(sound, 1)).unwrap();
        fs::write(tree.content().join("ja.voice.ron"), voice_document("perfect-dodge", sound)).unwrap();
        assert!(matches!(tree.cook(), Err(CookError::NotALine { .. })));
    }

    /// An error and what caused it, as a cook prints them.
    fn anyhow_chain(error: &dyn std::error::Error) -> String {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(next) = source {
            text.push_str(": ");
            text.push_str(&next.to_string());
            source = next.source();
        }
        text
    }

    #[test]
    fn a_file_that_is_not_vorbis_fails_the_cook_by_name() {
        let tree = Tree::new("audio_broken");
        fs::write(tree.content().join("broken.ogg"), b"not an ogg file").unwrap();
        let error = tree.cook().unwrap_err().to_string();
        assert!(error.contains("broken.ogg"), "{error}");
    }

    #[test]
    fn desktop_and_mobile_cook_the_same_bytes() {
        let tree = Tree::new("audio_cooks");
        let cooked = |cook: Cook| {
            let options = CookOptions { cook, cache: None, locked: false, editor: false };
            let out = tree.dir.path().join(cook.name());
            let report = cook_dir(&tree.content(), &out, &options).unwrap();
            let by_id: HashMap<AssetId, Vec<u8>> =
                report.assets.iter().map(|asset| (asset.id, fs::read(&asset.output).unwrap())).collect();
            by_id
        };
        assert_eq!(cooked(Cook::Desktop), cooked(Cook::Mobile));
    }

    #[test]
    fn the_importers_name_holds_the_decoder_in_cargo_lock() {
        let lock = include_str!("../../../Cargo.lock");
        let version = lock
            .split("[[package]]")
            .find_map(|package| {
                let mut lines = package.trim().lines();
                (lines.next()? == "name = \"lewton\"").then(|| {
                    lines.next().unwrap().trim_start_matches("version = \"").trim_end_matches('"').to_owned()
                })
            })
            .expect("lewton in Cargo.lock");
        assert_eq!(SOUND_IMPORTER.name, format!("ogg (lewton {version})"));
    }

    /// The whole point of the plain-data formats: no mixer in the cooker's tree. Cargo.lock lists
    /// every dependency of every platform and dev-dependencies too, so this errs on the strict side.
    #[test]
    fn the_cooker_links_no_mixer() {
        let lock = include_str!("../../../Cargo.lock");
        let mut dependencies: HashMap<&str, Vec<&str>> = HashMap::new();
        for package in lock.split("[[package]]").skip(1) {
            let name = package
                .lines()
                .find_map(|line| line.strip_prefix("name = \""))
                .map(|name| name.trim_end_matches('"'))
                .expect("every package has a name");
            let listed = package.split("dependencies = [").nth(1).and_then(|rest| rest.split(']').next());
            let names = listed
                .into_iter()
                .flat_map(|list| list.lines())
                .filter_map(|line| line.trim().trim_matches(|c| c == '"' || c == ',').split(' ').next())
                .filter(|name| !name.is_empty())
                .collect();
            dependencies.entry(name).or_default().extend::<Vec<&str>>(names);
        }
        let mut seen = BTreeSet::new();
        let mut to_visit = vec!["sapphire_pipeline"];
        while let Some(name) = to_visit.pop() {
            if seen.insert(name) {
                to_visit.extend(dependencies.get(name).into_iter().flatten().copied());
            }
        }
        assert!(seen.contains("lewton"), "the walk reaches the cooker's dependencies");
        for mixer in ["kira", "cpal", "sapphire_audio", "symphonia", "sapphire_render", "sapphire_world"] {
            assert!(!seen.contains(mixer), "the cooker reaches {mixer}");
        }
    }
}
