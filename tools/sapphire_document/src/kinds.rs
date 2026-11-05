//! The documents an editor knows by kind: how each reads, and what rules it has beyond reading.
//!
//! There are two kinds of "no" here, and a change record meets them differently. A parse that fails refuses
//! the edit, because a file that won't read is no document: [`EffectLibrary`] checks everything as it
//! reads, so all its rules are of this kind. [`Schema::check`] only reports, because a person is allowed to
//! drag a window somewhere wrong on the way to somewhere right, and `MoveSetDocument::check` is the
//! timeline's red marks.

use std::path::Path;
use std::sync::Arc;

use bevy_reflect::{GetTypeRegistration, TypeRegistry};
use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::graph::AnimGraph;
use sapphire_moves::MoveSetDocument;
use sapphire_timeline::TimelineDocument;
use sapphire_vfx::EffectLibrary;

use crate::set::{DocumentSet, SetError};

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

/// An effect library, such as `default_effects.ron`, which is compiled in and no cook reads.
#[must_use]
pub fn effects() -> Schema<EffectLibrary> {
    Schema {
        parse: |text| EffectLibrary::from_ron(text).map_err(|error| error.to_string()),
        check: no_rules,
        importer: None,
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
}

impl Kind {
    pub const ALL: [Self; 5] = [Self::Moves, Self::Graph, Self::Cue, Self::Effects, Self::Timeline];

    /// What a file's name says it is, if it's a document this crate knows. A library of effects has no suffix
    /// of its own beyond its name, as `default_effects.ron` shows.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        Self::ALL.into_iter().find(|kind| name.ends_with(kind.suffix()))
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Moves => ".moves.ron",
            Self::Graph => ".animgraph.ron",
            Self::Cue => ".cue.ron",
            Self::Effects => "_effects.ron",
            Self::Timeline => ".timeline.ron",
        }
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
        }
    }
}

fn read_document<T>(text: &str) -> Result<T, String>
where
    T: sapphire_reflect::Versioned + serde::de::DeserializeOwned + bevy_reflect::TypePath,
{
    sapphire_serialize::from_ron(text).map_err(|error| error.to_string())
}
