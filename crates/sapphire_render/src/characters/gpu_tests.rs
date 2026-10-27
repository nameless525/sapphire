//! Character tests that need a GPU: proximity fade tests that read the prepasses' targets back
//! (M1 C7), where every pass that draws a fading character keeps the pixels the main pass keeps;
//! and a character drawn as one instance per part, whose parts share its entry and so its
//! identity in the character prepass (M1 review RS-001). Run them with
//! `cargo test -p sapphire_render --tests -- --ignored`.

use glam::{Mat4, Vec3};
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, CharacterSurface, EyeLayer, EyeShading,
    HairHighlight, HairShading, LightSplit, Material, Outline, Rim, ThroughHair,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use crate::graph::{FrameGraph, GraphExecutor, TextureId};
use crate::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MeshHandle, MeshInstance,
    Msaa, PostParams, RenderSettings, Renderer, SkyGradient, ToneCurve, ViewData, Vignette,
};

const SIDE: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

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

/// A lit, outlined ball of radius 0.5 at the origin, its material embedded.
fn ball() -> Mesh {
    let (rings, segments) = (32u32, 64u32);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let n = Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos());
            vertices.push(Vertex {
                position: (n * 0.5).to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [0.0; 2],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
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
    let material = AnimeCharacterMaterial {
        outline: Outline { width: 6.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let slot = MaterialSlot::embedded("ball", Material::from_document(&material, &swizzles).expect("valid"));
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(&vertices, indices, submeshes, vec![slot], PositionFormat::Float32)
}

/// What a frame left in the target and the prepasses, a pixel each.
struct Frame {
    /// The target: black where nothing is drawn.
    color: Vec<[u8; 4]>,
    /// The character prepass's nearest surfaces' IDs (R).
    ids: Vec<u16>,
    /// The character prepass's depth.
    character_depth: Vec<f32>,
    /// The scene prepass's depth.
    scene_depth: Vec<f32>,
    /// The main pass's, when it isn't multisampled.
    main_depth: Option<Vec<f32>>,
}

/// The ball, listed as a character, before a black sky with no ambient light.
struct Scene {
    gpu: GpuContext,
    renderer: Renderer,
    packets: FramePacketRing,
    executor: GraphExecutor,
    target: wgpu::Texture,
    ball: MeshHandle,
    added: bool,
    msaa: bool,
}

impl Scene {
    fn new(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let msaa = settings.msaa != Msaa::Off;
        renderer.readable_depth = !msaa;
        let bytes = ball().to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("valid");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let ball = renderer.upload_mesh(mesh, &materials).expect("upload");
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
            packets: FramePacketRing::new(),
            executor: GraphExecutor::new(),
            target,
            ball,
            added: false,
            msaa,
        }
    }

    /// Draws a frame with the ball at `opacity` and reads everything back.
    fn render(&mut self, opacity: f32) -> Frame {
        let packet = self.packets.begin();
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.66, 1.0, 0.05);
        packet.views.push(ViewData::new(camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.post = passthrough();
        if !self.added {
            let instance = MeshInstance { id: InstanceId(1), mesh: self.ball, transform: Mat4::IDENTITY };
            packet.instances.changed.push(instance);
            self.added = true;
        }
        packet.characters.push(CharacterInstance {
            id: InstanceId(1),
            lighting: CharacterLighting::unstyled(packet.sun),
            face: CharacterFace::default(),
            body: CharacterBody::default(),
            opacity,
        });
        self.renderer.prepare(&self.gpu, self.packets.current());

        let words = |bytes: u32| (SIDE * bytes).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = |row: u32| {
            self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row * SIDE),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (color_row, ids_row, depth_row) = (words(4), words(4), words(4));
        let buffers =
            [buffer(color_row), buffer(ids_row), buffer(depth_row), buffer(depth_row), buffer(depth_row)];
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        let mut reads: Vec<(TextureId, wgpu::TextureAspect, u32, &wgpu::Buffer)> = vec![
            (targets.character_ids, wgpu::TextureAspect::All, ids_row, &buffers[1]),
            (targets.character_depth, wgpu::TextureAspect::DepthOnly, depth_row, &buffers[2]),
            (targets.depth, wgpu::TextureAspect::DepthOnly, depth_row, &buffers[3]),
        ];
        if !self.msaa {
            reads.push((targets.main_depth, wgpu::TextureAspect::DepthOnly, depth_row, &buffers[4]));
        }
        let mut pass = graph.add_pass("read back").side_effect();
        for &(texture, ..) in &reads {
            pass = pass.read(texture);
        }
        pass.record(move |ctx| {
            for (texture, aspect, row, buffer) in reads {
                let texture = ctx.texture(texture);
                ctx.encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo { aspect, ..texture.as_image_copy() },
                    wgpu::TexelCopyBufferInfo {
                        buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row),
                            rows_per_image: Some(SIDE),
                        },
                    },
                    texture.size(),
                );
            }
        });
        self.executor.execute(&self.gpu, graph).expect("a valid graph");
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffers[0],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(color_row),
                    rows_per_image: Some(SIDE),
                },
            },
            self.target.size(),
        );
        self.gpu.queue().submit([encoder.finish()]);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");

        let read = |buffer: &wgpu::Buffer, row: u32, texel: u32| -> Vec<u8> {
            buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
            self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let mapped = buffer.get_mapped_range(..).expect("mapped");
            mapped.chunks_exact(row as usize).flat_map(|r| r[..(SIDE * texel) as usize].to_vec()).collect()
        };
        let depth = |buffer: &wgpu::Buffer| -> Vec<f32> {
            bytemuck::pod_collect_to_vec::<u8, f32>(&read(buffer, depth_row, 4))
        };
        Frame {
            color: read(&buffers[0], color_row, 4).as_chunks::<4>().0.to_vec(),
            // Rg16Uint (M1 C6): R the nearest surface, G the nearest but hair.
            ids: bytemuck::pod_collect_to_vec::<u8, [u16; 2]>(&read(&buffers[1], ids_row, 4))
                .into_iter()
                .map(|[nearest, _]| nearest)
                .collect(),
            character_depth: depth(&buffers[2]),
            scene_depth: depth(&buffers[3]),
            main_depth: (!self.msaa).then(|| depth(&buffers[4])),
        }
    }
}

