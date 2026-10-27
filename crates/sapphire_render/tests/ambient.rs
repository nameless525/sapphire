//! Ambient tests that need a GPU (M1 W5): the probe grid in place of the hemisphere, its sky
//! relit from the frame's every frame, its bounce blended from the frame's keys, the leak test
//! at a thin wall, characters' ambient from it, replacing and removing it, its fade before an
//! empty brick, Mobile-Low's per-vertex read, capsule AO's falloff, and GTAO's creases,
//! stillness and symmetry, and the characters and lines it leaves out; nothing built or run for
//! work nothing reads; and foliage (M1 W6) lit by all of it as the environment is, with GTAO
//! seeing its cards as the prepass cuts them. Run with
//! `cargo test -p sapphire_render --test ambient -- --ignored`.
//!
//! Scenes have no key light and no environment specular, so an environment pixel is its albedo
//! times the ambient irradiance it receives (after ambient occlusion), and post shows linear
//! values as they are. Grids are one brick of probes 1 m apart, built here: uniform light per
//! probe, so what a point receives is plain arithmetic.

use std::f32::consts::PI;

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::material::{
    AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, FoliageGradient, FoliageMaterial,
    Material, MaterialDocument, Outline, OutlineCurve, Rim, Terminator, Translucency, Wind as WindResponse,
};
use sapphire_assets::mesh::{
    FoliageVertex, MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::probes::{BRICK_PROBES, BRICK_SIDE, EMPTY_BRICK, ProbeData, ProbeGrid};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, AmbientOcclusion, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterGi,
    CharacterInstance, CharacterLighting, ColorGrade, DirectionalLight, EnvironmentSettings,
    EnvironmentSpecular, FramePacketRing, GtaoSettings, InstanceId, MaterialHandle, MeshHandle, MeshInstance,
    Msaa, PostParams, ProbeKeys, ProbeLookup, RenderSettings, RenderStats, Renderer, SkyGradient, ToneCurve,
    ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// Linear albedo of every test surface.
const ALBEDO: f32 = 0.8;
/// How far a pixel may be from a prediction: 8-bit output and the grid's half floats.
const TOLERANCE: f32 = 0.02;
/// The test grid's first probe; probes stand 1 m apart, [`BRICK_SIDE`] along each axis, so a
/// lookup on the ground at y = 0 (raised 0.3 m off it) is a full cell inside on every axis
/// for x and z within ±1 m.
const ORIGIN: Vec3 = Vec3::new(-2.0, -1.5, -2.0);
/// The L2 SH coefficient of the constant function 1 (`sapphire_math::sh`): √(4π).
const SH_CONSTANT: f32 = 3.544_907_7;
/// The L0 basis constant: an L1 irradiance of `E` everywhere has `E / SH_C0` as its first
/// coefficient.
const SH_C0: f32 = 0.282_094_8;

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

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// No MSAA, no temporal AA and no environment specular: each pixel is its albedo times its
/// irradiance, shaded once at its center. `occlusion` as asked.
fn settings(occlusion: AmbientOcclusion) -> RenderSettings {
    RenderSettings {
        msaa: Msaa::Off,
        environment: EnvironmentSettings {
            specular: EnvironmentSpecular::Off,
            probes: ProbeLookup::PerPixel,
        },
        ambient_occlusion: occlusion,
        ..RenderSettings::default()
    }
}

/// A grey environment material of [`ALBEDO`], Lambert, without terminator or distance styling.
fn plain() -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        roughness: 0.5,
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

/// A grey anime character material of [`ALBEDO`] without rim or outline.
fn character_material() -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, z_offset: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A foliage material of [`ALBEDO`] grey, lit plainly (no wrap, canopy occlusion, gradient,
/// translucency or distance fade) and still in any wind, opaque or, with a `cutoff`, cut out by
/// its card map's alpha.
fn leaves(cutoff: f32) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        alpha_cutoff: cutoff,
        wrap: 0.0,
        terminator_width: 0.0,
        terminator_growth: 0.0,
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: [1.0; 3], tip: [1.0; 3] },
        wind: WindResponse { sway: 0.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..FoliageMaterial::default()
    }
}

/// A 4 × 4 white card map whose alpha is `alpha` everywhere.
fn card_map(gpu: &GpuContext, alpha: f32) -> GpuTexture {
    let texel = [1.0, 1.0, 1.0, alpha].map(|c| f16::from_f32(c).to_bits());
    let texels = vec![texel; 16];
    let desc = TextureDesc {
        format: TextureFormat::Rgba16Float,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let bytes: &[u8] = bytemuck::cast_slice(&texels);
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![bytes] }, "card").expect("texture")
}

fn vertex(position: Vec3, normal: Vec3) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// A square centered at `center` facing `normal`, `half` from its center to each edge.
fn square(center: Vec3, normal: Vec3, half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let across = if normal.y.abs() > 0.9 { Vec3::X } else { Vec3::Y.cross(normal).normalize() };
    let up = normal.cross(across);
    let corner = |s: f32, t: f32| vertex(center + (across * s + up * t) * half, normal);
    (vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], vec![0, 1, 2, 0, 2, 3])
}

/// An up-facing square of `cells` × `cells` squares `side` meters wide, centered on the origin.
fn tessellated(cells: u32, side: f32) -> (Vec<Vertex>, Vec<u32>) {
    let half = 0.5 * cells as f32 * side;
    let row = cells + 1;
    let vertices = (0..row * row)
        .map(|i| {
            vertex(Vec3::new((i % row) as f32 * side - half, 0.0, (i / row) as f32 * side - half), Vec3::Y)
        })
        .collect();
    let indices = (0..cells * cells)
        .flat_map(|cell| {
            let corner = cell / cells * row + cell % cells;
            let [a, b, c, d] = [corner, corner + 1, corner + row + 1, corner + row];
            // Counterclockwise seen from above.
            [a, d, c, a, c, b]
        })
        .collect();
    (vertices, indices)
}

