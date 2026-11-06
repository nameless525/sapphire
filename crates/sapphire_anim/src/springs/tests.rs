use sapphire_assets::rig::{
    CharacterRig, Collider as RigCollider, ColliderGroup, ColliderShape, Spring, SpringJoint, SpringTip,
};
use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};

use super::*;

const STRAND: [u16; 5] = [3, 4, 5, 6, 7];
const ARM: usize = 8;
const TIP: usize = 7;

fn joint(name: &str, parent: Option<u16>, translation: [f32; 3]) -> Joint {
    Joint { name: name.to_owned(), parent, rest: JointTransform { translation, ..JointTransform::IDENTITY } }
}

/// A root, a chest, a head with a strand 40 cm long hanging down its back, and an arm beside the chest.
fn joints() -> Vec<Joint> {
    vec![
        joint("root", None, [0.0; 3]),
        joint("chest", Some(0), [0.0, 1.0, 0.0]),
        joint("head", Some(1), [0.0, 0.4, 0.0]),
        joint("strand 1", Some(2), [0.0, 0.0, -0.1]),
        joint("strand 2", Some(3), [0.0, -0.1, 0.0]),
        joint("strand 3", Some(4), [0.0, -0.1, 0.0]),
        joint("strand 4", Some(5), [0.0, -0.1, 0.0]),
        joint("strand end", Some(6), [0.0, -0.1, 0.0]),
        joint("arm", Some(1), [0.3, 0.0, 0.0]),
    ]
}

fn spring_joint(joint: u16, stiffness: f32, drag: f32, gravity: f32) -> SpringJoint {
    SpringJoint {
        joint,
        hit_radius: 0.01,
        stiffness,
        gravity_power: gravity,
        gravity_dir: [0.0, -1.0, 0.0],
        drag_force: drag,
    }
}

fn strand(stiffness: f32, drag: f32, gravity: f32) -> Spring {
    Spring {
        name: "strand".into(),
        joints: STRAND.iter().map(|&joint| spring_joint(joint, stiffness, drag, gravity)).collect(),
        tip: SpringTip::Joint,
        collider_groups: Vec::new(),
        center: None,
    }
}

fn rig(springs: Vec<Spring>) -> CharacterRig {
    CharacterRig { joint_count: joints().len() as u32, springs, ..CharacterRig::default() }
}

/// The hair the character ships with, near enough.
fn hair() -> CharacterRig {
    rig(vec![strand(0.5, 0.4, 0.1)])
}

/// What the animation does over time.
type Motion = fn(f64) -> Posed;

/// What a test's motion makes of a tick.
#[derive(Clone, Copy, Debug)]
struct Posed {
    root: Vec3,
    /// How the whole character is turned.
    turn: Quat,
    chest: Quat,
    head: Quat,
    /// Where the arm is on the chest.
    arm: Vec3,
}

impl Default for Posed {
    fn default() -> Self {
        Self {
            root: Vec3::ZERO,
            turn: Quat::IDENTITY,
            chest: Quat::IDENTITY,
            head: Quat::IDENTITY,
            arm: Vec3::new(0.3, 0.0, 0.0),
        }
    }
}

fn still(_: f64) -> Posed {
    Posed::default()
}

/// Swaying and turning from a standstill, at about a walk's pace.
fn sway(ticks: f64) -> Posed {
    let seconds = ticks as f32 / 60.0;
    let wave = |hz: f32| (seconds * hz * core::f32::consts::TAU).sin();
    let ease = |hz: f32| 1.0 - (seconds * hz * core::f32::consts::TAU).cos();
    Posed {
        root: Vec3::new(ease(0.7) * 0.25, 0.0, ease(0.35) * 0.3),
        chest: Quat::from_rotation_y(wave(1.1) * 0.5),
        head: Quat::from_rotation_x(wave(1.7) * 0.2),
        ..Posed::default()
    }
}

/// A strike, as fast as the character's own: still, then a lunge of 35 cm and a turn of the body by a
/// third of a radian over 6 ticks, then still.
fn strike(ticks: f64) -> Posed {
    let u = ((ticks as f32 - 30.0) / 6.0).clamp(0.0, 1.0);
    let eased = u * u * (3.0 - 2.0 * u);
    Posed {
        root: Vec3::new(0.0, 0.0, eased * 0.35),
        chest: Quat::from_rotation_y(eased * 0.6),
        head: Quat::from_rotation_y(eased * -0.2),
        ..Posed::default()
    }
}

/// A character and its springs, driven as a frame drives them.
struct Character {
    pose: CharacterPose,
    springs: SpringBones,
    world: Affine3A,
}

impl Character {
    fn new(rig: &CharacterRig, substeps: Substeps) -> Self {
        let bytes = rig.to_cooked().expect("cooked");
        let rig = CharacterRig::read(&bytes).expect("a valid rig");
        let skeleton_bytes = Skeleton::new(&joints()).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
        let mut springs = SpringBones::new(rig, &pose).expect("it has springs");
        springs.set_substeps(substeps);
        // VRM's own, and the most these tests can ask of them.
        springs.set_tuning(SpringTuning::VRM);
        Self { pose, springs, world: Affine3A::IDENTITY }
    }

    fn set_pose(&mut self, posed: Posed) {
        self.pose.reset();
        let locals = self.pose.locals_mut();
        locals[1].rotation = posed.chest;
        locals[2].rotation = posed.head;
        locals[ARM].translation = posed.arm;
        self.pose.finish();
        self.world = Affine3A::from_rotation_translation(posed.turn, posed.root);
    }

    /// A frame at `ticks` of `motion`, with its pose evaluated.
    fn frame(&mut self, motion: Motion, ticks: f64) -> SpringWork {
        self.set_pose(motion(ticks));
        let frame = self.frame_at(ticks, true, true);
        self.springs.update(&mut self.pose, &frame)
    }

    fn frame_at(&self, ticks: f64, pose_moved: bool, active: bool) -> SpringFrame {
        SpringFrame { world_from_model: self.world, ticks, pose_moved, active }
    }

