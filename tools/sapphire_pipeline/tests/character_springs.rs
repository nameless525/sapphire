//! Spring bones on the character itself: its own chains and colliders, as the VRM says them, under its
//! own strike, as its document says it (M2 N5). `sapphire_anim` tests its own promises on small rigs; this
//! is whether they hold on what ships, which needs no GPU and no cook.

#![allow(clippy::print_stdout, reason = "the measurements here are worth reading with --nocapture")]

use std::path::Path;

use sapphire_anim::{
    ArchivedClip, ArchivedSkeleton, CharacterPose, Clip, DEFAULT_ROOT_INERTIA, Skeleton, SpringBones,
    SpringFrame, SpringQuality, Substeps, root_motion,
};
use sapphire_assets::AssetId;
use sapphire_assets::rig::{ArchivedCharacterRig, CharacterRig};
use sapphire_math::{Affine3A, Vec3};
use sapphire_pipeline::clip::retarget::HumanMap;
use sapphire_pipeline::clip::{ClipTarget, ReductionError, cook};
use sapphire_pipeline::import::vrm;
use sapphire_pipeline::keypose;
use sapphire_serialize::cooked::AlignedVec;

/// The character, cooked in memory, as the app reads it.
struct Character {
    rig: AlignedVec,
    skeleton: AlignedVec,
    clip: AlignedVec,
    binds: Vec<sapphire_assets::mesh::SkinBind>,
    morph_targets: usize,
}

impl Character {
    fn load() -> Self {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let vrm_bytes = std::fs::read(
            content.join("third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm"),
        )
        .expect("the character");
        let rig = vrm::import_rig(&vrm_bytes, None).expect("its rig");
        let imported = vrm::import(&vrm_bytes, None).expect("the character");
        let skin = imported.mesh.skin.as_ref().expect("skinned");
        let skeleton = Skeleton::new(&skin.joints);
        let document =
            std::fs::read(content.join("lookdev/character/strike.keypose.ron")).expect("the strike");
        let document = keypose::read(&document).expect("it reads");
        let sampled = keypose::sample(&document, "strike", &skeleton, &HumanMap::from_rig(&rig), &[])
            .expect("it samples");
        let target =
            ClipTarget { skeleton: Some((AssetId::from_bytes([7; 16]), &skeleton)), morph_defaults: &[] };
        let clip = cook(&sampled, &target, &ReductionError::default()).to_cooked().expect("cooked");
        Self {
            rig: rig.to_cooked().expect("cooked"),
            skeleton: skeleton.to_cooked().expect("cooked"),
            clip,
            binds: skin.binds.clone(),
            morph_targets: imported.mesh.morph_targets.len(),
        }
    }

    fn rig(&self) -> &ArchivedCharacterRig {
        CharacterRig::read(&self.rig).expect("a valid rig")
    }

    fn skeleton(&self) -> &ArchivedSkeleton {
        Skeleton::read(&self.skeleton).expect("a valid skeleton")
    }

    fn clip(&self) -> &ArchivedClip {
        Clip::read(&self.clip).expect("a valid clip")
    }

    /// A pose with its constraints, and its springs at `substeps`.
    fn posed(&self, substeps: Substeps) -> (CharacterPose, SpringBones) {
        let rig = CharacterRig::from_cooked(&self.rig).expect("a valid rig");
        let pose =
            CharacterPose::new(self.skeleton(), &self.binds, &rig.constraints, vec![0.0; self.morph_targets]);
        let mut springs = SpringBones::new(self.rig(), &pose).expect("it has springs");
        springs.set_substeps(substeps);
        (pose, springs)
    }
}

/// The clip at `ticks`, and the springs run up to then: what `Evaluation::run` does. Where it stands.
fn frame(
    pose: &mut CharacterPose,
    springs: &mut SpringBones,
    clip: &ArchivedClip,
    ticks: f64,
    speed: f32,
) -> Affine3A {
    let position = (ticks as f32 * speed).min(f32::from(clip.ticks.to_native()));
    pose.sample(clip, position);
    pose.finish();
    let root = root_motion(clip, position).map_or(Affine3A::IDENTITY, |root| root.to_affine3a());
    springs.update(pose, &SpringFrame { world_from_model: root, ticks, pose_moved: true, active: true });
    root
}

/// Every swinging joint's name and which colliders its chain keeps it out of.
fn chains(character: &Character, springs: &SpringBones) -> Vec<(String, Vec<usize>)> {
    let rig = character.rig();
    let skeleton = character.skeleton();
    springs
        .tails()
        .map(|tail| {
            let spring = rig
                .springs
                .iter()
                .find(|spring| spring.joints.iter().any(|entry| entry.joint.to_native() == tail.joint))
                .expect("its chain");
            let colliders = spring
                .collider_groups
                .iter()
                .flat_map(|group| rig.collider_groups[group.to_native() as usize].colliders.iter())
                .map(|collider| collider.to_native() as usize)
                .collect();
            (skeleton.names[usize::from(tail.joint)].to_string(), colliders)
        })
        .collect()
}

