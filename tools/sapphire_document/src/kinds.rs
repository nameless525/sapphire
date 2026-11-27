//! The documents an editor knows by kind: how each reads, and what rules it has beyond reading.
//!
//! There are two kinds of "no" here, and a change record meets them differently. A parse that fails refuses
//! the edit, because a file that won't read is no document: [`EffectLibrary`] checks everything as it
//! reads, so all its rules are of this kind. [`Schema::check`] only reports, because a person is allowed to
//! drag a window somewhere wrong on the way to somewhere right, and `MoveSetDocument::check` is the
//! timeline's red marks.
//!
//! A kind is known by its file's name, but for materials: every family is a `.material.ron`, and its envelope
//! says which, so [`Kind::of`] reads that much of the file. The families with a kind here are the anime
//! character's and the environment's; foliage, sky and fog card materials open as no kind yet.
//!
//! Some kinds' rules need more than the document, and so are a cook's, not a schema's: an assembly's
//! pieces and shapes (the pipeline's geometry), a dialogue's flags (the game's registry), a ramp's images.

use std::path::Path;
use std::sync::Arc;

use bevy_reflect::{GetTypeRegistration, TypeRegistry};
use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::graph::AnimGraph;
use sapphire_assets::material::{AnimeCharacterMaterial, EnvironmentMaterial, MaterialDocument};
use sapphire_assets::ramp::RampMap;
use sapphire_assets::ui::UiDocument;
use sapphire_colliders::ColliderDocument;
use sapphire_dialogue::DialogueDocument;
use sapphire_moves::MoveSetDocument;
use sapphire_reflect::stable_name;
use sapphire_timeline::TimelineDocument;
use sapphire_tod::TodTimeline;
use sapphire_tod::light_rig::LightRigLibrary;
use sapphire_vfx::EffectLibrary;
use sapphire_zone::encounter::EncounterDocument;
use sapphire_zone::prefab::Assembly;
use sapphire_zone::{FoliageDocument, ZoneDocument};

use crate::journal::JournalEntry;
use crate::set::{DocumentSet, Noticed, SetError};

/// How a type of document reads, and what's wrong with one that does.
pub struct Schema<T> {
    pub parse: fn(&str) -> Result<T, String>,
    pub check: fn(&T) -> Vec<String>,
    /// The importer a cook gives a new file of this kind, as its sidecar writes it. `None` for a document no
    /// cook reads: see [`crate::identity`].
    pub importer: Option<&'static str>,
}

impl<T> Clone for Schema<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Schema<T> {}

fn no_rules<T>(_: &T) -> Vec<String> {
    Vec::new()
}

/// A registry that holds `T` and all it's made of.
#[must_use]
pub fn registry_of<T: GetTypeRegistration>() -> TypeRegistry {
    let mut registry = TypeRegistry::new();
    registry.register::<T>();
    registry
}

/// A `.moves.ron`.
#[must_use]
pub fn moves() -> Schema<MoveSetDocument> {
    Schema {
        parse: |text| MoveSetDocument::parse(text).map_err(|error| error.to_string()),
        check: |document| document.check().iter().map(ToString::to_string).collect(),
        importer: Some("MoveSet(())"),
    }
}

/// An `.animgraph.ron`.
#[must_use]
pub fn graph() -> Schema<AnimGraph> {
    Schema {
        parse: read_document,
        check: |document| document.check().iter().map(ToString::to_string).collect(),
        importer: Some("AnimGraph(())"),
    }
}

/// A `.cue.ron`.
#[must_use]
pub fn cue() -> Schema<Cue> {
    Schema {
        parse: read_document,
        check: |document| document.check().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some("Cue(())"),
    }
}

/// A `.timeline.ron`.
#[must_use]
pub fn timeline() -> Schema<TimelineDocument> {
    Schema {
        parse: read_document,
        check: |document| document.check().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some("Timeline(())"),
    }
}

/// A `.foliage.ron` (M6 G1a): a zone's plants' kinds and the rules that place them. Whether a kind's mesh is a
/// static mesh is a cook's to check.
#[must_use]
pub fn foliage() -> Schema<FoliageDocument> {
    Schema {
        parse: read_document,
        check: |document| document.check().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some("Foliage(())"),
    }
}

/// A `.zone.ron`. Its checks are its parse's: a zone reads only if it's right.
#[must_use]
pub fn zone() -> Schema<ZoneDocument> {
    Schema {
        parse: |text| ZoneDocument::from_ron(text).map_err(|error| error.to_string()),
        check: no_rules,
        importer: Some(ZONE_IMPORTER),
    }
}