    /// Where `joint` is in the world.
    fn world_of(&self, joint: usize) -> Vec3 {
        self.world.transform_point3(Vec3::from(self.pose.model()[joint].translation))
    }

    /// Where `joint` would be at its parent's place with the chain as the pose has it, in the world:
    /// the strand hanging straight from wherever the head is.
    fn posed_tip(&self) -> Vec3 {
        let head = self.pose.model()[2];
        self.world.transform_point3(head.transform_point3(Vec3::new(0.0, -0.4, -0.1)))
    }

    /// Every output, as bits.
    fn bits(&self) -> Vec<u32> {
        let mut bits = Vec::new();
        for local in self.pose.locals() {
            bits.extend(local.rotation.to_array().map(f32::to_bits));
        }
        for model in self.pose.model() {
            bits.extend(model.to_cols_array().map(f32::to_bits));
        }
        bits
    }
}

/// `motion` for `seconds` at `hz`, from a start everyone shares: the tip's place in the world at every
/// 30th of a second, which every rate here draws.
fn tip_path(rig: &CharacterRig, substeps: Substeps, hz: u32, motion: Motion, seconds: u32) -> Vec<Vec3> {
    let mut character = Character::new(rig, substeps);
    let mut path = Vec::new();
    for frame in 0..=seconds * hz {
        let ticks = f64::from(frame) * 60.0 / f64::from(hz);
        character.frame(motion, ticks);
        if frame % (hz / 30) == 0 {
            path.push(character.world_of(TIP));
        }
    }
    path
}

