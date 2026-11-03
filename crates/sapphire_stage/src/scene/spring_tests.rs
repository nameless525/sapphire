//! Spring bones through a scene (M2 N5): the path through [`Scene::animate`], which has a key of its own.
//! What they compute is `sapphire_anim`'s to test, and on the shipped character `sapphire_pipeline`'s.

use core::f32::consts::{FRAC_1_SQRT_2, PI};

use sapphire_anim::{
    CharacterPose, Clip, Joint, JointTransform, PackedRotation, RotationTrack, Skeleton, SpringBones,
    SpringQuality, Substeps, VectorRange, VectorTrack,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{DEFAULT_RATE, RootMotion};
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{CharacterRig, Spring, SpringJoint, SpringTip};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_render::MeshHandle;
use sapphire_time::ScaleSource;

use super::*;
use crate::animate::ClipPlayer;
use crate::extract::RenderPose;
use crate::lookdev::FigureJoints;
use crate::orbit::OrbitCamera;

const JOINTS: usize = 8;
const STRAND_END: usize = 7;
const BOUNDS: [[f32; 3]; 2] = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];

fn skeleton() -> Skeleton {
    let joint = |name: &str, parent: Option<u16>, translation: [f32; 3]| Joint {
        name: name.to_owned(),
        parent,
        rest: JointTransform { translation, ..JointTransform::IDENTITY },
    };
    Skeleton::new(&[
        joint("hips", None, [0.0, 0.8, 0.0]),
        joint("spine", Some(0), [0.0, 0.4, 0.0]),
        joint("head", Some(1), [0.0, 0.3, 0.0]),
        joint("left foot", Some(0), [0.1, -0.7, 0.0]),
        joint("right foot", Some(0), [-0.1, -0.7, 0.0]),
        joint("strand 1", Some(2), [0.0, 0.0, -0.1]),
        joint("strand 2", Some(5), [0.0, -0.2, 0.0]),
        joint("strand end", Some(6), [0.0, -0.2, 0.0]),
    ])
}

/// Over 60 ticks: the head turning a quarter about Y and 3 m of root motion.
fn clip() -> Clip {
    clip_travelling(3.0)
}

/// Over 60 ticks: the head turning a quarter about Y and `travel` meters of root motion.
fn clip_travelling(travel: f32) -> Clip {
    let root = VectorRange::enclosing([[0.0; 3], [0.0, 0.0, travel]]);
    Clip {
        name: "turn".into(),
        skeleton: Some(AssetId::from_bytes([9; 16])),
        joint_count: JOINTS as u32,
        rate: DEFAULT_RATE,
        ticks: 60,
        rotations: vec![RotationTrack { joint: 2, count: 2, first: 0 }],
        translations: Vec::new(),
        root: Some(RootMotion {
            joint: 0,
            translation: VectorTrack { joint: 0, count: 2, first: 0, range: root },
            rotation: RotationTrack { joint: 0, count: 1, first: 2 },
        }),
        rotation_ticks: vec![0, 60, 0],
        rotation_keys: vec![
            PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
            PackedRotation::encode([0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2]),
            PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
        ],
        vector_ticks: vec![0, 60],
        vector_keys: vec![root.encode([0.0; 3]), root.encode([0.0, 0.0, travel])],
        ..Clip::default()
    }
}

/// A clip of no ticks: the character holds it, so its position repeats however the clock runs.
fn held() -> Clip {
    Clip {
        name: "held".into(),
        skeleton: Some(AssetId::from_bytes([9; 16])),
        joint_count: JOINTS as u32,
        rate: DEFAULT_RATE,
        ..Clip::default()
    }
}

fn player(springs: bool) -> ClipPlayer {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let binds: Vec<SkinBind> = (0..JOINTS as u16)
        .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
        .collect();
    let pose = CharacterPose::new(skeleton, &binds, &[], Vec::new());
    let entry = |joint: u16| SpringJoint {
        joint,
        hit_radius: 0.0,
        stiffness: 0.5,
        gravity_power: 0.0,
        gravity_dir: [0.0, -1.0, 0.0],
        drag_force: 0.4,
    };
    let rig = CharacterRig {
        joint_count: JOINTS as u32,
        springs: vec![Spring {
            name: "strand".into(),
            joints: vec![entry(5), entry(6), entry(7)],
            tip: SpringTip::Joint,
            collider_groups: Vec::new(),
            center: None,
        }],
        ..CharacterRig::default()
    };
    let rig_bytes = rig.to_cooked().expect("cooked");
    let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
    let made = SpringBones::new(rig, &pose);
    let rest = CharacterPose::new(skeleton, &[], &[], Vec::new());
    ClipPlayer::new(pose, FigureJoints::for_test(&rest, 2, 0.0, [3, 4]))
        .with_springs(made)
        .with_springs_on(springs)
}

