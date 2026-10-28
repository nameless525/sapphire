//! The renderer's debug views (M1 F5 part 2, the `debug_view` module): what each draws, that
//! they hold under every antialiasing mode, that with the view off nothing of them is built or
//! drawn, and that a view builds its own pipelines alone. They need a GPU: run with `cargo test
//! -p sapphire_render --test gpu debug_view:: -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, BandDiffuse, CharacterRegion, DiffuseMode, EnvironmentMaterial,
    LightSplit, Material, Outline, OutlineCurve, RampDiffuse, Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::debug_view::{
    BAND_TONES, CHARACTER_TONE, LINE_SHADE, NO_LIGHTS, OVERDRAW_SCALE, WIREFRAME_TONES, heat, light_position,
    outline_width_color, overdraw_position, ramp_row_color, region_color,
};
use sapphire_render::graph::{FrameGraph, GraphExecutor, GraphReport};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DebugView, DebugViewStats, DirectionalLight, FramePacketRing, HeadBasis,
    InstanceId, LightKind, LightPath, LocalLight, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings,
    Renderer, ShaderFile, ShaderSources, ShaderUpdate, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (640, 360);
/// The camera's vertical field of view; it sits on +Z, 6 m from the origin, looking down −Z.
const FOV: f32 = 38.0 * PI / 180.0;
const EYE: Vec3 = Vec3::new(0.0, 0.0, 6.0);
/// The ramp the ramp-mode materials name.
const RAMP: &str = "7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d";

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

fn target(gpu: &GpuContext) -> wgpu::Texture {
    gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A unit UV sphere at the origin in the anime character family, fully lit in `rgb`, with no rim
/// or outline; unless opaque, its alpha is a half, which `alpha` blends or tests.
fn sphere(rgb: [f32; 3], alpha: AlphaMode) -> Mesh {
    let material = AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], if alpha == AlphaMode::Opaque { 1.0 } else { 0.5 }],
        alpha,
        light_split: LightSplit { threshold: -1.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    sphere_of(Material::from_document(&material, &swizzles).expect("a valid material"))
}

/// A unit UV sphere at the origin in the environment family, in `rgb`.
fn environment_sphere(rgb: [f32; 3]) -> Mesh {
    let material =
        EnvironmentMaterial { base_color: [rgb[0], rgb[1], rgb[2], 1.0], ..EnvironmentMaterial::default() };
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    sphere_of(Material::from_document(&material, &swizzles).expect("a valid material"))
}

fn sphere_of(material: Material) -> Mesh {
    sphere_in(material, [0; 4])
}

/// A unit UV sphere at the origin drawn with `material`, its quarters (by latitude, top to
/// bottom) in the material IDs `ids`, each with its own vertices.
fn sphere_in(material: Material, ids: [u8; 4]) -> Mesh {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 32;
    const QUARTER: u32 = RINGS / 4;
    let stride = SEGMENTS + 1;
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for (quarter, id) in (0..).zip(ids) {
        let first = vertices.len() as u32;
        let flags = VertexFlags::with_material_id(MaterialId::new(id).expect("a material ID"));
        for ring in quarter * QUARTER..=(quarter + 1) * QUARTER {
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
                    flags,
                });
            }
        }
        for ring in 0..QUARTER {
            for segment in 0..SEGMENTS {
                let a = first + ring * stride + segment;
                let (b, c, d) = (a + stride, a + 1, a + stride + 1);
                indices.extend([a, c, b, c, d, b]);
            }
        }
    }
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// A point facing the camera inside quarter `quarter` ([`sphere_in`]'s) of a sphere at `center`
/// of `radius`, a few pixels in from its edges on screen (the outer quarters' middles are
/// nearly at the silhouette).
fn quarter_point(center: Vec3, radius: f32, quarter: usize) -> Vec3 {
    let theta = [0.8, 1.5, 2.5, 3.2][quarter] * PI / 4.0;
    center + radius * Vec3::new(0.0, theta.cos(), theta.sin())
}

/// An opaque anime character material lit as [`sphere`]'s, in `diffuse` mode over `regions`,
/// with an outline `outline` pixels wide on a 1080-row target (0 for none).
fn character(diffuse: DiffuseMode, regions: Vec<CharacterRegion>, outline: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        light_split: LightSplit { threshold: -1.0, softness: 0.02 },
        diffuse,
        regions,
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: outline, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A 4 × 16 ramp of greys ([`RAMP`]): a row per material ID and half of the day.
fn ramp(gpu: &GpuContext) -> GpuTexture {
    let texels: Vec<u8> =
        (0..16u8).flat_map(|row| [60 + row * 8, 60 + row * 8, 60 + row * 8, 255].repeat(4)).collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 16,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![&texels] }, "ramp").expect("a ramp")
}

/// Post that leaves the scene's colors as they are below about 0.98.
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

fn camera() -> CameraData {
    CameraData::look_at(EYE, Vec3::ZERO, Vec3::Y, FOV, SIZE.0 as f32 / SIZE.1 as f32, 0.05)
}

/// The pixel `world` projects to.
fn pixel_of(world: Vec3) -> (u32, u32) {
    let camera = camera();
    let clip = camera.proj * camera.view * world.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = (ndc.x * 0.5 + 0.5) * SIZE.0 as f32;
    let y = (0.5 - ndc.y * 0.5) * SIZE.1 as f32;
    (x as u32, y as u32)
}

