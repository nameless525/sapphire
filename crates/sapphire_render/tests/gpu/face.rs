//! Face tests that need a GPU (M1 C3): the face permutation of the anime character shader, its
//! SDF light split in head space with both channels and the mirror, its masks, and the heads and
//! expressions characters carry beside their light rigs. Run with
//! `cargo test -p sapphire_render --test gpu face:: -- --ignored`.
//!
//! Scenes are lit by white key lights and no ambient light, with no rim and no outline, so every
//! pixel is the lit albedo or the authored shadow tone. Instances not listed as characters take
//! the world's sun and a head at rest; listed ones their own key light and head. The face maps
//! are analytic, written as half floats at texel centers, so filtering is exact and each shadow
//! edge can be predicted to the pixel from `sapphire_assets::character_maps::face_sdf`.

use std::f32::consts::PI;

use glam::{Mat4, Quat, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::face_sdf::{self, HeadLight};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, CharacterSurface, DiffuseMode, FaceShading, LightSplit,
    Material, MaterialDocument, Outline, RampDiffuse, Rim,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, FaceExpression, FramePacketRing, HeadBasis, InstanceId,
    MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, Renderer, SkyGradient,
    ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels: the unit square fills it.
const SIDE: u32 = 128;
/// Linear albedo of the test faces, and their shadow tone's multiplier.
const ALBEDO: f32 = 0.8;
const SHADE: f32 = 0.25;
const LIT: [f32; 3] = [ALBEDO; 3];
const SHADOW: [f32; 3] = [ALBEDO * SHADE; 3];
/// How far a pixel's linear color may be from the prediction: 8-bit output.
const TOLERANCE: f32 = 0.02;
/// How far, in pixels, an edge may be from the prediction.
const EDGE_TOLERANCE: f32 = 1.5;

/// The texture IDs materials name; each test binds its own textures to them.
const SDF: &str = "4a8f2c1e-7b3d-4e9a-8c5f-1d2e3f4a5b6c";
const MASK: &str = "9c1d2e3f-4a5b-4c6d-8e7f-0a1b2c3d4e5f";
const EXPRESSION: &str = "2b3c4d5e-6f70-4182-9384-a5b6c7d8e9f0";
const RAMP: &str = "5e6f7a8b-9c0d-4e1f-a2b3-c4d5e6f7a8b9";

fn id(text: &str) -> AssetId {
    text.parse().expect("a valid asset ID")
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

fn assert_near(actual: [f32; 3], expected: [f32; 3], what: &str) {
    let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < TOLERANCE);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

fn mul(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] * b[i])
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The unit vector toward a light at `yaw` degrees from head-forward toward the head's right
/// (negative: its left) and `elevation` degrees up, for a head at rest: facing +Z with +Y up,
/// whose right is -X.
fn toward(yaw: f32, elevation: f32) -> Vec3 {
    let (yaw, elevation) = (yaw.to_radians(), elevation.to_radians());
    Vec3::new(-yaw.sin() * elevation.cos(), elevation.sin(), yaw.cos() * elevation.cos())
}

/// A face material reading the SDF map, band mode with one shade, no rim or outline, and a
/// one-pixel shadow edge.
fn face_material(face: FaceShading) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface: CharacterSurface::Face(face),
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion {
            shade_color: [SHADE; 3],
            outline_color: [0.9, 0.1, 0.2],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// Face settings over the SDF map with nothing else: no edge softness and no highlight.
fn plain_face() -> FaceShading {
    FaceShading { softness: 0.0, highlight: 0.0, ..FaceShading::new(id(SDF)) }
}

/// One rendered frame, as linear RGB per pixel.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
}

impl Frame {
    fn linear(&self, x: usize, y: usize) -> [f32; 3] {
        let pixel = self.pixels[y * SIDE as usize + x];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (usize, usize) {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let side = SIDE as f32;
        (((x + 1.0) * 0.5 * side) as usize, ((1.0 - y) * 0.5 * side) as usize)
    }

    /// The linear color where `point` projects.
    fn at(&self, point: Vec3) -> [f32; 3] {
        let (x, y) = self.pixel_of(point);
        self.linear(x, y)
    }

    /// The color at texture coordinate `(u, v)` of an unrotated unit square filling the frame.
    fn at_uv(&self, u: f32, v: f32) -> [f32; 3] {
        let side = SIDE as f32;
        self.linear((u * side) as usize, (v * side) as usize)
    }

    /// Where red changes most steeply between pixels `from` and `to` of the row (`horizontal`)
    /// or column through the middle: the shadow edge, in pixels from the left or top.
    fn edge(&self, horizontal: bool, from: usize, to: usize) -> f32 {
        let middle = SIDE as usize / 2;
        let red = |i: usize| if horizontal { self.linear(i, middle)[0] } else { self.linear(middle, i)[0] };
        let (_, at) = (from..to - 2)
            .map(|i| ((red(i + 2) - red(i)).abs(), i + 1))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .expect("pixels");
        at as f32 + 0.5
    }

    /// Whether every pixel of the row (`horizontal`) or column through the middle, between
    /// `from` and `to`, is `color`.
    fn all(&self, horizontal: bool, from: usize, to: usize, color: [f32; 3]) -> bool {
        let middle = SIDE as usize / 2;
        (from..to).all(|i| {
            let pixel = if horizontal { self.linear(i, middle) } else { self.linear(middle, i) };
            pixel.iter().zip(color).all(|(a, e)| (a - e).abs() < TOLERANCE)
        })
    }
}

/// A renderer and everything a test frame needs.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    sun: DirectionalLight,
    ambient: AmbientLight,
    added: Vec<MeshInstance>,
    /// Listed every frame, as an app lists its characters.
    characters: Vec<CharacterInstance>,
    next_id: u64,
}

impl Harness {
    /// A square target, seen by a camera on +Z that frames the unit square at the origin edge to
    /// edge, lit by a white sun from the camera.
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let settings = RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() };
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
            camera: CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.05),
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            added: Vec::new(),
            characters: Vec::new(),
            next_id: 0,
        };
        harness.view_from(0.0);
        harness
    }

    /// Frames the unit square from `yaw` degrees around it, toward the head's right (-X), at the
    /// distance that fits it edge to edge from the front.
    fn view_from(&mut self, yaw: f32) {
        let fov = 38f32.to_radians();
        let distance = 1.0 / (fov * 0.5).tan();
        let eye = Quat::from_rotation_y(-yaw.to_radians()) * Vec3::new(0.0, 0.0, distance);
        self.camera = CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, fov, 1.0, 0.05);
    }

    /// Lights the scene from `toward_sun`, a vector from the surface toward the sun.
    fn sun_from(&mut self, toward_sun: Vec3) {
        self.sun.direction = -toward_sun;
    }

    /// An RGBA16 float texture of linear data, `size` texels a side, per `texel(u, v)` at each
    /// texel's center, read through `swizzle`.
    fn half_texture(&self, size: u32, swizzle: Swizzle, texel: impl Fn(f32, f32) -> [f32; 4]) -> GpuTexture {
        let center = |i: u32| (i as f32 + 0.5) / size as f32;
        let texels: Vec<[u16; 4]> = (0..size)
            .flat_map(|y| (0..size).map(move |x| (x, y)))
            .map(|(x, y)| texel(center(x), center(y)).map(|c| f16::from_f32(c).to_bits()))
            .collect();
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width: size,
            height: size,
            mip_level_count: 1,
            swizzle,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        GpuTexture::upload(&self.gpu, &Texture { desc, levels: vec![bytes] }, "half").expect("texture")
    }

    /// An SDF face map holding `horizontal(u)` in R and `vertical(v)` in G.
    fn sdf(&self, horizontal: fn(f32) -> f32, vertical: fn(f32) -> f32) -> GpuTexture {
        self.half_texture(256, Swizzle::IDENTITY, |u, v| [horizontal(u), vertical(v), 0.0, 1.0])
    }

    /// Cooks `document` against the textures it names, bound by ID, and uploads it.
    fn material(
        &mut self,
        document: &AnimeCharacterMaterial,
        textures: &[(&str, &GpuTexture)],
    ) -> MaterialHandle {
        let bytes = Self::cook(document, textures);
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    /// Replaces `handle` with `document`.
    fn update(
        &mut self,
        handle: MaterialHandle,
        document: &AnimeCharacterMaterial,
        textures: &[(&str, &GpuTexture)],
    ) {
        let bytes = Self::cook(document, textures);
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.update_material(handle, Material::read(&bytes).expect("read"), lookup).expect("update");
    }

    fn cook(document: &AnimeCharacterMaterial, textures: &[(&str, &GpuTexture)]) -> Vec<u8> {
        let swizzles: Vec<Swizzle> = document
            .textures()
            .iter()
            .map(|slot| {
                slot.and_then(|asked| textures.iter().find(|(name, _)| id(name) == asked))
                    .map_or(Swizzle::IDENTITY, |(_, texture)| texture.swizzle())
            })
            .collect();
        Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec()
    }

    /// Uploads `mesh` (one material slot) drawn with `material`.
    fn mesh(&mut self, mesh: &Mesh, material: MaterialHandle) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
    }

    /// Adds an instance of `mesh` with the next frame.
    fn add(&mut self, mesh: MeshHandle, transform: Mat4) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        id
    }

    /// Lists instance `id` as a character from the next frame on, in place of its last listing:
    /// lit by a white key light from `toward_key` (a vector toward it) with unstyled GI, with
    /// `head` and `expression`.
    fn character(&mut self, id: InstanceId, toward_key: Vec3, head: HeadBasis, expression: FaceExpression) {
        let key = DirectionalLight { direction: -toward_key, color: Vec3::ONE, intensity: 1.0 };
        let lighting = CharacterLighting::unstyled(key);
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance {
            id,
            lighting,
            face: CharacterFace { head, expression },
            body: CharacterBody::default(),
            opacity: 1.0,
        });
    }

    /// Prepares, records and submits one frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = self.sun;
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient = self.ambient;
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        packet.characters.extend_from_slice(&self.characters);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back(), camera: self.camera }
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
}

