//! A cook's refusal, for a tool that ran the cook as a process to read (M6 T5): the source it refused, the field of
//! that source's document where the refusal knows one, and why. `cook --refusal <file>` writes one when a cook
//! fails and takes an old one away when a cook succeeds, so the file is there exactly when the last cook failed.
//!
//! The editor reads it after a cook it started from a save fails, and shows it by document and field. It's a
//! document like any (`sapphire::pipeline::CookRefusal`, version 1), written whole through a temporary and a rename,
//! so a reader never sees half of one. The editor can't link a tool, so it reads it by a type of its own under the
//! same name.

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_document::{Kind, problem};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use serde::{Deserialize, Serialize};

use crate::cook::CookError;

/// Why a cook stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[type_name = "CookRefusal"]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    /// The source the cook refused, as the cook named it (under its `--source`), if the refusal is one source's.
    pub source: Option<PathBuf>,
    /// The field of that source's document the refusal is about, as a property path (`base_color_map`,
    /// `regions[1].roughness`), where it names one.
    pub field: Option<String>,
    /// What went wrong, then each thing that caused it, a line each.
    pub message: String,
}

impl Versioned for Refusal {
    /// 1: M6 T5.
    const VERSION: u32 = 1;

    fn upgrade<D: Decoder>(version: u32, _data: D) -> Result<Self, VersionError<D::Error>> {
        Err(VersionError::TooOld { found: version, oldest: Self::VERSION })
    }
}

impl Refusal {
    /// The refusal a cook's error amounts to.
    #[must_use]
    pub fn of_cook(error: &CookError) -> Self {
        Self { source: source_of(error).map(Path::to_owned), field: field_of(error), message: chain(error) }
    }

