//! Outlines v2 (M1 C4) on the GPU: hulls that stay closed at hard edges and through skinning,
//! the width's distance curve, FOV normalization, model scale and LOD cutoff, view-space
//! flattening and push-back, lines lit in light, shade and at night, and the hull's own velocity;
//! and lines under temporal AA (M1 P1) against 4× MSAA: turning, under a circling camera,
//! upsampled and held still. Run with `cargo test -p sapphire_render --tests -- --ignored`.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, OutlineCurve, Rim,
};
use sapphire_assets::mesh::{
    Influences, Joint, JointTransform, Lod, MaterialSlot, Mesh, PositionFormat, Skin, SkinBind, Submesh,
    VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags, VertexInfluence,
};
use sapphire_assets::texture::Swizzle;
use sapphire_pipeline::bake::{self, OutlineSettings};
use sapphire_pipeline::mesh::{ImportedMaterial, ImportedMesh};
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    MeshHandle, MeshInstance, Msaa, PostParams, PrepassTargets, RenderSettings, RenderStats, Renderer,
    SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (960, 540);

/// Linear 0.5 grey, the test sky's: a surface of this albedo, lit by the unit key and the 6%
/// ambient, shows the sky's brightness.
const GREY: [f32; 3] = [0.5 / 1.06; 3];

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

fn target(gpu: &GpuContext, (width, height): (u32, u32)) -> wgpu::Texture {
    gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// `material` cooked as the importer embeds a generated one: no textures.
fn cooked(material: &AnimeCharacterMaterial) -> Material {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(material, &swizzles).expect("a valid material")
}

/// Uploads `mesh` the way the runtime loads one, with the materials it embeds.
fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("a valid mesh");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

/// A curve that holds the pixel width at every distance this side of `cutoff`.
fn flat_curve(cutoff: f32) -> OutlineCurve {
    OutlineCurve { near: (0.0, 1.0), mid: 1.0, far: (cutoff, 1.0), cutoff }
}

/// Lit in `rgb` with no shadow tone and no rim, a black outline `width` pixels wide at 1080 rows
/// at every distance, extruded across the screen alone.
fn flat(rgb: [f32; 3], width: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width, curve: flat_curve(1000.0), flatten: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A UV sphere of `radius` at the origin, its outline normals `outline(normal)`.
fn sphere(radius: f32, material: &AnimeCharacterMaterial, outline: impl Fn(Vec3) -> Vec3) -> Mesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: (n * radius).to_array(),
                normal: n.to_array(),
                outline_normal: outline(n).to_array(),
                uv: [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, c, b, c, d, b]);
        }
    }
    single(&vertices, indices, material)
}

/// A mesh of `vertices` and `indices` with one material.
fn single(vertices: &[Vertex], indices: Vec<u32>, material: &AnimeCharacterMaterial) -> Mesh {
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("material", cooked(material))];
    Mesh::from_vertices(vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// A box of `half_extents` with hard edges: four vertices a face, each with its face's normal,
/// which is its outline normal too until the cooker's bake welds them (`baked`). `segments`
/// slices it along Y, for bending.
fn hard_box(half_extents: Vec3, segments: u32) -> (Vec<Vertex>, Vec<u32>) {
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (normal, u, v) in faces {
        // The side faces are sliced along Y; the caps are one quad.
        let steps = if normal.y == 0.0 { segments } else { 1 };
        let along_y = u.y != 0.0;
        let base = vertices.len() as u32;
        for row in 0..=steps {
            for column in 0..=1u32 {
                let (s, t) = if along_y {
                    (row as f32 / steps as f32 * 2.0 - 1.0, column as f32 * 2.0 - 1.0)
                } else {
                    (column as f32 * 2.0 - 1.0, row as f32 / steps as f32 * 2.0 - 1.0)
                };
                let position = (normal + u * s + v * t) * half_extents;
                vertices.push(Vertex {
                    position: position.to_array(),
                    normal: normal.to_array(),
                    outline_normal: normal.to_array(),
                    uv: [0.0; 2],
                    color: VERTEX_COLOR_NEUTRAL,
                    flags: VertexFlags::NONE,
                });
            }
        }
        for row in 0..steps {
            let a = base + row * 2;
            // u × v is the face normal: (s, t) = (−1, −1), (1, −1), (1, 1), (−1, 1) wind CCW.
            let quad = if along_y { [a, a + 2, a + 3, a + 1] } else { [a, a + 1, a + 3, a + 2] };
            indices.extend([quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
        }
    }
    (vertices, indices)
}

/// `vertices` and `indices` as the cooker cooks them: outline normals baked by welding split
/// vertices (`sapphire_pipeline::bake`), then packed, the outline normals into their tangent
/// frames.
fn baked(vertices: &[Vertex], indices: &[u32], material: &AnimeCharacterMaterial) -> Mesh {
    let submesh = Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 };
    let mut mesh = ImportedMesh {
        vertices: vertices.to_vec(),
        authored_outline: vec![false; vertices.len()],
        indices: indices.to_vec(),
        lods: vec![Lod { vertex_count: vertices.len() as u32, error: 0.0, submeshes: vec![submesh] }],
        materials: vec![ImportedMaterial { name: "material".into(), generated: material.clone() }],
        ..ImportedMesh::default()
    };
    bake::outline_normals(&mut mesh, &OutlineSettings::default());
    mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).expect("packs")
}

fn instance(id: u64, mesh: MeshHandle, transform: Mat4) -> MeshInstance {
    MeshInstance { id: InstanceId(id), mesh, transform }
}

/// A camera at `eye` looking at the origin with a vertical field of view of `fov` degrees.
fn camera(eye: Vec3, fov: f32) -> CameraData {
    looking(eye, Vec3::ZERO, fov)
}

/// A camera at `eye` looking at `target` with a vertical field of view of `fov` degrees.
fn looking(eye: Vec3, target: Vec3, fov: f32) -> CameraData {
    CameraData::look_at(eye, target, Vec3::Y, fov.to_radians(), SIZE.0 as f32 / SIZE.1 as f32, 0.05)
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

/// A prepass target to copy after a frame, and the readback it goes to.
type PrepassRead<'a> = (fn(&PrepassTargets) -> TextureId, &'a Readback);

/// A buffer a 4-byte-per-texel texture is copied into.
struct Readback {
    buffer: wgpu::Buffer,
    padded: u32,
}

impl Readback {
    fn new(gpu: &GpuContext) -> Self {
        let padded = (SIZE.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIZE.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, padded }
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        let aspect = if texture.format().is_depth_stencil_format() {
            wgpu::TextureAspect::DepthOnly
        } else {
            wgpu::TextureAspect::All
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { aspect, ..texture.as_image_copy() },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded),
                    rows_per_image: Some(SIZE.1),
                },
            },
            texture.size(),
        );
    }

    fn texels(&self, gpu: &GpuContext) -> Vec<[u8; 4]> {
        self.buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let texels = {
            let mapped = self.buffer.get_mapped_range(..).expect("mapped");
            mapped
                .chunks_exact(self.padded as usize)
                .flat_map(|row| row[..SIZE.0 as usize * 4].as_chunks::<4>().0.iter().copied())
                .collect()
        };
        self.buffer.unmap();
        texels
    }
}

