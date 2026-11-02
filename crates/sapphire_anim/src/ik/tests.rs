//! IK on a small humanoid: what a character's solve must keep true, stage by stage.

use std::f32::consts::{FRAC_PI_2, PI};

use sapphire_assets::rig::{ConstraintKind, HumanBone};
use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
use sapphire_math::{Affine3A, Quat, Transform, Vec3};

use super::*;
use crate::character::CharacterPose;

const HIPS: usize = 1;
const SPINE: usize = 2;
const CHEST: usize = 3;
const NECK: usize = 4;
const HEAD: usize = 5;
const LEFT_EYE: usize = 6;
const RIGHT_EYE: usize = 7;
const LEFT_UPPER_LEG: usize = 8;
const LEFT_LOWER_LEG: usize = 9;
const LEFT_FOOT: usize = 10;
const LEFT_TOES: usize = 11;
const RIGHT_UPPER_LEG: usize = 12;
const RIGHT_LOWER_LEG: usize = 13;
const RIGHT_FOOT: usize = 14;
const RIGHT_TOES: usize = 15;
const LEFT_UPPER_ARM: usize = 16;
const LEFT_LOWER_ARM: usize = 17;
const LEFT_HAND: usize = 18;
const RIGHT_UPPER_ARM: usize = 19;
const RIGHT_LOWER_ARM: usize = 20;
const RIGHT_HAND: usize = 21;
const SOCKET: usize = 22;
const HELPER: usize = 23;

/// The ankle's height over the floor in the fixture below.
const ANKLE: f32 = 0.12;

fn joint(parent: Option<usize>, translation: [f32; 3], rotation: Quat) -> Joint {
    Joint {
        name: String::new(),
        parent: parent.map(|parent| parent as u16),
        rest: JointTransform { translation, rotation: rotation.to_array(), ..JointTransform::IDENTITY },
    }
}

/// A humanoid facing +Z, its feet on the floor at the model's origin, with knees very slightly bent
/// and its joints' local axes turned every which way, as a real rig's are.
fn skeleton() -> Skeleton {
    let odd = Quat::from_xyzw(0.5, 0.5, 0.5, 0.5);
    let flat = Quat::IDENTITY;
    let joints = vec![
        joint(None, [0.0, 0.0, 0.0], flat),
        joint(Some(0), [0.0, 0.9, 0.0], Quat::from_rotation_x(0.1)),
        joint(Some(HIPS), [0.0, 0.1, 0.0], flat),
        joint(Some(SPINE), [0.0, 0.15, 0.0], Quat::from_rotation_x(-0.1)),
        joint(Some(CHEST), [0.0, 0.25, 0.0], flat),
        joint(Some(NECK), [0.0, 0.1, 0.0], odd),
        joint(Some(HEAD), [0.05, 0.03, 0.02], odd.inverse()),
        joint(Some(HEAD), [0.05, 0.03, -0.02], odd.inverse()),
        // The legs: 0.4 and 0.4, hips 0.1 either side, with the knee a little forward. Each foot's own
        // height keeps the ankle ANKLE over the floor at rest.
        joint(Some(HIPS), [0.1, 0.0, 0.0], Quat::from_rotation_x(-0.1)),
        joint(Some(LEFT_UPPER_LEG), [0.0, -0.39, 0.03], flat),
        joint(Some(LEFT_LOWER_LEG), [0.0, -0.39, -0.03], flat),
        joint(Some(LEFT_FOOT), [0.0, -0.06, 0.12], flat),
        joint(Some(HIPS), [-0.1, 0.0, 0.0], Quat::from_rotation_x(-0.1)),
        joint(Some(RIGHT_UPPER_LEG), [0.0, -0.39, 0.03], flat),
        joint(Some(RIGHT_LOWER_LEG), [0.0, -0.39, -0.03], flat),
        joint(Some(RIGHT_FOOT), [0.0, -0.06, 0.12], flat),
        // The arms hang out sideways, each with its elbow a little back.
        joint(Some(CHEST), [0.2, 0.2, 0.0], odd),
        joint(Some(LEFT_UPPER_ARM), [0.0, 0.25, -0.02], flat),
        joint(Some(LEFT_LOWER_ARM), [0.0, 0.25, 0.02], flat),
        joint(Some(CHEST), [-0.2, 0.2, 0.0], odd),
        joint(Some(RIGHT_UPPER_ARM), [0.0, 0.25, -0.02], flat),
        joint(Some(RIGHT_LOWER_ARM), [0.0, 0.25, 0.02], flat),
        // A socket on the right hand, and a helper the rig keeps rolling with the left forearm.
        joint(Some(RIGHT_HAND), [0.0, 0.08, 0.0], flat),
        joint(Some(LEFT_UPPER_ARM), [0.0, 0.25, -0.02], flat),
    ];
    Skeleton::new(&joints)
}

fn bones() -> Vec<(HumanBone, usize)> {
    vec![
        (HumanBone::Hips, HIPS),
        (HumanBone::Spine, SPINE),
        (HumanBone::Chest, CHEST),
        (HumanBone::Neck, NECK),
        (HumanBone::Head, HEAD),
        (HumanBone::LeftEye, LEFT_EYE),
        (HumanBone::RightEye, RIGHT_EYE),
        (HumanBone::LeftUpperLeg, LEFT_UPPER_LEG),
        (HumanBone::LeftLowerLeg, LEFT_LOWER_LEG),
        (HumanBone::LeftFoot, LEFT_FOOT),
        (HumanBone::LeftToes, LEFT_TOES),
        (HumanBone::RightUpperLeg, RIGHT_UPPER_LEG),
        (HumanBone::RightLowerLeg, RIGHT_LOWER_LEG),
        (HumanBone::RightFoot, RIGHT_FOOT),
        (HumanBone::RightToes, RIGHT_TOES),
        (HumanBone::LeftUpperArm, LEFT_UPPER_ARM),
        (HumanBone::LeftLowerArm, LEFT_LOWER_ARM),
        (HumanBone::LeftHand, LEFT_HAND),
        (HumanBone::RightUpperArm, RIGHT_UPPER_ARM),
        (HumanBone::RightLowerArm, RIGHT_LOWER_ARM),
        (HumanBone::RightHand, RIGHT_HAND),
    ]
}

