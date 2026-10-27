//! Tests of the character prepass's readers that need a GPU (M1 C5): the depth-offset rim's
//! width, light gate, region mask and overlap, and its camera rim (M1 C8); the bangs' shadow on
//! faces; eyes through hair, its fade with the view, the IDs it keys on and the fog over it; and
//! the prepass's counters.
//! Run with
//! `cargo test -p sapphire_render --test rim_hair -- --ignored`.
//!
//! Scenes have no ambient light, a black sky, no outline and, for the rim, black albedo, so every
//! lit pixel of a rim test is rim. Characters are listed with white key lights.

use glam::{Mat4, Quat, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, CharacterSurface, EyeShading, FaceShading, HairHighlight,
    HairShading, HairShadow, LightSplit, Material, MaterialDocument, Outline, OutlineCurve, Rim, ThroughHair,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CameraRim, CharacterBody, CharacterFace, CharacterInstance,
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
/// A face's SDF map, which each test binds.
const SDF: &str = "6c0e1f2a-3b4c-4d5e-8f60-718293a4b5c6";

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

/// An outline distance curve that keeps widths at their full size from 10 cm to 60 m, so the
/// rim's width in pixels is the material's alone.
const FLAT: OutlineCurve = OutlineCurve { near: (0.1, 1.0), mid: 50.0, far: (60.0, 1.0), cutoff: 100.0 };

/// A body material with black albedo, a one-pixel light split and no outline: only its rim
/// lights it. The rim is `width` pixels at 1080 rows, its depth step a meter (deeper than any
/// test shape, so only what lies well behind makes a rim), gated by the light by `light_gate`,
/// with the Fresnel gate open; `regions` carry the per-region scales.
fn rim_material(width: f32, light_gate: f32, regions: Vec<CharacterRegion>) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions,
        rim: Rim { width, strength: 1.0, threshold: 1.0, fresnel: 1.0, light_gate, back_light: 0.0 },
        outline: Outline { width: 0.0, curve: FLAT, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A region rimming at `strength`, `width` and `overlap`.
fn region(strength: f32, width: f32, overlap: f32) -> CharacterRegion {
    CharacterRegion {
        rim_strength: strength,
        rim_width: width,
        rim_overlap: overlap,
        ..CharacterRegion::default()
    }
}

/// A flat-lit material in `color` and `surface`, one-pixel split, no rim or outline.
fn flat_material(surface: CharacterSurface, color: [f32; 3]) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface,
        base_color: [color[0], color[1], color[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// Hair with no mask and no highlight.
fn plain_hair() -> CharacterSurface {
    CharacterSurface::Hair(HairShading {
        mask_map: None,
        highlight: HairHighlight { strength: 0.0, ..HairHighlight::default() },
    })
}

/// A shape's vertices and triangles.
struct Part {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2], region: u8) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: [255; 3],
        flags: VertexFlags::with_material_id(MaterialId::new(region).expect("a material ID")),
    }
}

/// A sphere of `radius` around `center`, in `region`.
fn sphere(center: Vec3, radius: f32, region: u8) -> Part {
    let (rings, segments) = (48u32, 96u32);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let n = Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos());
            let uv = [segment as f32 / segments as f32, ring as f32 / rings as f32];
            vertices.push(vertex(center + n * radius, n, uv, region));
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
    Part { vertices, indices }
}