/// The unit square at the origin facing +Z, `u` from left to right (toward +X, the head's left)
/// and `v` from top to bottom, as a front-projected face is mapped.
fn square() -> Mesh {
    let vertices: Vec<Vertex> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .map(|(x, y)| Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [(x + 1.0) * 0.5, (1.0 - y) * 0.5],
            color: [255; 3],
            flags: VertexFlags::NONE,
        })
        .collect();
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        vec![0, 1, 2, 0, 2, 3],
        submeshes,
        vec![MaterialSlot::embedded("face", placeholder)],
        PositionFormat::Unorm16,
    )
}

/// A face map's horizontal channel lighting the head's right side longest: `1 - u`, so a light
/// at yaw θ from the right puts the edge at `u = 1 - θ/180°`.
fn right_lit_longest(u: f32) -> f32 {
    1.0 - u
}

/// A vertical channel lighting the top longest: `1 - v`, so a light rising to elevation e puts
/// the edge at `v = 1 - e/180°`.
fn top_lit_longest(v: f32) -> f32 {
    1.0 - v
}

/// A channel that never shadows.
fn never(_: f32) -> f32 {
    1.0
}

/// Pixels from an edge of the frame that filtering wraps across (the sampler repeats) and the
/// edge detector skips.
const MARGIN: usize = 3;

