//! Debug draw's pass (M2 T1, the `debug_draw` module), rendered and read back: where a line lands
//! and how wide it is, depth-tested against on top behind a surface under every antialiasing mode
//! and render scale, that a frame with nothing builds and adds nothing, and hot reload. They need a
//! GPU: run with `cargo test -p sapphire_render --test gpu debug_draw:: -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{EnvironmentMaterial, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_core::debug_draw::{Color, DebugDrawList, DebugLine, Depth, Shape};
use sapphire_render::debug_draw::{PASS, line_width};
use sapphire_render::graph::{FrameGraph, GraphExecutor, GraphReport};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    MeshInstance, Msaa, PostParams, RenderSettings, Renderer, SCENE_PREPASS, ShaderError, ShaderFile,
    ShaderSources, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (640, 360);
const FOV: f32 = 38.0 * PI / 180.0;
/// The camera sits here, looking down −Z.
const EYE: Vec3 = Vec3::new(0.0, 0.0, 6.0);
const NEAR: f32 = 0.05;
/// A display color no surface here has.
const LINE: Color = Color::rgb(255, 40, 200);

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

/// A unit UV sphere at the origin in the environment family, in a green-grey.
fn sphere() -> Mesh {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 32;
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
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
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, c, b, c, d, b]);
        }
    }
    let document = EnvironmentMaterial { base_color: [0.3, 0.5, 0.3, 1.0], ..EnvironmentMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&document, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// Post that leaves the scene's colors as they are below about 0.98.
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

/// The world point on the plane through the origin facing the camera that lands on the middle of
/// pixel `(x, y)` of a target of `size`.
fn world_at(size: (u32, u32), x: u32, y: u32) -> Vec3 {
    let half_height = (FOV * 0.5).tan() * EYE.z;
    let half_width = half_height * size.0 as f32 / size.1 as f32;
    let ndc_x = (x as f32 + 0.5) / size.0 as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (y as f32 + 0.5) / size.1 as f32 * 2.0;
    Vec3::new(ndc_x * half_width, ndc_y * half_height, 0.0)
}

/// The pixel `world` projects to, in a target of [`SIZE`].
fn pixel_of(world: Vec3) -> (u32, u32) {
    let camera = CameraData::look_at(EYE, Vec3::ZERO, Vec3::Y, FOV, SIZE.0 as f32 / SIZE.1 as f32, NEAR);
    let clip = camera.proj * camera.view * world.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = (ndc.x * 0.5 + 0.5) * SIZE.0 as f32;
    let y = (0.5 - ndc.y * 0.5) * SIZE.1 as f32;
    (x as u32, y as u32)
}

fn line(from: Vec3, to: Vec3, depth: Depth) -> DebugLine {
    DebugLine { from, to, color: LINE, depth }
}

/// A renderer drawing a unit sphere at the origin against an even grey sky.
struct Scene {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    added: bool,
    mesh: sapphire_render::MeshHandle,
    size: (u32, u32),
}

impl Scene {
    fn new(gpu: &GpuContext, settings: RenderSettings, size: (u32, u32)) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, settings).expect("renderer");
        let bytes = sphere().to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");
        let mut graph = GraphExecutor::new();
        graph.set_report(true);
        Self { renderer, graph, packets: FramePacketRing::new(), added: false, mesh, size }
    }

    /// Draws one frame with `lines` into `texture`, checks it raised no validation error and
    /// returns its pixels, row by row.
    fn render(&mut self, gpu: &GpuContext, texture: &wgpu::Texture, lines: &[DebugLine]) -> Pixels {
        self.record(gpu, texture, lines);
        read_back(gpu, texture)
    }

    /// Runs and completes a frame whose pixels are not inspected.
    fn complete(&mut self, gpu: &GpuContext, texture: &wgpu::Texture, lines: &[DebugLine]) {
        self.record(gpu, texture, lines);
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("frame complete");
    }

    fn record(&mut self, gpu: &GpuContext, texture: &wgpu::Texture, lines: &[DebugLine]) {
        let packet = self.packets.begin();
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        packet.views.push(ViewData::new(CameraData::look_at(EYE, Vec3::ZERO, Vec3::Y, FOV, aspect, NEAR)));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
        packet.ambient = AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 };
        packet.post = passthrough();
        packet.debug.lines.extend_from_slice(lines);
        packet.debug.stats.lines = lines.len() as u32;
        if !std::mem::replace(&mut self.added, true) {
            packet.instances.changed.push(MeshInstance {
                id: InstanceId(0),
                mesh: self.mesh,
                transform: Mat4::IDENTITY,
            });
        }
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let view = texture.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target);
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }

    fn report(&self) -> &GraphReport {
        self.graph.report().expect("the report is on")
    }

    /// Whether the last frame's graph ran pass `name`.
    fn ran(&self, name: &str) -> bool {
        self.report().passes.iter().any(|pass| pass.name == name && pass.order.is_some())
    }
}

