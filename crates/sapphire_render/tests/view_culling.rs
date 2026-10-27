//! Main-view culling (review RA-O01): the main pass, the scene prepass and the debug views skip
//! static environment and foliage instances wholly outside the main view's frustum, which must
//! change no pixel. Each test draws its frames with culling on and off and compares them byte for
//! byte. Run with `cargo test -p sapphire_render --test view_culling -- --ignored`.

use glam::{Mat4, Vec3};
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, FoliageGradient,
    FoliageMaterial, Material, MaterialDocument, Terminator, Translucency, Wind as MaterialWind,
};
use sapphire_assets::mesh::{
    FoliageVertex, MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DebugView, DirectionalLight, FramePacketRing,
    InstanceId, MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, Renderer,
    SkyGradient, ToneCurve, ViewCullingStats, ViewData, Vignette, Wind,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
const NEAR: f32 = 0.05;
/// The cameras' vertical field of view, radians.
const FOV: f32 = 0.9;

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

/// A lit environment material of `color` that glows a little.
fn environment(color: Vec3, alpha: AlphaMode) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: color.extend(0.5).to_array(),
        alpha,
        specular_clamp: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        emission: (color * 0.2).to_array(),
        ..EnvironmentMaterial::default()
    }
}

/// An opaque foliage card of `color` whose whole height sways with the wind's trunk level.
fn leaves(color: Vec3) -> FoliageMaterial {
    FoliageMaterial {
        base_color: color.extend(1.0).to_array(),
        alpha_cutoff: 0.0,
        wrap: 0.0,
        terminator_width: 0.0,
        terminator_growth: 0.0,
        shadow_tint: [1.0; 3],
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: [1.0; 3], tip: [1.0; 3] },
        wind: MaterialWind { sway: 1.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..FoliageMaterial::default()
    }
}

/// A square of `half` meters from its center to each edge, facing +z, standing on the origin,
/// with the trunk's whole sway where `swaying` (a foliage stream).
fn square(half: f32, swaying: bool) -> Mesh {
    let corner = |s: f32, t: f32| Vertex {
        position: [s * half, (t + 1.0) * half, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [(s + 1.0) * 0.5, (1.0 - t) * 0.5],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("part", placeholder)];
    let mut mesh =
        Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Unorm16);
    if swaying {
        let sway = FoliageVertex { sway: 1.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        mesh.streams = mesh.streams.clone().with_foliage(&[FoliageVertex::decode(sway.encode()); 4]);
    }
    mesh
}

/// What the scenes draw with: three environment materials (warm, cool, and a blended green
/// glass) and a swaying foliage card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Warm,
    Cool,
    Glass,
    Card,
}

/// One frame: where the camera is, whether it cut there, and the debug view shown.
#[derive(Clone, Copy, Debug)]
struct Shot {
    camera: CameraData,
    cut: bool,
    view: DebugView,
}

/// A camera at `eye` looking at `target`.
fn camera(eye: Vec3, target: Vec3) -> CameraData {
    CameraData::look_at(eye, target, Vec3::Y, FOV, 1.0, NEAR)
}

fn shot(eye: Vec3, target: Vec3) -> Shot {
    Shot { camera: camera(eye, target), cut: false, view: DebugView::Off }
}

/// Draws `instances` (kind, transform) through `shots` under `settings`, in a wind along −x
/// where `wind`: each frame's pixels and culling counts. Fails on any validation error.
fn render(
    settings: RenderSettings,
    instances: &[(Kind, Mat4)],
    shots: &[Shot],
    wind: bool,
) -> Vec<(Vec<u8>, ViewCullingStats)> {
    let gpu = GpuContext::new(
        &GpuInstance::new(&GpuInstanceDesc::default()).expect("instance"),
        &GpuContextDesc::default(),
        None,
    )
    .expect("device");
    let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
    let mut material = |bytes: Vec<u8>| -> MaterialHandle {
        renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    };
    let materials = [
        material(cook(&environment(Vec3::new(0.8, 0.5, 0.2), AlphaMode::Opaque))),
        material(cook(&environment(Vec3::new(0.2, 0.5, 0.8), AlphaMode::Opaque))),
        material(cook(&environment(Vec3::new(0.2, 0.9, 0.3), AlphaMode::Blend))),
        material(cook(&leaves(Vec3::new(0.9, 0.1, 0.1)))),
    ];
    let mut mesh = |mesh: Mesh, material: MaterialHandle| -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
    };
    let meshes = [
        mesh(square(0.1, false), materials[0]),
        mesh(square(0.1, false), materials[1]),
        mesh(square(0.1, false), materials[2]),
        mesh(square(0.25, true), materials[3]),
    ];
    let mut added: Vec<MeshInstance> = instances
        .iter()
        .enumerate()
        .map(|(id, &(kind, transform))| MeshInstance {
            id: InstanceId(id as u64),
            mesh: meshes[kind as usize],
            transform,
        })
        .collect();

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
    let mut frames = Vec::new();
    for (index, shot) in shots.iter().enumerate() {
        renderer.set_debug_view(shot.view);
        let packet = packets.begin();
        let mut main = ViewData::new(shot.camera);
        main.cut = shot.cut;
        packet.views.push(main);
        packet.time_seconds = 0.25 * index as f64;
        if wind {
            packet.wind = Wind { direction: Vec3::NEG_X, strength: 3.0, gustiness: 0.0 };
        }
        packet.sun = DirectionalLight {
            direction: Vec3::new(-0.3, -0.4, -1.0).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        };
        let sky = Vec3::new(0.05, 0.05, 0.1);
        packet.sky = SkyGradient { zenith: sky, horizon: sky, ground: sky };
        packet.ambient = AmbientLight { sky: Vec3::splat(0.2), ground: Vec3::splat(0.1), intensity: 1.0 };
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
        frames.push((read(&gpu, &target), renderer.stats().view_culling));
    }
    frames
}

