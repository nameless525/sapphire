//! Post's compositing layers (M1 P2) and screen effects (M1 P3, [`effects`]), rendered and read
//! back. They need a GPU: run them with `cargo test -p sapphire_render --lib -- --ignored post::`.
//!
//! Frames go through post with a tone curve that is linear up to about 0.98, a neutral grade and
//! no bloom or vignette unless a test sets them, so the target holds the scene's own values,
//! sRGB-encoded, to within the LUT's precision, and each layer's effect can be predicted.

mod effects;

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Mat4, Vec2, Vec3, Vec4};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::layers::{DEFAULT_LENS, FlareKind};
use crate::graph::{FrameGraph, GraphExecutor};
use crate::quality::{TierId, TierTable};
use crate::{
    AmbientLight, BloomParams, CameraData, CharacterBloom, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, Diffusion, DirectionalLight, FramePacket, FramePacketRing, GradientBlend,
    GradientLayer, GradientShape, InstanceId, LensFlare, MAX_GRADIENT_LAYERS, MeshHandle, MeshInstance, Msaa,
    PostParams, RenderSettings, Renderer, ShaderFile, ShaderSources, SkyDisc, SkyGradient, ToneCurve,
    ViewData, Vignette,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Post that shows linear values below about 0.98 as they are, with no layers.
fn passthrough() -> PostParams {
    PostParams {
        bloom: BloomParams { intensity: 0.0, ..BloomParams::default() },
        tone_curve: ToneCurve {
            linear_start: 0.001,
            linear_length: 0.99,
            black_tightness: 1.0,
            ..ToneCurve::default()
        },
        grade_a: ColorGrade::NEUTRAL,
        grade_b: ColorGrade::NEUTRAL,
        vignette: Vignette { intensity: 0.0, ..Vignette::default() },
        ..PostParams::default()
    }
}

const BLACK: SkyGradient = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };

/// The vertices and indices of a UV sphere of `radius` around `center`.
fn sphere_geometry(center: Vec3, radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 32;
    const SEGMENTS: u32 = 48;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: (center + n * radius).to_array(),
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
    (vertices, indices)
}

/// A flat material in `rgb`, lit everywhere, with no rim or outline.
fn flat(rgb: [f32; 3]) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// The sun along the view direction of every test camera (−Z), so every surface faces it.
const SUN: DirectionalLight = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };

/// A renderer drawing frames into a readable sRGB target, with the meshes it uploaded.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    size: (u32, u32),
    camera: CameraData,
    /// Copied into every packet.
    sky: SkyGradient,
    /// Added with the next packet.
    pending: Vec<MeshInstance>,
    /// Listed as characters in every packet.
    characters: Vec<InstanceId>,
    next_id: u64,
}

