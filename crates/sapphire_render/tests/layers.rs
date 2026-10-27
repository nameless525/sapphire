//! Tests of the anime character shader's layers and the proximity fade that need a GPU (M1 C7):
//! the stylized highlight's shape, threshold, mask and side; the matcap's channels, blends and
//! strengths; the reflection add's gate, strength and roughness, and the ambient work it makes
//! run; stockings by N·V and their sheen; emission,
//! its breath and its bloom; the layer mask; and the fade's dither, whole or none at the ends,
//! its share of pixels, its outlines, rim and draw list, under MSAA and temporal AA; and
//! alpha-tested surfaces cut out of the character prepass as the main pass cuts them. Run with
//! `cargo test -p sapphire_render --test layers -- --ignored`.
//!
//! Scenes have no ambient light and a black sky unless a test says otherwise, so a material's
//! layers are all that lights what its flat albedo doesn't. Characters are listed with white key
//! lights.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, Emission, LightSplit, Matcap, MatcapBlend,
    MatcapChannel, Material, MaterialDocument, Outline, OutlineCurve, RegionMatcap, Rim, Specular,
    SpecularModel, Stockings,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBloom, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, FramePacketRing, HeadBasis, InstanceId, MaterialHandle,
    MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, RenderStats, Renderer, SkyGradient,
    ToneCurve, ViewData, Vignette,
};
use sapphire_math::sh;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 256;
/// How far a pixel's linear color may be from the prediction: 8-bit output.
const TOLERANCE: f32 = 0.025;
/// The textures tests bind, by the IDs their materials name.
const MATCAP: &str = "0c7a1c4e-1111-4c7a-8c7a-0000000000a1";
const MASK: &str = "0c7a1c4e-2222-4c7a-8c7a-0000000000a2";
const CONTROL: &str = "0c7a1c4e-3333-4c7a-8c7a-0000000000a3";
const CARD: &str = "0c7a1c4e-4444-4c7a-8c7a-0000000000a4";

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

fn scale(color: [f32; 3], by: f32) -> [f32; 3] {
    color.map(|c| c * by)
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}

/// The sky gradient in a direction, as `sky_color` in frame.wgsl has it.
fn sky_color(sky: SkyGradient, dir: Vec3) -> Vec3 {
    if dir.y >= 0.0 {
        sky.horizon.lerp(sky.zenith, dir.y.clamp(0.0, 1.0).sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-dir.y * 5.0).clamp(0.0, 1.0))
    }
}

/// A flat-lit material: `albedo`, a one-pixel split at N·L 0 whose shadow side is black, no rim
/// or outline, one region; tests add their layers.
fn flat(albedo: [f32; 3]) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [albedo[0], albedo[1], albedo[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// `material` with its one region changed by `edit`.
fn with_region(
    mut material: AnimeCharacterMaterial,
    edit: impl Fn(&mut CharacterRegion),
) -> AnimeCharacterMaterial {
    material.regions.iter_mut().for_each(edit);
    material
}

/// A shape's vertices and triangles.
struct Part {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

/// A sphere of `radius` around `center`, `u` around it and `v` from the top pole down.
fn sphere(center: Vec3, radius: f32) -> Part {
    let (rings, segments) = (64u32, 128u32);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let n = Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos());
            vertices.push(Vertex {
                position: (center + n * radius).to_array(),
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
    Part { vertices, indices }
}

/// A rectangle facing +Z at depth `z`, from `min` to `max` in x and y.
fn rect(min: (f32, f32), max: (f32, f32), z: f32) -> Part {
    let vertices = [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)]
        .into_iter()
        .map(|(x, y)| Vertex {
            position: [x, y, z],
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)],
            color: [255; 3],
            flags: VertexFlags::with_material_id(MaterialId::new(0).expect("a material ID")),
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

    /// The pixels within `radius` pixels of where `point` projects.
    fn around(&self, point: Vec3, radius: usize) -> Vec<[f32; 3]> {
        let (cx, cy) = self.pixel_of(point);
        let mut pixels = Vec::new();
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                pixels.push(self.linear(x, y));
            }
        }
        pixels
    }
}

/// The point of a sphere of `radius` at the origin whose normal is `normal`.
fn on_sphere(radius: f32, normal: Vec3) -> Vec3 {
    normal.normalize() * radius
}

/// A renderer and everything a test frame needs.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    textures: HashMap<AssetId, GpuTexture>,
    added: Vec<MeshInstance>,
    characters: Vec<CharacterInstance>,
    sky: SkyGradient,
    ambient: AmbientLight,
    post: PostParams,
    time: f64,
    next_id: u64,
}

