//! Anime character shader tests that need a GPU (M1 C1): both diffuse modes, the control map,
//! the normal map, the static permutations and the vertex contract; and the character light
//! rig's per-character key light and stylized GI (M1 C2). Run with
//! `cargo test -p sapphire_render --test character -- --ignored`.
//!
//! Most scenes use no ambient light (and a black sky), no rim and no outline, so every pixel is
//! the sun's light on the lit or the shadow side, and colors can be predicted exactly.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::{MaterialId, ilm};
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, BandDiffuse, CharacterRegion, DiffuseMode, LightSplit, Material,
    Outline, RampDiffuse, RegionGi, Rim,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat, full_mip_count};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::{GpuTexture, check_format};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterFace, CharacterGi, CharacterInstance, CharacterLighting,
    ColorGrade, DirectionalLight, FramePacketRing, InstanceId, LightKind, LocalLight, MaterialHandle,
    MeshHandle, MeshInstance, Msaa, PostParams, RenderError, RenderSettings, Renderer, SkyGradient,
    ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Linear albedo of the test materials: under the tone curve's shoulder however lit.
const ALBEDO: f32 = 0.8;
/// How far a pixel's linear color may be from the prediction: 8-bit output, MSAA-free
/// interiors.
const TOLERANCE: f32 = 0.02;

/// The texture IDs materials name; each test binds its own textures to them.
const CONTROL: &str = "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b";
const RAMP: &str = "7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d";
const BASE: &str = "1d2c3b4a-5f6e-4d7c-9b8a-a1b2c3d4e5f6";
const NORMAL: &str = "a0b1c2d3-e4f5-4a6b-8c7d-9e0f1a2b3c4d";

fn id(text: &str) -> AssetId {
    text.parse().expect("a valid asset ID")
}

/// Post-processing that shows linear values below about 0.98 as they are: a tone curve that is
/// linear from black, a neutral grade, and no bloom or vignette.
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
    }
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (encoded * 255.0).round() as u8
}

fn assert_near(actual: [f32; 3], expected: [f32; 3], what: &str) {
    let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < TOLERANCE);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

fn scale(rgb: [f32; 3], by: f32) -> [f32; 3] {
    rgb.map(|c| c * by)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// A lit, outline-free test material: the split at N·L 0 with a one-pixel edge, no rim.
fn plain() -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [ALBEDO, ALBEDO, ALBEDO, 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, z_offset: 0.0 },
        ..AnimeCharacterMaterial::default()
    }
}

/// `count` regions, each with the defaults (and so its own day and night ramp rows).
fn regions(count: usize) -> Vec<CharacterRegion> {
    vec![CharacterRegion::default(); count]
}

/// One rendered frame, as linear RGB per pixel.
struct Frame {
    pixels: Vec<[u8; 4]>,
    size: (u32, u32),
    camera: CameraData,
}

impl Frame {
    fn linear(&self, x: usize, y: usize) -> [f32; 3] {
        let pixel = self.pixels[y * self.size.0 as usize + x];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (usize, usize) {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        (((x + 1.0) * 0.5 * width) as usize, ((1.0 - y) * 0.5 * height) as usize)
    }

    /// The linear color where `point` projects.
    fn at(&self, point: Vec3) -> [f32; 3] {
        let (x, y) = self.pixel_of(point);
        self.linear(x, y)
    }

    /// Where along the middle row, between columns `from` and `to`, red rises most steeply: the
    /// light split, as a pixel position.
    fn rising_edge(&self, from: usize, to: usize) -> f32 {
        let y = self.size.1 as usize / 2;
        let red = |x: usize| self.linear(x, y)[0];
        let (_, x) = (from..to - 2)
            .map(|x| (red(x + 2) - red(x), x + 1))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .expect("pixels");
        x as f32 + 0.5
    }
}

/// A renderer and everything a test frame needs, with the scene's lighting as plain fields.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    size: (u32, u32),
    camera: CameraData,
    sun: DirectionalLight,
    sky: SkyGradient,
    ambient: AmbientLight,
    night: f32,
    /// Copied into every packet: the instances lit by light rigs of their own.
    characters: Vec<CharacterInstance>,
    /// Copied into every packet.
    lights: Vec<LocalLight>,
    added: Vec<MeshInstance>,
    removed: Vec<InstanceId>,
    next_id: u64,
}

