//! Render-scale tests that need a GPU (M1 R6). Run with
//! `cargo test -p sapphire_render --test render_scale -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Material, Outline, Rim};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor, GraphReport};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, Renderer, SkyGradient, ToneCurve, ViewData,
    Vignette, scaled_size,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Linear 0.5 grey under the test light (the unit sun plus 12% of the grey as ambient), like
/// the sky.
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

/// A unit UV sphere at the origin in `rgb`, with a black outline `outline_px` wide at 1080p.
fn sphere(rgb: [f32; 3], outline_px: f32) -> Mesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: n.to_array(),
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
    // Lit everywhere (no shadow tone or rim), so only the outline and the silhouette show.
    let material = AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: outline_px, z_offset: 0.0 },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// Post that shows linear values below about 0.98 as they are: a tone curve linear from black,
/// a neutral grade, no bloom or vignette.
fn passthrough() -> PostParams {
    PostParams {
        bloom: BloomParams { intensity: 0.0, ..BloomParams::default() },
        tone_curve: ToneCurve {
            linear_start: 0.001,
            linear_length: 0.99,
            black_tightness: 1.0,
            ..ToneCurve::default()
        },
        grade_a: ColorGrade::NEUTRAL,
        grade_b: ColorGrade::NEUTRAL,
        vignette: Vignette { intensity: 0.0, ..Vignette::default() },
        ..PostParams::default()
    }
}

/// A camera on +Z with the unit sphere filling `fraction` of the image height.
fn camera((width, height): (u32, u32), fraction: f32) -> CameraData {
    let fov = 38f32.to_radians();
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

/// A renderer drawing one sphere against an even grey sky.
struct Scene {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    mesh: MeshHandle,
    post: PostParams,
    /// The sphere is added by the first frame and kept after.
    added: bool,
}

impl Scene {
    fn new(gpu: &GpuContext, msaa: Msaa, mesh: &Mesh, post: PostParams) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, RenderSettings { msaa, ..RenderSettings::default() })
            .expect("renderer");
        let bytes = mesh.to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");
        let mut graph = GraphExecutor::new();
        graph.set_report(true);
        Self { renderer, graph, packets: FramePacketRing::new(), mesh, post, added: false }
    }

    /// Draws one frame into `texture` at `scale`, with the sphere filling `fraction` of its
    /// height, checks it raised no validation error and returns its pixels.
    fn render(
        &mut self,
        gpu: &GpuContext,
        texture: &wgpu::Texture,
        scale: f32,
        fraction: f32,
    ) -> Vec<[u8; 4]> {
        let size = (texture.width(), texture.height());
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera(size, fraction)));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
        packet.ambient = AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 };
        packet.post = self.post;
        if !std::mem::replace(&mut self.added, true) {
            packet.instances.changed.push(MeshInstance {
                id: InstanceId(0),
                mesh: self.mesh,
                transform: Mat4::IDENTITY,
            });
        }

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.set_render_scale(scale);
        self.renderer.prepare(gpu, self.packets.current());
        let view = texture.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target);
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{size:?} at scale {scale}: {error:?}");
        read_back(gpu, texture)
    }

    fn report(&self) -> &GraphReport {
        self.graph.report().expect("the report is on")
    }
}