impl Harness {
    fn new() -> Self {
        Self::with(RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() })
    }

    fn with(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
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
            textures: HashMap::new(),
            added: Vec::new(),
            characters: Vec::new(),
            sky: SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO },
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            post: passthrough(),
            time: 0.0,
            next_id: 0,
        }
    }

    /// Binds a `width` × `height` linear RGBA texture whose texel at `(u, v)` (its center, in
    /// texture coordinates) is `texel(u, v)` to `name`.
    fn texture(&mut self, name: &str, width: u32, height: u32, texel: impl Fn(f32, f32) -> [f32; 4]) {
        let mut texels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let (u, v) = ((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
                texels.push(texel(u, v).map(|c| f16::from_f32(c).to_bits()));
            }
        }
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width,
            height,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        let bytes: &[u8] = bytemuck::cast_slice(&texels);
        let texture =
            GpuTexture::upload(&self.gpu, &Texture { desc, levels: vec![bytes] }, name).expect("texture");
        self.textures.insert(id(name), texture);
    }

    fn cook(document: &AnimeCharacterMaterial) -> Vec<u8> {
        document.validate().expect("a valid test material");
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec()
    }

    fn material(&mut self, document: &AnimeCharacterMaterial) -> MaterialHandle {
        let bytes = Self::cook(document);
        let textures = &self.textures;
        let lookup = |asked: AssetId| textures.get(&asked);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    fn update(&mut self, handle: MaterialHandle, document: &AnimeCharacterMaterial) {
        let bytes = Self::cook(document);
        let textures = &self.textures;
        let lookup = |asked: AssetId| textures.get(&asked);
        self.renderer.update_material(handle, Material::read(&bytes).expect("read"), lookup).expect("update");
    }

    fn mesh(&mut self, parts: &[Part], materials: &[MaterialHandle]) -> MeshHandle {
        let bytes = mesh(parts).to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials).expect("upload")
    }

    /// Adds an instance of `mesh` with the next frame and lists it as a character lit by a white
    /// key light of `intensity` from `toward_key`.
    fn character(&mut self, mesh: MeshHandle, toward_key: Vec3, intensity: f32) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
        let key = DirectionalLight { direction: -toward_key.normalize(), color: Vec3::ONE, intensity };
        self.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(key),
            face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
            body: CharacterBody::default(),
            opacity: 1.0,
        });
        id
    }

    /// Adds an instance of `mesh` that isn't a character.
    fn prop(&mut self, mesh: MeshHandle) {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
    }

    /// Sets character `id`'s opacity from the next frame on.
    fn opacity(&mut self, id: InstanceId, opacity: f32) {
        let character = self.characters.iter_mut().find(|c| c.id == id).expect("a character");
        character.opacity = opacity;
    }

    /// Prepares, records and submits a frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.post = self.post;
        packet.time_seconds = self.time;
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

/// A ball of radius 0.5 at the origin, drawn with `material`, listed as a character lit from
/// the camera's side by a white key light of intensity 1.
fn ball(harness: &mut Harness, material: &AnimeCharacterMaterial) -> (MaterialHandle, InstanceId) {
    let handle = harness.material(material);
    let mesh = harness.mesh(&[sphere(Vec3::ZERO, 0.5)], &[handle]);
    let id = harness.character(mesh, Vec3::Z, 1.0);
    (handle, id)
}

/// A white highlight of `size` in `model`, crisp, untinted.
fn highlight(model: SpecularModel, size: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        specular: Specular { model, softness: 0.0, albedo: 0.0 },
        ..with_region(flat([0.2; 3]), |region| {
            region.specular_color = [0.5; 3];
            region.specular_size = size;
        })
    }
}

