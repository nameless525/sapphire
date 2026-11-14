//! A party's secondary motion (M5 C2): the benchmark's hero (M5 B3), generated in memory, three of her
//! running together, the one on field stepping every frame and the others taking turns as each tier's
//! rotation says. Whether the turns keep to the floor and the bound is tested here on what the cast
//! ships; what they cost is printed by the ignored test, in release, on a machine to itself:
//! `cargo test --release -p sapphire_pipeline --test party_springs -- --ignored --nocapture`. Her skirt as
//! cloth is the sheet Aster's motion wears (`content/combat/hero/hero.motion.ron`, M5 C4): Aster is this
//! hero at the `ci` scale.

#![allow(clippy::print_stdout, reason = "the measurements here are worth reading with --nocapture")]
#![allow(
    clippy::disallowed_methods,
    reason = "a test's own measurements are never cooked (the crate's clippy.toml)"
)]

use std::path::Path;

use sapphire_anim::{
    CLOTH_EASE_SECONDS, CharacterMotion, CharacterPose, Skeleton, SpringBones, SpringFloor, SpringFrame,
    SpringPace, SpringQuality, StaggerQuality,
};
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_math::{Affine3A, Quat, Vec3};
use sapphire_pipeline::bench::{Preset, cast};
use sapphire_pipeline::figure::{self, FileKind};
use sapphire_pipeline::import::vrm;
use sapphire_serialize::cooked::AlignedVec;

/// The cast's hero, cooked in memory: the `ci` preset's, which has the budget's skeleton, chains and
/// colliders, cut coarser, so the springs cost what the budget's do.
struct Hero {
    rig: AlignedVec,
    skeleton: AlignedVec,
    binds: Vec<sapphire_assets::mesh::SkinBind>,
    morph_targets: usize,
    /// The joints the run turns: hips, spine, chest, head, then each upper leg.
    turned: [usize; 6],
}

impl Hero {
    fn load() -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/cast");
        let members = cast::members(&dir, Preset::Ci).expect("the cast");
        let hero = members.iter().find(|member| member.figure.name == "hero").expect("the hero");
        let files = figure::figure_files(&hero.copy(0)).expect("her files");
        let model = files.iter().find(|file| file.kind == FileKind::Model).expect("her model");
        let rig = vrm::import_rig(&model.bytes, None).expect("her rig");
        let imported = vrm::import(&model.bytes, None).expect("her model");
        let skin = imported.mesh.skin.as_ref().expect("skinned");
        let skeleton = Skeleton::new(&skin.joints);
        let bone = |bone: HumanBone| {
            let entry = rig.humanoid.iter().find(|entry| entry.bone == bone).expect("a humanoid bone");
            usize::from(entry.joint)
        };
        let turned = [
            HumanBone::Hips,
            HumanBone::Spine,
            HumanBone::Chest,
            HumanBone::Head,
            HumanBone::LeftUpperLeg,
            HumanBone::RightUpperLeg,
        ]
        .map(bone);
        Self {
            rig: rig.to_cooked().expect("cooked"),
            skeleton: skeleton.to_cooked().expect("cooked"),
            binds: skin.binds.clone(),
            morph_targets: imported.mesh.morph_targets.len(),
            turned,
        }
    }

    /// Her pose and springs at `quality`, with her skirt as cloth at `cloth` iterations if any.
    fn posed(&self, quality: &SpringQuality, cloth: Option<u32>) -> (CharacterPose, SpringBones) {
        let rig = CharacterRig::from_cooked(&self.rig).expect("a valid rig");
        let read = CharacterRig::read(&self.rig).expect("a valid rig");
        let skeleton = Skeleton::read(&self.skeleton).expect("a valid skeleton");
        let pose = CharacterPose::new(skeleton, &self.binds, &rig.constraints, vec![0.0; self.morph_targets]);
        let mut springs = SpringBones::new(read, &pose).expect("she has springs");
        springs.set_substeps(quality.substeps);
        springs.set_tuning(aster().springs);
        if let Some(iterations) = cloth {
            springs.set_cloth(read, &aster().cloth).expect("her skirt's chains");
            springs.set_cloth_iterations(iterations);
        }
        (pose, springs)
    }

    /// Copy `place` of her at `frame` of a run round a circle 6 m across, a quarter of a lap apart, at
    /// 5 m/s, her hips and legs swinging with the stride and her head turning to look about.
    fn run(&self, pose: &mut CharacterPose, place: usize, frame: u32) -> Affine3A {
        let seconds = frame as f32 / 60.0;
        let stride = (seconds * 1.4 * std::f32::consts::TAU).sin();
        let look = (seconds * 0.6 * std::f32::consts::TAU).sin();
        pose.reset();
        let [hips, spine, chest, head, left, right] = self.turned;
        let locals = pose.locals_mut();
        locals[hips].rotation = Quat::from_rotation_y(stride * 0.25) * locals[hips].rotation;
        locals[spine].rotation = Quat::from_rotation_x(0.15) * locals[spine].rotation;
        locals[chest].rotation = Quat::from_rotation_y(-stride * 0.3) * locals[chest].rotation;
        locals[head].rotation = Quat::from_rotation_y(look * 0.7) * locals[head].rotation;
        locals[left].rotation = Quat::from_rotation_x(-stride * 0.7) * locals[left].rotation;
        locals[right].rotation = Quat::from_rotation_x(stride * 0.7) * locals[right].rotation;
        pose.finish();
        let angle = seconds * 5.0 / 3.0 + place as f32 * std::f32::consts::FRAC_PI_2;
        let at = Vec3::new(angle.cos() * 3.0, stride.abs() * 0.04, angle.sin() * 3.0);
        Affine3A::from_rotation_translation(Quat::from_rotation_y(-angle), at)
    }
}