/// A face square drawn with `document` over `textures`, its instance at rest.
fn face_square(
    harness: &mut Harness,
    document: &AnimeCharacterMaterial,
    textures: &[(&str, &GpuTexture)],
) -> InstanceId {
    let material = harness.material(document, textures);
    let mesh = harness.mesh(&square(), material);
    harness.add(mesh, Mat4::IDENTITY)
}

#[test]
#[ignore = "needs a GPU"]
fn the_face_permutation_draws_faces_and_only_faces() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, never);
    let textures = [(SDF, &sdf)];
    let stats = |harness: &Harness| harness.renderer.stats();
    let body = AnimeCharacterMaterial { surface: CharacterSurface::Body, ..face_material(plain_face()) };
    let face = harness.material(&face_material(plain_face()), &textures);
    assert_eq!(stats(&harness).character_pipelines, 2, "a material alone builds nothing");
    let mesh = harness.mesh(&square(), face);
    harness.add(mesh, Mat4::IDENTITY);
    let body = harness.material(&body, &textures);
    let body_mesh = harness.mesh(&square(), body);
    harness.add(body_mesh, Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)));
    harness.render();
    let s = stats(&harness);
    // The face's surface pipeline; its outline is the opaque body's.
    assert_eq!(s.character_pipelines, 3);
    assert_eq!((s.draws, s.face_draws), (2, 1));
    let builds = s.draw_list_builds;

    // A body material turned into a face moves its draw to the face variant: one rebuild.
    harness.update(body, &face_material(plain_face()), &textures);
    harness.render();
    let s = stats(&harness);
    assert_eq!((s.draws, s.face_draws, s.draw_list_builds), (2, 2, builds + 1));
    assert_eq!(s.character_pipelines, 3, "built already");
}

