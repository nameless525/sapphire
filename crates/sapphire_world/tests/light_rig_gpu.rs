//! The character light rig end to end (M1 C2): presets resolved under time of day and the
//! camera, through the frame packet, drawn by the renderer. They need a GPU: run them with
//! `cargo test -p sapphire_world --test light_rig_gpu -- --ignored`.
//!
//! Each scene is unit spheres seen from +Z, lit by their rigs' key lights alone (no ambient, no
//! rim, no outline), so every pixel away from a terminator is the albedo, or its authored
//! shade, times the key light, as resolved on the CPU.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_math::color::luminance;
use sapphire_math::{Mat4, Vec3};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance, CharacterLight,
    CharacterLighting, ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MeshHandle, MeshInstance,
    PostParams, RenderSettings, Renderer, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use sapphire_time::ScaledClock;
use sapphire_world::light_rig::{LightRig, LightRigId, LightRigLibrary};
use sapphire_world::tod::{TimeOfDay, TodCutscene, TodFrame, TodLook, default_day};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 192;
const ALBEDO: f32 = 0.8;
const SHADE: [f32; 3] = [0.55, 0.5, 0.65];
/// N·L at least this far from the split, so the pixel is wholly lit or wholly in shade.
const CLEAR_OF_THE_SPLIT: f32 = 0.15;
const TOLERANCE: f32 = 0.025;

/// Shows linear values below about 0.98 as they are (as the renderer's own tests do).
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

/// A unit sphere at the origin: a two-tone split at N·L 0, no rim, no outline.
fn sphere_mesh() -> Mesh {
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
    let material = AnimeCharacterMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: SHADE, ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// Spheres seen from +Z, each a character lit by a rig.
struct Stage {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    mesh: MeshHandle,
    /// The spheres' centers, by instance ID; added with the first frame.
    spheres: Vec<Vec3>,
    added: bool,
}

/// One rendered frame, and the lighting each sphere was drawn with.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    lighting: Vec<CharacterLighting>,
}

impl Stage {
    /// A camera on +Z framing `spheres` (their centers) along the X axis.
    fn new(spheres: &[Vec3]) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let bytes = sphere_mesh().to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let half_width = spheres.iter().map(|center| center.x.abs()).fold(0.0, f32::max) + 1.2;
        let fov = 0.6f32;
        let distance = half_width / (fov * 0.5).tan();
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, Vec3::Y, fov, 1.0, 0.05);
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera,
            mesh,
            spheres: spheres.to_vec(),
            added: false,
        }
    }

    /// Draws a frame of time of day's `frame`, lighting sphere `i` with `rigs[i]` (its head at
    /// its center). Post shows linear light as it is, and there's no ambient light.
    fn render(&mut self, frame: &TodFrame, rigs: &[LightRig]) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        frame.write(packet);
        packet.post = passthrough();
        packet.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
        let mut lighting = Vec::new();
        for (index, (center, rig)) in self.spheres.iter().zip(rigs).enumerate() {
            let id = InstanceId(index as u64);
            if !self.added {
                let transform = Mat4::from_translation(*center);
                packet.instances.changed.push(MeshInstance { id, mesh: self.mesh, transform });
            }
            let resolved = frame.character_lighting(rig, &self.camera, *center);
            packet.characters.push(CharacterInstance {
                id,
                lighting: resolved,
                face: CharacterFace::default(),
                body: CharacterBody::default(),
                opacity: 1.0,
            });
            lighting.push(resolved);
        }
        self.added = true;
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back(), camera: self.camera, lighting }
    }

    fn read_back(&self) -> Vec<[u8; 4]> {
        let padded = (SIZE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIZE),
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
                    rows_per_image: Some(SIZE),
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
            .flat_map(|row| row[..SIZE as usize * 4].as_chunks::<4>().0.iter().copied())
            .collect()
    }
}

