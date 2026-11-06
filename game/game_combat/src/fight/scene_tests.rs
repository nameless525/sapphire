//! A fight on a scene, with no GPU: what stands where, what fires when, and what it allocates.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use sapphire_anim::{
    ArchivedClip, CharacterPose, Clip, Joint, JointTransform, PackedRotation, RotationTrack, Skeleton,
    VectorRange, VectorTrack,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{DEFAULT_RATE, RootMotion, WeightRange, WeightTrack};
use sapphire_assets::mesh::SkinBind;
use sapphire_gameplay::Simulation;
use sapphire_math::{Affine3A, Mat4, Quat, Vec2, Vec3};
use sapphire_render::{CharacterBody, FaceExpression, FramePacket, InstanceId, MeshHandle};
use sapphire_time::TICK_HZ;
use sapphire_vfx::VfxBudget;

use super::*;
use crate::arena::{ArenaGround, ArenaShot};
use sapphire_stage::lookdev::{self, FigureJoints, Shot, TownMeshes};
use sapphire_stage::scene::{Figure, LookDev};
use sapphire_stage::turntable::Turntable;

const HIPS: [f32; 3] = [0.0, 0.8, 0.0];

const BOUNDS: [[f32; 3]; 2] = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];

/// A scene and a fight's show on it, which reads as the scene it's on.
pub(super) struct Staged {
    pub(super) scene: Scene,
    pub(super) show: FightShow,
}

impl Deref for Staged {
    type Target = Scene;

    fn deref(&self) -> &Scene {
        &self.scene
    }
}

impl DerefMut for Staged {
    fn deref_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
}

impl Staged {
    pub(super) fn of(scene: Scene) -> Self {
        Self { scene, show: FightShow::default() }
    }

    pub(super) fn frame_packet(&mut self, alpha: f32, size: Option<(u32, u32)>) -> &FramePacket {
        self.scene.frame_packet(alpha, size, &mut self.show)
    }

    pub(super) fn follow_simulation(&mut self, sim: &Simulation, cast: &[sapphire_gameplay::ActorId]) {
        self.show.follow_simulation(&mut self.scene, sim, cast);
    }

    pub(super) fn show_cues(&mut self, sim: &Simulation) {
        self.show.show_cues(&mut self.scene, sim);
    }

    pub(super) fn end_cue_frame(&mut self, dt: f32, view_dt: f32, alpha: f32, rumble: &mut dyn RumbleSink) {
        self.show.end_cue_frame(&mut self.scene, dt, view_dt, alpha, rumble);
    }

    pub(super) fn play_cues(&mut self, budget: VfxBudget) -> Result<(), String> {
        self.show.play_cues(&mut self.scene, budget)
    }

    pub(super) fn drive_arena_camera(&mut self, dt: f32, look: Vec2, zoom: f32) -> bool {
        self.show.drive_camera(dt, look, zoom)
    }

    pub(super) fn camera_heading(&self) -> f32 {
        self.show.camera_heading(&self.scene)
    }

    pub(super) fn spawn_enemy(&mut self, meshes: EnemyMeshes, held: enemy::Held) {
        self.show.spawn_enemy(&mut self.scene, meshes, held);
    }

    pub(super) fn set_enemy_meshes(&mut self, meshes: EnemyMeshes) {
        self.show.set_enemy_meshes(&mut self.scene, meshes);
    }

    pub(super) fn spawn_blade(&mut self, mesh: MeshHandle, held_hand: Mat4) {
        self.show.spawn_blade(&mut self.scene, mesh, held_hand);
    }

    pub(super) fn set_blade_mesh(&mut self, mesh: MeshHandle) {
        self.show.set_blade_mesh(&mut self.scene, mesh);
    }

    pub(super) fn spawn_arena(&mut self, arena: Arena) {
        self.show.spawn_arena(&mut self.scene, arena);
    }

    pub(super) fn play_fighters(&mut self, player: GraphDriver, enemy: GraphDriver) {
        self.show.play_fighters(&mut self.scene, player, enemy);
    }

    pub(super) fn animate_fighters(&mut self, player: &ClipTable<'_>, enemy: &ClipTable<'_>, alpha: f32) {
        self.show.animate_fighters(&mut self.scene, player, enemy, alpha);
    }

    pub(super) fn cue_show(&self) -> Option<&CueShow> {
        self.show.cue_show()
    }

    pub(super) fn cue_sounds(&self) -> CueSounds<'_> {
        self.show.cue_sounds()
    }
}

fn instance_id(entity: Entity) -> InstanceId {
    InstanceId(entity.to_bits())
}

fn scene() -> Staged {
    Staged::of(Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false))
}

fn skeleton() -> Skeleton {
    let joint = |name: &str, parent: Option<u16>, translation: [f32; 3]| Joint {
        name: name.to_owned(),
        parent,
        rest: JointTransform { translation, ..JointTransform::IDENTITY },
    };
    Skeleton::new(&[
        joint("hips", None, HIPS),
        joint("spine", Some(0), [0.0, 0.4, 0.0]),
        joint("head", Some(1), [0.0, 0.3, 0.0]),
        joint("left foot", Some(0), [0.1, -0.7, 0.0]),
        joint("right foot", Some(0), [-0.1, -0.7, 0.0]),
    ])
}