/// Aster's motion, as her kind's document names it: her springs' tuning and her skirt as one sheet.
fn aster() -> CharacterMotion {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/hero/hero.motion.ron");
    CharacterMotion::from_ron(&std::fs::read_to_string(path).expect("shipped")).expect("it reads")
}

/// The height of her hem at rest, in her model's space.
const HEM: f32 = 0.62;

/// Who steps in a party of `count` at `frame` on a tier: the first leads, the rest take turns as the
/// stage's schedule turns them, in their order.
fn pace(stagger: &StaggerQuality, count: usize, place: usize, frame: u32) -> SpringPace {
    if place == 0 {
        return SpringPace::Lead;
    }
    let turns = (count - 1) as u32;
    let stepping = stagger.stepping(turns);
    if stepping >= turns {
        return SpringPace::Step;
    }
    let first = (frame % turns) * stepping % turns;
    let along = (place as u32 - 1 + turns - first) % turns;
    if along < stepping { SpringPace::Step } else { SpringPace::Wait }
}

/// Three heroes running with a floor, on every tier's rotation, never go through it: on every frame a
/// character steps, every tail stays above it by its hit radius. Between, its chains ride with its body.
#[test]
fn a_party_taking_turns_keeps_above_the_floor() {
    let hero = Hero::load();
    for quality in SpringQuality::SHIPPED {
        let mut party: Vec<_> = (0..3).map(|_| hero.posed(&quality, None)).collect();
        // A floor at the height of her hem, which her skirt and ribbons swing into every stride: the
        // ground as it is to a character kneeling, every frame.
        let floor = SpringFloor { point: Vec3::new(0.0, HEM, 0.0), normal: Vec3::Y };
        let mut lowest = f32::MAX;
        let mut worst = String::new();
        // Each starts from her pose, through the floor, until her first step lifts her out of it.
        let mut stepped = [false; 3];
        for frame in 0..600 {
            for (place, (pose, springs)) in party.iter_mut().enumerate() {
                springs.set_pace(pace(&quality.stagger, 3, place, frame));
                springs.set_floor(Some(floor));
                let world = hero.run(pose, place, frame);
                let ticks = f64::from(frame);
                let frame = SpringFrame { world_from_model: world, ticks, pose_moved: true, active: true };
                let stepping = springs.update(pose, &frame).substeps > 0;
                stepped[place] |= stepping;
                // A frame that waits moves no tail: its chains ride with the body, floor and all.
                if !stepped[place] || !stepping {
                    continue;
                }
                let level = world.transform_point3(floor.point).y;
                for tail in springs.tails() {
                    let above = tail.position.y - level - tail.hit_radius;
                    if above < lowest {
                        lowest = above;
                        worst = format!(
                            "place {place}, {:?}, joint {}, tick {ticks}",
                            springs.pace(),
                            tail.joint
                        );
                    }
                }
            }
        }
        println!("{:?}: lowest tail {lowest:.5} m above the floor past its radius", quality.substeps);
        assert!(lowest >= -1e-4, "{:?}: a tail {lowest} m into the floor ({worst})", quality.substeps);
    }
}

/// The camera's height of view on the field (`default_camera.ron`'s explore `fov_y`), in radians.
const FOV_Y: f32 = 0.85;

