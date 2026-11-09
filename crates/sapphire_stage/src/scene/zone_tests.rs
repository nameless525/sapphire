//! A synthetic zone through the whole stage, on a real renderer: what streaming does to the packet, what
//! a new renderer and a closing zone do to it, and what a walk costs. GPU tests, as the rest of the
//! stage's are.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurement's report")]

use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::material::{AnimeCharacterMaterial, Material, MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::texture::Swizzle;
use sapphire_assets::{
    AssetBudgets, AssetId, AssetKind, AssetServer, LoadPriority, MemoryPackSource, MountState, PipelineKey,
};
use sapphire_livelink::LivePacks;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use sapphire_synthetic_zone::SyntheticZone;
use sapphire_time::Stopwatch;
use sapphire_world::streaming::{StreamBudget, StreamSettings};
use sapphire_zone::CookedExit;

use super::*;
use crate::content::Content;
use crate::extract::RenderMesh;
use crate::orbit::OrbitCamera;

const CHARACTER: AssetId = AssetId::from_bytes([0x77; 16]);
const SKY: AssetId = AssetId::from_bytes([0x78; 16]);
const SIDE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const STEM: &str = "synthetic";
const PACK: &str = "zone_synthetic";
/// An asset that's never loaded, there for the keys its row in the pack lists.
const LISTS_KEYS: AssetId = AssetId::from_bytes([0x79; 16]);

/// The pack the rooms' character is in, so that it stays when the room's goes.
const BASE_PACK: &str = "base";

fn room_pack(room: u32) -> String {
    format!("zone_room_{room}")
}

/// The ID of the exit to room `room`.
fn exit_to(room: u32) -> u64 {
    sapphire_core::Name::new(&format!("to_room_{room}")).as_u64()
}

fn room(room: u32) -> ZoneTarget {
    ZoneTarget { zone: SyntheticZone::room_id(room), pack: room_pack(room), arrive: 0 }
}

/// An asset listing `keys`.
fn listing(keys: Vec<PipelineKey>) -> PackAsset {
    listing_as(LISTS_KEYS, keys)
}

/// An asset `id` listing `keys`.
fn listing_as(id: AssetId, keys: Vec<PipelineKey>) -> PackAsset {
    let mut listed =
        PackAsset::encode(id, AssetKind::Mesh, Codec::Lz4, &[0; 64], Vec::new(), 3).expect("an asset");
    listed.pipeline_keys = keys;
    listed
}

/// A cooked triangle with a material of its own, the zone's stage needing a character to stand there.
fn character() -> PackAsset {
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x, y],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material =
        Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("a valid material");
    let slot = MaterialSlot::embedded("skin", material);
    let submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
    let vertices = [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(0.0, 1.0)];
    let mesh = Mesh::from_vertices(&vertices, vec![0, 1, 2], submeshes, vec![slot], PositionFormat::Float32);
    let bytes = mesh.to_cooked().expect("it cooks").into_vec();
    PackAsset::encode(CHARACTER, AssetKind::Mesh, Codec::Lz4, &bytes, Vec::new(), 3).expect("it encodes")
}

/// The stage over a synthetic zone in memory, run frame by frame as the app runs it.
struct Walk {
    // Drop order: what lives on the GPU before the device.
    scene: Scene,
    content: Content,
    graph: GraphExecutor,
    renderer: Renderer,
    target: wgpu::Texture,
    gpu: GpuContext,
    /// Where the zone's origin lies, as the scene sees it: minus the first focus.
    start: Vec3,
}

impl Walk {
    fn new(zone: SyntheticZone, settings: StreamSettings, start: Vec3) -> Self {
        Self::listing(zone, settings, start, Vec::new(), None)
    }