/// A box of half extents `half` around `center`, one square per face.
fn cuboid(center: Vec3, half: Vec3) -> (Vec<Vertex>, Vec<u32>) {
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for normal in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
        let (face, face_indices) = square(normal, normal, 1.0);
        let first = vertices.len() as u32;
        vertices.extend(face.into_iter().map(|mut v| {
            v.position = (center + Vec3::from(v.position) * half).to_array();
            v
        }));
        indices.extend(face_indices.into_iter().map(|index| first + index));
    }
    (vertices, indices)
}

/// A UV sphere of `radius` around `center`.
fn sphere(center: Vec3, radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 32;
    const SEGMENTS: u32 = 48;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = 2.0 * PI * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(vertex(center + n * radius, n));
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

/// `part` as a mesh of one material slot, with f32 positions.
fn mesh((vertices, indices): (Vec<Vertex>, Vec<u32>)) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("part", placeholder)],
        PositionFormat::Float32,
    )
}

/// A probe's place in the test brick: its index along x, y and z.
type Probe = [u32; 3];

/// A one-brick grid from [`ORIGIN`], every probe in open space: sky visibility `visibility`
/// (the same in every direction), per key in `hours` the uniform bounce irradiance
/// `bounce(probe, key)`, and the free distances `free(probe)` (+x, −x, +y, −y, +z, −z).
fn grid(
    hours: &[f32],
    visibility: f32,
    bounce: impl Fn(Probe, usize) -> Vec3,
    free: impl Fn(Probe) -> [u8; 6],
) -> ProbeGrid {
    let half = |value: f32| f16::from_f32(value).to_bits();
    let probes: Vec<Probe> = (0..BRICK_PROBES)
        .map(|i| [i % BRICK_SIDE, (i / BRICK_SIDE) % BRICK_SIDE, i / (BRICK_SIDE * BRICK_SIDE)])
        .collect();
    let visibility = probes
        .iter()
        .flat_map(|_| (0..9).map(|k| half(if k == 0 { visibility * SH_CONSTANT } else { 0.0 })))
        .collect();
    let bounce = probes
        .iter()
        .flat_map(|&probe| (0..hours.len()).map(move |key| (probe, key)))
        .flat_map(|(probe, key)| {
            let irradiance = bounce(probe, key);
            (0..3).flat_map(move |c| (0..4).map(move |k| if k == 0 { irradiance[c] / SH_C0 } else { 0.0 }))
        })
        .map(half)
        .collect();
    ProbeGrid {
        origin: ORIGIN.to_array(),
        spacing: 1.0,
        bricks: [1, 1, 1],
        brick_table: vec![0],
        brick_probes: (0..BRICK_PROBES).collect(),
        key_hours: hours.to_vec(),
        visibility_coefficients: 9,
        visibility,
        bounce,
        probe_data: probes
            .iter()
            .map(|&probe| ProbeData { free: free(probe), valid: true }.to_bits())
            .collect(),
    }
}

/// Free distances of a full spacing every way: nothing between any probe and its neighbors.
fn open(_: Probe) -> [u8; 6] {
    [ProbeData::FULL; 6]
}

/// One rendered frame.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    stats: RenderStats,
}

