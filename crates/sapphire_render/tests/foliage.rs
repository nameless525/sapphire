//! Foliage tests that need a GPU (M1 W6): the wind moving cards in the main pass and the scene
//! prepass alike, as the CPU mirror says, with velocity that follows it; alpha-to-coverage under
//! MSAA where the tiers ask for it and a sharp alpha test elsewhere; shadow casters that cut cards
//! out; the scene prepass cutting cards out exactly as the main pass does; the frame's fog over
//! foliage as over the world (M1 W3); the meshes foliage refuses; and the counters. Run with
//! `cargo test -p sapphire_render --test foliage -- --ignored`.
//!
//! Scenes have a black sky, no ambient light and post that shows linear values as they are, so a
//! drawn pixel is lit and the sky stays black.

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, FoliageGradient, FoliageMaterial,
    Material, MaterialDocument, Terminator, Translucency, Wind as WindResponse,
};
use sapphire_assets::mesh::{
    FoliageVertex, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Submesh,
    VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::foliage::wind_offset;
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::quality::TierTable;
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, Fog, FogLayer, FoliageSettings,
    FramePacketRing, HeightFog, InstanceId, MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams,
    PrepassTargets, RenderError, RenderSettings, Renderer, ShadowSettings, SkyGradient, ToneCurve, ViewData,
    Vignette, Wind,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// The card map's ID, bound to each test's texture.
const CARD: &str = "5b0e9c1d-7a2f-4e3b-9c4d-1e2f3a4b5c6d";

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

/// A flat foliage material: `grey`, lit plainly by the key light (no wrap, a hard terminator),
/// with no canopy occlusion, gradient, translucency or distance fade, and `wind`'s response.
fn flat(grey: f32, alpha_cutoff: f32, wind: WindResponse) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [grey, grey, grey, 1.0],
        base_color_map: Some(id(CARD)),
        alpha_cutoff,
        wrap: 0.0,
        terminator_width: 0.0,
        terminator_growth: 0.0,
        shadow_tint: [1.0; 3],
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: [1.0; 3], tip: [1.0; 3] },
        wind,
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..FoliageMaterial::default()
    }
}

/// No response to the wind.
const STILL: WindResponse = WindResponse { sway: 0.0, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 };

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

/// A square centered at `center` facing `normal`, `half` from its center to each edge, `u` along
/// `across` and `v` down `normal × across`.
fn square(center: Vec3, normal: Vec3, across: Vec3, half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let up = normal.cross(across);
    let corner = |s: f32, t: f32| {
        let uv = [(s + 1.0) * 0.5, (1.0 - t) * 0.5];
        vertex(center + (across * s + up * t) * half, normal, uv)
    };
    (vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], vec![0, 1, 2, 0, 2, 3])
}

/// A mesh of `parts`, each its own submesh drawing its own material slot, with f32 positions,
/// and `foliage` as every vertex's foliage data when given.
fn mesh(parts: Vec<(Vec<Vertex>, Vec<u32>)>, foliage: Option<FoliageVertex>) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let (mut vertices, mut indices, mut submeshes, mut slots) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (part, (part_vertices, part_indices)) in parts.into_iter().enumerate() {
        let first = vertices.len() as u32;
        submeshes.push(Submesh {
            first_index: indices.len() as u32,
            index_count: part_indices.len() as u32,
            material: part as u32,
        });
        indices.extend(part_indices.into_iter().map(|index| first + index));
        vertices.extend(part_vertices);
        slots.push(MaterialSlot::embedded(format!("part {part}"), placeholder.clone()));
    }
    let mut mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32);
    if let Some(data) = foliage {
        mesh.streams = mesh.streams.clone().with_foliage(&vec![data; vertices.len()]);
    }
    mesh
}

/// Foliage data as the stream stores it: quantized, then read back as the shaders read it.
fn stored(data: FoliageVertex) -> FoliageVertex {
    FoliageVertex::decode(data.encode())
}