    /// A refusal that names no source: what went wrong after the assets cooked (the audit, the index, the packs).
    #[must_use]
    pub fn of_other(error: &(dyn Error + 'static)) -> Self {
        Self { source: None, field: None, message: chain(error) }
    }

    /// Writes it to `path` whole, through a temporary beside it and a rename.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let text = sapphire_serialize::to_ron(self).map_err(io::Error::other)?;
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path)
    }

    /// Takes away a refusal an earlier cook left at `path`, if there's one.
    pub fn clear(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// An error and what caused it, a line each.
pub(crate) fn chain(error: &(dyn Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        message.push('\n');
        message.push_str(&error.to_string());
        cause = error.source();
    }
    message
}

/// The source a cook's error is about, if it's one source's.
fn source_of(error: &CookError) -> Option<&Path> {
    use CookError as E;
    match error {
        E::NonFiniteUv { path, .. }
        | E::UnsupportedSource { path, .. }
        | E::SourceChanged { path, .. }
        | E::Import { path, .. }
        | E::Fbx { path, .. }
        | E::Assembly { path, .. }
        | E::Vrm { path, .. }
        | E::Clip { path, .. }
        | E::InvalidRig { path, .. }
        | E::Skeleton { path, .. }
        | E::Texture { path, .. }
        | E::Map { path, .. }
        | E::Procedural { path, .. }
        | E::Invalid { path, .. }
        | E::Merge { path, .. }
        | E::Material { path, .. }
        | E::Foliage { path, .. }
        | E::ProbeGrid { path, .. }
        | E::Reflections { path, .. }
        | E::Colliders { path, .. }
        | E::Encounter { path, .. }
        | E::Enemy { path, .. }
        | E::EnemyReference { path, .. }
        | E::Playable { path, .. }
        | E::PlayableReference { path, .. }
        | E::HeldJointCut { path, .. }
        | E::Motion { path, .. }
        | E::Effects { path, .. }
        | E::EffectSheet { path, .. }
        | E::NotASheet { path, .. }
        | E::FoliageDocument { path, .. }
        | E::FoliageReference { path, .. }
        | E::Locale { path, .. }
        | E::Timeline { path, .. }
        | E::TimelineKey { path, .. }
        | E::Dialogue { path, .. }
        | E::Ui { path, .. }
        | E::Audio { path, .. }
        | E::NotASound { path, .. }
        | E::NotAStem { path, .. }
        | E::NotALine { path, .. }
        | E::LineWithoutMouth { path, .. }
        | E::Zone { path, .. }
        | E::ZoneReference { path, .. }
        | E::Font { path, .. }
        | E::NotAFont { path, .. }
        | E::MoveSet { path, .. }
        | E::Script { path, .. }
        | E::AnimGraph { path, .. }
        | E::NotAClip { path, .. }
        | E::MaterialSlot { path, .. }
        | E::NotAMaterial { path, .. }
        | E::Panicked { path, .. }
        | E::Read { path, .. }
        | E::Write { path, .. } => Some(path),
        E::Meta(_)
        | E::Localization(_)
        | E::Registry(_)
        | E::DialogueKey(_)
        | E::Screen(_)
        | E::OutputCollision { .. }
        | E::CaseCollision { .. }
        | E::Cache(_) => None,
    }
}

/// The field of the source's document a cook's error is about, where it knows one (M6 T6): a material's texture
/// binding or a reference to another asset by its error's own field, else the first error of the chain that names one by the documents' convention
/// (`` `placements[2].scale`: … ``, [`problem::named_field`]), else, if the source's own text doesn't read as its
/// kind, where its parser says it stopped ([`problem::locate`]). A RON position in the chain itself is never
/// read: it may be another document's, one the source names (an assembly's part, an effect library's sheet). So
/// a new refusal names its field by its message alone, as the zone's, the effect sheets' and the foliage's do;
/// one that doesn't is the whole document's.
fn field_of(error: &CookError) -> Option<String> {
    match error {
        CookError::Material { source, .. } if let Some(field) = source.field() => {
            return Some(field.to_owned());
        }
        CookError::EnemyReference { field, .. } | CookError::PlayableReference { field, .. } => {
            return Some((*field).to_owned());
        }
        CookError::FoliageReference { field, .. } | CookError::ZoneReference { field, .. } => {
            return Some(field.clone());
        }
        _ => {}
    }
    let mut cause: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(error) = cause {
        if let Some(field) = problem::named_field(&error.to_string()) {
            return Some(field.to_owned());
        }
        cause = error.source();
    }
    let path = source_of(error)?;
    let text = fs::read_to_string(path).ok()?;
    let failure = Kind::of(path)?.read(&text).err()?;
    problem::locate(&failure, Some(&text)).field().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use sapphire_assets::AssetId;

    use super::*;
    use crate::material::MaterialCookError;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../sapphire_document/test_content/problems").join(name)
    }

    /// The errors that carry their field as data name it; a cause that leads with a path names that; with
    /// neither, a source whose own text doesn't read is on the field its parser stopped at.
    #[test]
    fn a_refusal_names_its_field_by_its_data_its_chain_or_its_own_parse() {
        let id = AssetId::from_bytes([3; 16]);
        let path = PathBuf::from("content/zones/a.zone.ron");
        let zone = CookError::ZoneReference {
            path: path.clone(),
            field: "kinds[1].prefab".into(),
            what: "a prefab",
            id,
        };
        assert_eq!(Refusal::of_cook(&zone).field.as_deref(), Some("kinds[1].prefab"));
        let foliage = CookError::FoliageReference { path: path.clone(), field: "rules[0].model".into(), id };
        assert_eq!(Refusal::of_cook(&foliage).field.as_deref(), Some("rules[0].model"));
        let enemy = CookError::EnemyReference { path: path.clone(), field: "moves", what: "a move set", id };
        assert_eq!(Refusal::of_cook(&enemy).field.as_deref(), Some("moves"));
        let playable =
            CookError::PlayableReference { path: path.clone(), field: "brain", what: "a brain", id };
        assert_eq!(Refusal::of_cook(&playable).field.as_deref(), Some("brain"));

        let chain = CookError::Read {
            path: path.clone(),
            source: io::Error::other("`placements[0].scale`: is zero"),
        };
        assert_eq!(Refusal::of_cook(&chain).field.as_deref(), Some("placements[0].scale"));

        // M6 T16a: a subtitle no table holds is its timeline's, on its field.
        let timeline = PathBuf::from("content/zones/a/gate.timeline.ron");
        let missing = crate::timeline::MissingString {
            field: "subtitles[1].key".into(),
            key: "unwritten".into(),
            locale: "en".into(),
        };
        let subtitle = CookError::TimelineKey { path: timeline.clone(), source: Box::new(missing) };
        let refusal = Refusal::of_cook(&subtitle);
        assert_eq!(
            (refusal.source.as_deref(), refusal.field.as_deref()),
            (Some(&*timeline), Some("subtitles[1].key"))
        );

        let broken = fixture("unknown_field.zone.ron");
        let own = CookError::Read { path: broken, source: io::Error::other("it stopped") };
        assert_eq!(Refusal::of_cook(&own).field.as_deref(), Some("placements[0].colour"), "its own parse");
    }

    /// A RON position in a refusal of a source that reads is another document's (one it names, whose text
    /// failed), so it lands on no field of the source's.
    #[test]
    fn a_nested_documents_position_is_not_the_sources() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/zones/square/square.zone.ron");
        let source = source.canonicalize().expect("a shipped zone");
        let nested = CookError::Read {
            path: source,
            source: io::Error::other("its prefab doesn't read: 7:12: expected a float"),
        };
        let refusal = Refusal::of_cook(&nested);
        assert_eq!(refusal.field, None, "{}", refusal.message);
    }

    #[test]
    fn a_material_refusal_names_its_source_and_field_and_reads_back() {
        let id = AssetId::from_bytes([7; 16]);
        let error = CookError::Material {
            path: PathBuf::from("content/samples/broken.material.ron"),
            source: Box::new(MaterialCookError::MissingTexture {
                slot: "base_color",
                field: "base_color_map",
                id,
            }),
        };
        let refusal = Refusal::of_cook(&error);
        assert_eq!(refusal.source.as_deref(), Some(Path::new("content/samples/broken.material.ron")));
        assert_eq!(refusal.field.as_deref(), Some("base_color_map"));
        let lines: Vec<&str> = refusal.message.lines().collect();
        assert_eq!(lines.len(), 2, "the error, then its cause: {}", refusal.message);
        assert!(lines[1].contains("texture slot `base_color`"), "{}", refusal.message);

        let dir = std::env::temp_dir().join(format!("sapphire_pipeline_refusal_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("editor.refusal.ron");
        refusal.write(&file).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("sapphire::pipeline::CookRefusal"), "{text}");
        let read: Refusal = sapphire_serialize::from_ron(&text).unwrap();
        assert_eq!(read, refusal);
        Refusal::clear(&file).unwrap();
        assert!(!file.exists());
        Refusal::clear(&file).expect("nothing to take away is no error");
        fs::remove_dir_all(dir).unwrap();
    }
}
