//! Temporal AA tests that need a GPU (M1 R7). Run with
//! `cargo test -p sapphire_render --test temporal -- --ignored`.
//!
//! Set `SAPPHIRE_TAA_DUMP=<dir>` to write the frames the tests compare there as PNGs.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, LightSplit, Material, Outline, Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
    VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor, GraphReport, TextureId};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::temporal::{jitter, jitter_phases};
use sapphire_render::{
    BloomParams, CameraData, Celestial, ColorGrade, DirectionalLight, FramePacketRing, HistoryReset,
    InstanceId, MeshHandle, MeshInstance, Msaa, PostParams, PrepassTargets, RenderSettings, Renderer,
    ShaderError, ShaderFile, ShaderSources, ShaderUpdate, SkyDisc, SkyGradient, SkyLayers, ToneCurve,
    ViewData, Vignette, scaled_size,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

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

fn cooked(material: &AnimeCharacterMaterial) -> Material {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(material, &swizzles).expect("a valid material")
}

/// A flat material in `rgb`: lit everywhere (no shadow tone, no rim), with a black outline
/// `outline_px` wide at 1080 rows.
fn flat(rgb: [f32; 3], outline_px: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: [1.0; 3],
            outline_color: [0.0; 3],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: outline_px, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// [`flat`] with a two-tone light split: the side away from the sun in a third of the color.
fn two_tone(rgb: [f32; 3], outline_px: f32) -> AnimeCharacterMaterial {
    let mut material = flat(rgb, outline_px);
    material.regions[0].shade_color = [0.35; 3];
    material
}

fn mesh(vertices: &[Vertex], indices: Vec<u32>, material: &AnimeCharacterMaterial) -> Mesh {
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("surface", cooked(material))];
    Mesh::from_vertices(vertices, indices, submeshes, slots, PositionFormat::Float32)
}

fn vertex(position: Vec3, normal: Vec3) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// A unit UV sphere at the origin.
fn sphere(material: &AnimeCharacterMaterial) -> Mesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(vertex(n, n));
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
    mesh(&vertices, indices, material)
}

/// `sphere` with a morph target of zero deltas: its instances are characters (the character
/// prepass draws skinned and morphed instances) and look exactly like the sphere.
fn character_sphere(material: &AnimeCharacterMaterial) -> Mesh {
    let mut mesh = sphere(material);
    mesh.morph_targets =
        vec![MorphTarget { name: "still".into(), default_weight: 0.0, first_delta: 0, delta_count: 1 }];
    mesh.morph_deltas = vec![MorphDelta::new(0, [0.0; 3], [0.0; 3])];
    mesh
}

/// Rectangles in the plane z = 0 facing +Z, each `[min, max]` in x and y, without outlines.
fn rectangles(rects: &[[[f32; 2]; 2]], material: &AnimeCharacterMaterial) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for [min, max] in rects {
        let base = vertices.len() as u32;
        for (x, y) in [(min[0], min[1]), (max[0], min[1]), (max[0], max[1]), (min[0], max[1])] {
            vertices.push(vertex(Vec3::new(x, y, 0.0), Vec3::Z));
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let material =
        AnimeCharacterMaterial { outline: Outline { width: 0.0, ..Outline::default() }, ..material.clone() };
    mesh(&vertices, indices, &material)
}

/// `count` bars `width` wide and 4 high across x ∈ [-span, span]: hard edges with the sky
/// between them.
fn comb(count: u32, width: f32, span: f32, material: &AnimeCharacterMaterial) -> Mesh {
    let bars: Vec<[[f32; 2]; 2]> = (0..count)
        .map(|bar| {
            let x = -span + 2.0 * span * (bar as f32 + 0.5) / count as f32;
            [[x - width * 0.5, -2.0], [x + width * 0.5, 2.0]]
        })
        .collect();
    rectangles(&bars, material)
}

fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("a valid mesh");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

/// Post that shows linear values below about 0.98 as they are.
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

/// A camera at `eye` looking down -Z, `height` world units filling the image's height in the
/// plane z = 0 (`pixels per unit` = target height / `height` there).
fn camera(size: (u32, u32), eye: Vec3, height: f32) -> CameraData {
    let fov = 30f32.to_radians();
    let distance = height * 0.5 / (fov * 0.5).tan();
    let eye = Vec3::new(eye.x, eye.y, eye.z + distance);
    CameraData::look_at(eye, eye - Vec3::Z, Vec3::Y, fov, size.0 as f32 / size.1 as f32, 0.05)
}

fn instance(id: u64, mesh: MeshHandle, transform: Mat4) -> MeshInstance {
    MeshInstance { id: InstanceId(id), mesh, transform }
}

fn taa_settings() -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() }
}

/// What to read back after a frame: a texture from the frame's targets.
type Read<'a> = (fn(&PrepassTargets) -> Option<TextureId>, &'a Readback);

/// A renderer and its frames, drawing into one target.
struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    sky: SkyGradient,
    /// Where the bodies and the stars are, and what the sky draws of them (M1 W2): by default
    /// the gradient alone.
    layers: Option<(Celestial, SkyLayers)>,
}

impl Frames {
    fn new(gpu: &GpuContext, settings: RenderSettings, size: (u32, u32)) -> Self {
        let mut graph = GraphExecutor::new();
        graph.set_report(true);
        Self {
            renderer: Renderer::new(gpu, FORMAT, settings).expect("renderer"),
            graph,
            packets: FramePacketRing::new(),
            target: target(gpu, size),
            sky: SkyGradient {
                zenith: Vec3::splat(0.5),
                horizon: Vec3::splat(0.5),
                ground: Vec3::splat(0.5),
            },
            layers: None,
        }
    }