/// A frame read back: sRGB bytes, by row.
struct Pixels {
    width: u32,
    rgba: Vec<[u8; 4]>,
}

impl Pixels {
    fn at(&self, (x, y): (u32, u32)) -> [u8; 4] {
        self.rgba[(y * self.width + x) as usize]
    }

    /// How much of [`LINE`]'s color the pixel holds, in linear light, as a target blends, by its
    /// blue over its green: a surface here has none, and full coverage is 1.
    fn line_at(&self, at: (u32, u32)) -> f32 {
        let [_, g, b, _] = self.at(at);
        (linear(b) - linear(g)) / (linear(LINE.b) - linear(LINE.g))
    }

    /// Whether the pixel or one beside it holds most of a line, for a line whose subpixel position
    /// isn't known.
    fn line_near(&self, (x, y): (u32, u32)) -> bool {
        (x.saturating_sub(2)..=x + 2)
            .flat_map(|x| (y.saturating_sub(2)..=y + 2).map(move |y| (x, y)))
            .any(|at| self.line_at(at) > 0.4)
    }

    /// Whether the pixel is `color`, within the code value an sRGB round trip may lose: backends
    /// round it differently (lavapipe writes linear 0.5 as 187, Metal as 188).
    fn is(&self, at: (u32, u32), color: Color) -> bool {
        let pixel = self.at(at);
        [color.r, color.g, color.b].iter().zip(pixel).all(|(&want, got)| want.abs_diff(got) <= 1)
    }

    /// Pixels holding any of the line's color.
    fn line_pixels(&self) -> usize {
        self.rgba.iter().filter(|[_, g, b, _]| i32::from(*b) - i32::from(*g) > 12).count()
    }
}