/// The look-dev stage, with the character playing `player`.
fn stage_playing(player: ClipPlayer) -> Scene {
    let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
    let pose = RenderPose { palette: vec![Affine3A::IDENTITY; JOINTS], morph_weights: Vec::new() };
    let figure = Figure {
        head: Vec3::new(0.0, 1.6, 0.0),
        feet: Vec3::ZERO,
        body_radius: 0.16,
        face: Quat::IDENTITY,
        expression: FaceExpression::default(),
    };
    let town = TownMeshes {
        town: MeshHandle::from_raw(1),
        plants: [MeshHandle::from_raw(2); 8],
        mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
    };
    scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose, figure });
    scene.play_clip(player);
    scene
}

fn stage(springs: bool) -> Scene {
    stage_playing(player(springs))
}

/// One frame: the palette its packet sends, if it sends one.
fn frame(scene: &mut Scene, clip: &ArchivedClip, alpha: f32) -> Option<Vec<[f32; 12]>> {
    scene.animate_clip(Some((clip, 0)), alpha);
    let id = extract::instance_id(scene.character);
    let packet = scene.frame_packet(alpha, Some((1920, 1080)));
    let posed = packet.skinned.iter().find(|skinned| skinned.id == id);
    let palette = posed.map(|skinned| {
        packet.palette(skinned).expect("in range").iter().map(Affine3A::to_cols_array).collect()
    });
    scene.end_frame();
    palette
}

/// A step and a frame.
fn tick(scene: &mut Scene, clip: &ArchivedClip) -> Option<Vec<[f32; 12]>> {
    scene.step();
    frame(scene, clip, 1.0)
}

/// Where the strand's end is in the character's space, in a palette of model transforms.
fn strand_end(palette: &[[f32; 12]]) -> Vec3 {
    let columns = palette[STRAND_END];
    Vec3::new(columns[9], columns[10], columns[11])
}

/// The character has clips but no springs on: nothing about a frame changes, and what would have
/// run costs nothing.
#[test]
fn with_springs_off_the_frame_is_what_it_was() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut off = stage(false);
    let palette = tick(&mut off, clip).expect("posed");
    let stats = off.anim_stats();
    assert_eq!((stats.springs, stats.spring_substeps), (0, 0));
    assert_eq!(stats.spring_time, Duration::ZERO);
    // The strand hangs straight below the head.
    let head = Vec3::new(palette[2][9], palette[2][10], palette[2][11]);
    let hanging = head + Affine3A::from_cols_array(&palette[2]).transform_vector3(Vec3::new(0.0, -0.4, -0.1));
    assert!(strand_end(&palette).abs_diff_eq(hanging, 1e-4));
}

/// A held clip's position repeats with its clock running, and hair goes on settling, until it has
/// stopped; only then does the character stop being sent.
#[test]
fn hair_goes_on_settling_after_the_clip_holds() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let bytes = held().to_cooked().expect("cooked");
    let held = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    for _ in 0..45 {
        assert!(tick(&mut scene, clip).is_some());
        let stats = scene.anim_stats();
        assert_eq!((stats.evaluated, stats.springs), (1, 1));
        assert!(stats.spring_time > Duration::ZERO);
    }
    // The position is new once, then repeats.
    assert!(tick(&mut scene, held).is_some());
    assert_eq!(scene.anim_stats().evaluated, 1);
    let mut settling = 0;
    let mut stopped = None;
    for at in 0..600 {
        let palette = tick(&mut scene, held);
        let stats = scene.anim_stats();
        assert_eq!(stats.evaluated, 0, "the clip has nothing new");
        if palette.is_some() {
            assert_eq!(stats.springs, 1);
            settling += 1;
        } else {
            assert_eq!((stats.springs, stats.spring_substeps), (0, 0));
            stopped = Some(at);
            break;
        }
    }
    assert!(settling > 30, "it went on for {settling} frames");
    assert!(stopped.is_some(), "and then stopped");
    // And it stays stopped.
    for _ in 0..30 {
        assert_eq!(tick(&mut scene, held), None);
    }
}

