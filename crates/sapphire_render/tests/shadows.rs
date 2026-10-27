//! Shadow tests that need a GPU (M1 R4): the cascades' stability and bias, who casts into what
//! and who receives what, faces taking the environment's shadow whole, the characters' own maps
//! and capsules, the environment shader's tinted shadows, and stability under temporal AA. Run
//! with `cargo test -p sapphire_render --test shadows -- --ignored`.
//!
//! Scenes use no ambient light, a black sky and no rim or outline, so every surface pixel is its
//! material's lit tone (albedo × the key light) or its shade tone (albedo × [`SHADE`]), and a
//! pixel's red channel tells which. Most surfaces are the anime character family's, whose flat
//! tones read plainly; the environment family's (M1 W4) are tested on their own.
//!
//! Set `SAPPHIRE_SHADOW_DUMP=<dir>` to write the frames the tests read there as PNGs.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Quat, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, CharacterSurface, DistanceStylization, EnvironmentMaterial,
    FaceShading, LightSplit, Material, MaterialDocument, Outline, Rim, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    BloomParams, CameraData, CharacterBody, CharacterFace, CharacterGi, CharacterInstance, CharacterLighting,
    ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MaterialHandle, MeshHandle, MeshInstance,
    Msaa, PostParams, RenderSettings, RenderStats, Renderer, ShaderError, ShaderFile, ShaderSources,
    ShaderUpdate, ShadowFilter, ShadowSettings, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Square targets this many pixels a side.
const SIZE: u32 = 256;
/// The cameras' vertical field of view.
const FOV: f32 = 40.0 * PI / 180.0;
/// Linear albedo of every test surface.
const ALBEDO: f32 = 0.8;
/// The shade tone's tint: albedo × this where a surface is in shadow.
const SHADE: [f32; 3] = [0.25, 0.3, 0.5];
/// The face map's ID, bound to each test's texture.
const SDF: &str = "6a4f3e2d-1c0b-4a9f-8e7d-6c5b4a3f2e1d";

fn id(text: &str) -> AssetId {
    text.parse().expect("a valid asset ID")
}

/// Desktop-High's shadows.
fn desktop() -> RenderSettings {
    RenderSettings { shadows: Some(ShadowSettings::default()), ..RenderSettings::default() }
}

/// Mobile-Low's: two cascades of 1024, one cached, the 3 × 3 tent and capsules for characters.
fn mobile() -> RenderSettings {
    let shadows = ShadowSettings {
        cascades: 2,
        resolution: 1024,
        distance: 40,
        cached_cascades: 1,
        character_shadow: false,
        filter: ShadowFilter::Tent3,
        ..ShadowSettings::default()
    };
    RenderSettings { shadows: Some(shadows), ..RenderSettings::default() }
}

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

/// A surface split at N·L = `threshold` with a one-pixel edge, in band mode with the [`SHADE`]
/// tone, without rim or outline.
fn surface(threshold: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        light_split: LightSplit { threshold, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: SHADE, ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, z_offset: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// An environment surface of [`ALBEDO`] with Lambert's falloff, a one-pixel terminator and no
/// highlight or distance stylization: lit, albedo × (keep + (1 − keep) N·L) of the key light;
/// in shadow, albedo × keep, where keep is the frame's shadow tint.
fn environment() -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

/// A face over the [`SDF`] map, otherwise as [`surface`] split at 0.
fn face_surface() -> AnimeCharacterMaterial {
    let face = FaceShading { softness: 0.0, highlight: 0.0, ..FaceShading::new(id(SDF)) };
    AnimeCharacterMaterial { surface: CharacterSurface::Face(face), ..surface(0.0) }
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// `vertices` and `indices` with one (placeholder) material slot; tests draw it with their own.
fn mesh_of(vertices: &[Vertex], indices: Vec<u32>) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&surface(0.0), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("surface", placeholder)],
        PositionFormat::Unorm16,
    )
}

/// The square from −1 to 1 in X and Z at the origin, facing up.
fn ground() -> Mesh {
    let vertices: Vec<Vertex> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .map(|(x, z)| vertex(Vec3::new(x, 0.0, z), Vec3::Y, [(x + 1.0) * 0.5, (z + 1.0) * 0.5]))
        .collect();
    mesh_of(&vertices, vec![0, 2, 1, 0, 3, 2])
}

/// The square from −1 to 1 in X and Y at the origin, facing +Z, `u` left to right and `v` top to
/// bottom, as a front-projected face is mapped.
fn upright_square() -> Mesh {
    let vertices: Vec<Vertex> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .map(|(x, y)| vertex(Vec3::new(x, y, 0.0), Vec3::Z, [(x + 1.0) * 0.5, (1.0 - y) * 0.5]))
        .collect();
    mesh_of(&vertices, vec![0, 1, 2, 0, 2, 3])
}

/// The box of 1 × 1 × 1 standing on the origin, with hard edges.
fn cuboid() -> Mesh {
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for (normal, u, v) in faces {
        let first = vertices.len() as u32;
        for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let p = normal * 0.5 + u * a + v * b + Vec3::new(0.0, 0.5, 0.0);
            vertices.push(vertex(p, normal, [a + 0.5, 0.5 - b]));
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    mesh_of(&vertices, indices)
}

/// A UV sphere of radius 1 at the origin.
fn sphere() -> Mesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(vertex(n, n, [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32]));
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
    mesh_of(&vertices, indices)
}

