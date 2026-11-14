//! Wet surfaces' tests that need a GPU (M5 L4): the frame's weather darkens a porous surface and leaves a
//! non-porous one as it was, glosses a surface more the wetter it is, stands puddles that reflect the probe
//! they lie in (a closed red room's floor goes red) and the sky beyond every probe, on Mobile-High's
//! approximate specular as on desktop's; Mobile-Low only darkens; a dry frame draws to the byte as one
//! that never saw weather; going wet builds nothing where it was declared, the wet surfaces as first uses
//! where it wasn't, and nothing across a tier change; wetness just above zero draws as dry; and the puddle
//! mask is the world's, so it holds still as the camera moves. Run with
//! `cargo test -p sapphire_render --test gpu wet:: -- --ignored --test-threads=1`.
//!
//! Post shows linear values as they are; the sky, the hemisphere and the key are each set per test so a
//! pixel holds only the light the test reads.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::material::{
    DistanceStylization, EnvironmentMaterial, Material, MaterialDocument, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::reflections::{ReflectionProbe, ReflectionProbes};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, EnvironmentSettings,
    EnvironmentSpecular, FramePacketRing, InstanceId, MaterialHandle, MeshInstance, Msaa, PipelineInventory,
    PostParams, ProbeKeys, ProbeLookup, ReflectionSettings, RenderSettings, RenderStats, Renderer,
    SkyGradient, ToneCurve, ViewData, Vignette, Weather,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIDE: u32 = 96;
/// The probes' cook size.
const SIZE: u32 = 32;
/// The camera's vertical field of view.
const FOV: f32 = 0.9;
/// Where the floor's checked, seen from [`Harness::new`]'s camera.
const POINTS: [Vec3; 3] = [Vec3::ZERO, Vec3::new(0.6, 0.0, -0.6), Vec3::new(-0.8, 0.0, 0.4)];

const WET: Weather = Weather { wetness: 1.0, puddles: 0.0 };
const FLOODED: Weather = Weather { wetness: 1.0, puddles: 1.0 };

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

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// No MSAA or temporal AA, environment specular as `specular`.
fn settings(specular: EnvironmentSpecular) -> RenderSettings {
    let probes =
        if specular == EnvironmentSpecular::Off { ProbeLookup::PerVertex } else { ProbeLookup::PerPixel };
    RenderSettings {
        msaa: Msaa::Off,
        environment: EnvironmentSettings { specular, probes },
        reflections: ReflectionSettings { cube_size: 128 },
        ..RenderSettings::default()
    }
}

/// A flat surface of `color`, `roughness` and `metallic`, its porosity as authored, a hard terminator
/// and no distance stylization.
fn surface(color: f32, roughness: f32, metallic: f32, porosity: Option<f32>) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [color, color, color, 1.0],
        roughness,
        metallic,
        porosity,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

/// A floor `half` metres from its middle to each side, at height 0.
fn floor(half: f32) -> Mesh {
    let vertex = |position: Vec3| Vertex {
        position: position.to_array(),
        normal: Vec3::Y.to_array(),
        outline_normal: Vec3::Y.to_array(),
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let corners = [
        Vec3::new(-half, 0.0, -half),
        Vec3::new(-half, 0.0, half),
        Vec3::new(half, 0.0, half),
        Vec3::new(half, 0.0, -half),
    ];
    let vertices: Vec<Vertex> = corners.into_iter().map(vertex).collect();
    let document = EnvironmentMaterial::default();
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let placeholder = Material::from_document(&document, &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        vec![0, 1, 2, 0, 2, 3],
        submeshes,
        vec![MaterialSlot::embedded("floor", placeholder)],
        PositionFormat::Float32,
    )
}

/// One probe of every face `color`, closed to the sky, reaching the box `min` to `max` from `position`.
fn probe(position: Vec3, min: Vec3, max: Vec3, color: Vec3) -> ReflectionProbes {
    let texel = [color.x, color.y, color.z, 0.0].map(|value| f16::from_f32(value).to_bits());
    ReflectionProbes {
        size: SIZE,
        key_hours: vec![12.0],
        probes: vec![ReflectionProbe {
            name: "room".to_owned(),
            position: position.to_array(),
            parallax_min: min.to_array(),
            parallax_max: max.to_array(),
        }],
        texels: std::iter::repeat_n(texel, 6 * (SIZE * SIZE) as usize).flatten().collect(),
    }
}

/// A red room about the floor's middle, the floor on its box's lowest face.
fn red_room() -> ReflectionProbes {
    probe(Vec3::new(0.0, 1.5, 0.0), Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 4.0, 4.0), RED)
}