/// Each tier's output, in rows: the desktops' render caps (spec §3.1) and the phones' native heights (§3.3),
/// which are larger than they render at, so a pop counts more pixels there.
const ROWS: [(&str, SpringQuality, f32); 4] = [
    ("Desktop-High", SpringQuality::DESKTOP_HIGH, 1440.0),
    ("Desktop-Low", SpringQuality::DESKTOP_LOW, 1080.0),
    ("Mobile-High", SpringQuality::MOBILE, 1179.0),
    ("Mobile-Low", SpringQuality::MOBILE_LOW, 1170.0),
];

/// What a tier's cloth switching at its distance adds to a running skirt's motion, in output pixels at the
/// distance it switches: dropping at the tier's distance, or coming back inside nine tenths of it, on a hero
/// running and turning, at twelve places along her stride, `eased` or at once. Two measures, each the worst
/// over every tail and place:
///
/// - `first`: on the switch's frame, how far the tail's step differs from its step in a run that never
///   switched, which until then was the same run to the bit: the pop of a switch at once;
/// - `spike`: over the frames from the switch to a quarter second past the ease, how much further the
///   tail's step ever changed from one frame to the next (a pop is a step that changes at once) than it
///   ever did over the same frames in either run that never switched, the cloth on throughout or off.
///   Runs that part keep parting, so this reads about a pixel even where nothing pops: a skirt going off
///   eased, whose links only let go.
struct Pops {
    first: f32,
    spike: f32,
}

fn pops(hero: &Hero, quality: &SpringQuality, rows: f32, comes_on: bool, eased: bool) -> Pops {
    const WARM: u32 = 120;
    let window = ((CLOTH_EASE_SECONDS + 0.25) * 60.0) as usize;
    let iterations = quality.cloth.iterations;
    let (before, after) = if comes_on { (0, iterations) } else { (iterations, 0) };
    let distance = if comes_on { quality.cloth.distance * 0.9 } else { quality.cloth.distance };
    let pixels_a_meter = rows / (2.0 * distance * (FOV_Y * 0.5).tan());
    let run = |from: u32, switch_at: Option<usize>, last: usize| {
        let (mut pose, mut springs) = hero.posed(quality, Some(from));
        let mut track = Vec::with_capacity(last + 1);
        for frame in 0..=last {
            if Some(frame) == switch_at {
                if eased {
                    springs.ease_cloth_iterations(after);
                } else {
                    springs.set_cloth_iterations(after);
                }
            }
            let world = hero.run(&mut pose, 0, frame as u32);
            let step =
                SpringFrame { world_from_model: world, ticks: frame as f64, pose_moved: true, active: true };
            springs.update(&mut pose, &step);
            track.push(springs.tails().map(|tail| tail.position).collect::<Vec<_>>());
        }
        track
    };
    let last = WARM as usize + 12 * 4 + window;
    let (old, new) = (run(before, None, last), run(after, None, last));
    let change = |track: &[Vec<Vec3>], frame: usize, tail: usize| {
        (track[frame][tail] - track[frame - 1][tail] * 2.0 + track[frame - 2][tail]).length()
    };
    let mut worst = Pops { first: 0.0, spike: 0.0 };
    for place in 0..12 {
        let at = WARM as usize + place * 4;
        let switched = run(before, Some(at), at + window);
        for tail in 0..switched[at].len() {
            let step = switched[at][tail] - switched[at - 1][tail];
            let own = old[at][tail] - old[at - 1][tail];
            worst.first = worst.first.max((step - own).length() * pixels_a_meter);
            let (mut most, mut own_most) = (0.0_f32, 0.0_f32);
            for frame in at..at + window {
                most = most.max(change(&switched, frame, tail));
                own_most = own_most.max(change(&old, frame, tail)).max(change(&new, frame, tail));
            }
            worst.spike = worst.spike.max((most - own_most) * pixels_a_meter);
        }
    }
    worst
}

