//! Reflection probes' tests that need a GPU (M5 L3): a closed red room's glossy floor reflects red, an
//! open one the live sky through the bake's sky share, a frame's keys blend as it names them, the faces
//! read (the iOS Simulator's, without cube arrays) agrees with the cube array, nothing is built, made or
//! dispatched for a zone without probes, composing counts its dispatches in the ambient pass and adds no
//! draw to any pass's counts, a tier change composes them again at the new tier's size, and
//! hiding them leaves the sky's reflection; a character's reflective part reads the probe the picker names
//! for where it stands and the sky's reflection, to the byte, beyond every probe's reach; a floor inside one
//! room by a wall it shares with another reflects its own room's probe whole; and Mobile-Low
//! holds its keys at the size it composes at, averaged as the composition would, which a set handed to a
//! larger tier keeps. Run with
//! `cargo test -p sapphire_render --test gpu reflections:: -- --ignored --test-threads=1`.
//!
//! Scenes have no key light: the floor is a white metal mirror (roughness 0, F0 1), so a pixel is the light
//! reflected toward it, and post shows linear values as they are.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, DistanceStylization, EnvironmentMaterial, LightSplit, Material,
    MaterialDocument, Outline, Rim, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::probes::ProbeGrid;
use sapphire_assets::reflections::{ReflectionProbe, ReflectionProbes};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, EnvironmentSettings, EnvironmentSpecular,
    FramePacketRing, HeadBasis, InstanceId, MaterialDrive, MaterialHandle, MeshHandle, MeshInstance, Msaa,
    PostParams, ProbeKeys, ProbeLookup, ReflectionProbesHandle, ReflectionSettings, RenderSettings,
    RenderStats, Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIDE: u32 = 96;
/// 8-bit output and half floats.
const TOLERANCE: f32 = 0.02;
/// The probes' cook size: the desktop tiers compose at it.
const SIZE: u32 = 32;
/// Where the floor's checked: on the probe's box's lowest face, away from its sides, so its whole weight.
const POINTS: [Vec3; 3] = [Vec3::ZERO, Vec3::new(0.6, 0.0, -0.6), Vec3::new(-0.8, 0.0, 0.4)];

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

/// No MSAA or temporal AA, environment specular as `specular`, composed probes `cube_size` a side.
fn settings(specular: EnvironmentSpecular, cube_size: u32) -> RenderSettings {
    RenderSettings {
        msaa: Msaa::Off,
        environment: EnvironmentSettings { specular, probes: ProbeLookup::PerPixel },
        reflections: ReflectionSettings { cube_size },
        ..RenderSettings::default()
    }
}

fn desktop() -> RenderSettings {
    settings(EnvironmentSpecular::Full, 128)
}

/// A white metal mirror.
fn mirror(roughness: f32) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [1.0; 4],
        roughness,
        metallic: 1.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

/// A floor 6 m square at height 0.
fn floor() -> Mesh {
    let vertex = |position: Vec3| Vertex {
        position: position.to_array(),
        normal: Vec3::Y.to_array(),
        outline_normal: Vec3::Y.to_array(),
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let corners = [
        Vec3::new(-3.0, 0.0, -3.0),
        Vec3::new(-3.0, 0.0, 3.0),
        Vec3::new(3.0, 0.0, 3.0),
        Vec3::new(3.0, 0.0, -3.0),
    ];
    let vertices: Vec<Vertex> = corners.into_iter().map(vertex).collect();
    let indices = vec![0, 1, 2, 0, 2, 3];
    let document = EnvironmentMaterial::default();
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let placeholder = Material::from_document(&document, &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("floor", placeholder)],
        PositionFormat::Float32,
    )
}