/// A renderer drawing into a `SIZE` target, and the scene constants its packets carry.
struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    texture: wgpu::Texture,
    color: Readback,
    sun: DirectionalLight,
    ambient: AmbientLight,
    /// Sent with the next frame only.
    poses: Vec<(u64, Vec<Affine3A>)>,
}

impl Frames {
    fn new(gpu: &GpuContext, settings: RenderSettings) -> Self {
        Self {
            renderer: Renderer::new(gpu, FORMAT, settings).expect("renderer"),
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            texture: target(gpu, SIZE),
            color: Readback::new(gpu),
            // Along the view direction of a camera on +Z: everything it sees is lit.
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
            ambient: AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 },
            poses: Vec::new(),
        }
    }

    fn msaa(gpu: &GpuContext) -> Self {
        Self::new(gpu, RenderSettings::default())
    }

    /// Draws one frame from `camera`, adding or changing the `changed` instances, and returns
    /// its pixels; `reads` copies prepass targets into readbacks (and so runs the prepasses).
    fn render(
        &mut self,
        gpu: &GpuContext,
        camera: CameraData,
        changed: &[MeshInstance],
        reads: &[PrepassRead<'_>],
    ) -> Vec<[u8; 4]> {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.sun = self.sun;
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
        packet.ambient = self.ambient;
        packet.post = passthrough();
        packet.instances.changed.extend_from_slice(changed);
        for (id, palette) in self.poses.drain(..) {
            packet.push_skinned(InstanceId(id), palette, []);
        }
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let view = self.texture.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        let reads: Vec<(TextureId, &Readback)> =
            reads.iter().map(|(target, readback)| (target(&targets), *readback)).collect();
        let (texture, color) = (&self.texture, &self.color);
        let mut pass = graph.add_pass("read back").side_effect().read(target);
        for (texture, _) in &reads {
            pass = pass.read(*texture);
        }
        pass.record(move |ctx| {
            color.copy(ctx.encoder, texture);
            for (texture, readback) in reads {
                readback.copy(ctx.encoder, ctx.texture(texture));
            }
        });
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        self.color.texels(gpu)
    }

    fn stats(&self) -> RenderStats {
        self.renderer.stats()
    }
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear(pixel: [u8; 4]) -> Vec3 {
    Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
}

fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE.0 + x) as usize]
}

/// Outline thickness in pixels where the middle row crosses the left half: each pixel's
/// darkness against the grey sky and surface is its outline coverage.
fn left_outline_width(pixels: &[[u8; 4]]) -> f32 {
    let row = SIZE.1 / 2;
    (0..SIZE.0 / 2).map(|x| (1.0 - linear(at(pixels, x, row)).x / 0.5).clamp(0.0, 1.0)).sum()
}

/// An `Rg16Float` texel's two values.
fn rg16f(texel: [u8; 4]) -> [f32; 2] {
    [u16::from_le_bytes([texel[0], texel[1]]), u16::from_le_bytes([texel[2], texel[3]])].map(f16_to_f32)
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// tan(22.5°): outline widths are normalized to a 45° vertical field of view.
const REFERENCE_TAN: f32 = 0.414_213_57;

/// The view depth of the silhouette of a sphere of `radius` seen from `distance` away: where the
/// middle row's outline grows from.
fn silhouette_depth(distance: f32, radius: f32) -> f32 {
    (distance * distance - radius * radius) / distance
}

/// The camera distance at which a sphere of `radius` has its silhouette at the effective
/// distance `effective` through a vertical field of view of `fov` degrees.
fn distance_for(effective: f32, fov: f32, radius: f32) -> f32 {
    let depth = effective * REFERENCE_TAN / (fov.to_radians() * 0.5).tan();
    0.5 * (depth + (depth * depth + 4.0 * radius * radius).sqrt())
}

#[test]
#[ignore = "needs a GPU"]
fn the_width_follows_its_curve_the_field_of_view_and_the_model_scale() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    let curve = OutlineCurve { near: (1.0, 1.6), mid: 3.0, far: (12.0, 0.5), cutoff: 1000.0 };
    let radius = 0.3;
    let material = |model_scale| {
        let mut material = flat(GREY, 20.0);
        material.outline.curve = curve;
        material.outline.model_scale = model_scale;
        material
    };
    let full = upload(&mut frames.renderer, &sphere(radius, &material(1.0), |n| n));
    let half = upload(&mut frames.renderer, &sphere(radius, &material(0.5), |n| n));
    // 20 px at 1080 rows is 10 here.
    let mut measure = |mesh, effective: f32, fov: f32| {
        let distance = distance_for(effective, fov, radius);
        let changed = [instance(1, mesh, Mat4::IDENTITY)];
        let pixels = frames.render(&gpu, camera(Vec3::new(0.0, 0.0, distance), fov), &changed, &[]);
        left_outline_width(&pixels)
    };
    let mut widths = Vec::new();
    // Closer than `near` the width holds in the world; the two segments; past `far`, in the world.
    for effective in [0.6, 2.0, 5.0, 8.0, 24.0] {
        let expected = 10.0 * curve.scale(effective);
        let width = measure(full, effective, 45.0);
        eprintln!("{effective:>4} m effective: {width:.2} px, expected {expected:.2}");
        assert!(
            (width - expected).abs() < 0.4 + 0.04 * expected,
            "{effective} m: {width} against {expected}"
        );
        widths.push((effective, width));
    }
    // FOV normalization: zoomed in or out to the same effective distance, the same width.
    for fov in [20.0, 70.0] {
        for &(effective, at_45) in &widths[1..4] {
            let width = measure(full, effective, fov);
            eprintln!("{effective:>4} m effective at {fov}°: {width:.2} px against {at_45:.2} at 45°");
            assert!((width - at_45).abs() < 0.3, "{fov}°, {effective} m: {width} against {at_45}");
        }
    }
    // The model scale scales it.
    let width = measure(half, 2.0, 45.0);
    let expected = 0.5 * widths[1].1;
    eprintln!("model scale 0.5: {width:.2} px, expected {expected:.2}");
    assert!((width - expected).abs() < 0.3, "{width} against {expected}");
}