impl Harness {
    fn new(settings: RenderSettings, size: (u32, u32)) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let aspect = size.0 as f32 / size.1 as f32;
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 0.8, aspect, 0.05);
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            size,
            camera,
            sky: BLACK,
            pending: Vec::new(),
            characters: Vec::new(),
            next_id: 0,
        }
    }

    /// Adds a sphere of `radius` at `center` in `material`, as a character or not.
    fn sphere(&mut self, center: Vec3, radius: f32, material: AnimeCharacterMaterial, character: bool) {
        let (vertices, indices) = sphere_geometry(center, radius);
        let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let cooked = Material::from_document(&material, &swizzles).expect("a valid material");
        let slots = vec![MaterialSlot::embedded("sphere", cooked)];
        let mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32);
        let bytes = mesh.to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = self.renderer.upload_embedded_materials(mesh).expect("materials");
        let handle: MeshHandle = self.renderer.upload_mesh(mesh, &materials).expect("upload");
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.pending.push(MeshInstance { id, mesh: handle, transform: Mat4::IDENTITY });
        if character {
            self.characters.push(id);
        }
    }

    /// Renders `frames` frames with `post`, each packet set up by `fill`, and reads the last back
    /// as sRGB-encoded texels.
    fn render_frames(
        &mut self,
        frames: u32,
        post: PostParams,
        fill: impl Fn(&mut FramePacket),
    ) -> Vec<[u8; 4]> {
        for _ in 0..frames {
            let packet = self.packets.begin();
            packet.views.push(ViewData::new(self.camera));
            packet.sun = SUN;
            packet.sky = self.sky;
            packet.ambient = AmbientLight { intensity: 0.0, ..AmbientLight::default() };
            packet.post = post;
            packet.instances.changed.append(&mut self.pending);
            for &id in &self.characters {
                let lighting = CharacterLighting::unstyled(SUN);
                let face = CharacterFace::default();
                packet.characters.push(CharacterInstance {
                    id,
                    lighting,
                    face,
                    body: CharacterBody::default(),
                });
            }
            fill(packet);
            let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
            self.renderer.prepare(&self.gpu, self.packets.current());
            let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, target).expect("a view and a target");
            self.graph.execute(&self.gpu, graph).expect("a valid graph");
            assert!(pollster::block_on(scope.pop()).is_none(), "no validation errors");
        }
        read_texture(&self.gpu, &self.target)
    }

    fn render(&mut self, post: PostParams) -> Vec<[u8; 4]> {
        self.render_frames(1, post, |_| {})
    }

    /// Where world-space `point` lands on the target, in pixels from the top left.
    fn project(&self, point: Vec3) -> Vec2 {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        Vec2::new((ndc.x + 1.0) * 0.5 * self.size.0 as f32, (1.0 - ndc.y) * 0.5 * self.size.1 as f32)
    }

    /// The direction from the camera through texture coordinates `uv`.
    fn direction(&self, uv: Vec2) -> Vec3 {
        let ndc = Vec4::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
        let near = (self.camera.proj * self.camera.view).inverse() * ndc;
        (near.truncate() / near.w - self.camera.position).normalize()
    }
}

fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (width, height) = (texture.width(), texture.height());
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

/// Linear light each pixel of `with` adds over `without`, by luminance.
fn added(with: &[[u8; 4]], without: &[[u8; 4]]) -> Vec<f32> {
    let luminance = |t: [u8; 4]| {
        Vec3::new(0.2126, 0.7152, 0.0722).dot(Vec3::new(
            srgb_to_linear(t[0]),
            srgb_to_linear(t[1]),
            srgb_to_linear(t[2]),
        ))
    };
    with.iter().zip(without).map(|(&a, &b)| luminance(a) - luminance(b)).collect()
}

/// The largest difference of any channel between two images, in display steps.
fn max_step(a: &[[u8; 4]], b: &[[u8; 4]]) -> u8 {
    a.iter().zip(b).flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]))).max().unwrap_or(0)
}

/// The mean difference of the channels between two images, in display steps.
fn mean_step(a: &[[u8; 4]], b: &[[u8; 4]]) -> f64 {
    let sum: u64 =
        a.iter().zip(b).flat_map(|(a, b)| (0..3).map(move |c| u64::from(a[c].abs_diff(b[c])))).sum();
    sum as f64 / (a.len() * 3) as f64
}

/// Sums `values` over the pixels of a `size` image whose centers lie between `inner` and
/// `outer` pixels from `center`.
fn ring_sum(values: &[f32], size: (u32, u32), center: Vec2, inner: f32, outer: f32) -> f32 {
    let mut sum = 0.0;
    for (index, &value) in values.iter().enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        let distance = pixel.distance(center);
        if distance >= inner && distance < outer {
            sum += value;
        }
    }
    sum
}

/// The weighted centroid of `values` over the pixels within `radius` of `around`.
fn centroid(values: &[f32], size: (u32, u32), around: Vec2, radius: f32) -> (Vec2, f32) {
    let (mut sum, mut weight) = (Vec2::ZERO, 0.0);
    for (index, &value) in values.iter().enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        if pixel.distance(around) < radius && value > 0.0 {
            sum += pixel * value;
            weight += value;
        }
    }
    (sum / weight.max(1e-9), weight)
}