/// Over 60 ticks: the head turning a quarter about Y, the hips dropping 0.2 m and
/// leaning, the second morph target from 0 to 1, and 3 m of root motion.
pub(super) fn clip() -> Clip {
    let half = core::f32::consts::FRAC_1_SQRT_2;
    let drop = VectorRange::enclosing([HIPS, [0.0, 0.6, 0.0]]);
    let root = VectorRange::enclosing([[0.0; 3], [0.0, 0.0, 3.0]]);
    let weight = WeightRange::enclosing([0.0, 1.0]);
    Clip {
        name: "turn".into(),
        skeleton: Some(AssetId::from_bytes([9; 16])),
        joint_count: 5,
        morph_target_count: 2,
        rate: DEFAULT_RATE,
        ticks: 60,
        rotations: vec![
            RotationTrack { joint: 1, count: 2, first: 0 },
            RotationTrack { joint: 2, count: 2, first: 2 },
        ],
        translations: vec![VectorTrack { joint: 0, count: 2, first: 0, range: drop }],
        weights: vec![WeightTrack { target: 1, count: 2, first: 0, range: weight }],
        root: Some(RootMotion {
            joint: 0,
            translation: VectorTrack { joint: 0, count: 2, first: 2, range: root },
            rotation: RotationTrack { joint: 0, count: 1, first: 4 },
        }),
        rotation_ticks: vec![0, 60, 0, 60, 0],
        rotation_keys: vec![
            PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
            PackedRotation::encode(Quat::from_rotation_x(0.4).to_array()),
            PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
            PackedRotation::encode([0.0, half, 0.0, half]),
            PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
        ],
        vector_ticks: vec![0, 60, 0, 60],
        vector_keys: vec![
            drop.encode(HIPS),
            drop.encode([0.0, 0.6, 0.0]),
            root.encode([0.0; 3]),
            root.encode([0.0, 0.0, 3.0]),
        ],
        weight_ticks: vec![0, 60],
        weight_keys: vec![weight.encode(0.0), weight.encode(1.0)],
        ..Clip::default()
    }
}

fn skeleton_pose_rest() -> CharacterPose {
    let bytes = skeleton().to_cooked().expect("cooked");
    let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
    CharacterPose::new(skeleton, &[], &[], Vec::new())
}

/// A graph with `clip()` for its only clip, playing it standing and for the show's moves.
fn graph() -> sapphire_anim::AnimGraph {
    let mut graph = sapphire_anim::AnimGraph::from_document(
        r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
            clips: [(name: "turn", source: "15af86bb-67b3-4208-bc54-1618aabf450b")],
            locomotion: (
                start: "ground",
                states: [(name: "ground", plays: Clip(clip: "turn", looping: true))],
            ),
            combat: (decay: 5, exit: 5, moves: [
                (move: "strike", clip: "turn"),
                (move: "dodge_back", clip: "turn"),
            ]),
        ))"#,
    )
    .expect("a good document");
    graph.resolve(|_| GRAPH_CLIP);
    graph
}

fn driver(table: &ClipTable<'_>) -> GraphDriver {
    let bytes = skeleton().to_cooked().expect("cooked");
    let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
    let binds: Vec<SkinBind> = (0..5)
        .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
        .collect();
    let binding = sapphire_anim::GraphBinding::new(&graph(), skeleton, None, 2, table).expect("it binds");
    let pose = CharacterPose::new(skeleton, &binds, &[], vec![0.25, 0.0]);
    let player = sapphire_anim::GraphPlayer::new(Arc::new(binding), pose, 1);
    GraphDriver::new(player, FigureJoints::of_joints(&skeleton_pose_rest(), 2, 0.0, [3, 4]))
}

pub(super) const GRAPH_CLIP: AssetId = AssetId::from_bytes([8; 16]);

/// What one frame's packet says of `id`.
#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    palette: Option<Vec<[f32; 12]>>,
    weights: Option<Vec<f32>>,
    moved_to: Option<Vec3>,
    head: Option<Vec3>,
    forward: Vec3,
    body: CharacterBody,
    removed: bool,
}

/// The look-dev stage holding the character still.
fn stage() -> Staged {
    let mut scene = Staged::of(Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false));
    let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 5], morph_weights: vec![0.25, 0.0] };
    let figure = Figure {
        head: Vec3::new(0.0, 1.6, 0.0),
        feet: Vec3::ZERO,
        body_radius: 0.16,
        face: Quat::IDENTITY,
        expression: FaceExpression::default(),
    };
    let town = TownMeshes {
        plants: [MeshHandle::from_raw(2); 8],
        mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
    };
    scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose, figure });
    scene
}

fn frame(scene: &mut Staged, clip: &ArchivedClip, alpha: f32, id: InstanceId) -> Drawn {
    scene.animate_clip(Some((clip, 0)), alpha);
    let drawn = drawn(scene, alpha, id);
    scene.end_frame();
    drawn
}

/// What this frame's packet says of `id`.
fn drawn(scene: &mut Staged, alpha: f32, id: InstanceId) -> Drawn {
    let packet = scene.frame_packet(alpha, Some((1920, 1080)));
    let posed = packet.skinned.iter().find(|skinned| skinned.id == id);
    let palette = posed.map(|skinned| {
        packet.palette(skinned).expect("in range").iter().map(Affine3A::to_cols_array).collect()
    });
    let weights = posed.map(|skinned| packet.morph_weights(skinned).expect("in range").to_vec());
    let moved_to = packet
        .instances
        .changed
        .iter()
        .find(|instance| instance.id == id)
        .map(|instance| instance.transform.transform_point3(Vec3::ZERO));
    let entry = packet.characters.iter().find(|character| character.id == id);
    Drawn {
        palette,
        weights,
        moved_to,
        head: entry.and_then(|entry| entry.lighting.gi.head),
        forward: entry.map_or(Vec3::ZERO, |entry| entry.face.head.forward),
        body: entry.map_or(CharacterBody::default(), |entry| entry.body),
        removed: packet.instances.removed.contains(&id),
    }
}

