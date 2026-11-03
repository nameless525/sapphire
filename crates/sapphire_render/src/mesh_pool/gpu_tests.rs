//! Giving meshes and materials back (M3 W3), on a real device, with the fence held where a slow GPU
//! would have it: room a frame may be reading is never given to an upload, and a handle that's been
//! freed, or was another renderer's, names nothing. They need a GPU: run them with
//! `cargo test -p sapphire_render --lib -- --ignored`.

use glam::{Mat4, Vec3};
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, FoliageMaterial, Material,
    Terminator,
};
use sapphire_assets::mesh::{
    FoliageVertex, Influences, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin, SkinBind,
    Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags, VertexInfluence,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::Placement;
use crate::graph::{FrameGraph, GraphExecutor};
use crate::{
    AmbientLight, CameraData, DirectionalLight, FramePacketRing, InstanceId, MeshHandle, MeshInstance,
    RenderError, RenderSettings, RenderStats, Renderer, SkyGradient, ViewData,
};
use sapphire_assets::mesh::quantize_weights;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIDE: u32 = 16;
const RED: Vec3 = Vec3::new(1.0, 0.0, 0.0);
const GREEN: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const BLUE: Vec3 = Vec3::new(0.0, 0.0, 1.0);

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// A flat material of `color`, glowing so that it reads with no lights.
fn flat(color: Vec3) -> Material {
    let material = EnvironmentMaterial {
        base_color: color.extend(1.0).to_array(),
        alpha: AlphaMode::Opaque,
        specular_clamp: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        emission: color.to_array(),
        ..EnvironmentMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&material, &swizzles).expect("a valid material")
}

/// A square facing the camera, filling the frame, in `quads * quads` pieces, with a material of its own.
fn square(color: Vec3, quads: u32) -> Mesh {
    let row = quads + 1;
    let vertices: Vec<Vertex> = (0..row * row)
        .map(|n| {
            let (u, v) = ((n % row) as f32 / quads as f32, (n / row) as f32 / quads as f32);
            Vertex {
                position: [u * 4.0 - 2.0, v * 4.0 - 2.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                outline_normal: [0.0, 0.0, 1.0],
                uv: [u, v],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            }
        })
        .collect();
    let mut indices = Vec::new();
    for z in 0..quads {
        for x in 0..quads {
            let at = z * row + x;
            indices.extend_from_slice(&[at, at + 1, at + row, at + 1, at + row + 1, at + row]);
        }
    }
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("flat", flat(color))];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

fn upload(renderer: &mut Renderer, color: Vec3, quads: u32) -> MeshHandle {
    let bytes = square(color, quads).to_cooked().expect("it cooks");
    renderer.upload_mesh_with_slots(Mesh::read(&bytes).expect("a valid mesh"), |_| None).expect("it uploads")
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.1 > 0 && b.1 > 0 && a.0 < b.0 + b.1 && b.0 < a.0 + a.1
}

fn disjoint(a: Placement, b: Placement) -> bool {
    !overlaps(a.vertices, b.vertices) && !overlaps(a.indices, b.indices)
}

/// A renderer and a target, run frame by frame.
struct Bench {
    // Drop order: what lives on the GPU before the device.
    packets: FramePacketRing,
    executor: GraphExecutor,
    renderer: Renderer,
    target: wgpu::Texture,
    gpu: GpuContext,
}

impl Bench {
    fn new() -> Self {
        let gpu = gpu();
        let renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { packets: FramePacketRing::new(), executor: GraphExecutor::new(), renderer, target, gpu }
    }

    fn upload(&mut self, color: Vec3, quads: u32) -> MeshHandle {
        upload(&mut self.renderer, color, quads)
    }

    fn placement(&self, mesh: MeshHandle) -> Placement {
        self.renderer.mesh_placement(mesh)
    }

    /// Prepares a frame that adds or moves `changed` and removes `removed`, by instance number.
    fn prepare(&mut self, changed: &[(u64, MeshHandle)], removed: &[u64]) {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(CameraData::look_at(
            Vec3::new(0.0, 0.0, 1.5),
            Vec3::ZERO,
            Vec3::Y,
            0.8,
            1.0,
            0.05,
        )));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.0 };
        let sky = Vec3::new(0.0, 0.0, 0.0);
        packet.sky = SkyGradient { zenith: sky, horizon: sky, ground: sky };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.shadow_tint = Vec3::ONE;
        packet.instances.changed.extend(changed.iter().map(|&(id, mesh)| MeshInstance {
            id: InstanceId(id),
            mesh,
            transform: Mat4::IDENTITY,
        }));
        packet.instances.removed.extend(removed.iter().map(|&id| InstanceId(id)));
        self.renderer.prepare(&self.gpu, self.packets.current());
    }

    /// Records and submits what was prepared.
    fn submit(&mut self) {
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let output = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, output).expect("a view and a target");
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.executor.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }

    fn frame(&mut self, changed: &[(u64, MeshHandle)], removed: &[u64]) {
        self.prepare(changed, removed);
        self.submit();
    }

    /// Waits for the GPU, so that the fence, held or not, knows every frame so far is finished.
    fn wait(&self) {
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    }

    /// Frames until nothing's left retiring.
    fn settle(&mut self) {
        for _ in 0..16 {
            if self.stats().retiring == 0 {
                return;
            }
            self.wait();
            self.frame(&[], &[]);
        }
        panic!("it never settled: {:?}", self.stats().retiring);
    }

    fn stats(&self) -> RenderStats {
        self.renderer.stats()
    }

    /// The middle pixel of what was last submitted.
    fn pixel(&self) -> [u8; 4] {
        let padded = (SIDE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIDE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIDE),
                },
            },
            self.target.size(),
        );
        self.gpu.queue().submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = buffer.get_mapped_range(..).expect("mapped");
        let at = (padded * (SIDE / 2) + SIDE / 2 * 4) as usize;
        [mapped[at], mapped[at + 1], mapped[at + 2], mapped[at + 3]]
    }
}