/// Points on the camera's side of a unit sphere, spread over it: `(x, y)` in its silhouette.
const PROBES: [(f32, f32); 9] = [
    (0.0, 0.0),
    (0.6, 0.0),
    (-0.6, 0.0),
    (0.0, 0.6),
    (0.0, -0.6),
    (0.5, 0.5),
    (-0.5, 0.5),
    (0.5, -0.5),
    (-0.5, -0.5),
];

impl Frame {
    /// The linear color where `point` projects.
    fn at(&self, point: Vec3) -> [f32; 3] {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let (px, py) = (((x + 1.0) * 0.5 * SIZE as f32) as usize, ((1.0 - y) * 0.5 * SIZE as f32) as usize);
        let pixel = self.pixels[py * SIZE as usize + px];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// Checks sphere `index` (at `center`) shows its resolved key light at every probe clear of
    /// the split: the albedo times the key where N·L is positive, the shade times it where
    /// negative. Returns which probes were lit.
    fn check(&self, index: usize, center: Vec3, what: &str) -> Vec<bool> {
        let key = self.lighting[index].key;
        let toward_key = (-key.direction).normalize();
        let radiance = key.color * key.intensity;
        let mut lit = Vec::new();
        for (x, y) in PROBES {
            let normal = Vec3::new(x, y, (1.0 - x * x - y * y).sqrt());
            let n_dot_l = normal.dot(toward_key);
            if n_dot_l.abs() < CLEAR_OF_THE_SPLIT {
                continue;
            }
            let tone = if n_dot_l > 0.0 { Vec3::ONE } else { Vec3::from(SHADE) };
            let expected = (tone * radiance * ALBEDO).to_array();
            let actual = self.at(center + normal);
            let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < TOLERANCE);
            assert!(close, "{what}: at ({x}, {y}) {actual:?}, expected {expected:?} (N·L {n_dot_l:.2})");
            lit.push(n_dot_l > 0.0);
        }
        lit
    }
}

/// A look with the sun alone, from `toward_sun` (a vector toward it), and a white character key.
fn sun_look(toward_sun: Vec3) -> TodFrame {
    let sun = DirectionalLight { direction: -toward_sun, color: Vec3::ONE, intensity: 1.0 };
    let moon = DirectionalLight { intensity: 0.0, ..TodLook::default().moon };
    let character_light = CharacterLight { color: Vec3::ONE, intensity: 0.9 };
    TodFrame {
        hours: 12.0,
        look: TodLook { sun, moon, character_light, ..TodLook::default() },
        shot_rig: None,
    }
}

fn rigs() -> LightRigLibrary {
    LightRigLibrary::default_rigs()
}

#[test]
#[ignore = "needs a GPU"]
fn a_backlit_character_keeps_a_lit_side_toward_the_camera() {
    let mut stage = Stage::new(&[Vec3::ZERO]);
    // The sun behind the character and to the right, as the camera sees it (149° round).
    let frame = sun_look(Vec3::new(0.6, 0.3, -1.0));
    let neutral = stage.render(&frame, &[LightRig::default()]);
    let lit = neutral.check(0, Vec3::ZERO, "the sun itself");
    assert!(lit.iter().all(|&lit| !lit), "following the sun, the camera's side is all in shade");
    let library = rigs();
    let gameplay = stage.render(&frame, &[*library.rig(LightRigId::DEFAULT)]);
    let lit = gameplay.check(0, Vec3::ZERO, "gameplay");
    assert!(
        lit.iter().any(|&lit| lit) && lit.iter().any(|&lit| !lit),
        "a lit side and a shaded one: {lit:?}"
    );
    // Folded back from 149° to 62° round from the camera's side, on the sun's side: the right
    // lit, the left not.
    assert!(gameplay.at(Vec3::new(0.8, 0.0, 0.6))[0] > gameplay.at(Vec3::new(-0.8, 0.0, 0.6))[0] + 0.2);
}