#[test]
#[ignore = "needs a GPU"]
fn the_horizontal_channel_puts_the_edge_where_the_light_angle_predicts_on_both_sides() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, never);
    face_square(&mut harness, &face_material(plain_face()), &[(SDF, &sdf)]);
    let side = SIDE as f32;
    let (from, to) = (MARGIN, SIDE as usize - MARGIN);
    for yaw in [30.0, 60.0, 90.0, 120.0, 150.0] {
        // From the head's right: lit from the left of the image (the head's right) to the edge.
        harness.sun_from(toward(yaw, 0.0));
        let frame = harness.render();
        let expected = (1.0 - yaw / 180.0) * side;
        let edge = frame.edge(true, from, to);
        assert!(
            (edge - expected).abs() < EDGE_TOLERANCE,
            "{yaw}° from the right: edge {edge}, expected {expected}"
        );
        assert!(frame.all(true, from, expected as usize - 2, LIT), "{yaw}°: lit before the edge");
        assert!(frame.all(true, expected as usize + 2, to, SHADOW), "{yaw}°: shadowed after it");

        // From the head's left the map is mirrored: the same edge from the other side.
        harness.sun_from(toward(-yaw, 0.0));
        let frame = harness.render();
        let expected = yaw / 180.0 * side;
        let edge = frame.edge(true, from, to);
        assert!(
            (edge - expected).abs() < EDGE_TOLERANCE,
            "{yaw}° from the left: edge {edge}, expected {expected}"
        );
        assert!(frame.all(true, expected as usize + 2, to, LIT), "{yaw}° mirrored: lit after the edge");
        assert!(frame.all(true, from, expected as usize - 2, SHADOW), "{yaw}° mirrored: shadowed before it");
    }
    // Straight ahead the whole face is lit; straight behind, none of it.
    harness.sun_from(toward(0.0, 0.0));
    assert!(harness.render().all(true, from, to, LIT));
    harness.sun_from(toward(180.0, 0.0));
    assert!(harness.render().all(true, from, to, SHADOW));
}

