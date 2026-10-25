//! Renders the M0 toon test scene offscreen and writes it to a PNG.
//!
//! ```text
//! cargo run -p sapphire_render --example headless -- out.png [fov_degrees]
//! ```
//!
//! The optional vertical field of view (default 38°) moves the camera so the scene keeps the
//! same framing, which makes outline widths easy to compare across lenses.
//!
//! The scene is a smooth sphere beside a hard-edged cube, built in code. The cube's shading
//! normals are split per face, while its outline normals are averaged across shared
//! positions (what the cooker bakes for real assets), so the hull closes at every edge.

use std::collections::HashMap;
use std::error::Error;
use std::f32::consts::{PI, TAU};
use std::fs::File;
use std::io::BufWriter;

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::mesh::{MeshAsset, MeshVertex, Submesh, ToonMaterialParams};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    CameraData, DirectionalLight, FramePacket, InstanceId, MeshInstance, Msaa, RenderSettings, Renderer,
    SkyGradient, ViewData,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
/// Rendering to an sRGB texture means the PNG bytes come out already encoded.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn main() -> Result<(), Box<dyn Error>> {
    let filter: Targets = std::env::var("RUST_LOG")
        .ok()
        .and_then(|filter| filter.parse().ok())
        .unwrap_or_else(|| "info".parse().expect("valid"));
    tracing_subscriber::registry().with(filter).with(tracing_subscriber::fmt::layer()).init();
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("usage: headless <output.png> [fov_degrees]")?;
    let fov_degrees: f32 = args.next().map_or(Ok(38.0), |fov| fov.parse())?;

    let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None)?;
    let mut renderer =
        Renderer::new(&gpu, FORMAT, RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() })?;
    let mesh = renderer.upload_mesh(&test_scene())?;

    let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("headless target"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Dolly with the lens so the scene fills the same part of the frame at any FOV.
    let look_at = Vec3::new(0.0, 0.55, 0.0);
    let fov = fov_degrees.to_radians();
    let dolly = (19f32.to_radians().tan() / (fov * 0.5).tan()).max(0.05);
    let camera = CameraData::look_at(
        look_at + Vec3::new(0.0, 0.8, 4.3) * dolly,
        look_at,
        Vec3::Y,
        fov,
        WIDTH as f32 / HEIGHT as f32,
        0.1,
    );
    // One frame, so one packet: it adds the scene's only instance.
    let mut packet = FramePacket {
        views: vec![ViewData { camera }],
        sun: DirectionalLight {
            direction: Vec3::new(0.7, -0.6, -0.35),
            color: Vec3::new(1.0, 0.97, 0.92),
            intensity: 1.0,
        },
        sky: SkyGradient::default(),
        ..FramePacket::default()
    };
    packet.instances.changed.push(MeshInstance { id: InstanceId(0), mesh, transform: Mat4::IDENTITY });
    renderer.prepare(&gpu, &packet);
    let mut graph = FrameGraph::new();
    let target_id = graph.import_texture("target", &target_view);
    renderer.record(&mut graph, target_id);
    GraphExecutor::new().execute(&gpu, graph)?;

    let pixels = read_back(&gpu, &target)?;
    let mut png = png::Encoder::new(BufWriter::new(File::create(&output)?), WIDTH, HEIGHT);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(&pixels)?;
    tracing::info!(output, "wrote frame");
    Ok(())
}

/// Copies an RGBA8 texture into tightly packed rows.
fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Result<Vec<u8>, Box<dyn Error>> {
    let row_bytes = WIDTH * 4;
    let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded_row_bytes * HEIGHT),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(HEIGHT),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        // The receiver outlives the poll below, so a failed send can't happen.
        let _ = sender.send(result);
    });
    gpu.device().poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;

    let mapped = buffer.get_mapped_range(..)?;
    let pixels = mapped
        .chunks_exact(padded_row_bytes as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect();
    Ok(pixels)
}

