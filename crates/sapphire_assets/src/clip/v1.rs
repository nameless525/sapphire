//! Version 1 of the cooked clip (M2 N1), frozen, read for one release after version 2 (cooker rule
//! 2): the same tracks and keys, with no contacts, events or report. Its parts kept their layouts,
//! so it shares them; nothing writes it, and its tests read bytes v0.2.0 wrote
//! (`tests/fixtures/clip_v1.sclip`), so a part that changes its layout fails them.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;

use super::{
    Clip, ClipError, PackedRotation, PackedVector, RootMotion, RotationTrack, VectorTrack, WeightTrack,
};
use crate::AssetId;

/// The version this reads.
pub(super) const VERSION: u32 = 1;

/// [`Clip`] as version 1 wrote it, under its name.
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Deserialize)]
#[type_path = "sapphire::assets"]
#[type_name = "Clip"]
pub(super) struct ClipV1 {
    pub name: String,
    pub skeleton: Option<AssetId>,
    pub joint_count: u32,
    pub morph_target_count: u32,
    pub rate: u16,
    pub ticks: u16,
    pub rotations: Vec<RotationTrack>,
    pub translations: Vec<VectorTrack>,
    pub scales: Vec<VectorTrack>,
    pub weights: Vec<WeightTrack>,
    pub root: Option<RootMotion>,
    pub rotation_ticks: Vec<u16>,
    pub rotation_keys: Vec<PackedRotation>,
    pub vector_ticks: Vec<u16>,
    pub vector_keys: Vec<PackedVector>,
    pub weight_ticks: Vec<u16>,
    pub weight_keys: Vec<u16>,
}

impl Versioned for ClipV1 {
    const VERSION: u32 = VERSION;
}

/// Version 1 bytes as a clip of today's version, unchecked past the archive: the caller writes and
/// reads it back.
pub(super) fn read(bytes: &[u8]) -> Result<Clip, ClipError> {
    let archived = sapphire_serialize::access::<ClipV1>(bytes)?;
    let old = rkyv::deserialize::<ClipV1, rkyv::rancor::Error>(archived)
        .map_err(|error| ClipError::Cooked(error.into()))?;
    Ok(Clip {
        name: old.name,
        skeleton: old.skeleton,
        joint_count: old.joint_count,
        morph_target_count: old.morph_target_count,
        rate: old.rate,
        ticks: old.ticks,
        rotations: old.rotations,
        translations: old.translations,
        scales: old.scales,
        weights: old.weights,
        root: old.root,
        rotation_ticks: old.rotation_ticks,
        rotation_keys: old.rotation_keys,
        vector_ticks: old.vector_ticks,
        vector_keys: old.vector_keys,
        weight_ticks: old.weight_ticks,
        weight_keys: old.weight_keys,
        contacts: Vec::new(),
        events: Vec::new(),
        report: None,
    })
}

/// A clip as version 1 wrote it, for tests: `tests/fixtures/clip_v1.sclip`, the clip tests' sample
/// written by v0.2.0's own `to_cooked`, from a checkout of that tag (M6 A1's backfill). It is what a
/// 0.2.0 pack holds, laid out by the code that wrote one, not by today's part types, so a change to one
/// of those or to [`ClipV1`]'s fields fails the tests that read it. Never rewritten.
#[cfg(test)]
pub(crate) fn fixture() -> sapphire_serialize::cooked::AlignedVec {
    let mut bytes = sapphire_serialize::cooked::AlignedVec::new();
    bytes.extend_from_slice(include_bytes!("../../tests/fixtures/clip_v1.sclip"));
    bytes
}
