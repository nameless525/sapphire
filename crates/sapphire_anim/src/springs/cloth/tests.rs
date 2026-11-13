use sapphire_assets::rig::{
    CharacterRig, Collider as RigCollider, ColliderGroup, ColliderShape, Spring, SpringJoint, SpringTip,
};
use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};

use super::super::*;
use super::*;

const HIPS: usize = 1;
const LEFT_LEG: usize = 2;
const RIGHT_LEG: usize = 3;
/// The first chain's first joint.
const FIRST_CHAIN: usize = 4;

/// A skirt: a root, hips a meter up, two legs with a capsule each, and `chains` chains of `joints` joints
/// hanging round the hips, flaring out a little, each swinging toward a 6 cm tail past its last.
struct Skirt {
    chains: usize,
    joints: usize,
}

impl Skirt {
    /// The tests' skirt. `tests/cost.rs` times one of eight by eight.
    const SMALL: Self = Self { chains: 6, joints: 4 };

    fn skeleton(&self) -> Vec<Joint> {
        let joint = |name: String, parent: usize, translation: [f32; 3]| Joint {
            name,
            parent: Some(parent as u16),
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        };
        let mut joints = vec![
            Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY },
            joint("hips".into(), 0, [0.0, 1.0, 0.0]),
            joint("left leg".into(), HIPS, [0.09, 0.0, 0.0]),
            joint("right leg".into(), HIPS, [-0.09, 0.0, 0.0]),
        ];
        for chain in 0..self.chains {
            let (sin, cos) = (std::f32::consts::TAU * chain as f32 / self.chains as f32).sin_cos();
            for at in 0..self.joints {
                let name = format!("skirt {chain} {at}");
                if at == 0 {
                    joints.push(joint(name, HIPS, [0.18 * cos, -0.05, 0.18 * sin]));
                } else {
                    joints.push(joint(name, joints.len() - 1, [0.015 * cos, -0.06, 0.015 * sin]));
                }
            }
        }
        joints
    }

    fn rig(&self) -> CharacterRig {
        let leg = |joint: usize| RigCollider {
            joint: joint as u16,
            shape: ColliderShape::Capsule { offset: [0.0; 3], radius: 0.07, tail: [0.0, -0.8, 0.0] },
        };
        let springs = (0..self.chains)
            .map(|chain| Spring {
                name: format!("skirt {chain}"),
                joints: (0..self.joints)
                    .map(|at| SpringJoint {
                        joint: (FIRST_CHAIN + chain * self.joints + at) as u16,
                        hit_radius: 0.02,
                        stiffness: 0.3,
                        gravity_power: 0.5,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.2,
                    })
                    .collect(),
                tip: SpringTip::Extended { length: 0.06 },
                collider_groups: vec![0],
                center: None,
            })
            .collect();
        CharacterRig {
            joint_count: self.skeleton().len() as u32,
            springs,
            colliders: vec![leg(LEFT_LEG), leg(RIGHT_LEG)],
            collider_groups: vec![ColliderGroup { name: "legs".into(), colliders: vec![0, 1] }],
            ..CharacterRig::default()
        }
    }

    /// The whole skirt as one closed sheet.
    fn sheet(&self) -> ClothSheet {
        ClothSheet {
            chains: (0..self.chains).map(|chain| format!("skirt {chain}")).collect(),
            closed: true,
            lateral_compliance: 1e-6,
            bend_compliance: 1e-4,
        }
    }
}

/// What a test's motion makes of a tick.
#[derive(Clone, Copy, Debug)]
struct Posed {
    root: Vec3,
    turn: Quat,
    hips: Quat,
    left: Quat,
    right: Quat,
}

impl Default for Posed {
    fn default() -> Self {
        Self {
            root: Vec3::ZERO,
            turn: Quat::IDENTITY,
            hips: Quat::IDENTITY,
            left: Quat::IDENTITY,
            right: Quat::IDENTITY,
        }
    }
}

type Motion = fn(f64) -> Posed;

