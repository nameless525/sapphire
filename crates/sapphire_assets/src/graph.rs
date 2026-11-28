//! Animation graphs (`.animgraph.ron` documents, cooked to `.sgraph`; M2 N3, M6 A5, spec §5.13): what a
//! person writes to say how a character's clips play. `sapphire_body_pose` binds and plays them.
//!
//! The document is [`v2::AnimGraph`], a node graph: parameters, state machines, blend spaces, layers by
//! mask, events. The cook resolves each clip's ID ([`clip_id`]) and compiles it to a
//! [`GraphProgram`](v2::GraphProgram), flat tables with a static worst-case cost, and that program is what
//! an `.sgraph` holds and what a character binds.
//!
//! # Why it's a cooked kind
//!
//! A graph names clips, so the asset server can load them as its dependencies and hot reload remount a
//! new version the way it does every other asset, on the desktop and over live link. The kinds are
//! closed and checked here ([`AssetKind`](crate::AssetKind)), so the format lives here, as a cue's does.
//! A program keeps its states', machines', moves' and reactions' names, which a binding resolves for one
//! character, and which is what lets a new version of a graph keep a running state of the same name.
//!
//! What's wrong with a document is named all at once, by its path in the document
//! ([`v2::AnimGraph::check`]). What depends on a character (a clip for another skeleton, a bone it
//! lacks) is `sapphire_body_pose`'s to say when it binds.
//!
//! # Version 1
//!
//! M2's version, a locomotion machine with combat, additive and reaction layers, still reads: through
//! its own check, then converted to version 2 ([`v2::convert`]), so an old document isn't lost. Nothing
//! writes it. This module holds what both versions share: clips, masks, the blink and the limits.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::CookedError;
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::rig::HumanBone;

pub(crate) mod v1;
pub mod v2;

pub use v2::GraphIssue;

/// Conventional file extension for cooked graphs.
pub const GRAPH_EXTENSION: &str = "sgraph";

/// The most clips one graph may name: a character's frame finds them by index, and a frame's sampling
/// is what the budget pays for.
pub const MAX_GRAPH_CLIPS: usize = 64;
/// The most samples in one blend space. Its weights cost the square of this every frame.
pub const MAX_BLEND_SAMPLES: usize = 16;
/// The most moves a combat layer's table names.
pub const MAX_GRAPH_MOVES: usize = 256;
/// The most additive layers, each of which samples a clip every frame.
pub const MAX_ADDITIVE_LAYERS: usize = 4;
/// The most reactions a graph knows.
pub const MAX_REACTIONS: usize = 16;
/// The most masks, and the most bones in one.
pub const MAX_MASKS: usize = 8;
pub const MAX_MASK_BONES: usize = 16;
/// The longest a decay may be, in ticks: 2 seconds. A longer one is no transition.
pub const MAX_DECAY_TICKS: u32 = 120;

/// A clip a graph plays, and the name the rest of its document calls it by.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphClip {
    pub name: String,
    /// The sidecar ID of the file it comes from.
    pub source: AssetId,
    /// Which of the file's clips: `None` for a key-pose document, which is its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    /// The character it was retargeted to: `None` for a clip on its source's own rig. On a key-pose
    /// document, a rig of its sidecar's `also`: `None` is its `rig`, which naming here is an error, as
    /// nothing reading a graph's document over live link has a sidecar to tell them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AssetId>,
    /// The clip's asset ID, which a person can't write since it's a hash of the other three. The cook
    /// fills this in ([`v2::AnimGraph::resolve`]).
    #[serde(skip, default = "no_id")]
    #[reflect(ignore, default = "no_id")]
    pub id: AssetId,
}

fn no_id() -> AssetId {
    AssetId::from_bytes([0; 16])
}

/// The ID a clip's own cook gives it. A key-pose document is its own clip for its first rig, so that ID
/// is its source's; the rest are sub-assets of their sources, as the pipeline's cook derives them. Here
/// so an app can take a graph's document over live link with no cook in between (M2 T2).
#[must_use]
pub fn clip_id(clip: &GraphClip) -> AssetId {
    match (&clip.clip, clip.target) {
        (Some(name), target) => imported_clip_id(clip.source, name, target),
        (None, Some(target)) => clip.source.sub_asset(&format!("rig/{target}")),
        (None, None) => clip.source,
    }
}

