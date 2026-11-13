//! A zone's reflection probes (M5 L2, spec §5.27): the cubes a zone document's `reflections` are baked
//! into, `town.zone.ron` → `town.sreflect`, a sub-asset of the zone that its `.szone` names.
//!
//! # What's baked
//!
//! For each probe and each time-of-day key of the zone's day (its `day`, or `sapphire_world`'s default day
//! for a zone without one), a cube of what the probe sees of the zone lit under that key, without the sky,
//! and how much of the sky each texel sees in alpha ([`sapphire_assets::reflections`] says it all). The
//! probe grid's tracer bakes them ([`crate::probes::bake::cubes`]), on what the zone draws: its cells, its
//! backdrop and its instances, as placed, with their materials as the grid's bake sees them. Its local
//! lights are left out, as the grid leaves them out.
//!
//! Surfels carry the sky's light and the bounce within the probes' parallax boxes, grown by the grid's
//! margin; past that, a hit is lit under the open sky.
//!
//! # Sizes
//!
//! A face is [`size`] texels a side: 128 on the desktop cook, which Desktop-High and Desktop-Low use, and 64
//! on the mobile cook, which Mobile-High uses as it is and Mobile-Low a level down, at 32. Only the top
//! level is cooked: the runtime adds the live sky to it first, so it prefilters what it composed.
//!
//! # Keys
//!
//! What a bake reads is its key: the probes and where the zone's instances stand
//! ([`ZonePlan::reflections_recipe`]), every mesh it sees and every material those name, by their keys,
//! and the day's keys as they resolve. A save to a light, an encounter or anything else the probes don't
//! see leaves them alone.

use std::path::Path;

use glam::Vec3;
use sapphire_assets::AssetId;
use sapphire_assets::reflections::{ReflectionProbe, ReflectionProbes, ReflectionProbesError};
use sapphire_zone::ParallaxBox;

use crate::cache::{Dependency, Stage};
use crate::cook::Cook;
use crate::probes::bake::BakeError;
use crate::probes::bake::cubes::{self, CubeProbe};
use crate::probes::{self, BakeTimeline, PlacedMesh, ProbeCookError, ProbeInput};
use crate::zone::ZonePlan;
use sapphire_math::det;

/// Reading the zone's probes, as cache keys know it. Bump the version whenever its output changes for the
/// same document.
pub const IMPORTER: Stage = Stage { name: "reflection probes", version: 1 };

/// The bake and the `.sreflect` writer. Bump the version whenever they change output for the same probes
/// and inputs.
pub const COOKER: Stage = Stage { name: "sreflect bake", version: 1 };

/// Texels along a face's side for `cook` (see the [module docs](self)).
pub const fn size(cook: Cook) -> u32 {
    match cook {
        Cook::Desktop => 128,
        Cook::Mobile => 64,
    }
}

