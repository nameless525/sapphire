//! Outlines v2 (M1 C4) on the GPU: hulls that stay closed at hard edges and through skinning,
//! the width's distance curve, FOV normalization, model scale and LOD cutoff, view-space
//! flattening and push-back, lines lit in light, shade and at night, and the hull's own velocity,
//! under motion and under temporal AA. Run with `cargo test -p sapphire_render --tests --
//! --ignored`.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

use glam::{Affine3A, Mat4, Vec3};
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

#[test]
#[ignore = "needs a GPU"]
fn hulls_move_with_their_own_velocity() {
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
    // pixels inside it on the far side, which lies deeper than the silhouette: where it is, a
    // translation moves it by the inverse of that depth.
    let depth_of = |out: f32| {
        let meters_per_pixel = 2.0 * silhouette_depth(distance, radius) * tan / SIZE.1 as f32;
        let cosine = 1.0 - (line - out) * meters_per_pixel / radius;
        silhouette_depth(distance, radius) + radius * (1.0 - cosine * cosine).max(0.0).sqrt()
    };
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

    // Moved right by 5 cm: the hull with its surface, each part by its own depth.
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
        let (found, expected) = (vx * SIZE.0 as f32, shift(depth_of(out)));
        eprintln!(
            "moved: the hull {out} px out moved {found:.3} px, expected {expected:.3} (the silhouette {moved_px:.3})"
        );
        assert!((found - expected).abs() < 0.015 * expected && vy.abs() < 1e-4, "{found} against {expected}");
    }
    // Held still: exactly none.
    frames.render(&gpu, view, &[], &reads);
    let texels = velocity.texels(&gpu);
    for (x, _) in pixels(moved_px) {
        assert_eq!(rg16f(at(&texels, x, row)), [0.0, 0.0], "still again: none at x = {x}");
    }
    // The camera moving left moves everything right, by the same depths.
    let panned = looking(Vec3::new(-step, 0.0, distance), Vec3::new(-step, 0.0, 0.0), fov);
    frames.render(&gpu, panned, &[], &reads);
    let texels = velocity.texels(&gpu);
    for (x, out) in pixels(2.0 * moved_px) {
        let (found, expected) = (rg16f(at(&texels, x, row))[0] * SIZE.0 as f32, shift(depth_of(out)));
        eprintln!("panned: the hull {out} px out moved {found:.3} px, expected {expected:.3}");
        assert!((found - expected).abs() < 0.015 * expected, "{found} against {expected}");
    }

    // Turning about its own vertical axis by ω, the silhouette holds still while the surface
    // slides: the front's middle across at ωR, the sphere's points behind the silhouette the other
    // way at ωR sin β, β past it. The line shows the hull of those points, and moves with them, as
    // much slower and the other way as they are: closer to the silhouette's stillness than the
    // front's surface (R7's resolve borrowed that until hulls drew velocity), and still the
    // farther from it the deeper into the line.
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
    let meters_per_pixel = 2.0 * silhouette_depth(distance, radius) * tan / SIZE.1 as f32;
    let found: Vec<(f32, f32)> =
        pixels(0.0).map(|(x, out)| (out, rg16f(at(&texels, x, row))[0] * SIZE.0 as f32)).collect();
    for &[(out, left), (_, right)] in found.as_chunks::<2>().0 {
        let cosine = 1.0 - (line - out) * meters_per_pixel / radius;
        let sine = (1.0 - cosine * cosine).sqrt();
        let expected = -0.5 * focal_x * turn * radius * sine / depth_of(out) * SIZE.0 as f32;
        // The points' motion in depth shifts both sides alike by perspective; their mean is the
        // sliding alone.
        let mean = 0.5 * (left + right);
        eprintln!(
            "turning: the hull {out} px out moves {left:.3} and {right:.3} px, the sliding {expected:.3}"
        );
        assert!((mean - expected).abs() < 0.1 * expected.abs(), "{mean} against {expected}");
        assert!(left < 0.0 && right < 0.0 && left.max(right).abs() < 0.4 * front, "{left}, {right}: {front}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_turning_body_keeps_its_outline_under_temporal_aa() {
    let gpu = gpu();
    let taa_settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    let mut taa = Frames::new(&gpu, taa_settings);
    let mut msaa = Frames::msaa(&gpu);
    // 5.5 px at 1080 rows (the default), 2.75 here: beyond a 3 × 3 dilation's reach.
    let material = flat([0.9, 0.6, 0.3], 5.5);
    let (radius, distance, fov): (f32, f32, f32) = (1.0, 4.0, 45.0);
    let taa_ball = upload(&mut taa.renderer, &sphere(radius, &material, |n| n));
    let msaa_ball = upload(&mut msaa.renderer, &sphere(radius, &material, |n| n));
    let view = camera(Vec3::new(0.0, 0.0, distance), fov);
    let silhouette_px = (radius / (distance * distance - radius * radius).sqrt())
        / (fov.to_radians() * 0.5).tan()
        * SIZE.1 as f32
        / 2.0;
    let row = SIZE.1 / 2;
    // Each side's outline along the middle row: its total darkness against the sky.
    let darkness = |pixels: &[[u8; 4]], range: std::ops::Range<u32>| -> f32 {
        range.map(|x| (1.0 - linear(at(pixels, x, row)).y / 0.5).clamp(0.0, 1.0)).sum()
    };
    let center = SIZE.0 / 2;
    let left = center - silhouette_px as u32 - 12..center - silhouette_px as u32 + 2;
    let right = center + silhouette_px as u32 - 2..center + silhouette_px as u32 + 12;
    // Turning at 3 rad/s at 60 fps, as the golden scene `turntable-taa-motion` does, averaged over
    // a jitter cycle after 16 frames to settle.
    let (mut taa_dark, mut msaa_dark) = (0.0, 0.0);
    for frame in 0..24 {
        let turn = Mat4::from_rotation_y(0.05 * frame as f32);
        let taa_pixels = taa.render(&gpu, view, &[instance(1, taa_ball, turn)], &[]);
        let msaa_pixels = msaa.render(&gpu, view, &[instance(1, msaa_ball, turn)], &[]);
        if frame >= 16 {
            taa_dark += (darkness(&taa_pixels, left.clone()) + darkness(&taa_pixels, right.clone())) / 16.0;
            msaa_dark +=
                (darkness(&msaa_pixels, left.clone()) + darkness(&msaa_pixels, right.clone())) / 16.0;
        }
    }
    eprintln!("turning: TAA's outline darkness {taa_dark:.2} a side against 4× MSAA's {msaa_dark:.2}");
    assert!(taa_dark > 0.85 * msaa_dark && taa_dark < 1.15 * msaa_dark, "{taa_dark} against {msaa_dark}");
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
