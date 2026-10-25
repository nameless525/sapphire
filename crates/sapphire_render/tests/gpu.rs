//! Renderer tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::mesh::{MeshAsset, MeshVertex, Submesh, ToonMaterialParams};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MeshHandle,
    MeshInstance, Msaa, PostParams, RenderError, RenderSettings, Renderer, SkyGradient, ToneCurve, ViewData,
    Vignette,
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

/// A unit-radius UV sphere at the origin with one material.
fn sphere(material: ToonMaterialParams) -> MeshAsset {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut mesh = MeshAsset::default();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            mesh.vertices.push(MeshVertex {
                position: n,
                normal: n,
                outline_normal: n,
                uv: [0.0; 2],
                color: [255; 4],
            });
        }
    }
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            mesh.indices.extend([a, c, b, c, d, b]);
        }
    }
    mesh.submeshes.push(Submesh {
        first_index: 0,
        index_count: mesh.indices.len() as u32,
        material_index: 0,
        _reserved: 0,
    });
    mesh.materials.push(material);
    mesh.bounds = [[-1.0; 3], [1.0; 3]];
    mesh
}

/// A flat material: no shadow tone and no rim, black outline.
fn flat(rgb: [f32; 3], outline_px: f32) -> ToonMaterialParams {
    ToonMaterialParams {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        shade_color: [1.0; 4],
        outline_color: [0.0, 0.0, 0.0, 1.0],
        params: [0.0, 0.02, 0.0, outline_px],
    }
}

/// Linear 0.5 grey, the same as the test sky: lit by the unit sun plus 12% sky ambient.
const GREY: [f32; 3] = [0.5 / 1.06; 3];

fn white(outline_px: f32) -> ToonMaterialParams {
    flat([1.0; 3], outline_px)
}

/// Fully lit in `rgb`, with no outline.
fn colored(rgb: [f32; 3]) -> ToonMaterialParams {
    ToonMaterialParams { base_color: [rgb[0], rgb[1], rgb[2], 1.0], shade_color: [1.0; 4], ..white(0.0) }
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
    /// Copied into every packet: [`passthrough`] by default.
    post: PostParams,
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
            post: passthrough(),
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
        let packet = self.packets.begin();
        packet.views.push(ViewData { camera });
        // Light along the view direction, so everything the camera sees is lit.
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky = self.sky;
        packet.post = self.post;
        packet.instances.changed.extend_from_slice(changed);
        packet.instances.removed.extend(removed.iter().copied().map(InstanceId));

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", view);
        self.renderer.record(&mut graph, target);
        self.graph.execute(gpu, graph).expect("a valid graph");
        pollster::block_on(scope.pop())
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
    let mesh = frames.renderer.upload_mesh(&sphere(flat(GREY, 10.0))).expect("upload");
    // Added by the first frame; the scene keeps it for the rest.
    let mut add = vec![instance(0, mesh, Mat4::IDENTITY)];
    for size in [(960, 540), (1920, 1080)] {
        let texture = target(&gpu, FORMAT, size);
        let view = texture.create_view(&Default::default());
        // 10 px at 1080p.
        let expected = 10.0 * size.1 as f32 / 1080.0;
        for fov in [8.0, 38.0, 90.0] {
            // Filling 60% of the height keeps the camera inside the distance curve's flat range
            // (0.5–25 m) even at 8°.
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
                    let handle = frames.renderer.upload_mesh(&mesh).expect("upload");
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
    let mesh = frames.renderer.upload_mesh(&sphere(white(4.0))).expect("upload");
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
        frames.renderer.upload_mesh(&sphere(colored([1.0, 0.0, 0.0]))).expect("upload");
    }
    let green = frames.renderer.upload_mesh(&sphere(colored([0.0, 1.0, 0.0]))).expect("upload");
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let instances = [instance(0, green, Mat4::IDENTITY)];
    assert!(frames.render(&gpu, &view, framing_camera(38.0, size, 0.5), &instances, &[]).is_none());
    let center = read_back(&gpu, &texture)[(32 * 64 + 32) as usize];
    assert!(center[1] > 200 && center[0] < 80, "center pixel {center:?} should be the green sphere");

    let stats = frames.renderer.stats();
    assert_eq!((stats.draws, stats.instances), (1, 1));
    let sphere_bytes = {
        let mesh = sphere(white(0.0));
        (mesh.vertices.len() * size_of::<MeshVertex>()
            + mesh.indices.len() * 4
            + size_of::<ToonMaterialParams>()) as u64
    };
    assert_eq!(stats.mesh_pool_bytes, 4 * sphere_bytes);
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
    let red = frames.renderer.upload_mesh(&sphere(colored([1.0, 0.0, 0.0]))).expect("upload");
    let green = frames.renderer.upload_mesh(&sphere(colored([0.0, 1.0, 0.0]))).expect("upload");
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
        let mesh = frames.renderer.upload_mesh(&sphere(flat([0.9, 0.2, 0.15], 3.0))).expect("upload");
        frames.sky = SkyGradient::default();
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
        let mesh = frames.renderer.upload_mesh(&sphere(flat([brightness; 3], 0.0))).expect("upload");
        frames.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
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
    let green = frames.renderer.upload_mesh(&sphere(colored([0.0, 1.0, 0.0]))).expect("upload");
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
            let settings = RenderSettings { msaa, bloom_levels };
            let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
            let handle = frames.renderer.upload_mesh(&mesh).expect("upload");
            frames.post =
                PostParams { grade_b: strong_grade(0.3), grade_blend: 0.5, ..PostParams::default() };
            let mut add = vec![instance(0, handle, Mat4::IDENTITY)];
            for size in [(1, 1), (3, 2), (17, 1000), (641, 359)] {
                let texture = target(&gpu, FORMAT, size);
                let view = texture.create_view(&Default::default());
                let camera = framing_camera(38.0, size, 0.5);
                let error = frames.render(&gpu, &view, camera, &std::mem::take(&mut add), &[]);
                assert!(error.is_none(), "{settings:?} at {size:?}: {error:?}");
                // Uploads, main, the bloom chain (a prefilter, then a down and an up pass per
                // further level) and the composite.
                let expected = 3 + if bloom_levels > 0 { 2 * bloom_levels - 1 } else { 0 };
                assert_eq!(frames.graph.stats().passes, expected, "{settings:?}");
            }
        }
    }
}