#[test]
#[ignore = "needs a GPU"]
fn the_lod_cutoff_drops_far_hulls_after_fading_them() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    let radius = 0.3;
    let mut material = flat(GREY, 20.0);
    material.outline.curve = flat_curve(10.0);
    let mesh = upload(&mut frames.renderer, &sphere(radius, &material, |n| n));
    let measure = |frames: &mut Frames, distance: f32, fov: f32, scale: f32| {
        frames.renderer.set_outline_distance(scale);
        let changed = [instance(1, mesh, Mat4::IDENTITY)];
        let pixels = frames.render(&gpu, camera(Vec3::new(0.0, 0.0, distance), fov), &changed, &[]);
        let stats = frames.stats();
        (left_outline_width(&pixels), stats.outline_draws, stats.outlines_culled)
    };
    let (width, drawn, culled) = measure(&mut frames, distance_for(5.0, 45.0, radius), 45.0, 1.0);
    assert!(
        (width - 10.0).abs() < 0.4 && (drawn, culled) == (1, 0),
        "full width: {width}, {drawn}, {culled}"
    );
    // Over the cutoff's last quarter the width fades: at 8.5 m, 60% of it.
    let (width, drawn, _) = measure(&mut frames, distance_for(8.5, 45.0, radius), 45.0, 1.0);
    eprintln!("fading at 8.5 m: {width:.2} px");
    assert!((width - 6.0).abs() < 0.5 && drawn == 1, "{width}");
    // Past the cutoff, no hull at all.
    let (width, drawn, culled) = measure(&mut frames, 12.0, 45.0, 1.0);
    assert!(width < 0.05 && (drawn, culled) == (0, 1), "culled: {width}, {drawn}, {culled}");
    // Zooming in on it brings it back: at 20° it is 5.1 m away, effectively.
    let (width, drawn, culled) = measure(&mut frames, 12.0, 20.0, 1.0);
    eprintln!("zoomed in from 12 m: {width:.2} px");
    assert!(width > 9.0 && (drawn, culled) == (1, 0), "{width}, {drawn}, {culled}");
    // The renderer's scale on every cutoff (`r.outline_distance`): at 0.9 the same 8.5 m fades to
    // 22% (the cutoff at 9 m), and at 0 no hull draws.
    let (width, drawn, _) = measure(&mut frames, distance_for(8.5, 45.0, radius), 45.0, 0.9);
    eprintln!("fading at 8.5 m with the cutoff at 9: {width:.2} px");
    assert!((width - 10.0 * (9.0 - 8.5) / 2.25).abs() < 0.5 && drawn == 1, "{width}");
    let (width, drawn, culled) = measure(&mut frames, distance_for(1.0, 45.0, radius), 45.0, 0.0);
    assert!(width < 0.05 && (drawn, culled) == (0, 1), "none: {width}, {drawn}, {culled}");
}