/// The strike at its own speed and at up to seven times that, an arm far faster than any move: on every
/// sub-step count that ships, with the root inertia that ships and with all of it, at every sub-step, a
/// tail stays out of every collider of its chain, past whatever its rest pose has inside one. The
/// exception is a tail caught between two, as a strand between the chest and a lowered arm: it can't
/// clear both, and clears the body first (the rig's colliders overlap there), going into the arm by no
/// more than the arm's own radius.
#[test]
fn the_characters_hair_stays_out_of_its_colliders_through_a_fast_strike() {
    /// How far past a collider's surface counts as nothing: a capsule's nearest point moves under the
    /// tail, and a later collider can move it after an earlier cleared it.
    const CLEAR: f32 = 0.004;
    /// How far a tail may be in a limb it is squeezed against: about the upper arm's radius.
    const SQUEEZED: f32 = 0.05;
    let character = Character::load();
    let clip = character.clip();
    let settings = SpringQuality::SHIPPED
        .into_iter()
        .flat_map(|quality| [DEFAULT_ROOT_INERTIA, 1.0].map(|share| (quality.substeps, share)));
    for (substeps, share) in settings {
        let count = substeps.count();
        for fast in [1.0_f32, 3.0, 7.0] {
            let (mut pose, mut springs) = character.posed(substeps);
            springs.set_root_inertia(share);
            let chains = chains(&character, &springs);
            // Every sub-step gets a frame of its own, so all of them are looked at.
            let frames = (60.0 / fast) as u32 * count;
            let mut at_rest = Vec::new();
            let (mut worst_alone, mut worst_squeezed) = (0.0_f32, 0.0_f32);
            for index in 0..=frames {
                let ticks = f64::from(index) / f64::from(count);
                frame(&mut pose, &mut springs, clip, ticks, fast);
                let tails: Vec<_> = springs.tails().collect();
                let colliders: Vec<_> = springs.colliders().collect();
                if index == 0 {
                    at_rest = tails
                        .iter()
                        .zip(&chains)
                        .map(|(tail, (_, ids))| {
                            ids.iter()
                                .map(|&id| tail.hit_radius - colliders[id].distance(tail.position))
                                .fold(0.0_f32, f32::max)
                        })
                        .collect();
                }
                for (at, (tail, (name, ids))) in tails.iter().zip(&chains).enumerate() {
                    let inside = |id: usize| tail.hit_radius - colliders[id].distance(tail.position);
                    for &id in ids {
                        let deep = inside(id) - at_rest[at];
                        let squeezed = ids.iter().any(|&other| other != id && inside(other) > -CLEAR);
                        if squeezed {
                            worst_squeezed = worst_squeezed.max(deep);
                            assert!(
                                deep < SQUEEZED,
                                "{substeps:?}, inertia {share}, {fast}×, frame {index}: {name} is {deep} m in \
                                 collider {id}, caught between it and another"
                            );
                        } else {
                            worst_alone = worst_alone.max(deep);
                            assert!(
                                deep < CLEAR,
                                "{substeps:?}, inertia {share}, {fast}×, frame {index}: {name} is {deep} m in \
                                 collider {id}"
                            );
                        }
                    }
                }
            }
            println!(
                "{substeps:?}, inertia {share}, {fast}×: {} mm in at most alone, {} mm between two",
                worst_alone * 1e3,
                worst_squeezed * 1e3
            );
        }
    }
}

/// As it ships, the character's hair and cloth come well off the pose during the strike and back to it
/// after: measured against a twin with no springs, tail against the joint below it.
#[test]
fn the_characters_hair_and_cloth_lag_the_strike_and_settle() {
    let character = Character::load();
    let clip = character.clip();
    let rig = character.rig();
    for quality in SpringQuality::SHIPPED {
        let (mut pose, mut springs) = character.posed(quality.substeps);
        let (mut twin, _) = character.posed(quality.substeps);
        assert_eq!(springs.swing_count(), 57, "22 chains, each ending in a joint");
        let names = chains(&character, &springs);
        let below: Vec<usize> = springs
            .tails()
            .map(|tail| {
                rig.springs
                    .iter()
                    .flat_map(|spring| spring.joints.iter().zip(spring.joints.iter().skip(1)))
                    .find(|(joint, _)| joint.joint.to_native() == tail.joint)
                    .map(|(_, next)| usize::from(next.joint.to_native()))
                    .expect("every chain ends in a joint")
            })
            .collect();
        let mut most = vec![0.0_f32; below.len()];
        let mut last = Vec::new();
        for tick in 0..=300 {
            let world = frame(&mut pose, &mut springs, clip, f64::from(tick), 1.0);
            twin.sample(clip, (tick as f32).min(f32::from(clip.ticks.to_native())));
            twin.finish();
            last = springs
                .tails()
                .zip(&below)
                .map(|(tail, &next)| {
                    let posed = Vec3::from(twin.model()[next].translation);
                    tail.position.distance(world.transform_point3(posed))
                })
                .collect();
            for (most, off) in most.iter_mut().zip(&last) {
                *most = most.max(*off);
            }
        }
        // Gravity and colliders hold a settled tail a little off the pose.
        for (kind, is_kind) in [
            ("hair", (|name: &str| name.contains("Hair")) as fn(&str) -> bool),
            ("cloth", |name: &str| !name.contains("Hair")),
        ] {
            let of = |values: &[f32]| -> f32 {
                values
                    .iter()
                    .zip(&names)
                    .filter(|(_, (name, _))| is_kind(name))
                    .map(|(value, _)| *value)
                    .fold(0.0, f32::max)
            };
            let (swung, settled) = (of(&most), of(&last));
            println!("{quality:?}: {kind} came {swung} m off the pose, and settled at {settled} m");
            assert!(swung > 0.05, "{quality:?}: the {kind} barely swung: {swung} m");
            assert!(
                swung > 2.0 * settled,
                "{quality:?}: the {kind} swung {swung} m and settled at {settled} m"
            );
        }
    }
}