    fn taa(gpu: &GpuContext, size: (u32, u32)) -> Self {
        Self::new(gpu, taa_settings(), size)
    }

    fn msaa(gpu: &GpuContext, size: (u32, u32)) -> Self {
        Self::new(gpu, RenderSettings::default(), size)
    }

    fn size(&self) -> (u32, u32) {
        (self.target.width(), self.target.height())
    }

    fn report(&self) -> &GraphReport {
        self.graph.report().expect("the report is on")
    }

    /// Draws one frame from `view` with the `changed` instances, reading `reads` back, and
    /// returns the target's pixels. Fails on a validation error.
    fn frame(
        &mut self,
        gpu: &GpuContext,
        view: ViewData,
        changed: &[MeshInstance],
        reads: &[Read<'_>],
    ) -> Vec<[u8; 4]> {
        let packet = self.packets.begin();
        packet.views.push(view);
        // Along the view, so everything the camera sees is lit.
        packet.sun =
            DirectionalLight { direction: Vec3::new(0.3, -0.2, -1.0), color: Vec3::ONE, intensity: 1.0 };
        packet.sky = self.sky;
        if let Some((celestial, layers)) = self.layers {
            packet.celestial = celestial;
            packet.sky_layers = layers;
        }
        packet.post = passthrough();
        packet.instances.changed.extend_from_slice(changed);

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let target_view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &target_view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        let reads: Vec<(TextureId, &Readback)> = reads
            .iter()
            .map(|(pick, readback)| (pick(&targets).expect("present this frame"), *readback))
            .collect();
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
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        read_back(gpu, &self.target)
    }

    /// Resizes the target, as a window resize does.
    fn resize(&mut self, gpu: &GpuContext, size: (u32, u32)) {
        self.target = target(gpu, size);
    }
}

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

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let readback = Readback::new(gpu, (texture.width(), texture.height()), 4);
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    readback.copy(&mut encoder, texture);
    gpu.queue().submit([encoder.finish()]);
    readback.texels(gpu).into_iter().map(|texel| [texel[0], texel[1], texel[2], texel[3]]).collect()
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// An `Rg16Float` texel's two values.
fn rg16f(texel: &[u8]) -> [f32; 2] {
    [u16::from_le_bytes([texel[0], texel[1]]), u16::from_le_bytes([texel[2], texel[3]])].map(f16_to_f32)
}

fn depth(texel: &[u8]) -> f32 {
    f32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]])
}

/// Writes `pixels` as `<name>.png` into `SAPPHIRE_TAA_DUMP`, if set.
fn dump(name: &str, (width, height): (u32, u32), pixels: &[[u8; 4]]) {
    let Some(dir) = std::env::var_os("SAPPHIRE_TAA_DUMP") else { return };
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(&path).expect("create"));
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    writer.write_image_data(pixels.as_flattened()).expect("write");
}

/// How two images differ: the mean and 99th percentile of each pixel's largest channel
/// difference in 8-bit steps, and the count of pixels over `threshold`.
#[derive(Debug)]
struct Difference {
    mean: f64,
    p99: u8,
    over: usize,
}

fn difference(a: &[[u8; 4]], b: &[[u8; 4]], threshold: u8) -> Difference {
    let mut differences: Vec<u8> =
        a.iter().zip(b).map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)).collect();
    let mean = differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
    let over = differences.iter().filter(|&&d| d > threshold).count();
    differences.sort_unstable();
    Difference { mean, p99: differences[differences.len() * 99 / 100], over }
}

/// Pixels within `radius` of an edge of `reference`: where its neighborhood spans more than
/// `contrast` steps. Antialiasing differs there; ghosts and lag show outside it.
fn edge_zone(reference: &[[u8; 4]], (width, height): (u32, u32), radius: i32) -> Vec<bool> {
    const CONTRAST: u8 = 24;
    let at = |x: i32, y: i32| {
        reference[(y.clamp(0, height as i32 - 1) * width as i32 + x.clamp(0, width as i32 - 1)) as usize]
    };
    (0..height as i32)
        .flat_map(|y| (0..width as i32).map(move |x| (x, y)))
        .map(|(x, y)| {
            let center = at(x, y);
            (-radius..=radius).any(|dy| {
                (-radius..=radius)
                    .any(|dx| (0..3).any(|c| at(x + dx, y + dy)[c].abs_diff(center[c]) > CONTRAST))
            })
        })
        .collect()
}

/// The largest difference between `a` and `b` outside `zone`, and how many pixels there differ
/// by more than `threshold`.
fn outside(a: &[[u8; 4]], b: &[[u8; 4]], zone: &[bool], threshold: u8) -> (u8, usize) {
    let differences = a
        .iter()
        .zip(b)
        .zip(zone)
        .filter(|(_, edge)| !**edge)
        .map(|((a, b), _)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0));
    differences.fold((0, 0), |(max, over), d| (max.max(d), over + usize::from(d > threshold)))
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(value: f32) -> u8 {
    let c = if value <= 0.0031308 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `pixels` of a target `factor` times `size` in each direction, averaged down to `size` in
/// linear light: with 4× MSAA and a factor of 4, 64 samples a pixel.
fn downsample(pixels: &[[u8; 4]], (width, height): (u32, u32), factor: u32) -> Vec<[u8; 4]> {
    let wide = (width * factor) as usize;
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let mut sum = [0.0f32; 3];
            for sy in 0..factor {
                for sx in 0..factor {
                    let p = pixels[((y * factor + sy) as usize) * wide + (x * factor + sx) as usize];
                    for c in 0..3 {
                        sum[c] += srgb_to_linear(p[c]);
                    }
                }
            }
            let n = (factor * factor) as f32;
            [linear_to_srgb(sum[0] / n), linear_to_srgb(sum[1] / n), linear_to_srgb(sum[2] / n), 255]
        })
        .collect()
}