/// An RGBA16 float texture of linear data, `side` × `side`, per `texel(u, v)` at each texel's
/// center.
fn half_texture(gpu: &GpuContext, side: u32, texel: impl Fn(f32, f32) -> [f32; 4]) -> GpuTexture {
    let texels: Vec<[u16; 4]> = (0..side)
        .flat_map(|y| (0..side).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (u, v) = ((x as f32 + 0.5) / side as f32, (y as f32 + 0.5) / side as f32);
            texel(u, v).map(|c| f16::from_f32(c).to_bits())
        })
        .collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba16Float,
        width: side,
        height: side,
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

/// One rendered frame, as sRGB bytes per pixel.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
}

impl Frame {
    fn linear(&self, x: u32, y: u32) -> Vec3 {
        let pixel = self.pixels[(y * SIDE + x) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// Whether anything was drawn at `(x, y)` over the black sky.
    fn drawn(&self, x: u32, y: u32) -> bool {
        self.pixels[(y * SIDE + x) as usize][..3] != [0, 0, 0]
    }

    /// The world-space ray through pixel `(x, y)`'s center: the eye and a unit direction.
    fn ray(&self, x: u32, y: u32) -> (Vec3, Vec3) {
        let inverse = (self.camera.proj * self.camera.view).inverse();
        let ndc_x = (x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
        let ndc_y = 1.0 - (y as f32 + 0.5) / SIDE as f32 * 2.0;
        let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.5));
        (self.camera.position, (far - self.camera.position).normalize())
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (u32, u32) {
        let uv = self.uv_of(point);
        ((uv.0 * SIDE as f32) as u32, (uv.1 * SIDE as f32) as u32)
    }

    /// Where `point` projects, in texture coordinates (x right, y down), unjittered.
    fn uv_of(&self, point: Vec3) -> (f32, f32) {
        let ndc = (self.camera.proj * self.camera.view).project_point3(point);
        ((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5)
    }

    /// The smallest rectangle of pixels holding every drawn one: `(min x, min y, max x, max y)`.
    fn drawn_bounds(&self) -> (u32, u32, u32, u32) {
        let mut bounds = (SIDE, SIDE, 0, 0);
        for y in 0..SIDE {
            for x in 0..SIDE {
                if self.drawn(x, y) {
                    bounds = (bounds.0.min(x), bounds.1.min(y), bounds.2.max(x), bounds.3.max(y));
                }
            }
        }
        bounds
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
    shadow_tint: Vec3,
    sky: SkyGradient,
    fog: Fog,
    wind: Wind,
    time_seconds: f64,
    added: Vec<MeshInstance>,
    next_id: u64,
}

impl Harness {
    fn new(settings: RenderSettings) -> Self {
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
            camera: CameraData::look_at(Vec3::new(0.0, 0.0, 2.0), Vec3::ZERO, Vec3::Y, 0.9, 1.0, 0.05),
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 },
            shadow_tint: Vec3::ZERO,
            sky: SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO },
            fog: Fog::default(),
            wind: Wind::default(),
            time_seconds: 0.0,
            added: Vec::new(),
            next_id: 0,
        }
    }

    /// Without MSAA or temporal AA, so every pixel is shaded once at its center.
    fn exact() -> Self {
        Self::new(RenderSettings { msaa: Msaa::Off, temporal: false, ..RenderSettings::default() })
    }

    /// Cooks `document` against the textures it names and uploads it.
    fn material<D: MaterialDocument>(
        &mut self,
        document: &D,
        textures: &[(&str, &GpuTexture)],
    ) -> MaterialHandle {
        let bytes = cook(document, textures);
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    /// Uploads `mesh`, its slots drawn with `materials`, or says why the renderer won't.
    fn try_mesh(&mut self, mesh: &Mesh, materials: &[MaterialHandle]) -> Result<MeshHandle, RenderError> {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials)
    }

    fn mesh(&mut self, mesh: &Mesh, materials: &[MaterialHandle]) -> MeshHandle {
        self.try_mesh(mesh, materials).expect("upload")
    }

    /// Adds an instance of `mesh` with the next frame.
    fn add(&mut self, mesh: MeshHandle, transform: Mat4) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        id
    }

    fn render(&mut self) -> Frame {
        self.render_reading(&[])
    }