fn pose() -> CharacterPose {
    let bytes = skeleton().to_cooked().expect("cooked");
    let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
    let helper = sapphire_assets::rig::Constraint {
        joint: HELPER as u16,
        source: LEFT_LOWER_ARM as u16,
        weight: 1.0,
        kind: ConstraintKind::Rotation,
    };
    CharacterPose::new(skeleton, &[], &[helper], vec![0.0; 4])
}

/// VRM's map for the shipped character: a turn of up to 10° at a gaze of 90°.
fn eyes(pose: &CharacterPose) -> EyeRig {
    let map = RangeMap { input_max: FRAC_PI_2, output: 10f32.to_radians() };
    let eye = |joint: usize| Some((joint as u16, pose.rest_rotation(HEAD)));
    EyeRig {
        inner: map,
        outer: map,
        down: map,
        up: map,
        drive: EyeDrive::Bones { left: eye(LEFT_EYE), right: eye(RIGHT_EYE) },
    }
}

fn character() -> (CharacterPose, CharacterIk) {
    let mut pose = pose();
    let rig = IkRig::new(&pose, &bones(), Vec3::new(0.05, 0.03, 0.0), Some(eyes(&pose)), 0.0);
    let ik = CharacterIk::new(rig, FootSettings::default(), LookSettings::default());
    pose.reset();
    (pose, ik)
}

const TICK: f32 = 1.0 / 60.0;

fn input() -> IkInput {
    IkInput {
        dt: TICK,
        world_from_model: Affine3A::IDENTITY,
        feet: FeetInput::default(),
        look: LookInput::default(),
        weapon: None,
    }
}

/// The ground under `probe`, a plane through `point` facing `normal`.
fn on_plane(probe: Vec3, point: Vec3, normal: Vec3) -> GroundHit {
    let normal = normal.normalize();
    let height = point.y + ((point.x - probe.x) * normal.x + (point.z - probe.z) * normal.z) / normal.y;
    GroundHit { point: Vec3::new(probe.x, height, probe.z), normal }
}

/// Hands over `ground` for what `ik`'s probes ask, and steps once.
fn step(
    pose: &mut CharacterPose,
    ik: &mut CharacterIk,
    world_from_model: Affine3A,
    weight: f32,
    ground: &dyn Fn(Vec3) -> Option<GroundHit>,
) {
    pose.reset();
    ik.prepare(pose);
    let probes = ik.foot_probes(pose, &world_from_model).expect("it has feet");
    let foot =
        |probe: &FootProbe| FootGround { ankle: ground(probe.ankle), ball: probe.ball.and_then(ground) };
    let mut input = input();
    input.world_from_model = world_from_model;
    input.feet = FeetInput { weight, ground: [foot(&probes[0]), foot(&probes[1])] };
    ik.solve(pose, &input);
    pose.finish();
}

fn world_position(pose: &CharacterPose, world_from_model: Affine3A, joint: usize) -> Vec3 {
    world_from_model.transform_point3(Vec3::from(pose.model()[joint].translation))
}

fn bits(pose: &CharacterPose) -> Vec<u32> {
    pose.locals()
        .iter()
        .flat_map(|local| {
            local
                .translation
                .to_array()
                .into_iter()
                .chain(local.rotation.to_array())
                .chain(local.scale.to_array())
                .map(f32::to_bits)
        })
        .chain(pose.model().iter().flat_map(|model| model.to_cols_array().map(f32::to_bits)))
        .collect()
}

fn slope(angle: f32) -> impl Fn(Vec3) -> Option<GroundHit> {
    let normal = Vec3::new(-angle.sin(), angle.cos(), 0.0);
    move |probe| Some(on_plane(probe, Vec3::ZERO, normal))
}

// Foot placement.

/// The distance from a sole to the plane through `point`: the ankle's height along the normal, taken
/// off the ankle's height over the plane.
fn sole_gap(pose: &CharacterPose, world: Affine3A, joint: usize, point: Vec3, normal: Vec3) -> f32 {
    (world_position(pose, world, joint) - point).dot(normal.normalize()) - ANKLE
}

/// The controller's round foot hovers over a 30° slope by `radius × (1/cos θ − 1)`: 4.6 cm for its
/// 0.3 m capsule. Both soles land on the slope all the same, with the hips dropped for the lower.
#[test]
fn the_round_foots_gap_closes_on_a_slope() {
    let (mut pose, mut ik) = character();
    let angle = 30f32.to_radians();
    let hover = 0.3 * (1.0 / angle.cos() - 1.0);
    assert!((hover - 0.0464).abs() < 1e-3);
    let world = Affine3A::from_translation(Vec3::new(0.0, hover, 0.0));
    let normal = Vec3::new(-angle.sin(), angle.cos(), 0.0);
    step(&mut pose, &mut ik, world, 1.0, &slope(angle));
    for foot in [LEFT_FOOT, RIGHT_FOOT] {
        let gap = sole_gap(&pose, world, foot, Vec3::ZERO, normal);
        assert!(gap.abs() < 2e-3, "{foot}: {gap} m off the slope");
    }
    // The lower foot needed the hips to come down, by more than the hover alone.
    let lowered = world_position(&pose, world, HIPS).y - (hover + 0.9);
    assert!(lowered < -hover, "the hips went {lowered} m");
    // And the soles lie along the slope.
    for foot in [LEFT_FOOT, RIGHT_FOOT] {
        let sole = pose.model()[foot].transform_vector3(Vec3::NEG_Y).normalize();
        assert!(sole.dot(-normal) > 0.999, "{foot}: its sole points along {sole}");
    }
}

#[test]
fn a_flat_floor_changes_nothing_not_one_bit() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::from_translation(Vec3::new(3.0, 1.5, -2.0));
    let floor = |probe: Vec3| Some(on_plane(probe, Vec3::new(0.0, 1.5, 0.0), Vec3::Y));
    step(&mut pose, &mut ik, world, 1.0, &floor);
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose));
}