fn translate(x: f32, y: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, 0.0)) * Mat4::from_scale(Vec3::splat(scale))
}

#[test]
#[ignore = "needs a GPU"]
fn the_projection_moves_by_the_jitter_sequence() {
    let gpu = gpu();
    let size = (200, 120);
    let mut frames = Frames::taa(&gpu, size);
    // One world unit per 60 pixels at z = 0: a rectangle covering everything left of x = 100.3
    // and above y = 60.3 in pixels.
    let pixels_per_unit = 60.0;
    let edge = |pixels: f32, half: f32| (pixels - half) / pixels_per_unit;
    let right = edge(100.3, 100.0);
    let bottom = -edge(60.3, 60.0);
    let quad =
        upload(&mut frames.renderer, &rectangles(&[[[-10.0, bottom], [right, 10.0]]], &flat([1.0; 3], 0.0)));
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    let readback = Readback::new(&gpu, size, 4);
    let phases = jitter_phases(size.1, size.1);
    for frame in 0..2 * u64::from(phases) {
        let changed = [instance(0, quad, Mat4::IDENTITY)];
        frames.frame(&gpu, view, if frame == 0 { &changed } else { &[] }, &[(|t| Some(t.depth), &readback)]);
        let texels = readback.texels(&gpu);
        let covered = |x: u32, y: u32| depth(&texels[(y * size.0 + x) as usize]) > 0.0;
        // Pixel p samples p + 0.5 - jitter of the unjittered image: x = 100 is covered when
        // 100.5 - jitter < 100.3, row 60 when 60.5 - jitter < 60.3.
        let [jx, jy] = jitter(frame, phases);
        assert_eq!(covered(100, 30), jx > 0.2, "frame {frame}: x jitter {jx}");
        assert_eq!(covered(99, 30), jx > -0.8, "frame {frame}");
        assert_eq!(covered(30, 60), jy > 0.2, "frame {frame}: y jitter {jy}");
        let stats = frames.renderer.stats().temporal;
        assert_eq!((stats.jitter_index, stats.jitter_phases), ((frame % u64::from(phases)) as u32, phases));
        assert_eq!(stats.jitter(), [jx, jy]);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn velocity_ignores_the_jitter() {
    let gpu = gpu();
    let size = (160, 90);
    let mut frames = Frames::taa(&gpu, size);
    let ball = upload(&mut frames.renderer, &sphere(&flat([0.8; 3], 4.0)));
    let quad = upload(&mut frames.renderer, &rectangles(&[[[-0.3, -0.3], [0.3, 0.3]]], &flat([0.5; 3], 0.0)));
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    let velocity = Readback::new(&gpu, size, 4);
    for frame in 0..10 {
        let changed = [instance(0, ball, translate(-0.5, 0.0, 0.4))];
        frames.frame(
            &gpu,
            view,
            if frame == 0 { &changed } else { &[] },
            &[(|t| Some(t.velocity), &velocity)],
        );
        let texels = velocity.texels(&gpu);
        // Exactly zero (the y axis's sign flip leaves -0 in places).
        assert!(texels.iter().all(|texel| rg16f(texel) == [0.0, 0.0]), "frame {frame}: still geometry moves");
    }
    // A rectangle in the plane z = 0 moving 3 pixels right a frame moves exactly that,
    // whatever the jitter (45 pixels per unit).
    let step = 3.0 / 45.0;
    for frame in 0..10 {
        let at = Mat4::from_translation(Vec3::new(0.5 + step * frame as f32, 0.0, 0.0));
        frames.frame(&gpu, view, &[instance(1, quad, at)], &[(|t| Some(t.velocity), &velocity)]);
        if frame == 0 {
            continue;
        }
        let texels = velocity.texels(&gpu);
        let center = ((size.1 / 2) * size.0 + size.0 / 2 + 22 + 3 * frame) as usize;
        let [vx, vy] = rg16f(&texels[center]);
        assert!((vx * size.0 as f32 - 3.0).abs() < 5e-3 && vy == 0.0, "frame {frame}: {vx} {vy}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_still_scene_converges_to_msaa_and_holds_still() {
    let gpu = gpu();
    let size = (480, 270);
    // An outlined two-tone sphere (the sun from the side splits it) before a comb of bars.
    let material = two_tone([0.9, 0.55, 0.3], 5.5);
    let bars = flat([0.1, 0.15, 0.3], 0.0);
    let draw = |settings: RenderSettings, size: (u32, u32), frames: u32| -> Vec<Vec<[u8; 4]>> {
        let mut renderer = Frames::new(&gpu, settings, size);
        let ball = upload(&mut renderer.renderer, &sphere(&material));
        let comb = upload(&mut renderer.renderer, &comb(9, 0.06, 1.6, &bars));
        let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
        let changed = [
            instance(0, ball, translate(0.0, 0.0, 0.6)),
            instance(1, comb, Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0))),
        ];
        (0..frames)
            .map(|frame| renderer.frame(&gpu, view, if frame == 0 { &changed } else { &[] }, &[]))
            .collect()
    };
    // The truth: 64 samples a pixel, 4× MSAA at four times the size, averaged down.
    let truth = downsample(&draw(RenderSettings::default(), (size.0 * 4, size.1 * 4), 1)[0], size, 4);
    let msaa = difference(&draw(RenderSettings::default(), size, 1)[0], &truth, 32);
    let taa = draw(taa_settings(), size, 48);
    let first = difference(&taa[0], &truth, 32);
    let converged = difference(&taa[47], &truth, 32);
    dump("still_taa", size, &taa[47]);
    dump("still_truth", size, &truth);
    eprintln!(
        "against 64 samples a pixel: one sample {first:?}, 4× MSAA {msaa:?}, converged TAA {converged:?}"
    );
    // Converged, it is within twice 4× MSAA's distance from the truth (the rest is the filter's
    // shape: a Gaussian against the reference's box), and far closer than one sample. Measured
    // on an M5: one sample 1.25 mean, 2,012 pixels over 32; MSAA 0.32, 193; TAA 0.42, 261.
    assert!(
        converged.mean < msaa.mean * 2.0 && converged.over < msaa.over * 2,
        "{converged:?} against {msaa:?}"
    );
    assert!(converged.mean * 2.0 < first.mean, "{converged:?} against the first frame's {first:?}");
    // After convergence each frame repeats the last within a step or two nearly everywhere: on
    // an M5 the mean change is at most 0.06 steps and at most 127 of 129,600 pixels move by
    // more, where a jitter phase's samples all miss a thin feature's end and its clip box
    // closes on the background.
    let changes: Vec<Difference> =
        (32..48).map(|frame| difference(&taa[frame], &taa[frame - 1], 2)).collect();
    let worst_mean = changes.iter().map(|change| change.mean).fold(0.0, f64::max);
    let worst_over = changes.iter().map(|change| change.over).max().unwrap_or(0);
    eprintln!("frame to frame, converged: mean at most {worst_mean:.3}, at most {worst_over} pixels over 2");
    for (frame, change) in (32..48).zip(&changes) {
        assert!(change.mean < 0.1 && change.over < 260, "frame {frame}: {change:?}");
    }
}

/// Draws a comb of `bars` bars behind a sphere with `taa` and `msaa` alike for `frames`
/// frames, seen by `view(frame)` with the sphere at `ball(frame)`, and returns the last frames.
fn render_both(
    gpu: &GpuContext,
    size: (u32, u32),
    frames: u32,
    (sky, bars): (SkyGradient, u32),
    view: impl Fn(u32) -> CameraData,
    ball: impl Fn(u32) -> Mat4,
) -> (Vec<[u8; 4]>, Vec<[u8; 4]>) {
    let mut taa = Frames::taa(gpu, size);
    let mut msaa = Frames::msaa(gpu, size);
    let sphere_mesh = sphere(&flat([0.5, 0.5, 0.55], 0.0));
    let comb_mesh = comb(bars, 0.84 / bars as f32, 2.0, &flat([0.08, 0.08, 0.1], 0.0));
    let mut last = (Vec::new(), Vec::new());
    for frames_of in [&mut taa, &mut msaa] {
        frames_of.sky = sky;
        let ball_mesh = upload(&mut frames_of.renderer, &sphere_mesh);
        let comb_handle = upload(&mut frames_of.renderer, &comb_mesh);
        let mut pixels = Vec::new();
        for frame in 0..frames {
            let view = ViewData::new(view(frame));
            let comb_at = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.5));
            let changed = [instance(0, ball_mesh, ball(frame)), instance(1, comb_handle, comb_at)];
            pixels = frames_of.frame(gpu, view, &changed, &[]);
        }
        if last.0.is_empty() {
            last.0 = pixels;
        } else {
            last.1 = pixels;
        }
    }
    last
}