    /// Draws one frame, also copying the prepass targets `reads` picks into their readbacks.
    /// Fails on any validation error.
    fn render_reading(&mut self, reads: &[Read<'_>]) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.time_seconds = self.time_seconds;
        packet.wind = self.wind;
        packet.sun = self.sun;
        packet.sky = self.sky;
        packet.fog = self.fog;
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.shadow_tint = self.shadow_tint;
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        let reads: Vec<(TextureId, &Readback)> =
            reads.iter().map(|(pick, readback)| (pick(&targets), *readback)).collect();
        if !reads.is_empty() {
            let mut pass = graph.add_pass("read back").side_effect();
            for (texture, _) in &reads {
                pass = pass.read(*texture);
            }
            pass.record(move |ctx| {
                for (texture, readback) in reads {
                    readback.copy(ctx.encoder, ctx.texture(texture));
                }
            });
        }
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        let target = Readback::new(&self.gpu, (SIDE, SIDE), 4);
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        target.copy(&mut encoder, &self.target);
        self.gpu.queue().submit([encoder.finish()]);
        let pixels = target
            .texels(&self.gpu)
            .into_iter()
            .map(|texel| [texel[0], texel[1], texel[2], texel[3]])
            .collect();
        Frame { pixels, camera: self.camera }
    }
}

/// A prepass target to read back after a frame, and where to.
type Read<'a> = (fn(&PrepassTargets) -> TextureId, &'a Readback);

/// A buffer a texture of up to 8 bytes per texel is copied into, row by row.
struct Readback {
    buffer: wgpu::Buffer,
    size: (u32, u32),
    texel: u32,
    padded: u32,
}

impl Readback {
    fn new(gpu: &GpuContext, size: (u32, u32), texel: u32) -> Self {
        let padded = (size.0 * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * size.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, size, texel, padded }
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
                    rows_per_image: Some(self.size.1),
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
                .flat_map(|row| row[..(self.size.0 * self.texel) as usize].chunks_exact(self.texel as usize))
                .map(<[u8]>::to_vec)
                .collect()
        };
        self.buffer.unmap();
        texels
    }
}

/// An `Rg16Float` texel's two values.
fn rg16f(texel: &[u8]) -> [f32; 2] {
    [u16::from_le_bytes([texel[0], texel[1]]), u16::from_le_bytes([texel[2], texel[3]])]
        .map(|bits| f16::from_bits(bits).to_f32())
}

fn depth(texel: &[u8]) -> f32 {
    f32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]])
}

/// A card map whose alpha is `alpha(u, v)`, white.
fn card_map(gpu: &GpuContext, alpha: impl Fn(f32, f32) -> f32) -> GpuTexture {
    half_texture(gpu, 64, |u, v| [1.0, 1.0, 1.0, alpha(u, v)])
}