/// One foot on a step: the step's foot rises, the other stays where the clip has it.
#[test]
fn one_foot_up_a_step_bends_that_knee_and_leaves_the_other_foot() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let step_height = 0.18;
    let ground = move |probe: Vec3| {
        let height = if probe.x > 0.0 { step_height } else { 0.0 };
        Some(GroundHit { point: Vec3::new(probe.x, height, probe.z), normal: Vec3::Y })
    };
    step(&mut pose, &mut ik, world, 1.0, &ground);
    let (left, right) = (world_position(&pose, world, LEFT_FOOT), world_position(&pose, world, RIGHT_FOOT));
    assert!((left.y - (step_height + ANKLE)).abs() < 1e-3, "{left}");
    assert!((right.y - ANKLE).abs() < 1e-3, "{right}");
    assert_eq!(world_position(&pose, world, HIPS).y, 0.9, "both soles reach without the hips");
    // The raised knee goes forward, not back.
    assert!(
        pose.model()[LEFT_LOWER_LEG].translation.z > 0.05,
        "{}",
        pose.model()[LEFT_LOWER_LEG].translation
    );
}

/// A foot whose toes are over a step's edge stands on the step, and one overhanging an edge stays up.
#[test]
fn a_foot_facing_a_step_stands_on_it_and_a_foot_past_an_edge_stays_on_top() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let riser = 0.06;
    let up = move |probe: Vec3| {
        let height = if probe.z > riser { 0.15 } else { 0.0 };
        Some(GroundHit { point: Vec3::new(probe.x, height, probe.z), normal: Vec3::Y })
    };
    step(&mut pose, &mut ik, world, 1.0, &up);
    let ankle = world_position(&pose, world, LEFT_FOOT);
    assert!((ankle.y - (0.15 + ANKLE)).abs() < 1e-3, "the ball is on the step: {ankle}");
    let (mut pose, mut ik) = character();
    let down = move |probe: Vec3| {
        let height = if probe.z > riser { 0.0 } else { 0.15 };
        Some(GroundHit { point: Vec3::new(probe.x, height, probe.z), normal: Vec3::Y })
    };
    step(&mut pose, &mut ik, world, 1.0, &down);
    let ankle = world_position(&pose, world, LEFT_FOOT);
    assert!((ankle.y - (0.15 + ANKLE)).abs() < 1e-3, "the ankle is over the step: {ankle}");
}

/// Hips drop as far as they have to and legs keep their lengths.
#[test]
fn a_foot_that_cannot_reach_brings_the_hips_down_and_no_bone_stretches() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let upper = pose.model()[LEFT_LOWER_LEG].translation.distance(pose.model()[LEFT_UPPER_LEG].translation);
    let low = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -0.2, probe.z), normal: Vec3::Y });
    step(&mut pose, &mut ik, world, 1.0, &low);
    assert!(world_position(&pose, world, HIPS).y < 0.72, "{}", world_position(&pose, world, HIPS));
    let bent = pose.model()[LEFT_LOWER_LEG].translation.distance(pose.model()[LEFT_UPPER_LEG].translation);
    assert!((bent - upper).abs() < 1e-5, "{bent} against {upper}");
    assert!((world_position(&pose, world, LEFT_FOOT).y - (ANKLE - 0.2)).abs() < 2e-3);
}

/// In the air, or in a move that says so: it fades off smoothly, and on again.
#[test]
fn the_weight_fades_feet_off_and_on_with_no_jump() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ground = slope(25f32.to_radians());
    step(&mut pose, &mut ik, world, 1.0, &ground);
    let mut last = world_position(&pose, world, LEFT_FOOT);
    let mut reached = false;
    for (weight, ticks) in [(0.0, 20), (1.0, 20)] {
        for _ in 0..ticks {
            step(&mut pose, &mut ik, world, weight, &ground);
            let now = world_position(&pose, world, LEFT_FOOT);
            assert!(now.distance(last) < 0.02, "a jump of {} m", now.distance(last));
            last = now;
        }
        let fade = ik.feet().expect("it has feet").fade();
        assert_eq!(fade, weight, "it got there in 20 ticks");
        reached = true;
    }
    assert!(reached);
    // Off at the end of a fade, once the hips are home, means the clip's own pose, bit for bit.
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 0.0, &ground);
    }
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose), "weight 0 is no solve at all");
}

/// The ground changing under a foot eases in, and a planted foot stays put if only its root moved.
#[test]
fn a_change_of_ground_eases_and_a_root_popping_up_a_step_does_not_lift_the_foot() {
    let (mut pose, mut ik) = character();
    let flat = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, 0.0, probe.z), normal: Vec3::Y });
    step(&mut pose, &mut ik, Affine3A::IDENTITY, 1.0, &flat);
    // The root pops up 15 cm, as the controller's step-up does: the ground hasn't moved, so nor has the
    // foot, and nor have the hips on screen.
    let hips = world_position(&pose, Affine3A::IDENTITY, HIPS).y;
    let world = Affine3A::from_translation(Vec3::new(0.0, 0.15, 0.0));
    step(&mut pose, &mut ik, world, 1.0, &flat);
    let foot = world_position(&pose, world, LEFT_FOOT).y;
    assert!((foot - ANKLE).abs() < 1e-3, "the foot stays: {foot}");
    let popped = world_position(&pose, world, HIPS).y;
    assert!((popped - hips).abs() < 1e-3, "and the hips, from {hips} to {popped}");
    // A new surface under the foot, 10 cm up, eases in over a few ticks.
    let raised = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, 0.1, probe.z), normal: Vec3::Y });
    let world = Affine3A::IDENTITY;
    step(&mut pose, &mut ik, world, 1.0, &flat);
    step(&mut pose, &mut ik, world, 1.0, &raised);
    let first = world_position(&pose, world, LEFT_FOOT).y;
    assert!(first > ANKLE + 0.005 && first < ANKLE + 0.05, "{first}");
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 1.0, &raised);
    }
    assert!((world_position(&pose, world, LEFT_FOOT).y - (ANKLE + 0.1)).abs() < 1e-3);
    assert!(ik.feet().expect("it has feet").is_settled(), "and it's exactly there, to skip the work");
}