/// A skirt's cloth switching off at a tier's distance, and on again inside it, as the hero runs and turns
/// (M5 C4): switched at once, the links coming back move the hem 19 to 27 pixels in a frame beyond its own
/// step; eased over [`CLOTH_EASE_SECONDS`], as the stage switches it, the switch's frame moves no tail a
/// tenth of a pixel beyond it, and no frame after changes a tail's step more than it changes on its own,
/// cloth on or off, by more than the measure's own reading where nothing pops (`Pops::spike`). Worst over
/// twelve places along her stride, each way, at each tier's output size.
#[test]
fn the_cloth_switching_at_its_distance_never_pops() {
    let hero = Hero::load();
    for (tier, quality, rows) in ROWS {
        for comes_on in [false, true] {
            let way = if comes_on { "on" } else { "off" };
            let at_once = pops(&hero, &quality, rows, comes_on, false);
            let eased = pops(&hero, &quality, rows, comes_on, true);
            println!(
                "{tier}: cloth {way}: at once {:.2} px on its frame and a {:.2} px spike; eased {:.2} px and {:.2} px",
                at_once.first, at_once.spike, eased.first, eased.spike
            );
            assert!(
                eased.first < 0.1,
                "{tier}: cloth {way} eased moves a tail {} px on its frame",
                eased.first
            );
            assert!(eased.spike < 2.0, "{tier}: cloth {way} eased jolts a tail {} px", eased.spike);
        }
    }
}

/// What a party's springs cost a frame, at each tier's settings: one hero alone, three with two taking
/// turns, and three all stepping every frame (M5 C2); then her skirt as cloth (M5 C4), one alone at 4, 2
/// and 1 iterations and at the tier's, and three taking turns, each with her skirt at the tier's. The least
/// of interleaved rounds, of whole frames of all three. Not a gate.
#[test]
#[ignore = "prints what a party's springs cost; run it in release on a machine to itself"]
#[allow(clippy::disallowed_methods, reason = "a timing test reads the OS clock, and reads nothing else")]
fn what_a_partys_springs_cost() {
    use std::hint::black_box;
    use std::time::Instant;

    const FRAMES: u32 = 1_200;
    const ROUNDS: u32 = 7;
    /// A run's cloth at the tier's iterations.
    const TIERS: u32 = u32::MAX;
    let hero = Hero::load();
    let tiers = [
        ("Desktop-High", SpringQuality::DESKTOP_HIGH),
        ("Desktop-Low", SpringQuality::DESKTOP_LOW),
        ("Mobile-High", SpringQuality::MOBILE),
        ("Mobile-Low", SpringQuality::MOBILE_LOW),
    ];
    // (name, how many, everyone leads, cloth iterations)
    let runs: [(&str, usize, bool, Option<u32>); 9] = [
        ("one alone", 1, true, None),
        ("three taking turns", 3, false, None),
        ("three every frame", 3, true, None),
        ("three taking turns, with a floor", 3, false, None),
        ("one alone, skirt as cloth at 4", 1, true, Some(4)),
        ("one alone, skirt as cloth at 2", 1, true, Some(2)),
        ("one alone, skirt as cloth at 1", 1, true, Some(1)),
        ("one alone, skirt as cloth at the tier's", 1, true, Some(TIERS)),
        ("three taking turns, skirts as cloth at the tier's", 3, false, Some(TIERS)),
    ];
    let floor = SpringFloor { point: Vec3::new(0.0, HEM, 0.0), normal: Vec3::Y };
    for (tier, quality) in tiers {
        let mut least = [f64::MAX; 9];
        let mut steps = [0_u32; 9];
        for _ in 0..ROUNDS {
            for (index, &(name, count, all_lead, cloth)) in runs.iter().enumerate() {
                let cloth =
                    cloth.map(
                        |iterations| if iterations == TIERS { quality.cloth.iterations } else { iterations },
                    );
                let mut party: Vec<_> = (0..count).map(|_| hero.posed(&quality, cloth)).collect();
                let floored = name.ends_with("floor");
                let mut spent = 0.0;
                steps[index] = 0;
                for frame in 0..FRAMES {
                    for (place, (pose, springs)) in party.iter_mut().enumerate() {
                        let world = hero.run(pose, place, frame);
                        let paced = if all_lead {
                            SpringPace::Lead
                        } else {
                            pace(&quality.stagger, count, place, frame)
                        };
                        let started = Instant::now();
                        springs.set_pace(paced);
                        springs.set_floor(floored.then_some(floor));
                        let frame = SpringFrame {
                            world_from_model: world,
                            ticks: f64::from(frame),
                            pose_moved: true,
                            active: true,
                        };
                        let work = springs.update(pose, &frame);
                        spent += started.elapsed().as_secs_f64();
                        steps[index] += work.substeps;
                        black_box(pose.model());
                    }
                }
                least[index] = least[index].min(spent);
            }
        }
        for (index, (name, ..)) in runs.iter().enumerate() {
            println!(
                "{tier} ({:?}, {:?}): {name}: {:.1} µs a frame, {} sub-steps a frame",
                quality.substeps,
                quality.stagger,
                least[index] * 1e6 / f64::from(FRAMES),
                steps[index] / FRAMES,
            );
        }
    }
}