/// A camera at `eye` looking at `target`, square, with up +Y.
fn looking(eye: Vec3, target: Vec3) -> CameraData {
    CameraData::look_at(eye, target, Vec3::Y, FOV, 1.0, 0.05)
}

/// A camera `height` meters above `center` looking straight down, the image's right +X and its
/// down +Z: ground positions map to pixels linearly.
fn top_down(center: Vec3, height: f32) -> CameraData {
    CameraData::look_at(center + Vec3::Y * height, center, Vec3::NEG_Z, FOV, 1.0, 0.05)
}

/// A vector toward the light at `elevation` degrees above the horizon, its horizontal part
/// `azimuth` degrees from +X toward +Z.
fn toward(elevation: f32, azimuth: f32) -> Vec3 {
    let (e, a) = (elevation.to_radians(), azimuth.to_radians());
    Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
}

/// One rendered frame.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    stats: RenderStats,
}

impl Frame {
    fn linear(&self, x: usize, y: usize) -> [f32; 3] {
        let pixel = self.pixels[y * SIZE as usize + x];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// The pixel `point` projects into, if it is in the image.
    fn pixel_of(&self, point: Vec3) -> Option<(usize, usize)> {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let (px, py) = ((x + 1.0) * 0.5 * SIZE as f32, (1.0 - y) * 0.5 * SIZE as f32);
        let inside = clip.w > 0.0 && (0.0..SIZE as f32).contains(&px) && (0.0..SIZE as f32).contains(&py);
        inside.then_some((px as usize, py as usize))
    }

    fn at(&self, point: Vec3) -> [f32; 3] {
        let (x, y) = self.pixel_of(point).unwrap_or_else(|| panic!("{point} is out of view"));
        self.linear(x, y)
    }

    /// Whether `point` shows its surface's lit tone.
    fn lit_at(&self, point: Vec3) -> bool {
        is_lit(self.at(point))
    }

    /// Whether `point` shows its surface's shade tone.
    fn shaded_at(&self, point: Vec3) -> bool {
        is_shaded(self.at(point))
    }

    fn dump(&self, name: &str) {
        let Some(dir) = std::env::var_os("SAPPHIRE_SHADOW_DUMP") else { return };
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        let file = std::fs::File::create(&path).expect("create the dump");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(bytemuck::cast_slice(&self.pixels)).expect("pixels");
    }
}

/// The lit tone: albedo × a white key light of intensity 1.
fn is_lit(rgb: [f32; 3]) -> bool {
    rgb[0] > 0.6 && (rgb[2] - rgb[0]).abs() < 0.1
}

/// The shade tone: albedo × [`SHADE`].
fn is_shaded(rgb: [f32; 3]) -> bool {
    (rgb[0] - ALBEDO * SHADE[0]).abs() < 0.06 && (rgb[2] - ALBEDO * SHADE[2]).abs() < 0.08
}

/// A renderer and the scene it draws.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    sun: DirectionalLight,
    /// The share of the key light environment surfaces keep in shadow.
    shadow_tint: Vec3,
    /// Copied into every packet.
    characters: Vec<CharacterInstance>,
    added: Vec<MeshInstance>,
    removed: Vec<InstanceId>,
    next_id: u64,
}

impl Harness {
    fn new(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
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
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: looking(Vec3::new(0.0, 2.0, 6.0), Vec3::ZERO),
            sun: DirectionalLight { direction: -toward(45.0, 0.0), color: Vec3::ONE, intensity: 1.0 },
            shadow_tint: Vec3::new(0.4, 0.45, 0.65),
            characters: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            next_id: 0,
        }
    }

    fn sun_toward(&mut self, toward: Vec3) {
        self.sun.direction = -toward;
    }

    fn material<D: MaterialDocument>(
        &mut self,
        document: &D,
        textures: &[(&str, &GpuTexture)],
    ) -> MaterialHandle {
        let swizzles: Vec<Swizzle> = {
            document
                .textures()
                .iter()
                .map(|slot| {
                    slot.and_then(|asked| textures.iter().find(|(name, _)| id(name) == asked))
                        .map_or(Swizzle::IDENTITY, |(_, texture)| texture.swizzle())
                })
                .collect()
        };
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    fn mesh(&mut self, mesh: &Mesh, material: MaterialHandle) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
    }

    /// An instance of `mesh` drawn with `document`, from the next frame.
    fn add<D: MaterialDocument>(&mut self, mesh: &Mesh, document: &D, transform: Mat4) -> InstanceId {
        let material = self.material(document, &[]);
        let mesh = self.mesh(mesh, material);
        self.add_mesh(mesh, transform)
    }

    fn add_mesh(&mut self, mesh: MeshHandle, transform: Mat4) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        id
    }

    /// A 200 m ground at height 0.
    fn ground(&mut self) -> InstanceId {
        let place = Mat4::from_scale(Vec3::new(100.0, 1.0, 100.0));
        self.add(&ground(), &surface(-0.95), place)
    }

    /// A box `size` large standing at `at`.
    fn cuboid(&mut self, at: Vec3, size: Vec3) -> InstanceId {
        self.add(&cuboid(), &surface(-0.2), Mat4::from_scale_rotation_translation(size, Quat::IDENTITY, at))
    }

    /// Lists `id` as a character lit by the world's key light (unstyled), with `body` and its
    /// head at `head`.
    fn character(&mut self, id: InstanceId, body: CharacterBody, head: Option<Vec3>) {
        let mut lighting = CharacterLighting::unstyled(self.sun);
        lighting.gi = CharacterGi { head, ..CharacterGi::default() };
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance { id, lighting, face: CharacterFace::default(), body });
    }

    /// Prepares, records and submits one frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = self.sun;
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient.intensity = 0.0;
        packet.shadow_tint = self.shadow_tint;
        packet.post = passthrough();
        packet.characters.extend_from_slice(&self.characters);
        packet.instances.changed.append(&mut self.added);
        packet.instances.removed.append(&mut self.removed);
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

    /// A uniform face map: lit for any light (R and G at 1).
    fn lit_face_map(&self) -> GpuTexture {
        let texels = vec![[f16::ONE.to_bits(), f16::ONE.to_bits(), 0, f16::ONE.to_bits()]; 16];
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width: 4,
            height: 4,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        GpuTexture::upload(&self.gpu, &Texture { desc, levels: vec![bytes] }, "face map").expect("texture")
    }
}