impl Frame {
    fn linear_at_pixel(&self, x: u32, y: u32) -> Vec3 {
        let pixel = self.pixels[(y * SIDE + x) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (u32, u32) {
        let ndc = (self.camera.proj * self.camera.view).project_point3(point);
        let (x, y) = ((ndc.x + 1.0) * 0.5 * SIDE as f32, (1.0 - ndc.y) * 0.5 * SIDE as f32);
        assert!((0.0..SIDE as f32).contains(&x) && (0.0..SIDE as f32).contains(&y), "{point} is out of view");
        (x as u32, y as u32)
    }

    /// The linear color where `point` shows.
    fn at(&self, point: Vec3) -> Vec3 {
        let (x, y) = self.pixel_of(point);
        self.linear_at_pixel(x, y)
    }
}

fn assert_near(actual: Vec3, expected: Vec3, what: &str) {
    assert!((actual - expected).abs().max_element() <= TOLERANCE, "{what}: {actual} against {expected}");
}

/// A renderer and everything a test frame needs.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    sky: SkyGradient,
    ambient: AmbientLight,
    keys: ProbeKeys,
    characters: Vec<CharacterInstance>,
    added: Vec<MeshInstance>,
    next_id: u64,
}

impl Harness {
    fn new(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let target = Self::target(&gpu, (SIDE, SIDE));
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: Self::looking(Vec3::new(0.0, 3.0, 2.5), Vec3::ZERO),
            sky: SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO },
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            keys: ProbeKeys::single(0),
            characters: Vec::new(),
            added: Vec::new(),
            next_id: 0,
        }
    }

    /// A readable target of `width` × `height` pixels.
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

    fn looking(eye: Vec3, target: Vec3) -> CameraData {
        CameraData::look_at(eye, target, Vec3::Y, 0.9, 1.0, 0.05)
    }

    fn material<D: MaterialDocument>(&mut self, document: &D) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    fn mesh(&mut self, mesh: &Mesh, material: MaterialHandle) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
    }

    /// Adds `part` in the [`plain`] environment material with the next frame.
    fn environment(&mut self, part: (Vec<Vertex>, Vec<u32>)) -> InstanceId {
        let material = self.material(&plain());
        let mesh = self.mesh(&mesh(part), material);
        self.add(mesh)
    }

    /// Adds `part` as foliage in `document`, cut out by `map` when given, with the next frame:
    /// still, and at the canopy's surface everywhere (gradient 1).
    fn foliage(
        &mut self,
        part: (Vec<Vertex>, Vec<u32>),
        document: &FoliageMaterial,
        map: Option<&GpuTexture>,
    ) -> InstanceId {
        let document = FoliageMaterial { base_color_map: map.map(|_| card_id()), ..document.clone() };
        let swizzles: Vec<Swizzle> = document
            .textures()
            .iter()
            .map(|slot| slot.and(map).map_or(Swizzle::IDENTITY, GpuTexture::swizzle))
            .collect();
        let bytes = Material::from_document(&document, &swizzles).expect("valid").to_cooked().expect("cook");
        let material =
            self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| map).expect("upload");
        let mut mesh = mesh(part);
        let still = FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        mesh.streams = mesh.streams.clone().with_foliage(&vec![still; mesh.streams.len()]);
        let mesh = self.mesh(&mesh, material);
        self.add(mesh)
    }

    fn add(&mut self, mesh: MeshHandle) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
        id
    }

    /// A ground 6 m square at height 0.
    fn ground(&mut self) -> InstanceId {
        self.environment(square(Vec3::ZERO, Vec3::Y, 3.0))
    }

    /// Lists `id` (with or without an instance) as a character with `body`, lit by its GI alone.
    fn character(&mut self, id: InstanceId, body: CharacterBody) {
        let key = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ZERO, intensity: 1.0 };
        let lighting = CharacterLighting { gi: CharacterGi::default(), ..CharacterLighting::unstyled(key) };
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance {
            id,
            lighting,
            face: CharacterFace::default(),
            body,
            opacity: 1.0,
        });
    }

    /// Uploads `grid` in place of the renderer's.
    fn set_grid(&mut self, grid: Option<&ProbeGrid>) {
        let bytes = grid.map(|grid| grid.to_cooked().expect("cook"));
        let archived = bytes.as_ref().map(|bytes| ProbeGrid::read(bytes).expect("a valid grid"));
        self.renderer.set_probe_grid(archived).expect("the grid fits");
    }

    /// Prepares, records and submits one frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 0.0 };
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.probe_keys = self.keys;
        packet.post = passthrough();
        packet.characters.extend_from_slice(&self.characters);
        packet.instances.changed.append(&mut self.added);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back(), camera: self.camera, stats: self.renderer.stats() }
    }

    /// Renders `frames` frames and returns the last.
    fn settle(&mut self, frames: u32) -> Frame {
        for _ in 1..frames {
            self.render();
        }
        self.render()
    }

    fn read_back(&self) -> Vec<[u8; 4]> {
        let (width, height) = (self.target.width(), self.target.height());
        let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * height),
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
                    rows_per_image: Some(height),
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
            .flat_map(|row| row[..width as usize * 4].as_chunks::<4>().0.iter().copied())
            .collect()
    }
}

#[test]
#[ignore = "needs a GPU"]
fn an_open_grid_lights_as_the_hemisphere_does_and_follows_the_sky() {
    // A grid that sees the sky every way and holds no bounce: an up-facing surface receives
    // exactly time of day's authored sky, as from the hemisphere, whatever the sky's color, and
    // follows it from the frame it changes.
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    harness.ground();
    let points = [Vec3::ZERO, Vec3::new(0.6, 0.0, -0.6), Vec3::new(-0.8, 0.0, 0.4)];
    let open_sky = grid(&[12.0], 1.0, |_, _| Vec3::ZERO, open);
    for (index, sky) in [Vec3::new(0.6, 0.35, 0.15), Vec3::new(0.15, 0.3, 0.6)].into_iter().enumerate() {
        harness.ambient = AmbientLight { sky, ground: Vec3::splat(0.05), intensity: 1.0 };
        harness.sky = SkyGradient { zenith: sky, horizon: sky * 0.8 + 0.1, ground: Vec3::splat(0.05) };
        harness.set_grid(None);
        let hemisphere = harness.settle(2);
        harness.set_grid(Some(&open_sky));
        // The sky changes the frame it does; the cube's faces catch up over six more.
        let first = harness.render();
        let settled = harness.settle(8);
        assert!(
            settled.stats.ambient.grid && settled.stats.ambient.blended_keys == 1,
            "{:?}",
            settled.stats.ambient
        );
        for point in points {
            assert_near(hemisphere.at(point), ALBEDO * sky, "the hemisphere facing up");
            assert_near(
                first.at(point),
                ALBEDO * sky,
                &format!("sky {index}, the grid's first frame at {point}"),
            );
            assert_near(settled.at(point), ALBEDO * sky, &format!("sky {index}, the grid at {point}"));
        }
    }
    // Half the sky hidden from every probe: half the light.
    let half_open = grid(&[12.0], 0.5, |_, _| Vec3::ZERO, open);
    harness.set_grid(Some(&half_open));
    let frame = harness.settle(2);
    assert_near(frame.at(Vec3::ZERO), 0.5 * ALBEDO * harness.ambient.sky, "half the sky");
}

#[test]
#[ignore = "needs a GPU"]
fn bounce_blends_the_keys_the_frame_names() {
    // Two keys of bounce, red and blue, no sky: each key alone, a blend, and none.
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    harness.ground();
    let (red, blue) = (Vec3::new(0.9, 0.1, 0.05), Vec3::new(0.05, 0.1, 0.9));
    harness.set_grid(Some(&grid(&[6.0, 18.0], 0.0, |_, key| [red, blue][key], open)));
    let blend = ProbeKeys { keys: [0, 1, 0, 0], weights: [0.25, 0.75, 0.0, 0.0] };
    let cases = [
        (ProbeKeys::single(0), red, 1),
        (ProbeKeys::single(1), blue, 1),
        (blend, 0.25 * red + 0.75 * blue, 2),
        (ProbeKeys::default(), Vec3::ZERO, 0),
    ];
    for (keys, irradiance, blended) in cases {
        harness.keys = keys;
        let frame = harness.render();
        assert_eq!(frame.stats.ambient.blended_keys, blended, "{keys:?}");
        assert_near(frame.at(Vec3::new(0.3, 0.0, 0.2)), ALBEDO * irradiance, &format!("{keys:?}"));
    }
}

