//! Spring bones on the character itself: its own chains and colliders, as the VRM says them, under its
//! own strike, as its document says it (M2 N5). `sapphire_anim` tests its own promises on small rigs; this
//! is whether they hold on what ships, which needs no GPU and no cook.

#![allow(clippy::print_stdout, reason = "the measurements here are worth reading with --nocapture")]

use std::path::Path;

use sapphire_anim::{
    ArchivedClip, ArchivedSkeleton, CharacterPose, Clip, Skeleton, SpringBones, SpringFrame, SpringQuality,
    SpringTuning, Substeps, root_motion,
};
use sapphire_assets::AssetId;
use sapphire_assets::rig::{ArchivedCharacterRig, CharacterRig};
use sapphire_math::{Affine3A, Vec3};
use sapphire_pipeline::clip::retarget::{HumanMap, RetargetOptions, Rig, retarget};
use sapphire_pipeline::clip::source::ClipSource;
use sapphire_pipeline::clip::{ClipTarget, ReductionError, cook};
use sapphire_pipeline::import::gltf::clips;
use sapphire_pipeline::import::vrm;
use sapphire_pipeline::keypose;
use sapphire_pipeline::meta::{self, sidecar_path};
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

    /// One of the character's key-pose documents, cooked as its sidecar says.
    fn key_pose_clip(&self, document: &str) -> AlignedVec {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let bytes = std::fs::read(content.join(format!("lookdev/character/{document}.keypose.ron")))
            .expect("the document");
        let document_read = keypose::read(&bytes).expect("it reads");
        let rig = CharacterRig::from_cooked(&self.rig).expect("a valid rig");
        let skeleton = Skeleton::from_cooked(&self.skeleton).expect("a valid skeleton");
        let sampled = keypose::sample(&document_read, document, &skeleton, &HumanMap::from_rig(&rig), &[])
            .expect("it samples");
        let target =
            ClipTarget { skeleton: Some((AssetId::from_bytes([7; 16]), &skeleton)), morph_defaults: &[] };
        cook(&sampled, &target, &ReductionError::default()).to_cooked().expect("cooked")
    }

    /// One of KayKit's clips on the character, cooked as its sidecars say.
    fn kaykit_clip(&self, file: &str, name: &str) -> AlignedVec {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kaykit_character_animations/Rig_Medium");
        let path = folder.join(format!("Rig_Medium_{file}.glb"));
        let owner =
            meta::read(&sidecar_path(&folder.join("Rig_Medium_General.glb"))).expect("the rig's sidecar");
        let sidecar = meta::read(&sidecar_path(&path)).expect("its sidecar");
        let bytes = std::fs::read(&path).expect("the file");
        let source = ClipSource::import(&path, &bytes, &sidecar.importer).expect("it imports");
        let settings = sidecar.importer.clips().expect("it has clips");
        let index = clips::names(&bytes).expect("names").iter().position(|n| n == name).expect(name);
        let sampled = source.sample(index, settings).expect("it samples");
        let joints = Skeleton::new(&source.joints);
        let map =
            HumanMap::from_names(&owner.importer.clips().expect("clips").humanoid, &joints).expect("a map");
        let entry = settings
            .retarget
            .iter()
            .find(|entry| entry.names.iter().any(|candidate| candidate == name))
            .expect("it retargets");
        let rig = CharacterRig::from_cooked(&self.rig).expect("a valid rig");
        let character_map = HumanMap::from_rig(&rig);
        let character = Skeleton::from_cooked(&self.skeleton).expect("a valid skeleton");
        let out = retarget(
            &sampled,
            Rig { skeleton: &joints, map: &map },
            Rig { skeleton: &character, map: &character_map },
            &RetargetOptions {
                scale: entry.scale,
                ground: entry.ground,
                adjust: entry.adjust.clone(),
                gain: entry.gain.clone(),
                follow: entry.follow.clone(),
            },
        )
        .expect("it retargets");
        let target =
            ClipTarget { skeleton: Some((AssetId::from_bytes([7; 16]), &character)), morph_defaults: &[] };
        cook(&out, &target, &settings.error).to_cooked().expect("cooked")
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
/// sub-step count that ships, with the limits off and the shipped root inertia or all of it, at every
/// sub-step, a tail stays out of every collider of its chain, past whatever its rest pose has inside
/// one. The exception is a tail caught between two, as a strand between the chest and a lowered arm: it
/// can't clear both, and clears the body first (the rig's colliders overlap there), going into the arm
/// by no more than the arm's own radius.
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
        .flat_map(|quality| [SpringTuning::ACTION.root_inertia, 1.0].map(|share| (quality.substeps, share)));
    for (substeps, share) in settings {
        let count = substeps.count();
        for fast in [1.0_f32, 3.0, 7.0] {
            let (mut pose, mut springs) = character.posed(substeps);
            springs.set_tuning(SpringTuning { root_inertia: share, ..SpringTuning::VRM });
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

/// What a clip does to the chains, measured against a twin with no springs: the worst of each.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    /// How far a joint points from where the animated pose points it, in degrees.
    swing: f32,
    /// How much more a joint bends from the one above it than the animated pose has it, in degrees.
    curl: f32,
    /// How far a tail is from where the animated pose has it, in meters.
    off: f32,
    /// How far a tail's motion from one tick to the next differs from its twin's, in meters a tick.
    jolt: f32,
    /// How far a tail was from the pose at the end, where it has come back to.
    settled: f32,
    /// How far into its chain's colliders a tail goes, past where the pose starts it, in meters.
    inside: f32,
}

impl Shape {
    fn take(&mut self, other: &Self) {
        self.swing = self.swing.max(other.swing);
        self.curl = self.curl.max(other.curl);
        self.off = self.off.max(other.off);
        self.jolt = self.jolt.max(other.jolt);
        self.settled = self.settled.max(other.settled);
        self.inside = self.inside.max(other.inside);
    }
}

/// A clip by name, as `Character` cooks it.
enum Source {
    Strike,
    Idle(&'static str),
    Kaykit(&'static str, &'static str),
}

/// The clips a fight is mostly made of, each with how fast a game runs the character across the world
/// under it, from standing, in meters a second, and how many times it plays. A clip that doesn't loop
/// plays once.
const CLIPS: [(&str, Source, f32, u32); 7] = [
    ("the run", Source::Kaykit("MovementBasic", "Running_B"), 0.0, 4),
    ("the run, travelling", Source::Kaykit("MovementBasic", "Running_B"), 6.0, 4),
    ("the walk", Source::Kaykit("MovementBasic", "Walking_B"), 1.5, 3),
    ("the combat idle", Source::Idle("idle_combat"), 0.0, 2),
    ("the hit", Source::Kaykit("General", "Hit_B"), 0.0, 1),
    ("the dodge", Source::Kaykit("MovementAdvanced", "Dodge_Forward"), 0.0, 1),
    ("the strike", Source::Strike, 0.0, 1),
];

/// Hair, then cloth.
fn shape_of(
    character: &Character,
    clip: &ArchivedClip,
    substeps: Substeps,
    tuning: SpringTuning,
    travel: f32,
    plays: u32,
) -> [Shape; 2] {
    let (mut pose, mut springs) = character.posed(substeps);
    let (mut twin, _) = character.posed(substeps);
    springs.set_tuning(tuning);
    let names = chains(character, &springs);
    let rig = character.rig();
    let chain: Vec<(usize, usize)> = springs
        .tails()
        .map(|tail| {
            rig.springs
                .iter()
                .flat_map(|spring| spring.joints.iter().zip(spring.joints.iter().skip(1)))
                .find(|(joint, _)| joint.joint.to_native() == tail.joint)
                .map(|(joint, next)| {
                    (usize::from(joint.joint.to_native()), usize::from(next.joint.to_native()))
                })
                .expect("every chain ends in a joint")
        })
        .collect();
    let toward = |pose: &CharacterPose, from: usize, to: usize| {
        (Vec3::from(pose.model()[to].translation) - Vec3::from(pose.model()[from].translation)).normalize()
    };
    let length = f32::from(clip.ticks.to_native());
    let per_tick = clip.rate.to_native() as f32 / 60.0;
    let mut shapes = [Shape::default(); 2];
    let mut travelled = 0.0;
    let mut motion = vec![(Vec3::ZERO, Vec3::ZERO); chain.len()];
    let mut was = vec![(Vec3::ZERO, Vec3::ZERO); chain.len()];
    let mut inside_at_start = vec![0.0_f32; chain.len()];
    let playing = (length / per_tick) as u32 * plays;
    // Then held, for it to settle.
    let ticks = playing + 300;
    for tick in 0..ticks {
        let ending = tick >= playing;
        let position = if ending { length } else { (tick as f32 * per_tick) % length };
        pose.sample(clip, position);
        pose.finish();
        twin.sample(clip, position);
        twin.finish();
        let speed = if ending { 0.0 } else { travel * (tick as f32 / 30.0).min(1.0) };
        travelled += speed / 60.0;
        let world = Affine3A::from_translation(Vec3::new(0.0, 0.0, travelled))
            * root_motion(clip, position).map_or(Affine3A::IDENTITY, |root| root.to_affine3a());
        springs.update(
            &mut pose,
            &SpringFrame { world_from_model: world, ticks: f64::from(tick), pose_moved: true, active: true },
        );
        let colliders: Vec<_> = springs.colliders().collect();
        for ((at, tail), (_, ids)) in springs.tails().enumerate().zip(&names) {
            let inside = ids
                .iter()
                .map(|&id| tail.hit_radius - colliders[id].distance(tail.position))
                .fold(0.0_f32, f32::max);
            if tick == 0 {
                inside_at_start[at] = inside;
            }
            let shape = &mut shapes[usize::from(!names[at].0.contains("Hair"))];
            shape.inside = shape.inside.max(inside - inside_at_start[at]);
        }
        for (at, &(joint, next)) in chain.iter().enumerate() {
            let shape = &mut shapes[usize::from(!names[at].0.contains("Hair"))];
            let (ours, posed) = (toward(&pose, joint, next), toward(&twin, joint, next));
            shape.swing = shape.swing.max(ours.angle_between(posed).to_degrees());
            if let Some(parent) = pose.parent(joint) {
                let bend = toward(&pose, parent, joint).angle_between(ours);
                let posed_bend = toward(&twin, parent, joint).angle_between(posed);
                shape.curl = shape.curl.max((bend - posed_bend).to_degrees());
            }
            let (tail, posed_tail) =
                (Vec3::from(pose.model()[next].translation), Vec3::from(twin.model()[next].translation));
            let off = tail.distance(posed_tail);
            shape.off = shape.off.max(off);
            if tick == ticks - 1 {
                shape.settled = shape.settled.max(off);
            }
            let now = (tail - was[at].0, posed_tail - was[at].1);
            if tick > 1 {
                let change = (now.0 - motion[at].0) - (now.1 - motion[at].1);
                shape.jolt = shape.jolt.max(change.length());
            }
            motion[at] = now;
            was[at] = (tail, posed_tail);
        }
    }
    shapes
}

/// The worst any clip of [`CLIPS`] does to the chains, on any sub-step count that ships.
fn worst_of_every_clip(character: &Character, tuning: SpringTuning) -> [Shape; 2] {
    let mut worst = [Shape::default(); 2];
    for (name, source, travel, plays) in &CLIPS {
        let bytes = match source {
            Source::Strike => character.clip.clone(),
            Source::Idle(document) => character.key_pose_clip(document),
            Source::Kaykit(file, name) => character.kaykit_clip(file, name),
        };
        let clip = Clip::read(&bytes).expect("a valid clip");
        for quality in SpringQuality::SHIPPED {
            let [hair, cloth] = shape_of(character, clip, quality.substeps, tuning, *travel, *plays);
            println!("{name}, {:?}: hair {hair:?}, cloth {cloth:?}", quality.substeps);
            worst[0].take(&hair);
            worst[1].take(&cloth);
        }
    }
    worst
}

/// As it ships, the character's chains keep their shape on everything a fight does: a joint stays near
/// where the animated pose points it, a strand bends in a curve and never curls, tails stay near the
/// pose and they come back to it.
#[test]
fn the_characters_chains_keep_their_shape_through_a_fight() {
    let character = Character::load();
    let [hair, cloth] = worst_of_every_clip(&character, SpringTuning::ACTION);
    println!("as it ships: hair {hair:?}, cloth {cloth:?}");
    // What the drawn frame shows, between two sub-steps that each hold to the limits.
    let swing_limit = SpringTuning::ACTION.swing_limit.to_degrees() + 1.0;
    let bend_limit = SpringTuning::ACTION.bend_limit.to_degrees() + 1.0;
    // At rest what holds hair off the pose is the body: a strand lying over an arm the walk ends across it.
    for (name, shape, off, settled) in [("hair", hair, 0.35, 0.25), ("cloth", cloth, 0.07, 0.01)] {
        assert!(shape.swing <= swing_limit, "{name} swung {}° off the pose", shape.swing);
        assert!(shape.curl <= bend_limit, "{name} curled {}° past the pose", shape.curl);
        assert!(shape.off <= off, "{name}'s tails came {} m off the pose", shape.off);
        assert!(shape.settled <= settled, "{name}'s tails settled {} m off the pose", shape.settled);
    }
    // A strand can be overrun where its limits leave it no room: at worst about an arm's reach, so an
    // arm goes through it rather than past it.
    assert!(hair.inside <= 0.09, "hair went {} m into a collider", hair.inside);
    assert!(hair.jolt <= 0.2, "a tail's motion jumped {} m in a tick", hair.jolt);
    assert!(hair.off > 0.1, "and it still swings: {} m", hair.off);
    assert!(cloth.off > 0.02, "and it still swings: {} m", cloth.off);
}

/// With the tuning off, VRM's step alone breaks all of that on the run: this is what the tuning is for.
#[test]
fn vrms_step_alone_flings_the_characters_hair_on_the_run() {
    let character = Character::load();
    let bytes = character.kaykit_clip("MovementBasic", "Running_B");
    let run = Clip::read(&bytes).expect("a valid clip");
    let vrm = SpringTuning { root_inertia: SpringTuning::ACTION.root_inertia, ..SpringTuning::VRM };
    let [hair, cloth] = shape_of(&character, run, Substeps::Two, vrm, 0.0, 4);
    let [tuned_hair, tuned_cloth] = shape_of(&character, run, Substeps::Two, SpringTuning::ACTION, 0.0, 4);
    println!("VRM's step: hair {hair:?}, cloth {cloth:?}");
    println!("tuned: hair {tuned_hair:?}, cloth {tuned_cloth:?}");
    assert!(hair.swing > 150.0 && hair.curl > 90.0 && hair.off > 0.5, "{hair:?}");
    assert!(cloth.swing > 100.0, "{cloth:?}");
    assert!(tuned_hair.off < hair.off * 0.6, "{tuned_hair:?} against {hair:?}");
    assert!(tuned_cloth.off < cloth.off * 0.5, "{tuned_cloth:?} against {cloth:?}");
}

/// A run has no root motion of its own, so the frame the character draws is the same in place. The
/// travel only counts where a game moves the character, by the root inertia.
#[test]
fn the_run_has_no_root_motion_of_its_own() {
    let character = Character::load();
    let bytes = character.kaykit_clip("MovementBasic", "Running_B");
    let run = Clip::read(&bytes).expect("a valid clip");
    let length = f32::from(run.ticks.to_native());
    let root = root_motion(run, length).map_or(Vec3::ZERO, |root| root.translation);
    assert!(root.length() < 1e-3, "{root:?}");
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