const RED: Vec3 = Vec3::new(0.5, 0.02, 0.02);

/// The light a frame holds besides the floor's material: the sky and hemisphere (`lit_sky`, or black),
/// and the key at `key` intensity, shining toward `key_direction`.
#[derive(Clone, Copy)]
struct Light {
    lit_sky: bool,
    key: f32,
    key_direction: Vec3,
}

/// A key from above and nothing else: what's drawn is the diffuse.
const KEY_ONLY: Light = Light { lit_sky: false, key: 1.0, key_direction: Vec3::NEG_Y };
/// The sky and its hemisphere, no key: what's drawn beyond the hemisphere's diffuse is reflection.
const SKY_ONLY: Light = Light { lit_sky: true, key: 0.0, key_direction: Vec3::NEG_Y };
/// Nothing but what a probe gives.
const DARK: Light = Light { lit_sky: false, key: 0.0, key_direction: Vec3::NEG_Y };

struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    stats: RenderStats,
}

impl Frame {
    fn at(&self, point: Vec3) -> Vec3 {
        let ndc = (self.camera.proj * self.camera.view).project_point3(point);
        let (x, y) = ((ndc.x + 1.0) * 0.5 * SIDE as f32, (1.0 - ndc.y) * 0.5 * SIDE as f32);
        let pixel = self.pixels[(y as u32 * SIDE + x as u32) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// The largest difference from `other` in any channel of any pixel, in code values.
    fn differs_from(&self, other: &Frame) -> u8 {
        let pairs = self.pixels.iter().zip(&other.pixels);
        pairs.flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]))).max().unwrap_or(0)
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y * SIDE + x) as usize]
    }
}

struct Harness {
    gpu: Arc<GpuContext>,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    added: Vec<MeshInstance>,
    floor_material: MaterialHandle,
    weather: Weather,
    light: Light,
}