#[test]
#[ignore = "needs a GPU"]
fn wind_moves_cards_in_every_pass_and_their_velocity_follows() {
    // A level card 2 m above its plant's root, swaying as a whole (sway weight 1), seen from above
    // with the sun overhead. Without gusts the wind moves every vertex alike.
    let response = WindResponse { sway: 0.4, ..STILL };
    let data = stored(FoliageVertex { sway: 1.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 });
    let wind = Wind { direction: Vec3::X, strength: 1.0, gustiness: 0.0 };
    let (start, end) = (10.0, 10.4);
    for levels in [3, 0] {
        let foliage = FoliageSettings { wind_levels: levels, ..FoliageSettings::default() };
        let settings =
            RenderSettings { msaa: Msaa::Off, temporal: false, foliage, ..RenderSettings::default() };
        let mut harness = Harness::new(settings);
        harness.sun.direction = Vec3::NEG_Y;
        harness.wind = wind;
        harness.camera = CameraData::look_at(
            Vec3::new(0.0, 4.5, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::NEG_Z,
            0.9,
            1.0,
            0.05,
        );
        let map = card_map(&harness.gpu, |_, _| 1.0);
        let material = harness.material(&flat(0.5, 0.0, response), &[(CARD, &map)]);
        let card = mesh(vec![square(Vec3::new(0.0, 2.0, 0.0), Vec3::Y, Vec3::X, 0.5)], Some(data));
        let card = harness.mesh(&card, &[material]);
        harness.add(card, Mat4::IDENTITY);

        harness.time_seconds = start;
        harness.render();
        harness.time_seconds = end;
        let readback = || Readback::new(&harness.gpu, (SIDE, SIDE), 4);
        let (depths, velocities) = (readback(), readback());
        let reads: [Read<'_>; 2] = [(|t| t.depth, &depths), (|t| t.velocity, &velocities)];
        let frame = harness.render_reading(&reads);
        let (depths, velocities) = (depths.texels(&harness.gpu), velocities.texels(&harness.gpu));

        let offset = |time: f64| {
            wind_offset(&wind, time, levels, &response, &data, Vec3::new(0.0, 2.0, 0.0), Vec3::ZERO)
        };
        let (now, before) = (offset(end), offset(start));
        if levels == 0 {
            assert_eq!((now, before), (Vec3::ZERO, Vec3::ZERO), "no levels, no wind");
        } else {
            assert!(now.x > 0.05 && now.distance(before) > 0.04, "a breeze moves it: {before} to {now}");
        }

        // The main pass draws the card where the wind took it: its corners' pixels, moved.
        let corners =
            [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(x, z)| Vec3::new(x, 2.0, z) + now);
        let pixels =
            corners.map(|corner| frame.uv_of(corner)).map(|(u, v)| (u * SIDE as f32, v * SIDE as f32));
        let expected = (
            pixels.iter().map(|p| p.0).fold(f32::MAX, f32::min),
            pixels.iter().map(|p| p.1).fold(f32::MAX, f32::min),
            pixels.iter().map(|p| p.0).fold(f32::MIN, f32::max),
            pixels.iter().map(|p| p.1).fold(f32::MIN, f32::max),
        );
        let drawn = frame.drawn_bounds();
        let near = |pixel: u32, edge: f32| (pixel as f32 + 0.5 - edge).abs() <= 1.0;
        assert!(
            near(drawn.0, expected.0)
                && near(drawn.1, expected.1)
                && near(drawn.2, expected.2)
                && near(drawn.3, expected.3),
            "{levels} levels: drawn over {drawn:?}, the wind says {expected:?}"
        );

        // The scene prepass holds it there, and its velocity is the wind's motion since the last
        // frame: the surface under the middle pixel was the wind's difference away.
        let center = Vec3::new(0.0, 2.0, 0.0) + now;
        let (x, y) = frame.pixel_of(center);
        let index = (y * SIDE + x) as usize;
        assert!(depth(&depths[index]) > 0.0, "in the prepass");
        let (u, v) = frame.uv_of(center);
        let (u0, v0) = frame.uv_of(center - (now - before));
        let velocity = rg16f(&velocities[index]);
        let expected = [u - u0, v - v0];
        if levels > 0 {
            assert!(expected[0].abs() * SIDE as f32 > 2.0, "the test moves the card {expected:?}");
        }
        assert!(
            (velocity[0] - expected[0]).abs() < 0.002 && (velocity[1] - expected[1]).abs() < 0.002,
            "{levels} levels: velocity {velocity:?}, the wind's motion {expected:?}"
        );
    }
}

/// Pixels between the card's lit tone and the black sky: partly covered ones.
fn partial_pixels(frame: &Frame, full: f32) -> usize {
    let mut partial = 0;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let value = frame.linear(x, y).x;
            if value > 0.15 * full && value < 0.85 * full {
                partial += 1;
            }
        }
    }
    partial
}

/// A card filling the view, its alpha crossing the cutoff along a slanted line through the
/// middle: the only edge in the frame is the alpha test's. Returns the frame and the card's tone.
fn slanted_edge(settings: RenderSettings) -> (Frame, f32) {
    let mut harness = Harness::new(settings);
    let map = card_map(&harness.gpu, |u, v| 0.8 * u + 0.4 * v - 0.1);
    let grey = 0.5;
    let material = harness.material(&flat(grey, 0.5, STILL), &[(CARD, &map)]);
    let data = stored(FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 });
    let card = mesh(vec![square(Vec3::ZERO, Vec3::Z, Vec3::X, 2.0)], Some(data));
    let card = harness.mesh(&card, &[material]);
    harness.add(card, Mat4::IDENTITY);
    let frame = harness.render();
    (frame, grey)
}

