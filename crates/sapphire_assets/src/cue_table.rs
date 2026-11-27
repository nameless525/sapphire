//! The cue table as a document (M2 E3): what each of the simulation's cues fires, by name. It holds names and
//! IDs only: `sapphire_cues`' dispatcher resolves them against the libraries they name at load, and the shipped
//! table is `sapphire_cues::default_table`.
//!
//! Plain data here, below the renderer and the world, so the editor's documents reflect it without linking either
//! (M6 T19, as M6 T4 did the ramps): its stable names are still `sapphire::cues`, as every table's envelope says.

use std::collections::BTreeSet;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_moves::Weight;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::voice::is_line_key;

/// The most effects one row spawns.
pub const MAX_ROW_EFFECTS: usize = 3;
/// The most sounds one row plays.
pub const MAX_ROW_SOUNDS: usize = 3;
/// The strongest a row's shake may be, on top of what its preset authors: past this a typo, since
/// the camera holds the sum to its limits anyway.
const MAX_SHAKE_STRENGTH: f32 = 4.0;
/// A rumble's longest. A pad buzzing longer than this isn't a hit.
const MAX_RUMBLE_SECONDS: f32 = 2.0;

/// One rumble, as a table authors it: a flat pulse, a strength per motor and a length. `sapphire_cues`' mixer
/// plays the strongest of those overlapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(deny_unknown_fields)]
pub struct Rumble {
    /// The low-frequency motor, the heavy one: 0 to 1.
    pub low: f32,
    /// The high-frequency motor, the sharp one: 0 to 1.
    pub high: f32,
    /// How long, in real seconds.
    pub seconds: f32,
}

/// Where a row's effect plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[reflect(Serialize, Deserialize)]
pub enum EffectPlace {
    /// A hit's point, with the effect's +Y along its normal, out of the surface. A cue with no hit plays
    /// where its actor stands.
    #[default]
    Hit,
    /// Where the cue's subject stands, upright: a hit's target, or the actor whose cue it is.
    Feet,
    /// Over the subject's head, upright: a cue's mark (M4 A5), `sapphire_cues::CROWN_CLEARANCE`
    /// over its highest hurtbox.
    Crown,
    /// Where the subject stands, turned the way it faces, so the effect's +Z is its front: a swing's arc round
    /// its body (M5 L5).
    Facing,
    /// On the ground where the subject stands, its +Y the ground's normal there and its +Z the subject's front
    /// (M5 L7): a decal's place. On the subject's own clock (`clock: Actor` on its own cue), an effect here
    /// also learns when the subject's running move next strikes, which a telegraph's decal fills until.
    Ground,
}

/// Whose clock a row's effect runs on, so what can freeze it.
///
/// A frozen owner's effect emits nothing until it thaws, so an effect a hit starts belongs on
/// [`Self::World`], or a blow landing on someone already in hitstop shows late. A hit's own step isn't
/// frozen: its freeze covers the steps after.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[reflect(Serialize, Deserialize)]
pub enum EffectOwner {
    /// The world's: hitstop doesn't hold it, and slow motion does slow it.
    #[default]
    World,
    /// A hit's target's, or the cue's actor's.
    Subject,
    /// The cue's actor's: a hit's attacker.
    Actor,
}

/// Where a row's screen effect centers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[reflect(Serialize, Deserialize)]
pub enum ScreenCenter {
    /// A hit's point on screen. A cue with no hit, or a point behind the camera, keeps the preset's.
    #[default]
    Hit,
    /// Wherever the preset authors.
    Preset,
}

/// An effect a row spawns, from `sapphire_vfx`'s library. It has to end by itself: nothing keeps its
/// handle, so a trail or a rate with no duration is refused ([`CueTableError::EndlessEffect`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(deny_unknown_fields)]
pub struct EffectFire {
    pub effect: String,
    #[serde(default)]
    pub at: EffectPlace,
    #[serde(default)]
    pub clock: EffectOwner,
}

