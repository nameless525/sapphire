//! Proximity fade tests that read the prepasses' targets back (M1 C7): every pass that draws a
//! fading character keeps the pixels the main pass keeps. They need a GPU: run them with
//! `cargo test -p sapphire_render --tests -- --ignored`.

use glam::{Mat4, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, Material, Outline};
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
        packet.post = PostParams {
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
        };
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
    // Single-sampled, under temporal AA: the dither turns with the jitter, the same in every
    // pass of a frame. The main pass's depth shows what it kept.
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