/// The importer a cook writes in a new zone's sidecar: a zone cooks meshes, so it has their settings, laid out at
/// the depth the sidecar puts them. `tools/sapphire_pipeline/tests/document_sidecars.rs` holds it to the cooker's.
const ZONE_IMPORTER: &str = "Zone((
            outline: (
                weld_tolerance: 0.0001,
            ),
            lods: (
                levels: [],
                mobile_skip: 0,
            ),
            position_format: Float32,
        ))";

/// An effect library, such as `default_effects.ron`, which is compiled in and no cook reads.
#[must_use]
pub fn effects() -> Schema<EffectLibrary> {
    Schema {
        parse: |text| EffectLibrary::from_ron(text).map_err(|error| error.to_string()),
        check: no_rules,
        importer: None,
    }
}

/// A screen, `.ui.ron`. What its paths read is its model's to say, and a cook's to check, so here only
/// its own rules run.
#[must_use]
pub fn ui() -> Schema<UiDocument> {
    Schema {
        parse: read_document,
        check: |document| document.issues().iter().map(ToString::to_string).collect(),
        importer: Some("Ui(())"),
    }
}

/// An anime character's `.material.ron`.
#[must_use]
pub fn material() -> Schema<AnimeCharacterMaterial> {
    Schema { parse: read_document, check: material_rules, importer: Some(MATERIAL_IMPORTER) }
}

/// An environment's `.material.ron`.
#[must_use]
pub fn environment_material() -> Schema<EnvironmentMaterial> {
    Schema { parse: read_document, check: material_rules, importer: Some(MATERIAL_IMPORTER) }
}

const MATERIAL_IMPORTER: &str = "Material(())";

fn material_rules<T: MaterialDocument>(document: &T) -> Vec<String> {
    document.validate().err().map(|error| error.to_string()).into_iter().collect()
}

/// A `.ramp.ron`. Its images are a cook's to read, so a row past an image's end is a cook's to find.
#[must_use]
pub fn ramp() -> Schema<RampMap> {
    Schema {
        parse: read_document,
        check: |document| document.validate().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some(RAMP_IMPORTER),
    }
}

/// The importer a cook writes in a new ramp's sidecar, at the depth the sidecar puts it.
const RAMP_IMPORTER: &str = "CharacterMap(Ramp((
            width: 256,
            row_height: 4,
            mips: false,
        )))";

/// An `.assembly.ron`, a zone's prefab. What it places is checked by a cook, which has its kits and the
/// shapes' geometry.
#[must_use]
pub fn assembly() -> Schema<Assembly> {
    Schema { parse: read_document, check: no_rules, importer: Some(ASSEMBLY_IMPORTER) }
}

/// The importer a cook writes in a new assembly's sidecar: an assembly cooks a mesh, so it has a mesh's
/// settings, laid out at the depth the sidecar puts them.
const ASSEMBLY_IMPORTER: &str = "Assembly((
            outline: (
                weld_tolerance: 0.0001,
            ),
            lods: (
                levels: [
                    (
                        triangle_ratio: 0.5,
                        max_error: 0.002,
                    ),
                    (
                        triangle_ratio: 0.25,
                        max_error: 0.01,
                    ),
                    (
                        triangle_ratio: 0.125,
                        max_error: 0.03,
                    ),
                ],
                mobile_skip: 1,
            ),
            position_format: Unorm16,
            materials: {},
        ))";

/// An `.encounter.ron`. Its spawners and trigger are its zone's to check.
#[must_use]
pub fn encounter() -> Schema<EncounterDocument> {
    Schema {
        parse: read_document,
        check: |document| document.check().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some("Encounter(())"),
    }
}

/// A `.dialogue.ron`. Its rules need the game's flags, so they're a cook's.
#[must_use]
pub fn dialogue() -> Schema<DialogueDocument> {
    Schema { parse: read_document, check: no_rules, importer: Some("Dialogue(())") }
}

/// A day of time of day's keys, such as `lookdev_day.ron`. It's checked as it reads, and no cook reads it
/// alone: the zone that names it, or the build that compiles it in, does.
#[must_use]
pub fn day() -> Schema<TodTimeline> {
    Schema {
        parse: |text| TodTimeline::from_ron(text).map_err(|error| error.to_string()),
        check: no_rules,
        importer: None,
    }
}

/// A library of character light rigs, such as `lookdev_light_rigs.ron`. It's checked as it reads, and no
/// cook reads it alone.
#[must_use]
pub fn light_rigs() -> Schema<LightRigLibrary> {
    Schema {
        parse: |text| LightRigLibrary::from_ron(text).map_err(|error| error.to_string()),
        check: no_rules,
        importer: None,
    }
}

