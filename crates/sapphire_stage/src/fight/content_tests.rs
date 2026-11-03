//! A fight over a pack in memory, run frame by frame as the app runs it: what a new version of a fighter, its
//! graph or its clips does to it. These need a GPU.

use std::sync::Arc;

use sapphire_anim::{AnimGraph, Clip, ClipSource, Joint, JointTransform};
use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::{
    Influences, MaterialSlot, Mesh, PositionFormat, Skin, SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
    VertexFlags, VertexInfluence, quantize_weights,
};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::rig::{CharacterRig, HumanBone, HumanoidJoint};
use sapphire_assets::texture::Swizzle;
use sapphire_assets::{
    AssetBudgets, AssetChange, AssetKind, AssetServer, LoadError, LoadPriority, MemoryPackSource,
};
use sapphire_colliders::ColliderSet;
use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_livelink::LivePacks;
use sapphire_math::{Affine3A, Mat4, Vec3};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{RenderSettings, SkinningStats};
use sapphire_rhi::{GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::*;
use crate::anim_graph::{GraphScript, GraphShow};
use crate::arena;
use crate::content::SKY;
use crate::lookdev::{self, Plant, Shot, TownMeshes};
use crate::scene::{LookDev, Scene};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIDE: u32 = 32;
/// The test's pack.
const PACK: &str = "shared";

/// A fight over a pack in memory, run frame by frame as the app runs it.
struct Stage {
    // Drop order: what lives on the GPU before the device.
    scene: Scene,
    show: FightShow,
    content: Content,
    graph: GraphExecutor,
    renderer: Renderer,
    target: wgpu::Texture,
    gpu: GpuContext,
    source: Arc<MemoryPackSource>,
}

fn triangle(x: f32) -> Mesh {
    let vertex = |position: [f32; 3]| Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material =
        Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("a valid material");
    Mesh::from_vertices(
        &[vertex([x, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 2.0, 0.0])],
        vec![0, 1, 2],
        vec![Submesh { first_index: 0, index_count: 3, material: 0 }],
        vec![MaterialSlot::embedded("skin", material)],
        PositionFormat::Unorm16,
    )
}

/// `bytes` as a pack holds them.
fn packed(id: AssetId, kind: AssetKind, bytes: &[u8]) -> PackAsset {
    packed_with(id, kind, bytes, Vec::new())
}

/// `bytes` as a pack holds them, naming `dependencies`.
fn packed_with(id: AssetId, kind: AssetKind, bytes: &[u8], dependencies: Vec<AssetId>) -> PackAsset {
    PackAsset::encode(id, kind, Codec::Lz4, bytes, dependencies, 3).expect("encoded")
}

/// The test character's skeleton.
const SKELETON: AssetId = AssetId::from_bytes([0x5e; 16]);

/// A clip for the character: its root swings 1.5 m from one side to the other over [`SWING_TICKS`].
const SWING: AssetId = AssetId::from_bytes([0x5f; 16]);

const SWING_TICKS: u16 = 30;

/// A graph for the character that plays [`SWING`].
const GRAPH: AssetId = AssetId::from_bytes([0x60; 16]);

/// A cooked set of moves, as the graph show plays.
const MOVES: AssetId = AssetId::from_bytes([0x61; 16]);

/// A second clip, as [`SWING`], that a new version of the graph can play instead.
const OTHER_SWING: AssetId = AssetId::from_bytes([0x62; 16]);

/// The test enemy's skeleton, on the names its rig needs.
const ENEMY_SKELETON: AssetId = AssetId::from_bytes([0x63; 16]);

/// [`SWING`], for the enemy.
const ENEMY_SWING: AssetId = AssetId::from_bytes([0x64; 16]);

/// A pack with the look-dev character, its first vertex at `x`, its rig and its skeleton: every
/// required humanoid bone a joint of its own under the root, and `extra` joints more.
fn pack(x: f32, extra: usize) -> Vec<u8> {
    pack_with_decay(x, extra, 5)
}

fn pack_with_decay(x: f32, extra: usize, decay: u32) -> Vec<u8> {
    pack_of(x, extra, decay, 0)
}

/// The first pack, with `strike` so many ticks longer in its cooked moves.
#[cfg(feature = "dev")]
fn pack_with_moves(longer_by: u32) -> Vec<u8> {
    pack_of(0.0, 0, 5, longer_by)
}

fn pack_of(x: f32, extra: usize, decay: u32, recovery: u32) -> Vec<u8> {
    pack_of_graph(x, extra, swing_graph(decay), SWING, recovery)
}

/// The first pack, with `graph` naming `clip`.
fn pack_of_graph(x: f32, extra: usize, graph: AnimGraph, clip: AssetId, recovery: u32) -> Vec<u8> {
    write_pack(pack_assets(x, extra, graph, clip, recovery)).expect("a pack")
}

/// What [`pack_of_graph`] packs.
fn pack_assets(x: f32, extra: usize, graph: AnimGraph, clip: AssetId, recovery: u32) -> Vec<PackAsset> {
    let count = HumanBone::REQUIRED.len() + extra;
    let at = |joint: usize| [joint as f32 * 0.1, 1.0, 0.0];
    let joints: Vec<Joint> = (0..count)
        .map(|joint| Joint {
            name: format!("joint {joint}"),
            parent: (joint > 0).then_some(0),
            rest: JointTransform {
                translation: if joint > 0 { [joint as f32 * 0.1, 0.0, 0.0] } else { at(0) },
                ..JointTransform::IDENTITY
            },
        })
        .collect();
    let binds = (0..count)
        .map(|joint| SkinBind {
            joint: joint as u16,
            inverse_bind_matrix: Mat4::from_translation(-Vec3::from(at(joint))).to_cols_array_2d(),
        })
        .collect();
    let held = VertexInfluence { binds: [0; 4], weights: quantize_weights([1.0, 0.0, 0.0, 0.0]) };
    let mut mesh = triangle(x);
    mesh.skin = Some(Skin {
        skeleton: Some(SKELETON),
        joint_count: count as u32,
        binds,
        influences: Influences::pack(&[held; 3], count),
    });
    // The enemy's own: its rig's names in a chain, and `extra` joints more.
    let names = enemy::joint_names();
    let enemy_count = names.len() + extra;
    let enemy_joints: Vec<Joint> = (0..enemy_count)
        .map(|joint| Joint {
            name: names.get(joint).map_or_else(|| format!("joint {joint}"), |name| (*name).to_owned()),
            parent: joint.checked_sub(1).map(|parent| parent as u16),
            rest: JointTransform { translation: [0.0, 0.1, 0.0], ..JointTransform::IDENTITY },
        })
        .collect();
    let mut warrior = triangle(x);
    warrior.skin = Some(Skin {
        skeleton: Some(ENEMY_SKELETON),
        joint_count: enemy_count as u32,
        binds: (0..enemy_count)
            .map(|joint| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            })
            .collect(),
        influences: Influences::pack(&[held; 3], enemy_count),
    });
    let enemy_swing = Clip { skeleton: Some(ENEMY_SKELETON), ..swing(enemy_count) };
    let mut enemy_graph = graph.clone();
    enemy_graph.resolve(|_| ENEMY_SWING);
    let mut humanoid: Vec<_> = HumanBone::REQUIRED
        .iter()
        .enumerate()
        .map(|(joint, &bone)| HumanoidJoint { bone, joint: joint as u16 })
        .collect();
    humanoid.sort_by_key(|entry| entry.bone);
    let rig = CharacterRig {
        joint_count: count as u32,
        material_slot_count: 1,
        humanoid,
        ..CharacterRig::default()
    };
    vec![
        packed_with(lookdev::CHARACTER, AssetKind::Mesh, &mesh.to_cooked().expect("cooked"), vec![SKELETON]),
        packed(lookdev::RIG, AssetKind::Rig, &rig.to_cooked().expect("cooked")),
        packed(SKELETON, AssetKind::Skeleton, &Skeleton::new(&joints).to_cooked().expect("cooked")),
        packed_with(SWING, AssetKind::Clip, &swing(count).to_cooked().expect("cooked"), vec![SKELETON]),
        packed_with(OTHER_SWING, AssetKind::Clip, &swing(count).to_cooked().expect("cooked"), vec![SKELETON]),
        packed_with(GRAPH, AssetKind::AnimGraph, &graph.to_cooked().expect("cooked"), vec![clip]),
        packed(MOVES, AssetKind::MoveSet, &show_moves(recovery).to_cooked().expect("cooked")),
        // The enemy, as a fight loads it.
        packed_with(
            enemy::WARRIOR,
            AssetKind::Mesh,
            &warrior.to_cooked().expect("cooked"),
            vec![ENEMY_SKELETON],
        ),
        packed(enemy::BLADE, AssetKind::Mesh, &triangle(x).to_cooked().expect("cooked")),
        packed(
            ENEMY_SKELETON,
            AssetKind::Skeleton,
            &Skeleton::new(&enemy_joints).to_cooked().expect("cooked"),
        ),
        packed_with(
            ENEMY_SWING,
            AssetKind::Clip,
            &enemy_swing.to_cooked().expect("cooked"),
            vec![ENEMY_SKELETON],
        ),
        packed_with(
            enemy::GRAPH,
            AssetKind::AnimGraph,
            &enemy_graph.to_cooked().expect("cooked"),
            vec![ENEMY_SWING],
        ),
    ]
}

