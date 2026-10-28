//! Renderer tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, Rim,
};
use sapphire_assets::mesh::{
    Influences, Joint, JointTransform, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin,
    SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags, VertexInfluence,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    LightKind, LightPath, LocalLight, MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams,
    PrepassTargets, RenderError, RenderSettings, Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

fn target(gpu: &GpuContext, format: wgpu::TextureFormat, (width, height): (u32, u32)) -> wgpu::Texture {
    gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The vertices and indices of a UV sphere of `radius` around `center`.
fn sphere_geometry(center: Vec3, radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: (center + n * radius).to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
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
    (vertices, indices)
}

/// `material` cooked as the importer embeds a generated one: no textures.
fn cooked(material: &AnimeCharacterMaterial) -> Material {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(material, &swizzles).expect("a valid material")
}

/// A UV sphere of `radius` around `center` with one material, positions stored in `format`.
fn sphere_at(center: Vec3, radius: f32, material: AnimeCharacterMaterial, format: PositionFormat) -> Mesh {
    let (vertices, indices) = sphere_geometry(center, radius);
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", cooked(&material))];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, format)
}

/// A unit-radius UV sphere at the origin with one material.
fn sphere(material: AnimeCharacterMaterial) -> Mesh {
    sphere_at(Vec3::ZERO, 1.0, material, PositionFormat::Unorm16)
}

/// Uploads `mesh` the way the runtime loads one: cooked to bytes, then read in place, with the
/// materials it embeds.
fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("a valid mesh");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

/// A flat material: no shadow tone and no rim, black outline.
fn flat(rgb: [f32; 3], outline_px: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: outline_px, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// `material` with `rgb` as its shadow side's color, a multiplier on albedo.
fn shaded(mut material: AnimeCharacterMaterial, rgb: [f32; 3]) -> AnimeCharacterMaterial {
    material.regions[0].shade_color = rgb;
    material
}

/// Linear 0.5 grey, the same as the test sky: lit by the unit sun plus the default 12% grey
/// ambient.
const GREY: [f32; 3] = [0.5 / 1.06; 3];

fn white(outline_px: f32) -> AnimeCharacterMaterial {
    flat([1.0; 3], outline_px)
}

/// Fully lit in `rgb`, with no outline.
fn colored(rgb: [f32; 3]) -> AnimeCharacterMaterial {
    flat(rgb, 0.0)
}

fn instance(id: u64, mesh: MeshHandle, transform: Mat4) -> MeshInstance {
    MeshInstance { id: InstanceId(id), mesh, transform }
}

/// Post-processing that shows linear values below about 0.98 as they are, as M0 drew them:
/// a tone curve that is linear from black, a neutral grade, and no bloom or vignette.
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

/// A camera on +Z looking at the origin, with the unit sphere filling `fraction` of the
/// image height at any field of view.
fn framing_camera(fov_degrees: f32, (width, height): (u32, u32), fraction: f32) -> CameraData {
    let fov = fov_degrees.to_radians();
    let distance = 1.0 / (fraction * (fov * 0.5).tan());
    CameraData::look_at(
        Vec3::new(0.0, 0.0, distance),
        Vec3::ZERO,
        Vec3::Y,
        fov,
        width as f32 / height as f32,
        0.05,
    )
}

/// A renderer, the graph executor that runs its frames and the packets it draws.
struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    /// Copied into every packet: an even grey by default.
    sky: SkyGradient,
    /// Copied into every packet: 12% of the grey sky from every side by default.
    ambient: AmbientLight,
    /// Copied into every packet: [`passthrough`] by default.
    post: PostParams,
    /// Copied into every packet: along the view direction by default, so everything the camera
    /// sees is lit.
    sun: DirectionalLight,
    /// Copied into every packet: none by default.
    lights: Vec<LocalLight>,
    /// Sent with the next packet only: instance, palette and morph weights.
    poses: Vec<(u64, Vec<Affine3A>, Vec<f32>)>,
}

impl Frames {
    fn new(gpu: &GpuContext, format: wgpu::TextureFormat, msaa: Msaa) -> Result<Self, RenderError> {
        Self::with_settings(gpu, format, RenderSettings { msaa, ..RenderSettings::default() })
    }

    fn with_settings(
        gpu: &GpuContext,
        format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        Ok(Self {
            renderer: Renderer::new(gpu, format, settings)?,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            sky: SkyGradient {
                zenith: Vec3::splat(0.5),
                horizon: Vec3::splat(0.5),
                ground: Vec3::splat(0.5),
            },
            ambient: AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 },
            post: passthrough(),
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
            lights: Vec::new(),
            poses: Vec::new(),
        })
    }

    /// Prepares, records and submits one frame seen by `camera`, whose packet adds or changes
    /// the `changed` instances and removes the `removed` ones. Returns any validation error it
    /// raised.
    fn render(
        &mut self,
        gpu: &GpuContext,
        view: &wgpu::TextureView,
        camera: CameraData,
        changed: &[MeshInstance],
        removed: &[u64],
    ) -> Option<wgpu::Error> {
        self.render_reading(gpu, view, camera, changed, removed, &[])
    }

    /// [`Self::render`], copying the prepass targets `reads` names into readbacks, which
    /// [`Readback::texels`] then reads. Reading a target makes its prepass run.
    fn render_reading(
        &mut self,
        gpu: &GpuContext,
        view: &wgpu::TextureView,
        camera: CameraData,
        changed: &[MeshInstance],
        removed: &[u64],
        reads: &[PrepassRead<'_>],
    ) -> Option<wgpu::Error> {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.sun = self.sun;
        packet.lights.extend_from_slice(&self.lights);
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.post = self.post;
        packet.instances.changed.extend_from_slice(changed);
        packet.instances.removed.extend(removed.iter().copied().map(InstanceId));
        for (id, palette, weights) in self.poses.drain(..) {
            packet.push_skinned(InstanceId(id), palette, weights);
        }

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        if !reads.is_empty() {
            let reads: Vec<(TextureId, &Readback)> =
                reads.iter().map(|(target, readback)| (target(&targets), *readback)).collect();
            let mut pass = graph.add_pass("read prepasses").side_effect();
            for (texture, _) in &reads {
                pass = pass.read(*texture);
            }
            pass.record(move |ctx| {
                for (texture, readback) in reads {
                    readback.copy(ctx.encoder, ctx.texture(texture));
                }
            });
        }
        self.graph.execute(gpu, graph).expect("a valid graph");
        pollster::block_on(scope.pop())
    }
}

/// A prepass target to read back, picked from the frame's targets, and where it goes.
type PrepassRead<'a> = (fn(&PrepassTargets) -> TextureId, &'a Readback);

/// A buffer a 4-byte-per-texel texture is copied into, row by row.
struct Readback {
    buffer: wgpu::Buffer,
    size: (u32, u32),
    padded: u32,
}

impl Readback {
    fn new(gpu: &GpuContext, size: (u32, u32)) -> Self {
        let padded = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * size.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, size, padded }
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
                    rows_per_image: Some(self.size.1),
                },
            },
            texture.size(),
        );
    }

    /// The texels, as 4-byte words in rows top to bottom.
    fn texels(&self, gpu: &GpuContext) -> Vec<[u8; 4]> {
        self.buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let texels = {
            let mapped = self.buffer.get_mapped_range(..).expect("mapped");
            mapped
                .chunks_exact(self.padded as usize)
                .flat_map(|row| row[..self.size.0 as usize * 4].as_chunks::<4>().0.iter().copied())
                .collect()
        };
        self.buffer.unmap();
        texels
    }
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

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
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
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..width as usize * 4].as_chunks::<4>().0.iter().copied())
        .collect()
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Outline thickness in pixels where the middle row crosses the sphere's left silhouette.
/// The sphere and the background are the same grey, so each pixel's darkness relative to it
/// is its outline coverage, which counts antialiased edge pixels fractionally.
fn left_outline_width(pixels: &[[u8; 4]], (width, height): (u32, u32)) -> f32 {
    let row = &pixels[(height / 2 * width) as usize..][..width as usize];
    let background = srgb_to_linear(row[0][0]);
    row[..width as usize / 2].iter().map(|p| (1.0 - srgb_to_linear(p[0]) / background).clamp(0.0, 1.0)).sum()
}