/// An arena whose fighters both play `driver`'s graph, standing at the start of a fight, and that
/// fight: the warrior's blade hangs from joint 2.
pub(super) fn fighting(table: &ClipTable<'_>) -> (Staged, Simulation, Vec<sapphire_gameplay::ActorId>) {
    use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

    let mut scene = Staged::of(Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false));
    let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 5], morph_weights: vec![0.25, 0.0] };
    let figure = Figure { head: Vec3::new(0.0, 1.6, 0.0), ..Figure::MANNEQUIN };
    let town = TownMeshes {
        plants: [MeshHandle::from_raw(2); lookdev::Plant::ALL.len()],
        mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
    };
    let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
    let set = ColliderSet::read(&cooked).expect("it reads");
    let held = enemy::Held { pose: pose.clone(), figure, blade: Mat4::IDENTITY, hand: 2 };
    scene.spawn_arena(Arena {
        enemy: EnemyMeshes { warrior: MeshHandle::from_raw(4), blade: MeshHandle::from_raw(5) },
        held,
        shot: ArenaShot::Follow,
        look: LookDev { town, fog_cards: None, pose, figure },
        ground: ArenaGround::new(set).expect("the stage stands on it"),
    });
    scene.play_fighters(driver(table).with_right_hand(1), driver(table));
    let scenario = find_scenario(SCENARIOS, crate::arena::FIGHT_SCENARIO).expect("listed");
    let (sim, cast) = scenario.start(scenario.config).expect("it starts");
    (scene, sim, cast)
}

/// The character's sword stays in the hand of the held pose until its graph has made a pose.
#[test]
fn a_graphs_hand_says_nothing_until_it_has_a_pose() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = Clip::read(&bytes).expect("a valid clip");
    let table = ClipTable::new(0, [(GRAPH_CLIP, Some(cooked))].into_iter());
    let (mut scene, mut sim, cast) = fighting(&table);
    let character = scene.character();
    let hand = |scene: &Staged| scene.world().get::<GraphDriver>(character).expect("on").right_hand();
    assert_eq!(hand(&scene), None, "no pose yet");
    sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
    scene.follow_simulation(&sim, &cast);
    scene.animate_fighters(&table, &table, 1.0);
    let held = scene.world().get::<GraphDriver>(character).expect("on").player.pose().model()[1];
    assert_eq!(hand(&scene), Some(Mat4::from(held)));
}

/// Both fighters play from the steps' views, the enemy's blade goes where its hand does, a frozen
/// clock asks nothing of them, and a frame allocates nothing.
#[test]
fn both_fighters_play_their_graphs_and_the_blade_stays_in_hand() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = Clip::read(&bytes).expect("a valid clip");
    let table = ClipTable::new(0, [(GRAPH_CLIP, Some(cooked))].into_iter());
    let (mut scene, mut sim, cast) = fighting(&table);
    let (warrior, blade) = scene.show.enemy.expect("it stands there");
    let palette =
        |scene: &Staged, entity| scene.world().get::<RenderPose>(entity).expect("posed").palette.clone();
    let held = palette(&scene, warrior);

    scene.animate_fighters(&table, &table, 1.0);
    assert_eq!(scene.anim_stats().characters, 0, "nothing before a fight");
    assert_eq!(palette(&scene, warrior), held, "it holds what it has");

    sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
    scene.follow_simulation(&sim, &cast);
    scene.animate_fighters(&table, &table, 1.0);
    assert_eq!(scene.anim_stats().characters, 2);
    assert_eq!(scene.anim_stats().evaluated, 2);
    assert_eq!(scene.anim_stats().joints, 10);
    assert_ne!(palette(&scene, warrior), held, "the graph has it");
    assert_ne!(palette(&scene, scene.character()), held);

    let _ = scene.frame_packet(1.0, Some((16, 9)));
    scene.end_frame();
    let world = scene.world();
    let drawn = |entity| world.get::<RenderTransform>(entity).expect("placed").0;
    let driver = world.get::<GraphDriver>(warrior).expect("playing");
    let hand = Mat4::from(driver.player.pose().model()[2]);
    let expected = drawn(warrior) * hand;
    assert!(drawn(blade).abs_diff_eq(expected, 1e-5), "{} against {expected}", drawn(blade));

    scene.animate_fighters(&table, &table, 1.0);
    let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
        for _ in 0..200 {
            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            scene.follow_simulation(&sim, &cast);
            scene.animate_fighters(&table, &table, 0.5);
        }
    });
    if sapphire_core::alloc::PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }

    let mut frozen = false;
    for _ in 0..240 {
        sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
        scene.follow_simulation(&sim, &cast);
        frozen = sim.actor(cast[0]).expect("alive").clock_delta == sapphire_time::SimDuration::ZERO;
        scene.animate_fighters(&table, &table, 1.0);
        if frozen {
            break;
        }
    }
    assert!(frozen, "a hit stops its target's clock within 4 s");
    for alpha in [0.25, 0.75] {
        scene.animate_fighters(&table, &table, alpha);
        assert!(scene.anim_stats().evaluated <= 1, "at most the one still moving");
    }
}