/// `document` cooked, without textures.
fn cook<D: MaterialDocument>(document: &D) -> Vec<u8> {
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let material = Material::from_document(document, &swizzles).expect("a valid material");
    material.to_cooked().expect("cook").to_vec()
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

/// A deterministic generator: xorshift, uniform in `[low, high)`.
struct Rng(u64);

impl Rng {
    fn range(&mut self, low: f32, high: f32) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        low + (high - low) * ((self.0 >> 40) as f32 / (1u64 << 24) as f32)
    }
}

/// The review's fixture: 64 quads in view of a camera at (0, 0, 5) looking at the origin, and
/// 1 024 around it out of view (behind it, beside it, above and below), interleaved in slot order.
fn crowd() -> Vec<(Kind, Mat4)> {
    let mut rng = Rng(0x5eed_00a1);
    let kinds = [Kind::Warm, Kind::Cool, Kind::Glass];
    (0..1088)
        .map(|index: u32| {
            let kind = kinds[index as usize % 3];
            let position = if index.is_multiple_of(17) {
                let cell = index / 17;
                Vec3::new(
                    (cell % 8) as f32 * 0.35 - 1.225,
                    (cell / 8) as f32 * 0.35 - 1.4,
                    -0.01 * (cell % 5) as f32,
                )
            } else {
                match index % 4 {
                    0 => Vec3::new(rng.range(-10.0, 10.0), rng.range(-10.0, 10.0), rng.range(7.0, 30.0)),
                    1 => Vec3::new(rng.range(-60.0, -40.0), rng.range(-5.0, 5.0), rng.range(-30.0, 0.0)),
                    2 => Vec3::new(rng.range(40.0, 60.0), rng.range(-5.0, 5.0), rng.range(-30.0, 0.0)),
                    _ => Vec3::new(
                        rng.range(-10.0, 10.0),
                        rng.range(40.0, 60.0) * if index % 8 == 3 { 1.0 } else { -1.0 },
                        rng.range(-30.0, 0.0),
                    ),
                }
            };
            (kind, Mat4::from_translation(position))
        })
        .collect()
}

fn settings(view_culling: bool) -> RenderSettings {
    RenderSettings { view_culling, ..RenderSettings::default() }
}

/// TAA's settings: MSAA off and a jittered projection.
fn jittered(view_culling: bool) -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, ..settings(view_culling) }
}

/// Draws `shots` with culling on and off and checks every frame is the same, byte for byte:
/// the frames with culling on.
fn same_with_and_without(
    settings: fn(bool) -> RenderSettings,
    instances: &[(Kind, Mat4)],
    shots: &[Shot],
    wind: bool,
) -> Vec<(Vec<u8>, ViewCullingStats)> {
    let culled = render(settings(true), instances, shots, wind);
    let whole = render(settings(false), instances, shots, wind);
    for (frame, ((culled, stats), (whole, off))) in culled.iter().zip(&whole).enumerate() {
        assert_eq!(off.tested + off.culled, 0, "culling off tests nothing");
        let differ = culled.chunks(4).zip(whole.chunks(4)).filter(|(a, b)| a != b).count();
        assert!(differ == 0, "frame {frame}: {differ} pixels differ with culling ({stats:?})");
    }
    culled
}

#[test]
#[ignore = "needs a GPU"]
fn offscreen_instances_are_culled_and_change_no_pixel() {
    let shots = [shot(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO)];
    let frames = same_with_and_without(settings, &crowd(), &shots, false);
    let (pixels, stats) = &frames[0];
    assert_eq!(*stats, ViewCullingStats { tested: 1088, culled: 1024, drawn: 64 });
    // The quads in view show: not the sky alone.
    let sky = &pixels[..4];
    assert!(pixels.chunks(4).filter(|pixel| pixel != &sky).count() > 1000);
}

