//! The look at every tier and scale (M4 T0, the M4 plan's exit test 4): lines keep their width
//! in output pixels, and the anime character shading's two-tone edge stays hard enough, on each
//! shipped tier's settings, at the output heights the tiers are made for, at render scale 1, at
//! the tier's own scale, at the thermal ladder's 0.85 and at the tier's dynamic floor. Run with
//! `cargo test -p sapphire_render --test gpu look_tiers:: -- --ignored --nocapture`.
//!
//! The subject is a sphere lit from the side, so its terminator runs down the middle of the frame,
//! split hard into its two tones (`softness` 0, the one-pixel edge every split gets: the edge an
//! upscale softens most), with a black line round it of the look-dev character's width. Its lit
//! side and the sky are one grey, so a pixel's darkness at the silhouette is the line's coverage.
//! `sapphire_testkit::look` measures both, as it measures captures.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, OutlineCurve, Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::{QualityTier, TierId, TierTable};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId,
    MeshHandle, MeshInstance, PostParams, Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use sapphire_testkit::look::{Edge, Lines, darkness, luminance};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Linear 0.5 grey, the test sky's: a surface of this albedo, lit by the unit key and the 6%
/// ambient, shows the sky's brightness.
const GREY: [f32; 3] = [0.5 / 1.06; 3];
/// The shadow side's tone, as a share of the lit side's.
const SHADE: f32 = 0.4;
/// The look-dev character's line, in pixels at 1080 rows.
const LINE_WIDTH: f32 = 5.5;
/// The vertical field of view, in degrees, and the share of the frame's height the sphere spans.
const FOV: f32 = 45.0;
const SPAN: f32 = 0.45;
/// How far the key light turns from +X toward +Y, in radians, so the terminator slants across the
/// rows measured and they sample every phase of a scaled scene's pixel grid: a vertical edge would
/// fall at one phase, which reads a bilinear upscale as anything from sharp to twice its width.
const TILT: f32 = 0.15;
/// Rows measured, through the sphere's middle half.
const ROWS: u32 = 21;
/// Frames drawn before the one measured: temporal AA's history settles over a few jitter cycles.
const TAA_FRAMES: u32 = 24;

/// Each class's output: the desktop tiers' 1080 rows, and a phone's 1170 (an iPhone 12 or 13 held
/// sideways), narrowed to save CPU rasterizers' time: only the height sets what a scale draws.
fn output(tier: TierId) -> (u32, u32) {
    match tier {
        TierId::DesktopHigh | TierId::DesktopLow => (640, 1080),
        TierId::MobileHigh | TierId::MobileLow => (640, 1170),
    }
}

/// The scales a tier draws at for `height` output rows: 1, the tier's own (the phones' capped
/// height), the ladder's 0.85 of it, and its dynamic floor.
fn scales(tier: &QualityTier, height: u32) -> Vec<(&'static str, f32)> {
    let base = tier.resolution.base_scale(height);
    let mut scales = vec![("scale 1", 1.0)];
    if base < 1.0 {
        scales.push(("the tier's scale", base));
    }
    scales.extend([("0.85", 0.85 * base), ("floor", tier.resolution.dynamic_floor * base)]);
    scales
}

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

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