#[test]
#[ignore = "needs a GPU"]
fn hulls_off_the_view_axis_are_culled_by_their_depth() {
    // The shader fades a line by its view depth; at the side of the frame a mesh's distance runs
    // up to about 1.2× its depth (review RA-002), so culling by distance dropped hulls whose lines
    // were still drawn. A cutoff of 10 m, and meshes near the frame's edges with their nearest
    // depth inside it and their distance past it, under 4× MSAA and under temporal AA.
    let gpu = gpu();
    let radius = 0.2;
    let mut material = flat(GREY, 20.0);
    material.outline.curve = flat_curve(10.0);
    let ball = sphere(radius, &material, |n| n);
    // Dark pixels against the grey sky and surface: the line.
    let lines = |pixels: &[[u8; 4]]| pixels.iter().filter(|&&pixel| linear(pixel).x < 0.3).count();
    // The camera's vertical field of view and aspect, and the ball's place and scale in view
    // space (the camera sits at the origin, looking down −Z).
    let cases: [(&str, f32, f32, Vec3, Vec3); 5] = [
        // The review's: its surface 8.4–8.8 m deep, 10.1 m past its bounds' radius away.
        ("the review's sphere", 45.0, 16.0 / 9.0, Vec3::new(5.9, 0.0, -8.6), Vec3::ONE),
        ("near the top at 70°", 70.0, 16.0 / 9.0, Vec3::new(0.0, 3.5, -5.6), Vec3::ONE),
        ("at the side of a 21:9 view at 30°", 30.0, 21.0 / 9.0, Vec3::new(-8.4, 0.0, -14.1), Vec3::ONE),
        // A rod two meters long pointing away, at the frame's side.
        ("a rod at the side", 45.0, 16.0 / 9.0, Vec3::new(6.0, 0.0, -10.3), Vec3::new(0.75, 0.75, 5.0)),
        // Wholly past the cutoff in depth: culled, as before.
        ("past the cutoff", 45.0, 16.0 / 9.0, Vec3::new(5.9, 0.0, -10.6), Vec3::ONE),
    ];
    let temporal = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    for settings in [RenderSettings::default(), temporal] {
        for (name, fov, aspect, place, scale) in cases {
            let mut frames = Frames::new(&gpu, settings);
            let mesh = upload(&mut frames.renderer, &ball);
            let camera =
                CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, fov.to_radians(), aspect, 0.05);
            let transform = Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, place);
            let pixels = frames.render(&gpu, camera, &[instance(1, mesh, transform)], &[]);
            let stats = frames.stats();
            let (drawn, culled, dark) = (stats.outline_draws, stats.outlines_culled, lines(&pixels));
            let effective = (fov.to_radians() * 0.5).tan() / REFERENCE_TAN;
            let nearest = (-place.z - radius * scale.z) * effective;
            eprintln!(
                "{name} (temporal {}): nearest {nearest:.2} effective m, {:.2} m away; \
                 drawn {drawn}, culled {culled}, {dark} line pixels",
                settings.temporal,
                (place.length() - radius * 3f32.sqrt() * scale.max_element()) * effective,
            );
            if nearest < 10.0 {
                assert!((drawn, culled) == (1, 0) && dark > 0, "{name}: {drawn}, {culled}, {dark}");
            } else {
                assert!((drawn, culled) == (0, 1) && dark == 0, "{name}: {drawn}, {culled}, {dark}");
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn flattening_narrows_and_pushes_back_extrusions_that_face_the_camera() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    let depth = Readback::new(&gpu);
    // Outline normals tilted toward the camera (+Z): at the silhouette their part across the
    // screen is under half their length. A narrow lens from far away keeps the view rays parallel.
    let tilt = 2.0;
    let (radius, distance, fov): (f32, f32, f32) = (1.0, 20.0, 10.0);
    let across = {
        // The silhouette's normal in view space, tilted.
        let sine = radius / distance;
        let normal = Vec3::new(-(1.0 - sine * sine).sqrt(), 0.0, sine);
        (normal + Vec3::Z * tilt).normalize().truncate().length()
    };
    let silhouette_px = (radius / (distance * distance - radius * radius).sqrt())
        / (fov.to_radians() * 0.5).tan()
        * SIZE.1 as f32
        / 2.0;
    let view = camera(Vec3::new(0.0, 0.0, distance), fov);
    let mut id = 0;
    // The line's width, and the hull's view depth a pixel and a half outside the silhouette (from
    // the scene prepass, which draws the hull too).
    let mut draw = |frames: &mut Frames, width: f32, flatten: f32| {
        id += 1;
        let mut material = flat(GREY, width);
        material.outline.flatten = flatten;
        let mesh =
            upload(&mut frames.renderer, &sphere(radius, &material, |n| (n + Vec3::Z * tilt).normalize()));
        let pixels = frames.render(
            &gpu,
            view,
            &[instance(id, mesh, Mat4::IDENTITY)],
            &[(|targets| targets.depth, &depth)],
        );
        // Out of the way of the next.
        frames.render(&gpu, view, &[instance(id, mesh, Mat4::from_translation(Vec3::splat(1e4)))], &[]);
        let x = (SIZE.0 as f32 / 2.0 - silhouette_px - 1.5) as u32;
        let texel = depth.texels(&gpu)[(SIZE.1 / 2 * SIZE.0 + x) as usize];
        (left_outline_width(&pixels), 0.05 / f32::from_le_bytes(texel))
    };
    // Across the screen: 20 px at 1080 rows (10 here) × across / |(across, flattening)|.
    let narrowing = |flatten: f32| across / (across * across + flatten * flatten).sqrt();
    for flatten in [0.0f32, 0.1, 0.5, 1.0] {
        let (width, _) = draw(&mut frames, 20.0, flatten);
        let expected = 10.0 * narrowing(flatten);
        eprintln!("flatten {flatten}: {width:.2} px (expected {expected:.2})");
        assert!(
            (width - expected).abs() < 0.4 + 0.04 * expected,
            "flatten {flatten}: {width} against {expected}"
        );
    }
    // Back along the view ray by the extrusion's depth part, flattening / |(across, flattening)| of
    // its full width in meters, without moving on screen: against an unflattened hull of the same
    // width across the screen, the same lines lie that much deeper.
    let (unflattened_width, unflattened) = draw(&mut frames, 20.0 * narrowing(1.0), 0.0);
    let (flattened_width, flattened) = draw(&mut frames, 20.0, 1.0);
    assert!(
        (unflattened_width - flattened_width).abs() < 0.3,
        "{unflattened_width} against {flattened_width}"
    );
    let meters_per_pixel =
        2.0 * silhouette_depth(distance, radius) * (fov.to_radians() * 0.5).tan() / SIZE.1 as f32;
    let expected = 1.0 / (across * across + 1.0).sqrt() * 10.0 * meters_per_pixel;
    let pushed = flattened - unflattened;
    eprintln!("flattened by 1: pushed back {pushed:.5} m, expected {expected:.5}");
    // The hull there lies a little behind the silhouette, where a pixel is wider in meters.
    assert!((pushed - expected).abs() < 0.2 * expected, "{pushed} against {expected}");
}

#[test]
#[ignore = "needs a GPU"]
fn lines_take_the_light_in_light_shade_and_night() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    // The key from +X: the right silhouette is lit, the left in shade.
    frames.sun = DirectionalLight { direction: Vec3::new(-1.0, 0.0, -0.2), color: Vec3::ONE, intensity: 1.0 };
    let (radius, distance, fov): (f32, f32, f32) = (1.0, 6.0, 30.0);
    let line = Vec3::new(0.4, 0.2, 0.1);
    let shade = Vec3::new(0.5, 0.6, 0.7);
    let albedo = Vec3::new(0.5, 1.0, 0.25);
    let material = |albedo_weight: f32, lighting: f32| AnimeCharacterMaterial {
        base_color: albedo.extend(1.0).to_array(),
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: shade.to_array(),
            outline_color: line.to_array(),
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        // 40 px at 1080 rows: 20 here, so the line's middle is well inside it.
        outline: Outline {
            width: 40.0,
            curve: flat_curve(1000.0),
            flatten: 0.0,
            albedo: albedo_weight,
            lighting,
            ..Outline::default()
        },
        ..AnimeCharacterMaterial::default()
    };
    let silhouette_px = (radius / (distance * distance - radius * radius).sqrt())
        / (fov.to_radians() * 0.5).tan()
        * SIZE.1 as f32
        / 2.0;
    let (left, right) =
        (SIZE.0 as f32 / 2.0 - silhouette_px - 10.0, SIZE.0 as f32 / 2.0 + silhouette_px + 10.0);
    let view = camera(Vec3::new(0.0, 0.0, distance), fov);
    let mut id = 0;
    let mut lines = |frames: &mut Frames, material: &AnimeCharacterMaterial| {
        id += 1;
        let mesh = upload(&mut frames.renderer, &sphere(radius, material, |n| n));
        let changed = [instance(id, mesh, Mat4::IDENTITY)];
        let pixels = frames.render(&gpu, view, &changed, &[]);
        // Out of the way of the next.
        frames.render(&gpu, view, &[instance(id, mesh, Mat4::from_translation(Vec3::splat(1e4)))], &[]);
        let row = SIZE.1 / 2;
        (linear(at(&pixels, left as u32, row)), linear(at(&pixels, right as u32, row)))
    };
    let near = |found: Vec3, expected: Vec3, what: &str| {
        eprintln!("{what}: {found:.3} (expected {expected:.3})");
        assert!(
            (found - expected).abs().max_element() < 0.006 + 0.04 * expected.max_element(),
            "{what}: {found} against {expected}"
        );
    };

    // By day: the key light on the lit side, its shadow color on the other, both plus the GI
    // (the ambient hemisphere's 6%, flattened toward the camera).
    let gi = Vec3::splat(0.06);
    let (shaded, lit) = lines(&mut frames, &material(0.0, 1.0));
    near(lit, line * (Vec3::ONE + gi), "lit");
    near(shaded, line * (shade + gi), "in shade");
    // The albedo weight multiplies the line by the surface's color.
    let (shaded, lit) = lines(&mut frames, &material(1.0, 1.0));
    near(lit, line * albedo * (Vec3::ONE + gi), "lit, of the albedo");
    near(shaded, line * albedo * (shade + gi), "in shade, of the albedo");
    // Unlit: the color as authored, on both sides.
    let (shaded, lit) = lines(&mut frames, &material(0.0, 0.0));
    near(lit, line, "unlit");
    near(shaded, line, "unlit, the shadow side");
    // At night: a dim blue key and a deep blue ambient darken and tint the lines.
    let moon = Vec3::new(0.6, 0.7, 1.0) * 0.4;
    frames.sun = DirectionalLight {
        direction: Vec3::new(-1.0, 0.0, -0.2),
        color: Vec3::new(0.6, 0.7, 1.0),
        intensity: 0.4,
    };
    frames.ambient =
        AmbientLight { sky: Vec3::new(0.1, 0.12, 0.3), ground: Vec3::new(0.1, 0.12, 0.3), intensity: 0.5 };
    let night_gi = Vec3::new(0.05, 0.06, 0.15);
    let (shaded, lit) = lines(&mut frames, &material(0.0, 1.0));
    near(lit, line * (moon + night_gi), "lit at night");
    near(shaded, line * (shade * moon + night_gi), "in shade at night");
    assert!(lit.max_element() < 0.5 * line.max_element(), "darker than by day");
}