/// Pixels within `fraction` of the radius of the disc a sphere at `center` of `radius` covers on
/// screen (roughly: its silhouette's width at its center's depth).
fn disc(center: Vec3, radius: f32, fraction: f32) -> impl Fn((u32, u32)) -> bool {
    let (cx, cy) = pixel_of(center);
    let edge = pixel_of(center + Vec3::new(radius, 0.0, 0.0)).0;
    let r = (edge - cx) as f32 * fraction;
    move |(x, y)| {
        let (dx, dy) = (x as f32 - cx as f32, y as f32 - cy as f32);
        dx * dx + dy * dy < r * r
    }
}

/// A sphere in the scene.
#[derive(Clone, Copy)]
struct Placed {
    mesh: MeshHandle,
    center: Vec3,
    radius: f32,
    /// Listed as a character, so the character prepass holds it.
    character: bool,
}

/// A renderer drawing spheres against an even grey sky, with the sun behind the camera.
struct Scene {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    /// Added in this order by the first frame.
    placed: Vec<Placed>,
    lights: Vec<LocalLight>,
    /// Time of day's blend from the day ramp rows to the night ones.
    night: f32,
    added: bool,
}

impl Scene {
    fn new(gpu: &GpuContext, settings: RenderSettings) -> Self {
        let renderer = Renderer::new(gpu, FORMAT, settings).expect("renderer");
        let mut graph = GraphExecutor::new();
        graph.set_report(true);
        Self {
            renderer,
            graph,
            packets: FramePacketRing::new(),
            placed: Vec::new(),
            lights: Vec::new(),
            night: 0.0,
            added: false,
        }
    }

    fn upload(&mut self, mesh: &Mesh) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = self.renderer.upload_embedded_materials(mesh).expect("materials");
        self.renderer.upload_mesh(mesh, &materials).expect("upload")
    }

    /// Uploads a sphere ([`sphere_in`]) drawn with `document`, its quarters in the material IDs
    /// `ids`, with `ramp` as the ramp the material may name ([`RAMP`]).
    fn character_sphere(
        &mut self,
        document: &AnimeCharacterMaterial,
        ids: [u8; 4],
        ramp: Option<&GpuTexture>,
    ) -> MeshHandle {
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let material = Material::from_document(document, &swizzles).expect("a valid material");
        let bytes = material.to_cooked().expect("cook");
        let ramp_id: AssetId = RAMP.parse().expect("an asset ID");
        let textures = |id: AssetId| ramp.filter(|_| id == ramp_id);
        let handle =
            self.renderer.upload_material(Material::read(&bytes).expect("read"), textures).expect("material");
        let bytes = sphere_in(material, ids).to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[handle]).expect("upload")
    }

    fn place(&mut self, mesh: MeshHandle, center: Vec3, radius: f32, character: bool) {
        self.placed.push(Placed { mesh, center, radius, character });
    }

    /// Draws one frame showing `view` into `texture`, checks it raised no validation error and
    /// returns its pixels.
    fn render(&mut self, gpu: &GpuContext, texture: &wgpu::Texture, view: DebugView) -> Vec<[u8; 4]> {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera()));
        let key = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sun = key;
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
        packet.ambient = AmbientLight { sky: Vec3::splat(0.5), ground: Vec3::splat(0.5), intensity: 0.12 };
        packet.post = passthrough();
        packet.day_night = self.night;
        packet.lights.extend_from_slice(&self.lights);
        let add = !std::mem::replace(&mut self.added, true);
        for (index, placed) in self.placed.iter().enumerate() {
            let id = InstanceId(index as u64);
            if add {
                let transform =
                    Mat4::from_translation(placed.center) * Mat4::from_scale(Vec3::splat(placed.radius));
                packet.instances.changed.push(MeshInstance { id, mesh: placed.mesh, transform });
            }
            if placed.character {
                packet.characters.push(CharacterInstance {
                    id,
                    lighting: CharacterLighting::unstyled(key),
                    face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
                    body: CharacterBody::default(),
                    opacity: 1.0,
                });
            }
        }

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.set_debug_view(view);
        self.renderer.prepare(gpu, self.packets.current());
        let target_view = texture.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &target_view);
        self.renderer.record(&mut graph, target);
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{view:?}: {error:?}");
        read_back(gpu, texture)
    }

    fn report(&self) -> &GraphReport {
        self.graph.report().expect("the report is on")
    }

    fn stats(&self) -> DebugViewStats {
        self.renderer.stats().debug_view
    }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
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

fn at(pixels: &[[u8; 4]], (x, y): (u32, u32)) -> [u8; 4] {
    pixels[(y * SIZE.0 + x) as usize]
}

/// The views' passes in the last frame's graph that ran.
fn view_passes(report: &GraphReport) -> Vec<&'static str> {
    report
        .passes
        .iter()
        .filter(|pass| pass.name.starts_with("debug view") && pass.order.is_some())
        .map(|pass| pass.name)
        .collect()
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(value: f32) -> u8 {
    let c = value.clamp(0.0, 1.0);
    let encoded = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (encoded * 255.0).round() as u8
}

/// A display color (sRGB-encoded, 0 to 1) as the target's bytes.
fn display(color: [f32; 3]) -> [u8; 3] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// `color` (display) blended at `alpha` over `under`, as the target's sRGB blending does it.
fn blended(under: [u8; 4], color: [f32; 3], alpha: f32) -> [u8; 3] {
    std::array::from_fn(|c| {
        let over = srgb_to_linear(display(color)[c]);
        linear_to_srgb(over * alpha + srgb_to_linear(under[c]) * (1.0 - alpha))
    })
}