/// A display byte in linear light.
fn linear(byte: u8) -> f32 {
    let display = f32::from(byte) / 255.0;
    if display <= 0.04045 { display / 12.92 } else { ((display + 0.055) / 1.055).powf(2.4) }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Pixels {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
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
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped");
    let mut rgba = Vec::with_capacity((width * height) as usize);
    for row in mapped.chunks_exact(padded as usize) {
        rgba.extend(row[..(width * 4) as usize].as_chunks::<4>().0.iter().copied());
    }
    Pixels { width, rgba }
}

/// The modes a depth-tested line must hold under: 1×, MSAA, TAA (the main pass's depth transient,
/// the prepass's persistent and jittered) and a smaller scene than target.
fn modes() -> [(&'static str, RenderSettings, f32); 5] {
    let taa = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    let msaa = RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() };
    let off = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    [
        ("off", off, 1.0),
        ("msaa", msaa, 1.0),
        ("taa", taa, 1.0),
        ("off, half", off, 0.5),
        ("taa, half", taa, 0.5),
    ]
}

#[test]
#[ignore = "needs a GPU"]
fn a_line_lands_where_it_projects_in_the_color_it_names() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    // Along a row's middle, over the sky above the sphere.
    let (row, from, to) = (40, 100, 500);
    let lines = [line(world_at(SIZE, from, row), world_at(SIZE, to, row), Depth::Tested)];
    let pixels = scene.render(&gpu, &texture, &lines);
    let middle = 300;
    assert!(pixels.is((middle, row), LINE), "past exposure and grade: {:?}", pixels.at((middle, row)));
    // 1.5 px wide with a one pixel soft edge, so a pixel away holds a quarter and two pixels nothing.
    let beside = pixels.line_at((middle, row + 1));
    assert!((beside - 0.25).abs() < 0.03, "{beside}");
    assert!(pixels.line_at((middle, row - 2)).abs() < 0.01);
    assert!(pixels.line_at((middle, row + 2)).abs() < 0.01);
    // It starts and ends where it was told, with its round cap, and nowhere else.
    assert!(pixels.line_at((from + 1, row)) > 0.9);
    assert!(pixels.line_at((to - 1, row)) > 0.9);
    // Its round caps: a pixel past each end, as a pixel beside it, and off the line's row less.
    assert!((pixels.line_at((from - 1, row)) - 0.25).abs() < 0.03);
    assert!((pixels.line_at((to + 1, row)) - 0.25).abs() < 0.03);
    assert!(pixels.line_at((from - 1, row + 1)) < 0.02);
    assert!(pixels.line_at((from - 3, row)).abs() < 0.01);
    assert!(pixels.line_at((to + 3, row)).abs() < 0.01);
    let expected = (to - from) as usize;
    let drawn = pixels.line_pixels();
    assert!(drawn >= expected && drawn < expected * 4, "{drawn} pixels for a line {expected} long");
    let stats = scene.renderer.stats().debug_draw;
    assert_eq!((stats.drawn, stats.list.lines, stats.over_limit), (1, 1, 0));
    assert_eq!(stats.pipelines, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_line_keeps_its_width_on_screen_at_any_distance_and_widens_with_the_target() {
    let gpu = gpu();
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    // Pixels across a horizontal line through a pixel's middle, at a target's middle column.
    let width_on = |size: (u32, u32), z: f32| {
        let texture = target(&gpu, size);
        let mut scene = Scene::new(&gpu, settings, size);
        // A row near the top, so it's over the sky, moved along the view ray to `z`.
        let (row, column) = (size.1 / 10, size.0 / 2);
        let scale = (EYE.z - z) / EYE.z;
        let world = |x| {
            let on_plane = world_at(size, x, row);
            Vec3::new(on_plane.x * scale, on_plane.y * scale, z)
        };
        let lines = [line(world(size.0 / 4), world(size.0 * 3 / 4), Depth::OnTop)];
        let pixels = scene.render(&gpu, &texture, &lines);
        (row - 8..row + 8).map(|y| pixels.line_at((column, y)).clamp(0.0, 1.0)).sum::<f32>()
    };
    for (size, rows) in [(SIZE, 360), ((1280, 720), 720), ((2560, 1440), 1440)] {
        let expected = line_width(rows);
        for z in [0.0, 4.0, -40.0] {
            let width = width_on(size, z);
            // Summing a soft edge's samples across a line gives its width, wherever it falls.
            assert!((width - expected).abs() < 0.1, "{size:?} at z {z}: {width} against {expected}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_line_crossing_the_near_plane_is_cut_there() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    // From a point up and to the left, towards and past the camera, on over its shoulder.
    let start = Vec3::new(-2.0, 1.2, 0.0);
    let behind = Vec3::new(-2.0, 1.2, EYE.z + 4.0);
    // Whichever end is the one behind the camera.
    for lines in [line(start, behind, Depth::OnTop), line(behind, start, Depth::OnTop)] {
        let pixels = scene.render(&gpu, &texture, &[lines]);
        // In front of the camera it runs from its start out to the frame's edge.
        let (x, y) = pixel_of(start);
        assert!(pixels.line_near((x, y)), "{x} {y}");
        let along = start + Vec3::Z * 2.0;
        assert!(pixels.line_near(pixel_of(along)), "{:?}", pixel_of(along));
        // An end behind the camera mirrors through it if divided by its w: the line would then run
        // from the start towards the frame's middle and on.
        let toward_middle = (x + (SIZE.0 / 2 - x) / 2, y + (SIZE.1 / 2 - y) / 2);
        assert!(!pixels.line_near(toward_middle), "{toward_middle:?}");
        assert!(!pixels.line_near((SIZE.0 / 2, SIZE.1 / 2)));
    }

    // Wholly behind the camera, nothing draws, nor does a line from the eye's own position.
    let behind = [
        line(Vec3::new(-1.0, 1.0, 8.0), Vec3::new(1.0, 1.0, 8.0), Depth::OnTop),
        line(Vec3::new(0.0, 0.0, 7.0), Vec3::new(0.0, 0.0, 9.0), Depth::OnTop),
    ];
    assert_eq!(scene.render(&gpu, &texture, &behind).line_pixels(), 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_line_of_no_length_draws_a_dot() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    let at = world_at(SIZE, 100, 40);
    let pixels = scene.render(&gpu, &texture, &[line(at, at, Depth::Tested)]);
    assert!(pixels.is((100, 40), LINE));
    assert!(pixels.line_pixels() <= 9, "{}", pixels.line_pixels());
}

#[test]
#[ignore = "needs a GPU"]
fn alpha_blends_the_line_over_the_frame() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    let (from, to) = (world_at(SIZE, 100, 40), world_at(SIZE, 200, 40));
    let sky = scene.render(&gpu, &texture, &[]).at((150, 40));
    let half = DebugLine { from, to, color: LINE.with_alpha(128), depth: Depth::Tested };
    let blended = scene.render(&gpu, &texture, &[half]).at((150, 40));
    let opaque = LINE;
    // In linear light, as an sRGB target blends.
    let expected = |sky_byte, line_byte| linear(sky_byte) * 0.5 + linear(line_byte) * 0.5;
    for (channel, line_byte) in [opaque.r, opaque.g, opaque.b].into_iter().enumerate() {
        let (got, want) = (linear(blended[channel]), expected(sky[channel], line_byte));
        assert!((got - want).abs() < 0.015, "channel {channel}: {got} against {want}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn depth_tested_hides_behind_a_surface_and_on_top_doesnt() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let middle = (SIZE.0 / 2, SIZE.1 / 2);
    // The sphere covers the middle of the frame; each line runs across it, out over the sky.
    let across = |z: f32, depth| line(Vec3::new(-5.0, 0.0, z), Vec3::new(5.0, 0.0, z), depth);
    let sky_side = pixel_of(Vec3::new(-2.0, 0.0, 0.0));
    for (mode, settings, scale) in modes() {
        let mut scene = Scene::new(&gpu, settings, SIZE);
        scene.renderer.set_render_scale(scale);
        // Temporal AA settles first, as a frame that isn't its first would.
        for _ in 0..8 {
            scene.complete(&gpu, &texture, &[]);
        }
        assert_eq!(scene.render(&gpu, &texture, &[]).line_pixels(), 0, "{mode}: nothing drawn");

        let behind = scene.render(&gpu, &texture, &[across(-2.0, Depth::Tested)]);
        assert!(!behind.line_near(middle), "{mode}: hidden behind the sphere");
        assert!(behind.line_near(sky_side), "{mode}: shows beside it, over the sky");

        let on_top = scene.render(&gpu, &texture, &[across(-2.0, Depth::OnTop)]);
        assert!(on_top.line_near(middle), "{mode}: on top reads through the sphere");
        assert!(on_top.line_near(sky_side), "{mode}");

        let in_front = scene.render(&gpu, &texture, &[across(2.0, Depth::Tested)]);
        assert!(in_front.line_near(middle), "{mode}: in front of the sphere, it shows");

        let stats = scene.renderer.stats().debug_draw;
        assert_eq!(stats.drawn, 1, "{mode}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_line_on_a_surface_shows_and_one_just_inside_it_doesnt() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let middle = (SIZE.0 / 2, SIZE.1 / 2);
    for (mode, settings, scale) in modes() {
        let mut scene = Scene::new(&gpu, settings, SIZE);
        scene.renderer.set_render_scale(scale);
        for _ in 0..8 {
            scene.complete(&gpu, &texture, &[]);
        }
        // Short, so it stays against the sphere's near pole.
        let across = |z: f32| line(Vec3::new(-0.05, 0.0, z), Vec3::new(0.05, 0.0, z), Depth::Tested);
        assert!(scene.render(&gpu, &texture, &[across(1.0)]).line_near(middle), "{mode}: on the surface");
        // The margin at 5 m is 2 cm, so what rests on a surface doesn't fight it.
        assert!(scene.render(&gpu, &texture, &[across(0.995)]).line_near(middle), "{mode}: 5 mm inside");
        assert!(!scene.render(&gpu, &texture, &[across(0.9)]).line_near(middle), "{mode}: 10 cm inside");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn lines_hold_still_under_temporal_aa() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    // One over the sky, one across the sphere's front and one going behind its edge.
    let lines = [
        line(world_at(SIZE, 100, 40), world_at(SIZE, 500, 60), Depth::Tested),
        line(Vec3::new(-0.3, 0.3, 2.0), Vec3::new(0.3, 0.2, 2.0), Depth::Tested),
        line(Vec3::new(-0.2, -0.2, -0.5), Vec3::new(2.0, -0.2, -0.5), Depth::Tested),
    ];
    for _ in 0..14 {
        scene.complete(&gpu, &texture, &lines);
    }
    let previous = scene.render(&gpu, &texture, &lines).rgba;
    let last = scene.render(&gpu, &texture, &lines).rgba;
    // Where the line is whole, whatever the jitter does to the frame under it.
    let solid = |pixel: &[u8; 4]| i32::from(pixel[2]) - i32::from(pixel[1]) > 150;
    let solid_pixels = last.iter().filter(|pixel| solid(pixel)).count();
    assert!(solid_pixels > 200, "{solid_pixels}");
    let moved = last.iter().zip(&previous).filter(|(a, b)| solid(a) != solid(b)).count();
    // The third line's end at the sphere's silhouette moves with the depth's jitter (module docs).
    assert!(moved <= 6, "{moved} of {solid_pixels} pixels went in or out of the line between frames");
}

#[test]
#[ignore = "needs a GPU"]
fn a_frame_with_nothing_builds_nothing_and_adds_no_pass() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let held = || {
        let report = gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        (report.hub.render_pipelines.num_kept_from_user, report.hub.compute_pipelines.num_kept_from_user)
    };
    // A setting whose frames need no scene prepass of their own, so this can see one turn up.
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    for _ in 0..3 {
        scene.render(&gpu, &texture, &[]);
    }
    let without = held();
    eprintln!("{without:?} pipelines with nothing drawn");
    for _ in 0..3 {
        scene.render(&gpu, &texture, &[]);
    }
    assert_eq!(held(), without, "frames with nothing drawn build nothing");
    assert!(!scene.ran(PASS));
    assert!(!scene.ran(SCENE_PREPASS), "the scene prepass stays culled");
    let stats = scene.renderer.stats().debug_draw;
    assert_eq!(stats.pipelines, 0);
    assert_eq!(stats.buffer_bytes, 0);

    let lines = [line(Vec3::new(-1.0, 1.5, 0.0), Vec3::new(1.0, 1.5, 0.0), Depth::Tested)];
    scene.render(&gpu, &texture, &lines);
    // Its own, and the scene prepass's plain one and the hulls', which only now can run.
    let with_lines = (without.0 + 3, without.1);
    assert_eq!(held(), with_lines, "the first frame with lines builds what it reads");
    assert!(scene.ran(PASS));
    assert!(scene.ran(SCENE_PREPASS), "tested lines read its depth");
    let stats = scene.renderer.stats().debug_draw;
    assert_eq!(stats.pipelines, 1);
    assert!(stats.buffer_bytes > 0);

    // Many more lines, and none again: no new pipeline, and no pass or prepass once none draw.
    let many = vec![lines[0]; 5_000];
    scene.render(&gpu, &texture, &many);
    assert_eq!(scene.renderer.stats().debug_draw.drawn, 5_000);
    assert_eq!(held(), with_lines);
    scene.render(&gpu, &texture, &[]);
    assert_eq!(held(), with_lines);
    assert!(!scene.ran(PASS));
    assert!(!scene.ran(SCENE_PREPASS));
    assert_eq!(scene.renderer.stats().debug_draw.drawn, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_lists_shapes_draw_through_the_pass() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    let mut list = DebugDrawList::default();
    let center = Vec3::new(-2.0, 0.0, 0.0);
    list.shape(Shape::Sphere { center, radius: 0.5 }, LINE, Depth::Tested);
    let pixels = scene.render(&gpu, &texture, &list.lines);
    // Its circle facing the camera passes through each of these.
    for offset in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y] {
        assert!(pixels.line_near(pixel_of(center + offset * 0.5)), "{offset}");
    }
    assert!(!pixels.line_near(pixel_of(center + Vec3::new(0.2, 0.2, 0.0))), "hollow");
}

#[test]
#[ignore = "needs a GPU"]
fn a_shader_edit_rebuilds_its_pipeline_once_it_is_built() {
    let gpu = gpu();
    let texture = target(&gpu, SIZE);
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut scene = Scene::new(&gpu, settings, SIZE);
    let lines = [line(world_at(SIZE, 100, 40), world_at(SIZE, 200, 40), Depth::OnTop)];
    let original = ShaderFile::DebugDraw.compiled_in();
    let anchor = "return vec4<f32>(in.color.rgb, in.color.a * coverage);";
    assert!(original.contains(anchor));
    let edit = |color: &str| {
        let mut sources = ShaderSources::compiled_in();
        sources.set(ShaderFile::DebugDraw, original.replace(anchor, color));
        sources
    };
    let green = edit("return vec4<f32>(0.0, 1.0, 0.0, coverage);");

    // Nothing built yet: the program counts, and no pipeline.
    scene.render(&gpu, &texture, &[]);
    let update = scene.renderer.set_shader_sources(&green).expect("a valid edit");
    assert_eq!((update.programs, update.pipelines), (1, 0), "built only when its work can run");
    scene.renderer.set_shader_sources(&ShaderSources::compiled_in()).expect("back");

    assert!(scene.render(&gpu, &texture, &lines).is((150, 40), LINE));
    let update = scene.renderer.set_shader_sources(&green).expect("a valid edit");
    assert_eq!((update.programs, update.pipelines), (1, 1));
    let edited = Color::rgb(0, 255, 0);
    assert!(scene.render(&gpu, &texture, &lines).is((150, 40), edited));

    // A broken one names its file and changes nothing.
    let broken = edit("return vec4<f32>(0.0, 1.0, 0.0, coverag);");
    let Err(ShaderError::Invalid { file, .. }) = scene.renderer.set_shader_sources(&broken) else {
        panic!("a compile error");
    };
    assert_eq!(file, ShaderFile::DebugDraw);
    assert!(scene.render(&gpu, &texture, &lines).is((150, 40), edited));
    // An edit of `frame.wgsl`, which this program doesn't include, leaves it alone.
    assert!(!ShaderFile::DebugDraw.compiled_in().contains("struct FrameUniforms"));
}