#[test]
#[ignore = "needs a GPU"]
fn hard_edges_keep_their_hulls_closed_static_and_skinned() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    let view = camera(Vec3::new(2.2, 1.6, 2.8), 30.0);
    // 16 px at 1080 rows: 8 here. Every pixel within 3 of the silhouette must be line.
    let reach = 3;
    let red = AnimeCharacterMaterial { base_color: [0.8, 0.05, 0.05, 1.0], ..flat(GREY, 16.0) };
    let bare_red = AnimeCharacterMaterial { outline: Outline { width: 0.0, ..red.outline }, ..red.clone() };
    // Gap pixels in the line around `mesh`: the sky showing within `reach` of its surface.
    let gaps = |frames: &mut Frames, mut outlined: Mesh, skin: Option<(Skin, Vec<Affine3A>)>, id: u64| {
        let mut bare = outlined.clone();
        bare.materials = vec![MaterialSlot::embedded("material", cooked(&bare_red))];
        let poses = skin.as_ref().map(|(_, palette)| palette.clone());
        if let Some((skin, _)) = skin {
            outlined.skin = Some(skin.clone());
            bare.skin = Some(skin);
        }
        let mut image = |mesh: &Mesh, id: u64| {
            let handle = upload(&mut frames.renderer, mesh);
            if let Some(palette) = &poses {
                frames.poses.push((id, palette.clone()));
            }
            let pixels = frames.render(&gpu, view, &[instance(id, handle, Mat4::IDENTITY)], &[]);
            frames.render(&gpu, view, &[instance(id, handle, Mat4::from_translation(Vec3::splat(1e4)))], &[]);
            pixels
        };
        let bare_pixels = image(&bare, id);
        let surface: Vec<bool> =
            bare_pixels.iter().map(|&p| linear(p).x > 0.3 && linear(p).y < 0.1).collect();
        let lined = image(&outlined, id + 1);
        dump(&format!("hull_{id}_bare"), &bare_pixels);
        dump(&format!("hull_{id}_lined"), &lined);
        let (width, height) = (SIZE.0 as i32, SIZE.1 as i32);
        let mut count = 0;
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                if surface[index] {
                    continue;
                }
                let near_surface = (-reach..=reach).any(|dy| {
                    (-reach..=reach).any(|dx| {
                        let (sx, sy) = (x + dx, y + dy);
                        dx * dx + dy * dy <= reach * reach
                            && (0..width).contains(&sx)
                            && (0..height).contains(&sy)
                            && surface[(sy * width + sx) as usize]
                    })
                });
                // The sky's green (0.5) showing: the red surface and the black line have none.
                if near_surface && linear(lined[index]).y > 0.2 {
                    count += 1;
                }
            }
        }
        count
    };
    let (vertices, indices) = hard_box(Vec3::new(0.5, 0.5, 0.5), 1);
    let welded = gaps(&mut frames, baked(&vertices, &indices, &red), None, 1);
    let split = gaps(&mut frames, single(&vertices, indices.clone(), &red), None, 3);
    eprintln!("a hard-edged box: {welded} gap pixels welded, {split} with split outline normals");
    assert_eq!(welded, 0, "welded outline normals keep the hull closed");
    assert!(split > 20, "split ones open it at the corners ({split}): the test sees cracks");

    // Bent at a joint by linear blend skinning, whose blended matrix there is no rotation: the
    // split vertices of each hard edge share one outline normal through it.
    let (vertices, indices) = hard_box(Vec3::new(0.25, 0.6, 0.25), 2);
    let influences: Vec<VertexInfluence> = vertices
        .iter()
        .map(|vertex| match vertex.position[1] {
            y if y < -0.1 => VertexInfluence { binds: [0, 0, 0, 0], weights: [255, 0, 0, 0] },
            y if y > 0.1 => VertexInfluence { binds: [1, 0, 0, 0], weights: [255, 0, 0, 0] },
            _ => VertexInfluence { binds: [0, 1, 0, 0], weights: [128, 127, 0, 0] },
        })
        .collect();
    let joint = |name: &str| Joint { name: name.into(), parent: None, rest: JointTransform::IDENTITY };
    let bind = |joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() };
    let skin = Skin {
        joints: vec![joint("lower"), joint("upper")],
        binds: vec![bind(0), bind(1)],
        influences: Influences::pack(&influences, 2),
    };
    // The upper half turns 70° about Z around the joint.
    let bend = Affine3A::from_rotation_z(70f32.to_radians());
    let palette = vec![Affine3A::IDENTITY, bend];
    let bent = gaps(&mut frames, baked(&vertices, &indices, &red), Some((skin, palette)), 5);
    eprintln!("bent at its joint: {bent} gap pixels");
    assert_eq!(bent, 0, "the bent box's hull stays closed");
}