/// Hitstop and a stopped world hold hair where they hold the pose: no sub-steps, no writes, so no
/// skinning. Then it carries on from there, with no pop.
#[test]
fn a_frozen_clock_holds_the_hair_and_it_carries_on() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    let mut last = Vec3::ZERO;
    for _ in 0..30 {
        last = strand_end(&tick(&mut scene, clip).expect("moving"));
    }
    scene.apply(SimActions { hitstop: true, ..SimActions::default() });
    for _ in 0..HITSTOP_TICKS {
        scene.step();
        for alpha in [0.25, 1.0] {
            assert_eq!(frame(&mut scene, clip, alpha), None, "held");
            assert_eq!(scene.anim_stats().spring_substeps, 0);
            assert_eq!(scene.anim_stats().springs, 0);
        }
    }
    let next = strand_end(&tick(&mut scene, clip).expect("moving again"));
    let more = strand_end(&tick(&mut scene, clip).expect("moving again"));
    let gap = next.distance(last);
    assert!(gap < 2.0 * more.distance(next) + 1e-3, "no pop: {gap} m, then {} m", more.distance(next));

    scene.world_clock_mut().0.push_scale(ScaleSource::Debug, 0).expect("room");
    scene.step();
    frame(&mut scene, clip, 1.0);
    scene.step();
    assert_eq!(frame(&mut scene, clip, 0.5), None);
    assert_eq!(scene.anim_stats().spring_substeps, 0);
}

/// Springs run up to the presented time, so hair never leads the body: half way between two steps, the
/// sub-step at the middle has run and the one at the step's end hasn't.
#[test]
fn springs_run_up_to_presented_time() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    tick(&mut scene, clip);
    tick(&mut scene, clip);
    scene.step();
    frame(&mut scene, clip, 0.5);
    assert_eq!(scene.anim_stats().spring_substeps, 1, "two a tick, half a tick on");
    frame(&mut scene, clip, 1.0);
    assert_eq!(scene.anim_stats().spring_substeps, 1, "and the other");
    frame(&mut scene, clip, 1.0);
    assert_eq!(scene.anim_stats().spring_substeps, 0, "and none for the same time");
}

/// World slow motion slows hair: it is on the character's clock, which the world's scale slows, so
/// sub-steps come further apart.
#[test]
fn world_slow_motion_slows_the_hair() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    tick(&mut scene, clip);
    scene.world_clock_mut().0.push_scale(ScaleSource::Debug, 200).expect("room");
    let mut substeps = 0;
    // The first step may be one more or less, as a sub-step lands.
    for _ in 0..50 {
        tick(&mut scene, clip);
        substeps += scene.anim_stats().spring_substeps;
    }
    assert!((19..=21).contains(&substeps), "two a tick at a fifth of the speed: {substeps} in 50 ticks");
}

/// A tier's setting reaches the springs, and phones run fewer.
#[test]
fn the_tier_sets_how_many_substeps_run() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    tick(&mut scene, clip);
    tick(&mut scene, clip);
    assert_eq!(scene.anim_stats().spring_substeps, 2);
    scene.set_spring_quality(SpringQuality::MOBILE);
    tick(&mut scene, clip);
    let character = scene.character;
    let player = scene.ecs.world().get::<ClipPlayer>(character).expect("playing");
    assert_eq!(player.spring_substeps(), Some(Substeps::One));
    tick(&mut scene, clip);
    assert_eq!(scene.anim_stats().spring_substeps, 1);
    // Each tier's setting.
    use sapphire_render::quality::TierId;
    for (tier, substeps) in
        [(TierId::DesktopHigh, 2), (TierId::DesktopLow, 2), (TierId::MobileHigh, 1), (TierId::MobileLow, 1)]
    {
        assert_eq!(crate::animate::spring_quality(tier).substeps.count(), substeps, "{tier:?}");
    }
}

/// Off screen they sleep, the pose running as it does: the same bytes as springs off, at no cost.
/// Coming back goes on from the pose with no motion stored: no whip.
#[test]
fn off_screen_they_sleep_and_coming_back_starts_from_the_pose() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    let mut twin = stage(false);
    let on_screen = OrbitCamera::shot(Vec3::new(0.0, 1.0, 0.0), PI, 0.1, 4.0, 0.8);
    *scene.camera_mut() = OrbitCamera::shot(Vec3::new(0.0, 1.0, 0.0), PI, 0.1, 4.0, 0.8);
    for _ in 0..20 {
        tick(&mut scene, clip);
        tick(&mut twin, clip);
        assert_eq!(scene.anim_stats().springs, 1, "on screen");
    }
    // The camera beside the character, looking away from it.
    *scene.camera_mut() = OrbitCamera::shot(Vec3::new(0.0, 1.0, -8.0), 0.0, 0.0, 4.0, 0.8);
    for _ in 0..10 {
        let palette = tick(&mut scene, clip).expect("the clip still plays");
        let posed = tick(&mut twin, clip).expect("the clip still plays");
        let stats = scene.anim_stats();
        assert_eq!((stats.evaluated, stats.springs, stats.spring_substeps), (1, 0, 0));
        assert_eq!(palette, posed, "the pose as it would be with none");
    }
    // Back: the first frame is the pose too, and from there it moves about as far as the pose does.
    *scene.camera_mut() = on_screen;
    let palette = tick(&mut scene, clip).expect("moving");
    let posed = tick(&mut twin, clip).expect("moving");
    let stats = scene.anim_stats();
    assert_eq!((stats.springs, stats.spring_substeps), (1, 0), "it settles first");
    assert_eq!(palette, posed);
    let mut last = strand_end(&palette);
    for _ in 0..10 {
        let palette = tick(&mut scene, clip).expect("moving");
        assert!(scene.anim_stats().springs == 1);
        let end = strand_end(&palette);
        assert!(end.distance(last) < 0.06, "no whip: {} m in a tick", end.distance(last));
        last = end;
    }
}