/// Which of red, green and blue a pixel mostly is, or `None` for the black of nothing drawn.
fn color(pixel: [u8; 4]) -> Option<Vec3> {
    let [r, g, b, _] = pixel;
    if r.max(g).max(b) < 64 {
        return None;
    }
    Some(if r > g && r > b {
        RED
    } else if g > b {
        GREEN
    } else {
        BLUE
    })
}

#[test]
#[ignore = "needs a GPU"]
fn a_freed_meshs_room_stays_out_of_reach_until_the_frame_that_read_it_is_finished() {
    let mut bench = Bench::new();
    let red = bench.upload(RED, 2);
    bench.frame(&[(1, red)], &[]);
    assert_eq!(color(bench.pixel()), Some(RED));
    let room = bench.placement(red);

    // A GPU that hasn't finished the frame that drew it, however many frames go by.
    bench.renderer.hold_frames_at(Some(0));
    bench.renderer.free_mesh(red);
    bench.frame(&[], &[1]);
    for _ in 0..4 {
        let other = bench.upload(GREEN, 2);
        assert!(disjoint(bench.placement(other), room), "the frame that drew it may be reading that");
        bench.frame(&[], &[]);
        bench.wait();
    }
    assert!(bench.stats().retiring > 0);

    // Finished: the next upload that fits gets it.
    bench.renderer.hold_frames_at(None);
    bench.settle();
    let next = bench.upload(BLUE, 2);
    assert_eq!(bench.placement(next).vertices, room.vertices);
    assert_eq!(bench.placement(next).indices, room.indices);
}

