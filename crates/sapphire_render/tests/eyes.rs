//! Tests of anime eyes' layers that need a GPU (M1 C6): the layers' order and the eye depth
//! mask that clips them, a lid closing over them, the parallax refraction fake's shift, the
//! highlights' drift and sparkle on the frame's clock, eyes through hair under a closed lid, the
//! fog over the layers, and the proximity fade's dither over them (M1 C7). Run with
//! `cargo test -p sapphire_render --test eyes -- --ignored`.
//!
//! Scenes have no ambient light, a black sky and no outline; characters are listed with white key
//! lights from the camera's side, so an eye (always lit) shows its albedo.

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::{MaterialId, eye_mask};
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, CharacterSurface, EyeLayer, EyeShading, FaceShading,
    HairHighlight, HairShading, HighlightMotion, IrisShape, LightSplit, Material, MaterialDocument, Outline,
    Rim, ThroughHair,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, Fog, FogLayer, FramePacketRing, HeadBasis, InstanceId,
    MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, RenderStats, Renderer,
    SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 256;
/// How far a pixel's linear color may be from the prediction: 8-bit output.
const TOLERANCE: f32 = 0.025;
/// A face's SDF map (the lids'), and an eye's base color map, which tests bind.
const SDF: &str = "6c0e1f2a-3b4c-4d5e-8f60-718293a4b5c6";
const EYE_MAP: &str = "0b3c58a1-4f27-4e1d-9c6a-2d7e8f901a2b";

/// Colors: the eye white, the lid's shadow (its shade is a quarter of it), the iris, a
/// highlight, the skin around the eye (a body), a lid (a face) and hair.
const WHITE: [f32; 3] = [0.9, 0.9, 0.9];
const SHADOW: [f32; 3] = [0.4, 0.4, 0.8];
const IRIS: [f32; 3] = [0.1, 0.2, 0.7];
const HIGHLIGHT: [f32; 3] = [0.9, 0.8, 0.3];
const SKIN: [f32; 3] = [0.7, 0.5, 0.4];
const LID: [f32; 3] = [0.8, 0.7, 0.6];
const HAIR: [f32; 3] = [0.1, 0.6, 0.2];

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

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|c| c * s)
}

/// A flat-lit material in `color` (with `alpha`) and `surface`, one-pixel split, shade a quarter
/// of the albedo, no rim or outline.
fn flat_material(
    surface: CharacterSurface,
    color: [f32; 3],
    alpha: AlphaMode,
    a: f32,
) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface,
        alpha,
        base_color: [color[0], color[1], color[2], a],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// An eye layer's material: the eye itself opaque, the others blended at `alpha`.
fn layer_material(eye: EyeShading, color: [f32; 3], alpha: f32) -> AnimeCharacterMaterial {
    let mode = if eye.layer.is_overlay() { AlphaMode::Blend } else { AlphaMode::Opaque };
    flat_material(CharacterSurface::Eye(eye), color, mode, alpha)
}

fn layer(layer: EyeLayer) -> EyeShading {
    EyeShading { layer, ..EyeShading::default() }
}

/// A highlight that holds still and never glints.
fn still() -> EyeLayer {
    EyeLayer::Highlight(HighlightMotion { drift: 0.0, drift_rate: 0.0, sparkle: 0.0, sparkle_rate: 0.0 })
}

/// A lid: a face always lit by its SDF map.
fn lid_material() -> AnimeCharacterMaterial {
    let face = FaceShading { softness: 0.0, highlight: 0.0, ..FaceShading::new(id(SDF)) };
    flat_material(CharacterSurface::Face(face), LID, AlphaMode::Opaque, 1.0)
}

fn skin_material() -> AnimeCharacterMaterial {
    flat_material(CharacterSurface::Body, SKIN, AlphaMode::Opaque, 1.0)
}

fn hair_material() -> AnimeCharacterMaterial {
    let hair = HairShading {
        mask_map: None,
        highlight: HairHighlight { strength: 0.0, ..HairHighlight::default() },
    };
    flat_material(CharacterSurface::Hair(hair), HAIR, AlphaMode::Opaque, 1.0)
}

/// A shape's vertices and triangles.
struct Part {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: [255; 3],
        flags: VertexFlags::with_material_id(MaterialId::new(0).expect("a material ID")),
    }
}