/// A foot with nothing under it goes back to its animation, alone.
#[test]
fn a_foot_over_nothing_lets_go_of_the_ground_alone() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ledge = |probe: Vec3| {
        (probe.x < 0.0).then_some(GroundHit { point: Vec3::new(probe.x, 0.1, probe.z), normal: Vec3::Y })
    };
    step(&mut pose, &mut ik, world, 1.0, &ledge);
    for _ in 0..30 {
        step(&mut pose, &mut ik, world, 1.0, &ledge);
    }
    assert!((world_position(&pose, world, RIGHT_FOOT).y - (ANKLE + 0.1)).abs() < 2e-3);
    assert!(
        (world_position(&pose, world, LEFT_FOOT).y - ANKLE).abs() < 2e-3,
        "{}",
        world_position(&pose, world, LEFT_FOOT)
    );
}

/// A lifted foot keeps its lift over a raised floor and lets go of a slope's tilt.
#[test]
fn a_lifted_foot_keeps_its_lift_and_does_not_tilt() {
    let (mut pose, mut ik) = character();
    let lift = 0.2;
    let world = Affine3A::IDENTITY;
    let angle = 25f32.to_radians();
    let ground = slope(angle);
    pose.reset();
    // The clip raises the left knee and turns the shin back so the ankle comes up by `lift`.
    let raised_at = {
        let solving = &mut pose.solving();
        solving.refresh_chain(LEFT_FOOT);
        let target = solving.position(LEFT_FOOT) + Vec3::Y * lift;
        let goal = TwoBoneGoal { target, pole: None, hint: Vec3::Z, soften: 1.0, weight: 1.0 };
        two_bone(solving, TwoBone { root: LEFT_UPPER_LEG, mid: LEFT_LOWER_LEG, end: LEFT_FOOT }, &goal);
        solving.local(LEFT_LOWER_LEG)
    };
    let upper = pose.locals()[LEFT_UPPER_LEG];
    let sample = |pose: &mut CharacterPose| {
        pose.reset();
        pose.locals_mut()[LEFT_UPPER_LEG] = upper;
        pose.locals_mut()[LEFT_LOWER_LEG] = raised_at;
    };
    sample(&mut pose);
    pose.finish();
    let clip_foot = pose.model()[LEFT_FOOT].matrix3;
    sample(&mut pose);
    ik.prepare(&mut pose);
    let probes = ik.foot_probes(&mut pose, &world).expect("it has feet");
    let foot = |probe: &FootProbe| FootGround { ankle: ground(probe.ankle), ball: None };
    let mut request = input();
    request.feet = FeetInput { weight: 1.0, ground: [foot(&probes[0]), foot(&probes[1])] };
    ik.solve(&mut pose, &request);
    pose.finish();
    // Its ankle stands the lift over the ground where it is: none of the planted foot's tilt.
    let ankle = Vec3::from(pose.model()[LEFT_FOOT].translation);
    let under = ground(ankle).expect("ground").point.y;
    assert!((ankle.y - (under + ANKLE + lift)).abs() < 2e-3, "{ankle} over {under}");
    let foot = pose.model()[LEFT_FOOT].matrix3;
    assert!(
        foot.abs_diff_eq(clip_foot, 1e-4),
        "its foot stays as the clip turned it: {foot} against {clip_foot}"
    );
}

// Look-at.

fn looking(pose: &mut CharacterPose, ik: &mut CharacterIk, target: Option<Vec3>, weight: f32, dt: f32) {
    pose.reset();
    ik.prepare(pose);
    let mut request = input();
    request.dt = dt;
    request.look = LookInput { target, weight };
    ik.solve(pose, &request);
    pose.finish();
}

/// The head's forward in the model's space.
fn head_forward(pose: &CharacterPose) -> Vec3 {
    let (_, rotation, _) = pose.model()[HEAD].to_scale_rotation_translation();
    (rotation * pose.rest_rotation(HEAD).inverse() * Vec3::Z).normalize()
}

fn settle(pose: &mut CharacterPose, ik: &mut CharacterIk, target: Vec3) {
    for _ in 0..240 {
        looking(pose, ik, Some(target), 1.0, TICK);
    }
}

#[test]
fn the_head_looks_at_a_target_within_its_limits() {
    let (mut pose, mut ik) = character();
    let target = Vec3::new(1.5, 1.8, 3.0);
    settle(&mut pose, &mut ik, target);
    let eyes = pose.model()[HEAD].transform_point3(Vec3::new(0.05, 0.03, 0.0));
    let forward = head_forward(&pose);
    let wanted = (target - eyes).normalize();
    assert!(forward.angle_between(wanted) < 1.5f32.to_radians(), "{forward} against {wanted}");
    assert!(ik.look().expect("it has a head").is_settled());
    // The turn shares out down the neck: every joint of it took some.
    for joint in [NECK, HEAD] {
        let turned = pose.locals()[joint].rotation.angle_between(pose.rest()[joint].rotation);
        assert!(turned > 0.05, "{joint}: {turned}");
    }
}

/// Looking up and to the side, a head doesn't lean: its ear-to-ear line stays level.
#[test]
fn a_diagonal_look_keeps_the_head_level() {
    let (mut pose, mut ik) = character();
    settle(&mut pose, &mut ik, Vec3::new(3.0, 2.8, 2.5));
    let (_, rotation, _) = pose.model()[HEAD].to_scale_rotation_translation();
    let turn = rotation * pose.rest_rotation(HEAD).inverse();
    let across = turn * Vec3::X;
    assert!((turn * Vec3::Z).y > 0.2, "it looks up");
    assert!(across.y.abs() < 0.02, "it leans: {across}");
}

#[test]
fn the_limits_hold_past_the_shoulder() {
    let (mut pose, mut ik) = character();
    let limit = LookSettings::default().yaw_limit;
    settle(&mut pose, &mut ik, Vec3::new(5.0, 1.6, 0.5));
    let forward = head_forward(&pose);
    let yaw = forward.x.atan2(forward.z);
    assert!((yaw - limit).abs() < 3f32.to_radians(), "yaw {yaw} against {limit}");
}