#[test]
#[ignore = "needs a GPU"]
fn diffusion_glows_wider_than_bloom_softly_and_only_above_its_threshold() {
    let size = (320, 180);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sphere(Vec3::ZERO, 0.25, flat([6.0; 3]), false);
    let center = harness.project(Vec3::ZERO);
    let radius = harness.project(Vec3::new(0.25, 0.0, 0.0)).x - center.x;
    let plain = harness.render(passthrough());
    let diffusion = |intensity: f32, threshold: f32| PostParams {
        diffusion: Diffusion { intensity, threshold, color: Vec3::ONE },
        ..passthrough()
    };
    let glow = |harness: &mut Harness, intensity: f32, threshold: f32| {
        added(&harness.render(diffusion(intensity, threshold)), &plain)
    };
    // Off draws exactly the plain frame.
    assert_eq!(harness.render(diffusion(0.0, 0.0)), plain, "no intensity, no glow");
    // Energy: the glow grows with its intensity, never past white.
    let outside = |values: &[f32]| ring_sum(values, size, center, radius * 1.5, 1000.0);
    let energies = [0.5, 1.0, 2.0, 4.0].map(|intensity| outside(&glow(&mut harness, intensity, 0.0)));
    assert!(energies[0] > 1.0, "a visible glow: {energies:?}");
    assert!(energies.windows(2).all(|pair| pair[1] > pair[0]), "it grows with intensity: {energies:?}");
    // Centered on its source.
    let wide = glow(&mut harness, 2.0, 0.0);
    let (middle, _) = centroid(&wide, size, center, 60.0);
    assert!(middle.distance(center) < 2.0, "{middle} vs {center}");
    // Extent: it holds its light much farther out than bloom, which falls off fast past the
    // source (both reach as far as the bloom chain's widest level does).
    let bloom = PostParams {
        bloom: BloomParams { threshold: 0.0, knee: 0.0, intensity: 1.0, scatter: 0.7 },
        ..passthrough()
    };
    let bloom = added(&harness.render(bloom), &plain);
    let reach = |values: &[f32]| {
        let near = ring_sum(values, size, center, radius * 1.5, radius * 3.0);
        let far = ring_sum(values, size, center, radius * 3.0, radius * 6.0);
        far / near
    };
    let (diffusion_reach, bloom_reach) = (reach(&wide), reach(&bloom));
    eprintln!(
        "light 3-6 radii out over 1.5-3 radii out: diffusion {diffusion_reach:.3}, bloom {bloom_reach:.3}"
    );
    assert!(
        diffusion_reach > 1.3 * bloom_reach,
        "far over near light: diffusion {diffusion_reach}, bloom {bloom_reach}"
    );
    assert!(ring_sum(&wide, size, center, radius * 6.0, radius * 8.0) > 1.0, "and still glows 6 radii out");
    // The threshold: a higher one lets less through, and one far above the source's light none.
    let thresholds = [0.0, 0.2, 0.8].map(|threshold| outside(&glow(&mut harness, 2.0, threshold)));
    assert!(thresholds.windows(2).all(|pair| pair[1] < pair[0]), "{thresholds:?}");
    let above = harness.render(diffusion(2.0, 1000.0));
    assert!(max_step(&above, &plain) <= 1, "below its threshold, no glow");
}