/// A rectangle facing +Z at depth `z`, from `min` to `max` in x and y, its UVs running left to
/// right and top to bottom.
fn rect(min: (f32, f32), max: (f32, f32), z: f32) -> Part {
    let vertices = [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)]
        .into_iter()
        .map(|(x, y)| {
            let uv = [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)];
            vertex(Vec3::new(x, y, z), Vec3::Z, uv)
        })
        .collect();
    Part { vertices, indices: vec![0, 1, 2, 0, 2, 3] }
}

/// `parts` as one mesh, each part a submesh drawn with its own material slot, in order.
fn mesh(parts: &[Part]) -> Mesh {
    let (mut vertices, mut indices, mut submeshes) = (Vec::new(), Vec::new(), Vec::new());
    for (slot, part) in parts.iter().enumerate() {
        let base = vertices.len() as u32;
        let first_index = indices.len() as u32;
        vertices.extend_from_slice(&part.vertices);
        indices.extend(part.indices.iter().map(|index| base + index));
        submeshes.push(Submesh {
            first_index,
            index_count: part.indices.len() as u32,
            material: slot as u32,
        });
    }
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let slots = (0..parts.len())
        .map(|slot| MaterialSlot::embedded(format!("part {slot}"), placeholder.clone()))
        .collect();
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// One rendered frame.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    stats: RenderStats,
}

impl Frame {
    fn linear(&self, x: usize, y: usize) -> [f32; 3] {
        let pixel = self.pixels[y * SIDE as usize + x];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// Where `point` projects, in pixels.
    fn project(&self, point: Vec3) -> (f32, f32) {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let side = SIDE as f32;
        ((x + 1.0) * 0.5 * side, (1.0 - y) * 0.5 * side)
    }

    /// The linear color where `point` projects.
    fn at(&self, point: Vec3) -> [f32; 3] {
        let (x, y) = self.project(point);
        self.linear(x as usize, y as usize)
    }

    /// The centroid, in pixels, of the pixels within `radius` pixels of where `around` projects
    /// that `matches` picks, and how many there are.
    fn centroid(&self, around: Vec3, radius: f32, matches: impl Fn([f32; 3]) -> bool) -> ((f32, f32), usize) {
        let (cx, cy) = self.project(around);
        let (mut sum, mut count) = ((0.0, 0.0), 0);
        for y in 0..SIDE as usize {
            for x in 0..SIDE as usize {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                if (px - cx).abs() <= radius && (py - cy).abs() <= radius && matches(self.linear(x, y)) {
                    sum = (sum.0 + px, sum.1 + py);
                    count += 1;
                }
            }
        }
        assert!(count > 0, "no matching pixels around {around}");
        ((sum.0 / count as f32, sum.1 / count as f32), count)
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
    added: Vec<MeshInstance>,
    removed: Vec<InstanceId>,
    characters: Vec<CharacterInstance>,
    fog: Fog,
    time: f64,
    next_id: u64,
    sdf: GpuTexture,
    eye_map: Option<GpuTexture>,
}

impl Harness {
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
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
        // A face's SDF map lit by every light: both channels at their full angle.
        let texels = vec![[f16::ONE.to_bits(); 4]; 16];
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width: 4,
            height: 4,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        let sdf = GpuTexture::upload(&gpu, &Texture { desc, levels: vec![bytes] }, "sdf").expect("texture");
        let mut harness = Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: CameraData::look_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y, 0.66, 1.0, 0.05),
            added: Vec::new(),
            removed: Vec::new(),
            characters: Vec::new(),
            fog: Fog::default(),
            time: 0.0,
            next_id: 0,
            sdf,
            eye_map: None,
        };
        harness.view(Vec3::Z, 4.0, 38.0);
        harness
    }

    /// Looks at the origin from `distance` along `direction` (from the origin toward the eye)
    /// through a vertical field of view of `fov` degrees.
    fn view(&mut self, direction: Vec3, distance: f32, fov: f32) {
        let eye = direction.normalize() * distance;
        self.camera = CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, fov.to_radians(), 1.0, 0.05);
    }

    /// Uploads an eye's base color map, `side` texels square, white with a black disc of
    /// `radius` (texture coordinates) at its center: a pupil to follow.
    fn pupil_map(&mut self, side: u32, radius: f32) {
        let texels: Vec<[u8; 4]> = (0..side * side)
            .map(|i| {
                let (x, y) = ((i % side) as f32 + 0.5, (i / side) as f32 + 0.5);
                let (u, v) = (x / side as f32 - 0.5, y / side as f32 - 0.5);
                if u * u + v * v <= radius * radius { [0, 0, 0, 255] } else { [255; 4] }
            })
            .collect();
        let desc = TextureDesc {
            format: TextureFormat::Rgba8UnormSrgb,
            width: side,
            height: side,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        let texture = Texture { desc, levels: vec![bytes] };
        self.eye_map = Some(GpuTexture::upload(&self.gpu, &texture, "pupil").expect("texture"));
    }

    fn material(&mut self, document: &AnimeCharacterMaterial) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes =
            Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec();
        let (sdf, eye_map) = (&self.sdf, self.eye_map.as_ref());
        let lookup = |asked: AssetId| match asked {
            asked if asked == id(SDF) => Some(sdf),
            asked if asked == id(EYE_MAP) => eye_map,
            _ => None,
        };
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    /// Uploads `parts` as one mesh drawn with `materials`, a material per part.
    fn mesh(&mut self, parts: &[Part], materials: &[MaterialHandle]) -> MeshHandle {
        let bytes = mesh(parts).to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials).expect("upload")
    }

    /// Adds an instance of `mesh` at the origin with the next frame and lists it as a character
    /// lit by a white key light from +Z, its head at rest.
    fn character(&mut self, mesh: MeshHandle) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
        let key = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        self.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(key),
            face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
            body: CharacterBody::default(),
            opacity: 1.0,
        });
        id
    }

    /// Removes instance `id` with the next frame.
    fn remove(&mut self, id: InstanceId) {
        self.characters.retain(|character| character.id != id);
        self.removed.push(id);
    }

    /// Prepares, records and submits a frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.time_seconds = self.time;
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        packet.instances.removed.append(&mut self.removed);
        packet.characters.extend_from_slice(&self.characters);
        packet.fog = self.fog;
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