#[test]
#[ignore = "needs a GPU"]
fn light_never_leaks_through_a_thin_wall() {
    // A wall 10 cm thick between the probes at x = 0 and x = 1: bounce is red on its −x side
    // and black on its +x side. Trilinear interpolation alone would bleed red through it and
    // darken the red side; the leak test drops the probes on the far side of the wall.
    let wall = (0.45, 0.55);
    let side = |probe: Probe| ORIGIN.x + probe[0] as f32;
    let red = |probe: Probe, _| if side(probe) < 0.5 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::ZERO };
    let measured = |probe: Probe| {
        let mut free = [ProbeData::FULL; 6];
        match side(probe) {
            x if x == 0.0 => free[0] = ProbeData::quantize(wall.0 - x, 1.0),
            x if x == 1.0 => free[1] = ProbeData::quantize(x - wall.1, 1.0),
            _ => {}
        }
        free
    };
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    harness.ground();
    harness.environment(cuboid(Vec3::new(0.5, 1.0, 0.0), Vec3::new(0.05, 1.0, 2.0)));
    harness.camera = Harness::looking(Vec3::new(0.5, 6.0, 2.5), Vec3::new(0.5, 0.0, 0.0));
    let (red_side, dark_side) = ([-0.3, 0.3], [0.65, 0.8, 0.95]);
    let at = |x: f32| Vec3::new(x, 0.0, 0.0);

    harness.set_grid(Some(&grid(&[12.0], 0.0, red, measured)));
    let frame = harness.render();
    for x in red_side {
        assert_near(frame.at(at(x)), Vec3::new(ALBEDO, 0.0, 0.0), &format!("the red side at x = {x}"));
    }
    for x in dark_side {
        assert_near(frame.at(at(x)), Vec3::ZERO, &format!("past the wall at x = {x}"));
    }

    // The same grid without its free distances: red leaks through, and the red side darkens.
    harness.set_grid(Some(&grid(&[12.0], 0.0, red, open)));
    let leaking = harness.render();
    assert!(leaking.at(at(0.65)).x > 0.2, "leaks without the test: {}", leaking.at(at(0.65)));
    assert!(leaking.at(at(0.3)).x < 0.65, "darkens without the test: {}", leaking.at(at(0.3)));
}

#[test]
#[ignore = "needs a GPU"]
fn characters_take_their_ambient_from_the_grid() {
    // A character in white hemisphere light, then inside a grid of red bounce: its GI turns red,
    // read from the grid at its body; without a body or head to read at, it keeps the hemisphere.
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    let material = harness.material(&character_material());
    let ball = harness.mesh(&mesh(sphere(Vec3::new(0.0, 1.0, 0.0), 0.4)), material);
    let id = harness.add(ball);
    let body = CharacterBody::capsule(Vec3::new(0.0, 0.7, 0.0), Vec3::new(0.0, 1.3, 0.0), 0.3);
    harness.character(id, body);
    harness.camera = Harness::looking(Vec3::new(0.0, 1.0, 3.0), Vec3::new(0.0, 1.0, 0.0));
    harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ONE, intensity: 0.5 };
    let front = Vec3::new(0.0, 1.0, 0.4);

    let white = harness.render().at(front);
    assert!(white.x > 0.05 && (white.x - white.y).abs() < TOLERANCE, "the hemisphere's grey: {white}");
    harness.set_grid(Some(&grid(&[12.0], 0.0, |_, _| Vec3::new(0.5, 0.05, 0.05), open)));
    let red = harness.render().at(front);
    assert!(red.x > 4.0 * red.y.max(0.005), "the grid's red: {red}");
    // No body and no head: nowhere to read the grid, so the hemisphere.
    harness.character(id, CharacterBody::default());
    let unplaced = harness.render().at(front);
    assert_near(unplaced, white, "no place to read the grid at");
}

#[test]
#[ignore = "needs a GPU"]
fn grids_replace_and_remove_in_place() {
    // A hot reload hands the renderer a new grid; removing it brings back the hemisphere, the
    // frame exactly as before there was one.
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    harness.ground();
    harness.ambient = AmbientLight { sky: Vec3::splat(0.4), ground: Vec3::splat(0.1), intensity: 1.0 };
    harness.sky =
        SkyGradient { zenith: Vec3::splat(0.4), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.1) };
    let before = harness.settle(2);
    assert!(!before.stats.ambient.grid);
    let point = Vec3::new(0.2, 0.0, 0.3);
    for (color, name) in [(Vec3::new(0.7, 0.05, 0.05), "red"), (Vec3::new(0.05, 0.05, 0.7), "blue")] {
        harness.set_grid(Some(&grid(&[12.0], 0.0, |_, _| color, open)));
        let frame = harness.render();
        let stats = frame.stats.ambient;
        assert_eq!(
            (stats.grid, stats.probes, stats.bricks, stats.keys),
            (true, BRICK_PROBES, 1, 1),
            "{name}"
        );
        assert_near(frame.at(point), ALBEDO * color, name);
    }
    harness.set_grid(None);
    let after = harness.settle(2);
    assert!(!after.stats.ambient.grid);
    assert!(before.pixels == after.pixels, "the hemisphere's frame again, exactly");
}

