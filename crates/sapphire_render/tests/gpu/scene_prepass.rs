//! The scene prepass's pipelines are built only where the pass can run: a tier that reads it builds
//! them with what's drawn, Mobile-Low, which doesn't, builds none, and debug lines, which read it when
//! they come, build what's drawn so far on their first frame. Run with
//! `cargo test -p sapphire_render --test gpu scene_prepass:: -- --ignored --test-threads=1`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::material::{AlphaMode, AnimeCharacterMaterial, Material, Outline};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_core::debug_draw::{Color, DebugLine, Depth};
use sapphire_render::debug_view::DebugView;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::{
    CameraData, DirectionalLight, FramePacketRing, InstanceId, MeshInstance, RenderSettings, Renderer,
    SCENE_PREPASS, ViewData,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (160, 90);
const FOV: f32 = 38.0 * PI / 180.0;
const EYE: Vec3 = Vec3::new(0.0, 0.0, 6.0);

/// A unit sphere in `document`.
fn sphere(document: &AnimeCharacterMaterial) -> Mesh {
    const RINGS: u32 = 8;
    const SEGMENTS: u32 = 12;
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: n.to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [0.5, 0.5],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, c, b, c, d, b]);
        }
    }
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(document, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", material)];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// An outlined material with no maps, cut out by `alpha`, on one side or both.
fn outlined(alpha: AlphaMode, double_sided: bool) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        alpha,
        double_sided,
        outline: Outline { width: 4.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

const MASK: AlphaMode = AlphaMode::Mask { cutoff: 0.5 };

struct Frames {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    added: Vec<MeshInstance>,
    next_id: u64,
}

impl Frames {
    fn new(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut graph = GraphExecutor::new();
        graph.set_report(true);
        Self { gpu, renderer, graph, packets: FramePacketRing::new(), target, added: Vec::new(), next_id: 0 }
    }

    /// Everything the scene prepass has a pipeline of its own for, but foliage (`cutouts.rs` has that),
    /// off to the sides and leaving the row at y 0 clear beside the sphere in the middle.
    fn add_every_kind(&mut self) {
        self.add(&outlined(AlphaMode::Opaque, false), Vec3::ZERO);
        self.add(&outlined(MASK, false), Vec3::new(-2.5, 1.6, 0.0));
        self.add(&outlined(AlphaMode::Opaque, true), Vec3::new(2.5, 1.6, 0.0));
        self.add(&outlined(MASK, true), Vec3::new(-2.5, -1.6, 0.0));
    }

    fn add(&mut self, document: &AnimeCharacterMaterial, at: Vec3) {
        let bytes = sphere(document).to_cooked().expect("cook");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        let materials = self.renderer.upload_embedded_materials(mesh).expect("materials");
        let mesh = self.renderer.upload_mesh(mesh, &materials).expect("upload");
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::from_translation(at) });
    }

    /// Draws a frame with `lines`, checks it raised no validation error, and reads it back.
    fn render(&mut self, lines: &[DebugLine]) -> Vec<u8> {
        let packet = self.packets.begin();
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        packet.views.push(ViewData::new(CameraData::look_at(EYE, Vec3::ZERO, Vec3::Y, FOV, aspect, 0.05)));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.instances.changed.append(&mut self.added);
        packet.debug.lines.extend_from_slice(lines);
        packet.debug.stats.lines = lines.len() as u32;
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        self.read_back()
    }

    fn read_back(&self) -> Vec<u8> {
        let padded = (SIZE.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIZE.1),
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
                    rows_per_image: Some(SIZE.1),
                },
            },
            self.target.size(),
        );
        self.gpu.queue().submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = buffer.get_mapped_range(..).expect("mapped");
        mapped.chunks_exact(padded as usize).flat_map(|row| row[..(SIZE.0 * 4) as usize].to_vec()).collect()
    }

    fn pipelines(&self) -> usize {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.render_pipelines.num_kept_from_user
    }

    /// Whether the last frame's graph ran the scene prepass.
    fn prepass_ran(&self) -> bool {
        let report = self.graph.report().expect("the report is on");
        report.passes.iter().any(|pass| pass.name == SCENE_PREPASS && pass.order.is_some())
    }
}

fn tier(id: TierId) -> RenderSettings {
    TierTable::shipped().get(id).render_settings()
}

/// What the scene prepass has for [`Frames::add_every_kind`]: the plain pipeline and the hulls', the
/// masked pair, and the two that cull nothing.
const SCENE_PREPASS_PIPELINES: usize = 6;

/// The bug: Mobile-Low built these for a pass its frame graph culls. It pays nothing now, and what it
/// draws is what it drew, byte for byte.
#[test]
#[ignore = "needs a GPU"]
fn mobile_low_builds_none_of_a_pass_it_never_runs() {
    let mut without = Frames::new(tier(TierId::MobileLow));
    without.add_every_kind();
    let pixels = without.render(&[]);
    assert!(!without.prepass_ran(), "the graph culls it");

    let mut kept = Frames::new(tier(TierId::MobileLow));
    kept.renderer.keep_scene_prepass();
    kept.add_every_kind();
    let kept_pixels = kept.render(&[]);
    assert!(!kept.prepass_ran(), "the graph culls it, built or not");

    assert_eq!(kept.pipelines() - without.pipelines(), SCENE_PREPASS_PIPELINES);
    assert!(pixels == kept_pixels, "the same picture either way");
}