/// Lit and shaded pixels among those whose surface `covered` accepts.
fn count(frame: &Frame, covered: impl Fn(usize, usize) -> bool) -> (u32, u32) {
    let (mut lit, mut shaded) = (0, 0);
    for y in 0..SIZE as usize {
        for x in 0..SIZE as usize {
            if covered(x, y) {
                let rgb = frame.linear(x, y);
                lit += u32::from(is_lit(rgb));
                shaded += u32::from(is_shaded(rgb));
            }
        }
    }
    (lit, shaded)
}

#[test]
#[ignore = "needs a GPU"]
fn lit_surfaces_never_shadow_themselves() {
    // A ground seen from eye height out to the horizon, through every cascade, under lights from
    // steep to grazing: nothing casts, so every ground pixel is lit. Acne would leave shade
    // speckles, mostly where the light grazes.
    for (name, settings) in [("desktop", desktop()), ("mobile", mobile())] {
        for (elevation, azimuth) in [(70.0, 30.0), (45.0, 120.0), (20.0, 200.0), (6.0, 300.0), (6.0, 90.0)] {
            let mut harness = Harness::new(settings);
            harness.ground();
            harness.sun_toward(toward(elevation, azimuth));
            harness.camera = looking(Vec3::new(0.0, 1.7, 0.0), Vec3::new(0.0, 0.0, -20.0));
            let frame = harness.render();
            frame.dump(&format!("acne-{name}-{elevation}-{azimuth}"));
            let (lit, shaded) = count(&frame, |x, y| frame.linear(x, y)[0] > 0.0);
            assert_eq!(shaded, 0, "{name} at {elevation}° from {azimuth}°: {shaded} shaded of {lit} lit");
            assert!(lit > SIZE * SIZE / 3, "{name}: the ground fills the lower half");
        }
    }
}

/// A box 1 m square and 2 m tall at the origin lit from +X at 45°, seen from `height` meters
/// above `center`.
fn box_scene(settings: RenderSettings, center: Vec3, height: f32) -> Harness {
    let mut harness = Harness::new(settings);
    harness.ground();
    harness.cuboid(Vec3::ZERO, Vec3::new(1.0, 2.0, 1.0));
    harness.sun_toward(toward(45.0, 0.0));
    harness.camera = top_down(center, height);
    harness
}