fn furthest_apart(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// The promise the module docs make: the frame rate doesn't change the result, beyond the coarser
/// path a chain's root takes between bigger frames. For a 40 cm strand, at each sub-step count that
/// ships, the tip stays within these of where the 120 Hz run has it, at every 30th of a second.
#[test]
fn springs_at_30_60_and_120_hz() {
    // A swaying walk: 2.5 mm at 30 Hz, 1 mm at 60 Hz. A strike: 25 mm at 30 Hz, 8 mm at 60 Hz, where a
    // lunge of 35 cm in 6 ticks is only 4 frames at 30 Hz.
    let tolerances: [(Motion, f32, f32); 2] = [(sway, 0.0025, 0.001), (strike, 0.025, 0.008)];
    for (motion, at_30, at_60) in tolerances {
        for substeps in [Substeps::One, Substeps::Two] {
            let reference = tip_path(&hair(), substeps, 120, motion, 3);
            let a = furthest_apart(&tip_path(&hair(), substeps, 30, motion, 3), &reference);
            let b = furthest_apart(&tip_path(&hair(), substeps, 60, motion, 3), &reference);
            assert!(a <= at_30, "{substeps:?}: 30 Hz is {a} m from 120 Hz");
            assert!(b <= at_60, "{substeps:?}: 60 Hz is {b} m from 120 Hz");
        }
    }
}

/// A character standing in a bare pose converges on its rest and stays there, with no jitter, at every
/// sub-step count. Then it stops: updates write nothing, so the character isn't skinned again.
#[test]
fn a_still_character_settles_without_jitter_and_stops() {
    for substeps in [Substeps::One, Substeps::Two, Substeps::Four] {
        for (stiffness, drag, gravity) in [(0.5, 0.4, 0.1), (0.5, 0.05, 0.0), (1.0, 0.5, 2.0)] {
            let mut character = Character::new(&rig(vec![strand(stiffness, drag, gravity)]), substeps);
            // Set swinging first.
            for tick in 0..60 {
                character.frame(sway, f64::from(tick));
            }
            let mut ticks = 60.0;
            character.set_pose(still(0.0));
            let mut stopped = None;
            let mut last = character.world_of(TIP);
            for tick in 0..1_200 {
                ticks += 1.0;
                let frame = character.frame_at(ticks, tick == 0, true);
                let work = character.springs.update(&mut character.pose, &frame);
                let tip = character.world_of(TIP);
                if tick > 600 {
                    let moved = tip.distance(last);
                    assert!(moved < 2e-4, "{substeps:?} still moving {moved} m a tick at {tick}");
                }
                last = tip;
                if !work.wrote_pose {
                    stopped = Some(tick);
                    break;
                }
            }
            let stopped = stopped.unwrap_or_else(|| panic!("{substeps:?} {stiffness} {drag}: never stopped"));
            assert!(character.springs.is_resting());
            assert!(stopped > 30, "it ran until the tails had been still a while, not {stopped}");
            // And stays stopped, to the bit, however time goes on.
            let bits = character.bits();
            for _ in 0..30 {
                ticks += 1.0;
                let frame = character.frame_at(ticks, false, true);
                assert!(!character.springs.is_due(frame.ticks, &frame.world_from_model));
                assert_eq!(character.springs.update(&mut character.pose, &frame), SpringWork::default());
                assert_eq!(character.bits(), bits);
            }
            // Until something moves it.
            let moved = character.frame(strike, 40.0);
            assert!(moved.wrote_pose);
            assert!(!character.springs.is_resting());
        }
    }
}

/// With no force but stiffness, the strand hangs as the pose has it: nothing moves it off the pose.
#[test]
fn a_chain_with_no_gravity_rests_where_the_pose_puts_it() {
    let mut character = Character::new(&rig(vec![strand(0.5, 0.4, 0.0)]), Substeps::Two);
    for tick in 0..300 {
        character.frame(still, f64::from(tick));
    }
    let tip = character.world_of(TIP);
    assert!(tip.abs_diff_eq(character.posed_tip(), 1e-5), "{tip} against {}", character.posed_tip());
}

/// One sub-step a tick is VRM's own step at 60 Hz: checked against the specification's formula, written
/// out by hand, for a single joint swinging behind a moving parent.
#[test]
fn one_substep_a_tick_is_vrms_step_at_60_hz() {
    let (stiffness, drag, gravity) = (0.5, 0.4, 0.3);
    let one = Spring {
        joints: vec![spring_joint(3, stiffness, drag, gravity), spring_joint(4, 1.0, 0.5, 0.0)],
        ..strand(0.0, 0.0, 0.0)
    };
    let mut character = Character::new(&rig(vec![one]), Substeps::One);
    fn walk(ticks: f64) -> Posed {
        Posed { root: Vec3::new((ticks as f32 / 20.0).sin() * 0.3, 0.0, 0.0), ..Posed::default() }
    }
    character.frame(walk, 0.0);
    let head_at = |ticks: f64| walk(ticks).root + Vec3::new(0.0, 1.4, -0.1);
    let mut tail = head_at(0.0) + Vec3::new(0.0, -0.1, 0.0);
    let mut before = tail;
    let dt = 1.0 / 60.0;
    for tick in 1..120 {
        let ticks = f64::from(tick);
        character.frame(walk, ticks);
        // VRMC_springBone-1.0's update.
        let head = head_at(ticks);
        let next = tail
            + (tail - before) * (1.0 - drag)
            + Vec3::NEG_Y * (stiffness * dt)
            + Vec3::NEG_Y * (gravity * dt);
        let next = head + (next - head).normalize() * 0.1;
        (before, tail) = (tail, next);
        // Its sub-step is the frame's; drawing lags one sub-step, so read the state, not the draw.
        let ours = character.springs.tails[0].now + character.springs.origin;
        assert!(ours.abs_diff_eq(tail, 2e-5), "tick {tick}: {ours} against the formula's {tail}");
    }
}

/// A sub-step count only changes how finely the same thing is computed, so a tier's hair moves as
/// another's does. The tip stays this near where one sub-step a tick has it, through a walk and a strike.
#[test]
fn every_substep_count_makes_about_the_same_motion() {
    let cases: [(&str, CharacterRig, Motion, f32, f32); 4] = [
        ("hair, a walk", hair(), sway, 0.035, 0.05),
        ("hair, a strike", hair(), strike, 0.07, 0.10),
        ("cloth, a walk", rig(vec![strand(0.5, 0.05, 0.0)]), sway, 0.05, 0.07),
        ("cloth, a strike", rig(vec![strand(0.5, 0.05, 0.0)]), strike, 0.12, 0.22),
    ];
    for (name, rig, motion, at_two, at_four) in cases {
        let one = tip_path(&rig, Substeps::One, 60, motion, 3);
        let two = furthest_apart(&tip_path(&rig, Substeps::Two, 60, motion, 3), &one);
        let four = furthest_apart(&tip_path(&rig, Substeps::Four, 60, motion, 3), &one);
        assert!(two <= at_two, "{name}: two sub-steps are {two} m from one");
        assert!(four <= at_four, "{name}: four sub-steps are {four} m from one");
    }
}

/// Hair trails the body: under a sudden step the tip moves away from the pose, and comes back.
#[test]
fn the_tip_lags_behind_the_body_and_comes_back() {
    let mut character = Character::new(&hair(), Substeps::Two);
    let mut most = 0.0_f32;
    for tick in 0..240 {
        character.frame(strike, f64::from(tick));
        let off = character.world_of(TIP).distance(character.posed_tip());
        if tick < 30 {
            assert!(off < 0.02, "still before the strike, {off} m off at {tick}");
        }
        most = most.max(off);
    }
    assert!(most > 0.1, "it swung: {most} m at most");
    let back = character.world_of(TIP).distance(character.posed_tip());
    assert!(back < 0.02, "and came back to {back} m");
}

/// A teleport with a reset doesn't move the hair: it stands as the pose has it, in the new place, and
/// nothing stored from before moves it. Without one the hair is left pointing back across the world,
/// which is what the reset is for.
#[test]
fn a_reset_settles_from_the_pose_with_no_whip() {
    fn walk(ticks: f64) -> Posed {
        Posed { root: Vec3::new(0.0, 0.0, ticks as f32 * 0.03), ..Posed::default() }
    }
    fn elsewhere(_: f64) -> Posed {
        Posed { root: Vec3::new(500.0, 0.0, 0.0), ..Posed::default() }
    }
    let run = |reset: bool| {
        let mut character = Character::new(&hair(), Substeps::Two);
        for tick in 0..60 {
            character.frame(walk, f64::from(tick));
        }
        if reset {
            character.springs.reset();
        }
        character.frame(elsewhere, 60.0);
        let first = character.world_of(TIP).distance(character.posed_tip());
        let mut most = first;
        for tick in 61..90 {
            character.frame(elsewhere, f64::from(tick));
            most = most.max(character.world_of(TIP).distance(character.posed_tip()));
        }
        (first, most)
    };
    let (first, most) = run(true);
    assert!(first < 1e-4, "with a reset it stands on the pose: {first} m off");
    assert!(most < 1e-3, "and stays: {most} m off");
    let (_, most) = run(false);
    assert!(most > 0.3, "without one it is flung right away: {most} m off");
}

/// A body that bobs a long way fast and sways as it does, as a run's does.
fn a_bob(ticks: f64) -> Posed {
    let phase = ticks as f32 * std::f32::consts::TAU / 24.0;
    Posed { root: Vec3::new(phase.cos() * 0.1, phase.sin() * 0.15, 0.0), ..Posed::default() }
}

/// The worst any joint of the strand comes off the pose, which hangs it straight down, and bends from
/// the joint above it.
fn worst_shape(tuning: SpringTuning, substeps: Substeps, motion: Motion) -> (f32, f32) {
    let mut character = Character::new(&hair(), substeps);
    character.springs.set_tuning(tuning);
    let (mut swing, mut bend) = (0.0_f32, 0.0_f32);
    for tick in 0..240 {
        character.frame(motion, f64::from(tick));
        let mut above = None;
        for pair in STRAND.windows(2) {
            let ours = (character.world_of(usize::from(pair[1])) - character.world_of(usize::from(pair[0])))
                .normalize();
            swing = swing.max(ours.angle_between(Vec3::NEG_Y));
            if let Some(above) = above {
                bend = bend.max(ours.angle_between(above));
            }
            above = Some(ours);
        }
    }
    (swing, bend)
}

/// VRM's step lets a joint go anywhere. Tuned, none leaves its cones at any sub-step count.
#[test]
fn the_limits_keep_a_strand_in_shape() {
    let (free_swing, free_bend) = worst_shape(SpringTuning::VRM, Substeps::Two, a_bob);
    assert!(free_swing > 1.5 && free_bend > 1.0, "VRM's step: {free_swing} and {free_bend} rad");
    let tuning = SpringTuning { root_inertia: 1.0, anchor_inertia: 1.0, ..SpringTuning::ACTION };
    for substeps in [Substeps::One, Substeps::Two, Substeps::Four] {
        let (swing, bend) = worst_shape(tuning, substeps, a_bob);
        assert!(swing <= tuning.swing_limit + 0.02, "{substeps:?}: {swing} rad off the pose");
        assert!(bend <= tuning.bend_limit + 0.02, "{substeps:?}: bends {bend} rad");
    }
}

/// The share of the motion of the joint a chain hangs from that its tails feel: with none it moves as
/// a rigid part of that joint, with all of it as VRM has it, and between the two it swings between them.
#[test]
fn the_anchor_inertia_is_the_share_of_the_anchors_motion_chains_feel() {
    fn chest_bob(ticks: f64) -> Posed {
        let turn = (ticks as f32 * std::f32::consts::TAU / 24.0).sin() * 0.6;
        Posed { chest: Quat::from_rotation_x(turn), ..Posed::default() }
    }
    let most = |share: f32| {
        // No gravity, whose droop is off the pose whatever carries the chain.
        let mut character = Character::new(&rig(vec![strand(0.5, 0.4, 0.0)]), Substeps::Two);
        character.springs.set_tuning(SpringTuning { anchor_inertia: share, ..SpringTuning::VRM });
        let mut most = 0.0_f32;
        for tick in 0..120 {
            character.frame(chest_bob, f64::from(tick));
            most = most.max(character.world_of(TIP).distance(character.posed_tip()));
        }
        most
    };
    let (none, some, all) = (most(0.0), most(SpringTuning::ACTION.anchor_inertia), most(1.0));
    assert!(none < 1e-3, "with none it is carried with the pose: {none} m off");
    assert!(some > 0.03, "with some it swings: {some} m");
    assert!(all > some * 1.2, "and with all it swings more: {all} m against {some}");
}

/// The tuned step holds what the clock promises: 30, 60 and 120 Hz draw about the same hair.
#[test]
fn the_tuning_keeps_to_the_rates() {
    for (motion, at_30, at_60) in
        [(sway as Motion, 0.0025, 0.001), (strike, 0.025, 0.008), (a_bob, 0.025, 0.01)]
    {
        let path = |hz| {
            let mut character = Character::new(&hair(), Substeps::Two);
            character.springs.set_tuning(SpringTuning::ACTION);
            let mut path = Vec::new();
            for frame in 0..=3 * hz {
                let ticks = f64::from(frame) * f64::from(SPRING_TICK_HZ) / f64::from(hz);
                character.frame(motion, ticks);
                if frame % (hz / 30) == 0 {
                    path.push(character.world_of(TIP));
                }
            }
            path
        };
        let reference = path(120);
        let (thirty, sixty) = (furthest_apart(&path(30), &reference), furthest_apart(&path(60), &reference));
        assert!(thirty <= at_30, "30 Hz is {thirty} m from 120");
        assert!(sixty <= at_60, "60 Hz is {sixty} m from 120");
    }
}

/// The root inertia is the share of the character's own motion its chains feel. Told none, its travel
/// and turning at any speed leave hair as the pose has it. Told all, as VRM has it, a lunge flings it.
#[test]
fn the_root_inertia_is_the_share_of_the_characters_motion_chains_feel() {
    fn lunge(ticks: f64) -> Posed {
        let u = ((ticks as f32 - 10.0) / 6.0).clamp(0.0, 1.0);
        Posed { root: Vec3::new(0.0, 0.0, u * 0.5), ..Posed::default() }
    }
    fn turn(ticks: f64) -> Posed {
        let u = ((ticks as f32 - 10.0) / 6.0).clamp(0.0, 1.0);
        Posed { turn: Quat::from_rotation_y(u * 3.0), ..Posed::default() }
    }
    for motion in [lunge as Motion, turn] {
        let most = |share: f32| {
            let mut character = Character::new(&hair(), Substeps::Two);
            character.springs.set_tuning(SpringTuning { root_inertia: share, ..SpringTuning::VRM });
            let mut most = 0.0_f32;
            for tick in 0..60 {
                character.frame(motion, f64::from(tick));
                most = most.max(character.world_of(TIP).distance(character.posed_tip()));
            }
            most
        };
        let (none, some, all) = (most(0.0), most(SpringTuning::ACTION.root_inertia), most(1.0));
        assert!(none < 5e-4, "with none it hangs as the pose has it: {none} m off");
        assert!(some > 0.03, "with some it swings: {some} m");
        assert!(all > 2.0 * some, "and with all it swings much more: {all} m against {some}");
    }
}

/// A clock that stands still holds the springs to the bit, and they carry on as if it never stopped.
#[test]
fn a_frozen_clock_holds_to_the_bit_and_carries_on() {
    for substeps in [Substeps::One, Substeps::Two] {
        let mut stopping = Character::new(&hair(), substeps);
        let mut running = Character::new(&hair(), substeps);
        for tick in 0..40 {
            stopping.frame(strike, f64::from(tick) + 0.25);
            running.frame(strike, f64::from(tick) + 0.25);
        }
        let held = stopping.bits();
        let tails = stopping.springs.tails.clone();
        for _ in 0..12 {
            // Whether or not the frame evaluates the pose again, as a graph may on a frozen clock.
            let frame = stopping.frame_at(39.25, false, true);
            assert!(!stopping.springs.is_due(frame.ticks, &frame.world_from_model));
            assert_eq!(stopping.springs.update(&mut stopping.pose, &frame), SpringWork::default());
            assert_eq!(stopping.bits(), held);
            let work = stopping.frame(strike, 39.25);
            assert_eq!(work.substeps, 0);
            assert_eq!(stopping.bits(), held, "the same pose evaluated again draws the same");
            assert_eq!(stopping.springs.tails, tails);
        }
        // Past the freeze, frames come at the same times as the other's: no pop, and no difference.
        for tick in 40..70 {
            stopping.frame(strike, f64::from(tick) + 0.25);
            running.frame(strike, f64::from(tick) + 0.25);
            assert_eq!(stopping.bits(), running.bits(), "{substeps:?} at {tick}");
        }
    }
}

/// Slow motion is the same path, slower: a fifth of a tick a frame ends where a whole tick a frame
/// does, within what frame rate alone changes.
#[test]
fn slow_motion_takes_the_same_path_slowly() {
    let mut slowed = Character::new(&hair(), Substeps::Two);
    let mut full = Character::new(&hair(), Substeps::Two);
    for frame in 0..=900 {
        let ticks = f64::from(frame) / 5.0;
        slowed.frame(strike, ticks);
        if frame % 5 == 0 {
            full.frame(strike, ticks);
            let apart = slowed.world_of(TIP).distance(full.world_of(TIP));
            assert!(apart < 0.008, "{apart} m apart at tick {ticks}");
        }
    }
}

/// Between sub-steps a frame draws an interpolated turn, so frames between two sub-steps move smoothly
/// on and never jump: at 240 Hz with one sub-step a tick, three of every four frames run no sub-step,
/// and the tip's path through them is as smooth as the path of the sub-steps themselves.
#[test]
fn frames_between_substeps_move_smoothly() {
    let mut character = Character::new(&hair(), Substeps::One);
    let mut fine = Vec::new();
    for frame in 0..=4 * 120 {
        let work = character.frame(sway, f64::from(frame) / 4.0);
        if frame % 4 != 0 {
            assert_eq!(work.substeps, 0, "frame {frame}");
        }
        // Past the first second's start from nothing.
        if frame > 4 * 60 {
            fine.push(character.world_of(TIP));
        }
    }
    let steps: Vec<Vec3> = fine.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let bend = steps.windows(2).map(|pair| (pair[1] - pair[0]).length()).fold(0.0, f32::max);
    let speed = steps.iter().map(|step| step.length()).fold(0.0, f32::max);
    // Drawing only what a sub-step left, 3 frames in 4 wouldn't move at all, and the fourth would move
    // as far as all of them: a bend as big as the biggest step.
    assert!(bend < speed * 0.35, "a bend of {bend} m against steps of up to {speed} m");
}

/// Off-screen nothing runs, and coming back goes on from the pose with no motion stored: a character
/// that walked 50 m unseen doesn't drag its hair after it.
#[test]
fn asleep_nothing_runs_and_waking_settles_from_the_pose() {
    let mut character = Character::new(&hair(), Substeps::Two);
    for tick in 0..60 {
        assert!(character.frame(sway, f64::from(tick)).wrote_pose);
    }
    fn away(ticks: f64) -> Posed {
        Posed {
            root: Vec3::new(ticks as f32 * 0.03, 0.0, 0.0),
            chest: Quat::from_rotation_y(1.0),
            ..Posed::default()
        }
    }
    let tails = character.springs.tails.clone();
    for tick in 60..200 {
        character.set_pose(away(f64::from(tick)));
        let posed = character.bits();
        let frame = character.frame_at(f64::from(tick), true, false);
        assert_eq!(character.springs.update(&mut character.pose, &frame), SpringWork::default());
        assert_eq!(character.bits(), posed, "the pose as the animation made it");
        assert_eq!(character.springs.tails, tails, "no work");
    }
    // Awake: first the frame settles on the pose, then motion starts from nothing.
    character.set_pose(away(200.0));
    let frame = character.frame_at(200.0, true, true);
    let work = character.springs.update(&mut character.pose, &frame);
    assert_eq!(work, SpringWork { substeps: 0, wrote_pose: true });
    assert!(
        character.world_of(TIP).abs_diff_eq(character.posed_tip(), 1e-4),
        "{} against {}",
        character.world_of(TIP),
        character.posed_tip()
    );
    // Walking on at its pace: nothing whips, however far it went unseen.
    let mut last = character.world_of(TIP);
    for tick in 201..240 {
        character.frame(away, f64::from(tick));
        let tip = character.world_of(TIP);
        assert!(tip.distance(last) < 0.06, "at twice its pace at most: {} m at {tick}", tip.distance(last));
        last = tip;
    }

    // Asleep with nothing evaluated, then awake with nothing evaluated: the stale pose is the last
    // evaluated, not the springs' own result.
    let mut character = Character::new(&hair(), Substeps::Two);
    for tick in 0..40 {
        character.frame(sway, f64::from(tick));
    }
    character.set_pose(sway(40.0));
    let frame = character.frame_at(40.0, true, false);
    character.springs.update(&mut character.pose, &frame);
    let frame = character.frame_at(41.0, false, true);
    assert!(character.springs.is_due(frame.ticks, &frame.world_from_model), "waking is due");
    assert!(character.springs.update(&mut character.pose, &frame).wrote_pose);
    assert!(character.world_of(TIP).abs_diff_eq(character.posed_tip(), 1e-4));
}

/// The tier's limit, with a margin so a character standing at it doesn't flip.
#[test]
fn a_character_past_the_limit_sleeps_and_wakes_nearer() {
    let quality = SpringQuality::DESKTOP;
    assert!(quality.in_range(quality.distance - 0.1, true));
    assert!(!quality.in_range(quality.distance + 0.1, true));
    assert!(!quality.in_range(quality.distance - 0.1, false), "it wakes a little nearer");
    assert!(quality.in_range(quality.distance * 0.8, false));
    // Phones run fewer sub-steps and stop sooner.
    assert!(SpringQuality::MOBILE.substeps.count() < SpringQuality::DESKTOP.substeps.count());
    const { assert!(SpringQuality::MOBILE.distance < SpringQuality::DESKTOP.distance) };
}

/// A rig with a joint per chain and a collider beside the strand, in the given group.
fn collided(shape: ColliderShape) -> CharacterRig {
    let joints =
        vec![spring_joint(3, 0.5, 0.4, 0.1), spring_joint(4, 0.5, 0.4, 0.1), spring_joint(5, 0.5, 0.4, 0.1)];
    CharacterRig {
        colliders: vec![RigCollider { joint: ARM as u16, shape }],
        collider_groups: vec![ColliderGroup { name: "arm".into(), colliders: vec![0] }],
        ..rig(vec![Spring { joints, collider_groups: vec![0], ..strand(0.0, 0.0, 0.0) }])
    }
}

/// `collider`'s motion across the strand, from one side of it to the other, at 15 m/s: 25 cm a tick,
/// more than a collider's diameter in a tick.
fn fast_pass(ticks: f64) -> Posed {
    let x = ((ticks - 30.0) as f32 * 0.25).clamp(-3.0, 3.0);
    // The strand's second joint is 0.1 m below the head at z −0.1, on the chest's +0.3.
    Posed { arm: Vec3::new(x, 0.3, -0.1), ..Posed::default() }
}

/// A tail against a collider through a fast pass: never inside it, to a tolerance for a capsule's nearest
/// point moving under the tail, at every sub-step; pushed along ahead of it for as long as the collider
/// is near; and only then back to its rest, from the side the collider went. A collider stepping over
/// the tail would leave it on the side the collider came from. Run on each sub-step count that ships.
fn a_fast_pass_stays_out(shape: ColliderShape, reach: f32) {
    for quality in SpringQuality::SHIPPED {
        let count = quality.substeps.count();
        let mut character = Character::new(&collided(shape), quality.substeps);
        character.frame(fast_pass, 0.0);
        let mut furthest = 0.0_f32;
        // Every sub-step gets a frame of its own, so all of them are looked at.
        for frame in 1..120 * count {
            let ticks = f64::from(frame) / f64::from(count);
            character.frame(fast_pass, ticks);
            let springs = &character.springs;
            let placed = &springs.colliders_now[0];
            // A later tail hangs from the one before it, which may have left it no way out: a bone is
            // a line, and the collider can come between it and its head.
            for (at, tail) in springs.tails.iter().enumerate() {
                let inside = reach - tail.now.distance(placed.nearest(tail.now));
                let allowed = if at == 0 { 2e-3 } else { reach * 0.6 };
                assert!(inside < allowed, "{quality:?} at {ticks}: tail {at} is {inside} m in");
            }
            let tail = springs.tails[0].now;
            if ticks >= 30.0 {
                furthest = furthest.max(tail.x);
            }
            if (30.0..40.0).contains(&ticks) {
                assert!(tail.x > -1e-3, "{quality:?} at {ticks}: left behind at {tail}, past {placed:?}");
            }
        }
        assert!(furthest > 0.05, "{quality:?}: pushed along, to {furthest} m at most");
    }
}

/// The limits have the last word: an arm sweeping across a strand with no room left goes through it,
/// rather than flinging it past its limit, and the strand comes back when the arm has gone.
#[test]
fn the_limits_have_the_last_word_over_a_collider() {
    let rig = collided(ColliderShape::Sphere { offset: [0.0; 3], radius: 0.06 });
    let tuning = SpringTuning { root_inertia: 1.0, anchor_inertia: 1.0, ..SpringTuning::ACTION };
    for substeps in [Substeps::One, Substeps::Two, Substeps::Four] {
        let mut character = Character::new(&rig, substeps);
        character.springs.set_tuning(tuning);
        let mut last = None;
        for tick in 0..180 {
            character.frame(fast_pass, f64::from(tick));
            for pair in STRAND.windows(2).take(3) {
                let bone =
                    character.world_of(usize::from(pair[1])) - character.world_of(usize::from(pair[0]));
                let swing = bone.normalize().angle_between(Vec3::NEG_Y);
                assert!(
                    swing <= tuning.swing_limit + 0.02,
                    "{substeps:?} at {tick}: {swing} rad off the pose"
                );
            }
            let tip = character.world_of(TIP);
            if let Some(last) = last {
                let moved: Vec3 = tip - last;
                assert!(moved.length() < 0.2, "{substeps:?} at {tick}: thrown {} m", moved.length());
            }
            last = Some(tip);
        }
        let off = character.world_of(TIP).distance(character.posed_tip());
        assert!(off < 0.05, "{substeps:?}: {off} m off the pose once the arm has gone");
    }
}

#[test]
fn a_fast_sphere_never_passes_through_hair() {
    let radius = 0.06;
    a_fast_pass_stays_out(ColliderShape::Sphere { offset: [0.0; 3], radius }, radius + 0.01);
}

#[test]
fn a_fast_capsule_never_passes_through_hair() {
    let radius = 0.04;
    let shape = ColliderShape::Capsule { offset: [-0.1, 0.0, 0.0], radius, tail: [0.1, 0.0, 0.0] };
    a_fast_pass_stays_out(shape, radius + 0.01);
    // Across the capsule's length, as an arm sweeping past a strand end on.
    let shape = ColliderShape::Capsule { offset: [0.0, -0.1, 0.0], radius, tail: [0.0, 0.1, 0.0] };
    a_fast_pass_stays_out(shape, radius + 0.01);
}

/// The path test is what stops a fast collider stepping over a tail: a discrete test at the sub-step's
/// end sees it past the tail and pushes the tail through. This pins that the test has something to
/// catch: the last step really does step over.
#[test]
fn a_discrete_test_would_step_over_the_fast_pass() {
    let (was, now) = (
        Placed { start: Vec3::new(-0.13, 0.0, 0.0), axis: Vec3::ZERO, radius: 0.06 },
        Placed { start: Vec3::new(0.12, 0.0, 0.0), axis: Vec3::ZERO, radius: 0.06 },
    );
    let tail = Vec3::ZERO;
    // At the end the tail is behind the collider's center, so a discrete test would push it to -x.
    let out = contact(tail, tail, &was, &now, 0.01).expect("it touched");
    assert!(out.x > now.start.x, "ahead of it, on the side it started: {out}");
}

/// A chain with a center doesn't swing when the center moves, as far as its tails are carried.
#[test]
fn a_center_carries_its_chain_along() {
    fn jump(ticks: f64) -> Posed {
        Posed {
            chest: Quat::from_rotation_y(((ticks - 10.0).clamp(0.0, 4.0) * 0.4) as f32),
            ..Posed::default()
        }
    }
    let run = |center: Option<u16>| {
        let spring = Spring { center, ..strand(0.5, 0.4, 0.0) };
        let mut character = Character::new(&rig(vec![spring]), Substeps::Two);
        let mut most = 0.0_f32;
        for tick in 0..30 {
            character.frame(jump, f64::from(tick));
            most = most.max(character.world_of(TIP).distance(character.posed_tip()));
        }
        most
    };
    let free = run(None);
    let carried = run(Some(1));
    assert!(free > 0.05, "a free chain swings: {free} m");
    assert!(carried < free / 5.0, "a carried one much less: {carried} m against {free}");
}

/// VRM 0.x's virtual tails: the last joint swings too, toward a point past it along its bone.
#[test]
fn an_extended_tip_swings_the_last_joint() {
    let spring = Spring { tip: SpringTip::Extended { length: 0.07 }, ..strand(0.5, 0.4, 0.1) };
    let mut character = Character::new(&rig(vec![spring]), Substeps::Two);
    assert_eq!(character.springs.swing_count(), 5);
    for tick in 0..40 {
        character.frame(strike, f64::from(tick));
    }
    let turned = character.pose.locals()[TIP].rotation;
    assert!(!turned.abs_diff_eq(Quat::IDENTITY, 1e-3), "the last joint turned: {turned}");
    // And with a joint for a tip, it doesn't.
    let mut plain = Character::new(&hair(), Substeps::Two);
    assert_eq!(plain.springs.swing_count(), 4);
    for tick in 0..40 {
        plain.frame(strike, f64::from(tick));
    }
    assert_eq!(plain.pose.locals()[TIP].rotation, Quat::IDENTITY);
}

/// A chain may skip a joint, and a chain may hang from another's joint: what's between follows, and the
/// lower chain follows the upper one's swing.
#[test]
fn chains_with_joints_between_or_below_others_follow_them() {
    let (upper, lower) = (
        Spring {
            joints: vec![spring_joint(3, 0.5, 0.4, 0.0), spring_joint(5, 1.0, 0.5, 0.0)],
            ..strand(0.0, 0.0, 0.0)
        },
        Spring {
            joints: vec![
                spring_joint(5, 0.5, 0.4, 0.0),
                spring_joint(6, 0.5, 0.4, 0.0),
                spring_joint(7, 1.0, 0.5, 0.0),
            ],
            ..strand(0.0, 0.0, 0.0)
        },
    );
    let mut character = Character::new(&rig(vec![lower, upper]), Substeps::Two);
    assert_eq!(character.springs.swing_count(), 3, "3 and 5 and 6");
    assert_eq!(character.springs.steps.len(), 4, "4 is walked through");
    for tick in 0..40 {
        character.frame(strike, f64::from(tick));
    }
    // The joint between keeps its pose relative to its parent.
    assert_eq!(character.pose.locals()[4].rotation, Quat::IDENTITY);
    assert_ne!(character.pose.locals()[3].rotation, Quat::IDENTITY);
    // The solved tails are where the skeleton puts the joints, the lower chain's root included.
    let first_tail = character.springs.tails[0].now + character.springs.origin;
    // Drawing lags a sub-step, so compare with what that sub-step left in the walk.
    let walked =
        character.springs.slots[character.springs.anchors.len() + 2].position + character.springs.origin;
    assert!(first_tail.abs_diff_eq(walked, 1e-5), "{first_tail} against {walked}");
}

/// Motion survives a new version of the character, when it swings the same joints: a stiffer rig goes
/// on from where the tails were. One that swings others starts again, from the pose.
#[test]
fn motion_survives_a_new_version_of_the_character() {
    let mut old = Character::new(&hair(), Substeps::One);
    for tick in 0..34 {
        old.frame(strike, f64::from(tick));
    }
    let tails = old.springs.tails.clone();

    let mut new = Character::new(&rig(vec![strand(1.0, 0.4, 0.1)]), Substeps::Two);
    new.springs.continue_from(&old.springs);
    assert_eq!(new.springs.substeps(), Substeps::One, "the tier hasn't changed");
    assert_eq!(new.springs.tails, tails);
    let work = new.frame(strike, 34.0);
    assert_eq!(work.substeps, 1, "it goes on, rather than settling");
    assert!(new.springs.tails[0].now.distance(tails[0].now) < 0.1, "from where it was");

    let other = Spring {
        joints: vec![spring_joint(4, 0.5, 0.4, 0.0), spring_joint(5, 1.0, 0.5, 0.0)],
        ..strand(0.0, 0.0, 0.0)
    };
    let mut new = Character::new(&rig(vec![other]), Substeps::Two);
    new.springs.continue_from(&old.springs);
    assert_eq!(new.frame(strike, 34.0), SpringWork { substeps: 0, wrote_pose: true }, "settles");
}

/// A new version can swap one of a chain's colliders for one the old version hadn't, on a joint it
/// already placed: the same joints swing the same way, with more colliders to place.
#[test]
fn a_new_version_with_another_collider_keeps_running() {
    let sphere = |radius| ColliderShape::Sphere { offset: [0.0; 3], radius };
    let collider = |radius| RigCollider { joint: ARM as u16, shape: sphere(radius) };
    let with = |colliders: Vec<RigCollider>, listed: Vec<u32>| CharacterRig {
        colliders,
        collider_groups: vec![ColliderGroup { name: "arm".into(), colliders: listed }],
        ..collided(sphere(0.06))
    };
    let mut old = Character::new(&with(vec![collider(0.06), collider(0.07)], vec![0, 1]), Substeps::One);
    for tick in 0..34 {
        old.frame(fast_pass, f64::from(tick));
    }
    let mut new = Character::new(
        &with(vec![collider(0.06), collider(0.07), collider(0.08)], vec![0, 2]),
        Substeps::One,
    );
    new.springs.continue_from(&old.springs);
    for tick in 34..40 {
        new.frame(fast_pass, f64::from(tick));
    }
    assert!(new.world_of(TIP).is_finite());
}

/// A tier change keeps a tail's place and its speed, so hair doesn't jump.
#[test]
fn a_new_substep_count_keeps_the_motion() {
    let mut character = Character::new(&hair(), Substeps::Two);
    for tick in 0..34 {
        character.frame(strike, f64::from(tick));
    }
    let before = character.world_of(TIP);
    let speed = (character.springs.tails[0].now - character.springs.tails[0].before) * 2.0;
    character.springs.set_substeps(Substeps::One);
    let kept = character.springs.tails[0].now - character.springs.tails[0].before;
    assert!(kept.abs_diff_eq(speed, 1e-6), "{kept} against {speed}");
    character.frame(strike, 35.0);
    assert!(character.world_of(TIP).distance(before) < 0.2);
}

/// One non-finite frame doesn't stay in the state: the frames after it are finite again.
#[test]
fn a_bad_frame_heals() {
    let mut character = Character::new(&hair(), Substeps::Two);
    for tick in 0..10 {
        character.frame(strike, f64::from(tick));
    }
    character.set_pose(still(0.0));
    character.world = Affine3A::from_translation(Vec3::splat(f32::NAN));
    let frame = character.frame_at(10.0, true, true);
    character.springs.update(&mut character.pose, &frame);
    for tick in 11..16 {
        character.frame(strike, f64::from(tick));
    }
    assert!(character.world_of(TIP).is_finite());
    assert!(character.world_of(TIP).distance(character.posed_tip()) < 0.1);
}

/// A rig with no springs makes nothing, so it costs nothing; nor does one for another skeleton.
#[test]
fn a_rig_with_no_springs_makes_none() {
    let bytes = rig(Vec::new()).to_cooked().expect("cooked");
    let none = CharacterRig::read(&bytes).expect("a valid rig");
    let skeleton_bytes = Skeleton::new(&joints()).to_cooked().expect("cooked");
    let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
    assert!(SpringBones::new(none, &pose).is_none());

    // A chain of one joint with a joint for a tip has nothing to swing.
    let single = Spring { joints: vec![spring_joint(3, 1.0, 0.5, 0.0)], ..strand(0.0, 0.0, 0.0) };
    let bytes = rig(vec![single]).to_cooked().expect("cooked");
    assert!(SpringBones::new(CharacterRig::read(&bytes).expect("a valid rig"), &pose).is_none());

    let other = CharacterRig { joint_count: 20, ..hair() };
    let bytes = other.to_cooked().expect("cooked");
    assert!(SpringBones::new(CharacterRig::read(&bytes).expect("a valid rig"), &pose).is_none());
}

/// Drag converts exactly at one sub-step, and the speed where drag balances a push holds across counts.
#[test]
fn a_substep_count_keeps_the_speed_where_drag_balances_a_push() {
    for drag in [0.05, 0.4, 0.5, 1.0] {
        let one = Rate::new(drag, Substeps::One);
        assert!((one.keep - (1.0 - drag)).abs() < 1e-6);
        assert!((one.force - 1.0 / 60.0).abs() < 1e-6, "{drag}: {one:?}");
        assert!((one.carry - 1.0).abs() < 1e-6, "{drag}: {one:?}");
        for substeps in [Substeps::Two, Substeps::Four] {
            let rate = Rate::new(drag, substeps);
            let count = substeps.count() as f32;
            assert!((rate.keep.powf(count) - (1.0 - drag)).abs() < 1e-5, "{drag} {substeps:?}: {rate:?}");
            let speed = |rate: Rate, count: f32| count * rate.force / (1.0 - rate.keep).max(1e-6);
            assert!((speed(rate, count) - speed(one, 1.0)).abs() < 1e-4, "{drag} {substeps:?}: {rate:?}");
            assert!((rate.carry - rate.force * count * 60.0).abs() < 1e-5, "{drag} {substeps:?}: {rate:?}");
        }
    }
    let none = Rate::new(0.0, Substeps::Two);
    assert_eq!(none.keep, 1.0);
    assert!((none.force - 1.0 / 240.0).abs() < 1e-7);
    assert_eq!(none.carry, 0.5);
}

#[test]
fn spring_reload_refuses_changed_joint_identity_even_when_counts_match() {
    let mut old = Character::new(&hair(), Substeps::One);
    for tick in 0..20 {
        old.frame(sway, f64::from(tick));
    }
    assert!(!old.springs.settling);
    let mut changed = joints();
    changed[usize::from(STRAND[0])].name = "different strand".into();
    let bytes = Skeleton::new(&changed).to_cooked().unwrap();
    let pose = CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], Vec::new());
    let bytes = hair().to_cooked().unwrap();
    let mut springs = SpringBones::new(CharacterRig::read(&bytes).unwrap(), &pose).unwrap();
    springs.continue_from(&old.springs);
    assert!(springs.settling, "new joint identities must settle rather than inherit old tails");
    let mut compatible = Character::new(&hair(), Substeps::One);
    compatible.springs.continue_from(&old.springs);
    assert!(!compatible.springs.settling);
    assert_eq!(compatible.springs.tails, old.springs.tails);
}