/// A hull pixel's velocity is its line's (M1 C4, the silhouette's since M1 P1): exactly none
/// held still; under a translation or a camera pan the silhouette's motion, not that of the
/// deeper hull points the line shows; and on a body turning about its own axis next to none,
/// where those points slide by up to 3.4 px a frame (0.1 rad a frame; 3 rad/s at 60 fps is half
/// that). The line measures 19–28% of the sliding at each pixel, 18% in all.
#[test]
#[ignore = "needs a GPU"]
fn hulls_move_with_their_silhouette() {
    let gpu = gpu();
    let mut frames = Frames::msaa(&gpu);
    let velocity = Readback::new(&gpu);
    let reads: [PrepassRead<'_>; 1] = [(|targets| targets.velocity, &velocity)];
    let (radius, distance, fov): (f32, f32, f32) = (0.5, 4.0, 40.0);
    // 20 px at 1080 rows: 10 here.
    let line = 10.0;
    let mesh = upload(&mut frames.renderer, &sphere(radius, &flat(GREY, 2.0 * line), |n| n));
    let tan = (fov.to_radians() * 0.5).tan();
    let focal_x = 1.0 / (tan * SIZE.0 as f32 / SIZE.1 as f32);
    let silhouette_px = (radius / (distance * distance - radius * radius).sqrt()) / tan * SIZE.1 as f32 / 2.0;
    let row = SIZE.1 / 2;
    // A pixel `out` pixels beyond the silhouette shows the hull of the sphere point `line − out`
    // pixels inside it on the far side, which lies deeper than the silhouette.
    let meters_per_pixel = 2.0 * silhouette_depth(distance, radius) * tan / SIZE.1 as f32;
    let behind = |out: f32| {
        let cosine = 1.0 - (line - out) * meters_per_pixel / radius;
        (1.0 - cosine * cosine).max(0.0).sqrt()
    };
    let depth_of = |out: f32| silhouette_depth(distance, radius) + radius * behind(out);
    // Pixels 3, 5 and 8 out on each side, around a center `offset` pixels right of the image's.
    let pixels = |offset: f32| {
        let center = SIZE.0 as f32 / 2.0 + offset;
        [3.0f32, 5.0, 8.0].into_iter().flat_map(move |out| {
            [
                ((center - silhouette_px - out - 0.5) as u32, out),
                ((center + silhouette_px + out + 0.5) as u32, out),
            ]
        })
    };
    let view = camera(Vec3::new(0.0, 0.0, distance), fov);
    frames.render(&gpu, view, &[instance(1, mesh, Mat4::IDENTITY)], &reads);
    let texels = velocity.texels(&gpu);
    for (x, _) in pixels(0.0) {
        assert_eq!(rg16f(at(&texels, x, row)), [0.0, 0.0], "still: none at x = {x}");
    }

    // Moved right by 5 cm: the line with the silhouette, though the hull points it shows are
    // deeper and move less (by the inverse of their depth).
    let step = 0.05;
    let shift = |depth: f32| 0.5 * focal_x * step / depth * SIZE.0 as f32;
    let moved_px = shift(silhouette_depth(distance, radius));
    frames.render(
        &gpu,
        view,
        &[instance(1, mesh, Mat4::from_translation(Vec3::new(step, 0.0, 0.0)))],
        &reads,
    );
    let texels = velocity.texels(&gpu);
    for (x, out) in pixels(moved_px) {
        let [vx, vy] = rg16f(at(&texels, x, row));
        let found = vx * SIZE.0 as f32;
        eprintln!(
            "moved: the line {out} px out moved {found:.3} px, the silhouette {moved_px:.3} (its hull point {:.3})",
            shift(depth_of(out))
        );
        assert!((found - moved_px).abs() < 0.02 * moved_px && vy.abs() < 1e-4, "{found} against {moved_px}");
    }
    // Held still: exactly none.
    frames.render(&gpu, view, &[], &reads);
    let texels = velocity.texels(&gpu);
    for (x, _) in pixels(moved_px) {
        assert_eq!(rg16f(at(&texels, x, row)), [0.0, 0.0], "still again: none at x = {x}");
    }
    // The camera moving left moves everything right, the silhouette by its depth.
    let panned = looking(Vec3::new(-step, 0.0, distance), Vec3::new(-step, 0.0, 0.0), fov);
    frames.render(&gpu, panned, &[], &reads);
    let texels = velocity.texels(&gpu);
    for (x, out) in pixels(2.0 * moved_px) {
        let found = rg16f(at(&texels, x, row))[0] * SIZE.0 as f32;
        eprintln!("panned: the line {out} px out moved {found:.3} px, the silhouette {moved_px:.3}");
        assert!((found - moved_px).abs() < 0.02 * moved_px, "{found} against {moved_px}");
    }

    // Turning about its own vertical axis by ω, the silhouette holds still while the surface
    // slides: the front's middle across at ωR, the sphere's points behind the silhouette the other
    // way at ωR sin β, β past it. The line shows the hull of those points, but holds still with
    // the silhouette.
    let turn = 0.1;
    // A second sphere at rest in the middle of the first view, the first out of the way.
    let away = instance(1, mesh, Mat4::from_translation(Vec3::splat(1e4)));
    frames.render(&gpu, view, &[away, instance(2, mesh, Mat4::IDENTITY)], &reads);
    frames.render(&gpu, view, &[instance(2, mesh, Mat4::from_rotation_y(turn))], &reads);
    let texels = velocity.texels(&gpu);
    let center = rg16f(at(&texels, SIZE.0 / 2, row))[0] * SIZE.0 as f32;
    let front = 0.5 * focal_x * turn * radius / (distance - radius) * SIZE.0 as f32;
    eprintln!("turning: the surface's middle moves {center:.2} px, expected {front:.2}");
    assert!((center - front).abs() < 0.03 * front, "{center} against {front}");
    let (mut moved, mut slid) = (0.0, 0.0);
    for (x, out) in pixels(0.0) {
        let found = rg16f(at(&texels, x, row))[0] * SIZE.0 as f32;
        let sliding = 0.5 * focal_x * turn * radius * behind(out) / depth_of(out) * SIZE.0 as f32;
        eprintln!("turning: the line {out} px out moves {found:.3} px, its hull point slides {sliding:.3}");
        assert!(found.abs() < 0.35 * sliding, "{found} against the sliding {sliding}");
        moved += found.abs();
        slid += sliding;
    }
    assert!(moved < 0.25 * slid, "the line moves {moved} px against its points' {slid} in all");
}

/// How the lines around a sphere's silhouette look in one frame: each pixel's darkness against
/// the grey sky, in windows across both sides of the silhouette on 17 rows through it.
struct Lines(Vec<f32>);

impl Lines {
    /// The windows around a sphere of `radius_px` centered on the image: from 14 px outside the
    /// silhouette to 3 px inside it, where the line and its antialiased edges lie.
    fn of(pixels: &[[u8; 4]], radius_px: f32) -> Self {
        let center = (SIZE.0 as f32 / 2.0, SIZE.1 as f32 / 2.0);
        let mut darkness = Vec::new();
        for row in -8..=8 {
            let dy = row as f32 * radius_px / 10.0;
            let half = (radius_px * radius_px - dy * dy).sqrt();
            let y = (center.1 + dy) as u32;
            for (from, to) in [
                (center.0 - half - 14.0, center.0 - half + 3.0),
                (center.0 + half - 3.0, center.0 + half + 14.0),
            ] {
                darkness.extend(
                    (from as u32..to as u32)
                        .map(|x| (1.0 - linear(at(pixels, x, y)).y / 0.5).clamp(0.0, 1.0)),
                );
            }
        }
        Self(darkness)
    }

    /// The lines' strength: their total darkness.
    fn strength(&self) -> f32 {
        self.0.iter().sum()
    }

    /// Their contrast: the darkness-weighted mean darkness, which a blurred line loses.
    fn contrast(&self) -> f32 {
        self.0.iter().map(|d| d * d).sum::<f32>() / self.strength().max(1e-6)
    }

    /// Their width: how many pixels' worth the darkness spreads over (`strength / contrast`).
    fn width(&self) -> f32 {
        self.strength() / self.contrast().max(1e-6)
    }

    /// The mean change of a pixel's darkness from `previous`.
    fn change(&self, previous: &Self) -> f32 {
        self.0.iter().zip(&previous.0).map(|(a, b)| (a - b).abs()).sum::<f32>() / self.0.len() as f32
    }
}

/// How lines under TAA compare with 4× MSAA's over the last 8 frames of `frames` (a jitter cycle):
/// their strength, contrast and width relative to MSAA's, and their mean frame-to-frame change
/// (MSAA's alongside).
#[derive(Debug)]
struct LineComparison {
    strength: f32,
    contrast: f32,
    width: f32,
    change: f32,
    msaa_change: f32,
}

/// Draws the outlined unit sphere 4 m from the camera (its default 5.5 px line, 2.75 px here)
/// with TAA at render scale `scale` and with 4× MSAA alike for `frames` frames, frame `n` seen
/// by `view(n)` with the sphere at `transform(n)`, and compares their lines.
fn compare_lines(
    gpu: &GpuContext,
    scale: impl Fn(u32) -> f32,
    frames: u32,
    view: impl Fn(u32) -> CameraData,
    transform: impl Fn(u32) -> Mat4,
) -> LineComparison {
    let taa_settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    let mut taa = Frames::new(gpu, taa_settings);
    let mut msaa = Frames::msaa(gpu);
    let mesh = sphere(1.0, &flat([0.9, 0.6, 0.3], 5.5), |n| n);
    let (taa_ball, msaa_ball) = (upload(&mut taa.renderer, &mesh), upload(&mut msaa.renderer, &mesh));
    let radius_px = (1.0 / 15f32.sqrt()) / 22.5f32.to_radians().tan() * SIZE.1 as f32 / 2.0;
    let (mut taa_lines, mut msaa_lines) = (Vec::new(), Vec::new());
    for frame in 0..frames {
        let at = transform(frame);
        taa.renderer.set_render_scale(scale(frame));
        let taa_pixels = taa.render(gpu, view(frame), &[instance(1, taa_ball, at)], &[]);
        let msaa_pixels = msaa.render(gpu, view(frame), &[instance(1, msaa_ball, at)], &[]);
        if frame + 8 >= frames {
            taa_lines.push(Lines::of(&taa_pixels, radius_px));
            msaa_lines.push(Lines::of(&msaa_pixels, radius_px));
        }
    }
    let total = |lines: &[Lines], measure: fn(&Lines) -> f32| lines.iter().map(measure).sum::<f32>();
    let change = |lines: &[Lines]| {
        lines.windows(2).map(|pair| pair[1].change(&pair[0])).sum::<f32>() / (lines.len() - 1) as f32
    };
    LineComparison {
        strength: total(&taa_lines, Lines::strength) / total(&msaa_lines, Lines::strength),
        contrast: total(&taa_lines, Lines::contrast) / total(&msaa_lines, Lines::contrast),
        width: total(&taa_lines, Lines::width) / total(&msaa_lines, Lines::width),
        change: change(&taa_lines),
        msaa_change: change(&msaa_lines),
    }
}

/// The camera 4 m from the origin on a circle about the vertical axis, `angle` round it from +Z.
fn circling(angle: f32) -> CameraData {
    camera(Vec3::new(4.0 * angle.sin(), 0.0, 4.0 * angle.cos()), 45.0)
}

/// Turning at 3 rad/s at 60 fps, as the golden scenes `turntable-taa-motion` and
/// `outlines-turning-taa` do.
const TURN: f32 = 0.05;

/// A body turning about its own axis under TAA (M1 P1): its silhouette, and so its line, holds
/// still while the hull's points slide through it (`hulls_move_with_their_silhouette`). With
/// the hull points' velocity and characters' reactivity (M1 C4's resolve), the line smeared
/// across itself: 82% of 4× MSAA's contrast, 122% of its width, and its pixels changed 4.2 times
/// as much from frame to frame as MSAA's. With the line's velocity, drawn from the current frame
/// in motion and reprojected by whichever reprojection's history matches best: 94%, 115% and
/// 2.9 times.
#[test]
#[ignore = "needs a GPU"]
fn a_turning_body_keeps_its_lines_crisp_and_steady_under_temporal_aa() {
    let gpu = gpu();
    let still_camera = |_| camera(Vec3::new(0.0, 0.0, 4.0), 45.0);
    let lines =
        compare_lines(&gpu, |_| 1.0, 32, still_camera, |frame| Mat4::from_rotation_y(TURN * frame as f32));
    eprintln!("turning: {lines:?}");
    assert!((0.9..1.15).contains(&lines.strength), "{lines:?}");
    assert!(lines.contrast > 0.9 && lines.width < 1.19, "{lines:?}");
    assert!(lines.change < 3.6 * lines.msaa_change, "{lines:?}");
}

/// A camera circling a still body under TAA (M1 P1): the same sliding, from the camera's motion,
/// with the silhouette holding still on screen. Before: 82% of 4× MSAA's contrast, 122% of its
/// width and 4.0 times its change a frame; now 96%, 112% and 3.1 times.
#[test]
#[ignore = "needs a GPU"]
fn lines_hold_still_under_a_circling_camera() {
    let gpu = gpu();
    let lines = compare_lines(&gpu, |_| 1.0, 32, |frame| circling(TURN * frame as f32), |_| Mat4::IDENTITY);
    eprintln!("circling: {lines:?}");
    assert!((0.9..1.15).contains(&lines.strength), "{lines:?}");
    assert!(lines.contrast > 0.9 && lines.width < 1.19, "{lines:?}");
    assert!(lines.change < 3.6 * lines.msaa_change, "{lines:?}");
}

/// Upsampled (Desktop-High draws 1440 rows of a 4K output), a turning body's line keeps its width
/// in output pixels (M1 P1): the scene at two thirds of the output's size, against native 4×
/// MSAA. Before, it spread to 147% of MSAA's width at 69% of its contrast; now 118% and 88%.
/// Held still, the upsampled line measures 125% and 85% here, as before, and on the sandbox's
/// character at 4K 103% and 96%.
#[test]
#[ignore = "needs a GPU"]
fn upsampled_lines_keep_their_width_in_output_pixels() {
    let gpu = gpu();
    let still_camera = |_| camera(Vec3::new(0.0, 0.0, 4.0), 45.0);
    let lines =
        compare_lines(&gpu, |_| 0.667, 32, still_camera, |frame| Mat4::from_rotation_y(TURN * frame as f32));
    eprintln!("upsampled, turning: {lines:?}");
    assert!((0.9..1.15).contains(&lines.strength), "{lines:?}");
    assert!(lines.width < 1.3 && lines.contrast > 0.84, "{lines:?}");
}

/// A render-scale step (dynamic resolution) in mid-turn is a non-event for lines (M1 P1): the
/// history is at the output's size and outlives it (M1 R7), and a moving line is drawn from the
/// current frame at whatever size the scene is. Stepping from 1 to 0.85 halfway through the
/// measured frames, the line measures between its measures at either scale held (107% of 4×
/// MSAA's strength, against 105% and 108%; 93% of its contrast, against 92% and 94%) and
/// changes from frame to frame no more than at 0.85 (0.037 a pixel, against 0.043 and 0.033).
#[test]
#[ignore = "needs a GPU"]
fn lines_ride_out_a_render_scale_step() {
    let gpu = gpu();
    let still_camera = |_| camera(Vec3::new(0.0, 0.0, 4.0), 45.0);
    let turning = |frame: u32| Mat4::from_rotation_y(TURN * frame as f32);
    let native = compare_lines(&gpu, |_| 1.0, 32, still_camera, turning);
    let reduced = compare_lines(&gpu, |_| 0.85, 32, still_camera, turning);
    let stepped = compare_lines(&gpu, |frame| if frame < 28 { 1.0 } else { 0.85 }, 32, still_camera, turning);
    eprintln!("at 1: {native:?}\nat 0.85: {reduced:?}\nstepped from 1 to 0.85: {stepped:?}");
    for (name, found, at_native, at_reduced) in [
        ("strength", stepped.strength, native.strength, reduced.strength),
        ("contrast", stepped.contrast, native.contrast, reduced.contrast),
        ("width", stepped.width, native.width, reduced.width),
    ] {
        let (low, high) = (at_native.min(at_reduced), at_native.max(at_reduced));
        assert!(found > low * 0.97 && found < high * 1.03, "{name}: {found} against {low}–{high}");
    }
    assert!(stepped.change < 1.1 * native.change.max(reduced.change), "{stepped:?}");
}

/// Held still, lines converge and hold steady: the full history, unchanged by M1 P1 (91% of 4×
/// MSAA's contrast, 116% of its width, a pixel changing by 0.015 a frame).
#[test]
#[ignore = "needs a GPU"]
fn still_lines_hold_steady() {
    let gpu = gpu();
    let still_camera = |_| camera(Vec3::new(0.0, 0.0, 4.0), 45.0);
    let lines = compare_lines(&gpu, |_| 1.0, 40, still_camera, |_| Mat4::from_rotation_y(0.3));
    eprintln!("still: {lines:?}");
    assert!((0.95..1.15).contains(&lines.strength) && lines.contrast > 0.88, "{lines:?}");
    assert!(lines.change < 0.02, "{lines:?}");
}

/// Writes `pixels` as `<name>.png` into `SAPPHIRE_OUTLINE_DUMP`, if set.
fn dump(name: &str, pixels: &[[u8; 4]]) {
    let Some(dir) = std::env::var_os("SAPPHIRE_OUTLINE_DUMP") else { return };
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("create"));
    let mut encoder = png::Encoder::new(file, SIZE.0, SIZE.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    writer.write_image_data(pixels.as_flattened()).expect("write");
}