/// A zone's `.colliders.ron`. Its rules are its cook's, which a check runs whole: what it refuses is shown, by
/// the collider it names. What a placement draws is held to the colliders it names by the zone's cook (M4 X1).
#[must_use]
pub fn colliders() -> Schema<ColliderDocument> {
    Schema {
        parse: |text| ColliderDocument::from_ron(text).map_err(|error| error.to_string()),
        check: |document| document.cook().err().map(|error| error.to_string()).into_iter().collect(),
        importer: Some("Colliders(())"),
    }
}

/// The kinds of document there are, which a browser lists and a file's name says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Moves,
    Graph,
    Cue,
    Effects,
    Timeline,
    Ui,
    Zone,
    Material,
    EnvironmentMaterial,
    Ramp,
    Assembly,
    Encounter,
    Dialogue,
    Day,
    LightRigs,
    Colliders,
    Foliage,
}

/// What every material family's file name ends with.
const MATERIAL_SUFFIX: &str = ".material.ron";

impl Kind {
    pub const ALL: [Self; 17] = [
        Self::Moves,
        Self::Graph,
        Self::Cue,
        Self::Effects,
        Self::Timeline,
        Self::Ui,
        Self::Zone,
        Self::Material,
        Self::EnvironmentMaterial,
        Self::Ramp,
        Self::Assembly,
        Self::Encounter,
        Self::Dialogue,
        Self::Day,
        Self::LightRigs,
        Self::Colliders,
        Self::Foliage,
    ];