/// A smooth sphere (material 0) beside a rotated hard-edged cube (material 1).
fn test_scene() -> MeshAsset {
    let mut mesh = MeshAsset::default();
    append_submesh(&mut mesh, 0, |vertices, indices| {
        uv_sphere(vertices, indices, Vec3::new(-0.95, 0.75, 0.0), 0.75)
    });
    // Yawed and tipped toward the camera so three faces show: two lit, one in shadow.
    let cube_transform = Mat4::from_rotation_translation(
        Quat::from_rotation_y(35f32.to_radians()) * Quat::from_rotation_x(20f32.to_radians()),
        Vec3::new(1.0, 0.65, -0.1),
    );
    append_submesh(&mut mesh, 1, |vertices, indices| cube(vertices, indices, cube_transform, 0.6));

    mesh.materials = vec![
        // A warm "skin" tone whose shadow side shifts toward rose-violet instead of grey.
        ToonMaterialParams {
            base_color: [0.95, 0.62, 0.50, 1.0],
            shade_color: [0.78, 0.55, 0.70, 1.0],
            outline_color: [0.24, 0.09, 0.10, 1.0],
            // Outline width in pixels at 1080p (3.9 px in this 540p image).
            params: [0.05, 0.02, 0.4, 7.8],
        },
        // A saturated blue with a violet shadow tint and a navy outline.
        ToonMaterialParams {
            base_color: [0.16, 0.36, 0.85, 1.0],
            shade_color: [0.55, 0.50, 0.85, 1.0],
            outline_color: [0.03, 0.03, 0.12, 1.0],
            params: [0.0, 0.02, 0.3, 7.8],
        },
    ];
    average_outline_normals(&mut mesh.vertices);
    mesh.bounds = bounds(&mesh.vertices);
    mesh
}

/// Runs `build` to produce one object's vertices and indices, then appends them as a
/// submesh drawn with `material`.
fn append_submesh(
    mesh: &mut MeshAsset,
    material: u32,
    build: impl FnOnce(&mut Vec<MeshVertex>, &mut Vec<u32>),
) {
    let (base_vertex, first_index) = (mesh.vertices.len() as u32, mesh.indices.len() as u32);
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    build(&mut vertices, &mut indices);
    mesh.vertices.extend(vertices);
    mesh.indices.extend(indices.iter().map(|i| i + base_vertex));
    mesh.submeshes.push(Submesh {
        first_index,
        index_count: indices.len() as u32,
        material_index: material,
        _reserved: 0,
    });
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> MeshVertex {
    MeshVertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: [255; 4],
    }
}

fn uv_sphere(vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>, center: Vec3, radius: f32) {
    const RINGS: u32 = 32;
    const SEGMENTS: u32 = 48;
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let normal = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let uv = [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32];
            vertices.push(vertex(center + normal * radius, normal, uv));
        }
    }
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            // Counter-clockwise seen from outside; skip the zero-area triangles at the poles.
            if ring != 0 {
                indices.extend([a, c, b]);
            }
            if ring != RINGS - 1 {
                indices.extend([c, d, b]);
            }
        }
    }
}

fn cube(vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>, transform: Mat4, half_extent: f32) {
    // (outward normal, in-face "right" axis); "up" is normal × right, so corners listed
    // right-handed around the normal are counter-clockwise seen from outside.
    let faces = [
        (Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_X, Vec3::Z),
        (Vec3::Y, Vec3::X),
        (Vec3::NEG_Y, Vec3::X),
        (Vec3::Z, Vec3::X),
        (Vec3::NEG_Z, Vec3::NEG_X),
    ];
    for (normal, right) in faces {
        let up = normal.cross(right);
        let first = vertices.len() as u32;
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let local = (normal + right * u + up * v) * half_extent;
            vertices.push(vertex(
                transform.transform_point3(local),
                transform.transform_vector3(normal),
                [(u + 1.0) * 0.5, (1.0 - v) * 0.5],
            ));
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
}

/// Sets every vertex's outline normal to the average shading normal of all vertices at the
/// same position, so hard edges don't split the outline hull.
fn average_outline_normals(vertices: &mut [MeshVertex]) {
    let key = |v: &MeshVertex| v.position.map(|c| (c * 1.0e4).round() as i32);
    let mut sums: HashMap<[i32; 3], Vec3> = HashMap::new();
    for v in vertices.iter() {
        *sums.entry(key(v)).or_default() += Vec3::from(v.normal);
    }
    for v in vertices.iter_mut() {
        v.outline_normal = sums[&key(v)].normalize_or_zero().to_array();
    }
}

fn bounds(vertices: &[MeshVertex]) -> [[f32; 3]; 2] {
    let (min, max) = vertices.iter().fold((Vec3::MAX, Vec3::MIN), |(min, max), v| {
        let p = Vec3::from(v.position);
        (min.min(p), max.max(p))
    });
    [min.to_array(), max.to_array()]
}
