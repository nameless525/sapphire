use std::collections::BTreeMap;

use glam::Vec3;
use sapphire_assets::rig::HumanBone;

use super::*;
use crate::clip::report::Positions;
use crate::clip::retarget::tests::{Kaykit, character};

#[test]
fn every_bone_has_a_partner_of_the_other_side_and_the_middle_its_own() {
    for &bone in HumanBone::ALL {
        let other = partner(bone);
        assert_eq!(partner(other), bone);
        let sided = |bone: HumanBone| bone.name().starts_with("left") || bone.name().starts_with("right");
        assert_eq!(other == bone, !sided(bone), "{bone:?}");
    }
    assert_eq!(partner(HumanBone::LeftUpperArm), HumanBone::RightUpperArm);
    assert_eq!(partner(HumanBone::Hips), HumanBone::Hips);
}

/// The furthest any joint is between two clips' poses at any tick, in meters, root motion applied.
fn furthest(a: &SampledClip, b: &SampledClip, skeleton: &Skeleton) -> f32 {
    let (pa, pb) = (Positions::of(a, skeleton), Positions::of(b, skeleton));
    let root = |clip: &SampledClip, tick: usize| {
        clip.root.as_ref().map_or(Affine3::IDENTITY, |root| {
            Affine3::from_rotation_translation(root.rotation[tick], root.translation[tick])
        })
    };
    let mut worst = 0.0f32;
    for tick in 0..pa.ticks() {
        for joint in 0..skeleton.parents.len() {
            let at =
                |clip, positions: &Positions| root(clip, tick).transform_point3(positions.at(tick, joint));
            worst = worst.max(at(a, &pa).distance(at(b, &pb)));
        }
    }
    worst
}

/// Mirroring twice is the clip again: every joint where it was, within rounding, on KayKit's rig and on
/// the character's.
#[test]
fn mirroring_twice_gives_the_clip_back() {
    let kaykit = Kaykit::open("MovementAdvanced");
    let (left, on_character) = kaykit.on_character("Dodge_Left");
    let back = mirror(&mirror(&left, kaykit.rig()).unwrap(), kaykit.rig()).unwrap();
    let worst = furthest(&left, &back, kaykit.rig().skeleton);
    assert!(worst < 1e-4, "KayKit's rig: {worst} m");

    let (skeleton, map) = character();
    let rig = Rig { skeleton, map };
    let back = mirror(&mirror(&on_character, rig).unwrap(), rig).unwrap();
    let worst = furthest(&on_character, &back, skeleton);
    assert!(worst < 1e-4, "the character: {worst} m");
}

/// KayKit authored both side dodges: the left one mirrored travels as the right one does, and its feet
/// land on the other sides of the body.
#[test]
fn a_mirrored_side_dodge_travels_as_the_authored_other_side_does() {
    let kaykit = Kaykit::open("MovementAdvanced");
    let (left, right) = (kaykit.sampled("Dodge_Left"), kaykit.sampled("Dodge_Right"));
    let made = mirror(&left, kaykit.rig()).unwrap();
    let end = |clip: &SampledClip| *clip.root.as_ref().unwrap().translation.last().unwrap();
    let (authored, mirrored, original) = (end(&right), end(&made), end(&left));
    assert!(authored.x.abs() > 0.3, "the authored right dodge goes sideways: {authored}");
    assert!(authored.x * original.x < 0.0, "the two authored dodges go opposite ways");
    assert!(authored.distance(mirrored) < 0.05, "authored {authored}, mirrored {mirrored}");
    assert!((mirrored.x + original.x).abs() < 1e-6 && mirrored.z == original.z, "travel reflects exactly");

    let skeleton = kaykit.rig().skeleton;
    let positions = (Positions::of(&left, skeleton), Positions::of(&made, skeleton));
    let foot = |bone| usize::from(kaykit.rig().map.joint(bone).unwrap());
    for tick in 0..positions.0.ticks() {
        let left_foot = positions.0.at(tick, foot(HumanBone::LeftFoot));
        let mirrored_right = positions.1.at(tick, foot(HumanBone::RightFoot));
        let reflected = Vec3::new(-left_foot.x, left_foot.y, left_foot.z);
        assert!(reflected.distance(mirrored_right) < 1e-3, "tick {tick}: {reflected} and {mirrored_right}");
    }
}

/// A clip at rest is at rest mirrored: a symmetric rig's rest is its own mirror.
#[test]
fn the_rest_pose_mirrors_to_the_rest_pose() {
    let (skeleton, map) = character();
    let still = SampledClip { name: "still".into(), rate: 60, ticks: 2, ..SampledClip::default() };
    let made = mirror(&still, Rig { skeleton, map }).unwrap();
    let rest = rest_model(skeleton);
    let positions = Positions::of(&made, skeleton);
    for (joint, (rest, name)) in rest.iter().zip(&skeleton.names).enumerate() {
        let moved = positions.at(1, joint).distance(rest.translation);
        assert!(moved < 1e-4, "joint {name} moved {moved} m");
    }
}

#[test]
fn a_sided_bone_without_its_partner_cant_mirror() {
    let (skeleton, map) = character();
    let mut joints: BTreeMap<HumanBone, u16> = map.iter().collect();
    joints.remove(&HumanBone::RightToes);
    let lopsided = HumanMap::from_joints(joints);
    let still = SampledClip { name: "still".into(), rate: 60, ticks: 0, ..SampledClip::default() };
    assert_eq!(
        mirror(&still, Rig { skeleton, map: &lopsided }).unwrap_err(),
        MirrorError::NoPartner { bone: "leftToes", partner: "rightToes" }
    );
}