#[test]
#[ignore = "needs a GPU"]
fn alpha_to_coverage_softens_card_edges_under_msaa_where_tiers_ask() {
    // Every tier as it draws cards (its MSAA and foliage settings; temporal AA, which would
    // soften every edge, off).
    let table = TierTable::shipped();
    for tier in table.tiers() {
        let settings = RenderSettings {
            msaa: tier.antialiasing.msaa,
            foliage: tier.foliage,
            temporal: false,
            ..RenderSettings::default()
        };
        let (frame, grey) = slanted_edge(settings);
        let covers = tier.antialiasing.msaa != Msaa::Off && tier.foliage.alpha_to_coverage;
        let partial = partial_pixels(&frame, grey);
        if covers {
            assert!(partial > 40, "{:?}: alpha-to-coverage leaves a soft edge, {partial} pixels", tier.id);
        } else {
            assert_eq!(partial, 0, "{:?}: a sharp alpha test", tier.id);
        }
        // Covered pixels are the card's tone, uncovered ones the sky.
        let covered = frame.linear(SIDE - 4, SIDE - 4);
        assert!((covered.x - grey).abs() < 0.02, "{:?}: {covered}", tier.id);
        assert_eq!(frame.linear(3, 3), Vec3::ZERO, "{:?}", tier.id);
    }
    // MSAA without alpha-to-coverage tests alpha per pixel, as without MSAA.
    let foliage = FoliageSettings { alpha_to_coverage: false, ..FoliageSettings::default() };
    for (msaa, foliage, soft) in [(Msaa::X4, foliage, false), (Msaa::X4, FoliageSettings::default(), true)] {
        let settings = RenderSettings { msaa, foliage, temporal: false, ..RenderSettings::default() };
        let (frame, grey) = slanted_edge(settings);
        assert_eq!(partial_pixels(&frame, grey) > 40, soft, "{msaa:?} {foliage:?}");
    }
}

/// An upright card in the plane x = 0, from the ground to 1 m up and across z in -0.5..0.5, its
/// map's `u` along +z, standing on ground that takes shadows, with the sun 45° up toward +x: the
/// card's shadow falls on x in -1..0. Seen from above the shadow, the card edge-on beside it.
fn card_shadow(alpha_cutoff: f32) -> Frame {
    let settings = RenderSettings {
        msaa: Msaa::Off,
        temporal: false,
        shadows: Some(ShadowSettings::default()),
        ..RenderSettings::default()
    };
    let mut harness = Harness::new(settings);
    harness.sun.direction = -Vec3::new(1.0, 1.0, 0.0).normalize();
    harness.shadow_tint = Vec3::splat(0.2);
    harness.camera = CameraData::look_at(
        Vec3::new(-0.5, 2.5, 0.0),
        Vec3::new(-0.5, 0.0, 0.0),
        Vec3::NEG_Z,
        0.9,
        1.0,
        0.05,
    );
    // Leaves on the half of the card toward +z; the other half's alpha is 0.
    let map = card_map(&harness.gpu, |u, _| if u > 0.5 { 1.0 } else { 0.0 });
    let leaves = harness.material(&flat(0.5, alpha_cutoff, STILL), &[(CARD, &map)]);
    let data = stored(FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 });
    let card = mesh(vec![square(Vec3::new(0.0, 0.5, 0.0), Vec3::X, Vec3::Z, 0.5)], Some(data));
    let card = harness.mesh(&card, &[leaves]);
    harness.add(card, Mat4::IDENTITY);
    let ground = EnvironmentMaterial {
        base_color: [0.6, 0.6, 0.6, 1.0],
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    };
    let ground = harness.material(&ground, &[]);
    let floor = mesh(vec![square(Vec3::ZERO, Vec3::Y, Vec3::X, 3.0)], None);
    let floor = harness.mesh(&floor, &[ground]);
    harness.add(floor, Mat4::IDENTITY);
    harness.render()
}