/// The ground position at pixel column `x` of the middle row, for a camera from [`top_down`]
/// `height` above `center`.
fn ground_x(center: Vec3, height: f32, x: usize) -> f32 {
    let half = height * (FOV * 0.5).tan();
    center.x + ((x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0) * half
}

#[test]
#[ignore = "needs a GPU"]
fn shadows_meet_their_casters_and_end_where_the_light_says() {
    // The box's shadow on the ground runs from its −X face 2 m toward −X (a 2 m box under a 45°
    // light) and across its 1 m width. Peter-panning would open a lit gap at its base and pull
    // its far end in; acne would mark the lit ground. Measured along the middle row (z = 0) of a
    // top-down view, with pixels of about 1.1 cm (cascade 0's texels are 5 mm).
    let (center, height) = (Vec3::new(-1.45, 0.0, 0.0), 4.0);
    let pixel = 2.0 * height * (FOV * 0.5).tan() / SIZE as f32;
    let tolerance = 2.0 * pixel;
    for (name, settings) in [("desktop", desktop()), ("mobile", mobile())] {
        let mut harness = box_scene(settings, center, height);
        let frame = harness.render();
        frame.dump(&format!("contact-{name}"));
        let row = SIZE as usize / 2;
        // Every pixel from the box's foot out to the shadow's end is shade; past it, lit.
        let tone = |x: usize| frame.linear(x, row);
        let foot = (0..SIZE as usize).rev().find(|&x| ground_x(center, height, x) < -0.5).expect("in view");
        let end = (0..=foot).rev().find(|&x| !is_shaded(tone(x))).expect("the shadow ends in view");
        let gap = (end + 1..=foot).filter(|&x| is_lit(tone(x))).count();
        assert_eq!(gap, 0, "{name}: lit pixels between the foot and the shadow's end");
        let length = -0.5 - ground_x(center, height, end + 1);
        assert!((length - 2.0).abs() <= tolerance + pixel, "{name}: the shadow is {length} m long, not 2");
        assert!(is_lit(tone(end.saturating_sub(2))), "{name}: lit past the end");
        // Its sides: shade within the box's width, lit outside, at x = −1.5.
        let column = |z: f32| frame.at(Vec3::new(-1.5, 0.0, z));
        assert!(is_shaded(column(0.5 - tolerance)) && is_shaded(column(-0.5 + tolerance)), "{name}: sides");
        assert!(is_lit(column(0.5 + tolerance)) && is_lit(column(-0.5 - tolerance)), "{name}: outside");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn shadows_stay_on_the_world_as_the_camera_moves() {
    // Moving a top-down camera by 4 pixels moves the ground's image by exactly 4 pixels (an even
    // count, so the 2 × 2 quads screen-space derivatives are taken over pair the same pixels). The
    // cascades move with the camera by a fraction of a texel's worth too (4 pixels are 11.7 of
    // cascade 0's texels), but snapped to whole texels their grid stays fixed to the world, so the
    // shadow's pixels move by exactly 4 as well: nothing shimmers.
    let (center, height) = (Vec3::new(-1.45, 0.0, 0.0), 4.0);
    let pixel = 2.0 * height * (FOV * 0.5).tan() / SIZE as f32;
    let mut harness = box_scene(desktop(), center, height);
    let before = harness.render();
    harness.camera = top_down(center + Vec3::X * 4.0 * pixel, height);
    let after = harness.render();
    after.dump("snapped-after");
    // Compare the ground away from the box, whose top is nearer the camera and moves further:
    // at half the camera's height, it spans twice its width in the image.
    let box_columns = |x: usize| (-1.2..1.2).contains(&ground_x(center, height, x));
    let (mut compared, mut differing) = (0, 0);
    for y in 0..SIZE as usize {
        for x in 0..SIZE as usize - 4 {
            if box_columns(x + 4) || box_columns(x) {
                continue;
            }
            let (a, b) = (before.pixels[y * SIZE as usize + x + 4], after.pixels[y * SIZE as usize + x]);
            compared += 1;
            differing += usize::from(a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 1));
        }
    }
    assert_eq!(differing, 0, "{differing} of {compared} ground pixels changed beyond the shift");
}

#[test]
#[ignore = "needs a GPU"]
fn the_key_light_is_quantized_and_steps_redraw_every_cascade() {
    // A turn of the sun smaller than half a quantization step leaves every shadow exactly where it
    // was; a larger one moves them. The light is chosen on the grid: 45° from +X has a shadow
    // slope of exactly 1.
    let (center, height) = (Vec3::new(-1.45, 0.0, 0.0), 4.0);
    let mut harness = box_scene(desktop(), center, height);
    harness.render();
    let still = harness.render();
    assert_eq!(still.stats.shadows.cascades_drawn, 3, "warm: the far two take turns");
    // A slope of (1.001, 0.0009): under half of the 0.005 step either way.
    harness.sun_toward(Vec3::new(1.001, 1.0, 0.0009).normalize());
    let turned = harness.render();
    assert_eq!(turned.pixels, still.pixels, "a sub-step turn moves nothing");
    assert_eq!(turned.stats.shadows.light_steps, 0);
    harness.sun_toward(Vec3::new(1.1, 1.0, 0.0).normalize());
    let stepped = harness.render();
    stepped.dump("quantized-stepped");
    assert_eq!(stepped.stats.shadows.light_steps, 1);
    assert_eq!(stepped.stats.shadows.cascades_drawn, 4, "a step draws every cascade again");
    // The shadow's far end moved by 0.2 m.
    assert!(stepped.shaded_at(Vec3::new(-2.6, 0.0, 0.0)) && still.lit_at(Vec3::new(-2.6, 0.0, 0.0)));
}

/// A box `scale` meters wide and three times as tall at the origin, lit from +X at 45°, seen from
/// `height` meters straight above a point `scale` meters toward −X of it: at heights from a few
/// meters to past the shadow distance its shadow falls in each cascade in turn, the image the same
/// at every scale. Returns the frame and the shadow's middle and a point beside it in the light.
fn far_box(harness: &mut Harness, height: f32) -> (Frame, Vec3, Vec3) {
    let scale = height * 0.1;
    harness.camera = top_down(Vec3::new(-scale, 0.0, 0.0), height);
    let frame = harness.render();
    let shadow = Vec3::new(-0.5 * scale - 1.5 * scale, 0.0, 0.0);
    (frame, shadow, shadow + Vec3::new(0.0, 0.0, 1.2 * scale))
}

/// A harness with a ground and the box [`far_box`] looks at from `height`.
fn far_box_scene(settings: RenderSettings, height: f32) -> Harness {
    let mut harness = Harness::new(settings);
    harness.ground();
    let scale = height * 0.1;
    harness.cuboid(Vec3::ZERO, Vec3::new(scale, 3.0 * scale, scale));
    harness.sun_toward(toward(45.0, 0.0));
    harness
}

#[test]
#[ignore = "needs a GPU"]
fn every_cascade_casts_and_shadows_end_at_their_distance() {
    // Seen from above at a growing distance, the box's shadow falls in each cascade in turn: it
    // holds in every one out to the shadow distance (fading over its last tenth), and is gone
    // past it.
    for (name, settings, reach, heights) in [
        ("desktop", desktop(), 120.0, [3.0, 9.0, 22.0, 60.0, 100.0, 135.0]),
        ("mobile", mobile(), 40.0, [3.0, 9.0, 22.0, 30.0, 34.0, 48.0]),
    ] {
        for height in heights {
            let mut harness = far_box_scene(settings, height);
            let (frame, shadow, beside) = far_box(&mut harness, height);
            frame.dump(&format!("cascades-{name}-{height}"));
            assert!(frame.lit_at(beside), "{name} at {height} m: lit beside the shadow");
            if height < reach * 0.9 {
                assert!(frame.shaded_at(shadow), "{name} at {height} m: {:?}", frame.at(shadow));
            } else if height > reach {
                assert!(frame.lit_at(shadow), "{name} at {height} m: past the shadow distance");
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn cached_cascades_take_turns_and_keep_the_shadows_still() {
    // A still scene: after the first frame the far two cascades are drawn in turns, and every
    // frame is the same, cached cascades sampled with the matrices they were drawn with.
    let height = 60.0;
    let mut harness = far_box_scene(desktop(), height);
    let (first, shadow, _) = far_box(&mut harness, height);
    assert!(first.shaded_at(shadow));
    let drawn: Vec<u32> =
        (0..5).map(|_| far_box(&mut harness, height).0.stats.shadows.cascades_drawn).collect();
    assert_eq!(first.stats.shadows.cascades_drawn, 4);
    assert_eq!(drawn, [3; 5], "two fresh cascades and one cached a frame");
    let (last, ..) = far_box(&mut harness, height);
    assert_eq!(last.pixels, first.pixels, "the cached cascades draw the same shadows");
    assert_eq!(last.stats.shadows.cascades_cached, 1);
    let stats = last.stats.shadows;
    assert!(stats.caster_draws > 0 && stats.culled_draws < stats.caster_draws, "{stats:?}");
    // The camera drifting across the far shadow, by up to a few texels a frame: the shadow holds
    // every frame, whether its cascade was drawn this frame or cached.
    for step in 1..=8 {
        let center = Vec3::new(-6.0 + 0.4 * step as f32, 0.0, 0.3 * step as f32);
        harness.camera = top_down(center, height);
        let frame = harness.render();
        assert!(frame.shaded_at(shadow), "step {step}: {:?}", frame.at(shadow));
    }
}

/// A sphere of radius 0.5 at (0, 1, 0) under a slab above it, lit from above and in front, seen
/// from the front: the slab's shadow falls on the sphere's top, where `probe` lies.
struct Canopy {
    harness: Harness,
    sphere: InstanceId,
    slab: InstanceId,
    probe: Vec3,
}

fn canopy(settings: RenderSettings) -> Canopy {
    let mut harness = Harness::new(settings);
    harness.ground();
    let sphere = harness.add(
        &sphere(),
        &surface(0.0),
        Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, Vec3::Y),
    );
    let slab = harness.cuboid(Vec3::new(0.0, 1.8, 0.0), Vec3::new(1.6, 0.1, 1.6));
    harness.sun_toward(Vec3::new(0.0, 1.0, 0.3).normalize());
    harness.camera = looking(Vec3::new(0.0, 1.3, 4.0), Vec3::new(0.0, 1.1, 0.0));
    let probe = Vec3::Y + Vec3::new(0.0, 0.8, 0.6) * 0.5;
    Canopy { harness, sphere, slab, probe }
}

#[test]
#[ignore = "needs a GPU"]
fn characters_never_shadow_themselves_but_the_environment_shadows_them() {
    let body = CharacterBody::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.85, 0.0), 0.8);
    // Environment both: the slab shadows the sphere.
    let mut scene = canopy(desktop());
    let frame = scene.harness.render();
    frame.dump("self-environment");
    assert!(frame.shaded_at(scene.probe), "environment shadows environment: {:?}", frame.at(scene.probe));
    // One character, sphere and slab: it never shadows itself.
    let mut scene = canopy(desktop());
    let (sphere, slab) = (scene.sphere, scene.slab);
    scene.harness.character(sphere, body, None);
    scene.harness.character(slab, body, None);
    let frame = scene.harness.render();
    frame.dump("self-character");
    assert!(frame.lit_at(scene.probe), "a character never shadows itself: {:?}", frame.at(scene.probe));
    assert_eq!(frame.stats.shadows.character_maps, 1, "one body, one map");
    // A character under an environment slab: shadowed.
    let mut scene = canopy(desktop());
    let sphere = scene.sphere;
    scene.harness.character(
        sphere,
        CharacterBody::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.5),
        None,
    );
    let frame = scene.harness.render();
    frame.dump("self-under-environment");
    assert!(frame.shaded_at(scene.probe), "the environment shadows a character: {:?}", frame.at(scene.probe));
    // An environment sphere under a character's slab: shadowed by the character's own map
    // (desktop) and by its capsule (phones).
    for (name, settings) in [("desktop", desktop()), ("mobile", mobile())] {
        let mut scene = canopy(settings);
        let slab = scene.slab;
        // The slab's capsule: as wide as the slab, through its middle.
        let body = CharacterBody::capsule(Vec3::new(-0.3, 1.85, 0.0), Vec3::new(0.3, 1.85, 0.0), 0.8);
        scene.harness.character(slab, body, None);
        let frame = scene.harness.render();
        frame.dump(&format!("self-under-character-{name}"));
        assert!(
            frame.shaded_at(scene.probe),
            "{name}: a character shadows the environment: {:?}",
            frame.at(scene.probe)
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn characters_cast_their_own_shadow_on_the_ground() {
    // A character's sphere floating 2 m up, lit from 53° above +X: its shadow lies 1.5 m toward −X
    // on the ground, from its own map on desktop and its capsule on phones. A character without a
    // body casts nothing: characters never cast into the cascades.
    let at = Vec3::new(0.0, 2.0, 0.0);
    let shadow = Vec3::new(-1.5, 0.0, 0.0);
    for (name, settings, maps, capsules) in [("desktop", desktop(), 1, 0), ("mobile", mobile(), 0, 1)] {
        for bodied in [true, false] {
            let mut harness = Harness::new(settings);
            harness.ground();
            let sphere = harness.add(
                &sphere(),
                &surface(0.0),
                Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, at),
            );
            let body = if bodied { CharacterBody::capsule(at, at, 0.5) } else { CharacterBody::default() };
            harness.character(sphere, body, None);
            harness.sun_toward(Vec3::new(0.6, 0.8, 0.0));
            harness.camera = top_down(Vec3::new(-1.0, 0.0, 0.0), 6.0);
            let frame = harness.render();
            frame.dump(&format!("ground-{name}-{bodied}"));
            if bodied {
                assert!(frame.shaded_at(shadow), "{name}: its shadow: {:?}", frame.at(shadow));
                assert_eq!(
                    (frame.stats.shadows.character_maps, frame.stats.shadows.capsules),
                    (maps, capsules)
                );
            } else {
                assert!(frame.lit_at(shadow), "{name}: no body, no shadow of its own");
            }
            for outside in [shadow + Vec3::new(0.0, 0.0, 0.8), shadow + Vec3::new(-0.9, 0.0, 0.0)] {
                assert!(frame.lit_at(outside), "{name}: lit around its shadow at {outside}");
            }
            // The sphere's top, toward the light, is lit: no shadow of its own.
            assert!(frame.lit_at(at + Vec3::new(0.3, 0.4, 0.0)), "{name}: never shadowed by itself");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn lit_outlines_go_into_a_cast_shadow_with_their_surface() {
    // The canopy's sphere with a lit green line (M1 C4: the key light through the split and the
    // shadow tone), as a character: the slab's shadow over its top takes the line on its top into
    // the shadow tone with the surface, where with shadows off it stays lit. Line pixels are
    // the ones without red or blue: the line is pure green × its tone, the surfaces grey or blue
    // (the line's edge pixels blend with the black sky, so tones are compared as a ratio).
    let line = |settings: RenderSettings, name: &str| {
        let mut scene = canopy(settings);
        let lined = AnimeCharacterMaterial {
            regions: vec![CharacterRegion {
                shade_color: SHADE,
                outline_color: [0.0, 1.0, 0.0],
                ..CharacterRegion::default()
            }],
            outline: Outline { width: 6.0, z_offset: 0.0, albedo: 0.0, lighting: 1.0, ..Outline::default() },
            ..surface(0.0)
        };
        let place = Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, Vec3::Y);
        let sphere = scene.harness.add(&sphere(), &lined, place);
        scene.harness.removed.push(scene.sphere);
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.5);
        scene.harness.character(sphere, body, None);
        let frame = scene.harness.render();
        frame.dump(&format!("outline-{name}"));
        // The mean green of the line's pixels in the sphere's upper half, under the slab.
        let top = frame.pixel_of(Vec3::new(0.0, 1.5, 0.0)).expect("in view").1;
        let middle = frame.pixel_of(Vec3::Y).expect("in view").1;
        let (mut sum, mut count) = (0.0, 0);
        for y in top.saturating_sub(12)..middle {
            for x in 0..SIZE as usize {
                let [r, g, b] = frame.linear(x, y);
                if g > 0.02 && r < 0.01 && b < 0.01 {
                    sum += g;
                    count += 1;
                }
            }
        }
        assert!(count > 20, "the line's pixels: {count}");
        sum / count as f32
    };
    let (lit, shadowed) = (line(RenderSettings::default(), "off"), line(desktop(), "on"));
    let ratio = shadowed / lit;
    assert!((ratio - SHADE[1]).abs() < 0.08, "the shade tone's share of the lit line: {shadowed} / {lit}");
}

#[test]
#[ignore = "needs a GPU"]
fn a_face_takes_the_environments_shadow_whole() {
    // An upright square facing the camera, lit from the front by its rig, and a wall in front of
    // it whose shadow's edge (the light from the front and above) runs down the square's middle.
    // Drawn as a body, the square splits along that edge, per pixel. Drawn as a face, it takes
    // the shadow whole: one tone over the whole face, the shade where the wall covers its head,
    // the lit tone where it doesn't, never the edge across it.
    let wall = |harness: &mut Harness, edge: f32| {
        let width = 4.0;
        harness.cuboid(Vec3::new(edge - width * 0.5, -1.0, 3.0), Vec3::new(width, 4.0, 0.2))
    };
    let mut tones = Vec::new();
    for edge in [-1.5, 0.0, 1.5] {
        for face in [false, true] {
            let mut harness = Harness::new(desktop());
            harness.sun_toward(Vec3::new(0.0, 0.25, 1.0).normalize());
            let map = harness.lit_face_map();
            let document = if face { face_surface() } else { surface(0.0) };
            let material = harness.material(&document, &[(SDF, &map)]);
            let square = harness.mesh(&upright_square(), material);
            let id = harness.add_mesh(square, Mat4::from_scale(Vec3::splat(0.5)));
            // The rig lights it from the front; its head (the face pass's probe) is its center.
            harness.character(id, CharacterBody::default(), Some(Vec3::ZERO));
            let mut lighting =
                CharacterLighting::unstyled(DirectionalLight { direction: Vec3::NEG_Z, ..harness.sun });
            lighting.gi.head = Some(Vec3::ZERO);
            harness.characters[0].lighting = lighting;
            wall(&mut harness, edge);
            harness.camera = looking(Vec3::new(0.0, 0.0, 2.0), Vec3::ZERO);
            let frame = harness.render();
            frame.dump(&format!("face-{edge}-{face}"));
            let left = frame.at(Vec3::new(-0.3, 0.0, 0.0));
            let right = frame.at(Vec3::new(0.3, 0.0, 0.0));
            tones.push((edge, face, left, right));
            if face {
                let whole = (0..3).all(|c| (left[c] - right[c]).abs() < 0.01);
                assert!(whole, "edge at {edge}: the face takes the shadow whole: {left:?} vs {right:?}");
            }
        }
    }
    let tone = |edge: f32, face: bool| {
        tones.iter().find(|t| t.0 == edge && t.1 == face).map(|t| (t.2, t.3)).expect("rendered")
    };
    // The body splits down the middle where the edge crosses it.
    let (left, right) = tone(0.0, false);
    assert!(is_shaded(left) && is_lit(right), "the body splits: {left:?} | {right:?}");
    // The face is lit with its head in the light, shaded with it covered.
    assert!(is_lit(tone(-1.5, true).0), "uncovered: lit");
    assert!(is_shaded(tone(1.5, true).0), "covered: shade");
}

#[test]
#[ignore = "needs a GPU"]
fn shadows_with_nothing_casting_leave_the_frame_as_without_them() {
    // Receivers return exactly 1 where nothing occludes them, so a lit scene draws the same
    // tones with shadows on as with them off: no pixel turns to shade or back. The shaders with
    // shadows are other code, so a split's edge pixel may round a few steps differently.
    let frame = |settings: RenderSettings| {
        let mut harness = Harness::new(settings);
        harness.ground();
        let sphere = harness.add(&sphere(), &surface(0.0), Mat4::from_translation(Vec3::new(1.5, 1.0, -2.0)));
        harness.character(sphere, CharacterBody::default(), None);
        harness.sun_toward(toward(60.0, 30.0));
        harness.camera = looking(Vec3::new(0.0, 2.0, 4.0), Vec3::new(0.5, 0.5, -2.0));
        harness.render()
    };
    let off = frame(RenderSettings::default());
    let on = frame(desktop());
    assert!(!off.stats.shadows.enabled && on.stats.shadows.enabled);
    let (mut differing, mut turned) = (0, 0);
    for y in 0..SIZE as usize {
        for x in 0..SIZE as usize {
            let index = y * SIZE as usize + x;
            let (a, b) = (off.pixels[index], on.pixels[index]);
            differing += usize::from(a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 1));
            let (a, b) = (off.linear(x, y), on.linear(x, y));
            turned += usize::from((is_lit(a) && is_shaded(b)) || (is_shaded(a) && is_lit(b)));
        }
    }
    assert_eq!(turned, 0, "{turned} pixels changed tone");
    assert!(differing <= 4, "{differing} pixels differ by more than a step");
}

#[test]
#[ignore = "needs a GPU"]
fn environment_surfaces_take_every_shadow_tinted() {
    // The environment shader's ground (W4's `cast_shadow` hook) under a box, an environment
    // caster, and a character sphere, which casts its own map on desktop and its capsule on
    // phones: both shadows fall on it, and in them the key light keeps the frame's shadow tint,
    // never black. The tint colors them as time of day sets it.
    let (center, height) = (Vec3::new(-1.0, 0.0, 1.5), 8.0);
    let n_dot_l = toward(45.0, 0.0).y;
    for (name, settings) in [("desktop", desktop()), ("mobile", mobile())] {
        let mut harness = Harness::new(settings);
        let place = Mat4::from_scale(Vec3::new(100.0, 1.0, 100.0));
        harness.add(&ground(), &environment(), place);
        let size = Vec3::new(1.0, 2.0, 1.0);
        harness.add(
            &cuboid(),
            &environment(),
            Mat4::from_scale_rotation_translation(size, Quat::IDENTITY, Vec3::ZERO),
        );
        let at = Vec3::new(0.0, 1.0, 3.0);
        let sphere = harness.add(
            &sphere(),
            &surface(0.0),
            Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, at),
        );
        harness.character(sphere, CharacterBody::capsule(at, at, 0.5), None);
        harness.sun_toward(toward(45.0, 0.0));
        harness.camera = top_down(center, height);
        for tint in [Vec3::new(0.4, 0.45, 0.65), Vec3::new(0.6, 0.35, 0.3)] {
            harness.shadow_tint = tint;
            let frame = harness.render();
            frame.dump(&format!("environment-{name}-{:.1}", tint.x));
            let shadowed = (tint * ALBEDO).to_array();
            let lit = ((tint + (Vec3::ONE - tint) * n_dot_l) * ALBEDO).to_array();
            let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 0.03);
            // The box's shadow, 2 m toward −X; the sphere's, 1 m toward −X of it (a 45° light).
            for point in [Vec3::new(-1.5, 0.0, 0.0), Vec3::new(-1.0, 0.0, 3.0)] {
                let rgb = frame.at(point);
                assert!(near(rgb, shadowed), "{name}: {point} shows {rgb:?}, not the tinted {shadowed:?}");
            }
            for point in [Vec3::new(-1.5, 0.0, 1.5), Vec3::new(1.5, 0.0, 0.0)] {
                let rgb = frame.at(point);
                assert!(near(rgb, lit), "{name}: {point} shows {rgb:?}, not lit {lit:?}");
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn shadows_hold_still_under_temporal_aa() {
    // A shadowed scene under temporal AA, held still while it converges: the jittered frames
    // sample the same smooth shadow field, so once converged, a frame barely differs from the
    // last.
    let settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..desktop() };
    let mut harness = Harness::new(settings);
    harness.ground();
    harness.cuboid(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.8, 2.5, 0.8));
    harness.cuboid(Vec3::new(-2.0, 0.0, -6.0), Vec3::new(1.0, 4.0, 1.0));
    let sphere = harness.add(
        &sphere(),
        &surface(0.0),
        Mat4::from_scale_rotation_translation(Vec3::splat(0.5), Quat::IDENTITY, Vec3::new(1.2, 0.5, 0.0)),
    );
    harness.character(
        sphere,
        CharacterBody::capsule(Vec3::new(1.2, 0.5, 0.0), Vec3::new(1.2, 0.5, 0.0), 0.5),
        None,
    );
    harness.sun_toward(toward(35.0, 40.0));
    harness.camera = looking(Vec3::new(1.0, 2.5, 5.0), Vec3::new(0.0, 0.5, -2.0));
    let mut frames: Vec<Vec<[u8; 4]>> = Vec::new();
    for _ in 0..48 {
        frames.push(harness.render().pixels);
    }
    let (a, b) = (&frames[46], &frames[47]);
    let steps: Vec<u32> =
        a.iter().zip(b).flat_map(|(a, b)| (0..3).map(move |c| u32::from(a[c].abs_diff(b[c])))).collect();
    let mean = steps.iter().sum::<u32>() as f32 / steps.len() as f32;
    let moving = a.iter().zip(b).filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > 2)).count();
    eprintln!("converged frame-to-frame change: mean {mean:.4} steps, {moving} pixels over 2");
    assert!(mean < 0.06, "mean change {mean} steps");
    assert!(moving < (SIZE * SIZE / 400) as usize, "{moving} pixels change by more than 2 steps");
}

#[test]
#[ignore = "needs a GPU"]
fn an_edited_shadows_wgsl_rebuilds_the_shadow_passes() {
    // Hot reload (M1 A8): `shadows.wgsl` is a program of its own, whose edits rebuild the face
    // pass and every caster built so far on the layouts they had; `frame.wgsl`, which holds the
    // receivers, rebuilds them too. A comment draws the same frame; an edit the pipelines refuse
    // keeps the ones the renderer has.
    let (center, height) = (Vec3::new(-1.45, 0.0, 0.0), 4.0);
    let mut harness = box_scene(desktop(), center, height);
    let character = harness.add(&sphere(), &surface(0.0), Mat4::from_translation(Vec3::new(-3.0, 1.0, 0.0)));
    harness.character(
        character,
        CharacterBody::capsule(Vec3::new(-3.0, 1.0, 0.0), Vec3::new(-3.0, 1.0, 0.0), 1.0),
        Some(Vec3::new(-3.0, 1.5, 0.0)),
    );
    let before = harness.render();
    let comment = |sources: &ShaderSources, file: ShaderFile| {
        let mut edited = sources.clone();
        edited.set(file, format!("{}\n// edited\n", sources.get(file)));
        edited
    };
    let shadows = comment(&ShaderSources::compiled_in(), ShaderFile::Shadows);
    let update = harness.renderer.set_shader_sources(&shadows).expect("a valid edit");
    assert_eq!(
        update,
        ShaderUpdate { programs: 1, pipelines: 2 },
        "the face pass and the quantized meshes' caster"
    );
    assert_eq!(harness.render().pixels, before.pixels, "a comment draws the same frame");
    // The receivers live in frame.wgsl: every program with it, the shadow passes', foliage's (M1
    // W6), the ambient passes' (M1 W5) and the debug views' (M1 F5) included.
    let frame = comment(&shadows, ShaderFile::Frame);
    let update = harness.renderer.set_shader_sources(&frame).expect("a valid edit");
    assert_eq!(update.programs, 11);
    assert_eq!(harness.render().pixels, before.pixels);
    // A renamed entry point validates but no pipeline can build from it: refused, and the
    // shadows go on drawing with what the renderer has.
    let mut renamed = frame.clone();
    renamed
        .set(ShaderFile::Shadows, frame.get(ShaderFile::Shadows).replace("fn cs_face_shadow", "fn cs_faces"));
    let error = harness.renderer.set_shader_sources(&renamed).expect_err("no entry point");
    assert!(matches!(error, ShaderError::Pipeline { .. }), "{error}");
    assert_eq!(harness.render().pixels, before.pixels);
}