#[test]
#[ignore = "needs a GPU"]
fn outline_width_is_independent_of_fov_and_resolution() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
    // A curve that holds the pixel width from half a meter to 20 m (outlines v2, M1 C4): the
    // sphere looks the same size at every field of view, so its effective distance stays within
    // it (2.6 to 4 m); `tests/outline.rs` follows the default curve's slope.
    let mut material = flat(GREY, 10.0);
    material.outline.curve.far.1 = 1.0;
    let mesh = upload(&mut frames.renderer, &sphere(material));
    // Added by the first frame; the scene keeps it for the rest.
    let mut add = vec![instance(0, mesh, Mat4::IDENTITY)];
    for size in [(960, 540), (1920, 1080)] {
        let texture = target(&gpu, FORMAT, size);
        let view = texture.create_view(&Default::default());
        // 10 px at 1080p.
        let expected = 10.0 * size.1 as f32 / 1080.0;
        for fov in [8.0, 38.0, 90.0] {
            let error =
                frames.render(&gpu, &view, framing_camera(fov, size, 0.6), &std::mem::take(&mut add), &[]);
            assert!(error.is_none(), "{error:?}");
            let width = left_outline_width(&read_back(&gpu, &texture), size);
            assert!(
                (width - expected).abs() < 0.75,
                "fov {fov}° at {size:?}: outline {width:.2} px, expected {expected:.1}"
            );
            eprintln!("fov {fov:>4}° at {size:?}: outline {width:.2} px (expected {expected:.1})");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn msaa_support_matches_device_validation() {
    let gpu = gpu();
    let depth = gpu.format_support(wgpu::TextureFormat::Depth32Float);
    // The main pass multisamples HDR scene color; the target only takes post's output.
    let scene = gpu.format_support(wgpu::TextureFormat::Rgba16Float);
    let mesh = sphere(white(4.0));
    for format in [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Rg11b10Ufloat,
        wgpu::TextureFormat::Rgb9e5Ufloat,
    ] {
        let color = gpu.format_support(format);
        for msaa in [Msaa::Off, Msaa::X2, Msaa::X4, Msaa::X8] {
            let samples = msaa.sample_count();
            let supported = color.renderable
                && scene.supports_samples(samples)
                && (samples == 1 || scene.multisample_resolve)
                && depth.supports_samples(samples);
            match Frames::new(&gpu, format, msaa) {
                Ok(mut frames) => {
                    assert!(supported, "{format:?} x{samples} accepted but unsupported");
                    let handle = upload(&mut frames.renderer, &mesh);
                    let texture = target(&gpu, format, (64, 64));
                    let view = texture.create_view(&Default::default());
                    let instances = [instance(0, handle, Mat4::IDENTITY)];
                    let camera = framing_camera(38.0, (64, 64), 0.5);
                    let error = frames.render(&gpu, &view, camera, &instances, &[]);
                    assert!(error.is_none(), "{format:?} x{samples}: {error:?}");
                }
                Err(RenderError::NotRenderable(_) | RenderError::UnsupportedMsaa { .. }) => {
                    assert!(!supported, "{format:?} x{samples} rejected but supported");
                }
                Err(other) => panic!("{format:?} x{samples}: {other}"),
            }
        }
    }
    // 2× is the case the old adapter-level check got wrong: it must work wherever reported.
    if scene.supports_samples(2) && depth.supports_samples(2) {
        Renderer::new(&gpu, FORMAT, RenderSettings { msaa: Msaa::X2, ..RenderSettings::default() })
            .expect("2x MSAA");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn internal_targets_follow_the_target_size() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let mesh = upload(&mut frames.renderer, &sphere(white(4.0)));
    let mut add = vec![instance(0, mesh, Mat4::IDENTITY)];
    frames.post = PostParams::default();
    for size in [(480, 270), (481, 270), (17, 1000), (480, 270)] {
        let texture = target(&gpu, FORMAT, size);
        let view = texture.create_view(&Default::default());
        let error =
            frames.render(&gpu, &view, framing_camera(38.0, size, 0.5), &std::mem::take(&mut add), &[]);
        assert!(error.is_none(), "{size:?}: {error:?}");
    }
    // Per size: MSAA color, depth and scene color, plus 9 bloom textures (5 down, 4 up; each up
    // level overlaps its down level). Returning to 480×270 reuses its set.
    let stats = frames.graph.stats();
    assert_eq!((stats.transient_slots, stats.pool.created, stats.pool.evicted), (12, 36, 0));
}

#[test]
#[ignore = "needs a GPU"]
fn mesh_relative_indices_draw_every_mesh() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    // Several meshes, so the last one starts deep inside the shared vertex buffer.
    for _ in 0..3 {
        upload(&mut frames.renderer, &sphere(colored([1.0, 0.0, 0.0])));
    }
    let green = upload(&mut frames.renderer, &sphere(colored([0.0, 1.0, 0.0])));
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let instances = [instance(0, green, Mat4::IDENTITY)];
    assert!(frames.render(&gpu, &view, framing_camera(38.0, size, 0.5), &instances, &[]).is_none());
    let center = read_back(&gpu, &texture)[(32 * 64 + 32) as usize];
    assert!(center[1] > 200 && center[0] < 80, "center pixel {center:?} should be the green sphere");

    let stats = frames.renderer.stats();
    assert_eq!((stats.draws, stats.instances), (1, 1));
    // 24 bytes a vertex over the three quantized streams.
    let sphere_bytes = {
        let mesh = sphere(white(0.0));
        (mesh.streams.len() * 24 + mesh.indices.len() * 4) as u64
    };
    assert_eq!(stats.mesh_pool_bytes, 4 * sphere_bytes);
}

/// The mean of each channel's absolute difference, in 8-bit steps, and how many pixels differ by
/// more than `threshold` steps in some channel.
fn difference(a: &[[u8; 4]], b: &[[u8; 4]], threshold: u8) -> (f64, usize) {
    let total: u64 = a.iter().zip(b).flat_map(|(a, b)| (0..3).map(|c| u64::from(a[c].abs_diff(b[c])))).sum();
    let differing = a.iter().zip(b).filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > threshold)).count();
    (total as f64 / (3 * a.len()) as f64, differing)
}

#[test]
#[ignore = "needs a GPU"]
fn quantized_meshes_match_their_f32_reference() {
    let gpu = gpu();
    let size = (320, 240);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    // Off-center and not unit-sized, so a wrong dequantization offset or scale shows.
    let (center, radius) = (Vec3::new(0.31, -0.17, 0.12), 0.83);
    let material = AnimeCharacterMaterial {
        base_color: [0.9, 0.55, 0.4, 1.0],
        regions: vec![CharacterRegion {
            shade_color: [0.6, 0.5, 0.8],
            outline_color: [0.1, 0.05, 0.1],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.4, ..Rim::default() },
        outline: Outline { width: 6.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let render = |format| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
        // Light from the upper left, so the two-tone terminator crosses the sphere.
        frames.sun = DirectionalLight {
            direction: Vec3::new(0.6, -0.5, -0.6).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        frames.sky = SkyGradient::default();
        frames.ambient = AmbientLight::default();
        let mesh = upload(&mut frames.renderer, &sphere_at(center, radius, material.clone(), format));
        let camera = CameraData::look_at(
            center + Vec3::new(0.0, 0.0, 3.0),
            center,
            Vec3::Y,
            38f32.to_radians(),
            size.0 as f32 / size.1 as f32,
            0.05,
        );
        let error = frames.render(&gpu, &view, camera, &[instance(0, mesh, Mat4::IDENTITY)], &[]);
        assert!(error.is_none(), "{format:?}: {error:?}");
        read_back(&gpu, &texture)
    };
    let reference = render(PositionFormat::Float32);
    let quantized = render(PositionFormat::Unorm16);
    let (mean, differing) = difference(&reference, &quantized, 8);
    eprintln!("quantized vs f32 positions: mean {mean:.4} steps, {differing} pixels differ by more than 8");
    // Positions are within 13 µm here, under 1/1000 of a pixel, so only a few silhouette pixels
    // whose MSAA samples sit right on an edge may flip (4 of 76 800 on an M5).
    assert!(mean < 0.05 && differing <= 16, "mean {mean} steps, {differing} pixels");
    assert!(reference.iter().any(|p| p[..3] != reference[0][..3]), "the sphere is in the picture");
}

/// A sphere skinned to two palette entries, the upper half bending with the second, and with one
/// morph target that would move every vertex half a meter, at full default weight.
fn skinned_sphere(material: AnimeCharacterMaterial) -> Mesh {
    let mut mesh = sphere(material);
    let influences: Vec<VertexInfluence> = (0..mesh.streams.len())
        .map(|v| {
            let y = mesh.streams.vertex(v).expect("in range").position[1];
            // Blends from the first entry below the equator to the second above it.
            let upper = ((y + 0.4) / 0.8).clamp(0.0, 1.0);
            let weights = sapphire_assets::mesh::quantize_weights([1.0 - upper, upper, 0.0, 0.0]);
            VertexInfluence { binds: [0, 1, 0, 0], weights }
        })
        .collect();
    let mut tip = Joint { name: "tip".into(), parent: Some(0), rest: JointTransform::IDENTITY };
    tip.rest.translation = [0.0, 1.0, 0.0];
    mesh.skin = Some(Skin {
        joints: vec![Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY }, tip],
        binds: vec![
            SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() },
            SkinBind {
                joint: 1,
                inverse_bind_matrix: Mat4::from_translation(Vec3::NEG_Y).to_cols_array_2d(),
            },
        ],
        influences: Influences::pack(&influences, 2),
    });
    let vertex_count = mesh.streams.len() as u32;
    mesh.morph_targets = vec![MorphTarget {
        name: "shift".into(),
        default_weight: 1.0,
        first_delta: 0,
        delta_count: vertex_count,
    }];
    mesh.morph_deltas = (0..vertex_count).map(|v| MorphDelta::new(v, [0.5, 0.0, 0.0], [0.0; 3])).collect();
    mesh
}

/// The second palette entry of [`skinned_sphere`] turned `angle` about Z around its joint at
/// y = 0 and lifted by `lift`.
fn bend(angle: f32, lift: f32) -> Vec<Affine3A> {
    let bent = Affine3A::from_rotation_translation(Quat::from_rotation_z(angle), Vec3::new(0.0, lift, 0.0));
    vec![Affine3A::IDENTITY, bent]
}

/// `mesh` (a skinned sphere) skinned on the CPU, as a static mesh with f32 positions.
fn skinned_on_the_cpu(mesh: &Mesh, palette: &[Affine3A], weights: &[f32]) -> Mesh {
    let skin = mesh.skin.as_ref().expect("skinned");
    let vertices: Vec<Vertex> = (0..mesh.streams.len())
        .map(|v| {
            let mut vertex = mesh.streams.vertex(v).expect("in range");
            let mut position = Vec3::from(vertex.position);
            for (target, morph) in mesh.morph_targets.iter().enumerate() {
                let first = morph.first_delta as usize;
                let deltas = &mesh.morph_deltas[first..first + morph.delta_count as usize];
                if let Some(delta) = deltas.iter().find(|delta| delta.vertex() == v as u32) {
                    position += weights.get(target).copied().unwrap_or(0.0) * Vec3::from(delta.position());
                }
            }
            let influence = skin.influences.get(v).expect("one per vertex");
            let mut blended = Affine3A::ZERO;
            for (&bind, weight) in influence.binds.iter().zip(influence.weights) {
                let weight = f32::from(weight) / 255.0;
                blended.matrix3 += palette[bind as usize].matrix3 * weight;
                blended.translation += palette[bind as usize].translation * weight;
            }
            vertex.position = blended.transform_point3(position).to_array();
            vertex.normal = (blended.matrix3 * Vec3::from(vertex.normal)).normalize().to_array();
            vertex.outline_normal =
                (blended.matrix3 * Vec3::from(vertex.outline_normal)).normalize().to_array();
            vertex
        })
        .collect();
    let lod = &mesh.lods[0];
    Mesh::from_vertices(
        &vertices,
        mesh.indices.clone(),
        lod.submeshes.clone(),
        mesh.materials.clone(),
        PositionFormat::Float32,
    )
}

#[test]
#[ignore = "needs a GPU"]
fn skinned_meshes_draw_in_bind_pose_until_posed() {
    let gpu = gpu();
    let size = (96, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let material = flat([0.9, 0.2, 0.15], 3.0);
    let plain = sphere(material.clone());
    let skinned = skinned_sphere(material);
    let render = |mesh: &Mesh, pose: Option<Vec<Affine3A>>| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
        let handle = upload(&mut frames.renderer, mesh);
        frames.poses.extend(pose.map(|palette| (0, palette, vec![])));
        let camera = framing_camera(38.0, size, 0.6);
        assert!(frames.render(&gpu, &view, camera, &[instance(0, handle, Mat4::IDENTITY)], &[]).is_none());
        (read_back(&gpu, &texture), frames.renderer.stats())
    };
    let (plain_pixels, plain_stats) = render(&plain, None);
    let (rest_pixels, rest_stats) = render(&skinned, None);
    assert!(rest_pixels == plain_pixels, "never posed: the bind pose, and the default morph weight unused");
    let (identity_pixels, identity_stats) = render(&skinned, Some(vec![Affine3A::IDENTITY; 2]));
    let (mean, differing) = difference(&plain_pixels, &identity_pixels, 8);
    eprintln!(
        "identity palette vs unskinned: mean {mean:.4} steps, {differing} pixels differ by more than 8"
    );
    // Positions come out exact; normals may round to a neighboring snorm16.
    assert!(mean < 0.05 && differing <= 4, "mean {mean} steps, {differing} pixels");

    // The skin and morph target join the pool: 8 bytes of influences and 4 of morph offsets per
    // vertex, plus an offset and 16 bytes per delta.
    let vertices = plain.streams.len() as u64;
    assert_eq!(
        rest_stats.mesh_pool_bytes,
        plain_stats.mesh_pool_bytes + 8 * vertices + 4 * (vertices + 1) + 16 * vertices
    );
    let (rest, identity) = (rest_stats.skinning, identity_stats.skinning);
    assert_eq!((rest.instances, rest.skinned, rest.vertices, rest.palette_bytes), (1, 1, vertices as u32, 0));
    assert_eq!((identity.skinned, identity.palette_bytes, identity.output_bytes), (1, 96, 32 * vertices));
    assert_eq!(plain_stats.skinning, sapphire_render::SkinningStats::default(), "static meshes cost nothing");
}

#[test]
#[ignore = "needs a GPU"]
fn posed_skins_and_morphs_match_the_same_mesh_deformed_on_the_cpu() {
    let gpu = gpu();
    let size = (160, 120);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let material = AnimeCharacterMaterial {
        base_color: [0.9, 0.55, 0.4, 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: [0.6, 0.5, 0.8],
            outline_color: [0.1, 0.05, 0.1],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.4, ..Rim::default() },
        outline: Outline { width: 3.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let skinned = skinned_sphere(material.clone());
    let render = |mesh: &Mesh, pose: Option<(Vec<Affine3A>, Vec<f32>)>| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
        frames.sun =
            DirectionalLight { direction: Vec3::new(0.6, -0.5, -0.6), color: Vec3::ONE, intensity: 1.0 };
        let handle = upload(&mut frames.renderer, mesh);
        frames.poses.extend(pose.map(|(palette, weights)| (0, palette, weights)));
        let camera = framing_camera(38.0, size, 0.45);
        assert!(frames.render(&gpu, &view, camera, &[instance(0, handle, Mat4::IDENTITY)], &[]).is_none());
        read_back(&gpu, &texture)
    };
    for (palette, weights) in
        [(bend(0.6, 0.1), vec![0.0]), (bend(-0.3, 0.0), vec![0.4]), (bend(0.0, 0.0), vec![-0.25])]
    {
        let gpu_pixels = render(&skinned, Some((palette.clone(), weights.clone())));
        let cpu_pixels = render(&skinned_on_the_cpu(&skinned, &palette, &weights), None);
        let (mean, differing) = difference(&cpu_pixels, &gpu_pixels, 8);
        eprintln!(
            "{weights:?}: GPU vs CPU deformation: mean {mean:.4} steps, {differing} pixels differ by more than 8"
        );
        assert!(mean < 0.1 && differing <= 30, "mean {mean} steps, {differing} pixels");
        let bind = render(&sphere(material.clone()), None);
        assert!(difference(&bind, &gpu_pixels, 8).1 > 200, "the pose moved the sphere");
    }
}

/// A camera-facing square of side 0.8 at the origin, in the XY plane.
fn square(material: AnimeCharacterMaterial) -> Mesh {
    let vertex = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [0.0; 2],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [vertex(-0.4, -0.4), vertex(0.4, -0.4), vertex(0.4, 0.4), vertex(-0.4, 0.4)];
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("square", cooked(&material))];
    Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Unorm16)
}