/// How far from the ball's center along its equator, in its radius, the highlight reaches: the
/// first point leftward whose color is the plain diffuse's.
fn highlight_reach(frame: &Frame) -> f32 {
    (0..100)
        .map(|i| i as f32 / 100.0)
        .find(|&s| frame.at(on_sphere(0.5, Vec3::new(-s, 0.0, (1.0 - s * s).sqrt())))[0] < 0.45)
        .expect("the highlight ends on the ball")
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_highlight_is_a_crisp_shape_of_n_dot_h_cut_at_the_threshold() {
    let mut harness = Harness::new();
    let (material, _) = ball(&mut harness, &highlight(SpecularModel::Blinn, 32.0));
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.0, 0.0, 0.5)), [0.7; 3], "albedo plus the highlight, facing");
    assert_near(frame.at(on_sphere(0.5, Vec3::new(-0.6, 0.0, 0.8))), [0.2; 3], "outside it");
    // Blinn's N·H^32 is 0.5 at about 11.9°: the light and the camera are almost one direction, so
    // H is nearly the view, and the cut lies about sin(11.9°) of the radius out.
    let reach = highlight_reach(&frame);
    assert!((reach - 0.2).abs() < 0.04, "Blinn 32: {reach}");
    assert_eq!(frame.stats.characters, 1);

    // A larger exponent, a smaller highlight; a rougher GGX, a larger one.
    harness.update(material, &highlight(SpecularModel::Blinn, 128.0));
    let small = highlight_reach(&harness.render());
    assert!(small < reach * 0.7, "Blinn 128 against 32: {small} {reach}");
    harness.update(material, &highlight(SpecularModel::Ggx, 0.5));
    let ggx = highlight_reach(&harness.render());
    // GGX at roughness r (α = r²) is 0.5 where N·H = sqrt((α·sqrt(2) − 1)/(α − 1)): about 21.8°
    // at 0.5.
    assert!((ggx - 0.37).abs() < 0.05, "GGX 0.5: {ggx}");

    // Taking the albedo tints it.
    let mut tinted = highlight(SpecularModel::Blinn, 32.0);
    tinted.base_color = [0.4, 0.2, 0.1, 1.0];
    tinted.specular.albedo = 1.0;
    harness.update(material, &tinted);
    let expected = add([0.4, 0.2, 0.1], [0.2, 0.1, 0.05]);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), expected, "region color × albedo");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_control_map_sizes_and_masks_the_highlight_and_it_stays_on_the_lit_side() {
    let mut harness = Harness::new();
    // R mask 1, G neutral bias, B threshold 0.8 (cut at 0.2), A material ID 0.
    harness.texture(CONTROL, 4, 4, |_, _| [1.0, 0.5, 0.8, 0.0]);
    let mapped = |mut material: AnimeCharacterMaterial| {
        material.control_map = Some(id(CONTROL));
        material
    };
    let plain = highlight(SpecularModel::Blinn, 32.0);
    let (material, character) = ball(&mut harness, &plain);
    let base = highlight_reach(&harness.render());
    harness.update(material, &mapped(plain.clone()));
    let wide = highlight_reach(&harness.render());
    assert!(wide > base * 1.5, "a lower cut, a larger highlight: {wide} against {base}");

    // The mask turns it off.
    harness.texture(CONTROL, 4, 4, |_, _| [0.0, 0.5, 0.8, 0.0]);
    harness.update(material, &mapped(plain.clone()));
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), [0.2; 3], "masked");

    // Lit from the side, the highlight is on the lit half alone: none where N·L < 0.
    harness.update(material, &plain);
    let toward_key = Vec3::new(1.0, 0.0, -0.3).normalize();
    let key = DirectionalLight { direction: -toward_key, color: Vec3::ONE, intensity: 1.0 };
    harness.characters.iter_mut().find(|c| c.id == character).expect("listed").lighting =
        CharacterLighting::unstyled(key);
    let frame = harness.render();
    let peak = frame.at(on_sphere(0.5, toward_key + Vec3::Z));
    assert!(peak[0] > 0.6, "the highlight toward H on the lit side: {peak:?}");
    for point in frame.around(on_sphere(0.5, Vec3::new(-0.6, 0.0, 0.8)), 2) {
        assert_near(point, [0.0; 3], "the shadow side: black, no highlight");
    }
}