/// The one this exists for: a target whipping round behind the character. Every tick's turn stays
/// under the rate, whichever way the target goes, and nothing happens at dead behind.
#[test]
fn a_target_crossing_behind_never_snaps() {
    let (mut pose, mut ik) = character();
    let rate = LookSettings::default().rate;
    let target = |angle: f32| Vec3::new(4.0 * angle.sin(), 1.6, 4.0 * angle.cos());
    settle(&mut pose, &mut ik, target(2.0));
    let mut last = head_forward(&pose);
    // Round through dead behind and back, faster than the head can follow.
    for step in 0..360 {
        let angle = 2.0 + 2.0 * PI * (step as f32 / 180.0);
        looking(&mut pose, &mut ik, Some(target(angle)), 1.0, TICK);
        let now = head_forward(&pose);
        let turned = last.angle_between(now);
        assert!(
            turned <= rate * TICK * 1.02,
            "step {step}: turned {turned} rad, the rate allows {}",
            rate * TICK
        );
        last = now;
    }
    // And a target that jumps from one shoulder to the other: the same.
    settle(&mut pose, &mut ik, target(1.0));
    let mut last = head_forward(&pose);
    for _ in 0..60 {
        looking(&mut pose, &mut ik, Some(target(-1.0)), 1.0, TICK);
        let now = head_forward(&pose);
        assert!(last.angle_between(now) <= rate * TICK * 1.02);
        last = now;
    }
}

/// Hovering exactly behind doesn't wobble.
#[test]
fn a_target_hovering_dead_behind_leaves_the_head_still() {
    let (mut pose, mut ik) = character();
    settle(&mut pose, &mut ik, Vec3::new(0.0, 1.6, -4.0));
    let settled = head_forward(&pose);
    for step in 0..60 {
        let wobble = if step % 2 == 0 { 0.02 } else { -0.02 };
        looking(&mut pose, &mut ik, Some(Vec3::new(wobble, 1.6, -4.0)), 1.0, TICK);
        assert!(head_forward(&pose).angle_between(settled) < 0.01, "step {step}");
    }
}

#[test]
fn the_eyes_turn_by_the_rigs_range_map_from_the_rest_pose_of_a_rig_with_any_axes() {
    let (mut pose, mut ik) = character();
    // Toward the left at 60° from straight on and level, within the head's limit, so the eyes have only
    // what parallax leaves them.
    let target = Vec3::new(3.0, 1.46, 3.0 * 3f32.sqrt());
    settle(&mut pose, &mut ik, target);
    let forward = head_forward(&pose);
    let eye_rest = pose.rest_rotation(LEFT_EYE);
    let (_, eye_now, _) = pose.model()[LEFT_EYE].to_scale_rotation_translation();
    let along = (eye_now * eye_rest.inverse()) * Vec3::Z;
    // The eyes lead the head by the small angle the map allows, the way the target lies from it.
    let eyes = pose.model()[HEAD].transform_point3(Vec3::new(0.05, 0.03, 0.0));
    let left = (target - eyes).normalize().angle_between(forward);
    let wanted = left / FRAC_PI_2 * 10f32.to_radians();
    assert!((along.angle_between(forward) - wanted).abs() < 2f32.to_radians(), "{along} {forward} {wanted}");
}

#[test]
fn expressions_take_the_eyes_weight_where_the_rig_has_no_eye_bones() {
    let mut pose = pose();
    let map = RangeMap { input_max: FRAC_PI_2, output: 1.0 };
    let drive = EyeDrive::Expressions {
        up: vec![(0, 1.0)],
        down: vec![(1, 0.5)],
        left: vec![(2, 1.0)],
        right: vec![(3, 1.0)],
    };
    let eyes = EyeRig { inner: map, outer: map, down: map, up: map, drive };
    let rig = IkRig::new(&pose, &bones(), Vec3::ZERO, Some(eyes), 0.0);
    let mut ik = CharacterIk::new(rig, FootSettings::default(), LookSettings::default());
    // Past the head's limit, up and to the left, so the eyes have some way left to go.
    let target = Vec3::new(5.0, 3.5, 1.0);
    for _ in 0..240 {
        pose.reset();
        pose.weights_mut().fill(0.0);
        ik.prepare(&mut pose);
        let mut request = input();
        request.look = LookInput { target: Some(target), weight: 1.0 };
        ik.solve(&mut pose, &request);
        pose.finish();
    }
    let weights = pose.weights();
    assert!(weights[0] > 0.05, "up: {weights:?}");
    assert_eq!(weights[1], 0.0, "not down: {weights:?}");
    assert!(weights[2] > 0.05, "left: {weights:?}");
    assert_eq!(weights[3], 0.0, "not right: {weights:?}");
}

#[test]
fn looking_fades_in_and_out_from_the_pose() {
    let (mut pose, mut ik) = character();
    let target = Vec3::new(2.0, 1.6, 2.0);
    let fade_ticks = (LookSettings::default().fade / TICK).ceil() as usize;
    looking(&mut pose, &mut ik, Some(target), 1.0, TICK);
    let early = head_forward(&pose);
    let mut last = early;
    for _ in 0..fade_ticks {
        looking(&mut pose, &mut ik, Some(target), 1.0, TICK);
        let now = head_forward(&pose);
        assert!(last.angle_between(now) < 0.08, "{}", last.angle_between(now));
        last = now;
    }
    assert_eq!(ik.look().expect("a head").fade(), 1.0);
    assert!(early.x < last.x, "it came round by degrees");
    for _ in 0..=fade_ticks {
        looking(&mut pose, &mut ik, None, 1.0, TICK);
        let now = head_forward(&pose);
        assert!(last.angle_between(now) < 0.08, "{}", last.angle_between(now));
        last = now;
    }
    assert_eq!(ik.look().expect("a head").fade(), 0.0);
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose), "nothing to look at is no solve at all");
}

/// The first look after none starts from where the target is, by weight alone: it never sweeps across
/// from wherever it was last looking.
#[test]
fn a_new_look_comes_up_from_the_pose_not_from_the_last_target() {
    let (mut pose, mut ik) = character();
    settle(&mut pose, &mut ik, Vec3::new(3.0, 1.6, 1.0));
    for _ in 0..60 {
        looking(&mut pose, &mut ik, None, 1.0, TICK);
    }
    looking(&mut pose, &mut ik, Some(Vec3::new(-3.0, 1.6, 1.0)), 1.0, TICK);
    assert!(head_forward(&pose).x <= 0.0, "{}", head_forward(&pose));
}

// A frozen clock.