#[test]
#[ignore = "needs a GPU"]
fn velocity_follows_transforms_skins_and_the_camera() {
    let gpu = gpu();
    let size = (200, 100);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let material = flat([0.5; 3], 0.0);
    let still = upload(&mut frames.renderer, &square(material.clone()));
    let mut skinned = square(material);
    skinned.skin = Some(Skin {
        joints: vec![Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY }],
        binds: vec![SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() }],
        influences: Influences::pack(&[VertexInfluence { binds: [0; 4], weights: [255, 0, 0, 0] }; 4], 1),
    });
    let skinned = upload(&mut frames.renderer, &skinned);
    let fov = 60f32.to_radians();
    let aspect = size.0 as f32 / size.1 as f32;
    let look = |x: f32| {
        CameraData::look_at(Vec3::new(x, 0.0, 3.0), Vec3::new(x, 0.0, 0.0), Vec3::Y, fov, aspect, 0.05)
    };
    let velocity = Readback::new(&gpu, size);
    let reads: [PrepassRead<'_>; 1] = [(|targets| targets.velocity, &velocity)];
    // The velocity at a world point on the z = 0 plane, seen from `camera`.
    let texel = |camera: &CameraData, point: Vec3, texels: &[[u8; 4]]| {
        let clip = camera.proj * camera.view * point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let x = ((ndc.x + 1.0) * 0.5 * size.0 as f32) as u32;
        let y = ((1.0 - ndc.y) * 0.5 * size.1 as f32) as u32;
        rg16f(texels[(y * size.0 + x) as usize])
    };
    let near = |found: [f32; 2], expected: [f32; 2]| (0..2).all(|i| (found[i] - expected[i]).abs() < 1e-4);
    // A world motion of `distance` at view depth 3 moves `focal × distance / 3` in NDC, half that
    // in texture coordinates.
    let (focal_x, focal_y) = (1.0 / ((fov * 0.5).tan() * aspect), 1.0 / (fov * 0.5).tan());

    let static_at = |x: f32| instance(1, still, Mat4::from_translation(Vec3::new(x, 0.0, 0.0)));
    let skinned_at = instance(2, skinned, Mat4::from_translation(Vec3::new(0.6, 0.0, 0.0)));
    let camera = look(0.0);
    assert!(
        frames.render_reading(&gpu, &view, camera, &[static_at(-0.6), skinned_at], &[], &reads).is_none()
    );
    let texels = velocity.texels(&gpu);
    assert!(texels.iter().all(|&texel| rg16f(texel) == [0.0, 0.0]), "nothing has moved yet");

    // The static square moves right, the skinned one's palette lifts it.
    frames.poses.push((2, vec![Affine3A::from_translation(Vec3::new(0.0, 0.05, 0.0))], vec![]));
    assert!(frames.render_reading(&gpu, &view, camera, &[static_at(-0.5)], &[], &reads).is_none());
    let texels = velocity.texels(&gpu);
    let moved = texel(&camera, Vec3::new(-0.5, 0.0, 0.0), &texels);
    let lifted = texel(&camera, Vec3::new(0.6, 0.05, 0.0), &texels);
    eprintln!("moved {moved:?}, lifted {lifted:?}");
    assert!(near(moved, [0.5 * focal_x * 0.1 / 3.0, 0.0]), "{moved:?}");
    assert!(near(lifted, [0.0, -0.5 * focal_y * 0.05 / 3.0]), "{lifted:?}");
    assert_eq!(rg16f(texels[0]), [0.0, 0.0], "the background");

    // Both hold still: no motion.
    assert!(frames.render_reading(&gpu, &view, camera, &[], &[], &reads).is_none());
    let texels = velocity.texels(&gpu);
    assert_eq!(texel(&camera, Vec3::new(-0.5, 0.0, 0.0), &texels), [0.0, 0.0]);
    assert_eq!(texel(&camera, Vec3::new(0.6, 0.05, 0.0), &texels), [0.0, 0.0]);

    // The camera moves right: everything moves left.
    let moved_camera = look(0.1);
    assert!(frames.render_reading(&gpu, &view, moved_camera, &[], &[], &reads).is_none());
    let texels = velocity.texels(&gpu);
    for point in [Vec3::new(-0.5, 0.0, 0.0), Vec3::new(0.6, 0.05, 0.0)] {
        let found = texel(&moved_camera, point, &texels);
        assert!(near(found, [-0.5 * focal_x * 0.1 / 3.0, 0.0]), "{point}: {found:?}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn skinning_and_prepasses_run_at_any_size_and_msaa() {
    let gpu = gpu();
    let material = white(3.0);
    let mut morph_only = sphere(material.clone());
    morph_only.morph_targets =
        vec![MorphTarget { name: "dent".into(), default_weight: 0.0, first_delta: 0, delta_count: 2 }];
    morph_only.morph_deltas =
        vec![MorphDelta::new(3, [0.0, -0.2, 0.0], [0.0; 3]), MorphDelta::new(40, [0.1, 0.0, 0.0], [0.0; 3])];
    for msaa in [Msaa::Off, Msaa::X4] {
        let mut frames = Frames::new(&gpu, FORMAT, msaa).expect("renderer");
        let meshes = [sphere(material.clone()), skinned_sphere(material.clone()), morph_only.clone()]
            .map(|mesh| upload(&mut frames.renderer, &mesh));
        let mut add: Vec<MeshInstance> = meshes
            .iter()
            .enumerate()
            .map(|(index, &mesh)| {
                instance(index as u64, mesh, Mat4::from_translation(Vec3::new(index as f32 - 1.0, 0.0, 0.0)))
            })
            .collect();
        for (frame, size) in [(1, 1), (17, 1000), (481, 270), (64, 64)].into_iter().enumerate() {
            let texture = target(&gpu, FORMAT, size);
            let view = texture.create_view(&Default::default());
            let readbacks = [(); 4].map(|()| Readback::new(&gpu, size));
            let reads: [PrepassRead<'_>; 4] = [
                (|targets| targets.character_depth, &readbacks[0]),
                (|targets| targets.depth, &readbacks[1]),
                (|targets| targets.normals, &readbacks[2]),
                (|targets| targets.velocity, &readbacks[3]),
            ];
            frames.poses.push((1, bend(0.1 * frame as f32, 0.0), vec![0.5]));
            frames.poses.push((2, vec![], vec![frame as f32 * 0.3]));
            let camera = framing_camera(38.0, size, 0.3);
            let error = frames.render_reading(&gpu, &view, camera, &std::mem::take(&mut add), &[], &reads);
            assert!(error.is_none(), "{msaa:?} at {size:?}: {error:?}");
            // Uploads, skinning, both prepasses, the sky cube's face, main, the composite (no
            // bloom) and the readback.
            let expected = 8;
            assert_eq!(frames.graph.stats().passes, expected, "{msaa:?} at {size:?}");
            let stats = frames.renderer.stats().skinning;
            assert_eq!((stats.instances, stats.skinned, stats.ignored_poses), (2, 2, 0));
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn skinned_outputs_survive_growth_and_bad_poses_are_ignored() {
    let gpu = gpu();
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let mesh = upload(&mut frames.renderer, &skinned_sphere(white(3.0)));
    let still = upload(&mut frames.renderer, &sphere(white(3.0)));
    let camera = framing_camera(38.0, size, 0.5);
    let far_away = |ids: std::ops::Range<u64>| {
        ids.map(|id| instance(id, mesh, Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0))))
            .collect::<Vec<_>>()
    };

    frames.poses.push((0, bend(0.7, 0.2), vec![0.3]));
    assert!(
        frames
            .render(
                &gpu,
                &view,
                camera,
                &[instance(0, mesh, Mat4::IDENTITY), instance(99, still, Mat4::ZERO)],
                &[]
            )
            .is_none()
    );
    let posed = read_back(&gpu, &texture);
    let before = frames.renderer.stats();

    // Thirty more characters grow the output buffer while instance 0 holds its pose.
    assert!(frames.render(&gpu, &view, camera, &far_away(1..31), &[]).is_none());
    let grown = frames.renderer.stats();
    assert!(grown.buffer_growths > before.buffer_growths, "{grown:?}");
    assert_eq!((grown.skinning.instances, grown.skinning.skinned), (31, 30), "only the new ones skin");
    assert!(read_back(&gpu, &texture) == posed, "instance 0's outputs moved with the buffer");

    // Removed and added again: the freed outputs are reused.
    let bytes = grown.skinning.output_bytes;
    assert!(frames.render(&gpu, &view, camera, &[], &(1..31).collect::<Vec<_>>()).is_none());
    assert_eq!(frames.renderer.stats().skinning.instances, 1);
    assert!(frames.render(&gpu, &view, camera, &far_away(31..61), &[]).is_none());
    assert_eq!(frames.renderer.stats().skinning.output_bytes, bytes);

    // Poses that can't apply change nothing: an unknown instance, a static one, a palette of the
    // wrong length, and a NaN weight.
    frames.poses = vec![
        (500, bend(0.1, 0.0), vec![]),
        (99, bend(0.1, 0.0), vec![]),
        (0, vec![Affine3A::IDENTITY], vec![]),
        (0, bend(0.1, 0.0), vec![f32::NAN]),
    ];
    assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
    let stats = frames.renderer.stats().skinning;
    assert_eq!((stats.ignored_poses, stats.skinned), (4, 0));
    assert!(read_back(&gpu, &texture) == posed);
}

/// A pixel of `rgb`'s hue: one channel high, the others low.
fn is(pixel: [u8; 4], rgb: [bool; 3]) -> bool {
    (0..3).all(|c| if rgb[c] { pixel[c] > 200 } else { pixel[c] < 80 })
}

const RED: [bool; 3] = [true, false, false];
const GREEN: [bool; 3] = [false, true, false];

#[test]
#[ignore = "needs a GPU"]
fn instances_persist_and_update_from_deltas() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let red = upload(&mut frames.renderer, &sphere(colored([1.0, 0.0, 0.0])));
    let green = upload(&mut frames.renderer, &sphere(colored([0.0, 1.0, 0.0])));
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    // At 60° from 4 m, world x and y of ±1 land 14 pixels from the center: pixel 18 or 46.
    let camera =
        CameraData::look_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y, 60f32.to_radians(), 1.0, 0.05);
    let at = |x: f32, y: f32| {
        Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, Vec3::new(x, y, 0.0))
    };
    let (left, right, up_left) = ((18, 32), (46, 32), (18, 18));
    let mut frame = |changed: &[MeshInstance], removed: &[u64]| {
        assert!(frames.render(&gpu, &view, camera, changed, removed).is_none());
        let pixels = read_back(&gpu, &texture);
        let stats = frames.renderer.stats();
        (move |(x, y): (usize, usize)| pixels[y * 64 + x], stats)
    };

    let (pixel, stats) = frame(&[instance(1, red, at(-1.0, 0.0)), instance(2, green, at(1.0, 0.0))], &[]);
    assert!(is(pixel(left), RED) && is(pixel(right), GREEN));
    assert_eq!((stats.instances, stats.draws, stats.changed_instances, stats.draw_list_builds), (2, 2, 2, 1));

    // Nothing changed: both still draw, nothing is rebuilt.
    let (pixel, stats) = frame(&[], &[]);
    assert!(is(pixel(left), RED) && is(pixel(right), GREEN));
    assert_eq!((stats.instances, stats.changed_instances, stats.draw_list_builds), (2, 0, 1));

    // A move uploads one instance and keeps the draw list.
    let (pixel, stats) = frame(&[instance(1, red, at(-1.0, 1.0))], &[]);
    assert!(is(pixel(up_left), RED) && !is(pixel(left), RED) && is(pixel(right), GREEN));
    assert_eq!((stats.changed_instances, stats.draw_list_builds), (1, 1));

    // A removal and an addition each rebuild it; the newcomer reuses the freed slot.
    let (pixel, stats) = frame(&[], &[2]);
    assert!(is(pixel(up_left), RED) && !is(pixel(right), GREEN));
    assert_eq!((stats.instances, stats.draws, stats.draw_list_builds), (1, 1, 2));
    let (pixel, stats) = frame(&[instance(3, green, at(1.0, 0.0))], &[]);
    assert!(is(pixel(up_left), RED) && is(pixel(right), GREEN));
    assert_eq!((stats.instances, stats.draw_list_builds), (2, 3));

    // So does a mesh change.
    let (pixel, stats) = frame(&[instance(1, green, at(-1.0, 1.0))], &[]);
    assert!(is(pixel(up_left), GREEN));
    assert_eq!(stats.draw_list_builds, 4);
}