#[test]
#[ignore = "needs a GPU"]
fn the_vertical_channel_shadows_from_above_and_intersects_the_horizontal() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, top_lit_longest);
    face_square(&mut harness, &face_material(plain_face()), &[(SDF, &sdf)]);
    let side = SIDE as f32;
    let (from, to) = (MARGIN, SIDE as usize - MARGIN);
    for elevation in [30.0, 60.0, 90.0] {
        // Rising straight in front: the top stays lit longest; the horizontal channel lights
        // everything.
        harness.sun_from(toward(0.0, elevation));
        let frame = harness.render();
        let expected = (1.0 - elevation / 180.0) * side;
        let edge = frame.edge(false, from, to);
        assert!(
            (edge - expected).abs() < EDGE_TOLERANCE,
            "{elevation}°: edge row {edge}, expected {expected}"
        );
        assert!(frame.all(false, from, expected as usize - 2, LIT), "{elevation}°: lit above");
        assert!(frame.all(false, expected as usize + 2, to, SHADOW), "{elevation}°: shadowed below");
    }

    // From the right and above: lit where both channels light, as the reference predicts.
    let light = toward(90.0, 45.0);
    harness.sun_from(light);
    let frame = harness.render();
    let head = HeadLight::new([-light.x, light.y, light.z]);
    for (u, v) in [(0.2, 0.2), (0.5, 0.5), (0.8, 0.2), (0.2, 0.9), (0.8, 0.9), (0.55, 0.65), (0.7, 0.7)] {
        let margin = face_sdf::lit_margin(head, 1.0 - u, u, 1.0 - v);
        let expected = if margin > 0.0 { LIT } else { SHADOW };
        assert_near(frame.at_uv(u, v), expected, &format!("({u}, {v}), margin {margin}"));
    }
    // Behind and above, the faded horizontal angle (115°) would light the light's side alone;
    // near the head's forward–up plane both sides are read and neither is lit, from either side.
    for yaw in [179.5, 180.5] {
        harness.sun_from(toward(yaw, 50.0));
        assert!(harness.render().all(true, from, to, SHADOW), "behind and above, {yaw}°");
    }
    // Over the top, neither side wins: a light just left or right of overhead lights the same.
    harness.sun_from(toward(90.0, 89.0));
    let right = harness.render();
    harness.sun_from(toward(-90.0, 89.0));
    let left = harness.render();
    for u in [0.1, 0.3, 0.5, 0.7, 0.9] {
        for v in [0.1, 0.45, 0.8] {
            assert_near(left.at_uv(u, v), right.at_uv(u, v), &format!("over the top at ({u}, {v})"));
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_one_channel_map_never_shadows_by_the_vertical_channel() {
    let mut harness = Harness::new();
    // Cooked as A2 cooks one-channel SDF maps: R8, its G read as 1 through the swizzle.
    let sdf = sdf_r8(&harness, |u| 1.0 - u);
    face_square(&mut harness, &face_material(plain_face()), &[(SDF, &sdf)]);
    harness.sun_from(toward(0.0, 80.0));
    let frame = harness.render();
    assert!(frame.all(false, MARGIN, SIDE as usize - MARGIN, LIT), "a top light shadows nothing");
    harness.sun_from(toward(60.0, 0.0));
    let edge = harness.render().edge(true, MARGIN, SIDE as usize - MARGIN);
    let expected = (1.0 - 60.0 / 180.0) * SIDE as f32;
    assert!((edge - expected).abs() < EDGE_TOLERANCE, "the horizontal channel still splits: {edge}");
}

/// A one-channel R8 SDF map with the swizzle A2's cook gives it.
fn sdf_r8(harness: &Harness, horizontal: fn(f32) -> f32) -> GpuTexture {
    let size = 256u32;
    let texels: Vec<u8> = (0..size * size)
        .map(|i| (horizontal(((i % size) as f32 + 0.5) / size as f32) * 255.0).round() as u8)
        .collect();
    let desc = TextureDesc {
        format: TextureFormat::R8Unorm,
        width: size,
        height: size,
        mip_level_count: 1,
        swizzle: face_sdf::ONE_CHANNEL_SWIZZLE,
    };
    GpuTexture::upload(&harness.gpu, &Texture { desc, levels: vec![&texels] }, "sdf r8").expect("texture")
}

#[test]
#[ignore = "needs a GPU"]
fn the_characters_key_light_is_taken_in_its_heads_space() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, top_lit_longest);
    let face = face_square(&mut harness, &face_material(plain_face()), &[(SDF, &sdf)]);
    let side = SIDE as f32;
    let (from, to) = (MARGIN, SIDE as usize - MARGIN);
    let none = FaceExpression::default();
    // The world's sun from behind would shadow the whole face: listed, the face takes its key.
    harness.sun_from(toward(180.0, 0.0));

    // The head turned 40° to its left (about +Y): a key light 30° from the world's -X side is
    // 70° from the head's right.
    let turned = HeadBasis::turned(Quat::from_rotation_y(40f32.to_radians()));
    harness.character(face, toward(30.0, 0.0), turned, none);
    let edge = harness.render().edge(true, from, to);
    let expected = (1.0 - 70.0 / 180.0) * side;
    assert!((edge - expected).abs() < EDGE_TOLERANCE, "turned head: edge {edge}, expected {expected}");

    // The head tilted back 50° (nose up, about its right): a key straight in front is 50° below
    // head-forward, which only the horizontal channel reads: all lit. Tilted forward instead,
    // the key is 50° above, in the vertical channel.
    let tilt = |degrees: f32| HeadBasis::turned(Quat::from_rotation_x(degrees.to_radians()));
    harness.character(face, toward(0.0, 0.0), tilt(-50.0), none);
    assert!(harness.render().all(false, from, to, LIT), "light from below the head");
    harness.character(face, toward(0.0, 0.0), tilt(50.0), none);
    let edge = harness.render().edge(false, from, to);
    let expected = (1.0 - 50.0 / 180.0) * side;
    assert!((edge - expected).abs() < EDGE_TOLERANCE, "tilted forward: edge row {edge}, expected {expected}");

    // An unnormalized basis whose up leans toward forward is squared to it, keeping forward.
    let leaning = HeadBasis { forward: Vec3::new(0.0, 0.0, 2.0), up: Vec3::new(0.0, 3.0, 3.0) };
    harness.character(face, toward(60.0, 0.0), leaning, none);
    let edge = harness.render().edge(true, from, to);
    let expected = (1.0 - 60.0 / 180.0) * side;
    assert!((edge - expected).abs() < EDGE_TOLERANCE, "orthonormalized: edge {edge}, expected {expected}");

    // A face with no basis is ignored, and counted: the instance falls back to the world's sun,
    // here from behind, and a head at rest.
    harness.character(face, toward(0.0, 0.0), HeadBasis { forward: Vec3::ZERO, up: Vec3::Y }, none);
    assert!(harness.render().all(true, from, to, SHADOW), "the sun from behind");
    let stats = harness.renderer.stats();
    assert_eq!((stats.characters, stats.ignored_characters), (0, 1));
    // As is a character no longer listed.
    harness.characters.clear();
    harness.sun_from(toward(90.0, 0.0));
    let edge = harness.render().edge(true, from, to);
    assert!((edge - 0.5 * side).abs() < EDGE_TOLERANCE, "the sun from the right, at rest: {edge}");
}