/// The unit sphere, grey on its lit side and [`SHADE`] of that on its shadow side, split hard,
/// with a black line [`LINE_WIDTH`] wide at every distance and no rim.
fn sphere() -> Mesh {
    const RINGS: u32 = 64;
    const SEGMENTS: u32 = 96;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: n.to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
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
    let material = AnimeCharacterMaterial {
        base_color: [GREY[0], GREY[1], GREY[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion {
            shade_color: [SHADE; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline {
            width: LINE_WIDTH,
            curve: OutlineCurve { near: (0.0, 1.0), mid: 1.0, far: (1000.0, 1.0), cutoff: 1000.0 },
            flatten: 0.0,
            ..Outline::default()
        },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// Post that shows linear values below about 0.98 as they are.
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

/// How far the camera stands from the sphere's middle, on +Z.
fn distance() -> f32 {
    1.0 / (SPAN * (FOV.to_radians() * 0.5).tan())
}

/// The sphere's silhouette radius in pixels on an output `height` rows tall.
fn radius_px(height: u32) -> f32 {
    let d = distance();
    (1.0 / (d * d - 1.0).sqrt()) / (FOV.to_radians() * 0.5).tan() * height as f32 / 2.0
}

/// One tier's renderer drawing the sphere into its class's output.
struct Shot {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    mesh: MeshHandle,
    texture: wgpu::Texture,
    added: bool,
}

impl Shot {
    fn new(gpu: &GpuContext, tier: &QualityTier) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, tier.render_settings()).expect("renderer");
        let bytes = sphere().to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");
        let texture = target(gpu, output(tier.id));
        Self {
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            mesh,
            texture,
            added: false,
        }
    }

    /// Draws `frames` frames at `scale` and returns the last one's pixels.
    fn render(&mut self, gpu: &GpuContext, scale: f32, frames: u32) -> Vec<[u8; 4]> {
        let (width, height) = output_of(&self.texture);
        for _ in 0..frames {
            let packet = self.packets.begin();
            let camera = CameraData::look_at(
                Vec3::new(0.0, 0.0, distance()),
                Vec3::ZERO,
                Vec3::Y,
                FOV.to_radians(),
                width as f32 / height as f32,
                0.05,
            );
            packet.views.push(ViewData::new(camera));
            // From +X, turned up by `TILT`, so the terminator runs down through the middle, slanting:
            // lit on the right, shaded on the left.
            let toward = Vec3::new(TILT.cos(), TILT.sin(), 0.0);
            packet.sun = DirectionalLight { direction: -toward, color: Vec3::ONE, intensity: 1.0 };
            packet.sky =
                SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
            packet.ambient =
                AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 };
            packet.post = passthrough();
            if !std::mem::replace(&mut self.added, true) {
                packet.instances.changed.push(MeshInstance {
                    id: InstanceId(0),
                    mesh: self.mesh,
                    transform: Mat4::IDENTITY,
                });
            }
            let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
            self.renderer.set_render_scale(scale);
            self.renderer.prepare(gpu, self.packets.current());
            let view = self.texture.create_view(&Default::default());
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, target);
            self.graph.execute(gpu, graph).expect("a valid graph");
            let error = pollster::block_on(scope.pop());
            assert!(error.is_none(), "scale {scale}: {error:?}");
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("frame complete");
        }
        read_back(gpu, &self.texture)
    }
}

fn output_of(texture: &wgpu::Texture) -> (u32, u32) {
    (texture.width(), texture.height())
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (width, height) = output_of(texture);
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..width as usize * 4].as_chunks::<4>().0.iter().copied())
        .collect()
}

/// What one frame of the sphere measures, in output pixels.
#[derive(Clone, Copy, Debug)]
struct Look {
    /// The line at the lit side's silhouette, over rows through the sphere's middle half.
    lines: Lines,
    /// The terminator's mean 10–90% width and steepest pixel over the same rows.
    edge_width: f32,
    edge_steepest: f32,
}

impl Look {
    fn of(pixels: &[[u8; 4]], (width, height): (u32, u32)) -> Self {
        let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
        let (cx, cy, radius) = (width as f32 / 2.0, height as f32 / 2.0, radius_px(height));
        let line_px = LINE_WIDTH * height as f32 / 1080.0;
        let half = ROWS as i32 / 2;
        let rows: Vec<u32> =
            (-half..=half).map(|i| (cy + i as f32 * radius / (2 * half) as f32) as u32).collect();
        let mut darknesses = Vec::new();
        let (mut edge_width, mut edge_steepest) = (0.0, 0.0);
        for &y in &rows {
            let dy = y as f32 + 0.5 - cy;
            let silhouette = cx + (radius * radius - dy * dy).sqrt();
            let sky = luminance(at(width - 1, y));
            let from = (silhouette - 4.0) as u32;
            let to = ((silhouette + 2.0 * line_px + 8.0) as u32).min(width - 1);
            darknesses.extend((from..to).map(|x| darkness(luminance(at(x, y)), sky)));
            // The terminator's plane holds the view axis, so it projects to a line through the middle.
            let terminator = (cx + dy * TILT.tan()) as u32;
            let profile: Vec<f32> = (terminator - 16..terminator + 16).map(|x| luminance(at(x, y))).collect();
            let edge = Edge::of(&profile, 0.05).expect("a terminator across the middle");
            edge_width += edge.width;
            edge_steepest += edge.steepest;
        }
        let rows = rows.len() as f32;
        Self {
            lines: Lines::from_darkness(darknesses),
            edge_width: edge_width / rows,
            edge_steepest: edge_steepest / rows,
        }
    }
}