/// The graph show's moves as a cook makes them, with `strike` so many ticks longer.
fn show_moves(longer_by: u32) -> sapphire_moves::MoveSet {
    let text = include_str!("../../../../content/lookdev/character/show.moves.ron");
    let mut document = sapphire_moves::MoveSetDocument::parse(text).expect("parses");
    document.moves[0].recovery += longer_by;
    document.compile().expect("compiles")
}

/// The swing's document, looping. Its decay is all that tells one version from another. Its source is
/// the clip's own ID, so what a file of it resolves to is what a cook of it does.
fn swing_document(decay: u32) -> String {
    format!(
        r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
            clips: [(name: "swing", source: "{SWING}")],
            locomotion: (
                start: "ground",
                states: [(name: "ground", plays: Clip(clip: "swing", looping: true))],
            ),
            combat: (decay: {decay}, exit: {decay}, moves: []),
        ))"#
    )
}

/// [`swing_document`], as a cook makes it.
fn swing_graph(decay: u32) -> AnimGraph {
    let mut graph = AnimGraph::from_document(&swing_document(decay)).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    graph
}

/// The root from 0.75 m to the character's right to 0.75 m to its left, a clip for `joints` joints.
fn swing(joints: usize) -> Clip {
    let from = [-0.75, 1.0, 0.0];
    let to = [0.75, 1.0, 0.0];
    let range = sapphire_anim::VectorRange::enclosing([from, to]);
    Clip {
        name: "swing".into(),
        skeleton: Some(SKELETON),
        joint_count: joints as u32,
        rate: sapphire_assets::clip::DEFAULT_RATE,
        ticks: SWING_TICKS,
        translations: vec![sapphire_anim::VectorTrack { joint: 0, count: 2, first: 0, range }],
        vector_ticks: vec![0, SWING_TICKS],
        vector_keys: vec![range.encode(from), range.encode(to)],
        ..Clip::default()
    }
}