/// The blade is where the hand is, held or playing, and goes with the character when root
/// motion moves it. It draws as a character with no body, lit like the one holding it, and
/// keeps its place through a device loss's new mesh.
#[test]
fn the_blade_follows_the_hand_and_adds_no_body() {
    let bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&bytes).expect("a valid clip");
    let mut scene = stage();
    let held_hand = Mat4::from_translation(Vec3::new(-0.2, 0.9, 0.1));
    scene.spawn_blade(MeshHandle::from_raw(9), held_hand);
    let entity = scene.show.blade.expect("in the hand").entity;
    let id = instance_id(entity);
    let character = instance_id(scene.character());
    let transform =
        |scene: &Staged, entity: Entity| scene.world().get::<RenderTransform>(entity).expect("it draws").0;

    // Held: the held pose's hand.
    let packet = scene.frame_packet(1.0, Some((1920, 1080)));
    let drawn = packet.instances.changed.iter().find(|instance| instance.id == id).expect("it draws");
    assert!(drawn.transform.abs_diff_eq(held_hand * blade::socket(), 1e-5), "{:?}", drawn.transform);
    let listed = |id| packet.characters.iter().find(|entry| entry.id == id).expect("listed");
    assert!(!listed(id).body.casts(), "it adds no capsule");
    let (own, holder) = (listed(id).lighting.key, listed(character).lighting.key);
    assert!(own.direction.abs_diff_eq(holder.direction, 1e-4), "the two are lit alike");
    scene.end_frame();

    // Playing: the pose's hand, with the root's place.
    let rest = skeleton_pose_rest();
    let hand = 1;
    let joints = FigureJoints::of_joints(&rest, 2, 0.0, [3, 4]).with_right_hand(hand);
    let bytes = skeleton().to_cooked().expect("cooked");
    let binds: Vec<SkinBind> = (0..5)
        .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
        .collect();
    let pose = CharacterPose::new(Skeleton::read(&bytes).expect("valid"), &binds, &[], vec![0.25, 0.0]);
    scene.play_clip(ClipPlayer::new(pose, joints));
    let mut places = Vec::new();
    for _ in 0..30 {
        scene.step();
        frame(&mut scene, clip, 1.0, id);
        let holder = scene.world().get::<ClipPlayer>(scene.character()).expect("playing");
        let expected =
            transform(&scene, scene.character()) * holder.right_hand().expect("found") * blade::socket();
        let place = transform(&scene, entity);
        assert!(place.abs_diff_eq(expected, 1e-4), "{place:?} against {expected:?}");
        places.push(place.transform_point3(Vec3::ZERO));
    }
    assert!(places[0].distance(places[29]) > 0.5, "root motion carried it: {places:?}");

    // A device loss's new mesh keeps its place.
    let before = transform(&scene, entity);
    scene.set_blade_mesh(MeshHandle::from_raw(10));
    let packet = scene.frame_packet(1.0, Some((1920, 1080)));
    let drawn = packet.instances.changed.iter().find(|instance| instance.id == id).expect("re-meshed");
    assert_eq!(drawn.mesh, MeshHandle::from_raw(10));
    assert!(drawn.transform.abs_diff_eq(before, 1e-5));
}

/// The arena's stage with stand-in meshes, framed by `shot`, and the figure it holds the character by.
pub(super) fn arena_stage(shot: ArenaShot) -> (Staged, Figure) {
    let plain = MeshHandle::from_raw(1);
    let town = TownMeshes {
        plants: [plain; lookdev::Plant::ALL.len()],
        mountains: [plain; lookdev::MOUNTAINS.len()],
    };
    let figure = Figure { head: Vec3::new(0.0, 1.45, 0.0), ..Figure::MANNEQUIN };
    let pose = RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] };
    let look = LookDev { town, fog_cards: None, pose, figure };
    let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
    let set = ColliderSet::read(&cooked).expect("it reads");
    let ground = ArenaGround::new(set).expect("the stage stands on it");
    let mut scene = Staged::of(Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false));
    let enemy = EnemyMeshes { warrior: MeshHandle::from_raw(2), blade: MeshHandle::from_raw(3) };
    let blade_in_model = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
    let held = enemy::Held {
        pose: RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] },
        figure: Figure { head: Vec3::new(0.0, 2.2, 0.0), ..Figure::MANNEQUIN },
        blade: blade_in_model,
        hand: 0,
    };
    scene.spawn_arena(Arena { enemy, held, shot, look, ground });
    (scene, figure)
}

fn square_look() -> Arc<Timeline> {
    let text = include_str!("../../../../content/sequences/square_look.timeline.ron");
    let document = sapphire_timeline::TimelineDocument::from_ron(text).expect("reads");
    Arc::new(document.cook().expect("cooks"))
}

/// One frame `dt` after the last: where the view is, and what post it's drawn through.
fn timeline_frame(scene: &mut Staged, dt: f32) -> (Vec3, PostParams) {
    scene.drive_arena_camera(dt, Vec2::ZERO, 0.0);
    let post = scene.frame_packet(1.0, Some((640, 360))).post;
    scene.end_frame();
    (scene.camera().eye(), post)
}

/// What Q1's golden shows, all the way through: the fight's held, the view goes to the shot and the post
/// takes its offsets, and at the end everything's as it was, the post at once and the view by its blend.
#[test]
fn a_timeline_takes_the_view_and_the_post_and_gives_them_back() {
    const FRAME: f32 = 1.0 / 60.0;
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    let (eye_before, post_before) = timeline_frame(&mut scene, FRAME);
    scene.show.play_timeline(square_look(), Origin::default());
    assert!(scene.show.holds_simulation());
    assert_eq!(scene.show.timeline_player().map(TimelinePlayer::length), Some(4.0));

    for _ in 0..120 {
        timeline_frame(&mut scene, FRAME);
    }
    let (eye, post) = timeline_frame(&mut scene, FRAME);
    assert!(scene.show.holds_simulation());
    assert!(eye.distance(eye_before) > 3.0, "on the shot: {eye} from {eye_before}");
    assert!(eye.z < 3.0, "in front of the player, where no rig goes: {eye}");
    assert!((post.exposure - post_before.exposure + 0.35).abs() < 1e-3, "{}", post.exposure);
    assert!((post.vignette.intensity - post_before.vignette.intensity - 0.8).abs() < 1e-3);

    let frames_left = 2.1 / FRAME;
    for _ in 0..frames_left as u32 {
        timeline_frame(&mut scene, FRAME);
    }
    assert!(!scene.show.holds_simulation());
    assert!(scene.show.timeline_player().is_none(), "it's gone once it's over");
    let (eye, post) = timeline_frame(&mut scene, FRAME);
    assert_eq!(post, post_before, "handing back moves nothing");
    assert!(eye.distance(eye_before) > 0.01, "the view's still on its way back: {eye}");
    for _ in 0..180 {
        timeline_frame(&mut scene, FRAME);
    }
    let (eye, _) = timeline_frame(&mut scene, FRAME);
    assert!(eye.distance(eye_before) < 0.01, "and home: {eye} for {eye_before}");
}

