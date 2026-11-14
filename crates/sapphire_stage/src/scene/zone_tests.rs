//! A synthetic zone through the whole stage, on a real renderer: what streaming does to the packet, what
//! a new renderer and a closing zone do to it, and what a walk costs. GPU tests, as the rest of the
//! stage's are.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurement's report")]

use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::material::{
    AnimeCharacterMaterial, Material, MaterialFamily, MaterialFlags, SkyMaterial,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::probes::{BOUNCE_COEFFICIENTS, BRICK_PROBES, ProbeData, ProbeGrid};
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
use sapphire_zone::{CookedExit, ZoneFog, ZonePresentation, ZoneWeather, ZoneWind};

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
        let budgets = AssetBudgets::UNLIMITED;
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

    /// As [`Self::rooms`], each room dressed by its `looks` entry (M5 M6), with no listed keys.
    fn dressed_rooms(zone: SyntheticZone, looks: &[RoomLook], budgets: AssetBudgets) -> Self {
        let rooms = looks.len() as u32;
        let source = Arc::new(MemoryPackSource::new());
        source.insert(BASE_PACK, write_pack(vec![character()]).expect("a pack"));
        for (room, look) in (0..rooms).zip(looks) {
            let next = (room + 1) % rooms;
            let exit = CookedExit {
                id: exit_to(next),
                name: format!("to_room_{next}"),
                to: SyntheticZone::room_id(next),
                pack: room_pack(next),
                arrive: 0,
                prefetch: true,
            };
            let (extra, dress) = look.assets(room);
            let pack = zone.dressed_room_pack(room, vec![exit], extra, dress).expect("a pack");
            source.insert(&room_pack(room), pack);
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
        self.frame_seeing(|_| {})
    }

    /// [`Self::frame`], showing `see` its packet before it's prepared.
    fn frame_seeing(&mut self, see: impl FnOnce(&FramePacket)) -> Frame {
        let before = self.renderer.stats();
        let stopwatch = Stopwatch::start();
        self.content.update(&mut self.renderer, &self.gpu);
        let uploads = stopwatch.elapsed();
        let grew = self.renderer.stats().buffer_growths != before.buffer_growths;
        self.scene.follow_content(&mut self.content, &mut ());
        self.scene.step();
        let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut ());
        see(packet);
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
    pool: PoolTrip,
}

/// What the mesh pool did over a trip's arrival (M5 D9).
#[derive(Clone, Copy, Debug)]
struct PoolTrip {
    /// Its capacity as the zone was ready to arrive in: the last zone's and what's come of the new one.
    before: u64,
    /// The most its buffers held from there to the arrival's end, those replaced and still read among them.
    peak: u64,
    /// Frames from arriving until [`Scene::zone_arrived`].
    frames: u32,
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
        let before = self.renderer.stats().mesh_pool_capacity;
        let mut trip = Trip {
            load: stopwatch.elapsed(),
            frames,
            worst_upload,
            pool: PoolTrip { before, peak: before, frames: 0 },
        };
        let arrived = self.scene.arrive_in_zone(room(to).zone, &mut self.content).expect("it's ready");
        if let Some(left) = arrived.left {
            let report = left.expect("the last zone's pack unmounts");
            assert_eq!(report.cancelled, 0, "{report:?}");
        }
        for _ in 0..1_000 {
            if self.scene.zone_arrived(&self.content) {
                break;
            }
            self.frame();
            let stats = self.renderer.stats();
            trip.pool.peak = trip.pool.peak.max(stats.mesh_pool_capacity + stats.mesh_pool.retired);
            trip.pool.frames += 1;
        }
        let pool = self.renderer.stats().mesh_pool;
        assert!(self.scene.zone_arrived(&self.content), "{:?}, {pool:?}", self.scene.stream_stats());
        self.scene.end_arrival(&mut self.content);
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
            mesh_pool_capacity: stats.mesh_pool_capacity,
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
    /// What the pool's buffers hold (M5 D9): a trip gives the last place's room back.
    mesh_pool_capacity: u64,
    materials: u32,
    material_bytes: u64,
    texture_bytes: u64,
    gpu_textures: usize,
}

/// What a room looks like (M5 M6): the hours its probe grid was baked under, if it has one, whether it names a
/// sky material and the look-dev day, and its air.
#[derive(Clone, Default)]
struct RoomLook {
    grid: Option<Vec<f32>>,
    sky: bool,
    day: bool,
    wind: Option<ZoneWind>,
    weather: Option<ZoneWeather>,
    fog: Option<ZoneFog>,
}

/// The look-dev day, which a room may name as its own.
const LOOKDEV_DAY: &str = include_str!("../../../../content/lookdev/lookdev_day.ron");

impl RoomLook {
    fn grid_id(room: u32) -> AssetId {
        SyntheticZone::room_id(room).sub_asset("probes")
    }

    fn sky_id(room: u32) -> AssetId {
        SyntheticZone::room_id(room).sub_asset("sky")
    }

    /// Room `room`'s assets for this look, and what its presentation says of it.
    fn assets(&self, room: u32) -> (Vec<PackAsset>, impl FnOnce(&mut ZonePresentation) + use<>) {
        let mut assets = Vec::new();
        if let Some(hours) = &self.grid {
            let bytes = probe_grid(hours).to_cooked().expect("a grid").into_vec();
            let grid = Self::grid_id(room);
            assets.push(
                PackAsset::encode(grid, AssetKind::ProbeGrid, Codec::Lz4, &bytes, Vec::new(), 3).unwrap(),
            );
        }
        if self.sky {
            let swizzles = [Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()];
            let sky = Material::from_document(&SkyMaterial::default(), &swizzles).expect("a sky material");
            let bytes = sky.to_cooked().expect("it cooks").into_vec();
            let id = Self::sky_id(room);
            assets
                .push(PackAsset::encode(id, AssetKind::Material, Codec::Lz4, &bytes, Vec::new(), 3).unwrap());
        }
        let look = self.clone();
        let dress = move |presentation: &mut ZonePresentation| {
            presentation.probes = look.grid.as_ref().map(|_| Self::grid_id(room));
            presentation.sky = look.sky.then(|| Self::sky_id(room));
            presentation.day = look.day.then(|| LOOKDEV_DAY.to_owned());
            presentation.wind = look.wind;
            presentation.weather = look.weather;
            presentation.fog = look.fog;
        };
        (assets, dress)
    }
}

/// A one-brick probe grid baked under `hours`, every probe open, its light all the same.
fn probe_grid(hours: &[f32]) -> ProbeGrid {
    let probes = BRICK_PROBES as usize;
    // 0.5 and about 0.1 as half floats.
    let (half, tenth) = (0x3800_u16, 0x2e66_u16);
    ProbeGrid {
        origin: [0.0; 3],
        spacing: 2.0,
        bricks: [1, 1, 1],
        brick_table: vec![0],
        brick_probes: (0..BRICK_PROBES).collect(),
        key_hours: hours.to_vec(),
        visibility_coefficients: 4,
        visibility: vec![half; probes * 4],
        bounce: vec![tenth; probes * hours.len() * BOUNCE_COEFFICIENTS as usize],
        probe_data: vec![ProbeData { free: [ProbeData::FULL; 6], valid: true }.to_bits(); probes],
    }
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
    AssetBudgets { resident: [1; AssetKind::ALL.len()], ..AssetBudgets::UNLIMITED }
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
        use sapphire_render::quality::{TierId, TierTable};
        let tiers = TierTable::shipped();
        let budgets = |id| Content::tier_budgets(tiers.get(id));
        vec![
            ("Desktop-Low", desktop, StreamSettings::DESKTOP, budgets(TierId::DesktopLow)),
            ("Mobile-Low", mobile, StreamSettings::MOBILE, budgets(TierId::MobileLow)),
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
    AssetBudgets::UNLIMITED
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

    // A trip takes what's already there. The room it arrives in leads back, so the last one's pack stays
    // mounted for it (M5 M6), though nobody's asked for the last zone again yet.
    let stopwatch = Stopwatch::start();
    walk.trip(1, settings);
    assert!(stopwatch.elapsed() < Duration::from_secs(5));
    let (server, _) = walk.content.streaming_parts();
    assert!(matches!(server.mount_state(&room_pack(0)), MountState::Mounted));
    assert!(walk.scene.zone_load(room(0).zone) == ZoneLoad::Waiting, "and nobody's asked for the last");
    // Once it's all in, as the first room was counted.
    walk.holds();
    assert_eq!(walk.drawing(), drawing);
}

/// A trip brings the look of the zone it arrives in (M5 M6): its probe grid on the renderer in place of the
/// last one's, or none, its day, its sky material and its air; and a zone that names none of them gives the
/// stage its own back. A zone coming next leaves the grid on the renderer the one on stage's until it's arrived
/// in. And a zone that doesn't lead back to the last one lets its pack go.
#[test]
#[ignore = "needs a GPU"]
fn a_trip_brings_the_look_of_the_zone_it_arrives_in() {
    let settings = small_settings();
    let looks = [
        RoomLook {
            grid: Some(vec![6.0]),
            sky: true,
            day: true,
            wind: Some(ZoneWind { direction: [0.0, 0.0, 1.0], strength: 2.5, gustiness: 0.1 }),
            weather: Some(ZoneWeather { wetness: 0.7, puddles: 0.3 }),
            ..RoomLook::default()
        },
        RoomLook { grid: Some(vec![12.0, 18.0]), ..RoomLook::default() },
        RoomLook { fog: Some(ZoneFog { density: 2.0, height_base: Some(1.5) }), ..RoomLook::default() },
    ];
    let mut walk = Walk::dressed_rooms(SMALL, &looks, no_limits());
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    let (stage_day, stage_wind) = (walk.scene.time_of_day().timeline().clone(), walk.scene.wind);
    let packet = |walk: &mut Walk| {
        // What the grid's change reaches is on the renderer from the frame after the arrival's.
        walk.frame();
        let mut seen = None;
        walk.frame_seeing(|packet| seen = Some(packet.clone()));
        seen.expect("a packet")
    };

    walk.trip(0, settings);
    let in_first = packet(&mut walk);
    let on_renderer = |walk: &Walk| {
        let ambient = walk.renderer.stats().ambient;
        ambient.grid.then_some(ambient.keys)
    };
    assert_eq!(walk.content.probe_hours(), [6.0], "its grid's on the renderer");
    assert_eq!(on_renderer(&walk), Some(1));
    assert_eq!(walk.content.zone_probe_grid().map(|grid| grid.key_hours.len()), Some(1));
    assert_eq!(walk.scene.time_of_day().timeline(), &lookdev::day(), "its day");
    assert!(in_first.sky_material.is_some(), "its sky");
    assert_eq!(in_first.sky_material, walk.content.zone_sky_material());
    assert_eq!((in_first.wind.strength, in_first.wind.direction), (2.5, Vec3::Z), "its wind");
    assert_eq!((in_first.weather.wetness, in_first.weather.puddles), (0.7, 0.3), "its weather");

    // The next one coming leaves the grid as it is.
    walk.scene.prefetch_exits(&mut walk.content, settings);
    let mut frames = 0;
    while walk.scene.zone_load(room(1).zone) != ZoneLoad::Ready {
        walk.frame();
        frames += 1;
        assert!(frames < 100_000, "it never came");
        std::thread::sleep(Duration::from_micros(100));
    }
    walk.frame();
    assert_eq!(walk.content.probe_hours(), [6.0], "the grid on stage stays until the next is arrived in");
    assert_eq!(on_renderer(&walk), Some(1));

    walk.trip(1, settings);
    let in_second = packet(&mut walk);
    assert_eq!(walk.content.probe_hours(), [12.0, 18.0], "the next zone's grid replaced it");
    assert_eq!(on_renderer(&walk), Some(2));
    assert_eq!(walk.scene.time_of_day().timeline(), &stage_day, "the stage's own day again");
    assert_eq!(in_second.sky_material, None, "and the app's sky, which this stage has none of");
    assert_eq!(walk.scene.wind, stage_wind);
    assert_eq!(in_second.weather.wetness, 0.0, "dry");
    let (server, _) = walk.content.streaming_parts();
    assert!(matches!(server.mount_state(&room_pack(0)), MountState::Unmounted), "room 1 doesn't lead back");

    walk.trip(2, settings);
    let in_third = packet(&mut walk);
    assert!(walk.content.probe_hours().is_empty(), "a zone with no grid takes the last one's off");
    assert!(walk.content.zone_probe_grid().is_none());
    // The renderer's own copy too, though the last zone's pack, and the grid with it, went at the arrival.
    assert_eq!(on_renderer(&walk), None);
    assert_eq!(in_third.fog.near.density, 2.0 * in_second.fog.near.density, "its fog, twice the day's");
    assert_eq!(in_third.fog.height.density, 2.0 * in_second.fog.height.density);
    assert_eq!(in_third.fog.height.base, 1.5, "on its floor");

    walk.trip(0, settings);
    let again = packet(&mut walk);
    assert_eq!(walk.content.probe_hours(), [6.0], "and back");
    assert_eq!(again.fog.near.density, in_first.fog.near.density);
    assert!(again.sky_material.is_some());
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

/// A hub's next zone warms its pipelines up off the frame (M4 H1), and asks for no mesh until they're taken
/// in: no frame builds one or waits for one.
#[test]
#[ignore = "needs a GPU"]
fn a_zone_coming_next_warms_up_off_the_frame() {
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
    let before = walk.renderer.stats().pipelines;
    assert_eq!(before.hitches(), 0, "{}", walk.renderer.first_use_report());
    walk.scene.prefetch_zone(room(0), settings);
    let mut frames = 0;
    while walk.scene.zone_load(room(0).zone) != ZoneLoad::Ready {
        walk.frame();
        frames += 1;
        assert!(frames < 100_000, "it never came");
        std::thread::sleep(Duration::from_micros(100));
    }
    let warmed = walk.renderer.stats().pipelines;
    assert!(warmed.prewarmed > before.prewarmed, "its keys were built");
    assert_eq!(
        warmed.warmed_off_frame - before.warmed_off_frame,
        warmed.prewarmed - before.prewarmed,
        "each on the warm-up's threads"
    );
    assert_eq!(walk.renderer.stats().warm_up.answered, walk.renderer.stats().warm_up.requests);
    walk.trip(0, settings);
    walk.settle();
    let stats = walk.renderer.stats().pipelines;
    assert_eq!(
        stats.first_use_builds, before.first_use_builds,
        "nothing came before its pipelines: {stats:?}"
    );
    // The trip's own arrival may wait on a load screen (`Content::open_zone`), but builds nothing on a frame.
    assert_eq!(stats.frame_thread_builds, 0, "{}", walk.renderer.first_use_report());
}

/// Trip after trip between two zones, a hub and a room, each with a probe grid it brings (M5 M6), on each class
/// of device's settings and budgets: after the first round, each visit to a room holds the same as the visit
/// before it, on the server and the renderer, its mesh pool's capacity among it (M5 D9), the other's pack kept
/// mounted as the room leads back to it, and no buffer but the mesh pool's grows. `--nocapture` in release
/// reports it at a size worth reporting; a debug build, as CI's, goes between small ones and holds the same.
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
    // Grids of a size, as the rooms are.
    let looks = [
        RoomLook { grid: Some(vec![6.0, 18.0]), ..RoomLook::default() },
        RoomLook { grid: Some(vec![12.0, 20.0]), ..RoomLook::default() },
    ];
    for (name, zone, settings, budgets) in tiers {
        let mut walk = Walk::dressed_rooms(zone, &looks, budgets);
        walk.look_from(Vec3::new(32.0, 0.0, 32.0));
        walk.trip(0, settings);
        let in_first = walk.holds();
        let one_room = walk.renderer.stats().mesh_pool_capacity;
        println!(
            "{name}: {} cells of {} triangles and a {}px texture, {one_room} B of mesh pool: {in_first:?}",
            zone.side * zone.side,
            2 * zone.quads * zone.quads,
            zone.texture,
        );
        // A hub holds where it is and where it leads at once, so the first round may grow buffers to two rooms'
        // worth; no round after it grows anything but the mesh pool, which gave the last room back.
        let mut after_first: Option<(u64, u32)> = None;
        // What each room held on the round before, from the second on.
        let mut visits: [Option<TripHolds>; 2] = [None, None];
        let mut holds_as_before = |walk: &mut Walk, room: u32, lap: u32, how: &str| {
            let held = walk.holds();
            assert_eq!(walk.content.probe_hours(), looks[room as usize].grid.as_deref().unwrap_or_default());
            if lap > 0 {
                let before = visits[room as usize].get_or_insert(held);
                assert_eq!(held, *before, "{name}, trip {lap}, {how}");
            }
        };
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
            holds_as_before(&mut walk, lap % 2, lap, "warm");
            // Cold: nothing went ahead of it.
            let cold = walk.trip((lap + 1) % 2, settings);
            holds_as_before(&mut walk, (lap + 1) % 2, lap, "cold");
            println!(
                "{name}, round {lap}: warm {:?} over {} frames, cold {:?} over {} frames, worst upload {:?}",
                warm.load, warm.frames, cold.load, cold.frames, cold.worst_upload,
            );
            walk.trip(lap % 2, settings);
            holds_as_before(&mut walk, lap % 2, lap, "back");
            // The pool gives each room's room back as it's left (M5 D9), so it grows again on each arrival, to
            // what that room held the time before: `holds_as_before` counts its capacity. Nothing else grows.
            let stats = walk.renderer.stats();
            let now = (stats.mesh_pool_capacity, stats.buffer_growths - stats.mesh_pool.growths);
            match after_first {
                None => {
                    println!("{name}: {} B of mesh pool after the first round", now.0);
                    after_first = Some(now);
                }
                Some((capacity, growths)) => {
                    assert_eq!(now.0, capacity, "{name}, trip {lap}: the pool grew");
                    assert_eq!(now.1, growths, "{name}, trip {lap}: a buffer grew");
                }
            }
        }
    }
}

/// A trip gives the mesh pool's room back under its wipe (M5 D9): around three rooms, each leading only to the
/// next, so a room left isn't kept coming (as the town isn't, and unlike a hub's way back, M5 M6), and on each
/// arrival the pool comes down to its floor for the room arrived in and the character, the same on each visit to
/// a room, below what it held with the room left in it. Its old buffers and its new are both alive until the fence
/// lets the old go, and never more: the peak is what it held before and its floor after.
#[test]
#[ignore = "needs a GPU"]
fn a_trip_leaves_the_mesh_pool_at_its_floor() {
    let zone = SyntheticZone { side: 4, cell_size: 32.0, instances: 2, lights: 1, quads: 64, texture: 0 };
    let settings = small_settings();
    let mut walk = Walk::rooms(zone, 3, no_limits(), Vec::new());
    walk.look_from(Vec3::new(16.0, 0.0, 16.0));
    let mib = |bytes: u64| bytes as f64 / 1_048_576.0;
    let mut floors: [Option<u64>; 3] = [None; 3];
    for (visit, to) in [0, 1, 2, 0, 1].into_iter().enumerate() {
        let compactions = walk.renderer.stats().mesh_pool.compactions;
        let trip = walk.trip(to, settings);
        let stats = walk.renderer.stats();
        let pool = stats.mesh_pool;
        println!(
            "trip {visit} to room {to}: {:.2} MiB of mesh pool before arriving, {:.2} after over {} frames, {} parts \
             compacted, a peak of {:.2}",
            mib(trip.pool.before),
            mib(stats.mesh_pool_capacity),
            trip.pool.frames,
            pool.compactions - compactions,
            mib(trip.pool.peak),
        );
        assert!(!pool.compacting, "trip {visit}: {pool:?}");
        assert_eq!(stats.mesh_pool_capacity, pool.floor, "trip {visit}: at the floor for what it holds");
        let floor = *floors[to as usize].get_or_insert(pool.floor);
        assert_eq!(pool.floor, floor, "trip {visit}: room {to}'s floor, as on its first visit");
        assert!(trip.pool.peak <= trip.pool.before + pool.floor, "trip {visit}: {trip:?}");
        if visit > 0 {
            assert!(stats.mesh_pool_capacity < trip.pool.before, "trip {visit}: the last room's given back");
            assert!(pool.compactions > compactions, "trip {visit}: it compacted");
        }
        // What the arrival let go of goes back to the device once the fence lets it.
        walk.drain();
        assert_eq!(walk.renderer.stats().mesh_pool.retired, 0);
    }
}
