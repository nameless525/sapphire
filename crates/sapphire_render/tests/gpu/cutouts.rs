//! Cutouts over a background that isn't black (M1, the Metal discard audit): on every shipped
//! tier, the holes an alpha test cuts in an environment surface and in a foliage card show what
//! lies behind them, and foliage's scene prepass cuts the same holes. Apple's Metal compiler has
//! miscompiled a fragment stage's per-pixel `discard`, writing the pixels it cut black, with their
//! depth (docs/conventions.md, "Fragment stages that cut pixels out"); the other GPU tests draw
//! their cutouts over a black sky, where such a hole passes. Run with
//! `cargo test -p sapphire_render --test gpu cutouts:: -- --ignored`.
//!
//! The scene: a red card speckled with holes (its map's alpha 0 or 1 per texel, under two pixels
//! a texel, so the holes' edges split 2 × 2 quads) before a green wall on the left and the blue
//! sky. The card is emissive or lit head-on, the wall emissive, and post shows linear values as
//! they are, so each pixel is the card, the wall or the sky, or under MSAA and temporal AA a
//! blend of them: never dark.

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, FoliageGradient,
    FoliageMaterial, Material, MaterialDocument, Terminator, Translucency, Wind,
};
use sapphire_assets::mesh::{
    FoliageVertex, MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::TierTable;
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    LightKind, LocalLight, MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings,
    Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// The card map's ID.
const CARD: &str = "2f6d8a4c-1b3e-4c5d-9e7f-8a9b0c1d2e3f";
/// Texels along each side of the card map: prime, so its texels never line up with pixels.
const CARD_TEXELS: u32 = 61;
/// The card's color, the wall's and the sky's, linear.
const CARD_COLOR: Vec3 = Vec3::new(0.9, 0.1, 0.1);
const WALL_COLOR: Vec3 = Vec3::new(0.1, 0.8, 0.1);
const SKY_COLOR: Vec3 = Vec3::new(0.0, 0.0, 1.0);
/// The darkest sum of a pixel's linear channels a blend of the three can have, less 8-bit
/// rounding: a sample written black takes a pixel below it.
const DARKEST: f32 = 0.9;
/// The camera's near plane, and a depth between the card's (2 m from the camera) and the wall's
/// (3 m): reverse-Z with an infinite far plane, depth is near / distance.
const NEAR: f32 = 0.05;
const BETWEEN_DEPTHS: f32 = NEAR / 2.5;

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

/// A square in the plane z = `z` facing +z, centered at (`x`, 0), `half` from its center to each
/// edge.
fn square(x: f32, z: f32, half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let corner = |s: f32, t: f32| {
        let uv = [(s + 1.0) * 0.5, (1.0 - t) * 0.5];
        vertex(Vec3::new(x + s * half, t * half, z), Vec3::Z, uv)
    };
    (vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], vec![0, 1, 2, 0, 2, 3])
}

/// A mesh of one part, with f32 positions, and still foliage data where `foliage`.
fn mesh((vertices, indices): (Vec<Vertex>, Vec<u32>), foliage: bool) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("part", placeholder)];
    let count = vertices.len();
    let mut mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32);
    if foliage {
        let still = FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        mesh.streams = mesh.streams.clone().with_foliage(&vec![FoliageVertex::decode(still.encode()); count]);
    }
    mesh
}

/// The card map: white, its alpha 0 or 1 per texel by a hash, about half of each.
fn speckled(gpu: &GpuContext) -> GpuTexture {
    let texels: Vec<[u16; 4]> = (0..CARD_TEXELS * CARD_TEXELS)
        .map(|i| {
            let mut h = i.wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995;
            h ^= h >> 15;
            h = h.wrapping_mul(0x2c1b_3c6d);
            h ^= h >> 12;
            let alpha = if h & 1 == 0 { 1.0 } else { 0.0 };
            [1.0, 1.0, 1.0, alpha].map(|c: f32| f16::from_f32(c).to_bits())
        })
        .collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba16Float,
        width: CARD_TEXELS,
        height: CARD_TEXELS,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let bytes: &[u8] = bytemuck::cast_slice(&texels);
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![bytes] }, "card").expect("texture")
}