/// Every other tier reads the pass, and has all it draws through from the first frame: nothing to build
/// by keeping it.
#[test]
#[ignore = "needs a GPU"]
fn every_tier_that_reads_the_pass_builds_it() {
    for id in [TierId::DesktopHigh, TierId::DesktopLow, TierId::MobileHigh] {
        let mut frames = Frames::new(tier(id));
        frames.add_every_kind();
        frames.render(&[]);
        assert!(frames.prepass_ran(), "{id}");
        let built = frames.pipelines();
        frames.renderer.keep_scene_prepass();
        assert_eq!(frames.pipelines(), built, "{id}");
    }
}

/// Debug lines read the pass: their first frame on Mobile-Low builds it for all that's drawn and tests
/// lines against it, what's drawn later has its share built with it, and a frame with no lines goes
/// back to culling it.
#[test]
#[ignore = "needs a GPU"]
fn debug_lines_on_mobile_low_build_the_pass_and_test_against_it() {
    let mut frames = Frames::new(tier(TierId::MobileLow));
    frames.add_every_kind();
    let before = frames.render(&[]);
    let built = frames.pipelines();
    assert!(!frames.prepass_ran());

    // Across the frame, behind the sphere in the middle.
    let color = Color::rgb(255, 40, 200);
    let (from, to) = (Vec3::new(-20.0, 0.0, -3.0), Vec3::new(20.0, 0.0, -3.0));
    let lines = [DebugLine { from, to, color, depth: Depth::Tested }];
    let drawn = frames.render(&lines);
    assert!(frames.prepass_ran());
    // The debug pass's own pipeline, and the scene prepass's.
    assert_eq!(frames.pipelines(), built + 1 + SCENE_PREPASS_PIPELINES);
    let pixel = |frame: &[u8], x: u32| {
        let at = ((SIZE.1 / 2 * SIZE.0 + x) * 4) as usize;
        [frame[at], frame[at + 1], frame[at + 2]]
    };
    let row_differs = |x: u32| {
        (SIZE.1 / 2 - 2..=SIZE.1 / 2 + 2).any(|y| {
            let at = ((y * SIZE.0 + x) * 4) as usize;
            drawn[at..at + 3] != before[at..at + 3]
        })
    };
    assert_eq!(pixel(&drawn, SIZE.0 / 2), pixel(&before, SIZE.0 / 2), "the sphere hides it");
    assert!(row_differs(SIZE.0 / 8), "beside the sphere it shows");

    let built = frames.pipelines();
    frames.renderer.keep_scene_prepass();
    assert_eq!(frames.pipelines(), built, "kept once");

    frames.render(&[]);
    assert!(!frames.prepass_ran(), "no lines, no pass");
    assert_eq!(frames.pipelines(), built, "and what it built stays, for the next lines");
}

/// A kind first drawn after debug lines came builds its share of the pass with itself: it ends up with
/// just what a renderer that kept the pass from the start has, and doesn't panic for want of any.
#[test]
#[ignore = "needs a GPU"]
fn what_is_drawn_after_debug_lines_builds_its_share() {
    let (from, to) = (Vec3::new(-20.0, 0.0, -3.0), Vec3::new(20.0, 0.0, -3.0));
    let lines = [DebugLine { from, to, color: Color::rgb(255, 40, 200), depth: Depth::Tested }];
    let later = [
        (outlined(MASK, false), Vec3::new(-2.5, 1.6, 0.0)),
        (outlined(AlphaMode::Opaque, true), Vec3::new(2.5, 1.6, 0.0)),
        (outlined(MASK, true), Vec3::new(-2.5, -1.6, 0.0)),
    ];

    let mut frames = Frames::new(tier(TierId::MobileLow));
    frames.add(&outlined(AlphaMode::Opaque, false), Vec3::ZERO);
    frames.render(&lines);
    for (document, at) in &later {
        frames.add(document, *at);
    }
    let pixels = frames.render(&lines);
    assert!(frames.prepass_ran());

    let mut kept = Frames::new(tier(TierId::MobileLow));
    kept.renderer.keep_scene_prepass();
    kept.add_every_kind();
    let kept_pixels = kept.render(&lines);
    assert_eq!(frames.pipelines(), kept.pipelines());
    assert!(pixels == kept_pixels, "the same picture either way");
}

/// A debug view reads the character prepass alone: every one of them runs on a tier with no scene
/// prepass, and none builds it.
#[test]
#[ignore = "needs a GPU"]
fn debug_views_read_no_scene_prepass() {
    let mut frames = Frames::new(tier(TierId::MobileLow));
    frames.add_every_kind();
    frames.render(&[]);
    let character_pipelines = frames.renderer.stats().character_pipelines;
    for view in DebugView::ALL {
        frames.renderer.set_debug_view(view);
        frames.render(&[]);
        assert!(!frames.prepass_ran(), "{view:?}");
        assert_eq!(frames.renderer.stats().character_pipelines, character_pipelines, "{view:?}");
    }
}