/// A camera shake a row fires, from the camera tuning's presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(deny_unknown_fields)]
pub struct ShakeFire {
    pub preset: String,
    /// On top of what the preset authors: 1 as authored.
    #[serde(default = "as_authored")]
    pub strength: f32,
}

fn as_authored() -> f32 {
    1.0
}

/// A screen effect a row pushes, from the world's library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(deny_unknown_fields)]
pub struct ScreenFire {
    pub preset: String,
    #[serde(default)]
    pub center: ScreenCenter,
}

/// What one cue fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(deny_unknown_fields)]
pub struct CueRow {
    /// The cue's name, as a move or `sapphire_gameplay`'s resolve publishes it.
    pub cue: String,
    /// Only for a hit of this weight. A row with a weight wins over one without.
    #[serde(default)]
    pub weight: Option<Weight>,
    #[serde(default)]
    pub effects: Vec<EffectFire>,
    /// `sapphire_audio` cues, by their documents' IDs: `Cue` documents in `audio.spack`.
    #[serde(default)]
    pub sounds: Vec<AssetId>,
    #[serde(default)]
    pub shake: Option<ShakeFire>,
    #[serde(default)]
    pub screen: Option<ScreenFire>,
    #[serde(default)]
    pub rumble: Option<Rumble>,
    /// A voice line to speak, by its key, in the voice language: one of the table's
    /// [`CueTable::voices`] (M3 A1).
    #[serde(default)]
    pub line: Option<String>,
    /// A cut-in to play (M5 L10), by its name in the game's `CutInLibrary` (`sapphire_world::cutin`): the
    /// game looks it up for the kind that plays it. The dispatcher only says it was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutin: Option<String>,
}

impl CueRow {
    /// The row's name in messages: its cue, and its weight if it has one.
    #[must_use]
    pub fn label(&self) -> String {
        match self.weight {
            Some(weight) => format!("{} ({weight:?})", self.cue),
            None => self.cue.clone(),
        }
    }

    fn validate(&self, index: usize) -> Result<(), CueTableError> {
        let invalid = |field: &'static str, problem: &'static str| CueTableError::Invalid {
            index,
            row: self.label(),
            field,
            problem,
        };
        if self.effects.len() > MAX_ROW_EFFECTS {
            return Err(invalid("effects", "has more than a row can spawn"));
        }
        if self.sounds.len() > MAX_ROW_SOUNDS {
            return Err(invalid("sounds", "has more than a row can play"));
        }
        if let Some(shake) = &self.shake
            && !(shake.strength > 0.0 && shake.strength <= MAX_SHAKE_STRENGTH)
        {
            return Err(invalid("shake.strength", "must be above 0 and at most 4"));
        }
        if let Some(line) = &self.line
            && !is_line_key(line)
        {
            return Err(invalid("line", "isn't a key: a letter, then letters, digits, `-` and `_`"));
        }
        if self.cutin.as_ref().is_some_and(String::is_empty) {
            return Err(invalid("cutin", "can't be empty"));
        }
        if let Some(rumble) = &self.rumble {
            if !(0.0..=1.0).contains(&rumble.low) {
                return Err(invalid("rumble.low", "must be from 0 to 1"));
            }
            if !(0.0..=1.0).contains(&rumble.high) {
                return Err(invalid("rumble.high", "must be from 0 to 1"));
            }
            if !(rumble.seconds > 0.0 && rumble.seconds <= MAX_RUMBLE_SECONDS) {
                return Err(invalid("rumble.seconds", "must be above 0 and at most 2"));
            }
        }
        Ok(())
    }
}