/// A rectangle facing +Z at depth `z`, from `min` to `max` in x and y, in `region`, its UVs
/// running left to right and top to bottom.
fn rect(min: (f32, f32), max: (f32, f32), z: f32, region: u8) -> Part {
    let vertices = [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)]
        .into_iter()
        .map(|(x, y)| {
            let uv = [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)];
            vertex(Vec3::new(x, y, z), Vec3::Z, uv, region)
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

    /// Whether the pixel `point` projects into is lit at all (rim tests: anything above black).
    fn lit(&self, x: usize, y: usize) -> bool {
        self.linear(x, y).iter().any(|&c| c > 0.05)
    }

    /// The runs of lit pixels along row `y` from either end, toward the middle: the left
    /// silhouette's rim and the right one's, in pixels.
    fn rims(&self, y: usize) -> (usize, usize) {
        let row: Vec<bool> = (0..SIDE as usize).map(|x| self.lit(x, y)).collect();
        let run = |pixels: &mut dyn Iterator<Item = &bool>| {
            pixels.skip_while(|lit| !**lit).take_while(|lit| **lit).count()
        };
        let half = SIDE as usize / 2;
        (run(&mut row[..half].iter()), run(&mut row[half..].iter().rev()))
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
    /// The frame's fog (M1 W3): none by default.
    fog: Fog,
    next_id: u64,
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
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.66, 1.0, 0.05),
            added: Vec::new(),
            removed: Vec::new(),
            characters: Vec::new(),
            fog: Fog::default(),
            next_id: 0,
        }
    }

    /// Looks at the origin from `distance` along `direction` (from the origin toward the eye)
    /// through a vertical field of view of `fov` degrees.
    fn view(&mut self, direction: Vec3, distance: f32, fov: f32) {
        let eye = direction.normalize() * distance;
        self.camera = CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, fov.to_radians(), 1.0, 0.05);
    }

    /// A face's SDF map lit by every light: both channels at their full angle.
    fn always_lit_sdf(&self) -> GpuTexture {
        let texels = vec![[f16::ONE.to_bits(); 4]; 16];
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width: 4,
            height: 4,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        GpuTexture::upload(&self.gpu, &Texture { desc, levels: vec![bytes] }, "sdf").expect("texture")
    }

    fn cook(document: &AnimeCharacterMaterial) -> Vec<u8> {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec()
    }

    /// Uploads `document`, its SDF map (if it names one) bound to `sdf`.
    fn material(&mut self, document: &AnimeCharacterMaterial, sdf: Option<&GpuTexture>) -> MaterialHandle {
        let lookup = |asked: AssetId| (asked == id(SDF)).then_some(sdf).flatten();
        let bytes = Self::cook(document);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    /// Replaces `handle` with `document`, its SDF map (if it names one) bound to `sdf`.
    fn update_with(
        &mut self,
        handle: MaterialHandle,
        document: &AnimeCharacterMaterial,
        sdf: Option<&GpuTexture>,
    ) {
        let lookup = |asked: AssetId| (asked == id(SDF)).then_some(sdf).flatten();
        let bytes = Self::cook(document);
        self.renderer.update_material(handle, Material::read(&bytes).expect("read"), lookup).expect("update");
    }

    /// Replaces `handle` with `document`, which names no textures.
    fn update(&mut self, handle: MaterialHandle, document: &AnimeCharacterMaterial) {
        self.update_with(handle, document, None);
    }

    /// Uploads `parts` as one mesh drawn with `materials`, a material per part.
    fn mesh(&mut self, parts: &[Part], materials: &[MaterialHandle]) -> MeshHandle {
        let bytes = mesh(parts).to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials).expect("upload")
    }

    /// Adds an instance of `mesh` at `transform` with the next frame and lists it as a character
    /// lit by a white key light from `toward_key`, its head at rest.
    fn character(&mut self, mesh: MeshHandle, transform: Mat4, toward_key: Vec3) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        self.light(id, toward_key);
        id
    }

    /// Lights character `id` from `toward_key` from the next frame on.
    fn light(&mut self, id: InstanceId, toward_key: Vec3) {
        let key = DirectionalLight { direction: -toward_key.normalize(), color: Vec3::ONE, intensity: 1.0 };
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(key),
            face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
            body: CharacterBody::default(),
            opacity: 1.0,
        });
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