#[test]
#[ignore = "needs a GPU"]
fn each_character_is_lit_by_its_own_preset() {
    let centers = [Vec3::new(-1.3, 0.0, 0.0), Vec3::new(1.3, 0.0, 0.0)];
    let mut stage = Stage::new(&centers);
    let library = rigs();
    let presets =
        [*library.rig(LightRigId::DEFAULT), *library.rig(library.find("portrait").expect("shipped"))];
    // The sun from the right, low: the gameplay sphere lit on its right, the portrait one from
    // the camera's upper left whatever the sun does.
    for toward_sun in [Vec3::new(1.0, 0.3, 0.2), Vec3::new(-0.3, 0.9, -0.2)] {
        let frame = stage.render(&sun_look(toward_sun), &presets);
        frame.check(0, centers[0], "gameplay");
        frame.check(1, centers[1], "portrait");
        let portrait = frame.lighting[1].key.direction;
        let upper_left = Vec3::new(-0.6, 0.6, 0.5);
        assert!((-portrait).normalize().dot(upper_left.normalize()) > 0.8, "{portrait}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_shot_rig_turns_the_key_as_its_cutscene_blends_in() {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let mut tod = TimeOfDay::new(default_day(), clock, 14.0).expect("complete");
    let mut stage = Stage::new(&[Vec3::ZERO]);
    let library = rigs();
    let gameplay = *library.rig(LightRigId::DEFAULT);
    let portrait = *library.rig(library.find("portrait").expect("shipped"));
    let shot = TodCutscene::new(4.0, 2.0, 0.0, None, Vec::new(), Some(portrait)).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    let mut directions = Vec::new();
    for seconds in [0.0, 0.7, 1.0, 1.3, 3.0] {
        tod.seek_cutscene(seconds);
        let frame = *tod.resolve(Vec3::ZERO, 0.0);
        let drawn = stage.render(&frame, &[gameplay]);
        drawn.check(0, Vec3::ZERO, &format!("{seconds} s into the shot"));
        directions.push((-drawn.lighting[0].key.direction).normalize());
    }
    // From the afternoon sun's side to the portrait key's, along one great circle.
    let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos();
    let total = angle(directions[0], directions[4]);
    assert!(total > 0.5, "the two keys differ: {total}");
    for pair in directions.windows(2) {
        assert!(angle(directions[0], pair[1]) >= angle(directions[0], pair[0]), "it only turns one way");
    }
    let halfway = angle(directions[0], directions[2]);
    assert!(
        (halfway - total * 0.5).abs() < 0.02,
        "half the weight halfway through the blend: {halfway} of {total}"
    );
}

#[test]
#[ignore = "needs a GPU"]
fn the_key_takes_time_of_days_character_light_not_the_worlds() {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let mut tod = TimeOfDay::new(default_day(), clock, 0.0).expect("complete");
    let mut stage = Stage::new(&[Vec3::ZERO]);
    let gameplay = *rigs().rig(LightRigId::DEFAULT);
    for hours in [0.0, 6.5, 10.0, 17.5] {
        tod.clock_mut().set_hours(hours);
        let frame = *tod.resolve(Vec3::ZERO, 0.0);
        let drawn = stage.render(&frame, &[gameplay]);
        drawn.check(0, Vec3::ZERO, &format!("{hours}:00"));
        let curve = frame.look.character_key();
        assert_eq!(
            drawn.lighting[0].key.color * drawn.lighting[0].key.intensity,
            curve.color * curve.intensity
        );
    }
    // At midnight the character's key is a little brighter than the moon that lights the world
    // (in luminance, about 1.25 times: the default day's night lift).
    tod.clock_mut().set_hours(0.0);
    let frame = *tod.resolve(Vec3::ZERO, 0.0);
    let drawn = stage.render(&frame, &[gameplay]);
    let moon = frame.look.key_light();
    let lit = luminance(Vec3::from(drawn.at(Vec3::new(0.0, 0.5, 0.866))));
    let moonlit = ALBEDO * luminance(moon.color) * moon.intensity;
    assert!(lit > moonlit * 1.15, "{lit} vs {moonlit} lit by the moon {moon:?}");
}