/// Cooks `document` against the textures it names, bound by ID, as the pipeline would.
fn cook<D: MaterialDocument>(document: &D, textures: &[(&str, &GpuTexture)]) -> Vec<u8> {
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

/// Uploads cooked material `bytes`, its textures bound by ID.
fn upload_material(
    renderer: &mut Renderer,
    bytes: &[u8],
    textures: &[(&str, &GpuTexture)],
) -> MaterialHandle {
    let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
    renderer.upload_material(Material::read(bytes).expect("read"), lookup).expect("upload")
}

/// Uploads `mesh`, its one slot drawn with `material`.
fn upload_mesh(renderer: &mut Renderer, mesh: &Mesh, material: MaterialHandle) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
}

/// An unlit environment material of `color`, alpha-tested by the card map where `cut`.
fn emissive(color: Vec3, cut: bool) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        base_color_map: cut.then(|| id(CARD)),
        alpha: if cut { AlphaMode::Mask { cutoff: 0.5 } } else { AlphaMode::Opaque },
        specular_clamp: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        emission: color.to_array(),
        ..EnvironmentMaterial::default()
    }
}

/// A foliage card of the card's color, lit plainly, alpha-tested by the card map.
fn leaves() -> FoliageMaterial {
    FoliageMaterial {
        base_color: CARD_COLOR.extend(1.0).to_array(),
        base_color_map: Some(id(CARD)),
        alpha_cutoff: 0.5,
        wrap: 0.0,
        terminator_width: 0.0,
        terminator_growth: 0.0,
        shadow_tint: [1.0; 3],
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: [1.0; 3], tip: [1.0; 3] },
        wind: Wind { sway: 0.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..FoliageMaterial::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Card {
    Environment,
    Foliage,
}

/// One rendered frame, and the scene prepass's depth.
struct Frame {
    pixels: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl Frame {
    fn linear(&self, x: u32, y: u32) -> Vec3 {
        let pixel = self.pixels[(y * SIDE + x) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// Whether the card shows at `(x, y)`, more than anything behind it.
    fn card(&self, x: u32, y: u32) -> bool {
        let pixel = self.linear(x, y);
        pixel.x > pixel.y.max(pixel.z)
    }
}

/// The scene under `settings`: the card, the wall and the sky, drawn `frames` times.
fn render(settings: RenderSettings, card: Card, frames: u32) -> Frame {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
    let map = speckled(&gpu);
    let textures = [(CARD, &map)];
    let wall = upload_material(&mut renderer, &cook(&emissive(WALL_COLOR, false), &[]), &[]);
    let card_bytes = match card {
        Card::Environment => cook(&emissive(CARD_COLOR, true), &textures),
        Card::Foliage => cook(&leaves(), &textures),
    };
    let card_material = upload_material(&mut renderer, &card_bytes, &textures);
    let wall = upload_mesh(&mut renderer, &mesh(square(-1.0, -1.0, 1.0), false), wall);
    let card =
        upload_mesh(&mut renderer, &mesh(square(0.0, 0.0, 0.75), card == Card::Foliage), card_material);
    let mut added = vec![
        MeshInstance { id: InstanceId(0), mesh: wall, transform: Mat4::IDENTITY },
        MeshInstance { id: InstanceId(1), mesh: card, transform: Mat4::IDENTITY },
    ];

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
    let view = target.create_view(&Default::default());
    let colors = Readback::new(&gpu, 4);
    let depths = Readback::new(&gpu, 4);
    let mut executor = GraphExecutor::new();
    let mut packets = FramePacketRing::new();
    for _ in 0..frames {
        let packet = packets.begin();
        packet.views.push(ViewData::new(CameraData::look_at(
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::ZERO,
            Vec3::Y,
            0.9,
            1.0,
            NEAR,
        )));
        // Head-on: the foliage card lit by the sun at full strength, and a dim lamp in front so
        // the local lights' branch runs.
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.lights.push(LocalLight {
            position: Vec3::new(0.3, 0.2, 0.6),
            color: Vec3::ONE,
            intensity: 0.05,
            range: 2.0,
            kind: LightKind::Point,
        });
        packet.sky = SkyGradient { zenith: SKY_COLOR, horizon: SKY_COLOR, ground: SKY_COLOR };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.shadow_tint = Vec3::ONE;
        packet.post = passthrough();
        packet.instances.changed.append(&mut added);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.prepare(&gpu, packets.current());
        let mut graph = FrameGraph::new();
        let output = graph.import_texture("target", &view);
        let targets = renderer.record(&mut graph, output).expect("a view and a target");
        let depth = targets.depth;
        let depths = &depths;
        graph.add_pass("read back").side_effect().read(depth).record(move |ctx| {
            depths.copy(ctx.encoder, ctx.texture(depth));
        });
        executor.execute(&gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    colors.copy(&mut encoder, &target);
    gpu.queue().submit([encoder.finish()]);
    Frame {
        pixels: colors.texels(&gpu).into_iter().map(|t| [t[0], t[1], t[2], t[3]]).collect(),
        depth: depths
            .texels(&gpu)
            .into_iter()
            .map(|t| f32::from_le_bytes([t[0], t[1], t[2], t[3]]))
            .collect(),
    }
}

/// A buffer a `SIDE` × `SIDE` texture of `texel` bytes per texel is copied into, row by row.
struct Readback {
    buffer: wgpu::Buffer,
    texel: u32,
    padded: u32,
}

impl Readback {
    fn new(gpu: &GpuContext, texel: u32) -> Self {
        let padded = (SIDE * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIDE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, texel, padded }
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        let aspect = if texture.format().is_depth_stencil_format() {
            wgpu::TextureAspect::DepthOnly
        } else {
            wgpu::TextureAspect::All
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { aspect, ..texture.as_image_copy() },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded),
                    rows_per_image: Some(SIDE),
                },
            },
            texture.size(),
        );
    }

    /// Each texel's bytes, rows top to bottom.
    fn texels(&self, gpu: &GpuContext) -> Vec<Vec<u8>> {
        self.buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let texels = {
            let mapped = self.buffer.get_mapped_range(..).expect("mapped");
            mapped
                .chunks_exact(self.padded as usize)
                .flat_map(|row| row[..(SIDE * self.texel) as usize].chunks_exact(self.texel as usize))
                .map(<[u8]>::to_vec)
                .collect()
        };
        self.buffer.unmap();
        texels
    }
}

/// Every shipped tier's settings by name, and both kinds of drawing without antialiasing.
fn tiers() -> Vec<(String, RenderSettings)> {
    let mut tiers: Vec<(String, RenderSettings)> = TierTable::shipped()
        .tiers()
        .iter()
        .map(|tier| (format!("{:?}", tier.id), tier.render_settings()))
        .collect();
    tiers.push(("no antialiasing".into(), RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() }));
    tiers
}

/// Asserts that no pixel of `frame` is darker than a blend of the card, the wall and the sky, and
/// that about half of the card's middle shows through it.
fn holes_show_what_lies_behind(frame: &Frame, what: &str) {
    for y in 0..SIDE {
        for x in 0..SIDE {
            let pixel = frame.linear(x, y);
            let sum = pixel.x + pixel.y + pixel.z;
            assert!(sum >= DARKEST, "{what}: ({x}, {y}) is {pixel}, darker than the scene");
        }
    }
    // The card's middle: it spans ±0.75 of the ±0.97 the view sees at its distance.
    let (low, high) = (SIDE / 5, SIDE * 4 / 5);
    let middle = (high - low) * (high - low);
    let shown =
        (low..high).flat_map(|y| (low..high).map(move |x| (x, y))).filter(|&(x, y)| frame.card(x, y)).count();
    let share = shown as f32 / middle as f32;
    assert!((0.3..0.7).contains(&share), "{what}: the card shows at {share} of its middle");
}

#[test]
#[ignore = "needs a GPU"]
fn holes_in_alpha_tested_environment_surfaces_show_what_lies_behind() {
    for (tier, settings) in tiers() {
        let frames = if settings.temporal { 4 } else { 1 };
        let frame = render(settings, Card::Environment, frames);
        holes_show_what_lies_behind(&frame, &format!("{tier}: environment"));
    }
}

#[test]
#[ignore = "needs a GPU"]
fn holes_in_foliage_cards_show_what_lies_behind_and_the_prepass_cuts_them_too() {
    for (tier, settings) in tiers() {
        let frames = if settings.temporal { 4 } else { 1 };
        let frame = render(settings, Card::Foliage, frames);
        holes_show_what_lies_behind(&frame, &format!("{tier}: foliage"));
    }
    // Without antialiasing every pixel is the card or what lies behind it, and the scene prepass
    // holds the card's depth exactly where the main pass draws the card.
    let settings = RenderSettings { msaa: Msaa::Off, temporal: false, ..RenderSettings::default() };
    let frame = render(settings, Card::Foliage, 1);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let at_card = frame.depth[(y * SIDE + x) as usize] > BETWEEN_DEPTHS;
            assert_eq!(at_card, frame.card(x, y), "({x}, {y}): the prepass and the main pass disagree");
        }
    }
}