#[test]
#[ignore = "needs a GPU"]
fn characters_bloom_with_their_own_threshold_and_strength_with_and_without_temporal_aa() {
    let size = (320, 180);
    let paths = [
        ("4x MSAA", RenderSettings::default(), 1),
        ("TAA", RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() }, 24),
    ];
    for (path, settings, frames) in paths {
        let mut harness = Harness::new(settings, size);
        // Two equally bright spheres: a character on the left, the world on the right.
        harness.sphere(Vec3::new(-1.0, 0.0, 0.0), 0.2, flat([3.0; 3]), true);
        harness.sphere(Vec3::new(1.0, 0.0, 0.0), 0.2, flat([3.0; 3]), false);
        let (left, right) = (harness.project(Vec3::new(-1.0, 0.0, 0.0)), harness.project(Vec3::X));
        let radius = harness.project(Vec3::new(1.2, 0.0, 0.0)).x - right.x;
        let bloom = |character_bloom| PostParams {
            bloom: BloomParams { threshold: 1.0, knee: 0.5, intensity: 1.0, scatter: 0.7 },
            character_bloom,
            ..passthrough()
        };
        let unbloomed = harness.render_frames(frames, passthrough(), |_| {});
        let glow = |harness: &mut Harness, character_bloom| {
            let image = harness.render_frames(frames, bloom(character_bloom), |_| {});
            let added = added(&image, &unbloomed);
            let around = |center| ring_sum(&added, size, center, radius * 1.3, radius * 3.0);
            (around(left), around(right))
        };
        let (character, world) = glow(&mut harness, CharacterBloom::NEUTRAL);
        assert!(world > 1.0, "{path}: the world blooms: {world}");
        assert!((character / world - 1.0).abs() < 0.03, "{path}: neutral, alike: {character} vs {world}");
        let own = CharacterBloom { threshold: 2.0, strength: 0.25 };
        let (masked, world_masked) = glow(&mut harness, own);
        assert!(masked < 0.2 * world_masked, "{path}: the character blooms less: {masked} vs {world_masked}");
        assert!(world_masked > 0.95 * world && world_masked <= world * 1.001, "{path}: the world's holds");
        // Strength alone scales the character's bloom (not exactly in proportion: the Karis
        // average weighs dimmer taps more).
        let (half, _) = glow(&mut harness, CharacterBloom { threshold: 1.0, strength: 0.5 });
        assert!(
            half > 0.35 * character && half < 0.75 * character,
            "{path}: half strength: {half} vs {character}"
        );
        assert!(harness.renderer.stats().post.character_bloom, "{path}");
        eprintln!(
            "{path}: character over world bloom {:.3} neutral, {:.3} with its own, {:.3} at half strength",
            character / world,
            masked / world_masked,
            half / character
        );
    }
}

/// The CPU twin of post.wgsl's `blended`, on sRGB-encoded values.
fn blend(a: f32, b: f32, mode: GradientBlend) -> f32 {
    match mode {
        GradientBlend::Normal => b,
        GradientBlend::Multiply => a * b,
        GradientBlend::Screen => 1.0 - (1.0 - a) * (1.0 - b),
        GradientBlend::Overlay if a < 0.5 => 2.0 * a * b,
        GradientBlend::Overlay => 1.0 - (1.0 - a) * (2.0 - 2.0 * b),
        GradientBlend::SoftLight if b <= 0.5 => a - (1.0 - 2.0 * b) * a * (1.0 - a),
        GradientBlend::SoftLight => {
            let lifted = if a <= 0.25 { ((16.0 * a - 12.0) * a + 4.0) * a } else { a.sqrt() };
            a + (2.0 * b - 1.0) * (lifted - a)
        }
        GradientBlend::Add => (a + b).min(1.0),
    }
}