impl Harness {
    /// A renderer of `settings` with a floor `half` metres to a side in `material`, seen from above and
    /// to one side.
    fn new(settings: RenderSettings, material: &EnvironmentMaterial, half: f32, light: Light) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = Arc::new(GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device"));
        let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
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
        let floor_material = upload_material(&mut renderer, material);
        let bytes = floor(half).to_cooked().expect("cook");
        let mesh =
            renderer.upload_mesh(Mesh::read(&bytes).expect("valid"), &[floor_material]).expect("upload");
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: CameraData::look_at(Vec3::new(0.0, 2.0, 2.5), Vec3::ZERO, Vec3::Y, FOV, 1.0, 0.05),
            added: vec![MeshInstance { id: InstanceId(0), mesh, transform: Mat4::IDENTITY }],
            floor_material,
            weather: Weather::DRY,
            light,
        }
    }

    /// Gives the floor `material` in place of its own.
    fn repaint(&mut self, material: &EnvironmentMaterial) {
        let swizzles = vec![Swizzle::IDENTITY; material.textures().len()];
        let bytes = Material::from_document(material, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer
            .update_material(self.floor_material, Material::read(&bytes).expect("read"), |_| None)
            .expect("update");
    }

    /// Uploads and shows `probes`.
    fn show(&mut self, probes: &ReflectionProbes) {
        let bytes = probes.to_cooked().expect("cook");
        let handle = self
            .renderer
            .upload_reflection_probes(ReflectionProbes::read(&bytes).expect("valid"))
            .expect("fits");
        self.renderer.set_reflection_probes(Some(handle)).expect("shown");
    }

    fn draw(&mut self) {
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight {
            direction: self.light.key_direction,
            color: Vec3::ONE,
            intensity: self.light.key,
        };
        let (sky, ambient) = if self.light.lit_sky {
            (
                SkyGradient {
                    zenith: Vec3::new(0.05, 0.15, 0.6),
                    horizon: Vec3::new(0.2, 0.3, 0.5),
                    ground: Vec3::new(0.05, 0.05, 0.05),
                },
                AmbientLight { sky: Vec3::new(0.1, 0.2, 0.5), ground: Vec3::splat(0.05), intensity: 1.0 },
            )
        } else {
            (
                SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO },
                AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            )
        };
        packet.sky = sky;
        packet.ambient = ambient;
        packet.weather = self.weather;
        packet.probe_keys = ProbeKeys::single(0);
        packet.reflection_keys = ProbeKeys::single(0);
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }

    /// Renders `frames` frames in `weather` and reads the last back.
    fn settle(&mut self, weather: Weather, frames: u32) -> Frame {
        self.weather = weather;
        for _ in 0..frames {
            self.draw();
        }
        Frame { pixels: self.read_back(), camera: self.camera, stats: self.renderer.stats() }
    }

    fn read_back(&self) -> Vec<[u8; 4]> {
        let padded = (SIDE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIDE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIDE),
                },
            },
            self.target.size(),
        );
        self.gpu.queue().submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = buffer.get_mapped_range(..).expect("mapped");
        mapped
            .chunks_exact(padded as usize)
            .flat_map(|row| row[..SIDE as usize * 4].as_chunks::<4>().0.iter().copied())
            .collect()
    }

    /// Changes to a tier of `settings`, as the stage does at a pause (M4 T3).
    fn change_tier(&mut self, settings: RenderSettings) {
        let mut build = self.renderer.build_tier(&self.gpu, settings).expect("builds");
        for _ in 0..600 {
            if build.is_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.renderer.take_tier(build, None).expect("taken");
    }

    /// Render and compute pipelines the backend holds.
    fn pipelines(&self) -> [usize; 2] {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        [report.hub.render_pipelines.num_kept_from_user, report.hub.compute_pipelines.num_kept_from_user]
    }
}

fn upload_material(renderer: &mut Renderer, document: &EnvironmentMaterial) -> MaterialHandle {
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
    renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
}

/// `wet` over `dry` at each of [`POINTS`], channel by channel, the dry value at least `floor`.
fn ratios(wet: &Frame, dry: &Frame, floor: f32) -> Vec<Vec3> {
    POINTS
        .iter()
        .map(|&point| {
            let dry = dry.at(point);
            assert!(dry.min_element() >= floor, "a dry floor bright enough to measure: {dry}");
            wet.at(point) / dry
        })
        .collect()
}

/// A soaked grey's share of its dry self: `1 - WET_DARKENING` (`environment.wgsl`'s `wet_albedo`).
const SOAKED: f32 = 0.6;