/// Nothing a target does moves a frozen character's solve, so skipping it loses nothing, and the head
/// takes up the new target at its usual speed when the clock thaws.
#[test]
fn a_frozen_clock_moves_nothing_however_the_target_moves_and_thawing_never_snaps() {
    let (mut pose, mut ik) = character();
    let ground = slope(20f32.to_radians());
    let world = Affine3A::IDENTITY;
    let run = |pose: &mut CharacterPose, ik: &mut CharacterIk, dt: f32, target: Vec3| {
        pose.reset();
        ik.prepare(pose);
        let probes = ik.foot_probes(pose, &world).expect("it has feet");
        let foot =
            |probe: &FootProbe| FootGround { ankle: ground(probe.ankle), ball: probe.ball.and_then(&ground) };
        let mut request = input();
        request.dt = dt;
        request.feet = FeetInput { weight: 1.0, ground: [foot(&probes[0]), foot(&probes[1])] };
        request.look = LookInput { target: Some(target), weight: 1.0 };
        ik.solve(pose, &request);
        pose.finish();
    };
    for _ in 0..120 {
        run(&mut pose, &mut ik, TICK, Vec3::new(2.0, 1.6, 2.0));
    }
    let held = bits(&pose);
    let forward = head_forward(&pose);
    for step in 0..30 {
        run(&mut pose, &mut ik, 0.0, Vec3::new(-2.0 - step as f32 * 0.1, 1.6, 1.0));
        assert_eq!(bits(&pose), held, "frozen, step {step}");
    }
    let rate = LookSettings::default().rate;
    run(&mut pose, &mut ik, TICK, Vec3::new(-5.0, 1.6, 1.0));
    let turned = head_forward(&pose).angle_between(forward);
    assert!(turned > 0.0 && turned <= rate * TICK * 1.02, "thawed: {turned}");
}

// Hands.

fn palm() -> Transform {
    Transform::from_translation(Vec3::new(0.0, 0.05, 0.0))
}

fn holding(pose: &mut CharacterPose, ik: &mut CharacterIk, weapon: HeldWeapon) {
    pose.reset();
    ik.prepare(pose);
    let mut request = input();
    request.weapon = Some(weapon);
    ik.solve(pose, &request);
    pose.finish();
}

/// A palm's place in the model's space: the hand's transform times its grip point.
fn palm_of(pose: &CharacterPose, hand: usize) -> Affine3A {
    pose.model()[hand] * palm().to_affine3a()
}

/// A weapon in a right hand, by a socket: the left hand goes to its second grip, wherever the right
/// hand's clip takes it.
#[test]
fn the_off_hand_goes_to_a_grip_on_what_the_main_hand_carries() {
    let (mut pose, mut ik) = character();
    let on_weapon = Transform {
        translation: Vec3::new(0.0, 0.3, 0.05),
        rotation: Quat::from_rotation_x(0.4),
        scale: Vec3::ONE,
    };
    let weapon = HeldWeapon {
        place: WeaponPlace::Joint { joint: SOCKET as u16, offset: Affine3A::IDENTITY },
        main: Hand::Right,
        main_grip: None,
        off_grip: Some(Grip { on_weapon, in_hand: palm(), pole: None, weight: 1.0 }),
    };
    holding(&mut pose, &mut ik, weapon);
    let wanted = pose.model()[SOCKET] * on_weapon.to_affine3a();
    let got = palm_of(&pose, LEFT_HAND);
    // An arm's reach away.
    let shoulder = Vec3::from(pose.model()[LEFT_UPPER_ARM].translation);
    let reach = 0.5;
    if shoulder.distance(Vec3::from(wanted.translation)) < reach * 0.95 {
        assert!(
            Vec3::from(got.translation).distance(Vec3::from(wanted.translation)) < 2e-3,
            "{got:?} against {wanted:?}"
        );
    }
    let (_, got_rotation, _) = got.to_scale_rotation_translation();
    let (_, wanted_rotation, _) = wanted.to_scale_rotation_translation();
    assert!(got_rotation.angle_between(wanted_rotation) < 1e-3, "the hand turned to the grip");
}

/// A placed weapon: both hands follow it, each by its weight, and the rig's helpers follow the arm.
#[test]
fn both_hands_follow_a_placed_weapon_and_weight_zero_lets_go() {
    let (mut pose, mut ik) = character();
    let place = Affine3A::from_rotation_translation(Quat::from_rotation_y(0.3), Vec3::new(0.0, 1.2, 0.35));
    let grip = |x: f32, weight: f32| Grip {
        on_weapon: Transform::from_translation(Vec3::new(x, 0.0, 0.0)),
        in_hand: palm(),
        pole: Some(Vec3::NEG_Y),
        weight,
    };
    let weapon = HeldWeapon {
        place: WeaponPlace::Model(place),
        main: Hand::Right,
        main_grip: Some(grip(-0.1, 1.0)),
        off_grip: Some(grip(0.1, 1.0)),
    };
    holding(&mut pose, &mut ik, weapon);
    for (hand, x) in [(RIGHT_HAND, -0.1), (LEFT_HAND, 0.1)] {
        let wanted = place.transform_point3(Vec3::new(x, 0.0, 0.0));
        let got = Vec3::from(palm_of(&pose, hand).translation);
        assert!(got.distance(wanted) < 3e-3, "{hand}: {got} against {wanted}");
    }
    let elbow = Vec3::from(pose.model()[LEFT_LOWER_ARM].translation);
    let (shoulder, hand) = (
        Vec3::from(pose.model()[LEFT_UPPER_ARM].translation),
        Vec3::from(pose.model()[LEFT_HAND].translation),
    );
    assert!(elbow.y < shoulder.lerp(hand, 0.5).y, "the pole takes the elbow down: {elbow}");
    // The rig's helper, which copies the forearm's turn from rest, follows the solve.
    let forearm = pose.locals()[LEFT_LOWER_ARM].rotation;
    let helper = pose.locals()[HELPER].rotation;
    assert!(helper.angle_between(forearm) < 1e-4, "{helper} against {forearm}");
    let released = HeldWeapon { main_grip: Some(grip(-0.1, 0.0)), off_grip: Some(grip(0.1, 0.0)), ..weapon };
    holding(&mut pose, &mut ik, released);
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose), "at weight 0 the arms stay as animated");
}