/// A red sphere against the default sky at 96×64, for comparing post settings frame by frame.
struct PostScene {
    frames: Frames,
    texture: wgpu::Texture,
    /// The sphere until the first frame adds it.
    add: Vec<MeshInstance>,
}

impl PostScene {
    const SIZE: (u32, u32) = (96, 64);

    fn new(gpu: &GpuContext) -> Self {
        let mut frames = Frames::new(gpu, FORMAT, Msaa::default()).expect("renderer");
        let mesh = upload(&mut frames.renderer, &sphere(flat([0.9, 0.2, 0.15], 3.0)));
        frames.sky = SkyGradient::default();
        frames.ambient = AmbientLight::default();
        Self {
            frames,
            texture: target(gpu, FORMAT, Self::SIZE),
            add: vec![instance(0, mesh, Mat4::IDENTITY)],
        }
    }

    /// Renders with `post` and reads the frame back.
    fn render(&mut self, gpu: &GpuContext, post: PostParams) -> Vec<[u8; 4]> {
        self.frames.post = post;
        let view = self.texture.create_view(&Default::default());
        let camera = framing_camera(38.0, Self::SIZE, 0.6);
        let error = self.frames.render(gpu, &view, camera, &std::mem::take(&mut self.add), &[]);
        assert!(error.is_none(), "{error:?}");
        read_back(gpu, &self.texture)
    }
}

/// A grade strong enough to change every pixel of the test scene.
fn strong_grade(temperature: f32) -> ColorGrade {
    ColorGrade {
        temperature,
        saturation: 1.6,
        contrast: 1.3,
        lift: Vec3::splat(0.04),
        gain: Vec3::new(1.05, 0.95, 1.0),
        ..ColorGrade::NEUTRAL
    }
}