    /// As [`Self::new`], with an asset in the zone's pack listing `keys`, and `left_out` left out of what the
    /// zone builds as it opens.
    fn listing(
        zone: SyntheticZone,
        settings: StreamSettings,
        start: Vec3,
        keys: Vec<PipelineKey>,
        left_out: Option<String>,
    ) -> Self {
        let budgets = AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] };
        Self::over(zone, settings, start, budgets, keys, left_out)
    }

    fn with_budgets(
        zone: SyntheticZone,
        settings: StreamSettings,
        start: Vec3,
        budgets: AssetBudgets,
    ) -> Self {
        Self::over(zone, settings, start, budgets, Vec::new(), None)
    }

    fn over(
        zone: SyntheticZone,
        settings: StreamSettings,
        start: Vec3,
        budgets: AssetBudgets,
        keys: Vec<PipelineKey>,
        left_out: Option<String>,
    ) -> Self {
        let source = Arc::new(MemoryPackSource::new());
        source.insert(PACK, zone.pack(vec![character(), listing(keys)]).expect("a pack"));
        let mut walk = Self::standing(source, PACK, budgets, left_out);
        let opened = ZoneStage::open(
            &mut walk.content,
            SyntheticZone::id(),
            STEM,
            start,
            settings,
            &mut walk.renderer,
            &walk.gpu,
        );
        walk.scene.open_zone(opened.expect("the zone"), &mut walk.content).expect("installed");
        walk.start = start;
        walk.look_from(start);
        walk
    }

    /// The stage with nothing open, its character in `pack`.
    fn standing(
        source: Arc<MemoryPackSource>,
        pack: &str,
        budgets: AssetBudgets,
        left_out: Option<String>,
    ) -> Self {
        let live_packs = Arc::new(LivePacks::new(source));
        let mut server = AssetServer::new(live_packs.clone(), budgets);
        server.mount_now(pack).expect("the pack");
        let character = server.load_with(CHARACTER, LoadPriority::Critical).expect("loading");
        let mut content = Content::over_server(server, CHARACTER, character, SKY, live_packs);
        content.leave_out_of_zones(left_out);
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let (mesh, bounds) = content.character(&mut renderer, &gpu).expect("the character");
        let mut scene = Scene::with_character(mesh, bounds, false);
        // It holds still, so that what a packet changes is the zone's.
        let character = scene.character;
        scene.ecs.world_mut().entity_mut(character).remove::<Turntable>();
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
        let graph = GraphExecutor::new();
        let mut walk = Self { scene, content, graph, renderer, target, gpu, start: Vec3::ZERO };
        walk.look_from(Vec3::ZERO);
        walk
    }

    /// The stage with nothing open, and `rooms` zones to go between in packs of their own, each with an exit
    /// to the next, the last's to the first, marked `prefetch`.
    fn rooms(zone: SyntheticZone, rooms: u32, budgets: AssetBudgets, keys: Vec<PipelineKey>) -> Self {
        let source = Arc::new(MemoryPackSource::new());
        source.insert(BASE_PACK, write_pack(vec![character()]).expect("a pack"));
        for room in 0..rooms {
            let next = (room + 1) % rooms;
            let exit = CookedExit {
                id: exit_to(next),
                name: format!("to_room_{next}"),
                to: SyntheticZone::room_id(next),
                pack: room_pack(next),
                arrive: 0,
                prefetch: true,
            };
            let extra = vec![listing_as(LISTS_KEYS.sub_asset(&format!("room/{room}")), keys.clone())];
            source.insert(&room_pack(room), zone.room_pack(room, vec![exit], extra).expect("a pack"));
        }
        Self::standing(source, BASE_PACK, budgets, None)
    }

    /// Puts the camera at about `place`, in the zone's space.
    fn look_from(&mut self, place: Vec3) {
        *self.scene.camera_mut() = OrbitCamera::fitting(place - self.start, (0.5, 0.5));
    }

    /// One frame as the app runs it. Returns how many instances and how many lights its packet changed
    /// and removed, how many lights it has, and what the content's uploads and the renderer's prepare
    /// took and did.
    fn frame(&mut self) -> Frame {
        let before = self.renderer.stats();
        let stopwatch = Stopwatch::start();
        self.content.update(&mut self.renderer, &self.gpu);
        let uploads = stopwatch.elapsed();
        let grew = self.renderer.stats().buffer_growths != before.buffer_growths;
        self.scene.follow_content(&mut self.content, &mut ());
        self.scene.step();
        let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut ());
        let mut frame = Frame {
            changed: packet.instances.changed.len(),
            removed: packet.instances.removed.len(),
            lights: packet.lights.len(),
            uploads,
            grew,
            ..Frame::default()
        };
        let stopwatch = Stopwatch::start();
        self.renderer.prepare(&self.gpu, packet);
        frame.prepare = stopwatch.elapsed();
        frame.rebuilt = self.renderer.stats().draw_list_builds != before.draw_list_builds;
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        self.scene.end_frame();
        // A surface's present holds a game to a few frames in flight. With none, a CPU device falls as
        // far behind as the queue lets it, and what's on its way back through those frames fills the pool.
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        frame
    }

    /// Frames until what's been let go of has gone from the renderer: what's left is what the scene
    /// draws and what the server still holds.
    fn drain(&mut self) {
        for _ in 0..1_000 {
            self.frame();
            if self.renderer.stats().retiring == 0 {
                return;
            }
            self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        }
        panic!("it never drained: {:?}", self.renderer.stats().retiring);
    }

    /// The textures that live on the GPU, ours and the renderer's.
    fn gpu_textures(&self) -> usize {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.textures.num_kept_from_user
    }

    /// How many entities draw a mesh: before the first frame, those the scene has of its own.
    fn drawing(&mut self) -> usize {
        let world = self.scene.ecs.world_mut();
        world.query::<&RenderMesh>().iter(world).count()
    }

    /// Frames until the streamer has nothing left to do, totting them up.
    fn settle(&mut self) -> Frame {
        let mut total = Frame::default();
        for _ in 0..10_000 {
            let frame = self.frame();
            total.changed += frame.changed;
            total.removed += frame.removed;
            total.lights = frame.lights;
            let stats = self.scene.stream_stats().expect("a zone is open");
            if stats.pending == 0 && stats.loading == 0 {
                // What it just spawned is in the next packet.
                let last = self.frame();
                total.changed += last.changed;
                total.removed += last.removed;
                return total;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        panic!("it stalled: {:?}", self.scene.stream_stats());
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    changed: usize,
    removed: usize,
    lights: usize,
    uploads: Duration,
    prepare: Duration,
    /// The uploads grew a buffer.
    grew: bool,
    /// Prepare rebuilt the draw list.
    rebuilt: bool,
}

/// What a trip took, and what it holds once it's over.
#[derive(Clone, Copy, Debug)]
struct Trip {
    /// From asking to the zone being ready to arrive in.
    load: Duration,
    frames: u32,
    /// The longest frame's content update, while it loaded.
    worst_upload: Duration,
}

impl Walk {
    /// Goes to `to`, as someone under a wipe waits for it: a zone already coming goes on from where it's got.
    fn trip(&mut self, to: u32, settings: StreamSettings) -> Trip {
        self.scene.prefetch_zone(room(to), settings);
        self.scene.hurry_zone(room(to).zone);
        let stopwatch = Stopwatch::start();
        let mut frames = 0;
        let mut worst_upload = Duration::ZERO;
        while self.scene.zone_load(room(to).zone) != ZoneLoad::Ready {
            assert_eq!(
                std::mem::discriminant(&self.scene.zone_load(room(to).zone)),
                std::mem::discriminant(&ZoneLoad::Waiting),
                "{:?}",
                self.scene.zone_load(room(to).zone),
            );
            worst_upload = worst_upload.max(self.frame().uploads);
            frames += 1;
            assert!(frames < 100_000, "it never came");
            std::thread::sleep(Duration::from_micros(100));
        }
        let trip = Trip { load: stopwatch.elapsed(), frames, worst_upload };
        let arrived = self.scene.arrive_in_zone(room(to).zone, &mut self.content).expect("it's ready");
        if let Some(left) = arrived.left {
            let report = left.expect("the last zone's pack unmounts");
            assert_eq!(report.cancelled, 0, "{report:?}");
        }
        for _ in 0..1_000 {
            if self.scene.zone_settled() {
                break;
            }
            self.frame();
        }
        assert!(self.scene.zone_settled(), "{:?}", self.scene.stream_stats());
        self.scene.release_zone_focus();
        trip
    }

    /// What's left of a trip's cost, once what it let go of has gone.
    fn holds(&mut self) -> TripHolds {
        self.settle();
        self.drain();
        let (server, _) = self.content.streaming_parts();
        let resident = server.stats().resident.iter().sum();
        let stats = self.renderer.stats();
        TripHolds {
            resident,
            meshes: stats.meshes,
            mesh_pool_bytes: stats.mesh_pool_bytes,
            materials: stats.materials,
            material_bytes: stats.material_bytes,
            texture_bytes: self.content.texture_bytes(),
            gpu_textures: self.gpu_textures(),
        }
    }
}

/// What being in a place holds: the same on each visit, or a trip's leaking.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TripHolds {
    /// The server's cooked bytes.
    resident: u64,
    meshes: u32,
    mesh_pool_bytes: u64,
    materials: u32,
    material_bytes: u64,
    texture_bytes: u64,
    gpu_textures: usize,
}

/// A little of everything, near enough to see all at once.
const SMALL: SyntheticZone =
    SyntheticZone { side: 4, cell_size: 32.0, instances: 2, lights: 1, quads: 2, texture: 0 };

fn small_settings() -> StreamSettings {
    StreamSettings {
        activate: 32.0,
        load: 48.0,
        release: 64.0,
        budget: StreamBudget { entities: 1_000, micros: 1_000_000 },
    }
}

#[test]
#[ignore = "needs a GPU"]
fn replacing_a_zone_does_not_orphan_its_entities_or_unmount_its_new_handles() {
    let mut walk = Walk::new(SyntheticZone::SMALL, StreamSettings::DESKTOP, Vec3::ZERO);
    walk.settle();
    let drawing = walk.drawing();
    for _ in 0..4 {
        let next = ZoneStage::open(
            &mut walk.content,
            SyntheticZone::id(),
            STEM,
            Vec3::ZERO,
            StreamSettings::DESKTOP,
            &mut walk.renderer,
            &walk.gpu,
        )
        .expect("replacement loaded");
        walk.scene.open_zone(next, &mut walk.content).expect("replacement installed");
        walk.settle();
        assert_eq!(walk.drawing(), drawing);
        assert!(walk.content.streaming_parts().0.is_mounted(PACK));
        assert_eq!(walk.scene.stream_stats().expect("zone").failed, 0);
    }
    walk.scene.close_zone(&mut walk.content).expect("open").expect("closed");
    assert_eq!(walk.drawing(), 1, "only the character remains");
}

/// A pack mounted after the load's prewarm builds its own keys before a cell of it arrives, unless told not
/// to: a load's `--inventory-without` has to fail whichever pack lists the entry.
#[test]
#[ignore = "needs a GPU"]
fn a_zone_builds_what_its_pack_lists_as_it_opens() {
    let key = PipelineKey {
        family: MaterialFamily::Environment,
        flags: MaterialFlags::empty(),
        format: PositionFormat::Float32,
        reflection: false,
    };
    let prewarmed = |keys: Vec<PipelineKey>, left_out: Option<String>| {
        let walk = Walk::listing(SMALL, small_settings(), Vec3::ZERO, keys, left_out);
        walk.renderer.stats().pipelines.prewarmed
    };
    // A renderer warms some of its own as it starts, whatever a pack lists.
    let listing_nothing = prewarmed(Vec::new(), None);
    assert!(prewarmed(vec![key], None) > listing_nothing, "the key was built");
    assert_eq!(prewarmed(vec![key], Some(format!("mesh {key}"))), listing_nothing, "the entry was left out");
}

#[test]
#[ignore = "needs a GPU"]
fn a_cell_reaches_the_packet_as_instance_deltas_and_lights_and_leaves_the_same_way() {
    // At a corner: its own cell and the two beside it and the one across are all within 32 m.
    let mut walk = Walk::new(SMALL, small_settings(), Vec3::new(16.0, 0.0, 16.0));
    let own = walk.drawing();
    let first = walk.settle();
    let stats = walk.scene.stream_stats().expect("a zone is open");
    assert_eq!(stats.failed, 0);
    let cells = stats.active - 1;
    assert!(cells >= 4, "{stats:?}");
    // The scene's own, the backdrop, and each cell's mesh and instances.
    assert_eq!(first.changed as u32, own as u32 + 1 + cells * (1 + SMALL.instances), "{stats:?}");
    assert_eq!(first.removed, 0);
    assert_eq!(first.lights as u32, cells * SMALL.lights);

    // Far enough off the zone's other corner that only cells there are left, and those first ones gone.
    walk.look_from(Vec3::new(112.0, 0.0, 112.0));
    let second = walk.settle();
    let after = walk.scene.stream_stats().expect("a zone is open");
    assert!(after.total_removed > 0);
    assert_eq!(second.removed as u32, cells * (1 + SMALL.instances));
    assert_eq!(walk.frame().changed, 0, "nothing moves once it's settled");
    assert_eq!(walk.frame().removed, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_budget_of_one_shows_a_cell_a_thing_at_a_time_and_the_packet_agrees() {
    let budget = StreamBudget { entities: 1, micros: 0 };
    let settings = StreamSettings { budget, ..small_settings() };
    let mut walk = Walk::new(SMALL, settings, Vec3::new(16.0, 0.0, 16.0));
    // The load screen's frame places what it loaded, all at once: from there on, the budget.
    walk.frame();
    walk.look_from(Vec3::new(112.0, 0.0, 112.0));
    for _ in 0..2_000 {
        let frame = walk.frame();
        assert!(frame.changed <= 2, "{frame:?}");
        std::thread::sleep(Duration::from_micros(100));
    }
    let stats = walk.scene.stream_stats().expect("a zone is open");
    assert_eq!((stats.pending, stats.failed), (0, 0), "{stats:?}");
    assert!(stats.total_removed > 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_new_renderer_draws_what_the_zone_had_placed() {
    let mut walk = Walk::new(SMALL, small_settings(), Vec3::new(16.0, 0.0, 16.0));
    walk.settle();
    let cells = walk.scene.stream_stats().expect("a zone is open");

    let _old = walk.content.renderer_rebuilt();
    let mut renderer = Renderer::new(&walk.gpu, FORMAT, RenderSettings::default()).expect("renderer");
    walk.content.character(&mut renderer, &walk.gpu).expect("the character");
    walk.scene.set_zone_meshes(&mut walk.content, &mut renderer, &walk.gpu);
    walk.renderer = renderer;
    walk.scene.resend_instances();

    // Each entity draws the handle the new renderer has for its mesh, and frames go on over it.
    let zone = walk.scene.zone.as_mut().expect("a zone is open");
    let placed: Vec<(Entity, AssetId)> =
        zone.streamer.placed_mut().filter_map(|placed| Some((placed.entity, placed.mesh?))).collect();
    {
        let (_, meshes) = walk.content.streaming_parts();
        for (entity, mesh) in placed {
            let drawn = walk.scene.ecs.world().get::<RenderMesh>(entity).expect("it draws").0;
            assert_eq!(Some(drawn), meshes(mesh), "{mesh}");
        }
    }
    let frame = walk.frame();
    assert!(frame.changed as u32 > cells.active, "all of it, sent again: {frame:?}");
    assert_eq!(walk.scene.stream_stats().expect("a zone is open").failed, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn closing_a_zone_takes_its_instances_away_and_lets_its_pack_go() {
    let mut walk = Walk::new(SMALL, small_settings(), Vec3::new(16.0, 0.0, 16.0));
    let own = walk.drawing();
    let opened = walk.settle();
    let live = opened.changed - own;

    // The character stays, in the pack the test put it in: so it can't go with the pack's assets.
    let report = walk.scene.close_zone(&mut walk.content).expect("a zone is open").expect("it unmounts");
    assert_eq!(report.cancelled, 0);
    assert!(report.unloaded > 0, "{report:?}");
    assert!(walk.scene.stream_stats().is_none());
    let frame = walk.frame();
    assert_eq!(frame.removed, live);
    assert_eq!(frame.lights, 0);
    let (server, _) = walk.content.streaming_parts();
    assert!(matches!(server.mount_state(PACK), MountState::Unmounted));
    assert!(!server.contains(SyntheticZone::id()));
    // Frames go on, with nothing left of it. And closing again is nothing to do.
    walk.frame();
    assert!(walk.scene.close_zone(&mut walk.content).is_none());
    // Its meshes, materials and textures came back: the character's, and the sky's, are what's left.
    walk.drain();
    let stats = walk.renderer.stats();
    assert_eq!(stats.meshes, 1, "{stats:?}");
    assert!(stats.meshes_freed > 0);
    assert_eq!(walk.content.texture_bytes(), 0);
}

/// Budgets of next to nothing, so that a server unloads whatever nothing holds as soon as nothing does.
fn nothing_cached() -> AssetBudgets {
    AssetBudgets { upload_per_frame: u64::MAX, resident: [1; AssetKind::ALL.len()] }
}

/// What a visit to one place holds on the renderer: the same on each visit, or something's leaking.
#[derive(Debug, PartialEq)]
struct Holds {
    draws: u32,
    instances: u32,
    meshes: u32,
    mesh_pool_bytes: u64,
    materials: u32,
    material_bytes: u64,
    texture_bytes: u64,
    gpu_textures: usize,
}

/// A walk out to the zone's far corner and back, over and over, with each cell evicted when it's left:
/// every return draws the same from the same room, and the buffers stop growing after the first.
#[test]
#[ignore = "needs a GPU"]
fn cells_evicted_and_entered_again_cost_nothing_more_each_time() {
    let zone = SyntheticZone { texture: 4, ..SMALL };
    let (home, away) = (Vec3::new(16.0, 0.0, 16.0), Vec3::new(112.0, 0.0, 112.0));
    let mut walk = Walk::with_budgets(zone, small_settings(), home, nothing_cached());
    let visit = |walk: &mut Walk, place: Vec3| {
        walk.look_from(place);
        walk.settle();
        walk.drain();
        let stats = walk.scene.stream_stats().expect("a zone is open");
        assert_eq!((stats.pending, stats.loading, stats.failed), (0, 0, 0), "{stats:?}");
        let stats = walk.renderer.stats();
        Holds {
            draws: stats.draws,
            instances: stats.instances,
            meshes: stats.meshes,
            mesh_pool_bytes: stats.mesh_pool_bytes,
            materials: stats.materials,
            material_bytes: stats.material_bytes,
            texture_bytes: walk.content.texture_bytes(),
            gpu_textures: walk.gpu_textures(),
        }
    };
    let at_home = visit(&mut walk, home);
    let at_away = visit(&mut walk, away);
    assert!(at_home.texture_bytes > 0, "{at_home:?}");
    let (capacity, growths) = {
        let stats = walk.renderer.stats();
        (stats.mesh_pool_capacity, stats.buffer_growths)
    };
    let mut freed = walk.renderer.stats().meshes_freed;
    assert!(freed > 0, "the first cells left");
    for lap in 0..6 {
        assert_eq!(visit(&mut walk, home), at_home, "lap {lap}");
        assert_eq!(visit(&mut walk, away), at_away, "lap {lap}");
        let stats = walk.renderer.stats();
        assert_eq!(stats.mesh_pool_capacity, capacity, "lap {lap}: the pool grew");
        assert_eq!(stats.buffer_growths, growths, "lap {lap}: a buffer grew");
        assert!(stats.meshes_freed > freed, "lap {lap}: the cells it left stayed");
        freed = stats.meshes_freed;
    }
}

/// The renderer set aside for a trial, and kept in the end, was told nothing of what the server unloaded
/// meanwhile: it gives those back all the same.
#[test]
#[ignore = "needs a GPU"]
fn a_renderer_kept_after_a_trial_gives_back_what_was_unloaded_during_it() {
    let zone = SyntheticZone { texture: 4, ..SMALL };
    let mut walk = Walk::new(zone, small_settings(), Vec3::new(16.0, 0.0, 16.0));
    walk.settle();
    walk.drain();
    let stats = walk.renderer.stats();
    assert!(stats.meshes > 1 && walk.content.texture_bytes() > 0, "{stats:?}");

    let old = walk.content.renderer_rebuilt();
    let mut trial = Renderer::new(&walk.gpu, FORMAT, RenderSettings::default()).expect("renderer");
    walk.scene.close_zone(&mut walk.content).expect("a zone is open").expect("it unmounts");
    walk.content.update(&mut trial, &walk.gpu);
    assert_eq!(trial.stats().meshes, 0, "the server's unloads went to the trial, which held none of it");

    // The trial fails. Back on the first renderer, what it holds of them goes.
    drop(trial);
    walk.content.keep_renderer(old);
    assert_eq!(walk.content.texture_bytes(), 0);
    walk.drain();
    let stats = walk.renderer.stats();
    assert_eq!(stats.meshes, 1, "only the character's: {stats:?}");
    assert!(stats.meshes_freed > 0, "{stats:?}");
}

/// A walk that takes in every cell of a zone bigger than its budgets, there and back, on each class of
/// device's settings and budgets: what's on the GPU stays in them, and a return costs no more room than
/// the way out. `--nocapture` in release reports it, with what uploads and the draw list's rebuilds
/// cost; a debug build, as CI's, walks a small zone on small budgets and holds the same.
#[test]
#[ignore = "needs a GPU"]
fn a_walk_over_a_whole_zone_keeps_what_it_holds_within_its_budgets() {
    let tiers = if cfg!(debug_assertions) {
        let zone = SyntheticZone { side: 8, quads: 4, texture: 8, ..SyntheticZone::LARGE };
        let settings =
            StreamSettings { activate: 64.0, load: 96.0, release: 128.0, ..StreamSettings::DESKTOP };
        let mut budgets = AssetBudgets::DESKTOP;
        budgets.resident[AssetKind::Mesh.index()] = 64 * 1024;
        budgets.resident[AssetKind::Texture.index()] = 16 * 1024;
        vec![("debug", zone, settings, budgets)]
    } else {
        let desktop = SyntheticZone { quads: 128, texture: 1024, ..SyntheticZone::LARGE };
        let mobile = SyntheticZone { quads: 64, texture: 512, ..SyntheticZone::LARGE };
        vec![
            ("Desktop-Low", desktop, StreamSettings::DESKTOP, AssetBudgets::DESKTOP),
            ("Mobile-Low", mobile, StreamSettings::MOBILE, AssetBudgets::MOBILE),
        ]
    };
    for (name, zone, settings, budgets) in tiers {
        let width = zone.cell_size * zone.side as f32;
        // Up and down lanes close enough that every cell is loaded from one or another.
        let spacing = 2.0 * settings.load;
        let lanes = (width / spacing).ceil() as usize;
        let mut path = Vec::new();
        for lane in 0..lanes {
            let z = (lane as f32 + 0.5) * spacing;
            let (from, to) = if lane % 2 == 0 { (0.0, width) } else { (width, 0.0) };
            path.push(Vec3::new(from, 0.0, z));
            path.push(Vec3::new(to, 0.0, z));
        }
        let back: Vec<Vec3> = path.iter().rev().copied().collect();
        path.extend(back);
        // 40 m/s, at 60 frames a second.
        let step = 40.0 / 60.0;
        let (mesh_budget, texture_budget) =
            (budgets.resident(AssetKind::Mesh), budgets.resident(AssetKind::Texture));
        let mut walk = Walk::with_budgets(zone, settings, path[0], budgets);

        let mut frames = 0u32;
        let mut most_textures = 0;
        let mut most_mesh_bytes = 0;
        let mut most_resident_meshes = 0;
        let mut most_resident_textures = 0;
        let mut most_meshes = 0;
        let mut most_retiring = 0;
        let mut most_materials = 0;
        let mut half = None;
        let mut rebuilt = Vec::new();
        let mut steady = Vec::new();
        let mut grown = Vec::new();
        let mut plain = Vec::new();
        // The longest uploads, and the bytes they uploaded.
        let mut worst_uploads = (Duration::ZERO, 0);
        for (at, (&from, &to)) in path.iter().zip(&path[1..]).enumerate() {
            if at == path.len() / 2 {
                let stats = walk.renderer.stats();
                half = Some((stats.mesh_pool_capacity, stats.buffer_growths, stats.meshes_freed));
            }
            let steps = (from.distance(to) / step).ceil().max(1.0) as u32;
            for n in 0..steps {
                walk.look_from(from.lerp(to, n as f32 / steps as f32));
                let frame = walk.frame();
                frames += 1;
                let stats = walk.renderer.stats();
                most_textures = most_textures.max(walk.content.texture_bytes());
                most_mesh_bytes = most_mesh_bytes.max(stats.mesh_pool_bytes);
                let assets = walk.content.stats();
                let resident = assets.resident;
                if frames > 1 && frame.uploads > worst_uploads.0 {
                    worst_uploads = (frame.uploads, assets.last_upload.bytes);
                }
                most_resident_meshes = most_resident_meshes.max(resident[AssetKind::Mesh.index()]);
                most_resident_textures = most_resident_textures.max(resident[AssetKind::Texture.index()]);
                most_meshes = most_meshes.max(stats.meshes);
                most_retiring = most_retiring.max(stats.retiring);
                most_materials = most_materials.max(stats.materials);
                if frames > 1 {
                    (if frame.rebuilt { &mut rebuilt } else { &mut steady }).push(frame.prepare);
                    (if frame.grew { &mut grown } else { &mut plain }).push(frame.uploads);
                }
            }
        }
        let stats = walk.renderer.stats();
        let streaming = walk.scene.stream_stats().expect("a zone is open");
        assert_eq!(streaming.failed, 0, "{streaming:?}");
        let (capacity, growths, freed_out) = half.expect("it got halfway");
        assert!(freed_out > 0, "{name}: nothing was given back on the way out");
        assert!(stats.meshes_freed > freed_out, "{name}: nothing was given back on the way home");
        assert!(most_textures <= texture_budget, "{name}: {most_textures} of textures, of {texture_budget}");
        // The server counts loads on their way, and trims at its next update; the pool's bytes lag it by
        // what's on its way back through a frame. Either can pass a budget by that much, and no more.
        let near = |budget: u64| budget + budget / 8;
        assert!(
            most_resident_textures <= near(texture_budget),
            "{name}: {most_resident_textures}, of {texture_budget}"
        );
        assert!(
            most_resident_meshes <= near(mesh_budget),
            "{name}: {most_resident_meshes}, of {mesh_budget}"
        );
        assert!(
            most_mesh_bytes <= near(mesh_budget),
            "{name}: {most_mesh_bytes} of meshes, of {mesh_budget}"
        );
        assert_eq!(stats.mesh_pool_capacity, capacity, "{name}: the way back grew the pool");
        for list in [&mut rebuilt, &mut steady, &mut grown, &mut plain] {
            list.push(Duration::ZERO);
            list.sort_unstable();
        }
        let at =
            |list: &[Duration], percent: usize| list[(list.len() - 1) * percent / 100].as_secs_f64() * 1e6;
        println!(
            "{name} memory: {frames} frames over {} cells; textures most {:.0} MiB of {:.0}; meshes resident most \
             {:.0} MiB (textures {most_resident_textures}), in the pool most {:.0} MiB, of {:.0} (pool capacity {:.0} MiB, {} growths in all, {growths} on the way out); most meshes {most_meshes}, \
             materials {most_materials}, retiring {most_retiring}; freed {}; streaming over its budget in {} \
             frames, peak {:.0} us; the longest uploads were {:.1} MiB; prepare us with a rebuild ({}) p50 {:.0} \
             p99 {:.0} worst {:.0}, without ({}) p50 {:.0} p99 {:.0} worst {:.0}; uploads us with growth ({}) p50 \
             {:.0} worst {:.0}, without ({}) p50 {:.0} p99 {:.0} worst {:.0}",
            zone.side * zone.side,
            most_textures as f64 / 1048576.0,
            texture_budget as f64 / 1048576.0,
            most_resident_meshes as f64 / 1048576.0,
            most_mesh_bytes as f64 / 1048576.0,
            mesh_budget as f64 / 1048576.0,
            stats.mesh_pool_capacity as f64 / 1048576.0,
            stats.buffer_growths,
            stats.meshes_freed,
            streaming.over_budget_frames,
            streaming.peak_time.as_secs_f64() * 1e6,
            worst_uploads.1 as f64 / 1048576.0,
            rebuilt.len() - 1,
            at(&rebuilt, 50),
            at(&rebuilt, 99),
            at(&rebuilt, 100),
            steady.len() - 1,
            at(&steady, 50),
            at(&steady, 99),
            at(&steady, 100),
            grown.len() - 1,
            at(&grown, 50),
            at(&grown, 100),
            plain.len() - 1,
            at(&plain, 50),
            at(&plain, 99),
            at(&plain, 100),
        );
    }
}

/// A sprint across a zone, on each class of device's settings: what §3.2's row reads, with what the
/// uploads cost beside it. `--nocapture` in release gives figures to hold against 500 and 1,000 µs; a
/// debug build, as CI's, walks a small zone and only holds that nothing fails.
#[test]
#[ignore = "needs a GPU"]
fn a_walk_across_a_zone_stays_on_its_budget() {
    for (name, settings) in [("Desktop-Low", StreamSettings::DESKTOP), ("Mobile-Low", StreamSettings::MOBILE)]
    {
        let zone = if cfg!(debug_assertions) {
            SyntheticZone { side: 8, quads: 4, ..SyntheticZone::LARGE }
        } else {
            SyntheticZone::LARGE
        };
        let width = zone.cell_size * zone.side as f32;
        let middle = width / 2.0;
        let mut walk = Walk::new(zone, settings, Vec3::new(0.0, 0.0, middle));
        // 10 m/s, at 60 frames a second.
        let step = 10.0 / 60.0;
        let frames = (width / step) as usize;
        let mut times = Vec::with_capacity(frames);
        let mut uploads = Vec::with_capacity(frames);
        let mut first = Duration::ZERO;
        let mut over = 0;
        let mut most_placed = 0;
        let mut most_entities = 0;
        for frame in 0..frames {
            walk.look_from(Vec3::new(frame as f32 * step, 0.0, middle));
            let drawn = walk.frame();
            uploads.push(drawn.uploads);
            let stats = walk.scene.stream_stats().expect("a zone is open");
            most_placed = most_placed.max(if frame == 0 { 0 } else { stats.placed });
            most_entities = most_entities.max(stats.entities);
            // The first frame places a load screen's worth, off the budget.
            if frame == 0 {
                first = stats.time;
                continue;
            }
            over += u32::from(stats.time.as_micros() > u128::from(settings.budget.micros));
            times.push(stats.time);
            assert!(stats.placed <= settings.budget.entities, "{stats:?}");
        }
        let stats = walk.scene.stream_stats().expect("a zone is open");
        assert_eq!(stats.failed, 0, "{stats:?}");
        assert!(stats.total_removed > 0, "it left some behind: {stats:?}");
        times.sort_unstable();
        uploads.sort_unstable();
        let at =
            |list: &[Duration], percent: usize| list[(list.len() - 1) * percent / 100].as_secs_f64() * 1e6;
        println!(
            "{name} walk: {frames} frames, {} cells; first frame us {:.1}; after it streaming us p50 {:.1} p99 \
             {:.1} p99.9 {:.1} worst {:.1} against {}, {over} over; uploads us p50 {:.1} p99 {:.1} worst {:.1}; \
             most placed in a frame {most_placed}, most entities {most_entities}, placed {} removed {}",
            zone.side * zone.side,
            first.as_secs_f64() * 1e6,
            at(&times, 50),
            at(&times, 99),
            times[(times.len() - 1) * 999 / 1000].as_secs_f64() * 1e6,
            at(&times, 100),
            settings.budget.micros,
            at(&uploads, 50),
            at(&uploads, 99),
            at(&uploads, 100),
            stats.total_placed,
            stats.total_removed,
        );
    }
}

fn no_limits() -> AssetBudgets {
    AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] }
}

/// A zone coming next costs a hub no change on screen: nothing's placed until it's arrived in, and its pack
/// comes with `mount`, its meshes and its pipelines over frames. Arriving takes the last zone's all away.
#[test]
#[ignore = "needs a GPU"]
fn a_zone_coming_next_places_nothing_until_it_is_arrived_in_and_then_the_last_one_goes() {
    let settings = small_settings();
    let key = PipelineKey {
        family: MaterialFamily::Environment,
        flags: MaterialFlags::empty(),
        format: PositionFormat::Float32,
        reflection: false,
    };
    let mut walk = Walk::rooms(SMALL, 2, no_limits(), vec![key]);
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    walk.trip(0, settings);
    let at_first = walk.holds();
    let drawing = walk.drawing();

    // What the hub says is next: nobody's going yet.
    walk.scene.prefetch_exits(&mut walk.content, settings);
    let target = room(1);
    assert_eq!(
        walk.scene.zone_exit(exit_to(1), &mut walk.content),
        Some(target.clone()),
        "it reads what the zone says",
    );
    assert_eq!(walk.scene.zone_load(target.zone), ZoneLoad::Waiting);
    let mut frames = 0;
    while walk.scene.zone_load(target.zone) != ZoneLoad::Ready {
        let frame = walk.frame();
        assert_eq!((frame.changed, frame.removed), (0, 0), "nothing on stage moved: {frame:?}");
        frames += 1;
        assert!(frames < 100_000, "it never came");
        std::thread::sleep(Duration::from_micros(100));
    }
    assert_eq!(walk.drawing(), drawing, "and nothing's placed");
    let coming = walk.holds();
    assert!(coming.resident > at_first.resident, "{coming:?} over {at_first:?}");
    let (server, _) = walk.content.streaming_parts();
    assert!(matches!(server.mount_state(&room_pack(0)), MountState::Mounted));
    assert!(matches!(server.mount_state(&room_pack(1)), MountState::Mounted));

    // A trip takes what's already there.
    let stopwatch = Stopwatch::start();
    walk.trip(1, settings);
    assert!(stopwatch.elapsed() < Duration::from_secs(5));
    let (server, _) = walk.content.streaming_parts();
    assert!(matches!(server.mount_state(&room_pack(0)), MountState::Unmounted));
    assert!(!server.contains(SyntheticZone::room_id(0)));
    assert!(walk.scene.zone_load(room(0).zone) == ZoneLoad::Waiting, "and nobody's asked for the last");
    assert_eq!(walk.holds().resident, at_first.resident, "the same size of room, and the other's gone");
    assert_eq!(walk.drawing(), drawing);
}

/// A pack that isn't there says so, and what's on stage stays. Asking again tries again.
#[test]
#[ignore = "needs a GPU"]
fn a_zone_that_cannot_come_leaves_what_is_on_stage() {
    let settings = small_settings();
    let mut walk = Walk::rooms(SMALL, 1, no_limits(), Vec::new());
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    walk.trip(0, settings);
    let held = walk.holds();

    let nowhere = ZoneTarget { zone: SyntheticZone::room_id(9), pack: "zone_nowhere".to_owned(), arrive: 0 };
    walk.scene.prefetch_zone(nowhere.clone(), settings);
    let mut frames = 0;
    while !matches!(walk.scene.zone_load(nowhere.zone), ZoneLoad::Failed(_)) {
        walk.frame();
        frames += 1;
        assert!(frames < 100_000, "it never said");
        std::thread::sleep(Duration::from_micros(100));
    }
    assert!(walk.scene.arrive_in_zone(nowhere.zone, &mut walk.content).is_none());
    assert!(walk.scene.zone_settled(), "the zone on stage stays");
    assert_eq!(walk.holds(), held);
    walk.scene.prefetch_zone(nowhere.clone(), settings);
    assert_eq!(walk.scene.zone_load(nowhere.zone), ZoneLoad::Waiting, "trying again");
}

/// A hub stays within streaming's budgets: past nine tenths of the room meshes have, a zone coming next
/// waits, unless someone's waiting for it.
#[test]
#[ignore = "needs a GPU"]
fn a_full_server_makes_a_prefetch_wait_until_someone_is_waiting_for_it() {
    let settings = small_settings();
    let mut budgets = no_limits();
    let mut walk = {
        // As much room as the first room's meshes need, and no more.
        let mut probe = Walk::rooms(SMALL, 2, no_limits(), Vec::new());
        probe.look_from(Vec3::new(16.0, 0.0, 16.0));
        probe.trip(0, settings);
        probe.holds();
        let (server, _) = probe.content.streaming_parts();
        budgets.resident[AssetKind::Mesh.index()] = server.stats().resident[AssetKind::Mesh.index()];
        Walk::rooms(SMALL, 2, budgets, Vec::new())
    };
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    walk.trip(0, settings);
    walk.holds();
    let target = room(1);
    walk.scene.prefetch_zone(target.clone(), settings);
    for _ in 0..200 {
        walk.frame();
        std::thread::sleep(Duration::from_micros(100));
    }
    assert_eq!(walk.scene.zone_load(target.zone), ZoneLoad::Waiting);
    walk.trip(1, settings);
    assert_eq!(walk.scene.stream_stats().expect("a zone is open").failed, 0);
}

/// A hub's next zone builds its pipelines a frame at a time, and asks for no mesh until they're built. Under
/// a wipe they all go at once, there being nothing on screen to stall.
#[test]
#[ignore = "needs a GPU"]
fn a_zone_coming_next_builds_a_pipeline_a_frame_unless_someone_is_waiting() {
    let settings = small_settings();
    let flags = [
        MaterialFlags::empty(),
        MaterialFlags::DOUBLE_SIDED,
        MaterialFlags::ALPHA_TEST,
        MaterialFlags::DOUBLE_SIDED | MaterialFlags::ALPHA_TEST,
    ];
    let keys: Vec<PipelineKey> = flags
        .iter()
        .map(|&flags| PipelineKey {
            family: MaterialFamily::Environment,
            flags,
            format: PositionFormat::Float32,
            reflection: false,
        })
        .collect();
    let mut walk = Walk::rooms(SMALL, 1, no_limits(), keys);
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    // The character's own, on its first frames.
    for _ in 0..3 {
        walk.frame();
    }
    let first_use = walk.renderer.stats().pipelines.first_use_builds;
    let before = walk.renderer.stats().pipelines.prewarmed;
    walk.scene.prefetch_zone(room(0), settings);
    let mut last = before;
    let mut frames_building = 0;
    let mut frames = 0;
    while walk.scene.zone_load(room(0).zone) != ZoneLoad::Ready {
        walk.frame();
        let built = walk.renderer.stats().pipelines.prewarmed;
        assert!(built <= last + 1, "{built} after {last}");
        frames_building += u32::from(built > last);
        last = built;
        frames += 1;
        assert!(frames < 100_000, "it never came");
        std::thread::sleep(Duration::from_micros(100));
    }
    assert!(frames_building >= 2, "over frames: {frames_building}");
    walk.trip(0, settings);
    walk.settle();
    let stats = walk.renderer.stats().pipelines;
    assert_eq!(stats.first_use_builds, first_use, "nothing came before its pipelines: {stats:?}");
}

/// Trip after trip between two zones, a hub and a room, on each class of device's settings and budgets: each
/// visit holds the same as the visit before it, on the server and the renderer, and no buffer grows after
/// the first round. `--nocapture` in release reports it at a size worth reporting; a debug build, as CI's,
/// goes between small ones and holds the same.
#[test]
#[ignore = "needs a GPU"]
fn a_repeated_trip_does_not_grow() {
    let tiers = if cfg!(debug_assertions) {
        let zone = SyntheticZone { texture: 8, ..SMALL };
        vec![
            ("debug, Desktop's budgets", zone, StreamSettings::DESKTOP, AssetBudgets::DESKTOP),
            ("debug, Mobile's budgets", zone, StreamSettings::MOBILE, AssetBudgets::MOBILE),
        ]
    } else {
        let room =
            SyntheticZone { side: 8, cell_size: 64.0, instances: 8, lights: 2, quads: 64, texture: 512 };
        let mobile = SyntheticZone { quads: 32, texture: 256, ..room };
        vec![
            ("Desktop's budgets", room, StreamSettings::DESKTOP, AssetBudgets::DESKTOP),
            ("Mobile's budgets", mobile, StreamSettings::MOBILE, AssetBudgets::MOBILE),
        ]
    };
    for (name, zone, settings, budgets) in tiers {
        let mut walk = Walk::rooms(zone, 2, budgets, Vec::new());
        walk.look_from(Vec3::new(32.0, 0.0, 32.0));
        walk.trip(0, settings);
        let in_first = walk.holds();
        walk.trip(1, settings);
        let in_second = walk.holds();
        assert_eq!(in_first, in_second, "{name}: two rooms of a size hold the same");
        let (capacity, growths) = {
            let stats = walk.renderer.stats();
            (stats.mesh_pool_capacity, stats.buffer_growths)
        };
        println!(
            "{name}: {} cells of {} triangles and a {}px texture, {capacity} B of mesh pool: {in_first:?}",
            zone.side * zone.side,
            2 * zone.quads * zone.quads,
            zone.texture,
        );
        for lap in 0..6 {
            // Warm: the hub says the next is coming, and it's in before the trip.
            walk.scene.prefetch_exits(&mut walk.content, settings);
            for _ in 0..600 {
                walk.frame();
                if walk.scene.zone_load(room(lap % 2).zone) == ZoneLoad::Ready {
                    break;
                }
                std::thread::sleep(Duration::from_micros(500));
            }
            let warm = walk.trip(lap % 2, settings);
            let held = walk.holds();
            assert_eq!(held, in_first, "{name}, trip {lap}, warm");
            // Cold: nothing went ahead of it.
            let cold = walk.trip((lap + 1) % 2, settings);
            let held = walk.holds();
            assert_eq!(held, in_first, "{name}, trip {lap}, cold");
            let stats = walk.renderer.stats();
            assert_eq!(stats.mesh_pool_capacity, capacity, "{name}, trip {lap}: the pool grew");
            assert_eq!(stats.buffer_growths, growths, "{name}, trip {lap}: a buffer grew");
            println!(
                "{name}, round {lap}: warm {:?} over {} frames, cold {:?} over {} frames, worst upload {:?}",
                warm.load, warm.frames, cold.load, cold.frames, cold.worst_upload,
            );
            walk.trip(lap % 2, settings);
            assert_eq!(walk.holds(), in_first, "{name}, trip {lap}, back");
        }
    }
}