#[test]
fn a_skip_lets_go_on_the_next_frame_and_blends_home() {
    const FRAME: f32 = 1.0 / 60.0;
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    let (eye_before, post_before) = timeline_frame(&mut scene, FRAME);
    assert_eq!(scene.show.skip_timeline(), None, "nothing to skip");
    scene.show.play_timeline(square_look(), Origin::default());
    for _ in 0..90 {
        timeline_frame(&mut scene, FRAME);
    }
    let at_ms = scene.show.skip_timeline().expect("playing");
    assert!((1450..=1550).contains(&at_ms), "{at_ms} ms in");
    assert!(!scene.show.holds_simulation(), "the fight goes on from here");
    let (eye, post) = timeline_frame(&mut scene, FRAME);
    assert_eq!(post, post_before);
    assert!(eye.distance(eye_before) > 1.0, "a blend, not a cut: {eye}");
    for _ in 0..180 {
        timeline_frame(&mut scene, FRAME);
    }
    let (eye, _) = timeline_frame(&mut scene, FRAME);
    assert!(eye.distance(eye_before) < 0.01, "and home: {eye} for {eye_before}");
}

/// A timeline's time scale is on the simulation's world, and what the scene's clock times has to stay
/// with it, as it does with a perfect dodge.
#[test]
fn the_scenes_clock_follows_a_timelines_time_scale() {
    let text = include_str!("../../../../crates/sapphire_timeline/test_content/cinematic.timeline.ron");
    let timeline = sapphire_timeline::TimelineDocument::from_ron(text).expect("reads").cook().expect("cooks");
    let mut sim = Simulation::new(sapphire_gameplay::SimConfig::default());
    let id = sim.add_timeline(timeline.simulation).expect("loads");
    sim.play_timeline(id).expect("plays");
    let mut scene = scene();
    let mut slowed = false;
    for _ in 0..timeline.presentation.length as u32 * TICK_HZ + 2 {
        sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
        scene.follow_simulation(&sim, &[]);
        let world = sim.world().resource::<WorldClock>().0.effective_scale();
        assert_eq!(scene.world_clock_mut().0.effective_scale(), world);
        slowed |= world < 1.0;
    }
    assert!(slowed, "the timeline's scale came and went");
    assert_eq!(scene.world_clock_mut().0.effective_scale(), 1.0);
}

/// A cutscene holds a fight that steps itself, and the fight takes its next step once it's let go, with
/// the fighters' clock never drawn behind what the hold drew.
#[test]
fn a_cutscene_holds_the_fight_and_a_skip_lets_it_go() {
    let option = CombatOption::Fight { script: Some("idle".to_owned()), party: false };
    let (mut staged, _) = arena_stage(ArenaShot::Director);
    let mut fight = Fight {
        combat: Some(Combat::new(&option).expect("sets up")),
        show: std::mem::take(&mut staged.show),
    };
    let tick = |fight: &Fight| fight.combat.as_ref().expect("on").simulation().tick();
    let mut laps = FrameLaps::default();
    fight.step_alone(&mut staged.scene, &mut laps);
    let started = tick(&fight);
    assert_eq!(fight.show.alpha_of_fighters(0.25), 0.25, "a fight going on draws between its steps");

    fight.show.play_timeline(square_look(), Origin::default());
    for _ in 0..30 {
        fight.step_alone(&mut staged.scene, &mut laps);
    }
    assert_eq!(tick(&fight), started, "held");
    assert_eq!(fight.show.alpha_of_fighters(0.25), 1.0, "on the latest step");

    fight.show.skip_timeline().expect("playing");
    assert_eq!(fight.show.alpha_of_fighters(0.25), 1.0, "and still, until there's another");
    fight.step_alone(&mut staged.scene, &mut laps);
    assert_eq!(tick(&fight), started + 1);
    assert_eq!(fight.show.alpha_of_fighters(0.25), 0.25);
}

/// A new fight stands everyone somewhere else, so the renderer hears of it once: temporal AA drops
/// what it drew of the last fight, and velocity counts the camera as unmoved. A swap, which stands
/// someone new in the same place with the camera going on, isn't a cut.
#[test]
fn a_new_fight_cuts_the_view_once_and_a_swap_doesnt() {
    use sapphire_gameplay::replay::{Recording, SCENARIOS, arena_walk, find_scenario, find_script};

    let scenario = find_scenario(SCENARIOS, crate::arena::SCENARIO).expect("listed");
    let cut_of = |scene: &mut Staged| {
        let cut = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").cut;
        scene.end_frame();
        cut
    };
    let (mut scene, _) = arena_stage(ArenaShot::Follow);
    assert!(!cut_of(&mut scene), "a first frame has nothing to drop");
    let mut first = Recording::new(scenario, scenario.config).expect("sets up");
    let cast = first.cast().to_vec();
    let mut source = arena_walk;
    for _ in 0..80 {
        first.step(&mut source).expect("short");
        scene.follow_simulation(first.simulation(), &cast);
    }
    assert!(!cut_of(&mut scene), "a fight going on");

    let mut next = Recording::new(scenario, scenario.config).expect("sets up");
    next.step(&mut source).expect("short");
    scene.follow_simulation(next.simulation(), &cast);
    // A frame with no target draws no view, and leaves the cut for the one that does.
    let _ = scene.frame_packet(1.0, None);
    scene.end_frame();
    assert!(cut_of(&mut scene), "the new fight's first view");
    assert!(!cut_of(&mut scene), "and only that");

    let party = find_scenario(SCENARIOS, crate::arena::PARTY_SCENARIO).expect("listed");
    let mut swapping = Recording::new(party, party.config).expect("sets up");
    let cast = swapping.cast().to_vec();
    let mut source = find_script("party-swap").expect("listed").actions;
    let (mut scene, _) = arena_stage(ArenaShot::Follow);
    for _ in 0..240 {
        swapping.step(&mut source).expect("short");
        scene.follow_simulation(swapping.simulation(), &cast);
        assert!(!cut_of(&mut scene), "tick {}", swapping.simulation().tick());
    }
    assert!(swapping.simulation().party().swaps() > 0, "it swapped");
}