#[test]
#[ignore = "needs a GPU"]
fn values_above_one_survive_into_bloom() {
    let gpu = gpu();
    let size = (128, 128);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let glow_around = |brightness: f32| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
        let mesh = upload(&mut frames.renderer, &sphere(flat([brightness; 3], 0.0)));
        frames.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        frames.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
        // A hard threshold at 1: anything clamped to 1 on the way would add no bloom at all.
        frames.post.bloom = BloomParams { threshold: 1.0, knee: 0.0, intensity: 1.0, scatter: 0.7 };
        // A sphere 26 px across on black.
        let camera = framing_camera(38.0, size, 0.2);
        let error = frames.render(&gpu, &view, camera, &[instance(0, mesh, Mat4::IDENTITY)], &[]);
        assert!(error.is_none(), "{error:?}");
        let pixels = read_back(&gpu, &texture);
        let at = |x: u32, y: u32| pixels[(y * size.0 + x) as usize];
        assert!(at(64, 64)[0] > 200, "the sphere itself is bright: {:?}", at(64, 64));
        // Twice the sphere's radius out, on all four sides.
        [at(64 + 26, 64), at(64 - 26, 64), at(64, 64 + 26), at(64, 64 - 26)].map(|pixel| pixel[0])
    };
    assert_eq!(glow_around(0.9), [0; 4], "below the threshold: no bloom");
    let halo = glow_around(4.0);
    assert!(halo.iter().all(|&red| red >= 8), "an HDR spot must glow around itself: {halo:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn growing_the_instance_buffer_keeps_uploaded_instances() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let green = upload(&mut frames.renderer, &sphere(colored([0.0, 1.0, 0.0])));
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, size, 0.5);
    // Zero-scale instances draw nothing; they only fill slots.
    let hidden =
        |ids: std::ops::Range<u64>| ids.map(|id| instance(id, green, Mat4::ZERO)).collect::<Vec<_>>();

    let mut first = vec![instance(0, green, Mat4::IDENTITY)];
    first.extend(hidden(1..200));
    assert!(frames.render(&gpu, &view, camera, &first, &[]).is_none());
    let growths = frames.renderer.stats().buffer_growths;
    // Past the initial 256 slots: the buffer grows while slot 0 has no pending upload.
    assert!(frames.render(&gpu, &view, camera, &hidden(200..400), &[]).is_none());
    assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());

    let stats = frames.renderer.stats();
    assert!(stats.buffer_growths > growths, "{stats:?}");
    assert_eq!(stats.instances, 400);
    let center = read_back(&gpu, &texture)[(32 * 64 + 32) as usize];
    assert!(is(center, GREEN), "instance 0 survived the growth: center {center:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn filter_strength_zero_renders_the_neutral_grade() {
    let gpu = gpu();
    let mut scene = PostScene::new(&gpu);
    let mut render = |grade, filter_strength| {
        scene.render(&gpu, PostParams { grade_a: grade, filter_strength, ..PostParams::default() })
    };
    let neutral = render(ColorGrade::NEUTRAL, 1.0);
    let off = render(strong_grade(0.5), 0.0);
    let half = render(strong_grade(0.5), 0.5);
    let full = render(strong_grade(0.5), 1.0);
    assert!(off == neutral, "strength 0 is exactly the neutral grade");
    assert!(full != neutral && half != neutral && half != full, "the slider scales the grade");
}

#[test]
#[ignore = "needs a GPU"]
fn blending_two_luts_at_weights_zero_and_one_matches_each_lut_alone() {
    let gpu = gpu();
    let mut scene = PostScene::new(&gpu);
    let (warm, cool) = (strong_grade(0.6), strong_grade(-0.6));
    let mut render = |grade_a, grade_b, grade_blend| {
        let pixels =
            scene.render(&gpu, PostParams { grade_a, grade_b, grade_blend, ..PostParams::default() });
        (pixels, scene.frames.renderer.stats().lut_bakes)
    };
    let (warm_alone, bakes) = render(warm, ColorGrade::NEUTRAL, 0.0);
    assert_eq!(bakes, 1, "an unused grade B is not baked");
    let (cool_alone, bakes) = render(cool, ColorGrade::NEUTRAL, 0.0);
    assert_eq!(bakes, 2);
    let (at_zero, bakes) = render(warm, cool, 0.0);
    assert!(at_zero == warm_alone, "weight 0 is LUT A alone");
    let (at_one, _) = render(warm, cool, 1.0);
    assert!(at_one == cool_alone, "weight 1 is LUT B alone");
    let (halfway, after) = render(warm, cool, 0.5);
    assert!(halfway != warm_alone && halfway != cool_alone && warm_alone != cool_alone);
    assert_eq!(after, bakes, "both LUTs stayed baked while the blend moved");
}

#[test]
#[ignore = "needs a GPU"]
fn post_runs_at_any_size_bloom_length_and_msaa() {
    let gpu = gpu();
    let mesh = sphere(white(4.0));
    for msaa in [Msaa::Off, Msaa::X4] {
        for bloom_levels in [0, 1, 5, sapphire_render::MAX_BLOOM_LEVELS] {
            let settings = RenderSettings { msaa, bloom_levels, ..RenderSettings::default() };
            let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
            let handle = upload(&mut frames.renderer, &mesh);
            frames.post =
                PostParams { grade_b: strong_grade(0.3), grade_blend: 0.5, ..PostParams::default() };
            let mut add = vec![instance(0, handle, Mat4::IDENTITY)];
            for size in [(1, 1), (3, 2), (17, 1000), (641, 359)] {
                let texture = target(&gpu, FORMAT, size);
                let view = texture.create_view(&Default::default());
                let camera = framing_camera(38.0, size, 0.5);
                let error = frames.render(&gpu, &view, camera, &std::mem::take(&mut add), &[]);
                assert!(error.is_none(), "{settings:?} at {size:?}: {error:?}");
                // Uploads, the sky cube's face, the character prepass, main, the bloom chain (a
                // prefilter, then a down and an up pass per further level) and the composite.
                let expected = 5 + if bloom_levels > 0 { 2 * bloom_levels - 1 } else { 0 };
                assert_eq!(frames.graph.stats().passes, expected, "{settings:?}");
            }
        }
    }
}

/// White on the lit side and half as bright in shadow, with no rim or outline: the plainest
/// two-tone material, so each light's split shows as a factor of two.
fn two_tone() -> AnimeCharacterMaterial {
    shaded(flat([1.0; 3], 0.0), [0.5; 3])
}

fn point_light(position: Vec3, color: Vec3, intensity: f32, range: f32) -> LocalLight {
    LocalLight { position, color, intensity, range, kind: LightKind::Point }
}

/// Linear red along the middle row.
fn middle_row_red(pixels: &[[u8; 4]], (width, height): (u32, u32)) -> Vec<f32> {
    let row = &pixels[(height / 2 * width) as usize..][..width as usize];
    row.iter().map(|pixel| srgb_to_linear(pixel[0])).collect()
}

#[test]
#[ignore = "needs a GPU"]
fn a_local_light_shifts_the_light_split() {
    let gpu = gpu();
    let size = (160, 160);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
    let mesh = upload(&mut frames.renderer, &sphere(two_tone()));
    frames.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
    frames.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
    // The sun from the left lights the left half: 0.5 lit, 0.25 in shadow.
    frames.sun = DirectionalLight { direction: Vec3::X, color: Vec3::ONE, intensity: 0.5 };
    let camera = framing_camera(38.0, size, 0.6);
    let mut add = vec![instance(0, mesh, Mat4::IDENTITY)];
    let mut render = |frames: &mut Frames| {
        let error = frames.render(&gpu, &view, camera, &std::mem::take(&mut add), &[]);
        assert!(error.is_none(), "{error:?}");
        read_back(&gpu, &texture)
    };
    let unlit = render(&mut frames);
    // A red light to the right of the sphere, on the side the sun leaves in shadow.
    let light = Vec3::new(2.0, 0.0, 0.3);
    frames.lights = vec![point_light(light, Vec3::X, 0.6, 4.0)];
    let lit = render(&mut frames);
    assert_eq!(frames.renderer.stats().lights.drawn, 1);

    // Where the light's own terminator crosses the middle row: the visible point X of the unit
    // sphere with N·(P − X) = 0, that is P·X = 1, found by bisection and projected.
    let (mut low, mut high) = (0.0f32, std::f32::consts::FRAC_PI_2);
    for _ in 0..40 {
        let middle = (low + high) * 0.5;
        if light.dot(Vec3::new(middle.sin(), 0.0, middle.cos())) < 1.0 { low = middle } else { high = middle }
    }
    let terminator = Vec3::new(low.sin(), 0.0, low.cos());
    let clip = camera.proj * camera.view * terminator.extend(1.0);
    let terminator_x = (clip.x / clip.w + 1.0) * 0.5 * size.0 as f32;

    let (before, after) = (middle_row_red(&unlit, size), middle_row_red(&lit, size));
    let center = size.0 as usize / 2;
    // The sphere's right half, inside its outline-free silhouette.
    let right: Vec<usize> = (center + 3..size.0 as usize).take_while(|&x| before[x] > 0.1).collect();
    assert!(right.len() > 30, "the sphere covers the right half: {}", right.len());
    let shade = before[right[0]];
    assert!((shade - 0.25).abs() < 0.03, "the sun's shadow tone: {shade}");
    assert!(right.iter().all(|&x| (before[x] - shade).abs() < 0.01), "without the light, one flat tone");

    // With it, the red the light adds jumps across its terminator within two pixels: the
    // light's shadow tone is half its lit tone. A Lambert term would rise by about 1/40 of
    // its range per pixel here.
    let added: Vec<f32> = (0..size.0 as usize).map(|x| after[x] - before[x]).collect();
    let (jump, at) = right[..right.len() - 2]
        .iter()
        .map(|&x| (added[x + 2] - added[x], x + 1))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("pixels");
    eprintln!(
        "local split at x = {at} (expected {terminator_x:.1}): +{jump:.3} over 2 px, {:.3} → {:.3} red added",
        added[at - 2],
        added[at + 3]
    );
    assert!(jump > 0.35 * added[at + 1], "a crisp split: +{jump:.3} against {:.3}", added[at + 1]);
    assert!((at as f32 + 0.5 - terminator_x).abs() < 3.0, "the split at {at}, expected {terminator_x:.1}");
    // Beyond it the light pushes the sun's shadow into a lit tone brighter than the sun's own.
    let pushed = after[at + 3];
    assert!(pushed > before[center - 8], "{pushed} against the sun-lit {}", before[center - 8]);
    // Short of it the light still tints, in its own shadow tone: the material's half.
    let ratio = added[at - 2] / added[at + 3];
    assert!((0.35..0.65).contains(&ratio), "shadow tone / lit tone: {ratio}");
    // Red light adds no green.
    let green = |pixels: &[[u8; 4]], x: usize| pixels[(size.1 / 2 * size.0) as usize + x][1];
    assert!(right.iter().all(|&x| green(&unlit, x).abs_diff(green(&lit, x)) <= 1));
}