/// The eye: a unit square at z = 0, its white; skin around and behind it; and over it, nearer
/// in turn, the lid's shadow over its top (y 0.2 to 0.5, half covering), an iris reaching past
/// its top edge (x ±0.3, y -0.3 to 0.7) and a highlight. The mesh lists the layers' parts
/// last-drawn first, so only the layers' sort puts them in order.
fn layered_eye(harness: &mut Harness, extra: &[(Part, MaterialHandle)]) -> MeshHandle {
    let white = harness.material(&layer_material(layer(EyeLayer::Eye), WHITE, 1.0));
    let shadow = harness.material(&layer_material(layer(EyeLayer::Shadow), SHADOW, 0.5));
    let iris = harness.material(&layer_material(layer(EyeLayer::Iris), IRIS, 1.0));
    let highlight = harness.material(&layer_material(layer(still()), HIGHLIGHT, 1.0));
    let skin = harness.material(&skin_material());
    let mut parts = vec![
        rect((0.05, 0.05), (0.15, 0.15), 0.03),
        rect((-0.3, -0.3), (0.3, 0.7), 0.02),
        rect((-0.5, 0.2), (0.5, 0.5), 0.01),
        rect((-0.5, -0.5), (0.5, 0.5), 0.0),
        rect((-1.0, -1.0), (1.0, 1.0), -0.05),
    ];
    let mut materials = vec![highlight, iris, shadow, white, skin];
    for (part, material) in extra {
        parts.push(Part { vertices: part.vertices.clone(), indices: part.indices.clone() });
        materials.push(*material);
    }
    harness.mesh(&parts, &materials)
}