fn close(actual: [u8; 4], expected: [u8; 3], tolerance: u8) -> bool {
    (0..3).all(|c| actual[c].abs_diff(expected[c]) <= tolerance)
}

/// The overdraw view's color for `fragments` a pixel.
fn overdraw_color(fragments: u32) -> [u8; 3] {
    if fragments > OVERDRAW_SCALE { [255; 3] } else { display(heat(overdraw_position(fragments))) }
}

/// The fragments the overdraw view's `pixel` shows: the count whose color is nearest.
fn fragments(pixel: [u8; 4]) -> u32 {
    let distance = |count: u32| {
        let color = overdraw_color(count);
        (0..3).map(|c| u32::from(pixel[c].abs_diff(color[c]))).sum::<u32>()
    };
    let count = (1..=OVERDRAW_SCALE + 1).min_by_key(|&count| distance(count)).expect("a scale");
    assert!(distance(count) <= 6, "{pixel:?} is no overdraw color (nearest {count})");
    count
}

/// The center of the legend's cell `cell` of `cells`.
fn legend_cell(cell: u32, cells: u32) -> (u32, u32) {
    let x = 0.02 + (cell as f32 + 0.5) / cells as f32 * 0.33;
    ((x * SIZE.0 as f32) as u32, (0.9525 * SIZE.1 as f32) as u32)
}