/// A table that isn't valid, or names what its libraries don't have.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CueTableError {
    #[error("row {0}: a cue's name can't be empty")]
    EmptyName(usize),
    #[error("cue `{0}` appears twice")]
    Duplicate(String),
    #[error("more rows than a table can hold")]
    TooMany,
    /// A field of the row at `index`, which the message leads with from the document's root (M6 T6).
    #[error("`rows[{index}].{field}`: cue `{row}`: `{field}` {problem}")]
    Invalid { index: usize, row: String, field: &'static str, problem: &'static str },
    #[error("cue `{row}`: no effect `{name}` in the effects library")]
    UnknownEffect { row: String, name: String },
    #[error("cue `{row}`: effect `{name}` plays until it's stopped, and a cue keeps nothing to stop it with")]
    EndlessEffect { row: String, name: String },
    #[error("cue `{row}`: no screen effect preset `{name}`")]
    UnknownPreset { row: String, name: String },
    #[error("cue `{row}`: no camera shake preset `{name}`")]
    UnknownShake { row: String, name: String },
    #[error("cue `{row}`: sound {sound} wasn't loaded when the app started, so it takes a restart")]
    UnloadedSound { row: String, sound: AssetId },
}

/// What each gameplay cue fires (a `CueTable` document, schema 1): one row per cue, or per cue and hit
/// weight. A cue with no row fires nothing and is counted (`sapphire_cues::CueStats::unmatched`); a row that
/// fires nothing says so on purpose. With the rows, the fight's score and its voice lines' tables
/// (M3 A1), which the rows' lines and the fight's phase play.
///
/// A table's rows are valid and distinct however it was made, deserialization included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::cues"]
#[serde(try_from = "UncheckedTable")]
pub struct CueTable {
    /// A `Score` document's ID: the stems the fight's phase plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    music: Option<AssetId>,
    /// `VoiceLines` documents' IDs, one a language: what the rows' lines say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    voices: Vec<AssetId>,
    /// A `Mix` document's ID (M6 U2): the buses, snapshots and ducking everything the fight plays goes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mix: Option<AssetId>,
    /// An `Effects` library's ID (M6 N1): what the rows' effects and the fight's sword trails play, loaded when a
    /// fight starts. None plays `sapphire_vfx`'s compiled-in library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effect_library: Option<AssetId>,
    rows: Vec<CueRow>,
}

impl Versioned for CueTable {
    const VERSION: u32 = 1;
}

/// A table as written, before [`CueTable::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedTable {
    #[serde(default)]
    music: Option<AssetId>,
    #[serde(default)]
    voices: Vec<AssetId>,
    #[serde(default)]
    mix: Option<AssetId>,
    #[serde(default)]
    effect_library: Option<AssetId>,
    rows: Vec<CueRow>,
}

impl TryFrom<UncheckedTable> for CueTable {
    type Error = CueTableError;

    fn try_from(table: UncheckedTable) -> Result<Self, CueTableError> {
        Ok(Self::new(table.rows)?
            .with_music(table.music, table.voices)
            .with_mix(table.mix)
            .with_effect_library(table.effect_library))
    }
}

impl CueTable {
    /// A table of `rows`, checked.
    pub fn new(rows: Vec<CueRow>) -> Result<Self, CueTableError> {
        if rows.len() > usize::from(u16::MAX) {
            return Err(CueTableError::TooMany);
        }
        let mut names = BTreeSet::new();
        for (index, row) in rows.iter().enumerate() {
            if row.cue.is_empty() {
                return Err(CueTableError::EmptyName(index));
            }
            if !names.insert((row.cue.as_str(), row.weight)) {
                return Err(CueTableError::Duplicate(row.label()));
            }
            row.validate(index)?;
        }
        Ok(Self { music: None, voices: Vec::new(), mix: None, effect_library: None, rows })
    }

    /// The table, with the score `music` plays and the voice lines' tables `voices`.
    #[must_use]
    pub fn with_music(self, music: Option<AssetId>, voices: Vec<AssetId>) -> Self {
        Self { music, voices, ..self }
    }

    /// The table, with the mix `mix` (M6 U2).
    #[must_use]
    pub fn with_mix(self, mix: Option<AssetId>) -> Self {
        Self { mix, ..self }
    }

    /// The `Mix` everything the fight plays goes through, if the table names one (M6 U2).
    #[must_use]
    pub fn mix(&self) -> Option<AssetId> {
        self.mix
    }

    /// The table, with the effects library `effect_library` (M6 N1).
    #[must_use]
    pub fn with_effect_library(self, effect_library: Option<AssetId>) -> Self {
        Self { effect_library, ..self }
    }