fn still(_: f64) -> Posed {
    Posed::default()
}

/// A brisk walk: legs swinging half a radian each way a second, the hips swaying, 1.4 m/s forward.
fn walk(ticks: f64) -> Posed {
    let seconds = ticks as f32 / 60.0;
    let wave = |hz: f32| (seconds * hz * std::f32::consts::TAU).sin();
    Posed {
        root: Vec3::new(0.0, wave(2.0).abs() * 0.03, seconds * 1.4),
        hips: Quat::from_rotation_y(wave(1.0) * 0.2),
        left: Quat::from_rotation_x(-wave(1.0) * 0.5),
        right: Quat::from_rotation_x(wave(1.0) * 0.5),
        ..Posed::default()
    }
}

/// A strike: still, then a lunge of 35 cm, a turn of the body by a third of a radian and a kick of the
/// left leg high into the skirt's front over 6 ticks, then still.
fn strike(ticks: f64) -> Posed {
    let u = ((ticks as f32 - 30.0) / 6.0).clamp(0.0, 1.0);
    let eased = u * u * (3.0 - 2.0 * u);
    Posed {
        root: Vec3::new(0.0, 0.0, eased * 0.35),
        turn: Quat::from_rotation_y(eased * 0.6),
        left: Quat::from_rotation_x(-eased * 1.2),
        ..Posed::default()
    }
}

/// A character in a skirt, driven as a frame drives it.
struct Character {
    pose: CharacterPose,
    springs: SpringBones,
    world: Affine3A,
}

impl Character {
    /// The skirt at `iterations`, or with no cloth at all for `None`.
    fn new(skirt: &Skirt, substeps: Substeps, iterations: Option<u32>) -> Self {
        let bytes = skirt.rig().to_cooked().expect("cooked");
        let rig = CharacterRig::read(&bytes).expect("a valid rig");
        let skeleton_bytes = Skeleton::new(&skirt.skeleton()).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
        let mut springs = SpringBones::new(rig, &pose).expect("it has springs");
        springs.set_substeps(substeps);
        if let Some(iterations) = iterations {
            springs.set_cloth(rig, &[skirt.sheet()]).expect("the skirt's chains");
            springs.set_cloth_iterations(iterations);
        }
        Self { pose, springs, world: Affine3A::IDENTITY }
    }

    fn set_pose(&mut self, posed: Posed) {
        self.pose.reset();
        let locals = self.pose.locals_mut();
        locals[HIPS].rotation = posed.hips;
        locals[LEFT_LEG].rotation = posed.left;
        locals[RIGHT_LEG].rotation = posed.right;
        self.pose.finish();
        self.world = Affine3A::from_rotation_translation(posed.turn, posed.root);
    }

    fn frame(&mut self, motion: Motion, ticks: f64) -> SpringWork {
        self.set_pose(motion(ticks));
        let frame = self.frame_at(ticks, true, true);
        self.springs.update(&mut self.pose, &frame)
    }

    fn frame_at(&self, ticks: f64, pose_moved: bool, active: bool) -> SpringFrame {
        SpringFrame { world_from_model: self.world, ticks, pose_moved, active }
    }

    /// Every tail, in the world.
    fn tails(&self) -> Vec<Vec3> {
        self.springs.tails().map(|tail| tail.position).collect()
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
        for tail in self.springs.tails() {
            bits.extend(tail.position.to_array().map(f32::to_bits));
        }
        bits
    }
}

/// `motion` for `seconds` at `hz`: every tail at every 30th of a second, which every rate here draws.
fn tail_paths(substeps: Substeps, iterations: u32, hz: u32, motion: Motion, seconds: u32) -> Vec<Vec<Vec3>> {
    let mut character = Character::new(&Skirt::SMALL, substeps, Some(iterations));
    let mut paths = Vec::new();
    for frame in 0..=seconds * hz {
        let ticks = f64::from(frame) * 60.0 / f64::from(hz);
        character.frame(motion, ticks);
        if frame % (hz / 30) == 0 {
            paths.push(character.tails());
        }
    }
    paths
}