/// A shake's roll, which `OrbitCamera::set_view` can't take, reaches the frame's view: everything
/// sums on the director's pose, so the roll is read back off it.
#[test]
fn a_directors_shake_rolls_the_view_the_frame_draws() {
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    // How far the view's right has left level, from a frame `dt` seconds on.
    let tilt = |scene: &mut Staged, dt: f32| {
        assert!(scene.drive_arena_camera(dt, sapphire_math::Vec2::ZERO, 0.0));
        let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera.view;
        scene.end_frame();
        view.inverse().x_axis.y.abs()
    };
    assert!(tilt(&mut scene, 1.0 / 60.0) < 1e-5, "level with no shake");
    let director = scene.show.arena.as_mut().expect("built").director().expect("this shot has one");
    let finish = director.tuning().shake_id("finish").expect("a shipped preset");
    director.fire_shake(finish, 1.0);
    let most = (0..30).map(|_| tilt(&mut scene, 1.0 / 60.0)).fold(0.0_f32, f32::max);
    assert!(most > 1e-3, "it rolls: {most}");
}

/// The arena stands its two actors where the simulation has them, and no turntable moves them:
/// at their spawn points before a replay moves them, then each step's position, and the camera
/// follows the player. Moved or not, its town is the look-dev stage's, on its day.
#[test]
fn the_arena_stands_its_actors_where_the_simulation_has_them() {
    use sapphire_gameplay::arena::spawns;
    use sapphire_gameplay::replay::{Recording, SCENARIOS, arena_walk, find_scenario};

    let (mut scene, figure) = arena_stage(ArenaShot::Follow);
    assert_eq!(scene.time_of_day().timeline(), &lookdev::day(), "the look-dev stage's day");
    assert!(scene.world().get::<Turntable>(scene.character()).is_none());

    let spawns = spawns();
    let (character, enemy) = (scene.character(), scene.show.arena.as_ref().expect("built").enemy_entity());
    let origin = |scene: &Staged, entity: Entity| {
        scene.world().get::<RenderTransform>(entity).expect("placed").0.transform_point3(Vec3::ZERO)
    };
    let _ = scene.frame_packet(1.0, Some((16, 9)));
    scene.end_frame();
    assert_eq!(origin(&scene, character), spawns.player.position());
    assert_eq!(origin(&scene, enemy), spawns.enemy.position());
    assert_eq!(scene.figure(), figure);
    // Drawn at its own size, with its blade where the model holds it, facing where it faces.
    let (warrior, blade) = scene.show.enemy.expect("it stands there");
    let scale = enemy::SCALE;
    let transform = |scene: &Staged, entity| scene.world().get::<RenderTransform>(entity).expect("placed").0;
    let upright = transform(&scene, warrior).transform_vector3(Vec3::Y).length();
    assert!((upright - scale).abs() < 1e-5, "{upright}");
    let hand = transform(&scene, blade).transform_point3(Vec3::ZERO);
    let expected = spawns.enemy.position() + Vec3::new(0.0, scale, 0.0);
    assert!(hand.distance(expected) < 1e-5, "{hand}");
    let packet = scene.packet();
    assert_eq!(packet.characters.len(), 3, "all three are lit as characters");

    let scenario = find_scenario(SCENARIOS, crate::arena::SCENARIO).expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut source = arena_walk;
    let cast = recording.cast().to_vec();
    for _ in 0..80 {
        recording.step(&mut source).expect("short");
        scene.follow_simulation(recording.simulation(), &cast);
    }
    let walked = recording.simulation().actor(cast[0]).expect("alive").position;
    let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera;
    let eye = view.position;
    scene.end_frame();
    assert_eq!(origin(&scene, character), walked);
    assert_eq!(origin(&scene, enemy), spawns.enemy.position(), "the enemy stands");
    assert!(walked.distance(spawns.player.position()) > 1.5, "it walked: {walked}");
    let target = crate::arena::follow_target(walked);
    assert!((eye.distance(target) - ArenaShot::Follow.camera(walked).eye().distance(target)).abs() < 1e-4);
    assert!(eye.z > walked.z, "the camera came with it: {eye}");

    // The director's view is the frame's and the stick's, behind the player and framing both.
    let (mut scene, _) = arena_stage(ArenaShot::Director);
    let player = spawns.player.position();
    let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera;
    let eye = view.position;
    scene.end_frame();
    assert!(eye.z > player.z, "behind the player, who faces -Z: {eye}");
    assert!(eye.distance(player) > 2.0, "{eye}");
    assert!((scene.camera().eye() - eye).length() < 1e-3, "{eye}");
    let heading = scene.camera_heading();
    assert!(sapphire_math::det::cos(heading) < -0.8, "it looks along -Z: {heading}");
    assert!(
        (sapphire_math::det::sin(heading - scene.camera().heading())).abs() < 1e-3,
        "the camera's and the director's headings agree: {heading}, {}",
        scene.camera().heading()
    );
    // A look turns it, and nothing moves it without time.
    assert!(scene.drive_arena_camera(1.0 / 60.0, sapphire_math::Vec2::new(0.2, 0.0), 0.0));
    let _ = scene.frame_packet(1.0, Some((16, 9)));
    scene.end_frame();
    assert_ne!(scene.camera_heading(), heading);
    // With another shot it takes nothing, and the plain camera's heading is the one.
    let (mut scene, _) = arena_stage(ArenaShot::Follow);
    assert!(!scene.drive_arena_camera(1.0, sapphire_math::Vec2::ZERO, 0.0));
    assert_eq!(scene.camera_heading(), scene.camera().heading());
}