#[test]
#[ignore = "needs a GPU adapter"]
fn eye_layers_blend_in_order_within_the_eye_and_nowhere_else() {
    let mut harness = Harness::new();
    let eye = layered_eye(&mut harness, &[]);
    harness.character(eye);
    let frame = harness.render();
    let shade = scale(SHADOW, 0.25);
    assert_near(frame.at(Vec3::new(0.4, -0.2, 0.0)), WHITE, "the eye white");
    assert_near(
        frame.at(Vec3::new(0.4, 0.35, 0.0)),
        mix(WHITE, shade, 0.5),
        "the lid's shadow, its shade tone",
    );
    assert_near(frame.at(Vec3::new(0.0, 0.35, 0.0)), IRIS, "the iris over the shadow");
    assert_near(frame.at(Vec3::ZERO), IRIS, "the iris");
    assert_near(frame.at(Vec3::new(0.1, 0.1, 0.0)), HIGHLIGHT, "the highlight over the iris");
    // The iris lies nearer than the skin above the eye, but the mask clips it to the eye.
    assert_near(frame.at(Vec3::new(0.0, 0.6, 0.0)), SKIN, "the iris past the eye white");
    assert_near(frame.at(Vec3::new(0.7, 0.0, 0.0)), SKIN, "the skin");
    let prepass = frame.stats.character_prepass;
    assert_eq!((prepass.draws, prepass.eye_layer_draws), (2, 3), "eye and skin; three layers");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_lid_over_the_eye_takes_its_layers_with_it_whatever_their_depth() {
    let mut harness = Harness::new();
    // The lid closes over the eye's top half, nearer than the eye and its shadow but behind the
    // iris and the highlight: the depth test alone would let those show over it.
    let lid = harness.material(&lid_material());
    let eye = layered_eye(&mut harness, &[(rect((-0.5, 0.0), (0.5, 0.5), 0.015), lid)]);
    harness.character(eye);
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.0, 0.35, 0.0)), LID, "the iris under the lid");
    assert_near(frame.at(Vec3::new(0.1, 0.1, 0.0)), LID, "the highlight under the lid");
    assert_near(frame.at(Vec3::new(0.4, 0.35, 0.0)), LID, "the shadow under the lid");
    assert_near(frame.at(Vec3::new(0.0, -0.2, 0.0)), IRIS, "the iris below the lid");
    assert_near(frame.at(Vec3::new(0.4, -0.2, 0.0)), WHITE, "the eye white below the lid");
}

/// The gray of the backdrop behind a fading eye, a prop lit head-on.
const BACKDROP: [f32; 3] = [0.3, 0.3, 0.3];

/// The pixels within `radius` pixels of where `point` projects, each either `color` (the
/// character) or the backdrop behind it: how many show `color`, and how many there are.
fn dithered(frame: &Frame, point: Vec3, radius: i32, color: [f32; 3], what: &str) -> (usize, usize) {
    let (cx, cy) = frame.project(point);
    let (mut shown, mut total) = (0, 0);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let pixel = frame.linear((cx as i32 + dx) as usize, (cy as i32 + dy) as usize);
            let near = |target: [f32; 3]| pixel.iter().zip(target).all(|(a, e)| (a - e).abs() < TOLERANCE);
            assert!(near(color) || near(BACKDROP), "{what}: {pixel:?} is neither {color:?} nor the backdrop");
            shown += usize::from(near(color));
            total += 1;
        }
    }
    (shown, total)
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_fading_characters_eye_layers_fade_with_it() {
    let mut harness = Harness::new();
    let eye = layered_eye(&mut harness, &[]);
    harness.character(eye);
    // A gray backdrop behind the character that isn't one, so a hole the dither cuts can't pass
    // for black (docs/conventions.md, "Fragment stages that cut pixels out").
    let backdrop = harness.material(&flat_material(CharacterSurface::Body, BACKDROP, AlphaMode::Opaque, 1.0));
    let backdrop = harness.mesh(&[rect((-3.0, -3.0), (3.0, 3.0), -0.3)], &[backdrop]);
    harness.added.push(MeshInstance { id: InstanceId(1000), mesh: backdrop, transform: Mat4::IDENTITY });
    // Half faded (M1 C7): every layer keeps the pixels the eye and the skin keep, the proximity
    // fade's dither, and blends over them alone. Where the dither leaves the character out, the
    // backdrop shows, never a layer over it or the eye white through the iris.
    harness.characters[0].opacity = 0.5;
    let half = harness.render();
    let shade = mix(WHITE, scale(SHADOW, 0.25), 0.5);
    for (point, color, what) in [
        (Vec3::new(0.0, -0.15, 0.0), IRIS, "the iris"),
        (Vec3::new(0.4, 0.35, 0.0), shade, "the lid's shadow"),
        (Vec3::new(0.4, -0.2, 0.0), WHITE, "the eye white"),
        (Vec3::new(0.75, 0.0, 0.0), SKIN, "the skin"),
    ] {
        let (shown, total) = dithered(&half, point, 3, color, what);
        let share = shown as f32 / total as f32;
        assert!((share - 0.5).abs() < 0.15, "{what}: {shown} of {total} pixels shown");
    }
    assert_eq!((half.stats.faded_characters, half.stats.character_prepass.eye_layer_draws), (1, 3));
    // Wholly faded: no layer is left behind, though all three still draw.
    harness.characters[0].opacity = 0.0;
    let gone = harness.render();
    for (point, what) in [(Vec3::ZERO, "the iris"), (Vec3::new(0.1, 0.1, 0.0), "the highlight")] {
        assert_eq!(dithered(&gone, point, 6, IRIS, what).0, 0, "{what}: the backdrop alone");
    }
}