impl Harness {
    /// A square target `side` pixels wide, seen by a camera on +Z that frames the unit square
    /// (or sphere) at the origin edge to edge, and lit by a white sun along `-Z`.
    fn new(side: u32) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let settings = RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() };
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let size = (side, side);
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
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
            size,
            camera: CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.05),
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
            sky: SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO },
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            night: 0.0,
            characters: Vec::new(),
            lights: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            next_id: 0,
        };
        harness.frame_unit(1.0);
        harness
    }

    /// Frames the unit square at the origin so it spans `fraction` of the image height.
    fn frame_unit(&mut self, fraction: f32) {
        let fov = 38f32.to_radians();
        let distance = 1.0 / (fraction * (fov * 0.5).tan());
        self.camera = CameraData::look_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, Vec3::Y, fov, 1.0, 0.05);
    }

    /// Lights the scene from `toward_sun`, a vector from the surface toward the sun.
    fn sun_from(&mut self, toward_sun: Vec3) {
        self.sun.direction = -toward_sun;
    }

    fn texture(&self, desc: TextureDesc, levels: &[&[u8]], label: &str) -> GpuTexture {
        GpuTexture::upload(&self.gpu, &Texture { desc, levels: levels.to_vec() }, label).expect("texture")
    }

    /// Cooks `document` against the textures it names, bound by ID, and uploads it.
    fn material(
        &mut self,
        document: &AnimeCharacterMaterial,
        textures: &[(&str, &GpuTexture)],
    ) -> Result<MaterialHandle, RenderError> {
        let swizzles = Self::swizzles(document, textures);
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup)
    }

    /// Replaces `handle` with `document`.
    fn update(
        &mut self,
        handle: MaterialHandle,
        document: &AnimeCharacterMaterial,
        textures: &[(&str, &GpuTexture)],
    ) {
        let swizzles = Self::swizzles(document, textures);
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.update_material(handle, Material::read(&bytes).expect("read"), lookup).expect("update");
    }

    /// Each slot's swizzle: its texture's, identity where none is bound.
    fn swizzles(document: &AnimeCharacterMaterial, textures: &[(&str, &GpuTexture)]) -> Vec<Swizzle> {
        use sapphire_assets::material::MaterialDocument;
        document
            .textures()
            .iter()
            .map(|slot| {
                slot.and_then(|asked| textures.iter().find(|(name, _)| id(name) == asked))
                    .map_or(Swizzle::IDENTITY, |(_, texture)| texture.swizzle())
            })
            .collect()
    }

    /// Uploads `mesh` (one material slot) drawn with `material`, cooked and read in place.
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

    fn remove(&mut self, id: InstanceId) {
        self.removed.push(id);
    }

    /// Lights instance `id` with `lighting` from the next frame on, in place of the sun.
    fn light_rig(&mut self, id: InstanceId, lighting: CharacterLighting) {
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance { id, lighting, face: CharacterFace::default() });
    }

    /// Prepares, records and submits one frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData { camera: self.camera });
        packet.sun = self.sun;
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.post = passthrough();
        packet.day_night = self.night;
        packet.characters.extend_from_slice(&self.characters);
        packet.lights.extend_from_slice(&self.lights);
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
        Frame { pixels: self.read_back(), size: self.size, camera: self.camera }
    }

    fn read_back(&self) -> Vec<[u8; 4]> {
        let (width, height) = self.size;
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

/// `vertices` and `indices` as a mesh with one (placeholder) material slot; tests draw it with
/// a material of their own.
fn mesh_of(vertices: &[Vertex], indices: Vec<u32>) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&plain(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("surface", placeholder)];
    Mesh::from_vertices(vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// The unit square at the origin facing +Z, `u` from left to right and `v` from top to bottom,
/// with each corner's color and flags from `corner(x, y)`.
fn square(corner: impl Fn(f32, f32) -> ([u8; 3], VertexFlags)) -> Mesh {
    let vertices: Vec<Vertex> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .map(|(x, y)| {
            let (color, flags) = corner(x, y);
            Vertex {
                position: [x, y, 0.0],
                normal: [0.0, 0.0, 1.0],
                outline_normal: [0.0, 0.0, 1.0],
                uv: [(x + 1.0) * 0.5, (1.0 - y) * 0.5],
                color,
                flags,
            }
        })
        .collect();
    mesh_of(&vertices, vec![0, 1, 2, 0, 2, 3])
}

fn plain_square() -> Mesh {
    square(|_, _| ([255; 3], VertexFlags::NONE))
}

/// A UV sphere of radius 1 at the origin, each vertex's color and flags from `vertex(normal)`.
fn sphere(vertex: impl Fn(Vec3) -> ([u8; 3], VertexFlags)) -> Mesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let (color, flags) = vertex(n);
            vertices.push(Vertex {
                position: n.to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32],
                color,
                flags,
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
    mesh_of(&vertices, indices)
}

/// The point of the unit sphere facing the camera (+Z) at `x`, `y`.
fn on_sphere(x: f32, y: f32) -> Vec3 {
    Vec3::new(x, y, (1.0 - x * x - y * y).max(0.0).sqrt())
}

/// An RGBA16 float texture of linear data, `width` × `height` texels per `texel(x, y)`, with
/// `levels` mip levels, each later level filled with `mip_texel`.
fn half_texture(
    harness: &Harness,
    (width, height): (u32, u32),
    texel: impl Fn(u32, u32) -> [f32; 4],
    levels: u32,
    mip_texel: [f32; 4],
) -> GpuTexture {
    let encode = |value: [f32; 4]| value.map(|c| f16::from_f32(c).to_bits());
    let mut data: Vec<Vec<u8>> = Vec::new();
    for level in 0..levels {
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        let texels: Vec<[u16; 4]> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| if level == 0 { encode(texel(x, y)) } else { encode(mip_texel) })
            .collect();
        data.push(bytemuck::cast_slice(&texels).to_vec());
    }
    let desc = TextureDesc {
        format: TextureFormat::Rgba16Float,
        width,
        height,
        mip_level_count: levels,
        swizzle: Swizzle::IDENTITY,
    };
    let levels: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    harness.texture(desc, &levels, "half")
}

/// A control map texel: neutral but for `bias` and material `id`.
fn control_texel(bias: f32, id: u8) -> [f32; 4] {
    let mut texel = ilm::NEUTRAL;
    texel[ilm::SHADOW_BIAS] = bias;
    texel[ilm::MATERIAL_ID] = ilm::encode_material_id(MaterialId::new(id).expect("an ID"));
    texel
}

/// Ramp row colors, linear, as stored in 8-bit sRGB: day rows of IDs 0–7, then night rows.
fn ramp_rows() -> [[u8; 3]; 16] {
    std::array::from_fn(|row| {
        let k = (row % 8) as f32;
        let linear = if row < 8 {
            [0.15 + 0.1 * k, 0.75 - 0.08 * k, 0.3]
        } else {
            [0.1, 0.2 + 0.05 * k, 0.8 - 0.06 * k]
        };
        linear.map(linear_to_srgb)
    })
}

fn row_linear(row: [u8; 3]) -> [f32; 3] {
    row.map(srgb_to_linear)
}

/// A ramp texture, 4 texels wide and a texel per row, each row one color.
fn ramp_texture(harness: &Harness, rows: &[[u8; 3]; 16]) -> GpuTexture {
    let texels: Vec<u8> = rows.iter().flat_map(|&[r, g, b]| [r, g, b, 255].repeat(4)).collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 16,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    harness.texture(desc, &[&texels], "ramp")
}

/// A ramp-mode material over `regions` regions, reading `CONTROL` and `RAMP`.
fn ramp_material(region_count: usize, mix: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        control_map: Some(id(CONTROL)),
        diffuse: DiffuseMode::Ramp(RampDiffuse { map: id(RAMP), mix }),
        regions: regions(region_count),
        ..plain()
    }
}