#[test]
#[ignore = "needs a GPU"]
fn nothing_is_built_or_run_for_work_nothing_reads() {
    // Without a grid or GTAO the ambient pass neither runs nor has a pipeline, and the reflection
    // cube waits for a grid even with environment specular on: a renderer pays for none of it
    // until the first grid arrives, which builds the light's pipelines, kept after it leaves.
    let full = EnvironmentSettings { specular: EnvironmentSpecular::Full, probes: ProbeLookup::PerPixel };
    let mut harness = Harness::new(RenderSettings { environment: full, ..settings(AmbientOcclusion::OFF) });
    harness.ground();
    let compute_pipelines = |harness: &Harness| {
        let report = harness.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.compute_pipelines.num_kept_from_user
    };
    let without = harness.settle(2).stats.ambient;
    assert_eq!((without.pipelines, without.dispatches, without.reflection_size), (0, 0, 0));
    let before = compute_pipelines(&harness);
    harness.set_grid(Some(&grid(&[12.0], 0.0, |_, _| Vec3::splat(0.5), open)));
    let arrived = harness.render().stats.ambient;
    // The sky's projection's three (each face's threads' sums, a face's sum and the sky light),
    // the prefilter, the relight and the characters'.
    assert_eq!(arrived.pipelines, 6);
    assert_eq!(compute_pipelines(&harness) - before, 6, "the stats count what was built");
    assert!(arrived.reflection_size > 0 && arrived.reflection_mips > 1);
    // The arrival projects (three dispatches) and prefilters every face and relights every
    // brick; no character entries.
    assert_eq!(arrived.dispatches, 3 + arrived.reflection_mips + 1);
    harness.set_grid(None);
    let left = harness.render().stats.ambient;
    assert_eq!((left.pipelines, left.dispatches), (6, 0));
    assert_eq!(compute_pipelines(&harness) - before, 6);
    // GTAO's three are built with a renderer that has it, and run every frame.
    let occlusion = AmbientOcclusion { capsules: true, gtao: Some(GtaoSettings::DESKTOP) };
    let mut harness = Harness::new(settings(occlusion));
    harness.ground();
    let frame = harness.render().stats.ambient;
    assert_eq!((frame.pipelines, frame.dispatches), (3, 3));
}