/// Grips aren't smoothed, so moving one takes effect with no time passing, and has a key of its own.
#[test]
fn a_grip_moving_under_a_frozen_clock_has_its_own_key() {
    let at = |x: f32| HeldWeapon {
        place: WeaponPlace::Model(Affine3A::from_translation(Vec3::new(x, 1.2, 0.3))),
        main: Hand::Right,
        main_grip: None,
        off_grip: Some(Grip { on_weapon: Transform::IDENTITY, in_hand: palm(), pole: None, weight: 1.0 }),
    };
    let request = |weapon| IkInput { dt: 0.0, weapon: Some(weapon), ..input() };
    assert_eq!(request(at(0.0)).frozen_key(), request(at(0.0)).frozen_key());
    assert_ne!(request(at(0.0)).frozen_key(), request(at(0.1)).frozen_key());
    let moving_look = IkInput { look: LookInput { target: Some(Vec3::X), weight: 1.0 }, ..request(at(0.0)) };
    assert_eq!(request(at(0.0)).frozen_key(), moving_look.frozen_key(), "a look target isn't in it");
    let (mut pose, mut ik) = character();
    holding(&mut pose, &mut ik, at(0.0));
    let first = palm_of(&pose, LEFT_HAND);
    holding(&mut pose, &mut ik, at(0.1));
    let second = palm_of(&pose, LEFT_HAND);
    assert!(first.translation.distance(second.translation) > 0.05);
}

// Everything together.

#[test]
fn a_rig_without_some_bones_does_what_it_can() {
    let mut pose = pose();
    let few = [(HumanBone::Head, HEAD), (HumanBone::Hips, HIPS)];
    let rig = IkRig::new(&pose, &few, Vec3::ZERO, None, 0.0);
    let mut ik = CharacterIk::new(rig, FootSettings::default(), LookSettings::default());
    assert!(ik.feet().is_none());
    pose.reset();
    ik.prepare(&mut pose);
    assert!(ik.foot_probes(&mut pose, &Affine3A::IDENTITY).is_none());
    let mut request = input();
    request.look = LookInput { target: Some(Vec3::new(1.0, 1.5, 3.0)), weight: 1.0 };
    for _ in 0..120 {
        pose.reset();
        ik.prepare(&mut pose);
        ik.solve(&mut pose, &request);
        pose.finish();
    }
    let forward = head_forward(&pose);
    let eyes = Vec3::from(pose.model()[HEAD].translation);
    let wanted = (Vec3::new(1.0, 1.5, 3.0) - eyes).normalize();
    assert!(forward.angle_between(wanted) < 1.0f32.to_radians(), "{forward} against {wanted}");
    // A rig with no head, nor anything else, has nothing to do.
    let mut bare = self::pose();
    let rig = IkRig::new(&bare, &[], Vec3::ZERO, None, 0.0);
    let mut none = CharacterIk::new(rig, FootSettings::default(), LookSettings::default());
    bare.reset();
    none.prepare(&mut bare);
    none.solve(&mut bare, &request);
    bare.finish();
    let solved = bits(&bare);
    bare.reset();
    bare.finish();
    assert_eq!(solved, bits(&bare));
}

/// The same state and pose give the same bytes, with the character anywhere and turned any way.
#[test]
fn a_turned_and_moved_character_stands_on_the_world_s_ground_and_looks_at_the_world_s_target() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::from_scale_rotation_translation(
        Vec3::splat(0.8),
        Quat::from_rotation_y(1.1),
        Vec3::new(4.0, 2.0, -3.0),
    );
    let angle = 20f32.to_radians();
    let normal = Vec3::new(-angle.sin(), angle.cos(), 0.0);
    let ground = |probe: Vec3| Some(on_plane(probe, Vec3::new(4.0, 2.0, -3.0), normal));
    let target = Vec3::new(8.0, 3.2, 0.0);
    let run = |pose: &mut CharacterPose, ik: &mut CharacterIk| {
        pose.reset();
        ik.prepare(pose);
        let probes = ik.foot_probes(pose, &world).expect("it has feet");
        let foot =
            |probe: &FootProbe| FootGround { ankle: ground(probe.ankle), ball: probe.ball.and_then(ground) };
        let mut request = input();
        request.world_from_model = world;
        request.feet = FeetInput { weight: 1.0, ground: [foot(&probes[0]), foot(&probes[1])] };
        request.look = LookInput { target: Some(target), weight: 1.0 };
        ik.solve(pose, &request);
        pose.finish();
    };
    for _ in 0..120 {
        run(&mut pose, &mut ik);
    }
    for foot in [LEFT_FOOT, RIGHT_FOOT] {
        let at = world_position(&pose, world, foot);
        let gap = (at - Vec3::new(4.0, 2.0, -3.0)).dot(normal) - ANKLE * 0.8;
        assert!(gap.abs() < 2e-3, "{foot}: {gap}");
    }
    let (_, turn, _) = (world * pose.model()[HEAD]).to_scale_rotation_translation();
    let forward = turn * pose.rest_rotation(HEAD).inverse() * Vec3::Z;
    let eyes = world.transform_point3(pose.model()[HEAD].transform_point3(Vec3::new(0.05, 0.03, 0.0)));
    assert!(forward.angle_between((target - eyes).normalize()) < 2f32.to_radians());
}

/// Edge cases, as numbers: where a solve must stay finite, and the shape of what it does at the ends.
#[test]
fn ik_edge_cases_snapshot() {
    let (mut pose, mut ik) = character();
    let mut lines = Vec::new();
    let mut record = |name: &str, pose: &CharacterPose| {
        let at = |joint: usize| Vec3::from(pose.model()[joint].translation);
        // Adding zero takes the sign off a rounded -0.
        let round = |v: Vec3| v.to_array().map(|value| (value * 1000.0).round() / 1000.0 + 0.0);
        lines.push(format!(
            "{name}: hips {:?} left foot {:?} right foot {:?} head {:?}",
            round(at(HIPS)),
            round(at(LEFT_FOOT)),
            round(at(RIGHT_FOOT)),
            round(head_forward(pose)),
        ));
    };
    step(&mut pose, &mut ik, Affine3A::IDENTITY, 1.0, &slope(35f32.to_radians()));
    record("a 35 degree slope", &pose);
    let (mut pose, mut ik) = character();
    step(&mut pose, &mut ik, Affine3A::IDENTITY, 1.0, &slope(80f32.to_radians()));
    record("too steep to stand on", &pose);
    let (mut pose, mut ik) = character();
    let far = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -5.0, probe.z), normal: Vec3::Y });
    step(&mut pose, &mut ik, Affine3A::IDENTITY, 1.0, &far);
    record("ground out of every reach", &pose);
    let (mut pose, mut ik) = character();
    let on_the_eyes = pose.model()[HEAD].transform_point3(Vec3::new(0.05, 0.03, 0.0));
    looking(&mut pose, &mut ik, Some(on_the_eyes), 1.0, TICK);
    record("a target on the eyes", &pose);
    let (mut pose, mut ik) = character();
    settle(&mut pose, &mut ik, Vec3::new(0.0, 50.0, 0.0));
    record("straight overhead", &pose);
    let (mut pose, mut ik) = character();
    settle(&mut pose, &mut ik, Vec3::new(0.0, 1.5, -5.0));
    record("dead behind", &pose);
    insta::assert_snapshot!(lines.join("\n"));
}