#[test]
#[ignore = "needs a GPU"]
fn the_head_is_the_characters_whatever_the_instance_transform() {
    // A face square turned 30° to its left (about +Y), lit by a key 40° from the world's -X
    // side. With its head turned alike, the key is 70° from the head's right; with its head left
    // at rest, 40°: the character's head decides, not the mesh. The edge lies on the square
    // where u = 1 - yaw/180, wherever that projects.
    let transform = Mat4::from_rotation_y(30f32.to_radians());
    let turned = HeadBasis::turned(Quat::from_rotation_y(30f32.to_radians()));
    for (head, yaw) in [(turned, 70.0), (HeadBasis::REST, 40.0)] {
        let mut harness = Harness::new();
        let sdf = harness.sdf(right_lit_longest, never);
        let material = harness.material(&face_material(plain_face()), &[(SDF, &sdf)]);
        let mesh = harness.mesh(&square(), material);
        let id = harness.add(mesh, transform);
        harness.character(id, toward(40.0, 0.0), head, FaceExpression::default());
        let frame = harness.render();
        let point = |u: f32| transform.transform_point3(Vec3::new(u * 2.0 - 1.0, 0.0, 0.0));
        let edge_u = 1.0 - yaw / 180.0;
        for offset in [0.06, 0.15] {
            assert_near(frame.at(point(edge_u - offset)), LIT, &format!("{yaw}°: {offset} before the edge"));
            assert_near(frame.at(point(edge_u + offset)), SHADOW, &format!("{yaw}°: {offset} after it"));
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn eyes_stay_lit_highlights_brighten_and_the_nose_line_follows_the_camera() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, never);
    // R nose line on the image's right half (the head's left: the nose's far side from a camera
    // on the head's right), G highlight on the left half, B always lit in a band at the top.
    let mask = harness.half_texture(64, Swizzle::IDENTITY, |u, v| {
        [f32::from(u > 0.5), f32::from(u < 0.5), f32::from(v < 0.25), 1.0]
    });
    let face = FaceShading {
        mask_map: Some(id(MASK)),
        highlight: 0.15,
        highlight_reveal: 0.1,
        nose_line: 0.4,
        ..plain_face()
    };
    let square = face_square(&mut harness, &face_material(face), &[(SDF, &sdf), (MASK, &mask)]);

    // The character's key from 120° on the right: lit up to u = 1/3. The world's sun, dimmer and
    // from behind, lights nothing of it: the highlight takes the key's light too.
    harness.sun = DirectionalLight { direction: -toward(180.0, 0.0), color: Vec3::ONE, intensity: 0.5 };
    let none = FaceExpression::default();
    harness.character(square, toward(120.0, 0.0), HeadBasis::REST, none);
    let frame = harness.render();
    assert_near(frame.at_uv(0.75, 0.1), LIT, "always lit, deep in the shadow");
    assert_near(frame.at_uv(0.75, 0.6), SHADOW, "below the band, in shadow");
    // The highlight: full where the margin passes the reveal (u < 1/3 - 0.1), none in shadow.
    assert_near(frame.at_uv(0.1, 0.6), [ALBEDO * 1.15; 3], "highlighted");
    assert_near(frame.at_uv(0.45, 0.6), SHADOW, "no highlight in shadow");
    // Fading in from the edge: at u = 1/3 - 0.05 the margin is half the reveal.
    let halfway = frame.at_uv(1.0 / 3.0 - 0.05, 0.6)[0];
    assert!(ALBEDO + 0.02 < halfway && halfway < ALBEDO * 1.15 - 0.02, "fading: {halfway}");
    // Seen from the front, no nose line: the camera turns at most 18° from any pixel here.
    assert!(frame.all(true, SIDE as usize / 2 + 4, SIDE as usize - MARGIN, SHADOW));

    // The camera 40° around to the head's right: the line shows in the outline color where the
    // mask is, the turn's sine being over 0.4 there, and not on the other half.
    harness.character(square, toward(0.0, 0.0), HeadBasis::REST, none);
    harness.view_from(40.0);
    let frame = harness.render();
    let outline = [0.9, 0.1, 0.2];
    assert_near(frame.at(Vec3::new(0.5, -0.5, 0.0)), outline, "the line, on the far side");
    assert_near(frame.at(Vec3::new(-0.6, -0.5, 0.0)), [ALBEDO * 1.15; 3], "the near side: highlight only");
    // From the head's left, the mask is mirrored onto the other half.
    harness.view_from(-40.0);
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(-0.5, -0.5, 0.0)), outline, "mirrored line");
    // Only a slight turn, under 0.4 even with the perspective: no line yet.
    harness.view_from(10.0);
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.5, -0.5, 0.0)), LIT, "too slight a turn");
}