/// The enemy stands beside the character in a shape of its own, and its blade lights as it does
/// but casts no capsule, so it adds nothing to shadows or occlusion. Each keeps its pose and
/// transform through a device loss's new meshes.
#[test]
fn the_enemy_stands_beside_the_character_in_a_shape_of_its_own() {
    let mut scene = Staged::of(Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false));
    let figure = Figure {
        head: Vec3::new(0.0, 1.45, 0.0),
        feet: Vec3::ZERO,
        body_radius: 0.16,
        face: Quat::IDENTITY,
        expression: FaceExpression::default(),
    };
    let town = TownMeshes {
        plants: [MeshHandle::from_raw(2); 8],
        mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
    };
    let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 2], morph_weights: Vec::new() };
    scene.spawn_look_dev(Shot::Enemy, LookDev { town, fog_cards: None, pose: pose.clone(), figure });
    let warrior_figure = Figure {
        head: Vec3::new(0.0, 2.0, 0.0),
        feet: Vec3::ZERO,
        body_radius: 0.3,
        face: Quat::IDENTITY,
        expression: FaceExpression::default(),
    };
    let held = enemy::Held {
        pose,
        figure: warrior_figure,
        blade: Mat4::from_translation(Vec3::new(-0.8, 1.2, 0.0)),
        hand: 0,
    };
    let meshes = EnemyMeshes { warrior: MeshHandle::from_raw(4), blade: MeshHandle::from_raw(5) };
    scene.spawn_enemy(meshes, held);
    scene.step();
    let (warrior, blade) = scene.show.enemy.expect("it stands there");
    let (warrior, blade) = (InstanceId(warrior.to_bits()), InstanceId(blade.to_bits()));
    let character = InstanceId(scene.character().to_bits());
    let packet = scene.frame_packet(1.0, Some((1920, 1080)));
    assert_eq!(packet.characters.len(), 3, "the character, the warrior and its blade");
    assert_eq!(packet.skinned.iter().filter(|skinned| skinned.id == warrior).count(), 1, "the pose is sent");
    let listed = |id| packet.characters.iter().find(|character| character.id == id).expect("listed");
    let scale = enemy::SCALE;
    assert!(
        (listed(warrior).body.radius - warrior_figure.body_radius * scale).abs() < 1e-4,
        "its own body, placed: {:?}",
        listed(warrior).body
    );
    assert!((listed(character).body.radius - figure.body_radius).abs() < 1e-5, "the character's own");
    assert!(!listed(blade).body.casts(), "the blade adds no capsule");
    let (warrior_key, blade_key) = (listed(warrior).lighting.key, listed(blade).lighting.key);
    assert!(warrior_key.direction.abs_diff_eq(blade_key.direction, 1e-4), "the two are lit alike");
    scene.end_frame();
    scene.set_enemy_meshes(EnemyMeshes { warrior: MeshHandle::from_raw(6), blade: MeshHandle::from_raw(7) });
    let packet = scene.frame_packet(1.0, Some((1920, 1080)));
    let drawn: Vec<MeshHandle> = packet.instances.changed.iter().map(|instance| instance.mesh).collect();
    assert!(
        drawn.contains(&MeshHandle::from_raw(6)) && drawn.contains(&MeshHandle::from_raw(7)),
        "{drawn:?}"
    );
}

/// G3's recording, from its start.
fn recorded_fight() -> Combat {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/fight.sreplay");
    let option = CombatOption::Replay { path, to_end: true };
    Combat::new(&option).expect("a committed recording")
}

#[test]
fn attaching_combat_prepares_cues_without_following_a_tick() {
    assert!(sapphire_core::alloc::is_installed());
    let mut staged = scene();
    staged.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
    let mut fight = Fight { combat: None, show: std::mem::take(&mut staged.show) };
    let combat = recorded_fight();
    let checksum = combat.simulation().checksum();

    fight.attach_combat(Some(combat));
    let combat = fight.combat.as_ref().expect("attached");
    assert_eq!(combat.simulation().tick(), 0);
    assert_eq!(combat.simulation().checksum(), checksum);
    assert!(fight.show.followed_run.is_none());
    let (_, first) = sapphire_core::alloc::count_in_scope(|| {
        fight.show.follow_simulation(&mut staged.scene, combat.simulation(), combat.cast())
    });
    assert_eq!(first.allocations, 0);
}

/// A recorded fight fired as `--cues` does: the frame the first hit lands in draws its spark and its
/// screen effect and fires its shake, which moves the view from the next, the frames before it draw
/// none of them, and the simulation still reaches its recorded checksums.
#[test]
fn a_fights_first_hit_reaches_the_frame_it_lands_in() {
    let mut combat = recorded_fight();
    let mut scene = scene();
    scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
    let view_at = |scene: &Staged| scene.camera().camera_data(16.0 / 9.0).view;
    let mut landed = false;
    for _ in 0..200 {
        scene.step();
        let still = view_at(&scene);
        combat.step(&mut no_bindings);
        scene.show_cues(combat.simulation());
        let hit = combat.simulation().cues().any(|cue| cue.hit.is_some());
        scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
        let shaken = view_at(&scene) != still;
        let packet = scene.frame_packet(1.0, Some((640, 360)));
        let (spark, effects) = (!packet.vfx.is_empty(), packet.post.effects);
        scene.end_frame();
        if hit {
            assert!(spark, "the hit's spark");
            assert_ne!(effects, sapphire_render::PostEffects::NONE, "the hit's screen effect");
            assert!(!shaken, "its shake is fired, and reads from where it begins, which is nothing");
            scene.step();
            scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
            assert!(view_at(&scene) != still, "the hit's shake, a frame on");
            landed = true;
            break;
        }
        assert_eq!(effects, sapphire_render::PostEffects::NONE, "nothing's landed yet");
    }
    assert!(landed, "the fight's first hit comes within 200 ticks");
    combat.check().expect("a fight fired is the fight recorded");
}