#[test]
#[ignore = "needs a GPU"]
fn gradient_layers_ramp_across_the_frame_and_blend_as_compositing_tools_do() {
    let size = (160, 90);
    let mut harness = Harness::new(RenderSettings::default(), size);
    let grey = 0.3;
    harness.sky =
        SkyGradient { zenith: Vec3::splat(grey), horizon: Vec3::splat(grey), ground: Vec3::splat(grey) };
    let plain = harness.render(passthrough());
    let base = linear_to_srgb(grey);
    let color = Vec3::new(0.15, 0.55, 0.9);
    let layer = |shape, blend, opacity| GradientLayer { shape, blend, color, opacity, start: 0.0, end: 1.0 };
    let with = |layers: &[GradientLayer], strength: f32| {
        let mut gradients = [GradientLayer::NONE; MAX_GRADIENT_LAYERS];
        gradients[..layers.len()].copy_from_slice(layers);
        PostParams { gradients, filter_strength: strength, ..passthrough() }
    };
    let at = |image: &[[u8; 4]], x: u32, y: u32| {
        let texel = image[(y * size.0 + x) as usize];
        [texel[0], texel[1], texel[2]].map(|v| f32::from(v) / 255.0)
    };
    let top = linear(0.0);
    let modes = [
        GradientBlend::Normal,
        GradientBlend::Multiply,
        GradientBlend::Screen,
        GradientBlend::Overlay,
        GradientBlend::SoftLight,
        GradientBlend::Add,
    ];
    for mode in modes {
        let image = harness.render(with(&[layer(top, mode, 0.8)], 1.0));
        // Full at the top edge, about half its opacity halfway down (smoothstep's middle), none
        // at the bottom.
        for y in [0, size.1 / 2] {
            let ramp = 1.0 - (y as f32 + 0.5) / size.1 as f32;
            let smooth = ramp * ramp * (3.0 - 2.0 * ramp) * 0.8;
            let expected = color.to_array().map(|c| {
                let b = linear_to_srgb(c);
                base + (blend(base, b, mode) - base) * smooth
            });
            let actual = at(&image, size.0 / 2, y);
            for channel in 0..3 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 2.5 / 255.0,
                    "{mode:?} at row {y}: {actual:?} vs {expected:?}"
                );
            }
        }
        assert!(
            (at(&image, size.0 / 2, size.1 - 1)[0] - base).abs() < 2.5 / 255.0,
            "{mode:?}: nothing at the bottom"
        );
    }
    // From the right edge: full there, nothing at the left.
    let right = harness.render(with(&[layer(linear(FRAC_PI_2), GradientBlend::Normal, 1.0)], 1.0));
    let expected = color.to_array().map(linear_to_srgb);
    assert!((at(&right, size.0 - 1, 40)[2] - expected[2]).abs() < 3.0 / 255.0);
    assert!((at(&right, 0, 40)[2] - base).abs() < 2.5 / 255.0);
    // Radial: full at its center, none past its end (half the diagonal here).
    let pool = GradientLayer { end: 0.5, ..layer(radial(Vec2::new(0.25, 0.5)), GradientBlend::Normal, 1.0) };
    let radial = harness.render(with(&[pool], 1.0));
    assert!((at(&radial, size.0 / 4, size.1 / 2)[2] - expected[2]).abs() < 3.0 / 255.0, "the center");
    assert!((at(&radial, size.0 - 1, size.1 / 2)[2] - base).abs() < 2.5 / 255.0, "past its end");
    // Layers composite in order: multiply then screen differs from screen then multiply.
    let multiply = layer(top, GradientBlend::Multiply, 1.0);
    let screen = GradientLayer { color: Vec3::splat(0.4), ..layer(top, GradientBlend::Screen, 1.0) };
    let first = harness.render(with(&[multiply, screen], 1.0));
    let second = harness.render(with(&[screen, multiply], 1.0));
    assert!(max_step(&first, &second) > 10, "the order shows");
    // The player's filter strength scales them: at 0 the frame is untinted.
    let untinted = harness.render(with(&[multiply, screen], 0.0));
    assert_eq!(untinted, plain, "filter strength 0 with a neutral grade: exactly the plain frame");
    assert_eq!(harness.renderer.stats().post.gradient_layers, 0);
    harness.render(with(&[multiply, screen], 1.0));
    assert_eq!(harness.renderer.stats().post.gradient_layers, 2);
}

fn linear(edge: f32) -> GradientShape {
    GradientShape::Linear { edge }
}

fn radial(center: Vec2) -> GradientShape {
    GradientShape::Radial { center }
}

/// The lens flare scene: a black sky with the sun's disc at `sun_uv` on the frame.
fn flare_frame(harness: &mut Harness, sun_uv: Vec2, flare: f32) -> Vec<[u8; 4]> {
    let toward = harness.direction(sun_uv);
    let post = PostParams { lens_flare: LensFlare { intensity: flare, color: Vec3::ONE }, ..passthrough() };
    harness.render_frames(1, post, |packet| {
        packet.celestial.sun.direction = -toward;
        packet.sky_layers.sun =
            SkyDisc { color: Vec3::ONE, intensity: 40.0, glow: Vec3::ZERO, glow_radius: 0.0 };
    })
}