    /// What a file is, if it's a document this crate knows: by its name, and for a material by the family its
    /// envelope names, which this reads from the file ([`Self::of_material`]); a material that can't be read
    /// is no kind. A library of effects, a day and a library of light rigs have no suffix of their own beyond
    /// their names, as `default_effects.ron`, `lookdev_day.ron` and `lookdev_light_rigs.ron` show.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        Self::of_text(path, || std::fs::read_to_string(path).ok())
    }

    /// [`Self::of`], with a material's text from `text` rather than its file: an open document's, say, which
    /// reads nothing. `text` is called only for a material.
    pub fn of_text(path: &Path, text: impl FnOnce() -> Option<String>) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if name.ends_with(MATERIAL_SUFFIX) {
            return Self::of_material(&text()?);
        }
        Self::ALL.into_iter().find(|kind| kind.suffix().is_some_and(|suffix| name.ends_with(suffix)))
    }

    /// The kind of a material document's text, by the family its envelope names: `None` for a family with
    /// no kind here.
    #[must_use]
    pub fn of_material(text: &str) -> Option<Self> {
        let family = sapphire_serialize::document::type_name(text).ok()?;
        [
            (Self::Material, stable_name::<AnimeCharacterMaterial>()),
            (Self::EnvironmentMaterial, stable_name::<EnvironmentMaterial>()),
        ]
        .into_iter()
        .find_map(|(kind, name)| (family == name).then_some(kind))
    }

    /// What its files' names end with: `None` for the materials, whose envelope says which they are.
    const fn suffix(self) -> Option<&'static str> {
        Some(match self {
            Self::Moves => ".moves.ron",
            Self::Graph => ".animgraph.ron",
            Self::Cue => ".cue.ron",
            Self::Effects => "_effects.ron",
            Self::Timeline => ".timeline.ron",
            Self::Ui => ".ui.ron",
            Self::Zone => ".zone.ron",
            Self::Material | Self::EnvironmentMaterial => return None,
            Self::Ramp => ".ramp.ron",
            Self::Assembly => ".assembly.ron",
            Self::Encounter => ".encounter.ron",
            Self::Dialogue => ".dialogue.ron",
            Self::Day => "_day.ron",
            Self::LightRigs => "_rigs.ron",
            Self::Colliders => ".colliders.ron",
            Self::Foliage => ".foliage.ron",
        })
    }

    /// Its heading in a list.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Moves => "Moves",
            Self::Graph => "Animation graphs",
            Self::Cue => "Cues",
            Self::Effects => "Effects",
            Self::Timeline => "Timelines",
            Self::Ui => "Screens",
            Self::Zone => "Zones",
            Self::Material => "Character materials",
            Self::EnvironmentMaterial => "Environment materials",
            Self::Ramp => "Ramps",
            Self::Assembly => "Prefabs",
            Self::Encounter => "Encounters",
            Self::Dialogue => "Dialogues",
            Self::Day => "Days",
            Self::LightRigs => "Light rigs",
            Self::Colliders => "Colliders",
            Self::Foliage => "Foliage",
        }
    }

    /// Opens the document at `path` as this kind.
    pub fn open(self, set: &mut DocumentSet, path: &Path) -> Result<AssetId, SetError> {
        match self {
            Self::Moves => set.open(path, moves(), Arc::new(registry_of::<MoveSetDocument>())),
            Self::Graph => set.open(path, graph(), Arc::new(registry_of::<AnimGraph>())),
            Self::Cue => set.open(path, cue(), Arc::new(registry_of::<Cue>())),
            Self::Effects => set.open(path, effects(), Arc::new(registry_of::<EffectLibrary>())),
            Self::Timeline => set.open(path, timeline(), Arc::new(registry_of::<TimelineDocument>())),
            Self::Ui => set.open(path, ui(), Arc::new(registry_of::<UiDocument>())),
            Self::Zone => set.open(path, zone(), Arc::new(registry_of::<ZoneDocument>())),
            Self::Material => set.open(path, material(), Arc::new(registry_of::<AnimeCharacterMaterial>())),
            Self::EnvironmentMaterial => {
                set.open(path, environment_material(), Arc::new(registry_of::<EnvironmentMaterial>()))
            }
            Self::Ramp => set.open(path, ramp(), Arc::new(registry_of::<RampMap>())),
            Self::Assembly => set.open(path, assembly(), Arc::new(registry_of::<Assembly>())),
            Self::Encounter => set.open(path, encounter(), Arc::new(registry_of::<EncounterDocument>())),
            Self::Dialogue => set.open(path, dialogue(), Arc::new(registry_of::<DialogueDocument>())),
            Self::Day => set.open(path, day(), Arc::new(registry_of::<TodTimeline>())),
            Self::LightRigs => set.open(path, light_rigs(), Arc::new(registry_of::<LightRigLibrary>())),
            Self::Colliders => set.open(path, colliders(), Arc::new(registry_of::<ColliderDocument>())),
            Self::Foliage => set.open(path, foliage(), Arc::new(registry_of::<FoliageDocument>())),
        }
    }

    /// Opens a journal's entry as a document of this kind ([`DocumentSet::recover`]).
    pub fn recover(self, set: &mut DocumentSet, entry: &JournalEntry) -> Result<Vec<Noticed>, SetError> {
        match self {
            Self::Moves => set.recover(entry, moves(), Arc::new(registry_of::<MoveSetDocument>())),
            Self::Graph => set.recover(entry, graph(), Arc::new(registry_of::<AnimGraph>())),
            Self::Cue => set.recover(entry, cue(), Arc::new(registry_of::<Cue>())),
            Self::Effects => set.recover(entry, effects(), Arc::new(registry_of::<EffectLibrary>())),
            Self::Timeline => set.recover(entry, timeline(), Arc::new(registry_of::<TimelineDocument>())),
            Self::Ui => set.recover(entry, ui(), Arc::new(registry_of::<UiDocument>())),
            Self::Zone => set.recover(entry, zone(), Arc::new(registry_of::<ZoneDocument>())),
            Self::Material => {
                set.recover(entry, material(), Arc::new(registry_of::<AnimeCharacterMaterial>()))
            }
            Self::EnvironmentMaterial => {
                set.recover(entry, environment_material(), Arc::new(registry_of::<EnvironmentMaterial>()))
            }
            Self::Ramp => set.recover(entry, ramp(), Arc::new(registry_of::<RampMap>())),
            Self::Assembly => set.recover(entry, assembly(), Arc::new(registry_of::<Assembly>())),
            Self::Encounter => set.recover(entry, encounter(), Arc::new(registry_of::<EncounterDocument>())),
            Self::Dialogue => set.recover(entry, dialogue(), Arc::new(registry_of::<DialogueDocument>())),
            Self::Day => set.recover(entry, day(), Arc::new(registry_of::<TodTimeline>())),
            Self::LightRigs => set.recover(entry, light_rigs(), Arc::new(registry_of::<LightRigLibrary>())),
            Self::Colliders => set.recover(entry, colliders(), Arc::new(registry_of::<ColliderDocument>())),
            Self::Foliage => set.recover(entry, foliage(), Arc::new(registry_of::<FoliageDocument>())),
        }
    }
}

fn read_document<T>(text: &str) -> Result<T, String>
where
    T: sapphire_reflect::Versioned + serde::de::DeserializeOwned + bevy_reflect::TypePath,
{
    sapphire_serialize::from_ron(text).map_err(|error| error.to_string())
}