#[test]
#[ignore = "needs a GPU"]
fn expression_weights_blush_and_gloom_each_character_alone() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, never);
    // Blush on the left half, gloom on the right; always lit in a band at the top.
    let expression = harness
        .half_texture(64, Swizzle::IDENTITY, |u, _| [f32::from(u < 0.5), f32::from(u > 0.5), 0.0, 1.0]);
    let mask = harness.half_texture(64, Swizzle::IDENTITY, |_, v| [0.0, 0.0, f32::from(v < 0.25), 1.0]);
    let blush_color = [1.0, 0.5, 0.6];
    let face = FaceShading {
        expression_map: Some(id(EXPRESSION)),
        mask_map: Some(id(MASK)),
        blush_color,
        ..plain_face()
    };
    let textures = [(SDF, &sdf), (EXPRESSION, &expression), (MASK, &mask)];
    let material = harness.material(&face_material(face), &textures);
    let mesh = harness.mesh(&square(), material);
    // Grey ambient light, which the GI adds at 12%; gloom takes it away as occlusion does.
    harness.ambient = AmbientLight { sky: Vec3::ONE, ground: Vec3::ONE, intensity: 0.12 };
    let lit = [ALBEDO * 1.12; 3];
    // Three small faces in a row, lit from the front.
    let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(0.3));
    let ids = [-0.66, 0.0, 0.66].map(|x| harness.add(mesh, at(x)));
    let front = toward(0.0, 0.0);
    harness.character(ids[0], front, HeadBasis::REST, FaceExpression { blush: 1.0, gloom: 0.0 });
    harness.character(ids[1], front, HeadBasis::REST, FaceExpression { blush: 0.5, gloom: 1.0 });
    let frame = harness.render();
    let point = |x: f32, u: f32| Vec3::new(x + (u * 2.0 - 1.0) * 0.3, 0.0, 0.0);
    // At v = 0.1, in the always-lit band.
    let top = |x: f32, u: f32| point(x, u) + Vec3::new(0.0, 0.24, 0.0);
    assert_near(frame.at(point(-0.66, 0.25)), mul(lit, blush_color), "full blush");
    assert_near(frame.at(point(-0.66, 0.75)), lit, "no gloom");
    assert_near(frame.at(point(0.0, 0.25)), mul(lit, mix([1.0; 3], blush_color, 0.5)), "half blush");
    assert_near(frame.at(point(0.0, 0.75)), SHADOW, "full gloom: the shadow tone, without GI");
    assert_near(frame.at(top(0.0, 0.75)), lit, "always lit under full gloom: lit, with its GI");
    assert_near(frame.at(point(0.66, 0.25)), lit, "not a character: no expression");
    assert_near(frame.at(point(0.66, 0.75)), lit, "not a character: no gloom");

    // Weights past [0, 1] clamp: a blush of 3 is a full blush, a gloom of -1 none.
    harness.character(ids[0], front, HeadBasis::REST, FaceExpression { blush: 3.0, gloom: -1.0 });
    let frame = harness.render();
    assert_near(frame.at(point(-0.66, 0.25)), mul(lit, blush_color), "clamped blush");
    assert_near(frame.at(point(-0.66, 0.75)), lit, "clamped gloom");
}