/// The direction texel `(x, y)` of cube face `face` faces, `size` texels a side (`probes.wgsl`'s
/// `cube_texel_direction`).
fn texel_direction(face: usize, x: u32, y: u32, size: u32) -> Vec3 {
    let (s, t) = ((x as f32 + 0.5) / size as f32 * 2.0 - 1.0, (y as f32 + 0.5) / size as f32 * 2.0 - 1.0);
    let (right, up, forward) = match face {
        0 => (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        1 => (Vec3::Z, Vec3::Y, Vec3::NEG_X),
        2 => (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        3 => (Vec3::X, Vec3::Z, Vec3::NEG_Y),
        4 => (Vec3::X, Vec3::Y, Vec3::Z),
        _ => (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
    };
    (forward + s * right - t * up).normalize()
}

/// One probe at the room's middle, the floor on its box's lowest face as a room's floor is (so it takes the
/// probe whole, not the fade past the box), baked `size` texels a side at `hours`:
/// per key, `texel(key, face, direction)` is a texel's radiance and sky share.
fn baked(size: u32, hours: &[f32], texel: impl Fn(usize, usize, Vec3) -> [f32; 4]) -> ReflectionProbes {
    let mut texels = Vec::new();
    for key in 0..hours.len() {
        for face in 0..6 {
            for y in 0..size {
                for x in 0..size {
                    let value = texel(key, face, texel_direction(face, x, y, size));
                    texels.extend(value.map(|value| f16::from_f32(value).to_bits()));
                }
            }
        }
    }
    ReflectionProbes {
        size,
        key_hours: hours.to_vec(),
        probes: vec![ReflectionProbe {
            name: "room".to_owned(),
            position: [0.0, 1.5, 0.0],
            parallax_min: [-4.0, 0.0, -4.0],
            parallax_max: [4.0, 4.0, 4.0],
        }],
        texels,
    }
}

/// As [`baked`] at [`SIZE`], each face one texel: `face(key, face)`.
fn room(hours: &[f32], face: impl Fn(usize, usize) -> [f32; 4]) -> ReflectionProbes {
    baked(SIZE, hours, |key, side, _| face(key, side))
}

/// Every face `color`, closed to the sky.
fn closed(color: Vec3) -> ReflectionProbes {
    room(&[12.0], |_, _| [color.x, color.y, color.z, 0.0])
}

/// Probes at [`SIZE`] of one key, each `(position, box min, box max, color)` with every face its color.
fn rooms(probes: &[(Vec3, Vec3, Vec3, Vec3)]) -> ReflectionProbes {
    let mut texels = Vec::new();
    for &(_, _, _, color) in probes {
        let texel = [color.x, color.y, color.z, 0.0].map(|value| f16::from_f32(value).to_bits());
        texels.extend(std::iter::repeat_n(texel, 6 * (SIZE * SIZE) as usize).flatten());
    }
    ReflectionProbes {
        size: SIZE,
        key_hours: vec![12.0],
        probes: probes
            .iter()
            .enumerate()
            .map(|(index, &(position, min, max, _))| ReflectionProbe {
                name: format!("room {index}"),
                position: position.to_array(),
                parallax_min: min.to_array(),
                parallax_max: max.to_array(),
            })
            .collect(),
        texels,
    }
}

/// An anime character material, white and flat, whose one region reflects by `metallic` as a mirror; no
/// rim, outline or shade color, so the reflection add is what tells it from its matte self.
fn chrome(metallic: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [1.0; 4],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion {
            shade_color: [0.0; 3],
            metallic,
            roughness: 0.0,
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A ball of radius 0.5 at the origin, in one anime character material slot.
fn ball() -> Mesh {
    let (rings, segments) = (32u32, 64u32);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let n = Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos());
            vertices.push(Vertex {
                position: (n * 0.5).to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [segment as f32 / segments as f32, ring as f32 / rings as f32],
                color: [255; 3],
                flags: VertexFlags::with_material_id(MaterialId::new(0).expect("a material ID")),
            });
        }
    }
    let stride = segments + 1;
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, b, c, c, b, d]);
        }
    }
    let document = AnimeCharacterMaterial::default();
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let placeholder = Material::from_document(&document, &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("ball", placeholder)],
        PositionFormat::Float32,
    )
}

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

    /// The 8-bit code values where each of `points` projects.
    fn codes_at(&self, points: &[Vec3]) -> Vec<[u8; 4]> {
        let code = |point: &Vec3| {
            let ndc = (self.camera.proj * self.camera.view).project_point3(*point);
            let (x, y) = ((ndc.x + 1.0) * 0.5 * SIDE as f32, (1.0 - ndc.y) * 0.5 * SIDE as f32);
            self.pixels[(y as u32 * SIDE + x as u32) as usize]
        };
        points.iter().map(code).collect()
    }
}