/// A mesh freed and another uploaded after a frame is prepared and before it's submitted: that frame
/// reads what it prepared, as queue writes land before it's submitted.
#[test]
#[ignore = "needs a GPU"]
fn a_frame_already_prepared_draws_what_it_prepared_whatever_is_freed_before_its_submitted() {
    let mut bench = Bench::new();
    let red = bench.upload(RED, 2);
    bench.frame(&[(1, red)], &[]);
    let room = bench.placement(red);

    bench.prepare(&[], &[]);
    bench.renderer.free_mesh(red);
    let green = bench.upload(GREEN, 2);
    assert!(disjoint(bench.placement(green), room));
    bench.submit();
    assert_eq!(color(bench.pixel()), Some(RED));

    // And the frame after it draws green over the same instance, with red on its way out.
    bench.frame(&[(1, green)], &[]);
    assert_eq!(color(bench.pixel()), Some(GREEN));
    bench.settle();
    assert_eq!(bench.stats().meshes, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_mesh_an_instance_draws_stays_until_the_instance_goes() {
    let mut bench = Bench::new();
    let red = bench.upload(RED, 2);
    bench.frame(&[(1, red)], &[]);
    let bytes = bench.stats().mesh_pool_bytes;
    bench.renderer.free_mesh(red);
    for _ in 0..8 {
        bench.frame(&[], &[]);
        bench.wait();
        assert_eq!(color(bench.pixel()), Some(RED), "it draws as it did");
        assert_eq!((bench.stats().meshes, bench.stats().mesh_pool_bytes), (1, bytes));
    }
    // Moved, it's still that mesh.
    bench.frame(&[(1, red)], &[]);
    assert_eq!(color(bench.pixel()), Some(RED));
    let materials = bench.stats().materials;

    bench.frame(&[], &[1]);
    assert_eq!(color(bench.pixel()), None);
    bench.settle();
    let stats = bench.stats();
    assert_eq!((stats.meshes, stats.mesh_pool_bytes, stats.meshes_freed), (0, 0, 1));
    assert_eq!(stats.materials, materials - 1, "the material it embedded went with it");
}

#[test]
#[ignore = "needs a GPU"]
fn a_freed_handle_names_nothing_even_when_its_index_comes_back() {
    let mut bench = Bench::new();
    let red = bench.upload(RED, 2);
    bench.frame(&[(1, red)], &[]);
    bench.renderer.free_mesh(red);
    bench.renderer.free_mesh(red);
    bench.frame(&[], &[1]);
    bench.settle();
    assert_eq!(bench.stats().meshes_freed, 1, "freed once, for all it was asked twice");

    let green = bench.upload(GREEN, 2);
    assert_eq!(green.index(), red.index(), "its record is reused");
    assert_ne!(green, red);

    // A stale instance draws nothing, not what came to hold the index.
    bench.frame(&[(2, red)], &[]);
    bench.frame(&[], &[]);
    assert_eq!(color(bench.pixel()), None);
    // Freeing and replacing through it touch nothing.
    bench.renderer.free_mesh(red);
    assert_eq!(bench.stats().retiring, 0);
    let bytes = square(BLUE, 2).to_cooked().expect("it cooks");
    let mesh = Mesh::read(&bytes).expect("a valid mesh");
    assert!(matches!(bench.renderer.update_mesh(red, mesh, |_| None), Err(RenderError::UnknownMesh(_))));

    // The instance given the live handle draws again.
    bench.frame(&[(2, green)], &[]);
    assert_eq!(color(bench.pixel()), Some(GREEN));
}

/// A lost device answers nothing, so what's retired can't wait for it, and what a new renderer on a new
/// device starts with is nothing of the old one's.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_device_leaves_nothing_waiting_and_nothing_for_the_next_renderer_to_trip_on() {
    let mut bench = Bench::new();
    let red = bench.upload(RED, 2);
    let blue = bench.upload(BLUE, 2);
    bench.frame(&[(1, red), (2, blue)], &[]);
    bench.renderer.free_mesh(red);
    bench.gpu.device().destroy();
    let _ = bench.gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(bench.gpu.is_lost());

    // Frames on a lost device run through `prepare` and get no further.
    bench.prepare(&[], &[1]);
    bench.prepare(&[], &[]);
    let stats = bench.stats();
    assert_eq!((stats.retiring, stats.meshes, stats.meshes_freed), (0, 1, 1));

    // The app's next renderer, on a new device, with the old handles still about.
    let mut next = Bench::new();
    let green = next.upload(GREEN, 2);
    next.renderer.free_mesh(red);
    next.renderer.free_mesh(blue);
    assert_eq!(next.stats().retiring, 0, "they're the old renderer's");
    assert_eq!(next.stats().meshes, 1);
    next.frame(&[(1, blue)], &[]);
    next.frame(&[], &[]);
    assert_eq!(color(next.pixel()), None, "an old renderer's handle draws nothing");
    next.frame(&[(1, green)], &[]);
    assert_eq!(color(next.pixel()), Some(GREEN));
    next.renderer.free_mesh(green);
    next.frame(&[], &[1]);
    next.settle();
    assert_eq!(next.stats().meshes, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_slot_a_mesh_still_draws_with_stays_and_a_freed_one_goes_to_the_next_upload() {
    let mut bench = Bench::new();
    let cooked = flat(RED).to_cooked().expect("it cooks");
    let shared = Material::read(&cooked).expect("a valid material");
    let handle = bench.renderer.upload_material(shared, |_| None).expect("it uploads");
    let row = 2;
    let vertices: Vec<Vertex> = (0..row * row)
        .map(|n| Vertex {
            position: [(n % row) as f32 * 4.0 - 2.0, (n / row) as f32 * 4.0 - 2.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        })
        .collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("placeholder", flat(GREEN))];
    let mesh =
        Mesh::from_vertices(&vertices, vec![0, 1, 2, 1, 3, 2], submeshes, slots, PositionFormat::Unorm16)
            .to_cooked()
            .expect("it cooks");
    let mesh = Mesh::read(&mesh).expect("a valid mesh");
    let drawn = bench.renderer.upload_mesh(mesh, &[handle]).expect("it uploads");
    bench.frame(&[(1, drawn)], &[]);
    assert_eq!(color(bench.pixel()), Some(RED));
    let materials = bench.stats().materials;
    let bytes = bench.stats().material_bytes;

    bench.renderer.free_material(handle);
    for _ in 0..4 {
        bench.frame(&[], &[]);
        bench.wait();
        assert_eq!(color(bench.pixel()), Some(RED), "the mesh still draws with it");
        assert_eq!(bench.stats().materials, materials);
    }

    bench.renderer.free_mesh(drawn);
    bench.frame(&[], &[1]);
    bench.settle();
    assert_eq!(bench.stats().materials, materials - 1);
    assert!(bench.stats().material_bytes < bytes);

    // Its slot is taken by the next of its family, and a stale handle is refused.
    let other = bench.renderer.upload_material(shared, |_| None).expect("it uploads");
    assert_eq!(other.index(), handle.index());
    assert_ne!(other, handle);
    assert_eq!(bench.stats().material_bytes, bytes);
    assert!(matches!(
        bench.renderer.update_material(handle, shared, |_| None),
        Err(RenderError::UnknownMaterial(_))
    ));
}

#[test]
#[ignore = "needs a GPU"]
fn a_failed_upload_takes_back_a_slot_it_reused_as_well_as_one_it_grew() {
    let mut bench = Bench::new();
    let cooked = flat(RED).to_cooked().expect("it cooks");
    let material = Material::read(&cooked).expect("a valid material");
    let before = bench.stats();
    let first = bench.renderer.upload_material(material, |_| None).expect("it uploads");
    assert!(bench.stats().material_bytes > before.material_bytes);
    bench.renderer.free_material(first);
    bench.frame(&[], &[]);
    bench.settle();
    assert_eq!(bench.stats().material_bytes, before.material_bytes);

    // Two environment slots, one from the free list and one new, for a mesh that then fails: foliage on
    // a mesh with no foliage stream.
    let leaves = Material::from_document(
        &FoliageMaterial::default(),
        &[Swizzle::IDENTITY; FoliageMaterial::TEXTURE_SLOTS.len()],
    )
    .expect("a valid material");
    let corner = Vertex {
        position: [0.0; 3],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let submeshes = vec![
        Submesh { first_index: 0, index_count: 3, material: 0 },
        Submesh { first_index: 3, index_count: 3, material: 1 },
        Submesh { first_index: 6, index_count: 3, material: 2 },
    ];
    let slots = vec![
        MaterialSlot::embedded("a", flat(RED)),
        MaterialSlot::embedded("b", flat(GREEN)),
        MaterialSlot::embedded("leaves", leaves),
    ];
    let indices = vec![0, 1, 2, 0, 1, 2, 0, 1, 2];
    let bytes = Mesh::from_vertices(&[corner; 3], indices, submeshes, slots, PositionFormat::Unorm16)
        .to_cooked()
        .expect("it cooks");
    for _ in 0..3 {
        let error =
            bench.renderer.upload_mesh_with_slots(Mesh::read(&bytes).expect("a valid mesh"), |_| None);
        assert!(matches!(error, Err(RenderError::NoFoliageStream)), "{error:?}");
        assert_eq!(bench.stats().materials, before.materials);
        assert_eq!(bench.stats().material_bytes, before.material_bytes);
    }

    // What it took back is there for what comes next, and works.
    let red = bench.upload(RED, 2);
    bench.frame(&[(1, red)], &[]);
    assert_eq!(color(bench.pixel()), Some(RED));
}

/// A skinned and morphed mesh lives in the deformable buffer and a plant has a foliage stream: each comes
/// back to where it was taken from.
#[test]
#[ignore = "needs a GPU"]
fn a_deformed_mesh_and_a_plant_come_back_like_any_other() {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let skin = || {
        let character =
            Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
        let mut mesh = square(RED, 2);
        mesh.materials = vec![MaterialSlot::embedded("skin", character)];
        let count = mesh.streams.len() as u32;
        let influence =
            VertexInfluence { binds: [0, 0, 0, 0], weights: quantize_weights([1.0, 0.0, 0.0, 0.0]) };
        mesh.skin = Some(Skin {
            skeleton: None,
            joint_count: 1,
            binds: vec![SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() }],
            influences: Influences::pack(&vec![influence; count as usize], 1),
        });
        mesh.morph_targets = vec![MorphTarget {
            name: "shift".into(),
            default_weight: 0.0,
            first_delta: 0,
            delta_count: count,
        }];
        mesh.morph_deltas = (0..count).map(|v| MorphDelta::new(v, [0.0; 3], [0.0; 3])).collect();
        mesh.to_cooked().expect("it cooks")
    };
    let plant = || {
        let leaves = Material::from_document(
            &FoliageMaterial::default(),
            &[Swizzle::IDENTITY; FoliageMaterial::TEXTURE_SLOTS.len()],
        )
        .expect("valid");
        let mut mesh = square(GREEN, 2);
        mesh.materials = vec![MaterialSlot::embedded("leaves", leaves)];
        let still = FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        let count = mesh.streams.len();
        mesh.streams = mesh.streams.clone().with_foliage(&vec![FoliageVertex::decode(still.encode()); count]);
        mesh.to_cooked().expect("it cooks")
    };
    let (skin, plant) = (skin(), plant());
    let mut bench = Bench::new();
    let both = |bench: &mut Bench| {
        let skinned = bench.renderer.upload_mesh_with_slots(Mesh::read(&skin).unwrap(), |_| None).unwrap();
        let plant = bench.renderer.upload_mesh_with_slots(Mesh::read(&plant).unwrap(), |_| None).unwrap();
        (skinned, plant)
    };
    let (skinned, leaves) = both(&mut bench);
    let (at_skinned, at_leaves) = (bench.placement(skinned), bench.placement(leaves));
    assert!(at_skinned.vertices.1 > 0 && at_leaves.foliage.1 > 0, "{at_skinned:?} {at_leaves:?}");
    bench.frame(&[(1, skinned), (2, leaves)], &[]);
    assert!(bench.stats().draws >= 2);

    // Each round comes back to the same room, with nothing growing once it's been round.
    let (mut meshes, mut growths) = ((skinned, leaves), None);
    for _ in 0..4 {
        bench.renderer.free_mesh(meshes.0);
        bench.renderer.free_mesh(meshes.1);
        bench.frame(&[], &[1, 2]);
        bench.settle();
        assert_eq!((bench.stats().meshes, bench.stats().mesh_pool_bytes), (0, 0));

        meshes = both(&mut bench);
        assert_eq!(bench.placement(meshes.0), at_skinned);
        assert_eq!(bench.placement(meshes.1), at_leaves);
        bench.frame(&[(1, meshes.0), (2, meshes.1)], &[]);
        let grown = (bench.stats().buffer_growths, bench.stats().mesh_pool_capacity);
        assert_eq!(*growths.get_or_insert(grown), grown);
    }
}

/// Whatever order things come and go in, no upload is ever given room a frame the fence hasn't let go
/// of drew from.
#[test]
#[ignore = "needs a GPU"]
fn any_order_of_uploads_frees_and_frames_gives_no_upload_what_an_unfinished_frame_reads() {
    /// A small generator: what the steps are needn't be good, only the same each run.
    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }
    let mut bench = Bench::new();
    let mut state = 0x5ca1_ab1e_u64;
    // Each mesh with its instance number, while it has one.
    let mut meshes: Vec<(MeshHandle, Placement, Option<u64>)> = Vec::new();
    // What each frame's draw list named, by its frame number.
    let mut frames: Vec<(u64, Vec<Placement>)> = Vec::new();
    let mut numbers = 0;
    let mut frame = 0;
    let mut finished = 0;
    for _ in 0..600 {
        match next(&mut state) % 4 {
            0 => {
                let quads = 1 + (next(&mut state) % 6) as u32;
                let mesh = bench.upload(RED, quads);
                let placement = bench.placement(mesh);
                for (read_by, placements) in frames.iter().filter(|(read_by, _)| *read_by > finished) {
                    for read in placements {
                        assert!(disjoint(placement, *read), "frame {read_by}, unfinished, reads {read:?}");
                    }
                }
                let number = (next(&mut state).is_multiple_of(2)).then(|| {
                    numbers += 1;
                    numbers
                });
                meshes.push((mesh, placement, number));
            }
            1 if !meshes.is_empty() => {
                let at = next(&mut state) as usize % meshes.len();
                let (mesh, _, number) = meshes.swap_remove(at);
                bench.renderer.free_mesh(mesh);
                // Its instance goes now, or next frame's other instances go on without it.
                frame += 1;
                let removed: Vec<u64> = number.into_iter().collect();
                let changed: Vec<_> = meshes
                    .iter()
                    .filter_map(|(mesh, _, number)| number.map(|number| (number, *mesh)))
                    .collect();
                bench.renderer.hold_frames_at(Some(finished));
                bench.frame(&changed, &removed);
                frames.push((frame, meshes.iter().filter(|m| m.2.is_some()).map(|m| m.1).collect()));
            }
            _ => {
                // The GPU gets somewhere, or doesn't.
                bench.wait();
                if next(&mut state).is_multiple_of(3) {
                    finished = frame;
                }
                frame += 1;
                let changed: Vec<_> = meshes
                    .iter()
                    .filter_map(|(mesh, _, number)| number.map(|number| (number, *mesh)))
                    .collect();
                bench.renderer.hold_frames_at(Some(finished));
                bench.frame(&changed, &[]);
                frames.push((frame, meshes.iter().filter(|m| m.2.is_some()).map(|m| m.1).collect()));
            }
        }
        frames.retain(|(read_by, _)| *read_by > finished);
    }
    assert!(bench.stats().meshes_freed > 50, "it hardly tried: {}", bench.stats().meshes_freed);
    bench.renderer.hold_frames_at(None);
    for (mesh, _, _) in meshes.drain(..) {
        bench.renderer.free_mesh(mesh);
    }
    let live: Vec<u64> = (1..=numbers).collect();
    bench.frame(&[], &live);
    bench.settle();
    assert_eq!(bench.stats().mesh_pool_bytes, 0, "everything came back");
}