/// A black ball with a matcap of `strength` lit and `shadow` in shadow, its region taking
/// `channel`, `blend` and `tint`.
fn matcapped(
    albedo: [f32; 3],
    channel: MatcapChannel,
    blend: MatcapBlend,
    tint: [f32; 3],
    shadow: f32,
) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        matcap: Some(Matcap { map: id(MATCAP), strength: 1.0, shadow_strength: shadow }),
        ..with_region(flat(albedo), |region| {
            region.matcap = RegionMatcap { channel, blend, color: tint };
        })
    }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_matcap_picks_its_channel_tint_and_blend_by_the_view_space_normal() {
    let mut harness = Harness::new();
    // Red where the view-space normal points left, blue where it points right; A 0.8 throughout.
    harness.texture(MATCAP, 8, 8, |u, _| if u < 0.5 { [0.6, 0.1, 0.1, 0.8] } else { [0.1, 0.1, 0.6, 0.8] });
    let add_rgb = matcapped([0.0; 3], MatcapChannel::Rgb, MatcapBlend::Add, [1.0; 3], 0.5);
    let (material, _) = ball(&mut harness, &add_rgb);
    let frame = harness.render();
    let (left, right) = (on_sphere(0.5, Vec3::new(-0.7, 0.0, 0.7)), on_sphere(0.5, Vec3::new(0.7, 0.0, 0.7)));
    assert_near(frame.at(left), [0.6, 0.1, 0.1], "the left of the matcap, added in the key's light");
    assert_near(frame.at(right), [0.1, 0.1, 0.6], "its right");

    // A grayscale channel × the region's tint.
    harness.update(material, &matcapped([0.0; 3], MatcapChannel::A, MatcapBlend::Add, [1.0, 0.5, 0.25], 0.5));
    assert_near(harness.render().at(left), [0.8, 0.4, 0.2], "A × the tint");
    harness.update(material, &matcapped([0.0; 3], MatcapChannel::R, MatcapBlend::Add, [1.0; 3], 0.5));
    assert_near(harness.render().at(right), [0.1; 3], "R as gray");
    harness.update(material, &matcapped([0.0; 3], MatcapChannel::Off, MatcapBlend::Add, [1.0; 3], 0.5));
    assert_near(harness.render().at(left), [0.0; 3], "a region without one");

    // Multiplying the albedo: the lit tone takes it.
    harness.update(material, &matcapped([0.5; 3], MatcapChannel::A, MatcapBlend::Multiply, [1.0; 3], 0.5));
    assert_near(harness.render().at(left), [0.4; 3], "albedo × 0.8");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_matcap_takes_its_shadow_strength_and_the_layer_masks_r() {
    let mut harness = Harness::new();
    harness.texture(MATCAP, 4, 4, |_, _| [0.4, 0.4, 0.4, 1.0]);
    harness.texture(MASK, 4, 4, |_, _| [0.5, 1.0, 1.0, 1.0]);
    let material = matcapped([0.0; 3], MatcapChannel::Rgb, MatcapBlend::Add, [1.0; 3], 0.25);
    let mesh_material = harness.material(&material);
    let mesh = harness.mesh(&[sphere(Vec3::ZERO, 0.5)], &[mesh_material]);
    // Lit from behind: every visible point is on the shadow side.
    harness.character(mesh, Vec3::NEG_Z, 1.0);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), [0.1; 3], "0.4 × the shadow strength");
    let masked = AnimeCharacterMaterial { layer_mask: Some(id(MASK)), ..material };
    harness.update(mesh_material, &masked);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), [0.05; 3], "× the mask's R");
}

/// A ball in `albedo` that reflects by `metallic`.
fn metal(albedo: [f32; 3], metallic: f32, roughness: f32) -> AnimeCharacterMaterial {
    with_region(flat(albedo), |region| {
        region.metallic = metallic;
        region.roughness = roughness;
    })
}