/// Where the pupil (at the eye's center) appears on an eye at z = 0 one meter wide, under a
/// parallax `depth` with the dome over the whole square, seen from `angle` degrees toward +X:
/// the point x where x − depth · sin(angle) · (1 − (2x)²) = 0.
fn pupil_at(depth: f32, angle: f32) -> f32 {
    let shift = depth * angle.to_radians().sin();
    (0..50).fold(0.0, |x: f32, _| shift * (1.0 - 4.0 * x * x))
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_iris_shifts_behind_the_cornea_as_the_view_turns() {
    let mut harness = Harness::new();
    harness.pupil_map(128, 0.06);
    let depth = 0.2;
    let eye = EyeShading { parallax: depth, iris: Some(IrisShape::default()), ..layer(EyeLayer::Eye) };
    let mut document = layer_material(eye, [1.0; 3], 1.0);
    document.base_color_map = Some(id(EYE_MAP));
    let material = harness.material(&document);
    let mesh = harness.mesh(&[rect((-0.5, -0.5), (0.5, 0.5), 0.0)], &[material]);
    harness.character(mesh);
    let dark = |color: [f32; 3]| color[0] < 0.2;
    for angle in [0.0f32, 30.0, -30.0, 50.0] {
        let direction = Vec3::new(angle.to_radians().sin(), 0.0, angle.to_radians().cos());
        harness.view(direction, 4.0, 38.0);
        let frame = harness.render();
        let expected = frame.project(Vec3::new(pupil_at(depth, angle), 0.0, 0.0));
        let (seen, count) = frame.centroid(Vec3::ZERO, 24.0, dark);
        assert!(count > 10, "{angle}°: the pupil shows ({count} pixels)");
        let off = ((seen.0 - expected.0).powi(2) + (seen.1 - expected.1).powi(2)).sqrt();
        assert!(off < 1.0, "{angle}°: the pupil at {seen:?}, expected {expected:?}");
    }
    // The shift is the parallax's: with none, the pupil holds the eye's center at any angle.
    let still_eye = layer_material(EyeShading { parallax: 0.0, ..eye }, [1.0; 3], 1.0);
    let still = harness.material(&AnimeCharacterMaterial { base_color_map: Some(id(EYE_MAP)), ..still_eye });
    let flat = harness.mesh(&[rect((-0.5, -0.5), (0.5, 0.5), 0.0)], &[still]);
    harness.remove(InstanceId(0));
    harness.character(flat);
    harness.view(Vec3::new(0.5, 0.0, 0.75f32.sqrt()), 4.0, 38.0);
    let frame = harness.render();
    let expected = frame.project(Vec3::ZERO);
    let (seen, _) = frame.centroid(Vec3::ZERO, 24.0, dark);
    assert!((seen.0 - expected.0).abs() < 1.0, "no parallax: {seen:?}, expected {expected:?}");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn highlights_drift_and_sparkle_on_the_frames_clock() {
    let mut harness = Harness::new();
    let motion = HighlightMotion { drift: 0.1, drift_rate: 0.25, sparkle: 1.0, sparkle_rate: 0.5 };
    let color = [0.3, 0.3, 0.1];
    let white = harness.material(&layer_material(layer(EyeLayer::Eye), [0.0; 3], 1.0));
    let highlight = harness.material(&layer_material(layer(EyeLayer::Highlight(motion)), color, 1.0));
    let parts = [rect((-0.5, -0.5), (0.5, 0.5), 0.0), rect((-0.04, -0.04), (0.04, 0.04), 0.01)];
    let mesh = harness.mesh(&parts, &[white, highlight]);
    // The renderer's first instance: slot 0, so its phase is 0.
    harness.character(mesh);
    let phase = eye_mask::phase(0);
    for seconds in [0.0f32, 0.37, 1.1, 2.3] {
        harness.time = f64::from(seconds);
        let frame = harness.render();
        let [dx, dy] = eye_mask::highlight_drift(motion.drift, motion.drift_rate, seconds, phase);
        let center = Vec3::new(dx, dy, 0.01);
        let glint = eye_mask::highlight_sparkle(motion.sparkle, motion.sparkle_rate, seconds, phase);
        assert_near(frame.at(center), scale(color, glint), &format!("the highlight's glint at {seconds} s"));
        let (seen, _) = frame.centroid(Vec3::ZERO, 30.0, |c| c[0] > 0.1);
        let expected = frame.project(center);
        let off = ((seen.0 - expected.0).powi(2) + (seen.1 - expected.1).powi(2)).sqrt();
        assert!(off < 1.0, "{seconds} s: the highlight at {seen:?}, expected {expected:?}");
    }
}

/// An eye drawn through hair at `opacity`, as `layer`.
fn through(layer: EyeLayer, opacity: f32) -> EyeShading {
    let through_hair = Some(ThroughHair { opacity, distance: 0.5, ..ThroughHair::default() });
    EyeShading { layer, through_hair, ..EyeShading::default() }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn eyes_under_a_closed_lid_stay_hidden_through_hair() {
    let mut harness = Harness::new();
    let eye = harness.material(&layer_material(through(EyeLayer::Eye, 0.5), WHITE, 1.0));
    let iris = harness.material(&layer_material(through(EyeLayer::Iris, 0.5), IRIS, 1.0));
    let hair = harness.material(&hair_material());
    let lid = harness.material(&lid_material());
    let eye_parts = || [rect((-0.5, -0.5), (0.5, 0.5), 0.0), rect((-0.2, -0.2), (0.2, 0.2), 0.01)];
    let bangs = || rect((-1.0, -1.0), (1.0, 1.0), 0.1);
    // Open: the eye, then its iris, each over the hair at half opacity.
    let [white_part, iris_part] = eye_parts();
    let open = harness.mesh(&[white_part, iris_part, bangs()], &[eye, iris, hair]);
    let id = harness.character(open);
    let frame = harness.render();
    let over_hair = mix(HAIR, WHITE, 0.5);
    assert_near(frame.at(Vec3::new(0.35, 0.0, 0.1)), over_hair, "the eye white through the bangs");
    assert_near(frame.at(Vec3::new(0.0, 0.0, 0.1)), mix(over_hair, IRIS, 0.5), "the iris over it");
    assert_eq!(frame.stats.character_prepass.through_hair_draws, 2);
    harness.remove(id);

    // Closed: a lid over the whole eye, under the bangs. Nothing shows through.
    let [white_part, iris_part] = eye_parts();
    let closed_lid = rect((-0.6, -0.6), (0.6, 0.6), 0.05);
    let closed = harness.mesh(&[white_part, iris_part, closed_lid, bangs()], &[eye, iris, lid, hair]);
    harness.character(closed);
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.35, 0.0, 0.1)), HAIR, "the closed eye's white");
    assert_near(frame.at(Vec3::new(0.0, 0.0, 0.1)), HAIR, "the closed eye's iris");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn eye_layers_take_their_characters_fog() {
    let mut harness = Harness::new();
    let eye = layered_eye(&mut harness, &[]);
    harness.character(eye);
    let haze = FogLayer { color: Vec3::new(0.9, 0.8, 0.3), density: 0.25, start: 0.0 };
    harness.fog = Fog { near: haze, characters: 0.75, ..Fog::default() };
    let frame = harness.render();
    let fog = harness.fog;
    let camera = Vec3::new(0.0, 0.0, 4.0);
    let fogged = |color: [f32; 3], depth: f32| {
        let ray = fog.ray(camera, Vec3::NEG_Z, 4.0 - depth, fog.characters, Vec3::ZERO, Vec3::Z);
        ray.apply(Vec3::from(color)).to_array()
    };
    assert_near(frame.at(Vec3::new(0.0, -0.2, 0.02)), fogged(IRIS, 0.02), "the iris, fogged");
    assert_near(frame.at(Vec3::new(0.1, 0.1, 0.03)), fogged(HIGHLIGHT, 0.03), "the highlight, fogged");
}