/// Each tier at each of its scales, against itself at scale 1 and at its own scale. Measured on an
/// Apple M5 (Metal), 2026-11-10, in output pixels, held still (the line is 5.5 px at 1080 rows and
/// 5.96 at 1170; a row crosses it on a slant, so it covers a little more):
///
/// | Tier | Scale | Line covered, spread | Contrast | Two-tone edge (10–90%) |
/// |---|---|---|---|---|
/// | Desktop-High, Desktop-Low | 1, 0.85, 0.7 | 6.01, 6.46–6.47 | 1.00 | 1.58, 1.62, 1.65 |
/// | Mobile-High | 1 | 6.24, 6.61 | 1 | 0.97 |
/// | | 0.632 (its own, 740 rows) | 6.27, 7.36 | 0.90 | 2.31 |
/// | | 0.538 (0.85) | 6.34, 7.68 | 0.88 | 2.64 |
/// | | 0.474 (floor) | 6.33, 7.84 | 0.86 | 2.85 |
/// | Mobile-Low | 1 | 6.24, 6.61 | 1 | 0.97 |
/// | | 0.504 (its own, 590 rows) | 6.33, 7.64 | 0.88 | 3.25 |
/// | | 0.429 (0.85) | 6.32, 8.01 | 0.84 | 3.16 |
/// | | 0.378 (floor) | 6.23, 8.42 | 0.79 | 3.58 |
///
/// Temporal AA rebuilds a still line and edge from its jittered history at the output's size, so
/// the desktop tiers' floor costs the look nothing here (a turning line is `outline`'s
/// `upsampled_lines_keep_their_width_in_output_pixels`). The phones' bilinear upscale keeps every
/// line's width but blurs it, and softens the hard edge to two or three pixels at the tier's own
/// scale, which its height cap sets; the floor adds 10–25% to that.
#[test]
#[ignore = "needs a GPU"]
fn lines_and_two_tone_edges_hold_at_every_tier_and_scale() {
    let gpu = gpu();
    let table = TierTable::shipped();
    for tier in table.tiers() {
        let mut shot = Shot::new(&gpu, tier);
        let size = output(tier.id);
        let frames = if tier.antialiasing.temporal { TAA_FRAMES } else { 2 };
        let (mut native, mut own): (Option<Look>, Option<Look>) = (None, None);
        for (name, scale) in scales(tier, size.1) {
            let look = Look::of(&shot.render(&gpu, scale, frames), size);
            let at_one = *native.get_or_insert(look);
            if name != "0.85" && name != "floor" {
                own = Some(look);
            }
            let own = own.expect("scale 1 comes first");
            let strength = look.lines.strength / at_one.lines.strength;
            let contrast = look.lines.contrast() / at_one.lines.contrast();
            eprintln!(
                "{} at {name} ({scale:.3}, {} rows): line {:.2} px covered, spread over {:.2} (strength \
                 {strength:.3} of scale 1's, contrast {contrast:.3} of it), two-tone edge {:.2} px, steepest {:.2}",
                tier.id,
                (size.1 as f32 * scale).round(),
                look.lines.strength / ROWS as f32,
                look.lines.width() / ROWS as f32,
                look.edge_width,
                look.edge_steepest,
            );
            let what = format!("{} at {name}: {look:?}", tier.id);
            // Lines keep their width in output pixels on every tier and at every scale.
            assert!((0.95..1.05).contains(&strength), "{what}: line strength {strength} of scale 1's");
            if tier.antialiasing.temporal {
                // Rebuilt from the history, a still line and edge hold as drawn at scale 1.
                assert!(contrast > 0.97, "{what}: contrast {contrast} of scale 1's");
                assert!(look.edge_width < 1.8, "{what}");
            } else {
                // A bilinear upscale blurs both; the floor no more than three tenths again.
                assert!(contrast > 0.75, "{what}: contrast {contrast} of scale 1's");
                assert!(look.edge_width < if scale == 1.0 { 1.2 } else { 4.0 }, "{what}");
                assert!(
                    look.edge_width < 1.3 * own.edge_width,
                    "{what}: {:.2} at its own scale",
                    own.edge_width
                );
            }
        }
    }
}