#[test]
#[ignore = "needs a GPU"]
fn a_porous_surface_darkens_and_a_non_porous_one_does_not() {
    // Lit by a key from straight above alone, with no sky to reflect, a pixel is the diffuse (the
    // highlight lies far from the camera's reflection, and a wet film's narrows it further).
    let porous = surface(0.5, 0.9, 0.0, Some(1.0));
    let mut harness = Harness::new(settings(EnvironmentSpecular::Full), &porous, 3.0, KEY_ONLY);
    let dry = harness.settle(Weather::DRY, 2);
    let wet = harness.settle(WET, 2);
    for ratio in ratios(&wet, &dry, 0.2) {
        assert!((ratio - Vec3::splat(SOAKED)).abs().max_element() < 0.04, "porous, soaked: {ratio}");
    }
    let half = harness.settle(Weather { wetness: 0.5, puddles: 0.0 }, 2);
    for ratio in ratios(&half, &dry, 0.2) {
        assert!(ratio.x > SOAKED + 0.1 && ratio.x < 0.95, "porous, half wet, darkens less: {ratio}");
    }

    // Authored dry, and left to the surface: a smooth dielectric takes no water in. (Their dry
    // highlights, faint this far from the key's reflection, go as the film smooths them.)
    for (what, material) in [
        ("authored non-porous", surface(0.5, 0.9, 0.0, Some(0.0))),
        ("a smooth dielectric", surface(0.5, 0.3, 0.0, None)),
    ] {
        harness.repaint(&material);
        let dry = harness.settle(Weather::DRY, 2);
        let wet = harness.settle(WET, 2);
        for ratio in ratios(&wet, &dry, 0.2) {
            assert!(ratio.min_element() > 0.95 && ratio.max_element() < 1.01, "{what}: {ratio}");
        }
    }

    // A rough dielectric left to the surface is porous.
    harness.repaint(&surface(0.5, 0.95, 0.0, None));
    let dry = harness.settle(Weather::DRY, 2);
    let wet = harness.settle(WET, 2);
    for ratio in ratios(&wet, &dry, 0.2) {
        assert!(ratio.x < 0.7, "a rough dielectric, derived porous: {ratio}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gloss_rises_with_wetness() {
    // A black, rough, non-porous floor in a red room reflects more of it the wetter it is: its diffuse
    // is nothing, and it doesn't darken, so a pixel is its reflection.
    let floor = surface(0.0, 0.9, 0.0, Some(0.0));
    let mut harness = Harness::new(settings(EnvironmentSpecular::Full), &floor, 3.0, DARK);
    harness.show(&red_room());
    let mut previous = [0.0; POINTS.len()];
    for wetness in [0.0, 0.5, 1.0] {
        let frame = harness.settle(Weather { wetness, puddles: 0.0 }, 2);
        for (index, &point) in POINTS.iter().enumerate() {
            let red = frame.at(point).x;
            assert!(red > previous[index] + 0.003, "wetness {wetness}: {red} after {}", previous[index]);
            previous[index] = red;
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_puddle_reflects_its_probe_and_the_sky_beyond_every_probe() {
    for specular in [EnvironmentSpecular::Full, EnvironmentSpecular::Approximate] {
        // A black, rough floor flooded: every flat place a puddle, a mirror of water.
        let floor = surface(0.0, 0.9, 0.0, None);
        let mut harness = Harness::new(settings(specular), &floor, 3.0, SKY_ONLY);
        let sky_dry = harness.settle(Weather::DRY, 2);
        let sky = harness.settle(FLOODED, 2);
        for &point in &POINTS {
            let (dry, wet) = (sky_dry.at(point), sky.at(point));
            assert!(wet.z > dry.z + 0.01, "{specular:?}: the sky in a puddle, {wet} against {dry} dry");
            assert!(wet.z > 1.5 * wet.x, "{specular:?}: blue: {wet}");
        }

        // A probe whose reach is elsewhere leaves the puddle the sky's.
        let far = Vec3::new(30.0, 0.0, 0.0);
        harness.show(&probe(far + Vec3::Y * 1.5, far - Vec3::splat(4.0), far + Vec3::splat(4.0), RED));
        let beyond = harness.settle(FLOODED, 2);
        assert!(beyond.differs_from(&sky) <= 1, "{specular:?}: beyond every probe, the sky's reflection");

        // Its own probe's red, where dry the floor reflected little of it.
        let mut harness = Harness::new(settings(specular), &floor, 3.0, DARK);
        harness.show(&red_room());
        let dry = harness.settle(Weather::DRY, 2);
        let flooded = harness.settle(FLOODED, 2);
        for &point in &POINTS {
            let (dry, wet) = (dry.at(point), flooded.at(point));
            assert!(
                wet.x > dry.x + 0.01 && wet.x > 5.0 * wet.y,
                "{specular:?}: red in the puddle: {wet}, {dry} dry"
            );
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn mobile_low_only_darkens() {
    // Without specular, a flooded street darkens as a wet one would and shows no gloss or puddle: a
    // non-porous floor in a red room looks as it did dry.
    let porous = surface(0.5, 0.9, 0.0, Some(1.0));
    let mut harness = Harness::new(settings(EnvironmentSpecular::Off), &porous, 3.0, KEY_ONLY);
    let dry = harness.settle(Weather::DRY, 2);
    let flooded = harness.settle(FLOODED, 2);
    for ratio in ratios(&flooded, &dry, 0.2) {
        assert!((ratio - Vec3::splat(SOAKED)).abs().max_element() < 0.04, "soaked: {ratio}");
    }
    harness.show(&red_room());
    // A metal, which shades in its own color here, is left non-porous by the surface.
    for (what, material) in
        [("non-porous", surface(0.5, 0.9, 0.0, Some(0.0))), ("a metal", surface(0.5, 0.9, 1.0, None))]
    {
        harness.repaint(&material);
        let dry = harness.settle(Weather::DRY, 2);
        let flooded = harness.settle(FLOODED, 2);
        let worst = flooded.differs_from(&dry);
        assert!(worst <= 1, "{what}: nothing but darkening, {worst} code values");
    }
}

/// The sky and the key both lit, a porous floor in the red room: every wet term has something to change.
fn lit_red_room(specular: EnvironmentSpecular) -> Harness {
    let light = Light { lit_sky: true, key: 1.0, key_direction: Vec3::NEG_Y };
    let mut harness = Harness::new(settings(specular), &surface(0.5, 0.9, 0.0, None), 3.0, light);
    harness.show(&red_room());
    harness
}

const SHOWER: Weather = Weather { wetness: 0.8, puddles: 0.6 };

#[test]
#[ignore = "needs a GPU"]
fn going_wet_builds_nothing_where_it_was_declared() {
    let mut harness = lit_red_room(EnvironmentSpecular::Full);
    let dry = harness.settle(Weather::DRY, 3);
    let built =
        harness.renderer.prewarm(&PipelineInventory { wet_surfaces: true, ..PipelineInventory::default() });
    assert!(built > 0, "the declaration builds the wet surfaces beside the dry");
    let (pipelines, stats) = (harness.pipelines(), harness.renderer.stats().pipelines);
    let wet = harness.settle(SHOWER, 2);
    assert!(wet.differs_from(&dry) > 10, "wet draws otherwise");
    assert_eq!(harness.pipelines(), pipelines, "no pipeline built for the wet frames");
    assert_eq!(wet.stats.pipelines, stats, "none at first use, and no frame waited");
    let again = harness.settle(Weather::DRY, 2);
    assert_eq!(again.differs_from(&dry), 0, "dry again draws to the byte as dry did");
    assert_eq!(harness.pipelines(), pipelines, "and builds nothing either");
}

#[test]
#[ignore = "needs a GPU"]
fn undeclared_going_wet_builds_the_wet_surfaces_as_first_uses() {
    let mut harness = lit_red_room(EnvironmentSpecular::Full);
    let dry = harness.settle(Weather::DRY, 3);
    let (pipelines, builds) = (harness.pipelines(), dry.stats.pipelines.first_use_builds);
    let wet = harness.settle(SHOWER, 2);
    let first_uses = wet.stats.pipelines.first_use_builds - builds;
    assert!(first_uses > 0, "an undeclared wet frame builds the wet surfaces, counted as first uses");
    assert_eq!(harness.pipelines()[0], pipelines[0] + first_uses as usize, "and only those");
    let again = harness.settle(Weather::DRY, 2);
    assert_eq!(again.differs_from(&dry), 0, "dry again draws to the byte as dry did");
    let wet_again = harness.settle(SHOWER, 2);
    assert_eq!(wet_again.differs_from(&wet), 0, "wet again as wet did");
    assert_eq!(wet_again.stats.pipelines.first_use_builds, builds + first_uses, "building nothing more");
}

#[test]
#[ignore = "needs a GPU"]
fn a_tier_change_in_the_wet_keeps_the_wet_surfaces() {
    for (declared, to) in [(true, EnvironmentSpecular::Approximate), (false, EnvironmentSpecular::Off)] {
        let mut harness = lit_red_room(EnvironmentSpecular::Full);
        if declared {
            harness
                .renderer
                .prewarm(&PipelineInventory { wet_surfaces: true, ..PipelineInventory::default() });
        }
        harness.settle(SHOWER, 3);
        harness.change_tier(settings(to));
        let builds = harness.renderer.stats().pipelines.first_use_builds;
        let wet = harness.settle(SHOWER, 2);
        assert_eq!(
            wet.stats.pipelines.first_use_builds, builds,
            "{to:?}: the new tier's wet frames build nothing"
        );
        if declared {
            let dry = harness.settle(Weather::DRY, 2);
            assert_eq!(dry.stats.pipelines.first_use_builds, builds, "{to:?}: nor its dry ones, declared");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn wetness_just_above_zero_draws_as_dry() {
    // The swap to the wet surfaces mustn't pop: the least weather draws as none, within a code value or two.
    for specular in [EnvironmentSpecular::Full, EnvironmentSpecular::Approximate, EnvironmentSpecular::Off] {
        let mut harness = lit_red_room(specular);
        let dry = harness.settle(Weather::DRY, 3);
        let least = harness.settle(Weather { wetness: 1e-4, puddles: 1e-4 }, 2);
        let worst = least.differs_from(&dry);
        assert!(worst <= 2, "{specular:?}: {worst} code values at the threshold");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_puddle_mask_holds_still_as_the_camera_moves() {
    // Looking straight down on a grey porous floor in half-filled puddles, lit by a key 60 degrees from
    // overhead alone (so no highlight lies in view): a puddle soaks the floor dark, and the frame is the
    // mask. Moving the camera along the floor by a
    // whole number of pixels moves the frame by exactly that many: the mask is the world's, not the view's.
    let height = 6.0;
    let pixel = 2.0 * height * (FOV * 0.5).tan() / SIDE as f32;
    // Even, so the 2 × 2 quads the edge's derivative is taken over cover the same ground.
    let shift = 8;
    let key = Light { lit_sky: false, key: 1.0, key_direction: Vec3::new(-0.866, -0.5, 0.0) };
    let mut harness =
        Harness::new(settings(EnvironmentSpecular::Full), &surface(0.5, 0.9, 0.0, Some(1.0)), 40.0, key);
    let look = |x: f32| {
        let eye = Vec3::new(x + 3.0, height, 1.0);
        CameraData::look_at(eye, eye - Vec3::Y, Vec3::NEG_Z, FOV, 1.0, 0.05)
    };
    harness.camera = look(0.0);
    let puddles = Weather { wetness: 0.0, puddles: 0.5 };
    let first = harness.settle(puddles, 2);
    harness.camera = look(shift as f32 * pixel);
    let moved = harness.settle(puddles, 2);

    let (mut dark, mut light, mut worst, mut changed) = (0, 0, 0u8, 0);
    for y in 0..SIDE {
        for x in 0..SIDE - shift {
            let (a, b) = (first.pixel(x + shift, y), moved.pixel(x, y));
            let difference = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            worst = worst.max(difference);
            changed += usize::from(difference > 1);
            // Dry, 0.25 linear (code 137); in a puddle, 0.124 (code 98).
            if a[0] < 110 {
                dark += 1;
            } else if a[0] > 128 {
                light += 1;
            }
        }
    }
    let total = (SIDE * (SIDE - shift)) as usize;
    assert!(dark > total / 8 && light > total / 8, "puddles and dry ground both: {dark} dark, {light} light");
    // A pixel's position differs by a rounding between the two views, which can tip an edge pixel's
    // coverage by a few code values; a mask that moved with the view would move whole edges.
    assert!(
        changed <= total / 200 && worst <= 12,
        "the mask moved: {changed} pixels, up to {worst} code values"
    );
}