/// Three spheres in a row and four lights around them: two points and two spots.
fn four_lights_scene(frames: &mut Frames) -> Vec<MeshInstance> {
    let colors = [[0.9, 0.85, 0.8], [0.6, 0.8, 0.95], [0.95, 0.7, 0.6]];
    let instances = colors
        .into_iter()
        .enumerate()
        .map(|(index, rgb)| {
            let mesh = upload(&mut frames.renderer, &sphere(shaded(flat(rgb, 3.0), [0.55, 0.5, 0.65])));
            let x = (index as f32 - 1.0) * 1.4;
            instance(
                index as u64,
                mesh,
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.6),
                    Quat::IDENTITY,
                    Vec3::new(x, 0.0, 0.0),
                ),
            )
        })
        .collect();
    let spot = |position: Vec3, target: Vec3, color: Vec3| LocalLight {
        position,
        color,
        intensity: 1.5,
        range: 5.0,
        kind: LightKind::Spot { direction: target - position, inner_angle: 0.15, outer_angle: 0.3 },
    };
    frames.lights = vec![
        point_light(Vec3::new(-1.0, 0.8, 0.8), Vec3::new(1.0, 0.4, 0.1), 1.2, 1.8),
        point_light(Vec3::new(1.6, -0.5, -0.5), Vec3::new(0.2, 0.7, 1.0), 2.0, 1.5),
        spot(Vec3::new(0.0, 2.5, 1.5), Vec3::ZERO, Vec3::new(1.0, 0.3, 0.7)),
        spot(Vec3::new(2.5, 1.0, 2.0), Vec3::new(1.4, 0.0, 0.0), Vec3::ONE),
    ];
    frames.sun = DirectionalLight { direction: Vec3::new(0.5, -0.6, -0.6), color: Vec3::ONE, intensity: 0.8 };
    instances
}

#[test]
#[ignore = "needs a GPU"]
fn clustered_and_per_object_paths_draw_four_lights_alike() {
    let gpu = gpu();
    let size = (256, 144);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = CameraData::look_at(Vec3::new(0.5, 1.0, 4.5), Vec3::ZERO, Vec3::Y, 0.8, 16.0 / 9.0, 0.05);
    let render = |light_path, lights: bool| {
        let settings = RenderSettings { light_path, ..RenderSettings::default() };
        let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
        frames.post = PostParams::default();
        frames.sky = SkyGradient::default();
        frames.ambient = AmbientLight::default();
        let instances = four_lights_scene(&mut frames);
        if !lights {
            frames.lights.clear();
        }
        let error = frames.render(&gpu, &view, camera, &instances, &[]);
        assert!(error.is_none(), "{light_path:?}: {error:?}");
        (read_back(&gpu, &texture), frames.renderer.stats().lights, frames.graph.stats().passes)
    };
    let (clustered, clustered_stats, clustered_passes) = render(LightPath::Clustered, true);
    let (per_object, object_stats, object_passes) = render(LightPath::PerObject, true);
    let (unlit, ..) = render(LightPath::Clustered, false);

    assert_eq!(
        (clustered_stats.drawn, clustered_stats.clusters, clustered_stats.list_capacity),
        (4, 3456, 64)
    );
    assert_eq!((object_stats.drawn, object_stats.clusters, object_stats.object_lists), (4, 0, 3));
    assert_eq!(clustered_passes, object_passes + 1, "only the clustered path bins");
    let changed = unlit.iter().zip(&clustered).filter(|(a, b)| a != b).count();
    assert!(changed > 2000, "the lights light the scene: {changed} pixels");
    // Each sphere lists every light that reaches it in index order, as its clusters do, so the
    // paths sum the same lights in the same order.
    let differences: Vec<u8> = clustered
        .iter()
        .zip(&per_object)
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
        .collect();
    let differing = differences.iter().filter(|&&difference| difference > 1).count();
    let worst = differences.iter().copied().max().unwrap_or(0);
    eprintln!("clustered vs per-object: {differing} pixels differ by more than 1, at most {worst}");
    assert!(
        differing <= (size.0 * size.1 / 1000) as usize && worst <= 8,
        "{differing} pixels, worst {worst}"
    );
}