#[test]
#[ignore = "needs a GPU"]
fn lens_flare_elements_sit_along_the_suns_axis_and_fade_as_the_sun_is_covered() {
    let size = (640, 360);
    let mut harness = Harness::new(RenderSettings::default(), size);
    let sun = Vec2::new(0.2, 0.3);
    let plain = flare_frame(&mut harness, sun, 0.0);
    let flared = flare_frame(&mut harness, sun, 1.0);
    let flare = added(&flared, &plain);
    let total: f32 = flare.iter().copied().filter(|v| *v > 0.0).sum();
    assert!(total > 100.0, "a visible flare: {total}");
    assert!(harness.renderer.stats().post.lens_flare);
    // Each ghost sits where the lens puts it along the axis from the sun through the middle.
    let to_pixels = |uv: Vec2| uv * Vec2::new(size.0 as f32, size.1 as f32);
    for (index, element) in DEFAULT_LENS.iter().enumerate() {
        if !matches!(element.kind, FlareKind::Disc | FlareKind::Hexagon) || element.position < 0.5 {
            continue;
        }
        let expected = to_pixels(sun + (Vec2::splat(0.5) - sun) * 2.0 * element.position);
        let reach = element.radius * size.1 as f32;
        if expected.x + reach > size.0 as f32 || expected.y + reach > size.1 as f32 {
            continue;
        }
        let (found, weight) = centroid(&flare, size, expected, reach * 1.2);
        assert!(weight > 1.0, "element {index} drew: {weight}");
        assert!(found.distance(expected) < 1.5, "element {index} at {found}, expected {expected}");
    }
    // A dark sphere in front of the sun puts the flare out; one covering part of its disc dims it.
    let toward = harness.direction(sun);
    let eye = harness.camera.position;
    let side = toward.cross(Vec3::Y).normalize();
    let covered = |offset: f32| {
        let mut covering = Harness::new(RenderSettings::default(), size);
        covering.sphere(eye + toward * 3.0 + side * offset, 0.15, flat([0.02; 3]), false);
        let without = flare_frame(&mut covering, sun, 0.0);
        let with = flare_frame(&mut covering, sun, 1.0);
        added(&with, &without).iter().copied().filter(|v| *v > 0.0).sum::<f32>()
    };
    let hidden = covered(0.0);
    assert!(hidden < 0.01 * total, "covered: {hidden} of {total}");
    let partly = covered(0.15);
    assert!(partly > 0.1 * total && partly < 0.9 * total, "partly covered: {partly} of {total}");
    // A sun off the frame draws none.
    let gone = Vec2::new(-0.05, 0.3);
    let outside = added(&flare_frame(&mut harness, gone, 1.0), &flare_frame(&mut harness, gone, 0.0));
    assert!(outside.iter().all(|v| v.abs() < 1e-3), "off screen, no flare");
}

#[test]
#[ignore = "needs a GPU"]
fn the_vignette_darkens_or_tints_the_corners_and_leaves_the_middle() {
    let size = (160, 90);
    let mut harness = Harness::new(RenderSettings::default(), size);
    let grey = 0.4;
    harness.sky =
        SkyGradient { zenith: Vec3::splat(grey), horizon: Vec3::splat(grey), ground: Vec3::splat(grey) };
    let vignette =
        |color| PostParams { vignette: Vignette { intensity: 0.5, start: 0.3, color }, ..passthrough() };
    let at = |image: &[[u8; 4]], x: u32, y: u32| image[(y * size.0 + x) as usize].map(srgb_to_linear);
    let dark = harness.render(vignette(Vec3::ZERO));
    assert!((at(&dark, size.0 / 2, size.1 / 2)[0] - grey).abs() < 0.01, "the middle is untouched");
    // The corner pixel's center reaches 0.99 of the way: smoothstep there is about 1.
    let corner = at(&dark, 0, 0);
    assert!((corner[0] - grey * 0.5).abs() < 0.01, "half as bright at the corners: {corner:?}");
    // Tinted toward blue: red and green go as before, blue stays.
    let blue = harness.render(vignette(Vec3::new(0.0, 0.0, 1.0)));
    let corner = at(&blue, size.0 - 1, size.1 - 1);
    assert!((corner[0] - grey * 0.5).abs() < 0.01 && (corner[2] - grey).abs() < 0.01, "{corner:?}");
    assert!(harness.renderer.stats().post.vignette_tint);
    assert_eq!(at(&blue, size.0 / 2, size.1 / 2), at(&dark, size.0 / 2, size.1 / 2));
}