/// The pixels well inside the ball, whose 5 × 5 block the whole ball's IDs all cover: clear of
/// its silhouette, where single-sampled prepasses and multisampled coverage differ.
fn inside(whole: &Frame) -> Vec<usize> {
    let side = SIDE as usize;
    let covered = |x: usize, y: usize| whole.ids[y * side + x] != 0;
    (2..side - 2)
        .flat_map(|y| (2..side - 2).map(move |x| (x, y)))
        .filter(|&(x, y)| (y - 2..=y + 2).all(|y| (x - 2..=x + 2).all(|x| covered(x, y))))
        .map(|(x, y)| y * side + x)
        .collect()
}

/// Asserts that inside the ball, the character prepass holds the character exactly where the
/// main pass drew it (`drawn`), at the scene prepass's depth, and that the fade kept about
/// `share` of the pixels.
fn assert_coherent(frame: &Frame, inside: &[usize], drawn: impl Fn(usize) -> bool, share: f32) {
    let mut kept = 0;
    for &pixel in inside {
        let listed = frame.ids[pixel] != 0;
        assert_eq!(listed, drawn(pixel), "pixel {pixel}: the character prepass and the main pass agree");
        assert_eq!(listed, frame.character_depth[pixel] > 0.0, "pixel {pixel}: IDs and depth agree");
        assert_eq!(
            frame.scene_depth[pixel].to_bits(),
            frame.character_depth[pixel].to_bits(),
            "pixel {pixel}: the scene prepass cuts the same holes, at the same depth"
        );
        kept += usize::from(listed);
    }
    let kept = kept as f32 / inside.len() as f32;
    assert!((kept - share).abs() < 0.1, "about {share} of the pixels kept: {kept}");
}

#[test]
#[ignore = "needs a GPU"]
fn the_prepasses_keep_a_fading_characters_pixels_where_the_main_pass_does() {
    // Single-sampled, under temporal AA: the dither turns every frame, the same in every pass
    // of a frame. The main pass's depth shows what it kept.
    let mut scene =
        Scene::new(RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() });
    let whole = scene.render(1.0);
    let inside = inside(&whole);
    assert!(inside.len() > 1500, "the ball fills part of the image: {}", inside.len());
    for _ in 0..3 {
        let frame = scene.render(0.5);
        let main = frame.main_depth.as_ref().expect("readable");
        assert_eq!(
            &frame.scene_depth, main,
            "the scene prepass's depth is the main pass's, outlines included"
        );
        assert_coherent(&frame, &inside, |pixel| main[pixel] > 0.0, 0.5);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn under_msaa_the_prepasses_keep_the_pixels_the_main_pass_covers_any_sample_of() {
    // At 0.1 the main pass covers one sample of four at 40% of the pixels, none at the rest:
    // the single-sampled prepasses keep the 40%, not the 10% a whole-pixel dither would.
    let mut scene = Scene::new(RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() });
    let whole = scene.render(1.0);
    let inside = inside(&whole);
    assert!(inside.len() > 1500, "the ball fills part of the image: {}", inside.len());
    let frame = scene.render(0.1);
    let color = &frame.color;
    assert_coherent(&frame, &inside, |pixel| color[pixel][..3].iter().any(|&c| c > 0), 0.4);
}