#[test]
#[ignore = "needs a GPU"]
fn instances_across_the_frustums_edges_draw() {
    // Quads straddling each edge of the view, quads just past each edge by less than the
    // padding, and quads well past each edge, 5 m from a camera at the origin looking down −z.
    let half_width = 5.0 * (FOV * 0.5).tan();
    let pixel = 2.0 * half_width / SIDE as f32;
    let mut instances = Vec::new();
    for (side, axis) in [(1.0, Vec3::X), (-1.0, Vec3::X), (1.0, Vec3::Y), (-1.0, Vec3::Y)] {
        let edge = axis * side * half_width - Vec3::Z * 5.0;
        // The squares are 0.2 m on a side, standing on their origin: their bounds' sphere is
        // centered 0.1 m up with a radius of 0.141 m.
        let center = Vec3::Y * 0.1;
        let out = |gap: f32| Mat4::from_translation(edge + axis * side * (0.1414 + gap) - center);
        instances.push((Kind::Warm, Mat4::from_translation(edge - center)));
        instances.push((Kind::Cool, out(0.5 * pixel)));
        instances.push((Kind::Glass, out(1.5 * pixel)));
        instances.push((Kind::Warm, out(1.0)));
    }
    let shots = [shot(Vec3::ZERO, Vec3::NEG_Z)];
    let frames = same_with_and_without(settings, &instances, &shots, false);
    // Only those well past their edge are culled: the padding keeps the ones a pixel or two out.
    assert_eq!(frames[0].1, ViewCullingStats { tested: 16, culled: 4, drawn: 12 });
}

#[test]
#[ignore = "needs a GPU"]
fn jittered_frames_and_a_panning_camera_draw_the_same() {
    // Temporal AA's jitter moves the projection by up to half a pixel, and its history carries
    // every frame into the next: a camera panning across the crowd brings quads in and out of
    // view at its edges.
    let shots: Vec<Shot> = (0..12)
        .map(|frame| shot(Vec3::new(0.15 * frame as f32, 0.0, 5.0), Vec3::new(0.3 * frame as f32, 0.0, 0.0)))
        .collect();
    let frames = same_with_and_without(jittered, &crowd(), &shots, false);
    let culled: Vec<u32> = frames.iter().map(|(_, stats)| stats.culled).collect();
    assert!(culled.iter().all(|&culled| culled >= 1024), "{culled:?}");
    assert!(culled.first() != culled.last(), "the pan culls more of the grid by its end: {culled:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn a_camera_cut_culls_by_the_new_view_at_once() {
    // Looking at the grid, then cut to look at the crowd to the right, whose quads were culled
    // the frame before, and back.
    let ahead = shot(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
    let right = Shot { cut: true, ..shot(Vec3::new(0.0, 0.0, 5.0), Vec3::new(50.0, 0.0, -15.0)) };
    let back = Shot { cut: true, ..ahead };
    let shots = [ahead, ahead, right, right, back];
    let frames = same_with_and_without(jittered, &crowd(), &shots, false);
    let culled: Vec<u32> = frames.iter().map(|(_, stats)| stats.culled).collect();
    assert_eq!(culled[0], 1024);
    assert!(culled[2] < 1024 && culled[2] > 64, "the right side's quads are drawn: {culled:?}");
    assert_eq!(culled[4], 1024);
}

#[test]
#[ignore = "needs a GPU"]
fn debug_views_replay_what_the_main_pass_draws() {
    let at = |view| Shot { view, ..shot(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO) };
    let shots = [
        at(DebugView::Wireframe),
        at(DebugView::Overdraw),
        at(DebugView::Lights),
        at(DebugView::MaterialIds),
    ];
    let frames = same_with_and_without(settings, &crowd(), &shots, false);
    assert!(frames.iter().all(|(_, stats)| stats.culled == 1024));
}

#[test]
#[ignore = "needs a GPU"]
fn foliage_the_wind_carries_into_view_is_drawn() {
    // A swaying card just past the view's right edge, 5 m ahead, and a wind along −x that carries
    // it 0.6 to 3 m in (its trunk level's lean and swing, at strength 3): its bounds lie outside the
    // padded frustum, 0.15 m past its edge, but the wind's margin keeps it.
    let half_width = 5.0 * (FOV * 0.5).tan();
    // The card is 0.5 m on a side: its sphere is centered 0.25 m up, 0.354 m in radius.
    let card = Mat4::from_translation(Vec3::new(half_width + 0.354 + 0.15, -0.25, -5.0));
    let ground = (Kind::Cool, Mat4::from_translation(Vec3::new(0.0, -0.3, -5.0)));
    let shots: Vec<Shot> = (0..4).map(|_| shot(Vec3::ZERO, Vec3::NEG_Z)).collect();
    let frames = same_with_and_without(settings, &[(Kind::Card, card), ground], &shots, true);
    for (frame, (pixels, stats)) in frames.iter().enumerate() {
        assert_eq!(*stats, ViewCullingStats { tested: 2, culled: 0, drawn: 2 }, "frame {frame}");
        let red = |pixel: &[u8]| u16::from(pixel[0]) > 2 * u16::from(pixel[1].max(pixel[2])) && pixel[0] > 60;
        let red = pixels.chunks(4).filter(|pixel| red(pixel)).count();
        assert!(red > 20, "frame {frame}: the card is in view ({red} pixels)");
    }
    // Calm, it stays out of view, and is culled.
    let frames = same_with_and_without(settings, &[(Kind::Card, card), ground], &shots[..1], false);
    assert_eq!(frames[0].1, ViewCullingStats { tested: 2, culled: 1, drawn: 1 });
}