#[test]
#[ignore = "needs a GPU"]
fn camera_motion_reprojects_geometry_and_the_sky() {
    let gpu = gpu();
    let size = (480, 270);
    // A sky that darkens steeply above the horizon, so a sky that isn't reprojected from depth
    // lags visibly behind a turning camera.
    let sky = SkyGradient {
        zenith: Vec3::new(0.02, 0.05, 0.3),
        horizon: Vec3::splat(0.9),
        ground: Vec3::splat(0.6),
    };
    // The camera slides across the comb, so the bars move against the sky (parallax), while it
    // turns right and tilts up, so the sky moves as well: about 2.5 and 1.5 pixels a frame.
    let view = |frame: u32| {
        let t = frame as f32;
        let eye = Vec3::new(t * 1.5 / 135.0, 0.2, 3.7);
        let (yaw, pitch) = (-t * 0.003, -0.05 + t * 0.0018);
        let forward = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
        let fov = 30f32.to_radians();
        CameraData::look_at(eye, eye + forward, Vec3::Y, fov, size.0 as f32 / size.1 as f32, 0.05)
    };
    let (taa, msaa) = render_both(&gpu, size, 24, (sky, 24), view, |_| translate(0.0, 0.3, 0.25));
    dump("camera_taa", size, &taa);
    dump("camera_msaa", size, &msaa);
    let edges = edge_zone(&msaa, size, 1);
    let (worst, over) = outside(&taa, &msaa, &edges, 12);
    let overall = difference(&taa, &msaa, 48);
    eprintln!("camera motion: away from edges max {worst}, {over} over 12; overall {overall:?}");
    // Only the edges' antialiasing differs, softer in motion: no bar ghosts behind the pan. With
    // velocity ignored, 6,050 pixels differed by more than 48 and the 99th percentile by 99.
    assert!(over < 100 && worst < 32, "{over} pixels away from edges differ, by up to {worst}");
    assert!(overall.over < 2000 && overall.p99 < 64, "{overall:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn outlines_keep_their_place_and_darkness_in_motion() {
    let gpu = gpu();
    let size = (960, 540);
    // The default outline, 5.5 px at 1080 rows: 2.75 px here, most of it beyond what a 3 × 3
    // velocity dilation reaches from the silhouette, since the hull draws no velocity.
    let material = flat([0.9, 0.6, 0.3], 5.5);
    let speed = 4.5 / 270.0;
    let mut taa_frames = Frames::new(&gpu, taa_settings(), size);
    let mut msaa_frames = Frames::new(&gpu, RenderSettings::default(), size);
    let taa_ball = upload(&mut taa_frames.renderer, &sphere(&material));
    let msaa_ball = upload(&mut msaa_frames.renderer, &sphere(&material));
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    // Along the middle row, each edge's outline: its total darkness (against the sky's and the
    // sphere's own brightness, both well above it) and where its darkness centers.
    let row = (size.1 / 2 * size.0) as usize;
    let edge = |pixels: &[[u8; 4]], range: std::ops::Range<usize>| {
        let dark: Vec<(usize, f32)> =
            range.map(|x| (x, (1.0 - f32::from(pixels[row + x][1]) / 188.0).clamp(0.0, 1.0))).collect();
        let total: f32 = dark.iter().map(|(_, d)| d).sum();
        let center = dark.iter().map(|(x, d)| *x as f32 * d).sum::<f32>() / total.max(1e-6);
        (total, center)
    };
    // A single frame's outline depends on where its jitter phase and the sphere's sub-pixel
    // position put the hull's samples, so each side is averaged over one jitter cycle (8
    // frames) after 16 to settle: TAA's darkness, 4× MSAA's, and how far TAA's center is from
    // MSAA's.
    let mut sides = [(0.0f32, 0.0f32, 0.0f32); 2];
    for frame in 0..24 {
        let at = translate(-0.6 + speed * frame as f32, 0.0, 0.5);
        let taa = taa_frames.frame(&gpu, view, &[instance(0, taa_ball, at)], &[]);
        let msaa = msaa_frames.frame(&gpu, view, &[instance(0, msaa_ball, at)], &[]);
        if frame < 16 {
            continue;
        }
        dump("outline_taa", size, &taa);
        dump("outline_msaa", size, &msaa);
        let center = (size.0 as f32 / 2.0 + (-0.6 + speed * frame as f32) * 270.0) as usize;
        let ranges = [center - 160..center - 120, center + 120..center + 160];
        for (side, range) in sides.iter_mut().zip(ranges) {
            let ((taa_total, taa_center), (msaa_total, msaa_center)) =
                (edge(&taa, range.clone()), edge(&msaa, range));
            side.0 += taa_total / 8.0;
            side.1 += msaa_total / 8.0;
            side.2 += (taa_center - msaa_center) / 8.0;
        }
    }
    for (name, (taa_total, msaa_total, offset)) in ["trailing", "leading"].into_iter().zip(sides) {
        eprintln!(
            "{name}: darkness {taa_total:.2} against 4× MSAA's {msaa_total:.2}, {offset:+.2} px from its center"
        );
        assert!(
            taa_total > msaa_total * 0.8 && taa_total < msaa_total * 1.25,
            "{name}: {taa_total} against {msaa_total}"
        );
        assert!(offset.abs() < 1.0, "{name}: {offset} px off");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_responsive_mask_marks_visible_characters() {
    let gpu = gpu();
    let size = (240, 135);
    let mut frames = Frames::taa(&gpu, size);
    let material = flat([0.8; 3], 0.0);
    let character = upload(&mut frames.renderer, &character_sphere(&material));
    let ball = upload(&mut frames.renderer, &sphere(&material));
    let wall = upload(&mut frames.renderer, &rectangles(&[[[-0.6, -1.0], [-0.45, 1.0]]], &material));
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    let mask = Readback::new(&gpu, size, 1);
    let changed = [
        instance(0, character, translate(-0.5, 0.0, 0.35)),
        instance(1, ball, translate(0.5, 0.0, 0.35)),
        // In front of the character's middle.
        instance(2, wall, Mat4::from_translation(Vec3::new(0.0, 0.0, 0.5))),
    ];
    frames.frame(&gpu, view, &changed, &[(|t| t.responsive, &mask)]);
    let texels = mask.texels(&gpu);
    let at = |x: f32, y: f32| {
        let pixel = (size.0 as f32 / 2.0 + x * 67.5, size.1 as f32 / 2.0 - y * 67.5);
        texels[pixel.1 as usize * size.0 as usize + pixel.0 as usize][0]
    };
    // In perspective the wall, nearer the camera, covers x ∈ [-0.69, -0.52] on screen, and the
    // character x ∈ [-0.89, -0.13].
    assert_eq!(at(-0.8, 0.0), 255, "the character, left of the wall");
    assert_eq!(at(-0.35, 0.1), 255, "the character, right of it");
    assert_eq!(at(-0.6, 0.0), 0, "behind the wall");
    assert_eq!(at(0.5, 0.0), 0, "a static sphere");
    assert_eq!(at(0.0, 0.5), 0, "the sky");
    let marked = texels.iter().filter(|texel| texel[0] == 255).count();
    assert!(texels.iter().all(|texel| texel[0] == 0 || texel[0] == 255));
    assert!(marked > 1000, "{marked} pixels");
}

#[test]
#[ignore = "needs a GPU"]
fn upsampling_restores_the_detail_a_bilinear_upscale_loses() {
    let gpu = gpu();
    let size = (480, 270);
    let material = two_tone([0.9, 0.55, 0.3], 5.5);
    let bars = flat([0.1, 0.15, 0.3], 0.0);
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    let draw = |settings: RenderSettings, scale: f32, frames: u32| {
        let mut renderer = Frames::new(&gpu, settings, size);
        let meshes = [
            upload(&mut renderer.renderer, &sphere(&material)),
            upload(&mut renderer.renderer, &comb(9, 0.06, 1.6, &bars)),
        ];
        renderer.renderer.set_render_scale(scale);
        let changed = [
            instance(0, meshes[0], translate(0.0, 0.0, 0.6)),
            instance(1, meshes[1], Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0))),
        ];
        let mut pixels = renderer.frame(&gpu, view, &changed, &[]);
        for _ in 1..frames {
            pixels = renderer.frame(&gpu, view, &[], &[]);
        }
        (pixels, renderer)
    };
    let (native, _) = draw(RenderSettings::default(), 1.0, 1);
    for scale in [2.0 / 3.0, 0.5] {
        let (upsampled, frames) = draw(taa_settings(), scale, 48);
        let (bilinear, _) = draw(RenderSettings::default(), scale, 1);
        // The scene renders at the scale; the resolve writes the target's size.
        let scene = scaled_size(size, scale);
        let texture = |name: &str| frames.report().textures.iter().find(|t| t.name == name).map(|t| t.size);
        assert_eq!(texture("scene color"), Some(scene));
        assert_eq!(texture("resolved"), Some(size));
        let stats = frames.renderer.stats().temporal;
        assert_eq!((stats.input_size, stats.output_size), (scene, size));
        assert_eq!(stats.jitter_phases, jitter_phases(scene.1, size.1));
        let taa = difference(&upsampled, &native, 32);
        let old = difference(&bilinear, &native, 32);
        eprintln!("scale {scale:.2}: TAAU {taa:?}, bilinear {old:?}");
        dump(&format!("upsampled_{}", (scale * 100.0) as u32), size, &upsampled);
        assert!(
            taa.mean < old.mean * 0.6 && taa.over < old.over * 3 / 4,
            "scale {scale}: {taa:?} against {old:?}"
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn history_resets_on_start_resize_and_cut_but_not_on_rescale() {
    let gpu = gpu();
    let mut frames = Frames::taa(&gpu, (160, 90));
    let ball = upload(&mut frames.renderer, &sphere(&flat([0.8, 0.4, 0.2], 5.5)));
    let view = ViewData::new(camera(frames.size(), Vec3::ZERO, 2.0));
    let reset = |frames: &Frames| {
        let stats = frames.renderer.stats().temporal;
        (stats.resets, stats.reset)
    };
    frames.frame(&gpu, view, &[instance(0, ball, translate(0.0, 0.0, 0.5))], &[]);
    assert_eq!(reset(&frames), (1, Some(HistoryReset::Start)));
    for _ in 0..4 {
        frames.frame(&gpu, view, &[], &[]);
        assert_eq!(reset(&frames), (1, None));
    }
    // A render-scale change keeps the history: it is at the output's size.
    frames.renderer.set_render_scale(0.75);
    frames.frame(&gpu, view, &[], &[]);
    assert_eq!(reset(&frames), (1, None), "a new render scale");
    frames.frame(&gpu, ViewData { cut: true, ..view }, &[], &[]);
    assert_eq!(reset(&frames), (2, Some(HistoryReset::Cut)));
    frames.frame(&gpu, view, &[], &[]);
    assert_eq!(reset(&frames), (2, None), "a cut resets one frame");
    frames.resize(&gpu, (200, 100));
    let view = ViewData::new(camera(frames.size(), Vec3::ZERO, 2.0));
    frames.frame(&gpu, view, &[], &[]);
    assert_eq!(reset(&frames), (3, Some(HistoryReset::Resize)));
    // A renderer made again (device loss, a tier change) starts over.
    let mut again = Frames::taa(&gpu, (200, 100));
    let ball = upload(&mut again.renderer, &sphere(&flat([0.8, 0.4, 0.2], 5.5)));
    again.frame(&gpu, view, &[instance(0, ball, translate(0.0, 0.0, 0.5))], &[]);
    assert_eq!(reset(&again), (1, Some(HistoryReset::Start)));
}

#[test]
#[ignore = "needs a GPU"]
fn a_render_scale_step_keeps_the_image_steady() {
    let gpu = gpu();
    let size = (480, 270);
    let mut frames = Frames::taa(&gpu, size);
    let ball = upload(&mut frames.renderer, &sphere(&two_tone([0.9, 0.55, 0.3], 5.5)));
    let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
    let mut converged = frames.frame(&gpu, view, &[instance(0, ball, translate(0.0, 0.0, 0.6))], &[]);
    for _ in 1..40 {
        converged = frames.frame(&gpu, view, &[], &[]);
    }
    // Dynamic resolution's step: the next frame stays antialiased instead of starting over.
    frames.renderer.set_render_scale(0.85);
    let stepped = frames.frame(&gpu, view, &[], &[]);
    let step = difference(&stepped, &converged, 32);
    frames.renderer.set_render_scale(1.0);
    let cut = frames.frame(&gpu, ViewData { cut: true, ..view }, &[], &[]);
    let restart = difference(&cut, &converged, 32);
    eprintln!("render-scale step {step:?}, reset {restart:?}");
    assert!(step.over * 3 < restart.over && step.mean < 0.5, "{step:?} against a reset's {restart:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn mobile_tiers_keep_msaa_in_tile_memory() {
    let gpu = gpu();
    let table = TierTable::shipped();
    for id in [TierId::MobileHigh, TierId::MobileLow] {
        let mut frames = Frames::new(&gpu, table.get(id).render_settings(), (320, 180));
        let ball = upload(&mut frames.renderer, &character_sphere(&flat([0.8; 3], 5.5)));
        let view = ViewData::new(camera(frames.size(), Vec3::ZERO, 2.0));
        frames.frame(&gpu, view, &[instance(0, ball, translate(0.0, 0.0, 0.5))], &[]);
        let report = frames.report();
        for name in ["msaa color", "depth"] {
            let texture = report.textures.iter().find(|texture| texture.name == name).expect(name);
            let desc = texture.transient.expect("a transient");
            assert_eq!(desc.sample_count, 4, "{id} {name}");
            // Cleared and discarded on tile: memoryless on Metal, lazily allocated on Vulkan.
            assert_eq!(
                desc.usage,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT,
                "{id} {name}"
            );
        }
        let running: Vec<&str> = report.running().map(|pass| pass.name).collect();
        for pass in ["taa", "responsive mask", "scene prepass", "character prepass"] {
            assert!(!running.contains(&pass), "{id} runs {pass}: {running:?}");
        }
        assert!(!frames.renderer.stats().temporal.enabled);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn temporal_aa_runs_at_any_size_scale_and_msaa() {
    let gpu = gpu();
    for msaa in [Msaa::Off, Msaa::X4] {
        let settings = RenderSettings { msaa, temporal: true, ..RenderSettings::default() };
        let mut frames = Frames::new(&gpu, settings, (1, 1));
        let ball = upload(&mut frames.renderer, &character_sphere(&flat([0.8; 3], 5.5)));
        let mut first = true;
        for size in [(1, 1), (17, 1000), (641, 361), (2, 3)] {
            frames.resize(&gpu, size);
            for scale in [1.0, 0.5, 0.25] {
                frames.renderer.set_render_scale(scale);
                let view = ViewData::new(camera(size, Vec3::ZERO, 2.0));
                let changed = [instance(0, ball, translate(0.0, 0.0, 0.5))];
                frames.frame(&gpu, view, if first { &changed } else { &[] }, &[]);
                first = false;
            }
        }
    }
}

/// Draws `count` frames of a still sphere and returns the last, the first a camera cut if `cut`.
fn draw_sphere(
    gpu: &GpuContext,
    frames: &mut Frames,
    ball: MeshHandle,
    count: u32,
    cut: bool,
) -> Vec<[u8; 4]> {
    let view = ViewData::new(camera(frames.size(), Vec3::ZERO, 2.0));
    let mut pixels = Vec::new();
    for frame in 0..count {
        let view = ViewData { cut: cut && frame == 0, ..view };
        pixels = frames.frame(gpu, view, &[instance(0, ball, translate(0.0, 0.0, 0.5))], &[]);
    }
    pixels
}

/// Hot reload (M1 A8) rebuilds the responsive mask's and the resolve's pipelines from an edited
/// `temporal.wgsl`, keeps them when an edit doesn't build, and goes back exactly.
#[test]
#[ignore = "needs a GPU"]
fn edited_temporal_shaders_rebuild_its_pipelines() {
    let gpu = gpu();
    let size = (160, 90);
    let mut frames = Frames::taa(&gpu, size);
    let ball = upload(&mut frames.renderer, &sphere(&flat([0.9, 0.6, 0.3], 5.5)));
    let middle = (size.1 / 2 * size.0 + size.0 / 2) as usize;
    let before = draw_sphere(&gpu, &mut frames, ball, 8, false);

    // The resolve writes blue instead of what it blended.
    let compiled = ShaderSources::compiled_in();
    let edit = |old: &str, new: &str| {
        let text = compiled.get(ShaderFile::Temporal);
        assert_eq!(text.matches(old).count(), 1, "{old:?}");
        let mut sources = compiled.clone();
        sources.set(ShaderFile::Temporal, text.replace(old, new));
        sources
    };
    let blue = edit("vec4<f32>(expand(blended), ", "vec4<f32>(vec3<f32>(0.0, 0.0, 1.0), ");
    let update = frames.renderer.set_shader_sources(&blue).expect("a valid edit");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 2 });
    let tinted = draw_sphere(&gpu, &mut frames, ball, 8, false)[middle];
    assert!(
        tinted[2] > tinted[0].saturating_add(100) && tinted[2] > tinted[1].saturating_add(100),
        "{tinted:?}"
    );

    // A resolve renamed validates but builds no pipeline: the renderer keeps what it has.
    let renamed = edit("fn cs_temporal_resolve(", "fn cs_renamed(");
    let error = frames.renderer.set_shader_sources(&renamed).unwrap_err();
    assert!(matches!(error, ShaderError::Pipeline { .. }), "{error}");
    assert_eq!(draw_sphere(&gpu, &mut frames, ball, 8, false)[middle], tinted);

    // Back to the compiled-in resolve: after a cut on the same jitter phase, the frames repeat
    // the first ones exactly.
    let update = frames.renderer.set_shader_sources(&compiled).expect("compiled in");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 2 });
    assert!(draw_sphere(&gpu, &mut frames, ball, 8, true) == before, "the frames differ from the first ones");
}

/// The light in `pixels`, summed in linear RGB.
fn total_light(pixels: &[[u8; 4]]) -> f32 {
    pixels.iter().map(|pixel| (0..3).map(|c| srgb_to_linear(pixel[c])).sum::<f32>()).sum()
}

/// The night sky under temporal AA (M1 W2's stars and moon, which write no velocity and so
/// reproject from depth at infinity). Held still, the stars keep their light from frame to frame
/// under the jitter: no shimmer. With the camera turning, the stars and the moon's disc keep
/// their light and their place against 4× MSAA's: no smears and no trails.
#[test]
#[ignore = "needs a GPU"]
fn stars_and_the_moon_hold_still_and_follow_a_turning_camera() {
    let gpu = gpu();
    let size = (320, 180);
    let fov = 60f32.to_radians();
    let forward = Vec3::new(0.2, 0.6, -1.0).normalize();
    // The moon in view, right of the center.
    let moon = Quat::from_rotation_y(-0.25) * forward;
    let night = (
        Celestial {
            moon: DirectionalLight { direction: -moon, color: Vec3::ONE, intensity: 0.1 },
            ..Celestial::default()
        },
        SkyLayers {
            stars: 1.0,
            moon: SkyDisc { color: Vec3::ONE, intensity: 2.0, glow: Vec3::ZERO, glow_radius: 0.0 },
            ..SkyLayers::default()
        },
    );
    let dark =
        SkyGradient { zenith: Vec3::splat(0.002), horizon: Vec3::splat(0.004), ground: Vec3::splat(0.002) };
    let aspect = size.0 as f32 / size.1 as f32;
    let view = |yaw: f32| {
        let direction = Quat::from_rotation_y(yaw) * forward;
        ViewData::new(CameraData::look_at(Vec3::ZERO, direction, Vec3::Y, fov, aspect, 0.05))
    };
    let mut taa = Frames::taa(&gpu, size);
    let mut msaa = Frames::msaa(&gpu, size);
    for frames in [&mut taa, &mut msaa] {
        frames.sky = dark;
        frames.layers = Some(night);
    }

    // How far `taa` is from `msaa` moved `dx` pixels right: the summed square difference of their
    // linear light, inside a 10-pixel margin. Smallest at the shift that lines their stars up.
    let mismatch = |taa: &[[u8; 4]], msaa: &[[u8; 4]], dx: i32| {
        let light = |pixel: &[u8; 4]| (0..3).map(|c| srgb_to_linear(pixel[c])).sum::<f32>();
        let (width, height) = (size.0 as i32, size.1 as i32);
        let mut sum = 0.0f32;
        for y in 10..height - 10 {
            for x in 10..width - 10 {
                let d = light(&taa[(y * width + x) as usize]) - light(&msaa[(y * width + x + dx) as usize]);
                sum += d * d;
            }
        }
        sum
    };

    // Held still: over two jitter cycles after convergence, the frame's light holds within 1%
    // and pixels change by a step or two at most, at the same light as 4× MSAA's.
    let still_msaa = msaa.frame(&gpu, view(0.0), &[], &[]);
    let reference = total_light(&still_msaa);
    let mut last = Vec::new();
    let (mut lowest, mut highest, mut worst) = (f32::MAX, 0.0f32, Difference { mean: 0.0, p99: 0, over: 0 });
    for frame in 0..40 {
        let pixels = taa.frame(&gpu, view(0.0), &[], &[]);
        if frame >= 24 {
            let light = total_light(&pixels);
            (lowest, highest) = (lowest.min(light), highest.max(light));
            let change = difference(&pixels, &last, 4);
            if change.over > worst.over || change.mean > worst.mean {
                worst = change;
            }
        }
        last = pixels;
    }
    dump("stars_still_taa", size, &last);
    let still_mismatch = mismatch(&last, &still_msaa, 0);
    eprintln!(
        "still: light {lowest:.1}–{highest:.1} against 4× MSAA's {reference:.1}, mismatch {still_mismatch:.0}; \
         frame to frame at worst {worst:?}"
    );
    assert!(reference > 10.0, "a sky of stars and a moon: {reference}");
    assert!(highest / lowest < 1.01, "the light flickers: {lowest} to {highest}");
    assert!((lowest / reference - 1.0).abs() < 0.05, "{lowest} against {reference}");
    assert!(worst.mean < 0.1 && worst.over < 40, "{worst:?}");

    // Turning by 1.3 pixels a frame: the sky's light matches 4× MSAA's within 5%, and its stars
    // sit where MSAA's do, closer than a third of what misplacing them by a pixel costs: no lag,
    // and no smear along the motion (the history's resampling softens them a little: three times
    // the still mismatch on an M5). Without the sky's reprojection the stars lose two thirds of
    // their light.
    let pixel = fov / size.1 as f32;
    let (mut taa_pixels, mut msaa_pixels) = (Vec::new(), Vec::new());
    for frame in 0..24 {
        let at = view(-(frame as f32) * 1.3 * pixel);
        taa_pixels = taa.frame(&gpu, at, &[], &[]);
        msaa_pixels = msaa.frame(&gpu, at, &[], &[]);
    }
    dump("stars_turning_taa", size, &taa_pixels);
    dump("stars_turning_msaa", size, &msaa_pixels);
    let (taa_light, msaa_light) = (total_light(&taa_pixels), total_light(&msaa_pixels));
    let [left, here, right] = [-1, 0, 1].map(|dx| mismatch(&taa_pixels, &msaa_pixels, dx));
    eprintln!(
        "turning: light {taa_light:.1} against 4× MSAA's {msaa_light:.1}; mismatch {here:.0} ({left:.0} a pixel \
         left, {right:.0} right)"
    );
    assert!((taa_light / msaa_light - 1.0).abs() < 0.05, "{taa_light} against {msaa_light}");
    assert!(here * 3.0 < left.min(right), "the stars lag or smear: {left}, {here}, {right}");
}