#[test]
#[ignore = "needs a GPU"]
fn face_shadows_take_the_bodys_ramp_at_the_depth_the_face_map_gives() {
    let mut harness = Harness::new();
    let sdf = harness.sdf(right_lit_longest, never);
    // Every row of the ramp a linear gradient in red, 0 in the deepest shadow to 1 at the edge.
    let ramp = {
        let width = 256u32;
        let texels: Vec<u8> = (0..16 * width)
            .flat_map(|i| {
                let u = ((i % width) as f32 + 0.5) / width as f32;
                let encoded = if u <= 0.003_130_8 { u * 12.92 } else { 1.055 * u.powf(1.0 / 2.4) - 0.055 };
                [(encoded * 255.0).round() as u8, 0, 0, 255]
            })
            .collect();
        let desc = TextureDesc {
            format: TextureFormat::Rgba8UnormSrgb,
            width,
            height: 16,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        GpuTexture::upload(&harness.gpu, &Texture { desc, levels: vec![&texels] }, "ramp").expect("ramp")
    };
    let document = AnimeCharacterMaterial {
        diffuse: DiffuseMode::Ramp(RampDiffuse { map: id(RAMP), mix: 1.0 }),
        ..face_material(plain_face())
    };
    face_square(&mut harness, &document, &[(SDF, &sdf), (RAMP, &ramp)]);
    harness.sun_from(toward(60.0, 0.0));
    let frame = harness.render();
    // With threshold 0 the ramp's u is 1 + sin(π × margin): a quarter turn past the edge is the
    // deepest shadow.
    for u in [0.72, 0.8, 0.9] {
        let margin: f32 = 1.0 - u - 60.0 / 180.0;
        let ramp_u = 1.0 + (PI * margin).max(-0.5 * PI).sin();
        let red = frame.at_uv(u, 0.5)[0];
        assert!((red - ALBEDO * ramp_u).abs() < 0.03, "u {u}: red {red}, expected {}", ALBEDO * ramp_u);
    }
}
