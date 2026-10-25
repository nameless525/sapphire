//! Renderer tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::mesh::{MeshAsset, MeshVertex, Submesh, ToonMaterialParams};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    CameraData, DirectionalLight, FrameView, MeshInstance, Msaa, RenderError, RenderSettings, Renderer,
    SkyGradient,
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

fn frame<'a>(camera: CameraData, instances: &'a [MeshInstance]) -> FrameView<'a> {
    FrameView {
        camera,
        // Light along the view direction, so everything the camera sees is lit.
        sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
        instances,
        sky: SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) },
        time_seconds: 0.0,
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

/// A renderer and the graph executor that runs its frames.
struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
}

impl Frames {
    fn new(gpu: &GpuContext, format: wgpu::TextureFormat, msaa: Msaa) -> Result<Self, RenderError> {
        Ok(Self {
            renderer: Renderer::new(gpu, format, RenderSettings { msaa })?,
            graph: GraphExecutor::new(),
        })
    }

    /// Prepares, records and submits one frame, and returns any validation error it raised.
    fn render(
        &mut self,
        gpu: &GpuContext,
        view: &wgpu::TextureView,
        frame: &FrameView<'_>,
    ) -> Option<wgpu::Error> {
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, frame);
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
    let instances = [MeshInstance { mesh, transform: Mat4::IDENTITY }];
    for size in [(960, 540), (1920, 1080)] {
        let texture = target(&gpu, FORMAT, size);
        let view = texture.create_view(&Default::default());
        // 10 px at 1080p.
        let expected = 10.0 * size.1 as f32 / 1080.0;
        for fov in [8.0, 38.0, 90.0] {
            // Filling 60% of the height keeps the camera inside the distance curve's flat range
            // (0.5–25 m) even at 8°.
            let error = frames.render(&gpu, &view, &frame(framing_camera(fov, size, 0.6), &instances));
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
                && color.supports_samples(samples)
                && (samples == 1 || color.multisample_resolve)
                && depth.supports_samples(samples);
            match Frames::new(&gpu, format, msaa) {
                Ok(mut frames) => {
                    assert!(supported, "{format:?} x{samples} accepted but unsupported");
                    let handle = frames.renderer.upload_mesh(&mesh).expect("upload");
                    let texture = target(&gpu, format, (64, 64));
                    let view = texture.create_view(&Default::default());
                    let instances = [MeshInstance { mesh: handle, transform: Mat4::IDENTITY }];
                    let error =
                        frames.render(&gpu, &view, &frame(framing_camera(38.0, (64, 64), 0.5), &instances));
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
    let rgba = gpu.format_support(FORMAT);
    if rgba.supports_samples(2) && depth.supports_samples(2) {
        Renderer::new(&gpu, FORMAT, RenderSettings { msaa: Msaa::X2 }).expect("2x MSAA");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn internal_targets_follow_the_target_size() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let mesh = frames.renderer.upload_mesh(&sphere(white(4.0))).expect("upload");
    let instances = [MeshInstance { mesh, transform: Mat4::IDENTITY }];
    for size in [(480, 270), (481, 270), (17, 1000), (480, 270)] {
        let texture = target(&gpu, FORMAT, size);
        let view = texture.create_view(&Default::default());
        let error = frames.render(&gpu, &view, &frame(framing_camera(38.0, size, 0.5), &instances));
        assert!(error.is_none(), "{size:?}: {error:?}");
    }
    // MSAA color and depth for each of the three sizes; returning to 480×270 reuses its pair.
    let stats = frames.graph.stats();
    assert_eq!((stats.transient_slots, stats.pool.created, stats.pool.evicted), (2, 6, 0));
}

#[test]
#[ignore = "needs a GPU"]
fn mesh_relative_indices_draw_every_mesh() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let colored = |rgb: [f32; 3]| ToonMaterialParams {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        shade_color: [1.0; 4],
        ..white(0.0)
    };
    // Several meshes, so the last one starts deep inside the shared vertex buffer.
    for _ in 0..3 {
        frames.renderer.upload_mesh(&sphere(colored([1.0, 0.0, 0.0]))).expect("upload");
    }
    let green = frames.renderer.upload_mesh(&sphere(colored([0.0, 1.0, 0.0]))).expect("upload");
    let size = (64, 64);
    let texture = target(&gpu, FORMAT, size);
    let view = texture.create_view(&Default::default());
    let instances = [MeshInstance { mesh: green, transform: Mat4::IDENTITY }];
    assert!(frames.render(&gpu, &view, &frame(framing_camera(38.0, size, 0.5), &instances)).is_none());
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