#[test]
#[ignore = "needs a GPU"]
fn shadow_casters_cut_cards_out() {
    let lit_and_shaded = |frame: &Frame| {
        let brightness = |z: f32| {
            let (x, y) = frame.pixel_of(Vec3::new(-0.5, 0.0, z));
            frame.linear(x, y).x
        };
        (brightness(-0.25), brightness(0.25), brightness(0.9))
    };
    // Alpha-tested, the leaves' half casts and the clear half lets the sun through.
    let (clear, leaves, beside) = lit_and_shaded(&card_shadow(0.5));
    assert!(beside > 0.3, "the ground beside the shadow is lit: {beside}");
    assert!((clear - beside).abs() < 0.02, "the sun through the clear half: {clear} against {beside}");
    assert!(leaves < 0.4 * beside, "the leaves' shadow: {leaves} against {beside}");
    // Opaque (a cutoff of 0, as bark), the whole card casts.
    let (clear, leaves, beside) = lit_and_shaded(&card_shadow(0.0));
    assert!(clear < 0.4 * beside && leaves < 0.4 * beside, "{clear}, {leaves} against {beside}");
}

#[test]
#[ignore = "needs a GPU"]
fn the_scene_prepass_cuts_cards_out_as_the_main_pass_does() {
    // A checkered card fluttering in the wind, seen face on: the prepass's depth holds exactly the
    // pixels the main pass draws, the wind's displacement and the holes alike.
    let mut harness = Harness::exact();
    harness.wind = Wind { direction: Vec3::new(1.0, 0.0, 0.4), strength: 1.5, gustiness: 0.5 };
    harness.time_seconds = 37.25;
    let map =
        card_map(&harness.gpu, |u, v| f32::from(((u * 6.0) as u32 + (v * 6.0) as u32).is_multiple_of(2)));
    let response = WindResponse { sway: 0.05, bend: 0.08, flutter: 0.04, flutter_frequency: 2.0 };
    let material = harness.material(&flat(0.5, 0.5, response), &[(CARD, &map)]);
    let data = stored(FoliageVertex { sway: 0.6, bend: 0.8, flutter: 1.0, phase: 0.3, gradient: 0.7 });
    let card = mesh(vec![square(Vec3::ZERO, Vec3::Z, Vec3::X, 0.6)], Some(data));
    let card = harness.mesh(&card, &[material]);
    harness.add(card, Mat4::IDENTITY);
    harness.render();
    harness.time_seconds += 1.0 / 60.0;
    let depths = Readback::new(&harness.gpu, (SIDE, SIDE), 4);
    let reads: [Read<'_>; 1] = [(|t| t.depth, &depths)];
    let frame = harness.render_reading(&reads);
    let depths = depths.texels(&harness.gpu);
    let (mut drawn, mut mismatched) = (0, 0);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let covered = depth(&depths[(y * SIDE + x) as usize]) > 0.0;
            drawn += usize::from(frame.drawn(x, y));
            mismatched += usize::from(covered != frame.drawn(x, y));
        }
    }
    let card_pixels = (SIDE * SIDE) as usize;
    assert!(drawn > card_pixels / 8 && drawn < card_pixels / 2, "half the card's squares: {drawn} pixels");
    assert_eq!(mismatched, 0, "the prepass and the main pass cover the same pixels");
}