#[test]
#[ignore = "needs a GPU"]
fn sharpening_steepens_edges_within_their_neighbors_and_off_draws_the_plain_frame() {
    for (scale, settings) in [(1.0, RenderSettings::default()), (0.5, RenderSettings::default())] {
        let size = (240, 135);
        let mut harness = Harness::new(settings, size);
        harness.renderer.set_render_scale(scale);
        harness.sky =
            SkyGradient { zenith: Vec3::splat(0.6), horizon: Vec3::splat(0.6), ground: Vec3::splat(0.6) };
        harness.sphere(Vec3::ZERO, 1.0, flat([0.1, 0.2, 0.3]), false);
        let soft = harness.render(passthrough());
        let off = harness.render(PostParams { sharpening: 0.0, ..passthrough() });
        assert_eq!(off, soft, "scale {scale}: off is the plain frame");
        assert!(!harness.renderer.stats().post.sharpening);
        let sharp = harness.render(PostParams { sharpening: 1.0, ..passthrough() });
        assert!(harness.renderer.stats().post.sharpening);
        // Across the sphere's left edge, the middle row: the steepest step grows.
        let row = |image: &[[u8; 4]]| -> Vec<i32> {
            let y = size.1 / 2;
            (0..size.0).map(|x| i32::from(image[(y * size.0 + x) as usize][1])).collect()
        };
        let steepest = |row: &[i32]| row.windows(2).map(|pair| (pair[1] - pair[0]).abs()).max().unwrap_or(0);
        let (soft_row, sharp_row) = (row(&soft), row(&sharp));
        assert!(
            steepest(&sharp_row) > steepest(&soft_row),
            "scale {scale}: {} vs {}",
            steepest(&sharp_row),
            steepest(&soft_row)
        );
        // RCAS rings at hard edges (the halo R7 measured), but less than the edge's own contrast:
        // no sharpened value leaves the range of the soft frame's 5 × 3 pixels around it by as
        // much as that range.
        let green = |image: &[[u8; 4]], x: u32, y: u32| i32::from(image[(y * size.0 + x) as usize][1]);
        let y = size.1 / 2;
        let mut ring = 0;
        for x in 2..size.0 - 2 {
            let around: Vec<i32> = (x - 2..=x + 2)
                .flat_map(|x| (y - 1..=y + 1).map(move |y| (x, y)))
                .map(|(x, y)| green(&soft, x, y))
                .collect();
            let (low, high) = (*around.iter().min().expect("15"), *around.iter().max().expect("15"));
            let value = green(&sharp, x, y);
            let past = (low - value).max(value - high).max(0);
            assert!(past < (high - low).max(3), "scale {scale}: x {x}: {value} past {low}..={high}");
            ring = ring.max(past);
        }
        eprintln!("scale {scale}: RCAS rings at most {ring} steps past the edge's neighbors");
        // Flat regions stay as they were.
        let flat_texel = |image: &[[u8; 4]]| image[(10 * size.0 + 10) as usize];
        let (sharp_flat, soft_flat) = (flat_texel(&sharp), flat_texel(&soft));
        assert!((0..3).all(|c| sharp_flat[c].abs_diff(soft_flat[c]) <= 1), "scale {scale}");
        // The sharpening variant's own path, with next to no sharpening, draws the plain frame.
        let barely = harness.render(PostParams { sharpening: 1e-6, ..passthrough() });
        assert!(max_step(&barely, &soft) <= 1, "scale {scale}");
    }
}

/// Every compositing layer at once: what the mobile-against-desktop test compares.
fn composited() -> PostParams {
    let top = GradientLayer {
        shape: GradientShape::Linear { edge: 0.0 },
        blend: GradientBlend::Multiply,
        color: Vec3::new(0.5, 0.6, 0.9),
        opacity: 0.4,
        start: 0.0,
        end: 0.6,
    };
    let pool = GradientLayer {
        shape: GradientShape::Radial { center: Vec2::new(0.5, 0.45) },
        blend: GradientBlend::SoftLight,
        color: Vec3::new(1.0, 0.85, 0.7),
        opacity: 0.3,
        start: 0.0,
        end: 0.9,
    };
    PostParams {
        bloom: BloomParams { threshold: 1.0, knee: 0.5, intensity: 0.4, scatter: 0.7 },
        vignette: Vignette { intensity: 0.3, start: 0.4, color: Vec3::new(0.1, 0.05, 0.2) },
        diffusion: Diffusion { intensity: 1.5, threshold: 0.1, color: Vec3::new(1.0, 0.8, 0.6) },
        character_bloom: CharacterBloom { threshold: 1.5, strength: 0.3 },
        gradients: [top, pool, GradientLayer::NONE, GradientLayer::NONE],
        lens_flare: LensFlare { intensity: 1.0, color: Vec3::new(1.0, 0.8, 0.6) },
        ..passthrough()
    }
}