fn furthest_apart(a: &[Vec<Vec3>], b: &[Vec<Vec3>]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().flatten().zip(b.iter().flatten()).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// The springs' promise, kept with the cloth on: the frame rate changes nothing but the coarser path a
/// chain's root and the rig's colliders take between bigger frames. At each sub-step count and iteration
/// count that ships, every tail stays within these of where the 120 Hz run has it, at every 30th of a
/// second, and at 30 Hz the cloth is never further off than the same chains as springs alone.
#[test]
fn cloth_at_30_60_and_120_hz() {
    // A walk: 7 mm at 30 Hz, 4 mm at 60 Hz (6.3 and 3.5 at most, as built). A strike: 70 mm and 15 mm
    // (62 and 12), where the kick turns the leg's capsule 1.2 rad into the skirt's front in 6 ticks, only
    // 3 frames at 30 Hz, and a contact's answer to a coarser sweep carries down the chains: alone, as
    // springs, they come 110 mm apart at 30 Hz. Without the kick the strike's lunge reads 6 mm.
    let tolerances: [(Motion, f32, f32); 2] = [(walk, 0.007, 0.004), (strike, 0.07, 0.015)];
    for (motion, at_30, at_60) in tolerances {
        for substeps in [Substeps::One, Substeps::Two] {
            let alone = furthest_apart(
                &tail_paths(substeps, 0, 30, motion, 3),
                &tail_paths(substeps, 0, 120, motion, 3),
            );
            for iterations in [4, 2, 1] {
                let reference = tail_paths(substeps, iterations, 120, motion, 3);
                let a = furthest_apart(&tail_paths(substeps, iterations, 30, motion, 3), &reference);
                let b = furthest_apart(&tail_paths(substeps, iterations, 60, motion, 3), &reference);
                assert!(a <= at_30, "{substeps:?}, {iterations} iterations: 30 Hz is {a} m from 120 Hz");
                assert!(b <= at_60, "{substeps:?}, {iterations} iterations: 60 Hz is {b} m from 120 Hz");
                assert!(
                    a <= alone,
                    "{substeps:?}, {iterations} iterations: 30 Hz is {a} m off, alone {alone}"
                );
            }
        }
    }
}

/// A clock that stands still holds the cloth to the bit and runs nothing, and it carries on as if it
/// never stopped.
#[test]
fn a_frozen_clock_holds_the_cloth_to_the_bit_and_carries_on() {
    for substeps in [Substeps::One, Substeps::Two] {
        let mut stopping = Character::new(&Skirt::SMALL, substeps, Some(4));
        let mut running = Character::new(&Skirt::SMALL, substeps, Some(4));
        for tick in 0..34 {
            stopping.frame(strike, f64::from(tick) + 0.25);
            running.frame(strike, f64::from(tick) + 0.25);
        }
        let held = stopping.bits();
        for _ in 0..12 {
            let frame = stopping.frame_at(33.25, false, true);
            assert!(!stopping.springs.is_due(frame.ticks, &frame.world_from_model));
            assert_eq!(stopping.springs.update(&mut stopping.pose, &frame), SpringWork::default());
            assert_eq!(stopping.bits(), held);
            // A graph may evaluate the same pose again on a frozen clock.
            assert_eq!(stopping.frame(strike, 33.25).substeps, 0);
            assert_eq!(stopping.bits(), held, "the same pose evaluated again draws the same");
        }
        for tick in 34..70 {
            stopping.frame(strike, f64::from(tick) + 0.25);
            running.frame(strike, f64::from(tick) + 0.25);
            assert_eq!(stopping.bits(), running.bits(), "{substeps:?} at {tick}");
        }
    }
}

/// No iterations is no cloth: the chains are springs, to the bit, so a tier or a distance can turn it
/// off with nothing else moving.
#[test]
fn no_iterations_is_the_springs_alone_to_the_bit() {
    for substeps in [Substeps::One, Substeps::Two] {
        let mut off = Character::new(&Skirt::SMALL, substeps, Some(0));
        let mut none = Character::new(&Skirt::SMALL, substeps, None);
        for frame in 0..240 {
            let ticks = f64::from(frame) * 0.5;
            off.frame(walk, ticks);
            none.frame(walk, ticks);
            assert_eq!(off.bits(), none.bits(), "{substeps:?} at {ticks}");
        }
    }
}

/// The most a sheet's links stretch or squeeze over `seconds` of `motion` at 60 Hz, at `iterations`: 0
/// measures the chains as springs alone.
fn worst_strain(iterations: u32, motion: Motion, seconds: u32) -> f32 {
    let mut character = Character::new(&Skirt::SMALL, Substeps::Two, Some(iterations));
    let mut worst = 0.0_f32;
    for tick in 0..seconds * 60 {
        character.frame(motion, f64::from(tick));
        worst = worst.max(character.springs.cloth_strain());
    }
    worst
}

/// The links hold the sheet together where the chains alone come apart, and more iterations hold it
/// nearer. As built, the walk's links reach 30% alone and 6%, 6% and 7% at 4, 2 and 1 iterations; the
/// strike's 58% alone and 10%, 10% and 22%.
#[test]
fn the_links_hold_the_sheet_together() {
    for motion in [walk as Motion, strike] {
        let alone = worst_strain(0, motion, 3);
        let [four, two, one] = [4, 2, 1].map(|iterations| worst_strain(iterations, motion, 3));
        assert!(four < one && two < one && one < alone, "{four}, {two}, {one}, alone {alone}");
        assert!(
            four < alone * 0.5,
            "four iterations hold it to {four} where the chains alone come to {alone}"
        );
    }
}

/// A leg kicking into the skirt's front pushes the chains beside the one it strikes: through the links,
/// they move more than they do as springs alone.
#[test]
fn a_kick_carries_through_the_sheet() {
    // The chains at +z, the front, which the left leg kicks into.
    let chains = Skirt::SMALL.chains;
    let joints = Skirt::SMALL.joints;
    let front = chains / 4;
    let moved = |iterations: u32| {
        let mut character = Character::new(&Skirt::SMALL, Substeps::Two, Some(iterations));
        character.frame(still, 0.0);
        let before = character.tails();
        for tick in 1..40 {
            character.frame(strike, f64::from(tick));
        }
        let after = character.tails();
        // The neighbours' tips, in the strike's own frame (it turns and lunges the whole body).
        let world = character.world.inverse();
        [front - 1, front + 1]
            .map(|chain| {
                let tip = chain * joints + joints - 1;
                world.transform_point3(after[tip]).distance(before[tip])
            })
            .into_iter()
            .fold(0.0, f32::max)
    };
    let (alone, linked) = (moved(0), moved(4));
    assert!(linked > alone * 1.2, "the neighbours moved {linked} m with the links and {alone} m alone");
}

/// A still character in a skirt settles with no jitter and stops, as springs do, at every count.
#[test]
fn a_still_skirt_settles_and_stops() {
    for substeps in [Substeps::One, Substeps::Two, Substeps::Four] {
        for iterations in [4, 1] {
            let mut character = Character::new(&Skirt::SMALL, substeps, Some(iterations));
            for tick in 0..60 {
                character.frame(walk, f64::from(tick));
            }
            character.set_pose(still(0.0));
            let mut ticks = 60.0;
            let mut stopped = None;
            for tick in 0..1_200 {
                ticks += 1.0;
                let frame = character.frame_at(ticks, tick == 0, true);
                if !character.springs.update(&mut character.pose, &frame).wrote_pose {
                    stopped = Some(tick);
                    break;
                }
            }
            assert!(stopped.is_some(), "{substeps:?}, {iterations} iterations: never stopped");
            assert!(character.springs.is_resting());
            assert!(character.springs.cloth_strain() < 0.02, "{}", character.springs.cloth_strain());
        }
    }
}

/// What a fight can throw at a skirt, round and round in laps of 20 seconds: walking, a strike, a spin at
/// four turns a second, then walking again, with no reset at any seam, so each is a teleport of metres and
/// a snap of the legs, and each lap starts 30 m from the last. [`soak`] resets once a lap.
fn violent(ticks: f64) -> Posed {
    const CYCLE: f64 = 1_200.0;
    let lap = (ticks / CYCLE).floor();
    let at = ticks - lap * CYCLE;
    // Each lap starts 30 m on from the last, with no reset: a teleport.
    let base = Vec3::new(30.0 * lap as f32, 0.0, 0.0);
    let posed = if at < 300.0 {
        walk(at)
    } else if at < 400.0 {
        strike(at - 300.0)
    } else if at < 520.0 {
        let spin = ((at - 400.0) / 60.0 * 4.0 * std::f64::consts::TAU) as f32;
        Posed { turn: Quat::from_rotation_y(spin), left: Quat::from_rotation_x(-0.8), ..walk(at) }
    } else {
        walk(at)
    };
    Posed { root: posed.root + base, ..posed }
}

/// [`violent`] for `minutes` at `hz` and `iterations` (0 for the chains as springs alone): the worst
/// link's stretch, and how far any bone came off its length. Every tail must stay finite.
fn soak(hz: u32, iterations: u32, minutes: u32) -> (f32, f32) {
    let mut character = Character::new(&Skirt::SMALL, Substeps::Two, Some(iterations));
    let (mut worst, mut furthest) = (0.0_f32, 0.0_f32);
    for frame in 0..=minutes * 60 * hz {
        let ticks = f64::from(frame) * 60.0 / f64::from(hz);
        // A reset three quarters of the way through every lap, as a cut would make.
        if ticks % 1_200.0 == 900.0 {
            character.springs.reset();
        }
        character.frame(violent, ticks);
        worst = worst.max(character.springs.cloth_strain());
        // Each tail after a chain's first hangs from the one before, at its bone's length.
        let tails: Vec<SpringTail> = character.springs.tails().collect();
        for (index, tail) in tails.iter().enumerate() {
            assert!(tail.position.is_finite(), "{hz} Hz, {iterations} iterations, at {ticks}");
            if index % Skirt::SMALL.joints > 0 {
                furthest =
                    furthest.max((tail.position.distance(tails[index - 1].position) - tail.length).abs());
            }
        }
    }
    (worst, furthest)
}

/// Ten simulated minutes of [`violent`] at 30, 60 and 120 Hz, at each iteration count that ships: every
/// tail stays finite, every bone keeps its length, and no link stretches past a bound, nor as far as the
/// chains alone come apart. As built the worst links reach 34%, 38% and 41% at 4, 2 and 1 iterations at
/// 30 Hz, 18%, 30% and 50% at 60 Hz, and 17%, 38% and 56% at 120 Hz, each on a teleport's frame, where
/// the chains alone reach 84% to 100%; bones stay within 0.06 mm of their length.
#[test]
fn a_ten_minute_soak_stays_bounded() {
    const MINUTES: u32 = 10;
    for hz in [30, 60, 120] {
        let (alone, _) = soak(hz, 0, MINUTES);
        for (iterations, bound) in [(4, 0.4), (2, 0.45), (1, 0.65)] {
            let (worst, furthest) = soak(hz, iterations, MINUTES);
            assert!(worst < bound, "{hz} Hz, {iterations} iterations: a link reached {worst} of its length");
            assert!(
                worst < alone,
                "{hz} Hz, {iterations} iterations: {worst}, where the chains alone reach {alone}"
            );
            assert!(
                furthest < 2e-4,
                "{hz} Hz, {iterations} iterations: a bone came {furthest} m off its length"
            );
        }
    }
}

/// A new version of the character with the same cloth carries on with its motion, to the bit.
#[test]
fn cloth_survives_a_new_version_of_the_character() {
    let mut replaced = Character::new(&Skirt::SMALL, Substeps::Two, Some(2));
    let mut kept = Character::new(&Skirt::SMALL, Substeps::Two, Some(2));
    for tick in 0..34 {
        replaced.frame(strike, f64::from(tick));
        kept.frame(strike, f64::from(tick));
    }
    let mut new = Character::new(&Skirt::SMALL, Substeps::Two, Some(4));
    new.springs.continue_from(&replaced.springs);
    assert_eq!(new.springs.cloth_iterations(), Some(2));
    replaced.springs = new.springs;
    for tick in 34..60 {
        replaced.frame(strike, f64::from(tick));
        kept.frame(strike, f64::from(tick));
        assert_eq!(replaced.bits(), kept.bits(), "at {tick}");
    }
}

/// A sheet names chains the rig has, each once, enough of them, with compliances that make sense.
#[test]
fn a_sheet_that_cant_be_worn_is_refused() {
    let skirt = Skirt::SMALL;
    let bytes = skirt.rig().to_cooked().expect("cooked");
    let rig = CharacterRig::read(&bytes).expect("a valid rig");
    let character = Character::new(&skirt, Substeps::Two, None);
    let mut springs = character.springs;
    let sheet = |chains: &[&str], closed: bool| ClothSheet {
        chains: chains.iter().map(|&name| name.to_owned()).collect(),
        closed,
        ..skirt.sheet()
    };
    let refused = [
        (sheet(&["skirt 0", "cape"], false), ClothError::UnknownChain("cape".into())),
        (sheet(&["skirt 0", "skirt 0"], false), ClothError::RepeatedChain("skirt 0".into())),
        (sheet(&["skirt 0"], false), ClothError::TooFewChains { has: 1, needs: 2 }),
        (sheet(&["skirt 0", "skirt 1"], true), ClothError::TooFewChains { has: 2, needs: 3 }),
        (
            ClothSheet { bend_compliance: -1.0, ..sheet(&["skirt 0", "skirt 1"], false) },
            ClothError::BadCompliance { field: "bend_compliance", value: -1.0 },
        ),
    ];
    for (sheet, error) in refused {
        assert_eq!(springs.set_cloth(rig, &[sheet]), Err(error));
    }
    assert_eq!(springs.cloth_iterations(), None);
    assert_eq!(springs.set_cloth(rig, &[sheet(&["skirt 0", "skirt 1"], false)]), Ok(()));
    assert_eq!(springs.cloth_iterations(), Some(4));
    springs.set_cloth_iterations(100);
    assert_eq!(springs.cloth_iterations(), Some(MAX_CLOTH_ITERATIONS));
    assert_eq!(springs.set_cloth(rig, &[]), Ok(()));
    assert_eq!(springs.cloth_iterations(), None);
}

/// The links a sheet makes: neighbours at each depth, and two along for bending, each pair once.
#[test]
fn a_closed_sheet_links_round_and_an_open_one_across() {
    let skirt = Skirt::SMALL;
    let bytes = skirt.rig().to_cooked().expect("cooked");
    let rig = CharacterRig::read(&bytes).expect("a valid rig");
    let mut springs = Character::new(&skirt, Substeps::Two, None).springs;
    let links = |springs: &SpringBones| springs.cloth.as_ref().map_or(0, |cloth| cloth.links.len());
    springs.set_cloth(rig, &[skirt.sheet()]).expect("closed");
    // Six chains round: six lateral pairs and six bend pairs, at each of four depths.
    assert_eq!(links(&springs), (6 + 6) * 4);
    springs.set_cloth(rig, &[ClothSheet { closed: false, ..skirt.sheet() }]).expect("open");
    assert_eq!(links(&springs), (5 + 4) * 4);
    let three = ClothSheet { chains: skirt.sheet().chains[..3].to_vec(), ..skirt.sheet() };
    springs.set_cloth(rig, &[three]).expect("three round");
    // Round three chains, two along is a neighbour the other way: three pairs at each depth.
    assert_eq!(links(&springs), 3 * 4);
}