#[test]
#[ignore = "needs a GPU"]
fn local_lights_run_at_any_size_and_msaa_on_both_paths() {
    let gpu = gpu();
    let mesh = sphere(white(4.0));
    for light_path in [LightPath::Clustered, LightPath::PerObject] {
        for msaa in [Msaa::Off, Msaa::X4] {
            // Asking for more lights than ever fit is clamped to MAX_LIGHTS.
            let settings =
                RenderSettings { msaa, light_path, max_lights: u32::MAX, ..RenderSettings::default() };
            let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
            let handle = upload(&mut frames.renderer, &mesh);
            frames.lights = vec![
                point_light(Vec3::new(1.2, 0.5, 1.0), Vec3::new(1.0, 0.5, 0.2), 1.0, 3.0),
                LocalLight {
                    kind: LightKind::Spot { direction: Vec3::NEG_Y, inner_angle: 0.2, outer_angle: 0.5 },
                    ..point_light(Vec3::new(0.0, 2.0, 0.0), Vec3::ONE, 1.0, 4.0)
                },
            ];
            let mut add = vec![instance(0, handle, Mat4::IDENTITY)];
            for size in [(1, 1), (3, 2), (17, 1000), (641, 359)] {
                let texture = target(&gpu, FORMAT, size);
                let view = texture.create_view(&Default::default());
                let camera = framing_camera(38.0, size, 0.5);
                let error = frames.render(&gpu, &view, camera, &std::mem::take(&mut add), &[]);
                assert!(error.is_none(), "{settings:?} at {size:?}: {error:?}");
                // Uploads, the sky cube's face, the character prepass, main and the composite (no
                // bloom), plus the cluster pass.
                let expected = 5 + u32::from(light_path == LightPath::Clustered);
                assert_eq!(frames.graph.stats().passes, expected, "{settings:?}");
                let stats = frames.renderer.stats().lights;
                let capacity = if light_path == LightPath::Clustered { 64 } else { 4 };
                assert_eq!((stats.drawn, stats.list_capacity), (2, capacity));
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn per_object_lists_survive_their_buffer_growing() {
    let gpu = gpu();
    let settings = RenderSettings { light_path: LightPath::PerObject, ..RenderSettings::default() };
    let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
    let mesh = upload(&mut frames.renderer, &sphere(two_tone()));
    frames.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
    frames.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
    frames.sun = DirectionalLight { direction: Vec3::Z, color: Vec3::ONE, intensity: 0.1 };
    frames.lights = vec![point_light(Vec3::new(0.0, 0.0, 2.0), Vec3::X, 1.0, 3.0)];
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, size, 0.5);
    let hidden = |ids: std::ops::Range<u64>| ids.map(|id| instance(id, mesh, Mat4::ZERO)).collect::<Vec<_>>();
    let red_center = |frames: &mut Frames, changed: &[MeshInstance]| {
        assert!(frames.render(&gpu, &view, camera, changed, &[]).is_none());
        read_back(&gpu, &texture)[(32 * 64 + 32) as usize][0]
    };

    let mut first = vec![instance(0, mesh, Mat4::IDENTITY)];
    first.extend(hidden(1..200));
    let red = red_center(&mut frames, &first);
    assert!(red > 150, "lit red by its list: {red}");
    let before = frames.renderer.stats();
    assert_eq!(before.lights.object_lists, 200, "new lights: every instance");
    // Past the first 256 lists: the list buffer grows while instance 0's list is uploaded.
    assert_eq!(red_center(&mut frames, &hidden(200..400)), red);
    let after = frames.renderer.stats();
    assert!(after.buffer_growths > before.buffer_growths, "{after:?}");
    assert_eq!(after.lights.object_lists, 200, "the same lights: only the new instances");
    assert_eq!(red_center(&mut frames, &[]), red, "instance 0's list survived the growth");
    assert_eq!(frames.renderer.stats().lights.object_lists, 0);
}

/// `material` cooked against textures with these swizzles, one per slot, as cooked bytes.
fn cooked_with(
    material: &AnimeCharacterMaterial,
    swizzles: &[Swizzle],
) -> sapphire_assets::material::AlignedVec {
    Material::from_document(material, swizzles).expect("a valid material").to_cooked().expect("cook")
}

/// A unit sphere drawn with an already uploaded material.
fn upload_with(renderer: &mut Renderer, material: MaterialHandle) -> MeshHandle {
    let bytes = sphere(white(0.0)).to_cooked().expect("cook");
    renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
}

/// The linear RGB of the middle row's pixel at `x`.
fn linear_at(pixels: &[[u8; 4]], (width, height): (u32, u32), x: usize) -> [f32; 3] {
    let pixel = pixels[(height / 2 * width) as usize + x];
    [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
}

fn assert_near(actual: [f32; 3], expected: [f32; 3], what: &str) {
    let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < 0.02);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn material_parameters_reach_the_shader() {
    let gpu = gpu();
    let size = (128, 128);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
    // No ambient, and the sun from the right: N·L is the sphere's x, so with threshold 0 the
    // split runs down the middle, lit to the right. At 0.8 every tone stays below the tone
    // curve's shoulder.
    frames.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
    frames.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
    frames.sun = DirectionalLight { direction: Vec3::NEG_X, color: Vec3::ONE, intensity: 0.8 };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let bytes = cooked_with(&two_tone(), &swizzles);
    let material = frames.renderer.upload_material(Material::read(&bytes).unwrap(), |_| None).unwrap();
    let mesh = upload_with(&mut frames.renderer, material);
    let camera = framing_camera(38.0, size, 0.6);
    let mut add = vec![instance(0, mesh, Mat4::IDENTITY)];
    let mut render = |frames: &mut Frames| {
        let error = frames.render(&gpu, &view, camera, &std::mem::take(&mut add), &[]);
        assert!(error.is_none(), "{error:?}");
        read_back(&gpu, &texture)
    };
    // The middle row's pixel over the sphere point with x = `sphere_x`.
    let column = |sphere_x: f32| {
        let point = Vec3::new(sphere_x, 0.0, (1.0 - sphere_x * sphere_x).sqrt());
        let clip = camera.proj * camera.view * point.extend(1.0);
        (clip.x / clip.w + 1.0) * 0.5 * size.0 as f32
    };
    let (lit_x, shadow_x) = (column(0.7) as usize, column(-0.7) as usize);
    let edit = |frames: &mut Frames, edit: &dyn Fn(&mut AnimeCharacterMaterial)| {
        let mut document = two_tone();
        edit(&mut document);
        let bytes = cooked_with(&document, &swizzles);
        frames.renderer.update_material(material, Material::read(&bytes).unwrap(), |_| None).unwrap();
    };

    let pixels = render(&mut frames);
    assert_near(linear_at(&pixels, size, lit_x), [0.8; 3], "lit");
    assert_near(linear_at(&pixels, size, shadow_x), [0.4; 3], "shadow");
    let builds = frames.renderer.stats().draw_list_builds;

    // Moving the threshold to 0.5 moves the split to where N·L is 0.5.
    edit(&mut frames, &|m| m.light_split.threshold = 0.5);
    let red = middle_row_red(&render(&mut frames), size);
    assert_eq!(frames.renderer.stats().material_uploads, 1, "an edit uploads its one slot");
    let center = size.0 as usize / 2;
    let (_, split) = (center - 20..center + 30)
        .map(|x| (red[x + 2] - red[x], x + 1))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("pixels");
    let expected = column(0.5);
    assert!((split as f32 + 0.5 - expected).abs() < 2.0, "split at {split}, expected {expected:.1}");

    // New colors: the lit side is the base color, the shadow side the base times the shade.
    edit(&mut frames, &|m| {
        m.base_color = [0.2, 0.6, 1.0, 1.0];
        m.regions[0].shade_color = [0.5, 0.25, 1.0];
    });
    let pixels = render(&mut frames);
    assert_near(linear_at(&pixels, size, lit_x), [0.16, 0.48, 0.8], "new base color");
    assert_near(linear_at(&pixels, size, shadow_x), [0.08, 0.12, 0.8], "new shade color");
    assert_eq!(frames.renderer.stats().draw_list_builds, builds, "edits never rebuild the draw list");
}

#[test]
#[ignore = "needs a GPU"]
fn textured_materials_sample_through_their_swizzle() {
    use sapphire_assets::AssetId;
    use sapphire_assets::texture::{Component, Texture, TextureDesc, TextureFormat};
    use sapphire_pipeline::cook::Cook;
    use sapphire_pipeline::import::image::{ChannelLayout, SourceImage};
    use sapphire_pipeline::texture::{self, MaskChannels, MaskSettings, TextureClass, TextureSettings};
    use sapphire_render::texture::{GpuTexture, check_format};

    let gpu = gpu();
    let size = (64, 64);
    let target_texture = target(&gpu, FORMAT, size);
    let view = target_texture.create_view(&Default::default());
    let id: AssetId = "7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d".parse().unwrap();
    // Lit head-on by the unit sun plus 6% ambient (12% of the grey), so this base color shows the
    // texture's logical channels as they are.
    let document = AnimeCharacterMaterial {
        base_color: [1.0 / 1.06, 1.0 / 1.06, 1.0 / 1.06, 1.0],
        base_color_map: Some(id),
        ..flat([1.0; 3], 0.0)
    };
    let slot_swizzles = |swizzle: Swizzle| {
        let mut swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        swizzles[AnimeCharacterMaterial::BASE_COLOR] = swizzle;
        swizzles
    };
    // Draws the sphere with `document` bound to `texture`, and returns the center's linear RGB.
    let center_color = |texture: &GpuTexture| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
        let bytes = cooked_with(&document, &slot_swizzles(texture.swizzle()));
        let lookup = |asked: AssetId| (asked == id).then_some(texture);
        let material = frames.renderer.upload_material(Material::read(&bytes).unwrap(), lookup).unwrap();
        let mesh = upload_with(&mut frames.renderer, material);
        let instances = [instance(0, mesh, Mat4::IDENTITY)];
        let error = frames.render(&gpu, &view, framing_camera(38.0, size, 0.6), &instances, &[]);
        assert!(error.is_none(), "{error:?}");
        linear_at(&read_back(&gpu, &target_texture), size, size.0 as usize / 2)
    };

    // On every device: an uncompressed texture whose swizzle reverses its color channels.
    let reversed = Swizzle([Component::B, Component::G, Component::R, Component::One]);
    let srgb = |c: f32| {
        (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } * 255.0).round() as u8
    };
    let texel = [srgb(0.8), srgb(0.5), srgb(0.2), 255];
    let texels: Vec<u8> = texel.repeat(16);
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: reversed,
    };
    let texture = GpuTexture::upload(&gpu, &Texture { desc, levels: vec![&texels] }, "reversed").unwrap();
    assert_near(center_color(&texture), [0.2, 0.5, 0.8], "the swizzle reverses the channels");

    // The material checks what it binds: a texture whose swizzle differs from the one it was
    // cooked against, or none at all, is an error.
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
    let identity = cooked_with(&document, &slot_swizzles(Swizzle::IDENTITY));
    let error =
        frames.renderer.upload_material(Material::read(&identity).unwrap(), |_| Some(&texture)).unwrap_err();
    assert!(matches!(error, RenderError::TextureSwizzle { slot: "base_color", .. }), "{error}");
    let error = frames.renderer.upload_material(Material::read(&identity).unwrap(), |_| None).unwrap_err();
    assert!(matches!(error, RenderError::MissingTexture { slot: "base_color", .. }), "{error}");

    // With ASTC: a two-channel mask from the mobile cook, which ASTC stores as luminance plus
    // alpha and reads back through `ra01`.
    let image =
        SourceImage::new(16, 16, ChannelLayout::GrayAlpha, vec![[0.75, 0.75, 0.75, 0.25]; 256]).unwrap();
    let mask = MaskSettings { channels: MaskChannels::Two, ..MaskSettings::default() };
    let settings = TextureSettings { class: TextureClass::Mask(mask) };
    let cooked = texture::cook(&image, &settings, Cook::Mobile).unwrap();
    let astc = cooked.texture();
    if check_format(&gpu, astc.desc.format).is_err() {
        eprintln!("{} has no ASTC: skipped the mobile cook's mask", gpu.caps().adapter.name);
        return;
    }
    assert_eq!(astc.desc.swizzle.to_string(), "ra01");
    let texture = GpuTexture::upload(&gpu, &astc, "mask").unwrap();
    assert_near(center_color(&texture), [0.75, 0.25, 0.0], "ASTC's two channels through `ra01`");
}

#[test]
#[ignore = "needs a GPU"]
fn packed_meshes_draw_with_the_materials_and_textures_their_slots_name() {
    use std::collections::HashMap;
    use std::sync::Arc;

    use sapphire_assets::mesh::SlotMaterial;
    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::texture::{Texture, TextureDesc, TextureFormat};
    use sapphire_assets::{
        AssetBudgets, AssetId, AssetKind, AssetServer, Handle, MemoryPackSource, TextureAsset,
    };
    use sapphire_render::texture::GpuTexture;

    let gpu = gpu();
    let size = (64, 64);
    let target_texture = target(&gpu, FORMAT, size);
    let view = target_texture.create_view(&Default::default());
    let [texture_id, material_id, mesh_id] = [1, 2, 3].map(|n| AssetId::from_bytes([n; 16]));

    // A 4×4 texture of linear (0.2, 0.5, 0.8); a material sampling it, lit so its color shows as
    // it is (see `textured_materials_sample_through_their_swizzle`); a sphere whose one slot names
    // the material. Packed as the pipeline packs them, dependencies listed.
    let srgb = |c: f32| {
        (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } * 255.0).round() as u8
    };
    let texels = [srgb(0.2), srgb(0.5), srgb(0.8), 255].repeat(16);
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let texture = Texture { desc, levels: vec![&texels] }.to_ktx2().unwrap();
    let document = AnimeCharacterMaterial {
        base_color: [1.0 / 1.06, 1.0 / 1.06, 1.0 / 1.06, 1.0],
        base_color_map: Some(texture_id),
        ..flat([1.0; 3], 0.0)
    };
    let material = cooked_with(&document, &[Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()]);
    let (vertices, indices) = sphere_geometry(Vec3::ZERO, 1.0);
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot { name: "body".into(), material: SlotMaterial::Asset(material_id) }];
    let mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
        .to_cooked()
        .unwrap();
    let packed = |id, kind: AssetKind, bytes: &[u8], dependencies| {
        PackAsset::encode(id, kind, Codec::Zstd, bytes, dependencies, 3).unwrap()
    };
    let pack = write_pack(vec![
        packed(texture_id, AssetKind::Texture, &texture, vec![]),
        packed(material_id, AssetKind::Material, &material, vec![texture_id]),
        packed(mesh_id, AssetKind::Mesh, &mesh, vec![material_id]),
    ])
    .unwrap();
    let source = Arc::new(MemoryPackSource::new());
    source.insert("shared", pack);
    let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
    server.mount("shared").unwrap();

    // The app's side of uploads: renderer objects by asset ID, each kind through the renderer's
    // own upload, resolving references through what is already uploaded.
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::X4).expect("renderer");
    let mut textures: HashMap<AssetId, GpuTexture> = HashMap::new();
    let mut materials: HashMap<AssetId, MaterialHandle> = HashMap::new();
    let mut meshes: HashMap<AssetId, MeshHandle> = HashMap::new();
    let handle: Handle<Mesh> = server.load(mesh_id).unwrap();
    server
        .finish(&handle, |upload| {
            if let Some(texture) = upload.get::<TextureAsset>() {
                textures.insert(upload.id(), GpuTexture::upload(&gpu, &texture, "texture")?);
            } else if let Some(material) = upload.get::<Material>() {
                let handle = frames.renderer.upload_material(material, |id| textures.get(&id))?;
                materials.insert(upload.id(), handle);
            } else if let Some(mesh) = upload.get::<Mesh>() {
                // Before the mesh's material is known, its slot doesn't resolve.
                let error = frames.renderer.upload_slot_materials(mesh, |_| None).unwrap_err();
                assert!(matches!(error, RenderError::UnresolvedMaterial { .. }), "{error}");
                let slots = frames.renderer.upload_slot_materials(mesh, |id| materials.get(&id).copied())?;
                assert_eq!(slots, [materials[&material_id]]);
                meshes.insert(upload.id(), frames.renderer.upload_mesh(mesh, &slots)?);
            }
            Ok(())
        })
        .unwrap();

    let instances = [instance(0, meshes[&mesh_id], Mat4::IDENTITY)];
    let error = frames.render(&gpu, &view, framing_camera(38.0, size, 0.6), &instances, &[]);
    assert!(error.is_none(), "{error:?}");
    let center = linear_at(&read_back(&gpu, &target_texture), size, size.0 as usize / 2);
    assert_near(center, [0.2, 0.5, 0.8], "the packed texture, through the packed material");
}