#[test]
#[ignore = "needs a GPU"]
fn mobile_lows_fused_pass_composites_as_desktop_does() {
    // At 1080 rows Mobile-Low renders 590 and has four bloom levels; Desktop-Low renders all
    // 1080 rows with five. The same scene at a sixth of that, each tier at its own scale.
    let size = (320, 180);
    let table = TierTable::shipped();
    let tiers = [TierId::DesktopLow, TierId::MobileLow].map(|id| {
        let tier = table.get(id);
        // One frame shows everything, so both draw with 4× MSAA.
        let settings = RenderSettings { msaa: Msaa::X4, temporal: false, ..tier.render_settings() };
        (id, settings, tier.resolution.base_scale(1080))
    });
    let mut contributions = Vec::new();
    for (id, settings, scale) in tiers {
        let mut harness = Harness::new(settings, size);
        harness.renderer.set_render_scale(scale);
        harness.sky = SkyGradient {
            zenith: Vec3::new(0.15, 0.3, 0.7),
            horizon: Vec3::new(0.9, 0.7, 0.5),
            ground: Vec3::new(0.2, 0.15, 0.25),
        };
        harness.sphere(Vec3::new(-0.8, 0.0, 0.0), 0.5, flat([2.5, 1.8, 1.4]), true);
        harness.sphere(Vec3::new(0.9, 0.3, 0.0), 0.35, flat([3.0; 3]), false);
        let sun_uv = Vec2::new(0.8, 0.25);
        let toward = harness.direction(sun_uv);
        let fill = |packet: &mut FramePacket| {
            packet.celestial.sun.direction = -toward;
            packet.sky_layers.sun =
                SkyDisc { color: Vec3::ONE, intensity: 30.0, glow: Vec3::ZERO, glow_radius: 0.0 };
        };
        let plain = harness.render_frames(1, composited().plain(), fill);
        let layered = harness.render_frames(1, composited(), fill);
        let stats = harness.renderer.stats();
        assert!(stats.post.diffusion && stats.post.lens_flare && stats.post.character_bloom, "{id:?}");
        contributions.push((id, plain, layered));
    }
    let [(_, desktop_plain, desktop), (_, mobile_plain, mobile)] = &contributions[..] else {
        panic!("two tiers")
    };
    // The layers add the same to each tier's own frame: the difference between the two tiers'
    // composites is no more than the difference between their plain frames (resolution, bloom
    // levels), plus a little for the diffusion's level, a different size on each.
    let plain_difference = mean_step(desktop_plain, mobile_plain);
    let layered_difference = mean_step(desktop, mobile);
    assert!(
        layered_difference < plain_difference + 1.0,
        "composited {layered_difference:.3} vs plain {plain_difference:.3} mean steps apart"
    );
    let layer_effect = mean_step(desktop, desktop_plain);
    assert!(layer_effect > 5.0, "the layers change the frame: {layer_effect:.3}");
    eprintln!(
        "Mobile-Low vs Desktop-Low: composited {layered_difference:.3}, plain {plain_difference:.3} mean steps \
         apart; the layers move Desktop-Low's frame by {layer_effect:.3}"
    );
}

#[test]
#[ignore = "needs a GPU"]
fn editing_post_wgsl_rebuilds_every_post_pipeline_and_draws_the_same_pixels() {
    let size = (160, 90);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sky =
        SkyGradient { zenith: Vec3::splat(0.3), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.2) };
    harness.sphere(Vec3::ZERO, 1.0, flat([2.0, 1.0, 0.5]), true);
    let before = harness.render(composited());
    let mut sources = ShaderSources::compiled_in();
    sources.set(ShaderFile::Post, format!("{}\n// An edit.\n", ShaderFile::Post.compiled_in()));
    let update = harness.renderer.set_shader_sources(&sources).expect("a valid edit");
    assert_eq!((update.programs, update.pipelines), (1, super::Pipelines::COUNT));
    assert_eq!(harness.render(composited()), before, "a comment changes no pixel");
}