/// Standing still, the character's hair and cloth settle with no jitter and stop, whatever they start
/// from, and draw near the pose: gravity and colliders are all that move them off it.
#[test]
fn the_character_settles_with_no_jitter_and_stops() {
    let character = Character::load();
    let clip = character.clip();
    for substeps in [Substeps::One, Substeps::Two] {
        let (mut pose, mut springs) = character.posed(substeps);
        for tick in 0..30 {
            frame(&mut pose, &mut springs, clip, f64::from(tick), 1.0);
        }
        let world = frame(&mut pose, &mut springs, clip, 30.0, 1.0);
        let mut last: Vec<Vec3> = springs.tails().map(|tail| tail.position).collect();
        let mut stopped = None;
        for tick in 31..1_800 {
            let work = springs.update(
                &mut pose,
                &SpringFrame {
                    world_from_model: world,
                    ticks: f64::from(tick),
                    pose_moved: false,
                    active: true,
                },
            );
            let now: Vec<Vec3> = springs.tails().map(|tail| tail.position).collect();
            let moved = now.iter().zip(&last).map(|(a, b)| a.distance(*b)).fold(0.0_f32, f32::max);
            if tick > 900 {
                assert!(moved < 1e-4, "{substeps:?}: still moving {moved} m a tick at {tick}");
            }
            last = now;
            if !work.wrote_pose {
                stopped = Some(tick);
                break;
            }
        }
        let stopped = stopped.expect("it stopped");
        println!("{substeps:?}: stopped {} ticks after the pose held", stopped - 30);
        assert!(springs.is_resting());
    }
}

/// What the character's springs cost a frame on this machine, with what the second `finish` is of it,
/// against what a pass over only the joints below the chains could save. Not a gate. Run it in release:
/// `cargo test --release -p sapphire_pipeline --test character_springs -- --ignored --nocapture`.
#[test]
#[ignore = "prints what the springs cost; run it in release"]
#[allow(clippy::disallowed_methods, reason = "a timing test reads the OS clock, and reads nothing else")]
fn what_the_characters_springs_cost() {
    use std::hint::black_box;
    use std::time::Instant;

    let character = Character::load();
    let clip = character.clip();
    const FRAMES: u32 = 20_000;
    for substeps in [Substeps::One, Substeps::Two] {
        // Frames only through the strike, where the colliders and motion are.
        let (mut pose, mut springs) = character.posed(substeps);
        let mut sampling = 0.0;
        let mut whole = 0.0;
        for index in 0..FRAMES {
            let ticks = f64::from(index);
            let position = (index % 60) as f32;
            let started = Instant::now();
            pose.sample(clip, position);
            pose.finish();
            sampling += started.elapsed().as_secs_f64();
            let started = Instant::now();
            springs.update(
                &mut pose,
                &SpringFrame { world_from_model: Affine3A::IDENTITY, ticks, pose_moved: true, active: true },
            );
            whole += started.elapsed().as_secs_f64();
            black_box(pose.model());
        }
        let started = Instant::now();
        for _ in 0..FRAMES {
            pose.finish();
            black_box(pose.model());
        }
        let finish = started.elapsed().as_secs_f64();
        let micros = |seconds: f64| seconds * 1e6 / f64::from(FRAMES);
        println!(
            "{substeps:?}: {} joints, {} swinging. Sampling and finishing the clip {:.2} µs; the springs at a \
             frame a tick {:.2} µs, of which the second finish {:.2} µs",
            pose.joint_count(),
            springs.swing_count(),
            micros(sampling),
            micros(whole),
            micros(finish),
        );
    }
}