/// The antialiasing modes the views draw under: 4× MSAA, none, and temporal AA.
fn antialiasing() -> [(&'static str, RenderSettings); 3] {
    let settings = |msaa, temporal| RenderSettings { msaa, temporal, ..RenderSettings::default() };
    [
        ("msaa", settings(Msaa::X4, false)),
        ("none", settings(Msaa::Off, false)),
        ("taa", settings(Msaa::Off, true)),
    ]
}

#[test]
#[ignore = "needs a GPU"]
fn off_builds_and_draws_nothing_and_leaves_the_frame_as_it_was() {
    let gpu = gpu();
    let texture = target(&gpu);
    let mut scene = Scene::new(&gpu, RenderSettings::default());
    let mesh = scene.upload(&sphere([0.8, 0.2, 0.1], AlphaMode::Opaque));
    scene.place(mesh, Vec3::ZERO, 1.0, true);
    scene.lights.push(LocalLight {
        position: Vec3::new(0.0, 0.0, 1.5),
        color: Vec3::ONE,
        intensity: 1.0,
        range: 2.0,
        kind: LightKind::Point,
    });

    let before = scene.render(&gpu, &texture, DebugView::Off);
    assert_eq!(scene.stats(), DebugViewStats::default(), "nothing built");
    assert!(view_passes(scene.report()).is_empty());

    for view in DebugView::ALL.into_iter().skip(1) {
        let pixels = scene.render(&gpu, &texture, view);
        let stats = scene.stats();
        if stats.unsupported {
            assert_eq!(stats.view, DebugView::Off);
            assert_eq!(pixels, before, "{view:?} can't draw here, so the frame is as it is");
            continue;
        }
        assert_eq!(stats.view, view);
        assert!(stats.pipelines > 0 && stats.copies_made > 0);
        // The main pass replayed (after the wireframe's line indices), then the view drawn over
        // the frame.
        let passes: &[&str] = match view {
            DebugView::Wireframe => &["debug view lines", "debug view replay", "debug view"],
            _ => &["debug view replay", "debug view"],
        };
        assert_eq!(view_passes(scene.report()), passes, "{view:?}");
        assert_ne!(pixels, before, "{view:?} shows");
    }

    let (pipelines, copies_made) = (scene.stats().pipelines, scene.stats().copies_made);
    let after = scene.render(&gpu, &texture, DebugView::Off);
    assert!(view_passes(scene.report()).is_empty(), "off draws nothing");
    assert_eq!(
        scene.stats(),
        DebugViewStats { pipelines, copies_made, ..DebugViewStats::default() },
        "kept for later"
    );
    let differing = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ with the view off again");
}

#[test]
#[ignore = "needs a GPU"]
fn overdraw_counts_the_fragments_each_pixel_shades() {
    let gpu = gpu();
    let texture = target(&gpu);
    // Near A, B behind it and drawn before it, C, transparent, over the sky, and D, alpha-tested
    // with its every fragment below the cutoff, over the sky too.
    let (a, b, c) = (Vec3::new(-1.2, 0.0, 0.0), Vec3::new(-0.6, 0.0, -2.0), Vec3::new(1.4, 0.0, 0.0));
    let d = Vec3::new(0.2, 1.4, 0.0);
    let radius = 0.8;
    // Where the spheres meet the middle row: A alone, A over B, B alone.
    let left_of_a = pixel_of(a + Vec3::new(-0.5, 0.0, 0.62));
    let a_over_b = (pixel_of(a + Vec3::new(0.6, 0.0, 0.5)).0, SIZE.1 / 2);
    let b_alone = (pixel_of(b + Vec3::new(0.4, 0.0, 0.69)).0, SIZE.1 / 2);
    assert!(disc(a, radius, 0.95)(a_over_b) && disc(b, radius, 0.95)(a_over_b));
    assert!(!disc(a, radius, 1.1)(b_alone) && disc(b, radius, 0.9)(b_alone));

    // Every antialiasing mode, and a half-size scene, which the view shows as blocks.
    let scaled = antialiasing().map(|(name, settings)| (name, settings, 1.0));
    let half = ("msaa at half scale", RenderSettings::default(), 0.5);
    for (name, settings, scale) in scaled.into_iter().chain([half]) {
        let mut scene = Scene::new(&gpu, settings);
        let opaque = scene.upload(&sphere([0.8, 0.2, 0.1], AlphaMode::Opaque));
        let transparent = scene.upload(&sphere([0.1, 0.4, 0.9], AlphaMode::Blend));
        let cut_out = scene.upload(&sphere([0.2, 0.8, 0.2], AlphaMode::Mask { cutoff: 0.9 }));
        scene.place(opaque, b, radius, false);
        scene.place(opaque, a, radius, true);
        scene.place(transparent, c, radius, false);
        scene.place(cut_out, d, 0.5, false);
        scene.renderer.set_render_scale(scale);
        // Temporal AA's history doesn't reach the view; two frames show it.
        let mut pixels = scene.render(&gpu, &texture, DebugView::Overdraw);
        if settings.temporal {
            pixels = scene.render(&gpu, &texture, DebugView::Overdraw);
        }
        assert_eq!(scene.stats().scale, OVERDRAW_SCALE);

        let count = |pixel| fragments(at(&pixels, pixel));
        assert_eq!(count((SIZE.0 - 4, 4)), 1, "{name}: the sky alone");
        assert_eq!(count(left_of_a), 1, "{name}: A alone");
        assert_eq!(count(a_over_b), 2, "{name}: B, then A over it");
        assert_eq!(count(b_alone), 1, "{name}: B alone");
        assert_eq!(count(pixel_of(c)), 2, "{name}: the sky, then C blended over it");
        // D's shader runs and discards: its fragments count, and the sky behind them.
        assert_eq!(count(pixel_of(d)), 2, "{name}: D's discarded fragments, then the sky");
        // The legend: one cell per count from 1 to the top, then one for more.
        let cells = OVERDRAW_SCALE + 1;
        for cell in 0..cells {
            let pixel = at(&pixels, legend_cell(cell, cells));
            assert!(close(pixel, overdraw_color(cell + 1), 1), "{name}: legend cell {cell} is {pixel:?}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn light_lists_show_the_lights_each_pixel_walks() {
    let gpu = gpu();
    let texture = target(&gpu);
    // A lit by one light in front of it; B, across the frame, by none.
    let (a, b) = (Vec3::new(-1.5, 0.0, 0.0), Vec3::new(1.5, 0.0, 0.0));
    let light = LocalLight {
        position: a + Vec3::new(0.0, 0.0, 1.0),
        color: Vec3::ONE,
        intensity: 0.5,
        range: 1.0,
        kind: LightKind::Point,
    };
    let scaled = antialiasing().map(|(name, settings)| (name, settings, 1.0));
    let half = ("msaa at half scale", RenderSettings::default(), 0.5);
    for path in [LightPath::Clustered, LightPath::PerObject] {
        for (name, settings, scale) in scaled.into_iter().chain([half]) {
            let settings = RenderSettings { light_path: path, ..settings };
            let mut scene = Scene::new(&gpu, settings);
            scene.renderer.set_render_scale(scale);
            let mesh = scene.upload(&sphere([0.6, 0.6, 0.6], AlphaMode::Opaque));
            scene.place(mesh, a, 0.6, true);
            scene.place(mesh, b, 0.6, false);
            scene.lights.push(light);
            // Under temporal AA the frame converges, and the view blends over it: compare once it
            // holds still.
            let frames = if settings.temporal { 12 } else { 1 };
            let mut frame = Vec::new();
            for _ in 0..frames {
                frame = scene.render(&gpu, &texture, DebugView::Off);
            }
            let pixels = scene.render(&gpu, &texture, DebugView::Lights);
            let capacity = scene.renderer.stats().lights.list_capacity;
            assert_eq!(scene.stats().scale, capacity, "{path:?}");
            if path == LightPath::PerObject {
                assert_eq!(capacity, 4);
            }

            let expect = |pixel, expected: [u8; 3], tolerance, what: &str| {
                let actual = at(&pixels, pixel);
                assert!(
                    close(actual, expected, tolerance),
                    "{path:?} {name}: {what} is {actual:?}, not {expected:?}"
                );
            };
            // Surfaces are their counts' colors on the scale, exactly; the sky dims the frame.
            expect(pixel_of(a), display(heat(light_position(1, capacity))), 1, "A, one light");
            expect(pixel_of(b), display(NO_LIGHTS), 1, "B, none");
            let sky = (SIZE.0 - 4, 4);
            let tolerance = if settings.temporal { 3 } else { 1 };
            expect(sky, blended(at(&frame, sky), [0.0; 3], 0.75), tolerance, "the sky");
            // The legend's ends: none (grey), one, and a full list.
            let cells = capacity + 1;
            for (cell, color) in [
                (0, NO_LIGHTS),
                (1, heat(light_position(1, capacity))),
                (capacity, heat(light_position(capacity, capacity))),
            ] {
                let pixel = at(&pixels, legend_cell(cell, cells));
                assert!(close(pixel, display(color), 1), "{path:?} {name}: legend cell {cell} is {pixel:?}");
            }
        }
    }
}

/// The wireframe's tone of the family called `name`.
fn tone(name: &str) -> [f32; 3] {
    WIREFRAME_TONES.iter().find(|(family, _)| *family == name).expect("a family").1
}

#[test]
#[ignore = "needs a GPU"]
fn the_wireframe_draws_the_edges_in_sight_and_holds_still() {
    let gpu = gpu();
    let texture = target(&gpu);
    // A, in the anime character family, in front of B, in the environment's.
    let (a, b, radius) = (Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.4, 0.0, -2.5), 0.8);
    let inside_a = disc(a, radius, 0.85);
    let b_alone = {
        let (inside_b, near_a) = (disc(b, radius, 0.85), disc(a, radius, 1.15));
        move |pixel| inside_b(pixel) && !near_a(pixel)
    };
    // The top-right corner holds nothing but sky.
    let sky = |(x, y): (u32, u32)| x > SIZE.0 * 3 / 4 && y < SIZE.1 / 4;
    let pixels_where = |test: &dyn Fn((u32, u32)) -> bool| {
        (0..SIZE.1)
            .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
            .filter(|&pixel| test(pixel))
            .collect::<Vec<_>>()
    };
    let (inside_a, b_alone, sky) = (pixels_where(&inside_a), pixels_where(&b_alone), pixels_where(&sky));
    assert!(b_alone.len() > 500);

    for (name, settings) in antialiasing() {
        let mut scene = Scene::new(&gpu, settings);
        let character = scene.upload(&sphere([0.6, 0.6, 0.6], AlphaMode::Opaque));
        let world = scene.upload(&environment_sphere([0.6, 0.6, 0.6]));
        scene.place(character, a, radius, true);
        scene.place(world, b, radius, false);
        let frame = scene.render(&gpu, &texture, DebugView::Off);
        let mut masks = Vec::new();
        for _ in 0..4 {
            let pixels = scene.render(&gpu, &texture, DebugView::Wireframe);
            assert_eq!(scene.stats().view, DebugView::Wireframe, "{name}: every device draws lines");
            let (warm, cool) = (display(tone("characters")), display(tone("environment")));
            let count = |region: &[(u32, u32)], color: [u8; 3]| {
                region.iter().filter(|&&pixel| close(at(&pixels, pixel), color, 1)).count()
            };
            let (a_warm, a_cool) = (count(&inside_a, warm), count(&inside_a, cool));
            let (b_warm, b_cool) = (count(&b_alone, warm), count(&b_alone, cool));
            assert!(a_warm * 5 > inside_a.len() && a_cool == 0, "{name}: A's {a_warm} warm, {a_cool} cool");
            assert!(b_cool * 5 > b_alone.len() && b_warm == 0, "{name}: B's {b_cool} cool, {b_warm} warm");
            // No lines in the sky, which shows dimmed.
            let unexpected = sky
                .iter()
                .filter(|&&pixel| !close(at(&pixels, pixel), blended(at(&frame, pixel), [0.0; 3], 0.8), 2));
            assert_eq!(unexpected.count(), 0, "{name}: the sky has lines");
            // Each pixel's line, if any: 1 a character's, 2 the environment's.
            let mask: Vec<u8> = pixels
                .iter()
                .map(|&pixel| u8::from(close(pixel, warm, 1)) | u8::from(close(pixel, cool, 1)) << 1)
                .collect();
            masks.push(mask);
        }
        // Temporal AA's jitter never moves a line.
        for (index, mask) in masks.iter().enumerate().skip(1) {
            let moved = mask.iter().zip(&masks[0]).filter(|(a, b)| a != b).count();
            eprintln!("{name}: frame {index}: {moved} line pixels moved");
            assert_eq!(moved, 0, "{name}: frame {index}: {moved} line pixels moved");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_views_rebuild_with_their_shader() {
    let gpu = gpu();
    let texture = target(&gpu);
    let mut scene = Scene::new(&gpu, RenderSettings::default());
    let mesh = scene.upload(&sphere([0.8, 0.2, 0.1], AlphaMode::Opaque));
    scene.place(mesh, Vec3::ZERO, 1.0, false);
    let before = scene.render(&gpu, &texture, DebugView::Overdraw);

    // A comment: the overdraw view's pass and the sky's stand-in, from the view's program alone.
    let mut sources = ShaderSources::compiled_in();
    let text = sources.get(ShaderFile::DebugView).to_owned();
    sources.set(ShaderFile::DebugView, format!("{text}\n// edited\n"));
    let update = scene.renderer.set_shader_sources(&sources).expect("a valid edit");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 2 });
    assert_eq!(scene.render(&gpu, &texture, DebugView::Overdraw), before);

    // Every pixel at the top of the scale.
    let top = "let fragments = select(counted, u32(max(legend, 0)) + 1u, legend >= 0);";
    assert!(text.contains(top));
    sources.set(
        ShaderFile::DebugView,
        text.replace(top, "let fragments = DEBUG_OVERDRAW_SCALE + counted * 0u;"),
    );
    scene.renderer.set_shader_sources(&sources).expect("a valid edit");
    let pixels = scene.render(&gpu, &texture, DebugView::Overdraw);
    assert!(close(at(&pixels, (4, 4)), overdraw_color(OVERDRAW_SCALE), 1), "{:?}", at(&pixels, (4, 4)));

    // A frame.wgsl edit rebuilds the copies of the main pass's pipelines with the next frame,
    // which draws as before through them.
    let (copies, made) = (scene.stats().pipelines - 2, scene.stats().copies_made);
    let mut shared = ShaderSources::compiled_in();
    shared.set(ShaderFile::Frame, format!("{}\n// edited\n", shared.get(ShaderFile::Frame)));
    scene.renderer.set_shader_sources(&shared).expect("a valid edit");
    assert_eq!(scene.render(&gpu, &texture, DebugView::Overdraw), before);
    assert_eq!(scene.stats().copies_made, made + copies, "every copy made again");
    // A prepass edit leaves them: they copy the main pass alone.
    let mut prepass = shared.clone();
    prepass.set(ShaderFile::Prepass, format!("{}\n// edited\n", shared.get(ShaderFile::Prepass)));
    scene.renderer.set_shader_sources(&prepass).expect("a valid edit");
    assert_eq!(scene.render(&gpu, &texture, DebugView::Overdraw), before);
    assert_eq!(scene.stats().copies_made, made + copies, "none made again");

    // The material views' copies compile the anime character program's debug entry points, so
    // an edit there makes them again: every pixel shows region 4.
    let center = pixel_of(Vec3::ZERO);
    let pixels = scene.render(&gpu, &texture, DebugView::MaterialIds);
    assert!(close(at(&pixels, center), display(region_color(0)), 1), "{:?}", at(&pixels, center));
    let mut character = prepass.clone();
    let text = character.get(ShaderFile::AnimeCharacter).to_owned();
    let values = "return vec4<f32>(drawn, f32(region), rows, extra);";
    assert!(text.contains(values));
    character
        .set(ShaderFile::AnimeCharacter, text.replace(values, "return vec4<f32>(drawn, 4.0, rows, extra);"));
    let made = scene.stats().copies_made;
    scene.renderer.set_shader_sources(&character).expect("a valid edit");
    let pixels = scene.render(&gpu, &texture, DebugView::MaterialIds);
    assert!(close(at(&pixels, center), display(region_color(4)), 1), "{:?}", at(&pixels, center));
    assert!(scene.stats().copies_made > made, "made again");
}

/// Pixels within `inner` and `outer` (fractions of its radius) of the disc a sphere at `center` of
/// `radius` covers on screen ([`disc`]).
fn ring(center: Vec3, radius: f32, inner: f32, outer: f32) -> Vec<(u32, u32)> {
    let (inside, within) = (disc(center, radius, inner), disc(center, radius, outer));
    (0..SIZE.1)
        .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
        .filter(|&pixel| within(pixel) && !inside(pixel))
        .collect()
}

/// `color` with a share `shade` of its light, as the material views darken lines.
fn shaded(color: [f32; 3], shade: f32) -> [f32; 3] {
    color.map(|channel| channel * shade)
}

#[test]
#[ignore = "needs a GPU"]
fn material_ids_and_ramp_rows_show_what_each_pixel_shades_with() {
    let gpu = gpu();
    let texture = target(&gpu);
    let ramp = ramp(&gpu);
    // A in two bands over six regions, B in ramp mode over four, C in three bands over one; their
    // quarters, top to bottom, in the material IDs below. A's last ID, 7, is past its regions, so
    // it shades with its last, 5; B's second region reads rows of its choosing.
    let radius = 0.8;
    let (a, b, c) = (Vec3::new(-1.7, 0.0, 0.0), Vec3::ZERO, Vec3::new(1.7, 0.0, 0.0));
    let (a_ids, a_regions) = ([0, 5, 2, 7], [0, 5, 2, 5]);
    let b_ids = [0, 1, 2, 3];
    let (b_day, b_night) = ([0, 6, 2, 3], [8, 9, 10, 11]);
    let chosen = CharacterRegion { ramp_row: Some(6), night_ramp_row: Some(9), ..CharacterRegion::default() };
    let mut b_regions = vec![CharacterRegion::default(); 4];
    b_regions[1] = chosen;
    let two = character(DiffuseMode::Bands(BandDiffuse::default()), vec![CharacterRegion::default(); 6], 0.0);
    let map = RAMP.parse().expect("an asset ID");
    let ramped = character(DiffuseMode::Ramp(RampDiffuse { map, mix: 1.0 }), b_regions, 0.0);
    let deep = BandDiffuse { deep_threshold: Some(-0.5) };
    let three = character(DiffuseMode::Bands(deep), vec![CharacterRegion::default()], 0.0);
    // E, in the environment family, in front of B's lower left, hides B there.
    let (e, e_radius) = (Vec3::new(-0.35, -0.35, 1.2), 0.2);
    let sky = (SIZE.0 - 4, 4);

    let scaled = antialiasing().map(|(name, settings)| (name, settings, 1.0));
    let half = ("msaa at half scale", RenderSettings::default(), 0.5);
    for (name, settings, scale) in scaled.into_iter().chain([half]) {
        let mut scene = Scene::new(&gpu, settings);
        scene.renderer.set_render_scale(scale);
        let a_mesh = scene.character_sphere(&two, a_ids, None);
        let b_mesh = scene.character_sphere(&ramped, b_ids, Some(&ramp));
        let c_mesh = scene.character_sphere(&three, [0; 4], None);
        let e_mesh = scene.upload(&environment_sphere([0.6, 0.6, 0.6]));
        scene.place(a_mesh, a, radius, true);
        scene.place(b_mesh, b, radius, true);
        scene.place(c_mesh, c, radius, true);
        scene.place(e_mesh, e, e_radius, false);
        // Under temporal AA the frame converges, and the views dim it: compare once it holds
        // still.
        let frames = if settings.temporal { 12 } else { 1 };
        let mut frame = Vec::new();
        for _ in 0..frames {
            frame = scene.render(&gpu, &texture, DebugView::Off);
        }
        let tolerance = if settings.temporal { 3 } else { 1 };
        let expect = |pixels: &[[u8; 4]], point: Vec3, color: [f32; 3], what: &str| {
            let actual = at(pixels, pixel_of(point));
            assert!(
                close(actual, display(color), 1),
                "{name}: {what} is {actual:?}, not {:?}",
                display(color)
            );
        };
        let dimmed = |pixels: &[[u8; 4]], pixel, what: &str| {
            let (actual, expected) = (at(pixels, pixel), blended(at(&frame, pixel), [0.0; 3], 0.75));
            assert!(close(actual, expected, tolerance), "{name}: {what} is {actual:?}, not {expected:?}");
        };
        let legend = |pixels: &[[u8; 4]], colors: &[[f32; 3]]| {
            for (cell, &color) in (0..).zip(colors) {
                let pixel = at(pixels, legend_cell(cell, colors.len() as u32));
                assert!(close(pixel, display(color), 1), "{name}: legend cell {cell} is {pixel:?}");
            }
        };

        // Each quarter in its region's color.
        let ids = scene.render(&gpu, &texture, DebugView::MaterialIds);
        assert_eq!(scene.stats().scale, 8);
        for quarter in 0..4 {
            let what = |sphere: &str| format!("{sphere}'s quarter {quarter}");
            expect(&ids, quarter_point(a, radius, quarter), region_color(a_regions[quarter]), &what("A"));
            expect(&ids, quarter_point(b, radius, quarter), region_color(b_ids[quarter].into()), &what("B"));
            expect(&ids, quarter_point(c, radius, quarter), region_color(0), &what("C"));
        }
        dimmed(&ids, pixel_of(e), "E, in front of B");
        dimmed(&ids, sky, "the sky");
        legend(&ids, &(0..8).map(region_color).collect::<Vec<_>>());

        // B's rows; A's and C's band modes.
        let rows = scene.render(&gpu, &texture, DebugView::RampRows);
        assert_eq!(scene.stats().scale, 16);
        for (quarter, row) in (0..).zip(b_day) {
            let what = |sphere: &str| format!("{sphere}'s quarter {quarter}");
            expect(&rows, quarter_point(a, radius, quarter), BAND_TONES[0], &what("A"));
            expect(&rows, quarter_point(b, radius, quarter), ramp_row_color(row), &what("B"));
            expect(&rows, quarter_point(c, radius, quarter), BAND_TONES[1], &what("C"));
        }
        dimmed(&rows, pixel_of(e), "E, in front of B");
        dimmed(&rows, sky, "the sky");
        let mut cells: Vec<[f32; 3]> = (0..16).map(ramp_row_color).collect();
        cells.extend(BAND_TONES);
        legend(&rows, &cells);

        // At night B reads its night rows; at dusk, half of each run of pixels across, in
        // stripes.
        scene.night = 1.0;
        let night = scene.render(&gpu, &texture, DebugView::RampRows);
        for (quarter, row) in (0..).zip(b_night) {
            let what = format!("B's quarter {quarter} at night");
            expect(&night, quarter_point(b, radius, quarter), ramp_row_color(row), &what);
        }
        expect(&night, quarter_point(a, radius, 1), BAND_TONES[0], "A at night");
        scene.night = 0.5;
        let dusk = scene.render(&gpu, &texture, DebugView::RampRows);
        for quarter in 0..4 {
            let (x, y) = pixel_of(quarter_point(b, radius, quarter));
            let (day, night) =
                (display(ramp_row_color(b_day[quarter])), display(ramp_row_color(b_night[quarter])));
            let block: Vec<[u8; 4]> = (y - 2..y + 2)
                .flat_map(|y| (x - 4..x + 4).map(move |x| (x, y)))
                .map(|pixel| at(&dusk, pixel))
                .collect();
            let days = block.iter().filter(|&&pixel| close(pixel, day, 1)).count();
            let nights = block.iter().filter(|&&pixel| close(pixel, night, 1)).count();
            assert_eq!((days, nights), (16, 16), "{name}: B's quarter {quarter} at dusk");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn outline_widths_show_each_lines_width() {
    let gpu = gpu();
    let texture = target(&gpu);
    // Both outlined 6 pixels wide on a 1080-row target. A is near, where the width curve keeps a
    // material's own width; B far, on the view's axis, where it thins the line.
    let width = 6.0;
    let (a, a_radius) = (Vec3::new(-1.0, 0.0, 3.0), 0.5);
    let (b, b_radius) = (Vec3::new(0.0, 0.0, -9.0), 0.8);
    // The outline shader's effective distance: the depth over the focal length × tan(22.5°).
    let effective = |depth: f32| depth * (FOV / 2.0).tan() / (PI / 8.0).tan();
    let curve = OutlineCurve::default();
    // Their silhouettes' depths (nearer than their centers).
    let silhouette = |center: Vec3, radius: f32| {
        let depth = EYE.z - center.z;
        depth - radius * radius / depth
    };
    assert_eq!(curve.scale(effective(silhouette(a, a_radius))), 1.0);
    let b_width = width * curve.scale(effective(silhouette(b, b_radius)));
    assert!((3.0..4.5).contains(&b_width), "{b_width}");
    let (a_ring, b_ring) = (ring(a, a_radius, 0.95, 1.15), ring(b, b_radius, 0.9, 1.3));
    let circumference = |center: Vec3, radius: f32| {
        let edge = pixel_of(center + Vec3::new(radius, 0.0, 0.0)).0;
        (TAU * (edge - pixel_of(center).0) as f32) as usize
    };
    let (a_around, b_around) = (circumference(a, a_radius), circumference(b, b_radius));

    for (name, settings) in antialiasing() {
        let mut scene = Scene::new(&gpu, settings);
        let outlined =
            character(DiffuseMode::Bands(BandDiffuse::default()), vec![CharacterRegion::default()], width);
        let mesh = scene.character_sphere(&outlined, [0; 4], None);
        scene.place(mesh, a, a_radius, true);
        scene.place(mesh, b, b_radius, true);
        let count = |pixels: &[[u8; 4]], ring: &[(u32, u32)], color: [f32; 3], tolerance| {
            ring.iter().filter(|&&pixel| close(at(pixels, pixel), display(color), tolerance)).count()
        };

        // Lines in their widths' colors, around surfaces in one tone.
        let widths = scene.render(&gpu, &texture, DebugView::OutlineWidths);
        assert_eq!(scene.stats().scale, 8);
        for (center, what) in [(a, "A"), (b, "B")] {
            let actual = at(&widths, pixel_of(center));
            assert!(close(actual, display(CHARACTER_TONE), 1), "{name}: {what}'s surface is {actual:?}");
        }
        // A's line is 2 pixels wide here, B's about 1.3.
        let a_line = count(&widths, &a_ring, outline_width_color(width), 1);
        assert!(a_line > a_around * 3 / 2, "{name}: A's line has {a_line} pixels, around {a_around}");
        let b_line = count(&widths, &b_ring, outline_width_color(b_width), 4);
        assert!(b_line > b_around * 4 / 5, "{name}: B's line has {b_line} pixels, around {b_around}");
        for cell in 0..9 {
            let pixel = at(&widths, legend_cell(cell, 9));
            let color = outline_width_color(cell as f32 + 0.5);
            assert!(close(pixel, display(color), 1), "{name}: legend cell {cell} is {pixel:?}");
        }

        // The material views darken lines: A's in its region's color, and its band mode's tone.
        let ids = scene.render(&gpu, &texture, DebugView::MaterialIds);
        let a_line = count(&ids, &a_ring, shaded(region_color(0), LINE_SHADE), 1);
        assert!(a_line > a_around * 3 / 2, "{name}: A's line has {a_line} pixels in its region's color");
        let rows = scene.render(&gpu, &texture, DebugView::RampRows);
        let a_line = count(&rows, &a_ring, shaded(BAND_TONES[0], LINE_SHADE), 1);
        assert!(a_line > a_around * 3 / 2, "{name}: A's line has {a_line} pixels in its band mode's tone");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_view_builds_its_own_pipelines_and_no_others() {
    let gpu = gpu();
    let texture = target(&gpu);
    // Every pipeline the device holds, as wgpu counts them.
    let held = || {
        let report = gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.render_pipelines.num_kept_from_user + report.hub.compute_pipelines.num_kept_from_user
    };
    let mut scene = Scene::new(&gpu, RenderSettings::default());
    // Outlined characters, opaque, alpha-tested and blended, and an environment sphere.
    for (index, alpha) in
        [AlphaMode::Opaque, AlphaMode::Mask { cutoff: 0.25 }, AlphaMode::Blend].into_iter().enumerate()
    {
        let base_color = [0.6, 0.6, 0.6, if alpha == AlphaMode::Opaque { 1.0 } else { 0.5 }];
        let document = AnimeCharacterMaterial {
            alpha,
            base_color,
            ..character(DiffuseMode::Bands(BandDiffuse::default()), vec![CharacterRegion::default()], 4.0)
        };
        let mesh = scene.character_sphere(&document, [0; 4], None);
        scene.place(mesh, Vec3::new(1.6 * index as f32 - 1.6, 0.0, 0.0), 0.6, true);
    }
    let world = scene.upload(&environment_sphere([0.6, 0.6, 0.6]));
    scene.place(world, Vec3::new(0.0, 1.2, -1.0), 0.5, false);
    scene.render(&gpu, &texture, DebugView::Off);
    scene.render(&gpu, &texture, DebugView::Off);
    let off = held();
    eprintln!("off: {off} pipelines");

    // Each view adds the pipelines it counts as its own and no others: whatever view shows, the
    // main pass's pipelines (the rest) are those it had with the view off.
    let mut before = scene.stats();
    for view in DebugView::ALL.into_iter().skip(1) {
        scene.render(&gpu, &texture, view);
        let stats = scene.stats();
        assert_eq!(stats.view, view);
        let now = held();
        let (added, copies) = (stats.pipelines - before.pipelines, stats.copies_made - before.copies_made);
        eprintln!(
            "{view:?}: {now} pipelines, {} the views': {added} added, {copies} of them copies",
            stats.pipelines
        );
        assert_eq!(now - off, stats.pipelines as usize, "{view:?}");
        // The material views share one set of copies: the first makes them and its pass, the
        // others their passes alone.
        match view {
            DebugView::MaterialIds => assert_eq!((added, copies > 0), (copies + 1, true), "{view:?}"),
            DebugView::RampRows | DebugView::OutlineWidths => assert_eq!((added, copies), (1, 0), "{view:?}"),
            _ => {}
        }
        before = stats;
        let shown = held();
        scene.render(&gpu, &texture, DebugView::Off);
        assert_eq!(held(), shown, "kept with the view off");
    }
}