/// Derives an imported animation's ID from its source, UTF-8 name and optional retarget rig.
///
/// The versioned, length-prefixed name and explicit target tag keep arbitrary source names from
/// aliasing a different tuple. This replaces the original delimiter-only IDs: existing imported
/// clips and graphs must be recooked together. Key-pose document IDs are unchanged.
#[must_use]
pub fn imported_clip_id(source: AssetId, name: &str, target: Option<AssetId>) -> AssetId {
    let target = target.map_or_else(|| "none".to_owned(), |id| format!("target/{id}"));
    source.sub_asset(&format!("clip-v2/{}:{name}/{target}", name.len()))
}

/// Which joints a layer touches: named humanoid bones, each with everything under it, later entries
/// over earlier ones. A joint it doesn't name takes no part.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphMask {
    pub name: String,
    pub bones: Vec<MaskBone>,
}

/// One entry of a [`GraphMask`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskBone {
    /// A humanoid bone, by its variant: `Spine`, `RightUpperArm`.
    pub bone: HumanBone,
    /// How much it takes part, 0 to 1.
    pub weight: f32,
}

/// A finite clip faded in over a range of speeds in m/s.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrySample {
    pub clip: String,
    pub fade: [f32; 2],
    /// Third landing sample only. Full below the first departure speed, zero above the second.
    #[serde(default)]
    pub short_lead: Option<[f32; 2]>,
}

/// One clip of a 2D blend space ([`v2::Blend2D`]).
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlendSample {
    pub clip: String,
    /// The velocity it plays at as authored, in m/s in the character's own axes: `(x, z)`, +X its
    /// left, +Z ahead.
    pub at: [f32; 2],
    /// Where in the clip the shared phase's 0 falls, as a fraction of it: the same footfall in every
    /// synced clip, so blending two doesn't cross their legs.
    #[serde(default)]
    pub phase: f32,
    /// Whether it shares the phase. An idle has no footfalls, so it runs on the clock instead.
    #[serde(default = "yes")]
    pub sync: bool,
}

fn yes() -> bool {
    true
}

/// A procedural blink: a morph expression, on the owner's clock, so hitstop holds it.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blink {
    /// The rig's expression that closes the eyes: `blink` on a VRM.
    pub expression: String,
    /// The least and most ticks from one blink's start to the next's.
    pub every: [u32; 2],
    /// Ticks to close, stay shut and open.
    pub close: u32,
    pub hold: u32,
    pub open: u32,
}

/// The widest a graph's eyes may wander from rest, in degrees of gaze either way.
pub const MAX_GAZE_DEGREES: f32 = 45.0;

/// Eyes that move on their own (M6 A22): the gaze holds a while, then jumps (a saccade) to somewhere else
/// within reach of the eyes' rest direction, through the rig's look-at, on the owner's clock, so hitstop
/// holds them as it does the blink. Presentation only: the fixed step never turns them.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eyes {
    /// How far the gaze wanders from rest, `[yaw, pitch]`, in degrees of gaze either way, as the rig's
    /// look-at reads a gaze: its range maps say how far that turns the eyes, or weighs their expressions.
    /// A gaze rests near the middle more often than out at the edge.
    pub reach: [f32; 2],
    /// The least and most ticks from one jump's start to the next's: how long a gaze holds.
    pub hold: [u32; 2],
    /// Degrees of gaze a tick a jump covers, on average over it, so a longer jump takes longer; a jump
    /// takes at least a tick.
    pub speed: f32,
    /// The chance, 0 to 1, that a jump as far as the reach's corner from rest comes with a blink, as eyes
    /// do on a large jump; a shorter jump's chance is smaller by its length. It needs the graph's `blink`,
    /// whose shape it plays. 0, the default, leaves the blinks the blink's alone.
    #[serde(default)]
    pub blink_chance: f32,
}

/// A graph that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// The document isn't a graph document at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names
    /// the field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// Everything a version 1 document's own check found wrong, which keeps it from converting.
    #[error("{}", issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid { issues: Vec<GraphIssue> },
    #[error(transparent)]
    Cooked(#[from] CookedError),
    /// Cooked bytes that deserialize but whose values no cook writes.
    #[error("a damaged graph: {0}")]
    Damaged(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn imported_clip_identity_encodes_the_entire_tuple() {
        let source = AssetId::from_bytes([1; 16]);
        let target = AssetId::from_bytes([2; 16]);
        assert_ne!(
            imported_clip_id(source, &format!("walk@{target}"), None),
            imported_clip_id(source, "walk", Some(target))
        );
        let mut ids = BTreeSet::new();
        for name in ["", "walk", "歩く/none", "4:walk/target", "walk@target", "none"] {
            for target in [None, Some(target), Some(source)] {
                assert!(ids.insert(imported_clip_id(source, name, target)));
            }
        }
    }
}
