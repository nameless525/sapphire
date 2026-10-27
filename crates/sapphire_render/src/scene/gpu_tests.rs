//! Full buffers (review RA-004): instances, draw records and per-object light lists past their
//! buffers' limits are left out, counted, and the frame draws everything else as it would.
//! The limits are made small for the tests (`Renderer::limit_buffers`). They need a GPU: run
//! them with `cargo test -p sapphire_render --lib -- --ignored`, in debug and release builds.

use glam::{Mat4, Vec3};
use sapphire_assets::material::{AlphaMode, DistanceStylization, EnvironmentMaterial, Material, Terminator};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use crate::graph::{FrameGraph, GraphExecutor};
use crate::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    LightKind, LightPath, LocalLight, MeshHandle, MeshInstance, PostParams, RenderSettings, RenderStats,
    Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 64;
/// Quads per row of the grid the instances stand in.
const COLUMNS: u32 = 4;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// Post-processing that shows linear values below about 0.98 as they are.
fn passthrough() -> PostParams {
    PostParams {
        exposure: 0.0,
        bloom: BloomParams { intensity: 0.0, ..BloomParams::default() },
        tone_curve: ToneCurve {
            linear_start: 0.001,
            linear_length: 0.99,
            black_tightness: 1.0,
            ..ToneCurve::default()
        },
        grade_a: ColorGrade::NEUTRAL,
        grade_b: ColorGrade::NEUTRAL,
        grade_blend: 0.0,
        filter_strength: 1.0,
        vignette: Vignette { intensity: 0.0, ..Vignette::default() },
        ..PostParams::default()
    }
}