/// Past the tier's distance they sleep too, nearer on a phone than a desktop, and wake a little nearer
/// than they slept.
#[test]
fn past_the_tiers_distance_they_sleep() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    let at = |distance: f32| OrbitCamera::shot(Vec3::new(0.0, 1.0, 0.0), PI, 0.0, distance, 0.3);
    let sees = |scene: &mut Scene, distance: f32| {
        *scene.camera_mut() = at(distance);
        scene.camera_mut().set_zoom(1.0);
        tick(scene, clip);
        scene.anim_stats().springs == 1
    };
    assert!(sees(&mut scene, 10.0));
    assert!(sees(&mut scene, 30.0), "within a desktop's 40 m");
    assert!(!sees(&mut scene, 50.0), "beyond it");
    assert!(!sees(&mut scene, 38.0), "and it wakes nearer than it slept");
    assert!(sees(&mut scene, 30.0));
    scene.set_spring_quality(SpringQuality::MOBILE);
    assert!(!sees(&mut scene, 30.0), "beyond a phone's 25 m");
    assert!(sees(&mut scene, 20.0));
}

/// A loop's first frame teleports the character back along its root motion. That resets the springs:
/// they settle on the pose, so nothing whips back across the world.
#[test]
fn the_loops_teleport_resets_them() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    for _ in 0..59 {
        tick(&mut scene, clip);
        assert_eq!(scene.anim_stats().springs, 1);
    }
    let palette = tick(&mut scene, clip).expect("the loop's first frame");
    let stats = scene.anim_stats();
    assert_eq!((stats.springs, stats.spring_substeps), (1, 0), "it settles rather than swings");
    let mut fresh = stage(false);
    for _ in 0..59 {
        tick(&mut fresh, clip);
    }
    assert_eq!(Some(palette), tick(&mut fresh, clip), "the pose at the clip's first frame");
    // In place, there is no teleport: the springs swing through the loop.
    let mut in_place = stage_playing(player(true).with_root_motion(false));
    for tick_at in 0..=60 {
        tick(&mut in_place, clip);
        if tick_at > 0 {
            assert_eq!(in_place.anim_stats().spring_substeps, 2, "at {tick_at}");
        }
    }
}

/// A clip that runs in place has no teleport at its loop, root motion on or not: the hair swings through.
#[test]
fn a_loop_in_place_keeps_its_swing() {
    let bytes = clip_travelling(0.001).to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    for tick_at in 0..=60 {
        tick(&mut scene, clip);
        if tick_at > 0 {
            assert_eq!(scene.anim_stats().spring_substeps, 2, "at {tick_at}");
        }
    }
}

/// A new version of the character under a clip that keeps playing keeps its hair's motion, and
/// `--springs off` stays off.
#[test]
fn a_new_version_of_the_character_keeps_the_motion() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage(true);
    for _ in 0..30 {
        tick(&mut scene, clip);
    }
    let character = scene.character;
    let tails = |scene: &Scene| -> Vec<Vec3> {
        let player = scene.ecs.world().get::<ClipPlayer>(character).expect("playing");
        player.springs().expect("springs").tails().map(|tail| tail.position).collect()
    };
    let before = tails(&scene);
    let mut new = player(true);
    new.continue_from(scene.ecs.world().get::<ClipPlayer>(character).expect("playing"));
    *scene.ecs.world_mut().get_mut::<ClipPlayer>(character).expect("playing") = new;
    assert_eq!(tails(&scene), before);
    tick(&mut scene, clip);
    assert_eq!(scene.anim_stats().spring_substeps, 2, "on it goes, without settling");

    let mut new = player(true);
    new.continue_from(&player(false));
    assert!(new.springs().is_none(), "off stays off");

    // Its motion carries to a thread, as each job's does.
    fn sends<T: Send>(_: &T) {}
    sends(&new);
}