#[test]
#[ignore = "needs a GPU"]
fn the_frame_s_fog_covers_foliage_as_the_world() {
    // A wall of leaves 12 m away, lit head on, through every layer of fog and the key light's
    // inscatter: each pixel is the leaves' lit tone seen through the CPU model's fog (`Fog::ray`).
    let mut harness = Harness::exact();
    harness.sky = SkyGradient {
        zenith: Vec3::new(0.2, 0.3, 0.6),
        horizon: Vec3::new(0.5, 0.55, 0.6),
        ground: Vec3::new(0.3, 0.3, 0.3),
    };
    harness.fog = Fog {
        near: FogLayer { color: Vec3::new(0.6, 0.5, 0.4), density: 0.02, start: 3.0 },
        far: FogLayer { color: Vec3::new(0.9, 0.95, 1.0), density: 0.01, start: 6.0 },
        height: HeightFog { color: Vec3::new(0.4, 0.5, 0.6), density: 0.04, falloff: 0.25, base: 0.0 },
        sun_inscatter: Vec3::new(0.15, 0.1, 0.05),
        characters: 0.25,
    };
    harness.sun.direction = -Vec3::new(0.1, 0.2, 1.0).normalize();
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.0, 1.5, -12.0), Vec3::Y, 0.9, 1.0, 0.05);
    let map = card_map(&harness.gpu, |_, _| 1.0);
    let grey = 0.4;
    let leaves = harness.material(&flat(grey, 0.0, STILL), &[(CARD, &map)]);
    let data = stored(FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 });
    let wall = mesh(vec![square(Vec3::new(0.0, 1.5, -12.0), Vec3::Z, Vec3::X, 8.0)], Some(data));
    let wall = harness.mesh(&wall, &[leaves]);
    harness.add(wall, Mat4::IDENTITY);
    let frame = harness.render();
    let toward_key = (-harness.sun.direction).normalize();
    // Lit head on by a white key light, without wrap or terminator: the tone is `grey × N·L`.
    let lit = grey * Vec3::Z.dot(toward_key);
    let mut worst = (0.0f32, (0, 0));
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (eye, dir) = frame.ray(x, y);
            let distance = (eye.z + 12.0) / -dir.z;
            let expected = harness
                .fog
                .ray(eye, dir, distance, 1.0, harness.sky.horizon, toward_key)
                .apply(Vec3::splat(lit));
            let error = (frame.linear(x, y) - expected).abs().max_element();
            if error > worst.0 {
                worst = (error, (x, y));
            }
        }
    }
    assert!(worst.0 < 0.02, "{} off the fog model at {:?}", worst.0, worst.1);
    let center = frame.linear(SIDE / 2, SIDE / 2);
    assert!((center - Vec3::splat(lit)).abs().max_element() > 0.05, "the fog shows: {center} against {lit}");
}

#[test]
#[ignore = "needs a GPU"]
fn foliage_needs_its_stream_on_a_static_mesh_and_is_counted() {
    let mut harness = Harness::exact();
    let map = card_map(&harness.gpu, |_, _| 1.0);
    let leaves = harness.material(&flat(0.5, 0.5, STILL), &[(CARD, &map)]);
    let bark = harness.material(&flat(0.3, 0.0, STILL), &[(CARD, &map)]);
    let quad = || square(Vec3::ZERO, Vec3::Z, Vec3::X, 0.3);
    // No foliage stream: refused, naming the bake.
    let bare = mesh(vec![quad()], None);
    assert!(matches!(harness.try_mesh(&bare, &[leaves]), Err(RenderError::NoFoliageStream)));
    // Morphed: foliage draws on static meshes alone.
    let data = stored(FoliageVertex { sway: 0.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 });
    let mut morphed = mesh(vec![quad()], Some(data));
    morphed.morph_targets =
        vec![MorphTarget { name: "still".into(), default_weight: 0.0, first_delta: 0, delta_count: 1 }];
    morphed.morph_deltas = vec![MorphDelta::new(0, [0.0; 3], [0.0; 3])];
    assert!(matches!(harness.try_mesh(&morphed, &[leaves]), Err(RenderError::DeformedFoliage)));

    // A plant of bark and leaves, twice, beside a mesh of other families: each foliage draw and
    // instance counted.
    let plant = mesh(vec![quad(), square(Vec3::new(0.0, 0.5, 0.0), Vec3::Z, Vec3::X, 0.3)], Some(data));
    let plant = harness.mesh(&plant, &[bark, leaves]);
    harness.add(plant, Mat4::from_translation(Vec3::new(-0.5, 0.0, 0.0)));
    harness.add(plant, Mat4::from_translation(Vec3::new(0.5, 0.0, 0.0)));
    let ground = harness.material(&EnvironmentMaterial::default(), &[]);
    let floor =
        harness.mesh(&mesh(vec![square(Vec3::new(0.0, -0.5, 0.0), Vec3::Y, Vec3::X, 1.0)], None), &[ground]);
    harness.add(floor, Mat4::IDENTITY);
    harness.render();
    let stats = harness.renderer.stats();
    assert_eq!((stats.foliage_draws, stats.foliage_instances), (4, 2), "{stats:?}");
    assert!(stats.foliage_pipelines >= 2, "opaque and alpha-tested surfaces: {stats:?}");
}
