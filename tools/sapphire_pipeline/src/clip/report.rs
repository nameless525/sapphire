//! A clip's report (M6 A1): what its cook measures of its motion for the content checks and the clip
//! inspector, beside each foot's slide, which [`contacts`](super::contacts) measures.
//!
//! Both read a clip's samples, before key reduction, on the rig they index: each joint's position
//! in model space at every tick, root motion left out ([`Positions`]). Reduction moves a joint by
//! a few tenths of a millimeter at most ([`ReductionError`](super::ReductionError)), well under
//! what either number is read for.
//!
//! - **Seam:** the furthest any joint is, at the last tick, from where it was at the first. A loop
//!   that plays its first tick after its last jumps by this much; 0 is a clean loop.
//! - **Radius:** the furthest any joint gets from the model's origin across the clip, joints only,
//!   no skin. A character's cooked bounds hold its rest pose; a clip that reaches past them leaves
//!   its posed bounds to catch it.
//!
//! In scalars and `det`, as everything on a clip's path is, so a report is one set of bytes on every
//! architecture.

use glam::{Affine3, Vec3};
use sapphire_assets::clip::ClipReport;
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::det;

use super::{SampledClip, rest_rotation};

/// Every joint's model-space position at every tick of a clip, root motion left out.
pub struct Positions {
    /// By tick, then by joint.
    ticks: Vec<Vec<Vec3>>,
}

impl Positions {
    /// The positions `sampled` puts `skeleton`'s joints at.
    pub fn of(sampled: &SampledClip, skeleton: &Skeleton) -> Self {
        let joints = skeleton.parents.len();
        let by_joint = |tracks: &[(u16, Vec<Vec3>)]| {
            let mut found: Vec<Option<usize>> = vec![None; joints];
            for (index, (joint, _)) in tracks.iter().enumerate() {
                if let Some(place) = found.get_mut(usize::from(*joint)) {
                    *place = Some(index);
                }
            }
            found
        };
        let (translations, scales) = (by_joint(&sampled.translations), by_joint(&sampled.scales));
        let mut rotations: Vec<Option<usize>> = vec![None; joints];
        for (index, (joint, _)) in sampled.rotations.iter().enumerate() {
            if let Some(place) = rotations.get_mut(usize::from(*joint)) {
                *place = Some(index);
            }
        }
        let mut model = vec![Affine3::IDENTITY; joints];
        let ticks = (0..=usize::from(sampled.ticks))
            .map(|tick| {
                for joint in 0..joints {
                    let rest = &skeleton.rest[joint];
                    let vector = |tracks: &[(u16, Vec<Vec3>)], track: Option<usize>, or: [f32; 3]| {
                        track.map_or(Vec3::from(or), |track| tracks[track].1[tick])
                    };
                    let rotation = rotations[joint].map_or_else(
                        || rest_rotation(skeleton, joint),
                        |track| det::quat_normalize(sampled.rotations[track].1[tick]),
                    );
                    let local = Affine3::from_scale_rotation_translation(
                        vector(&sampled.scales, scales[joint], rest.scale),
                        rotation,
                        vector(&sampled.translations, translations[joint], rest.translation),
                    );
                    let parent = skeleton.parents[joint];
                    model[joint] =
                        if parent == NO_PARENT { local } else { model[usize::from(parent)] * local };
                }
                model.iter().map(|joint| joint.translation).collect()
            })
            .collect();
        Self { ticks }
    }

    /// Joint `joint`'s position at `tick`.
    pub fn at(&self, tick: usize, joint: usize) -> Vec3 {
        self.ticks[tick][joint]
    }

    /// How many ticks it holds: the clip's length and one.
    pub fn ticks(&self) -> usize {
        self.ticks.len()
    }
}

/// The seam and radius of a clip whose joints `positions` holds. See the [module docs](self).
pub fn report(positions: &Positions) -> ClipReport {
    let (first, last) = (&positions.ticks[0], &positions.ticks[positions.ticks.len() - 1]);
    let seam = first.iter().zip(last).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    let radius = positions.ticks.iter().flatten().map(|position| position.length()).fold(0.0, f32::max);
    ClipReport { seam, radius }
}