/// A graph naming `ids`, looping the first in its one state.
fn graph_naming(ids: &[AssetId]) -> AnimGraph {
    let clips: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| format!(r#"(name: "clip {index}", source: "{id}")"#))
        .collect();
    let text = format!(
        r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
            clips: [{clips}],
            locomotion: (
                start: "ground",
                states: [(name: "ground", plays: Clip(clip: "clip 0", looping: true))],
            ),
            combat: (decay: 5, exit: 5, moves: []),
        ))"#,
        clips = clips.join(", "),
    );
    let mut graph = AnimGraph::from_document(&text).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    graph
}

/// The IDs of a whole graph's worth of clips that aren't [`SWING`].
fn a_graphs_worth_of_other_clips() -> Vec<AssetId> {
    (0..sapphire_assets::graph::MAX_GRAPH_CLIPS)
        .map(|index| AssetId::from_bytes([0x80 + index as u8; 16]))
        .collect()
}

/// A frame of a character still swinging: how far its root stands from the rest pose's x 0, and that
/// no clip it wanted was missing.
fn swinging(stage: &mut Stage) -> f32 {
    stage.frame();
    let player = stage.scene.graph_player().expect("playing");
    assert_eq!(player.stats().missing_clips, 0, "it read every clip it wanted");
    player.pose().model()[0].translation[0].abs()
}

/// [`swing_document`] with a blink no character here can bind, though it reads and its clip is in the
/// pack: the test rig has no expressions, and the enemy binds with no rig at all.
#[cfg(feature = "dev")]
fn blinking_document(decay: u32) -> String {
    swing_document(decay).replace(
        "combat:",
        r#"blink: Some((expression: "wink", every: (150, 360), close: 3, hold: 1, open: 7)), combat:"#,
    )
}

/// [`blinking_document`], as a cook makes it: a cook reads the same file a save writes.
#[cfg(feature = "dev")]
fn blinking_graph(decay: u32) -> AnimGraph {
    let mut graph = AnimGraph::from_document(&blinking_document(decay)).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    graph
}

impl Stage {
    fn new() -> Self {
        Self::over_pack(pack(0.0, 0))
    }

    fn over_pack(pack: Vec<u8>) -> Self {
        Self::over_pack_with(pack, Self::in_town)
    }

    /// The mannequin's kind of scene: the character alone, with no rig, as `--stage` and `--rim-rig`
    /// draw it.
    fn mannequin_of(mesh: &Mesh) -> Self {
        let bytes = mesh.to_cooked().expect("cooked");
        let pack = write_pack(vec![packed(lookdev::CHARACTER, AssetKind::Mesh, &bytes)]).expect("a pack");
        Self::over_pack_with(pack, |_, _, _, mesh, bounds| Scene::with_character(mesh, bounds, false))
    }