/// The size of the graph texture named `name` in the last frame.
fn texture_size(report: &GraphReport, name: &str) -> Option<(u32, u32)> {
    report.textures.iter().find(|texture| texture.name == name).map(|texture| texture.size)
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

/// Pixels that are clearly the red sphere rather than the grey sky.
fn red_pixels(pixels: &[[u8; 4]]) -> usize {
    pixels.iter().filter(|p| i32::from(p[0]) - i32::from(p[1]) > 60).count()
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[test]
#[ignore = "needs a GPU"]
fn half_scale_renders_a_half_size_scene_and_upsamples_it() {
    let gpu = gpu();
    let mesh = sphere([0.9, 0.05, 0.05], 0.0);
    for msaa in [Msaa::Off, Msaa::X4] {
        // With bloom and the default grade on, so the bloom chain runs at the scene's odd sizes.
        let mut scene = Scene::new(&gpu, msaa, &mesh, PostParams::default());
        for size in [(641, 361), (333, 187), (97, 1), (1, 1)] {
            let texture = target(&gpu, size);
            let full = scene.render(&gpu, &texture, 1.0, 0.6);
            assert_eq!(
                texture_size(scene.report(), "scene color"),
                Some(size),
                "{msaa:?}: scale 1 is the target's size"
            );

            let half = scene.render(&gpu, &texture, 0.5, 0.6);
            let scene_size = scaled_size(size, 0.5);
            let report = scene.report();
            for name in ["scene color", "depth"] {
                assert_eq!(texture_size(report, name), Some(scene_size), "{msaa:?} {size:?}: {name}");
            }
            if msaa != Msaa::Off {
                assert_eq!(texture_size(report, "msaa color"), Some(scene_size));
            }
            assert_eq!(texture_size(report, "target"), Some(size), "post writes the target's size");
            assert_eq!(scene.renderer.stats().scene_size, scene_size);
            if size.1 < 100 {
                continue;
            }
            // The upsampled sphere covers what the full-size one does, give or take its edge, and
            // its middle is the same color: the whole scene reaches the whole target.
            let (full_red, half_red) = (red_pixels(&full), red_pixels(&half));
            let edge = (full_red as f32).sqrt() * 4.0;
            assert!(
                (full_red as f32 - half_red as f32).abs() < edge,
                "{msaa:?} {size:?}: {half_red} red pixels at half scale, {full_red} at full"
            );
            let middle = (size.1 / 2 * size.0 + size.0 / 2) as usize;
            let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
            assert!(
                (0..3).all(|c| close(full[middle][c], half[middle][c])),
                "{:?} vs {:?}",
                full[middle],
                half[middle]
            );
            let corner = |pixels: &[[u8; 4]]| pixels[0];
            assert!((0..3).all(|c| close(corner(&full)[c], corner(&half)[c])), "the sky is the same");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn scale_one_draws_exactly_what_the_renderer_draws_without_a_scale() {
    let gpu = gpu();
    let mesh = sphere([0.9, 0.05, 0.05], 6.0);
    let size = (640, 360);
    let texture = target(&gpu, size);
    let mut unscaled = Scene::new(&gpu, Msaa::X4, &mesh, PostParams::default());
    let expected = unscaled.render(&gpu, &texture, 1.0, 0.6);
    // A renderer that drew at half scale and came back to 1 takes the texel-for-texel path again.
    let mut scaled = Scene::new(&gpu, Msaa::X4, &mesh, PostParams::default());
    scaled.render(&gpu, &texture, 0.5, 0.6);
    let pixels = scaled.render(&gpu, &texture, 1.0, 0.6);
    assert_eq!(texture_size(scaled.report(), "scene color"), Some(size));
    let differing = expected.iter().zip(&pixels).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ at scale 1");
}

/// Outline thickness in pixels where the middle row crosses the sphere's left silhouette: the
/// sphere and the sky are the same grey, so each pixel's darkness is its outline coverage.
fn left_outline_width(pixels: &[[u8; 4]], (width, height): (u32, u32)) -> f32 {
    let row = &pixels[(height / 2 * width) as usize..][..width as usize];
    let background = srgb_to_linear(row[0][0]);
    row[..width as usize / 2].iter().map(|p| (1.0 - srgb_to_linear(p[0]) / background).clamp(0.0, 1.0)).sum()
}

#[test]
#[ignore = "needs a GPU"]
fn outlines_keep_their_width_in_output_pixels() {
    let gpu = gpu();
    let size = (960, 540);
    let texture = target(&gpu, size);
    let mut scene = Scene::new(&gpu, Msaa::X4, &sphere(GREY, 10.0), passthrough());
    // 10 px at 1080p is 5 px here, at any render scale.
    for scale in [1.0, 0.75, 0.5] {
        let width = left_outline_width(&scene.render(&gpu, &texture, scale, 0.6), size);
        assert!((width - 5.0).abs() < 1.0, "scale {scale}: outline {width:.2} px, expected 5");
        eprintln!("scale {scale}: outline {width:.2} px (expected 5)");
    }
}