    /// The `Effects` library the rows' effects come from, if the table names one (M6 N1): a cooked library a
    /// fight loads when it starts. None is `sapphire_vfx`'s compiled-in one.
    #[must_use]
    pub fn effect_library(&self) -> Option<AssetId> {
        self.effect_library
    }

    /// The `Score` the fight's phase plays, if any.
    #[must_use]
    pub fn music(&self) -> Option<AssetId> {
        self.music
    }

    /// The `VoiceLines` the rows' lines are said from, one a language.
    #[must_use]
    pub fn voices(&self) -> &[AssetId] {
        &self.voices
    }

    /// Reads a table document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    #[must_use]
    pub fn rows(&self) -> &[CueRow] {
        &self.rows
    }

    /// An error naming the first row playing a `sapphire_audio` cue outside `loaded`, which a mixer would
    /// drop each time it fired. For whoever takes a new table into an app that has already loaded sounds.
    pub fn check_sounds(&self, loaded: &[AssetId]) -> Result<(), CueTableError> {
        for row in &self.rows {
            if let Some(&sound) = row.sounds.iter().find(|sound| !loaded.contains(sound)) {
                return Err(CueTableError::UnloadedSound { row: row.label(), sound });
            }
        }
        Ok(())
    }

    /// Every `sapphire_audio` cue the table plays, each once: what an app that plays sound loads and
    /// adds to its `AudioSystem`.
    #[must_use]
    pub fn sound_cues(&self) -> Vec<AssetId> {
        let mut cues: Vec<AssetId> = self.rows.iter().flat_map(|row| row.sounds.iter().copied()).collect();
        cues.sort_unstable();
        cues.dedup();
        cues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cue: &str) -> CueRow {
        CueRow {
            cue: cue.to_owned(),
            weight: None,
            effects: Vec::new(),
            sounds: Vec::new(),
            shake: None,
            screen: None,
            rumble: None,
            line: None,
            cutin: None,
        }
    }

    #[test]
    fn duplicate_rows_keep_validation_precedence() {
        let mut invalid = row("same");
        invalid.sounds = vec![AssetId::from_text("11111111-2222-4333-8444-555555555555"); MAX_ROW_SOUNDS + 1];
        assert!(matches!(CueTable::new(vec![row("same"), invalid.clone()]),
            Err(CueTableError::Duplicate(name)) if name == "same"));
        assert!(matches!(
            CueTable::new(vec![invalid, row("same")]),
            Err(CueTableError::Invalid { field: "sounds", .. })
        ));
    }

    #[test]
    fn music_and_voices_read_and_write_back() {
        let id = |n: u8| format!("11111111-2222-4333-8444-5555555555{n:02x}");
        let text = format!(
            "(type: \"sapphire::cues::CueTable\", version: 1, data: (music: Some(\"{}\"), voices: [\"{}\"], mix: Some(\"{}\"), effect_library: Some(\"{}\"), rows: []))",
            id(1),
            id(2),
            id(3),
            id(4)
        );
        let table = CueTable::from_ron(&text).expect("reads");
        assert_eq!(table.music(), Some(id(1).parse().unwrap()));
        assert_eq!(table.mix(), Some(id(3).parse().unwrap()));
        assert_eq!(table.effect_library(), Some(id(4).parse().unwrap()));
        assert_eq!(table.voices(), [id(2).parse::<AssetId>().unwrap()]);
        assert_eq!(CueTable::from_ron(&table.to_ron().expect("writes")).expect("reads back"), table);
    }

    #[test]
    fn sound_cues_lists_each_once() {
        let id = AssetId::from_text("11111111-2222-4333-8444-555555555555");
        let mut first = row("a");
        first.sounds = vec![id];
        let mut second = row("b");
        second.sounds = vec![id];
        assert_eq!(CueTable::new(vec![first, second]).expect("valid").sound_cues(), [id]);
    }