#[test]
#[ignore = "needs a GPU"]
fn ramp_rows_follow_material_ids_and_time_of_day() {
    let mut harness = Harness::new(64);
    // Eight vertical stripes, IDs 0 to 7 left to right.
    let control = half_texture(&harness, (8, 1), |x, _| control_texel(0.5, x as u8), 1, [0.0; 4]);
    let rows = ramp_rows();
    let ramp = ramp_texture(&harness, &rows);
    let textures = [(CONTROL, &control), (RAMP, &ramp)];
    let material = harness.material(&ramp_material(8, 1.0), &textures).expect("material");
    let mesh = harness.mesh(&plain_square(), material);
    harness.add(mesh, Mat4::IDENTITY);
    // The light from behind and to the side: N·L = -0.6 everywhere, all in shadow.
    harness.sun_from(Vec3::new(0.8, 0.0, -0.6));
    let stripe = |frame: &Frame, k: usize| frame.linear(k * 8 + 4, 32);

    for (night, what) in [(0.0, "day"), (1.0, "night"), (0.5, "dusk")] {
        harness.night = night;
        let frame = harness.render();
        for k in 0..8 {
            let expected = mix(row_linear(rows[k]), row_linear(rows[8 + k]), night);
            assert_near(stripe(&frame, k), scale(expected, ALBEDO), &format!("{what}, ID {k}"));
        }
    }

    // Lit, every ID shows the albedo.
    harness.sun_from(Vec3::Z);
    let frame = harness.render();
    for k in 0..8 {
        assert_near(stripe(&frame, k), [ALBEDO; 3], &format!("lit, ID {k}"));
    }
    // Explicit rows override the IDs' own, and the subsurface tint mixes in.
    harness.sun_from(Vec3::new(0.8, 0.0, -0.6));
    harness.night = 0.0;
    let mut document = ramp_material(8, 0.3);
    document.regions[2].ramp_row = Some(5);
    document.regions[2].subsurface_color = [0.94, 0.6, 0.41];
    harness.update(material, &document, &textures);
    let frame = harness.render();
    let expected = mix([0.94, 0.6, 0.41], row_linear(rows[5]), 0.3);
    assert_near(stripe(&frame, 2), scale(expected, ALBEDO), "subsurface mix with an explicit row");
    let expected = mix([1.0; 3], row_linear(rows[3]), 0.3);
    assert_near(stripe(&frame, 3), scale(expected, ALBEDO), "default subsurface (white)");
}