/// The split character's colors: the eye white, the iris over it and the hair in front of both.
const WHITE: [f32; 3] = [0.9, 0.9, 0.9];
const IRIS: [f32; 3] = [0.1, 0.2, 0.7];
const HAIR: [f32; 3] = [0.1, 0.6, 0.2];

/// A flat-lit material in `color` and `surface`: a one-pixel split, no rim or outline.
fn flat(surface: CharacterSurface, color: [f32; 3], alpha: AlphaMode) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface,
        alpha,
        base_color: [color[0], color[1], color[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// An eye layer that shows through hair at half strength: the eye white opaque, the iris blended
/// over it.
fn eye_layer(layer: EyeLayer) -> AnimeCharacterMaterial {
    let through_hair = Some(ThroughHair { opacity: 0.5, distance: 0.5, ..ThroughHair::default() });
    let eye = CharacterSurface::Eye(EyeShading { layer, through_hair, ..EyeShading::default() });
    if layer.is_overlay() { flat(eye, IRIS, AlphaMode::Blend) } else { flat(eye, WHITE, AlphaMode::Opaque) }
}

fn hair() -> AnimeCharacterMaterial {
    let hair = HairShading {
        mask_map: None,
        highlight: HairHighlight { strength: 0.0, ..HairHighlight::default() },
        ..HairShading::default()
    };
    flat(CharacterSurface::Hair(hair), HAIR, AlphaMode::Opaque)
}

/// A rectangle facing the camera (+Z), drawn with its material.
#[derive(Clone)]
struct Part {
    vertices: [Vertex; 4],
    material: AnimeCharacterMaterial,
}

/// A part at depth `z`, from `min` to `max` in x and y, drawn with `material`.
fn part(min: (f32, f32), max: (f32, f32), z: f32, material: AnimeCharacterMaterial) -> Part {
    let vertices = [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)].map(|(x, y)| Vertex {
        position: [x, y, z],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)],
        color: [255; 3],
        flags: VertexFlags::with_material_id(MaterialId::new(0).expect("a material ID")),
    });
    Part { vertices, material }
}

/// `parts` as one mesh: a submesh each, drawn with its own embedded material, in order.
fn mesh_of(parts: &[Part]) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let (mut vertices, mut indices, mut submeshes, mut slots) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (index, part) in parts.iter().enumerate() {
        let base = vertices.len() as u32;
        let first_index = indices.len() as u32;
        vertices.extend_from_slice(&part.vertices);
        indices.extend([0, 1, 2, 0, 2, 3].map(|corner| base + corner));
        submeshes.push(Submesh { first_index, index_count: 6, material: index as u32 });
        let material = Material::from_document(&part.material, &swizzles).expect("valid");
        slots.push(MaterialSlot::embedded(format!("part {index}"), material));
    }
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// Draws one frame of `instances`, each a mesh of its parts at the origin listed as a character
/// with its body (a key light from the camera's side, a face at rest, whole), before a black sky
/// with no ambient light, and reads the target back.
fn draw(instances: &[(&[Part], CharacterBody)]) -> Vec<[u8; 4]> {
    draw_faded(instances, 1.0)
}

/// [`draw`], every instance listed at `opacity`.
fn draw_faded(instances: &[(&[Part], CharacterBody)], opacity: f32) -> Vec<[u8; 4]> {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
    let mut packets = FramePacketRing::new();
    let packet = packets.begin();
    let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y, 0.66, 1.0, 0.05);
    packet.views.push(ViewData::new(camera));
    packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
    packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
    packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
    packet.post = passthrough();
    for (index, &(parts, body)) in instances.iter().enumerate() {
        let bytes = mesh_of(parts).to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("valid");
        let materials = renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = renderer.upload_mesh(mesh, &materials).expect("upload");
        let id = InstanceId(index as u64);
        packet.instances.changed.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
        packet.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(packet.sun),
            face: CharacterFace::default(),
            body,
            opacity,
        });
    }
    renderer.prepare(&gpu, packets.current());
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
    let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
    let view = target.create_view(&Default::default());
    let mut graph = FrameGraph::new();
    let output = graph.import_texture("target", &view);
    renderer.record(&mut graph, output).expect("a view and a target");
    GraphExecutor::new().execute(&gpu, graph).expect("a valid graph");
    let row = (SIDE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * SIDE),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(SIDE),
            },
        },
        target.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(row as usize)
        .flat_map(|r| r[..SIDE as usize * 4].as_chunks::<4>().0.to_vec())
        .collect()
}