/// A body on the floor has no feet to plant and nothing to look at, and it needs no controls to say
/// so: both fade with the hips, whatever a caller asks for.
#[test]
fn a_body_lying_down_plants_no_feet_and_looks_at_nothing() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let raised = |probe: Vec3| Some(on_plane(probe, Vec3::new(0.0, 0.1, 0.0), Vec3::Y));
    let solve = |pose: &mut CharacterPose, ik: &mut CharacterIk, hips: f32| {
        pose.reset();
        pose.locals_mut()[HIPS].translation.y = hips;
        ik.prepare(pose);
        let mut asked = input();
        asked.dt = 1.0 / 60.0;
        asked.look = LookInput { target: Some(Vec3::new(3.0, 1.0, 0.5)), weight: 1.0 };
        asked.feet.weight = 1.0;
        if let Some(probes) = ik.foot_probes(pose, &world) {
            for (ground, probe) in asked.feet.ground.iter_mut().zip(probes) {
                *ground = FootGround { ankle: raised(probe.ankle), ball: probe.ball.and_then(raised) };
            }
        }
        ik.solve(pose, &asked);
        pose.finish();
    };
    // Lying down, for long enough that any fade has run its course.
    for _ in 0..240 {
        solve(&mut pose, &mut ik, 0.1);
    }
    let lying = bits(&pose);
    pose.reset();
    pose.locals_mut()[HIPS].translation.y = 0.1;
    pose.finish();
    assert_eq!(lying, bits(&pose), "as the clip has it, to the bit");

    // Stood up, the same ask moves the feet and turns the head again.
    for _ in 0..240 {
        solve(&mut pose, &mut ik, 0.9);
    }
    let foot = world_position(&pose, world, LEFT_FOOT).y;
    assert!(foot > ANKLE + 0.05, "the foot stands on the raised ground: {foot}");
    assert!(head_forward(&pose).x > 0.1, "and the head looks: {}", head_forward(&pose));
}

/// The hips ease more slowly than the fade goes, so they still have some way to come when it ends, and
/// they come the rest of the way rather than jumping there.
#[test]
fn the_hips_come_home_after_a_fade_with_no_jump() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ground = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -0.18, probe.z), normal: Vec3::Y });
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 1.0, &ground);
    }
    let settled = pose.model()[HIPS].translation.y;
    assert!(settled < 0.75, "they went down with the feet: {settled}");
    let mut hips = settled;
    let mut foot = world_position(&pose, world, LEFT_FOOT);
    for tick in 0..60 {
        step(&mut pose, &mut ik, world, 0.0, &ground);
        let now = pose.model()[HIPS].translation.y;
        assert!((now - hips).abs() < 0.02, "the hips jumped {} m on tick {tick}", now - hips);
        hips = now;
        let now = world_position(&pose, world, LEFT_FOOT);
        assert!(now.distance(foot) < 0.03, "the foot jumped {} m on tick {tick}", now.distance(foot));
        foot = now;
    }
    assert!(ik.feet().expect("it has feet").is_settled());
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose), "and it ends as no solve at all");
}

/// A driver skips a character that says it's settled, which mustn't strand the hips on their way home.
#[test]
fn hips_on_their_way_home_are_not_settled() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ground = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -0.18, probe.z), normal: Vec3::Y });
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 1.0, &ground);
    }
    let feet = |ik: &CharacterIk| ik.feet().expect("it has feet").clone();
    while feet(&ik).fade() > 0.0 {
        step(&mut pose, &mut ik, world, 0.0, &ground);
    }
    assert!(!feet(&ik).is_settled(), "the fade has ended, and the hips haven't");
}

/// Feet coming back while the hips are still on their way home don't pull the hips anywhere at once.
#[test]
fn feet_fading_back_in_as_the_hips_come_home_ease_them() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ground = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -0.18, probe.z), normal: Vec3::Y });
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 1.0, &ground);
    }
    while ik.feet().expect("it has feet").fade() > 0.0 {
        step(&mut pose, &mut ik, world, 0.0, &ground);
    }
    let mut hips = pose.model()[HIPS].translation.y;
    assert!(hips < 0.89, "they're still on their way: {hips}");
    for tick in 0..30 {
        step(&mut pose, &mut ik, world, 1.0, &ground);
        let now = pose.model()[HIPS].translation.y;
        assert!((now - hips).abs() < 0.03, "the hips jumped {} m on tick {tick}", now - hips);
        hips = now;
    }
}

/// A cut forgets the hips' way home with everything else about the ground.
#[test]
fn a_cut_ends_the_hips_way_home() {
    let (mut pose, mut ik) = character();
    let world = Affine3A::IDENTITY;
    let ground = |probe: Vec3| Some(GroundHit { point: Vec3::new(probe.x, -0.18, probe.z), normal: Vec3::Y });
    for _ in 0..60 {
        step(&mut pose, &mut ik, world, 1.0, &ground);
    }
    while ik.feet().expect("it has feet").fade() > 0.0 {
        step(&mut pose, &mut ik, world, 0.0, &ground);
    }
    ik.reset();
    step(&mut pose, &mut ik, world, 0.0, &ground);
    let solved = bits(&pose);
    pose.reset();
    pose.finish();
    assert_eq!(solved, bits(&pose), "off after a cut is no solve at all");
    assert!(ik.feet().expect("it has feet").is_settled());
}