/// A lit environment material of `color` that glows a little, so every quad shows whatever
/// lights it.
fn environment(color: Vec3) -> Material {
    let material = EnvironmentMaterial {
        base_color: color.extend(1.0).to_array(),
        alpha: AlphaMode::Opaque,
        specular_clamp: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        emission: (color * 0.2).to_array(),
        ..EnvironmentMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(&material, &swizzles).expect("a valid material")
}

/// A 0.2 m square facing +z at the origin, its two triangles one submesh each when `split`, in
/// two colors.
fn quad(split: bool) -> Mesh {
    let corner = |x: f32, y: f32| Vertex {
        position: [x * 0.1, y * 0.1, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x, y],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
    let submeshes = if split {
        vec![
            Submesh { first_index: 0, index_count: 3, material: 0 },
            Submesh { first_index: 3, index_count: 3, material: 1 },
        ]
    } else {
        vec![Submesh { first_index: 0, index_count: 6, material: 0 }]
    };
    let slots = vec![
        MaterialSlot::embedded("warm", environment(Vec3::new(0.8, 0.5, 0.2))),
        MaterialSlot::embedded("cool", environment(Vec3::new(0.2, 0.5, 0.8))),
    ];
    Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Unorm16)
}

/// The instances: `count` quads in a grid, in slot order.
fn instances(mesh: MeshHandle, count: u32) -> Vec<MeshInstance> {
    (0..count)
        .map(|index| {
            let (column, row) = (index % COLUMNS, index / COLUMNS);
            let position = Vec3::new(column as f32 * 0.25 - 0.375, row as f32 * 0.25 - 0.375, 0.0);
            MeshInstance {
                id: InstanceId(u64::from(index)),
                mesh,
                transform: Mat4::from_translation(position),
            }
        })
        .collect()
}

/// Buffer limits for a scene: instances, draw records and per-object light lists.
#[derive(Clone, Copy, Debug)]
struct Limits {
    instances: u64,
    draws: u64,
    light_lists: u64,
}

/// A frame of `count` quads (split into two submeshes each where `split`) under `limits`,
/// lit by a point light on the per-object path: its pixels and the renderer's stats. Fails on
/// any validation error.
fn render(count: u32, split: bool, limits: Option<Limits>) -> (Vec<u8>, RenderStats) {
    let gpu = gpu();
    let settings =
        RenderSettings { light_path: LightPath::PerObject, max_lights: 8, ..RenderSettings::default() };
    let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
    if let Some(limits) = limits {
        renderer.limit_buffers(limits.instances, limits.draws, Some(limits.light_lists), None);
    }
    let mesh = quad(split).to_cooked().expect("cook");
    let mesh = Mesh::read(&mesh).expect("a valid mesh");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");

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
    let view = target.create_view(&Default::default());
    let mut executor = GraphExecutor::new();
    let mut packets = FramePacketRing::new();
    let mut added = instances(mesh, count);
    // Two frames: the second draws what the first uploaded and chose, as every later one would.
    for _ in 0..2 {
        let packet = packets.begin();
        packet.views.push(ViewData::new(CameraData::look_at(
            Vec3::new(0.0, 0.0, 1.5),
            Vec3::ZERO,
            Vec3::Y,
            0.8,
            1.0,
            0.05,
        )));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.5 };
        packet.lights.push(LocalLight {
            position: Vec3::new(0.2, 0.1, 0.3),
            color: Vec3::ONE,
            intensity: 0.5,
            range: 1.0,
            kind: LightKind::Point,
        });
        let sky = Vec3::new(0.05, 0.05, 0.1);
        packet.sky = SkyGradient { zenith: sky, horizon: sky, ground: sky };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.shadow_tint = Vec3::ONE;
        packet.post = passthrough();
        packet.instances.changed.append(&mut added);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.prepare(&gpu, packets.current());
        let mut graph = FrameGraph::new();
        let output = graph.import_texture("target", &view);
        renderer.record(&mut graph, output).expect("a view and a target");
        executor.execute(&gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }
    (read(&gpu, &target), renderer.stats())
}

/// `texture`'s texels, row by row.
fn read(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    let padded = (SIDE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIDE),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(SIDE),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped.chunks(padded as usize).flat_map(|row| &row[..(SIDE * 4) as usize]).copied().collect()
}

#[test]
#[ignore = "needs a GPU"]
fn instances_past_the_instance_buffers_limit_dont_draw() {
    // Each scene's instances fill the lists' limit too, as on any device: a list is 16 times
    // smaller than an instance.
    let limits = |slots| Limits { instances: slots, draws: 1024, light_lists: slots };
    for (count, limit) in [(7, 8), (8, 8), (9, 8), (12, 8)] {
        let (pixels, stats) = render(count, false, Some(limits(u64::from(limit))));
        let over = count.saturating_sub(limit);
        assert_eq!(
            (stats.instances, stats.instances_over_limit, stats.lights.lists_over_limit),
            (count, over, over),
            "{count} instances, room for {limit}"
        );
        assert_eq!(stats.draws, count.min(limit));
        // The instances that fit draw exactly as they would alone, and the rest would show.
        let (alone, _) = render(count.min(limit), false, None);
        assert!(pixels == alone, "{count} instances, room for {limit}: the first {limit} differ");
        if over > 0 {
            assert!(render(count, false, None).0 != alone, "the instances left out are in view");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn instances_whose_draws_pass_the_records_limit_dont_draw() {
    // Two draws an instance: five records hold two whole instances, not the third's first draw.
    for (count, records, drawn) in [(2, 5, 2), (3, 5, 2), (3, 6, 3), (4, 6, 3)] {
        let limits = Limits { instances: 64, draws: records, light_lists: 64 };
        let (pixels, stats) = render(count, true, Some(limits));
        assert_eq!(
            (stats.draws, stats.draws_over_limit),
            (2 * drawn, 2 * (count - drawn)),
            "{count} instances, room for {records} draws"
        );
        let (alone, _) = render(drawn, true, None);
        assert!(pixels == alone, "{count} instances, room for {records} draws: the first {drawn} differ");
        if drawn < count {
            assert!(render(count, true, None).0 != alone, "the instances left out are in view");
        }
    }
}