#[test]
#[ignore = "needs a GPU"]
fn material_ids_are_read_unfiltered_from_level_zero() {
    let mut harness = Harness::new(64);
    let rows = ramp_rows();
    let ramp = ramp_texture(&harness, &rows);
    harness.sun_from(Vec3::new(0.8, 0.0, -0.6));
    let [id0, id7] = [0, 7].map(|k| scale(row_linear(rows[k]), ALBEDO));
    // Every pixel of the square's interior shows ID 0's or ID 7's row, never a row in between
    // or the mips' ID 5; returns how many of each.
    let count_ids = |frame: &Frame, pixels: std::ops::Range<usize>, what: &str| {
        let mut counts = [0; 2];
        for y in pixels.clone() {
            for x in pixels.clone() {
                let color = frame.linear(x, y);
                let near = |expected: [f32; 3]| color.iter().zip(expected).all(|(a, e)| (a - e).abs() < 0.04);
                let found = [near(id0), near(id7)];
                assert!(found.iter().any(|&f| f), "{what}: pixel ({x}, {y}) is {color:?}, no stored ID's");
                counts[usize::from(found[1])] += 1;
            }
        }
        counts
    };

    // Magnified, alternating texel columns: linear filtering would blend IDs between texel
    // centers. Its mips hold ID 5, which a filtered read would show at a distance.
    let alternating = |x: u32, _| control_texel(0.5, if x.is_multiple_of(2) { 0 } else { 7 });
    let control =
        half_texture(&harness, (16, 16), alternating, full_mip_count(16, 16), control_texel(0.5, 5));
    let material = harness.material(&ramp_material(8, 1.0), &[(CONTROL, &control), (RAMP, &ramp)]).unwrap();
    let mesh = harness.mesh(&plain_square(), material);
    let big = harness.add(mesh, Mat4::IDENTITY);
    let counts = count_ids(&harness.render(), 2..62, "magnified");
    assert!(counts.iter().all(|&n| n > 1000), "both IDs show: {counts:?}");
    // Minified to 8 pixels for 16 texels, a mipmapped read would land on level 1 (ID 5).
    harness.remove(big);
    let small = harness.add(mesh, Mat4::from_scale(Vec3::new(0.125, 0.125, 1.0)));
    let counts = count_ids(&harness.render(), 29..35, "minified");
    assert_eq!(counts.iter().sum::<usize>(), 36);
    harness.remove(small);

    // The same through the pipeline's ILM cook: block-compressed with box-filtered mips, whose
    // levels past 0 average the one-texel columns to ID 3 or 4. 64 texels over 24 pixels read
    // level 1 or 2.
    use sapphire_pipeline::cook::Cook;
    use sapphire_pipeline::import::image::{ChannelLayout, SourceImage};
    use sapphire_pipeline::texture::{self, IlmSettings, TextureClass, TextureSettings};
    let pixels: Vec<[f32; 4]> = (0..64 * 64).map(|i| alternating(i % 64, 0)).collect();
    let image = SourceImage::new(64, 64, ChannelLayout::Rgba, pixels).expect("image");
    let settings = TextureSettings { class: TextureClass::Ilm(IlmSettings { mips: true }) };
    for cook in [Cook::Desktop, Cook::Mobile] {
        let cooked = texture::cook(&image, &settings, cook).expect("cook");
        let cooked = cooked.texture();
        if check_format(&harness.gpu, cooked.desc.format).is_err() {
            eprintln!("{:?} isn't supported here: skipped the {} cook", cooked.desc.format, cook.name());
            continue;
        }
        assert!(cooked.desc.mip_level_count > 1, "{}: mipmapped", cook.name());
        let control = GpuTexture::upload(&harness.gpu, &cooked, "ilm").expect("upload");
        let material =
            harness.material(&ramp_material(8, 1.0), &[(CONTROL, &control), (RAMP, &ramp)]).unwrap();
        let mesh = harness.mesh(&plain_square(), material);
        let instance = harness.add(mesh, Mat4::from_scale(Vec3::new(0.375, 0.375, 1.0)));
        let counts = count_ids(&harness.render(), 22..42, cook.name());
        assert!(counts.iter().all(|&n| n > 20), "{}: both IDs show: {counts:?}", cook.name());
        harness.remove(instance);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn band_mode_draws_two_or_three_bands_in_each_regions_colors() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.6);
    // The upper hemisphere is material ID 1, the lower one ID 0, from the vertex flags.
    let upper = VertexFlags::with_material_id(MaterialId::new(1).unwrap());
    let mesh = sphere(|n| ([255; 3], if n.y >= 0.0 { upper } else { VertexFlags::NONE }));
    let shades = [[1.0, 0.5, 0.25], [0.25, 0.5, 1.0]];
    let deep_shades = [[0.5, 0.25, 0.125], [0.125, 0.25, 0.5]];
    let mut document = AnimeCharacterMaterial {
        regions: (0..2)
            .map(|k| CharacterRegion {
                shade_color: shades[k],
                deep_shade_color: deep_shades[k],
                ..CharacterRegion::default()
            })
            .collect(),
        ..plain()
    };
    let material = harness.material(&document, &[]).unwrap();
    let mesh = harness.mesh(&mesh, material);
    harness.add(mesh, Mat4::IDENTITY);
    // From the right: N·L is the sphere's x.
    harness.sun_from(Vec3::X);
    let frame = harness.render();
    for (k, y) in [(1, 0.5), (0, -0.5)] {
        assert_near(frame.at(on_sphere(0.6, y)), [ALBEDO; 3], &format!("ID {k} lit"));
        for x in [-0.25, -0.75] {
            let expected = scale(shades[k], ALBEDO);
            assert_near(frame.at(on_sphere(x, y)), expected, &format!("ID {k}, two bands at x = {x}"));
        }
    }

    // A third band below N·L -0.5.
    document.diffuse = DiffuseMode::Bands(BandDiffuse { deep_threshold: Some(-0.5) });
    harness.update(material, &document, &[]);
    let frame = harness.render();
    for (k, y) in [(1, 0.5), (0, -0.5)] {
        assert_near(frame.at(on_sphere(0.6, y)), [ALBEDO; 3], &format!("ID {k} lit"));
        assert_near(frame.at(on_sphere(-0.25, y)), scale(shades[k], ALBEDO), &format!("ID {k} first shade"));
        assert_near(
            frame.at(on_sphere(-0.75, y)),
            scale(deep_shades[k], ALBEDO),
            &format!("ID {k} deep shade"),
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_control_maps_shadow_bias_offsets_n_dot_l_and_locks_the_side() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.6);
    harness.sun_from(Vec3::X);
    let shaded = |control: Option<&str>| AnimeCharacterMaterial {
        control_map: control.map(id),
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        ..plain()
    };
    let material = harness.material(&shaded(None), &[]).unwrap();
    let mesh = harness.mesh(&sphere(|_| ([255; 3], VertexFlags::NONE)), material);
    harness.add(mesh, Mat4::IDENTITY);
    let column = |frame: &Frame, x: f32| frame.pixel_of(on_sphere(x, 0.0)).0 as f32 + 0.5;
    let frame = harness.render();
    let edge = frame.rising_edge(20, 108);
    assert!((edge - column(&frame, 0.0)).abs() < 2.0, "no bias: split at {edge}, expected the middle");

    // A bias of G adds its offset, 2·G − 1, to N·L (`ilm::decode_shadow_bias`), so with the
    // threshold at 0 the split moves to N·L = −offset: a higher G lights more.
    let with_bias = |harness: &mut Harness, bias: f32, threshold: f32| {
        let control = half_texture(harness, (4, 4), |_, _| control_texel(bias, 0), 1, [0.0; 4]);
        let mut document = shaded(Some(CONTROL));
        document.light_split.threshold = threshold;
        harness.update(material, &document, &[(CONTROL, &control)]);
        harness.render()
    };
    for byte in [153u8, 77] {
        let bias = f32::from(byte) / 255.0;
        let frame = with_bias(&mut harness, bias, 0.0);
        let offset = ilm::decode_shadow_bias(bias);
        let (expected, edge) = (column(&frame, -offset), frame.rising_edge(12, 116));
        assert!((edge - expected).abs() < 2.0, "bias {bias}: split at {edge}, expected {expected:.1}");
    }
    // G = 1 lights a point that is in shadow at 0.5 (N·L -0.3), and G = 0 shadows a lit one
    // (N·L 0.3).
    let (lit, shadow) = ([ALBEDO; 3], [0.25 * ALBEDO; 3]);
    let [neutral, lights, shadows] = [0.0, 1.0, -1.0].map(ilm::encode_shadow_bias);
    assert_eq!((neutral, lights, shadows), (0.5, 1.0, 0.0));
    let frame = with_bias(&mut harness, neutral, 0.0);
    assert_near(frame.at(on_sphere(-0.3, 0.0)), shadow, "G 0.5, N·L -0.3");
    assert_near(frame.at(on_sphere(0.3, 0.0)), lit, "G 0.5, N·L 0.3");
    assert_near(with_bias(&mut harness, lights, 0.0).at(on_sphere(-0.3, 0.0)), lit, "G 1 lights");
    assert_near(with_bias(&mut harness, shadows, 0.0).at(on_sphere(0.3, 0.0)), shadow, "G 0 shadows");

    // At the locks no light changes the side, whatever the material's threshold: here one the
    // offset alone wouldn't bring past the input's range (±1.5 ∓ 0.94 leaves ±0.56).
    for (bias, threshold, toward_sun, expected) in
        [(0.97, 1.5, -Vec3::X, ALBEDO), (0.03, -1.5, Vec3::X, 0.25 * ALBEDO)]
    {
        harness.sun_from(toward_sun);
        let control = half_texture(&harness, (4, 4), |_, _| control_texel(bias, 0), 1, [0.0; 4]);
        let mut document = shaded(Some(CONTROL));
        document.light_split.threshold = threshold;
        harness.update(material, &document, &[(CONTROL, &control)]);
        let frame = harness.render();
        for x in [0.8, -0.8] {
            assert_near(frame.at(on_sphere(x, 0.0)), [expected; 3], &format!("bias {bias} at x = {x}"));
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn normal_maps_turn_the_split_in_world_space() {
    let mut harness = Harness::new(64);
    // Normals tilted 30° toward -X in the left half and +X in the right; then toward +Y (green
    // up, glTF's convention) in the top half and -Y in the bottom.
    let tilt = |t: f32| (255.0 * (0.5 + 0.5 * t)).round() as u8;
    let sideways: Vec<u8> = (0..16).flat_map(|i| [tilt(if i % 4 < 2 { -0.5 } else { 0.5 }), 128]).collect();
    let upward: Vec<u8> = (0..16).flat_map(|i| [128, tilt(if i / 4 < 2 { 0.5 } else { -0.5 })]).collect();
    let desc = TextureDesc {
        format: TextureFormat::Rg8Unorm,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    // The split at N·L 0.25: the flat normal (N·L 0 under a grazing light) is in shadow, a normal
    // tilted toward the light (N·L 0.5) is lit.
    let document = AnimeCharacterMaterial {
        normal_map: Some(id(NORMAL)),
        light_split: LightSplit { threshold: 0.25, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        ..plain()
    };
    let (lit, shadow) = ([ALBEDO; 3], [0.25 * ALBEDO; 3]);
    let mut instance = None;
    for (map, toward_sun, lit_pixel, shadow_pixel) in
        [(&sideways, Vec3::X, (45, 32), (19, 32)), (&upward, Vec3::Y, (32, 19), (32, 45))]
    {
        let normal = harness.texture(desc, &[map], "normal");
        let material = harness.material(&document, &[(NORMAL, &normal)]).unwrap();
        let mesh = harness.mesh(&plain_square(), material);
        if let Some(previous) = instance.replace(harness.add(mesh, Mat4::IDENTITY)) {
            harness.remove(previous);
        }
        harness.sun_from(toward_sun);
        let frame = harness.render();
        assert_near(frame.linear(lit_pixel.0, lit_pixel.1), lit, &format!("tilted toward {toward_sun}"));
        assert_near(
            frame.linear(shadow_pixel.0, shadow_pixel.1),
            shadow,
            &format!("tilted away from {toward_sun}"),
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn alpha_tested_and_transparent_materials() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.6);
    let grey = 0.5;
    harness.sky =
        SkyGradient { zenith: Vec3::splat(grey), horizon: Vec3::splat(grey), ground: Vec3::splat(grey) };
    harness.ambient = AmbientLight { sky: Vec3::splat(grey), ground: Vec3::splat(grey), intensity: 0.12 };
    // Coverage 0 where the sphere's x is negative (u from 0.25 to 0.75), on its near and far
    // sides alike, and 1 elsewhere.
    let texels: Vec<u8> =
        (0..64).flat_map(|x| [255, 255, 255, if (16..48).contains(&x) { 0 } else { 255 }]).collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 64,
        height: 1,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let coverage = harness.texture(desc, &[&texels], "coverage");
    let outlined = |alpha| AnimeCharacterMaterial {
        base_color_map: Some(id(BASE)),
        alpha,
        regions: vec![CharacterRegion { outline_color: [0.0; 3], ..CharacterRegion::default() }],
        outline: Outline { width: 30.0, z_offset: 0.0 },
        ..plain()
    };
    let material = harness.material(&outlined(AlphaMode::Opaque), &[(BASE, &coverage)]).unwrap();
    let mesh = harness.mesh(&sphere(|_| ([255; 3], VertexFlags::NONE)), material);
    harness.add(mesh, Mat4::IDENTITY);
    harness.sun_from(Vec3::Z);
    // Lit head-on, plus 12% of the grey as ambient.
    let surface = ALBEDO * (1.0 + 0.12 * grey);
    let left = on_sphere(-0.5, 0.0);
    // The left silhouette's outline, just outside the sphere.
    let outline_pixel = |frame: &Frame| {
        let (x, y) = frame.pixel_of(Vec3::new(-1.0, 0.0, 0.0));
        frame.linear(x - 2, y)
    };

    // Opaque: coverage is ignored, and the outline goes all round.
    let frame = harness.render();
    assert_near(frame.at(left), [surface; 3], "opaque");
    assert!(outline_pixel(&frame)[0] < 0.1, "opaque: outlined");

    // Alpha-tested: the left half is gone, surface and hull alike, so the sky shows through.
    harness.update(material, &outlined(AlphaMode::Mask { cutoff: 0.5 }), &[(BASE, &coverage)]);
    let frame = harness.render();
    assert_eq!(harness.renderer.stats().alpha_tested_draws, 1);
    assert_near(frame.at(left), [grey; 3], "alpha-tested, cut out");
    assert_near(frame.at(on_sphere(0.5, 0.0)), [surface; 3], "alpha-tested, kept");
    assert_near(outline_pixel(&frame), [grey; 3], "alpha-tested: no outline where cut");
    let (x, y) = frame.pixel_of(Vec3::new(1.0, 0.0, 0.0));
    assert!(frame.linear(x + 2, y)[0] < 0.1, "alpha-tested: outlined where kept");

    // Transparent at half coverage: blended over the sky after it, with no outline.
    let mut half = outlined(AlphaMode::Blend);
    half.base_color_map = None;
    half.base_color[3] = 0.5;
    harness.update(material, &half, &[]);
    let frame = harness.render();
    let stats = harness.renderer.stats();
    assert_eq!((stats.transparent_draws, stats.outline_draws), (1, 0));
    assert_near(frame.at(left), [0.5 * surface + 0.5 * grey; 3], "transparent");
    assert_near(outline_pixel(&frame), [grey; 3], "transparent: no outline");
}

#[test]
#[ignore = "needs a GPU"]
fn permutations_choose_pipelines_and_passes() {
    let mut harness = Harness::new(64);
    let stats = |harness: &Harness| harness.renderer.stats();
    assert_eq!(stats(&harness).character_pipelines, 2, "opaque surface and outline, quantized meshes");
    let outlined = AnimeCharacterMaterial { outline: Outline { width: 4.0, z_offset: 0.0 }, ..plain() };
    let documents = [
        outlined.clone(),
        AnimeCharacterMaterial { alpha: AlphaMode::Mask { cutoff: 0.5 }, ..outlined.clone() },
        AnimeCharacterMaterial { alpha: AlphaMode::Blend, ..outlined.clone() },
        plain(),
    ];
    let mut handles = Vec::new();
    for (k, document) in documents.iter().enumerate() {
        let material = harness.material(document, &[]).unwrap();
        handles.push(material);
        let mesh = harness.mesh(&plain_square(), material);
        harness.add(mesh, Mat4::from_translation(Vec3::new(k as f32 - 1.5, 0.0, -2.0)));
    }
    // Alpha-tested adds a surface and an outline pipeline; transparent a surface alone.
    assert_eq!(stats(&harness).character_pipelines, 5);
    harness.render();
    let s = stats(&harness);
    assert_eq!((s.draws, s.alpha_tested_draws, s.transparent_draws, s.outline_draws), (4, 1, 1, 2));
    let builds = s.draw_list_builds;

    // Color edits keep the permutation and the draw list.
    let mut recolored = documents[0].clone();
    recolored.base_color = [0.2, 0.4, 0.6, 1.0];
    harness.update(handles[0], &recolored, &[]);
    harness.render();
    assert_eq!(stats(&harness).draw_list_builds, builds);
    // A new permutation moves the draw: the list is rebuilt once.
    harness.update(
        handles[3],
        &AnimeCharacterMaterial { alpha: AlphaMode::Mask { cutoff: 0.5 }, ..plain() },
        &[],
    );
    harness.render();
    let s = stats(&harness);
    assert_eq!((s.draws, s.alpha_tested_draws, s.outline_draws), (4, 2, 2));
    assert_eq!(s.draw_list_builds, builds + 1);
    assert_eq!(s.character_pipelines, 5, "the variant was built already");
}

#[test]
#[ignore = "needs a GPU"]
fn vertex_colors_carry_occlusion_outline_width_and_z_offset() {
    // Occlusion: the square lit head-on (N·L 1), its AO 0 at the left edge and 1 at the right.
    // In half-Lambert space that is N·L 2·AO − 1, so the split falls where AO is 0.5: the
    // middle.
    let mut harness = Harness::new(64);
    let shaded = AnimeCharacterMaterial {
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        ..plain()
    };
    let material = harness.material(&shaded, &[]).unwrap();
    let occluded_left = square(|x, _| ([if x < 0.0 { 0 } else { 255 }, 255, 255], VertexFlags::NONE));
    let mesh = harness.mesh(&occluded_left, material);
    let instance = harness.add(mesh, Mat4::IDENTITY);
    let frame = harness.render();
    let edge = frame.rising_edge(4, 60);
    assert!((edge - 32.0).abs() < 1.5, "AO split at {edge}, expected the middle");
    assert_near(frame.linear(8, 32), [0.25 * ALBEDO; 3], "occluded");
    assert_near(frame.linear(56, 32), [ALBEDO; 3], "open");
    harness.remove(instance);

    // Outline width: G scales the material's.
    let mut harness = Harness::new(256);
    harness.frame_unit(0.6);
    let grey = 0.5;
    harness.sky =
        SkyGradient { zenith: Vec3::splat(grey), horizon: Vec3::splat(grey), ground: Vec3::splat(grey) };
    harness.ambient = AmbientLight { sky: Vec3::splat(grey), ground: Vec3::splat(grey), intensity: 0.12 };
    let outlined = AnimeCharacterMaterial {
        base_color: [grey / 1.06, grey / 1.06, grey / 1.06, 1.0],
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        outline: Outline { width: 40.0, z_offset: 0.0 },
        ..plain()
    };
    let material = harness.material(&outlined, &[]).unwrap();
    let mut widths = Vec::new();
    let mut previous = None;
    for g in [255u8, 128] {
        let mesh = harness.mesh(&sphere(|_| ([255, g, 255], VertexFlags::NONE)), material);
        if let Some(previous) = previous.replace(harness.add(mesh, Mat4::IDENTITY)) {
            harness.remove(previous);
        }
        let frame = harness.render();
        // Darkness against the equal-grey sphere and sky is outline coverage.
        let y = frame.size.1 as usize / 2;
        let width: f32 = (0..frame.size.0 as usize / 2)
            .map(|x| (1.0 - frame.linear(x, y)[0] / grey).clamp(0.0, 1.0))
            .sum();
        widths.push(width);
    }
    let expected = 40.0 * 256.0 / 1080.0;
    assert!((widths[0] - expected).abs() < 0.75, "full width {:.2}, expected {expected:.1}", widths[0]);
    let half = expected * 128.0 / 255.0;
    assert!((widths[1] - half).abs() < 0.75, "G = 128: width {:.2}, expected {half:.1}", widths[1]);

    // Z offset: B scales the material's push away from the camera. A small sphere in front of a
    // wall: its hull, pushed past the wall, disappears behind it.
    let mut harness = Harness::new(128);
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.8, 1.0, 0.05);
    harness.sun_from(Vec3::Z);
    let wall = harness.material(&plain(), &[]).unwrap();
    let wall = harness.mesh(&plain_square(), wall);
    harness.add(wall, Mat4::from_scale(Vec3::new(3.0, 3.0, 1.0)));
    let mut dark_pixels = Vec::new();
    let mut previous = None;
    for (z_offset, b) in [(0.0, 255u8), (1.0, 255), (1.0, 0)] {
        let document = AnimeCharacterMaterial {
            regions: vec![CharacterRegion { outline_color: [0.0; 3], ..CharacterRegion::default() }],
            outline: Outline { width: 30.0, z_offset },
            ..plain()
        };
        let material = harness.material(&document, &[]).unwrap();
        let mesh = harness.mesh(&sphere(|_| ([255, 255, b], VertexFlags::NONE)), material);
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.4),
            Default::default(),
            Vec3::new(0.0, 0.0, 0.45),
        );
        if let Some(previous) = previous.replace(harness.add(mesh, transform)) {
            harness.remove(previous);
        }
        let frame = harness.render();
        let y = frame.size.1 as usize / 2;
        dark_pixels.push((0..frame.size.0 as usize).filter(|&x| frame.linear(x, y)[0] < 0.1).count());
    }
    assert!(dark_pixels[0] >= 4, "the hull shows over the wall: {dark_pixels:?}");
    assert_eq!(dark_pixels[1], 0, "pushed behind the wall: {dark_pixels:?}");
    assert_eq!(dark_pixels[2], dark_pixels[0], "B = 0 doesn't push: {dark_pixels:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn data_slots_refuse_srgb_textures() {
    let mut harness = Harness::new(16);
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 1,
        height: 1,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let srgb = harness.texture(desc, &[&[128, 128, 128, 0]], "srgb");
    let document = AnimeCharacterMaterial { control_map: Some(id(CONTROL)), ..plain() };
    let error = harness.material(&document, &[(CONTROL, &srgb)]).unwrap_err();
    assert!(matches!(error, RenderError::SrgbData { slot: "control", .. }), "{error}");
    // Color slots take it.
    let document = AnimeCharacterMaterial { base_color_map: Some(id(BASE)), ..plain() };
    harness.material(&document, &[(BASE, &srgb)]).expect("a color texture");
}

// --- The character light rig (M1 C2): each character's key light and stylized GI.

/// Lighting by a key light traveling along `direction` with `radiance`, and `gi`.
fn rig(direction: Vec3, radiance: Vec3, gi: CharacterGi) -> CharacterLighting {
    CharacterLighting {
        key: DirectionalLight { direction, color: radiance, intensity: 1.0 },
        gi,
        ..CharacterLighting::unstyled(DirectionalLight::default())
    }
}

/// No key light: only the GI shows.
fn gi_only(gi: CharacterGi) -> CharacterLighting {
    rig(Vec3::NEG_Z, Vec3::ZERO, gi)
}

/// A plain material whose one region takes `gi`.
fn gi_material(gi: RegionGi) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial { regions: vec![CharacterRegion { gi, ..CharacterRegion::default() }], ..plain() }
}

/// White ambient light from above and none from below: a surface facing `n` receives
/// `0.5 + 0.5 n.y`.
fn sky_above() -> AmbientLight {
    AmbientLight { sky: Vec3::ONE, ground: Vec3::ZERO, intensity: 1.0 }
}

/// What [`sky_above`] gives a surface facing `n` (not necessarily unit).
fn sky_above_at(n: Vec3) -> f32 {
    0.5 + 0.5 * n.normalize().y
}

#[test]
#[ignore = "needs a GPU"]
fn each_character_takes_its_own_key_light() {
    let mut harness = Harness::new(96);
    harness.frame_unit(0.4);
    let material = harness.material(&plain(), &[]).unwrap();
    let mesh = harness.mesh(&plain_square(), material);
    let at = |x: f32| {
        Mat4::from_scale_rotation_translation(Vec3::splat(0.8), Default::default(), Vec3::new(x, 0.0, 0.0))
    };
    let (left, right) = (Vec3::new(-1.2, 0.0, 0.0), Vec3::new(1.2, 0.0, 0.0));
    let world = harness.add(mesh, at(left.x));
    let character = harness.add(mesh, at(right.x));
    // The world's sun from the front, half strength.
    harness.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.5 };

    // Unlisted, both are lit by the sun.
    let frame = harness.render();
    for point in [left, right] {
        assert_near(frame.at(point), [0.5 * ALBEDO; 3], "the sun");
    }
    // The character's own key: red, from the front. The world's square keeps the sun.
    let red = Vec3::new(0.9, 0.3, 0.1);
    harness.light_rig(character, rig(Vec3::NEG_Z, red, CharacterGi::default()));
    let frame = harness.render();
    assert_near(frame.at(left), [0.5 * ALBEDO; 3], "unlisted: still the sun");
    assert_near(frame.at(right), scale(red.to_array(), ALBEDO), "lit by its own key");
    assert_eq!(harness.renderer.stats().characters, 1);
    // From behind, the character's key puts it in its authored shade while the sun lights it.
    harness.light_rig(character, rig(Vec3::Z, red, CharacterGi::default()));
    let frame = harness.render();
    let shade = CharacterRegion::default().shade_color;
    let expected: [f32; 3] = std::array::from_fn(|c| ALBEDO * shade[c] * red[c]);
    assert_near(frame.at(right), expected, "in the shade of its own key");
    // A rig for an instance the renderer doesn't draw is ignored, and counted.
    harness.light_rig(InstanceId(99), rig(Vec3::Z, red, CharacterGi::default()));
    harness.render();
    assert_eq!((harness.renderer.stats().characters, harness.renderer.stats().ignored_characters), (1, 1));
    // Dropped from the list, the character goes back to the sun.
    harness.characters.clear();
    let frame = harness.render();
    assert_near(frame.at(right), [0.5 * ALBEDO; 3], "back to the sun");
    harness.remove(world);
}

#[test]
#[ignore = "needs a GPU"]
fn the_rim_takes_the_key_lights_color_and_the_rigs_scale() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.8);
    let rimmed = AnimeCharacterMaterial { rim: Rim { width: 0.3, strength: 0.6 }, ..plain() };
    let material = harness.material(&rimmed, &[]).unwrap();
    let mesh = harness.mesh(&sphere(|_| ([255; 3], VertexFlags::NONE)), material);
    let id = harness.add(mesh, Mat4::IDENTITY);
    // Lit from the front by a dim key: the rim brightens the lit silhouette.
    let key = Vec3::splat(0.3);
    let edge = on_sphere(0.97, 0.0);
    let center = Vec3::Z;
    let mut rim_at = |scale: f32| {
        harness
            .light_rig(id, CharacterLighting { rim: scale, ..rig(Vec3::NEG_Z, key, CharacterGi::default()) });
        let frame = harness.render();
        (frame.at(edge)[0], frame.at(center)[0])
    };
    let (none, center_none) = rim_at(0.0);
    let (full, center_full) = rim_at(1.0);
    let (double, _) = rim_at(2.0);
    assert!((center_none - center_full).abs() < 0.01, "no rim at the center");
    assert!(full > none + 0.02, "the rim shows: {none} then {full}");
    assert!(((double - none) - 2.0 * (full - none)).abs() < 0.02, "scaled: {none}, {full}, {double}");
}

#[test]
#[ignore = "needs a GPU"]
fn gi_normals_turn_toward_the_head_by_the_regions_sphere_weight() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.8);
    harness.ambient = sky_above();
    // The head below and behind the square, which faces +Z: spherized, the square's GI normals
    // tilt up, more toward its top.
    let head = Vec3::new(0.0, -1.0, -1.0);
    let points = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.6, 0.0),
        Vec3::new(0.0, -0.6, 0.0),
        Vec3::new(0.6, 0.3, 0.0),
    ];
    let mut previous = None;
    for weight in [0.0, 0.5, 1.0] {
        let material = harness.material(&gi_material(RegionGi { sphere: weight, skin: 0.0 }), &[]).unwrap();
        let mesh = harness.mesh(&plain_square(), material);
        let id = harness.add(mesh, Mat4::IDENTITY);
        if let Some(previous) = previous.replace(id) {
            harness.remove(previous);
        }
        harness.light_rig(id, gi_only(CharacterGi { head: Some(head), ..CharacterGi::default() }));
        let frame = harness.render();
        for point in points {
            let sphere = (point - head).normalize();
            let normal = (Vec3::Z * (1.0 - weight) + sphere * weight).normalize();
            let expected = ALBEDO * sky_above_at(normal);
            assert_near(frame.at(point), [expected; 3], &format!("weight {weight} at {point}"));
        }
    }
    // Without a head (unlisted instances' unstyled GI), the region's weight changes nothing.
    harness.light_rig(previous.expect("a square"), gi_only(CharacterGi::default()));
    let frame = harness.render();
    for point in points {
        assert_near(frame.at(point), [ALBEDO * 0.5; 3], &format!("no head, at {point}"));
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gi_normals_bend_toward_the_camera() {
    let mut harness = Harness::new(128);
    harness.frame_unit(0.9);
    harness.ambient = sky_above();
    let material = harness.material(&plain(), &[]).unwrap();
    let mesh = harness.mesh(&sphere(|_| ([255; 3], VertexFlags::NONE)), material);
    let id = harness.add(mesh, Mat4::IDENTITY);
    let eye = harness.camera.position;
    for bias in [0.0, 0.5, 1.0] {
        harness.light_rig(id, gi_only(CharacterGi { camera_bias: bias, ..CharacterGi::default() }));
        let frame = harness.render();
        for point in [on_sphere(0.0, -0.7), on_sphere(0.0, 0.7), on_sphere(-0.6, -0.2)] {
            let view = (eye - point).normalize();
            let expected = ALBEDO * sky_above_at(point + view * bias);
            assert_near(frame.at(point), [expected; 3], &format!("bias {bias} at {point}"));
        }
    }
}

/// HSV hue of linear RGB, in sixths of the circle.
fn hue([r, g, b]: [f32; 3]) -> f32 {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let chroma = max - min;
    if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_gi_clamp_holds_saturation_and_value_and_keeps_the_hue() {
    let mut harness = Harness::new(64);
    // A saturated orange ambient from every side.
    let orange = Vec3::new(0.8, 0.2, 0.1);
    harness.ambient = AmbientLight { sky: orange, ground: orange, intensity: 1.0 };
    let material = harness.material(&plain(), &[]).unwrap();
    let mesh = harness.mesh(&plain_square(), material);
    let id = harness.add(mesh, Mat4::IDENTITY);
    // Saturation 0.875 and value 0.8 before the clamp.
    let cases = [
        (1.0, f32::INFINITY, [0.8, 0.2, 0.1]),
        // Saturation to 0.5: the channels move toward the value, hue kept.
        (0.5, f32::INFINITY, [0.8, 0.8 - 0.6 * 0.5 / 0.875, 0.8 - 0.7 * 0.5 / 0.875]),
        // Value to 0.4: the whole color halves.
        (1.0, 0.4, [0.4, 0.1, 0.05]),
        (0.5, 0.4, [0.4, 0.4 - 0.3 * 0.5 / 0.875, 0.4 - 0.35 * 0.5 / 0.875]),
    ];
    for (max_saturation, max_value, gi) in cases {
        let lighting = gi_only(CharacterGi { max_saturation, max_value, ..CharacterGi::default() });
        harness.light_rig(id, lighting);
        let color = harness.render().linear(32, 32);
        let what = format!("saturation ≤ {max_saturation}, value ≤ {max_value}");
        assert_near(color, scale(gi, ALBEDO), &what);
        assert!((hue(color) - hue(orange.to_array())).abs() < 0.05, "{what}: hue {}", hue(color));
    }
    // An ambient within both ceilings comes back as it is.
    let pastel = Vec3::new(0.4, 0.35, 0.3);
    harness.ambient = AmbientLight { sky: pastel, ground: pastel, intensity: 1.0 };
    harness.light_rig(
        id,
        gi_only(CharacterGi { max_saturation: 0.5, max_value: 0.5, ..CharacterGi::default() }),
    );
    assert_near(harness.render().linear(32, 32), scale(pastel.to_array(), ALBEDO), "within the ceilings");
}

#[test]
#[ignore = "needs a GPU"]
fn skin_takes_the_skin_tint_and_occlusion_and_intensity_scale_the_gi() {
    let mut harness = Harness::new(64);
    harness.ambient = AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 1.0 };
    let tint = Vec3::new(1.2, 0.8, 0.5);
    let mut previous = None;
    let cases = [(0.0, 255, 1.0), (1.0, 255, 1.0), (0.5, 255, 1.0), (1.0, 128, 1.0), (0.0, 255, 0.5)];
    for (skin, occlusion, intensity) in cases {
        let material = harness.material(&gi_material(RegionGi { sphere: 0.0, skin }), &[]).unwrap();
        let mesh = harness.mesh(&square(|_, _| ([occlusion, 255, 255], VertexFlags::NONE)), material);
        let id = harness.add(mesh, Mat4::IDENTITY);
        if let Some(previous) = previous.replace(id) {
            harness.remove(previous);
        }
        harness.light_rig(id, gi_only(CharacterGi { skin_tint: tint, intensity, ..CharacterGi::default() }));
        let ao = f32::from(occlusion) / 255.0;
        let expected: [f32; 3] =
            std::array::from_fn(|c| ALBEDO * 0.5 * intensity * ao * (1.0 + (tint[c] - 1.0) * skin));
        let what = format!("skin {skin}, occlusion {ao:.2}, intensity {intensity}");
        assert_near(harness.render().linear(32, 32), expected, &what);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn local_lights_keep_their_light_under_the_rigs_ceiling() {
    let mut harness = Harness::new(64);
    let material = harness.material(&plain(), &[]).unwrap();
    let mesh = harness.mesh(&plain_square(), material);
    let id = harness.add(mesh, Mat4::IDENTITY);
    harness.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.1 };
    let lamp = LocalLight {
        position: Vec3::new(0.0, 0.0, 0.5),
        color: Vec3::ONE,
        intensity: 1.0,
        range: 3.0,
        kind: LightKind::Point,
    };
    let points = [Vec3::ZERO, Vec3::new(0.5, 0.5, 0.0), Vec3::new(-0.7, 0.2, 0.0)];
    // What the lamp adds at each point: with it, less without it.
    let added = |harness: &mut Harness| {
        harness.lights = vec![lamp];
        let lit = harness.render();
        harness.lights.clear();
        let unlit = harness.render();
        points.map(|point| {
            let (with, without) = (lit.at(point), unlit.at(point));
            [0, 1, 2].map(|c| with[c] - without[c])
        })
    };
    let by_the_sun = added(&mut harness);
    // A rig with a red key from behind and its own GI: the lamp adds the same light.
    let lighting =
        rig(Vec3::Z, Vec3::new(0.3, 0.0, 0.0), CharacterGi { camera_bias: 1.0, ..CharacterGi::default() });
    harness.light_rig(id, lighting);
    let by_the_rig = added(&mut harness);
    for (point, (sun, rig)) in points.iter().zip(by_the_sun.iter().zip(&by_the_rig)) {
        assert_near(*rig, *sun, &format!("the lamp at {point}, rigged and not"));
        assert!(sun[0] > 0.1, "the lamp lights {point}");
    }
    // A lower ceiling holds the lamp's light to it: at the center the lamp's luminance is its
    // range window.
    harness.light_rig(id, CharacterLighting { local_light_ceiling: 0.25, ..lighting });
    let held = added(&mut harness);
    let window = (1.0f32 - 0.25 / 9.0).powi(2);
    assert_near(held[0], scale(by_the_rig[0], 0.25 / window), "held under the ceiling");
}