/// The rim's width in pixels at the test target's height for a material width at 1080 rows.
fn rim_pixels(width: f32) -> f32 {
    width * SIDE as f32 / 1080.0
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_rim_keeps_its_width_in_pixels_across_distance_and_field_of_view() {
    let mut harness = Harness::new();
    let material = harness.material(&rim_material(40.0, 0.0, vec![region(1.0, 1.0, 1.0)]), None);
    let ball = harness.mesh(&[sphere(Vec3::ZERO, 0.5, 0)], &[material]);
    harness.character(ball, Mat4::IDENTITY, Vec3::Z);
    let expected = rim_pixels(40.0);
    let mut sizes = Vec::new();
    for (distance, fov) in [(3.0, 38.0), (6.0, 38.0), (6.0, 20.0), (12.0, 10.0)] {
        harness.view(Vec3::Z, distance, fov);
        let frame = harness.render();
        let (left, right) = frame.rims(SIDE as usize / 2);
        for (side, width) in [("left", left), ("right", right)] {
            assert!(
                (width as f32 - expected).abs() <= 1.5,
                "{side} rim at {distance} m through {fov}°: {width} px, expected {expected}"
            );
        }
        // The ball's size on screen changes all the while.
        let (center, edge) = (frame.pixel_of(Vec3::ZERO), frame.pixel_of(Vec3::new(-0.5, 0.0, 0.0)));
        sizes.push(center.0 - edge.0);
        // Its middle takes none: its normal faces the camera, so it looks at itself.
        assert_near(frame.at(Vec3::new(0.0, 0.0, 0.5)), [0.0; 3], "the middle");
    }
    assert!(sizes.windows(2).any(|pair| pair[0] != pair[1]), "the ball's size changed: {sizes:?}");

    // A region's width scales it.
    harness.update(material, &rim_material(40.0, 0.0, vec![region(1.0, 0.5, 1.0)]));
    harness.view(Vec3::Z, 3.0, 38.0);
    let (left, _) = harness.render().rims(SIDE as usize / 2);
    assert!((left as f32 - 0.5 * expected).abs() <= 1.5, "half the width: {left} px");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_light_gate_keeps_the_rim_to_the_lit_side_and_the_region_mask_turns_it_off() {
    let mut harness = Harness::new();
    let material = harness.material(&rim_material(40.0, 1.0, vec![region(1.0, 1.0, 1.0)]), None);
    let ball = harness.mesh(&[sphere(Vec3::ZERO, 0.5, 0)], &[material]);
    // Lit from the image's left.
    harness.character(ball, Mat4::IDENTITY, Vec3::new(-1.0, 0.0, 0.25));
    harness.view(Vec3::Z, 3.0, 38.0);
    let expected = rim_pixels(40.0);
    let (left, right) = harness.render().rims(SIDE as usize / 2);
    assert!((left as f32 - expected).abs() <= 1.5, "the lit side rims: {left} px");
    assert_eq!(right, 0, "the shadow side, fully gated, doesn't");

    // Half gated: the shadow side keeps half its rim's light, over the full width.
    harness.update(material, &rim_material(40.0, 0.5, vec![region(1.0, 1.0, 1.0)]));
    let frame = harness.render();
    let y = SIDE as usize / 2;
    let (left, right) = frame.rims(y);
    assert!(
        (right as f32 - expected).abs() <= 1.5 && (left as f32 - expected).abs() <= 1.5,
        "{left} {right}"
    );
    let first = (0..SIDE as usize).find(|&x| frame.lit(x, y)).expect("a rim");
    let last = (0..SIDE as usize).rev().find(|&x| frame.lit(x, y)).expect("a rim");
    let (lit, shaded) = (frame.linear(first + left / 2, y)[0], frame.linear(last - right / 2, y)[0]);
    assert!(lit > 0.3, "the lit side's rim: {lit}");
    assert!((shaded - 0.5 * lit).abs() < 0.05, "half on the shadow side: {shaded} against {lit}");

    // A region at strength 0 has none.
    harness.update(material, &rim_material(40.0, 0.0, vec![region(0.0, 1.0, 1.0)]));
    assert_eq!(harness.render().rims(SIDE as usize / 2), (0, 0), "masked out");
}

/// The unit vector from where pixel (`x`, `y`) of `frame` sees a ball of `radius` at the origin
/// toward the camera.
fn view_dir_on_ball(frame: &Frame, x: usize, y: usize, radius: f32) -> Vec3 {
    let side = SIDE as f32;
    let ndc = Vec3::new((x as f32 + 0.5) / side * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / side * 2.0, 0.5);
    let eye = frame.camera.position;
    let ray = ((frame.camera.proj * frame.camera.view).inverse().project_point3(ndc) - eye).normalize();
    let (b, c) = (eye.dot(ray), eye.length_squared() - radius * radius);
    assert!(b * b > c, "the pixel sees the ball");
    let point = eye + ray * (-b - (b * b - c).sqrt());
    (eye - point).normalize()
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_camera_rim_takes_the_rims_gating_by_its_share() {
    let mut harness = Harness::new();
    let material = harness.material(&rim_material(40.0, 1.0, vec![region(1.0, 1.0, 1.0)]), None);
    let ball = harness.mesh(&[sphere(Vec3::ZERO, 0.5, 0)], &[material]);
    // The key lights the image's left from the front, as a gameplay rig folds a key from behind
    // round to the camera's side; the camera rim comes from behind on the right.
    let character = harness.character(ball, Mat4::IDENTITY, Vec3::new(-1.0, 0.0, 0.25));
    harness.view(Vec3::Z, 3.0, 38.0);
    let behind = Vec3::new(1.0, 0.0, -0.5).normalize();
    let set = |harness: &mut Harness, share: f32| {
        let entry = harness.characters.iter_mut().find(|entry| entry.id == character).expect("listed");
        entry.lighting.camera_rim = CameraRim { direction: -behind * 2.0, share };
    };
    let y = SIDE as usize / 2;
    let expected = rim_pixels(40.0);

    // Without a share, the rim is the key light's exactly: the lit side alone.
    let keyed = harness.render();
    set(&mut harness, 0.0);
    let none = harness.render();
    assert!(none.pixels == keyed.pixels, "a share of 0 is no camera rim");
    let (left, right) = none.rims(y);
    assert!((left as f32 - expected).abs() <= 1.5 && right == 0, "{left} {right}");
    // A pixel in the middle of the left rim, and its mirror image on the right: the ball is
    // symmetric, so the two see the same mask.
    let first = (0..SIDE as usize).find(|&x| none.lit(x, y)).expect("a rim");
    let (at_left, at_right) = (first + left / 2, SIDE as usize - 1 - (first + left / 2));
    let full = none.linear(at_left, y);
    assert!(full[0] > 0.3, "the lit side's rim: {full:?}");

    // All of it: the rim moves to the camera rim's side, as bright as the key light's was.
    set(&mut harness, 1.0);
    let held = harness.render();
    let (left, right) = held.rims(y);
    assert!(left == 0 && (right as f32 - expected).abs() <= 1.5, "{left} {right}");
    assert_near(held.linear(at_right, y), full, "the camera rim's side");
    // Half: each side keeps half, the gates blended.
    set(&mut harness, 0.5);
    let half = harness.render();
    assert_near(half.linear(at_left, y), scale(full, 0.5), "the key light's side at half");
    assert_near(half.linear(at_right, y), scale(full, 0.5), "the camera rim's side at half");

    // Back-light weighting takes the camera rim's direction: 1 − V·L, stronger from behind.
    let back_lit = Rim { back_light: 1.0, ..rim_material(40.0, 1.0, Vec::new()).rim };
    let weighted =
        AnimeCharacterMaterial { rim: back_lit, ..rim_material(40.0, 1.0, vec![region(1.0, 1.0, 1.0)]) };
    harness.update(material, &weighted);
    set(&mut harness, 1.0);
    let frame = harness.render();
    let weight = 1.0 - view_dir_on_ball(&frame, at_right, y, 0.5).dot(behind);
    assert!(weight > 1.3, "from behind: {weight}");
    assert_near(frame.linear(at_right, y), scale(full, weight), "weighted by the camera rim");
}

/// `color` × `by`.
fn scale(color: [f32; 3], by: f32) -> [f32; 3] {
    color.map(|c| c * by)
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_region_keeps_its_rim_off_the_characters_own_steps_by_its_overlap() {
    let mut harness = Harness::new();
    // A ball (region 1) a meter and a half before a wall (region 0) of the same character.
    let regions = |overlap| vec![region(1.0, 1.0, 1.0), region(1.0, 1.0, overlap)];
    let material = harness.material(&rim_material(40.0, 0.0, regions(1.0)), None);
    let parts = [sphere(Vec3::ZERO, 0.5, 1), rect((-3.0, -3.0), (3.0, 3.0), -1.5, 0)];
    let figure = harness.mesh(&parts, &[material, material]);
    let character = harness.character(figure, Mat4::IDENTITY, Vec3::Z);
    harness.view(Vec3::Z, 3.0, 38.0);
    let expected = rim_pixels(40.0);
    let (left, right) = harness.render().rims(SIDE as usize / 2);
    assert!(
        (left as f32 - expected).abs() <= 1.5 && (right as f32 - expected).abs() <= 1.5,
        "{left} {right}"
    );

    // Overlap 0: the ball rims only against what isn't the character's own.
    harness.update(material, &rim_material(40.0, 0.0, regions(0.0)));
    assert_eq!(harness.render().rims(SIDE as usize / 2), (0, 0), "no rim on the character's own step");

    // The same wall as another character's: the ball rims against it again.
    harness.remove(character);
    let ball = harness.mesh(&[sphere(Vec3::ZERO, 0.5, 1)], &[material]);
    let wall = harness.mesh(&[rect((-3.0, -3.0), (3.0, 3.0), -1.5, 0)], &[material]);
    harness.character(ball, Mat4::IDENTITY, Vec3::Z);
    harness.character(wall, Mat4::IDENTITY, Vec3::Z);
    let (left, right) = harness.render().rims(SIDE as usize / 2);
    assert!(
        (left as f32 - expected).abs() <= 1.5 && (right as f32 - expected).abs() <= 1.5,
        "{left} {right}"
    );
}

/// The bangs'-shadow scene: a unit-square face (z = 0) always lit by its SDF map, and before it,
/// 0.1 m nearer, a hair patch over its upper middle, x from -0.25 to 0.25 and y from 0 to 0.5,
/// one character. Returns the face's material, to edit.
fn bangs_scene(harness: &mut Harness, sdf: &GpuTexture, toward_key: Vec3) -> (MaterialHandle, InstanceId) {
    let face = harness.material(&face_material(0.3), Some(sdf));
    let hair = harness.material(&flat_material(plain_hair(), [0.1, 0.2, 0.8]), None);
    let parts = [rect((-1.0, -1.0), (1.0, 1.0), 0.0, 0), rect((-0.25, 0.0), (0.25, 0.5), 0.1, 0)];
    let head = harness.mesh(&parts, &[face, hair]);
    let id = harness.character(head, Mat4::IDENTITY, toward_key);
    (face, id)
}

/// Face albedo, and its shadow tone (`flat_material`'s shade).
const FACE: [f32; 3] = [0.8, 0.7, 0.6];
const FACE_SHADOW: [f32; 3] = [0.2, 0.175, 0.15];

/// A face that the bangs shade from `distance` meters, its SDF map always lit.
fn face_material(distance: f32) -> AnimeCharacterMaterial {
    let face = FaceShading {
        softness: 0.0,
        highlight: 0.0,
        hair_shadow: HairShadow { distance, strength: 1.0 },
        ..FaceShading::new(id(SDF))
    };
    flat_material(CharacterSurface::Face(face), FACE)
}

/// The unit vector toward a light `yaw` degrees from head-forward toward the head's right (-X;
/// negative its left) and `elevation` degrees up, for a head at rest.
fn toward(yaw: f32, elevation: f32) -> Vec3 {
    let (yaw, elevation) = (yaw.to_radians(), elevation.to_radians());
    Vec3::new(-yaw.sin() * elevation.cos(), elevation.sin(), yaw.cos() * elevation.cos())
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_bangs_shadow_falls_on_the_face_away_from_the_key_light() {
    let mut harness = Harness::new();
    let sdf = harness.always_lit_sdf();
    harness.view(Vec3::Z, 1.0 / 19f32.to_radians().tan(), 38.0);
    // From above: the shadow hangs below the patch, about the distance × the light's share
    // across the screen (0.3 × sin 60° ≈ 0.26 m) deep.
    let (face, head) = bangs_scene(&mut harness, &sdf, toward(0.0, 60.0));
    let frame = harness.render();
    let face_at = |frame: &Frame, x: f32, y: f32| frame.at(Vec3::new(x, y, 0.0));
    assert_near(face_at(&frame, 0.0, -0.08), FACE_SHADOW, "just below the patch");
    assert_near(face_at(&frame, 0.0, -0.2), FACE_SHADOW, "within the shadow's depth");
    assert_near(face_at(&frame, 0.0, -0.36), FACE, "past it");
    assert_near(face_at(&frame, 0.45, 0.25), FACE, "beside the patch");
    assert_near(face_at(&frame, 0.0, 0.7), FACE, "above it");
    let stats = frame.stats.character_prepass;
    assert_eq!((stats.draws, stats.hair_shadow_characters, stats.through_hair_draws), (2, 1, 0));

    // From the head's right (the image's left): the shadow falls to the image's right.
    harness.light(head, toward(60.0, 0.0));
    let frame = harness.render();
    assert_near(face_at(&frame, 0.35, 0.25), FACE_SHADOW, "right of the patch");
    assert_near(face_at(&frame, -0.35, 0.25), FACE, "left of it");
    assert_near(face_at(&frame, 0.0, -0.08), FACE, "below it");

    // From the front, along the view: the light crosses no screen, so no shadow shows.
    harness.light(head, Vec3::Z);
    let frame = harness.render();
    for (x, y) in [(0.0, -0.05), (0.3, 0.25), (-0.3, 0.25)] {
        assert_near(face_at(&frame, x, y), FACE, "lit from the front");
    }

    // A face that casts no bangs' shadow.
    harness.light(head, toward(0.0, 60.0));
    harness.update_with(face, &face_material(0.0), Some(&sdf));
    let frame = harness.render();
    assert_near(face_at(&frame, 0.0, -0.08), FACE, "no shadow at distance 0");
    assert_eq!(frame.stats.character_prepass.hair_shadow_characters, 0);
}

/// Eye and hair colors for the eyes-through-hair scenes.
const EYE: [f32; 3] = [0.8, 0.1, 0.1];
const HAIR: [f32; 3] = [0.1, 0.2, 0.8];

/// An eye material drawn through hair at `opacity`.
fn eye_material(opacity: f32) -> AnimeCharacterMaterial {
    let through_hair = Some(ThroughHair { opacity, distance: 0.5, ..ThroughHair::default() });
    flat_material(CharacterSurface::Eye(EyeShading { through_hair, ..EyeShading::default() }), EYE)
}

/// The through-hair alpha the fades give for the camera `yaw` and `pitch` degrees off
/// head-forward, at the default fades (70° and 45°) and `opacity`.
fn faded(opacity: f32, yaw: f32, pitch: f32) -> f32 {
    let fade = |angle: f32, limit: f32| {
        1.0 - ((1.0 - angle.to_radians().cos()) / (1.0 - limit.to_radians().cos())).clamp(0.0, 1.0)
    };
    opacity * fade(yaw, 70.0).min(fade(pitch, 45.0))
}

#[test]
#[ignore = "needs a GPU adapter"]
fn eyes_show_through_their_own_hair_fading_as_the_camera_turns() {
    let mut harness = Harness::new();
    let eye = harness.material(&eye_material(0.5), None);
    let hair = harness.material(&flat_material(plain_hair(), HAIR), None);
    // An eye 0.1 m behind a wider patch of hair, one character, lit from the front.
    let parts = [rect((-0.5, -0.5), (0.5, 0.5), 0.0, 0), rect((-1.0, -1.0), (1.0, 1.0), 0.1, 0)];
    let head = harness.mesh(&parts, &[eye, hair]);
    let character = harness.character(head, Mat4::IDENTITY, Vec3::Z);
    let through = |harness: &mut Harness, direction: Vec3| {
        harness.view(direction, 4.0, 38.0);
        // The key light from the camera keeps both lit through any turn.
        harness.light(character, direction);
        let frame = harness.render();
        (frame.at(Vec3::new(0.0, 0.0, 0.1)), frame)
    };
    let (front, frame) = through(&mut harness, Vec3::Z);
    assert_near(front, mix(HAIR, EYE, 0.5), "half through, facing");
    assert_near(frame.at(Vec3::new(0.8, 0.8, 0.1)), HAIR, "hair where no eye lies behind");
    let stats = frame.stats.character_prepass;
    assert_eq!((stats.draws, stats.through_hair_draws), (2, 1));

    for (yaw, pitch) in [(35.0f32, 0.0f32), (-50.0, 0.0), (0.0, 25.0), (0.0, -30.0)] {
        let direction =
            Quat::from_rotation_y(-yaw.to_radians()) * Quat::from_rotation_x(-pitch.to_radians()) * Vec3::Z;
        let (color, _) = through(&mut harness, direction);
        let alpha = faded(0.5, yaw.abs(), pitch.abs());
        assert_near(color, mix(HAIR, EYE, alpha), &format!("yaw {yaw}°, pitch {pitch}°: alpha {alpha}"));
    }
    for (yaw, pitch) in [(75.0f32, 0.0f32), (0.0, 50.0)] {
        let direction =
            Quat::from_rotation_y(-yaw.to_radians()) * Quat::from_rotation_x(-pitch.to_radians()) * Vec3::Z;
        let (color, _) = through(&mut harness, direction);
        assert_near(color, HAIR, &format!("past the fade at yaw {yaw}°, pitch {pitch}°"));
    }

    // Opacity 0: none.
    harness.update(eye, &eye_material(0.0));
    let (color, frame) = through(&mut harness, Vec3::Z);
    assert_near(color, HAIR, "opacity 0");
    assert_eq!(frame.stats.character_prepass.through_hair_draws, 0, "no longer drawn through hair");
}

/// Eyes through hair end in the frame's fog (M1 W3) at their own distance and their character's
/// share, as the surface under them does: blended over the fogged hair, the eye's own pixel is
/// fogged too, not drawn clear through it.
#[test]
#[ignore = "needs a GPU adapter"]
fn eyes_through_hair_take_their_characters_fog() {
    let mut harness = Harness::new();
    let eye = harness.material(&eye_material(0.5), None);
    let hair = harness.material(&flat_material(plain_hair(), HAIR), None);
    let parts = [rect((-0.5, -0.5), (0.5, 0.5), 0.0, 0), rect((-1.0, -1.0), (1.0, 1.0), 0.1, 0)];
    let head = harness.mesh(&parts, &[eye, hair]);
    let character = harness.character(head, Mat4::IDENTITY, Vec3::Z);
    harness.view(Vec3::Z, 4.0, 38.0);
    harness.light(character, Vec3::Z);
    // A homogeneous haze from the camera on, of which characters take three quarters: about
    // half the light of the head reaches the camera.
    let haze = FogLayer { color: Vec3::new(0.9, 0.8, 0.3), density: 0.25, start: 0.0 };
    harness.fog = Fog { near: haze, characters: 0.75, ..Fog::default() };
    let frame = harness.render();
    let fog = harness.fog;
    let camera = Vec3::new(0.0, 0.0, 4.0);
    let fogged = |color: [f32; 3], depth: f32| {
        let ray = fog.ray(camera, Vec3::NEG_Z, 4.0 - depth, fog.characters, Vec3::ZERO, Vec3::Z);
        ray.apply(Vec3::from(color)).to_array()
    };
    let (hair_fogged, eye_fogged) = (fogged(HAIR, 0.1), fogged(EYE, 0.0));
    assert_near(frame.at(Vec3::new(0.8, 0.8, 0.1)), hair_fogged, "the hair, fogged");
    let through = frame.at(Vec3::new(0.0, 0.0, 0.1));
    assert_near(through, mix(hair_fogged, eye_fogged, 0.5), "the eye through it, fogged as well");
    // Unfogged, the eye would keep its own color through the haze, far from that.
    let clear_eye = mix(hair_fogged, EYE, 0.5);
    let apart = (0..3).map(|c| (clear_eye[c] - through[c]).abs()).fold(0.0, f32::max);
    assert!(apart > 0.1, "{through:?} against {clear_eye:?}");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn eyes_show_through_only_their_own_characters_hair() {
    let mut harness = Harness::new();
    harness.view(Vec3::Z, 4.0, 38.0);
    let eye = harness.material(&eye_material(0.5), None);
    let hair = harness.material(&flat_material(plain_hair(), HAIR), None);
    let skin = harness.material(&flat_material(CharacterSurface::Body, FACE), None);
    let eye_part = || rect((-0.3, -0.3), (0.3, 0.3), 0.0, 0);
    let front = || rect((-0.8, -0.8), (0.8, 0.8), 0.1, 0);
    // Another character's hair before the eye: it hides the eye.
    let lone_eye = harness.mesh(&[eye_part()], &[eye]);
    let other_hair = harness.mesh(&[front()], &[hair]);
    let first = harness.character(lone_eye, Mat4::IDENTITY, Vec3::Z);
    let second = harness.character(other_hair, Mat4::IDENTITY, Vec3::Z);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.1)), HAIR, "another character's hair");
    harness.remove(first);
    harness.remove(second);

    // The character's own skin before the eye: it hides it too, being no hair.
    let covered = harness.mesh(&[eye_part(), front()], &[eye, skin]);
    let id = harness.character(covered, Mat4::IDENTITY, Vec3::Z);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.1)), FACE, "the character's own skin");
    harness.remove(id);

    // Hair far before the eye, past the material's distance (0.5 m): hidden.
    let far = harness.mesh(&[eye_part(), rect((-0.8, -0.8), (0.8, 0.8), 0.8, 0)], &[eye, hair]);
    let id = harness.character(far, Mat4::IDENTITY, Vec3::Z);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.8)), HAIR, "hair too far before the eye");
    harness.remove(id);

    // Nothing before the eye: drawn whole, once.
    let bare = harness.mesh(&[eye_part()], &[eye]);
    harness.character(bare, Mat4::IDENTITY, Vec3::Z);
    assert_near(harness.render().at(Vec3::ZERO), EYE, "an uncovered eye");
}