/// Per channel, the scale M1 W5's projection gives the sky cube's radiance in the reflection
/// cube: the authored ambient sky over the L1 irradiance the sky gives a surface facing straight
/// up (`ambient::sky_radiance` has it analytically).
fn ambient_scale(sky: SkyGradient, ambient: AmbientLight) -> Vec3 {
    let directions = sh::fibonacci_directions(8192);
    let solid_angle = 4.0 * std::f32::consts::PI / directions.len() as f32;
    let mut channels = [sh::Projection::<{ sh::L2 }>::default(); 3];
    for &d in &directions {
        for (channel, value) in channels.iter_mut().zip(sky_color(sky, d).to_array()) {
            channel.add(d, value, solid_angle);
        }
    }
    let open = channels.map(|channel| sh::evaluate(&sh::irradiance(channel.coefficients)[..sh::L1], Vec3::Y));
    ambient.sky * ambient.intensity / Vec3::from(open)
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_reflection_add_shows_the_sky_at_the_ambients_strength_above_the_gate() {
    let mut harness = Harness::new();
    harness.sky = SkyGradient {
        zenith: Vec3::new(0.2, 0.35, 0.9),
        horizon: Vec3::new(0.7, 0.75, 0.8),
        ground: Vec3::new(0.3, 0.25, 0.2),
    };
    harness.ambient =
        AmbientLight { sky: Vec3::new(0.3, 0.3, 0.35), ground: Vec3::new(0.1, 0.1, 0.1), intensity: 1.0 };
    let albedo = [0.8, 0.6, 0.4];
    let (material, _) = ball(&mut harness, &metal(albedo, 0.0, 0.0));
    // No key light: the ambient and the sky stay within the 8-bit range.
    harness.characters[0].lighting.key.intensity = 0.0;
    let plain = harness.render();
    // Nothing reflects: no ambient pass, pipeline or cube (M1 W5, "Nothing for work nothing
    // reads"), without a grid.
    let off = plain.stats.ambient;
    assert_eq!((off.pipelines, off.dispatches, off.reflection_mips), (0, 0, 0));
    harness.update(material, &metal(albedo, 1.0, 0.0));
    let reflecting = harness.render();
    // The projection's three pipelines and the prefilter, built when the reflective material
    // first drew, and every face of the cube prefiltered that frame: the projection's three
    // dispatches (the faces' threads, the faces, the sky light), then one per mip.
    let on = reflecting.stats.ambient;
    assert_eq!((on.pipelines, on.reflection_size, on.prefiltered_faces), (4, 128, 6));
    assert_eq!(on.dispatches, 3 + on.reflection_mips);
    // The cube's mip 0 is the sky at the ambient's scale (W5's projection).
    let strength = ambient_scale(harness.sky, harness.ambient);
    let camera = harness.camera.position;
    let reflected = |normal: Vec3| {
        let point = on_sphere(0.5, normal);
        let view = (camera - point).normalize();
        let n = normal.normalize();
        (point, 2.0 * n.dot(view) * n - view)
    };
    // Reflections well above and below the horizon, where the sky cube's texels follow the
    // gradient (at the horizon its sqrt bends within a texel).
    let normals = [Vec3::new(0.0, 0.35, 0.94), Vec3::new(0.0, 0.6, 0.8), Vec3::new(-0.5, -0.3, 0.81)];
    for normal in normals {
        let (point, direction) = reflected(normal);
        let sky = sky_color(harness.sky, direction) * strength * Vec3::from(albedo);
        let expected = add(plain.at(point), sky.to_array());
        assert_near(reflecting.at(point), expected, &format!("the sky along {direction:?}"));
    }
    // The next frames prefilter the face the sky cube draws.
    assert_eq!(harness.render().stats.ambient.prefiltered_faces, 7);
    // A rough metal reads a rougher mip: the gradient between the zenith and below the horizon
    // blurs toward its mean.
    harness.update(material, &metal(albedo, 1.0, 1.0));
    let rough = harness.render();
    let (above, below) = (reflected(normals[1]).0, reflected(normals[2]).0);
    let spread = |frame: &Frame| (Vec3::from(frame.at(above)) - Vec3::from(frame.at(below))).length();
    assert!(spread(&rough) < 0.9 * spread(&reflecting), "{} vs {}", spread(&rough), spread(&reflecting));
    // Below the gate, none, and no ambient work; the layer mask's G scales metallic under it too.
    harness.update(material, &metal(albedo, 0.89, 0.0));
    let point = Vec3::new(0.0, 0.0, 0.5);
    let below_gate = harness.render();
    assert_near(below_gate.at(point), plain.at(point), "metallic 0.89");
    assert_eq!(below_gate.stats.ambient.dispatches, 0, "nothing reflects");
    harness.texture(MASK, 4, 4, |_, _| [1.0, 0.5, 1.0, 1.0]);
    let masked = AnimeCharacterMaterial { layer_mask: Some(id(MASK)), ..metal(albedo, 1.0, 0.0) };
    harness.update(material, &masked);
    assert_near(harness.render().at(point), plain.at(point), "1 × the mask's 0.5");
}

/// Skin in black stockings, opaque at the silhouette and a quarter facing the camera.
fn stockinged(weight: f32, sheen: [f32; 3], weave: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        stockings: Stockings {
            color: [0.0; 3],
            edge_opacity: 1.0,
            facing_opacity: 0.25,
            range: [0.2, 0.8],
            sheen,
            sheen_sharpness: 1.0,
            weave_scale: 400.0,
            weave_strength: weave,
            ..Stockings::default()
        },
        ..with_region(flat([0.8, 0.6, 0.5]), |region| region.stocking = weight)
    }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn stockings_darken_toward_the_silhouette_and_take_a_sheen_along_the_fabric() {
    let mut harness = Harness::new();
    let skin = [0.8, 0.6, 0.5];
    let (material, _) = ball(&mut harness, &stockinged(1.0, [0.0; 3], 0.0));
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.0, 0.0, 0.5)), scale(skin, 0.75), "a quarter facing the camera");
    // N·V about 0.15 near the silhouette: below the range, fully covered.
    let edge = on_sphere(0.5, Vec3::new(-0.987, 0.0, 0.16));
    let at_edge = frame.at(edge);
    assert!(at_edge.iter().all(|&c| c < 0.06), "black toward the silhouette: {at_edge:?}");
    harness.update(material, &stockinged(0.0, [0.0; 3], 0.0));
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), skin, "a region that wears none");

    // The sheen runs along v (top to bottom on the ball): facing, T is across H, at full strength
    // × the fabric's quarter.
    harness.update(material, &stockinged(1.0, [0.4; 3], 0.0));
    let expected = add(scale(skin, 0.75), [0.1; 3]);
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), expected, "the sheen");

    // A weave far finer than a pixel fades to its average.
    harness.update(material, &stockinged(1.0, [0.0; 3], 1.0));
    assert_near(harness.render().at(Vec3::new(0.0, 0.0, 0.5)), scale(skin, 0.75), "the weave's average");
}