#[test]
#[ignore = "needs a GPU"]
fn the_grid_fades_out_before_an_empty_brick() {
    // The test brick with an empty brick above it, under a hemisphere of blue from every side and
    // red bounce in the grid: a wall rising through the empty brick's floor goes from the grid's
    // red to the hemisphere's blue over the last cell below it, never in a step, as a character
    // rising past the geometry the bake kept bricks around.
    let mut harness = Harness::new(settings(AmbientOcclusion::OFF));
    let blue = Vec3::new(0.0, 0.0, 0.8);
    harness.ambient = AmbientLight { sky: blue, ground: blue, intensity: 1.0 };
    let below = grid(&[12.0], 0.0, |_, _| Vec3::new(0.8, 0.0, 0.0), open);
    let stacked = ProbeGrid { bricks: [1, 2, 1], brick_table: vec![0, EMPTY_BRICK], ..below };
    harness.set_grid(Some(&stacked));
    harness.environment(square(Vec3::new(0.0, 1.8, 0.0), Vec3::Z, 1.6));
    let eye = Vec3::new(0.0, 1.8, 3.5);
    harness.camera = Harness::looking(eye, Vec3::new(0.0, 1.8, 0.0));
    let frame = harness.render();
    // The kept brick's top face, where its probes border the empty brick: the grid's share falls
    // from 1 a spacing below it to 0 there, at the point the lookup reads, off the wall along its
    // normal and toward the camera (`PROBE_VIEW_BIAS`).
    let top = ORIGIN.y + 4.0;
    for step in 0..=22 {
        let point = Vec3::new(0.0, 0.7 + 0.1 * step as f32, 0.0);
        let read = point.y + 0.1 * (eye - point).normalize().y;
        let expected = (top - read).clamp(0.0, 1.0);
        let color = frame.at(point) / ALBEDO;
        let share = color.x / 0.8;
        assert!((color.x + color.z - 0.8).abs() < 0.04, "red and blue at {point}: {color}");
        assert!((share - expected).abs() < 0.04, "the grid's share at {point}: {share} against {expected}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn per_vertex_lookups_read_the_grid_at_the_vertices() {
    // Bounce rising across the grid, red along x and green along z, under a dark hemisphere:
    // Mobile-Low's per-vertex read gives each vertex what the per-pixel read gives the pixel
    // there, and in between the vertices' blend, which on squares a meter wide stays close.
    let bounce = |probe: Probe, _| Vec3::new(0.1 + 0.15 * probe[0] as f32, 0.1 + 0.1 * probe[2] as f32, 0.05);
    let render = |probes: ProbeLookup| {
        let settings = RenderSettings {
            environment: EnvironmentSettings { probes, ..settings(AmbientOcclusion::OFF).environment },
            ..settings(AmbientOcclusion::OFF)
        };
        let mut harness = Harness::new(settings);
        harness.environment(tessellated(6, 1.0));
        harness.set_grid(Some(&grid(&[12.0], 0.0, bounce, open)));
        harness.camera = Harness::looking(Vec3::new(0.0, 4.0, 0.01), Vec3::ZERO);
        harness.render()
    };
    let (pixel, vertex) = (render(ProbeLookup::PerPixel), render(ProbeLookup::PerVertex));
    assert!(pixel.at(Vec3::ZERO).x > 0.2, "the grid lights the ground: {}", pixel.at(Vec3::ZERO));
    for at in [Vec3::ZERO, Vec3::new(1.0, 0.0, -1.0), Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0)] {
        // A pixel's center lies within a hair of the vertex.
        assert_near(vertex.at(at), pixel.at(at), &format!("at the vertex {at}"));
    }
    for at in [Vec3::new(0.5, 0.0, 0.5), Vec3::new(-0.5, 0.0, -0.3)] {
        assert!((vertex.at(at) - pixel.at(at)).abs().max_element() < 0.05, "between the vertices at {at}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn capsule_ao_darkens_the_ground_only_near_a_body() {
    // A body standing on the ground (its capsule alone, no mesh), under white sky light from
    // above: the ground beneath it darkens most, less and less away from it as the sphere at
    // its base occludes less of the sky, and not at all a few radii out, where the frame
    // matches one without capsule AO exactly.
    let ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
    let (base, radius) = (0.25, 0.25);
    let body = CharacterBody::capsule(Vec3::new(0.0, base, 0.0), Vec3::new(0.0, 1.5, 0.0), radius);
    let render = |occlusion: AmbientOcclusion| {
        let mut harness = Harness::new(settings(occlusion));
        harness.ground();
        harness.ambient = ambient;
        harness.character(InstanceId(100), body);
        harness.camera = Harness::looking(Vec3::new(0.0, 4.0, 2.0), Vec3::new(0.8, 0.0, 0.0));
        harness.render()
    };
    let with = render(AmbientOcclusion { capsules: true, gtao: None });
    let without = render(AmbientOcclusion::OFF);
    assert_eq!(with.stats.ambient.capsules, 1);
    let distances = [0.0, 0.3, 0.5, 0.8, 1.6, 2.4];
    let lightness: Vec<f32> = distances.iter().map(|&x| with.at(Vec3::new(x, 0.0, 0.0)).x).collect();
    let open = without.at(Vec3::new(0.0, 0.0, 0.0)).x;
    assert!((open - ALBEDO).abs() < TOLERANCE, "unoccluded: {open}");
    assert!(lightness[0] < 0.25 * open, "beneath the body: {lightness:?}");
    assert!(lightness.is_sorted(), "lighter farther out: {lightness:?}");
    for (&x, &measured) in distances.iter().zip(&lightness) {
        let expected = capsule_model(x, base, radius);
        assert!((measured - expected).abs() < TOLERANCE, "{x} m out: {measured} against {expected}");
    }
    assert!(lightness[1] < open - 0.04, "darker 30 cm out: {lightness:?}");
    for &x in &distances[4..] {
        let point = Vec3::new(x, 0.0, 0.0);
        assert_eq!(
            with.pixels[pixel_index(&with, point)],
            without.pixels[pixel_index(&without, point)],
            "{x} m out"
        );
    }
}

/// What the ground `x` meters from a body's axis shows under white sky light of 1 from above:
/// [`ALBEDO`] times its visibility after capsule AO (`ambient.wgsl`'s `capsule_occlusion`: the
/// sphere of `radius` at the body's base, `height` up, the point nearest it on the axis) through
/// Jimenez's multi-bounce fit for the albedo.
fn capsule_model(x: f32, height: f32, radius: f32) -> f32 {
    let distance = (x * x + height * height).sqrt();
    let facing = height / distance;
    let ratio = radius / distance.max(radius);
    let t = ((distance - radius - 2.0 * radius) / (2.0 * radius)).clamp(0.0, 1.0);
    let reach = 1.0 - t * t * (3.0 - 2.0 * t);
    let v = 1.0 - facing * ratio * ratio * reach;
    let (a, b, c) = (2.0404 * ALBEDO - 0.3324, -4.7951 * ALBEDO + 0.6417, 2.7552 * ALBEDO + 0.6903);
    ALBEDO * v.max(((v * a + b) * v + c) * v)
}

fn pixel_index(frame: &Frame, point: Vec3) -> usize {
    let (x, y) = frame.pixel_of(point);
    (y * SIDE + x) as usize
}

#[test]
#[ignore = "needs a GPU"]
fn gtao_darkens_creases_and_holds_still() {
    // A box on the ground under white sky light: GTAO darkens the ground along its foot and
    // leaves open ground and the box's top as they are; a still scene draws the same frame
    // again without temporal AA, and settles under it.
    let occlusion = AmbientOcclusion { capsules: false, gtao: Some(GtaoSettings::DESKTOP) };
    let scene = |settings: RenderSettings| {
        let mut harness = Harness::new(settings);
        harness.ground();
        harness.environment(cuboid(Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5)));
        harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
        harness.camera = Harness::looking(Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.2, 0.2, 0.2));
        harness
    };
    let (crease, open_ground, top) =
        (Vec3::new(0.56, 0.0, 0.2), Vec3::new(1.8, 0.0, 1.4), Vec3::new(0.0, 1.0, 0.2));

    let mut plain = scene(settings(AmbientOcclusion::OFF));
    let unoccluded = plain.render();
    let mut harness = scene(settings(occlusion));
    let frame = harness.render();
    assert!(frame.stats.ambient.gtao && frame.stats.ambient.gtao_size == (SIDE / 2, SIDE / 2));
    // Light albedo keeps much of the light occlusion takes (multi-bounce), so the crease
    // darkens by less than its visibility.
    let (open, creased) = (unoccluded.at(crease).x, frame.at(crease).x);
    assert!(creased < 0.95 * open, "the crease darkens: {creased} against {open}");
    let farther = frame.at(Vec3::new(0.9, 0.0, 0.2)).x;
    assert!(creased < farther && farther <= open, "less farther from the box: {creased}, {farther}, {open}");
    for (point, what) in [(open_ground, "open ground"), (top, "the box's top")] {
        let (before, after) = (unoccluded.at(point).x, frame.at(point).x);
        assert!(after > before - 0.03 && after <= before, "{what}: {after} against {before}");
    }
    assert!(harness.render().pixels == frame.pixels, "the same frame again");

    // Under temporal AA the pattern turns every frame and the resolve averages it: consecutive
    // frames of the settled crease differ by little.
    let mut temporal = scene(RenderSettings { temporal: true, ..settings(occlusion) });
    let settled = temporal.settle(32);
    let next = temporal.render();
    let (a, b) = (settled.at(crease).x, next.at(crease).x);
    assert!((a - b).abs() < 0.01, "still: {a} then {b}");
    assert!(a < 0.95 * open, "and dark: {a} against {open}");
}

#[test]
#[ignore = "needs a GPU"]
fn gtao_leaves_scenes_too_small_for_its_mips_open() {
    // Review PW-001: GTAO's half-resolution view distance has `GTAO_DEPTH_MIPS` (4) mips, which a
    // texture needs 8 texels a side for, so a scene 14 pixels or less on both sides failed
    // validation. There GTAO now makes no textures and dispatches nothing, and the environment
    // reads the open stand-in, as without GTAO; growing the scene again brings it back, and
    // shrinking it again drops it without leaving stale occlusion. Without temporal AA and with
    // it, under MSAA and without.
    let occlusion = AmbientOcclusion { capsules: false, gtao: Some(GtaoSettings::DESKTOP) };
    let scene = |settings: RenderSettings| {
        let mut harness = Harness::new(settings);
        harness.ground();
        harness.environment(cuboid(Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5)));
        harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
        harness.camera = Harness::looking(Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.2, 0.2, 0.2));
        harness
    };
    // Target size and render scale, and the GTAO texture size expected: none below 15 pixels.
    let sizes = [
        ((1, 1), 1.0, (0, 0)),
        ((8, 8), 1.0, (0, 0)),
        ((14, 14), 1.0, (0, 0)),
        ((15, 15), 1.0, (8, 8)),
        ((1, 64), 1.0, (1, 32)),
        ((32, 32), 0.25, (0, 0)),
        ((64, 64), 1.0, (32, 32)),
        ((8, 8), 1.0, (0, 0)),
    ];
    for base in [
        settings(occlusion),
        RenderSettings { msaa: Msaa::X4, ..settings(occlusion) },
        RenderSettings { temporal: true, ..settings(occlusion) },
    ] {
        let mut harness = scene(base);
        let mut open = scene(RenderSettings { ambient_occlusion: AmbientOcclusion::OFF, ..base });
        let mut first_tiny = None;
        for (size, scale, expected) in sizes {
            for harness in [&mut harness, &mut open] {
                harness.target = Harness::target(&harness.gpu, size);
                harness.renderer.set_render_scale(scale);
            }
            let what = format!("{size:?} at {scale}, temporal {}, {:?}", base.temporal, base.msaa);
            let frame = harness.render();
            // Rendered every frame too, so temporal AA's jitter and history keep in step.
            let reference = open.render();
            assert_eq!(frame.stats.ambient.gtao_size, expected, "{what}");
            assert_eq!(frame.stats.ambient.dispatches, if expected == (0, 0) { 0 } else { 3 }, "{what}");
            if expected != (0, 0) {
                continue;
            }
            // Open occlusion: as the renderer without GTAO draws it.
            let worst = frame
                .pixels
                .iter()
                .zip(&reference.pixels)
                .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)))
                .max()
                .expect("pixels");
            assert!(worst <= 1, "{what}: up to {worst} code values from no GTAO");
            if size == (8, 8) {
                let first = first_tiny.get_or_insert_with(|| frame.pixels.clone());
                assert!(base.temporal || *first == frame.pixels, "{what}: the same frame as before");
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gtao_is_mirror_symmetric() {
    // A corridor between two walls under white sky light, seen down its middle: mirrored
    // geometry gives mirrored occlusion, about the screen's vertical center line and, with the
    // camera rolled a quarter turn, its horizontal one. Samples a half-resolution texel apart
    // read that texel's own pixel, so no screen direction occludes more than its mirror.
    let occlusion = AmbientOcclusion { capsules: false, gtao: Some(GtaoSettings::DESKTOP) };
    for (up, horizontal) in [(Vec3::Y, true), (Vec3::X, false)] {
        let scene = |settings: RenderSettings| {
            let mut harness = Harness::new(settings);
            harness.ground();
            for x in [-1.0, 1.0] {
                harness.environment(cuboid(Vec3::new(x, 0.6, -0.5), Vec3::new(0.1, 0.6, 2.0)));
            }
            harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
            harness.camera =
                CameraData::look_at(Vec3::new(0.0, 2.0, 2.4), Vec3::new(0.0, 0.0, -0.8), up, 0.9, 1.0, 0.05);
            harness
        };
        let plain = scene(settings(AmbientOcclusion::OFF)).render();
        let frame = scene(settings(occlusion)).render();
        // What occlusion takes from each pixel, summed over each half of the frame.
        let mut halves = [0.0f32; 2];
        for y in 0..SIDE {
            for x in 0..SIDE {
                let open = plain.linear_at_pixel(x, y).x;
                if open < 0.05 {
                    continue;
                }
                let taken = (1.0 - frame.linear_at_pixel(x, y).x / open).max(0.0);
                let half = if horizontal { x < SIDE / 2 } else { y < SIDE / 2 };
                halves[usize::from(half)] += taken;
            }
        }
        let [a, b] = halves;
        assert!(a > 1.0 && b > 1.0, "both creases darken: {a}, {b}");
        assert!((a - b).abs() <= 0.08 * a.max(b), "mirrored halves (up {up}): {a} against {b}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gtao_leaves_characters_and_their_lines_out() {
    // A ball on the ground under white sky light, in the environment's material and then as a
    // character with a wide outline: GTAO darkens the ground around the environment's ball, and
    // leaves the ground around the character's as it was, for capsule AO alone to darken. Its
    // surface and its hull both count: with the hull in, the ground beside it lost 30%; with
    // both, 60%. What remains is open ground's 2%.
    let occlusion = AmbientOcclusion { capsules: false, gtao: Some(GtaoSettings::DESKTOP) };
    let ball = sphere(Vec3::new(0.0, 0.5, 0.0), 0.5);
    let flat = OutlineCurve { near: (0.0, 1.0), mid: 1.0, far: (1000.0, 1.0), cutoff: 1000.0 };
    let outlined = AnimeCharacterMaterial {
        outline: Outline { width: 60.0, curve: flat, flatten: 0.0, ..Outline::default() },
        ..character_material()
    };
    let render = |occlusion: AmbientOcclusion, character: bool| {
        let mut harness = Harness::new(settings(occlusion));
        harness.ground();
        harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
        harness.camera = Harness::looking(Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.0, 0.3, 0.0));
        if character {
            let material = harness.material(&outlined);
            let mesh = harness.mesh(&mesh(ball.clone()), material);
            let id = harness.add(mesh);
            harness.character(id, CharacterBody::default());
        } else {
            harness.environment(ball.clone());
        }
        harness.render()
    };
    // The most GTAO takes from any pixel, as a share of its light.
    let taken = |character: bool| {
        let (plain, frame) = (render(AmbientOcclusion::OFF, character), render(occlusion, character));
        let mut most = 0.0f32;
        for y in 0..SIDE {
            for x in 0..SIDE {
                let open = plain.linear_at_pixel(x, y).x;
                if open >= 0.05 {
                    most = most.max(1.0 - frame.linear_at_pixel(x, y).x / open);
                }
            }
        }
        most
    };
    let (environment, character) = (taken(false), taken(true));
    assert!(environment > 0.2, "the environment's ball darkens the ground: {environment}");
    assert!(character < 0.03, "the character's doesn't: {character}");
}

/// The ID the test card maps are bound to.
fn card_id() -> sapphire_assets::AssetId {
    "5b0e9c1d-7a2f-4e3b-9c4d-1e2f3a4b5c6d".parse().expect("a valid asset ID")
}

#[test]
#[ignore = "needs a GPU"]
fn foliage_takes_the_grid_and_ambient_occlusion_as_the_environment() {
    // A bed of leaves (an opaque foliage card facing up, its volume normal up) where the
    // environment's ground was, in three of the scenes above: inside a grid of red bounce, beneath
    // a body under capsule AO, and along a box's foot under GTAO. Lit plainly, it shows what the
    // environment's ground shows, each effect included.
    let ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
    let body = CharacterBody::capsule(Vec3::new(0.0, 0.25, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.25);
    let all = AmbientOcclusion { capsules: true, gtao: Some(GtaoSettings::DESKTOP) };
    let render = |occlusion: AmbientOcclusion, foliage: bool, scene: &dyn Fn(&mut Harness)| {
        let mut harness = Harness::new(settings(occlusion));
        let ground = square(Vec3::ZERO, Vec3::Y, 3.0);
        if foliage {
            harness.foliage(ground, &leaves(0.0), None);
        } else {
            harness.environment(ground);
        }
        harness.ambient = ambient;
        scene(&mut harness);
        harness.render()
    };
    let red = |harness: &mut Harness| {
        harness.set_grid(Some(&grid(&[12.0], 0.0, |_, _| Vec3::new(0.5, 0.05, 0.05), open)));
        harness.camera = Harness::looking(Vec3::new(0.0, 4.0, 0.01), Vec3::ZERO);
    };
    let capsule = |harness: &mut Harness| {
        harness.character(InstanceId(100), body);
        harness.camera = Harness::looking(Vec3::new(0.0, 4.0, 2.0), Vec3::new(0.8, 0.0, 0.0));
    };
    let crease = |harness: &mut Harness| {
        harness.environment(cuboid(Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5)));
        harness.camera = Harness::looking(Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.2, 0.2, 0.2));
    };
    // What each scene adds to the ground, and the point on it compared.
    type Setup<'a> = &'a dyn Fn(&mut Harness);
    let cases: [(&str, Setup<'_>, Vec3); 3] = [
        ("the grid's red bounce", &red, Vec3::ZERO),
        ("capsule AO beneath a body", &capsule, Vec3::new(0.3, 0.0, 0.0)),
        ("GTAO along a box's foot", &crease, Vec3::new(0.56, 0.0, 0.2)),
    ];
    for (what, scene, point) in cases {
        let (leaves, ground) = (render(all, true, scene).at(point), render(all, false, scene).at(point));
        assert_near(leaves, ground, what);
        let open = render(AmbientOcclusion::OFF, true, scene).at(point);
        if what == "the grid's red bounce" {
            assert!(leaves.x > 4.0 * leaves.y.max(0.005), "{what}: {leaves}");
        } else {
            assert!(leaves.x < 0.95 * open.x, "{what} darkens the leaves: {leaves} against {open}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gtao_sees_cards_as_the_prepass_cuts_them() {
    // A leaf card standing on the ground under white sky light, its map's alpha 0 everywhere and
    // then 1: GTAO sees what the scene prepass draws, and the prepass cuts cards out as the main
    // pass does (M1 W6), so the cut-out card leaves the frame exactly as without it, no halo of its
    // quad on the ground, while the solid one darkens the ground along its foot.
    let occlusion = AmbientOcclusion { capsules: false, gtao: Some(GtaoSettings::DESKTOP) };
    let render = |alpha: Option<f32>| {
        let mut harness = Harness::new(settings(occlusion));
        harness.ground();
        harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 };
        harness.camera = Harness::looking(Vec3::new(2.0, 2.5, 3.0), Vec3::new(0.0, 0.3, 0.0));
        if let Some(alpha) = alpha {
            let map = card_map(&harness.gpu, alpha);
            harness.foliage(square(Vec3::new(0.0, 0.5, 0.0), Vec3::Z, 0.5), &leaves(0.5), Some(&map));
        }
        harness.render()
    };
    let (bare, cut, solid) = (render(None), render(Some(0.0)), render(Some(1.0)));
    assert!(cut.pixels == bare.pixels, "a card cut out whole leaves no trace");
    let foot = Vec3::new(0.3, 0.0, 0.1);
    let (open, darkened) = (bare.at(foot).x, solid.at(foot).x);
    assert!(darkened < 0.95 * open, "a solid card darkens its foot: {darkened} against {open}");
}