    /// The look-dev stage around the character.
    fn in_town(
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        mesh: MeshHandle,
        bounds: [[f32; 3]; 2],
    ) -> Scene {
        let (pose, figure) = lookdev::hold(content, renderer, gpu).expect("the character's pose");
        // What the town draws doesn't matter here.
        let bytes = triangle(0.0).to_cooked().expect("cooked");
        let plain = Mesh::read(&bytes).expect("a valid mesh");
        let materials = renderer.upload_embedded_materials(plain).expect("materials");
        let plain = renderer.upload_mesh(plain, &materials).expect("a mesh");
        let town = TownMeshes {
            town: plain,
            plants: Plant::ALL.map(|_| plain),
            mountains: lookdev::MOUNTAINS.map(|_| plain),
        };
        let mut scene = Scene::with_character(mesh, bounds, false);
        scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose, figure });
        scene
    }

    fn over_pack_with(
        pack: Vec<u8>,
        scene: impl FnOnce(&mut Content, &mut Renderer, &GpuContext, MeshHandle, [[f32; 3]; 2]) -> Scene,
    ) -> Self {
        let source = Arc::new(MemoryPackSource::new());
        source.insert(PACK, pack);
        let live_packs = Arc::new(LivePacks::new(source.clone()));
        let budgets = AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] };
        let mut server = AssetServer::new(live_packs.clone(), budgets);
        server.mount(PACK).expect("the pack");
        let character = server.load_with(lookdev::CHARACTER, LoadPriority::Critical).expect("loading");
        let mut content = Content::over_server(server, lookdev::CHARACTER, character, SKY, live_packs);
        content.keep_character_mesh();
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let (mesh, bounds) = content.character(&mut renderer, &gpu).expect("the character");
        let scene = scene(&mut content, &mut renderer, &gpu, mesh, bounds);
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            scene,
            show: FightShow::default(),
            content,
            graph: GraphExecutor::new(),
            renderer,
            target,
            gpu,
            source,
        }
    }

    /// One frame, as the app runs it: what the frame's loading uploaded, then extraction, and
    /// what the frame skins.
    fn frame(&mut self) -> SkinningStats {
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.follow_content(&mut self.content, &mut self.show);
        self.scene.step();
        self.scene.animate(&self.content, 1.0, &mut self.show);
        let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut self.show);
        self.renderer.prepare(&self.gpu, packet);
        let stats = self.renderer.stats().skinning;
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        self.scene.end_frame();
        stats
    }

    /// Plays the swing on the character, framed so its triangle is in view whichever way it swings.
    fn play_swing(&mut self) {
        self.content.load_clip(SWING, &mut self.renderer, &self.gpu).expect("the clip");
        self.scene.play_clip(lookdev::player(&self.content).expect("a player"));
        self.frame_on_character();
    }

    /// Plays the graph on the character, from a script that stands still.
    fn play_graph(&mut self) {
        self.content.load_graph(GRAPH, &mut self.renderer, &self.gpu).expect("the graph");
        let driver = lookdev::graph_driver(&self.content).expect("a driver");
        self.scene.play_graph(driver, GraphShow::new(GraphScript::Stand));
        self.frame_on_character();
    }

    /// Stands a fight's two fighters in the arena, each playing its graph.
    fn play_fighters(&mut self) {
        let enemy =
            self.show.load_enemy(&mut self.content, &mut self.renderer, &self.gpu).expect("the enemy");
        self.content.load_graph(GRAPH, &mut self.renderer, &self.gpu).expect("the graph");
        self.show
            .load_enemy_graph(&mut self.content, &mut self.renderer, &self.gpu)
            .expect("the enemy's graph");
        let (mesh, bounds) = self.content.character(&mut self.renderer, &self.gpu).expect("the character");
        let (pose, figure) =
            lookdev::hold(&mut self.content, &mut self.renderer, &self.gpu).expect("the character's pose");
        let bytes = triangle(0.0).to_cooked().expect("cooked");
        let plain = Mesh::read(&bytes).expect("a valid mesh");
        let materials = self.renderer.upload_embedded_materials(plain).expect("materials");
        let plain = self.renderer.upload_mesh(plain, &materials).expect("a mesh");
        let town = TownMeshes {
            town: plain,
            plants: Plant::ALL.map(|_| plain),
            mountains: lookdev::MOUNTAINS.map(|_| plain),
        };
        let held = self.show.held_enemy(&self.content).expect("the enemy's pose");
        let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
        let set = ColliderSet::read(&cooked).expect("it reads");
        let ground = arena::ArenaGround::new(set).expect("the stage stands on it");
        let world = ground.share();
        let shot = arena::ArenaShot::Follow;
        let look = LookDev { town, fog_cards: None, pose, figure };
        let mut scene = Scene::with_character(mesh, bounds, false);
        self.show.spawn_arena(&mut scene, Arena { enemy, held, shot, look, ground });
        let player = lookdev::graph_driver(&self.content).expect("the player's driver");
        let enemy = self.show.enemy_driver(&self.content, world).expect("the enemy's driver");
        self.show.play_fighters(&mut scene, player, enemy);
        self.scene = scene;
    }

    /// A fight of the arena's scenario, its first step told to the scene.
    fn start_fight(&mut self) -> (Simulation, Vec<ActorId>) {
        let scenario = find_scenario(SCENARIOS, arena::FIGHT_SCENARIO).expect("listed");
        let (sim, cast) = scenario.start(scenario.config).expect("it starts");
        let mut fight = (sim, cast);
        self.step_fight(&mut fight);
        fight
    }

    /// A step of `fight`, told to the scene, and a frame.
    fn step_fight(&mut self, (sim, cast): &mut (Simulation, Vec<ActorId>)) -> SkinningStats {
        sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
        self.show.follow_simulation(&mut self.scene, sim, cast);
        self.frame()
    }

    /// Frames the character from the front, close enough for its triangle to fill much of the target.
    fn frame_on_character(&mut self) {
        *self.scene.camera_mut() = crate::orbit::OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
    }

    /// One frame as [`Self::frame`] draws it, without a fixed step, read back.
    fn pixels(&mut self) -> Vec<u8> {
        let target = self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = crate::readback::Readback::new(&self.gpu, SIDE, SIDE);
        self.content.update(&mut self.renderer, &self.gpu);
        self.scene.follow_content(&mut self.content, &mut self.show);
        self.scene.animate(&self.content, 1.0, &mut self.show);
        let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut self.show);
        self.renderer.prepare(&self.gpu, packet);
        let view = target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let imported = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, imported).expect("a view and a target");
        readback.record(&mut graph, imported);
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        self.scene.end_frame();
        readback.read(&self.gpu).expect("its pixels")
    }

    /// The pack re-cooked, as a watched file or a push hands it over.
    fn recook(&mut self, x: f32, extra: usize) {
        self.source.insert(PACK, pack(x, extra));
        self.content.remount(PACK).expect("the new pack");
    }

    /// The pack re-cooked with `extra` joints more and a graph of `decay`, with `late` still as the
    /// first pack has them, as a cook's pack lands in parts. Bytes that don't change make no new
    /// version, so what's late lands when a later pack brings it.
    fn recook_all_but(&mut self, late: &[AssetId], extra: usize, decay: u32) {
        let first = pack_assets(0.0, 0, swing_graph(5), SWING, 0);
        let mut assets = pack_assets(0.0, extra, swing_graph(decay), SWING, 0);
        assets.retain(|asset| !late.contains(&asset.id));
        assets.extend(first.into_iter().filter(|asset| late.contains(&asset.id)));
        self.source.insert(PACK, write_pack(assets).expect("a pack"));
        self.content.remount(PACK).expect("the new pack");
    }

    /// Frames until `landed` says so.
    fn frames_until(&mut self, landed: impl Fn(&Self) -> bool) {
        for _ in 0..10_000 {
            self.frame();
            if landed(self) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("it never landed");
    }

    /// What plays the graph on `entity`.
    fn player_of(&self, entity: Entity) -> &sapphire_anim::GraphPlayer {
        &self.scene.world().get::<GraphDriver>(entity).expect("playing").player
    }

    /// Stands the enemy in `pose`, whatever its graph last played.
    fn set_enemy_pose(&mut self, pose: crate::extract::RenderPose) {
        let enemy = self.show.enemy.expect("it stands there").0;
        self.scene.world_mut().entity_mut(enemy).insert(pose);
    }

    /// What plays the enemy's graph.
    fn enemy_player(&self) -> &sapphire_anim::GraphPlayer {
        self.player_of(self.show.enemy.expect("it stands there").0)
    }

    /// How many joints the pose the character's graph plays has.
    fn joints(&self) -> usize {
        self.player_of(self.scene.character()).pose().joint_count()
    }

    /// How many joints the pose the enemy's graph plays has.
    fn enemy_joints(&self) -> usize {
        self.enemy_player().pose().joint_count()
    }

    /// The versions the character's mesh, rig and skeleton are at.
    fn versions(&self) -> (u32, u32, u32) {
        self.content.current_character_versions().expect("a rig")
    }

    /// Frames until the character's mesh, rig and skeleton are at `versions`, and what the last
    /// skinned.
    fn until(&mut self, versions: (u32, u32, u32)) -> SkinningStats {
        for _ in 0..10_000 {
            let stats = self.frame();
            if self.versions() == versions {
                return stats;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("still at {:?}", self.versions());
    }
}

/// A pack with a new character and a new graph for the enemy, as a cook that touches both makes: the
/// player's driver starts over for its new joints, and the enemy's graph still binds.
#[test]
#[ignore = "needs a GPU"]
fn a_new_character_and_a_new_enemy_graph_in_one_pack_reach_the_enemy_too() {
    let mut stage = Stage::new();
    stage.play_fighters();
    for _ in 0..3 {
        stage.frame();
    }
    assert!(!stage.enemy_player().has_replacement());

    stage.source.insert(PACK, pack_with_decay(0.5, 0, 9));
    stage.content.remount(PACK).expect("the new pack");
    let mut waiting = false;
    for _ in 0..1000 {
        stage.frame();
        waiting = stage.enemy_player().has_replacement();
        if waiting {
            break;
        }
    }
    assert!(waiting, "the enemy's new graph reached it");
}

/// A new version of the enemy with a joint more, as a re-export of its model makes: it plays on, on the
/// new joints, and nothing draws it in its bind pose for want of a palette that fits.
#[test]
#[ignore = "needs a GPU"]
fn a_new_skeleton_under_the_enemy_plays_on() {
    let mut stage = Stage::new();
    stage.play_fighters();
    let mut fight = stage.start_fight();
    let joints = stage.enemy_joints();
    assert_eq!(joints, enemy::joint_names().len());

    // The character's in the pack too, and while part of it has landed its pose may not fit.
    stage.recook(0.0, 1);
    stage.frames_until(|stage| stage.enemy_joints() != joints && stage.versions() == (1, 1, 1));
    assert_eq!(stage.enemy_joints(), joints + 1, "the enemy's driver is on the new skeleton");
    for _ in 0..30 {
        let stats = stage.step_fight(&mut fight);
        assert_eq!(stats.ignored_poses, 0, "{stats:?}");
    }
    let hand = stage.show.arena.as_deref_mut().expect("an arena").blade_hand();
    assert_eq!(enemy::joint_names()[hand], enemy::HAND_SLOT);
}

/// A skeleton with a joint more, then a clip and a graph for it, with the mesh still to come, as a cook's
/// pack lands in parts: the enemy keeps what it has, and neither their landing nor an edit hands its old
/// pose a binding to the new joints. What names the skeleton lands after it here, since what lands
/// before it binds the old joints, as it may, which `has_replacement` can't tell apart.
#[test]
#[ignore = "needs a GPU"]
fn an_enemy_waits_for_the_rest_of_a_new_version_to_land() {
    let mut stage = Stage::new();
    stage.play_fighters();
    let _fight = stage.start_fight();
    let joints = stage.enemy_joints();

    stage.recook_all_but(&[enemy::WARRIOR, SWING, OTHER_SWING, GRAPH, ENEMY_SWING, enemy::GRAPH], 1, 7);
    stage.frames_until(|stage| stage.show.enemy_versions(&stage.content) == Some((0, 1)));
    let versions = stage.content.graph_versions();
    stage.recook_all_but(&[enemy::WARRIOR], 1, 7);
    // Each graph and the clip it names.
    stage.frames_until(|stage| stage.content.graph_versions() >= versions + 4);
    assert_eq!(stage.enemy_joints(), joints, "it keeps what it has");
    let waiting = stage.enemy_player();
    assert!(!waiting.has_replacement(), "a graph bound to other joints waits with it");

    let mut edited = swing_graph(4);
    edited.resolve(|_| ENEMY_SWING);
    let Stage { scene, show, content, renderer, gpu, .. } = &mut stage;
    let error = show
        .replace_graph_document(scene, ENEMY_GRAPH, edited, content, renderer, gpu)
        .expect_err("an edit waits with it");
    assert!(error.contains("still landing"), "{error}");

    stage.source.insert(PACK, pack_with_decay(0.0, 1, 7));
    stage.content.remount(PACK).expect("the rest of it");
    stage.frames_until(|stage| stage.enemy_joints() != joints);
    assert_eq!(stage.enemy_joints(), joints + 1, "the rest landed, and so did it");
}

/// A fighter whose pose doesn't fit its driver for a moment plays nothing, and what its graph had yet
/// to hear waits for the frame it plays.
#[test]
#[ignore = "needs a GPU"]
fn a_cut_waits_for_a_frame_the_fighter_plays() {
    let mut stage = Stage::new();
    stage.play_fighters();
    let _fight = stage.start_fight();
    let fits = stage.enemy_player().pose().palette_len();
    let pose = |joints: usize| crate::extract::RenderPose {
        palette: vec![Affine3A::IDENTITY; joints],
        morph_weights: Vec::new(),
    };

    stage.set_enemy_pose(pose(fits + 1));
    stage.show.arena.as_deref_mut().expect("an arena").cut_next(1);
    stage.frame();
    assert!(stage.show.arena.as_deref_mut().expect("an arena").has_cut_waiting(1), "it played nothing");

    stage.set_enemy_pose(pose(fits));
    stage.frame();
    assert!(!stage.show.arena.as_deref_mut().expect("an arena").has_cut_waiting(1), "it's heard");
}

/// An edited graph for the enemy, as the next test's for the character: it binds against the enemy's
/// own skeleton, leaves the character's alone, and stays what binds until a pack brings another.
#[test]
#[ignore = "needs a GPU"]
fn an_edited_enemy_graph_stays_until_a_pack_replaces_it() {
    let mut stage = Stage::new();
    stage.play_fighters();
    stage.frame();
    let Stage { scene, show, content, renderer, gpu, .. } = &mut stage;

    // The character's clip is for the character's joints.
    let error = show
        .replace_graph_document(scene, ENEMY_GRAPH, swing_graph(4), content, renderer, gpu)
        .expect_err("another skeleton's clip");
    assert!(error.contains("swing"), "{error}");
    let player = |scene: &Scene, show: &FightShow| {
        scene
            .world()
            .get::<GraphDriver>(show.enemy.expect("it stands there").0)
            .expect("playing")
            .player
            .has_replacement()
    };
    assert!(!player(scene, show));

    let mut edited = swing_graph(4);
    edited.resolve(|_| ENEMY_SWING);
    show.replace_graph_document(scene, ENEMY_GRAPH, edited, content, renderer, gpu).expect("it binds");
    assert!(player(scene, show));
    assert_eq!(content.graph_document_of(ENEMY_GRAPH).expect("a graph").combat.decay, 4);
    assert_eq!(content.graph_document().expect("a graph").combat.decay, 5, "the character's stays");
    assert!(!content.take_graph_changed(), "its own clips are nothing new");

    stage.source.insert(PACK, pack_with_decay(0.0, 0, 9));
    stage.content.remount(PACK).expect("the new pack");
    for _ in 0..1000 {
        stage.frame();
        if stage.content.graph_document_of(ENEMY_GRAPH).expect("a graph").combat.decay == 9 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the cook of the document should win over the edit it came after");
}

/// A cooked set's new version reaches the simulation that plays its document (M2 T2), which is how
/// moves that name clips get there, and bytes that didn't change make no new revision.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU"]
fn a_new_cooked_set_reaches_the_fight() {
    let mut stage = Stage::new();
    stage.play_graph();
    let handle = stage.content.hold_move_set(MOVES).expect("in the pack");
    let mut tuning = crate::tuning::LiveTuning::following("show.moves.ron", handle);
    let revision = |stage: &Stage| {
        let show = stage.scene.graph_show().expect("a show");
        let set = show.simulation().actor(show.cast()[0]).expect("alive").moves.set.expect("a set");
        show.simulation().move_library().current_revision(set)
    };
    let frames = |stage: &mut Stage, tuning: &mut crate::tuning::LiveTuning, want: u32| {
        for _ in 0..1000 {
            stage.frame();
            let changes = stage.content.take_changes();
            let fight = crate::tuning::LiveFight { combat: None, scene: &mut stage.scene };
            let errors = tuning.follow_packs(&changes, &stage.content, fight);
            assert_eq!(errors, Vec::<String>::new());
            if revision(stage) == Some(want) {
                return;
            }
        }
        panic!("still at {:?}", revision(stage));
    };
    for _ in 0..10 {
        stage.frame();
    }
    assert_eq!(revision(&stage), Some(0));

    stage.source.insert(PACK, pack_with_moves(4));
    stage.content.remount(PACK).expect("the new pack");
    frames(&mut stage, &mut tuning, 1);
    let show = stage.scene.graph_show().expect("a show");
    let set = show.simulation().actor(show.cast()[0]).expect("alive").moves.set.expect("a set");
    assert!(show.simulation().move_library().is_current(set, &show_moves(4)));
}

/// A cook reads a document when it starts, so one that started before the newest save brings what the
/// file said then, which mustn't undo the save with nothing to say so. A cook of what the file says is
/// no edit past its own.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU"]
fn a_cook_older_than_the_newest_save_leaves_the_save_playing() {
    let mut stage = Stage::new();
    stage.play_graph();
    let handle = stage.content.hold_move_set(MOVES).expect("in the pack");
    let file = std::env::temp_dir()
        .join(format!("sapphire_sandbox_older_cook_{}", std::process::id()))
        .join("show.moves.ron");
    std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
    let save = |longer_by: u32| {
        let text = include_str!("../../../../content/lookdev/character/show.moves.ron");
        let mut document = sapphire_moves::MoveSetDocument::parse(text).expect("parses");
        document.moves[0].recovery += longer_by;
        std::fs::write(&file, document.to_text().expect("writes")).expect("the save");
    };
    let mut tuning = crate::tuning::LiveTuning::following("show.moves.ron", handle).reading(file.clone());
    let revision = |stage: &Stage| {
        let show = stage.scene.graph_show().expect("a show");
        let set = show.simulation().actor(show.cast()[0]).expect("alive").moves.set.expect("a set");
        show.simulation().move_library().current_revision(set)
    };
    let plays = |stage: &Stage, longer_by: u32| {
        let show = stage.scene.graph_show().expect("a show");
        let set = show.simulation().actor(show.cast()[0]).expect("alive").moves.set.expect("a set");
        show.simulation().move_library().is_current(set, &show_moves(longer_by))
    };
    // Frames until the pack has landed and been followed.
    let land = |stage: &mut Stage, tuning: &mut crate::tuning::LiveTuning, longer_by: u32| {
        stage.source.insert(PACK, pack_with_moves(longer_by));
        stage.content.remount(PACK).expect("the new pack");
        let mut followed = false;
        for _ in 0..1000 {
            stage.frame();
            let changes = stage.content.take_changes();
            followed |= changes
                .iter()
                .any(|change| matches!(change, AssetChange::Reloaded { kind: AssetKind::MoveSet, .. }));
            let fight = crate::tuning::LiveFight { combat: None, scene: &mut stage.scene };
            assert_eq!(tuning.follow_packs(&changes, &stage.content, fight), Vec::<String>::new());
            if followed {
                return;
            }
        }
        panic!("the pack never landed");
    };
    for _ in 0..10 {
        stage.frame();
    }

    // Saved during the cook: seven ticks longer, where it read four.
    save(7);
    land(&mut stage, &mut tuning, 4);
    assert!(plays(&stage, 7), "the save stands");
    assert_eq!(revision(&stage), Some(1), "one edit, not the cook and then the save");

    // The next cook reads the save, and is no edit.
    land(&mut stage, &mut tuning, 7);
    assert!(plays(&stage, 7));
    assert_eq!(revision(&stage), Some(1));

    // A file that doesn't read says nothing against a cook.
    std::fs::write(&file, "half a docu").expect("half a save");
    land(&mut stage, &mut tuning, 9);
    assert!(plays(&stage, 9));
    assert_eq!(revision(&stage), Some(2));
    std::fs::remove_dir_all(file.parent().expect("a folder")).expect("cleaned up");
}

/// The enemy's graph, which a bad save and a cook meet the same way: the save gives way to a cook that
/// fits, and a cook that doesn't leaves it waiting for the correction, which reaches it.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU"]
fn an_enemys_graph_that_never_fits_keeps_nothing_waiting() {
    let mut stage = Stage::new();
    stage.play_fighters();
    let _fight = stage.start_fight();
    let joints = stage.enemy_joints();
    let folder = std::env::temp_dir().join(format!("sapphire_sandbox_unfit_enemy_{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a folder");
    let file = folder.join("warrior.animgraph.ron");
    stage.content.read_graphs_from(vec![(ENEMY_GRAPH, file.clone())]);
    let save = blinking_document(4).replace(&SWING.to_string(), &ENEMY_SWING.to_string());
    std::fs::write(&file, save).expect("the save");

    stage.source.insert(PACK, pack_with_decay(0.0, 1, 7));
    stage.content.remount(PACK).expect("the new pack");
    stage.frames_until(|stage| stage.enemy_joints() != joints);
    std::fs::remove_dir_all(&folder).expect("cleaned up");
    assert_eq!(stage.content.graph_document_of(ENEMY_GRAPH).expect("a graph").combat.decay, 7, "the cook's");

    stage.source.insert(PACK, pack_of_graph(0.0, 2, blinking_graph(9), SWING, 0));
    stage.content.remount(PACK).expect("the next pack");
    stage.frames_until(|stage| {
        stage.show.enemy_versions(&stage.content) == Some((2, 2))
            && stage.content.graph_document_of(ENEMY_GRAPH).is_ok_and(|g| g.blink.is_some())
    });
    for _ in 0..30 {
        stage.frame();
    }
    assert_eq!(stage.enemy_joints(), joints + 1, "it keeps what it has");
    let mut corrected = swing_graph(4);
    corrected.resolve(|_| ENEMY_SWING);
    let Stage { scene, show, content, renderer, gpu, .. } = &mut stage;
    show.replace_graph_document(scene, ENEMY_GRAPH, corrected, content, renderer, gpu)
        .expect("that one fits");
    assert_eq!(stage.enemy_joints(), joints + 2, "it started over on the new joints");
}

/// A clip one fighter's graph can't hold says nothing about the other's, which goes on to hold what it
/// names, either way round, and the first failure is still heard.
#[cfg(feature = "dev")]
#[test]
#[ignore = "needs a GPU"]
fn a_clip_one_fighter_cant_hold_leaves_the_others_held() {
    let nothing_cooked = AssetId::from_bytes([0x33; 16]);
    let mut stage = Stage::new();
    stage.play_fighters();
    let content = &mut stage.content;

    content.set_live_graph(ENEMY_GRAPH, Some(graph_naming(&[nothing_cooked])));
    content.set_live_graph(GraphKey::CHARACTER, Some(graph_naming(&[OTHER_SWING])));
    let error = content
        .hold_graph_clips(&[(ENEMY_GRAPH, &[]), (GraphKey::CHARACTER, &[])])
        .expect_err("nothing cooked it");
    assert_eq!(error, LoadError::NotFound(nothing_cooked));
    assert!(
        content.holds_graph_clip(GraphKey::CHARACTER, OTHER_SWING),
        "the character's, past the enemy's failure"
    );

    content.set_live_graph(ENEMY_GRAPH, Some(graph_naming(&[OTHER_SWING])));
    content.set_live_graph(GraphKey::CHARACTER, Some(graph_naming(&[nothing_cooked])));
    let error = content
        .hold_graph_clips(&[(ENEMY_GRAPH, &[]), (GraphKey::CHARACTER, &[])])
        .expect_err("nothing cooked it");
    assert_eq!(error, LoadError::NotFound(nothing_cooked));
    assert!(content.holds_graph_clip(ENEMY_GRAPH, OTHER_SWING), "the enemy's, past the character's failure");
}

/// The same in a fight, where either fighter's clip landing retries both: the enemy's makes it whole
/// and leaves the player waiting for its own, and the player's starts its driver on the new joints.
#[test]
#[ignore = "needs a GPU"]
fn a_fighter_waits_for_clips_that_land_after_the_new_character() {
    let mut stage = Stage::new();
    stage.play_fighters();
    let mut fight = stage.start_fight();
    let (joints, enemy_joints) = (stage.joints(), stage.enemy_joints());

    stage.recook_all_but(&[SWING, OTHER_SWING, ENEMY_SWING], 1, 5);
    stage.frames_until(|stage| {
        stage.versions() == (1, 1, 1) && stage.show.enemy_versions(&stage.content) == Some((1, 1))
    });
    let versions = stage.content.graph_versions();
    stage.recook_all_but(&[SWING, OTHER_SWING], 1, 5);
    stage.frames_until(|stage| stage.content.graph_versions() != versions);
    assert_eq!(stage.joints(), joints, "it keeps what it has");
    let waiting = stage.scene.graph_player().expect("playing");
    assert!(!waiting.has_replacement(), "nothing bound to the new joints yet");
    assert_eq!(stage.enemy_joints(), enemy_joints + 1, "the enemy's whole");

    stage.recook(0.0, 1);
    stage.frames_until(|stage| stage.joints() != joints);
    assert_eq!(stage.joints(), joints + 1, "the clips landed, and it's on the new joints");
    for _ in 0..30 {
        let stats = stage.step_fight(&mut fight);
        assert_eq!(stats.ignored_poses, 0, "{stats:?}");
    }
}