/// A GPU device that is lost: destroyed, and the loss reported.
fn lost_gpu() -> GpuContext {
    let gpu = gpu();
    gpu.device().destroy();
    // wgpu reports a destroyed device once its queue drains, during a poll.
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(gpu.is_lost());
    gpu
}

/// QA-001: a renderer made on a lost device is an error, not a cascade of validation errors from
/// the dead objects it would hold (which stopped debug builds on the Android emulator).
#[test]
#[ignore = "needs a GPU"]
fn renderers_report_a_lost_device() {
    let gpu = lost_gpu();
    let error = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect_err("no renderer");
    assert!(matches!(error, RenderError::DeviceLost), "{error}");
}

/// QA-001: a shader build (hot reload, live link) whose device is lost before it is swapped in
/// is answered with the loss, not swapped in as if it had built.
#[test]
#[ignore = "needs a GPU"]
fn shader_builds_on_a_lost_device_are_answered_with_the_loss() {
    use std::time::Duration;

    use sapphire_render::{ShaderError, ShaderFile, ShaderSources};

    let gpu = gpu();
    let size = (16, 16);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let mut sources = ShaderSources::compiled_in();
    sources.set(ShaderFile::Post, format!("{}\n// edited\n", sources.get(ShaderFile::Post)));
    let request = frames.renderer.request_shader_sources(sources);
    gpu.device().destroy();
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    let camera = framing_camera(38.0, size, 0.6);
    // Frames 5 ms apart, for at most 30 s.
    let mut frame = 0;
    let outcome = loop {
        draw_frame(&mut frames, &gpu, &view, camera, &[]);
        if let Some(outcome) = frames.renderer.shader_outcomes().next() {
            break outcome;
        }
        frame += 1;
        assert!(frame < 6000, "the build never finished");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(outcome.request, request);
    let error = outcome.result.expect_err("nothing built on a lost device is swapped in");
    assert!(matches!(&error, ShaderError::Pipeline { programs, .. } if programs == "post"), "{error}");
    assert_eq!(frames.renderer.stats().shader_rebuilds, 0);
}

/// Prepares one frame seen by `camera` that adds or changes `changed`, records it into `view` and
/// executes it, as an app does. Returns whether the renderer recorded anything.
fn draw_frame(
    frames: &mut Frames,
    gpu: &GpuContext,
    view: &wgpu::TextureView,
    camera: CameraData,
    changed: &[MeshInstance],
) -> bool {
    let packet = frames.packets.begin();
    packet.views.push(ViewData::new(camera));
    packet.instances.changed.extend_from_slice(changed);
    frames.renderer.prepare(gpu, frames.packets.current());
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", view);
    let recorded = frames.renderer.record(&mut graph, target).is_some();
    frames.graph.execute(gpu, graph).expect("a valid graph");
    recorded
}

/// QA-001: what the Android emulator's startup did after its device was lost, in a debug build:
/// uploads that grow pool buffers (a skinned mesh's sources, as its character did), then frames,
/// which record nothing. Nothing panics; the app sees the loss and rebuilds.
#[test]
#[ignore = "needs a GPU"]
fn uploads_and_frames_after_a_device_loss_do_not_panic() {
    let gpu = gpu();
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, size, 0.6);
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let plain = upload(&mut frames.renderer, &sphere(white(1.0)));
    assert!(draw_frame(&mut frames, &gpu, &view, camera, &[instance(0, plain, Mat4::IDENTITY)]));
    gpu.device().destroy();
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(gpu.is_lost());
    let skinned = upload(&mut frames.renderer, &skinned_sphere(white(1.0)));
    for frame in 0..3 {
        let changed = [instance(frame, plain, Mat4::IDENTITY), instance(10 + frame, skinned, Mat4::IDENTITY)];
        assert!(!draw_frame(&mut frames, &gpu, &view, camera, &changed), "frame {frame} recorded");
    }
    assert!(gpu.is_lost());
}

/// The material counters [`failed_material_uploads_leave_nothing_behind`] holds still.
fn material_counts(renderer: &Renderer) -> (u32, u64) {
    let stats = renderer.stats();
    (stats.materials, stats.material_bytes)
}

/// A unit sphere whose two submeshes draw with the two embedded materials given, cooked.
fn two_slot_sphere(first: Material, second: Material) -> sapphire_assets::material::AlignedVec {
    let (vertices, indices) = sphere_geometry(Vec3::ZERO, 1.0);
    let half = (indices.len() / 6 * 3) as u32;
    let submeshes = vec![
        Submesh { first_index: 0, index_count: half, material: 0 },
        Submesh { first_index: half, index_count: indices.len() as u32 - half, material: 1 },
    ];
    let slots = vec![MaterialSlot::embedded("first", first), MaterialSlot::embedded("second", second)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
        .to_cooked()
        .expect("cook")
}

/// RA-005: an upload that fails partway takes back every material it uploaded, so retrying
/// rejected content never uses up material slots. (A family's buffer filling up partway is
/// `materials`' `a_full_family_takes_back_what_the_upload_took`.)
#[test]
#[ignore = "needs a GPU"]
fn failed_material_uploads_leave_nothing_behind() {
    use sapphire_assets::AssetId;
    use sapphire_assets::material::{EnvironmentMaterial, FoliageMaterial, SkyMaterial};

    let gpu = gpu();
    let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
    // Counted from a mesh that uploads, so the stats are current.
    upload(&mut renderer, &sphere(white(0.0)));
    let start = material_counts(&renderer);
    let character = cooked(&white(0.0));

    // A valid material, then one naming a texture: embedded materials have none to name.
    let textured =
        AnimeCharacterMaterial { base_color_map: Some(AssetId::from_bytes([7; 16])), ..white(0.0) };
    let bytes = two_slot_sphere(character.clone(), cooked(&textured));
    for attempt in 1..=3 {
        let error = renderer.upload_embedded_materials(Mesh::read(&bytes).unwrap()).unwrap_err();
        assert!(matches!(error, RenderError::MissingTexture { slot: "base_color", .. }), "{error}");
        assert_eq!(material_counts(&renderer), start, "attempt {attempt} left materials behind");
        let error = renderer.upload_mesh_with_slots(Mesh::read(&bytes).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::MissingTexture { .. }), "{error}");
        assert_eq!(material_counts(&renderer), start, "attempt {attempt} left materials behind");
    }

    // Then a sky material, which meshes don't draw with.
    let sky = Material::from_document(
        &SkyMaterial::default(),
        &[Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()],
    )
    .expect("a valid material");
    let bytes = two_slot_sphere(character.clone(), sky);
    let error = renderer.upload_mesh_with_slots(Mesh::read(&bytes).unwrap(), |_| None).unwrap_err();
    assert!(matches!(error, RenderError::NotDrawable(_)), "{error}");
    assert_eq!(material_counts(&renderer), start);

    // Then foliage, which the materials take but the mesh can't draw (no foliage stream): the
    // mesh fails after its materials were uploaded, and they go too.
    let leaves = Material::from_document(
        &FoliageMaterial::default(),
        &[Swizzle::IDENTITY; FoliageMaterial::TEXTURE_SLOTS.len()],
    )
    .expect("a valid material");
    let bytes = two_slot_sphere(character.clone(), leaves);
    for _ in 0..3 {
        let error = renderer.upload_mesh_with_slots(Mesh::read(&bytes).unwrap(), |_| None).unwrap_err();
        assert!(matches!(error, RenderError::NoFoliageStream), "{error}");
        assert_eq!(material_counts(&renderer), start);
    }
    // Uploaded in two calls, the materials are the caller's, given back as handles it holds.
    let mesh = Mesh::read(&bytes).unwrap();
    let handles = renderer.upload_slot_materials(mesh, |_| None).expect("drawable materials");
    assert!(matches!(renderer.upload_mesh(mesh, &handles), Err(RenderError::NoFoliageStream)));
    assert_eq!(material_counts(&renderer).0, start.0 + 2);

    // A mesh that uploads keeps its materials, of any families meshes draw with.
    let start = material_counts(&renderer);
    let wall = Material::from_document(
        &EnvironmentMaterial::default(),
        &[Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()],
    )
    .expect("a valid material");
    let bytes = two_slot_sphere(character, wall);
    renderer.upload_mesh_with_slots(Mesh::read(&bytes).unwrap(), |_| None).expect("upload");
    assert_eq!(material_counts(&renderer).0, start.0 + 2);
}