    #[test]
    fn check_sounds_names_the_row_playing_what_is_not_loaded() {
        let loaded = AssetId::from_text("11111111-2222-4333-8444-555555555555");
        let other = AssetId::from_text("99999999-2222-4333-8444-555555555555");
        let plain = CueRow { sounds: vec![loaded], ..row("a") };
        let heavy = CueRow { weight: Some(Weight::Heavy), sounds: vec![loaded, other], ..row("b") };
        assert_eq!(CueTable::new(vec![plain.clone()]).expect("valid").check_sounds(&[loaded]), Ok(()));
        assert_eq!(CueTable::new(vec![row("a")]).expect("valid").check_sounds(&[]), Ok(()));
        assert_eq!(
            CueTable::new(vec![plain, heavy]).expect("valid").check_sounds(&[loaded]),
            Err(CueTableError::UnloadedSound { row: "b (Heavy)".to_owned(), sound: other })
        );
    }

    #[test]
    fn a_cue_may_have_a_row_per_weight_but_not_two_of_one() {
        let weighted = |weight| CueRow { weight: Some(weight), ..row("hit") };
        assert!(CueTable::new(vec![row("hit"), weighted(Weight::Light), weighted(Weight::Heavy)]).is_ok());
        assert_eq!(
            CueTable::new(vec![weighted(Weight::Light), weighted(Weight::Light)]),
            Err(CueTableError::Duplicate("hit (Light)".to_owned()))
        );
        assert_eq!(
            CueTable::new(vec![row("hit"), row("hit")]),
            Err(CueTableError::Duplicate("hit".to_owned()))
        );
        assert_eq!(CueTable::new(vec![row("")]), Err(CueTableError::EmptyName(0)));
    }

    /// A mistake names the row and the field.
    #[test]
    fn validation_names_the_field() {
        let field_of = |row: CueRow| match CueTable::new(vec![row]) {
            Err(CueTableError::Invalid { field, .. }) => field,
            other => panic!("{other:?}"),
        };
        let rumble = |low, high, seconds| CueRow { rumble: Some(Rumble { low, high, seconds }), ..row("a") };
        assert_eq!(field_of(rumble(1.5, 0.0, 0.1)), "rumble.low");
        assert_eq!(field_of(rumble(f32::NAN, 0.0, 0.1)), "rumble.low");
        assert_eq!(field_of(rumble(0.0, -0.1, 0.1)), "rumble.high");
        assert_eq!(field_of(rumble(0.0, 0.0, 0.0)), "rumble.seconds");
        assert_eq!(field_of(rumble(0.0, 0.0, 3.0)), "rumble.seconds");
        let shake =
            |strength| CueRow { shake: Some(ShakeFire { preset: "a".to_owned(), strength }), ..row("a") };
        assert_eq!(field_of(shake(0.0)), "shake.strength");
        assert_eq!(field_of(shake(f32::INFINITY)), "shake.strength");
        let spark = EffectFire { effect: "a".to_owned(), at: EffectPlace::Hit, clock: EffectOwner::World };
        let effects = CueRow { effects: vec![spark; MAX_ROW_EFFECTS + 1], ..row("a") };
        assert_eq!(field_of(effects), "effects");
        let id = AssetId::from_text("11111111-2222-4333-8444-555555555555");
        assert_eq!(field_of(CueRow { sounds: vec![id; MAX_ROW_SOUNDS + 1], ..row("a") }), "sounds");
        assert_eq!(field_of(CueRow { line: Some("perfect dodge".to_owned()), ..row("a") }), "line");

        let text =
            |data: &str| format!(r#"(type: "sapphire::cues::CueTable", version: 1, data: (rows: [{data}]))"#);
        let error = CueTable::from_ron(&text(r#"(cue: "a", rumbel: None)"#)).expect_err("a misspelling");
        assert!(error.to_string().contains("rumbel"), "{error}");
        let error =
            CueTable::from_ron(&text(r#"(cue: "a", rumble: Some((low: 2.0, high: 0.0, seconds: 0.1)))"#))
                .expect_err("out of range");
        assert!(error.to_string().contains("rumble.low"), "{error}");
    }
}