/// Errors from baking a zone's reflection probes.
#[derive(Debug, thiserror::Error)]
pub enum ReflectionCookError {
    /// Reading what the probes see, or the day.
    #[error(transparent)]
    Scene(#[from] ProbeCookError),
    #[error(transparent)]
    Bake(#[from] BakeError),
    #[error("the baked probes are invalid")]
    Invalid(#[from] ReflectionProbesError),
}

/// What the zone's probes see, as placed: its places' merged meshes, in the zone's space, and its
/// instances.
pub fn placed_meshes(plan: &ZonePlan) -> Vec<PlacedMesh> {
    let places = plan.places().map(|place| PlacedMesh {
        mesh: plan.part_id(place),
        rotation: glam::Quat::IDENTITY,
        translation: Vec3::ZERO,
        scale: Vec3::ONE,
    });
    // As the world places an instance: `at · turn · scale`, the turn through `det`.
    let instances = plan.document().instances.iter().map(|instance| PlacedMesh {
        mesh: instance.mesh,
        rotation: det::quat_from_axis_angle(Vec3::Y, instance.turn.to_radians()),
        translation: Vec3::from(instance.at),
        scale: Vec3::from(instance.scale),
    });
    places.chain(instances).collect()
}

/// The day the probes are baked under: the zone's, from its document's directory, or the default day.
pub fn timeline(plan: &ZonePlan) -> BakeTimeline {
    plan.document().day.clone().map_or(BakeTimeline::DefaultDay, BakeTimeline::Document)
}

/// Everything a bake of the probes of the zone at `zone` reads but its recipe: each mesh it sees and every
/// material those name, by their keys, and the day's keys.
pub fn dependencies<'a>(
    plan: &ZonePlan,
    zone: &Path,
    input: impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Vec<Dependency>, ProbeCookError> {
    let placed = placed_meshes(plan);
    probes::scene_dependencies(placed.iter().map(|placed| placed.mesh), &timeline(plan), zone, input)
}

/// Bakes the probes of the zone at `zone` for `cook`: the `.sreflect`'s bytes, read back through the
/// runtime's checks.
pub fn cook<'a>(
    plan: &ZonePlan,
    zone: &Path,
    cook: Cook,
    input: impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Vec<u8>, ReflectionCookError> {
    let scene = probes::scene(&placed_meshes(plan), &input)?;
    let keys = probes::keys(&timeline(plan), zone)?;
    let document = &plan.document().reflections;
    let size = size(cook);
    let baked = bake(&scene, &keys, document, size, probes::bake::workers())?;
    let bytes = baked.to_cooked().map_err(ReflectionProbesError::Cooked)?;
    ReflectionProbes::read(&bytes)?;
    Ok(bytes.into_vec())
}

/// Bakes `document`'s probes at `size` on `threads` threads: the asset, every texel encoded.
pub fn bake(
    scene: &probes::bake::BakeScene,
    keys: &[probes::bake::BakeKey],
    document: &[sapphire_zone::ReflectionProbe],
    size: u32,
    threads: usize,
) -> Result<ReflectionProbes, BakeError> {
    let probes: Vec<CubeProbe> =
        document.iter().map(|probe| CubeProbe { position: Vec3::from(probe.position) }).collect();
    let texels = cubes::bake_cubes(scene, keys, &probes, reach(document), size, threads)?;
    Ok(ReflectionProbes {
        size,
        key_hours: keys.iter().map(|key| key.hours).collect(),
        probes: document
            .iter()
            .map(|probe| ReflectionProbe {
                name: probe.name.clone(),
                position: probe.position,
                parallax_min: probe.parallax.min,
                parallax_max: probe.parallax.max,
            })
            .collect(),
        texels: texels.iter().flat_map(|texel| encode(*texel)).collect(),
    })
}

/// The box every probe's parallax box is in: where surfels carry the sky and the bounce. Through
/// `det::min` and `max`, so an authored −0 is the same corner on every architecture.
fn reach(document: &[sapphire_zone::ReflectionProbe]) -> (Vec3, Vec3) {
    let boxes = document.iter().map(|probe| {
        let ParallaxBox { min, max } = probe.parallax;
        (min, max)
    });
    let (min, max) = boxes
        .reduce(|(min, max), (other_min, other_max)| {
            (
                std::array::from_fn(|axis| det::min(min[axis], other_min[axis])),
                std::array::from_fn(|axis| det::max(max[axis], other_max[axis])),
            )
        })
        .unwrap_or(([0.0; 3], [1.0; 3]));
    (Vec3::from(min), Vec3::from(max))
}

/// A texel as the asset holds it: half floats, the color held to what one holds, rounded to nearest.
pub fn encode([r, g, b, a]: [f32; 4]) -> [u16; 4] {
    let color = |v: f32| half::f16::from_f32(v.clamp(0.0, half::f16::MAX.to_f32())).to_bits();
    [color(r), color(g), color(b), half::f16::from_f32(a.clamp(0.0, 1.0)).to_bits()]
}