fn assert_near(actual: Vec3, expected: Vec3, what: &str) {
    assert!((actual - expected).abs().max_element() <= TOLERANCE, "{what}: {actual} against {expected}");
}

struct Harness {
    gpu: Arc<GpuContext>,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    keys: ProbeKeys,
    added: Vec<MeshInstance>,
    characters: Vec<CharacterInstance>,
    balls: Vec<MeshHandle>,
}

impl Harness {
    /// A renderer of `settings` with the mirror floor of `roughness`, reading cubes as a cube array unless
    /// `without_cube_arrays`.
    fn new(settings: RenderSettings, roughness: f32, without_cube_arrays: bool) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let desc = GpuContextDesc { without_cube_arrays, ..GpuContextDesc::default() };
        let gpu = Arc::new(GpuContext::new(&instance, &desc, None).expect("device"));
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
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
        let mut harness = Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: CameraData::look_at(Vec3::new(0.0, 2.0, 2.5), Vec3::ZERO, Vec3::Y, 0.9, 1.0, 0.05),
            keys: ProbeKeys::single(0),
            added: Vec::new(),
            characters: Vec::new(),
            balls: Vec::new(),
        };
        let material = harness.material(&mirror(roughness));
        let bytes = floor().to_cooked().expect("cook");
        let mesh =
            harness.renderer.upload_mesh(Mesh::read(&bytes).expect("valid"), &[material]).expect("upload");
        harness.added.push(MeshInstance { id: InstanceId(0), mesh, transform: Mat4::IDENTITY });
        harness
    }

    fn material<D: MaterialDocument>(&mut self, document: &D) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    fn update_material<D: MaterialDocument>(&mut self, handle: MaterialHandle, document: &D) {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer
            .update_material(handle, Material::read(&bytes).expect("read"), |_| None)
            .expect("update");
    }

    /// Lists a ball in `material` as a character centred at `centre`, its body a capsule through it, lit by
    /// nothing but the ambient light and what it reflects. Characters are instances 1 on, the floor being 0.
    fn character(&mut self, material: MaterialHandle, centre: Vec3) -> usize {
        let bytes = ball().to_cooked().expect("cook");
        let mesh =
            self.renderer.upload_mesh(Mesh::read(&bytes).expect("valid"), &[material]).expect("upload");
        let id = InstanceId(self.characters.len() as u64 + 1);
        self.added.push(MeshInstance { id, mesh, transform: Mat4::from_translation(centre) });
        let key = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 0.0 };
        self.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(key),
            face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
            body: CharacterBody::capsule(centre - 0.5 * Vec3::Y, centre + 0.5 * Vec3::Y, 0.5),
            opacity: 1.0,
            drive: MaterialDrive::NONE,
        });
        self.balls.push(mesh);
        self.characters.len() - 1
    }

    /// Moves character `index` (from [`Self::character`]) to be centred at `centre`.
    fn move_character(&mut self, index: usize, centre: Vec3) {
        let character = &mut self.characters[index];
        character.body = CharacterBody::capsule(centre - 0.5 * Vec3::Y, centre + 0.5 * Vec3::Y, 0.5);
        let (id, mesh) = (character.id, self.balls[index]);
        self.added.push(MeshInstance { id, mesh, transform: Mat4::from_translation(centre) });
    }

    /// Points on the side of a ball centred at `centre` that faces the camera, inside its silhouette.
    fn facing(&self, centre: Vec3) -> [Vec3; 5] {
        let toward = (self.camera.position - centre).normalize();
        let right = toward.cross(Vec3::Y).normalize();
        let up = right.cross(toward);
        [Vec3::ZERO, 0.2 * right, -0.2 * right, 0.2 * up, -0.2 * up]
            .map(|offset| centre + 0.45 * (toward + offset).normalize())
    }

    /// Changes to a tier of `settings` as the stage does at a pause (M4 T3).
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

    /// Uploads and shows `probes`.
    fn show(&mut self, probes: &ReflectionProbes) -> ReflectionProbesHandle {
        let bytes = probes.to_cooked().expect("cook");
        let handle = self
            .renderer
            .upload_reflection_probes(ReflectionProbes::read(&bytes).expect("valid"))
            .expect("fits");
        self.renderer.set_reflection_probes(Some(handle)).expect("shown");
        handle
    }

    fn set_grid(&mut self, grid: &ProbeGrid) {
        let bytes = grid.to_cooked().expect("cook");
        self.renderer.set_probe_grid(Some(ProbeGrid::read(&bytes).expect("valid"))).expect("fits");
    }

    fn draw(&mut self) {
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 0.0 };
        packet.sky = SkyGradient {
            zenith: Vec3::new(0.05, 0.15, 0.6),
            horizon: Vec3::new(0.2, 0.3, 0.5),
            ground: Vec3::new(0.05, 0.05, 0.05),
        };
        packet.ambient =
            AmbientLight { sky: Vec3::new(0.1, 0.2, 0.5), ground: Vec3::splat(0.05), intensity: 1.0 };
        packet.probe_keys = ProbeKeys::single(0);
        packet.reflection_keys = self.keys;
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        packet.characters.extend_from_slice(&self.characters);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }

    /// Renders `frames` frames and waits for them, reading nothing back.
    fn frames(&mut self, frames: u32) {
        for _ in 0..frames {
            self.draw();
        }
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("frames complete");
    }

    /// Renders `frames` frames and reads the last back.
    fn settle(&mut self, frames: u32) -> Frame {
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

    /// What the backend holds: render and compute pipelines, buffers and textures.
    fn resources(&self) -> [usize; 4] {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        let hub = report.hub;
        [
            hub.render_pipelines.num_kept_from_user,
            hub.compute_pipelines.num_kept_from_user,
            hub.buffers.num_kept_from_user,
            hub.textures.num_kept_from_user,
        ]
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_closed_red_room_reflects_red() {
    let red = Vec3::new(0.5, 0.02, 0.02);
    let mut harness = Harness::new(desktop(), 0.0, false);
    let sky = harness.settle(2);
    harness.show(&closed(red));
    let frame = harness.settle(2);
    for point in POINTS {
        assert_near(frame.at(point), red, "the floor in a red room");
        assert!(sky.at(point).z > sky.at(point).x, "the sky's reflection first: {}", sky.at(point));
    }
    let ambient = frame.stats.ambient;
    assert_eq!((ambient.reflection_probes, ambient.composed_size, ambient.composed_mips), (1, 32, 3));
    assert_eq!(
        ambient.composed_bytes,
        6 * 8 * (32 * 32 + 16 * 16 + 8 * 8),
        "Rgba16Float, six faces, three levels"
    );
}

/// Beside a wall it shares with a blue room whose probe stands by the wall, a red room's floor
/// reflects red to the wall (review F3): the blue probe reaches half a metre into the red room and its
/// position is nearer the cell's centre there, but a point inside the red box reads the probe of its
/// cell's two whose weight at it is larger, the red one whole.
#[test]
#[ignore = "needs a GPU"]
fn a_floor_inside_a_red_room_by_a_blue_rooms_wall_reflects_red() {
    let (red, blue) = (Vec3::new(0.5, 0.02, 0.02), Vec3::new(0.02, 0.02, 0.5));
    let mut harness = Harness::new(desktop(), 0.0, false);
    harness.show(&rooms(&[
        (Vec3::new(-2.5, 1.5, 0.0), Vec3::new(-3.0, 0.0, -3.0), Vec3::new(1.0, 3.0, 3.0), red),
        (Vec3::new(1.3, 1.5, 0.0), Vec3::new(1.0, 0.0, -3.0), Vec3::new(4.0, 3.0, 3.0), blue),
    ]));
    let frame = harness.settle(2);
    for point in [Vec3::new(0.6, 0.0, 0.0), Vec3::new(0.75, 0.0, -0.5), Vec3::new(0.9, 0.0, 0.3)] {
        assert_near(frame.at(point), red, &format!("the red room's floor at {point}"));
    }
    assert_near(frame.at(Vec3::new(1.2, 0.0, 0.0)), blue, "the blue room's floor past the wall");
}

#[test]
#[ignore = "needs a GPU"]
fn an_open_room_reflects_the_live_sky_through_the_sky_share() {
    // A grid that sees the sky every way reflects the reflection cube: the sky as the frame draws it. A
    // probe that's all sky share must reflect the same.
    // Without the box's correction (Mobile-High's read), which would put the sky at the box's walls, and
    // composed at the reflection cube's size.
    let grid = super::ambient::grid(&[12.0], 1.0, |_, _| Vec3::ZERO, super::ambient::open);
    let plain = settings(EnvironmentSpecular::Approximate, 128);
    let mut sky = Harness::new(plain, 0.0, false);
    sky.set_grid(&grid);
    let expected = sky.settle(3);
    let mut open = Harness::new(plain, 0.0, false);
    open.set_grid(&grid);
    open.show(&baked(128, &[12.0], |_, _, _| [0.0, 0.0, 0.0, 1.0]));
    let frame = open.settle(3);
    for point in POINTS {
        assert_near(frame.at(point), expected.at(point), "the floor of an open room");
        assert!(frame.at(point).z > 2.0 * frame.at(point).x, "blue sky: {}", frame.at(point));
    }
}

#[test]
#[ignore = "needs a GPU"]
fn keys_blend_as_the_frame_names_them() {
    let (red, green) = (Vec3::new(0.6, 0.0, 0.0), Vec3::new(0.0, 0.6, 0.0));
    let probes =
        room(&[6.0, 18.0], |key, _| if key == 0 { [0.6, 0.0, 0.0, 0.0] } else { [0.0, 0.6, 0.0, 0.0] });
    let mut harness = Harness::new(desktop(), 0.0, false);
    harness.show(&probes);
    assert_near(harness.settle(2).at(Vec3::ZERO), red, "the first key");
    harness.keys = ProbeKeys { keys: [0, 1, 0, 0], weights: [0.25, 0.75, 0.0, 0.0] };
    // A jump in the keys composes the probe again in the next frame.
    let blended = harness.settle(2);
    assert_near(
        blended.at(Vec3::ZERO),
        red * 0.25 + green * 0.75,
        "a quarter of the first and the rest of the second",
    );
    let compositions = blended.stats.ambient.compositions;
    let still = harness.settle(3);
    assert_eq!(still.stats.ambient.compositions, compositions, "nothing composed while the keys hold");
}

#[test]
#[ignore = "needs a GPU"]
fn the_faces_read_agrees_with_the_cube_array() {
    // A smooth environment, as a bake is (bright above, dark below, warmer toward +x), on a floor rough enough
    // to read the composed cube's lower levels and filter across its edges.
    let probes = baked(SIZE, &[12.0], |_, _, direction| {
        let up = direction.y * 0.5 + 0.5;
        [0.1 + 0.4 * up + 0.1 * direction.x, 0.08 + 0.45 * up, 0.06 + 0.6 * up - 0.05 * direction.z, 0.0]
    });
    let mut frames = [false, true].map(|without_cube_arrays| {
        let mut harness = Harness::new(desktop(), 0.3, without_cube_arrays);
        harness.show(&probes);
        harness.settle(2)
    });
    let [cube_array, faces] = &mut frames;
    let worst = cube_array.differs_from(faces);
    // L0's spike measured its fallback within 4 code values of the cube array, along the cube's edges;
    // on Metal, M5, this reads 1.
    assert!(worst <= 4, "{worst} code values apart");
}

#[test]
#[ignore = "needs a GPU"]
fn nothing_is_built_made_or_dispatched_without_probes() {
    // Frames are drawn and finished without reading back, whose buffers would count.
    let mut harness = Harness::new(desktop(), 0.0, false);
    harness.frames(3);
    let (before, first) = (harness.resources(), harness.renderer.stats());
    harness.frames(3);
    let later = harness.renderer.stats();
    assert_eq!(harness.resources(), before, "no pipeline, buffer or texture comes in a zone without probes");
    let ambient = later.ambient;
    assert_eq!(
        (ambient.probe_sets, ambient.reflection_probes, ambient.composed_bytes, ambient.compositions),
        (0, 0, 0, 0)
    );
    assert_eq!(later.pipelines.first_use_builds, first.pipelines.first_use_builds);
    assert_eq!(ambient.dispatches, first.ambient.dispatches, "the ambient pass runs as it did");

    // Probes then build the composition's pipeline and the environment's again, and compose once.
    harness.show(&closed(Vec3::X));
    harness.frames(1);
    let [render, compute, ..] = harness.resources();
    assert!(compute > before[1] && render >= before[0], "the composition's pipeline");
    assert_eq!(harness.renderer.stats().ambient.compositions, 1);
    let held = harness.resources();
    harness.frames(3);
    assert_eq!(harness.resources(), held, "nothing more while they show");
    assert_eq!(
        harness.renderer.stats().ambient.compositions,
        1,
        "and the one probe composed once, at the fixed hour"
    );
}

/// The probes' composition and prefilter are compute dispatches of the ambient pass and count there
/// (`AmbientStats::probe_dispatches`); they add no draw to any pass's counts (`RenderStats::passes`, M5 B4).
#[test]
#[ignore = "needs a GPU"]
fn probes_count_their_dispatches_in_the_ambient_pass_and_add_no_draw() {
    let mut harness = Harness::new(desktop(), 0.0, false);
    harness.frames(2);
    let without = harness.renderer.stats();
    assert_eq!((without.ambient.probe_dispatches, without.ambient.composed_probes), (0, 0));

    // Two probes at a fixed hour: the frame after they're shown composes both, each its 32² top level and
    // a prefilter for each of its two rougher levels, 16² and 8².
    let (red, green) = (Vec3::new(0.5, 0.02, 0.02), Vec3::new(0.02, 0.5, 0.02));
    harness.show(&rooms(&[
        (Vec3::new(-2.0, 1.0, -2.0), Vec3::new(-4.0, 0.0, -4.0), Vec3::new(0.0, 4.0, 0.0), red),
        (Vec3::new(2.0, 1.0, -2.0), Vec3::new(0.0, 0.0, -4.0), Vec3::new(4.0, 4.0, 0.0), green),
    ]));
    harness.frames(1);
    let composing = harness.renderer.stats();
    let ambient = composing.ambient;
    assert_eq!((ambient.composed_size, ambient.composed_mips), (32, 3));
    assert_eq!((ambient.composed_probes, ambient.probe_dispatches), (2, 6));
    // Beside them the sky light's projection, which composing reads: its three dispatches, or its last
    // alone when the sky cube drew no face.
    let projection = ambient.dispatches - without.ambient.dispatches - ambient.probe_dispatches;
    assert!(projection == 3 || projection == 1, "{ambient:?} against {:?}", without.ambient);
    assert_eq!(composing.passes, without.passes, "probes add no draw: {:#?}", composing.passes);

    // Composed, the next frame dispatches what it did without them, and draws the same.
    harness.frames(1);
    let after = harness.renderer.stats();
    assert_eq!((after.ambient.composed_probes, after.ambient.probe_dispatches), (0, 0));
    assert_eq!(after.ambient.dispatches, without.ambient.dispatches);
    assert_eq!(after.passes, without.passes);
}

#[test]
#[ignore = "needs a GPU"]
fn hiding_and_freeing_probes_leaves_the_sky() {
    let mut harness = Harness::new(desktop(), 0.0, false);
    let sky = harness.settle(2);
    let handle = harness.show(&closed(Vec3::new(0.5, 0.0, 0.0)));
    harness.settle(2);
    harness.renderer.set_reflection_probes(None).expect("hidden");
    let hidden = harness.settle(2);
    assert_near(hidden.at(Vec3::ZERO), sky.at(Vec3::ZERO), "hidden, the sky again");
    assert_eq!(hidden.stats.ambient.composed_bytes, 0);
    harness.renderer.set_reflection_probes(Some(handle)).expect("shown again");
    assert_near(harness.settle(2).at(Vec3::ZERO), Vec3::new(0.5, 0.0, 0.0), "shown again");
    harness.renderer.free_reflection_probes(handle);
    let freed = harness.settle(2);
    assert_near(freed.at(Vec3::ZERO), sky.at(Vec3::ZERO), "freed, the sky");
    assert_eq!((freed.stats.ambient.probe_sets, freed.stats.ambient.probe_key_bytes), (0, 0));
    assert!(harness.renderer.set_reflection_probes(Some(handle)).is_err(), "a freed set is unknown");
}

#[test]
#[ignore = "needs a GPU"]
fn a_tier_change_composes_them_again_at_its_size() {
    let red = Vec3::new(0.5, 0.0, 0.0);
    let mut harness = Harness::new(desktop(), 0.0, false);
    harness.show(&closed(red));
    assert_eq!(harness.settle(2).stats.ambient.composed_size, 32);
    // Mobile-High's: the approximate specular, without parallax, at 64 (a 32 cook composes at 32).
    harness.change_tier(settings(EnvironmentSpecular::Approximate, 64));
    let frame = harness.settle(2);
    assert_near(frame.at(Vec3::ZERO), red, "still red, by the new tier's binding");
    assert_eq!((frame.stats.ambient.probe_sets, frame.stats.ambient.reflection_probes), (1, 1));
}

/// The linear light a character's reflection adds at `points`: `reflecting` less `matte`, averaged.
fn reflected(reflecting: &Frame, matte: &Frame, points: &[Vec3]) -> Vec3 {
    points.iter().map(|&point| reflecting.at(point) - matte.at(point)).sum::<Vec3>() / points.len() as f32
}

#[test]
#[ignore = "needs a GPU"]
fn a_character_reads_the_probe_it_stands_in_and_the_sky_beyond_every_reach() {
    let red = Vec3::new(0.5, 0.02, 0.02);
    let centre = Vec3::new(0.0, 1.0, 0.0);
    let mut harness = Harness::new(desktop(), 0.0, false);
    let material = harness.material(&chrome(1.0));
    harness.character(material, centre);
    let points = harness.facing(centre);
    let sky = harness.settle(2);
    // A red probe whose box and reach end short of the character (its box at z ≤ -2 reaches to -1.5; the
    // body's centre is at 0): the sky's reflection, to the byte.
    let beyond =
        rooms(&[(Vec3::new(0.0, 1.0, -3.0), Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, 4.0, -2.0), red)]);
    harness.show(&beyond);
    assert_eq!(harness.settle(2).codes_at(&points), sky.codes_at(&points), "beyond the probe's reach");
    // In the red room, the reflection add reads red.
    harness.show(&closed(red));
    let reflecting = harness.settle(2);
    harness.update_material(material, &chrome(0.0));
    let matte = harness.settle(2);
    assert_near(reflected(&reflecting, &matte, &points), red, "the red room on the character");
    let ambient = reflecting.stats.ambient;
    assert_eq!((ambient.probe_key_size, ambient.composed_size), (SIZE, SIZE), "held at the cook's size");
}

#[test]
#[ignore = "needs a GPU"]
fn a_character_between_two_probes_reads_the_one_the_picker_names() {
    // A red probe's box from x = -4 to 0 and a green one's from 0 to 4, both at z ≤ 0; each reaches 0.5 m
    // past its faces. A body centred at x = 0.25 stands in the green box and the red one's reach: the
    // picker names the probe baked nearer, the green one (1.82 m against 2.30), whole.
    let (red, green) = (Vec3::new(0.5, 0.02, 0.02), Vec3::new(0.02, 0.5, 0.02));
    let probes = rooms(&[
        (Vec3::new(-2.0, 1.0, -2.0), Vec3::new(-4.0, 0.0, -4.0), Vec3::new(0.0, 4.0, 0.0), red),
        (Vec3::new(2.0, 1.0, -2.0), Vec3::new(0.0, 0.0, -4.0), Vec3::new(4.0, 4.0, 0.0), green),
    ]);
    let mut harness = Harness::new(desktop(), 0.0, false);
    let material = harness.material(&chrome(1.0));
    let character = harness.character(material, Vec3::new(0.25, 1.0, -1.5));
    harness.show(&probes);
    for (x, expected, what) in [(0.25, green, "the green probe, nearer"), (-0.75, red, "the red probe's box")]
    {
        let centre = Vec3::new(x, 1.0, -1.5);
        harness.move_character(character, centre);
        harness.update_material(material, &chrome(1.0));
        let reflecting = harness.settle(2);
        harness.update_material(material, &chrome(0.0));
        let matte = harness.settle(2);
        assert_near(reflected(&reflecting, &matte, &harness.facing(centre)), expected, what);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn mobile_low_holds_its_keys_at_its_size_and_a_larger_tier_keeps_them() {
    // A smooth environment baked at 64, as the mobile cook bakes, on Mobile-Low (characters alone read
    // probes, composed at 32).
    let probes = baked(64, &[12.0], |_, _, direction| {
        let up = direction.y * 0.5 + 0.5;
        [0.1 + 0.4 * up + 0.1 * direction.x, 0.08 + 0.45 * up, 0.06 + 0.6 * up - 0.05 * direction.z, 0.0]
    });
    let low = settings(EnvironmentSpecular::Off, 32);
    let high = settings(EnvironmentSpecular::Approximate, 64);
    let centre = Vec3::new(0.0, 1.0, 0.0);
    let mut held = Harness::new(low, 0.0, false);
    let material = held.material(&chrome(1.0));
    held.character(material, centre);
    held.show(&probes);
    let small = held.settle(2);
    let ambient = small.stats.ambient;
    assert_eq!((ambient.probe_key_size, ambient.composed_size), (32, 32));
    assert_eq!(ambient.probe_key_bytes, 6 * 32 * 32 * 8, "a quarter of the cook's 6 × 64² × 8");
    // The same set uploaded on Mobile-High holds the cook's 64 and, handed to Mobile-Low, composes from it
    // at 32 by averaging; held at 32 it was averaged on the CPU the same way, so the character reads the same
    // but for a half float's rounding.
    let mut full = Harness::new(high, 0.0, false);
    let material = full.material(&chrome(1.0));
    full.character(material, centre);
    full.show(&probes);
    assert_eq!(full.settle(2).stats.ambient.probe_key_size, 64);
    full.change_tier(low);
    let averaged = full.settle(2);
    assert_eq!(averaged.stats.ambient.composed_size, 32);
    let points = held.facing(centre);
    let worst = small
        .codes_at(&points)
        .iter()
        .zip(averaged.codes_at(&points))
        .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])))
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "{worst} code values apart");
    // Handed to Mobile-High, a set held at 32 composes at 32: the keys aren't there to compose larger.
    held.change_tier(high);
    let after = held.settle(2).stats.ambient;
    assert_eq!((after.probe_key_size, after.composed_size, after.reflection_probes), (32, 32, 1));
}