/// A ball that only glows: a black albedo under a key light of 0.
fn glowing(emission: [f32; 3], breath: Emission) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        emission: breath,
        ..with_region(flat([0.0; 3]), |region| region.emission = emission)
    }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn emission_glows_unlit_and_breathes() {
    let mut harness = Harness::new();
    let steady = Emission { period: 0.0, depth: 0.5, bloom: 1.0 };
    let handle = harness.material(&glowing([0.3, 0.6, 0.1], steady));
    let mesh = harness.mesh(&[sphere(Vec3::ZERO, 0.5)], &[handle]);
    harness.character(mesh, Vec3::Z, 0.0);
    let center = Vec3::new(0.0, 0.0, 0.5);
    assert_near(harness.render().at(center), [0.3, 0.6, 0.1], "steady, with no light");

    // A breath of 4 s at half depth: full at its start, half at its middle (instance slot 0's
    // phase is 0).
    let breathing = Emission { period: 4.0, depth: 0.5, bloom: 1.0 };
    harness.update(handle, &glowing([0.3, 0.6, 0.1], breathing));
    harness.time = 0.0;
    assert_near(harness.render().at(center), [0.3, 0.6, 0.1], "the breath's top");
    harness.time = 2.0;
    assert_near(harness.render().at(center), [0.15, 0.3, 0.05], "its bottom");
    harness.time = 1.0;
    assert_near(harness.render().at(center), [0.225, 0.45, 0.075], "halfway");

    // The layer mask's B paints it.
    harness.texture(MASK, 4, 4, |_, _| [1.0, 1.0, 0.5, 1.0]);
    let masked = AnimeCharacterMaterial { layer_mask: Some(id(MASK)), ..glowing([0.3, 0.6, 0.1], steady) };
    harness.update(handle, &masked);
    assert_near(harness.render().at(center), [0.15, 0.3, 0.05], "× the mask's B");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn emission_blooms_as_the_worlds_light_by_its_bloom() {
    let mut harness = Harness::new();
    // Bloom for the world, none for characters: only what counts as the world's light blooms.
    harness.post = PostParams {
        bloom: BloomParams { threshold: 0.5, knee: 0.0, intensity: 1.0, scatter: 0.7 },
        character_bloom: CharacterBloom { threshold: 1.0, strength: 0.0 },
        ..passthrough()
    };
    let steady = |bloom| Emission { period: 0.0, depth: 0.0, bloom };
    let handle = harness.material(&glowing([2.0, 2.0, 2.0], steady(0.0)));
    let mesh = harness.mesh(&[sphere(Vec3::ZERO, 0.5)], &[handle]);
    harness.character(mesh, Vec3::Z, 0.0);
    // Just outside the ball's silhouette, where only bloom reaches.
    let beside = Vec3::new(-0.62, 0.0, 0.0);
    let held = harness.render().at(beside);
    assert!(held[0] < 0.01, "a character's glow kept out of bloom: {held:?}");
    harness.update(handle, &glowing([2.0, 2.0, 2.0], steady(1.0)));
    let bloomed = harness.render().at(beside);
    assert!(bloomed[0] > 0.05, "blooming as the world's light: {bloomed:?}");
}

/// A sphere character in front of a gray wall that isn't one (it glows, so the ball's shadow
/// doesn't darken it), outlined `outline` pixels wide in red, its own surface green.
fn fade_scene(harness: &mut Harness, outline: f32) -> InstanceId {
    let mut material = flat([0.0, 0.6, 0.0]);
    material.outline = Outline {
        width: outline,
        curve: OutlineCurve { near: (0.1, 1.0), mid: 50.0, far: (60.0, 1.0), cutoff: 100.0 },
        lighting: 0.0,
        albedo: 0.0,
        ..Outline::default()
    };
    material.regions[0].outline_color = [0.6, 0.0, 0.0];
    let handle = harness.material(&material);
    let ball = harness.mesh(&[sphere(Vec3::ZERO, 0.5)], &[handle]);
    let wall_material = harness.material(&with_region(flat([0.0; 3]), |region| region.emission = [0.3; 3]));
    let wall = harness.mesh(&[rect((-3.0, -3.0), (3.0, 3.0), -1.0)], &[wall_material]);
    harness.prop(wall);
    harness.character(ball, Vec3::Z, 1.0)
}

/// Asserts that every pixel within `radius` of the ball's center on screen that doesn't show its
/// green shows the wall: nothing of the ball's left where its dither cuts it out.
fn holes_show_the_wall(frame: &Frame, radius: usize) {
    for pixel in frame.around(Vec3::new(0.0, 0.0, 0.5), radius).into_iter().filter(|p| p[1] < 0.45) {
        assert_near(pixel, [0.3; 3], "the wall through a hole");
    }
}

/// Of the pixels within `radius` of the ball's center on screen, the share that show its green.
fn coverage(frame: &Frame, radius: usize) -> f32 {
    let pixels = frame.around(Vec3::new(0.0, 0.0, 0.5), radius);
    pixels.iter().filter(|p| p[1] > 0.45).count() as f32 / pixels.len() as f32
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_fading_character_keeps_its_share_of_pixels_and_its_outline_follows() {
    let mut harness = Harness::new();
    let character = fade_scene(&mut harness, 8.0);
    let whole = harness.render();
    assert_eq!(coverage(&whole, 20), 1.0);
    let builds = whole.stats.draw_list_builds;

    // Whole at 1, and exactly as before.
    harness.opacity(character, 1.0);
    let same = harness.render();
    assert_eq!(same.pixels, whole.pixels, "opacity 1 draws as before");
    assert_eq!((same.stats.faded_characters, same.stats.draw_list_builds), (0, builds));

    // Half: every 4 × 4 block keeps eight pixels; what it leaves shows the wall behind.
    harness.opacity(character, 0.5);
    let half = harness.render();
    let share = coverage(&half, 20);
    assert!((share - 0.5).abs() < 0.03, "half the pixels: {share}");
    holes_show_the_wall(&half, 20);
    assert_eq!(half.stats.faded_characters, 1);
    assert!(half.stats.faded_draws >= 1);
    assert_eq!(half.stats.draw_list_builds, builds + 1, "routed to the alpha-tested variant once");

    // The outline keeps the same share: of the pixels its whole outline draws, about half.
    let red = |frame: &Frame| -> Vec<bool> {
        let (cx, cy) = frame.pixel_of(Vec3::ZERO);
        let around = (cy - 80..cy + 80).flat_map(|y| (cx - 80..cx + 80).map(move |x| (x, y)));
        around.map(|(x, y)| frame.linear(x, y)[0] > 0.45).collect()
    };
    let (whole_lines, half_lines) = (red(&whole), red(&half));
    let lines = whole_lines.iter().filter(|&&line| line).count();
    assert!(lines > 300, "the whole outline: {lines} pixels");
    let kept = whole_lines.iter().zip(&half_lines).filter(|&(&whole, &half)| whole && half).count();
    let share = kept as f32 / lines as f32;
    assert!((share - 0.5).abs() < 0.1, "about half the outline: {share}");
    let stray = whole_lines.iter().zip(&half_lines).filter(|&(&whole, &half)| half && !whole).count();
    assert_eq!(stray, 0, "no line where the whole outline has none");

    // A quarter: whole 2 × 2 quads cut out too.
    harness.opacity(character, 0.25);
    let quarter = harness.render();
    let share = coverage(&quarter, 20);
    assert!((share - 0.25).abs() < 0.03, "a quarter of the pixels: {share}");
    holes_show_the_wall(&quarter, 20);

    // Gone at 0, and back to opaque pipelines at 1.
    harness.opacity(character, 0.0);
    assert_eq!(coverage(&harness.render(), 20), 0.0, "none of it");
    harness.opacity(character, 1.0);
    let back = harness.render();
    assert_eq!(back.pixels, whole.pixels, "whole again");
    assert_eq!(back.stats.draw_list_builds, builds + 2);
}

/// A black ball with a wide rim.
fn rimmed() -> AnimeCharacterMaterial {
    let mut material = flat([0.0; 3]);
    material.rim =
        Rim { width: 40.0, strength: 1.0, threshold: 1.0, fresnel: 1.0, light_gate: 0.0, back_light: 0.0 };
    material.outline.curve = OutlineCurve { near: (0.1, 1.0), mid: 50.0, far: (60.0, 1.0), cutoff: 100.0 };
    material
}

/// A point a few pixels in from the ball's left silhouette, in its rim (N·V at the silhouette is
/// about 1/6 from the camera's distance).
fn rim_point() -> Vec3 {
    on_sphere(0.5, Vec3::new(-0.9, 0.0, 0.436))
}

#[test]
#[ignore = "needs a GPU adapter"]
fn alpha_tested_cutouts_stay_out_of_the_character_prepass() {
    let mut harness = Harness::new();
    // The ball, and a card of the same character in front of it, just left of the rim point:
    // over where the rim looks for its depth step. The card is alpha-tested by its map.
    let ball = harness.material(&rimmed());
    let card_material = |alpha: f32, harness: &mut Harness| {
        harness.texture(CARD, 4, 4, |_, _| [1.0, 0.0, 0.0, alpha]);
        AnimeCharacterMaterial {
            base_color_map: Some(id(CARD)),
            alpha: AlphaMode::Mask { cutoff: 0.5 },
            ..flat([1.0; 3])
        }
    };
    let opaque_card = card_material(1.0, &mut harness);
    let card = harness.material(&opaque_card);
    let parts = [sphere(Vec3::ZERO, 0.5), rect((-1.2, -0.3), (-0.405, 0.3), 0.6)];
    let mesh = harness.mesh(&parts, &[ball, card]);
    harness.character(mesh, Vec3::Z, 1.0);
    // Where the card is drawn, it is the character nearer than the threshold: no rim there.
    let blocked = harness.render().at(rim_point());
    assert!(blocked[0] < 0.05, "a drawn card hides the depth step: {blocked:?}");
    // Cut out everywhere, it is in no pass: the rim finds the step behind it again.
    let clear_card = card_material(0.0, &mut harness);
    harness.update(card, &clear_card);
    let frame = harness.render();
    let rim = frame.at(rim_point());
    assert!(rim[0] > 0.2, "the rim through the card's cutout: {rim:?}");
    assert_eq!(frame.at(Vec3::new(-0.8, 0.0, 0.6)), [0.0; 3], "and nothing of the card is drawn");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn a_fading_character_takes_no_rim() {
    let mut harness = Harness::new();
    let (_, character) = ball(&mut harness, &rimmed());
    let rim_point = rim_point();
    let lit = harness.render().at(rim_point);
    assert!(lit[0] > 0.2, "the rim: {lit:?}");
    // Faded by less than any threshold reaches: every pixel kept, the rim gone.
    harness.opacity(character, 0.99);
    let faded = harness.render();
    assert_near(faded.at(rim_point), [0.0; 3], "no rim while fading");
    assert_eq!(coverage(&faded, 2), 0.0, "black everywhere else too");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn under_msaa_a_fading_character_covers_its_share_of_every_pixels_samples() {
    let mut harness = Harness::with(RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() });
    let character = fade_scene(&mut harness, 0.0);
    harness.opacity(character, 0.5);
    let frame = harness.render();
    // Two of four samples everywhere inside: a blend of the ball and the wall, no holes.
    let expected = [0.15, 0.45, 0.15];
    for pixel in frame.around(Vec3::new(0.0, 0.0, 0.5), 12) {
        assert_near(pixel, expected, "half the samples");
    }
    // At 0.3: one sample of four, or two where the dither rounds up, 0.3 of them on average.
    harness.opacity(character, 0.3);
    let frame = harness.render();
    let pixels = frame.around(Vec3::new(0.0, 0.0, 0.5), 12);
    // Green blends from the wall's 0.3 to the ball's 0.6 by the share of samples covered.
    let mean = pixels.iter().map(|p| p[1]).sum::<f32>() / pixels.len() as f32;
    let share = (mean - 0.3) / (0.6 - 0.3);
    assert!((share - 0.3).abs() < 0.04, "the samples covered: {share}");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn temporal_aa_resolves_the_fades_dither_into_a_blend() {
    let mut harness =
        Harness::with(RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() });
    let character = fade_scene(&mut harness, 0.0);
    harness.opacity(character, 0.5);
    let mut frame = harness.render();
    for _ in 0..40 {
        frame = harness.render();
    }
    // The pattern turns every frame with the jitter, which the history averages: the pixels
    // inside are near the blend, none near either color.
    let pixels = frame.around(Vec3::new(0.0, 0.0, 0.5), 12);
    let greens: Vec<f32> = pixels.iter().map(|p| p[1]).collect();
    let mean = greens.iter().sum::<f32>() / greens.len() as f32;
    let spread = greens.iter().map(|g| (g - mean).abs()).fold(0.0, f32::max);
    assert!((mean - 0.45).abs() < 0.06, "the blend's green: {mean}");
    assert!(spread < 0.12, "resolved, not a screen door: {spread}");
}