/// How many pixels of `a` and `b` differ.
fn differing(a: &[[u8; 4]], b: &[[u8; 4]]) -> usize {
    a.iter().zip(b).filter(|(a, b)| a != b).count()
}

/// The linear color at the image's center: the middle of the eye.
fn center(pixels: &[[u8; 4]]) -> [f32; 3] {
    let pixel = pixels[(SIDE / 2 * SIDE + SIDE / 2) as usize];
    [0, 1, 2].map(|channel| {
        let c = f32::from(pixel[channel]) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    })
}

#[test]
#[ignore = "needs a GPU"]
fn a_character_drawn_as_one_instance_per_part_draws_as_one_mesh() {
    // An eye white with an iris over it, and hair in front that the eye shows through: drawn as
    // one mesh, or as one instance per part listed with one state and a body that casts, as
    // `CharacterInstance` says to list a character's parts. The iris draws only over its own
    // character's eye, and eyes only through their own character's hair, so the parts must
    // share one identity in the character prepass.
    let body = CharacterBody::capsule(Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.0, 0.5, 0.0), 0.3);
    let white = part((-0.5, -0.5), (0.5, 0.5), 0.0, eye_layer(EyeLayer::Eye));
    let iris = part((-0.2, -0.2), (0.2, 0.2), 0.01, eye_layer(EyeLayer::Iris));
    let bangs = part((-1.0, -1.0), (1.0, 1.0), 0.1, hair());

    let eye = [white.clone(), iris.clone()];
    let merged = draw(&[(&eye, body)]);
    let split = draw(&[(&eye[..1], body), (&eye[1..], body)]);
    let iris_drawn = |pixels: &[[u8; 4]]| center(pixels).iter().zip(IRIS).all(|(a, e)| (a - e).abs() < 0.03);
    assert!(iris_drawn(&merged), "the iris over the eye: {:?}", center(&merged));
    let layers = (differing(&merged, &split), center(&merged), center(&split));

    let whole = [white, iris, bangs];
    let merged = draw(&[(&whole, body)]);
    let split = draw(&[(&whole[..1], body), (&whole[1..2], body), (&whole[2..], body)]);
    let hair_alone = draw(&[(&whole[2..], body)]);
    assert!(differing(&merged, &hair_alone) > 1000, "the eye shows through the hair: {:?}", center(&merged));
    let through_hair = (differing(&merged, &split), center(&merged), center(&split));
    // (pixels that differ, the center merged, the center split)
    assert_eq!((layers.0, through_hair.0), (0, 0), "eye layers {layers:?}, through hair {through_hair:?}");

    // Faded, the parts' dither shares the character's phase too.
    for opacity in [0.25, 0.5, 0.75] {
        let merged = draw_faded(&[(&whole, body)], opacity);
        let split = draw_faded(&[(&whole[..1], body), (&whole[1..2], body), (&whole[2..], body)], opacity);
        assert_eq!(differing(&merged, &split), 0, "faded to {opacity}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn another_characters_iris_never_draws_over_an_eye() {
    // An iris listed with another body, or with the default body (which casts nothing, so its
    // instance is a character of its own), belongs to another character than the eye under it:
    // it never draws there, whatever else their state shares.
    let body = CharacterBody::capsule(Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.0, 0.5, 0.0), 0.3);
    let beside = CharacterBody::capsule(Vec3::new(0.7, -0.5, 0.0), Vec3::new(0.7, 0.5, 0.0), 0.3);
    let white = [part((-0.5, -0.5), (0.5, 0.5), 0.0, eye_layer(EyeLayer::Eye))];
    let iris = [part((-0.2, -0.2), (0.2, 0.2), 0.01, eye_layer(EyeLayer::Iris))];
    let alone = draw(&[(&white, body)]);
    assert_eq!(differing(&alone, &draw(&[(&white, body), (&iris, beside)])), 0, "another body");
    let none = CharacterBody::default();
    assert_eq!(differing(&draw(&[(&white, none)]), &draw(&[(&white, none), (&iris, none)])), 0, "no body");
}