/// A step's firing allocates nothing once a whole fight has warmed it (rule R6). The simulation's
/// own steps are left out: a replay's trail of checksums grows.
#[test]
fn a_fights_cues_do_not_allocate_once_warmed() {
    let mut scene = scene();
    scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
    let mut fight = || {
        let mut combat = recorded_fight();
        let mut allocations = 0;
        for _ in 0..1500 {
            scene.step();
            combat.step(&mut no_bindings);
            let ((), fired) = sapphire_core::alloc::count_in_scope(|| {
                scene.show_cues(combat.simulation());
                scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
            });
            allocations += fired.allocations;
            let _ = scene.frame_packet(1.0, Some((640, 360)));
            scene.end_frame();
        }
        allocations
    };
    let _ = fight();
    assert_eq!(fight(), 0, "rule R6");
}

/// Without `--cues`, a fight stepping beside the scene fires nothing, whatever it publishes.
#[test]
fn a_fight_without_the_flag_fires_nothing() {
    let mut combat = recorded_fight();
    let mut scene = scene();
    for _ in 0..200 {
        scene.step();
        combat.step(&mut no_bindings);
        scene.show_cues(combat.simulation());
        scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
        let packet = scene.frame_packet(1.0, Some((640, 360)));
        assert!(packet.vfx.is_empty());
        assert_eq!(packet.post.effects, sapphire_render::PostEffects::NONE);
        scene.end_frame();
    }
    assert!(scene.cue_show().is_none());
    assert!(scene.cue_sounds().requests.is_empty());
}

/// Reused storage can restart at the same tick as the last presentation, including zero.
#[test]
fn a_same_tick_restart_cuts_history_and_fighter_poses_once() {
    for tick in [0, 2] {
        let mut combat = Combat::new(&CombatOption::Fight { script: None, party: false }).expect("sets up");
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        let mut walk = |_: u64, cast: &[sapphire_gameplay::ActorId]| {
            let mut actions = sapphire_gameplay::TickActions::NEUTRAL;
            actions
                .set(
                    cast[0],
                    sapphire_gameplay::ActorActions {
                        buttons: sapphire_gameplay::Buttons::NONE,
                        stick: sapphire_gameplay::Stick::new(127, 0),
                    },
                )
                .expect("room");
            actions
        };
        for _ in 0..tick {
            combat.step(&mut walk);
        }
        scene.follow_simulation(combat.simulation(), combat.cast());
        let _ = scene.frame_packet(1.0, Some((16, 9)));
        scene.end_frame();
        combat.restart();
        for _ in 0..tick {
            combat.step(&mut no_bindings);
        }
        let followed = scene.show.follow_simulation(&mut scene.scene, combat.simulation(), combat.cast());
        assert!(followed.started_over, "same tick {tick}, fresh run generation");
        let arena = scene.show.arena.as_ref().expect("an arena");
        assert_eq!(arena.drawn(0, 0.0), arena.drawn(0, 1.0), "no pose interpolation from the last fight");
        assert!(scene.frame_packet(0.0, Some((16, 9))).main_view().expect("a view").cut);
        scene.end_frame();
        let followed = scene.show.follow_simulation(&mut scene.scene, combat.simulation(), combat.cast());
        assert!(!followed.started_over, "the same boundary is followed only once");
        assert!(!scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").cut);
        scene.end_frame();
    }
}

#[test]
fn an_independent_simulation_at_the_same_tick_cuts_and_snaps_once() {
    use sapphire_gameplay::replay::{Recording, SCENARIOS, arena_walk, find_scenario};

    let scenario = find_scenario(SCENARIOS, crate::arena::SCENARIO).expect("listed");
    for tick in [0, 20] {
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        let mut previous = Recording::new(scenario, scenario.config).expect("sets up");
        for _ in 0..tick {
            previous.step(&mut arena_walk).expect("short");
        }
        let old_world = previous.simulation().world().id();
        let old_position = previous.simulation().actor(previous.cast()[0]).expect("player").position;
        scene.follow_simulation(previous.simulation(), previous.cast());
        assert!(!scene.frame_packet(1.0, Some((16, 9))).main_view().expect("view").cut);
        scene.end_frame();
        drop(previous);
        let mut next = Recording::new(scenario, scenario.config).expect("sets up");
        for _ in 0..tick {
            next.step(&mut no_bindings).expect("short");
        }
        if tick > 0 {
            assert_ne!(
                next.simulation().actor(next.cast()[0]).expect("player").position,
                old_position,
                "the first fight walked, the second stood still"
            );
        }
        assert_ne!(next.simulation().world().id(), old_world);
        assert_eq!(next.simulation().tick(), tick);
        let followed = scene.show.follow_simulation(&mut scene.scene, next.simulation(), next.cast());
        assert!(followed.started_over, "fresh world at the same public tick");
        let arena = scene.show.arena.as_ref().expect("arena");
        assert_eq!(arena.drawn(0, 0.0), arena.drawn(0, 1.0), "the new root is snapped");
        assert!(scene.frame_packet(0.0, Some((16, 9))).main_view().expect("view").cut);
        scene.end_frame();
        let followed = scene.show.follow_simulation(&mut scene.scene, next.simulation(), next.cast());
        assert!(!followed.started_over, "the same new boundary is followed once");
        assert!(!scene.frame_packet(1.0, Some((16, 9))).main_view().expect("view").cut);
        scene.end_frame();
    }
}
